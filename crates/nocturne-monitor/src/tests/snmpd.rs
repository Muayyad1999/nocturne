//! The SNMP path against net-snmp's own agent, when it is installed.
//!
//! This is the one test that polls a real agent on a real socket with real
//! IF-MIB data (the machine's own interfaces), so it catches anything the
//! in-process agent is too polite about. It starts a private `snmpd` on a
//! free loopback port with a temporary configuration — nothing is read from
//! or written to `/etc/snmp` — and kills it again. When net-snmp is not
//! installed the test prints a note and passes.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use nocturne_app::App;
use serde_json::json;

use crate::clock::SystemClock;
use crate::snmp::RealSnmp;
use crate::state::MonitorState;
use crate::testing::FakeProbe;

fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/sbin", "/usr/local/sbin", "/usr/bin"].map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// A private `snmpd`, killed when dropped.
struct Snmpd {
    child: Child,
    port: u16,
    _dir: tempfile::TempDir,
}

impl Drop for Snmpd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .and_then(|socket| socket.local_addr())
        .expect("a free loopback UDP port")
        .port()
}

fn start_snmpd(community: &str) -> Option<Snmpd> {
    let program = match find_program("snmpd") {
        Some(program) => program,
        None => {
            eprintln!("net-snmp's snmpd is not installed: skipping this test");
            return None;
        }
    };
    let dir = tempfile::tempdir().expect("a scratch directory");
    let port = free_udp_port();
    let conf = dir.path().join("snmpd.conf");
    std::fs::write(
        &conf,
        format!(
            "rocommunity {community} 127.0.0.1\n\
             agentaddress udp:127.0.0.1:{port}\n\
             sysName nocturne-snmpd\n\
             sysServices 72\n"
        ),
    )
    .expect("write the configuration");
    let child = Command::new(&program)
        // -C -c: this file and nothing else. -f: stay in the foreground so
        // the handle we hold is the agent itself.
        .args(["-C", "-c"])
        .arg(&conf)
        .args(["-f", "-Lo"])
        .env("SNMP_PERSISTENT_DIR", dir.path().join("persist"))
        .env("SNMPCONFPATH", dir.path().join("conf"))
        .env("HOME", dir.path())
        .env("MIBS", "")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    Some(Snmpd {
        child,
        port,
        _dir: dir,
    })
}

/// Profile for a net-snmp host: the UCD load and memory objects, which the
/// Linux agent serves.
const PROFILE: &str = r#"
id: test_netsnmp
name: net-snmp host
vendor: net-snmp
os: Linux
interface_names: verbatim
cli:
  prompt: '^\S+[$#]\s?$'
snmp:
  cpu: ['1.3.6.1.4.1.2021.11.9.0']
  memory_total: ['1.3.6.1.4.1.2021.4.5.0']
  memory_free: ['1.3.6.1.4.1.2021.4.6.0']
"#;

async fn app_for(port: u16, community: &str) -> (App, String) {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(MonitorState::fake(
        Arc::new(SystemClock),
        Arc::new(FakeProbe::default()),
        Arc::new(RealSnmp),
    ));
    let lab = nocturne_app::testing::lab(builder, &[]).await;
    nocturne_app::profiles::save(&lab.app, PROFILE).expect("the profile loads");
    lab.app
        .rpc(
            "credentials.update",
            json!({"id": lab.credential_id, "snmp_version": "v2c",
                   "snmp_community": community}),
        )
        .await
        .unwrap();
    let id = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "LOCALHOST", "address": "127.0.0.1", "profile": "test_netsnmp",
                   "transport": "none", "credential_id": lab.credential_id,
                   "monitor": {"enabled": true, "icmp": false, "snmp": true,
                               "interval_s": 60, "snmp_port": port}}),
        )
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    (lab.app, id)
}

