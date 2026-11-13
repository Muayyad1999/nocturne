//! MAC addresses: every notation, the address flags, EUI-64, and the vendor
//! from a bundled copy of the IEEE registries.
//!
//! The registry holds the MA-L (24-bit OUI), MA-M (28-bit), MA-S and IAB
//! (36-bit) assignments; lookups take the longest matching prefix. It is
//! built by `data/build_oui.py`, embedded gzip-compressed and inflated on
//! first use.

use std::io::Read;
use std::ops::Range;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::ip::format_ipv6;

/// A MAC address in every common notation, with what can be read from it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MacInfo {
    /// `aa:bb:cc:dd:ee:ff` (Linux, Junos, most documentation).
    pub colon: String,
    /// `AA-BB-CC-DD-EE-FF` (IEEE canonical form, Windows).
    pub hyphen: String,
    /// `aabb.ccdd.eeff` (Cisco).
    pub dotted: String,
    /// `aabbccddeeff`.
    pub bare: String,
    /// The first three octets, `AA:BB:CC`.
    pub oui: String,
    /// The organisation the address block is registered to, if it is in the registry.
    pub vendor: Option<String>,
    /// The group bit is set: a multicast (or the broadcast) address.
    pub is_multicast: bool,
    /// The address is `ff:ff:ff:ff:ff:ff`.
    pub is_broadcast: bool,
    /// The local bit is set: assigned by an administrator or by software
    /// (virtual machines, randomised Wi-Fi addresses), not by a manufacturer.
    pub is_locally_administered: bool,
    /// The modified EUI-64 interface identifier, e.g. `a8bb:ccff:fedd:eeff`.
    pub eui64: String,
    /// The IPv6 link-local address SLAAC derives from the MAC, e.g. `fe80::a8bb:ccff:fedd:eeff`.
    pub link_local_ipv6: String,
}

/// Parse a MAC address written in any common notation, in either case.
///
/// Accepts `aa:bb:cc:dd:ee:ff`, `AA-BB-CC-DD-EE-FF`, `aabb.ccdd.eeff`,
/// `aabb-ccdd-eeff`, `aabbcc-ddeeff`, `aa bb cc dd ee ff`, `aabbccddeeff`,
/// `0xaabbccddeeff` and octets without their leading zero (`0:1b:21:a:b:c`).
pub fn info(input: &str) -> Result<MacInfo, ToolError> {
    const WHAT: &str = "MAC address";
    let text = input.trim();
    let digits = parse_octets(text).map_err(|reason| ToolError::invalid(WHAT, text, reason))?;
    let Ok(octets) = <[u8; 6]>::try_from(digits.as_slice()) else {
        let reason = match digits.len() {
            8 => "this has 8 octets: it is a 64-bit EUI-64 identifier, not a 48-bit MAC address"
                .to_owned(),
            count => format!("a MAC address has 6 octets (12 hex digits), this has {count}"),
        };
        return Err(ToolError::invalid(WHAT, text, reason));
    };

    let [a, b, c, d, e, f] = octets;
    let value = octets
        .iter()
        .fold(0u64, |value, octet| (value << 8) | u64::from(*octet));
    let interface_id = (u128::from(a ^ 0x02) << 56)
        | (u128::from(b) << 48)
        | (u128::from(c) << 40)
        | (0xfffe << 24)
        | u128::from(value & 0xff_ffff);
    Ok(MacInfo {
        colon: format!("{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}"),
        hyphen: format!("{a:02X}-{b:02X}-{c:02X}-{d:02X}-{e:02X}-{f:02X}"),
        dotted: format!("{a:02x}{b:02x}.{c:02x}{d:02x}.{e:02x}{f:02x}"),
        bare: format!("{value:012x}"),
        oui: format!("{a:02X}:{b:02X}:{c:02X}"),
        vendor: registry().lookup(value, 48).map(str::to_owned),
        is_multicast: a & 0x01 != 0,
        is_broadcast: octets == [0xff; 6],
        is_locally_administered: a & 0x02 != 0,
        eui64: format!("{:02x}{b:02x}:{c:02x}ff:fe{d:02x}:{e:02x}{f:02x}", a ^ 0x02),
        link_local_ipv6: format_ipv6((0xfe80 << 112) | interface_id),
    })
}

