//! IPv4 / IPv6 address and prefix primitives shared by the calculators.
//!
//! Addresses of both families are carried as a `u128` (IPv4 in the low 32
//! bits) so the prefix arithmetic is written once. Parsing is done by hand
//! rather than through `std::net` so that every rejection can say which part
//! of the input is wrong; the unit tests check the parsers against `std::net`.

use std::fmt;

use crate::ToolError;
use crate::error::snip;

/// Address family.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Family {
    V4,
    V6,
}

impl Family {
    /// Address length in bits.
    pub(crate) const fn bits(self) -> u8 {
        match self {
            Self::V4 => 32,
            Self::V6 => 128,
        }
    }

    /// The IP version number.
    pub(crate) const fn version(self) -> u8 {
        match self {
            Self::V4 => 4,
            Self::V6 => 6,
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::V4 => "IPv4",
            Self::V6 => "IPv6",
        }
    }
}

/// A single address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Addr {
    pub(crate) family: Family,
    pub(crate) value: u128,
}

impl Addr {
    pub(crate) fn v4(value: u32) -> Self {
        Self { family: Family::V4, value: u128::from(value) }
    }

    /// Parse a bare IPv4 or IPv6 address (no mask, surrounding whitespace ignored).
    pub(crate) fn parse(text: &str) -> Result<Self, ToolError> {
        let text = text.trim();
        if text.contains(':') {
            parse_ipv6(text).map(|value| Self { family: Family::V6, value })
        } else {
            parse_ipv4(text).map(Self::v4)
        }
    }
}

impl fmt::Display for Addr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.family {
            Family::V4 => f.write_str(&format_ipv4(low32(self.value))),
            Family::V6 => f.write_str(&format_ipv6(self.value)),
        }
    }
}

/// A CIDR block. `first` always has its host bits cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Net {
    pub(crate) family: Family,
    pub(crate) first: u128,
    pub(crate) prefix: u8,
}

impl Net {
    /// The block of length `prefix` that contains `addr`.
    pub(crate) fn containing(addr: Addr, prefix: u8) -> Self {
        let prefix = prefix.min(addr.family.bits());
        let host_mask = low_mask(addr.family.bits() - prefix);
        Self { family: addr.family, first: addr.value & !host_mask, prefix }
    }

    pub(crate) fn host_bits(self) -> u8 {
        self.family.bits() - self.prefix
    }

    /// The last address of the block (the broadcast address in IPv4).
    pub(crate) fn last(self) -> u128 {
        self.first | low_mask(self.host_bits())
    }

    pub(crate) fn network(self) -> Addr {
        Addr { family: self.family, value: self.first }
    }

    /// Whether `other` lies entirely inside this block.
    pub(crate) fn contains(self, other: Self) -> bool {
        self.family == other.family && self.first <= other.first && other.last() <= self.last()
    }

    pub(crate) fn overlaps(self, other: Self) -> bool {
        self.family == other.family && self.first <= other.last() && other.first <= self.last()
    }
}

impl fmt::Display for Net {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.network(), self.prefix)
    }
}

/// An address together with the network it was written in, e.g. `192.168.1.10/24`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Interface {
    pub(crate) addr: Addr,
    pub(crate) net: Net,
}

impl Interface {
    /// Parse `address`, `address/prefix`, `address/netmask`, `address netmask`
    /// or `address wildcard`. A missing mask means a host route (/32 or /128).
    ///
    /// A dotted mask that is valid both as a netmask and as a wildcard
    /// (`0.0.0.0`, `255.255.255.255`) is read as a netmask.
    pub(crate) fn parse(input: &str) -> Result<Self, ToolError> {
        let text = input.trim();
        if text.is_empty() {
            return Err(ToolError::invalid("network", "", "nothing was entered"));
        }
        let (addr_text, mask_text) = match text.split_once('/') {
            Some((addr, mask)) => (addr.trim(), Some(mask.trim())),
            None => {
                let mut tokens = text.split_whitespace();
                let addr = tokens.next().unwrap_or_default();
                let mask = tokens.next();
                if tokens.next().is_some() {
                    return Err(ToolError::invalid(
                        "network",
                        text,
                        "expected an address optionally followed by a prefix length or mask, \
                         e.g. `10.1.2.3/24` or `10.1.2.3 255.255.255.0`",
                    ));
                }
                (addr, mask)
            }
        };
        let addr = Addr::parse(addr_text)?;
        let prefix = match mask_text {
            None => addr.family.bits(),
            Some(mask) => parse_mask(addr.family, mask, text)?,
        };
        Ok(Self { addr, net: Net::containing(addr, prefix) })
    }
}

