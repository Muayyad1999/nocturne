//! Subnet calculator: everything about one network, and set operations on several.
//!
//! Networks are read leniently, the way an interface address is written:
//! `192.168.1.10/24` means "the /24 that contains 192.168.1.10", so host bits
//! need not be zero. Everything works for IPv4 and IPv6 alike.
//!
//! Host conventions: an IPv4 /31 has two usable addresses (RFC 3021) and a /32
//! one; in IPv6 the all-zeros address of a subnet (the Subnet-Router anycast
//! address, RFC 4291) is not counted as a host, except in /127 (RFC 6164) and
//! /128 — the same conventions as Python's `ipaddress` module.

use serde::{Deserialize, Serialize};

use crate::ToolError;
use crate::ip::{
    Addr, Family, Interface, Net, expand_ipv6, format_ipv4, low_mask, low32, merge_ranges, netmask_v4,
    pow2_string, range_to_nets,
};

/// [`split`] never returns more than this many subnets, whatever `limit` says.
pub const MAX_SPLIT_SUBNETS: usize = 65_536;

/// Everything worth knowing about an address and the network it is in.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SubnetInfo {
    /// IP version, 4 or 6.
    pub version: u8,
    /// The host address that was given.
    pub address: String,
    /// The network address.
    pub network: String,
    /// Prefix length.
    pub prefix: u8,
    /// The network in CIDR notation, e.g. `192.168.1.0/24`.
    pub cidr: String,
    /// Dotted netmask (IPv4 only, empty for IPv6).
    pub netmask: String,
    /// Wildcard (inverse) mask (IPv4 only).
    pub wildcard: String,
    /// Broadcast address (IPv4 only; empty for /31 and /32, which have none).
    pub broadcast: String,
    /// First usable host address.
    pub first_host: String,
    /// Last usable host address.
    pub last_host: String,
    /// Number of usable host addresses, in decimal.
    pub usable_hosts: String,
    /// Number of addresses in the network, in decimal.
    pub total_addresses: String,
    /// Classful class letter `A`-`E` (IPv4 only).
    pub class: String,
    /// What kind of address this is: `private (RFC 1918)`, `public`,
    /// `loopback`, `link-local`, `global unicast`, `unique local`, ...
    pub scope: String,
    /// The address as dotted binary octets (IPv4 only).
    pub binary_address: String,
    /// The netmask as dotted binary octets (IPv4 only).
    pub binary_netmask: String,
    /// The address in hexadecimal, e.g. `0xC0A8010A`.
    pub hex: String,
    /// The address as a decimal integer.
    pub integer: String,
    /// The PTR record name of the address (`in-addr.arpa` / `ip6.arpa`).
    pub reverse_dns: String,
    /// The address with every IPv6 group written out; for IPv4 the address itself.
    pub expanded: String,
    /// The address in RFC 5952 form; for IPv4 the address itself.
    pub compressed: String,
    /// The address is the network address of a network that has one.
    pub is_network_address: bool,
    /// The address is the broadcast address of an IPv4 network that has one.
    pub is_broadcast_address: bool,
}

/// The outcome of [`split`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SplitResult {
    /// How many subnets the split produces in total, in decimal.
    pub total: String,
    /// The first subnets, in address order.
    pub subnets: Vec<SubnetInfo>,
    /// Whether `subnets` holds fewer than `total`.
    pub truncated: bool,
}

/// Describe an address and its network.
///
/// Accepts `192.168.1.10/24`, `192.168.1.10 255.255.255.0`,
/// `192.168.1.10/255.255.255.0`, `192.168.1.10 0.0.0.255` (wildcard), a bare
/// address (a /32 or /128) and IPv6 such as `2001:db8::1/64`.
pub fn info(input: &str) -> Result<SubnetInfo, ToolError> {
    let Interface { addr, net } = Interface::parse(input)?;
    Ok(describe(addr, net))
}

/// Split a network into subnets of length `new_prefix`.
///
/// At most `limit` subnets (and never more than [`MAX_SPLIT_SUBNETS`]) are
/// returned; `total` is the true count however large it is.
pub fn split(network: &str, new_prefix: u8, limit: usize) -> Result<SplitResult, ToolError> {
    let parent = Interface::parse(network)?.net;
    check_prefix(parent.family, new_prefix)?;
    if new_prefix < parent.prefix {
        return Err(ToolError::invalid(
            "new prefix length",
            &format!("/{new_prefix}"),
            format!("it is shorter than the /{} of {parent}; subnets cannot be larger than the network", parent.prefix),
        ));
    }
    let doublings = new_prefix - parent.prefix;
    let total = 1u128.checked_shl(u32::from(doublings));
    let wanted = limit.min(MAX_SPLIT_SUBNETS);
    let count = total.map_or(wanted, |total| usize::try_from(total).unwrap_or(usize::MAX).min(wanted));

    // The step is only undefined (2^128) when the single subnet is ::/0 itself.
    let step = 1u128.checked_shl(u32::from(parent.family.bits() - new_prefix)).unwrap_or(0);
    let subnets = (0..count as u128)
        .map(|index| {
            let subnet = Net { family: parent.family, first: parent.first + index * step, prefix: new_prefix };
            describe(subnet.network(), subnet)
        })
        .collect();
    Ok(SplitResult {
        total: pow2_string(doublings),
        subnets,
        truncated: total.is_none_or(|total| (count as u128) < total),
    })
}

/// The smallest set of CIDR blocks that covers exactly the given networks
/// and addresses: nested and duplicate entries disappear, adjacent ones merge.
///
/// IPv4 and IPv6 entries may be mixed; IPv4 results come first. Blank entries
/// are skipped.
pub fn summarize(networks: &[String]) -> Result<Vec<String>, ToolError> {
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for net in parse_all(networks)? {
        let ranges = if net.family == Family::V4 { &mut v4 } else { &mut v6 };
        ranges.push((net.first, net.last()));
    }
    let mut nets = Vec::new();
    for (family, ranges) in [(Family::V4, v4), (Family::V6, v6)] {
        for (first, last) in merge_ranges(ranges) {
            range_to_nets(family, first, last, &mut nets);
        }
    }
    Ok(nets.iter().map(Net::to_string).collect())
}

