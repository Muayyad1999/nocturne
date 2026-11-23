//! The `snmp:` block of every built-in profile, read as the poller reads it.
//!
//! Each test loads a built-in profile by its id and answers for the device
//! from a fake agent. The rows are written by hand from the MIB definitions
//! and the values are this file's own. Their layout (which rows an agent
//! has, their indexes, units and states) follows recordings of real devices
//! published in the LibreNMS test corpus,
//! <https://github.com/librenms/librenms/tree/master/tests/snmpsim>, read on
//! 2026-11-23; the file each layout follows is named at the test. Nothing
//! here is a recording, and nothing here says a block works on a device: it
//! says the block asks for the objects its comments name, and that the
//! arithmetic over rows laid out like a real agent's gives the figure a
//! person would call true.

use nocturne_drivers::profile::Registry;
use nocturne_snmp::{Oid, Value, names};
use serde_json::json;

use crate::poll::Poll;
use crate::testing::{AgentData, AgentIf, Harness};

/// Poll, once, a new device on the built-in profile `profile` whose agent
/// holds `cells` beside one interface.
async fn read(harness: &Harness, profile: &str, cells: &[(String, Value)]) -> Poll {
    static DEVICES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = DEVICES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    harness.with_snmp_credential("public").await;
    let address = format!("10.8.{}.{}", n / 250, n % 250 + 1);
    harness.snmp.set(
        &format!("{address}:161"),
        AgentData {
            uptime_ticks: Some(100_000),
            counter64: true,
            interfaces: vec![AgentIf::up("ether1", 1_000)],
            scalars: cells.to_vec(),
            ..AgentData::default()
        },
    );
    let device = harness
        .device(
            &format!("B{n}"),
            &address,
            json!({"profile": profile, "credential_id": harness.credential_id,
                   "monitor": {"snmp": true}}),
        )
        .await;
    let poll = harness.poll(&device).await;
    assert!(poll.ok && poll.interfaces_read, "{poll:?}");
    poll
}

/// `<column>.<index>` = value, for each row.
fn column(oid: &str, rows: &[(&str, Value)]) -> Vec<(String, Value)> {
    rows.iter()
        .map(|(index, value)| (format!("{oid}.{index}"), value.clone()))
        .collect()
}

fn one(oid: &str, value: Value) -> Vec<(String, Value)> {
    vec![(oid.to_string(), value)]
}

fn int(value: i64) -> Value {
    Value::Integer(value)
}

fn gauge(value: u32) -> Value {
    Value::Gauge32(value)
}

fn text(value: &str) -> Value {
    Value::from(value)
}

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

fn named(list: &[(&str, f64)]) -> Vec<(String, f64)> {
    let mut list: Vec<(String, f64)> = list
        .iter()
        .map(|(name, value)| (name.to_string(), *value))
        .collect();
    list.sort_by(|a, b| a.0.cmp(&b.0));
    list
}

fn close(found: Option<f64>, expected: f64) -> bool {
    found.is_some_and(|found| (found - expected).abs() < 1e-9)
}

// --------------------------------------------------------------- RouterOS

const HR_STORAGE_SIZE: &str = "1.3.6.1.2.1.25.2.3.1.5";
const HR_STORAGE_USED: &str = "1.3.6.1.2.1.25.2.3.1.6";
const MTXR_GAUGE_VALUE: &str = "1.3.6.1.4.1.14988.1.1.3.100.1.3";
const MTXR_GAUGE_UNIT: &str = "1.3.6.1.4.1.14988.1.1.3.100.1.4";

/// hrStorageTable as RouterOS 7 lays it out (`routeros_rb750gr3.snmprec`):
/// main memory at 65536, the system disk at 131072, a tmpfs at 262145.
fn routeros_storage() -> Vec<(String, Value)> {
    [
        column(
            HR_STORAGE_SIZE,
            &[
                ("65536", int(262_144)),
                ("131072", int(16_384)),
                ("262145", int(1_000)),
            ],
        ),
        column(
            HR_STORAGE_USED,
            &[
                ("65536", int(70_000)),
                ("131072", int(12_000)),
                ("262145", int(0)),
            ],
        ),
    ]
    .concat()
}

#[tokio::test(start_paused = true)]
async fn routeros_memory_is_the_main_memory_row() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let poll = read(&harness, "mikrotik_routeros", &routeros_storage()).await;
    assert!(
        close(poll.measure("mem.pct", ""), 70_000.0 / 262_144.0 * 100.0),
        "{:?}",
        poll.measure("mem.pct", "")
    );
}

