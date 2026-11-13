//! Echo over ICMP sockets.
//!
//! * **Datagram sockets** (`SOCK_DGRAM` + `IPPROTO_ICMP`, unprivileged on Linux with
//!   `ping_group_range` and on macOS): one socket per echo. On Linux the kernel delivers only
//!   that socket's replies and reports ICMP errors through the socket's error queue.
//! * **Raw sockets** (root / `CAP_NET_RAW` / Administrator): a raw socket receives a copy of
//!   every ICMP packet that reaches the host, so one socket per address family is shared by all
//!   echoes and a reader task hands each reply to the echo waiting for it.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::PingReply;
use super::icmp::{self, Family, Incoming, IpHeader};
use crate::ProbeError;
use crate::util::{self, millis};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SocketKind {
    Datagram,
    Raw,
}

/// Why an echo over a socket did not produce a result.
#[derive(Debug)]
pub(crate) enum EchoError {
    /// This kind of socket may not be opened by this process; another method may still work.
    SocketDenied,
    Probe(ProbeError),
}

impl From<ProbeError> for EchoError {
    fn from(error: ProbeError) -> Self {
        EchoError::Probe(error)
    }
}

/// Opens a non-blocking ICMP socket. Also used to detect which kinds this process may open.
pub(crate) fn open(kind: SocketKind, family: Family) -> io::Result<Socket> {
    let (domain, protocol) = match family {
        Family::V4 => (Domain::IPV4, Protocol::ICMPV4),
        Family::V6 => (Domain::IPV6, Protocol::ICMPV6),
    };
    let socket_type = match kind {
        SocketKind::Datagram => Type::DGRAM,
        SocketKind::Raw => Type::RAW,
    };
    let socket = Socket::new(domain, socket_type, Some(protocol))?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

/// Opens an ICMP socket for sending echoes, classifying why it cannot be opened.
fn open_for_echo(kind: SocketKind, family: Family) -> Result<Socket, EchoError> {
    let socket = open(kind, family).map_err(|error| {
        if os_error_is(&error, FAMILY_MISSING) {
            EchoError::Probe(ProbeError::unsupported(match family {
                Family::V4 => "IPv4 is not available on this machine",
                Family::V6 => "IPv6 is not available on this machine",
            }))
        } else if error.kind() == io::ErrorKind::PermissionDenied
            || os_error_is(&error, SOCKET_TYPE_MISSING)
        {
            EchoError::SocketDenied
        } else {
            EchoError::Probe(ProbeError::Io(error))
        }
    })?;
    #[cfg(target_os = "linux")]
    {
        // Best effort: without them the echo still works, with less detail in the result.
        let _ = crate::sys::linux::enable_recv_ttl(&socket, family.is_v6());
        if kind == SocketKind::Datagram {
            let _ = crate::sys::linux::enable_error_queue(&socket, family.is_v6());
        }
    }
    Ok(socket)
}

/// Hands the socket to tokio. An ICMP socket is only ever used through sendto / recvfrom,
/// which is all a UDP socket wrapper does with its descriptor.
fn into_async(socket: Socket) -> Result<UdpSocket, EchoError> {
    let socket: std::net::UdpSocket = socket.into();
    UdpSocket::from_std(socket).map_err(|error| EchoError::Probe(ProbeError::Io(error)))
}

// These errno values have no portable `io::ErrorKind`, so the raw codes are compared.

/// EAFNOSUPPORT: the kernel runs without this address family.
const FAMILY_MISSING: &[i32] = if cfg!(windows) {
    &[10047]
} else if cfg!(target_os = "linux") {
    &[97]
} else {
    &[47]
};

/// EPROTONOSUPPORT / ESOCKTNOSUPPORT: this system has no such kind of ICMP socket.
const SOCKET_TYPE_MISSING: &[i32] = if cfg!(windows) {
    &[10043, 10044]
} else if cfg!(target_os = "linux") {
    &[93, 94]
} else {
    &[43, 44]
};

/// EMSGSIZE: the datagram is larger than the protocol can carry.
const MESSAGE_TOO_LONG: &[i32] = if cfg!(windows) {
    &[10040]
} else if cfg!(target_os = "linux") {
    &[90]
} else {
    &[40]
};

fn os_error_is(error: &io::Error, codes: &[i32]) -> bool {
    error
        .raw_os_error()
        .is_some_and(|code| codes.contains(&code))
}

fn send_error(error: io::Error, target: IpAddr, payload: usize) -> ProbeError {
    if os_error_is(&error, MESSAGE_TOO_LONG) {
        return ProbeError::invalid(format!(
            "a payload of {payload} bytes is too large to send to {target}"
        ));
    }
    if error.kind() == io::ErrorKind::InvalidInput {
        return ProbeError::invalid(format!(
            "an echo request cannot be sent to {target} ({error}); link-local IPv6 addresses \
             need an interface scope, which a bare address cannot carry"
        ));
    }
    ProbeError::from_network_io(error)
}

fn reply(sent_at: Instant, received_at: Instant, ttl: Option<u8>, payload_len: usize) -> PingReply {
    PingReply {
        rtt_ms: millis(received_at.saturating_duration_since(sent_at)),
        ttl,
        bytes: icmp::ECHO_HEADER + payload_len,
    }
}

fn parse(family: Family, datagram: &[u8], header: IpHeader) -> icmp::Parsed<'_> {
    match family {
        Family::V4 => icmp::parse_v4(datagram, header),
        Family::V6 => icmp::parse_v6(datagram),
    }
}

