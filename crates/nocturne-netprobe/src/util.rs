//! Small helpers shared by the probes: timing, statistics, address and MAC parsing.

use std::net::IpAddr;
use std::time::Duration;

use crate::ProbeError;

/// A duration as fractional milliseconds, rounded to the microsecond.
pub(crate) fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.0).round() / 1000.0
}

/// A random 64-bit value from the operating system.
pub(crate) fn random_u64() -> u64 {
    getrandom::u64().unwrap_or_else(|_| {
        // The OS generator is unavailable only on broken systems; the identifiers this feeds
        // need to be unpredictable enough to tell replies apart, not secret.
        use std::hash::{BuildHasher, Hasher};
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    })
}

/// min / avg / max / population standard deviation of a set of samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Summary {
    pub min: f64,
    pub avg: f64,
    pub max: f64,
    pub stddev: f64,
}

pub(crate) fn summarize(samples: &[f64]) -> Option<Summary> {
    if samples.is_empty() {
        return None;
    }
    let count = samples.len() as f64;
    let avg = samples.iter().sum::<f64>() / count;
    let variance = samples.iter().map(|x| (x - avg).powi(2)).sum::<f64>() / count;
    Some(Summary {
        min: samples.iter().copied().fold(f64::INFINITY, f64::min),
        avg: round3(avg),
        max: samples.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        stddev: round3(variance.sqrt()),
    })
}

pub(crate) fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// Formats a Unix timestamp as `YYYY-MM-DDTHH:MM:SSZ`.
pub(crate) fn format_unix_utc(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let second_of_day = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        second_of_day % 3600 / 60,
        second_of_day % 60
    )
}

/// Days since 1970-01-01 to a proleptic Gregorian (year, month, day).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    // Howard Hinnant's algorithm: shift the epoch to 0000-03-01 so leap days fall at the end
    // of each 400-year era.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// Checks that `host` can be a host name or an address at all.
pub(crate) fn validate_host(host: &str) -> Result<&str, ProbeError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(ProbeError::invalid(
            "the host is empty; enter a host name or an IP address",
        ));
    }
    if host.len() > 253 {
        return Err(ProbeError::invalid(format!(
            "the host name is {} characters long; DNS names are limited to 253",
            host.len()
        )));
    }
    if let Some(bad) = host
        .chars()
        .find(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '@' | ','))
    {
        return Err(ProbeError::invalid(format!(
            "'{host}' is not a host name or an IP address (unexpected character {bad:?})"
        )));
    }
    if host.starts_with('-') {
        return Err(ProbeError::invalid(format!(
            "'{host}' is not a host name or an IP address (it starts with '-')"
        )));
    }
    Ok(host)
}

/// Parses a literal IPv4 or IPv6 address, tolerating `[brackets]` and an IPv6 zone (`%eth0`).
pub(crate) fn parse_ip_literal(host: &str) -> Option<IpAddr> {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(address) = host.parse::<IpAddr>() {
        return Some(address);
    }
    let (address, _zone) = host.split_once('%')?;
    address.parse::<std::net::Ipv6Addr>().ok().map(IpAddr::V6)
}

/// Splits `host`, `host:port`, `[v6]` and `[v6]:port`. A bare IPv6 address is a host.
pub(crate) fn split_host_port(input: &str, default_port: u16) -> Result<(String, u16), ProbeError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ProbeError::invalid(
            "the host is empty; enter a host name or an IP address",
        ));
    }
    let parse_port = |text: &str| {
        text.parse::<u16>()
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| {
                ProbeError::invalid(format!(
                    "'{text}' in '{input}' is not a port number (expected 1-65535)"
                ))
            })
    };
    if let Some(rest) = input.strip_prefix('[') {
        let Some((host, after)) = rest.split_once(']') else {
            return Err(ProbeError::invalid(format!(
                "'{input}' has an opening '[' without a closing ']'"
            )));
        };
        let port = match after.strip_prefix(':') {
            Some(port) => parse_port(port)?,
            None if after.is_empty() => default_port,
            None => {
                return Err(ProbeError::invalid(format!(
                    "'{input}' has unexpected text after ']' (expected ':port')"
                )));
            }
        };
        return Ok((validate_host(host)?.to_owned(), port));
    }
    match input.rsplit_once(':') {
        // More than one colon without brackets is an IPv6 address, not host:port.
        Some((host, port)) if !host.contains(':') => {
            Ok((validate_host(host)?.to_owned(), parse_port(port)?))
        }
        _ => Ok((validate_host(input)?.to_owned(), default_port)),
    }
}

/// Parses a MAC address written as `aa:bb:cc:dd:ee:ff`, `AA-BB-CC-DD-EE-FF`, `aabb.ccdd.eeff`,
/// `aabbccddeeff`, or with single-digit groups as macOS prints them (`1:0:5e:0:0:fb`).
pub(crate) fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let text = text.trim();
    let mut mac = [0u8; 6];
    if text.contains([':', '-']) {
        let mut groups = text.split([':', '-']);
        for byte in &mut mac {
            let group = groups.next()?;
            if group.is_empty() || group.len() > 2 {
                return None;
            }
            *byte = u8::from_str_radix(group, 16).ok()?;
        }
        return groups.next().is_none().then_some(mac);
    }
    let digits: String = text.chars().filter(|c| *c != '.').collect();
    let dotted_ok = !text.contains('.') || text.split('.').all(|group| group.len() == 4);
    if digits.len() != 12 || !dotted_ok || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    for (index, byte) in mac.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(mac)
}

