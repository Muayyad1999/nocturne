//! What a flood costs the process: its resident memory, measured, and not
//! only the counters.
//!
//! A UDP datagram can be 64 KiB, and whoever can reach a receiver's port can
//! send as many as they like. Every queue between a socket and what writes,
//! and every forwarder's queue, is bounded in bytes as well as in entries.
//! Each test here floods one of them with the largest datagrams there are,
//! while what empties the queue is held up, and reads the process's resident
//! memory before, during and after (`/proc/self/status`: on a system without
//! it the counters are checked, the memory is not measured, and the test says
//! so).
//!
//! Each flood runs in a process of its own — this binary, started again for
//! that one flood — so that what one flood leaves in the allocator is neither
//! counted against the next nor hides what the next one costs. A flood is so
//! many datagrams, not so many seconds, and nothing is timed.

mod support;

use std::time::{Duration, Instant};

use nocturne_app::util::now_ms;
use nocturne_logs::state::LogsState;
use serde_json::{Value, json};
use support::*;

const MIB: u64 = 1024 * 1024;
/// What the test gives a receiver's queue (`queue_limit_bytes`). Small, so
/// that what a flood leaves queued is soon written: storing a line of 64 KiB
/// takes a debug build a quarter of a second, most of it redacting.
const BUDGET: u64 = 2 * MIB;
/// What the process may grow by besides what a queue may hold: a datagram or
/// two being read and parsed, what the allocator keeps for itself, the
/// runtime's own. A queue bounded in entries alone grows by many times this.
const SLACK: u64 = 24 * MIB;
/// The largest payload a UDP datagram carries over IPv4.
const LARGEST: usize = 65_507;
/// Names the flood the process started for one is to run.
const FLOOD: &str = "NOCTURNE_TEST_FLOOD";