fn parse_mask(family: Family, mask: &str, input: &str) -> Result<u8, ToolError> {
    let bits = family.bits();
    if mask.is_empty() {
        return Err(ToolError::invalid(
            "network",
            input,
            format!("the prefix length is missing after the address (0-{bits})"),
        ));
    }
    if mask.bytes().all(|b| b.is_ascii_digit()) {
        return match mask.parse::<u8>() {
            Ok(prefix) if prefix <= bits => Ok(prefix),
            _ => Err(ToolError::invalid(
                "prefix length",
                mask,
                format!("{} prefix lengths range from 0 to {bits}", family.label()),
            )),
        };
    }
    match family {
        Family::V4 => {
            if !mask.contains('.') {
                return Err(ToolError::invalid(
                    "netmask",
                    mask,
                    "expected a prefix length (0-32) or a dotted mask such as `255.255.255.0`",
                ));
            }
            let value = parse_dotted_quad(mask, "netmask")?;
            prefix_from_netmask(value).or_else(|| prefix_from_netmask(!value)).ok_or_else(|| {
                ToolError::invalid(
                    "netmask",
                    mask,
                    "the 1-bits are not contiguous, so it is neither a netmask nor a prefix-style \
                     wildcard (the wildcard tool handles non-contiguous ACL masks)",
                )
            })
        }
        Family::V6 => Err(ToolError::invalid(
            "prefix length",
            mask,
            "IPv6 networks are written with a prefix length from 0 to 128, e.g. `2001:db8::/64`",
        )),
    }
}

// ---------------------------------------------------------------- bit helpers

/// A mask with the `bits` lowest bits set (`bits` may be 128).
pub(crate) fn low_mask(bits: u8) -> u128 {
    if bits >= 128 { u128::MAX } else { (1u128 << bits) - 1 }
}

/// The low 32 bits of `value`.
pub(crate) fn low32(value: u128) -> u32 {
    (value & 0xffff_ffff) as u32
}

/// 2^`exponent` in decimal; `exponent` may be 128, which no integer type holds.
pub(crate) fn pow2_string(exponent: u8) -> String {
    match 1u128.checked_shl(u32::from(exponent)) {
        Some(value) => value.to_string(),
        None => "340282366920938463463374607431768211456".to_owned(),
    }
}

/// The IPv4 netmask for a prefix length (clamped to 32).
pub(crate) fn netmask_v4(prefix: u8) -> u32 {
    match prefix {
        0 => 0,
        1..=31 => u32::MAX << (32 - prefix),
        _ => u32::MAX,
    }
}

/// The prefix length of a netmask whose 1-bits are contiguous from the left.
pub(crate) fn prefix_from_netmask(mask: u32) -> Option<u8> {
    let ones = mask.leading_ones();
    (mask.count_ones() == ones).then_some(ones as u8)
}

// ------------------------------------------------------------------- ranges

/// Sort inclusive ranges and merge those that overlap or touch.
pub(crate) fn merge_ranges(mut ranges: Vec<(u128, u128)>) -> Vec<(u128, u128)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u128, u128)> = Vec::with_capacity(ranges.len());
    for (first, last) in ranges {
        match merged.last_mut() {
            // `first - 1 <= end` without overflowing when `first` is 0.
            Some((_, end)) if first.saturating_sub(1) <= *end => *end = (*end).max(last),
            _ => merged.push((first, last)),
        }
    }
    merged
}

/// Append the smallest list of CIDR blocks that covers exactly `first..=last`.
pub(crate) fn range_to_nets(family: Family, first: u128, last: u128, out: &mut Vec<Net>) {
    let bits = family.bits();
    let mut cursor = first;
    while cursor <= last {
        // The block starting at `cursor` can be as large as the alignment of
        // `cursor` allows, but must not run past `last`.
        let alignment = if cursor == 0 { bits } else { (cursor.trailing_zeros() as u8).min(bits) };
        let span = last - cursor;
        let fit = if span == u128::MAX { 128 } else { (127 - (span + 1).leading_zeros()) as u8 };
        let host_bits = alignment.min(fit);
        out.push(Net { family, first: cursor, prefix: bits - host_bits });
        match (cursor | low_mask(host_bits)).checked_add(1) {
            Some(next) => cursor = next,
            None => break,
        }
    }
}