/// The organisation a MAC address or a prefix of one is registered to.
///
/// The input can be a full address or just its beginning — at least the
/// three octets of an OUI (`00:1b:21`, `001B21`, `0000.0c`). Longer input
/// lets the lookup find the more specific MA-M, MA-S and IAB assignments.
/// For multicast addresses the owner of the corresponding address block is
/// returned. `None` when the input is not a MAC prefix or is not registered.
pub fn vendor(input: &str) -> Option<String> {
    let octets = parse_octets(input.trim()).ok()?;
    if !(3..=6).contains(&octets.len()) {
        return None;
    }
    let value = octets
        .iter()
        .fold(0u64, |value, octet| (value << 8) | u64::from(*octet));
    let known_bits = 8 * octets.len() as u32;
    registry()
        .lookup(value << (48 - known_bits), known_bits)
        .map(str::to_owned)
}

/// How many assignments the bundled registry holds, and the date (`YYYY-MM-DD`)
/// it was downloaded from the IEEE.
pub fn registry_info() -> (usize, &'static str) {
    let registry = registry();
    (registry.blocks.iter().map(Vec::len).sum(), &registry.date)
}

// ----------------------------------------------------------------- parsing

/// The octets of a MAC address or prefix written in any common notation.
/// The error is the reason the text is not one.
fn parse_octets(text: &str) -> Result<Vec<u8>, String> {
    // No notation of a 48-bit address comes near this; do not chew on junk.
    const MAX_LEN: usize = 40;
    if text.is_empty() {
        return Err("nothing was entered".to_owned());
    }
    if text.len() > MAX_LEN {
        return Err("it is too long to be a MAC address".to_owned());
    }

    let prefixed = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .filter(|hex| !hex.is_empty());
    let mut separators = text.chars().filter(|c| !c.is_ascii_hexdigit());
    let nibbles: Vec<u8> = match (prefixed, separators.next()) {
        (_, None) => text.bytes().map(nibble).collect(),
        (Some(hex), _) if hex.bytes().all(|b| b.is_ascii_hexdigit()) => {
            hex.bytes().map(nibble).collect()
        }
        (_, Some(separator)) => {
            if !matches!(separator, ':' | '-' | '.' | ' ') {
                return Err(format!(
                    "`{separator}` is neither a hex digit nor a separator (`:`, `-`, `.` or a space)"
                ));
            }
            if let Some(other) = separators.find(|c| *c != separator) {
                return Err(if matches!(other, ':' | '-' | '.' | ' ') {
                    format!("it mixes the separators `{separator}` and `{other}`")
                } else {
                    format!(
                        "`{other}` is neither a hex digit nor a separator (`:`, `-`, `.` or a space)"
                    )
                });
            }
            let groups: Vec<&str> = text.split(separator).collect();
            if groups.iter().any(|group| group.is_empty()) {
                return Err(format!("a group is empty: there is a stray `{separator}`"));
            }
            let longest = groups.iter().map(|group| group.len()).max().unwrap_or(0);
            if longest <= 2 {
                // Octet by octet; `0:1b:21:a:b:c` leaves out leading zeros.
                groups
                    .iter()
                    .flat_map(|group| format!("{group:0>2}").into_bytes())
                    .map(nibble)
                    .collect()
            } else {
                // Cisco `aabb.ccdd.eeff`, Huawei `aabb-ccdd-eeff`, HP `aabbcc-ddeeff`.
                let (last, full) = groups.split_last().unwrap_or((&"", &[]));
                let regular = matches!(longest, 4 | 6)
                    && full.iter().all(|group| group.len() == longest)
                    && last.len() <= longest;
                if !regular {
                    return Err(
                        "the groups must be octets (`aa:bb:cc:dd:ee:ff`), four hex digits (`aabb.ccdd.eeff`) \
                         or six (`aabbcc-ddeeff`)"
                            .to_owned(),
                    );
                }
                groups.concat().bytes().map(nibble).collect()
            }
        }
    };
    if !nibbles.len().is_multiple_of(2) {
        return Err(format!(
            "it has an odd number of hex digits ({})",
            nibbles.len()
        ));
    }
    Ok(nibbles
        .chunks_exact(2)
        .map(|pair| (pair[0] << 4) | pair[1])
        .collect())
}

