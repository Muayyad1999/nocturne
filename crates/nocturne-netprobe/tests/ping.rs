//! Loopback tests of the pinger with whatever method this machine allows, plus a forced run
//! through the system `ping` program.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::ping::{self, Method, PingEcho, PingOptions, Pinger};

const LOCALHOST: &str = "127.0.0.1";

fn ipv6_available() -> bool {
    std::net::UdpSocket::bind("[::1]:0").is_ok()
}

/// Runs one echo; `None` (with a note) when this machine has no way to ping at all.
async fn echo_or_skip(pinger: &Pinger, target: &str, payload: usize) -> Option<ping::PingReply> {
    let target: IpAddr = target.parse().unwrap();
    match pinger
        .ping_once(target, Duration::from_secs(2), payload)
        .await
    {
        Ok(reply) => Some(reply),
        Err(ProbeError::Unsupported(reason)) => {
            eprintln!("skipping: {reason}");
            None
        }
        Err(error) => panic!("ping {target} with {:?} failed: {error}", pinger.method()),
    }
}

#[tokio::test]
async fn detected_method_reaches_ipv4_loopback() {
    let pinger = Pinger::new();
    eprintln!("detected ping method: {:?}", pinger.method());
    let Some(reply) = echo_or_skip(&pinger, LOCALHOST, 56).await else {
        return;
    };
    assert_eq!(reply.bytes, 64, "8 header bytes + 56 data bytes");
    assert!(reply.rtt_ms >= 0.0 && reply.rtt_ms < 1000.0, "{reply:?}");
    if let Some(ttl) = reply.ttl {
        assert!(
            ttl >= 32,
            "loopback replies start with the system default TTL, got {ttl}"
        );
    }
}

#[tokio::test]
async fn detected_method_reaches_ipv6_loopback() {
    if !ipv6_available() {
        eprintln!("skipping: this machine has no IPv6");
        return;
    }
    let pinger = Pinger::new();
    let Some(reply) = echo_or_skip(&pinger, "::1", 56).await else {
        return;
    };
    assert_eq!(reply.bytes, 64);
    assert!(reply.rtt_ms < 1000.0, "{reply:?}");
}

#[tokio::test]
async fn ipv6_without_ipv6_is_reported_as_unsupported() {
    if ipv6_available() {
        return;
    }
    let pinger = Pinger::new();
    if pinger.method() == Method::SystemCommand {
        return;
    }
    let error = pinger
        .ping_once("::1".parse().unwrap(), Duration::from_secs(1), 56)
        .await
        .unwrap_err();
    assert!(
        matches!(&error, ProbeError::Unsupported(reason) if reason.contains("IPv6")),
        "{error:?}"
    );
}

#[tokio::test]
async fn forced_system_command_reaches_loopback() {
    let pinger = Pinger::with_method(Method::SystemCommand);
    assert_eq!(pinger.method(), Method::SystemCommand);
    let Some(reply) = echo_or_skip(&pinger, LOCALHOST, 56).await else {
        return;
    };
    assert_eq!(reply.bytes, 64);
    assert!(reply.rtt_ms >= 0.0 && reply.rtt_ms < 1000.0, "{reply:?}");
    assert!(
        reply.ttl.is_some(),
        "every system ping prints the TTL of an IPv4 reply"
    );
}

#[tokio::test]
async fn payload_sizes_are_echoed_back() {
    let pinger = Pinger::new();
    for payload in [0usize, 1, 7, 8, 9, 56, 1400, 8000] {
        let Some(reply) = echo_or_skip(&pinger, LOCALHOST, payload).await else {
            return;
        };
        assert_eq!(reply.bytes, payload + 8, "payload {payload}");
    }
}

#[tokio::test]
async fn many_concurrent_echoes_each_get_their_own_reply() {
    let pinger = Pinger::new();
    if echo_or_skip(&pinger, LOCALHOST, 56).await.is_none() {
        return;
    }
    // Different payload sizes make a reply handed to the wrong echo visible.
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..200usize {
        let pinger = pinger.clone();
        tasks.spawn(async move {
            let payload = 16 + index;
            let reply = pinger
                .ping_once(LOCALHOST.parse().unwrap(), Duration::from_secs(5), payload)
                .await;
            (payload, reply)
        });
    }
    let mut answered = 0;
    while let Some(result) = tasks.join_next().await {
        let (payload, reply) = result.unwrap();
        let reply = reply.unwrap_or_else(|error| panic!("echo with payload {payload}: {error}"));
        assert_eq!(reply.bytes, payload + 8);
        answered += 1;
    }
    assert_eq!(answered, 200);
}

