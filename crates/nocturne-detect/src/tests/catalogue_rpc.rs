//! `detections.list`, `detections.set_enabled`, `detections.recent`, and the
//! `detect` settings section.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use nocturne_app::App;

use super::support::*;
use crate::catalogue::{
    FAILED_UNLOCKS, HOST_KEY_CHANGED, SECRETS_CHANGED, UNPLANNED_CONFIG_CHANGE,
    UNREDACTED_CONFIG_VIEWS,
};

fn said_so(app: &App, device_id: &str, at: i64) {
    let entry_id = syslog_line(app, device_id, "syslog", at);
    app.publish(
        "logs.config_changed",
        &json!({"device_id": device_id, "by": "admin", "from": "console", "entry_id": entry_id, "at": at}),
    );
}

fn key_mismatch(app: &App, host: &str) {
    app.publish(
        "connect.host_key_changed",
        &json!({
            "host": host, "port": 22,
            "known": {"algorithm": "ssh-ed25519", "fingerprint": "SHA256:old", "first_seen": 1},
            "presented": {"algorithm": "ssh-ed25519", "fingerprint": "SHA256:new", "openssh": "ssh-ed25519 AAAA"},
        }),
    );
}

async fn changed_events(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
) -> Vec<Value> {
    let mut changed = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic == "detections.changed" {
            changed.push(event.payload);
        }
    }
    changed
}

#[tokio::test]
async fn the_settings_section_is_declared_with_the_specified_defaults() {
    let world = world(Build::FULL).await;
    assert_eq!(
        world
            .app
            .rpc("settings.get", json!({"section": "detect"}))
            .await
            .unwrap(),
        json!({
            "enabled": true,
            "grace_minutes": 15,
            "unredacted_views": {"count": 5, "window_min": 10},
            "failed_unlocks": {"count": 5, "window_min": 5},
            "disabled": [],
        })
    );
}

#[tokio::test]
async fn the_list_is_the_fixed_catalogue_and_says_what_each_is_decided_from() {
    let world = world(Build::FULL).await;
    let list = world.app.rpc("detections.list", Value::Null).await.unwrap();
    let list = list.as_array().unwrap();

    let ids: Vec<&str> = list
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "unplanned_config_change",
            "unplanned_reload",
            "host_key_changed",
            "unredacted_config_views",
            "secrets_changed",
            "failed_unlocks",
            "protected_change_outside_window",
        ]
    );
    let severities: Vec<&str> = list
        .iter()
        .map(|entry| entry["severity"].as_str().unwrap())
        .collect();
    assert_eq!(
        severities,
        [
            "high", "medium", "critical", "medium", "info", "high", "info"
        ]
    );
    for entry in list {
        for field in ["name", "description", "decided_from", "cannot_see"] {
            assert!(
                !entry[field].as_str().unwrap().trim().is_empty(),
                "{field} of {}",
                entry["id"]
            );
        }
        assert_eq!(entry["enabled"], true, "{}", entry["id"]);
        assert_eq!(entry["available"], true, "{}", entry["id"]);
        assert_eq!(entry.get("unavailable_reason"), None, "{}", entry["id"]);
        assert_eq!(entry.get("last_error"), None, "{}", entry["id"]);
    }
    // Nothing has fired: making the lab's own credential is not a hit.
    for entry in list {
        assert_eq!(entry["hits_24h"], 0, "{}", entry["id"]);
        assert_eq!(entry.get("last_hit_at"), None, "{}", entry["id"]);
    }

    // And one that has fired says so, and when.
    let app = &world.app;
    app.rpc(
        "credentials.update",
        json!({"id": world.lab.credential_id, "password": "another-Secret!"}),
    )
    .await
    .unwrap();
    settle(app).await;
    let entry = listed(app, SECRETS_CHANGED).await;
    assert_eq!(entry["hits_24h"], 1);
    assert!(entry["last_hit_at"].is_i64());
}

#[tokio::test]
async fn a_build_with_only_the_core_lists_what_it_cannot_run_and_why() {
    let world = world(Build::CORE).await;
    let list = world.app.rpc("detections.list", Value::Null).await.unwrap();
    let available: Vec<(&str, bool)> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["id"].as_str().unwrap(),
                entry["available"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        available,
        [
            ("unplanned_config_change", false),
            ("unplanned_reload", false),
            ("host_key_changed", true),
            ("unredacted_config_views", false),
            ("secrets_changed", true),
            ("failed_unlocks", true),
            ("protected_change_outside_window", false),
        ]
    );
    for entry in list.as_array().unwrap() {
        assert_eq!(
            entry["unavailable_reason"].is_string(),
            entry["available"] == false,
            "{entry}"
        );
    }
}