#[tokio::test(start_paused = true)]
async fn a_routeros_without_row_65536_has_no_memory_figure_rather_than_its_disk() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // RouterOS 2.9 to 4.x, as its manual lists it: the disk is row 1 and
    // memory row 2.
    let cells = [
        column(HR_STORAGE_SIZE, &[("1", int(65_536)), ("2", int(16_384))]),
        column(HR_STORAGE_USED, &[("1", int(30_000)), ("2", int(9_000))]),
    ]
    .concat();
    let poll = read(&harness, "mikrotik_routeros", &cells).await;
    assert_eq!(poll.measure("mem.pct", ""), None);
}

/// mtxrGaugeTable: (index, value, unit).
fn gauges(rows: &[(&str, i64, i64)]) -> Vec<(String, Value)> {
    rows.iter()
        .flat_map(|(index, value, unit)| {
            [
                (format!("{MTXR_GAUGE_VALUE}.{index}"), int(*value)),
                (format!("{MTXR_GAUGE_UNIT}.{index}"), int(*unit)),
            ]
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn routeros_temperatures_are_the_celsius_gauges_in_whole_degrees() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.temperature_high").await;
    // A CCR2004 on RouterOS 7.24 (`routeros_ccr2004-7.24.snmprec`): three
    // temperatures, two fan states, two fan speeds, two board temperatures
    // and two power-supply states in one column.
    let ccr = gauges(&[
        ("17", 45, 1),
        ("50", 52, 1),
        ("51", 39, 1),
        ("54", 0, 6),
        ("7001", 4_000, 2),
        ("7002", 4_100, 2),
        ("7101", 31, 1),
        ("7102", 34, 1),
        ("7401", 0, 6),
        ("7402", 0, 6),
    ]);
    let poll = read(&harness, "mikrotik_routeros", &ccr).await;
    assert_eq!(
        sensors(&poll),
        named(&[
            ("17", 45.0),
            ("50", 52.0),
            ("51", 39.0),
            ("7101", 31.0),
            ("7102", 34.0)
        ])
    );
    assert_eq!(poll.health.temp_c, Some(52.0));
    crate::rules::evaluate(&harness.app, &poll).await;
    assert!(
        harness.open_alerts().is_empty(),
        "4,100 rpm is not a temperature above 70 °C"
    );

    // An RB750Gr3 (`routeros_rb750gr3.snmprec`): a voltage in decivolts
    // beside the one temperature.
    let poll = read(
        &harness,
        "mikrotik_routeros",
        &gauges(&[("13", 240, 3), ("14", 37, 1)]),
    )
    .await;
    assert_eq!(sensors(&poll), named(&[("14", 37.0)]));
}

#[tokio::test(start_paused = true)]
async fn a_board_with_only_the_tenths_scalars_reads_345_as_34_5_degrees() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.temperature_high").await;
    // A board older than the gauge table (`routeros.snmprec`, RouterOS
    // 6.40): mtxrHlTemperature alone, in tenths, 34.5 °C.
    let poll = read(
        &harness,
        "mikrotik_routeros",
        &one("1.3.6.1.4.1.14988.1.1.3.10.0", int(345)),
    )
    .await;
    assert_eq!(sensors(&poll), named(&[("temperature", 34.5)]));
    assert_eq!(poll.health.temp_c, Some(34.5));
    crate::rules::evaluate(&harness.app, &poll).await;
    assert!(
        harness.open_alerts().is_empty(),
        "34.5 °C, not 345 °C above the 70 °C rule"
    );
}

#[tokio::test(start_paused = true)]
async fn a_board_with_the_gauge_table_does_not_report_its_sensors_twice() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // RouterOS 7.25 serves the gauge table and, for the same two sensors,
    // the tenths scalars: a walk of an RB1100AHx2 posted to the vendor's
    // forum (https://forum.mikrotik.com/raw/273987/240) shows 25 and 27 in
    // both.
    let cells = [
        gauges(&[("14", 25, 1), ("17", 27, 1), ("13", 240, 3)]),
        one("1.3.6.1.4.1.14988.1.1.3.10.0", int(250)),
        one("1.3.6.1.4.1.14988.1.1.3.11.0", int(270)),
    ]
    .concat();
    let poll = read(&harness, "mikrotik_routeros", &cells).await;
    assert_eq!(sensors(&poll), named(&[("14", 25.0), ("17", 27.0)]));
    assert_eq!(poll.sources.snmp.error, None);
}

