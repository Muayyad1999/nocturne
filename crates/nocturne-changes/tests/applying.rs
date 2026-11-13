//! Applying: the order of the steps, what is kept, and what happens when
//! anything at all goes wrong.

mod common;

use common::*;
use serde_json::json;

/// A low-risk change to a port, which needs no guard and no confirmation.
fn desk_change() -> serde_json::Value {
    json!({"type": "lines", "lines": [
        "interface GigabitEthernet0/2",
        "description desk 14",
        "exit",
    ]})
}

#[tokio::test]
async fn a_clean_apply_backs_up_verifies_saves_and_is_audited() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // Something worth redacting in the configuration the session will print.
    load(
        &lab,
        "SW1",
        &format!("{BASE_CONFIG}username backdoor privilege 15 secret 0 Sup3rS3cret\n"),
    );
    let before = lab.fake("SW1").running_config();

    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    assert_eq!(plan(&change)["risk"]["level"], "low");
    let applied = apply(app, &change).await;

    assert_eq!(target_state(&applied), "applied");
    assert_eq!(applied["state"], "applied");
    assert_eq!(
        steps(&applied),
        vec![
            "connect",
            "drift check",
            "snapshot",
            "backup",
            "apply",
            "verify",
            "save",
            "final backup"
        ]
    );

    // The device really changed, and really saved.
    let after = lab.fake("SW1").running_config();
    assert!(after.contains(" description desk 14\n"), "{after}");
    assert!(!after.contains("desk 12"));
    assert!(lab.fake("SW1").with(|state| state.saved()));

    // A version before and a version after, both kept.
    let result = &applied["targets"][0]["result"];
    let (before_id, after_id) = (
        result["before_version_id"].as_i64().unwrap(),
        result["after_version_id"].as_i64().unwrap(),
    );
    assert_ne!(before_id, after_id);
    let stored = versions(app, &lab.id("SW1"));
    assert!(
        stored
            .iter()
            .any(|(id, source, _)| *id == before_id && source == "pre-change")
            && stored
                .iter()
                .any(|(id, source, _)| *id == after_id && source == "post-change"),
        "{stored:?}"
    );
    let (_, kept) = app.store().config_get(before_id).unwrap();
    assert_eq!(
        canonical(&kept),
        canonical(&before),
        "the backup is what the device had"
    );

    // Every command carries the device's answer.
    let sent = result["sent"].as_array().unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[1]["command"], "description desk 14");
    assert!(sent.iter().all(|line| line["error"].is_null()));

    // The checks that were run say what they looked at.
    let checks: Vec<String> = result["verification"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| {
            assert_eq!(check["ok"], true, "{check}");
            check["name"].as_str().unwrap().to_string()
        })
        .collect();
    assert_eq!(checks, vec!["configuration", "neighbours", "interfaces"]);

    // The transcript is kept, with the secret taken out of it.
    let transcript = result["transcript"].as_str().unwrap();
    assert!(transcript.contains("description desk 14"), "{transcript}");
    assert!(!transcript.contains("Sup3rS3cret"), "{transcript}");
    assert!(transcript.contains("<secret:"), "{transcript}");

    // And it is all in the audit trail.
    let audit = audit(app);
    let sources: Vec<&str> = audit.iter().map(|(source, _)| source.as_str()).collect();
    for action in [
        "changes.create",
        "changes.plan",
        "changes.approve",
        "changes.apply",
    ] {
        assert!(
            sources.contains(&action),
            "{action} missing from {sources:?}"
        );
    }
    assert!(
        audit
            .iter()
            .any(|(source, message)| source == "changes.apply" && message.contains("applied")),
        "{audit:?}"
    );
    // The device's own audit trail says what was done to it.
    let per_device = app
        .store()
        .log_query(&nocturne_app::store::LogQuery {
            stream: Some("audit".into()),
            device_id: Some(lab.id("SW1")),
            ..Default::default()
        })
        .unwrap();
    assert!(
        per_device.iter().any(
            |entry| entry.source == "changes.apply" && entry.message.contains("`SW1`: applied")
        ),
        "{:?}",
        per_device
            .iter()
            .map(|entry| &entry.message)
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn a_configuration_that_changed_since_planning_is_a_conflict() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    approve(app, &change).await.unwrap();

    // Somebody else gets there first.
    load(
        &lab,
        "SW1",
        &BASE_CONFIG.replace("ip domain-name lab.example", "ip domain-name other.example"),
    );
    let before = lab.fake("SW1").running_config();

    let id = change["id"].as_str().unwrap();
    let job = try_job(app, "changes.apply", json!({"id": id}))
        .await
        .unwrap();
    assert_eq!(job.state, nocturne_app::jobs::JobState::Succeeded);
    let after = app.rpc("changes.get", json!({"id": id})).await.unwrap();
    assert_eq!(target_state(&after), "failed");
    assert_eq!(after["state"], "failed");
    let error = after["targets"][0]["result"]["error"].as_str().unwrap();
    assert!(error.contains("changed since this was planned"), "{error}");
    assert!(error.contains("plan again"), "{error}");
    assert_eq!(
        steps(&after),
        vec!["connect", "drift check"],
        "nothing is sent after a drift check fails"
    );
    assert_eq!(
        lab.fake("SW1").running_config(),
        before,
        "the device is untouched"
    );
}