#[tokio::test]
async fn switching_a_detection_off_is_audited_announced_kept_and_silences_it() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let mut events = app.subscribe();

    let entry = app
        .rpc(
            "detections.set_enabled",
            json!({"id": UNPLANNED_CONFIG_CHANGE, "enabled": false}),
        )
        .await
        .unwrap();
    assert_eq!(entry["id"], UNPLANNED_CONFIG_CHANGE);
    assert_eq!(entry["enabled"], false);
    assert_eq!(entry["available"], true);
    settle(app).await;

    // Announced once, however many paths noticed.
    let changed = changed_events(&mut events).await;
    assert_eq!(changed.len(), 1, "{changed:#?}");

    // Kept in the settings, where a restart finds it.
    assert_eq!(
        app.rpc("settings.get", json!({"section": "detect"}))
            .await
            .unwrap()["disabled"],
        json!([UNPLANNED_CONFIG_CHANGE])
    );
    let audit = app
        .rpc("audit.list", json!({"text": "detections.set_enabled"}))
        .await
        .unwrap();
    let audit = audit["entries"].as_array().unwrap();
    assert_eq!(audit.len(), 1, "{audit:#?}");
    assert_eq!(
        audit[0]["message"],
        "Switched the detection `Unplanned configuration change` off"
    );
    assert_eq!(
        audit[0]["body"],
        json!({"id": UNPLANNED_CONFIG_CHANGE, "enabled": false})
    );

    // Off means silent, and only that one.
    said_so(app, &sw1, now());
    key_mismatch(app, "10.0.0.1");
    settle(app).await;
    assert_eq!(
        hits_of(app, UNPLANNED_CONFIG_CHANGE).await,
        Vec::<Value>::new()
    );
    assert_eq!(hits_of(app, HOST_KEY_CHANGED).await.len(), 1);

    // On again, it fires again; saying "on" twice is one change, one entry.
    for _ in 0..2 {
        let entry = app
            .rpc(
                "detections.set_enabled",
                json!({"id": UNPLANNED_CONFIG_CHANGE, "enabled": true}),
            )
            .await
            .unwrap();
        assert_eq!(entry["enabled"], true);
    }
    said_so(app, &sw1, now());
    settle(app).await;
    assert_eq!(hits_of(app, UNPLANNED_CONFIG_CHANGE).await.len(), 1);
    let audit = app
        .rpc("audit.list", json!({"text": "detections.set_enabled"}))
        .await
        .unwrap();
    assert_eq!(audit["entries"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn set_enabled_refuses_what_is_not_a_detection_and_what_would_change_nothing() {
    let world = world(Build::FULL).await;
    let app = &world.app;

    let unknown = app
        .rpc(
            "detections.set_enabled",
            json!({"id": "new_mac_on_port", "enabled": true}),
        )
        .await
        .unwrap_err();
    assert_eq!(unknown.code, "invalid");
    assert!(
        unknown.message.contains("new_mac_on_port"),
        "{}",
        unknown.message
    );

    let missing = app
        .rpc("detections.set_enabled", json!({"id": HOST_KEY_CHANGED}))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "invalid");

    // With everything switched off as a whole, switching one on would still
    // read `enabled: false`; it says where the switch is instead.
    set(app, json!({"enabled": false})).await;
    let refused = app
        .rpc(
            "detections.set_enabled",
            json!({"id": HOST_KEY_CHANGED, "enabled": true}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "conflict");
    assert!(
        refused.message.contains("`detect` settings"),
        "{}",
        refused.message
    );
    // Switching one off is still recorded for when they come back on.
    let entry = app
        .rpc(
            "detections.set_enabled",
            json!({"id": HOST_KEY_CHANGED, "enabled": false}),
        )
        .await
        .unwrap();
    assert_eq!(entry["enabled"], false);
}

#[tokio::test]
async fn switched_off_as_a_whole_nothing_fires_and_every_entry_says_so() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let mut events = app.subscribe();

    set(app, json!({"enabled": false})).await;
    settle(app).await;
    // A switch moved in the settings, not through `set_enabled`: still
    // announced, so a screen showing the catalogue refetches it.
    assert_eq!(changed_events(&mut events).await.len(), 1);

    said_so(app, &sw1, now());
    key_mismatch(app, "10.0.0.1");
    app.rpc(
        "credentials.update",
        json!({"id": world.lab.credential_id, "password": "another-Secret!"}),
    )
    .await
    .unwrap();
    settle(app).await;

    // Three things that are each a hit when the detections are on.
    let all = recent(app, json!({})).await;
    assert_eq!(all, Vec::<Value>::new(), "{all:#?}");
    let list = app.rpc("detections.list", Value::Null).await.unwrap();
    for entry in list.as_array().unwrap() {
        assert_eq!(entry["enabled"], false, "{}", entry["id"]);
    }

    // A setting that is not a switch announces nothing.
    set(app, json!({"grace_minutes": 30})).await;
    settle(app).await;
    assert_eq!(changed_events(&mut events).await, Vec::<Value>::new());
}

#[tokio::test]
async fn recent_is_newest_first_and_filters_by_detection_device_time_and_limit() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let sw2 = world.device("SW2", "10.0.0.2").await;
    let t0 = now();

    // Stated times, a minute apart, oldest first.
    said_so(app, &sw1, t0 - 3 * MINUTE);
    said_so(app, &sw2, t0 - 2 * MINUTE);
    said_so(app, &sw1, t0 - MINUTE);
    key_mismatch(app, "10.0.0.2");
    settle(app).await;

    let at = |hits: &[Value]| -> Vec<i64> {
        hits.iter().map(|hit| hit["at"].as_i64().unwrap()).collect()
    };

    let config = recent(app, json!({"id": UNPLANNED_CONFIG_CHANGE})).await;
    assert_eq!(at(&config), [t0 - MINUTE, t0 - 2 * MINUTE, t0 - 3 * MINUTE]);

    // Everything: the three and the host key.
    let all = recent(app, Value::Null).await;
    assert_eq!(all.len(), 4, "{all:#?}");
    assert_eq!(all[0]["detection"], HOST_KEY_CHANGED);

    let of_sw2 = recent(app, json!({"device_id": sw2})).await;
    let detections: Vec<&str> = of_sw2
        .iter()
        .map(|hit| hit["detection"].as_str().unwrap())
        .collect();
    assert_eq!(detections, [HOST_KEY_CHANGED, UNPLANNED_CONFIG_CHANGE]);

    let both = recent(
        app,
        json!({"id": UNPLANNED_CONFIG_CHANGE, "device_id": sw1, "since": t0 - 90_000}),
    )
    .await;
    assert_eq!(at(&both), [t0 - MINUTE]);

    let limited = recent(app, json!({"id": UNPLANNED_CONFIG_CHANGE, "limit": 2})).await;
    assert_eq!(at(&limited), [t0 - MINUTE, t0 - 2 * MINUTE]);

    // A detection that never fired has none; that is not an error.
    assert_eq!(
        recent(app, json!({"id": UNREDACTED_CONFIG_VIEWS})).await,
        Vec::<Value>::new()
    );
    assert_eq!(
        recent(app, json!({"id": FAILED_UNLOCKS})).await,
        Vec::<Value>::new()
    );

    let unknown = app
        .rpc("detections.recent", json!({"id": "nope"}))
        .await
        .unwrap_err();
    assert_eq!(unknown.code, "invalid");
    let wrong_type = app
        .rpc("detections.recent", json!({"limit": "many"}))
        .await
        .unwrap_err();
    assert_eq!(wrong_type.code, "invalid");
}