// ------------------------------------------------------------------ parsing

/// Parse a dotted-quad IPv4 address. Octets must be plain decimal: no
/// leading zeros (`010` could mean ten or, read as octal, eight).
pub(crate) fn parse_ipv4(text: &str) -> Result<u32, ToolError> {
    parse_dotted_quad(text, "IPv4 address")
}

/// Parse four dotted decimal octets; `what` names the value in error messages
/// ("IPv4 address", "netmask", "wildcard mask").
pub(crate) fn parse_dotted_quad(text: &str, what: &str) -> Result<u32, ToolError> {
    let fail = |reason: String| Err(ToolError::invalid(what, text, reason));
    if text.is_empty() {
        return fail("nothing was entered".to_owned());
    }
    let octets = text.split('.').count();
    if octets != 4 {
        return fail(format!("expected 4 octets separated by dots, found {octets}"));
    }
    let mut value: u32 = 0;
    for part in text.split('.') {
        if part.is_empty() {
            return fail("an octet is missing between or around the dots".to_owned());
        }
        if !part.bytes().all(|b| b.is_ascii_digit()) {
            return fail(format!("`{}` is not a decimal number", snip(part, 20)));
        }
        let octet = match part.parse::<u8>() {
            Ok(octet) if part.len() <= 3 => octet,
            _ => return fail(format!("`{}` is not a valid octet (0-255)", snip(part, 20))),
        };
        if part.len() > 1 && part.starts_with('0') {
            return fail(format!(
                "octet `{part}` has a leading zero, which some systems read as octal; write `{octet}`"
            ));
        }
        value = (value << 8) | u32::from(octet);
    }
    Ok(value)
}

/// Parse an IPv6 address in any RFC 4291 text form, including an embedded
/// dotted-quad tail. Zone identifiers (`%eth0`) are rejected.
pub(crate) fn parse_ipv6(text: &str) -> Result<u128, ToolError> {
    const WHAT: &str = "IPv6 address";
    let fail = |reason: String| Err(ToolError::invalid(WHAT, text, reason));
    if text.is_empty() {
        return fail("nothing was entered".to_owned());
    }
    if let Some(position) = text.find('%') {
        return fail(format!(
            "`{}` is a zone identifier, which names a local interface and is not part of the address; remove it",
            snip(&text[position..], 20)
        ));
    }

    // `head :: tail`, where `::` stands for one or more zero groups.
    let (head, tail, compressed) = match text.split_once("::") {
        Some((_, tail)) if tail.contains("::") => {
            return fail("`::` can appear only once in an address".to_owned());
        }
        Some((head, tail)) => (head, tail, true),
        None => (text, "", false),
    };

    let mut groups = [0u16; 8];
    let mut head_len = 0usize;
    let mut tail_groups = [0u16; 8];
    let mut tail_len = 0usize;
    // Only the very last group of the whole address may be a dotted quad.
    let head_may_end_in_v4 = !compressed;
    if let Err(reason) = parse_groups(head, head_may_end_in_v4, &mut groups, &mut head_len) {
        return fail(reason);
    }
    if let Err(reason) = parse_groups(tail, true, &mut tail_groups, &mut tail_len) {
        return fail(reason);
    }

    let written = head_len + tail_len;
    if compressed {
        if written > 7 {
            return fail("`::` stands for at least one group of zeros, but all 8 groups are already written".to_owned());
        }
        groups[8 - tail_len..].copy_from_slice(&tail_groups[..tail_len]);
    } else if written != 8 {
        return fail(format!(
            "an IPv6 address has 8 groups of 16 bits, this has {written} (use `::` to abbreviate a run of zeros)"
        ));
    }
    Ok(groups.iter().fold(0u128, |value, group| (value << 16) | u128::from(*group)))
}

