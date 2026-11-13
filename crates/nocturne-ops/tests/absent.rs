//! A fresh installation: this feature and the core, and **nothing else**.
//!
//! Every other feature ops gathers from is missing — which is what a build
//! straight out of the box looks like. The whole surface still has to work,
//! and every part that has no source has to say which source and why, never
//! showing a zero for something nobody measured.

mod common;

use common::{With, lab, rpc};
use nocturne_app::util::now_ms;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

/// Everything ops would gather from if it were there.
const GATHERED_FROM: [&str; 11] = [
    "alerts.list",
    "alerts.get",
    "alerts.summary",
    "changes.list",
    "changes.get",
    "compliance.results",
    "insights.list",
    "config.devices",
    "config.search",
    "ipam.search",
    "logs.query",
];

#[tokio::test]
async fn none_of_the_features_ops_gathers_from_are_in_this_build() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let methods = lab.app.methods();
    for method in GATHERED_FROM {
        assert!(
            !methods.contains(&method),
            "`{method}` should not exist in this test"
        );
    }
    // And ops's own surface is all there.
    for method in [
        "overview.get",
        "incidents.list",
        "incidents.get",
        "incidents.create",
        "incidents.update",
        "incidents.delete",
        "incidents.note",
        "incidents.link",
        "incidents.sync",
        "incidents.acknowledge",
        "incidents.assign",
        "incidents.resolve",
        "incidents.reopen",
        "incidents.suggest_links",
        "incidents.export",
        "handover.generate",
        "handover.save",
        "handover.list",
        "handover.get",
        "handover.delete",
        "handover.export",
        "reports.kinds",
        "reports.generate",
        "reports.save",
        "reports.list",
        "reports.get",
        "reports.delete",
        "notes.list",
        "notes.get",
        "notes.save",
        "notes.delete",
        "search.query",
        "audit.search",
        "audit.actions",
    ] {
        assert!(methods.contains(&method), "`{method}` was not registered");
    }
}

