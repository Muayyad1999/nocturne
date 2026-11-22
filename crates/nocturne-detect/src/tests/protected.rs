//! `protected_change_outside_window`.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use nocturne_app::App;

use super::support::*;
use crate::catalogue::PROTECTED_CHANGE_OUTSIDE_WINDOW as ID;

/// A finished change as `changes.changed` carries it — the `ChangeSummary`
/// of `nocturne-changes/src/model.rs` — with protected hits on each device.
fn finished(id: &str, state: &str, devices: &[(&str, &str, u64)], updated_at: i64) -> Value {
    let mut change = change(id, "unused", state, updated_at);
    change["title"] = json!("Uplink trunk");
    change["protected_hits"] = json!(devices.iter().map(|(_, _, hits)| hits).sum::<u64>());
    change["devices"] = devices
        .iter()
        .map(|(device_id, target_state, hits)| {
            json!({
                "device_id": device_id, "name": "SW1", "planned": true, "risk": "high",
                "protected_hits": hits, "commands": 4, "state": target_state,
            })
        })
        .collect();
    change
}

fn announce(app: &App, change: &Value) {
    app.publish("changes.changed", change);
}

/// A maintenance window as `maintenance.list` returns it
/// (`MaintenanceWindow` in `nocturne-monitor/src/model.rs`).
fn window(scope: Value, starts_at: i64, ends_at: i64) -> Value {
    json!({
        "id": "w1", "name": "Saturday night", "scope": scope, "starts_at": starts_at,
        "ends_at": ends_at, "recurring": null, "note": "", "created_at": 1, "updated_at": 1,
    })
}

#[tokio::test]
async fn a_protected_change_with_no_window_open_is_a_hit_naming_the_change() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();

    announce(app, &finished("c1", "applied", &[(&sw1, "applied", 2)], at));
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(
        hit["title"],
        "`Uplink trunk` touched protected resources on SW1 outside any maintenance window"
    );
    assert_eq!(hit["severity"], "info");
    assert_eq!(hit["device_id"], json!(sw1));
    assert_eq!(hit["at"], json!(at));
    assert_eq!(
        hit["evidence"],
        json!({
            "decided_from": "changes.changed",
            "change_id": "c1",
            "state": "applied",
            "finished_at": at,
            "devices": [{"device_id": sw1, "protected_hits": 2}],
            "devices_no_longer_in_inventory": [],
            "windows_checked": 0,
            "grace_minutes": 15,
        })
    );
    assert_eq!(stored_events(app, ID).len(), 1);
    let raised = world
        .fakes
        .raised_with(&format!("detect:protected_change_outside_window:{sw1}:c1"));
    assert_eq!(raised.len(), 1, "{:#?}", world.fakes.raised.lock());
    assert_eq!(raised[0]["severity"], "info");
}

#[tokio::test]
async fn a_window_that_covers_the_device_explains_it_and_one_for_somewhere_else_does_not() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();
    let (opened, closes) = (at - 60 * MINUTE, at + 60 * MINUTE);

    for (scope, covered) in [
        (json!({}), true),
        (json!({"device_ids": [sw1]}), true),
        // The device is at site HQ, role access (see `World::device`).
        (json!({"sites": ["hq"], "roles": ["access"]}), true),
        (json!({"sites": ["Branch"]}), false),
        (json!({"device_ids": ["some-other-device"]}), false),
    ] {
        *world.fakes.windows.lock() = vec![window(scope.clone(), opened, closes)];
        let before = hits_of(app, ID).await.len();
        // A new change each time: the same one is only judged once.
        let id = format!("c-{before}-{covered}-{}", scope);
        announce(app, &finished(&id, "applied", &[(&sw1, "applied", 1)], at));
        settle(app).await;
        let fired = hits_of(app, ID).await.len() - before;
        assert_eq!(fired, usize::from(!covered), "scope {scope}");
    }
}

#[tokio::test]
async fn a_window_that_closed_within_the_grace_still_counts_and_an_older_one_does_not() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();

    // The work started inside the window and finished a few minutes after.
    *world.fakes.windows.lock() = vec![window(json!({}), at - 120 * MINUTE, at - 5 * MINUTE)];
    announce(app, &finished("c1", "applied", &[(&sw1, "applied", 1)], at));
    settle(app).await;
    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());

    *world.fakes.windows.lock() = vec![window(json!({}), at - 120 * MINUTE, at - 20 * MINUTE)];
    announce(app, &finished("c2", "applied", &[(&sw1, "applied", 1)], at));
    settle(app).await;
    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["evidence"]["windows_checked"], 1);

    // A window that has not opened yet announces nothing about now.
    *world.fakes.windows.lock() = vec![window(json!({}), at + 10 * MINUTE, at + 70 * MINUTE)];
    announce(app, &finished("c3", "applied", &[(&sw1, "applied", 1)], at));
    settle(app).await;
    assert_eq!(hits_of(app, ID).await.len(), 2);
}

