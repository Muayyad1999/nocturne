//! The firing notice an alert still owes when whatever held it back ends —
//! paid by the scheduler's own pass, for the alerts nothing looks at again.
//!
//! A configuration change, a log rule, a detection: each raises its alert
//! once. Raised inside a maintenance window, or snoozed before anybody was
//! told, such an alert used to stay unannounced for good, because only
//! another sighting paid the notice and there never was one.

use std::sync::Arc;
use std::time::Duration;

use nocturne_app::{AppBuilder, AppConfig};
use serde_json::{Value, json};

use super::notifications::{MINUTES, harness_with_a_channel, listen, window};
use crate::alerts::announce_owed;
use crate::model::{Alert, AlertState, DeviceRecord, Severity};
use crate::state::MonitorState;
use crate::testing::{FakeProbe, FakeSnmp, Harness, ManualClock, SteppedClock};

async fn raise(harness: &Harness, key: &str) -> Value {
    harness
        .rpc(
            "alerts.raise",
            json!({"rule": "Detection: unplanned reload", "severity": "critical",
                   "title": "Restarted with no change behind it", "dedup_key": key}),
        )
        .await
}

/// The keys of the alerts one pass announced.
fn pass(harness: &Harness) -> Vec<String> {
    let mut keys: Vec<String> = announce_owed(&harness.app, harness.now())
        .unwrap()
        .into_iter()
        .map(|alert| alert.dedup_key)
        .collect();
    keys.sort();
    keys
}

/// The `notification` of every `alerts.changed` published since the last
/// call.
fn notifications(events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>) -> Vec<Value> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic == "alerts.changed" {
            seen.push(event.payload["notification"].clone());
        }
    }
    seen
}

#[tokio::test]
async fn an_alert_raised_once_inside_a_window_is_announced_when_the_window_closes() {
    // 22:00–23:00. A detection fires at 22:00 and never again: it has said
    // what it had to say.
    let (harness, clock, listener) = harness_with_a_channel().await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    let raised = raise(&harness, "detect:reload:SW1").await;
    assert_eq!(raised["muted_by_maintenance"], true);
    assert!(raised["notified_at"].is_null());
    let seen_at = harness.now();
    let mut events = harness.app.subscribe();

    // 22:30: the window still covers it.
    clock.advance(30 * MINUTES);
    assert!(pass(&harness).is_empty());
    assert!(listener.events_after_a_pause().await.is_empty());

    // 23:01: nobody has raised it again and nothing evaluates it. The pass
    // is the only look it will ever get.
    clock.advance(31 * MINUTES);
    assert_eq!(pass(&harness), ["detect:reload:SW1"]);
    assert_eq!(listener.events(1).await, ["firing"]);
    assert_eq!(listener.requests()[0].1["alert"]["id"], raised["id"]);
    let told = crate::alerts::get(&harness.app, raised["id"].as_str().unwrap()).unwrap();
    assert_eq!(told.notified_at, Some(harness.now()));
    assert!(!told.muted_by_maintenance);
    assert_eq!(
        (told.count, told.last_seen_at),
        (1, seen_at),
        "told about, not seen again"
    );
    assert_eq!(
        notifications(&mut events),
        [json!("firing")],
        "the one event the desktop shell shows a notification for"
    );

    // Once.
    clock.advance(MINUTES);
    assert!(pass(&harness).is_empty());
    assert_eq!(listener.events_after_a_pause().await, ["firing"]);

    // And having been announced, its end is too.
    harness
        .rpc("alerts.clear", json!({"dedup_key": "detect:reload:SW1"}))
        .await;
    assert_eq!(listener.events(2).await, ["firing", "resolved"]);
}

#[tokio::test]
async fn a_configuration_change_made_inside_a_window_is_announced_after_it_to_the_rules_channels() {
    // Every change is its own alert, keyed by the version, so no later
    // event is ever another sighting of this one.
    let (harness, clock, everyone) = harness_with_a_channel().await;
    let changes = listen(vec![]).await;
    let changes_id = harness
        .rpc(
            "notify.channels.save",
            json!({"name": "changes", "kind": "webhook", "url": changes.url}),
        )
        .await["id"]
        .clone();
    harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.config_changed", "notify": [changes_id]}),
        )
        .await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    crate::rules::on_config_changed(
        &harness.app,
        &device,
        &json!({"device_id": device, "kind": "running", "version_id": 7, "added": 3, "removed": 1}),
    )
    .unwrap();
    assert_eq!(harness.open_alerts()[0].notified_at, None);

    clock.advance(61 * MINUTES);
    assert_eq!(pass(&harness).len(), 1);
    assert_eq!(changes.events(1).await, ["firing"]);
    assert_eq!(
        changes.requests()[0].1["alert"]["rule_name"],
        "Configuration changed"
    );
    assert!(
        everyone.events_after_a_pause().await.is_empty(),
        "the rule names its channel, and a late notice goes where a prompt one would have"
    );
}

