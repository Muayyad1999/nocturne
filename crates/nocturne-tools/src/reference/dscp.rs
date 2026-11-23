//! Differentiated Services code points.

use serde::{Deserialize, Serialize};

/// One DSCP value in all the ways it is written.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct DscpEntry {
    /// The code point's name: `EF`, `AF41`, `CS6`, `DF`, ... (`DSCP 47` for a value without one).
    pub name: String,
    /// The six-bit DSCP value, 0-63.
    pub dscp: u8,
    /// The value as six binary digits, e.g. `101110`.
    pub binary: String,
    /// The value of the whole ToS / Traffic Class byte with the ECN bits zero (`dscp * 4`).
    pub tos: u8,
    /// The IP precedence the value maps to (its top three bits).
    pub precedence: u8,
    /// The per-hop behaviour and where it is defined.
    pub phb: String,
    /// What the code point is normally used for (RFC 4594 and common practice).
    pub typical_use: String,
}

/// `(name, value, per-hop behaviour, typical use)` in descending order of value.
#[rustfmt::skip]
const CODE_POINTS: &[(&str, u8, &str, &str)] = &[
    ("CS7", 56, "Class Selector 7 (RFC 2474)", "Reserved for network control; not for user traffic"),
    ("CS6", 48, "Class Selector 6 (RFC 2474)", "Network control: routing protocols such as OSPF, BGP and BFD"),
    ("EF", 46, "Expedited Forwarding (RFC 3246)", "Voice bearer (RTP): low loss, low delay, low jitter, strictly policed"),
    ("NQB", 45, "Non-Queue-Building (RFC 9956)", "Low-rate flows that do not build queues, for a shallow low-latency queue (e.g. in DOCSIS and Wi-Fi access)"),
    ("VOICE-ADMIT", 44, "Voice-Admit (RFC 5865)", "Voice admitted by capacity admission control, treated like EF"),
    ("CS5", 40, "Class Selector 5 (RFC 2474)", "Signalling in RFC 4594 (SIP, H.323); broadcast video in Cisco designs"),
    ("AF43", 38, "Assured Forwarding class 4, high drop precedence (RFC 2597)", "Multimedia conferencing, out-of-contract traffic"),
    ("AF42", 36, "Assured Forwarding class 4, medium drop precedence (RFC 2597)", "Multimedia conferencing, excess traffic"),
    ("AF41", 34, "Assured Forwarding class 4, low drop precedence (RFC 2597)", "Multimedia conferencing: interactive video"),
    ("CS4", 32, "Class Selector 4 (RFC 2474)", "Real-time interactive: telepresence, gaming, remote desktop"),
    ("AF33", 30, "Assured Forwarding class 3, high drop precedence (RFC 2597)", "Multimedia streaming, out-of-contract traffic"),
    ("AF32", 28, "Assured Forwarding class 3, medium drop precedence (RFC 2597)", "Multimedia streaming, excess traffic"),
    ("AF31", 26, "Assured Forwarding class 3, low drop precedence (RFC 2597)", "Multimedia streaming: video on demand"),
    ("CS3", 24, "Class Selector 3 (RFC 2474)", "Broadcast video in RFC 4594; call signalling in Cisco designs"),
    ("AF23", 22, "Assured Forwarding class 2, high drop precedence (RFC 2597)", "Low-latency data, out-of-contract traffic"),
    ("AF22", 20, "Assured Forwarding class 2, medium drop precedence (RFC 2597)", "Low-latency data, excess traffic"),
    ("AF21", 18, "Assured Forwarding class 2, low drop precedence (RFC 2597)", "Low-latency data: transactional and interactive applications"),
    ("CS2", 16, "Class Selector 2 (RFC 2474)", "Operations and management: SNMP, syslog, SSH, NTP"),
    ("AF13", 14, "Assured Forwarding class 1, high drop precedence (RFC 2597)", "High-throughput data, out-of-contract traffic"),
    ("AF12", 12, "Assured Forwarding class 1, medium drop precedence (RFC 2597)", "High-throughput data, excess traffic"),
    ("AF11", 10, "Assured Forwarding class 1, low drop precedence (RFC 2597)", "High-throughput data: bulk transfers, backups"),
    ("CS1", 8, "Class Selector 1 (RFC 2474)", "Low-priority data in RFC 4594; the scavenger class before LE existed"),
    ("LE", 1, "Lower Effort (RFC 8622)", "Scavenger traffic that may be starved: bulk backups, software distribution"),
    ("DF", 0, "Default Forwarding, best effort; the same code point as CS0 (RFC 2474)", "Everything that is not classified otherwise"),
];

/// Other names people use for a code point.
const ALIASES: &[(&str, u8)] = &[
    ("cs0", 0),
    ("be", 0),
    ("default", 0),
    ("best-effort", 0),
    ("va", 44),
    ("voice-admit", 44),
];

