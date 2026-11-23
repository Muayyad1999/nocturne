//! Monitoring switched off for a device.
//!
//! One rule, stated once in `checks::watched`: a device that is not
//! monitored is not watched. Its checks do not run — not on the schedule and
//! not when somebody asks — they are not counted as up or down, the rules
//! say nothing about it, and what the rules had open on it is closed when
//! the switch is thrown. An alert that nothing will ever look at again is a
//! lie that stays on the screen.

use std::time::Duration;

use serde_json::json;

use crate::checks::model::CheckStatus;
use crate::testing::Harness;

fn titles(alerts: &[crate::model::Alert]) -> Vec<&str> {
    alerts.iter().map(|alert| alert.title.as_str()).collect()
}

fn problem_kinds(status: &serde_json::Value) -> Vec<&str> {
    status["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|problem| problem["kind"].as_str())
        .collect()
}

/// `web` on SW9: the box answers ping, the service does not, and the alert
/// for it is open.
async fn a_service_down_on_sw9(
    harness: &Harness,
    device_patch: serde_json::Value,
) -> (String, String) {
    harness.only_rule("builtin.service_check_down").await;
    let device = harness.device("SW9", "10.5.0.9", device_patch).await;
    harness.probe.down("10.5.0.80");
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.5.0.80", "interval_s": 30}),
        )
        .await;
    harness.run_check(&check).await;
    assert_eq!(
        titles(&harness.open_alerts()),
        ["Service check down — web on SW9"]
    );
    (device, check)
}

#[tokio::test(start_paused = true)]
async fn switching_monitoring_off_for_a_device_closes_what_was_open_on_it() {
    let harness = Harness::new().await;
    let (device, check) = a_service_down_on_sw9(&harness, json!({})).await;
    crate::scheduler::spawn(&harness.app);

    // The operator is about to decommission SW9.
    harness
        .rpc(
            "devices.update",
            json!({"id": device, "monitor": {"enabled": false, "icmp": true}}),
        )
        .await;
    tokio::time::sleep(Duration::from_secs(3_600)).await;

    assert_eq!(
        titles(&harness.open_alerts()),
        Vec::<&str>::new(),
        "nothing will look at SW9 again, so nothing about it may stay firing"
    );
    let closed = &harness.all_alerts()[0];
    assert!(
        closed.detail.contains("switched off"),
        "the record says why it closed: {}",
        closed.detail
    );
    let status = harness.rpc("monitor.status", json!({})).await;
    assert_eq!(
        status["checks"]["down"], 0,
        "a check nobody is running is not a check that is down: {}",
        status["checks"]
    );
    assert_eq!(status["checks"]["disabled"], 1);
    assert_eq!(status["checks"]["total"], 1);
    assert!(
        !problem_kinds(&status).contains(&"checks"),
        "{:?}",
        problem_kinds(&status)
    );
    assert_eq!(
        harness.check_state(&check).status,
        CheckStatus::Down,
        "what it last found is still on record; it just is not news"
    );

    // Nothing is lost by it: switched back on, the service is still down
    // and the first run says so again.
    harness
        .rpc(
            "devices.update",
            json!({"id": device, "monitor": {"enabled": true, "icmp": true}}),
        )
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(
        titles(&harness.open_alerts()),
        ["Service check down — web on SW9"]
    );
    harness.app.shutdown();
}

#[tokio::test(start_paused = true)]
async fn switching_monitoring_off_closes_only_what_the_rules_were_keeping_true() {
    // Three alerts on SW9: one a rule keeps true from run to run, one that
    // another feature raised, and one that records a configuration change.
    // Only the first is a claim that stops being checked when monitoring is
    // switched off.
    let harness = Harness::new().await;
    let (device, _) = a_service_down_on_sw9(&harness, json!({})).await;
    harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.config_changed", "enabled": true}),
        )
        .await;
    harness
        .rpc(
            "alerts.raise",
            json!({"rule": "log rule: authentication failures", "device_id": device,
                   "severity": "high", "title": "Repeated login failures",
                   "dedup_key": "logs:auth_fail:SW9"}),
        )
        .await;
    crate::rules::on_config_changed(
        &harness.app,
        &device,
        &json!({"device_id": device, "kind": "running", "version_id": 7, "added": 3, "removed": 1}),
    )
    .unwrap();
    assert_eq!(harness.open_alerts().len(), 3);
    crate::scheduler::spawn(&harness.app);

    harness
        .rpc(
            "devices.update",
            json!({"id": device, "monitor": {"enabled": false, "icmp": true}}),
        )
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;

    let mut left = harness.open_alerts();
    left.sort_by(|a, b| a.title.cmp(&b.title));
    assert_eq!(
        titles(&left),
        ["Configuration changed — SW9", "Repeated login failures"],
        "the service alert is closed; the other two were never the rules' to keep"
    );
    harness.app.shutdown();
}

#[tokio::test]
async fn switching_a_check_off_closes_its_alert_without_waiting_for_a_poll_that_may_never_come() {
    let harness = Harness::new().await;
    // A device with nothing to poll it by: only its checks ever touch it,
    // so no poll will come along and tidy up.
    let (_, check) = a_service_down_on_sw9(
        &harness,
        json!({"monitor": {"enabled": true, "icmp": false, "snmp": false}}),
    )
    .await;

    harness
        .rpc("checks.save", json!({"id": check, "enabled": false}))
        .await;

    assert_eq!(titles(&harness.open_alerts()), Vec::<&str>::new());
    let status = harness.rpc("monitor.status", json!({})).await;
    assert_eq!(status["checks"]["down"], 0);
    assert_eq!(status["checks"]["disabled"], 1);
}

