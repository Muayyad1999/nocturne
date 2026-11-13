//! The UDP side of the client: send a datagram, wait for the reply that belongs to it,
//! retransmit when nothing acceptable arrives in time.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use tokio::net::UdpSocket;
use tokio::time::{Instant, timeout_at};

use crate::SnmpError;

/// The largest UDP payload there is.
const MAX_DATAGRAM: usize = 65_535;

/// How many ICMP errors are shrugged off while waiting for one reply.
const MAX_ICMP_ERRORS_PER_ATTEMPT: u32 = 8;

#[derive(Debug)]
pub(crate) struct Transport {
    socket: UdpSocket,
    target: SocketAddr,
    timeout: Duration,
    /// Transmissions per request: the first one plus the configured retries.
    attempts: u32,
    buf: Vec<u8>,
}

impl Transport {
    /// Binds an ephemeral port of the target's address family.
    ///
    /// The socket is deliberately left unconnected: multi-homed agents (and virtual IPs) are
    /// known to answer from another address than the one polled, and a connected socket would
    /// silently drop those replies. Replies are matched by their ids instead, as net-snmp does.
    pub async fn bind(target: SocketAddr, timeout: Duration, retries: u32) -> Result<Transport, SnmpError> {
        let local: SocketAddr = match target {
            SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
            SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
        };
        let socket = UdpSocket::bind(local).await?;
        Ok(Transport {
            socket,
            target,
            timeout,
            attempts: retries.saturating_add(1),
            buf: vec![0; MAX_DATAGRAM],
        })
    }

    pub fn target(&self) -> SocketAddr {
        self.target
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Sends `datagram` up to `attempts` times, each time waiting one timeout for a reply.
    ///
    /// Every datagram that arrives is shown to `accept`, which answers `Ok(Some(reply))` to
    /// finish, `Ok(None)` for a stray, late or forged datagram that should be ignored, and
    /// `Err(_)` when the datagram is the reply but cannot be used.
    pub async fn exchange<T>(
        &mut self,
        datagram: &[u8],
        attempts: u32,
        mut accept: impl FnMut(&[u8]) -> Result<Option<T>, SnmpError>,
    ) -> Result<T, SnmpError> {
        for _ in 0..attempts {
            self.socket.send_to(datagram, self.target).await?;
            let deadline = Instant::now() + self.timeout;
            let mut icmp_errors = 0;
            loop {
                match timeout_at(deadline, self.socket.recv_from(&mut self.buf)).await {
                    Err(_elapsed) => break,
                    Ok(Ok((len, _from))) => {
                        if let Some(reply) = accept(&self.buf[..len])? {
                            return Ok(reply);
                        }
                    }
                    // An ICMP "port unreachable" for an earlier transmission, which some
                    // platforms report on unconnected sockets too. It says nothing about the
                    // next attempt, so it counts as silence.
                    Ok(Err(e)) if is_icmp_noise(&e) => {
                        icmp_errors += 1;
                        if icmp_errors > MAX_ICMP_ERRORS_PER_ATTEMPT {
                            // Never spin on a socket that keeps failing.
                            tokio::time::sleep_until(deadline).await;
                            break;
                        }
                    }
                    Ok(Err(e)) => return Err(e.into()),
                }
            }
        }
        Err(SnmpError::Timeout)
    }
}

fn is_icmp_noise(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset
    )
}
