//! Netmask / wildcard / prefix conversion and ACL wildcard matching (IPv4).
//!
//! A wildcard mask is the mask ACLs and OSPF `network` statements use: a
//! 1-bit means "don't care". For a prefix it is simply the inverted netmask,
//! but a wildcard may also be non-contiguous (`0.0.255.0`, `0.0.0.254`), which
//! no prefix can express; [`matches()`] and [`explain`] handle those too.

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::ip::{
    format_ipv4, group_thousands, netmask_v4, parse_dotted_quad, pow2_string, prefix_from_netmask,
};

/// A mask in all its spellings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MaskInfo {
    /// The prefix length, when the mask is contiguous.
    pub prefix: Option<u8>,
    /// Dotted netmask (1 = network bit).
    pub netmask: String,
    /// Dotted wildcard mask (1 = don't care), the inverse of `netmask`.
    pub wildcard: String,
    /// Whether the mask is a single block of 1-bits, i.e. equivalent to a prefix length.
    pub contiguous: bool,
    /// How many addresses a network (or ACL entry) with this mask covers, in decimal.
    pub addresses: String,
}

/// Convert between prefix length, netmask and wildcard mask.
///
/// Accepts a prefix length (`24`, `/24`), a dotted netmask (`255.255.255.0`),
/// a dotted wildcard (`0.0.0.255`) or a hexadecimal netmask as BSD systems
/// print it (`0xffffff00`). `0.0.0.0` and `255.255.255.255`, which are valid
/// both ways, are read as netmasks (/0 and /32).
///
/// A non-contiguous mask has no prefix length. It is taken as a netmask when
/// it starts with a 1-bit and as a wildcard otherwise, which is how such
/// masks are written in practice.
pub fn convert(input: &str) -> Result<MaskInfo, ToolError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(ToolError::invalid("mask", "", "nothing was entered"));
    }
    let digits = text.strip_prefix('/').map_or(text, str::trim_start);
    if digits.bytes().all(|b| b.is_ascii_digit()) && !digits.is_empty() {
        return match digits.parse::<u8>() {
            Ok(prefix) if prefix <= 32 => Ok(from_prefix(prefix)),
            _ => Err(ToolError::invalid(
                "prefix length",
                digits,
                "IPv4 prefix lengths range from 0 to 32 (IPv6 prefixes have no netmask or wildcard form)",
            )),
        };
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        if hex.is_empty() || hex.len() > 8 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ToolError::invalid(
                "mask",
                text,
                "a hexadecimal mask has up to 8 hex digits, e.g. `0xffffff00`",
            ));
        }
        // Up to 8 hex digits always fit.
        return Ok(from_mask(u32::from_str_radix(hex, 16).unwrap_or_default()));
    }
    if text.contains('.') && !text.starts_with('/') {
        return parse_dotted_quad(text, "mask").map(from_mask);
    }
    Err(ToolError::invalid(
        "mask",
        text,
        "expected a prefix length such as `24` or `/24`, a netmask such as `255.255.255.0` \
         or a wildcard such as `0.0.0.255`",
    ))
}

/// Whether `candidate` matches the ACL pair `address wildcard`.
///
/// Bits that are 1 in the wildcard are ignored, in the candidate and in the
/// address alike; non-contiguous wildcards work.
pub fn matches(address: &str, wildcard: &str, candidate: &str) -> Result<bool, ToolError> {
    let (address, wildcard) = parse_pair(address, wildcard)?;
    let candidate = parse_v4(candidate, "address")?;
    Ok((candidate ^ address) & !wildcard == 0)
}

