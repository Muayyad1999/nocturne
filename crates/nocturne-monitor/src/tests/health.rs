//! CPU, memory and temperature: the numbers that come from the OIDs a
//! device's profile names, and the rules that watch them.

use nocturne_snmp::Value;
use serde_json::json;

use crate::poll::Poll;
use crate::testing::{AgentData, AgentIf, Harness};

/// A profile whose `snmp:` block points at a processor table, a memory pool
/// and a column that holds nothing but temperatures, in the plain form every
/// profile used before an entry could be a mapping.
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
  temperature: ['1.3.6.1.4.1.9.9.13.1.3.1.3']
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
                ("1.3.6.1.4.1.9.9.13.1.3.1.3.7".into(), gauge(hot)),
                ("1.3.6.1.4.1.9.9.13.1.3.1.3.8".into(), gauge(40)),
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
  memory_used: ['1.3.6.1.2.1.25.2.3.1.6']
  memory_total: ['1.3.6.1.2.1.25.2.3.1.5']
"#,
    )
    .expect("the profile loads");
    harness.with_snmp_credential("public").await;
    harness.snmp.set(
        "10.0.0.2:161",
        AgentData {
            uptime_ticks: Some(100),
            counter64: true,
            // hrStorageUsed and hrStorageSize of one storage row.
            scalars: vec![
                ("1.3.6.1.2.1.25.2.3.1.6.1".into(), gauge(512)),
                ("1.3.6.1.2.1.25.2.3.1.5.1".into(), gauge(2_048)),
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

// ------------------------------------------------- what an entry may say
//
// Every agent below is written by hand. Where a layout follows what real
// agents serve, the test says whose.

/// Poll, once, a new device on the profile `yaml` (id `id`) whose agent
/// holds `cells` beside one interface.
async fn polled(harness: &Harness, id: &str, yaml: &str, cells: &[(&str, Value)]) -> Poll {
    static DEVICES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = DEVICES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    nocturne_app::profiles::save(&harness.app, yaml).expect("the profile loads");
    harness.with_snmp_credential("public").await;
    let address = format!("10.9.{}.{}", n / 250, n % 250 + 1);
    harness.snmp.set(
        &format!("{address}:161"),
        AgentData {
            uptime_ticks: Some(100_000),
            counter64: true,
            interfaces: vec![AgentIf::up("Gi0/1", 1_000)],
            scalars: cells
                .iter()
                .map(|(oid, value)| (oid.to_string(), value.clone()))
                .collect(),
            ..AgentData::default()
        },
    );
    let device = harness
        .device(
            &format!("DEV{n}"),
            &address,
            json!({"profile": id, "credential_id": harness.credential_id, "monitor": {"snmp": true}}),
        )
        .await;
    harness.poll(&device).await
}

/// The sensors a poll stored, by name.
fn sensors(poll: &Poll) -> Vec<(String, f64)> {
    let mut found: Vec<(String, f64)> = poll
        .measures
        .iter()
        .filter(|measure| measure.metric == "temp.c")
        .map(|measure| (measure.instance.clone(), measure.value))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

fn problems(poll: &Poll) -> String {
    poll.sources.snmp.error.clone().unwrap_or_default()
}

fn int(value: i64) -> Value {
    Value::Integer(value)
}

#[tokio::test(start_paused = true)]
async fn an_instance_that_does_not_end_in_zero_is_fetched_as_it_is_written() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let yaml = r#"
id: test_instance
name: Exact instances
snmp:
  memory_used: [{instance: 1.3.6.1.2.1.25.2.3.1.6.65536}]
  memory_total: [{instance: 1.3.6.1.2.1.25.2.3.1.5.65536}]
  temperature: [{instance: 1.3.6.1.4.1.14988.1.1.3.100.1.3.17, label: cpu}]
"#;
    // hrStorageSize and hrStorageUsed of RAM (65536) beside a disk that
    // sorts after it; a disk row before it would be the first row instead.
    let poll = polled(
        &harness,
        "test_instance",
        yaml,
        &[
            ("1.3.6.1.2.1.25.2.3.1.5.1", int(10_000)),
            ("1.3.6.1.2.1.25.2.3.1.6.1", int(9_000)),
            ("1.3.6.1.2.1.25.2.3.1.5.65536", int(100_000)),
            ("1.3.6.1.2.1.25.2.3.1.6.65536", int(25_000)),
        ],
    )
    .await;
    assert_eq!(
        poll.measure("mem.pct", ""),
        Some(25.0),
        "row 65536, not row 1"
    );
    assert!(
        sensors(&poll).is_empty(),
        "an instance the agent lacks is no reading"
    );
    assert!(poll.ok && poll.interfaces_read, "{poll:?}");
    assert_eq!(poll.sources.snmp.ok, Some(true));
    assert_eq!(
        poll.sources.snmp.error, None,
        "an object that is not there is not a problem"
    );

    let without = polled(&harness, "test_instance", yaml, &[]).await;
    assert_eq!(without.measure("mem.pct", ""), None);
    assert!(without.ok && without.interfaces_read);
    assert_eq!(without.sources.snmp.error, None);
}

#[tokio::test(start_paused = true)]
async fn a_row_counts_only_when_every_test_on_it_holds() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // entPhySensorTable with a type, a status and, in the entity table that
    // shares its index, a name.
    let yaml = r#"
id: test_where
name: Row tests
snmp:
  temperature:
    - oid: 1.3.6.1.2.1.99.1.1.1.4
      where:
        - {oid: 1.3.6.1.2.1.99.1.1.1.1, is: 8}
        - {oid: 1.3.6.1.2.1.99.1.1.1.5, in: [1, 3]}
        - {oid: 1.3.6.1.2.1.47.1.1.1.1.7, matches: '^(Inlet|CPU)'}
    - oid: 1.3.6.1.2.1.99.1.1.1.4
      where: [{oid: 1.3.6.1.2.1.99.1.1.1.1, is: 9}]
      label: humidity
"#;
    let row = |index: u32, kind: Option<i64>, status: Option<i64>, name: &str, value: i64| {
        let mut cells = vec![
            (format!("1.3.6.1.2.1.99.1.1.1.4.{index}"), int(value)),
            (
                format!("1.3.6.1.2.1.47.1.1.1.1.7.{index}"),
                Value::from(name),
            ),
        ];
        cells.extend(kind.map(|kind| (format!("1.3.6.1.2.1.99.1.1.1.1.{index}"), int(kind))));
        cells.extend(status.map(|status| (format!("1.3.6.1.2.1.99.1.1.1.5.{index}"), int(status))));
        cells
    };
    let cells: Vec<(String, Value)> = [
        row(1, Some(8), Some(1), "Inlet 1", 30),
        // unavailable(2): its value means nothing.
        row(2, Some(8), Some(2), "Inlet 2", 0),
        // rpm(10), a fan.
        row(3, Some(10), Some(1), "CPU fan", 4_000),
        row(4, Some(8), Some(1), "Exhaust", 45),
        row(5, Some(8), Some(3), "  CPU  ", 52),
        // No status cell at all: the test cannot hold.
        row(6, Some(8), None, "CPU 2", 60),
        // No type cell.
        row(7, None, Some(1), "Inlet 3", 33),
    ]
    .concat();
    let cells: Vec<(&str, Value)> = cells
        .iter()
        .map(|(oid, value)| (oid.as_str(), value.clone()))
        .collect();
    let poll = polled(&harness, "test_where", yaml, &cells).await;
    assert_eq!(
        sensors(&poll),
        [("1".to_string(), 30.0), ("5".to_string(), 52.0)],
        "every test holds on rows 1 and 5 only; no row is a humidity sensor"
    );
    assert!(poll.ok && poll.interfaces_read);
    assert_eq!(
        poll.sources.snmp.error, None,
        "a test that no row passes is no problem"
    );
}

#[tokio::test(start_paused = true)]
async fn tenths_thousandths_and_an_idle_figure_are_read_as_their_entries_say() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let yaml = r#"
id: test_arithmetic
name: Arithmetic
snmp:
  cpu: [{oid: 1.3.6.1.4.1.2021.11.11.0, complement: 100}]
  temperature:
    - {oid: 1.3.6.1.4.1.14988.1.1.3.10.0, scale: 0.1, label: board}
    - {oid: 1.3.6.1.4.1.2021.13.16.2.1.3, scale: 0.001, label: lm}
"#;
    let poll = polled(
        &harness,
        "test_arithmetic",
        yaml,
        &[
            // ssCpuIdle.0: 85 % idle.
            ("1.3.6.1.4.1.2021.11.11.0", int(85)),
            // mtxrHlTemperature.0, tenths: 33.0 °C.
            ("1.3.6.1.4.1.14988.1.1.3.10.0", int(330)),
            // lmTempSensorsValue, thousandths.
            ("1.3.6.1.4.1.2021.13.16.2.1.3.1", Value::Gauge32(45_000)),
            ("1.3.6.1.4.1.2021.13.16.2.1.3.2", Value::Gauge32(38_500)),
        ],
    )
    .await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(15.0));
    assert_eq!(
        sensors(&poll),
        [
            ("board".to_string(), 33.0),
            ("lm.1".to_string(), 45.0),
            ("lm.2".to_string(), 38.5),
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn a_sensor_is_read_in_its_own_scale_and_precision_or_not_at_all() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let yaml = r#"
id: test_entity_sensor
name: Entity sensors
snmp:
  temperature:
    - oid: 1.3.6.1.2.1.99.1.1.1.4
      where: [{oid: 1.3.6.1.2.1.99.1.1.1.1, is: 8}]
      si_scale: 1.3.6.1.2.1.99.1.1.1.2
      precision: 1.3.6.1.2.1.99.1.1.1.3
      absent: [-1000000000, 1000000000]
"#;
    // (index, type, scale, precision, value), any of them missing; the
    // layout of an IOS-XE IR1101's entity sensors (third-party recording
    // `iosxe_ir1101.snmprec` of the LibreNMS test corpus, not copied here).
    let sensor = |index: u32,
                  kind: i64,
                  scale: Option<i64>,
                  places: Option<i64>,
                  value: Option<i64>| {
        let mut cells = vec![(format!("1.3.6.1.2.1.99.1.1.1.1.{index}"), int(kind))];
        cells.extend(scale.map(|scale| (format!("1.3.6.1.2.1.99.1.1.1.2.{index}"), int(scale))));
        cells.extend(places.map(|places| (format!("1.3.6.1.2.1.99.1.1.1.3.{index}"), int(places))));
        cells.extend(value.map(|value| (format!("1.3.6.1.2.1.99.1.1.1.4.{index}"), int(value))));
        cells
    };
    let cells = [
        sensor(4001, 8, Some(9), Some(0), Some(41)),
        sensor(4128, 8, Some(9), Some(3), Some(34_574)),
        // voltsDC(4): not a temperature, whatever it reads.
        sensor(4129, 4, Some(9), Some(0), Some(12)),
        // RFC 3433's underflow marker.
        sensor(4130, 8, Some(9), Some(0), Some(-1_000_000_000)),
        // No scale: its unit cannot be known.
        sensor(4131, 8, None, Some(0), Some(30)),
        // milli(8).
        sensor(4132, 8, Some(8), Some(0), Some(41_500)),
        // No precision.
        sensor(4133, 8, Some(9), None, Some(40)),
        // A scale and a precision, and no value.
        sensor(4134, 8, Some(9), Some(0), None),
        // A scale that is not one of RFC 3433's.
        sensor(4135, 8, Some(99), Some(0), Some(40)),
    ]
    .concat();
    let cells: Vec<(&str, Value)> = cells
        .iter()
        .map(|(oid, value)| (oid.as_str(), value.clone()))
        .collect();
    let poll = polled(&harness, "test_entity_sensor", yaml, &cells).await;
    assert_eq!(
        sensors(&poll),
        [
            ("4001".to_string(), 41.0),
            ("4128".to_string(), 34.574),
            ("4132".to_string(), 41.5),
        ]
    );
    assert_eq!(poll.health.temp_c, Some(41.5));
    assert_eq!(poll.sources.snmp.error, None);
}

#[tokio::test(start_paused = true)]
async fn a_ready_made_memory_percentage_counts_only_where_the_quantities_give_none() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let yaml = r#"
id: test_memory_percent
name: Memory percentage
snmp:
  memory_used: [{instance: 1.3.6.1.2.1.25.2.3.1.6.65536}]
  memory_total: [{instance: 1.3.6.1.2.1.25.2.3.1.5.65536}]
  memory_percent: [1.3.6.1.4.1.9.9.305.1.1.2.0]
"#;
    let percent = ("1.3.6.1.4.1.9.9.305.1.1.2.0", Value::Gauge32(44));
    let only_percent = polled(
        &harness,
        "test_memory_percent",
        yaml,
        std::slice::from_ref(&percent),
    )
    .await;
    assert_eq!(only_percent.measure("mem.pct", ""), Some(44.0));

    let both = polled(
        &harness,
        "test_memory_percent",
        yaml,
        &[
            percent,
            ("1.3.6.1.2.1.25.2.3.1.5.65536", int(1_000)),
            ("1.3.6.1.2.1.25.2.3.1.6.65536", int(300)),
        ],
    )
    .await;
    assert_eq!(both.measure("mem.pct", ""), Some(30.0), "bytes win");
}

#[tokio::test(start_paused = true)]
async fn combine_says_whether_processors_are_averaged_or_the_busiest_counts() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let rows = [
        ("1.3.6.1.4.1.9.9.109.1.1.1.1.7.1", Value::Gauge32(10)),
        ("1.3.6.1.4.1.9.9.109.1.1.1.1.7.2", Value::Gauge32(30)),
        ("1.3.6.1.4.1.9.9.109.1.1.1.1.7.3", Value::Gauge32(80)),
    ];
    let mean = polled(
        &harness,
        "test_mean",
        "id: test_mean\nname: Mean\nsnmp:\n  cpu: [1.3.6.1.4.1.9.9.109.1.1.1.1.7]\n",
        &rows,
    )
    .await;
    assert_eq!(
        mean.measure("cpu.pct", ""),
        Some(40.0),
        "cores of one machine"
    );
    let max = polled(
        &harness,
        "test_max",
        "id: test_max\nname: Max\nsnmp:\n  cpu: [{oid: 1.3.6.1.4.1.9.9.109.1.1.1.1.7, combine: max}]\n",
        &rows,
    )
    .await;
    assert_eq!(
        max.measure("cpu.pct", ""),
        Some(80.0),
        "separate processors"
    );
}