/// The single smallest network that contains all of the given ones.
pub fn supernet(networks: &[String]) -> Result<String, ToolError> {
    let nets = parse_all(networks)?;
    let Some(&first) = nets.first() else {
        return Err(ToolError::invalid("network list", "", "at least one network is needed"));
    };
    if let Some(other) = nets.iter().find(|net| net.family != first.family) {
        return Err(ToolError::invalid(
            "network list",
            &other.to_string(),
            format!("it cannot share a supernet with {first}: IPv4 and IPv6 are separate address spaces"),
        ));
    }
    let lowest = nets.iter().map(|net| net.first).min().unwrap_or(first.first);
    let highest = nets.iter().map(|net| net.last()).max().unwrap_or(first.last());
    // The supernet's prefix is the run of leading bits on which the lowest
    // and highest address agree.
    let bits = first.family.bits();
    let differing = lowest ^ highest;
    let common = (differing.leading_zeros() as u8).saturating_sub(128 - bits);
    Ok(Net::containing(Addr { family: first.family, value: lowest }, common).to_string())
}

/// Whether `address` lies inside `network`.
///
/// `address` may itself be a network (`10.1.0.0/16`), in which case all of it
/// must be inside. An IPv4 address is never inside an IPv6 network or vice versa.
pub fn contains(network: &str, address: &str) -> Result<bool, ToolError> {
    let outer = Interface::parse(network)?.net;
    let inner = Interface::parse(address)?.net;
    Ok(outer.contains(inner))
}

/// Whether two networks share any address.
pub fn overlaps(a: &str, b: &str) -> Result<bool, ToolError> {
    let a = Interface::parse(a)?.net;
    let b = Interface::parse(b)?.net;
    Ok(a.overlaps(b))
}

/// The smallest list of CIDR blocks covering exactly the addresses from
/// `first` to `last` inclusive.
pub fn range_to_cidrs(first: &str, last: &str) -> Result<Vec<String>, ToolError> {
    let start = parse_plain_address(first)?;
    let end = parse_plain_address(last)?;
    if start.family != end.family {
        return Err(ToolError::invalid(
            "address range",
            &format!("{start} - {end}"),
            "both ends must be of the same family (IPv4 or IPv6)",
        ));
    }
    if start.value > end.value {
        return Err(ToolError::invalid(
            "address range",
            &format!("{start} - {end}"),
            "the first address is higher than the last one",
        ));
    }
    let mut nets = Vec::new();
    range_to_nets(start.family, start.value, end.value, &mut nets);
    Ok(nets.iter().map(Net::to_string).collect())
}

/// The lowest subnet of length `prefix` inside `parent` that overlaps none of
/// the `used` networks, or `None` when the parent has no such room left.
pub fn next_free(parent: &str, prefix: u8, used: &[String]) -> Result<Option<String>, ToolError> {
    let parent = Interface::parse(parent)?.net;
    check_prefix(parent.family, prefix)?;
    if prefix < parent.prefix {
        return Err(ToolError::DoesNotFit(format!("a /{prefix} is larger than the parent network {parent}")));
    }
    let block = low_mask(parent.family.bits() - prefix);
    let mut candidate = parent.first;
    for (first, last) in used_ranges(parent, used)? {
        if last < candidate {
            continue;
        }
        if candidate | block < first {
            break;
        }
        // Move to the first block boundary after this used range.
        let aligned = last.checked_add(1).and_then(|next| next.checked_add(block)).map(|next| next & !block);
        match aligned {
            Some(next) if next <= parent.last() => candidate = next,
            _ => return Ok(None),
        }
    }
    Ok(Some(Net { family: parent.family, first: candidate, prefix }.to_string()))
}

/// The address space of `parent` that none of the `used` networks occupy,
/// as the smallest list of CIDR blocks.
pub fn free_space(parent: &str, used: &[String]) -> Result<Vec<String>, ToolError> {
    let parent = Interface::parse(parent)?.net;
    let mut free = Vec::new();
    let mut cursor = Some(parent.first);
    for (first, last) in used_ranges(parent, used)? {
        if let Some(start) = cursor
            && start < first
        {
            range_to_nets(parent.family, start, first - 1, &mut free);
        }
        cursor = last.checked_add(1);
    }
    if let Some(start) = cursor
        && start <= parent.last()
    {
        range_to_nets(parent.family, start, parent.last(), &mut free);
    }
    Ok(free.iter().map(Net::to_string).collect())
}

// ----------------------------------------------------------------- helpers

fn check_prefix(family: Family, prefix: u8) -> Result<(), ToolError> {
    if prefix > family.bits() {
        return Err(ToolError::invalid(
            "prefix length",
            &format!("/{prefix}"),
            format!("{} prefix lengths range from 0 to {}", family.label(), family.bits()),
        ));
    }
    Ok(())
}

/// Parse every non-blank entry as a network.
fn parse_all(networks: &[String]) -> Result<Vec<Net>, ToolError> {
    networks
        .iter()
        .filter(|entry| !entry.trim().is_empty())
        .map(|entry| Interface::parse(entry).map(|interface| interface.net))
        .collect()
}

fn parse_plain_address(text: &str) -> Result<Addr, ToolError> {
    let text = text.trim();
    if text.contains('/') || text.split_whitespace().count() > 1 {
        return Err(ToolError::invalid("address", text, "a range runs between two plain addresses, without a mask"));
    }
    Addr::parse(text)
}

/// The parts of `parent` covered by `used`, as sorted disjoint ranges.
/// Entries of the other address family cannot overlap and are ignored.
fn used_ranges(parent: Net, used: &[String]) -> Result<Vec<(u128, u128)>, ToolError> {
    let ranges = parse_all(used)?
        .into_iter()
        .filter(|net| net.overlaps(parent))
        .map(|net| (net.first.max(parent.first), net.last().min(parent.last())))
        .collect();
    Ok(merge_ranges(ranges))
}

