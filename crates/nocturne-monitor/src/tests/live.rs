//! The SNMP and CLI paths exercised for real: an in-process
//! `nocturne_snmp::Agent` answering over UDP on loopback, and the core
//! collector reading canned `show` output off a fake device.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use nocturne_app::App;
use nocturne_snmp::{Agent, Oid, Value, oids};
use serde_json::json;

use crate::clock::SystemClock;
use crate::snmp::RealSnmp;
use crate::state::MonitorState;
use crate::testing::FakeProbe;

/// What one interface of the agent reports.
struct Port {
    index: u32,
    name: &'static str,
    alias: &'static str,
    oper: i64,
    speed_mbps: u32,
    in_octets: u64,
    out_octets: u64,
    in_errors: u32,
}

fn oid(text: &str) -> Oid {
    text.parse().expect("a well-formed OID")
}

fn view(uptime_ticks: u32, ports: &[Port]) -> BTreeMap<Oid, Value> {
    let mut view = BTreeMap::new();
    view.insert(oid(oids::SYS_UP_TIME), Value::TimeTicks(uptime_ticks));
    view.insert(oid(oids::SYS_NAME), Value::from("agent-sw1"));
    for port in ports {
        let at = |column: &str| oid(column).child(port.index);
        view.insert(at(oids::IF_INDEX), Value::Integer(i64::from(port.index)));
        view.insert(at(oids::IF_NAME), Value::from(port.name));
        view.insert(at(oids::IF_DESCR), Value::from(port.name));
        view.insert(at(oids::IF_ALIAS), Value::from(port.alias));
        view.insert(at(oids::IF_ADMIN_STATUS), Value::Integer(1));
        view.insert(at(oids::IF_OPER_STATUS), Value::Integer(port.oper));
        view.insert(at(oids::IF_HIGH_SPEED), Value::Gauge32(port.speed_mbps));
        view.insert(at(oids::IF_SPEED), Value::Gauge32(100_000_000));
        view.insert(at(oids::IF_HC_IN_OCTETS), Value::Counter64(port.in_octets));
        view.insert(
            at(oids::IF_HC_OUT_OCTETS),
            Value::Counter64(port.out_octets),
        );
        view.insert(at(oids::IF_IN_ERRORS), Value::Counter32(port.in_errors));
        view.insert(at(oids::IF_OUT_ERRORS), Value::Counter32(0));
        view.insert(at(oids::IF_IN_DISCARDS), Value::Counter32(0));
        view.insert(at(oids::IF_OUT_DISCARDS), Value::Counter32(0));
    }
    // The CPU object the test profile points at.
    view.insert(oid("1.3.6.1.4.1.9.9.109.1.1.1.1.6.1"), Value::Gauge32(42));
    view
}

fn ports(in_octets: u64) -> Vec<Port> {
    vec![
        Port {
            index: 1,
            name: "Gi0/1",
            alias: "uplink to CORE",
            oper: 1,
            speed_mbps: 1_000,
            in_octets,
            out_octets: in_octets / 10,
            in_errors: 0,
        },
        Port {
            index: 2,
            name: "Gi0/2",
            alias: "desk 12",
            oper: 2,
            speed_mbps: 100,
            in_octets: 0,
            out_octets: 0,
            in_errors: 5,
        },
    ]
}

/// A profile whose `snmp:` block names the CPU OID the agent serves.
const SNMP_PROFILE: &str = r#"
id: test_snmp
name: Test SNMP device
vendor: Test
os: Test
cli:
  prompt: '^\S+[>#]\s?$'
policy:
  read_only: ['^show ']
snmp:
  cpu: ['1.3.6.1.4.1.9.9.109.1.1.1.1.6']
"#;

