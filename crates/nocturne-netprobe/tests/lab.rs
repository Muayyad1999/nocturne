//! Tests that need real routers between this machine and its targets: ICMP errors from routers,
//! multi-hop traceroute, hosts that ignore ping. They run inside the network-namespace topology
//! built by `tests/lab/run.sh` (see that script for the addresses) and skip everywhere else.

use std::net::IpAddr;
use std::time::Duration;

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::ping::{Method, Pinger};
use nocturne_netprobe::sweep::{self, SweepOptions};
use nocturne_netprobe::traceroute::{self, TraceOptions};

const TARGET: &str = "10.201.3.10";
const R1: &str = "10.201.1.254";
const R1_SECOND: &str = "10.201.1.50";
const R2: &str = "10.201.2.254";

fn in_lab() -> bool {
    let inside = std::env::var_os("NETPROBE_LAB").is_some();
    if !inside {
        eprintln!("skipping: not inside the lab (run tests/lab/run.sh)");
    }
    inside
}

fn all_methods() -> [Pinger; 3] {
    [
        Pinger::with_method(Method::IcmpDatagram),
        Pinger::with_method(Method::IcmpRaw),
        Pinger::with_method(Method::SystemCommand),
    ]
}

async fn echo(
    pinger: &Pinger,
    target: &str,
    timeout_ms: u64,
) -> Result<nocturne_netprobe::ping::PingReply, ProbeError> {
    let target: IpAddr = target.parse().unwrap();
    pinger
        .ping_once(target, Duration::from_millis(timeout_ms), 56)
        .await
}

#[tokio::test]
async fn the_lab_allows_unprivileged_ping_sockets() {
    if !in_lab() {
        return;
    }
    assert_eq!(Pinger::new().method(), Method::IcmpDatagram);
}

#[tokio::test]
async fn every_method_pings_through_two_routers() {
    if !in_lab() {
        return;
    }
    for pinger in all_methods() {
        let reply = echo(&pinger, TARGET, 2000)
            .await
            .unwrap_or_else(|error| panic!("{:?}: {error}", pinger.method()));
        assert_eq!(reply.bytes, 64, "{:?}", pinger.method());
        // The target answers with TTL 64 and each router takes one off.
        assert_eq!(reply.ttl, Some(62), "{:?}", pinger.method());
        assert!(reply.rtt_ms < 500.0, "{reply:?}");
    }
}

#[tokio::test]
async fn a_router_without_a_route_reports_network_unreachable() {
    if !in_lab() {
        return;
    }
    for pinger in all_methods() {
        let error = echo(&pinger, "10.201.9.9", 2000).await.unwrap_err();
        let ProbeError::Unreachable(reason) = &error else {
            panic!("{:?}: expected unreachable, got {error:?}", pinger.method());
        };
        let reason = reason.to_lowercase();
        assert!(
            reason.contains("net") && reason.contains("unreachable"),
            "{:?}: {reason}",
            pinger.method()
        );
        assert!(
            reason.contains(&format!("reported by {R1}")),
            "{:?}: {reason}",
            pinger.method()
        );
    }
}

#[tokio::test]
async fn a_filtering_router_reports_administratively_prohibited() {
    if !in_lab() {
        return;
    }
    for pinger in all_methods() {
        let error = echo(&pinger, "10.201.66.1", 2000).await.unwrap_err();
        let ProbeError::Unreachable(reason) = &error else {
            panic!("{:?}: expected unreachable, got {error:?}", pinger.method());
        };
        let reason = reason.to_lowercase();
        assert!(
            reason.contains("prohibited") || reason.contains("filtered"),
            "{:?}: {reason}",
            pinger.method()
        );
        assert!(
            reason.contains(&format!("reported by {R2}")),
            "{:?}: {reason}",
            pinger.method()
        );
    }
}

