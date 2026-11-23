//! Counter arithmetic end to end: what a graph will actually show after a
//! wrap, after a reboot, and on the first poll of all.

use std::time::Duration;

use serde_json::json;

use crate::testing::{AgentData, AgentIf, Harness};

const PORT: &str = "GigabitEthernet0/1";

async fn switch(harness: &Harness, counter64: bool) -> String {
    harness.with_snmp_credential("public").await;
    harness
        .snmp_device(
            "SW1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(1_000_000),
                counter64,
                interfaces: vec![AgentIf {
                    in_octets: 1_000_000,
                    out_octets: 2_000_000,
                    ..AgentIf::up(PORT, 1_000)
                }],
                ..AgentData::default()
            },
        )
        .await
}

/// Move the agent on by `seconds`, adding the given octets and errors.
fn advance(harness: &Harness, seconds: u32, in_octets: u64, out_octets: u64, errors: u64) {
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap_or(0) + seconds * 100);
        agent.interfaces[0].in_octets = agent.interfaces[0].in_octets.wrapping_add(in_octets);
        agent.interfaces[0].out_octets = agent.interfaces[0].out_octets.wrapping_add(out_octets);
        agent.interfaces[0].in_errors = agent.interfaces[0].in_errors.wrapping_add(errors);
    });
}

#[tokio::test(start_paused = true)]
async fn the_first_poll_has_no_rates_because_there_is_nothing_to_subtract() {
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    let first = harness.poll(&id).await;
    assert!(first.interfaces_read);
    assert_eq!(first.interfaces[0].in_bps, None);
    assert_eq!(first.interfaces[0].util_pct, None);
    // But the things that need no history are there.
    assert_eq!(first.measure("if.oper", PORT), Some(1.0));
    assert_eq!(first.measure("uptime_s", ""), Some(10_000.0));
    assert_eq!(first.interfaces[0].speed_bps, Some(1_000_000_000));
}

#[tokio::test(start_paused = true)]
async fn rates_come_from_counter_deltas_over_uptime_deltas() {
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    harness.poll(&id).await;

    tokio::time::sleep(Duration::from_secs(60)).await;
    // 60 s of device time, 75 MB in, 7.5 MB out, 600 errors.
    advance(&harness, 60, 75_000_000, 7_500_000, 600);
    let poll = harness.poll(&id).await;
    let port = &poll.interfaces[0];
    assert_eq!(port.in_bps, Some(10_000_000.0));
    assert_eq!(port.out_bps, Some(1_000_000.0));
    assert_eq!(port.util_pct, Some(1.0), "10 Mbit/s of a 1 Gbit/s port");
    assert_eq!(port.in_err_ps, Some(10.0));
    assert_eq!(port.out_err_ps, Some(0.0), "a real zero, measured");
    assert_eq!(poll.measure("if.util_pct", PORT), Some(1.0));
}

#[tokio::test(start_paused = true)]
async fn the_poller_uses_device_time_not_its_own() {
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    harness.poll(&id).await;

    // The poller slept for five minutes, but the device only counts 60 s of
    // uptime (a slow agent, a paused VM): the rate follows the device.
    tokio::time::sleep(Duration::from_secs(300)).await;
    advance(&harness, 60, 75_000_000, 0, 0);
    let poll = harness.poll(&id).await;
    assert_eq!(poll.interfaces[0].in_bps, Some(10_000_000.0));
}

#[tokio::test(start_paused = true)]
async fn a_64_bit_counter_that_wraps_is_not_a_spike() {
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.interfaces[0].in_octets = u64::MAX - 999;
    });
    harness.poll(&id).await;

    tokio::time::sleep(Duration::from_secs(60)).await;
    // 1000 past the end of the range, then 7.5 MB more.
    advance(&harness, 60, 1_000 + 7_500_000, 0, 0);
    let poll = harness.poll(&id).await;
    assert_eq!(
        poll.interfaces[0].in_bps,
        Some((7_500_000 + 1_000) as f64 * 8.0 / 60.0),
        "the wrap is 7.5 MB in 60 s, not 1.8 × 10^19 bytes"
    );
}

#[tokio::test(start_paused = true)]
async fn a_64_bit_counter_that_starts_again_without_a_reboot_is_a_gap() {
    // Statistics cleared on the device, a line card re-seated: the counter
    // goes back to nothing while the uptime carries on. It had not counted
    // anywhere near 2^64, so it did not wrap.
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.interfaces[0].in_octets = 5_000_000_000;
    });
    harness.poll(&id).await;

    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        agent.interfaces[0].in_octets = 1_000;
        agent.interfaces[0].out_octets += 7_500_000;
    });
    let poll = harness.poll(&id).await;
    assert!(!poll.rebooted);
    assert_eq!(
        poll.interfaces[0].in_bps, None,
        "a gap, not 2.4 × 10^18 bit/s"
    );
    assert_eq!(poll.measure("if.in_bps", PORT), None);
    assert_eq!(
        poll.interfaces[0].out_bps,
        Some(1_000_000.0),
        "the counter that kept counting still has its rate"
    );
    assert_eq!(poll.interfaces[0].util_pct, Some(0.1));
}

