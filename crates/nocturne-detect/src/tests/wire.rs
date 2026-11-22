//! The shapes on the wire, pinned: the interface is typed from these field
//! names, so a field that appears, disappears or is renamed has to fail a
//! test rather than a screen.
//!
//! Run with `--nocapture` to print one real example of every method's
//! parameters and result, and of both events.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::support::*;
use crate::catalogue::UNPLANNED_CONFIG_CHANGE;

fn keys(value: &Value) -> Vec<&str> {
    value
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect()
}

fn show(what: &str, value: &Value) {
    println!(
        "--- {what}\n{}",
        serde_json::to_string_pretty(value).unwrap()
    );
}

#[tokio::test]
async fn every_method_and_event_has_exactly_the_specified_fields() {
    let world = world(Build {
        maintenance: false,
        ..Build::FULL
    })
    .await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let mut events = app.subscribe();

    // A hit, and a check that could not be made, so that every optional
    // field of an entry is present somewhere in the list.
    let at = now();
    let entry_id = syslog_line(app, &sw1, "syslog", at);
    app.publish(
        "logs.config_changed",
        &json!({"device_id": sw1, "by": "admin", "from": "console", "entry_id": entry_id, "at": at}),
    );
    settle(app).await;
    *world.fakes.changes_fail.lock() = Some("the database is locked".into());
    app.publish(
        "logs.config_changed",
        &json!({"device_id": sw1, "by": "admin", "from": "console", "entry_id": entry_id, "at": at}),
    );
    settle(app).await;

    // detections.list
    let list = app.rpc("detections.list", Value::Null).await.unwrap();
    show("detections.list -> result", &list);
    let list = list.as_array().unwrap();
    let fired = &list[0];
    assert_eq!(fired["id"], UNPLANNED_CONFIG_CHANGE);
    assert_eq!(
        keys(fired),
        [
            "id",
            "name",
            "description",
            "severity",
            "decided_from",
            "cannot_see",
            "enabled",
            "available",
            "last_hit_at",
            "hits_24h",
            "last_error",
        ]
    );
    assert_eq!(keys(&fired["last_error"]), ["at", "message"]);
    let unavailable = list.last().unwrap();
    assert_eq!(
        keys(unavailable),
        [
            "id",
            "name",
            "description",
            "severity",
            "decided_from",
            "cannot_see",
            "enabled",
            "available",
            "unavailable_reason",
            "hits_24h",
        ]
    );

    // detections.recent
    let params =
        json!({"id": UNPLANNED_CONFIG_CHANGE, "device_id": sw1, "since": at - MINUTE, "limit": 10});
    show("detections.recent -> params", &params);
    let hits = app.rpc("detections.recent", params).await.unwrap();
    show("detections.recent -> result", &hits);
    let hit = &hits[0];
    assert_eq!(
        keys(hit),
        [
            "id",
            "detection",
            "at",
            "device_id",
            "severity",
            "title",
            "evidence",
            "alert_id",
            "lab",
        ]
    );

    // The same detection decided from a backup: other evidence.
    *world.fakes.changes_fail.lock() = None;
    let sw2 = world.device("SW2", "10.0.0.2").await;
    backed_up_twice(app, &sw2, 8);
    settle(app).await;
    let from_backup = app
        .rpc(
            "detections.recent",
            json!({"id": UNPLANNED_CONFIG_CHANGE, "device_id": sw2}),
        )
        .await
        .unwrap();
    show(
        "detections.recent -> a hit decided from a backup",
        &from_backup[0],
    );
    assert_eq!(
        keys(&from_backup[0]["evidence"]),
        [
            "decided_from",
            "kind",
            "version_id",
            "previous_id",
            "between",
            "lines_added",
            "lines_removed",
            "changes_checked",
            "grace_minutes",
        ]
    );
    assert_eq!(keys(&from_backup[0]["evidence"]["between"]), ["from", "to"]);
    *world.fakes.changes_fail.lock() = Some("the database is locked".into());
    app.publish(
        "logs.config_changed",
        &json!({"device_id": sw1, "by": "admin", "from": "console", "entry_id": entry_id, "at": at}),
    );
    settle(app).await;

    // detections.set_enabled
    let params = json!({"id": UNPLANNED_CONFIG_CHANGE, "enabled": false});
    show("detections.set_enabled -> params", &params);
    let entry = app.rpc("detections.set_enabled", params).await.unwrap();
    show("detections.set_enabled -> result", &entry);
    assert_eq!(keys(&entry), keys(fired));
    settle(app).await;

    // The events.
    let mut hit_events = Vec::new();
    let mut changed_events = Vec::new();
    while let Ok(event) = events.try_recv() {
        match event.topic.as_str() {
            "detections.hit" => hit_events.push(event.payload),
            "detections.changed" => changed_events.push(event.payload),
            _ => {}
        }
    }
    show("event detections.hit", &hit_events[0]);
    assert_eq!(keys(&hit_events[0]), ["hit"]);
    assert_eq!(&hit_events[0]["hit"], hit);
    for changed in &changed_events {
        show("event detections.changed", changed);
    }
    // One for the check that failed, one when the next went through, one
    // when it failed again, and one for the switch.
    assert_eq!(
        changed_events,
        vec![json!({"id": UNPLANNED_CONFIG_CHANGE}); 4]
    );

    // What `alerts.raise` is handed.
    let raised = world.fakes.raised.lock();
    let raised = raised.last().unwrap();
    show("alerts.raise <- params", raised);
    assert_eq!(
        keys(raised),
        [
            "rule",
            "device_id",
            "severity",
            "title",
            "detail",
            "dedup_key"
        ]
    );
}