/// Describe in plain English what the ACL pair `address wildcard` matches,
/// with the first and last matching address and how many there are.
pub fn explain(address: &str, wildcard: &str) -> Result<String, ToolError> {
    let (address, wildcard) = parse_pair(address, wildcard)?;
    let first = address & !wildcard;
    let last = first | wildcard;
    let pair = format!("`{} {}`", format_ipv4(address), format_ipv4(wildcard));
    let count = group_thousands(&pow2_string(wildcard.count_ones() as u8));

    let mut text = match prefix_from_netmask(!wildcard) {
        Some(32) => format!(
            "{pair} matches exactly one address, {} (the same as `host {}`).",
            format_ipv4(first),
            format_ipv4(first)
        ),
        Some(0) => format!(
            "{pair} matches every IPv4 address, 0.0.0.0 to 255.255.255.255 ({count} addresses; the same as `any`)."
        ),
        Some(prefix) => format!(
            "{pair} matches the {count} addresses of {}/{prefix}, from {} to {}.",
            format_ipv4(first),
            format_ipv4(first),
            format_ipv4(last)
        ),
        None => {
            let octets: Vec<String> = first
                .to_be_bytes()
                .iter()
                .zip(wildcard.to_be_bytes())
                .enumerate()
                .map(|(index, (value, wild))| {
                    format!("octet {} {}", index + 1, describe_octet(*value, wild))
                })
                .collect();
            format!(
                "{pair} is a non-contiguous wildcard, so no single prefix describes it. It matches {count} addresses \
                 between {} and {}: {}.",
                format_ipv4(first),
                format_ipv4(last),
                octets.join(", ")
            )
        }
    };
    if first != address {
        text.push_str(&format!(
            " The address bits under the wildcard are ignored; the canonical form is `{} {}`.",
            format_ipv4(first),
            format_ipv4(wildcard)
        ));
    }
    // The classic slip: a subnet mask where the wildcard belongs.
    if let Some(prefix @ 1..=31) = prefix_from_netmask(wildcard) {
        text.push_str(&format!(
            " Note: {} is a subnet mask; if you meant the network {}/{prefix}, its wildcard is {}.",
            format_ipv4(wildcard),
            format_ipv4(address & wildcard),
            format_ipv4(!wildcard)
        ));
    }
    Ok(text)
}

fn from_prefix(prefix: u8) -> MaskInfo {
    let netmask = netmask_v4(prefix);
    MaskInfo {
        prefix: Some(prefix),
        netmask: format_ipv4(netmask),
        wildcard: format_ipv4(!netmask),
        contiguous: true,
        addresses: pow2_string(32 - prefix),
    }
}

fn from_mask(mask: u32) -> MaskInfo {
    if let Some(prefix) = prefix_from_netmask(mask).or_else(|| prefix_from_netmask(!mask)) {
        return from_prefix(prefix);
    }
    let wildcard = if mask.leading_ones() > 0 { !mask } else { mask };
    MaskInfo {
        prefix: None,
        netmask: format_ipv4(!wildcard),
        wildcard: format_ipv4(wildcard),
        contiguous: false,
        addresses: pow2_string(wildcard.count_ones() as u8),
    }
}

fn parse_pair(address: &str, wildcard: &str) -> Result<(u32, u32), ToolError> {
    Ok((
        parse_v4(address, "address")?,
        parse_v4(wildcard, "wildcard mask")?,
    ))
}

fn parse_v4(text: &str, what: &str) -> Result<u32, ToolError> {
    let text = text.trim();
    if text.contains(':') {
        return Err(ToolError::invalid(
            what,
            text,
            "wildcard masks are an IPv4 concept; IPv6 ACLs match on prefixes (see the subnet tool)",
        ));
    }
    parse_dotted_quad(text, what)
}

