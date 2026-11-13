//! Under load.
//!
//! Rate is the risk in a log receiver: a campus with a flapping uplink and a
//! spanning-tree problem can send tens of thousands of lines a minute, and
//! the application has to stay up, keep the database bounded and tell the
//! truth about what it threw away.
//!
//! The lines are not written by hand. The simulator's devices emit the log
//! messages a real switch emits when a port goes down, an EtherChannel fails
//! to form or a device reboots; these tests take those, put a priority on
//! the front the way a device does, and send them over a real socket.
//!
//! Most of the volume goes over TCP (RFC 6587). Not for convenience: a
//! hundred thousand datagrams at a loopback UDP socket are mostly dropped by
//! the *kernel* before any of this code sees them, and a test whose numbers
//! are decided by the size of a socket buffer says nothing about the
//! receiver. UDP has a test of its own that says exactly that.

mod support;

use std::time::{Duration, Instant};

use nocturne_netsim::{Lab, LabEvent};
use serde_json::{Value, json};
use support::*;

/// Log lines the simulator's own devices produced. Ports are shut and
/// brought back to make the campus talk.
fn simulated_lines() -> Vec<String> {
    let mut lab = Lab::from_spec(&nocturne_netsim::demo_campus()).expect("the demo lab builds");
    lab.settle(30_000_000);
    lab.take_events();
    let names: Vec<String> = lab
        .to_spec()
        .nodes
        .iter()
        .map(|node| node.name.clone())
        .collect();
    let mut lines = Vec::new();
    for name in &names {
        let Some(id) = lab.node_by_name(name) else {
            continue;
        };
        for port in ["GigabitEthernet0/1", "GigabitEthernet0/2"] {
            for state in ["shutdown", "no shutdown"] {
                let _ = lab.configure(id, &[format!("interface {port}"), state.to_string()]);
                lab.settle(5_000_000);
                lines.extend(
                    lab.take_events()
                        .into_iter()
                        .filter_map(|event| match event {
                            LabEvent::Syslog { severity, line, .. } => {
                                // A device puts the priority in front: facility
                                // local7 (23), and the severity of the message.
                                Some(format!(
                                    "<{}>{}",
                                    23 * 8 + u16::from(severity),
                                    line.replace('\n', " ")
                                ))
                            }
                            _ => None,
                        }),
                );
            }
        }
        if lines.len() > 200 {
            break;
        }
    }
    assert!(
        lines.len() > 20,
        "the simulator should have produced log lines, got {}",
        lines.len()
    );
    lines
}

/// One line of the storm: the simulator's line with a sequence number, so no
/// two are byte-identical (which is also what `service sequence-numbers` does).
fn line_at(lines: &[String], index: usize) -> String {
    lines[index % lines.len()].replacen('>', &format!(">{index}: "), 1)
}

/// Send `count` lines over one newline-framed TCP connection.
async fn blast_tcp(port: u16, lines: &[String], count: usize) -> usize {
    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the receiver accepts the connection");
    let mut chunk = String::with_capacity(64 * 1024);
    for index in 0..count {
        chunk.push_str(&line_at(lines, index));
        chunk.push('\n');
        if chunk.len() >= 32 * 1024 {
            stream.write_all(chunk.as_bytes()).await.expect("written");
            chunk.clear();
        }
    }
    stream.write_all(chunk.as_bytes()).await.expect("written");
    stream.shutdown().await.expect("the connection closes");
    count
}

/// Send `count` datagrams as fast as the socket takes them.
async fn blast_udp(port: u16, lines: &[String], count: usize) -> usize {
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0")
        .await
        .expect("a socket");
    let target: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().expect("an address");
    let mut sent = 0;
    for index in 0..count {
        if socket
            .send_to(line_at(lines, index).as_bytes(), target)
            .await
            .is_ok()
        {
            sent += 1;
        }
    }
    sent
}

fn counters(status: &Value) -> (u64, u64, u64) {
    let syslog = &status["syslog"];
    (
        syslog["received"].as_u64().unwrap_or_default(),
        syslog["stored"].as_u64().unwrap_or_default(),
        syslog["dropped"].as_u64().unwrap_or_default(),
    )
}

