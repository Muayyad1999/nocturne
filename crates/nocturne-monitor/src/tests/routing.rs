//! What the other features are written against: `alerts.raise` with a say
//! in who is told, `alerts.clear` to take an alert back when its condition is
//! over, and the one rule that keeps a lab's alerts from every channel.
//!
//! The channels are webhooks whose other end is a listener the test starts
//! on loopback, which records what it was sent.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use super::notifications::{Listener, MINUTES, harness_with_a_channel, listen, window};
use crate::model::AlertState;
use crate::testing::{FakeProbe, FakeSnmp, Harness, ManualClock};

/// A webhook channel and the listener behind it.
async fn channel(harness: &Harness, name: &str) -> (String, Listener) {
    let listener = listen(vec![]).await;
    let saved = harness
        .rpc(
            "notify.channels.save",
            json!({"name": name, "kind": "webhook", "url": listener.url}),
        )
        .await;
    (saved["id"].as_str().expect("an id").to_string(), listener)
}

/// The id of the one channel [`harness_with_a_channel`] made.
async fn the_channel(harness: &Harness) -> String {
    let channels = harness.rpc("notify.channels.list", json!({})).await;
    channels[0]["id"].as_str().expect("an id").to_string()
}

/// The `notification` of every `alerts.changed` published since the last
/// call, by dedup key.
fn notifications(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
) -> Vec<(String, Value)> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic == "alerts.changed" {
            seen.push((
                event.payload["alert"]["dedup_key"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                event.payload["notification"].clone(),
            ));
        }
    }
    seen
}

