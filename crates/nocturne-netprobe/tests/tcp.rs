//! Port scanning against listeners the tests open on loopback, cross-checked with nmap when it
//! is installed.

use std::net::SocketAddr;
use std::process::Command;
use std::time::{Duration, Instant};

use nocturne_netprobe::ProbeError;
use nocturne_netprobe::tcp::{self, PortResult, PortState, ScanOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const LOCALHOST: &str = "127.0.0.1";

/// A listener that greets every connection with `greeting` and then waits for the peer to leave.
async fn greeting_server(greeting: &'static [u8]) -> u16 {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let _ = stream.write_all(greeting).await;
                let mut sink = [0u8; 256];
                while matches!(stream.read(&mut sink).await, Ok(count) if count > 0) {}
            });
        }
    });
    port
}

/// A port that was just free: connecting to it is refused.
async fn closed_port() -> u16 {
    let listener = TcpListener::bind((LOCALHOST, 0)).await.unwrap();
    listener.local_addr().unwrap().port()
}

/// A plain HTTP server on one of the ports the scanner treats as HTTP, if any of them is free.
/// It records what each client sent before answering.
async fn http_server() -> Option<(u16, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>)> {
    let (requests, received) = tokio::sync::mpsc::unbounded_channel();
    for port in [
        8088u16, 8880, 8888, 9090, 8008, 8081, 8000, 9000, 3000, 8080,
    ] {
        let Ok(listener) = TcpListener::bind((LOCALHOST, port)).await else {
            continue;
        };
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let requests = requests.clone();
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0u8; 512];
                    while !request.ends_with(b"\r\n\r\n") {
                        match stream.read(&mut buffer).await {
                            Ok(count) if count > 0 => request.extend_from_slice(&buffer[..count]),
                            _ => break,
                        }
                    }
                    let _ = requests.send(request);
                    let _ = stream
                        .write_all(b"HTTP/1.0 200 OK\r\nServer: NocturneTest/1.0\r\nContent-Length: 0\r\n\r\n")
                        .await;
                });
            }
        });
        return Some((port, received));
    }
    None
}

/// How long a connection to a *closed* port may take to be refused.
///
/// Instant on Linux, which answers with a reset; a little over two seconds on
/// Windows, which retransmits the SYN before giving up. A port whose refusal
/// has not arrived yet is reported as filtered — correctly, since nothing has
/// been heard — so a budget shorter than this makes a closed port look
/// filtered there and nowhere else. It is an upper bound, so it costs a
/// platform that answers at once nothing at all.
const REFUSAL_BUDGET: Duration = Duration::from_secs(5);

fn fast_options() -> ScanOptions {
    ScanOptions {
        timeout_ms: REFUSAL_BUDGET.as_millis() as u64,
        concurrency: 50,
        grab_banners: true,
    }
}

#[tokio::test]
async fn scan_finds_open_and_closed_ports_and_reads_banners() {
    let ssh = greeting_server(b"SSH-2.0-NocturneTest_1.0\r\n").await;
    let ftp = greeting_server(b"220 (testFTPd 0.1)\r\n").await;
    let telnet = greeting_server(&[0xff, 0xfd, 0x18, 0xff, 0xfb, 0x01]).await;
    let silent = greeting_server(b"").await;
    let closed = closed_port().await;

    let ports = [closed, ssh, silent, telnet, ftp];
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<PortResult>();
    let results = tcp::scan(LOCALHOST, &ports, &fast_options(), Some(sender))
        .await
        .unwrap();

    // Every port, in the order given.
    assert_eq!(results.iter().map(|r| r.port).collect::<Vec<_>>(), ports);
    let by_port = |port: u16| results.iter().find(|r| r.port == port).unwrap();

    assert_eq!(by_port(closed).state, PortState::Closed);
    assert_eq!(by_port(closed).banner, None);
    assert!(
        by_port(closed).rtt_ms.is_some(),
        "a refusal is an answer with a round-trip time"
    );

    assert_eq!(by_port(ssh).state, PortState::Open);
    assert_eq!(
        by_port(ssh).banner.as_deref(),
        Some("SSH-2.0-NocturneTest_1.0")
    );
    assert!(by_port(ssh).rtt_ms.unwrap() < 500.0);

    assert_eq!(by_port(ftp).banner.as_deref(), Some("220 (testFTPd 0.1)"));
    assert_eq!(by_port(telnet).banner.as_deref(), Some("telnet"));

    assert_eq!(by_port(silent).state, PortState::Open);
    assert_eq!(by_port(silent).banner, None);

    // Open ports — and only those — were streamed.
    let mut streamed = Vec::new();
    while let Ok(result) = receiver.try_recv() {
        streamed.push(result.port);
    }
    streamed.sort_unstable();
    let mut open = vec![ssh, ftp, telnet, silent];
    open.sort_unstable();
    assert_eq!(streamed, open);

    // The result is what the UI receives as JSON.
    let json = serde_json::to_value(by_port(ssh)).unwrap();
    assert_eq!(json["state"], "open");
    assert_eq!(json["banner"], "SSH-2.0-NocturneTest_1.0");
}

