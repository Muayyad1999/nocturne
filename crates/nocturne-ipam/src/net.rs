//! The numeric side of an address or a prefix.
//!
//! Every *decision* about subnets — canonical form, validation, containment,
//! overlap, splitting, the next free block, free space, VLSM — is made by
//! [`nocturne_tools::subnet`] and [`nocturne_tools::vlsm`], which are the
//! tested arithmetic of this project. What lives here is only the numeric
//! form of what those functions return, because a prefix with fifty thousand
//! addresses in it cannot afford to re-parse a string for every comparison,
//! and because a list of addresses has to sort in address order rather than
//! alphabetically (`10.1.1.9` before `10.1.1.10`).

use std::net::IpAddr;

use nocturne_app::{AppError, AppResult};
use nocturne_tools::{ToolError, subnet};

/// A tool error as the person at the keyboard should see it: its messages are
/// already written for them ("`300` is not a valid octet in `10.0.300.1`").
pub fn tool_error(err: ToolError) -> AppError {
    AppError::invalid(err.to_string())
}

/// All ones in the low `bits`.
pub fn low_mask(bits: u8) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

/// `2^exponent` in decimal, exactly, for counts that outgrow `u64` (and, at
/// `/0` of IPv6, `u128`).
pub fn pow2_text(exponent: u32) -> String {
    if exponent < 128 {
        (1u128 << exponent).to_string()
    } else {
        // 2^128, the size of the whole IPv6 address space.
        "340282366920938463463374607431768211456".to_string()
    }
}

/// One address: its canonical text, its family and its numeric value.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Addr {
    pub family: u8,
    pub value: u128,
    pub text: String,
}

/// Lowercase fixed-width hex, so that SQLite's own string ordering is address
/// order and `BETWEEN` is a prefix's range.
pub fn key_of(family: u8, value: u128) -> String {
    if family == 4 {
        format!("{:08x}", value as u32)
    } else {
        format!("{value:032x}")
    }
}

pub fn to_value(ip: IpAddr) -> u128 {
    match ip {
        IpAddr::V4(v4) => u128::from(u32::from(v4)),
        IpAddr::V6(v6) => u128::from(v6),
    }
}

pub fn to_ip(family: u8, value: u128) -> IpAddr {
    if family == 4 {
        IpAddr::V4(std::net::Ipv4Addr::from(value as u32))
    } else {
        IpAddr::V6(std::net::Ipv6Addr::from(value))
    }
}

/// The canonical text of an address (RFC 5952 for IPv6, lower case).
pub fn text_of(family: u8, value: u128) -> String {
    to_ip(family, value).to_string()
}

impl Addr {
    pub fn new(family: u8, value: u128) -> Addr {
        Addr {
            family,
            value,
            text: text_of(family, value),
        }
    }

    pub fn key(&self) -> String {
        key_of(self.family, self.value)
    }
}

/// Read an address. A prefix is accepted and its host part taken, because a
/// person pasting `10.1.10.7/24` means the address.
pub fn addr(input: &str) -> AppResult<Addr> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AppError::invalid("no address was given"));
    }
    let info = subnet::info(input).map_err(tool_error)?;
    let ip: IpAddr = info.address.parse().map_err(|_| {
        AppError::internal(format!(
            "`{}` could not be read back as an address",
            info.address
        ))
    })?;
    Ok(Addr::new(info.version, to_value(ip)))
}

/// A prefix as numbers: the network address, the length and the family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Net {
    pub family: u8,
    pub base: u128,
    pub len: u8,
}

impl Net {
    pub fn bits(&self) -> u8 {
        if self.family == 4 { 32 } else { 128 }
    }

    /// The last address of the block (the broadcast address of an IPv4 block
    /// that has one).
    pub fn last(&self) -> u128 {
        self.base | low_mask(self.bits() - self.len)
    }

    /// How many addresses the block holds, exactly, as decimal text.
    pub fn total_text(&self) -> String {
        pow2_text(u32::from(self.bits() - self.len))
    }