fn audit_entries(harness: &Harness, action: &str) -> Vec<nocturne_app::store::LogEntry> {
    harness
        .app
        .store()
        .log_query(&nocturne_app::store::LogQuery {
            stream: Some("audit".into()),
            limit: Some(200),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .filter(|entry| entry.source == action)
        .collect()
}

// ------------------------------------------------------ alerts.raise: notify

#[tokio::test]
async fn an_alert_raised_for_named_channels_reaches_those_and_no_others() {
    let harness = Harness::new().await;
    let (pager_id, pager) = channel(&harness, "pager").await;
    let (_, chat) = channel(&harness, "chat").await;

    // Named twice, told once.
    let raised = harness
        .rpc(
            "alerts.raise",
            json!({"rule": "Port security", "severity": "high", "title": "Port security violation",
                   "dedup_key": "logs:r1:d1:Gi0/3", "notify": [pager_id, pager_id]}),
        )
        .await;
    assert_eq!(raised["notify"], json!([pager_id]));
    assert_eq!(pager.events(1).await, ["firing"]);

    // Its end goes to the people who heard its beginning, although the call
    // that ends it names no channel.
    let cleared = harness
        .rpc("alerts.clear", json!({"dedup_key": "logs:r1:d1:Gi0/3"}))
        .await;
    assert_eq!(cleared, json!({"cleared": true}));
    assert_eq!(pager.events(2).await, ["firing", "resolved"]);
    assert!(
        chat.events_after_a_pause().await.is_empty(),
        "nobody named the chat"
    );

    // Left out, or null, is the usual routing: every enabled channel.
    for (key, notify) in [("k2", None), ("k3", Some(Value::Null))] {
        let mut params = json!({"severity": "high", "title": "Something", "dedup_key": key});
        if let Some(notify) = notify {
            params["notify"] = notify;
        }
        let usual = harness.rpc("alerts.raise", params).await;
        assert!(usual["notify"].is_null(), "{usual}");
    }
    assert_eq!(chat.events(2).await, ["firing", "firing"]);
    assert_eq!(pager.events(4).await.len(), 4);
}

#[tokio::test]
async fn an_alert_raised_for_nobody_is_kept_and_never_announced() {
    let (harness, clock, listener) = harness_with_a_channel().await;
    let mut events = harness.app.subscribe();
    let raise = json!({"severity": "critical", "title": "Something", "dedup_key": "k1",
                       "notify": []});

    let raised = harness.rpc("alerts.raise", raise.clone()).await;
    assert_eq!(raised["notify"], json!([]));
    assert_eq!(raised["state"], "firing");
    assert!(raised["notified_at"].is_null());
    assert_eq!(harness.open_alerts().len(), 1, "raised, listed and shown");

    // Not at the next sighting, and not when the scheduler comes by for
    // what is still owed: nothing is owed.
    clock.advance(MINUTES);
    assert_eq!(harness.rpc("alerts.raise", raise).await["count"], 2);
    assert!(
        crate::alerts::announce_owed(&harness.app, harness.now())
            .unwrap()
            .is_empty()
    );
    // Nor when it ends: a "resolved" would be the first anybody heard.
    let cleared = harness
        .rpc("alerts.clear", json!({"dedup_key": "k1"}))
        .await;
    assert_eq!(cleared, json!({"cleared": true}));
    assert!(listener.events_after_a_pause().await.is_empty());
    assert_eq!(
        notifications(&mut events),
        [
            ("k1".to_string(), Value::Null),
            ("k1".to_string(), Value::Null),
            ("k1".to_string(), Value::Null)
        ],
        "and the desktop shell, which goes by this field, shows nothing either"
    );
}

#[tokio::test]
async fn a_channel_that_does_not_exist_is_refused_rather_than_ignored() {
    let (harness, _clock, listener) = harness_with_a_channel().await;
    let real = the_channel(&harness).await;
    for notify in [json!(["nope"]), json!([real, "nope"])] {
        let error = harness
            .app
            .rpc(
                "alerts.raise",
                json!({"severity": "high", "title": "Something", "dedup_key": "k1",
                       "notify": notify}),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "not_found");
        assert!(error.message.contains("`nope`"), "{error}");
    }
    assert!(
        harness.all_alerts().is_empty(),
        "an alert that would have reached fewer people than its caller meant is not raised"
    );
    assert!(listener.events_after_a_pause().await.is_empty());
}

#[tokio::test]
async fn who_is_told_follows_the_latest_raise_until_somebody_has_been() {
    let (harness, clock, first) = harness_with_a_channel().await;
    let first_id = the_channel(&harness).await;
    let (second_id, second) = channel(&harness, "second").await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    let raise = |notify: Value| {
        harness.rpc(
            "alerts.raise",
            json!({"severity": "high", "title": "Something", "dedup_key": "k1",
                   "notify": notify}),
        )
    };

    // Raised twice inside the window, for different people.
    raise(json!([first_id])).await;
    let seen = raise(json!([second_id])).await;
    assert_eq!(seen["notify"], json!([second_id]));

    clock.advance(61 * MINUTES);
    let announced = crate::alerts::announce_owed(&harness.app, harness.now()).unwrap();
    assert_eq!(announced.len(), 1);
    assert_eq!(second.events(1).await, ["firing"]);

    // Once it has been announced the audience stays: the people who were
    // told are the ones who must hear that it is over.
    let later = raise(json!([first_id])).await;
    assert_eq!(later["count"], 3);
    assert_eq!(later["notify"], json!([second_id]));
    harness
        .rpc("alerts.clear", json!({"dedup_key": "k1"}))
        .await;
    assert_eq!(second.events(2).await, ["firing", "resolved"]);
    assert!(first.events_after_a_pause().await.is_empty());
}

// -------------------------------------------------------------- alerts.clear

#[tokio::test]
async fn clearing_by_key_closes_what_another_feature_raised_and_says_whether_it_did() {
    let (harness, clock, listener) = harness_with_a_channel().await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    let key = "logs:builtin.ios.ospf:SW1:10.0.0.2 on Gi0/1";
    let raise = json!({"rule": "OSPF neighbour lost", "device_id": device, "severity": "high",
                       "title": "OSPF neighbour 10.0.0.2 lost on Gi0/1", "dedup_key": key});
    let raised = harness.rpc("alerts.raise", raise.clone()).await;
    assert_eq!(listener.events(1).await, ["firing"]);

    clock.advance(5 * MINUTES);
    let answer = harness
        .rpc(
            "alerts.clear",
            json!({"dedup_key": key, "note": "the opposite event arrived"}),
        )
        .await;
    assert_eq!(answer, json!({"cleared": true}));
    let closed = crate::alerts::get(&harness.app, raised["id"].as_str().unwrap()).unwrap();
    assert_eq!(closed.state, AlertState::Resolved);
    assert_eq!(closed.resolved_at, Some(harness.now()));
    assert_eq!(closed.detail, "the opposite event arrived");
    assert_eq!(listener.events(2).await, ["firing", "resolved"]);

    // Nothing open under the key any more, and nothing ever was under
    // another: `false`, and no error.
    for nothing_open in [key, "logs:never:raised"] {
        assert_eq!(
            harness
                .rpc("alerts.clear", json!({"dedup_key": nothing_open}))
                .await,
            json!({"cleared": false})
        );
    }

    // An alert somebody acknowledged is still open, and a condition that
    // is over closes it all the same.
    let again = harness.rpc("alerts.raise", raise).await;
    harness.rpc("alerts.ack", json!({"id": again["id"]})).await;
    assert_eq!(
        harness.rpc("alerts.clear", json!({"dedup_key": key})).await,
        json!({"cleared": true})
    );
    assert!(harness.open_alerts().is_empty());

    // Each clearing that closed something is in the audit trail — and
    // nothing in the entry is text a device could have written: not the
    // title, not the key, not the note.
    let audited = audit_entries(&harness, "alert.clear");
    assert_eq!(audited.len(), 2, "one per alert closed, none for `false`");
    for entry in &audited {
        assert_eq!(entry.device_id.as_deref(), Some(device.as_str()));
        let body = entry.body.clone().unwrap_or_default();
        assert_eq!(body["severity"], "high");
        let written = format!("{} {body}", entry.message);
        assert!(written.contains("OSPF neighbour lost"), "{written}");
        for captured in ["10.0.0.2", "Gi0/1", "opposite"] {
            assert!(!written.contains(captured), "{written}");
        }
    }
    let ids: Vec<&str> = audited
        .iter()
        .filter_map(|entry| entry.body.as_ref()?["alert_id"].as_str())
        .collect();
    assert!(
        ids.contains(&raised["id"].as_str().unwrap())
            && ids.contains(&again["id"].as_str().unwrap())
    );

    let error = harness
        .app
        .rpc("alerts.clear", json!({"dedup_key": "  "}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
}

#[tokio::test(start_paused = true)]
async fn an_alert_a_rule_owns_cannot_be_cleared_by_naming_its_key() {
    // The device is down and the rule says so. A feature that could close
    // that alert by passing its key would hide an outage the rule still
    // sees, until the rule's next sighting raised it again as a new one.
    let harness = Harness::new().await;
    harness.only_rule("builtin.device_down").await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.down("10.0.0.1");
    for _ in 0..2 {
        harness.poll_and_evaluate(&device).await;
        tokio::time::sleep(MINUTES).await;
    }
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].rule_id.as_deref(), Some("builtin.device_down"));

    let error = harness
        .app
        .rpc(
            "alerts.clear",
            json!({"dedup_key": open[0].dedup_key, "note": "all quiet"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "forbidden");
    assert!(error.message.contains("Device down"), "{error}");

    let still = crate::alerts::get(&harness.app, &open[0].id).unwrap();
    assert_eq!(still.state, AlertState::Firing);
    assert_ne!(still.detail, "all quiet");
    assert!(audit_entries(&harness, "alert.clear").is_empty());

    // The method looks first, to choose that answer, and a rule can raise
    // its alert between that look and the resolve. So what resolves refuses
    // a rule's alert by itself, whatever the look before it saw.
    let closed =
        crate::alerts::clear_raised(&harness.app, &open[0].dedup_key, harness.now(), None).unwrap();
    assert_eq!(closed, None);
    assert_eq!(
        crate::alerts::get(&harness.app, &open[0].id).unwrap(),
        still
    );
}

#[tokio::test]
async fn an_alert_cleared_and_raised_again_is_a_new_alert_with_its_own_announcement() {
    // A neighbour lost, found and lost again is two outages: the second is
    // not the first one reopened, and what was done to the first — a
    // snooze, an acknowledgement — does not keep the second one quiet.
    let (harness, clock, listener) = harness_with_a_channel().await;
    let key = "logs:builtin.ios.ospf:SW1:10.0.0.2";
    let raise = json!({"rule": "OSPF neighbour lost", "severity": "high",
                       "title": "OSPF neighbour lost", "dedup_key": key});

    let first = harness.rpc("alerts.raise", raise.clone()).await;
    assert_eq!(listener.events(1).await, ["firing"]);
    harness
        .rpc("alerts.snooze", json!({"id": first["id"], "minutes": 60}))
        .await;
    // Its end is told to the people who were told it began, snoozed or not.
    assert_eq!(
        harness.rpc("alerts.clear", json!({"dedup_key": key})).await,
        json!({"cleared": true})
    );
    assert_eq!(listener.events(2).await, ["firing", "resolved"]);

    clock.advance(MINUTES);
    let second = harness.rpc("alerts.raise", raise.clone()).await;
    assert_ne!(second["id"], first["id"]);
    assert_eq!(second["count"], 1);
    assert_eq!(second["started_at"], json!(harness.now()));
    assert!(second["snoozed_until"].is_null());
    assert_eq!(second["notified_at"], json!(harness.now()));
    assert_eq!(
        listener.events(3).await,
        ["firing", "resolved", "firing"],
        "a new firing, announced as one"
    );

    // Taken on by somebody, cleared, and lost a third time.
    harness.rpc("alerts.ack", json!({"id": second["id"]})).await;
    harness.rpc("alerts.clear", json!({"dedup_key": key})).await;
    clock.advance(MINUTES);
    let third = harness.rpc("alerts.raise", raise).await;
    assert_eq!(third["state"], "firing");
    assert!(third["acked_at"].is_null());
    assert_eq!(
        listener.events(5).await,
        ["firing", "resolved", "firing", "resolved", "firing"]
    );
    assert_eq!(harness.all_alerts().len(), 3);
    assert_eq!(harness.open_alerts().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_clear_that_lands_among_an_acknowledgement_and_a_snooze_still_closes_the_alert() {
    // Somebody clicks "acknowledge" and "snooze" at the moment the log rule
    // hears the neighbour come back. Whatever order the three are taken in,
    // the alert ends closed: a click that wrote back the copy it had read
    // would reopen an alert the clear had just resolved, with nothing left
    // that would ever close it. Several rounds, as in the test below.
    let harness = Harness::new().await;
    const ROUNDS: usize = 40;
    for round in 0..ROUNDS {
        let key = format!("logs:builtin.ios.ospf:SW1:{round}");
        let raised = harness
            .rpc(
                "alerts.raise",
                json!({"severity": "high", "title": "OSPF neighbour lost", "dedup_key": key}),
            )
            .await;
        let id = raised["id"].as_str().unwrap().to_string();
        let gate = Arc::new(tokio::sync::Barrier::new(3));
        let mut callers = tokio::task::JoinSet::new();
        for (method, params) in [
            ("alerts.clear", json!({"dedup_key": key})),
            ("alerts.ack", json!({"id": id, "note": "looking"})),
            ("alerts.snooze", json!({"id": id, "minutes": 30})),
        ] {
            let (app, gate) = (harness.app.clone(), gate.clone());
            callers.spawn(async move {
                gate.wait().await;
                (method, app.rpc(method, params).await)
            });
        }
        while let Some(done) = callers.join_next().await {
            let (method, answer) = done.expect("no caller panicked");
            match (method, answer) {
                ("alerts.clear", answer) => assert_eq!(
                    answer.expect("the clear is answered"),
                    json!({"cleared": true}),
                    "the alert was open, acknowledged or not"
                ),
                // An acknowledgement that came second is told so.
                ("alerts.ack", Err(late)) => assert_eq!(late.code, "conflict"),
                _ => {}
            }
        }
        let after = crate::alerts::get(&harness.app, &id).unwrap();
        assert_eq!(after.state, AlertState::Resolved, "round {round}");
        assert!(after.resolved_at.is_some());
    }
    assert!(harness.open_alerts().is_empty());
}

#[tokio::test(start_paused = true)]
async fn raising_under_a_rules_key_cannot_route_the_rules_notice_to_nobody() {
    // The other way round a rule's ownership: its alert is held back by a
    // window, and a caller raises under its key "for nobody". Were that
    // taken, the outage would stay unannounced after the window too.
    let harness = Harness::new().await;
    harness.only_rule("builtin.device_down").await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    harness.probe.down("10.0.0.1");
    for _ in 0..2 {
        harness.poll_and_evaluate(&device).await;
        tokio::time::sleep(MINUTES).await;
    }
    let open = harness.open_alerts()[0].clone();
    assert_eq!(open.notified_at, None);

    let seen = harness
        .rpc(
            "alerts.raise",
            json!({"severity": "critical", "title": "Device down", "dedup_key": open.dedup_key,
                   "notify": []}),
        )
        .await;
    assert_eq!(seen["id"], json!(open.id));
    assert!(
        seen["notify"].is_null(),
        "the rule's alert keeps the rule's routing"
    );

    tokio::time::sleep(60 * MINUTES).await;
    harness.poll_and_evaluate(&device).await;
    assert_eq!(
        crate::alerts::get(&harness.app, &open.id)
            .unwrap()
            .notified_at,
        Some(harness.now())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn raises_and_clears_of_one_key_at_once_tell_one_story() {
    // A log rule's neighbour flaps: lost, back, lost again, with the calls
    // landing on top of each other. Whatever order they are taken in, each
    // must be whole — a clear that read the alert, changed its copy and
    // wrote it back would undo a sighting counted in between, and two
    // clears that both found the alert open would both say they closed it.
    //
    // Several rounds, for the reason the other concurrency tests give: how
    // far the callers overlap is up to the machine. With the clear made
    // read-then-write, twelve rounds of eight clearers still passed one run
    // in ten on a loaded machine.
    let harness = Harness::new().await;
    const RAISERS: usize = 24;
    const CLEARERS: usize = 12;
    const ROUNDS: usize = 24;
    let key = "logs:builtin.ios.ospf:SW1:10.0.0.2";
    let mut said_cleared = 0usize;
    for _ in 0..ROUNDS {
        let gate = Arc::new(tokio::sync::Barrier::new(RAISERS + CLEARERS));
        let mut callers = tokio::task::JoinSet::new();
        for _ in 0..RAISERS {
            let (app, gate) = (harness.app.clone(), gate.clone());
            callers.spawn(async move {
                gate.wait().await;
                app.rpc(
                    "alerts.raise",
                    json!({"severity": "high", "title": "OSPF neighbour lost", "dedup_key": key}),
                )
                .await
                .expect("the alert is raised");
                false
            });
        }
        for _ in 0..CLEARERS {
            let (app, gate) = (harness.app.clone(), gate.clone());
            callers.spawn(async move {
                gate.wait().await;
                let answer = app
                    .rpc("alerts.clear", json!({"dedup_key": key}))
                    .await
                    .expect("the clear is answered");
                answer["cleared"] == json!(true)
            });
        }
        while let Some(done) = callers.join_next().await {
            if done.expect("no caller panicked") {
                said_cleared += 1;
            }
        }
    }

    let alerts = harness.all_alerts();
    let open = alerts.iter().filter(|alert| alert.state.is_open()).count();
    assert!(open <= 1, "{open} alerts open under one key");
    assert_eq!(
        alerts.len() - open,
        said_cleared,
        "every `cleared: true` closed one alert, and every alert closed was answered for once"
    );
    assert_eq!(
        alerts.iter().map(|alert| alert.count).sum::<u64>(),
        (ROUNDS * RAISERS) as u64,
        "every raise is a sighting of exactly one alert: none lost to a clear, none counted twice"
    );
    for alert in alerts.iter().filter(|alert| !alert.state.is_open()) {
        assert!(alert.resolved_at.is_some(), "a closed alert says when");
    }
}

// ------------------------------------------------------------- lab devices

#[tokio::test]
async fn an_alert_about_a_lab_device_reaches_no_channel_unless_the_setting_allows_it() {
    let (harness, clock, listener) = harness_with_a_channel().await;
    let channel_id = the_channel(&harness).await;
    let lab = harness
        .device(
            "LAB-R1",
            "",
            json!({"lab": {"lab_id": "lab1", "node": "R1"}}),
        )
        .await;
    // Reached through the simulator without a lab having made it.
    let simulated = harness
        .device("SIM-R2", "", json!({"transport": "sim"}))
        .await;
    let hardware = harness.device("SW1", "10.0.0.1", json!({})).await;
    let mut events = harness.app.subscribe();
    let raise = |device: &str, key: &str, notify: Option<Value>| {
        let mut params = json!({"rule": "Detection: unplanned reload", "device_id": device,
                                "severity": "critical", "title": "Restarted with no change behind it",
                                "dedup_key": key});
        if let Some(notify) = notify {
            params["notify"] = notify;
        }
        harness.rpc("alerts.raise", params)
    };

    // Raised, listed and shown like any other — and told to nobody.
    let quiet = raise(&lab, "lab:plain", None).await;
    assert_eq!(quiet["lab"], true);
    assert_eq!(quiet["state"], "firing");
    assert!(quiet["notified_at"].is_null());
    // Naming a channel does not get round it.
    let named = raise(&lab, "lab:named", Some(json!([channel_id]))).await;
    assert!(named["notified_at"].is_null());
    let simulated_alert = raise(&simulated, "sim:plain", None).await;
    assert_eq!(simulated_alert["lab"], true);
    assert!(simulated_alert["notified_at"].is_null());
    assert_eq!(harness.open_alerts().len(), 3);
    // Not at another sighting, nor when the scheduler comes by.
    clock.advance(MINUTES);
    raise(&lab, "lab:plain", None).await;
    assert!(
        crate::alerts::announce_owed(&harness.app, harness.now())
            .unwrap()
            .is_empty()
    );
    assert!(listener.events_after_a_pause().await.is_empty());
    assert!(
        notifications(&mut events)
            .iter()
            .all(|(_, notification)| notification.is_null()),
        "the event the desktop shell goes by carries no notice for a lab alert"
    );

    // Hardware is told about as ever.
    raise(&hardware, "hw:plain", None).await;
    assert_eq!(listener.events(1).await, ["firing"]);
    assert_eq!(listener.requests()[0].1["alert"]["device"], "SW1");
    assert_eq!(
        notifications(&mut events),
        [("hw:plain".to_string(), json!("firing"))]
    );

    // Switched on: what was being held back is announced at the next look,
    // once each, and a new lab alert as it is raised.
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"notify_for_lab": true}}),
        )
        .await;
    let seen = raise(&lab, "lab:plain", None).await;
    assert_eq!(seen["notified_at"], json!(harness.now()));
    let mut owed: Vec<String> = crate::alerts::announce_owed(&harness.app, harness.now())
        .unwrap()
        .into_iter()
        .map(|alert| alert.dedup_key)
        .collect();
    owed.sort();
    assert_eq!(owed, ["lab:named", "sim:plain"]);
    let fresh = raise(&lab, "lab:fresh", None).await;
    assert_eq!(fresh["notified_at"], json!(harness.now()));
    assert_eq!(listener.events(5).await.len(), 5);
    let mut told: Vec<Value> = notifications(&mut events)
        .into_iter()
        .map(|(key, notification)| json!([key, notification]))
        .collect();
    told.sort_by_key(Value::to_string);
    assert_eq!(
        told,
        [
            json!(["lab:fresh", "firing"]),
            json!(["lab:named", "firing"]),
            json!(["lab:plain", "firing"]),
            json!(["sim:plain", "firing"])
        ]
    );

    // Switched off again. An alert that was announced is still closed for
    // the people who were told; a new one is quiet again.
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"notify_for_lab": false}}),
        )
        .await;
    harness
        .rpc("alerts.clear", json!({"dedup_key": "lab:plain"}))
        .await;
    assert_eq!(listener.events(6).await.last().unwrap(), "resolved");
    let quiet_again = raise(&lab, "lab:later", None).await;
    assert!(quiet_again["notified_at"].is_null());
    assert_eq!(listener.events_after_a_pause().await.len(), 6);
}

