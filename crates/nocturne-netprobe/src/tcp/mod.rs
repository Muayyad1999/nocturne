//! TCP connect scan with banner grabbing.

mod services;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio::task::JoinSet;

use self::services::{COMMON, TOP_100, service_name, service_port, speaks_plain_http};
use crate::util::{millis, printable};
use crate::{ProbeError, dns};

/// What a connection attempt says about a port.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PortState {
    /// The connection was accepted.
    Open,
    /// The host refused the connection (TCP reset): nothing listens there.
    Closed,
    /// No answer, or an ICMP error: a firewall is dropping the traffic, or the host is down.
    Filtered,
}

/// The result for one port.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PortResult {
    pub port: u16,
    pub state: PortState,
    /// Time until the connection was accepted or refused; `None` when nothing answered.
    pub rtt_ms: Option<f64>,
    /// The usual service on this port (`ssh`, `https`, `winbox`, …); empty if there is none.
    pub service: String,
    /// What the server said about itself, when banners are grabbed.
    pub banner: Option<String>,
}

/// Options of [`scan`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct ScanOptions {
    /// How long to wait for each connection.
    pub timeout_ms: u64,
    /// How many ports are probed at the same time.
    pub concurrency: usize,
    /// Read what open ports volunteer (and ask HTTP ports for their `Server:` header).
    pub grab_banners: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            timeout_ms: 1000,
            concurrency: 200,
            grab_banners: true,
        }
    }
}

/// How long an open port gets to say something.
const BANNER_WAIT: Duration = Duration::from_millis(1500);
/// How much of a banner is read and shown.
const BANNER_READ: usize = 1024;
const BANNER_SHOWN: usize = 160;

/// Parses a port expression: numbers (`22,80,443`), ranges (`1-1024`), service names (`ssh`),
/// `top100` (the 100 ports most often open), `common` (a network administrator's shortlist),
/// `all`, or any mix separated by commas or spaces. The result is sorted without duplicates.
pub fn parse_ports(spec: &str) -> Result<Vec<u16>, ProbeError> {
    let mut selected = vec![false; 65_536];
    let mut any = false;
    for item in spec
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|item| !item.is_empty())
    {
        any = true;
        let lower = item.to_ascii_lowercase();
        match lower.as_str() {
            "top100" | "top-100" => TOP_100
                .iter()
                .for_each(|port| selected[usize::from(*port)] = true),
            "common" => COMMON
                .iter()
                .for_each(|port| selected[usize::from(*port)] = true),
            "all" | "*" => selected[1..].fill(true),
            _ => {
                let (first, last) = parse_range(item)?;
                selected[usize::from(first)..=usize::from(last)].fill(true);
            }
        }
    }
    if !any {
        return Err(ProbeError::invalid(
            "no ports given; use numbers (22,80,443), ranges (1-1024), top100 or common",
        ));
    }
    Ok((1..=u16::MAX)
        .filter(|port| selected[usize::from(*port)])
        .collect())
}

/// One item of a port expression as an inclusive range.
fn parse_range(item: &str) -> Result<(u16, u16), ProbeError> {
    let port = |text: &str| {
        text.parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| {
                ProbeError::invalid(format!(
                    "'{text}' in '{item}' is not a port; ports are numbers from 1 to 65535"
                ))
            })
    };
    if let Some(port) = service_port(item) {
        return Ok((port, port));
    }
    match item.split_once('-') {
        Some((first, last)) if !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()) => {
            let (first, last) = (port(first)?, port(last)?);
            if first > last {
                return Err(ProbeError::invalid(format!(
                    "the port range '{item}' is backwards; write it as {last}-{first}"
                )));
            }
            Ok((first, last))
        }
        _ if item.bytes().all(|b| b.is_ascii_digit()) => port(item).map(|port| (port, port)),
        _ => Err(ProbeError::invalid(format!(
            "'{item}' is not a port, a range, a known service name, top100 or common"
        ))),
    }
}