#[tokio::test(start_paused = true)]
async fn cpu_is_the_first_entry_that_answers_not_a_mean_over_the_entries() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let yaml = r#"
id: test_first
name: First that answers
snmp:
  cpu:
    - 1.3.6.1.4.1.9.9.109.1.1.1.1.7
    - 1.3.6.1.4.1.9.2.1.57.0
"#;
    let scalar = ("1.3.6.1.4.1.9.2.1.57.0", int(10));
    let both = polled(
        &harness,
        "test_first",
        yaml,
        &[
            ("1.3.6.1.4.1.9.9.109.1.1.1.1.7.1", Value::Gauge32(50)),
            ("1.3.6.1.4.1.9.9.109.1.1.1.1.7.2", Value::Gauge32(70)),
            scalar.clone(),
        ],
    )
    .await;
    assert_eq!(
        both.measure("cpu.pct", ""),
        Some(60.0),
        "not (50 + 70 + 10) / 3"
    );
    let fallback = polled(&harness, "test_first", yaml, &[scalar]).await;
    assert_eq!(fallback.measure("cpu.pct", ""), Some(10.0));
}

#[tokio::test(start_paused = true)]
async fn memory_entries_are_tried_in_the_order_they_are_listed() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // The pool's free bytes first, then the scalar the pool MIB replaced
    // (freeMem, obsolete since IOS 11.1). Before an entry could say more,
    // every scalar of a list was read ahead of any column, whatever the
    // order they were listed in.
    let yaml = r#"