#[tokio::test]
async fn http_ports_are_asked_for_their_server_header_and_nothing_else_is_sent() {
    let Some((port, mut requests)) = http_server().await else {
        eprintln!("skipping: none of the HTTP test ports is free");
        return;
    };
    let quiet = greeting_server(b"").await;
    let results = tcp::scan(LOCALHOST, &[port, quiet], &fast_options(), None)
        .await
        .unwrap();
    assert_eq!(results[0].state, PortState::Open);
    assert_eq!(results[0].banner.as_deref(), Some("NocturneTest/1.0"));
    assert!(!results[0].service.is_empty());
    let request = requests.recv().await.unwrap();
    assert_eq!(
        request, b"HEAD / HTTP/1.0\r\n\r\n",
        "exactly one HEAD request"
    );
    assert_eq!(results[1].banner, None);
}

#[tokio::test]
async fn banner_grabbing_can_be_turned_off() {
    let ssh = greeting_server(b"SSH-2.0-NocturneTest_1.0\r\n").await;
    let options = ScanOptions {
        grab_banners: false,
        ..fast_options()
    };
    let results = tcp::scan(LOCALHOST, &[ssh], &options, None).await.unwrap();
    assert_eq!(results[0].state, PortState::Open);
    assert_eq!(results[0].banner, None);
}

#[tokio::test]
async fn duplicates_are_scanned_once_and_service_names_are_filled_in() {
    let open = greeting_server(b"").await;
    let options = ScanOptions {
        grab_banners: false,
        ..fast_options()
    };
    let results = tcp::scan("localhost", &[open, 22, open, 443, 22], &options, None)
        .await
        .unwrap();
    assert_eq!(
        results.iter().map(|r| r.port).collect::<Vec<_>>(),
        [open, 22, 443]
    );
    assert_eq!(results[1].service, "ssh");
    assert_eq!(results[2].service, "https");
}

#[tokio::test]
async fn check_reports_one_port() {
    let open = greeting_server(b"SSH-2.0-NocturneTest_1.0\r\n").await;
    let closed = closed_port().await;
    let result = tcp::check(LOCALHOST, open, REFUSAL_BUDGET).await.unwrap();
    assert_eq!(result.state, PortState::Open);
    assert_eq!(result.banner, None, "check does not wait for a banner");
    let result = tcp::check(LOCALHOST, closed, REFUSAL_BUDGET).await.unwrap();
    assert_eq!(result.state, PortState::Closed);

    assert!(matches!(
        tcp::check(LOCALHOST, 0, Duration::from_secs(1)).await,
        Err(ProbeError::Invalid(_))
    ));
    assert!(matches!(
        tcp::check("no-such-host.invalid", 22, Duration::from_secs(1)).await,
        Err(ProbeError::Resolve)
    ));
}