#[tokio::test]
async fn a_missing_host_on_a_remote_lan_is_reported_by_its_router() {
    if !in_lab() {
        return;
    }
    // r2 gives up on ARP after about three seconds and then says so.
    for pinger in all_methods() {
        let error = echo(&pinger, "10.201.3.77", 6000).await.unwrap_err();
        let ProbeError::Unreachable(reason) = &error else {
            panic!("{:?}: expected unreachable, got {error:?}", pinger.method());
        };
        let reason = reason.to_lowercase();
        assert!(
            reason.contains("host unreachable"),
            "{:?}: {reason}",
            pinger.method()
        );
        assert!(
            reason.contains(&format!("reported by {R2}")),
            "{:?}: {reason}",
            pinger.method()
        );
    }
}

#[tokio::test]
async fn a_host_that_ignores_echo_requests_times_out() {
    if !in_lab() {
        return;
    }
    for pinger in all_methods() {
        let error = echo(&pinger, R1, 500).await.unwrap_err();
        assert!(
            matches!(error, ProbeError::Timeout),
            "{:?}: {error:?}",
            pinger.method()
        );
    }
}

#[tokio::test]
async fn concurrent_echoes_to_reachable_and_unreachable_targets_do_not_mix() {
    if !in_lab() {
        return;
    }
    for pinger in [
        Pinger::with_method(Method::IcmpDatagram),
        Pinger::with_method(Method::IcmpRaw),
    ] {
        // Linux routers send at most five route errors in a burst, then one per second (a
        // global setting the lab cannot change): let the routers recover from earlier tests
        // and ask for only two errors each.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut tasks = tokio::task::JoinSet::new();
        for index in 0..64u32 {
            let pinger = pinger.clone();
            tasks.spawn(async move {
                let target = match index {
                    10 | 40 => "10.201.9.9",
                    20 | 50 => "10.201.66.1",
                    _ => TARGET,
                };
                (target, echo(&pinger, target, 3000).await)
            });
        }
        let mut replies = 0;
        while let Some(joined) = tasks.join_next().await {
            let (target, result) = joined.unwrap();
            match (target, result) {
                (TARGET, Ok(reply)) => {
                    assert_eq!(reply.ttl, Some(62));
                    replies += 1;
                }
                ("10.201.9.9", Err(ProbeError::Unreachable(reason))) => {
                    assert_eq!(reason, format!("network unreachable (reported by {R1})"));
                }
                ("10.201.66.1", Err(ProbeError::Unreachable(reason))) => {
                    assert!(
                        reason.contains("prohibited") && reason.contains(R2),
                        "{reason}"
                    );
                }
                (target, other) => panic!("{:?} to {target}: {other:?}", pinger.method()),
            }
        }
        assert_eq!(replies, 60, "{:?}", pinger.method());
    }
}

fn trace_options(timeout_ms: u64) -> TraceOptions {
    TraceOptions {
        max_hops: 8,
        probes: 3,
        timeout_ms,
        resolve_names: false,
    }
}

#[tokio::test]
async fn traceroute_lists_both_routers_and_the_target() {
    if !in_lab() {
        return;
    }
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let report = traceroute::traceroute(TARGET, &trace_options(1500), Some(sender))
        .await
        .unwrap();
    assert_eq!(report.method, "native-udp");
    assert!(report.reached);
    let path: Vec<Option<&str>> = report
        .hops
        .iter()
        .map(|hop| hop.address.as_deref())
        .collect();
    assert_eq!(path, [Some(R1), Some(R2), Some(TARGET)]);
    for (index, hop) in report.hops.iter().enumerate() {
        assert_eq!(usize::from(hop.ttl), index + 1);
        assert_eq!(
            hop.rtts_ms.iter().flatten().count(),
            3,
            "every probe is answered: {hop:?}"
        );
        assert_eq!(hop.note, "");
    }
    let mut streamed = Vec::new();
    while let Ok(hop) = receiver.try_recv() {
        streamed.push(hop);
    }
    assert_eq!(streamed, report.hops, "streamed in order, once each");
}