#[test]
fn a_flood_of_the_largest_datagrams_costs_the_process_what_a_queue_may_hold_and_no_more() {
    for receiver in ["syslog", "traps", "flows", "forwarder"] {
        let (succeeded, output) = alone(receiver);
        // The harness's own `test flood ...` may stand in front of it.
        for line in output
            .lines()
            .filter_map(|line| line.find("memory:").map(|at| &line[at..]))
        {
            println!("{line}");
        }
        let tail: Vec<&str> = output.lines().rev().take(40).collect();
        assert!(
            succeeded,
            "the {receiver} flood failed:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
}

/// Run one flood in a process of its own, and say how it went and what it
/// printed.
fn alone(receiver: &str) -> (bool, String) {
    let scratch = tempfile::tempdir().expect("a temporary directory");
    let path = scratch.path().join("output");
    let output = std::fs::File::create(&path).expect("a file for the output");
    let mut child = std::process::Command::new(std::env::current_exe().expect("this binary"))
        .args([
            "--exact",
            "flood",
            "--include-ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(FLOOD, receiver)
        .stdout(output.try_clone().expect("the file, twice"))
        .stderr(output)
        .spawn()
        .expect("this binary starts again");
    let deadline = Instant::now() + Duration::from_secs(300);
    let status = loop {
        if let Some(status) = child.try_wait().expect("the child can be waited for") {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            break child.wait().expect("the child has ended");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let printed = std::fs::read_to_string(&path).unwrap_or_default();
    (status.success(), printed)
}

#[test]
#[ignore = "started by a_flood_of_the_largest_datagrams_costs_the_process_what_a_queue_may_hold_and_no_more, once for each receiver, each in a process of its own"]
fn flood() {
    let Ok(receiver) = std::env::var(FLOOD) else {
        return;
    };
    // On threads of their own, as in the application. A syslog or trap flood
    // holds the writer and may hold the rules' sweeper as well; a flow flood
    // holds only the counting task, and one thread more is all the collector
    // needs — and every thread a datagram of sixty thousand records is
    // decoded on keeps, in the allocator, what it was given there.
    let threads = if receiver == "flows" { 2 } else { 4 };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(threads)
        .enable_all()
        .build()
        .expect("a runtime");
    runtime.block_on(async {
        match receiver.as_str() {
            "syslog" => syslog().await,
            "traps" => traps().await,
            "flows" => flows().await,
            "forwarder" => forwarder().await,
            other => panic!("there is no flood called {other}"),
        }
    });
}

// ------------------------------------------------------------- measuring

/// The process's resident memory in bytes, where the system says what it is.
fn resident() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

fn mib(bytes: Option<u64>) -> String {
    bytes.map_or_else(
        || "?".to_string(),
        |bytes| format!("{:.1} MiB", bytes as f64 / MIB as f64),
    )
}

/// Resident memory before a flood, the most it was while the flood was held
/// in a queue, and after.
struct Memory {
    what: &'static str,
    before: Option<u64>,
    peak: Option<u64>,
    after: Option<u64>,
}

impl Memory {
    fn before(what: &'static str) -> Memory {
        let before = resident();
        Memory {
            what,
            before,
            peak: before,
            after: None,
        }
    }

    fn sample(&mut self) {
        if let Some(now) = resident() {
            self.peak = Some(self.peak.map_or(now, |peak| peak.max(now)));
        }
    }

    fn after(&mut self) {
        self.after = resident();
    }

    fn report(&self, numbers: &str) {
        let grew = self
            .peak
            .zip(self.before)
            .map(|(peak, before)| peak.saturating_sub(before));
        println!(
            "memory: {}: {} before, at most {} during the flood (+{}), {} after; {numbers}",
            self.what,
            mib(self.before),
            mib(self.peak),
            mib(grew),
            mib(self.after),
        );
    }

    /// The process grew by no more than `limit` while the flood waited.
    fn grew_by_at_most(&self, limit: u64) {
        let (Some(before), Some(peak)) = (self.before, self.peak) else {
            println!(
                "memory: {}: not measured: this system has no /proc/self/status",
                self.what
            );
            return;
        };
        let grew = peak.saturating_sub(before);
        assert!(
            grew <= limit,
            "{}: the process grew by {} while the flood waited, more than the {} its queue may hold and the {} anything else may take",
            self.what,
            mib(Some(grew)),
            mib(Some(limit - SLACK)),
            mib(Some(SLACK)),
        );
    }
}

fn number(status: &Value, key: &str) -> u64 {
    status[key].as_u64().unwrap_or_default()
}

/// Poll `logs.status` until `ready` says so, for as long as a debug build
/// may take to write what a flood left queued. A writer that is stuck fails
/// here instead of hanging the test.
async fn once_settled(
    app: &nocturne_app::App,
    what: &str,
    ready: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        let status = app
            .rpc("logs.status", Value::Null)
            .await
            .expect("the status is readable");
        if ready(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {status:#}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Send each datagram once, from one socket, and wait — without the
/// runtime's timers, since a runtime thread is held — until the receiver has
/// taken it off its socket, or for a quarter of a second, after which the
/// kernel is taken to have dropped it: a socket's buffer holds a few
/// datagrams this size. `between` runs after each.
fn send_each(
    port: u16,
    datagrams: impl Iterator<Item = Vec<u8>>,
    received: impl Fn() -> u64,
    mut between: impl FnMut(),
) {
    let sender = std::net::UdpSocket::bind("127.0.0.1:0").expect("a socket");
    // macOS sends no datagram larger than the socket's send buffer, 9 KiB
    // by default.
    socket2::SockRef::from(&sender)
        .set_send_buffer_size(256 * 1024)
        .expect("room for the largest datagram");
    for datagram in datagrams {
        let before = received();
        sender
            .send_to(&datagram, ("127.0.0.1", port))
            .expect("the datagram is sent");
        let patience = Instant::now() + Duration::from_millis(250);
        while received() == before && Instant::now() < patience {
            // See `wait_held`: a thread that sleeps is given something to do,
            // and takes the sockets over from the one that is held.
            tokio::spawn(async {});
            std::thread::sleep(Duration::from_millis(1));
        }
        between();
    }
}

// ----------------------------------------------------------------- floods

/// A syslog line as large as a datagram can be.
fn syslog_line(n: usize) -> Vec<u8> {
    let mut line = format!("<189>{n}: *Mar  1 00:00:00.000: %SYS-5-FLOOD: ").into_bytes();
    line.resize(LARGEST, b'x');
    line
}

async fn syslog() {
    const DATAGRAMS: usize = 600;
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"queue_limit_bytes": BUDGET}}),
    )
    .await
    .unwrap();
    let (udp, _) = start_receiver(app).await;
    let state = app.state::<LogsState>();
    let status = || serde_json::to_value(state.syslog.status(now_ms())).unwrap();

    // The writer stores a first line and is held where it shows it to the
    // rules: everything after that waits in the queue, or does not fit.
    let held = hold_the_rules(app);
    send_tcp_held(
        start_tcp_port(app),
        b"<189>1: the line the writer is held up on\n",
    );
    wait_held("the first line to be stored", || {
        number(&status(), "stored") == 1
    });

    let mut memory = Memory::before("syslog");
    let mut most_bytes = 0;
    let mut sent = 0;
    send_each(
        udp,
        (0..DATAGRAMS).map(syslog_line),
        || number(&status(), "received"),
        || {
            sent += 1;
            most_bytes = most_bytes.max(number(&status(), "queued_bytes"));
            if sent % 25 == 0 {
                memory.sample();
            }
        },
    );
    memory.sample();
    let flooded = status();
    held.release();
    let settled = once_settled(app, "everything received to be written or dropped", |all| {
        let syslog = &all["syslog"];
        number(syslog, "stored") + number(syslog, "dropped") == number(syslog, "received")
    })
    .await;
    memory.after();
    let settled = &settled["syslog"];
    memory.report(&format!(
        "{} received, {} stored, {} dropped, at most {} queued",
        number(settled, "received"),
        number(settled, "stored"),
        number(settled, "dropped"),
        mib(Some(most_bytes)),
    ));

    assert!(
        number(&flooded, "received") > DATAGRAMS as u64 / 2,
        "the flood reached the receiver: {flooded:#}"
    );
    assert!(
        number(settled, "dropped") > 0,
        "what did not fit was dropped and counted: {settled:#}"
    );
    assert_eq!(settled["max_queued_bytes"], json!(BUDGET), "{settled:#}");
    assert!(
        most_bytes > 0 && most_bytes <= BUDGET,
        "the status says what the queue holds, and it held no more than it may: {most_bytes}"
    );
    memory.grew_by_at_most(BUDGET + SLACK);
}

/// The TCP port the syslog receiver listens on.
fn start_tcp_port(app: &nocturne_app::App) -> u16 {
    nocturne_logs::syslog::bound_port(app, "tcp").expect("a TCP port")
}

/// An SNMPv2c trap as large as a datagram can be: one binding of text.
fn trap(n: u32) -> Vec<u8> {
    let datagram = |length: usize| {
        ber::v2c_trap(
            "public",
            n,
            &[1, 3, 6, 1, 4, 1, 99_999, 0, 1],
            &[ber::varbind(
                &[1, 3, 6, 1, 4, 1, 99_999, 1, 1, 0],
                ber::octets(&vec![b'x'; length]),
            )],
        )
    };
    let around = datagram(60_000).len() - 60_000;
    datagram(LARGEST - around)
}

async fn traps() {
    const DATAGRAMS: u32 = 1_000;
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    // Not what is measured here: the community is checked before the queue.
    let port = start_traps_with(
        app,
        json!({"queue_limit_bytes": BUDGET, "traps": {"require_known_community": false}}),
    )
    .await;
    assert_eq!(trap(1).len(), LARGEST);
    let state = app.state::<LogsState>();
    let status = || serde_json::to_value(state.traps.status(now_ms())).unwrap();

    let held = hold_the_rules(app);
    send_each(
        port,
        std::iter::once(trap(0)),
        || number(&status(), "received"),
        || {},
    );
    wait_held("the first trap to be stored", || {
        number(&status(), "stored") == 1
    });

    let mut memory = Memory::before("traps");
    let mut most_bytes = 0;
    let mut sent = 0;
    send_each(
        port,
        (1..=DATAGRAMS).map(trap),
        || number(&status(), "received"),
        || {
            sent += 1;
            most_bytes = most_bytes.max(number(&status(), "queued_bytes"));
            if sent % 25 == 0 {
                memory.sample();
            }
        },
    );
    memory.sample();
    let flooded = status();
    held.release();
    let received = number(&flooded, "received");
    let settled = once_settled(app, "every trap received to be stored or dropped", |all| {
        traps_accounted(all) >= received
    })
    .await;
    memory.after();
    let settled = &settled["traps"];
    memory.report(&format!(
        "{} received, {} stored, {} dropped, at most {} queued",
        number(settled, "received"),
        number(settled, "stored"),
        number(settled, "dropped"),
        mib(Some(most_bytes)),
    ));

    assert!(
        number(&flooded, "received") > u64::from(DATAGRAMS) / 2,
        "the flood reached the receiver: {flooded:#}"
    );
    assert!(
        number(settled, "dropped") > 0,
        "what did not fit was dropped and counted: {settled:#}"
    );
    assert_eq!(
        number(settled, "stored") + number(settled, "dropped"),
        number(settled, "received"),
        "every trap is stored or counted as dropped: {settled:#}"
    );
    assert_eq!(settled["max_queued_bytes"], json!(BUDGET), "{settled:#}");
    assert!(
        most_bytes > 0 && most_bytes <= BUDGET,
        "the status says what the queue holds, and it held no more than it may: {most_bytes}"
    );
    memory.grew_by_at_most(BUDGET + SLACK);
}

/// NetFlow v9 (RFC 3954): a header and the given flow sets.
fn v9(sets: &[Vec<u8>]) -> Vec<u8> {
    let mut datagram = Vec::new();
    for word in [9u16, sets.len() as u16] {
        datagram.extend_from_slice(&word.to_be_bytes());
    }
    for word in [86_400_000u32, 1_763_000_000, 1, 7] {
        datagram.extend_from_slice(&word.to_be_bytes());
    }
    for set in sets {
        datagram.extend_from_slice(set);
    }
    datagram
}

fn v9_set(id: u16, content: &[u8]) -> Vec<u8> {
    let mut set = id.to_be_bytes().to_vec();
    set.extend_from_slice(&((4 + content.len()) as u16).to_be_bytes());
    set.extend_from_slice(content);
    set
}

/// Template 300: a flow record of one byte, a source address one byte
/// long. As small as a record can be, and so as many to a datagram as
/// there can be — each of them a whole record once decoded.
fn tiny_template() -> Vec<u8> {
    let mut content = Vec::new();
    for field in [300u16, 1, 8, 1] {
        content.extend_from_slice(&field.to_be_bytes());
    }
    v9_set(0, &content)
}

/// A datagram as large as one can be, of template-300 records.
fn tiny_records() -> Vec<u8> {
    let room = (LARGEST - 20 - 4) / 4 * 4;
    v9(&[v9_set(300, &vec![1; room])])
}

async fn flows() {
    const DATAGRAMS: usize = 20;
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    // A million records may wait, as many as the setting allows: the byte
    // budget is what holds.
    let port = start_flows_with(
        app,
        json!({"queue_limit": 1_000_000, "queue_limit_bytes": BUDGET}),
    )
    .await;
    let state = app.state::<LogsState>();
    let status = || serde_json::to_value(state.flows.status(now_ms())).unwrap();
    let sender = std::net::UdpSocket::bind("127.0.0.1:0").expect("a socket");
    socket2::SockRef::from(&sender)
        .set_send_buffer_size(256 * 1024)
        .expect("room for the largest datagram");
    sender
        .send_to(&v9(&[tiny_template()]), ("127.0.0.1", port))
        .unwrap();
    wait_for(app, "the template to be learned", |all| {
        all["flows"]["exporters"] == json!(1)
    })
    .await;
    let records_each = (tiny_records().len() - 24) as u64;
    // What one such datagram is once decoded, at the most: a record each,
    // and the exporter's address written out in each.
    let one_datagram =
        65_536 * (std::mem::size_of::<nocturne_netprobe::flow::FlowRecord>() as u64 + 16);

    // The counting task waits where it asks whose the exporter is; what
    // the socket decodes waits in the queue, or does not fit.
    let held = hold_the_index(app);
    let mut memory = Memory::before("flows");
    let mut most_bytes = 0;
    let datagram = tiny_records();
    for sent in 0..DATAGRAMS {
        let before = number(&status(), "received");
        sender
            .send_to(&datagram, ("127.0.0.1", port))
            .expect("the datagram is sent");
        wait_held("the datagram to be decoded", || {
            number(&status(), "received") > before
        });
        if sent == 0 {
            // Until the counting task has taken the first off the queue, the
            // next ones find it there and do not fit beside it; the counting
            // task may not get a turn until the socket stops having work.
            wait_held("the first datagram to be taken from the queue", || {
                number(&status(), "queued") == 0
            });
        }
        most_bytes = most_bytes.max(number(&status(), "queued_bytes"));
        memory.sample();
    }
    let flooded = status();
    held.release();
    let records = number(&flooded, "records");
    let settled = once_settled(
        app,
        "every record decoded to be counted or dropped",
        |all| {
            let flows = &all["flows"];
            number(flows, "queued") == 0
                && number(flows, "counted") + number(flows, "dropped") == records
        },
    )
    .await;
    memory.after();
    let settled = &settled["flows"];
    memory.report(&format!(
        "{} datagrams of {records_each} records received, {} records counted, {} dropped, at most {} queued",
        number(settled, "received"),
        number(settled, "counted"),
        number(settled, "dropped"),
        mib(Some(most_bytes)),
    ));

    assert_eq!(
        number(settled, "records"),
        records_each * DATAGRAMS as u64,
        "every datagram was decoded: {settled:#}"
    );
    assert!(
        settled["drops"]["queue_full"].as_u64().unwrap_or_default() > 0,
        "what did not fit was dropped and counted, as a full queue: {settled:#}"
    );
    assert_eq!(settled["max_queued_bytes"], json!(BUDGET), "{settled:#}");
    // The counting task took the first datagram and was held with it; the
    // second found the queue empty, and one datagram is always let onto an
    // empty queue whatever it holds — this one more than twice the budget.
    // Nothing else got in.
    assert_eq!(
        number(settled, "counted"),
        2 * records_each,
        "what was being counted and the one datagram an empty queue takes were all that was counted: {settled:#}"
    );
    assert!(
        most_bytes > BUDGET && most_bytes <= one_datagram,
        "the status says what the queue holds, and it held that one datagram: {most_bytes}"
    );
    // The datagram waiting, the one being counted, and the one being
    // decoded — every datagram is, before it is known not to fit, and its
    // records grow by doubling while it is, so twice over. Twenty datagrams
    // queued as they came would be ten times this.
    memory.grew_by_at_most(4 * one_datagram + SLACK);
}

/// A stored row as large as a line can make one: its message and, in its
/// body, the line as received.
fn largest_entry(n: i64) -> nocturne_app::store::LogEntry {
    nocturne_app::store::LogEntry {
        id: 1_000_000 + n,
        ts: now_ms(),
        stream: "syslog".into(),
        device_id: None,
        severity: Some(5),
        source: "127.0.0.1".into(),
        message: "x".repeat(LARGEST),
        body: Some(json!({"raw": "x".repeat(LARGEST), "via": "syslog"})),
    }
}

async fn forwarder() {
    const ENTRIES: i64 = 1_000;
    let lab = lab(&[]).await;
    let app = &lab.app;
    // A collector that is not there: every attempt is refused at once, and
    // the forwarder waits a second, then two, between them — holding what
    // it took, with the rest in its queue.
    let gone = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let saved = app
        .rpc(
            "logs.forward.save",
            json!({"name": "Gone", "kind": "syslog_tcp", "host": "127.0.0.1", "port": gone, "streams": ["syslog"]}),
        )
        .await
        .unwrap();
    let id = saved["id"].as_str().unwrap().to_string();
    let row = || {
        let all = serde_json::to_value(nocturne_logs::forward::status(app).unwrap()).unwrap();
        all.as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == json!(id))
            .cloned()
            .expect("the forwarder's row")
    };
    let index = nocturne_logs::attribute::refresh(app, now_ms(), 0).unwrap();

    // What the store hands a forwarder after each write, as large as a
    // stored row can be. Each is offered, and dropped here: only what the
    // queue keeps is left of it.
    let mut memory = Memory::before("forwarder");
    let mut most_bytes = 0;
    for n in 0..ENTRIES {
        let entry = largest_entry(n);
        nocturne_logs::forward::offer(app, &index, std::iter::once(&entry));
        if n % 25 == 24 {
            memory.sample();
            most_bytes = most_bytes.max(number(&row(), "queued_bytes"));
        }
    }
    memory.sample();
    let flooded = row();
    // Deleting it lets go of what waited, and counts it.
    app.rpc("logs.forward.delete", json!({"id": id}))
        .await
        .unwrap();
    memory.after();
    let budget = flooded["max_queued_bytes"].as_u64();
    memory.report(&format!(
        "{ENTRIES} offered, {} dropped, {} waiting, at most {} queued of {}",
        number(&flooded, "dropped"),
        number(&flooded, "queued"),
        mib(Some(most_bytes)),
        mib(budget),
    ));

    assert!(
        number(&flooded, "dropped") > 0,
        "what did not fit was dropped and counted: {flooded:#}"
    );
    let budget = budget.expect("the status says how much the queue may hold");
    assert!(
        most_bytes > 0 && most_bytes <= budget,
        "the status says what the queue holds, and it held no more than it may: {most_bytes} of {budget}"
    );
    memory.grew_by_at_most(budget + SLACK);
}