#[tokio::test]
async fn recent_reads_past_events_that_are_not_hits_and_past_more_than_one_page() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    said_so(app, &sw1, now() - 5 * MINUTE);
    settle(app).await;
    // The `event` stream is shared with the log rules' signals and the alert
    // history. Six hundred of those after the hit, some of which even
    // mention a detection in passing, are more than one page of the store.
    for n in 0..600 {
        app.store()
            .log_append(&nocturne_app::store::LogEntry {
                id: 0,
                ts: now(),
                stream: "event".into(),
                device_id: Some(sw1.clone()),
                severity: None,
                source: "logs.interface_down".into(),
                message: format!("link {n} down; see detect.unplanned_config_change"),
                body: Some(json!({"n": n})),
            })
            .unwrap();
    }

    let hits = recent(app, json!({"id": UNPLANNED_CONFIG_CHANGE, "limit": 10})).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    // And they are not counted as hits either.
    assert_eq!(listed(app, UNPLANNED_CONFIG_CHANGE).await["hits_24h"], 1);
}

#[tokio::test]
async fn a_setting_that_cannot_be_read_takes_its_default_and_switches_nothing_back_on() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    set(app, json!({"disabled": [UNPLANNED_CONFIG_CHANGE]})).await;

    // `settings.set` stores what it is given. One value of the wrong type
    // must not make the section unreadable, or everything that was switched
    // off would quietly run again.
    set(
        app,
        json!({"grace_minutes": "a quarter of an hour", "failed_unlocks": {"count": -3}}),
    )
    .await;
    said_so(app, &sw1, now());
    settle(app).await;

    assert_eq!(
        hits_of(app, UNPLANNED_CONFIG_CHANGE).await,
        Vec::<Value>::new()
    );
    assert_eq!(listed(app, UNPLANNED_CONFIG_CHANGE).await["enabled"], false);
    assert_eq!(listed(app, HOST_KEY_CHANGED).await["enabled"], true);

    // And a count below one is one, not none: the first wrong password
    // already meets it.
    app.audit(
        "vault.wrong_password",
        None,
        "Refused a wrong password for the credential vault",
        json!({"method": "vault.unlock"}),
    );
    settle(app).await;
    let hits = hits_of(app, FAILED_UNLOCKS).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["evidence"]["count"], 1);
}

