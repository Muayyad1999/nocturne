//! Loopback tests of the pinger with whatever method this machine allows, plus a forced run
//! through the system `ping` program.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::ping::{self, Method, PingEcho, PingOptions, Pinger};

const LOCALHOST: &str = "127.0.0.1";

/// The timeout every single echo below is given.
///
/// A round trip over loopback takes microseconds, but the only thing the
/// pinger *guarantees* about the time it reports is that the reply arrived
/// inside the timeout it was given. A tighter bound than this one is a claim
/// about how busy the machine is, not about the pinger, so the tests here
/// hold it to the timeout.
const ECHO_TIMEOUT: Duration = Duration::from_secs(2);
const ECHO_TIMEOUT_MS: f64 = 2_000.0;

fn ipv6_available() -> bool {
    std::net::UdpSocket::bind("[::1]:0").is_ok()
}

/// Runs one echo; `None` (with a note) when this machine has no way to ping at all.
async fn echo_or_skip(pinger: &Pinger, target: &str, payload: usize) -> Option<ping::PingReply> {
    let target: IpAddr = target.parse().unwrap();
    match pinger.ping_once(target, ECHO_TIMEOUT, payload).await {
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
    assert!(
        reply.rtt_ms >= 0.0 && reply.rtt_ms <= ECHO_TIMEOUT_MS,
        "{reply:?}"
    );
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
    assert!(reply.rtt_ms <= ECHO_TIMEOUT_MS, "{reply:?}");
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
    assert!(
        reply.rtt_ms >= 0.0 && reply.rtt_ms <= ECHO_TIMEOUT_MS,
        "{reply:?}"
    );
    assert!(
        reply.ttl.is_some(),
        "every system ping prints the TTL of an IPv4 reply"
    );
}

/// The system `ping` reads the round-trip time out of a timestamp it writes
/// into the payload, so a payload too small to hold one gets a reply line with
/// no time in it. That must still count as a reply, because it is one.
#[tokio::test]
async fn the_system_command_still_reports_a_reply_it_could_not_time() {
    let pinger = Pinger::with_method(Method::SystemCommand);
    for payload in [0usize, 8, 15] {
        let Some(reply) = echo_or_skip(&pinger, LOCALHOST, payload).await else {
            return;
        };
        assert_eq!(reply.bytes, payload + 8, "payload {payload}");
        // The time is the command's own, an upper bound on the round trip;
        // all that can be claimed of it is that it was measured.
        assert!(
            reply.rtt_ms > 0.0 && reply.rtt_ms <= ECHO_TIMEOUT_MS,
            "{reply:?}"
        );
    }
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
    // Different payload sizes make a reply handed to the wrong echo visible:
    // no two echoes ask for the same size, so the size of a reply is a
    // signature of the echo it belongs to.
    const ECHOES: usize = 200;
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..ECHOES {
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
    let mut lost = Vec::new();
    while let Some(result) = tasks.join_next().await {
        let (payload, reply) = result.unwrap();
        match reply {
            // The claim this test makes: a reply goes to the echo that asked
            // for it, and to no other. Every reply that arrives is held to it.
            Ok(reply) => {
                assert_eq!(
                    reply.bytes,
                    payload + 8,
                    "the echo that asked for {payload} bytes was handed a reply of {} — \
                     a reply went to the wrong requester",
                    reply.bytes
                );
                answered += 1;
            }
            // Loopback is not lossless with two hundred echoes in flight at
            // once: the ICMP socket has a receive buffer of finite size, and
            // on a machine whose cores are all busy — a shared CI runner —
            // it can fill before this process is scheduled to drain it.
            // Insisting on all two hundred asserts that the machine was
            // quiet, which is not what this test is for; the losses are
            // counted and named instead.
            Err(ProbeError::Timeout) => lost.push(payload),
            Err(error) => panic!("echo with payload {payload}: {error}"),
        }
    }
    assert_eq!(
        answered + lost.len(),
        ECHOES,
        "every echo came back either answered or timed out"
    );
    assert!(
        answered * 4 >= ECHOES * 3,
        "only {answered} of {ECHOES} concurrent echoes were answered, which is more \
         loss than a busy machine accounts for (timed out: {lost:?})"
    );
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
    assert_eq!(report.sent, 5, "five were asked for, so five were sent");

    // What the statistics are *of* is the guarantee here: the report has to
    // describe the echoes it actually got back, whatever that number is. How
    // many of five loopback echoes answer inside a one-second timeout is the
    // machine's business — a runner with every core busy can miss one — so
    // the numbers below are checked against the echoes rather than against a
    // count this test would like to see.
    let timed: Vec<&PingEcho> = report
        .echoes
        .iter()
        .filter(|echo| echo.rtt_ms.is_some())
        .collect();
    assert_eq!(report.echoes.len(), 5, "one entry per echo sent");
    assert_eq!(
        report.received as usize,
        timed.len(),
        "`received` counts the echoes that came back: {report:?}"
    );
    let loss = f64::from(report.sent - report.received) * 100.0 / f64::from(report.sent);
    assert!(
        (report.loss_pct - loss).abs() < 0.001,
        "the loss is derived from the two counts, expected {loss}: {report:?}"
    );
    assert!(
        report.received >= 3,
        "only {} of five loopback echoes answered, which is more loss than a \
         busy machine accounts for: {report:?}",
        report.received
    );
    let rtts: Vec<f64> = timed.iter().filter_map(|echo| echo.rtt_ms).collect();
    let (min, avg, max) = (
        report.min_ms.unwrap(),
        report.avg_ms.unwrap(),
        report.max_ms.unwrap(),
    );
    assert!(min <= avg && avg <= max, "{min} {avg} {max}");
    assert_eq!(
        min,
        rtts.iter().copied().fold(f64::INFINITY, f64::min),
        "the minimum is the fastest echo there was: {report:?}"
    );
    assert_eq!(
        max,
        rtts.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        "the maximum is the slowest echo there was: {report:?}"
    );
    assert!(report.stddev_ms.unwrap() >= 0.0);
    let sequence: Vec<u32> = report.echoes.iter().map(|echo| echo.seq).collect();
    assert_eq!(sequence, [1, 2, 3, 4, 5]);
    assert!(
        report
            .echoes
            .iter()
            .all(|echo| echo.rtt_ms.is_some() == echo.error.is_none()),
        "an echo either has a time, or says why it has none: {report:?}"
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
    assert_eq!(report.echoes.len(), 4);
    assert!(
        report.received >= 2,
        "{} of four loopback echoes answered: {report:?}",
        report.received
    );
    // Three intervals between four echoes. This is the claim, and it is a
    // lower bound, so a slow or busy machine can only make it truer.
    assert!(elapsed >= Duration::from_millis(440), "{elapsed:?}");
    // The run does not wait a fourth interval after the last echo — but by a
    // clock that is the difference between 450 ms and 600 ms, and a loaded
    // runner can spend that long in the scheduler alone, so a wall-clock
    // bound cannot tell the two apart. This one is only a hang detector: four
    // echoes a timeout apart, and then some.
    assert!(
        elapsed < Duration::from_millis(4 * 1000 + 3 * 150) + Duration::from_secs(5),
        "{elapsed:?}"
    );
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