#[tokio::test]
async fn unanswered_connection_is_filtered_and_bounded_by_the_timeout() {
    // TEST-NET-2 is never assigned: the SYN is dropped, or a router rejects it.
    let started = Instant::now();
    let result = tcp::check("198.51.100.77", 81, Duration::from_millis(400))
        .await
        .unwrap();
    assert_ne!(result.state, PortState::Open);
    if result.state == PortState::Filtered {
        assert_eq!(result.rtt_ms, None);
    }
    // The claim is that the check is bounded by the timeout it was given
    // rather than by the kernel's own SYN retry schedule, which is north of
    // twenty seconds on every platform here. The bound only has to separate
    // those two, so it is set far above the 400 ms asked for: a bound a
    // whisker above the timeout measures the runner, not the tracker.
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_thousand_ports_with_bounded_concurrency() {
    let open = greeting_server(b"").await;
    let mut ports: Vec<u16> = (20_000..21_000).collect();
    ports.push(open);
    let options = ScanOptions {
        timeout_ms: 1000,
        concurrency: 64,
        grab_banners: false,
    };
    let started = Instant::now();
    let results = tcp::scan(LOCALHOST, &ports, &options, None).await.unwrap();
    // Every port asked about is reported on, exactly once, and the one thing
    // listening is the one found. That is what the scan guarantees, and none
    // of it depends on the machine.
    assert_eq!(results.len(), ports.len());
    let mut reported: Vec<u16> = results.iter().map(|result| result.port).collect();
    reported.sort_unstable();
    let mut asked = ports.clone();
    asked.sort_unstable();
    assert_eq!(reported, asked, "one result per port asked about");
    assert_eq!(results.last().unwrap().state, PortState::Open);
    assert_eq!(
        results
            .iter()
            .filter(|result| result.state == PortState::Open)
            .count(),
        1,
        "only the port that is listening is open"
    );
    // A thousand ports, sixty-four at a time, a second apiece: sixteen
    // seconds if every one of them had to wait out its timeout. The old bound
    // of twenty seconds was inside that, so the test could go red for a scan
    // that was behaving exactly as configured — it only passed because
    // loopback refuses a closed port at once. This one is a hang detector.
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn invalid_options_are_rejected() {
    let zero_timeout = ScanOptions {
        timeout_ms: 0,
        ..ScanOptions::default()
    };
    assert!(matches!(
        tcp::scan(LOCALHOST, &[22], &zero_timeout, None).await,
        Err(ProbeError::Invalid(_))
    ));
    let zero_workers = ScanOptions {
        concurrency: 0,
        ..ScanOptions::default()
    };
    assert!(matches!(
        tcp::scan(LOCALHOST, &[22], &zero_workers, None).await,
        Err(ProbeError::Invalid(_))
    ));
    assert!(matches!(
        tcp::scan(LOCALHOST, &[22, 0], &ScanOptions::default(), None).await,
        Err(ProbeError::Invalid(_))
    ));
    assert_eq!(
        tcp::scan(LOCALHOST, &[], &ScanOptions::default(), None)
            .await
            .unwrap(),
        vec![]
    );
}

/// Runs nmap and returns its grepable output, or `None` when nmap is not installed.
fn nmap(args: &[&str]) -> Option<String> {
    let output = Command::new("nmap").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[test]
fn top100_is_the_set_nmap_scans() {
    let Some(output) = nmap(&[
        "--top-ports",
        "100",
        "-sT",
        "-n",
        "-Pn",
        "-oG",
        "-",
        "-v",
        LOCALHOST,
    ]) else {
        eprintln!("skipping: nmap is not installed");
        return;
    };
    // "# Ports scanned: TCP(100;7,9,13,21-23,…) UDP(0;) …"
    let list = output
        .lines()
        .find_map(|line| line.split_once("TCP(100;"))
        .map(|(_, rest)| rest.split(')').next().unwrap().to_owned())
        .expect("nmap prints the ports it scanned");
    assert_eq!(
        tcp::parse_ports(&list).unwrap(),
        tcp::parse_ports("top100").unwrap()
    );
}

#[tokio::test]
async fn states_agree_with_nmap() {
    let ssh = greeting_server(b"SSH-2.0-NocturneTest_1.0\r\n").await;
    let silent = greeting_server(b"").await;
    let closed = closed_port().await;
    let ports = [ssh, silent, closed];
    let spec = ports.map(|port| port.to_string()).join(",");

    let ours = tcp::scan(LOCALHOST, &ports, &fast_options(), None)
        .await
        .unwrap();

    let args = [
        "-sT",
        "-n",
        "-Pn",
        "-p",
        spec.as_str(),
        "-oG",
        "-",
        LOCALHOST,
    ];
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let output = tokio::task::spawn_blocking(move || {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        nmap(&args)
    })
    .await
    .unwrap();
    let Some(output) = output else {
        eprintln!("skipping: nmap is not installed");
        return;
    };
    // "Host: 127.0.0.1 ()	Ports: 40001/open/tcp//…/, 40002/closed/tcp//…/"
    for result in &ours {
        let needle_open = format!("{}/open/tcp", result.port);
        let needle_closed = format!("{}/closed/tcp", result.port);
        let nmap_state = if output.contains(&needle_open) {
            PortState::Open
        } else if output.contains(&needle_closed) {
            PortState::Closed
        } else {
            PortState::Filtered
        };
        assert_eq!(
            result.state, nmap_state,
            "port {} (nmap said: {output})",
            result.port
        );
    }
    let _: SocketAddr = format!("{LOCALHOST}:{ssh}").parse().unwrap();
}
