//! Shared test support: an app with the log feature, devices with addresses,
//! and a real receiver on an ephemeral port.
//!
//! Every test here sends real syslog over a real socket. Nothing binds a
//! fixed port: the receiver is configured with port 0 and the test asks
//! `logs.status` which ports it got.

#![allow(dead_code)]

use std::time::Duration;

use nocturne_app::App;
use nocturne_app::testing::Lab;
use serde_json::{Value, json};
use tokio::net::{TcpStream, UdpSocket};

/// An app with the core, the log feature and `names` as devices.
pub async fn lab(names: &[&str]) -> Lab {
    let mut builder = nocturne_app::testing::builder();
    nocturne_logs::init(&mut builder);
    nocturne_app::testing::lab(builder, names).await
}

/// What stands in for the monitoring feature: `alerts.raise` and
/// `alerts.clear` as its specification describes them, keeping only which
/// dedup keys are open. Every call is published as `test.alert_raised` /
/// `test.alert_cleared` (the parameters, plus `cleared` for a clear), so a
/// test sees the hand-off without this crate linking to the monitor.
#[derive(Clone, Default)]
pub struct FakeMonitor {
    pub open: std::sync::Arc<std::sync::Mutex<std::collections::BTreeSet<String>>>,
    /// How often each method was called.
    pub raises: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub clears: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// A monitor that takes this long over `alerts.clear` (milliseconds).
    pub clear_delay_ms: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// A monitor that does not answer `alerts.raise` until this is lowered.
    pub hold_raises: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl FakeMonitor {
    pub fn open_keys(&self) -> Vec<String> {
        self.open.lock().unwrap().iter().cloned().collect()
    }

    pub fn raise_calls(&self) -> u64 {
        self.raises.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn clear_calls(&self) -> u64 {
        self.clears.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// An alert that is open although this process never raised it — as
    /// after a restart.
    pub fn already_open(&self, dedup_key: &str) {
        self.open.lock().unwrap().insert(dedup_key.to_string());
    }
}

/// An app with the log feature, `names` as devices, and a [`FakeMonitor`].
pub async fn lab_with_monitor(names: &[&str]) -> (Lab, FakeMonitor) {
    let monitor = FakeMonitor::default();
    let mut builder = nocturne_app::testing::builder();
    nocturne_logs::init(&mut builder);
    let raised = monitor.clone();
    builder
        .router
        .add("alerts.raise", move |app: App, params: Value| {
            let raised = raised.clone();
            async move {
                use std::sync::atomic::Ordering;
                while raised.hold_raises.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                raised.raises.fetch_add(1, Ordering::SeqCst);
                let key = params["dedup_key"].as_str().unwrap_or_default().to_string();
                raised.open.lock().unwrap().insert(key);
                app.publish("test.alert_raised", &params);
                Ok(json!({"id": "alert1"}))
            }
        });
    let cleared = monitor.clone();
    builder
        .router
        .add("alerts.clear", move |app: App, params: Value| {
            let cleared = cleared.clone();
            async move {
                use std::sync::atomic::Ordering;
                let delay = cleared.clear_delay_ms.load(Ordering::SeqCst);
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                cleared.clears.fetch_add(1, Ordering::SeqCst);
                let key = params["dedup_key"].as_str().unwrap_or_default();
                let was_open = cleared.open.lock().unwrap().remove(key);
                let mut seen = params.clone();
                seen["cleared"] = json!(was_open);
                app.publish("test.alert_cleared", &seen);
                Ok(json!({"cleared": was_open}))
            }
        });
    (nocturne_app::testing::lab(builder, names).await, monitor)
}

/// Wait until `ready` says so. Panics with `what` after five seconds.
pub async fn eventually(what: &str, ready: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !ready() {
        if std::time::Instant::now() > deadline {
            panic!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Long enough for a call that was (wrongly) queued to have been made:
/// what a test waits before it says that something did *not* happen.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(60)).await;
}

/// The signals of one rule out of a list of `logs.signal` payloads.
pub fn of_rule(signals: &[Value], rule_id: &str) -> Vec<Value> {
    signals
        .iter()
        .filter(|signal| signal["rule_id"] == json!(rule_id))
        .cloned()
        .collect()
}

/// Give a device the address its syslog will appear to come from.
pub async fn set_address(app: &App, device_id: &str, address: &str) {
    app.rpc(
        "devices.update",
        json!({"id": device_id, "address": address}),
    )
    .await
    .expect("the address is set");
}

/// A port nothing is using right now. `tcp_port: 0` means "no TCP listener"
/// in the settings, so a test that wants one has to name a port; the caller
/// below copes with it having been taken in between.
async fn free_tcp_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port");
    let port = listener.local_addr().expect("an address").port();
    drop(listener);
    port
}

/// Switch the receiver on (UDP on an ephemeral port, TCP on a free one) and
/// wait until it is bound. Returns `(udp_port, tcp_port)`.
pub async fn start_receiver(app: &App) -> (u16, u16) {
    let mut status = Value::Null;
    for _ in 0..5 {
        let tcp = free_tcp_port().await;
        app.rpc(
            "settings.set",
            json!({"section": "logs", "value": {"syslog": {"enabled": true, "udp_port": 0, "tcp_port": tcp, "bind": "127.0.0.1"}}}),
        )
        .await
        .expect("the settings are stored");
        status = wait_for(app, "the receiver to bind or refuse", |status| {
            status["syslog"]["listening"]
                .as_array()
                .is_some_and(|listening| listening.len() == 2)
                || !status["syslog"]["error"].is_null()
        })
        .await;
        if status["syslog"]["error"].is_null() {
            break;
        }
    }
    assert!(
        status["syslog"]["error"].is_null(),
        "the receiver did not start: {status:#}"
    );
    let port = |protocol: &str| {
        status["syslog"]["listening"]
            .as_array()
            .expect("listening")
            .iter()
            .filter_map(Value::as_str)
            .find_map(|entry| {
                entry
                    .strip_prefix(protocol)?
                    .trim()
                    .rsplit(':')
                    .next()?
                    .parse::<u16>()
                    .ok()
            })
            .unwrap_or_else(|| panic!("a {protocol} port in {:?}", status["syslog"]["listening"]))
    };
    (port("udp"), port("tcp"))
}

/// Poll `logs.status` until `ready` says so. Panics with the reason after
/// five seconds, which is a hung receiver, not a slow machine.
pub async fn wait_for(app: &App, what: &str, ready: impl Fn(&Value) -> bool) -> Value {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let status = app
            .rpc("logs.status", Value::Null)
            .await
            .expect("the status is readable");
        if ready(&status) {
            return status;
        }
        if std::time::Instant::now() > deadline {
            panic!("timed out waiting for {what}; status was {status:#}");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Wait until the receiver has accounted for `received` lines (stored or
/// deliberately dropped).
pub async fn wait_until_settled(app: &App, received: u64) -> Value {
    wait_for(app, "the receiver to settle", |status| {
        let syslog = &status["syslog"];
        syslog["received"].as_u64().unwrap_or_default() >= received
            && syslog["stored"].as_u64().unwrap_or_default()
                + syslog["dropped"].as_u64().unwrap_or_default()
                >= received
    })
    .await
}

/// Send datagrams from a chosen source address.
pub async fn send_udp(from: &str, port: u16, lines: &[&str]) {
    let socket = UdpSocket::bind(format!("{from}:0"))
        .await
        .unwrap_or_else(|err| panic!("a socket on {from}: {err}"));
    for line in lines {
        socket
            .send_to(line.as_bytes(), ("127.0.0.1", port))
            .await
            .expect("the datagram is sent");
    }
}

/// Send bytes over one TCP connection (the caller frames them).
pub async fn send_tcp(from: &str, port: u16, payload: &[u8]) {
    use tokio::io::AsyncWriteExt;
    let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
    socket
        .bind(format!("{from}:0").parse().expect("an address"))
        .expect("the source address is usable");
    let mut stream: TcpStream = socket
        .connect(format!("127.0.0.1:{port}").parse().expect("an address"))
        .await
        .expect("the receiver accepts the connection");
    stream.write_all(payload).await.expect("the lines are sent");
    stream.shutdown().await.expect("the connection closes");
}

/// The entries of a query, newest first.
pub async fn query(app: &App, params: Value) -> Vec<Value> {
    app.rpc("logs.query", params).await.expect("the query runs")["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The messages of a query, newest first.
pub async fn messages(app: &App, params: Value) -> Vec<String> {
    query(app, params)
        .await
        .into_iter()
        .map(|entry| entry["message"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Store lines through the real ingest path, at times of the test's
/// choosing: `(line, source address, timestamp)`.
///
/// The socket is the only thing skipped — parsing, attribution, redaction,
/// the index rows and the source list are the receiver's own code.
pub fn seed(app: &App, lines: &[(&str, &str, i64)]) -> Vec<i64> {
    seed_prepared(app, lines)
        .iter()
        .map(|item| item.entry.id)
        .collect()
}

/// Store lines and run the rules over them, at times of the test's
/// choosing: the rest of what the writer does after a batch is committed.
pub fn receive(app: &App, lines: &[(&str, &str, i64)]) -> Vec<nocturne_logs::db::Prepared> {
    let index = nocturne_logs::attribute::refresh(app, 0, 0).expect("the index builds");
    let batch = seed_prepared(app, lines);
    nocturne_logs::rules::evaluate(app, &index, &batch);
    nocturne_logs::tail::offer(app, &batch);
    batch
}

fn seed_prepared(app: &App, lines: &[(&str, &str, i64)]) -> Vec<nocturne_logs::db::Prepared> {
    let state = app.state::<nocturne_logs::state::LogsState>();
    let index = nocturne_logs::attribute::refresh(app, 0, 0).expect("the index builds");
    let mut batch = Vec::new();
    for (line, source, ts) in lines {
        let peer = format!("{source}:514").parse().expect("an address");
        let parsed = nocturne_netprobe::syslog::parse(line.as_bytes(), peer);
        let mut prepared =
            nocturne_logs::ingest::prepare(&state, &index, &parsed, *ts, "syslog", None);
        prepared.entry.ts = *ts;
        batch.push(prepared);
    }
    nocturne_logs::db::insert_batch(app.store(), &mut batch).expect("the batch is written");
    batch
}

/// The next event on a topic, or a panic after two seconds.
pub async fn next_event(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
    topic: &str,
) -> Value {
    let deadline = Duration::from_secs(2);
    let wanted = topic.to_string();
    tokio::time::timeout(deadline, async {
        loop {
            match events.recv().await {
                Ok(event) if event.topic == wanted => return event.payload,
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(err) => panic!("the event bus closed while waiting for {wanted}: {err}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no `{topic}` event arrived"))
}

/// Every event already waiting, with its topic. One call takes the whole
/// backlog, so a test that cares about two topics drains once.
pub fn drain_all(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    while let Ok(event) = events.try_recv() {
        found.push((event.topic, event.payload));
    }
    found
}

/// The payloads of one topic out of a drained backlog.
pub fn topic<'a>(events: &'a [(String, Value)], topic: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|(name, _)| name == topic)
        .map(|(_, payload)| payload)
        .collect()
}

/// Events on a topic that are already waiting, without blocking. Consumes
/// the whole backlog: use `drain_all` when more than one topic matters.
pub fn drained(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
    topic: &str,
) -> Vec<Value> {
    let mut found = Vec::new();
    while let Ok(event) = events.try_recv() {
        if event.topic == topic {
            found.push(event.payload);
        }
    }
    found
}

/// Whether this machine lets a test use a second loopback address, which is
/// how two devices are told apart by source address.
pub async fn loopback_alias(address: &str) -> bool {
    UdpSocket::bind(format!("{address}:0")).await.is_ok()
}

// ------------------------------------------------------------------ traps

/// Bytes from the hex a capture was written down in (whitespace ignored).
pub fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| {
            u8::from_str_radix(std::str::from_utf8(pair).expect("hex"), 16).expect("hex digits")
        })
        .collect()
}

/// Store an SNMPv2c community in a credential, as a person would in the
/// credential's form. It goes to the vault.
pub async fn set_community(app: &App, credential_id: &str, community: &str) {
    app.rpc(
        "credentials.update",
        json!({"id": credential_id, "snmp_version": "v2c", "snmp_community": community}),
    )
    .await
    .expect("the community is stored");
}

/// Switch the trap receiver on, on an ephemeral port of 127.0.0.1, with
/// whatever else the test wants in the `logs` section. Returns the port.
pub async fn start_traps_with(app: &App, mut section: Value) -> u16 {
    let wanted = json!({"enabled": true, "port": 0, "bind": "127.0.0.1"});
    match section["traps"].as_object_mut() {
        Some(traps) => {
            for (key, value) in wanted.as_object().expect("an object") {
                traps.entry(key.clone()).or_insert(value.clone());
            }
        }
        None => section["traps"] = wanted,
    }
    app.rpc("settings.set", json!({"section": "logs", "value": section}))
        .await
        .expect("the settings are stored");
    let status = wait_for(app, "the trap receiver to bind or refuse", |status| {
        status["traps"]["listening"]
            .as_array()
            .is_some_and(|listening| !listening.is_empty())
            || !status["traps"]["error"].is_null()
    })
    .await;
    assert!(
        status["traps"]["error"].is_null(),
        "the trap receiver did not start: {status:#}"
    );
    trap_port(&status)
}

pub async fn start_traps(app: &App) -> u16 {
    start_traps_with(app, json!({})).await
}

/// The port in a status's `traps.listening`.
pub fn trap_port(status: &Value) -> u16 {
    status["traps"]["listening"][0]
        .as_str()
        .and_then(|entry| entry.rsplit(':').next()?.parse().ok())
        .unwrap_or_else(|| panic!("a port in {:?}", status["traps"]["listening"]))
}

/// Everything the trap receiver has decided about, added up: every datagram
/// it takes off the socket ends in exactly one of these.
pub fn traps_accounted(status: &Value) -> u64 {
    [
        "stored",
        "dropped",
        "rejected",
        "unauthenticated",
        "malformed",
        "duplicates",
    ]
    .iter()
    .map(|key| status["traps"][key].as_u64().unwrap_or_default())
    .sum()
}

/// Wait until the trap receiver has accounted for `received` datagrams.
pub async fn wait_until_traps_settled(app: &App, received: u64) -> Value {
    wait_for(app, "the trap receiver to settle", |status| {
        status["traps"]["received"].as_u64().unwrap_or_default() >= received
            && traps_accounted(status) >= received
    })
    .await
}

/// Send datagrams to the trap receiver from a chosen source address, over
/// one socket (so they share a source port).
pub async fn send_datagrams(from: &str, port: u16, datagrams: &[Vec<u8>]) -> UdpSocket {
    let socket = UdpSocket::bind(format!("{from}:0"))
        .await
        .unwrap_or_else(|err| panic!("a socket on {from}: {err}"));
    for datagram in datagrams {
        socket
            .send_to(datagram, ("127.0.0.1", port))
            .await
            .expect("the datagram is sent");
    }
    socket
}

/// BER, by hand (X.690 as RFC 3416 uses it): only what a test needs to
/// write a datagram no tool will send for it.
pub mod ber {
    fn length(n: usize) -> Vec<u8> {
        match n {
            0..=127 => vec![n as u8],
            128..=255 => vec![0x81, n as u8],
            _ => vec![0x82, (n >> 8) as u8, n as u8],
        }
    }

    pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        out.extend(length(content.len()));
        out.extend_from_slice(content);
        out
    }

    /// A small non-negative INTEGER.
    pub fn integer(value: u32) -> Vec<u8> {
        let bytes = value.to_be_bytes();
        let skip = bytes.iter().take(3).take_while(|byte| **byte == 0).count();
        let mut content = bytes[skip..].to_vec();
        if content[0] & 0x80 != 0 {
            content.insert(0, 0);
        }
        tlv(0x02, &content)
    }

    pub fn octets(bytes: &[u8]) -> Vec<u8> {
        tlv(0x04, bytes)
    }

    pub fn null() -> Vec<u8> {
        tlv(0x05, &[])
    }

    pub fn oid(parts: &[u32]) -> Vec<u8> {
        let mut content = vec![(parts[0] * 40 + parts[1]) as u8];
        for part in &parts[2..] {
            let mut digits = vec![(part & 0x7f) as u8];
            let mut rest = part >> 7;
            while rest > 0 {
                digits.push((rest & 0x7f) as u8 | 0x80);
                rest >>= 7;
            }
            digits.reverse();
            content.extend(digits);
        }
        tlv(0x06, &content)
    }

    pub fn varbind(name: &[u32], value: Vec<u8>) -> Vec<u8> {
        let mut content = oid(name);
        content.extend(value);
        tlv(0x30, &content)
    }

    /// An SNMPv2c trap: sysUpTime.0, snmpTrapOID.0 = `trap`, then `binds`.
    pub fn v2c_trap(community: &str, request: u32, trap: &[u32], binds: &[Vec<u8>]) -> Vec<u8> {
        let mut list = varbind(&[1, 3, 6, 1, 2, 1, 1, 3, 0], tlv(0x43, &[0x30, 0x39]));
        list.extend(varbind(&[1, 3, 6, 1, 6, 3, 1, 1, 4, 1, 0], oid(trap)));
        for bind in binds {
            list.extend_from_slice(bind);
        }
        let mut pdu = integer(request);
        pdu.extend(integer(0));
        pdu.extend(integer(0));
        pdu.extend(tlv(0x30, &list));
        let mut message = integer(1);
        message.extend(octets(community.as_bytes()));
        message.extend(tlv(0xa7, &pdu));
        tlv(0x30, &message)
    }
}

// ------------------------------------------------------------------ flows

/// Switch the flow collector on, on an ephemeral port of 127.0.0.1, with
/// whatever else the test wants in the `logs` section. Returns the port.
pub async fn start_flows_with(app: &App, mut section: Value) -> u16 {
    let wanted = json!({"enabled": true, "port": 0, "bind": "127.0.0.1"});
    match section["flows"].as_object_mut() {
        Some(flows) => {
            for (key, value) in wanted.as_object().expect("an object") {
                flows.entry(key.clone()).or_insert(value.clone());
            }
        }
        None => section["flows"] = wanted,
    }
    app.rpc("settings.set", json!({"section": "logs", "value": section}))
        .await
        .expect("the settings are stored");
    let status = wait_for(app, "the flow collector to bind or refuse", |status| {
        status["flows"]["listening"]
            .as_array()
            .is_some_and(|listening| !listening.is_empty())
            || !status["flows"]["error"].is_null()
    })
    .await;
    assert!(
        status["flows"]["error"].is_null(),
        "the flow collector did not start: {status:#}"
    );
    flow_port(&status)
}

pub async fn start_flows(app: &App) -> u16 {
    start_flows_with(app, json!({})).await
}

/// The port in a status's `flows.listening`.
pub fn flow_port(status: &Value) -> u16 {
    status["flows"]["listening"][0]
        .as_str()
        .and_then(|entry| entry.rsplit(':').next()?.parse().ok())
        .unwrap_or_else(|| panic!("a port in {:?}", status["flows"]["listening"]))
}

/// Wait until the collector has decoded `records` flow records and every
/// one of them is either in the buckets or counted as dropped.
pub async fn wait_until_flows_settled(app: &App, records: u64) -> Value {
    wait_for(app, "the flow collector to settle", |status| {
        let flows = &status["flows"];
        let of = |key: &str| flows[key].as_u64().unwrap_or_default();
        of("records") >= records
            && of("queued") == 0
            && of("counted") + of("dropped") == of("records")
    })
    .await
}

/// Every table of the database, every row, as text: what "stored
/// somewhere" means when a test says a value is stored nowhere.
pub fn everything_stored(app: &App) -> String {
    use rusqlite::types::ValueRef;
    app.store()
        .with(|conn| {
            let tables: Vec<String> = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            let mut out = String::new();
            for table in tables {
                let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\""))?;
                let columns = statement.column_count();
                let mut rows = statement.query([])?;
                while let Some(row) = rows.next()? {
                    out.push_str(&table);
                    for column in 0..columns {
                        out.push('|');
                        match row.get_ref(column)? {
                            ValueRef::Null => {}
                            ValueRef::Integer(value) => out.push_str(&value.to_string()),
                            ValueRef::Real(value) => out.push_str(&value.to_string()),
                            ValueRef::Text(text) | ValueRef::Blob(text) => {
                                out.push_str(&String::from_utf8_lossy(text));
                            }
                        }
                    }
                    out.push('\n');
                }
            }
            Ok(out)
        })
        .expect("the database is readable")
}

/// Every log line written on this thread while this is alive.
///
/// A `#[tokio::test]` runs its tasks on the test's own thread, so this
/// sees what the receiver's tasks log as well as what the test does.
pub struct LogLines {
    lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    _guard: tracing::subscriber::DefaultGuard,
}

impl LogLines {
    pub fn capture() -> LogLines {
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let guard = tracing::subscriber::set_default(Collector(lines.clone()));
        LogLines {
            lines,
            _guard: guard,
        }
    }

    pub fn text(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }
}

struct Collector(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        use std::fmt::Write;
        let _ = write!(self.0, " {}={value:?}", field.name());
    }
}

impl tracing::Subscriber for Collector {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let mut line = Fields(format!("span {}", span.metadata().name()));
        span.record(&mut line);
        self.0.lock().unwrap().push(line.0);
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let mut line = Fields("span".to_string());
        values.record(&mut line);
        self.0.lock().unwrap().push(line.0);
    }

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        let mut line = Fields(event.metadata().target().to_string());
        event.record(&mut line);
        self.0.lock().unwrap().push(line.0);
    }

    fn enter(&self, _: &tracing::span::Id) {}

    fn exit(&self, _: &tracing::span::Id) {}
}

/// A net-snmp command-line tool, when it is installed.
pub fn net_snmp(tool: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain([
            std::path::PathBuf::from("/usr/bin"),
            std::path::PathBuf::from("/usr/local/bin"),
        ])
        .map(|directory| directory.join(tool))
        .find(|candidate| candidate.is_file())
}

/// Run a net-snmp tool and return whether it exited successfully.
///
/// It is given no MIBs to load and an empty directory for the files it
/// likes to write, so it neither depends on what this machine has
/// installed nor leaves anything behind. OIDs are therefore numbers.
pub async fn run_net_snmp(tool: &std::path::Path, arguments: &[String]) -> bool {
    let scratch = tempfile::tempdir().expect("a temporary directory");
    let output = tokio::process::Command::new(tool)
        .args(arguments)
        .env("MIBS", "")
        .env("SNMP_PERSISTENT_DIR", scratch.path())
        .env("SNMPCONFPATH", scratch.path())
        .env("HOME", scratch.path())
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .unwrap_or_else(|err| panic!("{} could not be run: {err}", tool.display()));
    output.status.success()
}
