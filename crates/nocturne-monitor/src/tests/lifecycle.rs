//! The alert lifecycle: fire once, count repeats, resolve, re-fire; `for_s`;
//! hysteresis; device-down suppression; maintenance and snoozes; dedup.

use std::time::Duration;

use serde_json::json;

use crate::model::{AlertState, Severity};
use crate::testing::{AgentData, AgentIf, Harness};

const MINUTE: Duration = Duration::from_secs(60);

async fn device_down_only(harness: &Harness) -> String {
    harness.only_rule("builtin.device_down").await;
    harness.device("SW1", "10.0.0.1", json!({})).await
}

#[tokio::test(start_paused = true)]
async fn an_alert_fires_once_counts_repeats_resolves_and_then_re_fires() {
    let harness = Harness::new().await;
    let id = device_down_only(&harness).await;
    harness.probe.down("10.0.0.1");

    // Damping first: one failure raises nothing.
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());

    tokio::time::sleep(MINUTE).await;
    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    let first = open[0].clone();
    assert_eq!(first.severity, Severity::Critical);
    assert_eq!(first.state, AlertState::Firing);
    assert_eq!(first.count, 1);
    assert_eq!(first.device.as_deref(), Some("SW1"));
    assert!(first.title.contains("SW1"));

    // Still down: the same alert, counted, not a second one.
    for expected in [2, 3] {
        tokio::time::sleep(MINUTE).await;
        harness.poll_and_evaluate(&id).await;
        let open = harness.open_alerts();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, first.id);
        assert_eq!(open[0].count, expected);
    }

    // Back up: it resolves itself.
    tokio::time::sleep(MINUTE).await;
    harness.probe.up("10.0.0.1");
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());
    let resolved = crate::alerts::get(&harness.app, &first.id).unwrap();
    assert_eq!(resolved.state, AlertState::Resolved);
    assert_eq!(resolved.resolved_at, Some(harness.now()));

    // Down again: a new alert, not the old one reopened.
    harness.probe.down("10.0.0.1");
    for _ in 0..2 {
        tokio::time::sleep(MINUTE).await;
        harness.poll_and_evaluate(&id).await;
    }
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert_ne!(open[0].id, first.id, "a new outage is a new alert");
    assert_eq!(open[0].count, 1);
    assert_eq!(harness.all_alerts().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_threshold_waits_for_its_period_before_firing() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    let rule = harness
        .rpc(
            "alerts.rules.save",
            json!({
                "name": "Loss", "severity": "medium", "for_s": 180,
                "condition": {"type": "threshold", "metric": "icmp.loss_pct",
                              "op": "gt", "value": 20.0, "clear_value": 5.0},
            }),
        )
        .await;
    assert_eq!(rule["for_s"], 180);
    harness.probe.set(
        "10.0.0.1",
        crate::testing::HostState {
            lost: 2,
            ..crate::testing::HostState::default()
        },
    );

    // 67 % loss, but the rule asks for three minutes of it.
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());
    tokio::time::sleep(MINUTE).await;
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());
    tokio::time::sleep(Duration::from_secs(121)).await;
    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].value.map(f64::round), Some(67.0));
    assert!(
        open[0].detail.contains("icmp.loss_pct"),
        "{}",
        open[0].detail
    );
}

#[tokio::test(start_paused = true)]
async fn a_condition_that_clears_before_its_period_never_fires() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness
        .rpc(
            "alerts.rules.save",
            json!({
                "name": "Loss", "severity": "medium", "for_s": 300,
                "condition": {"type": "threshold", "metric": "icmp.loss_pct", "op": "gt", "value": 20.0},
            }),
        )
        .await;
    harness.probe.set(
        "10.0.0.1",
        crate::testing::HostState {
            lost: 3,
            ..crate::testing::HostState::default()
        },
    );
    harness.poll_and_evaluate(&id).await;
    tokio::time::sleep(MINUTE).await;
    // The loss goes away, so the clock restarts.
    harness.probe.up("10.0.0.1");
    harness.poll_and_evaluate(&id).await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    harness.probe.set(
        "10.0.0.1",
        crate::testing::HostState {
            lost: 3,
            ..crate::testing::HostState::default()
        },
    );
    harness.poll_and_evaluate(&id).await;
    assert!(
        harness.open_alerts().is_empty(),
        "the wait starts again once the condition clears"
    );
}