#[tokio::test(start_paused = true)]
async fn an_outdoor_board_below_zero_reads_minus_five_whatever_type_its_release_sends() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // -5 °C, sent by RouterOS 6.47 to 7.11 as a Gauge32 (its 32-bit two's
    // complement, 4294967291) and by 7.12 and later as an Integer32, beside
    // a voltage and the tenths scalar of the same sensor.
    for sent in [gauge(4_294_967_291), int(-5)] {
        let cells = vec![
            (format!("{MTXR_GAUGE_VALUE}.13"), gauge(240)),
            (format!("{MTXR_GAUGE_UNIT}.13"), int(3)),
            (format!("{MTXR_GAUGE_VALUE}.14"), sent.clone()),
            (format!("{MTXR_GAUGE_UNIT}.14"), int(1)),
            ("1.3.6.1.4.1.14988.1.1.3.10.0".to_string(), int(-50)),
        ];
        let poll = read(&harness, "mikrotik_routeros", &cells).await;
        assert_eq!(sensors(&poll), named(&[("14", -5.0)]), "sent as {sent:?}");
        assert_eq!(poll.health.temp_c, Some(-5.0));
        assert_eq!(poll.sources.snmp.error, None, "sent as {sent:?}");
    }
}

// ------------------------------------------------------------------ Linux

const HR_PROCESSOR_LOAD: &str = "1.3.6.1.2.1.25.3.3.1.2";
const SS_CPU_IDLE: &str = "1.3.6.1.4.1.2021.11.11.0";
const MEM_TOTAL_REAL: &str = "1.3.6.1.4.1.2021.4.5.0";
const MEM_AVAIL_REAL: &str = "1.3.6.1.4.1.2021.4.6.0";
const MEM_TOTAL_FREE: &str = "1.3.6.1.4.1.2021.4.11.0";
const MEM_SYS_AVAIL: &str = "1.3.6.1.4.1.2021.4.27.0";

/// hrProcessorTable as net-snmp numbers it: hrDeviceIndex 196608 + n.
fn cores(loads: &[i64]) -> Vec<(String, Value)> {
    loads
        .iter()
        .enumerate()
        .map(|(core, load)| {
            (
                format!("{HR_PROCESSOR_LOAD}.{}", 196_608 + core),
                int(*load),
            )
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn a_saturated_linux_host_reads_100_percent_not_80() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let cells = [cores(&[100, 100, 100, 100]), one(SS_CPU_IDLE, int(0))].concat();
    let poll = read(&harness, "linux", &cells).await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(100.0));
}

#[tokio::test(start_paused = true)]
async fn linux_cpu_falls_back_to_the_idle_percentage_turned_round() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let poll = read(&harness, "linux", &one(SS_CPU_IDLE, int(20))).await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(80.0));
}

/// The UCD memory scalars as net-snmp 5.9.1 and later serve them: kB, with
/// memAvailReal the kernel's MemFree and memSysAvail its MemAvailable.
fn ucd_memory(with_sys_avail: bool) -> Vec<(String, Value)> {
    let mut cells = [
        one(MEM_TOTAL_REAL, int(16_000_000)),
        one(MEM_AVAIL_REAL, int(1_000_000)),
        one(MEM_TOTAL_FREE, int(1_500_000)),
    ]
    .concat();
    if with_sys_avail {
        cells.extend(one(MEM_SYS_AVAIL, Value::Counter64(10_000_000)));
    }
    cells
}

#[tokio::test(start_paused = true)]
async fn linux_memory_used_is_total_minus_available() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let poll = read(&harness, "linux", &ucd_memory(true)).await;
    // (16,000,000 - 10,000,000) / 16,000,000; the page cache is not "used".
    assert_eq!(poll.measure("mem.pct", ""), Some(37.5));
}

#[tokio::test(start_paused = true)]
async fn a_linux_agent_without_memsysavail_has_no_memory_figure() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let poll = read(&harness, "linux", &ucd_memory(false)).await;
    assert_eq!(
        poll.measure("mem.pct", ""),
        None,
        "neither MemFree nor free RAM plus free swap stands in for MemAvailable"
    );
}