    /// How many addresses the block holds, saturated where it does not fit.
    pub fn total(&self) -> u64 {
        let exponent = u32::from(self.bits() - self.len);
        if exponent >= 64 {
            u64::MAX
        } else {
            1u64 << exponent
        }
    }

    pub fn contains_value(&self, family: u8, value: u128) -> bool {
        family == self.family && value >= self.base && value <= self.last()
    }

    pub fn contains_addr(&self, address: &Addr) -> bool {
        self.contains_value(address.family, address.value)
    }

    /// Containment of a whole block, which is also what makes one prefix the
    /// parent of another.
    pub fn contains_net(&self, other: &Net) -> bool {
        other.family == self.family
            && other.len >= self.len
            && other.base >= self.base
            && other.last() <= self.last()
    }

    pub fn overlaps(&self, other: &Net) -> bool {
        self.family == other.family && self.base <= other.last() && other.base <= self.last()
    }

    pub fn cidr(&self) -> String {
        format!("{}/{}", text_of(self.family, self.base), self.len)
    }

    /// The network address of the block, as a key.
    pub fn key(&self) -> String {
        key_of(self.family, self.base)
    }

    pub fn last_key(&self) -> String {
        key_of(self.family, self.last())
    }

    /// How many host bits the block has, which is what decides whether it
    /// has a network address at all.
    pub fn host_bits(&self) -> u8 {
        self.bits() - self.len
    }

    /// Addresses a host can be given. A `/31` and a `/127` are
    /// point-to-point links with two usable addresses (RFC 3021, RFC 6164)
    /// and a `/32` or `/128` is one host; above those, IPv4 loses its
    /// network and broadcast addresses and IPv6 loses only the Subnet-Router
    /// anycast address at the bottom of the subnet. These are the same
    /// conventions `nocturne_tools::subnet` uses, and the tests check that
    /// they stay the same.
    pub fn usable_text(&self) -> String {
        match (self.host_bits(), self.family) {
            (0, _) => "1".to_string(),
            (1, _) => "2".to_string(),
            (_, 4) => (self.total() - 2).to_string(),
            (_, _) => decimal_minus_one(&self.total_text()),
        }
    }

    /// The same number, saturated where it does not fit.
    pub fn usable(&self) -> u64 {
        let exponent = u32::from(self.host_bits());
        match (self.host_bits(), self.family) {
            (0, _) => 1,
            (1, _) => 2,
            (_, 4) => self.total().saturating_sub(2),
            _ if exponent >= 64 => u64::MAX,
            _ => self.total().saturating_sub(1),
        }
    }

    /// Whether `value` is an address a host may have in this block.
    pub fn is_usable_host(&self, value: u128) -> bool {
        if !self.contains_value(self.family, value) {
            return false;
        }
        match (self.host_bits(), self.family) {
            (0 | 1, _) => true,
            (_, 4) => value != self.base && value != self.last(),
            // The all-zeros address of an IPv6 subnet is the Subnet-Router anycast address.
            (_, _) => value != self.base,
        }
    }

    /// The first and last address a host may have.
    pub fn host_range(&self) -> (u128, u128) {
        match (self.host_bits(), self.family) {
            (0 | 1, _) => (self.base, self.last()),
            (_, 4) => (self.base + 1, self.last() - 1),
            (_, _) => (self.base + 1, self.last()),
        }
    }
}