id: test_memory_order
name: Memory entries in order
snmp:
  memory_used: [1.3.6.1.4.1.9.9.48.1.1.1.5]
  memory_free: [1.3.6.1.4.1.9.9.48.1.1.1.6, 1.3.6.1.4.1.9.2.1.8.0]
"#;
    let used = ("1.3.6.1.4.1.9.9.48.1.1.1.5.1", Value::Gauge32(300));
    let pool_free = ("1.3.6.1.4.1.9.9.48.1.1.1.6.1", Value::Gauge32(100));
    let free_mem = ("1.3.6.1.4.1.9.2.1.8.0", int(700));
    let both = polled(
        &harness,
        "test_memory_order",
        yaml,
        &[used.clone(), pool_free, free_mem.clone()],
    )
    .await;
    assert_eq!(
        both.measure("mem.pct", ""),
        Some(75.0),
        "300 / (300 + 100), not 300 / (300 + 700)"
    );
    let older = polled(&harness, "test_memory_order", yaml, &[used, free_mem]).await;
    assert_eq!(
        older.measure("mem.pct", ""),
        Some(30.0),
        "the scalar, where the pool has no free column"
    );
}

#[tokio::test(start_paused = true)]
async fn two_sensors_of_one_name_store_neither_and_say_why() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.temperature_high").await;
    let cells = [
        ("1.3.6.1.4.1.14988.1.1.3.10.0", int(75)),
        ("1.3.6.1.4.1.14988.1.1.3.11.0", int(40)),
    ];
    let unnamed = r#"
