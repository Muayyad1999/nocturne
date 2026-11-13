//! Incidents through `App::rpc`, in the shape the UI will see.
//!
//! The thing being proved here is the timeline: that it gathers what really
//! happened, says where every line came from, never invents a line, and
//! never loses one without saying so.

mod common;

use common::{With, alert, backup, change, fixture, hours_ago, lab, planned_change, resolved, rpc};
use nocturne_app::util::now_ms;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

/// The entry of a timeline with this id.
fn entry<'a>(incident: &'a Value, id: &str) -> &'a Value {
    incident["timeline"]
        .as_array()
        .expect("a timeline")
        .iter()
        .find(|entry| entry["id"] == json!(id))
        .unwrap_or_else(|| panic!("no timeline entry `{id}`; there are {:?}", ids(incident)))
}

fn ids(incident: &Value) -> Vec<String> {
    incident["timeline"]
        .as_array()
        .expect("a timeline")
        .iter()
        .map(|entry| entry["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

#[tokio::test]
async fn an_incident_opened_from_an_alert_gathers_a_timeline_that_says_where_each_line_came_from() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    let started = hours_ago(2);
    fixture(app).alerts.lock().push(resolved(
        alert("a1", &one, "critical", started),
        started + 300_000,
        started + 900_000,
    ));
    fixture(app).changes.lock().push(change(
        "c1",
        "Shut Gi0/3",
        "applied",
        &one,
        started - 600_000,
        started - 240_000,
    ));
    // A configuration that moved, stored as a backup would be.
    backup(app, &one, "hostname SW1\nvlan 10\n", "scheduled");
    backup(app, &one, "hostname SW1\nvlan 10\nvlan 20\n", "post-change");

    let incident = rpc(
        app,
        "incidents.create",
        json!({"from_alert": "a1", "author": "mo", "impact": "All of HQ offline."}),
    )
    .await;
    let id = incident["id"].as_str().unwrap().to_string();

    // Opened from the alert: its title, its time, its severity, its device.
    assert_eq!(
        incident["title"],
        "SW1 is not answering".replace("SW1", &one)
    );
    assert_eq!(incident["severity"], "sev1", "critical becomes sev1");
    assert_eq!(incident["started_at"], json!(started));
    assert_eq!(incident["state"], "open");
    assert_eq!(incident["device_ids"], json!([one]));
    assert_eq!(incident["alert_ids"], json!(["a1"]));
    assert_eq!(incident["devices"][0]["name"], "SW1");

    // The alert's three moments are on the timeline at the alert's own times.
    let fired = entry(&incident, "alert:a1:fired");
    assert_eq!(fired["ts"], json!(started));
    assert_eq!(fired["kind"], "alert");
    assert_eq!(fired["ref"], json!({"type": "alert", "id": "a1"}));
    assert_eq!(fired["source"]["from"], "alerts.get");
    assert_eq!(
        fired["source"]["evidence"],
        "alert `a1`: started_at, severity, title"
    );
    assert!(
        fired["source"]["gathered_at"].as_i64().unwrap() >= started,
        "the entry says when it was read as well as when it happened"
    );
    assert_eq!(
        entry(&incident, "alert:a1:acknowledged")["ts"],
        json!(started + 300_000)
    );
    assert_eq!(
        entry(&incident, "alert:a1:resolved")["ts"],
        json!(started + 900_000)
    );

    // The configuration movement, with the diff and the snapshot it is from.
    let config_entries: Vec<&Value> = incident["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["source"]["from"] == json!("store.configs"))
        .collect();
    assert_eq!(config_entries.len(), 2, "the baseline and the change");
    assert!(
        config_entries[0]["text"]
            .as_str()
            .unwrap()
            .contains("recorded for the first time: 2 lines (scheduled)"),
        "a first backup is a baseline, not a change: {}",
        config_entries[0]
    );
    let config = config_entries[1];
    assert_eq!(config["kind"], "log");
    assert!(
        config["text"]
            .as_str()
            .unwrap()
            .contains("Running configuration of SW1 changed: +1 −0 lines (post-change)"),
        "{config}"
    );
    assert_eq!(config["ref"]["type"], "config_version");
    assert!(
        config["source"]["evidence"]
            .as_str()
            .unwrap()
            .contains("line counts diffed against version"),
        "{config}"
    );

    // Opening it is itself on the timeline, as a status entry.
    let opened = entry(&incident, &format!("status:{id}:opened"));
    assert_eq!(opened["author"], "mo");
    assert_eq!(opened["source"]["from"], "operator");
    assert!(
        incident["timeline"]
            .as_array()
            .unwrap()
            .windows(2)
            .all(|pair| pair[0]["ts"].as_i64().unwrap() <= pair[1]["ts"].as_i64().unwrap()),
        "the timeline is in time order"
    );
    // Nothing was read that could not be read.
    assert_eq!(incident["gaps"], 0);

    // ---- linking the change pulls its own events in, at its own times.
    let linked = rpc(
        app,
        "incidents.link",
        json!({"id": id, "change_ids": ["c1"], "author": "mo"}),
    )
    .await;
    assert_eq!(linked["linked"], json!(["change `c1`"]));
    assert!(linked["timeline_entries_added"].as_u64().unwrap() >= 2);
    let created = entry(&linked, "change:c1:created");
    assert_eq!(created["ts"], json!(started - 600_000));
    assert_eq!(created["author"], "mo", "the change says who made it");
    assert_eq!(created["source"]["from"], "changes.get");
    let applied = entry(&linked, &format!("change:c1:{one}:applied"));
    assert_eq!(applied["ts"], json!(started - 240_000));
    assert!(
        applied["text"]
            .as_str()
            .unwrap()
            .contains("Applied `Shut Gi0/3` on SW1"),
        "{applied}"
    );
    assert_eq!(
        applied["source"]["evidence"],
        format!("change `c1`: targets[{one}].result.{{state, finished_at}}")
    );
    assert_eq!(entry(&linked, "change:c1:approved")["author"], "zahra");

    // ---- gathering the same facts again changes nothing.
    let before = ids(&linked);
    let again = rpc(app, "incidents.sync", json!({"id": id})).await;
    assert_eq!(again["timeline_entries_added"], 0);
    assert_eq!(ids(&again), before, "no entry is duplicated or lost");
}

#[tokio::test]
async fn a_source_that_cannot_be_read_leaves_a_gap_that_says_what_may_be_missing() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    let started = hours_ago(1);
    fixture(app)
        .alerts
        .lock()
        .push(alert("a1", &one, "high", started));
    fixture(app).changes.lock().push(change(
        "c1",
        "Shut Gi0/3",
        "applied",
        &one,
        started,
        started + 60_000,
    ));

    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "Partial evidence", "started_at": started,
               "device_ids": [one], "alert_ids": ["a1"], "change_ids": ["c1"]}),
    )
    .await;
    let id = incident["id"].as_str().unwrap().to_string();
    assert_eq!(incident["gaps"], 0);

    // The change pipeline breaks; a re-read keeps what it has and marks the
    // hole where the change pipeline's entries would be.
    fixture(app).fail("changes.get");
    let synced = rpc(app, "incidents.sync", json!({"id": id})).await;
    assert_eq!(synced["gaps"], 1);
    let gap = entry(&synced, "gap:changes.get:failed");
    assert_eq!(gap["kind"], "gap");
    assert!(
        gap["text"].as_str().unwrap().contains(
            "The change pipeline could not be read, so this timeline may be missing entries"
        ),
        "{gap}"
    );
    assert!(
        gap["text"]
            .as_str()
            .unwrap()
            .contains("is broken in this test")
    );
    assert_eq!(gap["source"]["from"], "changes.get");
    assert!(
        gap["ts"].as_i64().unwrap() >= started,
        "the gap sits at the moment of the failed read"
    );
    // The alert's entries, which could be read, are still all there.
    assert!(ids(&synced).contains(&"alert:a1:fired".to_string()));

    // Failing again does not pile up a second mark for the same hole.
    let twice = rpc(app, "incidents.sync", json!({"id": id})).await;
    assert_eq!(twice["gaps"], 1);

    // A linked record that has disappeared is its own kind of hole.
    fixture(app).failing.lock().clear();
    fixture(app).changes.lock().clear();
    let gone = rpc(app, "incidents.sync", json!({"id": id})).await;
    let missing = entry(&gone, "gap:change:c1");
    assert!(
        missing["text"]
            .as_str()
            .unwrap()
            .contains("no longer has it"),
        "{missing}"
    );
    assert_eq!(missing["ref"], json!({"type": "change", "id": "c1"}));
}