#[tokio::test(start_paused = true)]
async fn a_32_bit_counter_that_wraps_is_not_a_spike() {
    let harness = Harness::new().await;
    // No 64-bit columns: an old agent, or SNMPv1.
    let id = switch(&harness, false).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.interfaces[0].in_octets = u64::from(u32::MAX) - 999;
    });
    harness.poll(&id).await;

    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        // 1000 to wrap, then 7.5 MB.
        agent.interfaces[0].in_octets = 7_500_000;
    });
    let poll = harness.poll(&id).await;
    assert_eq!(
        poll.interfaces[0].in_bps,
        Some((7_500_000 + 1_000) as f64 * 8.0 / 60.0)
    );
}

#[tokio::test(start_paused = true)]
async fn a_reboot_throws_the_interval_away_rather_than_inventing_a_spike() {
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    harness.poll(&id).await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    advance(&harness, 60, 75_000_000, 0, 0);
    let steady = harness.poll(&id).await;
    assert_eq!(steady.interfaces[0].in_bps, Some(10_000_000.0));

    // The switch restarts: uptime back to 30 s, counters from zero.
    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(3_000);
        agent.interfaces[0].in_octets = 400_000;
        agent.interfaces[0].out_octets = 0;
        agent.interfaces[0].in_errors = 0;
    });
    let after = harness.poll(&id).await;
    assert!(after.rebooted);
    assert_eq!(
        after.interfaces[0].in_bps, None,
        "no rate across a restart: a gap, not a spike"
    );
    assert_eq!(after.measure("if.in_bps", PORT), None);
    assert_eq!(after.measure("uptime_s", ""), Some(30.0));
    assert!(
        harness
            .events_of(&id)
            .iter()
            .any(|entry| entry.message.contains("restarted")),
        "the restart is worth an event"
    );

    // The poll after the restart has a baseline again.
    tokio::time::sleep(Duration::from_secs(60)).await;
    advance(&harness, 60, 7_500_000, 0, 0);
    let recovered = harness.poll(&id).await;
    assert!(!recovered.rebooted);
    assert_eq!(recovered.interfaces[0].in_bps, Some(1_000_000.0));
}

#[tokio::test(start_paused = true)]
async fn a_failed_read_between_two_good_ones_does_not_stretch_a_rate() {
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    harness.poll(&id).await;

    // The middle poll fails, so its counters are unknown.
    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.fail = Some("timed out".into())
    });
    let failed = harness.poll(&id).await;
    assert!(!failed.interfaces_read);

    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.snmp.edit("10.0.0.1:161", |agent| agent.fail = None);
    advance(&harness, 120, 150_000_000, 0, 0);
    let back = harness.poll(&id).await;
    assert_eq!(
        back.interfaces[0].in_bps, None,
        "the counters held before the failure were dropped, so there is nothing to subtract from"
    );

    tokio::time::sleep(Duration::from_secs(60)).await;
    advance(&harness, 60, 7_500_000, 0, 0);
    assert_eq!(
        harness.poll(&id).await.interfaces[0].in_bps,
        Some(1_000_000.0)
    );
}

#[tokio::test(start_paused = true)]
async fn an_interface_that_goes_down_reports_zero_oper_and_keeps_its_row() {
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    harness.poll(&id).await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        agent.interfaces[0].oper_up = false;
    });
    let poll = harness.poll(&id).await;
    assert_eq!(poll.measure("if.oper", PORT), Some(0.0));
    assert_eq!(poll.interfaces[0].admin_up, Some(true));
    assert_eq!(poll.interfaces[0].oper_up, Some(false));

    let rows = harness
        .rpc(
            "monitor.interfaces",
            json!({"device_id": id, "problem_only": true}),
        )
        .await;
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["name"], PORT);
}

#[tokio::test(start_paused = true)]
async fn snmp_interfaces_reach_the_collected_store_for_topology_to_use() {
    let harness = Harness::new().await;
    let id = switch(&harness, true).await;
    harness.poll(&id).await;
    let stored =
        nocturne_app::collect::latest(&harness.app, &id, nocturne_drivers::model::Kind::Interfaces)
            .unwrap()
            .expect("the ports of an SNMP-only device are collected");
    assert_eq!(stored.source, "snmp");
    let interfaces = stored.typed::<nocturne_drivers::model::Interface>();
    assert_eq!(interfaces.len(), 1);
    assert_eq!(interfaces[0].name, PORT);
    assert_eq!(interfaces[0].oper_up, Some(true));
    assert_eq!(interfaces[0].counters.in_octets, Some(1_000_000));

    // It is not rewritten on every poll: the previous generation has to stay
    // meaningful for "what changed since last time".
    tokio::time::sleep(Duration::from_secs(60)).await;
    advance(&harness, 60, 7_500_000, 0, 0);
    harness.poll(&id).await;
    let again =
        nocturne_app::collect::latest(&harness.app, &id, nocturne_drivers::model::Kind::Interfaces)
            .unwrap()
            .unwrap();
    assert_eq!(again.collected_at, stored.collected_at);

    // After the collection cadence it is refreshed.
    tokio::time::sleep(Duration::from_secs(300)).await;
    advance(&harness, 300, 1, 0, 0);
    harness.poll(&id).await;
    let refreshed =
        nocturne_app::collect::latest(&harness.app, &id, nocturne_drivers::model::Kind::Interfaces)
            .unwrap()
            .unwrap();
    assert!(refreshed.collected_at > stored.collected_at);
}
