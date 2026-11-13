//! CPU, memory and temperature: the numbers that come from the OIDs a
//! device's profile names, and the rules that watch them.

use serde_json::json;

use crate::testing::{AgentData, AgentIf, Harness};

/// A profile whose `snmp:` block points at a processor table, a memory pool
/// and an ENTITY-SENSOR temperature column.
const PROFILE: &str = r#"
id: test_health
name: Test device with health OIDs
vendor: Test
os: Test
cli:
  prompt: '^\S+[>#]\s?$'
policy:
  read_only: ['^show ']
snmp:
  cpu: ['1.3.6.1.4.1.9.9.109.1.1.1.1.6']
  memory_used: ['1.3.6.1.4.1.9.9.48.1.1.1.5']
  memory_free: ['1.3.6.1.4.1.9.9.48.1.1.1.6']
  temperature: ['1.3.6.1.2.1.99.1.1.1.4']
"#;

fn gauge(value: u32) -> nocturne_snmp::Value {
    nocturne_snmp::Value::Gauge32(value)
}

async fn device(harness: &Harness, cpu: u32, used: u32, free: u32, hot: u32) -> String {
    nocturne_app::profiles::save(&harness.app, PROFILE).expect("the profile loads");
    harness.with_snmp_credential("public").await;
    harness.snmp.set(
        "10.0.0.1:161",
        AgentData {
            uptime_ticks: Some(500_000),
            counter64: true,
            interfaces: vec![AgentIf::up("Gi0/1", 1_000)],
            scalars: vec![
                ("1.3.6.1.4.1.9.9.109.1.1.1.1.6.1".into(), gauge(cpu)),
                ("1.3.6.1.4.1.9.9.48.1.1.1.5.1".into(), gauge(used)),
                ("1.3.6.1.4.1.9.9.48.1.1.1.6.1".into(), gauge(free)),
                ("1.3.6.1.2.1.99.1.1.1.4.7".into(), gauge(hot)),
                ("1.3.6.1.2.1.99.1.1.1.4.8".into(), gauge(40)),
            ],
            ..AgentData::default()
        },
    );
    harness
        .device(
            "SW1",
            "10.0.0.1",
            json!({"profile": "test_health", "credential_id": harness.credential_id,
                   "monitor": {"snmp": true}}),
        )
        .await
}

