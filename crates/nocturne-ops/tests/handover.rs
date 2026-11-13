//! The shift handover through `App::rpc`: period boundaries, `unplanned`
//! detection, saving, and what it does when a source is missing.

mod common;

use common::{With, alert, backdate, backup, change, fixture, lab, reachability, resolved, rpc};
use nocturne_app::util::now_ms;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

const HOUR: i64 = 3_600_000;

#[tokio::test]
async fn the_handover_is_the_shift_and_only_the_shift() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    let now = now_ms();
    let since = now - 8 * HOUR;
    let until = now;

    // Inside the shift.
    fixture(app).alerts.lock().extend([
        resolved(
            alert("inside-resolved", &one, "high", since + HOUR),
            since + HOUR + 60_000,
            since + 2 * HOUR,
        ),
        alert("inside-open", &two, "critical", since + 3 * HOUR),
        // Before the shift and still firing: not "fired on my shift", but
        // certainly still open.
        alert("before-open", &one, "medium", since - 5 * HOUR),
        // Entirely before the shift.
        resolved(
            alert("before-done", &one, "low", since - 6 * HOUR),
            since - 6 * HOUR,
            since - 5 * HOUR,
        ),
    ]);
    fixture(app).changes.lock().extend([
        change(
            "in-applied",
            "Shut Gi0/3",
            "applied",
            &one,
            since,
            since + HOUR,
        ),
        change(
            "in-failed",
            "Add VLAN 9",
            "failed",
            &two,
            since + HOUR,
            since + 2 * HOUR,
        ),
        // Finished before the shift began.
        change(
            "before",
            "Old work",
            "applied",
            &one,
            since - 3 * HOUR,
            since - 2 * HOUR,
        ),
    ]);
    fixture(app).config_devices.lock().extend([
        json!({"device_id": one, "name": "SW1", "versions": 2,
               "last_error": "authentication failed"}),
        json!({"device_id": two, "name": "SW2", "versions": 1}),
    ]);

    let handover = rpc(
        app,
        "handover.generate",
        json!({"since": since, "until": until}),
    )
    .await;

    assert_eq!(handover["period"], json!({"since": since, "until": until}));
    assert_eq!(handover["alerts"]["fired"], 2, "only the two inside");
    assert_eq!(handover["alerts"]["resolved"], 1);
    let open: Vec<&str> = handover["alerts"]["still_open"]
        .as_array()
        .unwrap()
        .iter()
        .map(|alert| alert["id"].as_str().unwrap())
        .collect();
    assert_eq!(open.len(), 2, "{open:?}");
    assert!(open.contains(&"inside-open") && open.contains(&"before-open"));

    let applied: Vec<&str> = handover["changes"]["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        applied,
        ["in-applied"],
        "the one before the shift is not mine"
    );
    assert_eq!(
        handover["changes"]["failed_or_rolled_back"][0]["id"],
        "in-failed"
    );

    // Backups that failed come from the configuration feature, which is the
    // only thing that knows a backup was attempted and did not work.
    assert_eq!(handover["backups"]["failed"].as_array().unwrap().len(), 1);
    assert_eq!(handover["backups"]["failed"][0]["device"], "SW1");
    assert_eq!(
        handover["backups"]["failed"][0]["error"],
        "authentication failed"
    );
    assert!(handover["unavailable"].as_array().unwrap().is_empty());
    assert_eq!(handover["notes"], "");

    // Two consecutive handovers share no item and leave no gap.
    let earlier = rpc(
        app,
        "handover.generate",
        json!({"since": since - 8 * HOUR, "until": since}),
    )
    .await;
    assert_eq!(earlier["alerts"]["fired"], 2, "the two before the shift");
    let earlier_applied: Vec<&str> = earlier["changes"]["applied"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| change["id"].as_str().unwrap())
        .collect();
    assert_eq!(earlier_applied, ["before"]);

    assert_eq!(
        app.rpc("handover.generate", json!({"since": until, "until": since}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_configuration_change_no_applied_change_covers_is_unplanned() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    let now = now_ms();

    backup(app, &one, "hostname SW1\n", "scheduled");
    backup(app, &one, "hostname SW1\nvlan 20\n", "post-change");
    backup(app, &two, "hostname SW2\n", "scheduled");
    backup(app, &two, "hostname SW2\nno ip routing\n", "manual");
    // A change that covers SW1 right now, and nothing for SW2.
    fixture(app).changes.lock().push(change(
        "c1",
        "Add VLAN 20",
        "applied",
        &one,
        now - 60_000,
        now,
    ));

    let handover = rpc(
        app,
        "handover.generate",
        json!({"since": now - HOUR, "until": now + HOUR}),
    )
    .await;
    let changes = handover["config_changes"].as_array().unwrap();
    // Two devices, two movements each — the baseline and the change — but a
    // first backup is a baseline, so only the real movements can be
    // unplanned.
    let unplanned: Vec<&Value> = changes
        .iter()
        .filter(|change| change["unplanned"] == json!(true) && change["first"] == json!(false))
        .collect();
    assert_eq!(unplanned.len(), 1, "{changes:#?}");
    assert_eq!(unplanned[0]["device"], "SW2");
    assert_eq!(unplanned[0]["lines_added"], 1);
    assert_eq!(unplanned[0]["lines_removed"], 0);
    assert!(
        changes.iter().any(|change| change["device"] == json!("SW1")
            && change["unplanned"] == json!(false)
            && change["checked"] == json!(true)),
        "SW1's movement is explained by the applied change"
    );

    // Without a change feature, nothing can be called unplanned.
    let blind = common::lab(&["SW1"], With::nothing()).await;
    backup(&blind.app, &blind.id("SW1"), "a\n", "scheduled");
    backup(&blind.app, &blind.id("SW1"), "a\nb\n", "manual");
    let handover = rpc(
        &blind.app,
        "handover.generate",
        json!({"since": now - HOUR, "until": now + HOUR}),
    )
    .await;
    let changes = handover["config_changes"].as_array().unwrap();
    assert!(!changes.is_empty(), "the movements are still reported");
    assert!(
        changes
            .iter()
            .all(|change| change["unplanned"] == json!(false) && change["checked"] == json!(false)),
        "with nothing to check against, `unplanned` is not claimed: {changes:#?}"
    );
    assert_eq!(handover["changes"], Value::Null);
}

#[tokio::test]
async fn devices_that_went_down_come_from_the_reachability_samples() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    let now = now_ms();
    let since = now - 60 * 60_000;
    // Both devices were in the inventory before the shift began.
    backdate(app, &one, since - 60_000);
    backdate(app, &two, since - 60_000);
    // SW1: up for 20 minutes, down for 5, up again. SW2: never monitored.
    reachability(
        app,
        &one,
        since,
        &format!("{}{}{}", "1".repeat(20), "0".repeat(5), "1".repeat(34)),
    );
    nocturne_app::inventory::update_state(app, &two, |state| state.reachable = Some(false))
        .unwrap();

    let handover = rpc(
        app,
        "handover.generate",
        json!({"since": since, "until": now}),
    )
    .await;
    let went_down = handover["devices"]["went_down"].as_array().unwrap();
    assert_eq!(went_down.len(), 1, "{went_down:#?}");
    assert_eq!(went_down[0]["name"], "SW1");
    // The time is the sample bucket the store answered with, so it is
    // accurate to the bucket width rather than to the millisecond.
    let at = went_down[0]["at"].as_i64().unwrap();
    assert!(
        (at - (since + 20 * 60_000)).abs() < 1_000,
        "{at} is not around {}",
        since + 20 * 60_000
    );
    assert_eq!(went_down[0]["down_ms"], json!(5 * 60_000));
    assert_eq!(went_down[0]["ongoing"], false);
    assert!(
        went_down[0]["from"]
            .as_str()
            .unwrap()
            .contains("`up` samples of device"),
        "the line says which samples it came from: {}",
        went_down[0]
    );
    // SW2 is unreachable now, but nothing ever measured it, so it cannot be
    // said to have gone down during the shift.
    assert_eq!(
        handover["devices"]["without_reachability_samples"], 1,
        "SW2 has never been measured, so nothing can be said about it"
    );
    assert_eq!(handover["devices"]["unreachable_now"][0]["name"], "SW2");
}

#[tokio::test]
async fn a_removed_device_is_recovered_from_the_audit_trail() {
    let lab = lab(&["SW1", "SW2"], With::nothing()).await;
    let app = &lab.app;
    let two = lab.id("SW2");
    let since = now_ms() - HOUR;
    rpc(app, "devices.delete", json!({"ids": [two]})).await;

    let handover = rpc(
        app,
        "handover.generate",
        json!({"since": since, "until": now_ms() + 1_000}),
    )
    .await;
    let removed = handover["devices"]["removed"].as_array().unwrap();
    assert_eq!(removed.len(), 1);
    assert_eq!(
        removed[0]["name"], "SW2",
        "the name is in the audit message"
    );
    assert_eq!(removed[0]["device_id"], json!(two));
    assert!(
        removed[0]["from"]
            .as_str()
            .unwrap()
            .starts_with("audit entry "),
        "the line says where it came from: {}",
        removed[0]
    );
    // The device still there was added in this process, inside the period.
    assert_eq!(handover["devices"]["added"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn saving_a_handover_regenerates_its_data_and_keeps_only_the_prose() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    let now = now_ms();
    let since = now - 8 * HOUR;
    fixture(app)
        .alerts
        .lock()
        .push(alert("a1", &one, "critical", since + HOUR));

    let generated = rpc(
        app,
        "handover.generate",
        json!({"since": since, "until": now}),
    )
    .await;
    assert_eq!(generated["alerts"]["fired"], 1);

    // A client sends the document back with its data sections rewritten.
    let mut tampered = generated.clone();
    tampered["alerts"] = json!({"fired": 999, "resolved": 999, "still_open": []});
    tampered["devices"] = json!({"added": [], "removed": [], "went_down": [],
                                 "unreachable_now": []});
    let saved = rpc(
        app,
        "handover.save",
        json!({"handover": tampered, "notes": "Watch SW1. The optic is on order.",
               "author": "mo"}),
    )
    .await;
    assert_eq!(
        saved["data"]["alerts"]["fired"], 1,
        "the data sections are the server's own, not the ones sent"
    );
    assert_eq!(saved["notes"], "Watch SW1. The optic is on order.");
    assert_eq!(saved["period"], json!({"since": since, "until": now}));
    assert_eq!(saved["author"], "mo");

    let id = saved["id"].as_str().unwrap().to_string();
    let listed = rpc(app, "handover.list", json!({})).await;
    assert_eq!(listed["handovers"].as_array().unwrap().len(), 1);
    assert_eq!(listed["handovers"][0]["alerts_fired"], 1);
    assert_eq!(listed["handovers"][0]["has_notes"], true);
    assert_eq!(listed["handovers"][0]["id"], json!(id));

    let fetched = rpc(app, "handover.get", json!({"id": id})).await;
    assert_eq!(fetched["notes"], saved["notes"]);
    // The prose is searchable.
    assert_eq!(
        rpc(app, "handover.list", json!({"query": "optic"})).await["handovers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        rpc(app, "handover.list", json!({"query": "nonsense"})).await["handovers"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Editing keeps the id and the author and still regenerates the data.
    let edited = rpc(
        app,
        "handover.save",
        json!({"id": id, "since": since, "until": now, "notes": "Watch SW1 closely."}),
    )
    .await;
    assert_eq!(edited["id"], json!(id));
    assert_eq!(edited["author"], "mo");
    assert_eq!(edited["notes"], "Watch SW1 closely.");
    assert_eq!(edited["data"]["alerts"]["fired"], 1);

    assert_eq!(
        rpc(app, "handover.delete", json!({"id": id})).await["deleted"],
        true
    );
    assert_eq!(
        app.rpc("handover.get", json!({"id": id}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        app.rpc("handover.save", json!({"notes": "no period"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn the_export_reads_in_two_minutes_and_says_what_it_could_not_read() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    let now = now_ms();
    let since = now - 8 * HOUR;
    rpc(
        app,
        "incidents.create",
        json!({"title": "HQ outage", "severity": "sev1", "started_at": since + HOUR,
               "device_ids": [lab.id("SW1")], "assignee": "mo"}),
    )
    .await;

    let exported = rpc(
        app,
        "handover.export",
        json!({"since": since, "until": now, "notes": "Watch SW1.",
               "format": "markdown", "utc_offset_minutes": 0}),
    )
    .await;
    let markdown = exported["content"].as_str().unwrap();
    assert_eq!(exported["title"], "Shift handover");
    assert!(
        exported["filename"]
            .as_str()
            .unwrap()
            .starts_with("handover-")
    );
    assert!(markdown.contains("# Shift handover"));
    assert!(markdown.contains("**Shift:** "));
    assert!(markdown.contains("Watch SW1."));
    assert!(markdown.contains("| open | sev1 | HQ outage |"));
    // The features that are missing say so, each with its reason.
    assert!(
        markdown
            .contains("## Alerts\n\n*Not available — \\`alerts.list\\` is not part of this build*"),
        "{markdown}"
    );
    assert!(markdown.contains("## Changes\n\n*Not available —"));
    assert!(markdown.contains("## Backups\n\n*Not available —"));
    assert!(!markdown.contains("| 0 |"), "no invented zeros: {markdown}");

    // And in the other formats.
    for format in ["html", "csv", "json"] {
        let exported = rpc(
            app,
            "handover.export",
            json!({"since": since, "until": now, "format": format,
                   "utc_offset_minutes": 0}),
        )
        .await;
        let content = exported["content"].as_str().unwrap();
        assert!(!content.is_empty(), "{format} is empty");
        assert!(
            content.to_lowercase().contains("not available") || content.contains("unavailable"),
            "{format} does not say what was missing"
        );
    }
    assert_eq!(
        app.rpc("handover.export", json!({"format": "markdown"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}