#[tokio::test(start_paused = true)]
async fn hysteresis_keeps_an_alert_open_between_the_two_thresholds() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(1_000),
                counter64: true,
                interfaces: vec![AgentIf::up("Gi0/1", 1_000)],
                ..AgentData::default()
            },
        )
        .await;
    harness
        .rpc(
            "alerts.rules.save",
            json!({
                "name": "Busy port", "severity": "medium",
                "condition": {"type": "threshold", "metric": "if.util_pct", "op": "gt",
                              "value": 90.0, "clear_value": 80.0},
            }),
        )
        .await;

    let mut second = 0u32;
    // Octets for one minute at a given percentage of a 1 Gbit/s port.
    let mut step = |percent: f64| {
        second += 60;
        harness.snmp.edit("10.0.0.1:161", |agent| {
            agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
            let octets = (1_000_000_000.0 / 8.0 * 60.0 * percent / 100.0) as u64;
            agent.interfaces[0].in_octets += octets;
        });
    };

    harness.poll_and_evaluate(&id).await;
    tokio::time::sleep(MINUTE).await;
    step(95.0);
    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1, "95 % is above the firing threshold");
    let alert = open[0].clone();
    assert_eq!(alert.instance.as_deref(), Some("GigabitEthernet0/1"));

    // 85 %: below the alarm but above the clear value, so it stays open.
    tokio::time::sleep(MINUTE).await;
    step(85.0);
    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].id, alert.id);

    // 50 %: past the clear value, so it resolves.
    tokio::time::sleep(MINUTE).await;
    step(50.0);
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());
    assert_eq!(
        crate::alerts::get(&harness.app, &alert.id).unwrap().state,
        AlertState::Resolved
    );
}

#[tokio::test(start_paused = true)]
async fn a_device_that_is_down_raises_one_alert_not_fifty() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(1_000),
                counter64: true,
                interfaces: vec![
                    AgentIf {
                        alias: "uplink to CORE".into(),
                        ..AgentIf::up("Gi0/1", 1_000)
                    },
                    AgentIf {
                        oper_up: false,
                        alias: "desk 12".into(),
                        ..AgentIf::up("Gi0/2", 1_000)
                    },
                ],
                ..AgentData::default()
            },
        )
        .await;
    // Interface-down on anything described, plus device-down.
    harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.uplink_down", "condition": {"type": "interface_down", "only": "described"}}),
        )
        .await;

    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1, "{open:#?}");
    assert_eq!(open[0].instance.as_deref(), Some("GigabitEthernet0/2"));

    // Now the whole switch goes away.
    harness.probe.down("10.0.0.1");
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.fail = Some("timed out".into())
    });
    for _ in 0..2 {
        tokio::time::sleep(MINUTE).await;
        harness.poll_and_evaluate(&id).await;
    }
    let open = harness.open_alerts();
    assert!(
        open.iter().any(|alert| alert.rule_name == "Device down"),
        "{open:#?}"
    );
    assert_eq!(
        open.len(),
        2,
        "the port that was already down stays, but nothing new is raised while the device is down"
    );

    // Nothing new appears however long it stays down.
    for _ in 0..3 {
        tokio::time::sleep(MINUTE).await;
        harness.poll_and_evaluate(&id).await;
    }
    assert_eq!(harness.open_alerts().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn an_interface_that_comes_back_resolves_its_own_alert() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    harness.only_rule("builtin.uplink_down").await;
    harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.uplink_down", "enabled": true,
                   "condition": {"type": "interface_down", "only": "all"}}),
        )
        .await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(1_000),
                counter64: true,
                interfaces: vec![AgentIf {
                    oper_up: false,
                    ..AgentIf::up("Gi0/1", 1_000)
                }],
                ..AgentData::default()
            },
        )
        .await;
    harness.poll_and_evaluate(&id).await;
    assert_eq!(harness.open_alerts().len(), 1);

    tokio::time::sleep(MINUTE).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        agent.interfaces[0].oper_up = true;
    });
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());

    // A port that disappears from the table resolves too: it cannot clear
    // itself, so the rule does it.
    tokio::time::sleep(MINUTE).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        agent.interfaces[0].oper_up = false;
    });
    harness.poll_and_evaluate(&id).await;
    let alert = harness.open_alerts()[0].clone();
    tokio::time::sleep(MINUTE).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        agent.interfaces.clear();
    });
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());
    assert!(
        crate::alerts::get(&harness.app, &alert.id)
            .unwrap()
            .detail
            .contains("no longer reported")
    );
}

#[tokio::test(start_paused = true)]
async fn a_maintenance_window_mutes_without_hiding() {
    let harness = Harness::new().await;
    let id = device_down_only(&harness).await;
    let now = harness.now();
    harness
        .rpc(
            "maintenance.save",
            json!({"name": "Saturday night", "starts_at": now - 1_000,
                   "ends_at": now + 3_600_000, "scope": {"sites": []}}),
        )
        .await;
    harness.probe.down("10.0.0.1");
    for _ in 0..2 {
        harness.poll_and_evaluate(&id).await;
        tokio::time::sleep(MINUTE).await;
    }
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert!(
        open[0].muted_by_maintenance,
        "recorded, but nobody is woken"
    );
    assert!(open[0].muted(harness.now()));

    let status = harness.rpc("monitor.status", json!({})).await;
    assert!(
        status["problems"]
            .as_array()
            .unwrap()
            .iter()
            .any(|problem| problem["kind"] == "maintenance")
    );
}

