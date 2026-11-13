//! Reachability, damping, and the rule that a failed read is a gap.

use std::time::Duration;

use serde_json::json;

use crate::model::DeviceRecord;
use crate::testing::{AgentData, AgentIf, Harness, HostState};

#[tokio::test(start_paused = true)]
async fn a_device_that_answers_is_up_with_a_round_trip_time() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.set(
        "10.0.0.1",
        HostState {
            rtt_ms: 2.5,
            ..HostState::default()
        },
    );

    let poll = harness.poll(&id).await;
    assert!(poll.ok && !poll.down);
    assert_eq!(poll.via.as_deref(), Some("icmp"));
    assert_eq!(poll.measure("up", ""), Some(1.0));
    assert_eq!(poll.measure("icmp.rtt_ms", ""), Some(2.5));
    assert_eq!(poll.measure("icmp.loss_pct", ""), Some(0.0));

    let state = nocturne_app::inventory::state_of(&harness.app, &id).unwrap();
    assert_eq!(state.reachable, Some(true));
    assert_eq!(state.rtt_ms, Some(2.5));
    assert_eq!(state.last_seen, Some(harness.now()));
    assert!(state.last_error.is_none());
}

#[tokio::test(start_paused = true)]
async fn partial_loss_is_recorded_without_calling_the_device_down() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.set(
        "10.0.0.1",
        HostState {
            lost: 2,
            ..HostState::default()
        },
    );
    let poll = harness.poll(&id).await;
    assert!(poll.ok);
    // Three echoes, two lost.
    assert_eq!(
        poll.measure("icmp.loss_pct", "").map(|loss| loss.round()),
        Some(67.0)
    );
}

#[tokio::test(start_paused = true)]
async fn down_takes_two_failures_and_up_takes_one_success() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.down("10.0.0.1");

    // One failure: still up, because one lost poll is not an outage.
    let first = harness.poll(&id).await;
    assert!(!first.ok && !first.down && first.transition.is_none());
    assert_eq!(
        nocturne_app::inventory::state_of(&harness.app, &id)
            .unwrap()
            .reachable,
        Some(true)
    );

    tokio::time::sleep(Duration::from_secs(60)).await;
    let second = harness.poll(&id).await;
    assert!(second.down, "the second failure in a row is an outage");
    assert_eq!(second.transition, Some(false));

    // Still down: the transition is recorded once, not on every poll.
    tokio::time::sleep(Duration::from_secs(60)).await;
    let third = harness.poll(&id).await;
    assert!(third.down && third.transition.is_none());

    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.probe.up("10.0.0.1");
    let back = harness.poll(&id).await;
    assert!(back.ok && !back.down);
    assert_eq!(back.transition, Some(true), "up on the first success");

    let events = harness.events_of(&id);
    let transitions: Vec<&str> = events
        .iter()
        .filter_map(|entry| entry.body.as_ref()?["event"].as_str())
        .collect();
    assert_eq!(
        transitions,
        ["device_up", "device_down"],
        "newest first: one down, one up, and nothing else"
    );
}

#[tokio::test(start_paused = true)]
async fn a_longer_damping_setting_is_honoured() {
    let harness = Harness::new().await;
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"down_after": 4}}),
        )
        .await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.down("10.0.0.1");
    for expected in [false, false, false, true] {
        let poll = harness.poll(&id).await;
        assert_eq!(poll.down, expected);
        tokio::time::sleep(Duration::from_secs(60)).await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_device_that_drops_icmp_is_reached_on_its_management_port() {
    let harness = Harness::new().await;
    let id = harness
        .device("SW1", "10.0.0.1", json!({"transport": "ssh", "port": 2222}))
        .await;
    harness.probe.set(
        "10.0.0.1",
        HostState {
            up: false,
            tcp_up: true,
            rtt_ms: 7.0,
            ..HostState::default()
        },
    );
    let poll = harness.poll(&id).await;
    assert!(poll.ok);
    assert_eq!(poll.via.as_deref(), Some("tcp/2222"));
    // The loss ICMP measured is real and is recorded; the TCP handshake is
    // not an echo, so it is not written as one.
    assert_eq!(poll.measure("icmp.loss_pct", ""), Some(100.0));
    assert_eq!(poll.measure("icmp.rtt_ms", ""), None);
    assert_eq!(
        nocturne_app::inventory::state_of(&harness.app, &id)
            .unwrap()
            .rtt_ms,
        Some(7.0)
    );
}

#[tokio::test(start_paused = true)]
async fn a_probe_this_machine_cannot_send_is_not_a_measurement() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.set(
        "10.0.0.1",
        HostState {
            error: Some("permission denied: raw sockets need CAP_NET_RAW".into()),
            tcp_up: false,
            ..HostState::default()
        },
    );
    let poll = harness.poll(&id).await;
    assert!(!poll.ok);
    assert_eq!(
        poll.measure("icmp.loss_pct", ""),
        None,
        "nothing was sent, so nothing was lost"
    );
    assert_eq!(poll.measure("up", ""), Some(0.0));
    assert!(
        poll.error.as_deref().unwrap().contains("CAP_NET_RAW"),
        "{:?}",
        poll.error
    );
}

