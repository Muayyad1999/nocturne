//! Linux-only socket features, reached through nix's safe wrappers:
//!
//! * the **error queue** (`IP_RECVERR` / `IPV6_RECVERR`): ICMP errors caused by packets a socket
//!   sent are queued on that socket with the router that reported them. This is what makes an
//!   unprivileged traceroute possible and what tells a ping socket *why* a host is unreachable;
//! * the **TTL / hop limit** of a received datagram, delivered as ancillary data.

use std::io::{self, IoSliceMut};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::{AsFd, AsRawFd};

use nix::libc;
use nix::sys::socket::{ControlMessageOwned, MsgFlags, SockaddrStorage, recvmsg, setsockopt, sockopt};

/// Room for the ancillary data of one message: an extended error plus the offender's address
/// is under 64 bytes, a TTL is 4.
const CONTROL_SPACE: usize = 256;

/// Asks the kernel to queue ICMP errors for packets sent from this socket.
pub(crate) fn enable_error_queue(socket: &impl AsFd, ipv6: bool) -> io::Result<()> {
    if ipv6 {
        setsockopt(socket, sockopt::Ipv6RecvErr, &true)?;
    } else {
        setsockopt(socket, sockopt::Ipv4RecvErr, &true)?;
    }
    Ok(())
}

/// Asks the kernel to report the TTL / hop limit of every received datagram.
pub(crate) fn enable_recv_ttl(socket: &impl AsFd, ipv6: bool) -> io::Result<()> {
    if ipv6 {
        setsockopt(socket, sockopt::Ipv6RecvHopLimit, &true)?;
    } else {
        setsockopt(socket, sockopt::Ipv4RecvTtl, &true)?;
    }
    Ok(())
}

/// Sets the TTL / hop limit of the packets this socket sends.
pub(crate) fn set_ttl(socket: &impl AsFd, ipv6: bool, ttl: u8) -> io::Result<()> {
    let ttl = libc::c_int::from(ttl);
    if ipv6 {
        setsockopt(socket, sockopt::Ipv6Ttl, &ttl)?;
    } else {
        setsockopt(socket, sockopt::Ipv4Ttl, &ttl)?;
    }
    Ok(())
}

/// A datagram read with [`recv_with_ttl`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Received {
    pub len: usize,
    pub from: Option<IpAddr>,
    /// Present when [`enable_recv_ttl`] was called on the socket.
    pub ttl: Option<u8>,
}

/// Non-blocking read of one datagram together with its TTL / hop limit.
pub(crate) fn recv_with_ttl(socket: &impl AsRawFd, buffer: &mut [u8]) -> io::Result<Received> {
    let mut control = [0u8; CONTROL_SPACE];
    let mut iov = [IoSliceMut::new(buffer)];
    let message = recvmsg::<SockaddrStorage>(
        socket.as_raw_fd(),
        &mut iov,
        Some(&mut control),
        MsgFlags::MSG_DONTWAIT,
    )?;
    let mut ttl = None;
    if let Ok(control_messages) = message.cmsgs() {
        for control_message in control_messages {
            match control_message {
                ControlMessageOwned::Ipv4Ttl(value) | ControlMessageOwned::Ipv6HopLimit(value) => {
                    ttl = u8::try_from(value).ok();
                }
                _ => {}
            }
        }
    }
    Ok(Received {
        len: message.bytes,
        from: message.address.as_ref().and_then(storage_ip),
        ttl,
    })
}

/// Who generated a queued error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// An ICMP message from the network.
    Icmp,
    /// An ICMPv6 message from the network.
    Icmp6,
    /// The local stack (for example "message too long").
    Local,
    Other,
}

/// One entry of a socket's error queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueuedError {
    pub origin: Origin,
    /// The errno the error maps to (`EHOSTUNREACH`, `ECONNREFUSED`, …).
    pub errno: i32,
    pub icmp_type: u8,
    pub icmp_code: u8,
    /// The node that reported the error (the router whose TTL ran out, the unreachable host…).
    pub offender: Option<IpAddr>,
    /// How much of the packet that caused the error was copied into the caller's buffer.
    pub payload_len: usize,
}

/// Non-blocking read of the next error-queue entry. `WouldBlock` when the queue is empty.
///
/// `buffer` receives the payload of the packet that caused the error (for a ping socket: the
/// ICMP echo request starting at its ICMP header; for UDP: the datagram's payload).
pub(crate) fn recv_error(socket: &impl AsRawFd, buffer: &mut [u8]) -> io::Result<QueuedError> {
    let mut control = [0u8; CONTROL_SPACE];
    let mut iov = [IoSliceMut::new(buffer)];
    let message = recvmsg::<SockaddrStorage>(
        socket.as_raw_fd(),
        &mut iov,
        Some(&mut control),
        MsgFlags::MSG_ERRQUEUE | MsgFlags::MSG_DONTWAIT,
    )?;
    let payload_len = message.bytes;
    for control_message in message.cmsgs()? {
        let (error, offender) = match control_message {
            ControlMessageOwned::Ipv4RecvErr(error, offender) => (
                error,
                offender.map(|address| {
                    IpAddr::V4(Ipv4Addr::from(u32::from_be(address.sin_addr.s_addr)))
                }),
            ),
            ControlMessageOwned::Ipv6RecvErr(error, offender) => (
                error,
                offender.map(|address| IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr))),
            ),
            _ => continue,
        };
        return Ok(QueuedError {
            origin: match error.ee_origin {
                libc::SO_EE_ORIGIN_ICMP => Origin::Icmp,
                libc::SO_EE_ORIGIN_ICMP6 => Origin::Icmp6,
                libc::SO_EE_ORIGIN_LOCAL => Origin::Local,
                _ => Origin::Other,
            },
            errno: i32::try_from(error.ee_errno).unwrap_or(libc::EIO),
            icmp_type: error.ee_type,
            icmp_code: error.ee_code,
            // The kernel leaves an unspecified (all-zero) offender for locally generated errors.
            offender: offender.filter(|address| !address.is_unspecified()),
            payload_len,
        });
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "the socket error queue returned a message without an extended error",
    ))
}

fn storage_ip(address: &SockaddrStorage) -> Option<IpAddr> {
    if let Some(v4) = address.as_sockaddr_in() {
        return Some(IpAddr::V4(v4.ip()));
    }
    address.as_sockaddr_in6().map(|v6| IpAddr::V6(v6.ip()))
}