#[tokio::test(start_paused = true)]
async fn linux_temperatures_are_thousandths_of_a_degree() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.temperature_high").await;
    let cells = [
        // The third is -5 °C as net-snmp puts it in this Gauge32: (int)
        // millidegrees, masked to 32 bits.
        column(
            "1.3.6.1.4.1.2021.13.16.2.1.3",
            &[
                ("1", gauge(45_000)),
                ("2", gauge(38_500)),
                ("3", gauge(4_294_962_296)),
            ],
        ),
        // An ENTITY-SENSOR column on the same agent, as an AgentX subagent
        // serves it: every kind of sensor, none of them counted.
        column(
            "1.3.6.1.2.1.99.1.1.1.4",
            &[("1", int(45_000)), ("2", int(9_000))],
        ),
    ]
    .concat();
    let poll = read(&harness, "linux", &cells).await;
    assert_eq!(
        sensors(&poll),
        named(&[("1", 45.0), ("2", 38.5), ("3", -5.0)])
    );
    assert_eq!(poll.sources.snmp.error, None);
    crate::rules::evaluate(&harness.app, &poll).await;
    assert!(harness.open_alerts().is_empty());
}

// ------------------------------------------------------------------ Cisco

const CPM_CPU_TOTAL_1MIN_REV: &str = "1.3.6.1.4.1.9.9.109.1.1.1.1.7";
const AVG_BUSY1: &str = "1.3.6.1.4.1.9.2.1.57.0";

#[tokio::test(start_paused = true)]
async fn ios_cpu_is_the_table_when_the_old_scalar_also_answers() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // An IOS-XE 17 controller (`iosxe_c9800.snmprec`) that answers both.
    let cells = [
        column(CPM_CPU_TOTAL_1MIN_REV, &[("5", gauge(12))]),
        one(AVG_BUSY1, int(2)),
    ]
    .concat();
    let poll = read(&harness, "cisco_ios", &cells).await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(12.0), "not (12 + 2) / 2");

    let old = read(&harness, "cisco_ios", &one(AVG_BUSY1, int(3))).await;
    assert_eq!(
        old.measure("cpu.pct", ""),
        Some(3.0),
        "the scalar, where it is all there is"
    );
}

#[tokio::test(start_paused = true)]
async fn a_stack_reports_its_busiest_member() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.cpu_high").await;
    // A two-member stack (`iosxe_c3850.snmprec`): one row per member.
    let cells = column(
        CPM_CPU_TOTAL_1MIN_REV,
        &[("19", gauge(9)), ("20", gauge(5))],
    );
    let poll = read(&harness, "cisco_ios", &cells).await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(9.0), "not their mean, 7");

    // Seven idle members and one saturated: the mean, 16.9, would never
    // reach the rule.
    let mut stack: Vec<(String, f64)> = Vec::new();
    stack.extend((1..=7).map(|member| (format!("{CPM_CPU_TOTAL_1MIN_REV}.{member}"), 5.0)));
    stack.push((format!("{CPM_CPU_TOTAL_1MIN_REV}.8"), 100.0));
    let cells: Vec<(String, Value)> = stack
        .into_iter()
        .map(|(oid, load)| (oid, gauge(load as u32)))
        .collect();
    let poll = read(&harness, "cisco_ios", &cells).await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(100.0));
}

#[tokio::test(start_paused = true)]
async fn a_test_point_that_is_not_present_is_not_a_zero_degree_sensor() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // A pair of switches with one member's test points absent
    // (`iosxe_c9500x-svl.snmprec`): value and ciscoEnvMonTemperatureState.
    let value = "1.3.6.1.4.1.9.9.13.1.3.1.3";
    let state = "1.3.6.1.4.1.9.9.13.1.3.1.6";
    let cells = [
        column(
            value,
            &[
                ("1051", gauge(36)),
                ("1052", gauge(41)),
                ("2051", gauge(0)),
                ("2052", gauge(0)),
            ],
        ),
        column(
            state,
            &[
                ("1051", int(1)),
                ("1052", int(2)),
                ("2051", int(5)),
                ("2052", int(6)),
            ],
        ),
    ]
    .concat();
    let poll = read(&harness, "cisco_ios", &cells).await;
    assert_eq!(sensors(&poll), named(&[("1051", 36.0), ("1052", 41.0)]));
}