id: test_same_name
name: Same name
snmp:
  temperature: [1.3.6.1.4.1.14988.1.1.3.10.0, 1.3.6.1.4.1.14988.1.1.3.11.0]
"#;
    let poll = polled(&harness, "test_same_name", unnamed, &cells).await;
    assert!(sensors(&poll).is_empty(), "{:?}", sensors(&poll));
    assert_eq!(poll.health.temp_c, None);
    assert_eq!(poll.sources.snmp.ok, Some(true), "the agent answered");
    let said = problems(&poll);
    assert!(
        said.contains("snmp.temperature[0] (1.3.6.1.4.1.14988.1.1.3.10.0) and snmp.temperature[1] (1.3.6.1.4.1.14988.1.1.3.11.0)")
            && said.contains("distinct `label`s"),
        "{said}"
    );
    crate::rules::evaluate(&harness.app, &poll).await;
    assert!(
        harness.open_alerts().is_empty(),
        "75 °C under a name it shares raised nothing, and resolved nothing"
    );
    let view = harness
        .rpc("monitor.device", json!({"device_id": poll.device_id}))
        .await;
    assert_eq!(view["sources"]["snmp"]["ok"], true);
    assert_eq!(
        view["sources"]["snmp"]["error"],
        json!(said),
        "what the device page shows"
    );

    let labelled = r#"
id: test_labelled
name: Labelled
snmp:
  temperature:
    - {oid: 1.3.6.1.4.1.14988.1.1.3.10.0, label: board}
    - {oid: 1.3.6.1.4.1.14988.1.1.3.11.0, label: cpu}
"#;
    let poll = polled(&harness, "test_labelled", labelled, &cells).await;
    assert_eq!(
        sensors(&poll),
        [("board".to_string(), 75.0), ("cpu".to_string(), 40.0)]
    );
    assert_eq!(poll.sources.snmp.error, None);
}