#[tokio::test]
async fn nothing_that_ends_or_repeats_gets_a_lab_alert_past_the_setting() {
    // The ways an alert that was held back gets announced after all — a
    // window closing, a snooze running out, another sighting, the
    // scheduler's pass — and the notice that follows an alert out. None of
    // them may be a way round `notify_for_lab`.
    let (harness, clock, listener) = harness_with_a_channel().await;
    let channel_id = the_channel(&harness).await;
    let lab = harness
        .device(
            "LAB-R1",
            "",
            json!({"lab": {"lab_id": "lab1", "node": "R1"}}),
        )
        .await;
    let hardware = harness.device("SW1", "10.0.0.1", json!({})).await;
    let mut events = harness.app.subscribe();
    let raise = |device: Option<&str>, key: &str, notify: Option<Value>| {
        let mut params = json!({"rule": "Detection: unplanned reload", "device_id": device,
                                "severity": "critical", "title": "Restarted with no change behind it",
                                "dedup_key": key});
        if let Some(notify) = notify {
            params["notify"] = notify;
        }
        harness.rpc("alerts.raise", params)
    };

    // Raised inside a window; raised and snoozed; raised and left alone.
    window(&harness, Duration::ZERO, 60 * MINUTES).await;
    raise(Some(lab.as_str()), "lab:window", None).await;
    let snoozed = raise(Some(lab.as_str()), "lab:snoozed", Some(json!([channel_id]))).await;
    harness
        .rpc("alerts.snooze", json!({"id": snoozed["id"], "minutes": 30}))
        .await;
    let plain = raise(Some(lab.as_str()), "lab:plain", None).await;

    // The window closes and the snooze runs out, and the pass comes by.
    clock.advance(61 * MINUTES);
    assert!(
        crate::alerts::announce_owed(&harness.app, harness.now())
            .unwrap()
            .is_empty()
    );

    // Seen again on the lab device itself, with no window left to hold it.
    let again = raise(Some(lab.as_str()), "lab:window", None).await;
    assert_eq!(again["count"], 2);
    assert_eq!(again["muted_by_maintenance"], false);
    assert!(again["notified_at"].is_null(), "{again}");

    // The same key again from a caller that leaves the device out, names
    // another one, or names a channel: a sighting of the lab's alert, and
    // not a reason to tell anybody about it.
    for (device, notify) in [
        (None, None),
        (None, Some(json!([channel_id]))),
        (Some(hardware.as_str()), None),
    ] {
        let seen = raise(device, "lab:plain", notify).await;
        assert_eq!(seen["id"], plain["id"]);
        assert_eq!(seen["lab"], true);
        assert!(seen["notified_at"].is_null(), "{seen}");
    }
    assert_eq!(
        crate::alerts::get(&harness.app, plain["id"].as_str().unwrap())
            .unwrap()
            .count,
        4
    );
    clock.advance(MINUTES);
    assert!(
        crate::alerts::announce_owed(&harness.app, harness.now())
            .unwrap()
            .is_empty()
    );

    // Their ends, by a clear and by hand: an alert nobody was told about
    // goes without a word as well.
    harness
        .rpc("alerts.clear", json!({"dedup_key": "lab:window"}))
        .await;
    harness
        .rpc("alerts.resolve", json!({"id": snoozed["id"]}))
        .await;
    harness
        .rpc("alerts.clear", json!({"dedup_key": "lab:plain"}))
        .await;
    assert!(harness.open_alerts().is_empty());
    assert_eq!(harness.all_alerts().len(), 3, "all of it on record");

    assert!(
        listener.events_after_a_pause().await.is_empty(),
        "no channel heard a word"
    );
    let published = notifications(&mut events);
    assert!(published.len() >= 11, "{published:?}");
    assert!(
        published
            .iter()
            .all(|(_, notification)| notification.is_null()),
        "nor did the desktop shell: {published:?}"
    );
}