/// Connect scan of `ports` on `host`.
///
/// Results for open ports are streamed to `progress` as they are found; the return value lists
/// every port scanned, in the order given. Banner grabbing reads what the server volunteers
/// (SSH, FTP, SMTP, …; a Telnet option negotiation is reported as `telnet`) and, on ports that
/// usually speak plain HTTP, sends one `HEAD / HTTP/1.0` to read the `Server:` header. Nothing
/// else is ever sent.
pub async fn scan(
    host: &str,
    ports: &[u16],
    options: &ScanOptions,
    progress: Option<UnboundedSender<PortResult>>,
) -> Result<Vec<PortResult>, ProbeError> {
    if options.timeout_ms == 0 {
        return Err(ProbeError::invalid(
            "the scan timeout must be at least 1 ms",
        ));
    }
    if options.concurrency == 0 {
        return Err(ProbeError::invalid(
            "the scan concurrency must be at least 1",
        ));
    }
    if ports.contains(&0) {
        return Err(ProbeError::invalid(
            "0 is not a TCP port; ports are numbers from 1 to 65535",
        ));
    }
    let address = dns::resolve_one(host).await?;

    let mut unique: Vec<u16> = Vec::with_capacity(ports.len());
    let mut seen = vec![false; 65_536];
    for port in ports {
        if !std::mem::replace(&mut seen[usize::from(*port)], true) {
            unique.push(*port);
        }
    }
    let ports: Arc<[u16]> = unique.into();
    let timeout = Duration::from_millis(options.timeout_ms);
    let grab_banners = options.grab_banners;

    // Workers take the next unscanned port until none is left; dropping the set (when this
    // future is dropped) aborts them and closes their sockets.
    let next = Arc::new(AtomicUsize::new(0));
    let (done, mut finished) = mpsc::unbounded_channel::<(usize, PortResult)>();
    let mut workers = JoinSet::new();
    for _ in 0..options.concurrency.min(ports.len()) {
        let (ports, next, done) = (ports.clone(), next.clone(), done.clone());
        workers.spawn(async move {
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(port) = ports.get(index) else {
                    return;
                };
                let result = probe(address, *port, timeout, grab_banners).await;
                if done.send((index, result)).is_err() {
                    return;
                }
            }
        });
    }
    drop(done);

    let mut results: Vec<Option<PortResult>> = vec![None; ports.len()];
    while let Some((index, result)) = finished.recv().await {
        if result.state == PortState::Open
            && let Some(progress) = &progress
        {
            let _ = progress.send(result.clone());
        }
        results[index] = Some(result);
    }
    // The channel closes when every worker is done; a missing result means a worker died.
    results
        .into_iter()
        .map(|result| {
            result.ok_or_else(|| ProbeError::Io(std::io::Error::other("a scan task failed")))
        })
        .collect()
}

/// Tries to connect to one port. The banner is not read; `Ok` for every outcome the network
/// can produce (`state` tells which).
pub async fn check(host: &str, port: u16, timeout: Duration) -> Result<PortResult, ProbeError> {
    if port == 0 {
        return Err(ProbeError::invalid(
            "0 is not a TCP port; ports are numbers from 1 to 65535",
        ));
    }
    let address = dns::resolve_one(host).await?;
    Ok(probe(address, port, timeout, false).await)
}

/// Connects, waiting out moments when this process is out of sockets (many probes in flight
/// against a low file-descriptor limit) instead of reporting them as an answer from the network.
pub(crate) async fn connect(address: SocketAddr) -> std::io::Result<TcpStream> {
    loop {
        match TcpStream::connect(address).await {
            Err(error) if crate::util::is_out_of_resources(&error) => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            result => return result,
        }
    }
}

async fn probe(address: IpAddr, port: u16, timeout: Duration, grab_banner: bool) -> PortResult {
    let started = Instant::now();
    let attempt = tokio::time::timeout(timeout, connect(SocketAddr::new(address, port))).await;
    let rtt_ms = Some(millis(started.elapsed()));
    let (state, rtt_ms, banner) = match attempt {
        Ok(Ok(stream)) => {
            let banner = if grab_banner {
                let wait = BANNER_WAIT.min(timeout);
                tokio::time::timeout(wait, read_banner(stream, port))
                    .await
                    .unwrap_or(None)
            } else {
                None
            };
            (PortState::Open, rtt_ms, banner)
        }
        Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
            (PortState::Closed, rtt_ms, None)
        }
        // Timeouts, ICMP unreachable / prohibited, and local refusals to send.
        Ok(Err(_)) | Err(_) => (PortState::Filtered, None, None),
    };
    PortResult {
        port,
        state,
        rtt_ms,
        service: service_name(port).to_owned(),
        banner,
    }
}