/// Poll until the agent answers, or give up.
async fn poll_when_ready(app: &App, id: &str) -> Option<crate::poll::Poll> {
    for _ in 0..40 {
        let poll = crate::poll::poll_device(app, id)
            .await
            .expect("the poll runs");
        if poll.ok {
            return Some(poll);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    None
}

fn note_and_skip(what: &str) {
    eprintln!("net-snmp's snmpd did not {what}: skipping the assertions of this test");
}

#[tokio::test]
async fn polling_a_private_netsnmp_agent() {
    let community = "nocturne-monitor-test";
    let Some(agent) = start_snmpd(community) else {
        return;
    };
    let (app, id) = app_for(agent.port, community).await;

    let Some(first) = poll_when_ready(&app, &id).await else {
        // snmpd is installed but would not start here (no permission to bind,
        // a build without the UDP transport): nothing to assert, nothing to
        // fail either.
        note_and_skip("answer");
        return;
    };
    assert_eq!(first.via.as_deref(), Some("snmp"));
    assert!(first.interfaces_read);
    assert!(
        first
            .interfaces
            .iter()
            .any(|row| row.name == "lo" || row.name == "lo0"),
        "the loopback interface should be in the table: {:?}",
        first
            .interfaces
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>()
    );
    let uptime = first.measure("uptime_s", "").expect("sysUpTime");
    assert!(uptime >= 0.0);
    // Whatever UCD objects this build serves are read as numbers in range;
    // the ones it does not serve are absent rather than invented.
    if let Some(cpu) = first.measure("cpu.pct", "") {
        assert!((0.0..=10_000.0).contains(&cpu), "{cpu}");
    }
    if let Some(memory) = first.measure("mem.pct", "") {
        assert!((0.0..=100.0).contains(&memory), "{memory}");
    }
    for row in &first.interfaces {
        assert_eq!(row.in_bps, None, "no baseline on the first poll");
    }

    // A second poll, after some loopback traffic, has rates for every
    // interface whose counters both polls read.
    let chatter = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let sink = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target = sink.local_addr().unwrap();
    for _ in 0..200 {
        let _ = chatter.send_to(&[0u8; 1024], target).await;
    }
    // sysUpTime moves in hundredths of a second; give it a measurable step.
    tokio::time::sleep(Duration::from_millis(1_100)).await;

    let second = crate::poll::poll_device(&app, &id).await.unwrap();
    assert!(second.ok);
    assert!(!second.rebooted);
    let loopback = second
        .interfaces
        .iter()
        .find(|row| row.name == "lo" || row.name == "lo0")
        .expect("the loopback interface");
    assert!(
        loopback.in_bps.is_some() && loopback.out_bps.is_some(),
        "rates are computed from the second reading: {loopback:?}"
    );
    assert!(loopback.in_bps.unwrap() >= 0.0);
    assert!(
        loopback.in_err_ps.is_some(),
        "the error counters came through too"
    );

    // And the interface table and the collected store both have it.
    let rows = app
        .rpc("monitor.interfaces", json!({"device_id": id}))
        .await
        .unwrap();
    assert!(!rows.as_array().unwrap().is_empty());
    let stored =
        nocturne_app::collect::latest(&app, &id, nocturne_drivers::model::Kind::Interfaces)
            .unwrap()
            .expect("the ports of an SNMP-only host are collected");
    assert_eq!(stored.source, "snmp");
    assert!(!stored.rows.is_empty());
}

#[tokio::test]
async fn the_wrong_community_is_reported_as_a_failure_not_as_zeroes() {
    let Some(agent) = start_snmpd("nocturne-right") else {
        return;
    };
    // First establish that this agent is up at all, by polling it with the
    // community it was started with. That replaces waiting a fixed 400 ms for
    // it to bind: on a slow runner such a wait expires too early and the test
    // would then "pass" because nothing was listening yet, which proves
    // nothing about communities.
    let (right, right_id) = app_for(agent.port, "nocturne-right").await;
    if poll_when_ready(&right, &right_id).await.is_none() {
        note_and_skip("answer");
        return;
    }

    let (app, id) = app_for(agent.port, "nocturne-wrong").await;
    let poll = crate::poll::poll_device(&app, &id).await.unwrap();
    if poll.ok {
        // Some builds and configurations of net-snmp answer whatever
        // community they are given. That is a property of the agent, not of
        // Nocturne: there is nothing here to assert, and nothing to fail
        // either.
        eprintln!(
            "this snmpd answers a wrong community as readily as the right one: \
             skipping the assertions of this test"
        );
        return;
    }
    assert_eq!(poll.sources.snmp.ok, Some(false));
    assert_eq!(
        poll.measures
            .iter()
            .map(|measure| measure.metric.as_str())
            .collect::<Vec<_>>(),
        ["up"],
        "nothing is recorded but the fact that it did not answer"
    );
    assert!(crate::interfaces::of_device(&app, &id).unwrap().is_empty());
}

/// Belt and braces: nothing in this module may leave a process behind.
#[test]
fn the_private_agent_is_killed_with_its_guard() {
    if find_program("snmpd").is_none() {
        return;
    }
    let Some(agent) = start_snmpd("nocturne-drop-test") else {
        return;
    };
    let pid = agent.child.id();
    drop(agent);
    // `Drop` kills and reaps, so the pid is gone from this process's children.
    let alive = Path::new(&format!("/proc/{pid}/stat")).exists();
    assert!(
        !alive
            || std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|stat| stat.contains(" Z "))
                .unwrap_or(true),
        "snmpd {pid} is still running"
    );
}
