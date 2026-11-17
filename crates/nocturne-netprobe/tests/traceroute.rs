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

    // These hold whatever the network does with the probes.
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
        // How many probes a hop reports is not this test's business, and
        // "both of them came back" is a statement about the network, not
        // about the tracer. Two things are the tracer's: the native tracer
        // sends exactly as many probes as it was asked for and accounts for
        // every one of them (`None` for a probe nothing answered), and a
        // hop parsed out of the system program reports whatever that program
        // sent — Windows `tracert` always sends three, whatever `probes`
        // says, which is why asserting two here failed there.
        assert!(
            !hop.rtts_ms.is_empty(),
            "a hop accounts for its probes: {hop:?}"
        );
        if report.method == "native-udp" {
            assert_eq!(
                hop.rtts_ms.len(),
                usize::from(options.probes),
                "the native tracer sends what it was asked for: {hop:?}"
            );
        }
        // What did arrive is well formed: a time inside the hop's own budget.
        assert!(
            hop.rtts_ms
                .iter()
                .flatten()
                .all(|rtt| *rtt >= 0.0 && *rtt <= options.timeout_ms as f64),
            "{hop:?}"
        );
        // A hop that nothing answered says so, rather than looking like an
        // ordinary hop with no address.
        if hop.address.is_none() {
            assert_eq!(hop.note, "no reply");
            assert!(
                hop.rtts_ms.iter().all(Option::is_none),
                "a hop with no responder has no times: {hop:?}"
            );
        } else {
            assert!(
                hop.rtts_ms.iter().any(Option::is_some),
                "a hop with a responder has at least one time: {hop:?}"
            );
        }
    }
    // Four hops, two probes each, 300 ms apiece is a second and a bit when
    // the tracer is strictly sequential and nothing answers. The bound has to
    // be wider than the patience the system path allows itself, which is
    // `(timeout_ms + 1000) * max_hops * max(probes, 3) + 5 s` — 20.6 s for
    // these options. The old bound of 20 s was *inside* that, so the test
    // could fail for a tracer that was behaving exactly as designed. This one
    // only catches a tracer that ignores its budget altogether.
    assert!(elapsed < Duration::from_secs(45), "{elapsed:?}");

    // Whether the trace *ends without reaching* depends on the network being
    // honest about an address that is not assigned to anybody. Behind a
    // transparent proxy or a captive gateway something answers for every
    // address, and the trace is right to report that it got there — from a
    // different address than it asked for, which is the note it carries. That
    // is a network this test cannot make its point on, so it says so and
    // stops, rather than failing for something the tracer did correctly.
    if report.reached {
        let answered_for_it = report
            .hops
            .last()
            .is_some_and(|hop| hop.note.contains("answered for the destination"));
        assert!(
            answered_for_it,
            "an address that is assigned to nobody was reached, and nothing \
             says another host answered for it: {report:?}"
        );
        eprintln!(
            "skipping the unreachable-target check: this network answers for \
             addresses that do not exist ({} replied for {})",
            report
                .hops
                .last()
                .and_then(|hop| hop.address.clone())
                .unwrap_or_default(),
            report.address,
        );
    }
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
    // A trace that is not stopped by being dropped runs its 30 hops of 5 s
    // probes — minutes. The bound only has to be below that, so it is set
    // well above anything a busy machine adds to abandoning the future.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
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