fn describe(addr: Addr, net: Net) -> SubnetInfo {
    let family = net.family;
    let host_bits = net.host_bits();
    let last = net.last();
    let show = |value: u128| Addr { family, value }.to_string();

    // /31 and /127 are point-to-point links without network or broadcast
    // address; /32 and /128 are single hosts.
    let has_network_address = host_bits >= 2;
    let has_broadcast = family == Family::V4 && host_bits >= 2;
    let (first_host, last_host, usable_hosts) = match host_bits {
        0 => (addr.value, addr.value, "1".to_owned()),
        1 => (net.first, last, "2".to_owned()),
        _ if family == Family::V4 => (net.first + 1, last - 1, (low_mask(host_bits) - 1).to_string()),
        _ => (net.first + 1, last, low_mask(host_bits).to_string()),
    };

    let mut info = SubnetInfo {
        version: family.version(),
        address: addr.to_string(),
        network: net.network().to_string(),
        prefix: net.prefix,
        cidr: net.to_string(),
        netmask: String::new(),
        wildcard: String::new(),
        broadcast: if has_broadcast { show(last) } else { String::new() },
        first_host: show(first_host),
        last_host: show(last_host),
        usable_hosts,
        total_addresses: pow2_string(host_bits),
        class: String::new(),
        scope: scope(addr).to_owned(),
        binary_address: String::new(),
        binary_netmask: String::new(),
        hex: String::new(),
        integer: addr.value.to_string(),
        reverse_dns: String::new(),
        expanded: addr.to_string(),
        compressed: addr.to_string(),
        is_network_address: has_network_address && addr.value == net.first,
        is_broadcast_address: has_broadcast && addr.value == last,
    };
    match family {
        Family::V4 => {
            let value = low32(addr.value);
            let mask = netmask_v4(net.prefix);
            let [a, b, c, d] = value.to_be_bytes();
            info.netmask = format_ipv4(mask);
            info.wildcard = format_ipv4(!mask);
            info.class = class_of(a).to_owned();
            info.binary_address = dotted_binary(value);
            info.binary_netmask = dotted_binary(mask);
            info.hex = format!("0x{value:08X}");
            info.reverse_dns = format!("{d}.{c}.{b}.{a}.in-addr.arpa");
        }
        Family::V6 => {
            let nibbles: Vec<String> = (0..32).map(|index| format!("{:x}", (addr.value >> (4 * index)) & 0xf)).collect();
            info.hex = format!("0x{:032X}", addr.value);
            info.reverse_dns = format!("{}.ip6.arpa", nibbles.join("."));
            info.expanded = expand_ipv6(addr.value);
        }
    }
    info
}

fn dotted_binary(value: u32) -> String {
    let [a, b, c, d] = value.to_be_bytes();
    format!("{a:08b}.{b:08b}.{c:08b}.{d:08b}")
}

fn class_of(first_octet: u8) -> &'static str {
    match first_octet {
        0..=127 => "A",
        128..=191 => "B",
        192..=223 => "C",
        224..=239 => "D",
        240..=255 => "E",
    }
}

/// `(block, prefix length, scope)`, most specific block first. From the IANA
/// IPv4 Special-Purpose Address Registry; blocks that are globally reachable
/// (AS112, AMT, ...) count as public.
const V4_SCOPES: &[(u32, u8, &str)] = &[
    (0x0000_0000, 32, "unspecified"),
    (0xFFFF_FFFF, 32, "limited broadcast"),
    (0x0000_0000, 8, "this network (RFC 791)"),
    (0x0A00_0000, 8, "private (RFC 1918)"),
    (0xAC10_0000, 12, "private (RFC 1918)"),
    (0xC0A8_0000, 16, "private (RFC 1918)"),
    (0x6440_0000, 10, "CGNAT (RFC 6598)"),
    (0x7F00_0000, 8, "loopback"),
    (0xA9FE_0000, 16, "link-local"),
    (0xC000_0000, 24, "IETF protocol assignments (RFC 6890)"),
    (0xC000_0200, 24, "documentation"),
    (0xC633_6400, 24, "documentation"),
    (0xCB00_7100, 24, "documentation"),
    (0xC058_6300, 24, "6to4 relay anycast (deprecated)"),
    (0xC612_0000, 15, "benchmarking (RFC 2544)"),
    (0xE000_0000, 4, "multicast"),
    (0xF000_0000, 4, "reserved"),
];

/// As [`V4_SCOPES`], from the IANA IPv6 Special-Purpose Address Registry and
/// the IPv6 address space registry.
const V6_SCOPES: &[(u128, u8, &str)] = &[
    (0, 128, "unspecified"),
    (1, 128, "loopback"),
    (0xffff_0000_0000, 96, "IPv4-mapped"),
    (0x0064_ff9b_0000_0000_0000_0000_0000_0000, 96, "NAT64 (RFC 6052)"),
    (0x0064_ff9b_0001_0000_0000_0000_0000_0000, 48, "local-use NAT64 (RFC 8215)"),
    (0x0100_0000_0000_0000_0000_0000_0000_0000, 64, "discard-only (RFC 6666)"),
    (0x2001_0000_0000_0000_0000_0000_0000_0000, 32, "Teredo"),
    (0x2001_0002_0000_0000_0000_0000_0000_0000, 48, "benchmarking (RFC 5180)"),
    (0x2001_0010_0000_0000_0000_0000_0000_0000, 28, "ORCHID (deprecated)"),
    (0x2001_0020_0000_0000_0000_0000_0000_0000, 28, "ORCHIDv2 (RFC 7343)"),
    (0x2001_0db8_0000_0000_0000_0000_0000_0000, 32, "documentation"),
    (0x3fff_0000_0000_0000_0000_0000_0000_0000, 20, "documentation"),
    (0x2001_0000_0000_0000_0000_0000_0000_0000, 23, "IETF protocol assignments (RFC 2928)"),
    (0x2002_0000_0000_0000_0000_0000_0000_0000, 16, "6to4"),
    (0x5f00_0000_0000_0000_0000_0000_0000_0000, 16, "SRv6 SIDs (RFC 9602)"),
    (0xfc00_0000_0000_0000_0000_0000_0000_0000, 7, "unique local"),
    (0xfe80_0000_0000_0000_0000_0000_0000_0000, 10, "link-local"),
    (0xfec0_0000_0000_0000_0000_0000_0000_0000, 10, "site-local (deprecated)"),
    (0xff00_0000_0000_0000_0000_0000_0000_0000, 8, "multicast"),
    (0x2000_0000_0000_0000_0000_0000_0000_0000, 3, "global unicast"),
];

