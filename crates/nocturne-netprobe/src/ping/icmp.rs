//! ICMP / ICMPv6 echo codec: building echo requests and classifying what comes back.

use std::net::IpAddr;

/// Length of the ICMP echo header (type, code, checksum, identifier, sequence).
pub(crate) const ECHO_HEADER: usize = 8;
/// Largest echo payload that still fits in one IPv4 datagram (65535 - 20 IP - 8 ICMP).
pub(crate) const MAX_PAYLOAD: usize = 65_507;

const V4_ECHO_REPLY: u8 = 0;
const V4_UNREACHABLE: u8 = 3;
const V4_ECHO_REQUEST: u8 = 8;
const V4_TIME_EXCEEDED: u8 = 11;
const V4_PARAMETER_PROBLEM: u8 = 12;

const V6_UNREACHABLE: u8 = 1;
const V6_PACKET_TOO_BIG: u8 = 2;
const V6_TIME_EXCEEDED: u8 = 3;
const V6_PARAMETER_PROBLEM: u8 = 4;
const V6_ECHO_REQUEST: u8 = 128;
const V6_ECHO_REPLY: u8 = 129;

const PROTOCOL_ICMP: u8 = 1;
const PROTOCOL_ICMPV6: u8 = 58;
const IPV6_HEADER: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Family {
    V4,
    V6,
}

impl Family {
    pub(crate) fn of(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(_) => Family::V4,
            IpAddr::V6(_) => Family::V6,
        }
    }

    pub(crate) fn is_v6(self) -> bool {
        self == Family::V6
    }
}

/// The Internet checksum (RFC 1071) of `data`.
pub(crate) fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for pair in &mut chunks {
        sum += u32::from(u16::from_be_bytes([pair[0], pair[1]]));
    }
    if let [last] = chunks.remainder() {
        sum += u32::from(*last) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// The data carried by an echo: `nonce` first (as much of it as fits), then a counting pattern.
pub(crate) fn echo_payload(nonce: u64, len: usize) -> Vec<u8> {
    let mut payload: Vec<u8> = (0..len).map(|index| index as u8).collect();
    let nonce = nonce.to_be_bytes();
    let prefix = len.min(nonce.len());
    payload[..prefix].copy_from_slice(&nonce[..prefix]);
    payload
}

/// An echo request ready to send.
///
/// The IPv4 checksum is filled in. For ICMPv6 the checksum covers a pseudo-header only the
/// kernel knows, and the kernel computes it for every ICMPv6 socket, so it is left zero.
pub(crate) fn echo_request(family: Family, id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(ECHO_HEADER + payload.len());
    packet.push(match family {
        Family::V4 => V4_ECHO_REQUEST,
        Family::V6 => V6_ECHO_REQUEST,
    });
    packet.extend_from_slice(&[0, 0, 0]);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&seq.to_be_bytes());
    packet.extend_from_slice(payload);
    if family == Family::V4 {
        let sum = checksum(&packet);
        packet[2..4].copy_from_slice(&sum.to_be_bytes());
    }
    packet
}

/// Whether a received IPv4 datagram starts with its IP header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IpHeader {
    /// Raw sockets always deliver the IP header.
    Present,
    /// Datagram ICMP sockets deliver it on some systems (macOS) and not on others (Linux).
    Detect,
}

/// What a received ICMP message means to a pinger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Incoming<'a> {
    /// The answer to an echo request.
    EchoReply {
        id: u16,
        seq: u16,
        payload: &'a [u8],
    },
    /// An error about an echo request, which the message quotes.
    Error {
        id: u16,
        seq: u16,
        reason: &'static str,
    },
    /// Not about an echo: other ICMP traffic, our own outgoing request, or garbage.
    Other,
}

/// A parsed message and the TTL of the packet that carried it, when the IP header was visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parsed<'a> {
    pub ttl: Option<u8>,
    pub message: Incoming<'a>,
}

const OTHER: Parsed<'static> = Parsed {
    ttl: None,
    message: Incoming::Other,
};