#[tokio::test(start_paused = true)]
async fn a_snoozed_alert_is_muted_until_its_time_is_up() {
    let harness = Harness::new().await;
    let id = device_down_only(&harness).await;
    harness.probe.down("10.0.0.1");
    for _ in 0..2 {
        harness.poll_and_evaluate(&id).await;
        tokio::time::sleep(MINUTE).await;
    }
    let alert = harness.open_alerts()[0].clone();
    let snoozed = harness
        .rpc("alerts.snooze", json!({"id": alert.id, "minutes": 30}))
        .await;
    assert_eq!(
        snoozed["snoozed_until"].as_i64(),
        Some(harness.now() + 30 * 60_000)
    );
    let stored = crate::alerts::get(&harness.app, &alert.id).unwrap();
    assert!(stored.muted(harness.now()));
    tokio::time::sleep(Duration::from_secs(31 * 60)).await;
    assert!(!stored.muted(harness.now()));

    // Zero minutes cancels it.
    harness
        .rpc("alerts.snooze", json!({"id": alert.id, "minutes": 0}))
        .await;
    assert!(
        crate::alerts::get(&harness.app, &alert.id)
            .unwrap()
            .snoozed_until
            .is_none()
    );
}

#[tokio::test(start_paused = true)]
async fn lab_devices_alert_only_when_a_rule_asks_for_them() {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    let probe = std::sync::Arc::new(crate::testing::FakeProbe::default());
    builder.state(crate::state::MonitorState::fake(
        std::sync::Arc::new(crate::clock::VirtualClock::new(crate::testing::epoch())),
        probe,
        std::sync::Arc::new(crate::testing::FakeSnmp::default()),
    ));
    let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
    let id = lab.id("SW1");
    for rule in crate::rules::list(&lab.app).unwrap() {
        lab.app
            .rpc(
                "alerts.rules.save",
                json!({"id": rule.id, "enabled": rule.id == "builtin.uplink_down",
                       "condition": {"type": "interface_down", "only": "all"}}),
            )
            .await
            .unwrap();
    }
    // GigabitEthernet0/1 is shut on the fake, so the rule would match.
    lab.fake("SW1").with(|state| {
        state.link_down.push("GigabitEthernet0/1".into());
    });
    let poll = crate::poll::poll_device(&lab.app, &id).await.unwrap();
    crate::rules::evaluate(&lab.app, &poll).await;
    assert!(
        crate::alerts::list(&lab.app, &Default::default())
            .unwrap()
            .is_empty(),
        "a simulated device is not a reason to wake anybody"
    );

    lab.app
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.uplink_down", "scope": {"include_lab": true}}),
        )
        .await
        .unwrap();
    // Lab devices are read over the CLI every 30 s; let that come round.
    tokio::time::sleep(MINUTE).await;
    let poll = crate::poll::poll_device(&lab.app, &id).await.unwrap();
    crate::rules::evaluate(&lab.app, &poll).await;
    let alerts = crate::alerts::list(&lab.app, &Default::default()).unwrap();
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].lab, "and it is labelled as lab");
}

#[tokio::test(start_paused = true)]
async fn alerts_raise_deduplicates_on_the_key_the_caller_passes() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    let first = harness
        .rpc(
            "alerts.raise",
            json!({"rule": "log rule: authentication failures", "device_id": id,
                   "severity": "high", "title": "Repeated login failures",
                   "detail": "12 failures in 5 minutes", "dedup_key": "logs:auth_fail:SW1"}),
        )
        .await;
    assert_eq!(first["count"], 1);
    assert_eq!(first["rule_name"], "log rule: authentication failures");
    assert_eq!(first["device"], "SW1");

    let again = harness
        .rpc(
            "alerts.raise",
            json!({"severity": "high", "title": "Repeated login failures",
                   "device_id": id, "dedup_key": "logs:auth_fail:SW1"}),
        )
        .await;
    assert_eq!(again["id"], first["id"]);
    assert_eq!(again["count"], 2);
    assert_eq!(harness.open_alerts().len(), 1);

    // A different key is a different alert.
    harness
        .rpc(
            "alerts.raise",
            json!({"severity": "low", "title": "Something else", "dedup_key": "other"}),
        )
        .await;
    assert_eq!(harness.open_alerts().len(), 2);

    // Resolving and raising again starts a new one.
    harness
        .rpc("alerts.resolve", json!({"id": first["id"]}))
        .await;
    let third = harness
        .rpc(
            "alerts.raise",
            json!({"severity": "high", "title": "Repeated login failures",
                   "device_id": id, "dedup_key": "logs:auth_fail:SW1"}),
        )
        .await;
    assert_ne!(third["id"], first["id"]);
    assert_eq!(third["count"], 1);
}