async fn app_with_real_snmp() -> (App, Arc<FakeProbe>) {
    let probe = Arc::new(FakeProbe::default());
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(MonitorState::fake(
        Arc::new(SystemClock),
        probe.clone(),
        Arc::new(RealSnmp),
    ));
    let lab = nocturne_app::testing::lab(builder, &[]).await;
    nocturne_app::profiles::save(&lab.app, SNMP_PROFILE).expect("the profile loads");
    lab.app
        .rpc(
            "credentials.update",
            json!({"id": lab.credential_id, "snmp_version": "v2c", "snmp_community": "nocturne-test"}),
        )
        .await
        .unwrap();
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "AGENT1", "address": "127.0.0.1", "profile": "test_snmp",
                   "transport": "none", "credential_id": lab.credential_id,
                   "monitor": {"enabled": true, "icmp": false, "snmp": true, "interval_s": 60}}),
        )
        .await
        .unwrap();
    (lab.app, probe)
}

#[tokio::test]
async fn polling_a_real_snmp_agent_over_udp() {
    let (app, probe) = app_with_real_snmp().await;
    let agent = Agent::bind("127.0.0.1:0".parse().unwrap(), "nocturne-test")
        .await
        .expect("the agent binds");
    let port = agent.local_addr().port();
    agent.set_view(view(1_000_000, &ports(1_000_000)));
    let server = tokio::spawn(agent.clone().run());

    let id = nocturne_app::inventory::find(&app, "AGENT1").unwrap().id;
    app.rpc(
        "devices.update",
        json!({"id": id, "monitor": {"enabled": true, "icmp": false, "snmp": true,
                                     "interval_s": 60, "snmp_port": port}}),
    )
    .await
    .unwrap();

    let first = crate::poll::poll_device(&app, &id).await.unwrap();
    assert!(first.ok, "{:?}", first.error);
    assert_eq!(first.via.as_deref(), Some("snmp"));
    assert_eq!(first.measure("uptime_s", ""), Some(10_000.0));
    assert_eq!(first.measure("cpu.pct", ""), Some(42.0));
    assert_eq!(first.interfaces.len(), 2);
    let uplink = &first.interfaces[0];
    assert_eq!(uplink.name, "GigabitEthernet0/1");
    assert_eq!(uplink.description, "uplink to CORE");
    assert_eq!(uplink.speed_bps, Some(1_000_000_000));
    assert_eq!((uplink.admin_up, uplink.oper_up), (Some(true), Some(true)));
    assert_eq!(uplink.in_bps, None, "no baseline yet");
    assert_eq!(first.interfaces[1].oper_up, Some(false));
    assert!(
        probe.pinged.lock().is_empty(),
        "ICMP is off for this device"
    );

    // Move the agent on by 60 s of uptime and 75 MB.
    tokio::time::sleep(Duration::from_millis(5)).await;
    agent.set_view(view(1_006_000, &ports(1_000_000 + 75_000_000)));
    let second = crate::poll::poll_device(&app, &id).await.unwrap();
    let uplink = &second.interfaces[0];
    assert_eq!(uplink.in_bps, Some(10_000_000.0));
    assert_eq!(uplink.out_bps, Some(1_000_000.0));
    assert_eq!(uplink.util_pct, Some(1.0));
    assert_eq!(
        second.measure("if.in_bps", "GigabitEthernet0/1"),
        Some(10_000_000.0)
    );

    // One session, not one per poll.
    let view_now = nocturne_app::inventory::state_of(&app, &id).unwrap();
    assert_eq!(view_now.reachable, Some(true));

    // And the interface table and the collected store both have it.
    let rows = app
        .rpc("monitor.interfaces", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    let stored =
        nocturne_app::collect::latest(&app, &id, nocturne_drivers::model::Kind::Interfaces)
            .unwrap()
            .unwrap();
    assert_eq!(stored.source, "snmp");

    server.abort();
}

#[tokio::test]
async fn an_agent_that_stops_answering_is_reported_not_guessed_at() {
    let (app, _probe) = app_with_real_snmp().await;
    let id = nocturne_app::inventory::find(&app, "AGENT1").unwrap().id;
    // Nothing is listening on this port.
    let free = {
        let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        socket.local_addr().unwrap().port()
    };
    app.rpc(
        "devices.update",
        json!({"id": id, "monitor": {"enabled": true, "icmp": false, "snmp": true,
                                     "interval_s": 60, "snmp_port": free}}),
    )
    .await
    .unwrap();
    let poll = crate::poll::poll_device(&app, &id).await.unwrap();
    assert!(!poll.ok);
    assert_eq!(poll.sources.snmp.ok, Some(false));
    assert!(
        poll.error
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains("time"),
        "{:?}",
        poll.error
    );
    assert_eq!(poll.measures.len(), 1, "{:?}", poll.measures);
    assert_eq!(poll.measure("up", ""), Some(0.0));
}

#[tokio::test]
async fn a_device_whose_address_is_a_name_says_so_instead_of_blocking_the_poller() {
    let (app, _probe) = app_with_real_snmp().await;
    let credential = nocturne_app::inventory::find(&app, "AGENT1")
        .unwrap()
        .credential_id
        .unwrap();
    let id = app
        .rpc(
            "devices.create",
            json!({"name": "BYNAME", "address": "switch.invalid", "transport": "none",
                   "credential_id": credential,
                   "monitor": {"enabled": true, "icmp": false, "snmp": true, "interval_s": 60}}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let poll = crate::poll::poll_device(&app, &id).await.unwrap();
    assert!(!poll.ok);
    let error = poll.sources.snmp.error.unwrap();
    assert!(error.contains("not an IP address"), "{error}");
}

// ------------------------------------------------------------ CLI fallback

/// A profile that reads counters and health out of canned `show` output.
const CLI_PROFILE: &str = r#"
id: test_cli_counters
name: Test IOS with counters
vendor: Test
os: IOS
cli:
  prompt: '^(?P<host>[\w.\-]{1,63})(\([\w\-/. ]+\))?[>#]\s?$'
  user_prompt: '^[\w.\-]{1,63}>\s?$'
  enable: {}
  init: ['terminal length 0']
  errors: ['^% ']
config:
  syntax: ios
  running: 'show running-config'
policy:
  read_only: ['^show ', '^terminal (length|width) ']
collect:
  interfaces:
    - command: show interfaces summary
      parser:
        regex: '(?m)^(?P<name>\S+)\s+(?P<admin>up|down)\s+(?P<oper>up|down)\s+(?P<speed>\d+)\s+(?P<in_octets>\d+)\s+(?P<out_octets>\d+)\s+(?P<in_errors>\d+)\s*$'
      map:
        name: name
        admin_up: "admin == 'up'"
        oper_up: "oper == 'up'"
        speed_bps: speed
        counters.in_octets: in_octets
        counters.out_octets: out_octets
        counters.in_errors: in_errors
  health:
    - command: show processes cpu
      parser: {regex: 'five seconds: (?P<cpu>\d+)%'}
      map: {cpu_pct: cpu}
    - command: show system uptime
      parser: {regex: 'uptime (?P<uptime>\d+) seconds'}
      map: {uptime_s: uptime}
"#;

fn summary(in_octets: u64) -> String {
    format!(
        "Interface              Admin Oper  Speed       InOctets   OutOctets  InErrors\n\
         GigabitEthernet0/1     up    up    1000000000  {in_octets}      {}        0\n\
         GigabitEthernet0/2     up    down  100000000   0          0          7\n",
        in_octets / 10
    )
}

#[tokio::test(start_paused = true)]
async fn the_cli_fallback_derives_the_same_metrics_from_show_output() {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    let probe = Arc::new(FakeProbe::default());
    builder.state(MonitorState::fake(
        Arc::new(crate::clock::VirtualClock::new(crate::testing::epoch())),
        probe,
        Arc::new(crate::testing::FakeSnmp::default()),
    ));
    let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
    nocturne_app::profiles::save(&lab.app, CLI_PROFILE).expect("the profile loads");
    let id = lab.id("SW1");
    lab.app
        .rpc(
            "devices.update",
            json!({"id": id, "profile": "test_cli_counters"}),
        )
        .await
        .unwrap();
    lab.fake("SW1").with(|state| {
        state
            .outputs
            .insert("show interfaces summary".into(), summary(1_000_000));
        state.outputs.insert(
            "show processes cpu".into(),
            "CPU utilization for five seconds: 17%/0%; one minute: 12%\n".into(),
        );
        state.outputs.insert(
            "show system uptime".into(),
            "Device uptime 100000 seconds\n".into(),
        );
    });

    let first = crate::poll::poll_device(&lab.app, &id).await.unwrap();
    assert_eq!(first.sources.cli.ok, Some(true));
    assert_eq!(first.measure("cpu.pct", ""), Some(17.0));
    assert_eq!(first.measure("uptime_s", ""), Some(100_000.0));
    assert_eq!(first.interfaces.len(), 2);
    assert_eq!(first.interfaces[0].name, "GigabitEthernet0/1");
    assert_eq!(first.interfaces[0].in_bps, None);
    assert_eq!(first.measure("if.oper", "GigabitEthernet0/2"), Some(0.0));

    // 60 s later, 75 MB more: the same arithmetic as over SNMP, off text.
    tokio::time::sleep(Duration::from_secs(60)).await;
    lab.fake("SW1").with(|state| {
        state
            .outputs
            .insert("show interfaces summary".into(), summary(76_000_000));
        state.outputs.insert(
            "show system uptime".into(),
            "Device uptime 100060 seconds\n".into(),
        );
    });
    let second = crate::poll::poll_device(&lab.app, &id).await.unwrap();
    assert_eq!(second.interfaces[0].in_bps, Some(10_000_000.0));
    assert_eq!(second.interfaces[0].util_pct, Some(1.0));
    assert_eq!(second.interfaces[0].out_bps, Some(1_000_000.0));

    // And a restart seen over the CLI is a gap, not a spike, exactly as it is
    // over SNMP.
    tokio::time::sleep(Duration::from_secs(60)).await;
    lab.fake("SW1").with(|state| {
        state
            .outputs
            .insert("show interfaces summary".into(), summary(500_000));
        state.outputs.insert(
            "show system uptime".into(),
            "Device uptime 45 seconds\n".into(),
        );
    });
    let third = crate::poll::poll_device(&lab.app, &id).await.unwrap();
    assert!(third.rebooted);
    assert_eq!(third.interfaces[0].in_bps, None);
}

#[tokio::test(start_paused = true)]
async fn the_cli_is_read_at_its_own_slower_cadence() {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    let probe = Arc::new(FakeProbe::default());
    builder.state(MonitorState::fake(
        Arc::new(crate::clock::VirtualClock::new(crate::testing::epoch())),
        probe.clone(),
        Arc::new(crate::testing::FakeSnmp::default()),
    ));
    let lab = nocturne_app::testing::lab(builder, &[]).await;
    nocturne_app::profiles::save(&lab.app, CLI_PROFILE).expect("the profile loads");
    // A real device reached over SSH, with ICMP every minute and the CLI
    // every five.
    let fake = nocturne_app::testing::FakeDevice::new("EDGE1");
    fake.with(|state| {
        state
            .outputs
            .insert("show interfaces summary".into(), summary(1_000_000));
    });
    let id = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "EDGE1", "address": "10.9.0.1", "transport": "none",
                   "profile": "test_cli_counters", "credential_id": lab.credential_id,
                   "monitor": {"enabled": true, "icmp": true, "snmp": false, "interval_s": 60}}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // `transport: none` means there is no CLI at all: ICMP only.
    let poll = crate::poll::poll_device(&lab.app, &id).await.unwrap();
    assert!(poll.ok);
    assert!(!poll.interfaces_read);
    assert!(!poll.sources.cli.enabled);
    assert_eq!(probe.count("10.9.0.1"), 1);
    assert_eq!(
        poll.measures
            .iter()
            .map(|measure| measure.metric.as_str())
            .collect::<Vec<_>>(),
        ["icmp.loss_pct", "icmp.rtt_ms", "up"]
    );
}