fn scope(addr: Addr) -> &'static str {
    match addr.family {
        Family::V4 => {
            let value = low32(addr.value);
            V4_SCOPES
                .iter()
                .find(|(block, prefix, _)| value & netmask_v4(*prefix) == *block)
                .map_or("public", |(_, _, name)| name)
        }
        Family::V6 => V6_SCOPES
            .iter()
            .find(|(block, prefix, _)| addr.value & !low_mask(128 - prefix) == *block)
            .map_or("reserved", |(_, _, name)| name),
    }
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn info_for_a_typical_host() {
        let expected = SubnetInfo {
            version: 4,
            address: "192.168.1.10".into(),
            network: "192.168.1.0".into(),
            prefix: 24,
            cidr: "192.168.1.0/24".into(),
            netmask: "255.255.255.0".into(),
            wildcard: "0.0.0.255".into(),
            broadcast: "192.168.1.255".into(),
            first_host: "192.168.1.1".into(),
            last_host: "192.168.1.254".into(),
            usable_hosts: "254".into(),
            total_addresses: "256".into(),
            class: "C".into(),
            scope: "private (RFC 1918)".into(),
            binary_address: "11000000.10101000.00000001.00001010".into(),
            binary_netmask: "11111111.11111111.11111111.00000000".into(),
            hex: "0xC0A8010A".into(),
            integer: "3232235786".into(),
            reverse_dns: "10.1.168.192.in-addr.arpa".into(),
            expanded: "192.168.1.10".into(),
            compressed: "192.168.1.10".into(),
            is_network_address: false,
            is_broadcast_address: false,
        };
        for input in
            ["192.168.1.10/24", "192.168.1.10 255.255.255.0", "192.168.1.10/255.255.255.0", "192.168.1.10 0.0.0.255"]
        {
            assert_eq!(info(input).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn info_for_an_odd_prefix() {
        // 172.16.37.201/19: third octet 37 = 0b001_00101, so the network is 172.16.32.0.
        let info = info("172.16.37.201/19").unwrap();
        assert_eq!(info.network, "172.16.32.0");
        assert_eq!(info.netmask, "255.255.224.0");
        assert_eq!(info.wildcard, "0.0.31.255");
        assert_eq!(info.broadcast, "172.16.63.255");
        assert_eq!(info.first_host, "172.16.32.1");
        assert_eq!(info.last_host, "172.16.63.254");
        assert_eq!(info.usable_hosts, "8190");
        assert_eq!(info.total_addresses, "8192");
        assert_eq!(info.class, "B");
    }

    #[test]
    fn info_edge_prefixes_v4() {
        let all = info("0.0.0.0/0").unwrap();
        assert_eq!(
            (all.network.as_str(), all.broadcast.as_str(), all.first_host.as_str(), all.last_host.as_str()),
            ("0.0.0.0", "255.255.255.255", "0.0.0.1", "255.255.255.254")
        );
        assert_eq!((all.usable_hosts.as_str(), all.total_addresses.as_str()), ("4294967294", "4294967296"));
        assert_eq!(all.netmask, "0.0.0.0");
        assert_eq!(all.wildcard, "255.255.255.255");
        assert!(all.is_network_address);

        // RFC 3021 point-to-point: both addresses are hosts, there is no broadcast.
        for (input, address) in [("10.0.0.0/31", "10.0.0.0"), ("10.0.0.1/31", "10.0.0.1")] {
            let p2p = info(input).unwrap();
            assert_eq!(p2p.address, address);
            assert_eq!(p2p.cidr, "10.0.0.0/31");
            assert_eq!(p2p.broadcast, "");
            assert_eq!((p2p.first_host.as_str(), p2p.last_host.as_str()), ("10.0.0.0", "10.0.0.1"));
            assert_eq!((p2p.usable_hosts.as_str(), p2p.total_addresses.as_str()), ("2", "2"));
            assert!(!p2p.is_network_address && !p2p.is_broadcast_address);
        }

        let host = info("10.1.2.3").unwrap();
        assert_eq!(host.cidr, "10.1.2.3/32");
        assert_eq!(host.netmask, "255.255.255.255");
        assert_eq!(host.broadcast, "");
        assert_eq!((host.first_host.as_str(), host.last_host.as_str()), ("10.1.2.3", "10.1.2.3"));
        assert_eq!((host.usable_hosts.as_str(), host.total_addresses.as_str()), ("1", "1"));
        assert!(!host.is_network_address && !host.is_broadcast_address);

        let smallest = info("10.0.0.4/30").unwrap();
        assert_eq!((smallest.first_host.as_str(), smallest.last_host.as_str()), ("10.0.0.5", "10.0.0.6"));
        assert_eq!(smallest.usable_hosts, "2");
    }

    #[test]
    fn network_and_broadcast_addresses_are_flagged() {
        let network = info("192.168.1.0/24").unwrap();
        assert!(network.is_network_address && !network.is_broadcast_address);
        let broadcast = info("192.168.1.255/24").unwrap();
        assert!(!broadcast.is_network_address && broadcast.is_broadcast_address);
        assert_eq!(broadcast.broadcast, "192.168.1.255");
        assert_eq!(broadcast.address, "192.168.1.255");
        // The same address is an ordinary host in a larger network.
        let host = info("192.168.1.255/22").unwrap();
        assert!(!host.is_network_address && !host.is_broadcast_address);
        // IPv6 has no broadcast; the all-zeros address is the subnet-router anycast address.
        let v6 = info("2001:db8::/64").unwrap();
        assert!(v6.is_network_address && !v6.is_broadcast_address);
        assert!(!info("2001:db8::ffff:ffff:ffff:ffff/64").unwrap().is_broadcast_address);
    }

    #[test]
    fn info_for_ipv6() {
        let expected = SubnetInfo {
            version: 6,
            address: "2001:db8::1".into(),
            network: "2001:db8::".into(),
            prefix: 64,
            cidr: "2001:db8::/64".into(),
            netmask: String::new(),
            wildcard: String::new(),
            broadcast: String::new(),
            first_host: "2001:db8::1".into(),
            last_host: "2001:db8::ffff:ffff:ffff:ffff".into(),
            usable_hosts: "18446744073709551615".into(),
            total_addresses: "18446744073709551616".into(),
            class: String::new(),
            scope: "documentation".into(),
            binary_address: String::new(),
            binary_netmask: String::new(),
            hex: "0x20010DB8000000000000000000000001".into(),
            integer: "42540766411282592856903984951653826561".into(),
            reverse_dns: "1.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.0.8.b.d.0.1.0.0.2.ip6.arpa".into(),
            expanded: "2001:0db8:0000:0000:0000:0000:0000:0001".into(),
            compressed: "2001:db8::1".into(),
            is_network_address: false,
            is_broadcast_address: false,
        };
        assert_eq!(info("2001:db8::1/64").unwrap(), expected);
        assert_eq!(info("2001:0DB8:0:0:0:0:0:1/64").unwrap(), expected);
    }

    #[test]
    fn info_edge_prefixes_v6() {
        let all = info("::/0").unwrap();
        assert_eq!(all.total_addresses, "340282366920938463463374607431768211456");
        assert_eq!(all.usable_hosts, "340282366920938463463374607431768211455");
        assert_eq!((all.first_host.as_str(), all.last_host.as_str()), ("::1", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"));

        // RFC 6164 point-to-point.
        let p2p = info("2001:db8::1/127").unwrap();
        assert_eq!(p2p.cidr, "2001:db8::/127");
        assert_eq!((p2p.first_host.as_str(), p2p.last_host.as_str()), ("2001:db8::", "2001:db8::1"));
        assert_eq!((p2p.usable_hosts.as_str(), p2p.total_addresses.as_str()), ("2", "2"));
        assert!(!info("2001:db8::/127").unwrap().is_network_address);

        let host = info("2001:db8::5").unwrap();
        assert_eq!(host.cidr, "2001:db8::5/128");
        assert_eq!((host.first_host.as_str(), host.last_host.as_str()), ("2001:db8::5", "2001:db8::5"));
        assert_eq!((host.usable_hosts.as_str(), host.total_addresses.as_str()), ("1", "1"));

        let quad = info("2001:db8::4/126").unwrap();
        assert_eq!((quad.first_host.as_str(), quad.last_host.as_str()), ("2001:db8::5", "2001:db8::7"));
        assert_eq!((quad.usable_hosts.as_str(), quad.total_addresses.as_str()), ("3", "4"));
    }

    #[test]
    fn ipv4_mapped_ipv6_keeps_its_dotted_tail() {
        let mapped = info("::ffff:192.168.1.1/128").unwrap();
        assert_eq!(mapped.version, 6);
        assert_eq!(mapped.address, "::ffff:192.168.1.1");
        assert_eq!(mapped.compressed, "::ffff:192.168.1.1");
        assert_eq!(mapped.expanded, "0000:0000:0000:0000:0000:ffff:c0a8:0101");
        assert_eq!(mapped.scope, "IPv4-mapped");
        assert_eq!(mapped.integer, "281473913979137");
        // The hexadecimal spelling is the same address.
        assert_eq!(info("::ffff:c0a8:101").unwrap(), mapped);
    }

    #[test]
    fn zone_ids_and_junk_are_rejected() {
        assert!(info("fe80::1%eth0").unwrap_err().to_string().contains("zone identifier"));
        assert!(info("fe80::1%eth0/64").unwrap_err().to_string().contains("zone identifier"));
        assert_eq!(
            info("10.0.300.1").unwrap_err().to_string(),
            "invalid IPv4 address `10.0.300.1`: `300` is not a valid octet (0-255)"
        );
        assert!(info("").is_err());
        assert!(info("router1").is_err());
        assert!(info("10.0.0.1/24 extra").is_err());
    }

    #[test]
    fn classes() {
        for (address, class) in [
            ("0.0.0.0", "A"),
            ("10.0.0.1", "A"),
            ("127.255.255.255", "A"),
            ("128.0.0.0", "B"),
            ("191.255.255.255", "B"),
            ("192.0.0.0", "C"),
            ("223.255.255.255", "C"),
            ("224.0.0.0", "D"),
            ("239.255.255.255", "D"),
            ("240.0.0.0", "E"),
            ("255.255.255.255", "E"),
        ] {
            assert_eq!(info(address).unwrap().class, class, "{address}");
        }
    }

    #[test]
    fn scopes_v4() {
        for (address, scope) in [
            ("0.0.0.0", "unspecified"),
            ("0.1.2.3", "this network (RFC 791)"),
            ("9.255.255.255", "public"),
            ("10.0.0.0", "private (RFC 1918)"),
            ("10.255.255.255", "private (RFC 1918)"),
            ("11.0.0.0", "public"),
            ("100.63.255.255", "public"),
            ("100.64.0.0", "CGNAT (RFC 6598)"),
            ("100.127.255.255", "CGNAT (RFC 6598)"),
            ("100.128.0.0", "public"),
            ("127.0.0.1", "loopback"),
            ("169.254.10.20", "link-local"),
            ("172.15.255.255", "public"),
            ("172.16.0.0", "private (RFC 1918)"),
            ("172.31.255.255", "private (RFC 1918)"),
            ("172.32.0.0", "public"),
            ("192.0.0.8", "IETF protocol assignments (RFC 6890)"),
            ("192.0.2.55", "documentation"),
            ("192.88.99.1", "6to4 relay anycast (deprecated)"),
            ("192.167.255.255", "public"),
            ("192.168.0.0", "private (RFC 1918)"),
            ("192.168.255.255", "private (RFC 1918)"),
            ("192.169.0.0", "public"),
            ("198.17.255.255", "public"),
            ("198.18.0.0", "benchmarking (RFC 2544)"),
            ("198.19.255.255", "benchmarking (RFC 2544)"),
            ("198.20.0.0", "public"),
            ("198.51.100.7", "documentation"),
            ("203.0.113.200", "documentation"),
            ("8.8.8.8", "public"),
            ("223.255.255.255", "public"),
            ("224.0.0.5", "multicast"),
            ("239.255.255.250", "multicast"),
            ("240.0.0.1", "reserved"),
            ("255.255.255.254", "reserved"),
            ("255.255.255.255", "limited broadcast"),
        ] {
            assert_eq!(info(address).unwrap().scope, scope, "{address}");
        }
    }

    #[test]
    fn scopes_v6() {
        for (address, scope) in [
            ("::", "unspecified"),
            ("::1", "loopback"),
            ("::2", "reserved"),
            ("::ffff:10.0.0.1", "IPv4-mapped"),
            ("64:ff9b::192.0.2.1", "NAT64 (RFC 6052)"),
            ("64:ff9b:1::1", "local-use NAT64 (RFC 8215)"),
            ("100::1", "discard-only (RFC 6666)"),
            ("2001::1", "Teredo"),
            ("2001:2::1", "benchmarking (RFC 5180)"),
            ("2001:10::1", "ORCHID (deprecated)"),
            ("2001:20::1", "ORCHIDv2 (RFC 7343)"),
            ("2001:1::1", "IETF protocol assignments (RFC 2928)"),
            ("2001:db8:ffff::1", "documentation"),
            ("3fff:fff::1", "documentation"),
            ("2002:c000:204::1", "6to4"),
            ("2606:4700:4700::1111", "global unicast"),
            ("3fff:1000::1", "global unicast"),
            ("5f00::1", "SRv6 SIDs (RFC 9602)"),
            ("4000::1", "reserved"),
            ("fc00::1", "unique local"),
            ("fd12:3456:789a::1", "unique local"),
            ("fe80::1", "link-local"),
            ("febf::1", "link-local"),
            ("fec0::1", "site-local (deprecated)"),
            ("ff02::1", "multicast"),
            ("ff05::1:3", "multicast"),
        ] {
            assert_eq!(info(address).unwrap().scope, scope, "{address}");
        }
    }

    #[test]
    fn split_into_smaller_subnets() {
        let result = split("192.168.1.0/24", 26, 10).unwrap();
        assert_eq!(result.total, "4");
        assert!(!result.truncated);
        let cidrs: Vec<&str> = result.subnets.iter().map(|subnet| subnet.cidr.as_str()).collect();
        assert_eq!(cidrs, ["192.168.1.0/26", "192.168.1.64/26", "192.168.1.128/26", "192.168.1.192/26"]);
        assert_eq!(result.subnets[1].broadcast, "192.168.1.127");
        assert_eq!(result.subnets[1].first_host, "192.168.1.65");
        assert!(result.subnets.iter().all(|subnet| subnet.is_network_address));

        // Host bits in the input are ignored, as everywhere.
        assert_eq!(split("192.168.1.77/24", 26, 10).unwrap(), result);
        // Splitting into itself is the identity.
        let same = split("10.0.0.0/8", 8, 5).unwrap();
        assert_eq!((same.total.as_str(), same.subnets.len(), same.truncated), ("1", 1, false));
    }

    #[test]
    fn split_honours_its_limits() {
        let result = split("10.0.0.0/8", 24, 3).unwrap();
        assert_eq!(result.total, "65536");
        assert!(result.truncated);
        let cidrs: Vec<&str> = result.subnets.iter().map(|subnet| subnet.cidr.as_str()).collect();
        assert_eq!(cidrs, ["10.0.0.0/24", "10.0.1.0/24", "10.0.2.0/24"]);

        let none = split("10.0.0.0/8", 24, 0).unwrap();
        assert_eq!((none.total.as_str(), none.subnets.len(), none.truncated), ("65536", 0, true));

        // Astronomically many subnets: the count is exact, the list capped.
        let huge = split("::/0", 128, usize::MAX).unwrap();
        assert_eq!(huge.total, "340282366920938463463374607431768211456");
        assert_eq!(huge.subnets.len(), MAX_SPLIT_SUBNETS);
        assert!(huge.truncated);
        assert_eq!(huge.subnets[MAX_SPLIT_SUBNETS - 1].cidr, "::ffff/128");

        let whole = split("::/0", 0, 10).unwrap();
        assert_eq!((whole.total.as_str(), whole.subnets.len(), whole.truncated), ("1", 1, false));
        assert_eq!(whole.subnets[0].cidr, "::/0");

        let v6 = split("2001:db8::/32", 48, 2).unwrap();
        assert_eq!(v6.total, "65536");
        assert_eq!(v6.subnets[1].cidr, "2001:db8:1::/48");

        let exact = split("10.0.0.0/30", 32, 4).unwrap();
        assert_eq!((exact.subnets.len(), exact.truncated), (4, false));
        assert_eq!(exact.subnets[3].cidr, "10.0.0.3/32");
    }

    #[test]
    fn split_rejects_impossible_prefixes() {
        assert_eq!(
            split("10.0.0.0/24", 20, 10).unwrap_err().to_string(),
            "invalid new prefix length `/20`: it is shorter than the /24 of 10.0.0.0/24; subnets cannot be larger than the network"
        );
        assert_eq!(
            split("10.0.0.0/24", 33, 10).unwrap_err().to_string(),
            "invalid prefix length `/33`: IPv4 prefix lengths range from 0 to 32"
        );
        assert!(split("2001:db8::/32", 129, 10).is_err());
        assert!(split("nonsense", 24, 10).is_err());
    }

    #[test]
    fn summarize_merges_adjacent_nested_unsorted_and_duplicate_networks() {
        // Adjacent.
        assert_eq!(summarize(&strings(&["10.0.0.0/24", "10.0.1.0/24"])).unwrap(), ["10.0.0.0/23"]);
        // Adjacent but not aligned: 10.0.1.0/24 + 10.0.2.0/24 is not a /23.
        assert_eq!(summarize(&strings(&["10.0.1.0/24", "10.0.2.0/24"])).unwrap(), ["10.0.1.0/24", "10.0.2.0/24"]);
        // Nested and duplicate.
        assert_eq!(
            summarize(&strings(&["10.0.0.0/8", "10.1.2.0/24", "10.0.0.0/8", "10.255.255.255"])).unwrap(),
            ["10.0.0.0/8"]
        );
        // Unsorted, with a gap.
        assert_eq!(
            summarize(&strings(&["192.168.3.0/24", "192.168.0.0/24", "192.168.2.0/24", "192.168.1.0/24", "192.168.5.0/24"]))
                .unwrap(),
            ["192.168.0.0/22", "192.168.5.0/24"]
        );
        // Hosts merge into blocks.
        assert_eq!(
            summarize(&strings(&["10.0.0.1", "10.0.0.2", "10.0.0.3", "10.0.0.4"])).unwrap(),
            ["10.0.0.1/32", "10.0.0.2/31", "10.0.0.4/32"]
        );
        // The two halves of everything.
        assert_eq!(summarize(&strings(&["128.0.0.0/1", "0.0.0.0/1"])).unwrap(), ["0.0.0.0/0"]);
        // Host bits are ignored; blanks skipped; families kept apart, IPv4 first.
        assert_eq!(
            summarize(&strings(&["2001:db8:1::/48", "", "10.0.0.77/25", "2001:db8::/48", "  ", "10.0.0.200/25"])).unwrap(),
            ["10.0.0.0/24", "2001:db8::/47"]
        );
        assert_eq!(summarize(&[]).unwrap(), Vec::<String>::new());
        assert!(summarize(&strings(&["10.0.0.0/24", "bogus"])).is_err());
    }

    #[test]
    fn supernet_of_several_networks() {
        assert_eq!(supernet(&strings(&["10.0.0.0/24", "10.0.1.0/24"])).unwrap(), "10.0.0.0/23");
        assert_eq!(supernet(&strings(&["10.0.1.0/24", "10.0.2.0/24"])).unwrap(), "10.0.0.0/22");
        assert_eq!(supernet(&strings(&["192.168.10.0/24"])).unwrap(), "192.168.10.0/24");
        assert_eq!(supernet(&strings(&["192.168.10.7"])).unwrap(), "192.168.10.7/32");
        assert_eq!(supernet(&strings(&["10.0.0.1", "10.0.0.2"])).unwrap(), "10.0.0.0/30");
        assert_eq!(supernet(&strings(&["0.0.0.0", "255.255.255.255"])).unwrap(), "0.0.0.0/0");
        assert_eq!(supernet(&strings(&["172.16.0.0/16", "172.31.0.0/16", "172.20.5.0/24"])).unwrap(), "172.16.0.0/12");
        assert_eq!(supernet(&strings(&["2001:db8:a::/48", "2001:db8:b::/48"])).unwrap(), "2001:db8:a::/47");
        assert_eq!(supernet(&strings(&["::", "ffff::"])).unwrap(), "::/0");
        assert_eq!(supernet(&[]).unwrap_err().to_string(), "invalid network list: at least one network is needed");
        assert!(supernet(&strings(&["10.0.0.0/8", "2001:db8::/32"])).unwrap_err().to_string().contains("separate address spaces"));
    }

    #[test]
    fn contains_and_overlaps() {
        assert!(contains("10.0.0.0/8", "10.200.3.4").unwrap());
        assert!(!contains("10.0.0.0/8", "11.0.0.1").unwrap());
        assert!(contains("10.0.0.0/8", "10.0.0.0").unwrap());
        assert!(contains("10.0.0.0/8", "10.255.255.255").unwrap());
        assert!(contains("10.0.0.0/8", "10.1.0.0/16").unwrap());
        assert!(contains("10.0.0.0/8", "10.0.0.0/8").unwrap());
        assert!(!contains("10.1.0.0/16", "10.0.0.0/8").unwrap());
        assert!(contains("0.0.0.0/0", "203.0.113.9").unwrap());
        assert!(contains("2001:db8::/32", "2001:db8:ffff::1").unwrap());
        assert!(!contains("2001:db8::/32", "2001:db9::1").unwrap());
        assert!(!contains("::/0", "10.0.0.1").unwrap());
        assert!(!contains("0.0.0.0/0", "::1").unwrap());
        assert!(contains("10.0.0.0/8", "nope").is_err());

        assert!(overlaps("10.0.0.0/8", "10.1.0.0/16").unwrap());
        assert!(overlaps("10.1.0.0/16", "10.0.0.0/8").unwrap());
        assert!(overlaps("10.0.0.0/24", "10.0.0.255").unwrap());
        assert!(!overlaps("10.0.0.0/24", "10.0.1.0/24").unwrap());
        assert!(!overlaps("10.0.0.0/24", "2001:db8::/32").unwrap());
        assert!(overlaps("::/0", "2001:db8::/32").unwrap());
        assert!(overlaps("x", "10.0.0.0/8").is_err());
    }

    #[test]
    fn ranges_become_minimal_cidr_lists() {
        assert_eq!(range_to_cidrs("10.0.0.5", "10.0.1.20").unwrap(), [
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
        assert_eq!(range_to_cidrs("192.168.0.0", "192.168.255.255").unwrap(), ["192.168.0.0/16"]);
        assert_eq!(range_to_cidrs("10.0.0.1", "10.0.0.1").unwrap(), ["10.0.0.1/32"]);
        assert_eq!(range_to_cidrs("0.0.0.0", "255.255.255.255").unwrap(), ["0.0.0.0/0"]);
        assert_eq!(range_to_cidrs("2001:db8::", "2001:db8::ffff").unwrap(), ["2001:db8::/112"]);
        assert_eq!(range_to_cidrs("::", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff").unwrap(), ["::/0"]);
        assert!(range_to_cidrs("10.0.0.9", "10.0.0.1").unwrap_err().to_string().contains("higher than the last"));
        assert!(range_to_cidrs("10.0.0.1", "2001:db8::1").unwrap_err().to_string().contains("same family"));
        assert!(range_to_cidrs("10.0.0.0/24", "10.0.0.9").unwrap_err().to_string().contains("without a mask"));
    }

    #[test]
    fn next_free_finds_the_lowest_gap() {
        let used = strings(&["10.0.0.0/26", "10.0.0.64/27", "10.0.0.128/25"]);
        // 10.0.0.96/27 is the only hole.
        assert_eq!(next_free("10.0.0.0/24", 27, &used).unwrap().as_deref(), Some("10.0.0.96/27"));
        assert_eq!(next_free("10.0.0.0/24", 28, &used).unwrap().as_deref(), Some("10.0.0.96/28"));
        assert_eq!(next_free("10.0.0.0/24", 26, &used).unwrap(), None);
        assert_eq!(next_free("10.0.0.0/24", 24, &[]).unwrap().as_deref(), Some("10.0.0.0/24"));
        assert_eq!(next_free("10.0.0.0/24", 30, &[]).unwrap().as_deref(), Some("10.0.0.0/30"));
        // A small used block near the start pushes a larger request to the next boundary.
        assert_eq!(next_free("10.0.0.0/24", 26, &strings(&["10.0.0.4/30"])).unwrap().as_deref(), Some("10.0.0.64/26"));
        // Used entries outside the parent, of the other family, or blank do not matter.
        assert_eq!(
            next_free("10.0.0.0/24", 25, &strings(&["192.168.0.0/16", "2001:db8::/32", ""])).unwrap().as_deref(),
            Some("10.0.0.0/25")
        );
        // A used supernet leaves nothing.
        assert_eq!(next_free("10.0.0.0/24", 32, &strings(&["10.0.0.0/8"])).unwrap(), None);
        // The very top of the address space.
        assert_eq!(
            next_free("255.255.255.0/24", 25, &strings(&["255.255.255.0/25"])).unwrap().as_deref(),
            Some("255.255.255.128/25")
        );
        assert_eq!(next_free("255.255.255.0/24", 25, &strings(&["255.255.255.0/25", "255.255.255.255"])).unwrap(), None);
        assert_eq!(
            next_free("2001:db8::/32", 48, &strings(&["2001:db8::/48", "2001:db8:1::/48"])).unwrap().as_deref(),
            Some("2001:db8:2::/48")
        );
        assert_eq!(next_free("::/0", 1, &strings(&["8000::/1"])).unwrap().as_deref(), Some("::/1"));
        assert_eq!(next_free("::/0", 1, &strings(&["::/1"])).unwrap().as_deref(), Some("8000::/1"));
        assert_eq!(next_free("::/0", 1, &strings(&["::1", "ffff::"])).unwrap(), None);

        assert_eq!(
            next_free("10.0.0.0/24", 16, &[]).unwrap_err(),
            ToolError::DoesNotFit("a /16 is larger than the parent network 10.0.0.0/24".into())
        );
        assert!(next_free("10.0.0.0/24", 33, &[]).is_err());
        assert!(next_free("10.0.0.0/24", 28, &strings(&["junk"])).is_err());
    }

    #[test]
    fn free_space_is_the_complement_of_the_used_networks() {
        let used = strings(&["10.0.0.0/26", "10.0.0.64/27", "10.0.0.128/25"]);
        assert_eq!(free_space("10.0.0.0/24", &used).unwrap(), ["10.0.0.96/27"]);
        assert_eq!(free_space("10.0.0.0/24", &[]).unwrap(), ["10.0.0.0/24"]);
        assert_eq!(free_space("10.0.0.0/24", &strings(&["10.0.0.0/24"])).unwrap(), Vec::<String>::new());
        assert_eq!(free_space("10.0.0.0/24", &strings(&["10.0.0.0/8"])).unwrap(), Vec::<String>::new());
        assert_eq!(free_space("10.0.0.0/24", &strings(&["10.0.0.4/30"])).unwrap(), [
            "10.0.0.0/30",
            "10.0.0.8/29",
            "10.0.0.16/28",
            "10.0.0.32/27",
            "10.0.0.64/26",
            "10.0.0.128/25"
        ]);
        assert_eq!(free_space("10.0.0.0/24", &strings(&["10.0.0.255", "10.0.0.0"])).unwrap(), [
            "10.0.0.1/32",
            "10.0.0.2/31",
            "10.0.0.4/30",
            "10.0.0.8/29",
            "10.0.0.16/28",
            "10.0.0.32/27",
            "10.0.0.64/26",
            "10.0.0.128/26",
            "10.0.0.192/27",
            "10.0.0.224/28",
            "10.0.0.240/29",
            "10.0.0.248/30",
            "10.0.0.252/31",
            "10.0.0.254/32"
        ]);
        assert_eq!(free_space("0.0.0.0/0", &strings(&["128.0.0.0/1"])).unwrap(), ["0.0.0.0/1"]);
        assert_eq!(free_space("::/0", &strings(&["8000::/1"])).unwrap(), ["::/1"]);
        assert_eq!(free_space("::/0", &strings(&["::/1"])).unwrap(), ["8000::/1"]);
        assert_eq!(free_space("2001:db8::/47", &strings(&["2001:db8:1::/48", "10.0.0.0/8"])).unwrap(), ["2001:db8::/48"]);
        assert!(free_space("10.0.0.0/24", &strings(&["junk"])).is_err());
    }
}