/// The named code points, from the highest value down: CS7 ... EF ... AF11 ... DF.
pub fn dscp_table() -> Vec<DscpEntry> {
    CODE_POINTS
        .iter()
        .map(|(_, value, _, _)| entry(*value))
        .collect()
}

/// Look up a code point by name or value.
///
/// * a name: `ef`, `af41`, `cs6`, `le`, `be`, `default` (case-insensitive);
/// * a decimal DSCP value `0`-`63`, or six binary digits (`101110`);
/// * a ToS / Traffic Class byte: hexadecimal (`0xb8`), eight binary digits,
///   or a decimal value above 63 (`184`). Its two ECN bits are ignored.
///
/// Values without a standard name are still returned, described by the
/// registry pool they fall in. `None` if the query is none of the above.
pub fn dscp(query: &str) -> Option<DscpEntry> {
    let query = query.trim().to_ascii_lowercase();
    let is_binary = |digits: &str| digits.bytes().all(|b| matches!(b, b'0' | b'1'));
    let value = if let Some(value) = named(&query) {
        value
    } else if let Some(hex) = query.strip_prefix("0x") {
        u8::from_str_radix(hex, 16)
            .ok()
            .filter(|_| !hex.starts_with('+'))?
            >> 2
    } else if let Some(bits) = query.strip_prefix("0b").filter(|bits| is_binary(bits)) {
        match bits.len() {
            1..=6 => u8::from_str_radix(bits, 2).ok()?,
            7 | 8 => u8::from_str_radix(bits, 2).ok()? >> 2,
            _ => return None,
        }
    } else if query.len() == 6 && is_binary(&query) {
        u8::from_str_radix(&query, 2).ok()?
    } else if query.len() == 8 && is_binary(&query) {
        u8::from_str_radix(&query, 2).ok()? >> 2
    } else if query.bytes().all(|b| b.is_ascii_digit()) {
        match query.parse::<u8>().ok()? {
            value @ 0..=63 => value,
            tos => tos >> 2,
        }
    } else {
        return None;
    };
    Some(entry(value))
}

/// The value a code point's name stands for: `ef`, `af41`, `cs6`, `le`,
/// `be`, `default` (case-insensitive). Numbers are not names.
pub(crate) fn named(name: &str) -> Option<u8> {
    let name = name.trim();
    CODE_POINTS
        .iter()
        .find(|(candidate, ..)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value, ..)| *value)
        .or_else(|| {
            ALIASES
                .iter()
                .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
                .map(|(_, value)| *value)
        })
}

/// The registered `(name, per-hop behaviour, typical use)` of a six-bit
/// value, or `None` for a value the registry has not assigned.
pub(crate) fn registered(value: u8) -> Option<(&'static str, &'static str, &'static str)> {
    CODE_POINTS
        .iter()
        .find(|(_, candidate, ..)| *candidate == value & 0x3f)
        .map(|(name, _, phb, typical_use)| (*name, *phb, *typical_use))
}

/// What the registry says about a value it has not assigned: the pool the
/// value falls in (RFC 2474 as amended by RFC 8436).
pub(crate) fn pool(value: u8) -> &'static str {
    match value & 0b11 {
        0b11 => "No standard per-hop behaviour: pool 2, reserved for experimental or local use",
        0b01 => "No standard per-hop behaviour: pool 3, unassigned (standards action, RFC 8436)",
        _ => "No standard per-hop behaviour: pool 1, unassigned (standards action)",
    }
}