#[tokio::test(start_paused = true)]
async fn ios_memory_is_still_the_processor_pool() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // ciscoMemoryPoolTable of a 2960-X (`ios_2960x.snmprec`): Processor (1),
    // I/O (2) and a driver-text pool (20), used and free in bytes.
    let cells = [
        column(
            "1.3.6.1.4.1.9.9.48.1.1.1.5",
            &[
                ("1", gauge(60_000_000)),
                ("2", gauge(12_000_000)),
                ("20", gauge(700_000)),
            ],
        ),
        column(
            "1.3.6.1.4.1.9.9.48.1.1.1.6",
            &[
                ("1", gauge(340_000_000)),
                ("2", gauge(4_000_000)),
                ("20", gauge(300_000)),
            ],
        ),
    ]
    .concat();
    let poll = read(&harness, "cisco_ios", &cells).await;
    assert_eq!(poll.measure("mem.pct", ""), Some(15.0));
}

#[tokio::test(start_paused = true)]
async fn xr_memory_is_unknown_rather_than_a_line_cards() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // cempMemPoolHCUsed / HCFree on an ASR 9010 (`iosxr_asr9010.snmprec`):
    // a line card (entPhysicalIndex 30160112) sorts before the route
    // processor (38557239).
    let cells = [
        column(
            "1.3.6.1.4.1.9.9.221.1.1.1.1.18",
            &[
                ("30160112.1", Value::Counter64(3_600_000_000)),
                ("38557239.1", Value::Counter64(4_400_000_000)),
            ],
        ),
        column(
            "1.3.6.1.4.1.9.9.221.1.1.1.1.20",
            &[
                ("30160112.1", Value::Counter64(4_700_000_000)),
                ("38557239.1", Value::Counter64(1_600_000_000)),
            ],
        ),
    ]
    .concat();
    let poll = read(&harness, "cisco_xr", &cells).await;
    assert_eq!(poll.measure("mem.pct", ""), None);
}

#[tokio::test(start_paused = true)]
async fn xr_cpu_is_the_busiest_node() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // Route processors 0 and 1 and two line cards (`iosxr_asr9010.snmprec`).
    let cells = column(
        CPM_CPU_TOTAL_1MIN_REV,
        &[
            ("66", gauge(14)),
            ("82", gauge(3)),
            ("2050", gauge(5)),
            ("2194", gauge(4)),
        ],
    );
    let poll = read(&harness, "cisco_xr", &cells).await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(14.0), "not the mean, 6.5");
    let nxos = read(&harness, "cisco_nxos", &cells).await;
    assert_eq!(nxos.measure("cpu.pct", ""), Some(14.0));
}

// ------------------------------------------------------- ProCurve, Junos

#[tokio::test(start_paused = true)]
async fn fans_and_supply_bays_are_not_thermometers() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // entPhySensorTable of a 5406Rzl2 (`procurve.snmprec`): a power-supply
    // bay and fans typed other(1) reading 0, and two chassis thermometers.
    let mut rows: Vec<(String, Value)> = Vec::new();
    for (index, kind, value) in [
        ("5", 1, 0),
        ("11", 1, 0),
        ("12", 1, 0),
        ("13", 1, 0),
        ("14", 1, 0),
        ("15", 1, 0),
        ("16", 1, 0),
        ("25", 8, 23),
        ("11001", 1, 0),
        ("11002", 1, 0),
        ("11003", 1, 0),
        ("11004", 1, 0),
        ("11005", 1, 0),
        ("12001", 8, 23),
    ] {
        rows.extend([
            (format!("1.3.6.1.2.1.99.1.1.1.1.{index}"), int(kind)),
            (format!("1.3.6.1.2.1.99.1.1.1.2.{index}"), int(9)),
            (format!("1.3.6.1.2.1.99.1.1.1.3.{index}"), int(0)),
            (format!("1.3.6.1.2.1.99.1.1.1.4.{index}"), int(value)),
        ]);
    }
    let poll = read(&harness, "hp_procurve", &rows).await;
    assert_eq!(sensors(&poll), named(&[("12001", 23.0), ("25", 23.0)]));
}

const JNX_DESCR: &str = "1.3.6.1.4.1.2636.3.1.13.1.5";
const JNX_STATE: &str = "1.3.6.1.4.1.2636.3.1.13.1.6";
const JNX_TEMP: &str = "1.3.6.1.4.1.2636.3.1.13.1.7";
const JNX_CPU: &str = "1.3.6.1.4.1.2636.3.1.13.1.8";
const JNX_LOAD_5MIN: &str = "1.3.6.1.4.1.2636.3.1.13.1.21";