/// `aa:bb:cc:dd:ee:ff`
pub(crate) fn format_mac(mac: [u8; 6]) -> String {
    mac.map(|byte| format!("{byte:02x}")).join(":")
}

/// Makes text received from the network safe to show: control characters become spaces, runs of
/// whitespace collapse, and the result is cut at `max_chars`.
pub(crate) fn printable(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() || c == char::REPLACEMENT_CHARACTER {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
        if out.chars().count() >= max_chars {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn millis_rounds_to_microseconds() {
        assert_eq!(millis(Duration::from_micros(1_234)), 1.234);
        assert_eq!(millis(Duration::from_nanos(1_234_567)), 1.235);
        assert_eq!(millis(Duration::ZERO), 0.0);
    }

    #[test]
    fn summary_matches_hand_computed_values() {
        // 2, 4, 4, 4, 5, 5, 7, 9: mean 5, population standard deviation exactly 2.
        let summary = summarize(&[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).unwrap();
        assert_eq!(
            summary,
            Summary {
                min: 2.0,
                avg: 5.0,
                max: 9.0,
                stddev: 2.0
            }
        );
        assert_eq!(summarize(&[]), None);
        let single = summarize(&[0.75]).unwrap();
        assert_eq!((single.min, single.max, single.stddev), (0.75, 0.75, 0.0));
    }

    #[test]
    fn unix_time_formats_as_iso_8601() {
        assert_eq!(format_unix_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix_utc(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_unix_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(format_unix_utc(4_102_444_799), "2099-12-31T23:59:59Z");
        assert_eq!(format_unix_utc(-1), "1969-12-31T23:59:59Z");
        // 2100 is not a leap year: the day after 28 February is 1 March.
        assert_eq!(format_unix_utc(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn host_port_forms() {
        assert_eq!(
            split_host_port("sw1.lab", 53).unwrap(),
            ("sw1.lab".into(), 53)
        );
        assert_eq!(
            split_host_port("10.0.0.1:5353", 53).unwrap(),
            ("10.0.0.1".into(), 5353)
        );
        assert_eq!(
            split_host_port("2001:db8::1", 53).unwrap(),
            ("2001:db8::1".into(), 53)
        );
        assert_eq!(
            split_host_port("[2001:db8::1]", 53).unwrap(),
            ("2001:db8::1".into(), 53)
        );
        assert_eq!(
            split_host_port("[2001:db8::1]:853", 53).unwrap(),
            ("2001:db8::1".into(), 853)
        );
        for bad in [
            "",
            "host:",
            "host:0",
            "host:70000",
            "[::1",
            "[::1]x",
            "a b",
            "-n",
        ] {
            assert!(
                matches!(split_host_port(bad, 53), Err(ProbeError::Invalid(_))),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn ip_literals() {
        assert_eq!(parse_ip_literal("10.1.2.3"), Some("10.1.2.3".parse().unwrap()));
        assert_eq!(parse_ip_literal("[::1]"), Some("::1".parse().unwrap()));
        assert_eq!(
            parse_ip_literal("fe80::1%eth0"),
            Some("fe80::1".parse().unwrap())
        );
        assert_eq!(parse_ip_literal("10.1.2"), None);
        assert_eq!(parse_ip_literal("example.com"), None);
        assert_eq!(parse_ip_literal("10.0.0.1%eth0"), None);
    }

    #[test]
    fn mac_formats() {
        let expected = Some([0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]);
        assert_eq!(parse_mac("00:1a:2b:3c:4d:5e"), expected);
        assert_eq!(parse_mac("00-1A-2B-3C-4D-5E"), expected);
        assert_eq!(parse_mac("001a.2b3c.4d5e"), expected);
        assert_eq!(parse_mac("001A2B3C4D5E"), expected);
        assert_eq!(
            parse_mac("1:0:5e:0:0:fb"),
            Some([0x01, 0x00, 0x5e, 0x00, 0x00, 0xfb])
        );
        for bad in [
            "",
            "00:1a:2b:3c:4d",
            "00:1a:2b:3c:4d:5e:6f",
            "00:1a:2b:3c:4d:zz",
            "001a.2b3c.4d5",
            "001a2.b3c4.d5e",
            "001:a2:b3:c4:d5:e",
            "(incomplete)",
        ] {
            assert_eq!(parse_mac(bad), None, "{bad:?}");
        }
        assert_eq!(format_mac([0, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]), "00:1a:2b:3c:4d:5e");
    }

    #[test]
    fn printable_text() {
        assert_eq!(printable("SSH-2.0-OpenSSH_9.6\r\n", 80), "SSH-2.0-OpenSSH_9.6");
        assert_eq!(printable("a\x00\x07b\t\tc", 80), "a b c");
        assert_eq!(printable("abcdef", 3), "abc");
        assert_eq!(printable("\r\n", 80), "");
    }
}