#[tokio::test(start_paused = true)]
async fn a_value_that_cannot_be_what_the_profile_says_is_not_stored_and_says_so() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let yaml = r#"
id: test_backstop
name: Backstop
snmp:
  cpu: [1.3.6.1.4.1.9.9.109.1.1.1.1.7]
  memory_free: [{instance: 1.3.6.1.4.1.9.9.48.1.1.1.6.1}]
  memory_total: [{instance: 1.3.6.1.2.1.25.2.3.1.5.65536}]
  temperature:
    - {oid: 1.3.6.1.4.1.14988.1.1.3.10.0, label: board}
    - {oid: 1.3.6.1.4.1.14988.1.1.3.11.0, label: cpu}
"#;
    let poll = polled(
        &harness,
        "test_backstop",
        yaml,
        &[
            // Each row is judged on its own: 20 and 170 average to a
            // plausible 95.
            ("1.3.6.1.4.1.9.9.109.1.1.1.1.7.1", Value::Gauge32(20)),
            ("1.3.6.1.4.1.9.9.109.1.1.1.1.7.2", Value::Gauge32(170)),
            // A free amount at Gauge32's ceiling.
            ("1.3.6.1.4.1.9.9.48.1.1.1.6.1", Value::Gauge32(u32::MAX)),
            ("1.3.6.1.2.1.25.2.3.1.5.65536", int(8_000)),
            ("1.3.6.1.4.1.14988.1.1.3.10.0", Value::Gauge32(u32::MAX)),
            ("1.3.6.1.4.1.14988.1.1.3.11.0", int(41)),
        ],
    )
    .await;
    assert_eq!(poll.measure("cpu.pct", ""), None);
    assert_eq!(poll.measure("mem.pct", ""), None);
    assert_eq!(sensors(&poll), [("cpu".to_string(), 41.0)]);
    assert_eq!(poll.health.temp_c, Some(41.0));
    assert_eq!(poll.sources.snmp.ok, Some(true));
    let said = problems(&poll);
    for expected in [
        "snmp.cpu[0] (1.3.6.1.4.1.9.9.109.1.1.1.1.7): row 2 is 170, which is not a percentage",
        "snmp.memory_free[0] (1.3.6.1.4.1.9.9.48.1.1.1.6.1): the value is 4294967295, the most a Gauge32 holds",
        "snmp.temperature[0] (1.3.6.1.4.1.14988.1.1.3.10.0): sensor `board` reads 4294967295 °C",
    ] {
        assert!(said.contains(expected), "missing `{expected}` in: {said}");
    }

    // A negative amount, and a free amount larger than the total.
    let yaml = r#"
id: test_backstop_memory
name: Backstop memory
snmp:
  memory_used: [{instance: 1.3.6.1.2.1.25.2.3.1.6.1}]
  memory_free: [{instance: 1.3.6.1.4.1.2021.4.27.0}]
  memory_total: [{instance: 1.3.6.1.2.1.25.2.3.1.5.1}]
"#;
    let negative = polled(
        &harness,
        "test_backstop_memory",
        yaml,
        &[
            ("1.3.6.1.2.1.25.2.3.1.6.1", int(-5)),
            ("1.3.6.1.2.1.25.2.3.1.5.1", int(1_000)),
        ],
    )
    .await;
    assert_eq!(negative.measure("mem.pct", ""), None);
    assert!(
        problems(&negative).contains("cannot be negative"),
        "{}",
        problems(&negative)
    );
    let too_free = polled(
        &harness,
        "test_backstop_memory",
        yaml,
        &[
            ("1.3.6.1.4.1.2021.4.27.0", Value::Counter64(1_500)),
            ("1.3.6.1.2.1.25.2.3.1.5.1", int(1_000)),
        ],
    )
    .await;
    assert_eq!(too_free.measure("mem.pct", ""), None);
    assert!(
        problems(&too_free).contains("give -50.0 % in use"),
        "{}",
        problems(&too_free)
    );
}

#[tokio::test(start_paused = true)]
async fn text_and_exceptions_in_a_column_are_no_reading_and_no_problem() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let yaml = r#"
id: test_text
name: Text values
snmp:
  temperature: [1.3.6.1.4.1.9.9.13.1.3.1.3]
"#;
    let poll = polled(
        &harness,
        "test_text",
        yaml,
        &[
            ("1.3.6.1.4.1.9.9.13.1.3.1.3.1", Value::from("37 C")),
            ("1.3.6.1.4.1.9.9.13.1.3.1.3.2", Value::from("warm")),
            ("1.3.6.1.4.1.9.9.13.1.3.1.3.3", Value::NoSuchInstance),
        ],
    )
    .await;
    assert_eq!(sensors(&poll), [("1".to_string(), 37.0)]);
    assert_eq!(poll.sources.snmp.error, None);
}

