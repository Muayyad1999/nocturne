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