#[tokio::test(start_paused = true)]
async fn alerts_raise_works_with_no_device_and_refuses_what_it_cannot_key() {
    let harness = Harness::new().await;
    let alert = harness
        .rpc(
            "alerts.raise",
            json!({"severity": "info", "title": "Backup storage is nearly full",
                   "dedup_key": "config:storage"}),
        )
        .await;
    assert!(alert["device_id"].is_null());
    assert_eq!(alert["severity"], "info");

    for bad in [
        json!({"severity": "info", "title": "", "dedup_key": "x"}),
        json!({"severity": "info", "title": "x", "dedup_key": "  "}),
    ] {
        let error = harness.app.rpc("alerts.raise", bad).await.unwrap_err();
        assert_eq!(error.code, "invalid");
    }
    let error = harness
        .app
        .rpc(
            "alerts.raise",
            json!({"severity": "info", "title": "x", "dedup_key": "y", "device_id": "nope"}),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "not_found");
}

#[tokio::test(start_paused = true)]
async fn a_configuration_change_raises_its_own_alert_per_change() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    crate::rules::on_config_changed(
        &harness.app,
        &id,
        &json!({"device_id": id, "kind": "running", "version_id": 7, "added": 3, "removed": 1}),
    )
    .unwrap();
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].severity, Severity::Info);
    assert!(open[0].detail.contains("3 lines added, 1 removed"));

    // The same change again is the same alert; a later version is a new one.
    crate::rules::on_config_changed(
        &harness.app,
        &id,
        &json!({"device_id": id, "kind": "running", "version_id": 7, "added": 3, "removed": 1}),
    )
    .unwrap();
    assert_eq!(harness.open_alerts().len(), 1);
    crate::rules::on_config_changed(
        &harness.app,
        &id,
        &json!({"device_id": id, "kind": "running", "version_id": 8, "added": 1, "removed": 0}),
    )
    .unwrap();
    assert_eq!(harness.open_alerts().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_metric_that_stops_arriving_raises_a_no_data_alert() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness
        .rpc(
            "alerts.rules.save",
            json!({"name": "No latency readings", "severity": "low",
                   "condition": {"type": "no_data", "metric": "icmp.rtt_ms", "for_s": 600}}),
        )
        .await;

    harness.poll_and_evaluate(&id).await;
    assert!(
        harness.open_alerts().is_empty(),
        "fresh data, nothing wrong"
    );

    // The device stops answering echoes, so no new rtt is written.
    harness.probe.set(
        "10.0.0.1",
        crate::testing::HostState {
            up: false,
            tcp_up: true,
            ..crate::testing::HostState::default()
        },
    );
    tokio::time::sleep(Duration::from_secs(601)).await;
    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert!(
        open[0].detail.contains("no new value"),
        "{}",
        open[0].detail
    );

    // And it resolves when the readings come back.
    tokio::time::sleep(MINUTE).await;
    harness.probe.up("10.0.0.1");
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_round_trip_time_far_above_the_median_is_noticed() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.rtt_high").await;
    harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.rtt_high", "for_s": 0}),
        )
        .await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;

    for _ in 0..10 {
        harness.probe.set(
            "10.0.0.1",
            crate::testing::HostState {
                rtt_ms: 2.0,
                ..crate::testing::HostState::default()
            },
        );
        harness.poll_and_evaluate(&id).await;
        tokio::time::sleep(MINUTE).await;
    }
    assert!(harness.open_alerts().is_empty());

    harness.probe.set(
        "10.0.0.1",
        crate::testing::HostState {
            rtt_ms: 40.0,
            ..crate::testing::HostState::default()
        },
    );
    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1, "{open:#?}");
    assert!(open[0].detail.contains("median"), "{}", open[0].detail);
    assert_eq!(open[0].value, Some(40.0));
}