#[tokio::test(start_paused = true)]
async fn cpu_memory_and_every_sensor_are_recorded() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let id = device(&harness, 42, 300, 100, 71).await;
    let poll = harness.poll(&id).await;

    assert_eq!(poll.measure("cpu.pct", ""), Some(42.0));
    assert_eq!(
        poll.measure("mem.pct", ""),
        Some(75.0),
        "300 used of 300 + 100 free"
    );
    assert_eq!(poll.measure("temp.c", "7"), Some(71.0));
    assert_eq!(poll.measure("temp.c", "8"), Some(40.0));
    assert_eq!(poll.health.cpu_pct, Some(42.0));
    assert_eq!(poll.health.mem_pct, Some(75.0));
    assert_eq!(poll.health.temp_c, Some(71.0), "the hottest sensor");
    assert_eq!(poll.health.uptime_s, Some(5_000));

    let view = harness
        .rpc("monitor.device", json!({"device_id": id}))
        .await;
    assert_eq!(view["health"]["cpu_pct"], 42.0);
    assert_eq!(view["health"]["temp_c"], 71.0);
    assert_eq!(view["health"]["at"], harness.now());

    let series = harness
        .rpc("metrics.series", json!({"device_id": id}))
        .await;
    let listed: Vec<(String, String)> = series
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["metric"].as_str().unwrap().to_string(),
                entry["instance"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    for wanted in [
        ("cpu.pct", ""),
        ("mem.pct", ""),
        ("temp.c", "7"),
        ("temp.c", "8"),
        ("uptime_s", ""),
        ("if.oper", "GigabitEthernet0/1"),
        ("icmp.rtt_ms", ""),
        ("icmp.loss_pct", ""),
        ("up", ""),
    ] {
        assert!(
            listed
                .iter()
                .any(|(metric, instance)| metric == wanted.0 && instance == wanted.1),
            "{wanted:?} is missing from {listed:?}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn memory_from_used_and_total_rather_than_used_and_free() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    nocturne_app::profiles::save(
        &harness.app,
        r#"
id: test_health_total
name: Test device reporting a total
vendor: Test
os: Test
cli:
  prompt: '^\S+[>#]\s?$'
snmp:
  memory_used: ['1.3.6.1.4.1.2021.4.6.0']
  memory_total: ['1.3.6.1.4.1.2021.4.5.0']
"#,
    )
    .expect("the profile loads");
    harness.with_snmp_credential("public").await;
    harness.snmp.set(
        "10.0.0.2:161",
        AgentData {
            uptime_ticks: Some(100),
            counter64: true,
            scalars: vec![
                ("1.3.6.1.4.1.2021.4.6.0".into(), gauge(512)),
                ("1.3.6.1.4.1.2021.4.5.0".into(), gauge(2_048)),
            ],
            ..AgentData::default()
        },
    );
    let id = harness
        .device(
            "SW2",
            "10.0.0.2",
            json!({"profile": "test_health_total", "credential_id": harness.credential_id,
                   "monitor": {"snmp": true}}),
        )
        .await;
    let poll = harness.poll(&id).await;
    assert_eq!(poll.measure("mem.pct", ""), Some(25.0));
    assert_eq!(poll.measure("cpu.pct", ""), None, "no CPU OID, no guess");
}

#[tokio::test(start_paused = true)]
async fn the_built_in_temperature_rule_names_the_sensor_that_is_too_hot() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.temperature_high").await;
    let id = device(&harness, 10, 100, 900, 78).await;
    harness.poll_and_evaluate(&id).await;

    let open = harness.open_alerts();
    assert_eq!(open.len(), 1, "{open:#?}");
    assert_eq!(open[0].instance.as_deref(), Some("7"));
    assert_eq!(open[0].value, Some(78.0));
    assert!(open[0].detail.contains("temp.c on 7"), "{}", open[0].detail);

    // 67 °C is below the alarm but above the 65 °C clear value, so it holds.
    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        agent.scalars[3].1 = gauge(67);
    });
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    harness.poll_and_evaluate(&id).await;
    assert_eq!(harness.open_alerts().len(), 1);

    harness.snmp.edit("10.0.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        agent.scalars[3].1 = gauge(30);
    });
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    harness.poll_and_evaluate(&id).await;
    assert!(harness.open_alerts().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_built_in_cpu_rule_waits_five_minutes() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.cpu_high").await;
    let id = device(&harness, 97, 100, 900, 20).await;

    for minute in 0..5 {
        harness.poll_and_evaluate(&id).await;
        assert!(
            harness.open_alerts().is_empty(),
            "fired after only {minute} minutes"
        );
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        harness.snmp.edit("10.0.0.1:161", |agent| {
            agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
        });
    }
    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].severity, crate::model::Severity::High);
    assert_eq!(open[0].value, Some(97.0));
    assert!(open[0].instance.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_profile_with_no_snmp_oids_reports_interfaces_and_nothing_invented() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    harness.with_snmp_credential("public").await;
    let id = harness
        .snmp_device(
            "SW3",
            "10.0.0.3",
            AgentData {
                uptime_ticks: Some(100),
                counter64: true,
                interfaces: vec![AgentIf::up("Gi0/1", 1_000)],
                ..AgentData::default()
            },
        )
        .await;
    let poll = harness.poll(&id).await;
    assert_eq!(poll.health.cpu_pct, None);
    assert_eq!(poll.health.mem_pct, None);
    assert_eq!(poll.health.temp_c, None);
    assert_eq!(poll.health.uptime_s, Some(1));
    assert!(poll.interfaces_read);
}
