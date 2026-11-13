//! SNTP (RFC 4330): ask a time server what time it is and how far this machine's clock is off.

use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;

use crate::util::{self, round3};
use crate::{ProbeError, dns};

/// The answer of a time server.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NtpReport {
    /// The server as it was given.
    pub server: String,
    /// The address and port that answered.
    pub address: String,
    /// Distance from a reference clock: 1 is a primary server, 16 means unsynchronised.
    pub stratum: u8,
    /// What the server is synchronised to: the name of its clock (stratum 1, e.g. `GPS`) or the
    /// address of its upstream server.
    pub reference: String,
    /// `none`, `insert_second`, `delete_second`, or `unsynchronized` (the server's clock is not
    /// set; do not trust its time).
    pub leap: String,
    pub version: u8,
    /// How far the local clock is behind (positive) or ahead of (negative) the server's.
    pub offset_ms: f64,
    /// Round-trip network delay of the exchange.
    pub delay_ms: f64,
    /// The server's own distance to its reference clock.
    pub root_delay_ms: f64,
    pub root_dispersion_ms: f64,
    /// The server's clock when it sent the reply, as Unix seconds.
    pub server_time_unix: f64,
}

const PACKET_LEN: usize = 48;
const DEFAULT_PORT: u16 = 123;
/// Seconds between the NTP epoch (1900) and the Unix epoch (1970).
const UNIX_OFFSET: f64 = 2_208_988_800.0;
/// Seconds per NTP era (2^32).
const ERA: f64 = 4_294_967_296.0;
const MODE_CLIENT: u8 = 3;
const MODE_SERVER: u8 = 4;
const VERSION: u8 = 4;

/// One SNTP exchange with `server` (a host or address, optionally with `:port`).
///
/// `local_unix` is the caller's clock reading (Unix seconds) at the moment of the call; the
/// time of the reply's arrival is derived from it with a monotonic clock, so this function
/// never reads the wall clock itself.
pub async fn query(
    server: &str,
    timeout: Duration,
    local_unix: f64,
) -> Result<NtpReport, ProbeError> {
    if !local_unix.is_finite() || local_unix < 0.0 {
        return Err(ProbeError::invalid(
            "the local time must be a Unix timestamp in seconds (a non-negative number)",
        ));
    }
    let started = Instant::now();
    let (host, port) = util::split_host_port(server, DEFAULT_PORT)?;
    let address = SocketAddr::new(dns::resolve_one(&host).await?, port);
    let local: SocketAddr = match address {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(local).await?;
    socket
        .connect(address)
        .await
        .map_err(ProbeError::from_network_io)?;

    // T1: the local clock at transmission (name resolution took part of the time since the call).
    let sent_at = Instant::now();
    let t1 = local_unix + sent_at.duration_since(started).as_secs_f64();
    let request = request(t1);
    socket
        .send(&request)
        .await
        .map_err(ProbeError::from_network_io)?;

    let exchange = async {
        let mut buffer = [0u8; 1024];
        loop {
            let length = socket
                .recv(&mut buffer)
                .await
                .map_err(ProbeError::from_network_io)?;
            let t4 = t1 + sent_at.elapsed().as_secs_f64();
            // A reply to someone else's (or an earlier) request does not echo our timestamp.
            if let Some(reply) = parse_reply(&buffer[..length], &request[40..48])? {
                return Ok::<_, ProbeError>((reply, t4));
            }
        }
    };
    let remaining = timeout.saturating_sub(started.elapsed());
    let (reply, t4) = tokio::time::timeout(remaining, exchange)
        .await
        .map_err(|_| ProbeError::Timeout)??;

    // RFC 4330 section 5: T2 and T3 are the server's receive and transmit times.
    let offset = ((reply.receive - t1) + (reply.transmit - t4)) / 2.0;
    let delay = ((t4 - t1) - (reply.transmit - reply.receive)).max(0.0);
    Ok(NtpReport {
        server: server.trim().to_owned(),
        address: address.to_string(),
        stratum: reply.stratum,
        reference: reply.reference,
        leap: reply.leap.to_owned(),
        version: reply.version,
        offset_ms: round3(offset * 1000.0),
        delay_ms: round3(delay * 1000.0),
        root_delay_ms: round3(reply.root_delay * 1000.0),
        root_dispersion_ms: round3(reply.root_dispersion * 1000.0),
        server_time_unix: reply.transmit,
    })
}

/// A client request carrying `transmit_unix` as its transmit timestamp.
fn request(transmit_unix: f64) -> [u8; PACKET_LEN] {
    let mut packet = [0u8; PACKET_LEN];
    packet[0] = (VERSION << 3) | MODE_CLIENT;
    packet[40..48].copy_from_slice(&to_ntp_timestamp(transmit_unix));
    packet
}

/// Unix seconds as a 64-bit NTP timestamp (32 bits of seconds since 1900, 32 bits of fraction).
/// Times after 2036 wrap into the next era, as on the wire.
fn to_ntp_timestamp(unix: f64) -> [u8; 8] {
    let ntp = unix + UNIX_OFFSET;
    let seconds = ntp.floor();
    let fraction = ((ntp - seconds) * ERA) as u32;
    let seconds = (seconds % ERA) as u32;
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&seconds.to_be_bytes());
    bytes[4..].copy_from_slice(&fraction.to_be_bytes());
    bytes
}