/// Parses what an IPv4 ICMP socket received.
pub(crate) fn parse_v4(datagram: &[u8], header: IpHeader) -> Parsed<'_> {
    let (ttl, icmp) = match strip_ipv4_header(datagram) {
        Some((ttl, rest)) => (Some(ttl), rest),
        None if header == IpHeader::Present => return OTHER,
        None => (None, datagram),
    };
    let Some((kind, code, body)) = split_echo_header(icmp) else {
        return OTHER;
    };
    let message = match kind {
        V4_ECHO_REPLY if code == 0 => {
            // With the IP header in hand the message is complete, so the checksum can be checked;
            // a corrupted reply must not count as an answer.
            if ttl.is_some() && checksum(icmp) != 0 {
                Incoming::Other
            } else {
                Incoming::EchoReply {
                    id: be16(icmp, 4),
                    seq: be16(icmp, 6),
                    payload: body,
                }
            }
        }
        V4_UNREACHABLE | V4_TIME_EXCEEDED | V4_PARAMETER_PROBLEM => {
            match quoted_echo_v4(body) {
                Some((id, seq)) => Incoming::Error {
                    id,
                    seq,
                    reason: error_text_v4(kind, code),
                },
                None => Incoming::Other,
            }
        }
        _ => Incoming::Other,
    };
    Parsed { ttl, message }
}

/// Parses what an ICMPv6 socket received (never includes the IPv6 header).
pub(crate) fn parse_v6(datagram: &[u8]) -> Parsed<'_> {
    let Some((kind, code, body)) = split_echo_header(datagram) else {
        return OTHER;
    };
    let message = match kind {
        V6_ECHO_REPLY if code == 0 => Incoming::EchoReply {
            id: be16(datagram, 4),
            seq: be16(datagram, 6),
            payload: body,
        },
        V6_UNREACHABLE | V6_PACKET_TOO_BIG | V6_TIME_EXCEEDED | V6_PARAMETER_PROBLEM => {
            match quoted_echo_v6(body) {
                Some((id, seq)) => Incoming::Error {
                    id,
                    seq,
                    reason: error_text_v6(kind, code),
                },
                None => Incoming::Other,
            }
        }
        _ => Incoming::Other,
    };
    Parsed { ttl: None, message }
}

/// Splits off a leading IPv4 header that carries ICMP; returns its TTL and the ICMP message.
fn strip_ipv4_header(datagram: &[u8]) -> Option<(u8, &[u8])> {
    let first = *datagram.first()?;
    let header_len = usize::from(first & 0x0f) * 4;
    if first >> 4 != 4 || header_len < 20 || datagram.len() < header_len {
        return None;
    }
    if datagram[9] != PROTOCOL_ICMP {
        return None;
    }
    Some((datagram[8], &datagram[header_len..]))
}

/// (type, code, everything after the 8-byte header)
fn split_echo_header(icmp: &[u8]) -> Option<(u8, u8, &[u8])> {
    if icmp.len() < ECHO_HEADER {
        return None;
    }
    Some((icmp[0], icmp[1], &icmp[ECHO_HEADER..]))
}

/// The identifier and sequence of the echo request quoted in an ICMP error.
fn quoted_echo_v4(quoted: &[u8]) -> Option<(u16, u16)> {
    let first = *quoted.first()?;
    let header_len = usize::from(first & 0x0f) * 4;
    if first >> 4 != 4 || header_len < 20 || quoted.len() < header_len + ECHO_HEADER {
        return None;
    }
    let inner = &quoted[header_len..];
    (quoted[9] == PROTOCOL_ICMP && inner[0] == V4_ECHO_REQUEST)
        .then(|| (be16(inner, 4), be16(inner, 6)))
}

fn quoted_echo_v6(quoted: &[u8]) -> Option<(u16, u16)> {
    if quoted.len() < IPV6_HEADER + ECHO_HEADER || quoted[0] >> 4 != 6 {
        return None;
    }
    let inner = &quoted[IPV6_HEADER..];
    (quoted[6] == PROTOCOL_ICMPV6 && inner[0] == V6_ECHO_REQUEST)
        .then(|| (be16(inner, 4), be16(inner, 6)))
}

