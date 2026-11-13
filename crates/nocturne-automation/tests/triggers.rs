//! Triggers: an event topic, debounced, with the same allow-list a schedule
//! has and the same rule about not starting twice.

mod common;

use common::{Scans, lab_at, tick, tick_until};
use nocturne_automation::state::AutomationState;
use serde_json::{Value, json};

const MONDAY: i64 = 1_718_013_600_000;

fn scans(lab: &nocturne_app::testing::Lab) -> u32 {
    *lab.app.state::<Scans>().0.lock().expect("lock")
}

async fn save(lab: &nocturne_app::testing::Lab, extra: Value) -> Value {
    let mut params = json!({
        "name": "On an alert",
        "topic": "alerts.raised",
        "debounce_s": 2,
        "action": {"method": "insights.scan", "params": {}},
    });
    for (key, value) in extra.as_object().expect("an object") {
        params[key] = value.clone();
    }
    lab.app
        .rpc("triggers.save", params)
        .await
        .expect("triggers.save")
}

async fn fired_count(lab: &nocturne_app::testing::Lab, id: &str) -> u64 {
    lab.app
        .rpc("triggers.get", json!({"id": id}))
        .await
        .expect("triggers.get")["fired_count"]
        .as_u64()
        .expect("a count")
}

#[tokio::test(start_paused = true)]
async fn a_burst_of_events_fires_a_trigger_once() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let trigger = save(&lab, json!({})).await;
    let id = trigger["id"].as_str().expect("an id").to_string();

    for index in 0..5 {
        lab.app.publish(
            "alerts.raised",
            &json!({"device_id": lab.id("SW1"), "n": index}),
        );
    }
    tick_until(10, || scans(&lab) == 1).await;
    assert_eq!(scans(&lab), 1, "five events, one run");
    tick(10).await;
    assert_eq!(scans(&lab), 1, "and it stays one");
    assert_eq!(fired_count(&lab, &id).await, 1);

    // A second burst later is a second run.
    lab.app.publish("alerts.raised", &json!({"n": 99}));
    tick_until(10, || scans(&lab) == 2).await;
    assert_eq!(fired_count(&lab, &id).await, 2);
}

#[tokio::test(start_paused = true)]
async fn a_trigger_only_watches_its_own_topic_and_its_own_payloads() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    save(
        &lab,
        json!({"topic": "alerts.raised", "match_payload": {"/severity": "critical"}}),
    )
    .await;

    lab.app
        .publish("alerts.cleared", &json!({"severity": "critical"}));
    lab.app
        .publish("devices.changed", &json!({"severity": "critical"}));
    lab.app
        .publish("alerts.raised", &json!({"severity": "warning"}));
    lab.app.publish("alerts.raised", &json!({"nothing": true}));
    tick(10).await;
    assert_eq!(scans(&lab), 0, "none of those is what it asked for");

    lab.app
        .publish("alerts.raised", &json!({"severity": "critical"}));
    tick_until(10, || scans(&lab) == 1).await;
}

#[tokio::test(start_paused = true)]
async fn a_prefix_topic_watches_everything_under_it() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    save(&lab, json!({"topic": "alerts."})).await;
    lab.app.publish("alerts.cleared", &json!({}));
    tick_until(10, || scans(&lab) == 1).await;
}

#[tokio::test(start_paused = true)]
async fn a_trigger_can_hand_the_device_the_event_was_about_to_the_task_it_runs() {
    let (lab, _clock) = lab_at(&["SW1", "SW2"], MONDAY).await;
    let task = lab
        .app
        .rpc(
            "tasks.save",
            json!({"name": "Read the version", "select": {"filter": {"site": "Lab"}},
                   "action": {"type": "commands", "commands": ["show version"]}}),
        )
        .await
        .expect("tasks.save");
    save(
        &lab,
        json!({
            "topic": "devices.state",
            "debounce_s": 1,
            "pass_device": true,
            "action": {"method": "tasks.run", "params": {"id": task["id"]}},
        }),
    )
    .await;

    // SW2 went down; only SW2 should be looked at, although the task's own
    // filter covers the whole site.
    lab.app
        .publish("devices.state", &json!({"device_id": lab.id("SW2")}));
    tick_until(20, || !lab.fake("SW2").seen().is_empty()).await;

    assert!(
        lab.fake("SW1").seen().is_empty(),
        "the device the event was not about was left alone"
    );
    let runs = lab
        .app
        .rpc("runner.runs", json!({}))
        .await
        .expect("runner.runs");
    let runs = runs.as_array().expect("array");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0]["origin"], json!("trigger"));
    assert_eq!(runs[0]["devices"], json!(1));
    assert_eq!(runs[0]["task_id"], task["id"]);
}

