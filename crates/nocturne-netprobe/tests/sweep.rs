//! Host discovery against loopback.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::ping::Pinger;
use nocturne_netprobe::sweep::{self, HostResult, SweepOptions};
use tokio::net::TcpListener;

fn quick() -> SweepOptions {
    SweepOptions {
        timeout_ms: 300,
        retries: 0,
        tcp_fallback_ports: Vec::new(),
        ..SweepOptions::default()
    }
}

/// `false` (with a note) when this machine cannot ping at all, which a sweep needs.
async fn can_ping(pinger: &Pinger) -> bool {
    match pinger
        .ping_once("127.0.0.1".parse().unwrap(), Duration::from_secs(2), 0)
        .await
    {
        Ok(_) => true,
        Err(error) => {
            eprintln!("skipping: {error}");
            false
        }
    }
}

#[tokio::test]
async fn loopback_is_found_by_icmp_and_streamed() {
    let pinger = Pinger::new();
    if !can_ping(&pinger).await {
        return;
    }
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<HostResult>();
    let hosts = sweep::sweep(&pinger, "127.0.0.1", &quick(), Some(sender))
        .await
        .unwrap();
    assert_eq!(hosts.len(), 1);
    let host = &hosts[0];
    assert_eq!(host.address, "127.0.0.1");
    assert!(host.alive);
    assert_eq!(host.via, "icmp");
    assert!(host.rtt_ms.unwrap() < 300.0);
    assert_eq!(host.open_ports, Vec::<u16>::new());
    assert_eq!(host.hostname, None, "names are only resolved on request");

    let streamed = receiver.try_recv().unwrap();
    assert_eq!(streamed.address, "127.0.0.1");
    assert!(receiver.try_recv().is_err(), "each host is streamed once");

    let json = serde_json::to_value(host).unwrap();
    assert_eq!(json["via"], "icmp");
    assert_eq!(json["alive"], true);
}

#[tokio::test]
async fn open_fallback_ports_are_reported() {
    let pinger = Pinger::new();
    if !can_ping(&pinger).await {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let open = listener.local_addr().unwrap().port();
    let closed = {
        let free = TcpListener::bind("127.0.0.1:0").await.unwrap();
        free.local_addr().unwrap().port()
    };
    let options = SweepOptions {
        tcp_fallback_ports: vec![closed, open],
        ..quick()
    };
    let hosts = sweep::sweep(&pinger, "127.0.0.1", &options, None)
        .await
        .unwrap();
    assert_eq!(hosts.len(), 1);
    assert_eq!(hosts[0].via, "icmp");
    assert_eq!(hosts[0].open_ports, vec![open]);
}

#[tokio::test]
async fn ranges_names_and_duplicates() {
    let pinger = Pinger::new();
    if !can_ping(&pinger).await {
        return;
    }
    // Linux answers on all of 127.0.0.0/8; other systems only on 127.0.0.1.
    let hosts = sweep::sweep(
        &pinger,
        "127.0.0.1-127.0.0.6, localhost, 127.0.0.1",
        &quick(),
        None,
    )
    .await
    .unwrap();
    let addresses: Vec<IpAddr> = hosts
        .iter()
        .map(|host| host.address.parse().unwrap())
        .collect();
    assert!(
        addresses.contains(&"127.0.0.1".parse().unwrap()),
        "{addresses:?}"
    );
    let mut sorted = addresses.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(addresses, sorted, "sorted, without duplicates");
    if cfg!(target_os = "linux") {
        assert!(addresses.len() >= 6, "{addresses:?}");
    }
}

#[tokio::test]
async fn names_are_resolved_on_request() {
    let pinger = Pinger::new();
    if !can_ping(&pinger).await {
        return;
    }
    let options = SweepOptions {
        resolve_names: true,
        ..quick()
    };
    let hosts = sweep::sweep(&pinger, "127.0.0.1", &options, None)
        .await
        .unwrap();
    assert_eq!(hosts.len(), 1);
    let hosts_file = std::fs::read_to_string("/etc/hosts").unwrap_or_default();
    if hosts_file
        .lines()
        .any(|line| line.trim_start().starts_with("127.0.0.1"))
    {
        assert!(
            hosts[0].hostname.is_some(),
            "127.0.0.1 has a name in /etc/hosts"
        );
    }
}

#[tokio::test]
async fn silent_addresses_are_not_reported_and_do_not_take_forever() {
    let pinger = Pinger::new();
    if !can_ping(&pinger).await {
        return;
    }
    // 62 addresses of TEST-NET-2, which is never assigned, with a 250 ms timeout and one
    // retry: two rounds of probing, all in parallel.
    let options = SweepOptions {
        timeout_ms: 250,
        retries: 1,
        tcp_fallback_ports: vec![22, 80],
        ..SweepOptions::default()
    };
    let started = Instant::now();
    let hosts = sweep::sweep(&pinger, "198.51.100.64/26", &options, None)
        .await
        .unwrap();
    let elapsed = started.elapsed();
    for host in &hosts {
        // A network that answers for unassigned space (a captive gateway) is not our bug, but
        // whatever is reported must be inside the range.
        let address: std::net::Ipv4Addr = host.address.parse().unwrap();
        assert_eq!(address.octets()[..3], [198, 51, 100], "{host:?}");
    }
    // "Do not take forever" means the sweep probes in parallel rather than
    // address by address: 62 addresses, two echo rounds of 250 ms and then a
    // TCP fallback, is a second or so at the default concurrency of 128 and
    // over half a minute if it were serialised. The bound sits between the
    // two with room on both sides, rather than a whisker above the fast one.
    assert!(elapsed < Duration::from_secs(20), "{elapsed:?}");
}

#[tokio::test]
async fn invalid_requests_are_rejected_before_anything_is_sent() {
    let pinger = Pinger::new();
    let too_many = SweepOptions {
        limit: 100,
        ..quick()
    };
    let error = sweep::sweep(&pinger, "10.0.0.0/24", &too_many, None)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("limit is 100"), "{error}");

    for bad in [
        SweepOptions {
            concurrency: 0,
            ..quick()
        },
        SweepOptions {
            timeout_ms: 0,
            ..quick()
        },
        SweepOptions {
            tcp_fallback_ports: vec![0],
            ..quick()
        },
    ] {
        let error = sweep::sweep(&pinger, "127.0.0.1", &bad, None)
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Invalid(_)), "{error:?}");
    }
    assert!(matches!(
        sweep::sweep(&pinger, "", &quick(), None).await,
        Err(ProbeError::Invalid(_))
    ));
    assert!(matches!(
        sweep::sweep(&pinger, "no-such-host.invalid", &quick(), None).await,
        Err(ProbeError::Resolve)
    ));
}