fn be16(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

/// The identifier and sequence number at the start of an echo header, if `icmp` is long enough.
pub(crate) fn echo_ids(icmp: &[u8]) -> Option<(u16, u16)> {
    (icmp.len() >= ECHO_HEADER).then(|| (be16(icmp, 4), be16(icmp, 6)))
}

/// What an ICMPv4 error message says, in the words of RFC 792 / RFC 1812.
pub(crate) fn error_text_v4(kind: u8, code: u8) -> &'static str {
    match (kind, code) {
        (V4_UNREACHABLE, 0) => "network unreachable",
        (V4_UNREACHABLE, 1) => "host unreachable",
        (V4_UNREACHABLE, 2) => "protocol unreachable",
        (V4_UNREACHABLE, 3) => "port unreachable",
        (V4_UNREACHABLE, 4) => "fragmentation needed but the don't-fragment bit is set",
        (V4_UNREACHABLE, 5) => "source route failed",
        (V4_UNREACHABLE, 6) => "destination network unknown",
        (V4_UNREACHABLE, 7) => "destination host unknown",
        (V4_UNREACHABLE, 8) => "source host isolated",
        (V4_UNREACHABLE, 9) => "destination network administratively prohibited",
        (V4_UNREACHABLE, 10) => "destination host administratively prohibited",
        (V4_UNREACHABLE, 11) => "network unreachable for the type of service",
        (V4_UNREACHABLE, 12) => "host unreachable for the type of service",
        (V4_UNREACHABLE, 13) => "communication administratively prohibited (filtered)",
        (V4_UNREACHABLE, 14) => "host precedence violation",
        (V4_UNREACHABLE, 15) => "precedence cutoff in effect",
        (V4_UNREACHABLE, _) => "destination unreachable",
        (V4_TIME_EXCEEDED, 0) => "time to live exceeded in transit",
        (V4_TIME_EXCEEDED, 1) => "fragment reassembly time exceeded",
        (V4_TIME_EXCEEDED, _) => "time exceeded",
        (V4_PARAMETER_PROBLEM, _) => "IP header parameter problem",
        _ => "ICMP error",
    }
}

/// What an ICMPv6 error message says, in the words of RFC 4443.
pub(crate) fn error_text_v6(kind: u8, code: u8) -> &'static str {
    match (kind, code) {
        (V6_UNREACHABLE, 0) => "no route to destination",
        (V6_UNREACHABLE, 1) => "communication administratively prohibited (filtered)",
        (V6_UNREACHABLE, 2) => "beyond scope of the source address",
        (V6_UNREACHABLE, 3) => "address unreachable",
        (V6_UNREACHABLE, 4) => "port unreachable",
        (V6_UNREACHABLE, 5) => "source address failed ingress/egress policy",
        (V6_UNREACHABLE, 6) => "reject route to destination",
        (V6_UNREACHABLE, _) => "destination unreachable",
        (V6_PACKET_TOO_BIG, _) => "packet too big",
        (V6_TIME_EXCEEDED, 0) => "hop limit exceeded in transit",
        (V6_TIME_EXCEEDED, 1) => "fragment reassembly time exceeded",
        (V6_TIME_EXCEEDED, _) => "time exceeded",
        (V6_PARAMETER_PROBLEM, 1) => "unrecognized next header",
        (V6_PARAMETER_PROBLEM, 2) => "unrecognized IPv6 option",
        (V6_PARAMETER_PROBLEM, _) => "IPv6 header parameter problem",
        _ => "ICMPv6 error",
    }
}

/// `true` when an ICMP error means "this is as far as the packet got because its TTL ran out".
pub(crate) fn is_time_exceeded(family: Family, kind: u8) -> bool {
    match family {
        Family::V4 => kind == V4_TIME_EXCEEDED,
        Family::V6 => kind == V6_TIME_EXCEEDED,
    }
}

/// `true` for the ICMP error a closed UDP port answers with.
pub(crate) fn is_port_unreachable(family: Family, kind: u8, code: u8) -> bool {
    match family {
        Family::V4 => kind == V4_UNREACHABLE && code == 3,
        Family::V6 => kind == V6_UNREACHABLE && code == 4,
    }
}