/// The value of an ASCII hex digit (0 for anything else; callers check first).
fn nibble(digit: u8) -> u8 {
    (digit as char).to_digit(16).unwrap_or(0) as u8
}

// ---------------------------------------------------------------- registry

static REGISTRY_GZ: &[u8] = include_bytes!("../data/oui.tsv.gz");

/// Prefix lengths of the registries, longest first, with the index of their block.
const PREFIX_BITS: [u32; 3] = [36, 28, 24];

struct Registry {
    /// The inflated `PREFIX<TAB>Organisation` lines.
    text: String,
    date: String,
    /// `(prefix, organisation name in `text`)` sorted by prefix, one list per entry of [`PREFIX_BITS`].
    blocks: [Vec<(u64, Range<u32>)>; 3],
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| Registry::parse(REGISTRY_GZ))
}

impl Registry {
    /// Inflate and index the registry. Lines that are not understood are
    /// skipped; a corrupt file therefore yields an empty or partial registry
    /// (which the tests would catch) rather than a panic.
    fn parse(gz: &[u8]) -> Self {
        let mut text = String::new();
        if flate2::read::GzDecoder::new(gz)
            .read_to_string(&mut text)
            .is_err()
        {
            text.clear();
        }
        let mut date = String::new();
        let mut blocks: [Vec<(u64, Range<u32>)>; 3] = Default::default();
        let mut offset = 0usize;
        for line in text.split_inclusive('\n') {
            let start = offset;
            offset += line.len();
            let line = line.trim_end_matches(['\n', '\r']);
            let Some((key, name)) = line.split_once('\t') else {
                continue;
            };
            if key == "#date" {
                date = name.to_owned();
                continue;
            }
            let block = PREFIX_BITS
                .iter()
                .position(|bits| *bits as usize == 4 * key.len());
            let prefix = u64::from_str_radix(key, 16).ok();
            let name_start = u32::try_from(start + key.len() + 1).ok();
            let name_end = u32::try_from(start + line.len()).ok();
            if let (Some(block), Some(prefix), Some(name_start), Some(name_end)) =
                (block, prefix, name_start, name_end)
            {
                blocks[block].push((prefix, name_start..name_end));
            }
        }
        for block in &mut blocks {
            block.sort_unstable_by_key(|(prefix, _)| *prefix);
        }
        Self { text, date, blocks }
    }

    /// The organisation for the longest registered prefix of `mac`, of which
    /// only the first `known_bits` bits are known.
    ///
    /// An address block covers its group (multicast) addresses too, so a
    /// multicast address that is not registered as such is looked up again
    /// with the group bit cleared.
    fn lookup(&self, mac: u64, known_bits: u32) -> Option<&str> {
        const GROUP_BIT: u64 = 1 << 40;
        self.longest_match(mac, known_bits).or_else(|| {
            (mac & GROUP_BIT != 0)
                .then(|| self.longest_match(mac & !GROUP_BIT, known_bits))
                .flatten()
        })
    }