#[tokio::test]
async fn a_rejected_line_is_rolled_back_to_the_byte() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    lab.fake("SW1")
        .with(|state| state.reject.push("storm-control".into()));
    let before = lab.fake("SW1").running_config();

    let change = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": [
            "interface GigabitEthernet0/2",
            "description desk 14",
            "storm-control broadcast level 50",
            "exit",
        ]}),
        json!({}),
    )
    .await;
    let applied = apply(app, &change).await;

    assert_eq!(target_state(&applied), "rolled_back");
    assert_eq!(applied["state"], "rolled_back");
    let result = &applied["targets"][0]["result"];
    assert_eq!(result["failed_line"], 2);
    let error = result["error"].as_str().unwrap();
    assert!(error.contains("storm-control"), "{error}");
    assert!(
        steps(&applied).contains(&"rollback".to_string()),
        "{:?}",
        steps(&applied)
    );
    assert_eq!(
        lab.fake("SW1").running_config(),
        before,
        "the running configuration is back, byte for byte"
    );
    assert!(
        !lab.fake("SW1").seen().contains(&"write memory".to_string()),
        "a rolled-back change is never written to the startup configuration"
    );
}

#[tokio::test]
async fn a_neighbour_that_disappears_rolls_the_change_back() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    let fake = lab.fake("SW1").clone();
    // The uplink's neighbour goes away while the change is being made.
    when_step(app, "backup", move || {
        fake.with(|state| state.neighbors.clear());
    });

    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let applied = apply(app, &change).await;

    assert_eq!(target_state(&applied), "rolled_back");
    let checks = applied["targets"][0]["result"]["verification"]
        .as_array()
        .unwrap()
        .clone();
    let neighbours = checks
        .iter()
        .find(|check| check["name"] == "neighbours")
        .unwrap();
    assert_eq!(neighbours["ok"], false);
    assert!(
        neighbours["detail"]
            .as_str()
            .unwrap()
            .contains("CORE1.lab.example"),
        "{neighbours}"
    );
    assert_eq!(lab.fake("SW1").running_config(), before);
}

#[tokio::test]
async fn a_ping_that_stops_working_rolls_the_change_back() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    lab.fake("SW1")
        .with(|state| state.unreachable.push("10.0.0.1".into()));
    let before = lab.fake("SW1").running_config();

    let change = planned(
        app,
        &lab.id("SW1"),
        desk_change(),
        json!({"verify": {"pings": ["10.0.0.1"]}}),
    )
    .await;
    let applied = apply(app, &change).await;

    assert_eq!(target_state(&applied), "rolled_back");
    let checks = applied["targets"][0]["result"]["verification"]
        .as_array()
        .unwrap()
        .clone();
    let ping = checks
        .iter()
        .find(|check| check["name"] == "ping 10.0.0.1")
        .unwrap();
    assert_eq!(ping["ok"], false, "{ping}");
    assert!(ping["detail"].as_str().unwrap().contains("0%"), "{ping}");
    assert_eq!(lab.fake("SW1").running_config(), before);
}

#[tokio::test]
async fn the_guard_is_armed_before_the_change_and_disarmed_before_the_save() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(
        app,
        &lab.id("SW1"),
        desk_change(),
        json!({"guard_minutes": 10}),
    )
    .await;
    let guard = plan(&change)["guard"].clone();
    assert_eq!(guard["name"], "reload timer");
    assert_eq!(guard["minutes"], 10);
    assert_eq!(guard["arm"], json!(["reload in 10"]));

    let applied = apply(app, &change).await;
    assert_eq!(target_state(&applied), "applied");
    assert_eq!(
        steps(&applied),
        vec![
            "connect",
            "drift check",
            "snapshot",
            "backup",
            "arm guard",
            "apply",
            "verify",
            "disarm guard",
            "save",
            "final backup"
        ]
    );
    lab.fake("SW1").with(|state| {
        assert_eq!((state.reload_armed, state.reload_cancelled), (1, 1));
        assert_eq!(state.reload_in, None);
        assert!(state.saved());
    });
    let seen = lab.fake("SW1").seen();
    let cancelled = seen.iter().position(|line| line == "reload cancel");
    let saved = seen.iter().position(|line| line == "write memory");
    assert!(
        cancelled < saved && cancelled.is_some() && saved.is_some(),
        "the guard is cancelled before the configuration is saved: {seen:?}"
    );
}