/// One echo over its own datagram ICMP socket.
pub(crate) async fn echo_datagram(
    target: IpAddr,
    timeout: Duration,
    payload_len: usize,
) -> Result<PingReply, EchoError> {
    let family = Family::of(target);
    let socket = into_async(open_for_echo(SocketKind::Datagram, family)?)?;
    let random = util::random_u64();
    let (id, seq) = ((random >> 16) as u16, random as u16);
    let payload = icmp::echo_payload(util::random_u64(), payload_len);
    let request = icmp::echo_request(family, id, seq, &payload);
    let nonce = &payload[..payload.len().min(8)];
    // Linux rewrites the identifier to the socket's own and delivers only matching replies, so
    // the identifier we chose cannot be compared there.
    let expected_id = (!cfg!(target_os = "linux")).then_some(id);

    let sent_at = Instant::now();
    socket
        .send_to(&request, SocketAddr::new(target, 0))
        .await
        .map_err(|error| send_error(error, target, payload_len))?;

    let mut buffer = vec![0u8; payload_len + 256];
    let wait = async {
        loop {
            match datagram_event(&socket, &mut buffer).await? {
                Event::Unreachable(reason) => return Err(ProbeError::Unreachable(reason)),
                Event::Datagram { len, from, ttl } => {
                    let received_at = Instant::now();
                    let parsed = parse(family, &buffer[..len], IpHeader::Detect);
                    match parsed.message {
                        Incoming::EchoReply {
                            id,
                            seq: got,
                            payload,
                        } if got == seq
                            && expected_id.is_none_or(|expected| expected == id)
                            && payload.starts_with(nonce) =>
                        {
                            let ttl = ttl.or(parsed.ttl);
                            return Ok(reply(sent_at, received_at, ttl, payload.len()));
                        }
                        Incoming::Error {
                            id,
                            seq: got,
                            reason,
                        } if got == seq && expected_id.is_none_or(|expected| expected == id) => {
                            return Err(ProbeError::Unreachable(reported_by(reason, from)));
                        }
                        _ => {}
                    }
                }
            }
        }
    };
    match tokio::time::timeout(timeout, wait).await {
        Ok(result) => result.map_err(EchoError::Probe),
        Err(_) => Err(EchoError::Probe(ProbeError::Timeout)),
    }
}

fn reported_by(reason: &str, from: Option<IpAddr>) -> String {
    match from {
        Some(from) => format!("{reason} (reported by {from})"),
        None => reason.to_owned(),
    }
}

enum Event {
    Datagram {
        len: usize,
        from: Option<IpAddr>,
        /// From ancillary data, where the platform provides it.
        ttl: Option<u8>,
    },
    /// The network reported the echo undeliverable (Linux error queue).
    Unreachable(String),
}

