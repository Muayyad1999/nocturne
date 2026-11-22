//! `unplanned_config_change`.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use nocturne_app::App;

use super::support::*;
use crate::catalogue::UNPLANNED_CONFIG_CHANGE as ID;

/// `logs.config_changed` as the log rules publish it
/// (`publish` in `nocturne-logs/src/rules/`).
fn said_so(
    app: &App,
    device_id: Option<&str>,
    by: Option<&str>,
    from: Option<&str>,
    at: i64,
) -> i64 {
    let entry_id = syslog_line(app, device_id.unwrap_or("nobody"), "syslog", at);
    app.publish(
        "logs.config_changed",
        &json!({"device_id": device_id, "by": by, "from": from, "entry_id": entry_id, "at": at}),
    );
    entry_id
}

#[tokio::test]
async fn a_change_from_the_console_that_nobody_approved_is_a_hit_naming_the_log_line() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let mut events = app.subscribe();
    let at = now();

    let entry_id = said_so(app, Some(&sw1), Some("admin"), Some("console"), at);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(
        hit["title"],
        "admin changed SW1 from console and no approved change was being applied"
    );
    assert_eq!(hit["detection"], ID);
    assert_eq!(hit["severity"], "high");
    assert_eq!(hit["device_id"], json!(sw1));
    assert_eq!(hit["at"], json!(at));
    assert_eq!(hit["lab"], false);
    // The evidence names the record, and says what it is worth: this line
    // arrived on the receiver, where anybody could have sent it.
    assert_eq!(
        hit["evidence"],
        json!({
            "decided_from": "log",
            "log_entry_id": entry_id,
            "stream": "syslog",
            "via": "syslog",
            "by": "admin",
            "from": "console",
            "changes_checked": 0,
            "grace_minutes": 15,
            "caution": "Syslog and SNMP v1/v2c traps are unauthenticated: anything that can reach the receiver could have sent this.",
        })
    );

    // It is in the `event` stream, as itself.
    let stored = stored_events(app, ID);
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].device_id.as_deref(), Some(sw1.as_str()));
    assert_eq!(stored[0].message, hit["title"].as_str().unwrap());
    assert_eq!(stored[0].severity, Some(3));
    assert_eq!(stored[0].ts, at);
    assert_eq!(stored[0].body.as_ref(), Some(hit));

    // It was raised once, in the one alert list, under the specified key.
    let raised = world
        .fakes
        .raised_with(&format!("detect:unplanned_config_change:{sw1}:config"));
    assert_eq!(raised.len(), 1, "{:#?}", world.fakes.raised.lock());
    assert_eq!(raised[0]["severity"], "high");
    assert_eq!(raised[0]["device_id"], json!(sw1));
    assert_eq!(raised[0]["title"], hit["title"]);
    assert_eq!(
        raised[0]["rule"],
        "Detection: Unplanned configuration change"
    );
    assert!(hit["alert_id"].as_str().unwrap().starts_with("alert-"));

    // And announced.
    let mut announced = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic == "detections.hit" && event.payload["hit"]["detection"] == ID {
            announced.push(event.payload["hit"].clone());
        }
    }
    assert_eq!(announced, std::slice::from_ref(hit));

    let entry = listed(app, ID).await;
    assert_eq!(
        (&entry["hits_24h"], &entry["last_hit_at"]),
        (&json!(1), &json!(at))
    );
}

#[tokio::test]
async fn the_title_says_only_what_the_device_said() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    // RouterOS says who and not from where; some lines say neither.
    said_so(app, Some(&sw1), Some("admin"), None, now());
    said_so(app, Some(&sw1), None, Some("console"), now());
    said_so(app, Some(&sw1), None, None, now());
    settle(app).await;

    let titles: Vec<String> = hits_of(app, ID)
        .await
        .iter()
        .rev()
        .map(|hit| hit["title"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        titles,
        [
            "admin changed SW1 and no approved change was being applied",
            "SW1 was configured from console and no approved change was being applied",
            "SW1 was configured and no approved change was being applied",
        ]
    );
    // Three sightings, three hits, one alert key: the alert list folds them.
    let keys: Vec<Value> = world
        .fakes
        .raised
        .lock()
        .iter()
        .filter(|raised| raised["title"].as_str().unwrap().contains("SW1"))
        .map(|raised| raised["dedup_key"].clone())
        .collect();
    assert_eq!(
        keys,
        vec![json!(format!("detect:unplanned_config_change:{sw1}:config")); 3]
    );
}

