//! Syslog: a parser for what network devices actually send, a UDP / TCP listener, and the
//! other direction — an RFC 5424 formatter ([`format`]) and a sender over UDP, TCP and TLS
//! ([`send`]) for handing messages on to a collector.

pub mod format;
mod parse;
pub mod send;

use std::net::SocketAddr;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::{JoinHandle, JoinSet};

pub use self::parse::parse;
use crate::ProbeError;

/// One syslog message, taken apart.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SyslogMessage {
    /// The address the message came from.
    pub source: String,
    /// Facility and severity from the `<PRI>` prefix, when the message has one.
    pub facility: Option<u8>,
    pub severity: Option<u8>,
    /// The timestamp exactly as the device wrote it (not reinterpreted).
    pub timestamp: Option<String>,
    pub hostname: Option<String>,
    /// The process or application that logged the message.
    pub app: Option<String>,
    pub procid: Option<String>,
    pub msgid: Option<String>,
    /// The human-readable part.
    pub message: String,
    /// The vendor's event identifier, when the message carries one: Cisco `%LINK-3-UPDOWN` is
    /// (`LINK`, 3, `UPDOWN`); a Junos tag `UI_COMMIT` is (`UI`, none, `COMMIT`); RouterOS
    /// topics `system,info,account` are (`system`, 6, `account`).
    pub mnemonic: Option<Mnemonic>,
    /// The device's own message counter (Cisco's leading number, Huawei's `[n]`, RFC 5424
    /// `sequenceId`): gaps mean lost messages.
    pub sequence: Option<u64>,
    /// The message as received.
    pub raw: String,
}

/// A vendor event identifier.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Mnemonic {
    /// The subsystem that raised the event.
    pub facility: String,
    /// The severity the vendor attached to it (0 = emergency … 7 = debug).
    pub severity: Option<u8>,
    /// The event within the subsystem. Empty for RouterOS messages that have no topic besides
    /// the subsystem and the severity.
    pub name: String,
}

/// `emergency`, `alert`, `critical`, `error`, `warning`, `notice`, `informational`, `debug`.
pub fn severity_name(severity: u8) -> &'static str {
    match severity {
        0 => "emergency",
        1 => "alert",
        2 => "critical",
        3 => "error",
        4 => "warning",
        5 => "notice",
        6 => "informational",
        7 => "debug",
        _ => "unknown",
    }
}

/// The conventional name of a syslog facility (`kern`, `daemon`, `local7`, …).
pub fn facility_name(facility: u8) -> &'static str {
    const NAMES: [&str; 24] = [
        "kern", "user", "mail", "daemon", "auth", "syslog", "lpr", "news", "uucp", "cron",
        "authpriv", "ftp", "ntp", "audit", "alert", "clock", "local0", "local1", "local2",
        "local3", "local4", "local5", "local6", "local7",
    ];
    NAMES
        .get(usize::from(facility))
        .copied()
        .unwrap_or("unknown")
}

/// The largest message accepted over TCP; longer ones are cut here.
const MAX_MESSAGE: usize = 64 * 1024;
/// The most TCP connections served at once.
const MAX_CONNECTIONS: usize = 1024;

/// A running syslog listener. It stops when [`stop`](SyslogServer::stop) is called or it is
/// dropped.
#[derive(Debug)]
pub struct SyslogServer {
    udp_addr: SocketAddr,
    tcp_addr: Option<SocketAddr>,
    task: Option<JoinHandle<()>>,
}

impl SyslogServer {
    /// Listens on UDP and, if `tcp` is given, on TCP (RFC 6587: octet-counted and
    /// newline-framed messages, recognised per message). Parsed messages arrive on the channel.
    pub async fn start(
        udp: SocketAddr,
        tcp: Option<SocketAddr>,
    ) -> Result<(SyslogServer, UnboundedReceiver<SyslogMessage>), ProbeError> {
        let udp_socket = UdpSocket::bind(udp)
            .await
            .map_err(|error| ProbeError::from_bind(error, "UDP", udp))?;
        let listener = match tcp {
            Some(tcp) => Some(
                TcpListener::bind(tcp)
                    .await
                    .map_err(|error| ProbeError::from_bind(error, "TCP", tcp))?,
            ),
            None => None,
        };
        let udp_addr = udp_socket.local_addr()?;
        let tcp_addr = match &listener {
            Some(listener) => Some(listener.local_addr()?),
            None => None,
        };
        let (messages, receiver) = unbounded_channel();
        let task = tokio::spawn(serve(udp_socket, listener, messages));
        Ok((
            SyslogServer {
                udp_addr,
                tcp_addr,
                task: Some(task),
            },
            receiver,
        ))
    }

    /// The UDP address the server listens on (useful after binding port 0).
    pub fn udp_addr(&self) -> SocketAddr {
        self.udp_addr
    }

    pub fn tcp_addr(&self) -> Option<SocketAddr> {
        self.tcp_addr
    }