#[tokio::test]
async fn only_a_finished_change_that_touched_something_protected_is_judged() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();

    // No protected hits at all.
    announce(
        app,
        &finished("plain", "applied", &[(&sw1, "applied", 0)], at),
    );
    // Still running: the event is published at every step of an apply.
    announce(
        app,
        &finished("running", "applying", &[(&sw1, "applying", 2)], at),
    );
    // Approved, never applied.
    announce(
        app,
        &finished("approved", "approved", &[(&sw1, "pending", 2)], at),
    );
    // Finished, but this device was skipped: nothing was sent to it.
    announce(
        app,
        &finished("skipped", "failed", &[(&sw1, "skipped", 2)], at),
    );
    // Finished, and it failed on this device. That is what a device is
    // called when the pipeline never reached it as well as when the session
    // was lost half-way, so nothing is said to have been touched.
    announce(
        app,
        &finished("never-reached", "failed", &[(&sw1, "failed", 2)], at),
    );
    // A rollback has just been started on it: judged when that finishes.
    let mut rolling_back = finished("rolling-back", "applied", &[(&sw1, "applied", 2)], at);
    rolling_back["job_id"] = json!("job-7");
    announce(app, &rolling_back);
    // And a deleted change, as the change engine announces one.
    announce(app, &json!({"id": "gone", "state": "deleted"}));
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    assert_eq!(listed(app, ID).await.get("last_error"), None);
}

#[tokio::test]
async fn a_change_announced_twice_is_one_hit_and_its_later_rollback_is_another() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();

    let applied = finished("c1", "applied", &[(&sw1, "applied", 2)], at);
    announce(app, &applied);
    announce(app, &applied);
    settle(app).await;
    assert_eq!(hits_of(app, ID).await.len(), 1);

    // Rolled back by hand an hour later, still with no window: the device's
    // protected resources were touched again.
    announce(
        app,
        &finished("c1", "rolled_back", &[(&sw1, "rolled_back", 2)], at + 1),
    );
    settle(app).await;
    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 2, "{hits:#?}");
    assert_eq!(hits[0]["evidence"]["state"], "rolled_back");
    // Both under the change's key, so one alert.
    assert_eq!(
        world
            .fakes
            .raised_with(&format!("detect:protected_change_outside_window:{sw1}:c1"))
            .len(),
        2
    );
}

#[tokio::test]
async fn of_two_devices_only_the_one_outside_a_window_is_named() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let sw2 = world.device("SW2", "10.0.0.2").await;
    let at = now();
    *world.fakes.windows.lock() = vec![window(
        json!({"device_ids": [sw1]}),
        at - 60 * MINUTE,
        at + 60 * MINUTE,
    )];

    announce(
        app,
        &finished(
            "c1",
            "applied",
            &[(&sw1, "applied", 3), (&sw2, "applied", 1)],
            at,
        ),
    );
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["device_id"], json!(sw2));
    assert_eq!(
        hits[0]["evidence"]["devices"],
        json!([{"device_id": sw2, "protected_hits": 1}])
    );

    // With neither covered it is about the change, not about one device.
    *world.fakes.windows.lock() = Vec::new();
    announce(
        app,
        &finished(
            "c2",
            "applied",
            &[(&sw1, "applied", 3), (&sw2, "applied", 1)],
            at,
        ),
    );
    settle(app).await;
    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 2, "{hits:#?}");
    assert_eq!(hits[0].get("device_id"), None);
    assert_eq!(
        hits[0]["title"],
        "`Uplink trunk` touched protected resources on 2 devices outside any maintenance window"
    );
    assert_eq!(
        world
            .fakes
            .raised_with("detect:protected_change_outside_window:-:c2")
            .len(),
        1
    );
}

#[tokio::test]
async fn without_maintenance_windows_it_is_unavailable_with_the_reason_and_silent() {
    let world = world(Build {
        maintenance: false,
        ..Build::FULL
    })
    .await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    announce(
        app,
        &finished("c1", "applied", &[(&sw1, "applied", 2)], now()),
    );
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    let entry = listed(app, ID).await;
    assert_eq!(entry["available"], false);
    assert_eq!(
        entry["unavailable_reason"],
        "`maintenance.list` is not in this build, so every protected change would look as if it were outside a window"
    );
    // Unavailable is a stated condition, not a check that failed.
    assert_eq!(entry.get("last_error"), None);
}

#[tokio::test]
async fn windows_that_cannot_be_read_or_understood_are_a_failed_check_not_an_empty_list() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();

    *world.fakes.windows_fail.lock() = Some("the store is busy".into());
    announce(app, &finished("c1", "applied", &[(&sw1, "applied", 2)], at));
    settle(app).await;
    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    assert_eq!(
        listed(app, ID).await["last_error"]["message"],
        "`maintenance.list` failed: the store is busy"
    );

    // A recurrence this feature does not know could be open right now.
    *world.fakes.windows_fail.lock() = None;
    let mut monthly = window(
        json!({}),
        at - 90 * 24 * 60 * MINUTE,
        at - 89 * 24 * 60 * MINUTE,
    );
    monthly["recurring"] = json!("monthly");
    *world.fakes.windows.lock() = vec![monthly];
    announce(app, &finished("c2", "applied", &[(&sw1, "applied", 2)], at));
    settle(app).await;
    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    let message = listed(app, ID).await["last_error"]["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        message.contains("recurs `monthly`") && message.contains("Saturday night"),
        "{message}"
    );
}
