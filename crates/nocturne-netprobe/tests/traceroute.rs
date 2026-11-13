//! Traceroute against loopback with whatever method this platform uses.

use std::time::{Duration, Instant};

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::traceroute::{self, Hop, TraceOptions};

/// Runs a trace; `None` (with a note) when this machine has no way to trace at all.
async fn trace_or_skip(
    host: &str,
    options: &TraceOptions,
    progress: Option<tokio::sync::mpsc::UnboundedSender<Hop>>,
) -> Option<traceroute::TraceReport> {
    match traceroute::traceroute(host, options, progress).await {
        Ok(report) => Some(report),
        Err(ProbeError::Unsupported(reason) | ProbeError::Permission(reason)) => {
            eprintln!("skipping: {reason}");
            None
        }
        Err(error) => panic!("traceroute {host} failed: {error}"),
    }
}

#[tokio::test]
async fn loopback_is_one_hop_away() {
    let options = TraceOptions {
        max_hops: 5,
        probes: 3,
        timeout_ms: 1000,
        resolve_names: false,
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let Some(report) = trace_or_skip("127.0.0.1", &options, Some(sender)).await else {
        return;
    };
    eprintln!("traceroute method: {}", report.method);
    assert_eq!(report.target, "127.0.0.1");
    assert_eq!(report.address, "127.0.0.1");
    assert!(report.reached);
    assert!(
        ["native-udp", "system-traceroute", "system-tracert"].contains(&report.method.as_str()),
        "{}",
        report.method
    );
    assert_eq!(report.hops.len(), 1, "{:?}", report.hops);
    let hop = &report.hops[0];
    assert_eq!(hop.ttl, 1);
    assert_eq!(hop.address.as_deref(), Some("127.0.0.1"));
    assert_eq!(hop.rtts_ms.len(), 3);
    assert!(hop.rtts_ms.iter().flatten().count() >= 1, "{hop:?}");
    assert!(
        hop.rtts_ms
            .iter()
            .flatten()
            .all(|rtt| *rtt >= 0.0 && *rtt < 1000.0),
        "{hop:?}"
    );
    assert_eq!(hop.note, "");

    assert_eq!(receiver.try_recv().unwrap(), *hop);
    assert!(receiver.try_recv().is_err(), "each hop is streamed once");

    if cfg!(target_os = "linux") {
        assert_eq!(report.method, "native-udp");
    }
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["reached"], true);
    assert_eq!(json["hops"][0]["ttl"], 1);
}

#[tokio::test]
async fn names_are_resolved_on_request() {
    let options = TraceOptions {
        max_hops: 3,
        probes: 1,
        timeout_ms: 1000,
        resolve_names: true,
    };
    let Some(report) = trace_or_skip("localhost", &options, None).await else {
        return;
    };
    assert!(report.reached);
    assert_eq!(report.target, "localhost");
    let hosts = std::fs::read_to_string("/etc/hosts").unwrap_or_default();
    if report.address == "127.0.0.1"
        && hosts
            .lines()
            .any(|line| line.trim_start().starts_with("127.0.0.1"))
    {
        assert!(report.hops[0].hostname.is_some(), "{:?}", report.hops[0]);
    }
}

#[tokio::test]
async fn an_unanswered_trace_stops_at_max_hops_within_its_time_budget() {
    // TEST-NET-2 is never assigned. Whatever the network does with the probes (drop them,
    // or let a router report the destination unreachable), the trace ends without reaching it.
    let options = TraceOptions {
        max_hops: 4,
        probes: 2,
        timeout_ms: 300,
        resolve_names: false,
    };
    let started = Instant::now();
    let Some(report) = trace_or_skip("198.51.100.203", &options, None).await else {
        return;
    };
    let elapsed = started.elapsed();
    assert!(!report.reached, "{report:?}");
    assert!(
        !report.hops.is_empty() && report.hops.len() <= 4,
        "{:?}",
        report.hops
    );
    let ttls: Vec<u8> = report.hops.iter().map(|hop| hop.ttl).collect();
    assert_eq!(
        ttls,
        (1..=report.hops.len() as u8).collect::<Vec<_>>(),
        "in TTL order, no gaps"
    );
    for hop in &report.hops {
        assert_eq!(hop.rtts_ms.len(), 2, "{hop:?}");
        if hop.address.is_none() {
            assert_eq!(hop.note, "no reply");
        }
    }
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
}

#[tokio::test]
async fn dropping_a_trace_stops_it() {
    let options = TraceOptions {
        max_hops: 30,
        probes: 3,
        timeout_ms: 5000,
        resolve_names: false,
    };
    let trace = traceroute::traceroute("198.51.100.204", &options, None);
    let started = Instant::now();
    let _ = tokio::time::timeout(Duration::from_millis(200), trace).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    // Nothing of the abandoned trace lingers: a new one works right away.
    let quick = TraceOptions {
        max_hops: 2,
        probes: 1,
        timeout_ms: 500,
        resolve_names: false,
    };
    if let Some(report) = trace_or_skip("127.0.0.1", &quick, None).await {
        assert!(report.reached);
    }
}