    /// Stops listening and closes every connection; returns when the sockets are closed.
    pub async fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for SyslogServer {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// Receives datagrams and accepts connections until aborted, or until nobody listens to the
/// messages any more. Dropping the connection set closes the connections.
async fn serve(udp: UdpSocket, tcp: Option<TcpListener>, messages: UnboundedSender<SyslogMessage>) {
    let mut connections: JoinSet<()> = JoinSet::new();
    let mut datagram = vec![0u8; 65_536];
    loop {
        tokio::select! {
            received = udp.recv_from(&mut datagram) => match received {
                Ok((length, source)) => {
                    if messages.send(parse(&datagram[..length], source)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    tracing::debug!(%error, "syslog UDP receive failed");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            },
            accepted = accept(tcp.as_ref()) => match accepted {
                Ok((stream, peer)) if connections.len() < MAX_CONNECTIONS => {
                    connections.spawn(read_stream(stream, peer, messages.clone()));
                }
                // Over the limit: the connection is closed at once.
                Ok(_) => {}
                Err(error) => {
                    tracing::debug!(%error, "syslog TCP accept failed");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            },
            Some(_) = connections.join_next() => {}
        }
    }
}

/// Accepts a connection, or waits forever when there is no TCP listener.
async fn accept(listener: Option<&TcpListener>) -> std::io::Result<(TcpStream, SocketAddr)> {
    match listener {
        Some(listener) => listener.accept().await,
        None => std::future::pending().await,
    }
}

/// Reads the messages of one TCP connection.
async fn read_stream(
    mut stream: TcpStream,
    peer: SocketAddr,
    messages: UnboundedSender<SyslogMessage>,
) {
    let mut framer = Framer::default();
    let mut chunk = vec![0u8; 16 * 1024];
    loop {
        let count = match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(count) => count,
        };
        for frame in framer.push(&chunk[..count]) {
            if messages.send(parse(&frame, peer)).is_err() {
                return;
            }
        }
        if framer.broken {
            return;
        }
    }
    // A last line without its terminator still counts.
    if let Some(frame) = framer.finish() {
        let _ = messages.send(parse(&frame, peer));
    }
}

/// Splits a TCP byte stream into syslog messages (RFC 6587).
///
/// Each message is either *octet-counted* (`<length> <message>`, the length in decimal) or
/// *newline-framed*. The two are told apart per message: a count starts with a digit, a
/// message with `<`.
#[derive(Default)]
struct Framer {
    buffer: Vec<u8>,
    /// Skipping the rest of a line that was too long.
    discarding: bool,
    /// The stream cannot be followed any further (an impossible length prefix).
    broken: bool,
}

impl Framer {
    /// Adds received bytes and returns the messages that are now complete.
    fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buffer.extend_from_slice(data);
        let mut frames = Vec::new();
        loop {
            if self.discarding {
                match self.buffer.iter().position(|byte| *byte == b'\n') {
                    Some(end) => {
                        self.buffer.drain(..=end);
                        self.discarding = false;
                    }
                    None => {
                        self.buffer.clear();
                        break;
                    }
                }
            }
            // Line ends between messages (and after octet-counted ones) carry nothing.
            let skip = self
                .buffer
                .iter()
                .take_while(|byte| matches!(byte, b'\n' | b'\r' | 0))
                .count();
            self.buffer.drain(..skip);
            let Some(first) = self.buffer.first() else {
                break;
            };
            if first.is_ascii_digit() {
                // Octet counting: "<digits> " and then that many bytes, which begin with '<'.
                let digits = self
                    .buffer
                    .iter()
                    .take_while(|byte| byte.is_ascii_digit())
                    .count();
                let after = (self.buffer.get(digits), self.buffer.get(digits + 1));
                match after {
                    // Too early to tell a count from a line that starts with a number.
                    (None, _) | (Some(b' '), None) if digits <= 10 => break,
                    (Some(b' '), Some(b'<')) if digits <= 10 => {
                        let length = std::str::from_utf8(&self.buffer[..digits])
                            .ok()
                            .and_then(|text| text.parse::<usize>().ok())
                            .filter(|length| *length <= MAX_MESSAGE);
                        let Some(length) = length else {
                            // Nothing after an impossible count can be trusted to be a frame.
                            self.broken = true;
                            self.buffer.clear();
                            break;
                        };
                        if self.buffer.len() < digits + 1 + length {
                            break;
                        }
                        frames.push(
                            self.buffer
                                .drain(..digits + 1 + length)
                                .skip(digits + 1)
                                .collect(),
                        );
                        continue;
                    }
                    _ => {}
                }
            }
            match self.buffer.iter().position(|byte| *byte == b'\n') {
                Some(end) => {
                    let mut frame: Vec<u8> = self.buffer.drain(..=end).collect();
                    frame.truncate(end.min(MAX_MESSAGE));
                    frames.push(frame);
                }
                None if self.buffer.len() > MAX_MESSAGE => {
                    // An endless line: deliver its beginning and skip the rest.
                    frames.push(self.buffer[..MAX_MESSAGE].to_vec());
                    self.buffer.clear();
                    self.discarding = true;
                    break;
                }
                None => break,
            }
        }
        frames
    }

    /// What is left when the connection ends.
    fn finish(&mut self) -> Option<Vec<u8>> {
        let rest = std::mem::take(&mut self.buffer);
        let meaningful =
            !self.discarding && !self.broken && rest.iter().any(|byte| !byte.is_ascii_whitespace());
        meaningful.then_some(rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn names() {
        let severities: Vec<&str> = (0..=8).map(severity_name).collect();
        assert_eq!(
            severities,
            [
                "emergency",
                "alert",
                "critical",
                "error",
                "warning",
                "notice",
                "informational",
                "debug",
                "unknown"
            ]
        );
        assert_eq!(facility_name(0), "kern");
        assert_eq!(facility_name(3), "daemon");
        assert_eq!(facility_name(4), "auth");
        assert_eq!(facility_name(16), "local0");
        assert_eq!(facility_name(23), "local7");
        assert_eq!(facility_name(24), "unknown");
        // <189> is what Cisco devices send by default: local7.notice.
        assert_eq!(
            (facility_name(189 >> 3), severity_name(189 & 7)),
            ("local7", "notice")
        );
    }

    fn frames(chunks: &[&[u8]]) -> (Vec<String>, Framer) {
        let mut framer = Framer::default();
        let mut out = Vec::new();
        for chunk in chunks {
            for frame in framer.push(chunk) {
                out.push(String::from_utf8_lossy(&frame).into_owned());
            }
        }
        (out, framer)
    }

    #[test]
    fn newline_framing() {
        let (out, mut framer) = frames(&[
            b"<13>first\n<14>second\r\n\n<15>thi",
            b"rd\n<16>unterminated",
        ]);
        assert_eq!(out, ["<13>first", "<14>second\r", "<15>third"]);
        assert_eq!(framer.finish().as_deref(), Some(&b"<16>unterminated"[..]));
    }

    #[test]
    fn octet_counted_framing() {
        // RFC 6587 section 3.4.1: "MSG-LEN SP SYSLOG-MSG", with no separator in between.
        let (out, mut framer) = frames(&[b"9 <13>first11 <14>s\necond", b"9 <15>thi", b"r", b"d"]);
        assert_eq!(out, ["<13>first", "<14>s\necond", "<15>third"]);
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn both_framings_on_one_connection() {
        let (out, _) = frames(&[b"<13>line one\n11 <14>counted\n<15>line two\n9 <16>again"]);
        assert_eq!(
            out,
            ["<13>line one", "<14>counted", "<15>line two", "<16>again"]
        );
    }

    #[test]
    fn a_line_starting_with_a_number_is_a_line() {
        let (out, _) =
            frames(&[b"2026-11-13 device rebooted\n45: *Mar  1 00:02:11.003: %SYS-5-RESTART: x\n"]);
        assert_eq!(
            out,
            [
                "2026-11-13 device rebooted",
                "45: *Mar  1 00:02:11.003: %SYS-5-RESTART: x"
            ]
        );
    }

    #[test]
    fn oversized_input_is_bounded() {
        // An endless line is cut at the limit and the rest of it skipped.
        let mut framer = Framer::default();
        let mut delivered = Vec::new();
        for _ in 0..40 {
            delivered.extend(framer.push(&[b'a'; 4096]));
            assert!(framer.buffer.len() <= MAX_MESSAGE + 4096);
        }
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].len(), MAX_MESSAGE);
        delivered.extend(framer.push(b"tail of the long line\n<13>next\n"));
        assert_eq!(delivered.len(), 2);
        assert_eq!(delivered[1], b"<13>next");

        // An absurd length prefix ends the connection instead of waiting for gigabytes.
        let mut framer = Framer::default();
        assert!(framer.push(b"99999999 <13>x").is_empty());
        assert!(framer.broken);
        let mut framer = Framer::default();
        assert!(framer.push(b"65537 <13>x").is_empty());
        assert!(framer.broken);
        assert_eq!(framer.finish(), None);
    }

    #[test]
    fn random_streams_never_panic_or_grow_without_bound() {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        let mut framer = Framer::default();
        for _ in 0..20_000 {
            let mut chunk = vec![0u8; (state % 300) as usize];
            for byte in &mut chunk {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                // Mostly digits, spaces, newlines and '<' so that the framing logic is exercised.
                *byte = b"0123456789 \n<>ab\r\0"[(state % 18) as usize];
            }
            for frame in framer.push(&chunk) {
                assert!(frame.len() <= MAX_MESSAGE);
            }
            assert!(framer.buffer.len() <= MAX_MESSAGE + 300);
            if framer.broken {
                framer = Framer::default();
            }
        }
    }
}