#[tokio::test]
async fn a_change_the_pipeline_is_applying_explains_the_log_line() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    // Saved hours ago and still applying: it is the state that counts.
    *world.fakes.changes.lock() = vec![change("c1", &sw1, "applying", now() - 180 * MINUTE)];

    said_so(app, Some(&sw1), Some("nocturne"), Some("vty0"), now());
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
}

#[tokio::test]
async fn a_change_applied_inside_the_grace_explains_it_and_one_outside_does_not() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();

    *world.fakes.changes.lock() = vec![change("inside", &sw1, "applied", at - 14 * MINUTE)];
    said_so(app, Some(&sw1), Some("admin"), Some("console"), at);
    settle(app).await;
    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());

    *world.fakes.changes.lock() = vec![change("outside", &sw1, "applied", at - 16 * MINUTE)];
    said_so(app, Some(&sw1), Some("admin"), Some("console"), at);
    settle(app).await;
    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    // The hit shows what was compared: one change, and how far off it was.
    assert_eq!(hits[0]["evidence"]["changes_checked"], 1);
    assert_eq!(
        hits[0]["evidence"]["nearest_change"],
        json!({"change_id": "outside", "state": "applied", "updated_at": at - 16 * MINUTE})
    );

    // The grace is a setting, and widening it explains the same event.
    set(app, json!({"grace_minutes": 20})).await;
    said_so(app, Some(&sw1), Some("admin"), Some("console"), at);
    settle(app).await;
    assert_eq!(hits_of(app, ID).await.len(), 1);
}

#[tokio::test]
async fn a_change_on_another_device_or_one_that_never_ran_explains_nothing() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let sw2 = world.device("SW2", "10.0.0.2").await;
    *world.fakes.changes.lock() = vec![
        change("elsewhere", &sw2, "applied", now()),
        change("only-approved", &sw1, "approved", now()),
    ];

    said_so(app, Some(&sw1), Some("admin"), Some("console"), now());
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["device_id"], json!(sw1));
    assert_eq!(hits[0]["evidence"]["changes_checked"], 1);
}

#[tokio::test]
async fn a_line_from_a_source_that_is_no_known_device_accuses_nobody() {
    let world = world(Build::FULL).await;
    let app = &world.app;

    said_so(app, None, Some("admin"), Some("console"), now());
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
}

#[tokio::test]
async fn a_backup_that_differs_from_the_one_before_is_a_hit_naming_both_versions() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    let (previous_id, version_id) = backed_up_twice(app, &sw1, 8);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(
        hit["title"],
        "SW1's running configuration changed between two backups and no approved change was being applied"
    );
    let evidence = &hit["evidence"];
    assert_eq!(evidence["decided_from"], "backup");
    assert_eq!(evidence["version_id"], json!(version_id));
    assert_eq!(evidence["previous_id"], json!(previous_id));
    assert_eq!(
        (&evidence["lines_added"], &evidence["lines_removed"]),
        (&json!(1), &json!(0))
    );
    let (from, to) = (
        evidence["between"]["from"].as_i64().unwrap(),
        evidence["between"]["to"].as_i64().unwrap(),
    );
    assert_eq!(to - from, 8 * 60 * MINUTE);
    assert_eq!(hit["at"], json!(to));
    // A backup is something Nocturne read itself over its own session.
    assert_eq!(evidence.get("caution"), None);
    // The same key as a sighting in the log: one edit, one alert.
    assert_eq!(
        world
            .fakes
            .raised_with(&format!("detect:unplanned_config_change:{sw1}:config"))
            .len(),
        1
    );
}

#[tokio::test]
async fn a_change_applied_anywhere_between_the_two_backups_explains_the_difference() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    // Applied four hours before the backup that noticed it: far outside the
    // grace of that moment, well inside the stretch the change was made in.
    *world.fakes.changes.lock() = vec![change("c1", &sw1, "applied", now() - 4 * 60 * MINUTE)];

    backed_up_twice(app, &sw1, 8);
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
}