/// jnxOperatingTable: (index, descr, state, CPU, 5-minute load).
fn components(rows: &[(&str, &str, i64, u32, u32)]) -> Vec<(String, Value)> {
    rows.iter()
        .flat_map(|(index, descr, state, cpu, load)| {
            [
                (format!("{JNX_DESCR}.{index}"), text(descr)),
                (format!("{JNX_STATE}.{index}"), int(*state)),
                (format!("{JNX_CPU}.{index}"), gauge(*cpu)),
                (format!("{JNX_LOAD_5MIN}.{index}"), gauge(*load)),
            ]
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn junos_cpu_is_the_routing_engines() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // A QFX5100 (`junos_qfx5100.snmprec`): the chassis, two power supplies,
    // five fan trays, the FPC, its PIC and the Routing Engine, all running.
    // The FPC is busier than the Routing Engine, and is not the CPU asked
    // about.
    let qfx = components(&[
        ("1.1.0.0", "QFX5100-48S-6Q", 2, 0, 0),
        ("2.1.1.0", "Power Supply 0", 2, 0, 0),
        ("2.1.2.0", "Power Supply 1", 2, 0, 0),
        ("4.1.1.0", "Fan Tray 0", 2, 0, 0),
        ("4.1.2.0", "Fan Tray 1", 2, 0, 0),
        ("4.1.3.0", "Fan Tray 2", 2, 0, 0),
        ("4.1.4.0", "Fan Tray 3", 2, 0, 0),
        ("4.1.5.0", "Fan Tray 4", 2, 0, 0),
        ("7.1.0.0", "FPC: QFX5100-48S-6Q @ 0/*/*", 2, 25, 0),
        ("8.1.1.0", "PIC: 48x10G-6x40G @ 0/0/*", 2, 0, 0),
        ("9.1.0.0", "Routing Engine 0", 2, 19, 13),
    ]);
    let poll = read(&harness, "juniper_junos", &qfx).await;
    assert_eq!(
        poll.measure("cpu.pct", ""),
        Some(19.0),
        "not 13 / 11, nor the FPC's 25"
    );

    // An SRX240B (`junos_rpm.snmprec`): a Routing Engine without a number
    // beside a USB hub.
    let srx = components(&[
        ("9.1.0.0", "Routing Engine", 2, 40, 30),
        ("9.1.1.0", "USB Hub", 2, 0, 0),
    ]);
    let poll = read(&harness, "juniper_junos", &srx).await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(40.0), "not 20");

    // Two Routing Engines, after Juniper's own MX2010 example: the master is
    // running, the backup standing by.
    let mx = components(&[
        ("9.1.0.0", "Routing Engine 0", 2, 29, 20),
        ("9.2.0.0", "Routing Engine 1", 7, 3, 2),
    ]);
    let poll = read(&harness, "juniper_junos", &mx).await;
    assert_eq!(poll.measure("cpu.pct", ""), Some(29.0));
}

#[tokio::test(start_paused = true)]
async fn a_routing_engine_that_reads_zero_has_no_cpu_figure_rather_than_an_idle_one() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    // Juniper: "Zero if unavailable or inapplicable". Its own examples show
    // running Routing Engines at 3 to 74 % and the standby ones at 0.
    let blind = components(&[
        ("9.1.0.0", "Routing Engine 0", 2, 0, 0),
        ("7.1.0.0", "FPC: QFX5100-48S-6Q @ 0/*/*", 2, 25, 0),
    ]);
    let poll = read(&harness, "juniper_junos", &blind).await;
    assert_eq!(poll.measure("cpu.pct", ""), None, "unknown, not 0 %");
    assert_eq!(poll.sources.snmp.error, None);
}

#[tokio::test(start_paused = true)]
async fn a_component_without_a_sensor_has_no_temperature() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.temperature_high").await;
    // jnxOperatingTemp of the same QFX5100: zero, "unavailable or
    // inapplicable", for nine of eleven components.
    let mut rows = column(JNX_TEMP, &[("7.1.0.0", gauge(43)), ("9.1.0.0", gauge(27))]);
    for index in [
        "1.1.0.0", "2.1.1.0", "2.1.2.0", "4.1.1.0", "4.1.2.0", "4.1.3.0", "4.1.4.0", "4.1.5.0",
        "8.1.1.0",
    ] {
        rows.extend(column(JNX_TEMP, &[(index, gauge(0))]));
    }
    let poll = read(&harness, "juniper_junos", &rows).await;
    assert_eq!(
        sensors(&poll),
        named(&[("7.1.0.0", 43.0), ("9.1.0.0", 27.0)])
    );
    assert_eq!(poll.health.temp_c, Some(43.0));

    // A virtual MX (`junos_vmx.snmprec`): five components, no sensor at all.
    let vmx: Vec<(String, Value)> = ["1.1.0.0", "7.1.0.0", "8.1.1.0", "9.1.0.0", "9.1.1.0"]
        .into_iter()
        .flat_map(|index| column(JNX_TEMP, &[(index, gauge(0))]))
        .collect();
    let poll = read(&harness, "juniper_junos", &vmx).await;
    assert!(sensors(&poll).is_empty());
    assert_eq!(poll.health.temp_c, None, "unknown, not 0 °C");
}