/// What one octet of a matching address may be, given the octet of the
/// (canonical) address and of the wildcard.
fn describe_octet(value: u8, wild: u8) -> String {
    let high = value | wild;
    match wild {
        0 => format!("must be {value}"),
        255 => "can be anything".to_owned(),
        // The don't-care bits are the low bits: a plain range.
        _ if wild & (wild + 1) == 0 => format!("must be between {value} and {high}"),
        0b1111_1110 => format!("must be {}", if value == 0 { "even" } else { "odd" }),
        _ if wild.count_ones() <= 3 => {
            // Enumerate the subsets of the wildcard bits.
            let mut values = Vec::new();
            let mut subset = 0u8;
            loop {
                values.push((value | subset).to_string());
                subset = subset.wrapping_sub(wild) & wild;
                if subset == 0 {
                    break;
                }
            }
            format!("must be one of {}", values.join(", "))
        }
        _ => {
            let pattern: String = (0..8)
                .rev()
                .map(|bit| match (wild >> bit & 1, value >> bit & 1) {
                    (1, _) => 'x',
                    (_, 1) => '1',
                    _ => '0',
                })
                .collect();
            format!("must match the bit pattern {pattern} (x = any)")
        }
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn mask(prefix: Option<u8>, netmask: &str, wildcard: &str, addresses: &str) -> MaskInfo {
        MaskInfo {
            prefix,
            netmask: netmask.into(),
            wildcard: wildcard.into(),
            contiguous: prefix.is_some(),
            addresses: addresses.into(),
        }
    }

    #[test]
    fn converts_between_all_three_spellings() {
        let slash24 = mask(Some(24), "255.255.255.0", "0.0.0.255", "256");
        for input in [
            "24",
            "/24",
            " /24 ",
            "/ 24",
            "255.255.255.0",
            "0.0.0.255",
            "0xffffff00",
            "0XFFFFFF00",
            "0xff",
        ] {
            assert_eq!(convert(input).unwrap(), slash24, "{input}");
        }
        assert_eq!(
            convert("/19").unwrap(),
            mask(Some(19), "255.255.224.0", "0.0.31.255", "8192")
        );
        assert_eq!(
            convert("255.255.255.252").unwrap(),
            mask(Some(30), "255.255.255.252", "0.0.0.3", "4")
        );
        assert_eq!(
            convert("0.0.3.255").unwrap(),
            mask(Some(22), "255.255.252.0", "0.0.3.255", "1024")
        );
        assert_eq!(
            convert("128.0.0.0").unwrap(),
            mask(Some(1), "128.0.0.0", "127.255.255.255", "2147483648")
        );
        assert_eq!(
            convert("/31").unwrap(),
            mask(Some(31), "255.255.255.254", "0.0.0.1", "2")
        );
    }

    #[test]
    fn ambiguous_masks_are_netmasks() {
        let all = mask(Some(0), "0.0.0.0", "255.255.255.255", "4294967296");
        let host = mask(Some(32), "255.255.255.255", "0.0.0.0", "1");
        assert_eq!(convert("0").unwrap(), all);
        assert_eq!(convert("0.0.0.0").unwrap(), all);
        assert_eq!(convert("0x00000000").unwrap(), all);
        assert_eq!(convert("32").unwrap(), host);
        assert_eq!(convert("255.255.255.255").unwrap(), host);
    }

    #[test]
    fn non_contiguous_masks_have_no_prefix() {
        // Starts with 0-bits: a wildcard, e.g. "any value in the third octet".
        assert_eq!(
            convert("0.0.255.0").unwrap(),
            mask(None, "255.255.0.255", "0.0.255.0", "256")
        );
        assert_eq!(
            convert("0.0.0.254").unwrap(),
            mask(None, "255.255.255.1", "0.0.0.254", "128")
        );
        // Starts with 1-bits: a (discontiguous) netmask.
        assert_eq!(
            convert("255.255.0.255").unwrap(),
            mask(None, "255.255.0.255", "0.0.255.0", "256")
        );
        assert_eq!(
            convert("255.0.255.0").unwrap(),
            mask(None, "255.0.255.0", "0.255.0.255", "65536")
        );
        assert_eq!(
            convert("0xff00ff00").unwrap(),
            mask(None, "255.0.255.0", "0.255.0.255", "65536")
        );
    }

    #[test]
    fn convert_rejects_what_is_not_a_mask() {
        assert_eq!(
            convert("33").unwrap_err().to_string(),
            "invalid prefix length `33`: IPv4 prefix lengths range from 0 to 32 (IPv6 prefixes have no netmask or wildcard form)"
        );
        assert!(
            convert("/64")
                .unwrap_err()
                .to_string()
                .contains("IPv6 prefixes have no netmask")
        );
        assert!(convert("999999999999999999999").is_err());
        assert_eq!(
            convert("").unwrap_err().to_string(),
            "invalid mask: nothing was entered"
        );
        assert_eq!(
            convert("255.255.256.0").unwrap_err().to_string(),
            "invalid mask `255.255.256.0`: `256` is not a valid octet (0-255)"
        );
        assert_eq!(
            convert("255.255.0").unwrap_err().to_string(),
            "invalid mask `255.255.0`: expected 4 octets separated by dots, found 3"
        );
        assert!(
            convert("0xfffffff00")
                .unwrap_err()
                .to_string()
                .contains("up to 8 hex digits")
        );
        assert!(convert("0x").is_err());
        assert!(convert("0xzz").is_err());
        assert!(convert("/").is_err());
        assert!(convert("/255.255.255.0").is_err());
        assert!(convert("-24").is_err());
        assert!(
            convert("twenty-four")
                .unwrap_err()
                .to_string()
                .contains("expected a prefix length such as `24`")
        );
    }

    #[test]
    fn acl_matching() {
        // access-list 10 permit 10.1.2.0 0.0.0.255
        assert!(matches("10.1.2.0", "0.0.0.255", "10.1.2.77").unwrap());
        assert!(matches("10.1.2.0", "0.0.0.255", "10.1.2.0").unwrap());
        assert!(matches("10.1.2.0", "0.0.0.255", "10.1.2.255").unwrap());
        assert!(!matches("10.1.2.0", "0.0.0.255", "10.1.3.1").unwrap());
        // host and any
        assert!(matches("10.1.2.3", "0.0.0.0", "10.1.2.3").unwrap());
        assert!(!matches("10.1.2.3", "0.0.0.0", "10.1.2.4").unwrap());
        assert!(matches("0.0.0.0", "255.255.255.255", "203.0.113.9").unwrap());
        // Address bits under the wildcard do not matter.
        assert!(matches("10.1.2.99", "0.0.0.255", "10.1.2.1").unwrap());
    }

    #[test]
    fn non_contiguous_acl_matching() {
        // The third octet is "don't care": matches 10.1.x.5.
        assert!(matches("10.1.0.5", "0.0.255.0", "10.1.200.5").unwrap());
        assert!(!matches("10.1.0.5", "0.0.255.0", "10.1.200.6").unwrap());
        assert!(!matches("10.1.0.5", "0.0.255.0", "10.2.200.5").unwrap());
        // 0.0.0.254 keeps only the lowest bit: odd or even hosts.
        assert!(matches("192.168.1.1", "0.0.0.254", "192.168.1.77").unwrap());
        assert!(!matches("192.168.1.1", "0.0.0.254", "192.168.1.78").unwrap());
        assert!(matches("192.168.1.0", "0.0.0.254", "192.168.1.78").unwrap());
        // Every other /24 of 10.0.0.0/16: 10.0.{even}.x
        assert!(matches("10.0.0.0", "0.0.254.255", "10.0.12.200").unwrap());
        assert!(!matches("10.0.0.0", "0.0.254.255", "10.0.13.200").unwrap());
    }

    #[test]
    fn matching_rejects_bad_input() {
        assert_eq!(
            matches("10.1.2.0", "0.0.0.256", "10.1.2.3")
                .unwrap_err()
                .to_string(),
            "invalid wildcard mask `0.0.0.256`: `256` is not a valid octet (0-255)"
        );
        assert!(matches("10.1.2", "0.0.0.255", "10.1.2.3").is_err());
        assert!(matches("10.1.2.0", "0.0.0.255", "host").is_err());
        assert!(
            matches("2001:db8::", "0.0.0.255", "10.1.2.3")
                .unwrap_err()
                .to_string()
                .contains("IPv4 concept")
        );
        assert!(
            matches("10.1.2.0", "0.0.0.255", "2001:db8::1")
                .unwrap_err()
                .to_string()
                .contains("IPv4 concept")
        );
        assert!(matches("10.1.2.0", "/24", "10.1.2.3").is_err());
    }

    #[test]
    fn explains_prefix_style_wildcards() {
        assert_eq!(
            explain("10.1.2.0", "0.0.0.255").unwrap(),
            "`10.1.2.0 0.0.0.255` matches the 256 addresses of 10.1.2.0/24, from 10.1.2.0 to 10.1.2.255."
        );
        assert_eq!(
            explain("172.16.0.0", "0.15.255.255").unwrap(),
            "`172.16.0.0 0.15.255.255` matches the 1,048,576 addresses of 172.16.0.0/12, from 172.16.0.0 to 172.31.255.255."
        );
        assert_eq!(
            explain("10.1.2.3", "0.0.0.0").unwrap(),
            "`10.1.2.3 0.0.0.0` matches exactly one address, 10.1.2.3 (the same as `host 10.1.2.3`)."
        );
        assert_eq!(
            explain("0.0.0.0", "255.255.255.255").unwrap(),
            "`0.0.0.0 255.255.255.255` matches every IPv4 address, 0.0.0.0 to 255.255.255.255 (4,294,967,296 addresses; the same as `any`)."
        );
        assert_eq!(
            explain("10.1.2.99", "0.0.0.255").unwrap(),
            "`10.1.2.99 0.0.0.255` matches the 256 addresses of 10.1.2.0/24, from 10.1.2.0 to 10.1.2.255. \
             The address bits under the wildcard are ignored; the canonical form is `10.1.2.0 0.0.0.255`."
        );
    }

    #[test]
    fn explains_non_contiguous_wildcards() {
        assert_eq!(
            explain("10.1.0.5", "0.0.255.0").unwrap(),
            "`10.1.0.5 0.0.255.0` is a non-contiguous wildcard, so no single prefix describes it. It matches 256 addresses \
             between 10.1.0.5 and 10.1.255.5: octet 1 must be 10, octet 2 must be 1, octet 3 can be anything, octet 4 must be 5."
        );
        assert_eq!(
            explain("192.168.1.1", "0.0.0.254").unwrap(),
            "`192.168.1.1 0.0.0.254` is a non-contiguous wildcard, so no single prefix describes it. It matches 128 addresses \
             between 192.168.1.1 and 192.168.1.255: octet 1 must be 192, octet 2 must be 168, octet 3 must be 1, octet 4 must be odd."
        );
        assert!(
            explain("192.168.1.0", "0.0.0.254")
                .unwrap()
                .ends_with("octet 4 must be even.")
        );
        assert!(
            explain("10.0.0.0", "0.0.254.255")
                .unwrap()
                .contains("octet 3 must be even, octet 4 can be anything.")
        );
        // Low bits free in one octet, another octet fully free.
        assert!(explain("10.0.16.0", "0.255.7.0").unwrap().contains(
            "octet 2 can be anything, octet 3 must be between 16 and 23, octet 4 must be 0."
        ));
        // A few scattered bits: the values are listed.
        assert!(
            explain("10.0.0.1", "0.0.0.6")
                .unwrap()
                .contains("octet 4 must be one of 1, 3, 5, 7.")
        );
        assert!(
            explain("10.0.0.0", "0.0.0.129")
                .unwrap()
                .contains("octet 4 must be one of 0, 1, 128, 129.")
        );
        // Many scattered bits: a pattern.
        assert!(
            explain("10.0.0.1", "0.0.0.170")
                .unwrap()
                .contains("octet 4 must match the bit pattern x0x0x0x1 (x = any).")
        );
    }

    #[test]
    fn explain_points_out_a_subnet_mask_used_as_wildcard() {
        let text = explain("10.1.2.0", "255.255.255.0").unwrap();
        assert!(
            text.starts_with("`10.1.2.0 255.255.255.0` is a non-contiguous wildcard"),
            "{text}"
        );
        assert!(
            text.contains("It matches 16,777,216 addresses between 0.0.0.0 and 255.255.255.0"),
            "{text}"
        );
        assert!(
            text.ends_with("Note: 255.255.255.0 is a subnet mask; if you meant the network 10.1.2.0/24, its wildcard is 0.0.0.255."),
            "{text}"
        );
        // No such note for real wildcards or for host/any.
        assert!(!explain("10.1.2.0", "0.0.0.255").unwrap().contains("Note:"));
        assert!(!explain("10.1.2.0", "0.0.0.0").unwrap().contains("Note:"));
        assert!(
            !explain("10.1.2.0", "255.255.255.255")
                .unwrap()
                .contains("Note:")
        );
    }

    #[test]
    fn explain_agrees_with_matches() {
        // For every wildcard octet pattern, the first and last address match
        // and the count is 2^(wildcard bits).
        for wild in [
            0u32,
            1,
            6,
            0xFE,
            0x0000_FF00,
            0x00FF_00FF,
            0xFFFF_FFFF,
            0x0F0F_0F0F,
            0x8000_0001,
        ] {
            let address = 0xC0A8_0155 & !wild;
            let (address_text, wild_text) = (format_ipv4(address), format_ipv4(wild));
            let text = explain(&address_text, &wild_text).unwrap();
            assert!(matches(&address_text, &wild_text, &format_ipv4(address)).unwrap());
            assert!(matches(&address_text, &wild_text, &format_ipv4(address | wild)).unwrap());
            let count = group_thousands(&(1u64 << wild.count_ones()).to_string());
            if wild != 0 {
                assert!(text.contains(&count), "{text} should mention {count}");
            }
        }
    }
}