#[tokio::test]
async fn the_change_that_made_the_earlier_version_does_not_explain_what_was_done_to_it_since() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    // Thirty days ago an approved change was applied, and the earlier of the
    // two versions is the copy the pipeline stored as it finished.
    let month = 30 * 24 * 60;
    *world.fakes.changes.lock() = vec![change(
        "a-month-ago",
        &sw1,
        "applied",
        now() - month * MINUTE + 5_000,
    )];
    let (previous_id, version_id) = backed_up_twice(app, &sw1, month / 60);
    settle(app).await;
    // As far as anybody knows the earlier version was never fetched again,
    // so the change that made it may be what made the difference too.
    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());

    // But every nightly backup since fetched that same configuration, the
    // last of them two hours before the one that differs: whatever changed
    // the device did it in those two hours, a month after that change.
    let noticed = app.store().config_get(version_id).unwrap().0.taken_at;
    world
        .fakes
        .last_seen
        .lock()
        .insert(previous_id, noticed - 120 * MINUTE);
    app.publish(
        "config.changed",
        &json!({
            "device_id": sw1, "name": "SW1", "kind": "running",
            "version_id": version_id, "previous_id": previous_id, "added": 1, "removed": 0,
        }),
    );
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let evidence = &hits[0]["evidence"];
    // The stretch it was made in starts where the earlier version was last
    // seen, and the old change is shown as the nearest one, a month off.
    assert_eq!(
        evidence["between"],
        json!({"from": noticed - 120 * MINUTE, "to": noticed})
    );
    assert_eq!(evidence["nearest_change"]["change_id"], "a-month-ago");
    assert_eq!(evidence["changes_checked"], 1);
}

#[tokio::test]
async fn a_backup_in_which_no_line_differs_accuses_nobody() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    // A version was stored (the bytes differed) and the event counts no
    // line added or removed: the configuration is what it was.
    let (previous_id, version_id) = backed_up_twice(app, &sw1, 8);
    settle(app).await;
    assert_eq!(hits_of(app, ID).await.len(), 1, "the control: it differs");
    app.publish(
        "config.changed",
        &json!({
            "device_id": sw1, "name": "SW1", "kind": "running",
            "version_id": version_id, "previous_id": previous_id, "added": 0, "removed": 0,
        }),
    );
    settle(app).await;

    assert_eq!(hits_of(app, ID).await.len(), 1);
    assert_eq!(listed(app, ID).await.get("last_error"), None);
}

#[tokio::test]
async fn the_same_line_arriving_twice_is_one_hit_and_another_line_is_another() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let sw2 = world.device("SW2", "10.0.0.2").await;
    let at = now();
    let said = |device: &str, raw: &str, at: i64| {
        let entry_id = syslog_text(app, device, "syslog", at, raw);
        app.publish(
            "logs.config_changed",
            &json!({"device_id": device, "by": "admin", "from": "console", "entry_id": entry_id, "at": at}),
        );
        entry_id
    };
    let first = "<189>41: *Mar  1 00:13:01.000: %SYS-5-CONFIG_I: Configured from console by admin on console";
    let second = "<189>42: *Mar  1 00:13:09.000: %SYS-5-CONFIG_I: Configured from console by admin on console";

    // The network delivered the datagram twice (or the device logs to two
    // addresses that both end here): two stored lines, one thing said.
    let entry_id = said(&sw1, first, at);
    said(&sw1, first, at + 40);
    settle(app).await;
    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["evidence"]["log_entry_id"], json!(entry_id));
    assert_eq!(
        world
            .fakes
            .raised_with(&format!("detect:unplanned_config_change:{sw1}:config"))
            .len(),
        1
    );

    // The device's next line is another edit, and the same words from
    // another device are that device's own.
    said(&sw1, second, at + 80);
    said(&sw2, first, at + 90);
    settle(app).await;
    assert_eq!(
        recent(app, json!({"id": ID, "device_id": sw1})).await.len(),
        2
    );
    assert_eq!(
        recent(app, json!({"id": ID, "device_id": sw2})).await.len(),
        1
    );
}

#[tokio::test]
async fn a_backup_that_differs_with_nobody_to_ask_when_it_was_seen_is_not_judged() {
    // The log feature and the change engine, and no config feature: the
    // detection runs, and a `config.changed` has no business arriving. If
    // one does, nothing can say when its versions were seen.
    let world = world(Build {
        config: false,
        ..Build::FULL
    })
    .await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    backed_up_twice(app, &sw1, 8);
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    let entry = listed(app, ID).await;
    assert_eq!(entry["available"], true);
    assert_eq!(
        entry["last_error"]["message"],
        "`config.versions` is not in this build"
    );
}

#[tokio::test]
async fn a_difference_whose_start_cannot_be_bounded_is_not_judged_and_says_so() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    // The version before is gone (retention of one): when the change was
    // made is unknown, so nobody is accused.
    app.publish(
        "config.changed",
        &json!({"device_id": sw1, "kind": "running", "version_id": 9_001, "previous_id": 9_000, "added": 1, "removed": 0}),
    );
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    let entry = listed(app, ID).await;
    let message = entry["last_error"]["message"].as_str().unwrap();
    assert!(
        message.contains("no longer stored") && message.contains("SW1"),
        "{message}"
    );
}