// ------------------------------------------------- every built-in block

/// OIDs a built-in block may name although the application's name table
/// does not know them, each with the reason. Empty: every object named is
/// known. An entry the table has since learned fails the test, so the list
/// cannot go stale.
const UNNAMED: &[(&str, &str)] = &[];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Shape {
    /// A column, walked: the OID is the object itself.
    Column,
    /// One object fetched with GET: the object's `.0`.
    Scalar,
    /// An exact instance: the object and a row index.
    Instance,
}

/// The lines of a profile's own `snmp:` block, or `None` when it has none.
fn own_block(source: &str) -> Option<Vec<&str>> {
    let mut lines = source.lines().skip_while(|line| line.trim_end() != "snmp:");
    lines.next()?;
    Some(
        lines
            .take_while(|line| line.is_empty() || line.starts_with(' ') || line.starts_with('#'))
            .collect(),
    )
}

/// A line split into what YAML reads and its comment.
fn split_comment(line: &str) -> (&str, Option<&str>) {
    let mut quoted = false;
    let mut previous = ' ';
    for (at, c) in line.char_indices() {
        match c {
            '\'' => quoted = !quoted,
            '#' if !quoted && previous.is_whitespace() => {
                return (&line[..at], Some(line[at + 1..].trim()));
            }
            _ => {}
        }
        previous = c;
    }
    (line, None)
}

/// Every dotted number on the YAML side of a block's lines, with the first
/// word of that line's comment.
fn comments_by_oid(block: &[&str]) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in block {
        let (code, comment) = split_comment(line);
        let first = comment
            .and_then(|comment| comment.split_whitespace().next())
            .unwrap_or("")
            .trim_end_matches([':', ',', ';', ')'])
            .to_string();
        for token in code.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
            let token = token.trim_matches('.');
            if token.contains('.') {
                found.push((token.to_string(), first.clone()));
            }
        }
    }
    found
}

/// What is wrong with one OID of a block, if anything.
fn judge(oid: &str, shape: Shape, comments: &[(String, String)]) -> Option<String> {
    let oid = oid.trim().trim_start_matches('.');
    if UNNAMED.iter().any(|(unnamed, _)| *unnamed == oid) {
        return None;
    }
    let parsed: Oid = match oid.parse() {
        Ok(parsed) => parsed,
        Err(_) => return Some(format!("`{oid}` is not an OID")),
    };
    let Some(described) = names::describe(&parsed) else {
        return Some(format!(
            "`{oid}` is under no object the application's name table knows"
        ));
    };
    let (name, suffix) = described
        .split_once('.')
        .map_or((described.as_str(), ""), |(name, suffix)| (name, suffix));
    let bare = name.rsplit("::").next().unwrap_or(name);
    if bare.ends_with("Table") || bare.ends_with("Entry") {
        return Some(format!(
            "`{oid}` is {described}, a table or an entry, so every column of every row would count"
        ));
    }
    let shaped = match shape {
        Shape::Column => suffix.is_empty(),
        Shape::Scalar => suffix == "0",
        Shape::Instance => !suffix.is_empty(),
    };
    if !shaped {
        return Some(format!(
            "`{oid}` is {described}, which is not what a {shape:?} entry asks for"
        ));
    }
    let Some((_, first)) = comments.iter().find(|(token, _)| token == oid) else {
        return Some(format!("`{oid}` is not written on a line of the block"));
    };
    let without_module = described.rsplit("::").next().unwrap_or(&described);
    if first != without_module && first != bare {
        return Some(format!(
            "`{oid}` is {described}, and the comment on its line begins `{first}`"
        ));
    }
    None
}