    fn longest_match(&self, mac: u64, known_bits: u32) -> Option<&str> {
        PREFIX_BITS
            .iter()
            .zip(&self.blocks)
            .filter(|(bits, _)| **bits <= known_bits)
            .find_map(|(bits, block)| {
                let prefix = mac >> (48 - bits);
                let index = block
                    .binary_search_by_key(&prefix, |(prefix, _)| *prefix)
                    .ok()?;
                let name = &block[index].1;
                self.text.get(name.start as usize..name.end as usize)
            })
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn every_notation_gives_the_same_address() {
        let expected = MacInfo {
            colon: "aa:bb:cc:dd:ee:ff".into(),
            hyphen: "AA-BB-CC-DD-EE-FF".into(),
            dotted: "aabb.ccdd.eeff".into(),
            bare: "aabbccddeeff".into(),
            oui: "AA:BB:CC".into(),
            vendor: None,
            is_multicast: false,
            is_broadcast: false,
            is_locally_administered: true,
            eui64: "a8bb:ccff:fedd:eeff".into(),
            link_local_ipv6: "fe80::a8bb:ccff:fedd:eeff".into(),
        };
        for input in [
            "aa:bb:cc:dd:ee:ff",
            "AA:BB:CC:DD:EE:FF",
            "AA-BB-CC-DD-EE-FF",
            "aa-bb-cc-dd-ee-ff",
            "aabb.ccdd.eeff",
            "AABB.CCDD.EEFF",
            "aabb-ccdd-eeff",
            "aabb:ccdd:eeff",
            "aabbcc-ddeeff",
            "aabbcc:ddeeff",
            "aa bb cc dd ee ff",
            "aa.bb.cc.dd.ee.ff",
            "aabbccddeeff",
            "AaBbCcDdEeFf",
            "0xaabbccddeeff",
            "0XAABBCCDDEEFF",
            "  aa:bb:cc:dd:ee:ff\n",
        ] {
            assert_eq!(info(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn octets_may_omit_their_leading_zero() {
        // As printed by `arp -a` on BSD, macOS and Solaris.
        let info = info("0:1b:21:a:b:c").unwrap();
        assert_eq!(info.colon, "00:1b:21:0a:0b:0c");
        assert_eq!(info.dotted, "001b.210a.0b0c");
        assert_eq!(info.vendor.as_deref(), Some("Intel Corporate"));
    }

    #[test]
    fn eui64_and_link_local() {
        // RFC 4291 appendix A: flip the universal/local bit, insert ff:fe.
        let info = info("00:11:22:33:44:55").unwrap();
        assert_eq!(info.eui64, "0211:22ff:fe33:4455");
        assert_eq!(info.link_local_ipv6, "fe80::211:22ff:fe33:4455");
        // The textbook example from RFC 2373: 34-56-78-9A-BC-DE.
        let rfc = super::info("34-56-78-9A-BC-DE").unwrap();
        assert_eq!(rfc.eui64, "3656:78ff:fe9a:bcde");
        assert_eq!(rfc.link_local_ipv6, "fe80::3656:78ff:fe9a:bcde");
        // Zero groups are compressed the RFC 5952 way.
        assert_eq!(
            super::info("02:00:00:00:00:00").unwrap().link_local_ipv6,
            "fe80::ff:fe00:0"
        );
        assert_eq!(
            super::info("02:00:00:00:00:01").unwrap().eui64,
            "0000:00ff:fe00:0001"
        );
    }

    #[test]
    fn address_flags() {
        let unicast = info("00:1c:73:00:00:01").unwrap();
        assert!(!unicast.is_multicast && !unicast.is_broadcast && !unicast.is_locally_administered);

        let multicast = info("01:00:5e:00:00:fb").unwrap();
        assert!(
            multicast.is_multicast && !multicast.is_broadcast && !multicast.is_locally_administered
        );

        let ipv6_multicast = info("33:33:00:00:00:01").unwrap();
        assert!(ipv6_multicast.is_multicast && ipv6_multicast.is_locally_administered);

        let broadcast = info("ff:ff:ff:ff:ff:ff").unwrap();
        assert!(
            broadcast.is_multicast && broadcast.is_broadcast && broadcast.is_locally_administered
        );
        assert_eq!(broadcast.vendor, None);

        // A randomised ("private") Wi-Fi address: local bit set, no vendor.
        let random = info("06:12:34:56:78:9a").unwrap();
        assert!(!random.is_multicast && random.is_locally_administered);
        assert_eq!(random.vendor, None);
    }

    #[test]
    fn vendors_from_the_registry() {
        for (mac, vendor) in [
            ("00:00:0c:12:34:56", "Cisco Systems"),
            ("ac:3a:67:00:00:01", "Cisco Systems"),
            ("00:1c:73:aa:bb:cc", "Arista Networks"),
            ("4c:5e:0c:11:22:33", "Routerboard.com"),
            ("24:5a:4c:00:00:01", "Ubiquiti"),
            ("00:25:9e:00:00:01", "HUAWEI TECHNOLOGIES"),
            ("00:0f:e2:00:00:01", "Hangzhou H3C Technologies"),
            ("00:50:56:9a:00:01", "VMware"),
            ("b8:27:eb:01:02:03", "Raspberry Pi Foundation"),
            ("08:00:2b:01:02:03", "DIGITAL EQUIPMENT"),
            ("00-00-5E-00-01-0A", "ICANN, IANA Department"),
        ] {
            assert_eq!(info(mac).unwrap().vendor.as_deref(), Some(vendor), "{mac}");
            assert_eq!(super::vendor(mac).as_deref(), Some(vendor), "{mac}");
        }
        assert_eq!(info("00:00:0c:12:34:56").unwrap().oui, "00:00:0C");
    }

    #[test]
    fn longer_registry_prefixes_win() {
        // 70-B3-D5 is an IEEE block handed out in 36-bit pieces (MA-S).
        assert_eq!(
            vendor("70:b3:d5").as_deref(),
            Some("IEEE Registration Authority")
        );
        assert_eq!(
            vendor("70:b3:d5:00:1f:ff").as_deref(),
            Some("SOREDI touch systems")
        );
        assert_eq!(
            vendor("70b3.d500.1000").as_deref(),
            Some("SOREDI touch systems")
        );
        // MA-M: 28-bit prefixes.
        assert_eq!(vendor("00:55:da:1f:ff:ff").as_deref(), Some("KoolPOS"));
        assert_eq!(
            vendor("00:55:da:20:00:00").as_deref(),
            Some("Beijing Connected Information Technology")
        );
        // IAB: 36-bit prefixes in the old 00-50-C2 block.
        assert_eq!(
            vendor("00:50:c2:ff:f1:23").as_deref(),
            Some("MSR-Solutions")
        );
        assert_eq!(
            vendor("00:50:c2").as_deref(),
            Some("IEEE Registration Authority")
        );
        // With only four octets known, a 36-bit assignment cannot be told
        // apart, but a 28-bit one can.
        assert_eq!(
            vendor("70:b3:d5:00").as_deref(),
            Some("IEEE Registration Authority")
        );
        assert_eq!(vendor("00:55:da:1f").as_deref(), Some("KoolPOS"));
        // Assignments whose owner asked not to be listed.
        assert_eq!(vendor("70:b3:d5:ff:f0:00").as_deref(), Some("Private"));
        // A prefix the registry lists under several names keeps them all.
        assert_eq!(
            vendor("08:00:30").as_deref(),
            Some("CERN / NETWORK RESEARCH / ROYAL MELBOURNE INST OF TECH")
        );
    }

    #[test]
    fn multicast_addresses_resolve_to_the_block_owner() {
        // CDP/VTP/PVST+ use group addresses of Cisco's 00-00-0C block ...
        assert_eq!(
            info("01:00:0c:cc:cc:cc").unwrap().vendor.as_deref(),
            Some("Cisco Systems")
        );
        // ... and IPv4 multicast those of IANA's 00-00-5E.
        assert_eq!(
            info("01:00:5e:7f:ff:fa").unwrap().vendor.as_deref(),
            Some("ICANN, IANA Department")
        );
        assert_eq!(
            info("01:80:c2:00:00:0e").unwrap().vendor.as_deref(),
            Some("IEEE 802.1 Working Group")
        );
        // Two prefixes are registered with the group bit set; they are found as they are.
        assert_eq!(vendor("11:11:11").as_deref(), Some("Private"));
        assert_eq!(vendor("11:00:aa:00:00:01").as_deref(), Some("Private"));
    }

    #[test]
    fn vendor_accepts_prefixes_in_any_notation() {
        for prefix in [
            "00:00:0c",
            "00-00-0C",
            "00000c",
            "00000C",
            "0000.0c",
            "0:0:c",
            "0000.0c12",
            "00:00:0c:12:34",
            " 00:00:0c ",
        ] {
            assert_eq!(vendor(prefix).as_deref(), Some("Cisco Systems"), "{prefix}");
        }
        for not_a_prefix in [
            "",
            "00:00",
            "0000",
            "00000",
            "00:00:0c:12:34:56:78",
            "cisco",
            "00:00:0g",
            "00:00-0c",
            "zz:zz:zz",
        ] {
            assert_eq!(vendor(not_a_prefix), None, "{not_a_prefix}");
        }
        // Unassigned space.
        assert_eq!(vendor("ff:ff:fe"), None);
    }

    #[test]
    fn registry_metadata() {
        let (count, date) = registry_info();
        assert_eq!(count, 59_032);
        assert_eq!(date, "2026-11-13");
        let registry = registry();
        assert_eq!(
            registry.blocks.iter().map(Vec::len).collect::<Vec<_>>(),
            [11_853, 6_686, 40_493]
        );
        for block in &registry.blocks {
            assert!(
                block.windows(2).all(|pair| pair[0].0 < pair[1].0),
                "prefixes are sorted and unique"
            );
        }
        // Every name is present and tidy.
        for (_, name) in registry.blocks.iter().flatten() {
            let name = &registry.text[name.start as usize..name.end as usize];
            assert!(
                !name.is_empty() && name == name.trim() && !name.contains("  "),
                "{name:?}"
            );
        }
    }

    #[test]
    fn every_registry_entry_can_be_looked_up() {
        // An independent, naive lookup over the inflated text: the longest of
        // the 9-, 7- and 6-digit prefixes of an address that the file lists.
        let registry = registry();
        let listed: std::collections::HashMap<&str, &str> = registry
            .text
            .lines()
            .filter(|line| !line.starts_with('#'))
            .filter_map(|line| line.split_once('\t'))
            .collect();
        assert_eq!(listed.len(), registry_info().0);
        for prefix in listed.keys() {
            for filler in ["0", "5", "F"] {
                let address = format!("{prefix}{}", filler.repeat(12 - prefix.len()));
                let expected = [9, 7, 6]
                    .iter()
                    .find_map(|digits| listed.get(&address[..*digits]));
                assert_eq!(vendor(&address).as_deref(), expected.copied(), "{address}");
                assert_eq!(
                    info(&address).unwrap().vendor.as_deref(),
                    expected.copied(),
                    "{address}"
                );
            }
        }
    }

    #[test]
    fn a_corrupt_registry_is_empty_not_fatal() {
        let registry = Registry::parse(b"not gzip at all");
        assert!(registry.blocks.iter().all(Vec::is_empty));
        assert_eq!(registry.lookup(0x0000_0c12_3456, 48), None);
        // Half a file: whatever inflates cleanly is not trusted either.
        let truncated = Registry::parse(&REGISTRY_GZ[..REGISTRY_GZ.len() / 2]);
        assert!(truncated.blocks.iter().all(Vec::is_empty));
    }

    #[test]
    fn rejections_say_what_is_wrong() {
        let message = |input: &str| info(input).unwrap_err().to_string();
        assert_eq!(message(""), "invalid MAC address: nothing was entered");
        assert_eq!(
            message("aa:bb:cc:dd:ee"),
            "invalid MAC address `aa:bb:cc:dd:ee`: a MAC address has 6 octets (12 hex digits), this has 5"
        );
        assert_eq!(
            message("aa:bb:cc:dd:ee:ff:00"),
            "invalid MAC address `aa:bb:cc:dd:ee:ff:00`: a MAC address has 6 octets (12 hex digits), this has 7"
        );
        assert_eq!(
            message("00:11:22:ff:fe:33:44:55"),
            "invalid MAC address `00:11:22:ff:fe:33:44:55`: this has 8 octets: it is a 64-bit EUI-64 identifier, not a 48-bit MAC address"
        );
        assert_eq!(
            message("aa:bb:cc:dd:ee:fg"),
            "invalid MAC address `aa:bb:cc:dd:ee:fg`: `g` is neither a hex digit nor a separator (`:`, `-`, `.` or a space)"
        );
        assert_eq!(
            message("aa:bb-cc:dd:ee:ff"),
            "invalid MAC address `aa:bb-cc:dd:ee:ff`: it mixes the separators `:` and `-`"
        );
        assert_eq!(
            message("aa::cc:dd:ee:ff"),
            "invalid MAC address `aa::cc:dd:ee:ff`: a group is empty: there is a stray `:`"
        );
        assert_eq!(
            message("aabbccddeef"),
            "invalid MAC address `aabbccddeef`: it has an odd number of hex digits (11)"
        );
        assert!(message("aabb.ccdd.eef").contains("odd number of hex digits"));
        assert!(message("aab.bcc.dde.eff").contains("the groups must be octets"));
        assert!(message("aabbc.cddee.ff").contains("the groups must be octets"));
        assert!(message("aabb.cc.ddeeff").contains("the groups must be octets"));
        assert!(message(&"ab".repeat(1000)).contains("too long to be a MAC address"));
        assert!(message("0x").contains("neither a hex digit"));
        assert!(message("0xaabbccddeeffgg").contains("neither a hex digit"));
        assert!(
            message("аа:bb:cc:dd:ee:ff").contains("neither a hex digit"),
            "Cyrillic look-alikes are not hex digits"
        );
    }
}