/// Wait until the storm is over: the counters have stopped moving, the
/// queue is empty, and everything received is accounted for.
///
/// "Accounted for" alone is not enough to wait on — it is momentarily true
/// between two batches, while the sender is still going.
async fn settled(app: &nocturne_app::App) -> Value {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last = u64::MAX;
    let mut still = 0;
    loop {
        let status = app
            .rpc("logs.status", Value::Null)
            .await
            .expect("the status is readable");
        let (received, stored, dropped) = counters(&status);
        let quiet = received == last
            && received > 0
            && stored + dropped == received
            && status["syslog"]["queued"].as_u64() == Some(0);
        still = if quiet { still + 1 } else { 0 };
        if still >= 6 {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "the receiver never settled: {status:#}"
        );
        last = received;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_storm_that_outruns_the_disk_is_dropped_and_counted_not_buffered() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    // A deliberately tiny queue: the same path a default queue takes when a
    // campus sends faster than the disk can take it, without needing
    // millions of lines to get there.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"queue_limit": 200}}),
    )
    .await
    .unwrap();
    let (_, tcp) = start_receiver(app).await;
    let lines = simulated_lines();

    // Hold the database for a moment so the writer cannot drain while the
    // storm arrives: what a slow disk does, on purpose.
    let store = app.store().clone();
    let blocked = std::thread::spawn(move || {
        store
            .with(|_| {
                std::thread::sleep(Duration::from_millis(300));
                Ok(())
            })
            .expect("the lock is taken")
    });

    let started = Instant::now();
    let sent = blast_tcp(tcp, &lines, 20_000).await;
    blocked.join().expect("the holder finishes");
    let status = settled(app).await;
    let elapsed = started.elapsed();

    let (received, stored, dropped) = counters(&status);
    eprintln!(
        "tcp storm: sent {sent}, received {received}, stored {stored}, dropped {dropped} in {elapsed:?}"
    );

    assert_eq!(
        received, sent as u64,
        "TCP does not lose lines, so every one of them was seen: {status:#}"
    );
    assert!(
        dropped > 0,
        "with a 200-line queue and a blocked writer, lines must have been \
         dropped — and this is the number the user is shown: {status:#}"
    );
    assert_eq!(
        stored + dropped,
        received,
        "every line is either stored or counted as dropped: {status:#}"
    );
    assert!(
        status["syslog"]["queued"].as_u64().unwrap() <= 200,
        "the queue never grows past its limit: {status:#}"
    );
    assert!(
        elapsed < Duration::from_secs(60),
        "the storm must not take minutes: {elapsed:?}"
    );

    // What the status claims is stored is what is in the database, and all
    // of it is searchable.
    assert_eq!(status["store"]["rows"].as_u64().unwrap(), stored);
    let page = query(app, json!({"limit": 50})).await;
    assert_eq!(page.len(), 50);
    assert!(
        page[0]["body"]["mnemonic"].is_string(),
        "including what the device meant: {:#}",
        page[0]
    );

    // The application still answers, which is the other half of "does not
    // fall over".
    assert!(app.rpc("logs.sources", Value::Null).await.is_ok());
    let histogram = app
        .rpc("logs.histogram", json!({"buckets": 10}))
        .await
        .unwrap();
    let counted: i64 = histogram
        .as_array()
        .unwrap()
        .iter()
        .map(|bucket| bucket["count"].as_i64().unwrap_or_default())
        .sum();
    assert_eq!(counted as u64, stored, "the histogram counts what is there");
}