/// The text for an error reported through the Linux error queue.
pub(crate) fn error_text(family: Family, kind: u8, code: u8) -> &'static str {
    match family {
        Family::V4 => error_text_v4(kind, code),
        Family::V6 => error_text_v6(kind, code),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn checksum_matches_rfc_1071_example() {
        // RFC 1071 section 3: the words 0001 f203 f4f5 f6f7 sum to ddf2, checksum 220d.
        let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
        assert_eq!(checksum(&data), 0x220d);
        // An odd trailing byte is padded on the right.
        assert_eq!(checksum(&[0x01]), !0x0100);
        assert_eq!(checksum(&[]), 0xffff);
        // A message including its own checksum sums to zero.
        let mut with_sum = data.to_vec();
        with_sum.extend_from_slice(&0x220du16.to_be_bytes());
        assert_eq!(checksum(&with_sum), 0);
    }

    #[test]
    fn echo_request_layout() {
        let request = echo_request(Family::V4, 0x1234, 0x0001, b"abcdefgh");
        // type 8, code 0, checksum, id, seq, data. Checksum by hand:
        // 0800 + 0000 + 1234 + 0001 + 6162 + 6364 + 6566 + 6768 = 1_abc9 -> abca, complement 5435.
        assert_eq!(
            request,
            [
                0x08, 0x00, 0x54, 0x35, 0x12, 0x34, 0x00, 0x01, b'a', b'b', b'c', b'd', b'e',
                b'f', b'g', b'h'
            ]
        );
        assert_eq!(checksum(&request), 0);

        let v6 = echo_request(Family::V6, 0xbeef, 7, &[]);
        assert_eq!(v6, [128, 0, 0, 0, 0xbe, 0xef, 0, 7]);
    }

    #[test]
    fn payload_carries_the_nonce_then_a_pattern() {
        let payload = echo_payload(0x0102_0304_0506_0708, 12);
        assert_eq!(payload, [1, 2, 3, 4, 5, 6, 7, 8, 8, 9, 10, 11]);
        assert_eq!(echo_payload(0x0102_0304_0506_0708, 3), [1, 2, 3]);
        assert_eq!(echo_payload(1, 0), Vec::<u8>::new());
        assert_eq!(echo_payload(0, 300)[299], 43);
    }

    /// An IPv4 header for an ICMP packet with the given TTL and payload length.
    fn ipv4_header(ttl: u8, source: [u8; 4], destination: [u8; 4], payload_len: usize) -> Vec<u8> {
        let total = (20 + payload_len) as u16;
        let mut header = vec![0x45, 0x00];
        header.extend_from_slice(&total.to_be_bytes());
        header.extend_from_slice(&[0x12, 0x34, 0x00, 0x00, ttl, PROTOCOL_ICMP, 0x00, 0x00]);
        header.extend_from_slice(&source);
        header.extend_from_slice(&destination);
        header
    }

    fn echo_reply_v4(id: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
        let mut reply = echo_request(Family::V4, id, seq, payload);
        reply[0] = V4_ECHO_REPLY;
        reply[2..4].copy_from_slice(&[0, 0]);
        let sum = checksum(&reply);
        reply[2..4].copy_from_slice(&sum.to_be_bytes());
        reply
    }

    #[test]
    fn echo_reply_with_and_without_ip_header() {
        let reply = echo_reply_v4(0x4242, 9, b"nonce123rest");
        let expected = Incoming::EchoReply {
            id: 0x4242,
            seq: 9,
            payload: b"nonce123rest",
        };

        // Linux datagram socket: the bare ICMP message.
        let parsed = parse_v4(&reply, IpHeader::Detect);
        assert_eq!(parsed, Parsed { ttl: None, message: expected.clone() });

        // Raw socket (and macOS datagram socket): IP header first, TTL 57.
        let mut packet = ipv4_header(57, [192, 0, 2, 1], [192, 0, 2, 2], reply.len());
        packet.extend_from_slice(&reply);
        for header in [IpHeader::Present, IpHeader::Detect] {
            let parsed = parse_v4(&packet, header);
            assert_eq!(parsed, Parsed { ttl: Some(57), message: expected.clone() });
        }

        // IP options make the header longer than 20 bytes.
        let mut with_options = packet.clone();
        with_options[0] = 0x46;
        with_options.splice(20..20, [1, 1, 1, 1]);
        assert_eq!(parse_v4(&with_options, IpHeader::Present).message, expected);

        // A raw socket never yields a bare ICMP message.
        assert_eq!(parse_v4(&reply, IpHeader::Present).message, Incoming::Other);
    }

    #[test]
    fn corrupted_reply_is_not_an_answer() {
        let mut reply = echo_reply_v4(1, 1, b"payload!");
        reply[10] ^= 0xff;
        let mut packet = ipv4_header(64, [127, 0, 0, 1], [127, 0, 0, 1], reply.len());
        packet.extend_from_slice(&reply);
        assert_eq!(parse_v4(&packet, IpHeader::Present).message, Incoming::Other);
    }

    #[test]
    fn our_own_request_and_unrelated_traffic_are_ignored() {
        let request = echo_request(Family::V4, 1, 2, b"x");
        let mut packet = ipv4_header(64, [127, 0, 0, 1], [127, 0, 0, 1], request.len());
        packet.extend_from_slice(&request);
        assert_eq!(parse_v4(&packet, IpHeader::Present).message, Incoming::Other);
        // Router advertisement (type 9).
        let mut advert = vec![9, 0, 0, 0, 1, 2, 0, 30];
        let sum = checksum(&advert);
        advert[2..4].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(parse_v4(&advert, IpHeader::Detect).message, Incoming::Other);
        // A UDP packet's IP header is not mistaken for ICMP.
        let mut udp = ipv4_header(64, [10, 0, 0, 1], [10, 0, 0, 2], 8);
        udp[9] = 17;
        udp.extend_from_slice(&[0; 8]);
        assert_eq!(parse_v4(&udp, IpHeader::Present).message, Incoming::Other);
    }

    /// An ICMP error from a router quoting the echo request (id, seq) sent to 198.51.100.7.
    fn icmp_error_v4(kind: u8, code: u8, id: u16, seq: u16) -> Vec<u8> {
        let request = echo_request(Family::V4, id, seq, b"12345678");
        let mut message = vec![kind, code, 0, 0, 0, 0, 0, 0];
        message.extend_from_slice(&ipv4_header(1, [192, 0, 2, 2], [198, 51, 100, 7], request.len()));
        // RFC 792 only guarantees the first 8 bytes of the offending datagram.
        message.extend_from_slice(&request[..8]);
        let sum = checksum(&message);
        message[2..4].copy_from_slice(&sum.to_be_bytes());
        let mut packet = ipv4_header(254, [10, 9, 8, 7], [192, 0, 2, 2], message.len());
        packet.extend_from_slice(&message);
        packet
    }

    #[test]
    fn errors_quote_the_echo_they_are_about() {
        let cases = [
            (3, 1, "host unreachable"),
            (3, 0, "network unreachable"),
            (3, 13, "communication administratively prohibited (filtered)"),
            (3, 4, "fragmentation needed but the don't-fragment bit is set"),
            (3, 99, "destination unreachable"),
            (11, 0, "time to live exceeded in transit"),
            (12, 0, "IP header parameter problem"),
        ];
        for (kind, code, reason) in cases {
            let packet = icmp_error_v4(kind, code, 0x0a0b, 0x0c0d);
            let parsed = parse_v4(&packet, IpHeader::Present);
            assert_eq!(
                parsed,
                Parsed {
                    ttl: Some(254),
                    message: Incoming::Error { id: 0x0a0b, seq: 0x0c0d, reason }
                },
                "type {kind} code {code}"
            );
        }
        // Redirects and source quench are not failures of the echo.
        assert_eq!(parse_v4(&icmp_error_v4(5, 1, 1, 1), IpHeader::Present).message, Incoming::Other);
        assert_eq!(parse_v4(&icmp_error_v4(4, 0, 1, 1), IpHeader::Present).message, Incoming::Other);
    }

    #[test]
    fn error_about_someone_elses_udp_packet_is_ignored() {
        let mut packet = icmp_error_v4(3, 3, 1, 1);
        // Inner protocol UDP instead of ICMP: outer header 20 + ICMP header 8 + inner offset 9.
        packet[20 + 8 + 9] = 17;
        assert_eq!(parse_v4(&packet, IpHeader::Present).message, Incoming::Other);
    }

    fn ipv6_header(next_header: u8, payload_len: usize) -> Vec<u8> {
        let mut header = vec![0x60, 0, 0, 0];
        header.extend_from_slice(&(payload_len as u16).to_be_bytes());
        header.extend_from_slice(&[next_header, 64]);
        header.extend_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        header.extend_from_slice(&[0x20, 0x01, 0x0d, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2]);
        header
    }

    #[test]
    fn icmpv6_reply_and_errors() {
        let mut reply = echo_request(Family::V6, 0x7777, 3, b"sixsixsix");
        reply[0] = V6_ECHO_REPLY;
        reply[2..4].copy_from_slice(&[0xab, 0xcd]);
        assert_eq!(
            parse_v6(&reply),
            Parsed {
                ttl: None,
                message: Incoming::EchoReply { id: 0x7777, seq: 3, payload: b"sixsixsix" }
            }
        );

        let request = echo_request(Family::V6, 0x7777, 4, b"sixsixsix");
        for (kind, code, reason) in [
            (1, 0, "no route to destination"),
            (1, 3, "address unreachable"),
            (1, 1, "communication administratively prohibited (filtered)"),
            (2, 0, "packet too big"),
            (3, 0, "hop limit exceeded in transit"),
            (4, 1, "unrecognized next header"),
        ] {
            let mut error = vec![kind, code, 0, 0, 0, 0, 0, 0];
            error.extend_from_slice(&ipv6_header(PROTOCOL_ICMPV6, request.len()));
            error.extend_from_slice(&request);
            assert_eq!(
                parse_v6(&error).message,
                Incoming::Error { id: 0x7777, seq: 4, reason },
                "type {kind} code {code}"
            );
        }

        // Neighbour solicitation, our own request, and an error quoting a TCP packet.
        assert_eq!(parse_v6(&[135, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3]).message, Incoming::Other);
        assert_eq!(parse_v6(&request).message, Incoming::Other);
        let mut about_tcp = vec![1, 4, 0, 0, 0, 0, 0, 0];
        about_tcp.extend_from_slice(&ipv6_header(6, 20));
        about_tcp.extend_from_slice(&[0; 20]);
        assert_eq!(parse_v6(&about_tcp).message, Incoming::Other);
    }

    #[test]
    fn truncated_and_random_input_never_panics() {
        let reply = {
            let reply = echo_reply_v4(1, 2, b"0123456789abcdef");
            let mut packet = ipv4_header(64, [1, 2, 3, 4], [5, 6, 7, 8], reply.len());
            packet.extend_from_slice(&reply);
            packet
        };
        let error = icmp_error_v4(3, 1, 1, 2);
        for packet in [&reply, &error] {
            for cut in 0..packet.len() {
                let _ = parse_v4(&packet[..cut], IpHeader::Present);
                let _ = parse_v4(&packet[..cut], IpHeader::Detect);
                let _ = parse_v6(&packet[..cut]);
            }
        }
        let mut state = 0x2545_f491_4f6c_dd1du64;
        for _ in 0..20_000 {
            let mut junk = vec![0u8; (state % 120) as usize];
            for byte in &mut junk {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            let _ = parse_v4(&junk, IpHeader::Present);
            let _ = parse_v4(&junk, IpHeader::Detect);
            let _ = parse_v6(&junk);
            let _ = echo_ids(&junk);
        }
    }

    #[test]
    fn error_queue_classification() {
        assert!(is_time_exceeded(Family::V4, 11));
        assert!(is_time_exceeded(Family::V6, 3));
        assert!(!is_time_exceeded(Family::V4, 3));
        assert!(is_port_unreachable(Family::V4, 3, 3));
        assert!(is_port_unreachable(Family::V6, 1, 4));
        assert!(!is_port_unreachable(Family::V4, 3, 1));
        assert_eq!(error_text(Family::V4, 3, 1), "host unreachable");
        assert_eq!(error_text(Family::V6, 1, 3), "address unreachable");
    }
}