/// Reads what the server says first. On plain-HTTP ports a `HEAD` request is sent up front:
/// an HTTP server waits for it, and a server that speaks first is not disturbed by it.
async fn read_banner(mut stream: TcpStream, port: u16) -> Option<String> {
    if speaks_plain_http(port) {
        stream.write_all(b"HEAD / HTTP/1.0\r\n\r\n").await.ok()?;
    }
    let mut received = Vec::with_capacity(BANNER_READ);
    let mut buffer = [0u8; BANNER_READ];
    // Keep reading until the first line (or, for HTTP, the header block) is complete.
    loop {
        match stream.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                received.extend_from_slice(&buffer[..count]);
                let is_http = received.starts_with(b"HTTP/");
                let complete = if is_http {
                    contains(&received, b"\r\n\r\n") || contains(&received, b"\n\n")
                } else {
                    received.contains(&b'\n') || is_telnet_negotiation(&received)
                };
                if complete || received.len() >= BANNER_READ {
                    break;
                }
            }
        }
    }
    describe_banner(&received)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Telnet servers open with option negotiation: IAC (0xff) followed by WILL/WONT/DO/DONT.
fn is_telnet_negotiation(data: &[u8]) -> bool {
    matches!(data, [0xff, 0xfb..=0xfe, ..])
}

/// Turns the first bytes a server sent into a one-line description.
fn describe_banner(data: &[u8]) -> Option<String> {
    if data.is_empty() {
        return None;
    }
    if is_telnet_negotiation(data) {
        return Some("telnet".to_owned());
    }
    let text = String::from_utf8_lossy(data);
    if text.starts_with("HTTP/") {
        let mut lines = text.lines();
        let status = lines.next().unwrap_or_default();
        let server = lines
            .take_while(|line| !line.trim().is_empty())
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.trim().eq_ignore_ascii_case("server"))
            .map(|(_, value)| printable(value, BANNER_SHOWN))
            .filter(|value| !value.is_empty());
        return Some(server.unwrap_or_else(|| printable(status, BANNER_SHOWN)));
    }
    if let Some(version) = mysql_version(data) {
        return Some(format!("MySQL {version}"));
    }
    let first_line = text.lines().find(|line| !line.trim().is_empty())?;
    looks_textual(first_line).then(|| printable(first_line, BANNER_SHOWN))
}

/// `true` for a line a person could read: a binary protocol's first bytes are not a banner.
fn looks_textual(line: &str) -> bool {
    let total = line.chars().count();
    let readable = line
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_ascii_punctuation() || *c == ' ')
        .count();
    let letters = line.chars().filter(|c| c.is_alphanumeric()).count();
    letters >= 3 && readable * 10 >= total * 8
}

