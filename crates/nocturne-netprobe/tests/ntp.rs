//! SNTP against a real NTP server: a throw-away `chronyd` on loopback, and a public server
//! when the network allows it.

mod support;

use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nocturne_netprobe::{ProbeError, ntp};

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[cfg(unix)]
#[tokio::test]
async fn interoperates_with_chronyd() {
    let Some(chronyd) = support::require_program("chronyd") else {
        return;
    };
    let port = support::free_udp_port();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("chrony.conf");
    std::fs::write(
        &config,
        format!(
            "port {port}\nbindaddress 127.0.0.1\ncmdport 0\nlocal stratum 8\nallow 127.0.0.1\n\
             driftfile {dir}/drift\npidfile {dir}/chronyd.pid\n",
            dir = dir.path().display()
        ),
    )
    .unwrap();
    // -x: never touch the system clock; -n: stay in the foreground; -U: allow non-root.
    let mut command = Command::new(chronyd);
    command.args(["-n", "-x", "-U", "-f"]).arg(&config);
    if support::is_root() {
        command.args(["-u", "root"]);
    }
    let _server = support::Helper::spawn(&mut command).unwrap();

    let server = format!("127.0.0.1:{port}");
    let mut report = None;
    for _ in 0..40 {
        match ntp::query(&server, Duration::from_millis(500), unix_now()).await {
            Ok(answer) => {
                report = Some(answer);
                break;
            }
            // Not listening yet.
            Err(ProbeError::Refused | ProbeError::Timeout) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => panic!("query failed: {error}"),
        }
    }
    let Some(report) = report else {
        eprintln!("skipping: chronyd did not start serving on {server}");
        return;
    };
    assert_eq!(report.address, server);
    assert_eq!(report.stratum, 8, "{report:?}");
    assert_eq!(report.version, 4);
    assert_eq!(report.leap, "none");
    // chrony's local reference clock.
    assert_eq!(report.reference, "127.127.1.1");
    // The server reads the same clock as this test: the offset is measurement noise.
    assert!(
        report.offset_ms.abs() < 50.0,
        "offset {} ms",
        report.offset_ms
    );
    assert!(report.delay_ms < 50.0, "delay {} ms", report.delay_ms);
    assert!(
        (report.server_time_unix - unix_now()).abs() < 5.0,
        "{report:?}"
    );
    assert_eq!(report.root_delay_ms, 0.0);
}

#[tokio::test]
async fn public_time_server_if_the_network_allows_it() {
    // Outbound NTP is often filtered; that is a skip, not a failure.
    let report = match ntp::query("pool.ntp.org", Duration::from_secs(3), unix_now()).await {
        Ok(report) => report,
        Err(
            error @ (ProbeError::Timeout
            | ProbeError::Resolve
            | ProbeError::Refused
            | ProbeError::Unreachable(_)),
        ) => {
            eprintln!("skipping: no path to a public NTP server ({error})");
            return;
        }
        Err(error) => panic!("unexpected failure: {error}"),
    };
    eprintln!("pool.ntp.org: {report:?}");
    assert!((1..=15).contains(&report.stratum), "{report:?}");
    assert!((3..=4).contains(&report.version));
    assert!(report.address.ends_with(":123"));
    // Public servers are within a few seconds of any sanely configured machine.
    assert!(
        (report.server_time_unix - unix_now()).abs() < 3600.0,
        "{report:?}"
    );
}