#[tokio::test]
async fn run_now_leaves_the_checks_of_an_unmonitored_device_alone() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.service_check_down").await;
    let device = harness
        .device(
            "SW9",
            "10.5.0.9",
            json!({"monitor": {"enabled": false, "icmp": true}}),
        )
        .await;
    harness.probe.down("10.5.0.80");
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.5.0.80"}),
        )
        .await;

    for ask in [json!({"device_id": device}), json!({"id": check})] {
        let answer = harness.rpc("checks.run_now", ask.clone()).await;
        assert_eq!(answer["ran"], 0, "{ask}: {answer}");
        let skipped = &answer["checks"][0];
        assert_eq!(skipped["name"], "web");
        assert_eq!(skipped["skipped"], "monitoring_off", "{answer}");
        assert!(
            skipped["error"].as_str().unwrap().contains("SW9"),
            "it says which switch is off: {answer}"
        );
    }
    assert!(
        harness.probe.pinged.lock().is_empty(),
        "nothing was sent on behalf of a device that is switched off"
    );
    assert_eq!(harness.check_state(&check).status, CheckStatus::Unknown);
    assert_eq!(
        titles(&harness.open_alerts()),
        Vec::<&str>::new(),
        "and no alert was raised that nothing would ever look at again"
    );
}

#[tokio::test]
async fn a_verdict_recorded_about_an_unmonitored_device_raises_nothing() {
    // Whatever path a result takes to the rules — here the one the
    // scheduler uses, called directly — the rules do not act on it for a
    // device nobody is watching.
    let harness = Harness::new().await;
    harness.only_rule("builtin.service_check_down").await;
    let device = harness
        .device(
            "SW9",
            "10.5.0.9",
            json!({"monitor": {"enabled": false, "icmp": true}}),
        )
        .await;
    harness.probe.down("10.5.0.80");
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.5.0.80"}),
        )
        .await;
    let ran = harness.run_check(&check).await;
    assert_eq!(ran.state.status, CheckStatus::Down);
    assert_eq!(titles(&harness.open_alerts()), Vec::<&str>::new());
}

#[tokio::test]
async fn run_now_says_which_checks_it_ran_and_which_it_left() {
    // A mixed request: one check that is being watched, one that is
    // switched off itself.
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let device = harness.device("SW1", "10.5.0.1", json!({})).await;
    let on = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    let off = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.5.0.80", "enabled": false}),
        )
        .await;

    let answer = harness
        .rpc("checks.run_now", json!({"ids": [on, off]}))
        .await;
    assert_eq!(answer["ran"], 1);
    let by_name = |name: &str| {
        answer["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["name"] == name)
            .cloned()
            .unwrap()
    };
    assert_eq!(by_name("ping")["status"], "up");
    assert_eq!(by_name("web")["skipped"], "disabled");
    assert_eq!(harness.probe.count("10.5.0.80"), 0);
}

#[tokio::test]
async fn the_switches_for_the_whole_installation_are_the_same_rule_writ_large() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let device = harness.device("SW1", "10.5.0.1", json!({})).await;
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;
    let status = harness.rpc("monitor.status", json!({})).await;
    assert_eq!(status["checks"]["up"], 1);

    for (setting, skipped) in [
        ("checks_enabled", "checks_off"),
        ("enabled", "monitoring_off"),
    ] {
        harness
            .rpc(
                "settings.set",
                json!({"section": "monitoring", "value": {setting: false}}),
            )
            .await;
        let sent = harness.probe.count("10.5.0.1");
        let answer = harness.rpc("checks.run_now", json!({"id": check})).await;
        assert_eq!(answer["ran"], 0, "{setting}: {answer}");
        assert_eq!(answer["checks"][0]["skipped"], skipped);
        assert!(
            answer["checks"][0]["error"]
                .as_str()
                .unwrap()
                .contains(setting),
            "it names the setting: {answer}"
        );
        assert_eq!(harness.probe.count("10.5.0.1"), sent, "nothing was sent");
        let status = harness.rpc("monitor.status", json!({})).await;
        assert_eq!(
            (&status["checks"]["up"], &status["checks"]["disabled"]),
            (&json!(0), &json!(1)),
            "{setting}: a check nothing runs is not counted as up either"
        );
        harness
            .rpc(
                "settings.set",
                json!({"section": "monitoring", "value": {setting: true}}),
            )
            .await;
    }
    let answer = harness.rpc("checks.run_now", json!({"id": check})).await;
    assert_eq!(answer["ran"], 1, "switched back on, it runs: {answer}");
}

#[tokio::test(start_paused = true)]
async fn a_poll_asked_for_by_hand_raises_nothing_about_a_device_that_is_not_monitored() {
    // The same rule from the poller's side: `monitor.poll_now` will poll
    // any device it is given, and what it finds on one that is switched off
    // is an answer to the person who asked, not an alert.
    let harness = Harness::new().await;
    harness.only_rule("builtin.device_down").await;
    let device = harness
        .device(
            "SW9",
            "10.5.0.9",
            json!({"monitor": {"enabled": false, "icmp": true}}),
        )
        .await;
    harness.probe.down("10.5.0.9");
    harness.poll_and_evaluate(&device).await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    let poll = harness.poll_and_evaluate(&device).await;
    assert!(poll.down, "the poll itself still says what it found");
    assert_eq!(titles(&harness.open_alerts()), Vec::<&str>::new());
}