#[tokio::test]
async fn traceroute_ends_where_a_router_reports_the_destination_unreachable() {
    if !in_lab() {
        return;
    }
    let no_route = traceroute::traceroute("10.201.9.9", &trace_options(1500), None)
        .await
        .unwrap();
    assert!(!no_route.reached);
    assert_eq!(no_route.hops.len(), 1, "{:?}", no_route.hops);
    assert_eq!(no_route.hops[0].address.as_deref(), Some(R1));
    assert_eq!(no_route.hops[0].note, "network unreachable");

    let filtered = traceroute::traceroute("10.201.66.1", &trace_options(1500), None)
        .await
        .unwrap();
    assert!(!filtered.reached);
    let path: Vec<Option<&str>> = filtered
        .hops
        .iter()
        .map(|hop| hop.address.as_deref())
        .collect();
    assert_eq!(path, [Some(R1), Some(R2)]);
    assert_eq!(filtered.hops[0].note, "");
    assert_eq!(
        filtered.hops[1].note,
        "communication administratively prohibited (filtered)"
    );

    // r2 needs about three seconds to give up on ARP for a host that does not exist.
    let no_host = traceroute::traceroute("10.201.3.77", &trace_options(6000), None)
        .await
        .unwrap();
    assert!(!no_host.reached);
    let path: Vec<Option<&str>> = no_host
        .hops
        .iter()
        .map(|hop| hop.address.as_deref())
        .collect();
    assert_eq!(path, [Some(R1), Some(R2), Some(R2)]);
    assert_eq!(no_host.hops[2].note, "host unreachable");
}

#[tokio::test]
async fn sweep_finds_hosts_by_icmp_tcp_and_arp() {
    if !in_lab() {
        return;
    }
    let pinger = Pinger::new();
    let options = SweepOptions {
        timeout_ms: 500,
        ..SweepOptions::default()
    };
    let hosts = sweep::sweep(&pinger, "10.201.1.0/24", &options, None)
        .await
        .unwrap();
    let summary: Vec<(&str, &str, &[u16])> = hosts
        .iter()
        .map(|host| {
            (
                host.address.as_str(),
                host.via.as_str(),
                host.open_ports.as_slice(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            // This machine answers its own echo.
            ("10.201.1.1", "icmp", &[][..]),
            // r1 ignores echo requests; its open Winbox port gives it away.
            (R1_SECOND, "tcp/8291", &[8291][..]),
            // Nothing is open here, but a TCP reset is an answer too.
            (R1, "tcp/22", &[][..]),
        ]
    );
    assert_eq!(hosts[0].mac, None, "no neighbour entry for our own address");
    assert!(
        hosts[1].mac.is_some() && hosts[1].mac == hosts[2].mac,
        "{hosts:?}"
    );

    // Without TCP probes only the neighbour table can tell that r1 is there — including on
    // this second sweep, when its entries are no longer new.
    let icmp_only = SweepOptions {
        timeout_ms: 500,
        tcp_fallback_ports: Vec::new(),
        ..SweepOptions::default()
    };
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let hosts = sweep::sweep(&pinger, "10.201.1.0/24", &icmp_only, Some(sender))
        .await
        .unwrap();
    let summary: Vec<(&str, &str)> = hosts
        .iter()
        .map(|host| (host.address.as_str(), host.via.as_str()))
        .collect();
    assert_eq!(
        summary,
        [("10.201.1.1", "icmp"), (R1_SECOND, "arp"), (R1, "arp")]
    );
    assert!(hosts[1].mac.is_some());
    let mut streamed = 0;
    while receiver.try_recv().is_ok() {
        streamed += 1;
    }
    assert_eq!(streamed, 3);
}

#[tokio::test]
async fn sweep_reaches_routed_networks() {
    if !in_lab() {
        return;
    }
    let pinger = Pinger::new();
    let options = SweepOptions {
        timeout_ms: 500,
        retries: 0,
        tcp_fallback_ports: Vec::new(),
        ..SweepOptions::default()
    };
    let hosts = sweep::sweep(
        &pinger,
        "10.201.3.1-20, 10.201.2.250-10.201.2.254",
        &options,
        None,
    )
    .await
    .unwrap();
    let addresses: Vec<&str> = hosts.iter().map(|host| host.address.as_str()).collect();
    // r1 ignores echo and is not on these networks' neighbour tables; r2 and the target answer.
    assert_eq!(addresses, [R2, TARGET]);
    assert!(
        hosts
            .iter()
            .all(|host| host.via == "icmp" && host.mac.is_none())
    );
}