#[tokio::test]
async fn a_snooze_that_runs_out_lets_the_announcement_through_with_nobody_looking() {
    let (harness, clock, listener) = harness_with_a_channel().await;
    let opened = window(&harness, Duration::ZERO, 60 * MINUTES).await;
    let alert = raise(&harness, "k1").await;
    harness
        .rpc("alerts.snooze", json!({"id": alert["id"], "minutes": 30}))
        .await;
    // The window goes away; the snooze still stands.
    harness
        .rpc("maintenance.delete", json!({"id": opened["id"]}))
        .await;

    clock.advance(10 * MINUTES);
    assert!(pass(&harness).is_empty());
    assert!(listener.events_after_a_pause().await.is_empty());

    clock.advance(21 * MINUTES);
    assert_eq!(pass(&harness), ["k1"]);
    assert_eq!(listener.events(1).await, ["firing"]);
}

#[tokio::test]
async fn a_snooze_made_longer_moves_the_announcement_and_one_taken_off_lets_it_through() {
    // The pass reads the snooze as it stands when it comes by, not as it
    // stood when the alert was snoozed first.
    let (harness, clock, listener) = harness_with_a_channel().await;
    let opened = window(&harness, Duration::ZERO, 60 * MINUTES).await;
    let moved = raise(&harness, "moved").await;
    let lifted = raise(&harness, "lifted").await;
    for alert in [&moved, &lifted] {
        harness
            .rpc("alerts.snooze", json!({"id": alert["id"], "minutes": 30}))
            .await;
    }
    harness
        .rpc("maintenance.delete", json!({"id": opened["id"]}))
        .await;

    // Twenty minutes in, one snooze is made half an hour longer and the
    // other is taken off.
    clock.advance(20 * MINUTES);
    harness
        .rpc("alerts.snooze", json!({"id": moved["id"], "minutes": 30}))
        .await;
    harness
        .rpc("alerts.snooze", json!({"id": lifted["id"], "minutes": 0}))
        .await;
    assert_eq!(pass(&harness), ["lifted"]);
    assert_eq!(listener.events(1).await, ["firing"]);

    // Thirty-five minutes in: the snooze as first set has run out, and the
    // one that replaced it has not.
    clock.advance(15 * MINUTES);
    assert!(pass(&harness).is_empty());
    assert_eq!(listener.events_after_a_pause().await, ["firing"]);

    clock.advance(16 * MINUTES);
    assert_eq!(pass(&harness), ["moved"]);
    assert_eq!(listener.events(2).await, ["firing", "firing"]);
    assert_eq!(listener.requests()[1].1["alert"]["id"], moved["id"]);

    clock.advance(MINUTES);
    assert!(pass(&harness).is_empty(), "each of them once");
    assert_eq!(listener.events_after_a_pause().await.len(), 2);
}