#[tokio::test]
async fn a_session_cut_with_a_guard_armed_leaves_the_guard_to_fire() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    impatient(app).await;
    lab.fake("SW1")
        .with(|state| state.drop_on = Some("switchport voice vlan 20".into()));

    let change = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": [
            "interface GigabitEthernet0/2",
            "description desk 14",
            "switchport voice vlan 20",
            "exit",
        ]}),
        json!({"guard_minutes": 10}),
    )
    .await;

    // Once the guard is armed the device stops answering altogether.
    let app_for_event = app.clone();
    when_step(app, "arm guard", move || {
        app_for_event
            .state::<nocturne_app::connect::ConnectState>()
            .set_sim_connector(std::sync::Arc::new(nocturne_app::testing::FakeSim {
                devices: Vec::new(),
            }));
    });

    let applied = apply(app, &change).await;
    assert_eq!(target_state(&applied), "guard_fired");
    assert_eq!(applied["state"], "failed");
    let error = applied["targets"][0]["result"]["error"].as_str().unwrap();
    assert!(error.contains("reload timer"), "{error}");
    assert!(error.contains("10 minutes"), "{error}");
    assert!(
        error.contains("comes back as it was"),
        "it says what the device will do: {error}"
    );
    assert!(
        steps(&applied).iter().any(|step| step == "reconnect"),
        "it tried: {:?}",
        steps(&applied)
    );
    lab.fake("SW1").with(|state| {
        assert_eq!(state.reload_in, Some(10), "the guard is still armed");
        assert_eq!(state.reload_cancelled, 0, "and was never cancelled");
        assert!(!state.saved(), "nothing was saved");
    });
}

#[tokio::test]
async fn a_session_cut_without_a_guard_is_rolled_back_once_the_device_answers_again() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    impatient(app).await;
    let before = lab.fake("SW1").running_config();
    lab.fake("SW1")
        .with(|state| state.drop_on = Some("switchport voice vlan 20".into()));

    let change = planned(
        app,
        &lab.id("SW1"),
        json!({"type": "lines", "lines": [
            "interface GigabitEthernet0/2",
            "description desk 14",
            "switchport voice vlan 20",
            "exit",
        ]}),
        json!({"guard_minutes": 0}),
    )
    .await;
    assert!(plan(&change)["guard"].is_null(), "no guard was asked for");

    let applied = apply(app, &change).await;
    assert_eq!(target_state(&applied), "rolled_back");
    let error = applied["targets"][0]["result"]["error"].as_str().unwrap();
    assert!(error.contains("session was lost"), "{error}");
    assert!(error.contains("put back as it was"), "{error}");
    assert!(steps(&applied).iter().any(|step| step == "reconnect"));
    assert_eq!(
        lab.fake("SW1").running_config(),
        before,
        "the half-made change was undone"
    );
    assert!(!lab.fake("SW1").seen().contains(&"write memory".to_string()));
}

#[tokio::test]
async fn an_unexpected_configuration_afterwards_is_a_failed_check() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let before = lab.fake("SW1").running_config();
    let fake = lab.fake("SW1").clone();
    // Somebody types something else on the device in the middle of the change.
    when_step(app, "apply", move || {
        fake.with(|state| {
            let config = format!("{BASE_CONFIG}ntp server 192.0.2.99\n");
            let running = config.replace("description desk 12", "description desk 14");
            state.load(&running);
        });
    });

    let change = planned(app, &lab.id("SW1"), desk_change(), json!({})).await;
    let applied = apply(app, &change).await;

    assert_eq!(target_state(&applied), "rolled_back");
    let checks = applied["targets"][0]["result"]["verification"]
        .as_array()
        .unwrap()
        .clone();
    let configuration = checks
        .iter()
        .find(|check| check["name"] == "configuration")
        .unwrap();
    assert_eq!(configuration["ok"], false);
    let detail = configuration["detail"].as_str().unwrap();
    assert!(detail.contains("not what the plan predicted"), "{detail}");
    assert!(detail.contains("ntp server 192.0.2.99"), "{detail}");
    assert_eq!(
        canonical(&lab.fake("SW1").running_config()),
        canonical(&before),
        "and the device is put back"
    );
}

#[tokio::test]
async fn a_check_the_device_type_cannot_run_is_reported_not_assumed() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let change = planned(
        app,
        &lab.id("SW1"),
        desk_change(),
        json!({"save": false, "verify": {"routes": true}}),
    )
    .await;
    let applied = apply(app, &change).await;
    assert_eq!(target_state(&applied), "applied");

    let checks = applied["targets"][0]["result"]["verification"]
        .as_array()
        .unwrap()
        .clone();
    let routes = checks
        .iter()
        .find(|check| check["name"] == "routes")
        .unwrap();
    assert_eq!(routes["ok"], true);
    assert!(
        routes["detail"]
            .as_str()
            .unwrap()
            .starts_with("not checked"),
        "{routes}"
    );
    assert!(
        routes["detail"].as_str().unwrap().contains("routes"),
        "it says which kind is missing: {routes}"
    );

    // `save: false` leaves the startup configuration alone.
    assert!(!steps(&applied).contains(&"save".to_string()));
    assert!(!lab.fake("SW1").seen().contains(&"write memory".to_string()));
    assert!(
        !lab.fake("SW1").with(|state| state.saved()),
        "the change is in the running configuration only"
    );
}