#[tokio::test]
async fn the_whole_surface_answers_without_a_single_other_feature() {
    let lab = lab(&["SW1", "SW2"], With::nothing()).await;
    let app = &lab.app;
    let one = lab.id("SW1");
    let now = now_ms();

    // ---- the overview
    let overview = rpc(app, "overview.get", json!({})).await;
    assert_eq!(overview["devices"]["total"], 2);
    for absent in ["alerts", "changes", "compliance", "insights"] {
        assert_eq!(overview[absent], Value::Null, "{absent}");
    }
    assert_eq!(overview["unavailable"].as_array().unwrap().len(), 4);

    // ---- an incident, by hand, with a timeline of what the core knows
    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "Something is wrong", "severity": "sev2", "device_ids": [one],
               "started_at": now - 3_600_000, "author": "mo"}),
    )
    .await;
    let id = incident["id"].as_str().unwrap().to_string();
    assert_eq!(incident["state"], "open");
    assert_eq!(
        incident["gaps"], 0,
        "nothing failed: the missing features were never asked about this incident"
    );
    assert!(
        !incident["timeline"].as_array().unwrap().is_empty(),
        "opening it is itself on the timeline"
    );

    // Linking an alert that cannot exist is refused with the reason, not
    // accepted as if it had worked.
    let refused = app
        .rpc(
            "incidents.create",
            json!({"from_alert": "a1", "title": "From an alert"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "unavailable");
    assert!(
        refused
            .message
            .contains("`alerts.get` is not part of this build"),
        "{}",
        refused.message
    );

    // Linking by id still records the link, and the timeline says the
    // records behind it could not be read.
    let linked = rpc(
        app,
        "incidents.link",
        json!({"id": id, "alert_ids": ["a1"], "change_ids": ["c1"]}),
    )
    .await;
    assert_eq!(linked["alert_ids"], json!(["a1"]));
    assert_eq!(linked["gaps"], 2, "one per source that could not be read");
    let gaps: Vec<&Value> = linked["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["kind"] == json!("gap"))
        .collect();
    assert!(
        gaps.iter()
            .any(|gap| gap["source"]["from"] == json!("alerts.get")
                && gap["text"]
                    .as_str()
                    .unwrap()
                    .contains("is not part of this build")),
        "{gaps:#?}"
    );
    assert!(
        gaps.iter()
            .any(|gap| gap["source"]["from"] == json!("changes.get"))
    );

    // Everything else about an incident works unchanged.
    rpc(
        app,
        "incidents.note",
        json!({"id": id, "text": "Had a look."}),
    )
    .await;
    rpc(
        app,
        "incidents.acknowledge",
        json!({"id": id, "author": "mo"}),
    )
    .await;
    let suggested = rpc(app, "incidents.suggest_links", json!({"id": id})).await;
    assert_eq!(suggested["alerts"], json!([]));
    assert_eq!(suggested["changes"], json!([]));
    let why: Vec<&str> = suggested["unavailable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["section"].as_str().unwrap())
        .collect();
    assert_eq!(
        why,
        ["alerts", "changes"],
        "an empty suggestion list is explained, not left to look like 'nothing to suggest'"
    );
    let resolved = rpc(
        app,
        "incidents.resolve",
        json!({"id": id, "root_cause": "A loose cable."}),
    )
    .await;
    assert_eq!(resolved["state"], "resolved");
    let exported = rpc(
        app,
        "incidents.export",
        json!({"id": id, "utc_offset_minutes": 0}),
    )
    .await;
    let review = exported["content"].as_str().unwrap();
    assert!(review.contains("## Gaps in the evidence"));
    assert!(review.contains("## Root cause\n\nA loose cable."));

    // ---- the handover
    let handover = rpc(
        app,
        "handover.generate",
        json!({"since": now - 8 * 3_600_000, "until": now}),
    )
    .await;
    assert_eq!(handover["incidents"]["opened"].as_array().unwrap().len(), 1);
    for absent in ["alerts", "changes", "backups"] {
        assert_eq!(handover[absent], Value::Null, "{absent}");
    }
    assert!(
        handover["devices"].is_object(),
        "the core knows its devices"
    );
    assert!(handover["config_changes"].is_array());
    assert!(handover["jobs_failed"].is_array());
    let reasons: Vec<&str> = handover["unavailable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["section"].as_str().unwrap())
        .collect();
    assert_eq!(reasons, ["alerts", "changes", "backups"]);
    let saved = rpc(
        app,
        "handover.save",
        json!({"handover": handover, "notes": "Quiet shift.", "author": "mo"}),
    )
    .await;
    assert_eq!(saved["notes"], "Quiet shift.");
    assert_eq!(saved["data"]["alerts"], Value::Null);
    let head = rpc(app, "handover.list", json!({})).await;
    assert_eq!(
        head["handovers"][0]["alerts_fired"],
        Value::Null,
        "a count with no source is null in the list too, never 0"
    );
    assert_eq!(head["handovers"][0]["incidents_opened"], 1);
    rpc(
        app,
        "handover.export",
        json!({"id": saved["id"], "format": "html", "utc_offset_minutes": 0}),
    )
    .await;

    // ---- every report kind in every format
    for kind in [
        "inventory",
        "software",
        "availability",
        "changes",
        "backups",
        "compliance",
        "alerts",
    ] {
        for format in ["markdown", "csv", "html", "json"] {
            let report = rpc(
                app,
                "reports.generate",
                json!({"kind": kind, "format": format, "utc_offset_minutes": 0}),
            )
            .await;
            let content = report["content"].as_str().unwrap();
            assert!(!content.is_empty(), "{kind}/{format} is empty");
            assert_eq!(report["kind"], kind);
            if ["changes", "compliance", "alerts"].contains(&kind) {
                assert_eq!(report["rows"], 0, "{kind}/{format}");
                assert_eq!(
                    report["unavailable"].as_array().unwrap().len(),
                    1,
                    "{kind}/{format} does not explain itself"
                );
            } else {
                assert!(
                    report["unavailable"].as_array().unwrap().is_empty(),
                    "{kind}/{format} needs nothing but the core: {}",
                    report["unavailable"]
                );
            }
        }
    }
    rpc(
        app,
        "reports.save",
        json!({"kind": "inventory", "format": "markdown"}),
    )
    .await;
    assert_eq!(
        rpc(app, "reports.list", json!({})).await["reports"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // ---- notes, search and the audit trail
    rpc(
        app,
        "notes.save",
        json!({"title": "How to fail over the WAN", "body": "Shut [[SW1]]."}),
    )
    .await;
    assert_eq!(
        rpc(app, "notes.list", json!({"query": "wan"})).await["notes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let found = rpc(app, "search.query", json!({"q": "wan"})).await;
    assert_eq!(found["notes"].as_array().unwrap().len(), 1);
    assert!(found["devices"].is_array());
    assert!(found["incidents"].is_array());
    for absent in ["changes", "config", "addresses", "logs", "location"] {
        assert_eq!(found[absent], Value::Null, "{absent}");
    }
    let partial: Vec<&str> = found["partial"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["section"].as_str().unwrap())
        .collect();
    assert_eq!(
        partial,
        ["changes", "config", "addresses", "logs"],
        "every source that was not there is named"
    );
    assert!(
        found["partial"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["reason"] == json!("not_installed")),
        "{:?}",
        found["partial"]
    );
    // An address is a question only the topology feature can answer, so a
    // build without it says that rather than answering "not found".
    let located = rpc(app, "search.query", json!({"q": "10.0.0.9"})).await;
    assert_eq!(located["location"], Value::Null);
    let location = located["partial"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["section"] == json!("location"))
        .expect("the missing locator is named");
    assert_eq!(location["reason"], "not_installed");
    assert!(
        location["message"]
            .as_str()
            .unwrap()
            .contains("`locate.host` is not part of this build")
    );

    let audit = rpc(app, "audit.search", json!({"action": "incidents."})).await;
    assert!(!audit["entries"].as_array().unwrap().is_empty());
    assert!(
        rpc(app, "audit.actions", json!({})).await["actions"]
            .as_array()
            .unwrap()
            .len()
            >= 2
    );
}

#[tokio::test]
async fn an_empty_installation_with_no_devices_at_all() {
    let lab = lab(&[], With::nothing()).await;
    let app = &lab.app;
    let overview = rpc(app, "overview.get", json!({})).await;
    assert_eq!(overview["devices"]["total"], 0);
    assert_eq!(overview["devices"]["by_site"], json!([]));
    assert_eq!(overview["backups"]["devices_with_backup"], 0);
    assert_eq!(overview["backups"]["oldest_backup_at"], Value::Null);
    assert_eq!(overview["incidents"]["unresolved"], 0);
    // The core's own audit entries are the only activity there is.
    assert!(
        overview["activity"]
            .as_array()
            .unwrap()
            .iter()
            .all(|line| line["kind"] == json!("audit")),
        "{}",
        overview["activity"]
    );

    let report = rpc(
        app,
        "reports.generate",
        json!({"kind": "inventory", "format": "markdown"}),
    )
    .await;
    assert_eq!(report["rows"], 0);
    assert!(
        report["content"]
            .as_str()
            .unwrap()
            .contains("Nothing in this period."),
        "an empty table says it is empty: {}",
        report["content"]
    );
    assert_eq!(
        rpc(app, "incidents.list", json!({})).await["counts"]["total"],
        0
    );
    assert_eq!(
        rpc(app, "handover.generate", json!({"since": now_ms() - 1_000})).await["devices"]["added"],
        json!([])
    );
}