#[tokio::test(start_paused = true)]
async fn acknowledging_keeps_an_alert_open_and_resolving_by_hand_closes_it() {
    let harness = Harness::new().await;
    let id = device_down_only(&harness).await;
    harness.probe.down("10.0.0.1");
    for _ in 0..2 {
        harness.poll_and_evaluate(&id).await;
        tokio::time::sleep(MINUTE).await;
    }
    let alert = harness.open_alerts()[0].clone();
    let acked = harness
        .rpc(
            "alerts.ack",
            json!({"id": alert.id, "note": "known, waiting for the electrician"}),
        )
        .await;
    assert_eq!(acked["state"], "acknowledged");
    assert_eq!(acked["ack_note"], "known, waiting for the electrician");
    assert_eq!(harness.open_alerts().len(), 1, "acknowledged is still open");

    // Another sighting keeps it acknowledged rather than re-firing.
    harness.poll_and_evaluate(&id).await;
    let still = crate::alerts::get(&harness.app, &alert.id).unwrap();
    assert_eq!(still.state, AlertState::Acknowledged);
    assert_eq!(still.count, 2);

    let resolved = harness
        .rpc(
            "alerts.resolve",
            json!({"id": alert.id, "note": "power back"}),
        )
        .await;
    assert_eq!(resolved["state"], "resolved");
    assert!(harness.open_alerts().is_empty());
    // Acknowledging a resolved alert is refused with a reason.
    let error = harness
        .app
        .rpc("alerts.ack", json!({"id": alert.id}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "conflict");
}

// ------------------------------------------------------------ concurrency

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn one_problem_raised_from_many_places_at_once_is_one_alert() {
    // The look-up and the write used to be two visits to the store, so two
    // callers could both find nothing open and both raise — or both find
    // the alert and both write "one more", which loses a sighting.
    //
    // More than one round, because how far the callers overlap is up to
    // the machine: on a busy one a single round let the two-visit raise
    // through half the time.
    let harness = Harness::new().await;
    const KEYS: usize = 24;
    const CALLERS: usize = 16;
    const ROUNDS: usize = 8;
    for _ in 0..ROUNDS {
        let gate = std::sync::Arc::new(tokio::sync::Barrier::new(KEYS * CALLERS));
        let mut callers = tokio::task::JoinSet::new();
        for key in 0..KEYS {
            for _ in 0..CALLERS {
                let app = harness.app.clone();
                let gate = gate.clone();
                callers.spawn(async move {
                    gate.wait().await;
                    app.rpc(
                        "alerts.raise",
                        json!({"severity": "high", "title": "Repeated login failures",
                               "dedup_key": format!("logs:auth_fail:{key}")}),
                    )
                    .await
                    .expect("the alert is raised");
                });
            }
        }
        while let Some(done) = callers.join_next().await {
            done.expect("no caller panicked");
        }
    }

    let alerts = harness.all_alerts();
    assert_eq!(
        alerts.len(),
        KEYS,
        "one alert per key, however many raised it"
    );
    for alert in &alerts {
        assert_eq!(
            alert.count,
            (ROUNDS * CALLERS) as u64,
            "every sighting of `{}` counted, none lost",
            alert.dedup_key
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_acknowledgement_that_lands_among_sightings_loses_none_of_them() {
    // Acknowledging read the alert, changed its copy and saved it, so a
    // sighting counted in between was overwritten by the copy the click
    // had read. Clicks and sightings of the same alerts, let go together.
    let harness = Harness::new().await;
    const KEYS: usize = 12;
    const SIGHTINGS: usize = 8;
    const ROUNDS: usize = 8;
    let raise = |key: usize| {
        json!({"severity": "high", "title": "Repeated login failures",
               "dedup_key": format!("logs:auth_fail:{key}")})
    };
    let mut ids = Vec::new();
    for key in 0..KEYS {
        let alert = harness.rpc("alerts.raise", raise(key)).await;
        ids.push(alert["id"].as_str().expect("an id").to_string());
    }

    for _ in 0..ROUNDS {
        // A click before every sighting, so each has one right behind it.
        let gate = std::sync::Arc::new(tokio::sync::Barrier::new(KEYS * SIGHTINGS * 2));
        let mut callers = tokio::task::JoinSet::new();
        for (key, id) in ids.iter().enumerate() {
            for _ in 0..SIGHTINGS {
                let (app, open, id) = (harness.app.clone(), gate.clone(), id.clone());
                callers.spawn(async move {
                    open.wait().await;
                    app.rpc("alerts.ack", json!({"id": id, "note": "on it"}))
                        .await
                        .expect("the alert is acknowledged");
                });
                let (app, open, params) = (harness.app.clone(), gate.clone(), raise(key));
                callers.spawn(async move {
                    open.wait().await;
                    app.rpc("alerts.raise", params)
                        .await
                        .expect("the sighting is counted");
                });
            }
        }
        while let Some(done) = callers.join_next().await {
            done.expect("no caller panicked");
        }
    }

    let alerts = harness.all_alerts();
    assert_eq!(alerts.len(), KEYS);
    for alert in &alerts {
        assert_eq!(
            alert.count,
            (1 + ROUNDS * SIGHTINGS) as u64,
            "every sighting of `{}` counted, whatever was clicked meanwhile",
            alert.dedup_key
        );
        assert_eq!(
            alert.state,
            AlertState::Acknowledged,
            "and no sighting undid the acknowledgement"
        );
        assert_eq!(alert.ack_note.as_deref(), Some("on it"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn checks_that_finish_together_raise_one_alert_each_and_count_one_sighting() {
    // `checks.run_now` on a host that has just died: its checks record
    // `down` within a millisecond of each other and every one of them has
    // the rules look at the whole device.
    let harness = Harness::new().await;
    harness.only_rule("builtin.service_check_down").await;
    const DEVICES: usize = 12;
    const CHECKS: usize = 12;
    let mut devices = Vec::new();
    for index in 0..DEVICES {
        let address = format!("10.0.{index}.1");
        harness.probe.down(&address);
        let device = harness
            .device(&format!("SW{index}"), &address, json!({}))
            .await;
        for check in 0..CHECKS {
            harness
                .check_on(
                    &device,
                    &format!("svc{check}"),
                    json!({"kind": "icmp"}),
                    json!({}),
                )
                .await;
        }
        devices.push(device);
    }
    for device in &devices {
        let ran = harness
            .rpc("checks.run_now", json!({"device_id": device}))
            .await;
        assert_eq!(ran["ran"], CHECKS);
    }

    let alerts = harness.all_alerts();
    let mut keys: Vec<&str> = alerts
        .iter()
        .map(|alert| alert.dedup_key.as_str())
        .collect();
    keys.sort_unstable();
    let raised = keys.len();
    keys.dedup();
    assert_eq!(
        (raised, keys.len()),
        (DEVICES * CHECKS, DEVICES * CHECKS),
        "one alert per check: {raised} raised for {} keys",
        keys.len()
    );
    let recounted: Vec<(&str, u64)> = alerts
        .iter()
        .filter(|alert| alert.count != 1)
        .map(|alert| (alert.title.as_str(), alert.count))
        .collect();
    assert!(
        recounted.is_empty(),
        "each check was seen down once, whoever else looked: {recounted:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn evaluations_of_one_device_that_overlap_raise_one_alert_and_lose_no_sighting() {
    // The scheduler's poll and a `monitor.poll_now` of the same device
    // arriving together: the rules engine itself, run from many tasks at
    // once over a poll that found a dozen ports down. Every evaluation of
    // a poll is a sighting, so the count says exactly how many got through.
    //
    // The test above cannot say as much. Looking twice at one run of a
    // check is one sighting, so all it has to go on is a second alert, and
    // on a busy machine it found none in ten runs out of ten with the raise
    // put back to two visits to the store. A lost sighting is far more
    // common than a duplicate — every evaluation after the first can lose
    // one, for every port — and several rounds leave little to chance.
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    harness.only_rule("builtin.uplink_down").await;
    harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.uplink_down", "condition": {"type": "interface_down", "only": "all"}}),
        )
        .await;
    const DEVICES: usize = 4;
    const PORTS: usize = 12;
    const EVALUATIONS: usize = 16;
    const ROUNDS: usize = 6;
    let mut polls = Vec::new();
    for index in 0..DEVICES {
        let device = harness
            .snmp_device(
                &format!("SW{index}"),
                &format!("10.0.{index}.1"),
                AgentData {
                    uptime_ticks: Some(1_000),
                    counter64: true,
                    interfaces: (1..=PORTS)
                        .map(|port| AgentIf {
                            oper_up: false,
                            ..AgentIf::up(&format!("Gi0/{port}"), 1_000)
                        })
                        .collect(),
                    ..AgentData::default()
                },
            )
            .await;
        let poll = harness.poll(&device).await;
        assert_eq!(poll.interfaces.len(), PORTS);
        polls.push(std::sync::Arc::new(poll));
    }
    assert!(harness.all_alerts().is_empty(), "nothing evaluated yet");

    for _ in 0..ROUNDS {
        let gate = std::sync::Arc::new(tokio::sync::Barrier::new(DEVICES * EVALUATIONS));
        let mut evaluations = tokio::task::JoinSet::new();
        for poll in &polls {
            for _ in 0..EVALUATIONS {
                let app = harness.app.clone();
                let gate = gate.clone();
                let poll = poll.clone();
                evaluations.spawn(async move {
                    gate.wait().await;
                    crate::rules::evaluate(&app, &poll).await;
                });
            }
        }
        while let Some(done) = evaluations.join_next().await {
            done.expect("no evaluation panicked");
        }
    }

    let alerts = harness.all_alerts();
    assert_eq!(
        alerts.len(),
        DEVICES * PORTS,
        "one alert per port, however many evaluations found it down"
    );
    for alert in &alerts {
        assert_eq!(
            alert.count,
            (ROUNDS * EVALUATIONS) as u64,
            "every evaluation of `{}` counted, none lost",
            alert.title
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_poll_between_two_runs_of_a_check_is_not_another_sighting_of_it() {
    // A poll judges the checks from what they last found. Looking at the
    // same verdict again is not seeing the service down again.
    let harness = Harness::new().await;
    harness.only_rule("builtin.service_check_down").await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.down("10.0.0.80");
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.0.0.80", "interval_s": 300}),
        )
        .await;
    harness.run_check(&check).await;
    let seen_at = harness.now();
    for _ in 0..4 {
        tokio::time::sleep(MINUTE).await;
        harness.poll_and_evaluate(&device).await;
    }
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].count, 1);
    assert_eq!(
        open[0].last_seen_at, seen_at,
        "last seen when it was last measured"
    );

    tokio::time::sleep(MINUTE).await;
    harness.run_check(&check).await;
    let open = harness.open_alerts();
    assert_eq!(open[0].count, 2, "the next run is the next sighting");
    assert_eq!(open[0].last_seen_at, harness.now());
}

#[tokio::test(start_paused = true)]
async fn an_alert_resolved_by_hand_stays_resolved_until_the_problem_is_seen_again() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.service_check_down").await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.down("10.0.0.80");
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.0.0.80", "interval_s": 300}),
        )
        .await;
    harness.run_check(&check).await;
    let first = harness.open_alerts()[0].clone();

    tokio::time::sleep(MINUTE).await;
    harness.rpc("alerts.resolve", json!({"id": first.id})).await;
    // The verdict the poll finds is the one the resolved alert was about.
    tokio::time::sleep(MINUTE).await;
    harness.poll_and_evaluate(&device).await;
    assert!(
        harness.open_alerts().is_empty(),
        "nothing new has been measured since it was resolved"
    );

    tokio::time::sleep(MINUTE).await;
    harness.run_check(&check).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1, "measured again, and still down");
    assert_ne!(open[0].id, first.id);
}

#[tokio::test(start_paused = true)]
async fn an_all_clear_older_than_the_alerts_last_sighting_resolves_nothing() {
    // An evaluation reads "up", is overtaken by the run that finds the
    // service down and by the alert that run raises, and arrives with its
    // all-clear afterwards. What it knows is older than what the alert knows.
    let harness = Harness::new().await;
    harness.only_rule("builtin.service_check_down").await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.0.0.80", "interval_s": 300}),
        )
        .await;
    harness.run_check(&check).await;
    let was_up_at = harness.now();

    tokio::time::sleep(MINUTE).await;
    harness.probe.down("10.0.0.80");
    harness.run_check(&check).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);

    let overtaken = crate::alerts::resolve_key(
        &harness.app,
        &open[0].dedup_key,
        harness.now(),
        Some(was_up_at),
        Some("The icmp check `web` is up."),
        &[],
    )
    .unwrap();
    assert!(overtaken.is_none());
    let still = harness.open_alerts();
    assert_eq!(still.len(), 1, "down, as far as anybody has measured");
    assert_eq!(still[0].detail, open[0].detail);

    // An all-clear measured after the sighting is another matter.
    tokio::time::sleep(MINUTE).await;
    harness.probe.up("10.0.0.80");
    harness.run_check(&check).await;
    assert!(harness.open_alerts().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_sighting_from_before_the_clearing_run_cannot_reopen_an_alert_a_slow_poll_resolved() {
    // A poll's evaluation is dated when the poll set out. One that was out
    // while the check was seen down once more and then came back resolves
    // the alert on the strength of that last run — and used to date the
    // resolve, and with it the alert's last word, before a sighting the
    // alert had already counted. An evaluation still holding that sighting
    // then raised a new alert for a service that was up.
    let harness = Harness::new().await;
    harness.only_rule("builtin.service_check_down").await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.down("10.0.0.80");
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.0.0.80", "interval_s": 300}),
        )
        .await;
    harness.run_check(&check).await;

    tokio::time::sleep(MINUTE).await;
    let slow_poll = harness.poll(&device).await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    harness.run_check(&check).await;
    let seen_down_at = harness.now();
    assert_eq!(harness.open_alerts()[0].last_seen_at, seen_down_at);

    // The clearing run, which the poll's evaluation gets to before the
    // run's own.
    tokio::time::sleep(Duration::from_secs(5)).await;
    let cleared_at = harness.now();
    let (stored, _) = crate::checks::store::require(&harness.app, &check).unwrap();
    crate::checks::store::record(
        &harness.app,
        &stored,
        "SW1",
        &crate::checks::outcome::Outcome::up("ok", Some(1.0)),
        cleared_at,
        cleared_at,
    )
    .unwrap();
    crate::rules::evaluate(&harness.app, &slow_poll).await;
    let resolved = harness.all_alerts().remove(0);
    assert_eq!(resolved.state, AlertState::Resolved);

    // The evaluation that had read the second `down` and was overtaken.
    let (_, raised) = crate::alerts::fire(
        &harness.app,
        &crate::alerts::AlertSpec {
            dedup_key: resolved.dedup_key.clone(),
            rule_id: resolved.rule_id.clone(),
            rule_name: resolved.rule_name.clone(),
            device_id: resolved.device_id.clone(),
            instance: resolved.instance.clone(),
            severity: resolved.severity,
            title: resolved.title.clone(),
            observed_at: Some(seen_down_at),
            ..Default::default()
        },
        harness.now(),
    )
    .unwrap();
    assert!(
        !raised,
        "that sighting is what the resolved alert was about"
    );
    assert!(harness.open_alerts().is_empty());
    assert_eq!(
        (resolved.resolved_at, resolved.last_seen_at),
        (Some(cleared_at), cleared_at),
        "resolved as of the run that cleared it, not as of a poll that set out earlier"
    );
}

#[tokio::test(start_paused = true)]
async fn resolving_one_of_two_alerts_left_under_one_key_closes_the_one_that_was_asked_for() {
    // Two open alerts under one key cannot be raised any more, but a
    // database that ran the earlier version may hold some. Resolving went
    // by key, so clicking the older of the two closed the newer.
    let harness = Harness::new().await;
    let now = harness.now();
    for (id, started_at) in [("older", now - 120_000), ("newer", now - 60_000)] {
        let body = serde_json::to_string(&crate::model::Alert {
            id: id.into(),
            dedup_key: "k1".into(),
            title: "Something".into(),
            severity: Severity::Critical,
            started_at,
            last_seen_at: started_at,
            count: 1,
            notified_at: Some(started_at),
            ..Default::default()
        })
        .unwrap();
        harness
            .app
            .store()
            .with(|conn| {
                conn.execute(
                    "INSERT INTO monitor_alerts
                     (id, dedup_key, state, severity, device_id, started_at, last_seen_at, body)
                     VALUES (?1, 'k1', 'firing', 4, NULL, ?2, ?2, ?3)",
                    rusqlite::params![id, started_at, body],
                )
            })
            .unwrap();
    }

    let resolved = harness.rpc("alerts.resolve", json!({"id": "older"})).await;
    assert_eq!(
        (resolved["id"].as_str(), resolved["state"].as_str()),
        (Some("older"), Some("resolved"))
    );
    let open = harness.open_alerts();
    assert_eq!(
        open.iter()
            .map(|alert| alert.id.as_str())
            .collect::<Vec<_>>(),
        ["newer"]
    );
}

// ------------------------------------------------------------------ events

/// The `notification` of every `alerts.changed` published since the last
/// call, with the state the alert was in.
fn announced(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
) -> Vec<(String, serde_json::Value)> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic == "alerts.changed" {
            seen.push((
                event.payload["alert"]["state"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                event.payload["notification"].clone(),
            ));
        }
    }
    seen
}

#[tokio::test(start_paused = true)]
async fn the_alert_event_says_which_notification_it_carries_if_any() {
    // The desktop shell turns this event into a system notification, and
    // cannot work out for itself whether a window or a snooze covers it.
    let harness = Harness::new().await;
    let mut events = harness.app.subscribe();
    let raise = json!({"severity": "critical", "title": "Something", "dedup_key": "k1"});

    let alert = harness.rpc("alerts.raise", raise.clone()).await;
    assert_eq!(announced(&mut events), [("firing".into(), json!("firing"))]);
    harness.rpc("alerts.raise", raise.clone()).await;
    assert_eq!(
        announced(&mut events),
        [("firing".into(), json!(null))],
        "a repeat is not news"
    );
    harness
        .rpc("alerts.snooze", json!({"id": alert["id"], "minutes": 5}))
        .await;
    harness.rpc("alerts.ack", json!({"id": alert["id"]})).await;
    assert_eq!(
        announced(&mut events),
        [
            ("firing".into(), json!(null)),
            ("acknowledged".into(), json!(null))
        ]
    );
    harness
        .rpc("alerts.resolve", json!({"id": alert["id"]}))
        .await;
    assert_eq!(
        announced(&mut events),
        [("resolved".into(), json!("resolved"))]
    );

    // Inside a maintenance window the alert is published without one, and
    // the sighting that finds the window closed is the one that carries it.
    let now = harness.now();
    harness
        .rpc(
            "maintenance.save",
            json!({"name": "window", "starts_at": now, "ends_at": now + 3_600_000}),
        )
        .await;
    harness.rpc("alerts.raise", raise.clone()).await;
    assert_eq!(announced(&mut events), [("firing".into(), json!(null))]);
    tokio::time::sleep(Duration::from_secs(3_601)).await;
    let late = harness.rpc("alerts.raise", raise.clone()).await;
    assert_eq!(late["count"], 2);
    assert_eq!(announced(&mut events), [("firing".into(), json!("firing"))]);
}