#[tokio::test]
async fn ping_reports_statistics_and_streams_progress() {
    let pinger = Pinger::new();
    if echo_or_skip(&pinger, LOCALHOST, 56).await.is_none() {
        return;
    }
    let options = PingOptions {
        count: 5,
        interval_ms: 20,
        timeout_ms: 2000,
        payload: 32,
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<PingEcho>();
    let report = ping::ping(&pinger, "localhost", &options, Some(sender))
        .await
        .unwrap();

    assert_eq!(report.target, "localhost");
    assert!(
        report.address == "127.0.0.1" || report.address == "::1",
        "{}",
        report.address
    );
    assert_eq!(report.method, pinger.method());
    assert_eq!((report.sent, report.received), (5, 5));
    assert_eq!(report.loss_pct, 0.0);
    let (min, avg, max) = (
        report.min_ms.unwrap(),
        report.avg_ms.unwrap(),
        report.max_ms.unwrap(),
    );
    assert!(min <= avg && avg <= max, "{min} {avg} {max}");
    assert!(report.stddev_ms.unwrap() >= 0.0);
    let sequence: Vec<u32> = report.echoes.iter().map(|echo| echo.seq).collect();
    assert_eq!(sequence, [1, 2, 3, 4, 5]);
    assert!(
        report
            .echoes
            .iter()
            .all(|echo| echo.rtt_ms.is_some() && echo.error.is_none())
    );

    let mut streamed = Vec::new();
    while let Ok(echo) = receiver.try_recv() {
        streamed.push(echo);
    }
    streamed.sort_by_key(|echo| echo.seq);
    assert_eq!(streamed, report.echoes);

    // The report is what the UI receives as JSON.
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["sent"], 5);
    assert!(json["method"].is_string());
    assert_eq!(json["echoes"][0]["seq"], 1);
}

#[tokio::test]
async fn echoes_are_spaced_by_the_interval() {
    let pinger = Pinger::new();
    if echo_or_skip(&pinger, LOCALHOST, 56).await.is_none() {
        return;
    }
    let options = PingOptions {
        count: 4,
        interval_ms: 150,
        timeout_ms: 1000,
        payload: 8,
    };
    let started = Instant::now();
    let report = ping::ping(&pinger, LOCALHOST, &options, None)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert_eq!(report.received, 4);
    // Three intervals between four echoes; the run does not wait a fourth one at the end.
    assert!(elapsed >= Duration::from_millis(440), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
}

#[tokio::test]
async fn silent_target_times_out_within_the_timeout() {
    let pinger = Pinger::new();
    if echo_or_skip(&pinger, LOCALHOST, 56).await.is_none() {
        return;
    }
    // 198.51.100.0/24 (TEST-NET-2) is never assigned; depending on the network the echo is
    // dropped silently or a router reports it unreachable.
    let started = Instant::now();
    let result = pinger
        .ping_once(
            "198.51.100.201".parse().unwrap(),
            Duration::from_millis(400),
            56,
        )
        .await;
    let elapsed = started.elapsed();
    match result {
        Err(ProbeError::Timeout) => {
            assert!(elapsed >= Duration::from_millis(390), "{elapsed:?}");
        }
        Err(ProbeError::Unreachable(reason)) => eprintln!("unreachable: {reason}"),
        other => panic!("expected a timeout or unreachable, got {other:?}"),
    }
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");

    let report = ping::ping(
        &pinger,
        "198.51.100.201",
        &PingOptions {
            count: 2,
            interval_ms: 10,
            timeout_ms: 300,
            payload: 56,
        },
        None,
    )
    .await
    .unwrap();
    assert_eq!((report.sent, report.received), (2, 0));
    assert_eq!(report.loss_pct, 100.0);
    assert_eq!(report.min_ms, None);
    assert!(
        report
            .echoes
            .iter()
            .all(|echo| echo.error.is_some() && echo.rtt_ms.is_none())
    );
}

#[tokio::test]
async fn unresolvable_host_is_a_resolve_error() {
    let pinger = Pinger::new();
    let error = ping::ping(
        &pinger,
        "no-such-host.invalid",
        &PingOptions::default(),
        None,
    )
    .await
    .unwrap_err();
    assert!(matches!(error, ProbeError::Resolve), "{error:?}");
}

#[tokio::test]
async fn dropping_a_ping_in_flight_releases_everything() {
    let pinger = Pinger::new();
    if echo_or_skip(&pinger, LOCALHOST, 56).await.is_none() {
        return;
    }
    // Start long pings towards a silent address and abandon them.
    for _ in 0..50 {
        let pending = pinger.ping_once(
            "198.51.100.202".parse().unwrap(),
            Duration::from_secs(30),
            56,
        );
        let _ = tokio::time::timeout(Duration::from_millis(5), pending).await;
    }
    // The pinger still works and is not clogged by the abandoned echoes.
    let reply = pinger
        .ping_once(LOCALHOST.parse().unwrap(), Duration::from_secs(2), 56)
        .await
        .unwrap();
    assert_eq!(reply.bytes, 64);
}
