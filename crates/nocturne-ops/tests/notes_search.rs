//! Notes, the global search and the audit view, through `App::rpc`.

mod common;

use common::{With, fixture, lab, rpc};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

#[tokio::test]
async fn a_note_is_filed_against_devices_sites_tags_and_incidents() {
    let lab = lab(&["EDGE-RTR1", "CORE-SW1"], With::nothing()).await;
    let app = &lab.app;
    let edge = lab.id("EDGE-RTR1");
    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "WAN flap", "device_ids": [edge]}),
    )
    .await;

    let note = rpc(
        app,
        "notes.save",
        json!({
            "title": "Failing over the WAN link",
            "body": "Shut the primary uplink on [[EDGE-RTR1]] and watch BGP converge.\n\
                     The far end is [[ACC-SW9]], which we no longer have.",
            "tags": ["#runbook", "WAN"],
            "sites": ["HQ"],
            "incident_ids": [incident["id"].as_str().unwrap()],
            "author": "mo",
        }),
    )
    .await;
    let id = note["id"].as_str().unwrap().to_string();
    assert_eq!(note["tags"], json!(["runbook", "wan"]), "normalised");
    assert_eq!(
        note["device_ids"],
        json!([edge]),
        "the device named in the text is a device the note is about"
    );
    assert_eq!(note["links"][0]["text"], "EDGE-RTR1");
    assert_eq!(note["links"][0]["device_id"], json!(edge));
    assert_eq!(note["links"][1]["text"], "ACC-SW9");
    assert_eq!(
        note["links"][1]["device_id"],
        Value::Null,
        "a link to a device that does not exist is not invented"
    );

    // `notes.get` resolves the links against the inventory as it is now.
    let fetched = rpc(app, "notes.get", json!({"id": id})).await;
    assert_eq!(fetched["links"][0]["resolved"], true);
    assert_eq!(fetched["links"][0]["device"]["name"], "EDGE-RTR1");
    assert_eq!(fetched["links"][1]["resolved"], false);
    assert_eq!(fetched["devices"][0]["name"], "EDGE-RTR1");

    // Every way of finding it again.
    for filter in [
        json!({"device_id": edge}),
        json!({"site": "hq"}),
        json!({"tag": "RUNBOOK"}),
        json!({"incident_id": incident["id"]}),
        json!({"query": "fail over wan"}),
    ] {
        let listed = rpc(app, "notes.list", filter.clone()).await;
        assert_eq!(
            listed["notes"].as_array().unwrap().len(),
            1,
            "not found by {filter}"
        );
    }
    // A text search shows where the words were found.
    let found = rpc(app, "notes.list", json!({"query": "BGP"})).await;
    assert!(
        found["notes"][0]["excerpt"]
            .as_str()
            .unwrap()
            .contains("BGP converge"),
        "{}",
        found["notes"][0]
    );
    assert!(
        found["notes"][0].get("body").is_none(),
        "a list does not carry whole bodies"
    );
    assert!(
        rpc(app, "notes.list", json!({"device_id": lab.id("CORE-SW1")})).await["notes"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Editing keeps the id and re-resolves the links.
    let edited = rpc(
        app,
        "notes.save",
        json!({"id": id, "title": "Failing over the WAN link",
               "body": "Now about [[CORE-SW1]] instead."}),
    )
    .await;
    assert_eq!(edited["id"], json!(id));
    assert_eq!(edited["device_ids"], json!([lab.id("CORE-SW1")]));
    assert_eq!(edited["author"], "mo", "the author is not lost");
    assert!(
        rpc(app, "notes.list", json!({"device_id": edge})).await["notes"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the old link is gone"
    );

    assert_eq!(
        app.rpc("notes.save", json!({"title": "  ", "body": "x"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        rpc(app, "notes.delete", json!({"id": id})).await["deleted"],
        true
    );
    assert_eq!(
        app.rpc("notes.get", json!({"id": id}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn the_search_fans_out_and_reports_a_source_it_could_not_use() {
    let lab = lab(&["CORE-SW1"], With::all()).await;
    let app = &lab.app;
    let one = lab.id("CORE-SW1");
    rpc(
        app,
        "notes.save",
        json!({"title": "Core switch runbook", "body": "The core switch is [[CORE-SW1]]."}),
    )
    .await;
    rpc(
        app,
        "incidents.create",
        json!({"title": "Core switch down", "device_ids": [one]}),
    )
    .await;
    fixture(app).changes.lock().push(common::change(
        "c1",
        "Core switch uplink",
        "applied",
        &one,
        1_000,
        2_000,
    ));
    *fixture(app).config_matches.lock() = json!({
        "matches": [{"device_id": one, "name": "CORE-SW1", "line_no": 4,
                     "line": "hostname CORE-SW1"}],
        "devices_searched": 1, "devices_matched": 1, "truncated": false,
    });
    *fixture(app).ipam.lock() = json!({"prefixes": [], "addresses": [], "vlans": []});
    *fixture(app).logs.lock() = json!({"entries": [{"id": 1, "message": "core switch up"}]});

    // One word that is in the device's name, the note, the incident and the
    // change: every kind answers.
    let found = rpc(app, "search.query", json!({"q": "core"})).await;
    assert_eq!(found["devices"][0]["name"], "CORE-SW1");
    assert_eq!(found["notes"][0]["title"], "Core switch runbook");
    assert_eq!(found["incidents"][0]["title"], "Core switch down");
    assert_eq!(found["changes"][0]["id"], "c1");
    assert_eq!(found["config"][0]["line"], "hostname CORE-SW1");
    assert_eq!(found["logs"][0]["message"], "core switch up");
    assert!(found["addresses"]["prefixes"].is_array());
    assert_eq!(
        found["location"],
        Value::Null,
        "a hostname is not an address, so nothing was asked"
    );
    assert!(found["partial"].as_array().unwrap().is_empty());

    // An address goes to the topology feature as well.
    *fixture(app).locate.lock() = json!({
        "mac": "aa:bb:cc:dd:ee:ff",
        "ip": "10.0.0.9",
        "locations": [{"device_id": one, "device": "CORE-SW1", "interface": "Gi0/5",
                       "kind": "access", "seen_at": 1_000}],
        "best": 0, "how": ["arp", "mac"],
    });
    let located = rpc(app, "search.query", json!({"q": "10.0.0.9"})).await;
    assert_eq!(located["location"]["locations"][0]["interface"], "Gi0/5");
    assert_eq!(fixture(app).called("locate.host"), 1);

    // A failing source is named in `partial`, and the rest still answers.
    fixture(app).fail("config.search");
    let partial = rpc(app, "search.query", json!({"q": "core"})).await;
    assert_eq!(partial["config"], Value::Null);
    assert_eq!(partial["partial"].as_array().unwrap().len(), 1);
    assert_eq!(partial["partial"][0]["section"], "config");
    assert_eq!(partial["partial"][0]["reason"], "failed");
    assert_eq!(partial["notes"][0]["title"], "Core switch runbook");

    assert_eq!(
        app.rpc("search.query", json!({"q": "   "}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_slow_source_is_not_waited_for() {
    let lab = lab(&["CORE-SW1"], With::all()).await;
    let app = &lab.app;
    rpc(
        app,
        "settings.set",
        json!({"section": "ops", "value": {"search_budget_ms": 60}}),
    )
    .await;
    fixture(app).delay("logs.query");
    *fixture(app).config_matches.lock() = json!({"matches": []});

    let started = std::time::Instant::now();
    let found = rpc(app, "search.query", json!({"q": "anything"})).await;
    let waited = started.elapsed();
    assert!(
        waited < std::time::Duration::from_secs(5),
        "the search waited {waited:?} for a source that never answers"
    );
    assert_eq!(found["logs"], Value::Null);
    assert_eq!(found["partial"][0]["section"], "logs");
    assert_eq!(found["partial"][0]["reason"], "timeout");
    assert!(
        found["partial"][0]["message"]
            .as_str()
            .unwrap()
            .contains("did not answer within"),
        "{}",
        found["partial"][0]
    );
    // The sources that were quick are all there.
    assert!(found["config"].is_array());
    assert!(found["devices"].is_array());
}

#[tokio::test]
async fn the_audit_view_filters_and_joins_what_the_trail_mentions() {
    let lab = lab(&["SW1", "SW2"], With::nothing()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "Something happened", "device_ids": [one]}),
    )
    .await;
    let note = rpc(
        app,
        "notes.save",
        json!({"title": "A note", "body": "about [[SW1]]"}),
    )
    .await;
    rpc(app, "notes.delete", json!({"id": note["id"]})).await;

    let all = rpc(app, "audit.search", json!({})).await;
    assert!(all["entries"].as_array().unwrap().len() >= 3);
    assert_eq!(all["has_more"], false);

    // By action, exactly or by prefix.
    let exact = rpc(app, "audit.search", json!({"action": "notes.create"})).await;
    assert_eq!(exact["entries"].as_array().unwrap().len(), 1);
    let prefix = rpc(app, "audit.search", json!({"action": "notes."})).await;
    assert_eq!(prefix["entries"].as_array().unwrap().len(), 2);
    // By device.
    let by_device = rpc(app, "audit.search", json!({"device_id": one})).await;
    assert!(!by_device["entries"].as_array().unwrap().is_empty());
    // SW2 has its own audit entries — the core recorded its creation — but
    // none of this feature's.
    let other = rpc(
        app,
        "audit.search",
        json!({"device_id": lab.id("SW2"), "action": "incidents."}),
    )
    .await;
    assert_eq!(other["entries"], json!([]));
    let created = rpc(app, "audit.search", json!({"device_id": lab.id("SW2")})).await;
    assert_eq!(created["entries"][0]["action"], "device.create");
    // By what the entry refers to.
    let by_ref = rpc(
        app,
        "audit.search",
        json!({"reference": {"type": "incident", "id": incident["id"]}}),
    )
    .await;
    assert_eq!(by_ref["entries"].as_array().unwrap().len(), 1);
    let entry = &by_ref["entries"][0];
    assert_eq!(entry["action"], "incidents.create");
    assert!(
        entry["message"]
            .as_str()
            .unwrap()
            .contains("Opened sev3 incident `Something happened`")
    );
    assert_eq!(entry["device"]["name"], "SW1");
    assert_eq!(entry["refs"][0]["type"], "incident");
    assert_eq!(entry["refs"][0]["title"], "Something happened");
    assert_eq!(entry["refs"][0]["resolved"], true);

    // A reference to a record that has been deleted is still shown, and
    // marked as unresolved rather than silently dropped.
    let deleted = rpc(app, "audit.search", json!({"action": "notes.delete"})).await;
    assert_eq!(deleted["entries"][0]["refs"][0]["type"], "note");
    assert_eq!(deleted["entries"][0]["refs"][0]["resolved"], false);
    assert_eq!(deleted["entries"][0]["refs"][0]["title"], Value::Null);

    // Paging.
    let page = rpc(app, "audit.search", json!({"limit": 1})).await;
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["has_more"], true);
    let next = rpc(
        app,
        "audit.search",
        json!({"limit": 1, "before_id": page["next_before_id"]}),
    )
    .await;
    assert_ne!(next["entries"][0]["id"], page["entries"][0]["id"]);

    // The action menu.
    let actions = rpc(app, "audit.actions", json!({})).await;
    let names: Vec<&str> = actions["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action["action"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"incidents.create"), "{names:?}");
    assert!(names.contains(&"notes.delete"));
    assert_eq!(actions["truncated"], false);
}