#[tokio::test]
async fn a_notice_owed_when_the_application_is_closed_is_paid_once_by_the_next_one() {
    // What is owed is on the alert, in the store; where the pass had got to
    // is not, and does not need to be.
    let directory = tempfile::tempdir().unwrap();
    let start = |now: i64| {
        let mut builder = AppBuilder::new(AppConfig::at(directory.path())).expect("an app on disk");
        nocturne_app::init(&mut builder);
        crate::init(&mut builder);
        builder.state(MonitorState::fake(
            Arc::new(ManualClock::new(now)),
            Arc::new(FakeProbe::default()),
            Arc::new(FakeSnmp::default()),
        ));
        builder.build()
    };
    let keys = |announced: Vec<Alert>| -> Vec<String> {
        announced.into_iter().map(|alert| alert.dedup_key).collect()
    };
    let listener = listen(vec![]).await;
    let evening = crate::testing::epoch();
    let raise = |key: &str, device: Option<&str>| {
        json!({"rule": "Detection: unplanned reload", "device_id": device, "severity": "critical",
               "title": "Restarted with no change behind it", "dedup_key": key})
    };

    {
        let app = start(evening);
        app.rpc(
            "notify.channels.save",
            json!({"name": "hook", "kind": "webhook", "url": listener.url}),
        )
        .await
        .unwrap();
        let simulated = app
            .rpc(
                "devices.create",
                json!({"name": "SIM-R1", "address": "", "transport": "sim",
                       "monitor": {"enabled": false}}),
            )
            .await
            .unwrap();
        // Announced before the window opens: nothing is owed for it.
        let told = app.rpc("alerts.raise", raise("told", None)).await.unwrap();
        assert_eq!(told["notified_at"], json!(evening));
        assert_eq!(listener.events(1).await, ["firing"]);

        app.rpc(
            "maintenance.save",
            json!({"name": "Evening work", "starts_at": evening,
                   "ends_at": evening + 3_600_000}),
        )
        .await
        .unwrap();
        let held = app.rpc("alerts.raise", raise("held", None)).await.unwrap();
        assert!(held["notified_at"].is_null());
        let lab = app
            .rpc("alerts.raise", raise("lab", simulated["id"].as_str()))
            .await
            .unwrap();
        assert_eq!(lab["lab"], true);
        app.shutdown();
    }
    // Shutting down is asynchronous: the data directory is released once
    // the background tasks have had their turn.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The next morning's application, after the window. The alert it was
    // holding back is announced; the one announced last night is not
    // announced again; the lab's stays quiet across the restart.
    let morning = evening + 3_660_000;
    {
        let app = start(morning);
        assert_eq!(keys(announce_owed(&app, morning).unwrap()), ["held"]);
        assert_eq!(listener.events(2).await, ["firing", "firing"]);
        assert_eq!(listener.requests()[1].1["alert"]["dedup_key"], "held");
        assert!(announce_owed(&app, morning + 15_000).unwrap().is_empty());
        app.shutdown();
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    // And closed and started once more: paid is paid.
    let app = start(morning + 60_000);
    assert!(announce_owed(&app, morning + 60_000).unwrap().is_empty());
    assert_eq!(listener.events_after_a_pause().await.len(), 2);
    let on_record = crate::alerts::list(&app, &Default::default()).unwrap();
    let of = |key: &str| {
        on_record
            .iter()
            .find(|alert| alert.dedup_key == key)
            .unwrap_or_else(|| panic!("`{key}` is on record"))
    };
    assert_eq!(of("told").notified_at, Some(evening));
    assert_eq!(of("held").notified_at, Some(morning));
    assert_eq!(
        (of("lab").state, of("lab").notified_at),
        (AlertState::Firing, None)
    );
}

#[tokio::test]
async fn an_alert_somebody_took_on_or_that_ended_while_it_was_held_back_is_never_announced() {
    let (harness, clock, listener) = harness_with_a_channel().await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    let taken = raise(&harness, "acknowledged").await;
    let by_hand = raise(&harness, "resolved").await;
    raise(&harness, "cleared").await;
    harness.rpc("alerts.ack", json!({"id": taken["id"]})).await;
    harness
        .rpc("alerts.resolve", json!({"id": by_hand["id"]}))
        .await;
    harness
        .rpc("alerts.clear", json!({"dedup_key": "cleared"}))
        .await;

    clock.advance(61 * MINUTES);
    assert!(pass(&harness).is_empty());
    assert!(listener.events_after_a_pause().await.is_empty());
    assert_eq!(harness.all_alerts().len(), 3, "all three are on record");
}

/// An alert as the store holds it when the application starts: raised and
/// last seen hours ago, inside a window, announced to nobody.
fn from_last_night(harness: &Harness, alert: Alert) {
    let now = harness.now();
    let alert = Alert {
        severity: Severity::Critical,
        state: AlertState::Firing,
        started_at: now - 8 * 3_600_000,
        last_seen_at: now - 7 * 3_600_000,
        count: 30,
        muted_by_maintenance: true,
        ..alert
    };
    harness
        .app
        .store()
        .with(|conn| {
            conn.execute(
                "INSERT INTO monitor_alerts
                 (id, dedup_key, state, severity, device_id, started_at, last_seen_at, body)
                 VALUES (?1, ?2, 'firing', 4, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    alert.id,
                    alert.dedup_key,
                    alert.device_id,
                    alert.started_at,
                    alert.last_seen_at,
                    serde_json::to_string(&alert).unwrap()
                ],
            )
        })
        .unwrap();
}

#[tokio::test]
async fn what_the_rules_knew_before_the_application_was_closed_waits_for_the_devices_first_poll() {
    // Two switches went down in last night's window and the application was
    // closed before it ended. This morning the window is long over and both
    // alerts are open, firing and unannounced — on knowledge seven hours
    // old. SW1 is still down; SW2 came back at four.
    let (harness, _clock, listener) = harness_with_a_channel().await;
    harness.only_rule("builtin.device_down").await;
    let mut devices = Vec::new();
    for (name, address) in [("SW1", "10.0.0.1"), ("SW2", "10.0.0.2")] {
        let id = harness.device(name, address, json!({})).await;
        from_last_night(
            &harness,
            Alert {
                id: format!("night-{name}"),
                rule_id: Some("builtin.device_down".into()),
                rule_name: "Device down".into(),
                device_id: Some(id.clone()),
                device: Some(name.into()),
                title: format!("Device down — {name}"),
                dedup_key: crate::rules::dedup_key("builtin.device_down", &id, None),
                ..Alert::default()
            },
        );
        harness
            .app
            .store()
            .put(
                crate::db::collections::DEVICES,
                &id,
                &DeviceRecord {
                    device_id: id.clone(),
                    failures: 30,
                    down: true,
                    ..DeviceRecord::default()
                },
            )
            .unwrap();
        devices.push(id);
    }
    // And one a log rule raised in the same window. Nothing will ever look
    // at that again, so there is nothing to wait for.
    from_last_night(
        &harness,
        Alert {
            id: "night-log".into(),
            rule_name: "OSPF neighbour lost".into(),
            title: "OSPF neighbour lost".into(),
            dedup_key: "logs:ospf:SW1:10.0.0.9".into(),
            ..Alert::default()
        },
    );
    harness.probe.down("10.0.0.1");

    assert_eq!(pass(&harness), ["logs:ospf:SW1:10.0.0.9"]);
    assert_eq!(listener.events(1).await, ["firing"]);
    assert_eq!(listener.requests()[0].1["alert"]["id"], "night-log");

    // The first polls of the morning. SW1 is sighted again, which is what
    // announces it; SW2 answers, and its outage ends without having woken
    // anybody for something that was over hours ago.
    for id in &devices {
        harness.poll_and_evaluate(id).await;
    }
    assert_eq!(listener.events(2).await, ["firing", "firing"]);
    assert_eq!(listener.requests()[1].1["alert"]["id"], "night-SW1");
    assert_eq!(
        crate::alerts::get(&harness.app, "night-SW2").unwrap().state,
        AlertState::Resolved
    );
    assert!(pass(&harness).is_empty());
    assert_eq!(listener.events_after_a_pause().await.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn the_scheduler_makes_the_pass_itself_with_polling_off_and_the_clock_set_back() {
    let clock = Arc::new(SteppedClock::new(crate::testing::epoch()));
    let harness = Harness::with_clock(clock.clone()).await;
    // Polling is switched off for the whole installation. The log rules,
    // the detections and the insights raise alerts all the same, and what
    // those owe is owed.
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"enabled": false}}),
        )
        .await;
    crate::scheduler::spawn(&harness.app);
    tokio::time::sleep(Duration::from_secs(5)).await;
    // The time of day is put back an hour. A pass that asked the time of
    // day whether it was due would not come round again for an hour.
    clock.step(-3_600_000);

    let opened = window(&harness, Duration::ZERO, 10 * MINUTES).await;
    let raised = raise(&harness, "detect:reload:SW1").await;
    let id = raised["id"].as_str().unwrap();
    let mut events = harness.app.subscribe();

    tokio::time::sleep(9 * MINUTES).await;
    assert_eq!(
        crate::alerts::get(&harness.app, id).unwrap().notified_at,
        None
    );
    tokio::time::sleep(2 * MINUTES).await;
    let told = crate::alerts::get(&harness.app, id)
        .unwrap()
        .notified_at
        .expect("the scheduler announced it");
    let late_by = told - opened["ends_at"].as_i64().unwrap();
    assert!(
        (0..=16_000).contains(&late_by),
        "announced {late_by} ms after the window closed"
    );
    assert_eq!(notifications(&mut events), [json!("firing")], "once");
}