// -------------------------------------------------- SNMPv1, for real

/// An in-process agent over UDP, a device on it over SNMPv1, and the
/// profile `yaml`.
async fn v1_device(
    yaml: &str,
    view: std::collections::BTreeMap<nocturne_snmp::Oid, Value>,
) -> (
    nocturne_app::App,
    String,
    nocturne_snmp::Agent,
    tokio::task::JoinHandle<Result<(), nocturne_snmp::SnmpError>>,
) {
    use std::sync::Arc;
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(crate::state::MonitorState::fake(
        Arc::new(crate::clock::SystemClock),
        Arc::new(crate::testing::FakeProbe::default()),
        Arc::new(crate::snmp::RealSnmp),
    ));
    let lab = nocturne_app::testing::lab(builder, &[]).await;
    nocturne_app::profiles::save(&lab.app, yaml).expect("the profile loads");
    lab.app
        .rpc(
            "credentials.update",
            json!({"id": lab.credential_id, "snmp_version": "v1", "snmp_community": "nocturne-v1"}),
        )
        .await
        .unwrap();
    let agent = nocturne_snmp::Agent::bind("127.0.0.1:0".parse().unwrap(), "nocturne-v1")
        .await
        .expect("the agent binds");
    agent.set_view(view);
    let server = tokio::spawn(agent.clone().run());
    let id = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "V1", "address": "127.0.0.1", "profile": "test_v1",
                   "transport": "none", "credential_id": lab.credential_id,
                   "monitor": {"enabled": true, "icmp": false, "snmp": true, "interval_s": 60,
                               "snmp_port": agent.local_addr().port()}}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    (lab.app, id, agent, server)
}

fn v1_view(extra: &[(&str, Value)]) -> std::collections::BTreeMap<nocturne_snmp::Oid, Value> {
    use nocturne_snmp::oids;
    let oid = |text: &str| text.parse::<nocturne_snmp::Oid>().unwrap();
    let mut view = std::collections::BTreeMap::new();
    view.insert(oid(oids::SYS_UP_TIME), Value::TimeTicks(500_000));
    for (column, value) in [
        (oids::IF_DESCR, Value::from("eth0")),
        (oids::IF_NAME, Value::from("eth0")),
        (oids::IF_OPER_STATUS, Value::Integer(1)),
        (oids::IF_ADMIN_STATUS, Value::Integer(1)),
        (oids::IF_IN_OCTETS, Value::Counter32(1_000)),
        (oids::IF_OUT_OCTETS, Value::Counter32(2_000)),
    ] {
        view.insert(oid(column).child(1), value);
    }
    for (text, value) in extra {
        view.insert(oid(text), value.clone());
    }
    view
}

#[tokio::test]
async fn over_snmpv1_an_object_the_agent_lacks_costs_only_its_own_reading() {
    let yaml = r#"
id: test_v1
name: SNMPv1 health
snmp:
  cpu: [{oid: 1.3.6.1.4.1.2021.11.11.0, complement: 100}]
  memory_free: [1.3.6.1.4.1.2021.4.27.0]
  memory_total: [1.3.6.1.4.1.2021.4.5.0]
  temperature: [{instance: 1.3.6.1.4.1.2021.13.16.2.1.3.1, scale: 0.001, label: lm}]
"#;
    // memSysAvail.0 is a Counter64, which does not exist for an SNMPv1
    // manager: the agent answers noSuchName, the way it answers for an
    // object it never had (the sensor).
    let (app, id, _agent, server) = v1_device(
        yaml,
        v1_view(&[
            ("1.3.6.1.4.1.2021.11.11.0", Value::Integer(70)),
            ("1.3.6.1.4.1.2021.4.27.0", Value::Counter64(4_000_000)),
            ("1.3.6.1.4.1.2021.4.5.0", Value::Integer(16_000_000)),
        ]),
    )
    .await;
    let poll = crate::poll::poll_device(&app, &id).await.unwrap();
    assert!(poll.ok, "{:?}", poll.error);
    assert_eq!(poll.via.as_deref(), Some("snmp"));
    assert_eq!(
        poll.sources.snmp.ok,
        Some(true),
        "{:?}",
        poll.sources.snmp.error
    );
    assert_eq!(poll.measure("uptime_s", ""), Some(5_000.0));
    assert!(poll.interfaces_read && poll.interfaces.len() == 1);
    assert_eq!(poll.measure("cpu.pct", ""), Some(30.0));
    assert_eq!(
        poll.measure("mem.pct", ""),
        None,
        "no available amount, no figure"
    );
    assert!(sensors(&poll).is_empty());
    assert_eq!(poll.sources.snmp.error, None);
    server.abort();
}