/// The server version from a MySQL / MariaDB initial handshake packet (protocol 10): a 4-byte
/// packet header, the protocol number, then the version as a NUL-terminated string.
fn mysql_version(data: &[u8]) -> Option<&str> {
    let payload = data.get(4..)?;
    let (&protocol, rest) = payload.split_first()?;
    let end = rest.iter().position(|byte| *byte == 0)?;
    let version = std::str::from_utf8(&rest[..end]).ok()?;
    let plausible = protocol == 10
        && data[3] == 0
        && (3..=80).contains(&version.len())
        && version.starts_with(|c: char| c.is_ascii_digit())
        && version.chars().all(|c| c.is_ascii_graphic());
    plausible.then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn port_expressions() {
        assert_eq!(parse_ports("22,80,443").unwrap(), [22, 80, 443]);
        assert_eq!(parse_ports(" 443, 22 ,80 ").unwrap(), [22, 80, 443]);
        assert_eq!(parse_ports("1-5").unwrap(), [1, 2, 3, 4, 5]);
        assert_eq!(parse_ports("80,78-81,80").unwrap(), [78, 79, 80, 81]);
        assert_eq!(parse_ports("65535").unwrap(), [65535]);
        assert_eq!(parse_ports("65530-65535").unwrap().len(), 6);
        assert_eq!(parse_ports("ssh telnet;HTTPS").unwrap(), [22, 23, 443]);
        assert_eq!(parse_ports("top100").unwrap(), TOP_100);
        assert_eq!(parse_ports("common").unwrap(), COMMON);
        assert_eq!(parse_ports("all").unwrap().len(), 65_535);
        assert_eq!(parse_ports("1-65535").unwrap().len(), 65_535);

        let mixed = parse_ports("top100,8291,2000-2002").unwrap();
        assert_eq!(
            mixed.len(),
            102,
            "top100 has 2000 and 2001; 2002 and 8291 are new"
        );
        assert!(mixed.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(mixed.contains(&8291) && mixed.contains(&2002));
    }

    #[test]
    fn bad_port_expressions_explain_themselves() {
        let cases = [
            ("", "no ports given"),
            (" , ,", "no ports given"),
            ("0", "'0'"),
            ("65536", "'65536'"),
            ("80,abc", "'abc'"),
            ("100-1", "write it as 1-100"),
            ("1-", "''"),
            ("-5", "'-5'"),
            ("1-2-3", "'2-3'"),
            ("22/tcp", "'22/tcp'"),
            ("top1000", "'top1000'"),
            ("0-10", "'0'"),
            ("5-70000", "'70000'"),
        ];
        for (spec, expected) in cases {
            let error = parse_ports(spec).unwrap_err();
            assert!(
                matches!(error, ProbeError::Invalid(_)),
                "{spec:?}: {error:?}"
            );
            assert!(error.to_string().contains(expected), "{spec:?}: {error}");
        }
    }

    #[test]
    fn banners() {
        assert_eq!(
            describe_banner(b"SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5\r\n").as_deref(),
            Some("SSH-2.0-OpenSSH_9.6p1 Ubuntu-3ubuntu13.5")
        );
        assert_eq!(
            describe_banner(b"SSH-2.0-ROSSSH\r\n\x00\x00\x01\x14binary key exchange").as_deref(),
            Some("SSH-2.0-ROSSSH")
        );
        assert_eq!(
            describe_banner(b"220 (vsFTPd 3.0.5)\r\n").as_deref(),
            Some("220 (vsFTPd 3.0.5)")
        );
        assert_eq!(
            describe_banner(b"220 mail.example.com ESMTP Postfix (Ubuntu)\r\n").as_deref(),
            Some("220 mail.example.com ESMTP Postfix (Ubuntu)")
        );
        // IAC DO TERMINAL-TYPE, IAC WILL ECHO … as Cisco IOS and MikroTik send.
        assert_eq!(
            describe_banner(&[0xff, 0xfd, 0x18, 0xff, 0xfb, 0x01, 0xff, 0xfb, 0x03]).as_deref(),
            Some("telnet")
        );
        assert_eq!(
            describe_banner(b"HTTP/1.1 200 OK\r\nDate: Thu, 01 Jan 2026 00:00:00 GMT\r\nServer: nginx/1.24.0\r\nContent-Length: 0\r\n\r\n")
                .as_deref(),
            Some("nginx/1.24.0")
        );
        assert_eq!(
            describe_banner(
                b"HTTP/1.0 401 Unauthorized\r\nserver:  mini_httpd/1.30 26Oct2018 \r\n\r\n"
            )
            .as_deref(),
            Some("mini_httpd/1.30 26Oct2018")
        );
        // No Server header: the status line still says it is an HTTP server.
        assert_eq!(
            describe_banner(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\nServer: in-body")
                .as_deref(),
            Some("HTTP/1.1 404 Not Found")
        );
        assert_eq!(
            describe_banner(b"\r\n\r\nUser Access Verification\r\n").as_deref(),
            Some("User Access Verification")
        );
        assert_eq!(
            describe_banner(b"RFB 003.008\n").as_deref(),
            Some("RFB 003.008")
        );
        assert_eq!(
            describe_banner(b"+OK Dovecot ready.\r\n").as_deref(),
            Some("+OK Dovecot ready.")
        );
        // MySQL 8 handshake: length 0x4a, sequence 0, protocol 10, version, NUL, thread id …
        let mysql = b"\x4a\x00\x00\x00\x0a8.0.36-0ubuntu0.22.04.1\x00\x0b\x00\x00\x00\x5a\x3b\x41";
        assert_eq!(
            describe_banner(mysql).as_deref(),
            Some("MySQL 8.0.36-0ubuntu0.22.04.1")
        );
        // A TLS alert (what an HTTPS server answers to a plain-text request) is not a banner.
        assert_eq!(
            describe_banner(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x46]),
            None
        );
        assert_eq!(
            describe_banner(b"\x16\x03\x01\x02\x00\x01\x00\x01\xfc\x03\x03abc"),
            None
        );
        assert_eq!(describe_banner(b""), None);
        assert_eq!(describe_banner(b"\x00\x01\x02\x03"), None);
        assert_eq!(describe_banner(b"\r\n\r\n"), None);
        let long = [b'A'; 5000];
        assert_eq!(
            describe_banner(&long).map(|banner| banner.len()),
            Some(BANNER_SHOWN)
        );
    }

    #[test]
    fn options_default_and_deserialize_partially() {
        let options = ScanOptions::default();
        assert_eq!(
            (
                options.timeout_ms,
                options.concurrency,
                options.grab_banners
            ),
            (1000, 200, true)
        );
        let partial: ScanOptions = serde_json::from_str(r#"{"grab_banners": false}"#).unwrap();
        assert_eq!(
            partial,
            ScanOptions {
                grab_banners: false,
                ..ScanOptions::default()
            }
        );
        assert_eq!(
            serde_json::to_string(&PortState::Filtered).unwrap(),
            r#""filtered""#
        );
    }
}