#[tokio::test(start_paused = true)]
async fn one_pass_looks_at_a_bounded_number_of_alerts_and_the_next_carries_on_from_there() {
    // A lab that has been broken thoroughly: 450 alerts that owe a notice
    // for as long as `notify_for_lab` is off, which may be for ever. They
    // must not be read, all of them, every few seconds — and they must not
    // stand in front of the one alert that does become due.
    let harness = Harness::new().await;
    let lab = harness
        .device(
            "LAB-R1",
            "",
            json!({"lab": {"lab_id": "lab1", "node": "R1"}}),
        )
        .await;
    for index in 0..450 {
        harness
            .rpc(
                "alerts.raise",
                json!({"device_id": lab, "severity": "low", "title": "Broken on purpose",
                       "dedup_key": format!("lab:{index}")}),
            )
            .await;
    }
    // Newer than all of them, so last in the order the pass reads in.
    tokio::time::sleep(Duration::from_secs(1)).await;
    window(&harness, Duration::ZERO, 10 * MINUTES).await;
    raise(&harness, "hardware").await;
    tokio::time::sleep(11 * MINUTES).await;

    assert!(pass(&harness).is_empty(), "the first 200");
    assert!(pass(&harness).is_empty(), "the next 200");
    assert_eq!(pass(&harness), ["hardware"], "the last 51, and the one due");

    // The list has been gone through, so the next pass starts over with the
    // oldest — and now that lab alerts may be announced, each is, once.
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"notify_for_lab": true}}),
        )
        .await;
    assert_eq!(pass(&harness).len(), 200);
    assert_eq!(pass(&harness).len(), 200);
    assert_eq!(pass(&harness).len(), 50);
    assert!(pass(&harness).is_empty(), "each of them once");
}