#[tokio::test]
async fn an_snmpv1_request_with_missing_objects_answers_the_others() {
    use crate::snmp::SnmpTransport;
    let agent = nocturne_snmp::Agent::bind("127.0.0.1:0".parse().unwrap(), "nocturne-v1")
        .await
        .expect("the agent binds");
    let present = |text: &str| text.parse::<nocturne_snmp::Oid>().unwrap();
    agent.set_view(
        [
            (present("1.3.6.1.2.1.1.5.0"), Value::from("agent")),
            (present("1.3.6.1.2.1.1.6.0"), Value::from("rack 4")),
        ]
        .into(),
    );
    let server = tokio::spawn(agent.clone().run());
    let mut session = crate::snmp::RealSnmp
        .open(
            agent.local_addr(),
            nocturne_snmp::Auth::V1 {
                community: "nocturne-v1".into(),
            },
            std::time::Duration::from_secs(2),
        )
        .await
        .unwrap();
    let asked = [
        present("1.3.6.1.4.1.2021.4.27.0"),
        present("1.3.6.1.2.1.1.5.0"),
        present("1.3.6.1.4.1.9.2.1.57.0"),
        present("1.3.6.1.2.1.1.6.0"),
        present("1.3.6.1.4.1.9.2.1.58.0"),
    ];
    let answers = session.get_each(&asked).await.unwrap();
    assert_eq!(
        answers,
        [
            None,
            Some(Value::from("agent")),
            None,
            Some(Value::from("rack 4")),
            None
        ]
    );
    // A plain GET of the same list is one noSuchName for all of it: what
    // the health read used to hand on as the whole poll's failure.
    assert!(session.get(&asked).await.is_err());
    server.abort();
}

#[tokio::test(start_paused = true)]
async fn a_fallback_sensor_is_read_only_where_the_entries_above_gave_none() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let yaml = r#"
id: test_fallback
name: Fallback sensors
snmp:
  temperature:
    - 1.3.6.1.4.1.9.9.13.1.3.1.3
    - {oid: 1.3.6.1.4.1.14988.1.1.3.10.0, scale: 0.1, label: board, fallback: true}
    - {oid: 1.3.6.1.4.1.14988.1.1.3.11.0, scale: 0.1, label: cpu, fallback: true}
"#;
    let scalars = [
        ("1.3.6.1.4.1.14988.1.1.3.10.0", int(330)),
        ("1.3.6.1.4.1.14988.1.1.3.11.0", int(410)),
    ];
    let both = polled(
        &harness,
        "test_fallback",
        yaml,
        &[
            scalars[0].clone(),
            scalars[1].clone(),
            ("1.3.6.1.4.1.9.9.13.1.3.1.3.1", Value::Gauge32(33)),
        ],
    )
    .await;
    assert_eq!(
        sensors(&both),
        [("1".to_string(), 33.0)],
        "the same sensor, not counted again from the older object"
    );
    let older = polled(&harness, "test_fallback", yaml, &scalars).await;
    assert_eq!(
        sensors(&older),
        [("board".to_string(), 33.0), ("cpu".to_string(), 41.0)],
        "every fallback is read when nothing above them answered"
    );
}