#[tokio::test]
async fn acknowledge_assign_resolve_and_reopen_each_stamp_a_time_and_a_status_entry() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "Flapping uplink", "severity": "sev3", "device_ids": [lab.id("SW1")]}),
    )
    .await;
    let id = incident["id"].as_str().unwrap().to_string();

    let acked = rpc(
        app,
        "incidents.acknowledge",
        json!({"id": id, "author": "mo", "note": "looking at it"}),
    )
    .await;
    assert!(acked["acked_at"].as_i64().is_some());
    assert_eq!(acked["acked_by"], "mo");
    assert_eq!(acked["assignee"], "mo", "acknowledging takes it");
    assert!(
        entry(&acked, &format!("status:{id}:acknowledged"))["text"]
            .as_str()
            .unwrap()
            .contains("Acknowledged by mo: looking at it")
    );
    // Twice is a conflict, not a second acknowledgement.
    let again = app
        .rpc(
            "incidents.acknowledge",
            json!({"id": id, "author": "zahra"}),
        )
        .await
        .unwrap_err();
    assert_eq!(again.code, "conflict");
    assert!(again.message.contains("already acknowledged by mo"));

    let assigned = rpc(
        app,
        "incidents.assign",
        json!({"id": id, "to": "zahra", "author": "mo"}),
    )
    .await;
    assert_eq!(assigned["assignee"], "zahra");

    // Resolving needs a cause.
    let refused = app
        .rpc("incidents.resolve", json!({"id": id, "root_cause": "  "}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(refused.message.contains("teaches nobody anything"));
    let refused = app
        .rpc("incidents.update", json!({"id": id, "state": "resolved"}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");

    let out = rpc(
        app,
        "incidents.resolve",
        json!({"id": id, "root_cause": "A failing optic in Gi0/24.", "author": "zahra",
               "follow_ups": [{"text": "order a spare optic", "done": false}]}),
    )
    .await;
    assert_eq!(out["state"], "resolved");
    assert!(out["resolved_at"].as_i64().is_some());
    assert!(
        out["mitigated_at"].as_i64().is_some(),
        "resolving implies it"
    );
    assert_eq!(out["root_cause"], "A failing optic in Gi0/24.");

    // Reopening wants a reason, and clears the times.
    let refused = app
        .rpc("incidents.reopen", json!({"id": id, "why": ""}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    let back = rpc(
        app,
        "incidents.reopen",
        json!({"id": id, "why": "the replacement optic failed too", "author": "mo"}),
    )
    .await;
    assert_eq!(back["state"], "open");
    assert_eq!(back["resolved_at"], Value::Null);
    assert_eq!(back["mitigated_at"], Value::Null);
    assert!(back["timeline"].as_array().unwrap().iter().any(|entry| {
        entry["text"]
            .as_str()
            .unwrap_or_default()
            .contains("resolved → open: the replacement optic failed too")
    }));
    assert_eq!(
        back["root_cause"], "A failing optic in Gi0/24.",
        "what was learned is not thrown away"
    );

    // Every one of those is in the audit trail.
    let audit = rpc(
        app,
        "audit.search",
        json!({"action": "incidents.", "limit": 50}),
    )
    .await;
    let actions: Vec<&str> = audit["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["action"].as_str().unwrap())
        .collect();
    for wanted in [
        "incidents.create",
        "incidents.acknowledge",
        "incidents.update",
    ] {
        assert!(actions.contains(&wanted), "{wanted} is not in {actions:?}");
    }
    // And the trail is joined to the incident it is about.
    let first = &audit["entries"][0];
    assert_eq!(first["refs"][0]["type"], "incident");
    assert_eq!(first["refs"][0]["id"], json!(id));
    assert_eq!(first["refs"][0]["resolved"], true);
    assert_eq!(first["refs"][0]["title"], "Flapping uplink");
    assert_eq!(first["device"]["name"], "SW1");
}

#[tokio::test]
async fn a_note_goes_on_the_timeline_at_the_time_the_thing_happened() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "Odd packet loss", "started_at": hours_ago(3)}),
    )
    .await;
    let id = incident["id"].as_str().unwrap().to_string();
    let noted = rpc(
        app,
        "incidents.note",
        json!({"id": id, "text": "Swapped the patch lead at 02:15.", "ts": hours_ago(2),
               "author": "mo"}),
    )
    .await;
    let note = noted["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["kind"] == json!("note"))
        .expect("a note");
    assert_eq!(note["text"], "Swapped the patch lead at 02:15.");
    assert_eq!(note["author"], "mo");
    assert_eq!(note["source"]["from"], "operator");
    assert!(note["ts"].as_i64().unwrap() < now_ms() - 3_600_000);
    assert_eq!(
        app.rpc("incidents.note", json!({"id": id, "text": "   "}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn suggest_links_finds_the_change_four_minutes_before_the_outage() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    let now = now_ms();
    // The configuration moved just now; the incident started a moment later.
    backup(app, &one, "hostname SW1\n", "scheduled");
    backup(app, &one, "hostname SW1\nno ip routing\n", "manual");
    // Another device's movement is not this incident's business.
    backup(app, &two, "hostname SW2\n", "scheduled");
    backup(app, &two, "hostname SW2\nvlan 99\n", "manual");

    let started = now + 1_000;
    let mut unlinked = alert("a9", &one, "critical", started);
    unlinked["title"] = json!("HQ is unreachable");
    fixture(app).alerts.lock().push(unlinked);
    fixture(app)
        .changes
        .lock()
        .push(planned_change("c-planned", "Add VLAN 99", &one, now));

    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "HQ outage", "started_at": started, "device_ids": [one]}),
    )
    .await;
    let id = incident["id"].as_str().unwrap().to_string();

    let suggested = rpc(app, "incidents.suggest_links", json!({"id": id})).await;
    // The window reaches back before the incident started.
    assert!(suggested["window"]["since"].as_i64().unwrap() < started);
    // The alert on this device is offered, and it is not linked yet.
    assert_eq!(suggested["alerts"][0]["id"], "a9");
    assert_eq!(suggested["alerts"][0]["device"], "SW1");
    // The configuration change on this device is offered, with how long
    // before the incident it happened, and marked as not explained by any
    // applied change.
    let configs = suggested["config_changes"].as_array().unwrap();
    assert_eq!(
        configs.len(),
        1,
        "only this incident's devices: {configs:?}"
    );
    assert_eq!(configs[0]["device"], "SW1");
    assert_eq!(configs[0]["lines_added"], 1);
    assert_eq!(configs[0]["explained_by_change"], false);
    assert!(configs[0]["before_start_ms"].as_i64().unwrap() > 0);
    // A planned change that has not run is still offered, since it touches
    // the device inside the window.
    assert_eq!(suggested["changes"][0]["id"], "c-planned");
    assert!(suggested["unavailable"].as_array().unwrap().is_empty());

    // Once linked, it is no longer a suggestion.
    rpc(
        app,
        "incidents.link",
        json!({"id": id, "alert_ids": ["a9"], "change_ids": ["c-planned"]}),
    )
    .await;
    let after = rpc(app, "incidents.suggest_links", json!({"id": id})).await;
    assert!(after["alerts"].as_array().unwrap().is_empty());
    assert!(after["changes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_configuration_change_an_applied_change_explains_is_marked_as_such() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    let now = now_ms();
    backup(app, &one, "hostname SW1\n", "scheduled");
    backup(app, &one, "hostname SW1\nvlan 20\n", "post-change");
    fixture(app).changes.lock().push(change(
        "c1",
        "Add VLAN 20",
        "applied",
        &one,
        now - 120_000,
        now,
    ));
    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "After the VLAN change", "started_at": now, "device_ids": [one]}),
    )
    .await;
    let suggested = rpc(
        app,
        "incidents.suggest_links",
        json!({"id": incident["id"]}),
    )
    .await;
    assert_eq!(
        suggested["config_changes"][0]["explained_by_change"], true,
        "somebody meant this one: {}",
        suggested["config_changes"]
    );
}

#[tokio::test]
async fn incidents_are_listed_counted_and_filtered() {
    let lab = lab(&["SW1", "SW2"], With::nothing()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    let first = rpc(
        app,
        "incidents.create",
        json!({"title": "Core switch down", "severity": "sev1", "device_ids": [one],
               "started_at": hours_ago(5)}),
    )
    .await;
    rpc(
        app,
        "incidents.create",
        json!({"title": "Wireless controller slow", "severity": "sev3", "device_ids": [two],
               "started_at": hours_ago(3)}),
    )
    .await;
    rpc(
        app,
        "incidents.resolve",
        json!({"id": first["id"], "root_cause": "a cut fibre"}),
    )
    .await;

    let all = rpc(app, "incidents.list", json!({})).await;
    assert_eq!(all["incidents"].as_array().unwrap().len(), 2);
    assert_eq!(all["counts"]["total"], 2);
    assert_eq!(all["counts"]["by_state"]["resolved"], 1);
    assert_eq!(all["counts"]["by_state"]["open"], 1);
    assert_eq!(all["counts"]["by_severity"]["sev1"], 1);
    assert_eq!(all["counts"]["by_severity"]["sev2"], 0, "a real zero");
    // Newest first.
    assert_eq!(all["incidents"][0]["title"], "Wireless controller slow");
    assert_eq!(all["incidents"][0]["devices"][0]["name"], "SW2");

    let open = rpc(app, "incidents.list", json!({"state": "unresolved"})).await;
    assert_eq!(open["incidents"].as_array().unwrap().len(), 1);
    let sev1 = rpc(app, "incidents.list", json!({"severity": "sev1"})).await;
    assert_eq!(sev1["incidents"][0]["title"], "Core switch down");
    let by_device = rpc(app, "incidents.list", json!({"device_id": two})).await;
    assert_eq!(by_device["incidents"].as_array().unwrap().len(), 1);
    let by_text = rpc(app, "incidents.list", json!({"query": "fibre"})).await;
    assert_eq!(
        by_text["incidents"].as_array().unwrap().len(),
        1,
        "the cause is searchable"
    );
    assert_eq!(
        app.rpc("incidents.list", json!({"state": "nonsense"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );

    // Paging.
    let page = rpc(app, "incidents.list", json!({"limit": 1})).await;
    assert_eq!(page["incidents"].as_array().unwrap().len(), 1);
    let next = rpc(
        app,
        "incidents.list",
        json!({"limit": 1, "cursor": page["next_cursor"]}),
    )
    .await;
    assert_eq!(next["incidents"][0]["title"], "Core switch down");

    // Delete.
    assert_eq!(
        rpc(app, "incidents.delete", json!({"id": first["id"]})).await["deleted"],
        true
    );
    assert_eq!(
        app.rpc("incidents.get", json!({"id": first["id"]}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn the_export_is_a_review_document_whose_timeline_carries_its_evidence() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    let started = hours_ago(4);
    fixture(app).alerts.lock().push(resolved(
        alert("a1", &one, "critical", started),
        started + 60_000,
        started + 600_000,
    ));
    let incident = rpc(
        app,
        "incidents.create",
        json!({"from_alert": "a1", "title": "HQ offline", "impact": "Nobody could print.",
               "summary": "A cut fibre."}),
    )
    .await;
    let id = incident["id"].as_str().unwrap().to_string();
    rpc(
        app,
        "incidents.resolve",
        json!({"id": id, "root_cause": "A contractor cut the fibre.",
               "follow_ups": [{"text": "bury the duct", "done": false}]}),
    )
    .await;

    let exported = rpc(
        app,
        "incidents.export",
        json!({"id": id, "format": "markdown", "utc_offset_minutes": 0}),
    )
    .await;
    let markdown = exported["content"].as_str().unwrap();
    assert_eq!(exported["title"], "Incident review: HQ offline");
    assert!(
        exported["filename"]
            .as_str()
            .unwrap()
            .starts_with("incident-hq-offline-")
    );
    assert!(markdown.contains("**Severity:** sev1"));
    assert!(markdown.contains("**Devices:** SW1"));
    assert!(markdown.contains("**Times shown in:** UTC"));
    assert!(markdown.contains("## Timeline"));
    assert!(markdown.contains("| Time | Kind | What happened | Who | Evidence |"));
    assert!(
        markdown.contains(r"alerts.get — alert \`a1\`: started_at"),
        "the evidence column names the record and the field: {markdown}"
    );
    assert!(markdown.contains("## Root cause\n\nA contractor cut the fibre."));
    assert!(markdown.contains("| no | bury the duct |"));
    assert!(markdown.contains("- alert a1"));

    // The same document in the other three formats.
    let html = rpc(
        app,
        "incidents.export",
        json!({"id": id, "format": "html", "utc_offset_minutes": 0}),
    )
    .await;
    let html = html["content"].as_str().unwrap();
    assert!(html.starts_with("<!doctype html>") && !html.contains("<script"));
    let value = rpc(
        app,
        "incidents.export",
        json!({"id": id, "format": "json", "utc_offset_minutes": 0}),
    )
    .await;
    let document: Value = serde_json::from_str(value["content"].as_str().unwrap()).unwrap();
    assert_eq!(document["meta"]["Severity"], "sev1");
    let csv = rpc(
        app,
        "incidents.export",
        json!({"id": id, "format": "csv", "utc_offset_minutes": 0}),
    )
    .await;
    assert!(csv["content"].as_str().unwrap().contains("Timeline"));
}

#[tokio::test]
async fn a_device_added_later_brings_its_own_history_with_it() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    let started = hours_ago(1);
    backup(app, &two, "hostname SW2\n", "scheduled");
    backup(app, &two, "hostname SW2\nvlan 30\n", "manual");

    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "Spreading outage", "started_at": started, "device_ids": [one]}),
    )
    .await;
    let id = incident["id"].as_str().unwrap().to_string();
    assert!(
        !ids(&incident)
            .iter()
            .any(|entry| entry.starts_with("config:")),
        "SW1 has no stored configuration"
    );

    let widened = rpc(
        app,
        "incidents.link",
        json!({"id": id, "device_ids": [two]}),
    )
    .await;
    assert!(
        ids(&widened)
            .iter()
            .any(|entry| entry.starts_with(&format!("config:{two}:"))),
        "the new device's configuration history is now on the timeline: {:?}",
        ids(&widened)
    );
    assert_eq!(widened["devices"].as_array().unwrap().len(), 2);
}