#[tokio::test]
async fn without_the_change_engine_it_is_unavailable_with_the_reason_and_silent() {
    let world = world(Build {
        changes: false,
        ..Build::FULL
    })
    .await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    said_so(app, Some(&sw1), Some("admin"), Some("console"), now());
    backed_up_twice(app, &sw1, 1);
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    let entry = listed(app, ID).await;
    assert_eq!(entry["available"], false);
    assert_eq!(
        entry["unavailable_reason"],
        "`changes.list` is not in this build, so a change the pipeline made could not be told from one made by hand"
    );
    // Switched on, and still not able to run: the two are different things.
    assert_eq!(entry["enabled"], true);
    assert_eq!(entry["hits_24h"], 0);
    assert_eq!(entry.get("last_hit_at"), None);
    // And not able to run is a stated condition, not a check that failed:
    // the events were not looked at at all.
    assert_eq!(entry.get("last_error"), None);
}

#[tokio::test]
async fn a_change_engine_that_fails_is_recorded_as_a_failed_check_and_accuses_nobody() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    *world.fakes.changes_fail.lock() = Some("the database is locked".into());
    let before = now();

    said_so(app, Some(&sw1), Some("admin"), Some("console"), now());
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    let entry = listed(app, ID).await;
    assert_eq!(entry["available"], true);
    assert_eq!(
        entry["last_error"]["message"],
        "`changes.list` failed: the database is locked"
    );
    assert!(entry["last_error"]["at"].as_i64().unwrap() >= before);

    // The next check that goes through clears it.
    *world.fakes.changes_fail.lock() = None;
    said_so(app, Some(&sw1), Some("admin"), Some("console"), now());
    settle(app).await;
    assert_eq!(hits_of(app, ID).await.len(), 1);
    assert_eq!(listed(app, ID).await.get("last_error"), None);
}

#[tokio::test]
async fn without_an_alert_list_the_hit_is_kept_and_only_the_alert_is_missing() {
    let world = world(Build {
        alerts: false,
        ..Build::FULL
    })
    .await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let mut events = app.subscribe();

    said_so(app, Some(&sw1), Some("admin"), Some("console"), now());
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].get("alert_id"), None);
    assert_eq!(stored_events(app, ID).len(), 1);
    let mut announced = 0;
    while let Ok(event) = events.try_recv() {
        if event.topic == "detections.hit" && event.payload["hit"]["detection"] == ID {
            announced += 1;
        }
    }
    assert_eq!(announced, 1);
    assert_eq!(listed(app, ID).await["available"], true);
}

#[tokio::test]
async fn a_lab_devices_hit_is_labelled_lab_and_its_routing_is_left_to_the_alert_list() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let lab_sw = world.lab_device();

    // The lab hands its lines over in process; nobody on the network could
    // have forged this one.
    let at = now();
    let entry_id = syslog_line(app, &lab_sw, "lab", at);
    app.publish(
        "logs.config_changed",
        &json!({"device_id": lab_sw, "by": "console", "from": "console", "entry_id": entry_id, "at": at}),
    );
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["lab"], true);
    assert_eq!(hits[0]["evidence"]["via"], "lab");
    assert_eq!(hits[0]["evidence"].get("caution"), None);

    // The alert names the device, which is what the alert list's own rule
    // ("a lab device's alert reaches no channel") goes by, and it does not
    // say who to notify — that would override the rule from outside.
    let raised = world
        .fakes
        .raised_with(&format!("detect:unplanned_config_change:{lab_sw}:config"));
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0]["device_id"], json!(lab_sw));
    assert_eq!(raised[0].get("notify"), None);
}

#[tokio::test]
async fn a_device_deleted_since_is_still_named_by_the_hit_and_the_alert_goes_out_without_it() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    app.rpc("devices.delete", json!({"ids": [sw1]}))
        .await
        .unwrap();

    said_so(app, Some(&sw1), Some("admin"), Some("console"), now());
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["device_id"], json!(sw1));
    assert!(
        hits[0]["title"]
            .as_str()
            .unwrap()
            .contains("a device no longer in the inventory"),
        "{}",
        hits[0]["title"]
    );
    // The alert list refuses an alert for a device it cannot find; the hit
    // must not be lost to that.
    let raised = world
        .fakes
        .raised_with(&format!("detect:unplanned_config_change:{sw1}:config"));
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0]["device_id"], Value::Null);
    assert!(hits[0]["alert_id"].is_string());
}