/// What the reader made of a block of plain OID lists before an entry could
/// be a mapping, written down case by case from that reader, and read again
/// now. Where every list has one entry, nothing has changed. (The table also
/// passes against the old reader itself, put back for the purpose: see this
/// crate's NOTES, "Health over SNMP".)
#[tokio::test(start_paused = true)]
async fn a_plain_block_with_one_entry_a_list_reads_as_it_always_did() {
    struct Case {
        block: &'static str,
        cells: Vec<(&'static str, Value)>,
        cpu: Option<f64>,
        memory: Option<f64>,
        sensors: Vec<(&'static str, f64)>,
    }
    let case = |block, cells| Case {
        block,
        cells,
        cpu: None,
        memory: None,
        sensors: Vec::new(),
    };
    let cases = vec![
        // A column: the mean of its rows.
        Case {
            cpu: Some(30.0),
            ..case(
                "cpu: ['1.3.6.1.2.1.25.3.3.1.2']",
                vec![
                    ("1.3.6.1.2.1.25.3.3.1.2.1", int(10)),
                    ("1.3.6.1.2.1.25.3.3.1.2.2", int(20)),
                    ("1.3.6.1.2.1.25.3.3.1.2.3", int(60)),
                ],
            )
        },
        // A scalar, fetched with GET.
        Case {
            cpu: Some(42.0),
            ..case(
                "cpu: ['1.3.6.1.4.1.11.2.14.11.5.1.9.6.1.0']",
                vec![("1.3.6.1.4.1.11.2.14.11.5.1.9.6.1.0", int(42))],
            )
        },
        // A scalar written without its `.0`: walked, and found under itself.
        Case {
            cpu: Some(7.0),
            ..case(
                "cpu: ['1.3.6.1.4.1.9.2.1.57']",
                vec![("1.3.6.1.4.1.9.2.1.57.0", int(7))],
            )
        },
        // Two columns of one table: their first rows.
        Case {
            memory: Some(25.0),
            ..case(
                "memory_used: ['1.3.6.1.2.1.25.2.3.1.6']\n  memory_total: ['1.3.6.1.2.1.25.2.3.1.5']",
                vec![
                    ("1.3.6.1.2.1.25.2.3.1.6.1", int(25)),
                    ("1.3.6.1.2.1.25.2.3.1.5.1", int(100)),
                    ("1.3.6.1.2.1.25.2.3.1.6.2", int(5)),
                    ("1.3.6.1.2.1.25.2.3.1.5.2", int(10)),
                ],
            )
        },
        // Used and free columns.
        Case {
            memory: Some(75.0),
            ..case(
                "memory_used: ['1.3.6.1.4.1.9.9.48.1.1.1.5']\n  memory_free: ['1.3.6.1.4.1.9.9.48.1.1.1.6']",
                vec![
                    ("1.3.6.1.4.1.9.9.48.1.1.1.5.1", Value::Gauge32(300)),
                    ("1.3.6.1.4.1.9.9.48.1.1.1.6.1", Value::Gauge32(100)),
                ],
            )
        },
        // An exact instance written bare, not ending in `.0`, is walked, and
        // nothing is under an instance: no figure, as before. (`instance:`
        // is how it is read.)
        case(
            "memory_used: ['1.3.6.1.2.1.25.2.3.1.6.65536']\n  memory_total: ['1.3.6.1.2.1.25.2.3.1.5.65536']",
            vec![
                ("1.3.6.1.2.1.25.2.3.1.6.65536", int(25)),
                ("1.3.6.1.2.1.25.2.3.1.5.65536", int(100)),
            ],
        ),
        // Total and free.
        Case {
            memory: Some(75.0),
            ..case(
                "memory_total: ['1.3.6.1.4.1.2021.4.5.0']\n  memory_free: ['1.3.6.1.4.1.2021.4.27.0']",
                vec![
                    ("1.3.6.1.4.1.2021.4.5.0", int(1_000)),
                    ("1.3.6.1.4.1.2021.4.27.0", Value::Counter64(250)),
                ],
            )
        },
        // A temperature column: every row a sensor, named by its index.
        Case {
            sensors: vec![("7", 40.0), ("8", 45.0)],
            ..case(
                "temperature: ['1.3.6.1.4.1.9.9.13.1.3.1.3']",
                vec![
                    ("1.3.6.1.4.1.9.9.13.1.3.1.3.7", Value::Gauge32(40)),
                    ("1.3.6.1.4.1.9.9.13.1.3.1.3.8", Value::Gauge32(45)),
                ],
            )
        },
        // A scalar: one sensor with no name, its value taken as written
        // (that this object is in tenths is for a profile to say, with
        // `scale`).
        Case {
            sensors: vec![("", 36.0)],
            ..case(
                "temperature: ['1.3.6.1.4.1.14988.1.1.3.10.0']",
                vec![("1.3.6.1.4.1.14988.1.1.3.10.0", int(36))],
            )
        },
        // A gauge reported as text with its unit.
        Case {
            sensors: vec![("1", 37.0)],
            ..case(
                "temperature: ['1.3.6.1.4.1.9.9.13.1.3.1.3']",
                vec![("1.3.6.1.4.1.9.9.13.1.3.1.3.1", Value::from("37 C"))],
            )
        },
        // An OID that does not parse is passed over, and the rest read.
        Case {
            sensors: vec![("1", 30.0)],
            ..case(
                "cpu: ['not an oid']\n  temperature: ['1.3.6.1.4.1.9.9.13.1.3.1.3']",
                vec![("1.3.6.1.4.1.9.9.13.1.3.1.3.1", Value::Gauge32(30))],
            )
        },
        // Nothing there: nothing invented.
        case(
            "cpu: ['1.3.6.1.2.1.25.3.3.1.2']\n  temperature: ['1.3.6.1.4.1.9.9.13.1.3.1.3']",
            vec![],
        ),
    ];
    let harness = Harness::new().await;
    harness.silence_rules().await;
    for (n, case) in cases.into_iter().enumerate() {
        let id = format!("test_plain_{n}");
        let yaml = format!("id: {id}\nname: Plain {n}\nsnmp:\n  {}\n", case.block);
        let poll = polled(&harness, &id, &yaml, &case.cells).await;
        assert_eq!(
            poll.measure("cpu.pct", ""),
            case.cpu,
            "case {n}: {}",
            case.block
        );
        assert_eq!(
            poll.measure("mem.pct", ""),
            case.memory,
            "case {n}: {}",
            case.block
        );
        let expected: Vec<(String, f64)> = case
            .sensors
            .iter()
            .map(|(name, value)| (name.to_string(), *value))
            .collect();
        assert_eq!(sensors(&poll), expected, "case {n}: {}", case.block);
        assert!(poll.ok && poll.interfaces_read, "case {n}");
        assert_eq!(poll.sources.snmp.error, None, "case {n}: {}", case.block);
    }
}