/// Parse the colon-separated groups on one side of `::` into `out`.
fn parse_groups(side: &str, may_end_in_v4: bool, out: &mut [u16; 8], len: &mut usize) -> Result<(), String> {
    if side.is_empty() {
        return Ok(());
    }
    let last_index = side.split(':').count() - 1;
    for (index, group) in side.split(':').enumerate() {
        if group.is_empty() {
            return Err("a group is empty: there is a stray `:`".to_owned());
        }
        if group.contains('.') {
            if !(may_end_in_v4 && index == last_index) {
                return Err(format!(
                    "the dotted IPv4 part `{}` is only allowed as the last 32 bits",
                    snip(group, 20)
                ));
            }
            let v4 = parse_ipv4(group).map_err(|error| match error {
                ToolError::Invalid { reason, .. } => format!("in the embedded IPv4 part, {reason}"),
                other => other.to_string(),
            })?;
            if *len + 2 > 8 {
                return Err("there are more than 8 groups".to_owned());
            }
            out[*len] = (v4 >> 16) as u16;
            out[*len + 1] = (v4 & 0xffff) as u16;
            *len += 2;
            continue;
        }
        if !group.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!("`{}` is not hexadecimal", snip(group, 20)));
        }
        if group.len() > 4 {
            return Err(format!("group `{}` has more than 4 hex digits", snip(group, 20)));
        }
        let value = u16::from_str_radix(group, 16).map_err(|error| error.to_string())?;
        if *len == 8 {
            return Err("there are more than 8 groups".to_owned());
        }
        out[*len] = value;
        *len += 1;
    }
    Ok(())
}

// --------------------------------------------------------------- formatting

pub(crate) fn format_ipv4(value: u32) -> String {
    let [a, b, c, d] = value.to_be_bytes();
    format!("{a}.{b}.{c}.{d}")
}

fn groups_of(value: u128) -> [u16; 8] {
    std::array::from_fn(|index| ((value >> (112 - 16 * index)) & 0xffff) as u16)
}

/// RFC 5952 text form: lowercase, no leading zeros, the longest run of two or
/// more zero groups (the first one on a tie) written as `::`. IPv4-mapped
/// addresses keep their dotted tail (`::ffff:192.0.2.1`), as section 5 recommends.
pub(crate) fn format_ipv6(value: u128) -> String {
    if value >> 32 == 0xffff {
        return format!("::ffff:{}", format_ipv4(low32(value)));
    }
    let groups = groups_of(value);

    let (mut best_start, mut best_len) = (0usize, 0usize);
    let mut index = 0;
    while index < 8 {
        if groups[index] != 0 {
            index += 1;
            continue;
        }
        let start = index;
        while index < 8 && groups[index] == 0 {
            index += 1;
        }
        if index - start > best_len {
            (best_start, best_len) = (start, index - start);
        }
    }

    let hex = |part: &[u16]| part.iter().map(|group| format!("{group:x}")).collect::<Vec<_>>().join(":");
    if best_len < 2 {
        return hex(&groups);
    }
    format!("{}::{}", hex(&groups[..best_start]), hex(&groups[best_start + best_len..]))
}

/// All eight groups, four hex digits each.
pub(crate) fn expand_ipv6(value: u128) -> String {
    groups_of(value).iter().map(|group| format!("{group:04x}")).collect::<Vec<_>>().join(":")
}