#[tokio::test]
async fn a_flood_the_judge_cannot_keep_up_with_is_said_once_and_not_once_an_event() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    // The change engine does not answer, so the judge is stuck on the first
    // event while a forged flood of them arrives.
    let held = world.fakes.changes_hold.clone().lock_owned().await;
    let mut events = app.subscribe();
    let mut changed = Vec::new();

    for n in 0..1_500 {
        said_so(app, &sw1, now());
        if n % 50 == 0 {
            // Let the listener take what was published, and keep this
            // test's own view of the bus from falling behind.
            tokio::task::yield_now().await;
            changed.extend(changed_events(&mut events).await);
        }
    }
    // The queue is full and hundreds of events were turned away. A
    // detection none of them was for says so too: it may have missed one.
    let said_so_far = async {
        while listed(app, HOST_KEY_CHANGED)
            .await
            .get("last_error")
            .is_none()
        {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(30), said_so_far)
        .await
        .expect("events that were not judged are reported");
    changed.extend(changed_events(&mut events).await);

    // Hundreds lost, one announcement: the interface refetches the list
    // once, not once for every forged line.
    assert_eq!(changed, vec![json!({})]);
    let entry = listed(app, HOST_KEY_CHANGED).await;
    assert!(
        entry["last_error"]["message"]
            .as_str()
            .unwrap()
            .contains("not judged"),
        "{entry}"
    );
    drop(held);
}

#[tokio::test]
async fn a_check_that_crashes_is_reported_and_does_not_take_the_later_ones_with_it() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    // The change engine panics where this feature calls it.
    world
        .fakes
        .changes_panic
        .store(true, std::sync::atomic::Ordering::Relaxed);
    said_so(app, &sw1, now());
    // Everything published so far has been judged — which it could not be
    // if the judge had gone down with the check.
    settle(app).await;
    assert_eq!(
        hits_of(app, UNPLANNED_CONFIG_CHANGE).await,
        Vec::<Value>::new()
    );
    let entry = listed(app, UNPLANNED_CONFIG_CHANGE).await;
    assert!(
        entry["last_error"]["message"]
            .as_str()
            .unwrap()
            .contains("stopped unexpectedly"),
        "{entry}"
    );

    // The next event is judged like any other, and clears it.
    world
        .fakes
        .changes_panic
        .store(false, std::sync::atomic::Ordering::Relaxed);
    said_so(app, &sw1, now());
    key_mismatch(app, "10.0.0.1");
    settle(app).await;
    assert_eq!(hits_of(app, UNPLANNED_CONFIG_CHANGE).await.len(), 1);
    assert_eq!(hits_of(app, HOST_KEY_CHANGED).await.len(), 1);
    assert_eq!(
        listed(app, UNPLANNED_CONFIG_CHANGE).await.get("last_error"),
        None
    );
}