#[test]
fn every_oid_in_a_built_in_snmp_block_is_the_object_its_comment_names() {
    let (registry, errors) = Registry::builtin();
    assert!(errors.is_empty(), "{errors:#?}");
    let mut failures = Vec::new();
    let mut blocks = 0;
    let mut checked = 0;
    for platform in registry.platforms() {
        let Some(block) = own_block(&platform.source) else {
            continue;
        };
        blocks += 1;
        let comments = comments_by_oid(&block);
        for (list, entries) in platform.profile.snmp.lists() {
            for (index, entry) in entries.iter().enumerate() {
                let mut asked: Vec<(&str, Shape)> = Vec::new();
                if let Some(oid) = &entry.oid {
                    asked.push((
                        oid,
                        if entry.is_column() {
                            Shape::Column
                        } else {
                            Shape::Scalar
                        },
                    ));
                }
                if let Some(oid) = &entry.instance {
                    asked.push((oid, Shape::Instance));
                }
                asked.extend(
                    entry
                        .tests
                        .iter()
                        .map(|test| (test.oid.as_str(), Shape::Column)),
                );
                asked.extend(entry.si_scale.as_deref().map(|oid| (oid, Shape::Column)));
                asked.extend(entry.precision.as_deref().map(|oid| (oid, Shape::Column)));
                for (oid, shape) in asked {
                    checked += 1;
                    if let Some(problem) = judge(oid, shape, &comments) {
                        failures.push(format!(
                            "{}: snmp.{list}[{index}]: {problem}",
                            platform.id()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        blocks >= 7 && checked >= 25,
        "only {blocks} blocks and {checked} OIDs were looked at"
    );
    assert!(failures.is_empty(), "\n{}\n", failures.join("\n"));
}

#[test]
fn an_oid_excused_from_the_name_table_is_one_the_table_still_does_not_know() {
    for (oid, reason) in UNNAMED {
        assert!(
            !reason.trim().is_empty(),
            "`{oid}` is excused with no reason"
        );
        let parsed: Oid = oid.parse().expect("an OID");
        assert_eq!(
            names::describe(&parsed),
            None,
            "the name table now knows `{oid}`: take it off the list and give it a comment"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn an_alert_on_the_old_tenfold_series_clears_at_the_first_poll_of_the_new_block() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.temperature_high").await;
    // The block RouterOS had before: mtxrHlTemperature walked as a column,
    // so its one instance is the series `0`, in tenths, read as degrees.
    // 290 (29.0 °C) is above the rule's 70 and below anything the reader now
    // refuses, which is how such an alert could be open at an upgrade.
    nocturne_app::profiles::save(
        &harness.app,
        "id: routeros_before\nname: RouterOS, the old block\nsnmp:\n  temperature: ['1.3.6.1.4.1.14988.1.1.3.10']\n",
    )
    .expect("the profile loads");
    harness.with_snmp_credential("public").await;
    harness.snmp.set(
        "10.7.0.1:161",
        AgentData {
            uptime_ticks: Some(100_000),
            counter64: true,
            interfaces: vec![AgentIf::up("ether1", 1_000)],
            scalars: one("1.3.6.1.4.1.14988.1.1.3.10.0", int(290)),
            ..AgentData::default()
        },
    );
    let id = harness
        .device(
            "OLDBOARD",
            "10.7.0.1",
            json!({"profile": "routeros_before", "credential_id": harness.credential_id,
                   "monitor": {"snmp": true}}),
        )
        .await;
    harness.poll_and_evaluate(&id).await;
    let open = harness.open_alerts();
    assert_eq!(open.len(), 1, "{open:#?}");
    assert_eq!(open[0].instance.as_deref(), Some("0"));

    // The upgrade: the same board, read with the built-in block.
    harness
        .rpc(
            "devices.update",
            json!({"id": id, "profile": "mikrotik_routeros"}),
        )
        .await;
    harness.snmp.edit("10.7.0.1:161", |agent| {
        agent.uptime_ticks = Some(agent.uptime_ticks.unwrap() + 6_000);
    });
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    let poll = harness.poll_and_evaluate(&id).await;
    assert_eq!(sensors(&poll), named(&[("temperature", 29.0)]));
    assert!(
        harness.open_alerts().is_empty(),
        "the alert on the series that stopped is resolved: {:#?}",
        harness.open_alerts()
    );
    let resolved = harness.all_alerts();
    assert_eq!(resolved.len(), 1, "{resolved:#?}");
    assert!(
        resolved[0].resolved_at.is_some() && resolved[0].detail.contains("no longer reported"),
        "{resolved:#?}"
    );
}