fn entry(value: u8) -> DscpEntry {
    let value = value & 0x3f;
    let (name, phb, typical_use) = match registered(value) {
        Some((name, phb, typical_use)) => (name.to_owned(), phb.to_owned(), typical_use.to_owned()),
        None => (
            format!("DSCP {value}"),
            pool(value).to_owned(),
            String::new(),
        ),
    };
    DscpEntry {
        name,
        dscp: value,
        binary: format!("{value:06b}"),
        tos: value << 2,
        precedence: value >> 3,
        phb,
        typical_use,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_holds_the_registered_code_points() {
        let table = dscp_table();
        // The IANA DSCP registry: 8 class selectors, 12 AF, EF, VOICE-ADMIT, LE and NQB.
        assert_eq!(table.len(), 24);
        let values: Vec<(&str, u8)> = table
            .iter()
            .map(|entry| (entry.name.as_str(), entry.dscp))
            .collect();
        assert_eq!(
            values,
            [
                ("CS7", 56),
                ("CS6", 48),
                ("EF", 46),
                ("NQB", 45),
                ("VOICE-ADMIT", 44),
                ("CS5", 40),
                ("AF43", 38),
                ("AF42", 36),
                ("AF41", 34),
                ("CS4", 32),
                ("AF33", 30),
                ("AF32", 28),
                ("AF31", 26),
                ("CS3", 24),
                ("AF23", 22),
                ("AF22", 20),
                ("AF21", 18),
                ("CS2", 16),
                ("AF13", 14),
                ("AF12", 12),
                ("AF11", 10),
                ("CS1", 8),
                ("LE", 1),
                ("DF", 0),
            ]
        );
        for entry in &table {
            assert_eq!(entry.binary, format!("{:06b}", entry.dscp));
            assert_eq!(entry.tos, entry.dscp * 4);
            assert_eq!(entry.precedence, entry.dscp / 8);
            assert!(!entry.phb.is_empty() && !entry.typical_use.is_empty());
        }
    }

    #[test]
    fn af_and_cs_values_follow_their_formulas() {
        // AFxy = 8x + 2y (RFC 2597); CSx = 8x (RFC 2474).
        for class in 1..=4u8 {
            for drop in 1..=3u8 {
                let entry = dscp(&format!("AF{class}{drop}")).unwrap();
                assert_eq!(entry.dscp, 8 * class + 2 * drop);
                assert_eq!(entry.precedence, class);
            }
        }
        for class in 0..=7u8 {
            let entry = dscp(&format!("cs{class}")).unwrap();
            assert_eq!((entry.dscp, entry.precedence), (8 * class, class));
        }
    }

    #[test]
    fn lookups_in_every_notation() {
        let ef = dscp("ef").unwrap();
        assert_eq!(
            (
                ef.name.as_str(),
                ef.dscp,
                ef.binary.as_str(),
                ef.tos,
                ef.precedence
            ),
            ("EF", 46, "101110", 184, 5)
        );
        for query in [
            "EF",
            " ef ",
            "46",
            "0xb8",
            "0xB8",
            "184",
            "101110",
            "10111000",
            "0b101110",
            "0b10111000",
            "0xb9",
            "0xbb",
            "187",
        ] {
            assert_eq!(dscp(query).as_ref(), Some(&ef), "{query}");
        }
        let af41 = dscp("af41").unwrap();
        assert_eq!(
            (af41.dscp, af41.binary.as_str(), af41.tos, af41.precedence),
            (34, "100010", 136, 4)
        );
        assert_eq!(dscp("0x88").unwrap(), af41);
        assert_eq!(dscp("34").unwrap(), af41);
        for alias in [
            "be",
            "default",
            "cs0",
            "df",
            "0",
            "0x00",
            "000000",
            "best-effort",
        ] {
            assert_eq!(dscp(alias).unwrap().name, "DF", "{alias}");
        }
        assert_eq!(dscp("voice-admit").unwrap().dscp, 44);
        assert_eq!(dscp("cs6").unwrap().tos, 192);
        assert_eq!(dscp("0xc0").unwrap().name, "CS6");
        assert_eq!(dscp("0xe0").unwrap().name, "CS7");
        assert_eq!(dscp("63").unwrap().tos, 252);
        assert_eq!(dscp("255").unwrap().dscp, 63);
        assert_eq!(dscp("64").unwrap().dscp, 16);
    }

    #[test]
    fn dscp_and_tos_round_trip() {
        for value in 0..=63u8 {
            let entry = dscp(&value.to_string()).unwrap();
            assert_eq!(entry.dscp, value);
            assert_eq!(dscp(&format!("0x{:02x}", entry.tos)).unwrap(), entry);
            assert_eq!(dscp(&entry.binary).unwrap(), entry);
            assert_eq!(dscp(&format!("{:08b}", entry.tos)).unwrap(), entry);
            if entry.tos > 63 {
                assert_eq!(dscp(&entry.tos.to_string()).unwrap(), entry);
            }
            // The ECN bits do not change the code point.
            for ecn in 0..4u8 {
                assert_eq!(dscp(&format!("0x{:02x}", entry.tos | ecn)).unwrap(), entry);
            }
            if !entry.name.starts_with("DSCP ") {
                assert_eq!(dscp(&entry.name).unwrap(), entry);
            }
        }
    }

    #[test]
    fn values_without_a_name_are_described_by_their_pool() {
        let unnamed = dscp("47").unwrap();
        assert_eq!(
            (unnamed.name.as_str(), unnamed.tos, unnamed.precedence),
            ("DSCP 47", 188, 5)
        );
        assert!(unnamed.phb.contains("pool 2"));
        assert_eq!(unnamed.typical_use, "");
        assert!(dscp("5").unwrap().phb.contains("pool 3"));
        assert!(dscp("4").unwrap().phb.contains("pool 1"));
        let named = dscp_table().len();
        let unnamed = (0..=63)
            .filter(|value| dscp(&value.to_string()).unwrap().name.starts_with("DSCP "))
            .count();
        assert_eq!(named + unnamed, 64);
    }

    #[test]
    fn what_is_not_a_code_point() {
        for query in [
            "",
            "af44",
            "af10",
            "cs8",
            "256",
            "0x100",
            "0x",
            "0b",
            "0b111111111",
            "-1",
            "4.6",
            "ef41",
            "0x+8",
            "0b12",
            "1011102",
            "expedited",
        ] {
            assert_eq!(dscp(query), None, "{query}");
        }
    }
}
