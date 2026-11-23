//! The SNMP path against net-snmp's own agent, when it is installed.
//!
//! This is the one test that polls a real agent on a real socket with real
//! IF-MIB data (the machine's own interfaces), so it catches anything the
//! in-process agent is too polite about. It reads the agent with the
//! built-in `linux` profile, and compares the CPU and memory its `snmp:`
//! block gives with what /proc says at the same time. It starts a private
//! `snmpd` on a free loopback port with a temporary configuration — nothing
//! is read from or written to `/etc/snmp` — and kills it again. When
//! net-snmp is not installed the test prints a note and passes.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use nocturne_app::App;
use parking_lot::Mutex;
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

/// A child of the built-in `linux` profile whose CPU is only the idle
/// figure, turned round: the second way the built-in block can read a CPU.
const IDLE_ONLY: &str = r#"
id: test_linux_idle
name: Linux, CPU from ssCpuIdle alone
extends: linux
snmp:
  cpu:
    - {oid: 1.3.6.1.4.1.2021.11.11.0, complement: 100}
"#;

/// An app with one device on the agent at `port`, read with `profile` over
/// SNMP `version`.
async fn app_with(port: u16, community: &str, version: &str, profile: &str) -> (App, String) {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder.state(MonitorState::fake(
        Arc::new(SystemClock),
        Arc::new(FakeProbe::default()),
        Arc::new(RealSnmp),
    ));
    let lab = nocturne_app::testing::lab(builder, &[]).await;
    nocturne_app::profiles::save(&lab.app, IDLE_ONLY).expect("the profile loads");
    lab.app
        .rpc(
            "credentials.update",
            json!({"id": lab.credential_id, "snmp_version": version,
                   "snmp_community": community}),
        )
        .await
        .unwrap();
    let id = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": "LOCALHOST", "address": "127.0.0.1", "profile": profile,
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

/// The built-in `linux` profile over SNMPv2c, as a host would be set up.
async fn app_for(port: u16, community: &str) -> (App, String) {
    app_with(port, community, "v2c", "linux").await
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

/// Whether the agent answers at all, asked with a profile that reads
/// nothing but the interfaces. Waiting for the agent with the profile under
/// test instead would turn a read that profile breaks into a skipped test.
async fn agent_answers(port: u16, community: &str) -> bool {
    let (app, id) = app_with(port, community, "v2c", "generic").await;
    poll_when_ready(&app, &id).await.is_some()
}

/// Whether the agent serves memSysAvail.0 at all, asked directly rather than
/// through the profile under test, so that a block that stopped reading it
/// cannot pass for an agent too old to have it.
async fn serves_memsysavail(port: u16, community: &str) -> bool {
    use crate::snmp::SnmpTransport;
    let mut session = RealSnmp
        .open(
            std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            nocturne_snmp::Auth::V2c {
                community: community.into(),
            },
            Duration::from_secs(2),
        )
        .await
        .expect("a session on the agent");
    let available: nocturne_snmp::Oid = "1.3.6.1.4.1.2021.4.27.0".parse().unwrap();
    matches!(
        session
            .get_each(std::slice::from_ref(&available))
            .await
            .as_deref(),
        Ok([Some(_)])
    )
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
    if !agent_answers(agent.port, community).await {
        // snmpd is installed but would not start here (no permission to bind,
        // a build without the UDP transport): nothing to assert, nothing to
        // fail either.
        note_and_skip("answer");
        return;
    }
    let (app, id) = app_for(agent.port, community).await;
    let first = crate::poll::poll_device(&app, &id).await.unwrap();
    assert!(first.ok, "{:?}", first.sources.snmp.error);
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
        assert!((0.0..=100.0).contains(&cpu), "{cpu}");
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
    if !agent_answers(agent.port, "nocturne-right").await {
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

// ------------------------------------------------- this machine, by /proc

/// The aggregate `cpu` line of /proc/stat: the idle column, and the sum of
/// every column, which is how net-snmp counts the total (guest time twice).
fn cpu_ticks() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let columns: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|column| column.parse().ok())
        .collect();
    Some((*columns.get(3)?, columns.iter().sum()))
}

/// (MemTotal - MemAvailable) / MemTotal, in percent: what the profile's
/// command-line read means by memory in use.
fn memory_in_use() -> Option<f64> {
    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |name: &str| {
        meminfo.lines().find_map(|line| {
            line.strip_prefix(name)?
                .trim()
                .strip_suffix("kB")?
                .trim()
                .parse::<f64>()
                .ok()
        })
    };
    let total = field("MemTotal:")?;
    Some((total - field("MemAvailable:")?) / total * 100.0)
}

struct Sample {
    at: Instant,
    idle: u64,
    total: u64,
    memory: f64,
}

/// Reads /proc once a second, on a thread of its own so that nothing the
/// test awaits can delay a sample, until dropped.
struct Sampler {
    samples: Arc<Mutex<Vec<Sample>>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Sampler {
    fn start() -> Sampler {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (samples, stop) = (samples.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    if let (Some((idle, total)), Some(memory)) = (cpu_ticks(), memory_in_use()) {
                        samples.lock().push(Sample {
                            at: Instant::now(),
                            idle,
                            total,
                            memory,
                        });
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
        };
        Sampler {
            samples,
            stop,
            thread: Some(thread),
        }
    }

    /// Memory in use, lowest and highest, over the samples taken between
    /// `from` and `to`.
    fn memory_range(&self, from: Instant, to: Instant) -> Option<(f64, f64)> {
        range(
            self.samples
                .lock()
                .iter()
                .filter(|sample| sample.at >= from && sample.at <= to)
                .map(|sample| sample.memory),
        )
    }

    /// The busy share, lowest and highest, of every stretch of 53 to 57 s
    /// that ends between `from` and `to`. net-snmp samples /proc/stat every
    /// 5 s and reports the 55 s between its latest sample and the one eleven
    /// before it, so its figure is one of these.
    fn busy_range(&self, from: Instant, to: Instant) -> Option<(f64, f64)> {
        let samples = self.samples.lock();
        let mut busy = Vec::new();
        for end in samples
            .iter()
            .filter(|sample| sample.at >= from && sample.at <= to)
        {
            for start in samples.iter() {
                let Some(length) = end.at.checked_duration_since(start.at) else {
                    continue;
                };
                let ticks = end.total.saturating_sub(start.total);
                if (53..=57).contains(&length.as_secs()) && ticks > 0 {
                    let idle = end.idle.saturating_sub(start.idle) as f64;
                    busy.push(100.0 * (1.0 - idle / ticks as f64));
                }
            }
        }
        range(busy.into_iter())
    }
}

impl Drop for Sampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn range(values: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    values.fold(None, |range, value| match range {
        None => Some((value, value)),
        Some((low, high)) => Some((f64::min(low, value), f64::max(high, value))),
    })
}

/// Poll until the device reports a CPU figure for which a stretch of the
/// samples to compare it with exists, or 120 s after the agent started.
async fn poll_for_cpu(
    app: &App,
    id: &str,
    sampler: &Sampler,
    started: Instant,
) -> (f64, (f64, f64)) {
    loop {
        let before = Instant::now();
        let poll = crate::poll::poll_device(app, id).await.unwrap();
        assert!(poll.ok, "{:?}", poll.error);
        let after = Instant::now();
        if let Some(cpu) = poll.measure("cpu.pct", "")
            && let Some(busy) = sampler.busy_range(before - Duration::from_secs(6), after)
        {
            return (cpu, busy);
        }
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "no CPU figure 120 s after snmpd started; net-snmp has one after about 60 s"
        );
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::test]
async fn the_linux_profile_reads_this_machines_own_cpu_and_memory() {
    if cpu_ticks().is_none() || memory_in_use().is_none() {
        eprintln!(
            "this machine has no /proc/stat, or no MemAvailable in /proc/meminfo: skipping this test"
        );
        return;
    }
    let community = "nocturne-linux-test";
    let Some(agent) = start_snmpd(community) else {
        return;
    };
    let started = Instant::now();
    let sampler = Sampler::start();
    if !agent_answers(agent.port, community).await {
        note_and_skip("answer");
        return;
    }
    let (app, id) = app_for(agent.port, community).await;
    let first = crate::poll::poll_device(&app, &id).await.unwrap();
    assert!(
        first.ok && first.sources.snmp.ok == Some(true),
        "the agent answers, and a read with the linux profile fails: {:?}",
        first.sources.snmp.error
    );

    // Memory: net-snmp keeps what it read of /proc/meminfo for 5 s, so the
    // figure is compared with what /proc said over the 6 s before the poll
    // and just after it. Within 3 points.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let before = Instant::now();
    let poll = crate::poll::poll_device(&app, &id).await.unwrap();
    let after = Instant::now();
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let available = serves_memsysavail(agent.port, community).await;
    match poll.measure("mem.pct", "") {
        Some(memory) => {
            assert!(
                available,
                "a memory figure of {memory:.2} % from an agent that serves no memSysAvail"
            );
            let (low, high) = sampler
                .memory_range(
                    before - Duration::from_secs(6),
                    after + Duration::from_secs(2),
                )
                .expect("samples of /proc/meminfo around the poll");
            eprintln!("memory in use: the agent {memory:.2} %, /proc {low:.2} to {high:.2} %");
            assert!(
                (low - 3.0..=high + 3.0).contains(&memory),
                "the agent says {memory:.2} % of memory is in use; /proc said {low:.2} to {high:.2} %"
            );
        }
        None => {
            assert!(
                !available,
                "the agent serves memSysAvail.0 and the linux block made no memory figure of it"
            );
            // memSysAvail exists from net-snmp 5.9.1: an older agent gives
            // no figure, which is the block doing its job, not a failure.
            eprintln!(
                "this snmpd serves no memSysAvail (net-snmp before 5.9.1?): no memory figure to compare"
            );
        }
    }

    // CPU: hrProcessorLoad's mean over the cores, against /proc/stat over
    // the same minute. Within 12 points.
    let (cpu, (low, high)) = poll_for_cpu(&app, &id, &sampler, started).await;
    eprintln!(
        "CPU busy, hrProcessorLoad: the agent {cpu:.1} %, /proc/stat {low:.1} to {high:.1} % ({:.0} s after the agent started)",
        started.elapsed().as_secs_f64()
    );
    assert!(
        (low - 12.0..=high + 12.0).contains(&cpu),
        "the agent says the CPU is {cpu:.1} % busy; /proc/stat said {low:.1} to {high:.1} % over that minute"
    );

    // The same agent, through the block's second CPU entry alone.
    let (idle_app, idle_id) = app_with(agent.port, community, "v2c", "test_linux_idle").await;
    let (cpu, (low, high)) = poll_for_cpu(&idle_app, &idle_id, &sampler, started).await;
    eprintln!(
        "CPU busy, 100 - ssCpuIdle: the agent {cpu:.1} %, /proc/stat {low:.1} to {high:.1} %"
    );
    assert!(
        (low - 12.0..=high + 12.0).contains(&cpu),
        "100 - ssCpuIdle is {cpu:.1} %; /proc/stat said {low:.1} to {high:.1} % busy over that minute"
    );
}

#[tokio::test]
async fn over_snmpv1_the_linux_profile_reads_the_interfaces_and_no_memory() {
    let community = "nocturne-linux-v1";
    let Some(agent) = start_snmpd(community) else {
        return;
    };
    if !agent_answers(agent.port, community).await {
        note_and_skip("answer");
        return;
    }
    let (app, id) = app_with(agent.port, community, "v1", "linux").await;
    let poll = crate::poll::poll_device(&app, &id).await.unwrap();
    if poll.sources.snmp.error.as_deref() == Some("request timed out") {
        // A build of net-snmp without SNMPv1 says nothing at all.
        note_and_skip("answer over SNMPv1");
        return;
    }
    assert!(poll.ok, "{:?}", poll.sources.snmp.error);
    assert_eq!(poll.via.as_deref(), Some("snmp"));
    assert_eq!(poll.sources.snmp.ok, Some(true));
    assert!(poll.interfaces_read && !poll.interfaces.is_empty());
    assert!(poll.measure("uptime_s", "").is_some());
    // memSysAvail is a 64-bit object, which SNMPv1 cannot carry: no available
    // amount, so no figure, and the rest of the read is untouched by it.
    assert_eq!(poll.measure("mem.pct", ""), None);
    assert_eq!(
        poll.sources.snmp.error, None,
        "an absent object is no problem"
    );
    if let Some(cpu) = poll.measure("cpu.pct", "") {
        assert!((0.0..=100.0).contains(&cpu), "{cpu}");
    }
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