/// A hundred thousand lines pushed in under a second is roughly six million
/// a minute — two orders of magnitude above the "tens of thousands a minute"
/// a chattery campus produces. The point is not that none is lost: it is
/// that the queue absorbs its whole depth, the rest is counted, and
/// everything that was kept is intact and searchable afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hundred_thousand_lines_are_written_in_batches_and_stay_searchable() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let (_, tcp) = start_receiver(app).await;
    let lines = simulated_lines();

    let started = Instant::now();
    let sent = blast_tcp(tcp, &lines, 100_000).await;
    let status = settled(app).await;
    let elapsed = started.elapsed();

    let (received, stored, dropped) = counters(&status);
    eprintln!(
        "tcp flood: sent {sent}, received {received}, stored {stored}, dropped {dropped} \
         in {elapsed:?} ({:.0} lines/s)",
        stored as f64 / elapsed.as_secs_f64().max(0.001)
    );

    assert_eq!(received, sent as u64, "{status:#}");
    assert_eq!(stored + dropped, received, "{status:#}");
    assert!(
        stored >= 50_000,
        "the queue absorbs its whole depth before anything is dropped: {status:#}"
    );
    assert_eq!(status["store"]["rows"].as_u64().unwrap(), stored);
    assert!(
        elapsed < Duration::from_secs(120),
        "a hundred thousand lines must not take minutes: {elapsed:?}"
    );

    // A query over a hundred thousand rows is still a query.
    let search = Instant::now();
    let found = query(app, json!({"text": "UPDOWN", "limit": 100})).await;
    assert_eq!(found.len(), 100);
    assert!(
        search.elapsed() < Duration::from_secs(5),
        "searching took {:?}",
        search.elapsed()
    );

    // Retention still applies to what a storm leaves behind.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"max_rows": 1_000}}),
    )
    .await
    .unwrap();
    let pruned = nocturne_logs::ingest::prune_once(app).unwrap();
    assert!(pruned.by_rows > 0);
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["store"]["rows"], json!(1_000));
}

/// A rate a real campus reaches, rather than a burst nothing could absorb:
/// at this rate nothing is dropped at all, even in a test build.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sustained_rate_loses_nothing() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let (_, tcp) = start_receiver(app).await;
    let lines = simulated_lines();

    use tokio::io::AsyncWriteExt;
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", tcp))
        .await
        .expect("connected");
    let started = Instant::now();
    let total = 8_000;
    // Eighty batches of a hundred, ten milliseconds apart: 10,000 lines a
    // second, or 600,000 a minute.
    for batch in 0..80 {
        let mut chunk = String::new();
        for index in 0..100 {
            chunk.push_str(&line_at(&lines, batch * 100 + index));
            chunk.push('\n');
        }
        stream.write_all(chunk.as_bytes()).await.expect("written");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stream.shutdown().await.expect("closed");

    let status = settled(app).await;
    let (received, stored, dropped) = counters(&status);
    eprintln!(
        "sustained: sent {total}, received {received}, stored {stored}, dropped {dropped} in {:?}",
        started.elapsed()
    );
    assert_eq!(received, total as u64);
    assert_eq!(stored, total as u64, "nothing was dropped: {status:#}");
    assert_eq!(dropped, 0);
    assert_eq!(status["store"]["rows"].as_u64().unwrap(), stored);
    assert!(
        status["syslog"]["rate_per_minute"].as_u64().unwrap() > 0,
        "and the rate the user is shown is not zero: {status:#}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn what_the_kernel_drops_before_us_is_not_claimed_as_received() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let (udp, _) = start_receiver(app).await;
    let lines = simulated_lines();

    let sent = blast_udp(udp, &lines, 20_000).await;
    let status = settled(app).await;
    let (received, stored, dropped) = counters(&status);
    eprintln!("udp storm: sent {sent}, received {received}, stored {stored}, dropped {dropped}");

    assert!(
        received <= sent as u64,
        "the receiver cannot have seen more datagrams than were sent: {status:#}"
    );
    assert_eq!(
        stored + dropped,
        received,
        "what arrived is accounted for, and what the socket buffer swallowed \
         before this process saw it is not counted as if it had: {status:#}"
    );
    assert!(stored > 0, "{status:#}");
    assert_eq!(status["store"]["rows"].as_u64().unwrap(), stored);
    assert!(app.rpc("logs.status", Value::Null).await.is_ok());
}