#[tokio::test(start_paused = true)]
async fn a_trigger_whose_last_run_is_still_going_holds_the_burst_rather_than_losing_it() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let trigger = save(&lab, json!({"debounce_s": 1})).await;
    let id = trigger["id"].as_str().expect("an id").to_string();
    let state = lab.app.state::<AutomationState>();
    let guard = state.start_firing(&id).expect("the first claim");

    lab.app.publish("alerts.raised", &json!({"n": 1}));
    tick(10).await;
    assert_eq!(scans(&lab), 0, "the previous run has not finished");

    drop(guard);
    tick_until(20, || scans(&lab) == 1).await;
    assert_eq!(
        fired_count(&lab, &id).await,
        1,
        "the burst was held, not thrown away"
    );
}

#[tokio::test]
async fn a_trigger_cannot_watch_automations_own_events_or_call_what_a_schedule_cannot() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    for topic in [
        "automation.runs.changed",
        "schedules.fired",
        "triggers.fired",
        "tasks.changed",
    ] {
        let err = lab
            .app
            .rpc(
                "triggers.save",
                json!({"name": "Loop", "topic": topic,
                       "action": {"method": "insights.scan"}}),
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("`{topic}` would be a loop"));
        assert_eq!(err.code, "invalid");
        assert!(err.message.contains("loop"), "{}", err.message);
    }

    let err = lab
        .app
        .rpc(
            "triggers.save",
            json!({"name": "No", "topic": "alerts.raised",
                   "action": {"method": "changes.apply"}}),
        )
        .await
        .expect_err("not on the allow-list");
    assert_eq!(err.code, "forbidden");

    let err = lab
        .app
        .rpc(
            "triggers.save",
            json!({"name": "No", "topic": "", "action": {"method": "insights.scan"}}),
        )
        .await
        .expect_err("no topic");
    assert_eq!(err.code, "invalid");
}

#[tokio::test(start_paused = true)]
async fn a_disabled_or_deleted_trigger_does_not_fire() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let trigger = save(&lab, json!({"enabled": false})).await;
    lab.app.publish("alerts.raised", &json!({}));
    tick(10).await;
    assert_eq!(scans(&lab), 0);

    // Enabled again, it fires; deleted, it does not.
    lab.app
        .rpc(
            "triggers.save",
            json!({"id": trigger["id"], "name": "On an alert", "enabled": true,
                   "topic": "alerts.raised", "debounce_s": 1,
                   "action": {"method": "insights.scan"}}),
        )
        .await
        .expect("triggers.save");
    lab.app.publish("alerts.raised", &json!({}));
    tick_until(10, || scans(&lab) == 1).await;

    lab.app
        .rpc("triggers.delete", json!({"id": trigger["id"]}))
        .await
        .expect("triggers.delete");
    lab.app.publish("alerts.raised", &json!({}));
    tick(10).await;
    assert_eq!(scans(&lab), 1);
    assert!(
        lab.app
            .rpc("triggers.list", json!({}))
            .await
            .expect("list")
            .as_array()
            .expect("array")
            .is_empty()
    );
}

#[tokio::test]
async fn fire_now_runs_a_trigger_by_hand() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let trigger = save(&lab, json!({})).await;
    let result = lab
        .app
        .rpc("triggers.fire_now", json!({"id": trigger["id"]}))
        .await
        .expect("triggers.fire_now");
    assert_eq!(result["scanned"], json!(1));
}