/// Decimal subtraction of one, for IPv6 counts that do not fit any integer.
fn decimal_minus_one(digits: &str) -> String {
    let mut out: Vec<u8> = digits.as_bytes().to_vec();
    for index in (0..out.len()).rev() {
        if out[index] > b'0' {
            out[index] -= 1;
            break;
        }
        out[index] = b'9';
    }
    let text = String::from_utf8(out).unwrap_or_else(|_| digits.to_string());
    let trimmed = text.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// What a prefix given as text turned out to be.
pub struct ParsedNet {
    /// Canonical `network/length`.
    pub cidr: String,
    pub net: Net,
    /// `false` when the input had host bits set; `cidr` is then the network
    /// the person probably meant.
    pub was_canonical: bool,
    /// The address as given, which is what the correction message shows.
    pub given: String,
}

/// Read a prefix in any form [`subnet::info`] accepts (CIDR, netmask,
/// wildcard, a bare address, IPv6).
pub fn net(input: &str) -> AppResult<ParsedNet> {
    let input = input.trim();
    if input.is_empty() {
        return Err(AppError::invalid("no prefix was given"));
    }
    let info = subnet::info(input).map_err(tool_error)?;
    let network: IpAddr = info.network.parse().map_err(|_| {
        AppError::internal(format!(
            "`{}` could not be read back as an address",
            info.network
        ))
    })?;
    let base = to_value(network);
    // `SubnetInfo::is_network_address` is false for a /32 and a /128, which
    // have no network address to be; what matters here is whether the address
    // given *is* the base of the block, so it is compared directly.
    let given: IpAddr = info.address.parse().map_err(|_| {
        AppError::internal(format!(
            "`{}` could not be read back as an address",
            info.address
        ))
    })?;
    Ok(ParsedNet {
        cidr: info.cidr.clone(),
        net: Net {
            family: info.version,
            base,
            len: info.prefix,
        },
        was_canonical: to_value(given) == base,
        given: info.address.clone(),
    })
}

/// Read a prefix and refuse host bits, naming the network to write instead.
pub fn strict_net(input: &str) -> AppResult<(String, Net)> {
    let parsed = net(input)?;
    if !parsed.was_canonical {
        return Err(AppError::invalid(format!(
            "`{}` has host bits set: the prefix is `{}` (write that, or `{}` as an address)",
            input.trim(),
            parsed.cidr,
            parsed.given
        ))
        .with_details(serde_json::json!({
            "corrected": parsed.cidr,
            "address": parsed.given,
        })));
    }
    Ok((parsed.cidr, parsed.net))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The numbers here are the arithmetic the whole feature stands on, so
    /// they are also checked against the tool crate that owns it.
    #[test]
    fn a_blocks_edges_agree_with_the_subnet_calculator() {
        for cidr in [
            "10.0.0.0/8",
            "10.1.10.0/24",
            "10.1.10.0/30",
            "192.168.1.0/31",
            "192.168.1.7/32",
            "2001:db8::/64",
            "2001:db8::/127",
            "2001:db8::1/128",
            "0.0.0.0/0",
            "::/0",
        ] {
            let info = subnet::info(cidr).unwrap();
            let parsed = net(cidr).unwrap();
            assert_eq!(parsed.net.cidr(), info.cidr, "{cidr}");
            assert_eq!(parsed.net.total_text(), info.total_addresses, "{cidr}");
            assert_eq!(parsed.net.usable_text(), info.usable_hosts, "{cidr}");
            let (first, last) = parsed.net.host_range();
            assert_eq!(text_of(parsed.net.family, first), info.first_host, "{cidr}");
            assert_eq!(text_of(parsed.net.family, last), info.last_host, "{cidr}");
        }
    }

    #[test]
    fn a_thirty_one_has_two_hosts_and_a_thirty_two_has_one() {
        let p127 = net("2001:db8::/127").unwrap().net;
        assert_eq!(p127.usable(), 2, "RFC 6164 point-to-point");
        assert!(p127.is_usable_host(p127.base) && p127.is_usable_host(p127.last()));
        let p31 = net("192.168.1.0/31").unwrap().net;
        assert_eq!(p31.usable(), 2);
        assert!(p31.is_usable_host(p31.base) && p31.is_usable_host(p31.last()));
        let p32 = net("192.168.1.7/32").unwrap().net;
        assert_eq!(
            (p32.usable(), p32.base, p32.last()),
            (1, p32.base, p32.base)
        );
        assert!(p32.is_usable_host(p32.base));
        let p24 = net("10.1.10.0/24").unwrap().net;
        assert!(
            !p24.is_usable_host(p24.base),
            "the network address is not a host"
        );
        assert!(
            !p24.is_usable_host(p24.last()),
            "the broadcast address is not a host"
        );
        assert!(p24.is_usable_host(p24.base + 1) && p24.is_usable_host(p24.last() - 1));
        let v6 = net("2001:db8::/64").unwrap().net;
        assert!(
            !v6.is_usable_host(v6.base),
            "the Subnet-Router anycast address is not a host"
        );
        assert!(
            v6.is_usable_host(v6.last()),
            "IPv6 has no broadcast address"
        );
    }

    #[test]
    fn counts_too_large_for_an_integer_stay_exact_as_text() {
        assert_eq!(
            net("2001:db8::/64").unwrap().net.total_text(),
            "18446744073709551616"
        );
        assert_eq!(
            net("2001:db8::/64").unwrap().net.usable_text(),
            "18446744073709551615"
        );
        assert_eq!(
            net("::/0").unwrap().net.total_text(),
            "340282366920938463463374607431768211456"
        );
        assert_eq!(
            net("2001:db8::/64").unwrap().net.usable(),
            u64::MAX,
            "saturated, and `usable_text` is the truth"
        );
        assert_eq!(decimal_minus_one("1000"), "999");
        assert_eq!(decimal_minus_one("1"), "0");
    }

    #[test]
    fn host_bits_are_refused_with_the_network_to_write_instead() {
        let err = strict_net("10.1.10.7/24").unwrap_err();
        assert_eq!(err.code, "invalid");
        assert!(err.message.contains("10.1.10.0/24"), "{}", err.message);
        assert_eq!(err.details.unwrap()["corrected"], "10.1.10.0/24");
        assert_eq!(strict_net("10.1.10.0/24").unwrap().0, "10.1.10.0/24");
        // A netmask, a wildcard and a bare address are all prefixes too.
        assert_eq!(
            strict_net("10.1.10.0 255.255.255.0").unwrap().0,
            "10.1.10.0/24"
        );
        assert_eq!(strict_net("10.1.10.0 0.0.0.255").unwrap().0, "10.1.10.0/24");
        assert_eq!(strict_net("10.1.10.7").unwrap().0, "10.1.10.7/32");
        assert_eq!(strict_net("nonsense").unwrap_err().code, "invalid");
    }

    #[test]
    fn addresses_sort_and_canonicalise() {
        let a = addr("10.1.10.9").unwrap();
        let b = addr("10.1.10.10").unwrap();
        assert!(
            a.key() < b.key(),
            "keys sort in address order, not alphabetically"
        );
        assert_eq!(addr("2001:0DB8:0000::0001").unwrap().text, "2001:db8::1");
        assert_eq!(addr(" 10.1.10.7/24 ").unwrap().text, "10.1.10.7");
        assert_eq!(addr("10.1.10.7").unwrap().family, 4);
        assert_eq!(addr("::1").unwrap().family, 6);
        assert_eq!(addr("").unwrap_err().code, "invalid");
        assert_eq!(addr("10.0.300.1").unwrap_err().code, "invalid");
    }

    #[test]
    fn containment_and_overlap_agree_with_the_subnet_calculator() {
        let cases = [
            ("10.0.0.0/8", "10.1.0.0/16"),
            ("10.1.0.0/16", "10.2.0.0/16"),
            ("2001:db8::/32", "2001:db8:1::/48"),
            ("10.0.0.0/24", "10.0.0.0/24"),
        ];
        for (a, b) in cases {
            let (left, right) = (net(a).unwrap().net, net(b).unwrap().net);
            assert_eq!(
                left.overlaps(&right),
                subnet::overlaps(a, b).unwrap(),
                "{a} {b}"
            );
            assert_eq!(
                left.contains_net(&right),
                subnet::contains(a, &right.cidr()).unwrap() && right.len >= left.len,
                "{a} {b}"
            );
        }
        // Families never contain one another.
        assert!(
            !net("0.0.0.0/0")
                .unwrap()
                .net
                .contains_net(&net("::/0").unwrap().net)
        );
    }
}