/// A 64-bit NTP timestamp as Unix seconds; `None` for the "unknown" timestamp (all zero).
fn from_ntp_timestamp(bytes: &[u8]) -> Option<f64> {
    let seconds = u32::from_be_bytes(bytes[..4].try_into().ok()?);
    let fraction = u32::from_be_bytes(bytes[4..8].try_into().ok()?);
    if seconds == 0 && fraction == 0 {
        return None;
    }
    // RFC 4330 section 3: a clear top bit means the era that begins in February 2036.
    let era = if seconds & 0x8000_0000 == 0 { ERA } else { 0.0 };
    Some(f64::from(seconds) + era + f64::from(fraction) / ERA - UNIX_OFFSET)
}

/// 16.16 fixed-point seconds.
fn short_format(bytes: &[u8]) -> f64 {
    f64::from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])) / 65_536.0
}

/// The fields of a server reply this module reports.
#[derive(Debug, Clone, PartialEq)]
struct Reply {
    leap: &'static str,
    version: u8,
    stratum: u8,
    root_delay: f64,
    root_dispersion: f64,
    reference: String,
    /// T2 and T3 as Unix seconds.
    receive: f64,
    transmit: f64,
}

/// Decodes a reply to the request whose transmit timestamp was `sent_timestamp`.
///
/// `Ok(None)` for a datagram that is not the answer to that request (wrong mode or not echoing
/// the timestamp); `Err` for an answer that says the server will not or cannot tell the time.
fn parse_reply(datagram: &[u8], sent_timestamp: &[u8]) -> Result<Option<Reply>, ProbeError> {
    if datagram.len() < PACKET_LEN {
        return Ok(None);
    }
    let mode = datagram[0] & 0x07;
    if mode != MODE_SERVER || &datagram[24..32] != sent_timestamp {
        return Ok(None);
    }
    let stratum = datagram[1];
    let reference_id = &datagram[12..16];
    if stratum == 0 {
        // Kiss-o'-death: the reference identifier is a four-letter reason.
        let code: String = reference_id
            .iter()
            .filter(|byte| byte.is_ascii_graphic())
            .map(|byte| *byte as char)
            .collect();
        let meaning = match code.as_str() {
            "RATE" => "the server is rate-limiting this client; query less often",
            "DENY" | "RSTR" => "the server's access policy denies this client",
            "INIT" | "STEP" => "the server has not synchronised its own clock yet",
            _ => "the server declined to answer",
        };
        return Err(ProbeError::protocol(format!(
            "the NTP server answered with kiss-o'-death code {code:?}: {meaning}"
        )));
    }
    let (Some(receive), Some(transmit)) = (
        from_ntp_timestamp(&datagram[32..40]),
        from_ntp_timestamp(&datagram[40..48]),
    ) else {
        return Err(ProbeError::protocol(
            "the NTP server's reply carries no time (its clock is not set)",
        ));
    };
    let reference = if stratum == 1 {
        // A primary server names its clock in ASCII: "GPS", "PPS", "DCF"…
        reference_id
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| {
                if byte.is_ascii_graphic() {
                    *byte as char
                } else {
                    '?'
                }
            })
            .collect()
    } else {
        // A secondary server gives the IPv4 address of its upstream (or a hash of an IPv6 one).
        Ipv4Addr::new(
            reference_id[0],
            reference_id[1],
            reference_id[2],
            reference_id[3],
        )
        .to_string()
    };
    Ok(Some(Reply {
        leap: match datagram[0] >> 6 {
            0 => "none",
            1 => "insert_second",
            2 => "delete_second",
            _ => "unsynchronized",
        },
        version: (datagram[0] >> 3) & 0x07,
        stratum,
        root_delay: short_format(&datagram[4..8]),
        root_dispersion: short_format(&datagram[8..12]),
        reference,
        receive,
        transmit,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn request_layout() {
        let packet = request(1_700_000_000.5);
        // LI 0, version 4, mode 3 (client); everything but the transmit timestamp is zero.
        assert_eq!(packet[0], 0x23);
        assert!(packet[1..40].iter().all(|byte| *byte == 0));
        // 1700000000 + 2208988800 = 3908988800 = 0xe8fe6f80; half a second = 0x80000000.
        assert_eq!(
            packet[40..48],
            [0xe8, 0xfe, 0x6f, 0x80, 0x80, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn timestamps_round_trip_including_the_2036_era() {
        // The Unix epoch is 2208988800 = 0x83aa7e80 seconds into the NTP era.
        assert_eq!(to_ntp_timestamp(0.0), [0x83, 0xaa, 0x7e, 0x80, 0, 0, 0, 0]);
        assert_eq!(
            from_ntp_timestamp(&[0x83, 0xaa, 0x7e, 0x80, 0, 0, 0, 0]),
            Some(0.0)
        );
        assert_eq!(
            from_ntp_timestamp(&[0x83, 0xaa, 0x82, 0x68, 0x40, 0, 0, 0]),
            Some(1000.25)
        );
        // 10.25 s into era 1 (7 February 2036 06:28:16 UTC + 10.25 s = Unix 2085978506.25).
        let wrapped = to_ntp_timestamp(2_085_978_506.25);
        assert_eq!(wrapped, [0, 0, 0, 0x0a, 0x40, 0, 0, 0]);
        assert_eq!(from_ntp_timestamp(&wrapped), Some(2_085_978_506.25));
        // The last second of era 0.
        assert_eq!(
            from_ntp_timestamp(&[0xff, 0xff, 0xff, 0xff, 0, 0, 0, 0]),
            Some(2_085_978_495.0)
        );
        // All zero means "unknown" on the wire (it is also the first instant of era 1).
        assert_eq!(from_ntp_timestamp(&[0; 8]), None);
        assert_eq!(to_ntp_timestamp(2_085_978_496.0), [0; 8]);
        for unix in [
            1.0,
            1_700_000_000.123,
            2_085_978_495.9,
            2_085_978_496.5,
            4_000_000_000.75,
        ] {
            let back = from_ntp_timestamp(&to_ntp_timestamp(unix)).unwrap();
            assert!((back - unix).abs() < 1e-6, "{unix} -> {back}");
        }
    }

    /// A server reply built field by field.
    fn reply(
        first_byte: u8,
        stratum: u8,
        reference: [u8; 4],
        originate: &[u8],
        receive: f64,
        transmit: f64,
    ) -> Vec<u8> {
        let mut packet = vec![first_byte, stratum, 6, 0xe9];
        packet.extend_from_slice(&0x0000_0400u32.to_be_bytes()); // root delay 1/64 s
        packet.extend_from_slice(&0x0000_8000u32.to_be_bytes()); // root dispersion 1/2 s
        packet.extend_from_slice(&reference);
        packet.extend_from_slice(&to_ntp_timestamp(receive - 10.0)); // reference timestamp
        packet.extend_from_slice(originate);
        packet.extend_from_slice(&to_ntp_timestamp(receive));
        packet.extend_from_slice(&to_ntp_timestamp(transmit));
        packet
    }

    #[test]
    fn decodes_a_stratum_2_reply() {
        let sent = to_ntp_timestamp(1000.0);
        // LI 0, version 4, mode 4.
        let packet = reply(0x24, 2, [192, 0, 2, 1], &sent, 1000.150, 1000.151);
        let parsed = parse_reply(&packet, &sent).unwrap().unwrap();
        assert_eq!(parsed.leap, "none");
        assert_eq!(parsed.version, 4);
        assert_eq!(parsed.stratum, 2);
        assert_eq!(parsed.reference, "192.0.2.1");
        assert_eq!(parsed.root_delay, 0.015_625);
        assert_eq!(parsed.root_dispersion, 0.5);
        assert!((parsed.receive - 1000.150).abs() < 1e-6);
        assert!((parsed.transmit - 1000.151).abs() < 1e-6);
    }

    #[test]
    fn decodes_primary_servers_leap_warnings_and_old_versions() {
        let sent = to_ntp_timestamp(5.0);
        // LI 1 (a leap second will be inserted), version 3, mode 4, stratum 1 with a GPS clock.
        let packet = reply(0b01_011_100, 1, *b"GPS\0", &sent, 5.1, 5.2);
        let parsed = parse_reply(&packet, &sent).unwrap().unwrap();
        assert_eq!(
            (parsed.leap, parsed.version, parsed.stratum),
            ("insert_second", 3, 1)
        );
        assert_eq!(parsed.reference, "GPS");

        let alarm = reply(0b11_100_100, 16, [0; 4], &sent, 5.1, 5.2);
        let parsed = parse_reply(&alarm, &sent).unwrap().unwrap();
        assert_eq!((parsed.leap, parsed.stratum), ("unsynchronized", 16));
        let delete = reply(0b10_100_100, 3, [10, 0, 0, 1], &sent, 5.1, 5.2);
        assert_eq!(
            parse_reply(&delete, &sent).unwrap().unwrap().leap,
            "delete_second"
        );
    }

    #[test]
    fn replies_to_other_requests_are_ignored() {
        let sent = to_ntp_timestamp(1000.0);
        let other = to_ntp_timestamp(999.0);
        assert_eq!(
            parse_reply(&reply(0x24, 2, [192, 0, 2, 1], &other, 1.0, 2.0), &sent).unwrap(),
            None
        );
        // A client-mode or broadcast packet is not an answer either.
        assert_eq!(
            parse_reply(&reply(0x23, 2, [192, 0, 2, 1], &sent, 1.0, 2.0), &sent).unwrap(),
            None
        );
        assert_eq!(
            parse_reply(&reply(0x25, 2, [192, 0, 2, 1], &sent, 1.0, 2.0), &sent).unwrap(),
            None
        );
        // Too short to be NTP.
        assert_eq!(
            parse_reply(
                &reply(0x24, 2, [192, 0, 2, 1], &sent, 1.0, 2.0)[..47],
                &sent
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn kiss_of_death_and_unset_clocks_are_errors() {
        let sent = to_ntp_timestamp(1000.0);
        let rate = reply(0xe4, 0, *b"RATE", &sent, 1000.1, 1000.2);
        let error = parse_reply(&rate, &sent).unwrap_err().to_string();
        assert!(
            error.contains("\"RATE\"") && error.contains("rate-limiting"),
            "{error}"
        );
        let deny = reply(0xe4, 0, *b"DENY", &sent, 1000.1, 1000.2);
        assert!(
            parse_reply(&deny, &sent)
                .unwrap_err()
                .to_string()
                .contains("access policy")
        );

        let mut unset = reply(0xe4, 16, [0; 4], &sent, 1000.1, 1000.2);
        unset[40..48].fill(0);
        let error = parse_reply(&unset, &sent).unwrap_err();
        assert!(matches!(error, ProbeError::Protocol(_)), "{error:?}");
    }

    #[test]
    fn truncated_and_random_replies_never_panic() {
        let sent = to_ntp_timestamp(1000.0);
        let good = reply(0x24, 2, [192, 0, 2, 1], &sent, 1000.150, 1000.151);
        for cut in 0..=good.len() {
            let _ = parse_reply(&good[..cut], &sent);
        }
        let mut state = 0x853c_49e6_748f_ea9bu64;
        for round in 0..50_000 {
            let mut junk = vec![0u8; (state % 100) as usize];
            for byte in &mut junk {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            // Half of the time make it look like an answer so the field decoding is reached.
            if round % 2 == 0 && junk.len() >= PACKET_LEN {
                junk[0] = (junk[0] & 0xf8) | MODE_SERVER;
                junk[24..32].copy_from_slice(&sent);
            }
            if let Ok(Some(reply)) = parse_reply(&junk, &sent) {
                assert!(reply.receive.is_finite() && reply.transmit.is_finite());
            }
        }
    }

    /// A loopback SNTP server: answers each request `think` after receiving it, with its clock
    /// `ahead` seconds ahead of the client's (which it learns from the request).
    async fn fake_server(ahead: f64, think: Duration, send_stray_first: bool) -> SocketAddr {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = socket.local_addr().unwrap();
        tokio::spawn(async move {
            let mut buffer = [0u8; 128];
            while let Ok((length, peer)) = socket.recv_from(&mut buffer).await {
                assert_eq!(length, PACKET_LEN);
                assert_eq!(buffer[0], 0x23);
                let client_time = from_ntp_timestamp(&buffer[40..48]).unwrap();
                let originate = buffer[40..48].to_vec();
                if send_stray_first {
                    // A late answer to some earlier request.
                    let stray = reply(0x24, 2, [192, 0, 2, 1], &to_ntp_timestamp(1.0), 7.0, 8.0);
                    socket.send_to(&stray, peer).await.unwrap();
                }
                tokio::time::sleep(think).await;
                let receive = client_time + ahead;
                let transmit = receive + think.as_secs_f64();
                let answer = reply(0x24, 2, [192, 0, 2, 1], &originate, receive, transmit);
                socket.send_to(&answer, peer).await.unwrap();
            }
        });
        address
    }

    #[tokio::test]
    async fn query_computes_offset_and_delay() {
        // The server's clock is 2.5 s ahead and it takes 50 ms to answer.
        let server = fake_server(2.5, Duration::from_millis(50), true).await;
        let report = query(&server.to_string(), Duration::from_secs(2), 1_700_000_000.0)
            .await
            .unwrap();
        assert_eq!(report.server, server.to_string());
        assert_eq!(report.address, server.to_string());
        assert_eq!((report.stratum, report.version), (2, 4));
        assert_eq!(report.reference, "192.0.2.1");
        assert_eq!(report.leap, "none");
        assert_eq!(report.root_delay_ms, 15.625);
        assert_eq!(report.root_dispersion_ms, 500.0);
        // On loopback the network delay is far below the tolerance.
        assert!(
            (report.offset_ms - 2500.0).abs() < 20.0,
            "offset {}",
            report.offset_ms
        );
        assert!(
            report.delay_ms >= 0.0 && report.delay_ms < 20.0,
            "delay {}",
            report.delay_ms
        );
        assert!(
            (report.server_time_unix - 1_700_000_002.55).abs() < 0.02,
            "{}",
            report.server_time_unix
        );
    }

    #[tokio::test]
    async fn a_clock_that_is_ahead_gets_a_negative_offset() {
        let server = fake_server(-3600.0, Duration::ZERO, false).await;
        let report = query(&server.to_string(), Duration::from_secs(2), 1_700_000_000.0)
            .await
            .unwrap();
        assert!(
            (report.offset_ms + 3_600_000.0).abs() < 20.0,
            "offset {}",
            report.offset_ms
        );
    }

    #[tokio::test]
    async fn silence_times_out_and_a_closed_port_is_refused() {
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = silent.local_addr().unwrap().to_string();
        let started = Instant::now();
        let error = query(&address, Duration::from_millis(300), 1_700_000_000.0)
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Timeout), "{error:?}");
        assert!(started.elapsed() >= Duration::from_millis(290));
        assert!(started.elapsed() < Duration::from_secs(2));

        drop(silent);
        let error = query(&address, Duration::from_secs(2), 1_700_000_000.0)
            .await
            .unwrap_err();
        assert!(matches!(error, ProbeError::Refused), "{error:?}");
    }

    #[tokio::test]
    async fn bad_input_is_rejected() {
        for server in ["", "ntp.example:0", "ntp.example:port", "two words"] {
            let error = query(server, Duration::from_secs(1), 1_700_000_000.0)
                .await
                .unwrap_err();
            assert!(
                matches!(error, ProbeError::Invalid(_)),
                "{server:?}: {error:?}"
            );
        }
        for local in [f64::NAN, f64::INFINITY, -1.0] {
            let error = query("127.0.0.1", Duration::from_secs(1), local)
                .await
                .unwrap_err();
            assert!(
                matches!(error, ProbeError::Invalid(_)),
                "{local}: {error:?}"
            );
        }
        assert!(matches!(
            query("no-such-host.invalid", Duration::from_secs(1), 0.0).await,
            Err(ProbeError::Resolve)
        ));
    }
}