#[tokio::test]
async fn a_raise_that_names_another_device_does_not_announce_what_a_window_holds_back() {
    // The window is over SW1 alone, and the alert about SW1 is held back by
    // it. A second raise under the same key that names no device (or
    // another) has asked about no window of SW1's: it is counted, and the
    // notice waits for the window to close, as it would have.
    let (harness, clock, listener) = harness_with_a_channel().await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    let other = harness.device("SW2", "10.0.0.2", json!({})).await;
    let starts_at = harness.now();
    harness
        .rpc(
            "maintenance.save",
            json!({"name": "SW1 upgrade", "scope": {"device_ids": [device]},
                   "starts_at": starts_at, "ends_at": starts_at + 3_600_000}),
        )
        .await;
    let raise = |device: Option<&str>| {
        harness.rpc(
            "alerts.raise",
            json!({"device_id": device, "severity": "high", "title": "Port security violation",
                   "dedup_key": "logs:r1:SW1:Gi0/3"}),
        )
    };
    let held = raise(Some(device.as_str())).await;
    assert_eq!(held["muted_by_maintenance"], true);

    clock.advance(MINUTES);
    for elsewhere in [None, Some(other.as_str())] {
        let seen = raise(elsewhere).await;
        assert_eq!(seen["id"], held["id"]);
        assert_eq!(seen["device_id"], json!(device), "still about SW1");
        assert_eq!(seen["muted_by_maintenance"], true);
        assert!(seen["notified_at"].is_null(), "{seen}");
    }
    assert!(
        crate::alerts::announce_owed(&harness.app, harness.now())
            .unwrap()
            .is_empty()
    );
    assert!(listener.events_after_a_pause().await.is_empty());

    // The window closes: announced, once, by the pass that asks about the
    // alert's own device.
    clock.advance(60 * MINUTES);
    let announced = crate::alerts::announce_owed(&harness.app, harness.now()).unwrap();
    assert_eq!(announced.len(), 1);
    assert_eq!(announced[0].count, 3);
    assert_eq!(listener.events(1).await, ["firing"]);
    // And a raise that names the device it is about announces as ever.
    let fresh = harness
        .rpc(
            "alerts.raise",
            json!({"device_id": device, "severity": "high", "title": "Port security violation",
                   "dedup_key": "logs:r1:SW1:Gi0/4"}),
        )
        .await;
    assert_eq!(fresh["notified_at"], json!(harness.now()));
    assert_eq!(listener.events(2).await, ["firing", "firing"]);
}

