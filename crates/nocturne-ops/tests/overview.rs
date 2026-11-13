//! The overview through `App::rpc`, with every source present, with one
//! broken, and with the lot missing.

mod common;

use common::{With, alert, backup, change, fixture, insight, lab, planned_change, rpc};
use nocturne_app::util::now_ms;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

/// The reason recorded for a part that is `null`.
fn why(overview: &Value, section: &str) -> String {
    overview["unavailable"]
        .as_array()
        .expect("an unavailable list")
        .iter()
        .find(|entry| entry["section"] == json!(section))
        .map(|entry| {
            format!(
                "{}: {}",
                entry["reason"].as_str().unwrap_or_default(),
                entry["message"].as_str().unwrap_or_default()
            )
        })
        .unwrap_or_else(|| panic!("no reason recorded for `{section}`"))
}

#[tokio::test]
async fn an_overview_with_every_feature_present() {
    let lab = lab(&["SW1", "SW2", "SW3"], With::all()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    let now = now_ms();

    // SW1 is down, SW2 is up, SW3 has never been polled.
    nocturne_app::inventory::update_state(app, &one, |state| {
        state.reachable = Some(false);
        state.last_checked = Some(now);
    })
    .unwrap();
    nocturne_app::inventory::update_state(app, &two, |state| {
        state.reachable = Some(true);
        state.last_checked = Some(now);
    })
    .unwrap();

    fixture(app).alerts.lock().extend([
        alert("a1", &one, "critical", now - 60_000),
        alert("a2", &two, "low", now - 120_000),
    ]);
    fixture(app).changes.lock().extend([
        change(
            "c1",
            "Shut Gi0/3",
            "applied",
            &one,
            now - 600_000,
            now - 300_000,
        ),
        change(
            "c2",
            "Add VLAN 9",
            "failed",
            &two,
            now - 400_000,
            now - 200_000,
        ),
        planned_change("c3", "Waiting for a human", &one, now - 100_000),
    ]);
    *fixture(app).compliance.lock() = json!({
        "findings": [
            {"policy_id": "p1", "rule_id": "r1", "device_id": one, "status": "fail",
             "detail": "no vty acl"},
            {"policy_id": "p1", "rule_id": "r2", "device_id": two, "status": "fail",
             "detail": "telnet enabled"},
        ],
        "summary": {"devices": 3, "rules": 2, "pass": 4, "fail": 2, "score_pct": 66.7},
        "run_at": now - 3_600_000,
    });
    fixture(app).insights.lock().extend([
        insight("i1", &one, "high", "Duplex mismatch on Gi0/1"),
        insight("i2", &two, "low", "Unused VLAN 40"),
    ]);
    backup(app, &one, "hostname SW1\n", "scheduled");
    backup(app, &one, "hostname SW1\nvlan 10\n", "manual");

    let overview = rpc(app, "overview.get", json!({})).await;

    // Devices: down, up and never-polled are three different things.
    assert_eq!(overview["devices"]["total"], 3);
    assert_eq!(overview["devices"]["up"], 1);
    assert_eq!(overview["devices"]["down"], 1);
    assert_eq!(overview["devices"]["unknown"], 1);
    assert_eq!(overview["devices"]["lab"], 3, "they are simulated devices");

    // Alerts, counted by the summary method when the build has one.
    assert_eq!(overview["alerts"]["open"], 2);
    assert_eq!(overview["alerts"]["from"], "alerts.summary");
    assert_eq!(overview["alerts"]["by_severity"]["critical"], 1);
    assert_eq!(overview["alerts"]["by_severity"]["high"], 0, "a real zero");
    assert_eq!(overview["alerts"]["newest"][0]["id"], "a1");
    assert_eq!(
        overview["alerts"]["newest"][0]["device"], "SW1",
        "the inventory's name, not the one the alert recorded"
    );

    // Changes.
    assert_eq!(overview["changes"]["applied_24h"], 1);
    assert_eq!(overview["changes"]["failed_24h"], 1);
    assert_eq!(overview["changes"]["awaiting_approval"], 1);
    assert_eq!(overview["changes"]["applying"], 0);
    assert_eq!(overview["changes"]["recent"].as_array().unwrap().len(), 3);
    assert_eq!(overview["changes"]["truncated"], false);

    // Backups, from the stored snapshots.
    assert_eq!(overview["backups"]["devices_with_backup"], 1);
    assert_eq!(overview["backups"]["devices_without"], 2);
    assert_eq!(overview["backups"]["changed_24h"], 1);
    assert!(overview["backups"]["oldest_backup_at"].as_i64().is_some());

    // Compliance, and insights.
    assert_eq!(overview["compliance"]["score_pct"], 66.7);
    assert_eq!(overview["compliance"]["failing_devices"], 2);
    assert_eq!(overview["compliance"]["run_at"], json!(now - 3_600_000));
    assert_eq!(overview["insights"]["open"], 2);
    assert_eq!(
        overview["insights"]["top"][0]["id"], "i1",
        "the most severe first"
    );

    // Jobs and incidents are the core's and ours: always there.
    assert_eq!(overview["jobs"]["running"], json!([]));
    assert_eq!(overview["jobs"]["failed_24h"], 0);
    assert_eq!(overview["incidents"]["unresolved"], 0);

    // The activity feed merges the audit trail, alerts and changes.
    rpc(
        app,
        "incidents.create",
        json!({"title": "Something to put in the audit trail", "device_ids": [one]}),
    )
    .await;
    let again = rpc(app, "overview.get", json!({"refresh": true})).await;
    let kinds: Vec<&str> = again["activity"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["kind"].as_str().unwrap())
        .collect();
    for wanted in ["audit", "alert", "change"] {
        assert!(kinds.contains(&wanted), "{wanted} missing from {kinds:?}");
    }
    assert!(
        again["activity"].as_array().unwrap().len() <= 20,
        "the feed is capped"
    );
    assert!(
        again["activity"]
            .as_array()
            .unwrap()
            .windows(2)
            .all(|pair| pair[0]["ts"].as_i64().unwrap() >= pair[1]["ts"].as_i64().unwrap()),
        "newest first"
    );
    assert_eq!(again["incidents"]["unresolved"], 1);
    assert_eq!(again["incidents"]["by_severity"]["sev3"], 1);
    assert!(again["unavailable"].as_array().unwrap().is_empty());
    assert!(again["generated_at"].as_i64().unwrap() >= now);
}

#[tokio::test]
async fn nothing_firing_and_no_alerting_at_all_do_not_look_the_same() {
    // Present, with nothing to report.
    let quiet = lab(&["SW1"], With::all()).await;
    let quiet = rpc(&quiet.app, "overview.get", json!({})).await;
    assert_eq!(quiet["alerts"]["open"], 0);
    assert_eq!(quiet["insights"]["open"], 0);
    assert!(quiet["unavailable"].as_array().unwrap().is_empty());
    assert_eq!(
        quiet["compliance"]["run_at"],
        Value::Null,
        "the stub does not say when it ran, so the time is unknown rather than 0"
    );

    // Not installed.
    let bare = lab(&["SW1"], With::nothing()).await;
    let bare = rpc(&bare.app, "overview.get", json!({})).await;
    assert_eq!(bare["alerts"], Value::Null);
    assert_eq!(bare["insights"], Value::Null);
    assert_ne!(
        bare["alerts"], quiet["alerts"],
        "an absence must not be a zero"
    );
    assert!(why(&bare, "alerts").contains("not_installed"));
    assert!(
        why(&bare, "alerts").contains("`alerts.list` is not part of this build"),
        "the reason names the method that would have answered: {}",
        why(&bare, "alerts")
    );
}

#[tokio::test]
async fn every_feature_absent_leaves_a_usable_overview() {
    let lab = lab(&["SW1", "SW2"], With::nothing()).await;
    let app = &lab.app;
    let overview = rpc(app, "overview.get", json!({})).await;

    // What the core knows is there.
    assert_eq!(overview["devices"]["total"], 2);
    assert_eq!(overview["devices"]["unknown"], 2);
    assert_eq!(overview["jobs"]["failed_24h"], 0);
    assert_eq!(overview["incidents"]["unresolved"], 0);
    assert_eq!(overview["backups"]["devices_without"], 2);
    assert!(overview["activity"].is_array());
    assert!(overview["generated_at"].as_i64().is_some());

    // What needs another feature is null, each with its own reason.
    for section in ["alerts", "changes", "compliance", "insights"] {
        assert_eq!(
            overview[section],
            Value::Null,
            "`{section}` must be null, not zero"
        );
        let reason = why(&overview, section);
        assert!(reason.starts_with("not_installed"), "{section}: {reason}");
        assert!(
            reason.contains("is not part of this build"),
            "{section}: {reason}"
        );
    }
    assert_eq!(overview["unavailable"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn a_broken_feature_is_reported_as_broken_rather_than_as_empty() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    fixture(app).fail("alerts.summary");
    fixture(app).fail("alerts.list");
    fixture(app).fail("changes.list");
    let overview = rpc(app, "overview.get", json!({})).await;
    assert_eq!(overview["alerts"], Value::Null);
    assert_eq!(overview["changes"], Value::Null);
    let reason = why(&overview, "alerts");
    assert!(reason.starts_with("failed"), "{reason}");
    assert!(reason.contains("is broken in this test"), "{reason}");
    assert!(why(&overview, "changes").starts_with("failed"));
    // The parts that work are unaffected.
    assert_eq!(overview["devices"]["total"], 1);
    assert_eq!(overview["compliance"]["failing_devices"], 0);
}

#[tokio::test]
async fn a_feature_whose_answer_cannot_be_read_falls_back_or_says_so() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    fixture(app)
        .alerts
        .lock()
        .push(alert("a1", &one, "medium", now_ms()));
    // A summary this version cannot read falls back to the list, which can
    // be counted, and the result says which it used.
    fixture(app).garble("alerts.summary");
    let overview = rpc(app, "overview.get", json!({})).await;
    assert_eq!(overview["alerts"]["from"], "alerts.list");
    assert_eq!(overview["alerts"]["open"], 1);
    assert!(overview["unavailable"].as_array().unwrap().is_empty());

    // With both unreadable there is nothing to count, and it is not a zero.
    fixture(app).garble("alerts.list");
    let overview = rpc(app, "overview.get", json!({"refresh": true})).await;
    assert_eq!(overview["alerts"], Value::Null);
    assert!(
        why(&overview, "alerts").starts_with("malformed"),
        "{}",
        why(&overview, "alerts")
    );
}

#[tokio::test]
async fn the_overview_is_cached_briefly_and_can_be_refreshed() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let first = rpc(app, "overview.get", json!({})).await;
    let calls = fixture(app).called("alerts.summary");
    assert_eq!(calls, 1);

    // A second call within the cache window reads nothing again.
    let second = rpc(app, "overview.get", json!({})).await;
    assert_eq!(first["generated_at"], second["generated_at"]);
    assert_eq!(fixture(app).called("alerts.summary"), 1);

    // `refresh` goes back to the sources.
    rpc(app, "overview.get", json!({"refresh": true})).await;
    assert_eq!(fixture(app).called("alerts.summary"), 2);

    // The cache window is a setting.
    rpc(
        app,
        "settings.set",
        json!({"section": "ops", "value": {"overview_cache_ms": 0}}),
    )
    .await;
    rpc(app, "overview.get", json!({})).await;
    assert_eq!(fixture(app).called("alerts.summary"), 3);
}