#[cfg(target_os = "linux")]
async fn datagram_event(socket: &UdpSocket, buffer: &mut [u8]) -> Result<Event, ProbeError> {
    use crate::sys::linux;
    use tokio::io::Interest;

    loop {
        let ready = socket.ready(Interest::READABLE | Interest::ERROR).await?;
        if ready.is_error() {
            let mut quoted = [0u8; 64];
            match socket.try_io(Interest::ERROR, || linux::recv_error(socket, &mut quoted)) {
                Ok(error) => return Ok(Event::Unreachable(describe_queued(&error))),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(ProbeError::from_network_io(error)),
            }
        }
        if ready.is_readable() {
            match socket.try_io(Interest::READABLE, || linux::recv_with_ttl(socket, buffer)) {
                Ok(received) => {
                    return Ok(Event::Datagram {
                        len: received.len,
                        from: received.from,
                        ttl: received.ttl,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(ProbeError::from_network_io(error)),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn describe_queued(error: &crate::sys::linux::QueuedError) -> String {
    use crate::sys::linux::Origin;
    let what = match error.origin {
        Origin::Icmp => icmp::error_text_v4(error.icmp_type, error.icmp_code).to_owned(),
        Origin::Icmp6 => icmp::error_text_v6(error.icmp_type, error.icmp_code).to_owned(),
        Origin::Local | Origin::Other => io::Error::from_raw_os_error(error.errno).to_string(),
    };
    reported_by(&what, error.offender)
}

#[cfg(not(target_os = "linux"))]
async fn datagram_event(socket: &UdpSocket, buffer: &mut [u8]) -> Result<Event, ProbeError> {
    let (len, from) = socket
        .recv_from(buffer)
        .await
        .map_err(ProbeError::from_network_io)?;
    Ok(Event::Datagram {
        len,
        from: Some(from.ip()),
        ttl: None,
    })
}

/// What the reader task tells a waiting echo.
enum Outcome {
    Reply {
        received_at: Instant,
        ttl: Option<u8>,
        payload_len: usize,
    },
    Unreachable(String),
}

struct Waiter {
    /// The first bytes of the echo's payload; a reply must carry them back.
    nonce: Vec<u8>,
    sender: oneshot::Sender<Outcome>,
}

type Waiters = Arc<Mutex<HashMap<u16, Waiter>>>;

/// Receive buffer requested for the shared raw socket: room for a few thousand queued replies.
#[cfg(target_os = "linux")]
const RAW_RECEIVE_BUFFER: usize = 4 * 1024 * 1024;

/// A raw ICMP socket shared by every echo of one address family.
pub(crate) struct RawEndpoint {
    family: Family,
    socket: Arc<UdpSocket>,
    /// Identifier of every echo sent from this endpoint; echoes are told apart by sequence.
    id: u16,
    next_seq: AtomicU16,
    waiters: Waiters,
    reader: JoinHandle<()>,
}

impl RawEndpoint {
    /// Opens the socket and starts the reader. Must be called from within a tokio runtime.
    pub(crate) fn open(family: Family) -> Result<Self, EchoError> {
        let socket = open_for_echo(SocketKind::Raw, family)?;
        let id = util::random_u64() as u16;
        #[cfg(target_os = "linux")]
        {
            crate::sys::linux::grow_receive_buffer(&socket, RAW_RECEIVE_BUFFER);
            if family == Family::V4 {
                // Without the filter the socket still works; it just queues unrelated ICMP too.
                let _ = crate::sys::linux::filter_raw_icmpv4(&socket, id);
            }
        }
        let socket = Arc::new(into_async(socket)?);
        let waiters: Waiters = Arc::default();
        let reader = tokio::spawn(read_loop(socket.clone(), family, id, waiters.clone()));
        Ok(RawEndpoint {
            family,
            socket,
            id,
            next_seq: AtomicU16::new(util::random_u64() as u16),
            waiters,
            reader,
        })
    }

    /// `false` once the reader has stopped (socket failure, or its runtime shut down).
    pub(crate) fn is_alive(&self) -> bool {
        !self.reader.is_finished()
    }

    pub(crate) async fn echo(
        &self,
        target: IpAddr,
        timeout: Duration,
        payload_len: usize,
    ) -> Result<PingReply, ProbeError> {
        let payload = icmp::echo_payload(util::random_u64(), payload_len);
        let (seq, receiver) = self.register(&payload)?;
        let _unregister = Unregister {
            waiters: &self.waiters,
            seq,
        };
        let request = icmp::echo_request(self.family, self.id, seq, &payload);
        let sent_at = Instant::now();
        self.socket
            .send_to(&request, SocketAddr::new(target, 0))
            .await
            .map_err(|error| send_error(error, target, payload_len))?;
        match tokio::time::timeout(timeout, receiver).await {
            Err(_) => Err(ProbeError::Timeout),
            Ok(Err(_)) => Err(ProbeError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the raw ICMP socket stopped receiving",
            ))),
            Ok(Ok(Outcome::Reply {
                received_at,
                ttl,
                payload_len,
            })) => Ok(reply(sent_at, received_at, ttl, payload_len)),
            Ok(Ok(Outcome::Unreachable(reason))) => Err(ProbeError::Unreachable(reason)),
        }
    }

    /// Reserves a sequence number that no in-flight echo is using.
    fn register(&self, payload: &[u8]) -> Result<(u16, oneshot::Receiver<Outcome>), ProbeError> {
        let mut waiters = lock(&self.waiters);
        if waiters.len() > usize::from(u16::MAX) {
            return Err(ProbeError::unsupported(
                "too many echo requests are in flight at once (65536); lower the concurrency",
            ));
        }
        let seq = loop {
            let candidate = self.next_seq.fetch_add(1, Ordering::Relaxed);
            if !waiters.contains_key(&candidate) {
                break candidate;
            }
        };
        let (sender, receiver) = oneshot::channel();
        waiters.insert(
            seq,
            Waiter {
                nonce: payload[..payload.len().min(8)].to_vec(),
                sender,
            },
        );
        Ok((seq, receiver))
    }
}

impl Drop for RawEndpoint {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

/// Frees an echo's sequence number when the echo finishes or its future is dropped.
struct Unregister<'a> {
    waiters: &'a Waiters,
    seq: u16,
}

impl Drop for Unregister<'_> {
    fn drop(&mut self) {
        lock(self.waiters).remove(&self.seq);
    }
}

fn lock(waiters: &Waiters) -> std::sync::MutexGuard<'_, HashMap<u16, Waiter>> {
    waiters.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Receives everything the raw socket sees and wakes the echo each message belongs to.
async fn read_loop(socket: Arc<UdpSocket>, family: Family, id: u16, waiters: Waiters) {
    // Large enough for any IP datagram, so a reply is never truncated.
    let mut buffer = vec![0u8; 65_536];
    let mut consecutive_failures = 0u32;
    loop {
        let (len, from, ancillary_ttl) = match raw_receive(&socket, &mut buffer).await {
            Ok(received) => received,
            Err(error) => {
                // A full receive buffer (ENOBUFS) and similar are transient; a socket that keeps
                // failing is abandoned so that the next echo opens a fresh one.
                consecutive_failures += 1;
                if consecutive_failures > 20 {
                    tracing::warn!(%error, "raw ICMP socket keeps failing; closing it");
                    lock(&waiters).clear();
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
                continue;
            }
        };
        consecutive_failures = 0;
        let received_at = Instant::now();
        let parsed = parse(family, &buffer[..len], IpHeader::Present);
        match parsed.message {
            Incoming::EchoReply {
                id: got,
                seq,
                payload,
            } if got == id => {
                let mut waiters = lock(&waiters);
                let matches = waiters
                    .get(&seq)
                    .is_some_and(|waiter| payload.starts_with(&waiter.nonce));
                if matches && let Some(waiter) = waiters.remove(&seq) {
                    let _ = waiter.sender.send(Outcome::Reply {
                        received_at,
                        ttl: ancillary_ttl.or(parsed.ttl),
                        payload_len: payload.len(),
                    });
                }
            }
            Incoming::Error {
                id: got,
                seq,
                reason,
            } if got == id => {
                if let Some(waiter) = lock(&waiters).remove(&seq) {
                    let _ = waiter
                        .sender
                        .send(Outcome::Unreachable(reported_by(reason, from)));
                }
            }
            _ => {}
        }
    }
}

#[cfg(target_os = "linux")]
async fn raw_receive(
    socket: &UdpSocket,
    buffer: &mut [u8],
) -> io::Result<(usize, Option<IpAddr>, Option<u8>)> {
    let received = socket
        .async_io(tokio::io::Interest::READABLE, || {
            crate::sys::linux::recv_with_ttl(socket, buffer)
        })
        .await?;
    Ok((received.len, received.from, received.ttl))
}

#[cfg(not(target_os = "linux"))]
async fn raw_receive(
    socket: &UdpSocket,
    buffer: &mut [u8],
) -> io::Result<(usize, Option<IpAddr>, Option<u8>)> {
    let (len, from) = socket.recv_from(buffer).await?;
    Ok((len, Some(from.ip()), None))
}