/// `1234567` → `1,234,567`, for counts shown in prose.
pub(crate) fn group_thousands(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use pretty_assertions::assert_eq;

    use super::*;

    fn reason(error: ToolError) -> String {
        match error {
            ToolError::Invalid { reason, .. } => reason,
            other => panic!("unexpected error {other:?}"),
        }
    }

    #[test]
    fn ipv4_accepts_plain_dotted_quads() {
        assert_eq!(parse_ipv4("0.0.0.0"), Ok(0));
        assert_eq!(parse_ipv4("192.168.1.10"), Ok(0xC0A8_010A));
        assert_eq!(parse_ipv4("255.255.255.255"), Ok(u32::MAX));
    }

    #[test]
    fn ipv4_rejections_name_the_problem() {
        assert_eq!(
            parse_ipv4("10.0.300.1").unwrap_err().to_string(),
            "invalid IPv4 address `10.0.300.1`: `300` is not a valid octet (0-255)"
        );
        assert_eq!(reason(parse_ipv4("10.0.0").unwrap_err()), "expected 4 octets separated by dots, found 3");
        assert_eq!(reason(parse_ipv4("10.0.0.1.2").unwrap_err()), "expected 4 octets separated by dots, found 5");
        assert_eq!(reason(parse_ipv4("10..0.1").unwrap_err()), "an octet is missing between or around the dots");
        assert_eq!(reason(parse_ipv4("10.0.0.x1").unwrap_err()), "`x1` is not a decimal number");
        assert_eq!(reason(parse_ipv4("10.0.0.-1").unwrap_err()), "`-1` is not a decimal number");
        assert_eq!(reason(parse_ipv4("10.0.0.+1").unwrap_err()), "`+1` is not a decimal number");
        assert_eq!(reason(parse_ipv4("1.2.3.0256").unwrap_err()), "`0256` is not a valid octet (0-255)");
        assert_eq!(
            reason(parse_ipv4("010.1.1.1").unwrap_err()),
            "octet `010` has a leading zero, which some systems read as octal; write `10`"
        );
        assert_eq!(reason(parse_ipv4("").unwrap_err()), "nothing was entered");
        // Unicode digits are not decimal digits here.
        assert!(parse_ipv4("١.2.3.4").is_err());
    }

    #[test]
    fn ipv6_accepts_every_rfc4291_form() {
        for (text, expected) in [
            ("::", 0u128),
            ("::1", 1),
            ("1::", 1 << 112),
            ("2001:db8::1", 0x2001_0db8_0000_0000_0000_0000_0000_0001),
            ("2001:0DB8:0000:0000:0000:0000:0000:0001", 0x2001_0db8_0000_0000_0000_0000_0000_0001),
            ("fe80::1:2:3:4", 0xfe80_0000_0000_0000_0001_0002_0003_0004),
            ("1:2:3:4:5:6:7::", 0x0001_0002_0003_0004_0005_0006_0007_0000),
            ("::2:3:4:5:6:7:8", 0x0000_0002_0003_0004_0005_0006_0007_0008),
            ("::ffff:192.168.1.1", 0xffff_c0a8_0101),
            ("64:ff9b::192.0.2.33", 0x0064_ff9b_0000_0000_0000_0000_c000_0221),
            ("1:2:3:4:5:6:1.2.3.4", 0x0001_0002_0003_0004_0005_0006_0102_0304),
        ] {
            assert_eq!(parse_ipv6(text), Ok(expected), "{text}");
        }
    }

    #[test]
    fn ipv6_rejections_name_the_problem() {
        let why = |text: &str| reason(parse_ipv6(text).unwrap_err());
        assert_eq!(
            why("fe80::1%eth0"),
            "`%eth0` is a zone identifier, which names a local interface and is not part of the address; remove it"
        );
        assert_eq!(why("1::2::3"), "`::` can appear only once in an address");
        assert_eq!(why(":::"), "a group is empty: there is a stray `:`");
        assert_eq!(why("1:2:3:4:5:6:7:8:"), "a group is empty: there is a stray `:`");
        assert_eq!(why(":1:2:3:4:5:6:7:8"), "a group is empty: there is a stray `:`");
        assert_eq!(
            why("1:2:3:4:5:6:7"),
            "an IPv6 address has 8 groups of 16 bits, this has 7 (use `::` to abbreviate a run of zeros)"
        );
        assert_eq!(why("1:2:3:4:5:6:7:8:9"), "there are more than 8 groups");
        assert_eq!(
            why("1:2:3:4:5:6:7::8"),
            "`::` stands for at least one group of zeros, but all 8 groups are already written"
        );
        assert_eq!(why("2001:db8::12345"), "group `12345` has more than 4 hex digits");
        assert_eq!(why("00f256::"), "group `00f256` has more than 4 hex digits");
        assert_eq!(why("2001:db8::g"), "`g` is not hexadecimal");
        assert_eq!(why("2001:db8::+1"), "`+1` is not hexadecimal");
        assert_eq!(why("1.2.3.4::"), "the dotted IPv4 part `1.2.3.4` is only allowed as the last 32 bits");
        assert_eq!(why("::1.2.3.4:5"), "the dotted IPv4 part `1.2.3.4` is only allowed as the last 32 bits");
        assert_eq!(why("::ffff:1.2.3.256"), "in the embedded IPv4 part, `256` is not a valid octet (0-255)");
        assert_eq!(why("1:2:3:4:5:6:7:1.2.3.4"), "there are more than 8 groups");
    }

    #[test]
    fn ipv6_formats_per_rfc5952() {
        for (value, compressed) in [
            (0u128, "::"),
            (1, "::1"),
            (1 << 112, "1::"),
            (0x2001_0db8_0000_0000_0000_0000_0000_0001, "2001:db8::1"),
            // A single zero group is not compressed.
            (0x2001_0db8_0000_0001_0001_0001_0001_0001, "2001:db8:0:1:1:1:1:1"),
            // The longest run wins ...
            (0x2001_0000_0000_0001_0000_0000_0000_0001, "2001:0:0:1::1"),
            // ... and the first one on a tie.
            (0x2001_0db8_0000_0000_0001_0000_0000_0001, "2001:db8::1:0:0:1"),
            (0xffff_c0a8_0101, "::ffff:192.168.1.1"),
            (0xfe80_0000_0000_0000_0000_00ff_fe00_0000, "fe80::ff:fe00:0"),
            (u128::MAX, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"),
        ] {
            assert_eq!(format_ipv6(value), compressed);
        }
        assert_eq!(expand_ipv6(0x2001_0db8_0000_0000_0000_0000_0000_0001), "2001:0db8:0000:0000:0000:0000:0000:0001");
    }

    /// A small deterministic generator, so the comparison with `std::net` covers many shapes.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            self.0 >> 11
        }

        fn below(&mut self, bound: usize) -> usize {
            (self.next() % bound as u64) as usize
        }
    }

    #[test]
    fn ipv6_text_round_trips_and_matches_std() {
        let mut rng = Lcg(0x5eed);
        for _ in 0..20_000 {
            // Sparse addresses, so zero runs of every length and position occur.
            let mut value = 0u128;
            for _ in 0..8 {
                let group = match rng.below(3) {
                    0 => 0,
                    1 => rng.below(4) as u128,
                    _ => rng.below(0x1_0000) as u128,
                };
                value = (value << 16) | group;
            }
            let std_addr = Ipv6Addr::from(value);
            assert_eq!(format_ipv6(value), std_addr.to_string());
            assert_eq!(parse_ipv6(&format_ipv6(value)), Ok(value));
            assert_eq!(parse_ipv6(&expand_ipv6(value)), Ok(value));
        }
    }

    #[test]
    fn parsers_agree_with_std_on_random_text() {
        const ALPHABET: &[&str] =
            &["0", "1", "9", "a", "f", "F", "00", "255", "256", "ffff", "10000", ":", ":", "::", ".", ".", "g", "%", " "];
        let mut rng = Lcg(42);
        let mut accepted = 0;
        for _ in 0..200_000 {
            let pieces = 1 + rng.below(14);
            let text: String = (0..pieces).map(|_| ALPHABET[rng.below(ALPHABET.len())]).collect();
            let ours_v6 = parse_ipv6(&text).ok();
            let std_v6 = text.parse::<Ipv6Addr>().ok().map(u128::from);
            assert_eq!(ours_v6, std_v6, "IPv6 `{text}`");
            let ours_v4 = parse_ipv4(&text).ok();
            let std_v4 = text.parse::<Ipv4Addr>().ok().map(u32::from);
            assert_eq!(ours_v4, std_v4, "IPv4 `{text}`");
            accepted += usize::from(ours_v6.is_some() || ours_v4.is_some());
        }
        assert!(accepted > 500, "the generator should produce valid addresses too ({accepted})");
    }

    #[test]
    fn interface_forms() {
        let parsed = |text: &str| {
            let interface = Interface::parse(text).unwrap();
            (interface.addr.to_string(), interface.net.to_string())
        };
        let expected = ("192.168.1.10".to_owned(), "192.168.1.0/24".to_owned());
        assert_eq!(parsed("192.168.1.10/24"), expected);
        assert_eq!(parsed("192.168.1.10 255.255.255.0"), expected);
        assert_eq!(parsed("192.168.1.10/255.255.255.0"), expected);
        assert_eq!(parsed("192.168.1.10 0.0.0.255"), expected);
        assert_eq!(parsed("  192.168.1.10 / 24 "), expected);
        assert_eq!(parsed("192.168.1.10\t24"), expected);
        assert_eq!(parsed("192.168.1.10"), ("192.168.1.10".to_owned(), "192.168.1.10/32".to_owned()));
        assert_eq!(parsed("2001:db8::1/64"), ("2001:db8::1".to_owned(), "2001:db8::/64".to_owned()));
        assert_eq!(parsed("2001:db8::1"), ("2001:db8::1".to_owned(), "2001:db8::1/128".to_owned()));
        // Ambiguous masks are netmasks.
        assert_eq!(parsed("10.1.2.3 0.0.0.0").1, "0.0.0.0/0");
        assert_eq!(parsed("10.1.2.3 255.255.255.255").1, "10.1.2.3/32");
    }

    #[test]
    fn interface_rejections() {
        let message = |text: &str| Interface::parse(text).unwrap_err().to_string();
        assert_eq!(message("   "), "invalid network: nothing was entered");
        assert_eq!(message("10.0.0.1/33"), "invalid prefix length `33`: IPv4 prefix lengths range from 0 to 32");
        assert_eq!(message("2001:db8::/129"), "invalid prefix length `129`: IPv6 prefix lengths range from 0 to 128");
        assert_eq!(message("10.0.0.1/"), "invalid network `10.0.0.1/`: the prefix length is missing after the address (0-32)");
        assert!(message("10.0.0.1 255.0.255.0").starts_with("invalid netmask `255.0.255.0`: the 1-bits are not contiguous"));
        assert_eq!(message("10.0.0.1/255.255.256.0"), "invalid netmask `255.255.256.0`: `256` is not a valid octet (0-255)");
        assert!(message("2001:db8::1/ffff::").contains("IPv6 networks are written with a prefix length"));
        assert!(message("10.0.0.1 255.255.255.0 extra").contains("expected an address optionally followed by"));
        assert!(message("10.0.0.1/99999999999999999999").contains("range from 0 to 32"));
        assert_eq!(
            message("10.0.0.0/24/25"),
            "invalid netmask `24/25`: expected a prefix length (0-32) or a dotted mask such as `255.255.255.0`"
        );
        assert_eq!(message("10.0.0.0 255.255.0"), "invalid netmask `255.255.0`: expected 4 octets separated by dots, found 3");
    }

    #[test]
    fn masks_and_powers() {
        assert_eq!(netmask_v4(0), 0);
        assert_eq!(netmask_v4(24), 0xFFFF_FF00);
        assert_eq!(netmask_v4(32), u32::MAX);
        assert_eq!(prefix_from_netmask(0), Some(0));
        assert_eq!(prefix_from_netmask(0xFFFF_FE00), Some(23));
        assert_eq!(prefix_from_netmask(u32::MAX), Some(32));
        assert_eq!(prefix_from_netmask(0xFF00_FF00), None);
        assert_eq!(low_mask(0), 0);
        assert_eq!(low_mask(8), 255);
        assert_eq!(low_mask(128), u128::MAX);
        assert_eq!(pow2_string(0), "1");
        assert_eq!(pow2_string(64), "18446744073709551616");
        assert_eq!(pow2_string(127), "170141183460469231731687303715884105728");
        assert_eq!(pow2_string(128), "340282366920938463463374607431768211456");
        assert_eq!(group_thousands("1"), "1");
        assert_eq!(group_thousands("1234"), "1,234");
        assert_eq!(group_thousands("123456"), "123,456");
        assert_eq!(group_thousands("1234567"), "1,234,567");
    }

    #[test]
    fn ranges_merge_and_decompose() {
        assert_eq!(merge_ranges(vec![(10, 20), (0, 5), (21, 30), (6, 7), (100, 200), (150, 160)]), vec![
            (0, 7),
            (10, 30),
            (100, 200)
        ]);
        assert_eq!(merge_ranges(vec![(0, u128::MAX), (0, 0), (5, 9)]), vec![(0, u128::MAX)]);

        let cidrs = |family, first, last| {
            let mut nets = Vec::new();
            range_to_nets(family, first, last, &mut nets);
            nets.iter().map(Net::to_string).collect::<Vec<_>>()
        };
        let v4 = |text: &str| u128::from(parse_ipv4(text).unwrap());
        assert_eq!(cidrs(Family::V4, v4("10.0.0.5"), v4("10.0.1.20")), [
            "10.0.0.5/32",
            "10.0.0.6/31",
            "10.0.0.8/29",
            "10.0.0.16/28",
            "10.0.0.32/27",
            "10.0.0.64/26",
            "10.0.0.128/25",
            "10.0.1.0/28",
            "10.0.1.16/30",
            "10.0.1.20/32"
        ]);
        assert_eq!(cidrs(Family::V4, 0, u128::from(u32::MAX)), ["0.0.0.0/0"]);
        assert_eq!(cidrs(Family::V6, 0, u128::MAX), ["::/0"]);
        assert_eq!(cidrs(Family::V6, 1, u128::MAX).len(), 128);
        assert_eq!(cidrs(Family::V4, v4("255.255.255.255"), v4("255.255.255.255")), ["255.255.255.255/32"]);
    }
}