#[tokio::test]
async fn an_alert_the_rules_raise_about_a_lab_device_is_kept_quiet_the_same_way() {
    // The rules alert on a lab device only when one opts in — and then the
    // alert is on the screen, which is what a lab is for, and in no channel.
    let clock = Arc::new(ManualClock::new(crate::testing::epoch()));
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(crate::state::MonitorState::fake(
        clock.clone(),
        Arc::new(FakeProbe::default()),
        Arc::new(FakeSnmp::default()),
    ));
    let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
    let id = lab.id("SW1");
    let listener = listen(vec![]).await;
    let channel = lab
        .app
        .rpc(
            "notify.channels.save",
            json!({"name": "hook", "kind": "webhook", "url": listener.url}),
        )
        .await
        .unwrap();
    for rule in crate::rules::list(&lab.app).unwrap() {
        // The rule names the channel itself, which is no way round either.
        lab.app
            .rpc(
                "alerts.rules.save",
                json!({"id": rule.id, "enabled": rule.id == "builtin.uplink_down",
                       "condition": {"type": "interface_down", "only": "all"},
                       "scope": {"include_lab": true}, "notify": [channel["id"]]}),
            )
            .await
            .unwrap();
    }
    lab.fake("SW1").with(|state| {
        state.link_down.push("GigabitEthernet0/1".into());
    });
    let look = || async {
        let poll = crate::poll::poll_device(&lab.app, &id).await.unwrap();
        crate::rules::evaluate(&lab.app, &poll).await;
        crate::alerts::list(&lab.app, &Default::default()).unwrap()
    };

    let alerts = look().await;
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].lab);
    assert_eq!(alerts[0].notified_at, None);
    assert!(listener.events_after_a_pause().await.is_empty());

    lab.app
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"notify_for_lab": true}}),
        )
        .await
        .unwrap();
    // Lab devices are read over the CLI every 30 s; let that come round.
    clock.advance(MINUTES);
    let alerts = look().await;
    assert_eq!(alerts[0].notified_at, Some(clock_now(&clock)));
    assert_eq!(listener.events(1).await, ["firing"]);
}

fn clock_now(clock: &ManualClock) -> i64 {
    use crate::clock::Clock;
    clock.now_ms()
}
