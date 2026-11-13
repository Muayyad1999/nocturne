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