#[tokio::test(start_paused = true)]
async fn a_device_that_did_not_answer_writes_no_zeroes() {
    let harness = Harness::new().await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(100_000),
                counter64: true,
                interfaces: vec![AgentIf::up("Gi0/1", 1_000)],
                ..AgentData::default()
            },
        )
        .await;
    harness.with_snmp_credential("public").await;

    // Two good polls, so there are rates to lose.
    harness.poll(&id).await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(106_000);
        agent.interfaces[0].in_octets = 7_500_000;
    });
    let good = harness.poll(&id).await;
    assert_eq!(
        good.measure("if.in_bps", "GigabitEthernet0/1"),
        Some(1_000_000.0),
        "7.5 MB in 60 s is 1 Mbit/s"
    );

    // Now the device goes away entirely.
    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.probe.down("10.0.0.1");
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.fail = Some("timed out".into())
    });
    let bad = harness.poll(&id).await;

    let metrics: Vec<&str> = bad
        .measures
        .iter()
        .map(|measure| measure.metric.as_str())
        .collect();
    assert_eq!(
        metrics,
        ["icmp.loss_pct", "up"],
        "a failed poll writes only what it really measured"
    );
    assert_eq!(bad.measure("up", ""), Some(0.0));

    // And nothing landed in the series at that moment either.
    for metric in ["if.in_bps", "if.out_bps", "cpu.pct", "icmp.rtt_ms"] {
        let instance = if metric.starts_with("if.") {
            "GigabitEthernet0/1"
        } else {
            ""
        };
        let samples = harness.samples(&id, metric, instance);
        assert!(
            !samples.iter().any(|(ts, _)| *ts == bad.at),
            "{metric} should have a gap at the failed poll, found {samples:?}"
        );
    }
    assert_eq!(bad.sources.snmp.ok, Some(false));
    assert!(bad.sources.snmp.error.is_some());
}

#[tokio::test(start_paused = true)]
async fn snmp_answering_proves_a_device_that_drops_icmp_is_there() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(5_000),
                counter64: true,
                ..AgentData::default()
            },
        )
        .await;
    harness.probe.set(
        "10.0.0.1",
        HostState {
            up: false,
            tcp_up: false,
            ..HostState::default()
        },
    );
    let poll = harness.poll(&id).await;
    assert!(poll.ok);
    assert_eq!(poll.via.as_deref(), Some("snmp"));
    assert_eq!(poll.measure("uptime_s", ""), Some(50.0));
    assert!(poll.error.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_lab_device_is_asked_of_the_simulator_and_never_pinged() {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    let probe = std::sync::Arc::new(crate::testing::FakeProbe::default());
    let clock = std::sync::Arc::new(crate::clock::VirtualClock::new(crate::testing::epoch()));
    builder.state(crate::state::MonitorState::fake(
        clock,
        probe.clone(),
        std::sync::Arc::new(crate::testing::FakeSnmp::default()),
    ));
    let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
    let id = lab.id("SW1");

    let poll = crate::poll::poll_device(&lab.app, &id).await.unwrap();
    assert!(poll.ok && poll.lab);
    assert_eq!(poll.via.as_deref(), Some("sim"));
    assert!(
        probe.pinged.lock().is_empty(),
        "a lab address must never be pinged on the real network"
    );
    // The console reached the simulator, and the CLI fallback read its ports.
    assert!(poll.interfaces_read);
    assert!(
        poll.interfaces
            .iter()
            .any(|row| row.name == "GigabitEthernet0/1" && row.lab)
    );
    assert_eq!(poll.sources.cli.ok, Some(true));
}

#[tokio::test(start_paused = true)]
async fn the_damped_verdict_survives_a_restart_of_the_process() {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.down("10.0.0.1");
    harness.poll(&id).await;
    let record: DeviceRecord = harness
        .app
        .store()
        .get(crate::db::collections::DEVICES, &id)
        .unwrap()
        .unwrap();
    assert_eq!((record.failures, record.down), (1, false));

    // The counters live only in memory, but the damping state is a document.
    harness
        .app
        .state::<crate::state::MonitorState>()
        .forget(&id);
    tokio::time::sleep(Duration::from_secs(60)).await;
    let poll = harness.poll(&id).await;
    assert!(poll.down, "the earlier failure was not forgotten");
}

#[tokio::test(start_paused = true)]
async fn a_device_with_nothing_to_probe_it_with_is_refused_clearly() {
    let harness = Harness::new().await;
    let id = harness
        .device(
            "SW1",
            "10.0.0.1",
            json!({"transport": "none", "monitor": {"icmp": false, "snmp": false}}),
        )
        .await;
    let error = crate::poll::poll_device(&harness.app, &id)
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
    assert!(error.message.contains("no way to be reached"), "{error}");

    let status = harness.rpc("monitor.status", json!({})).await;
    let problems = status["problems"].as_array().unwrap();
    assert!(
        problems
            .iter()
            .any(|problem| problem["kind"] == "no_probe" && problem["device_id"] == id.as_str()),
        "{problems:#?}"
    );
}
