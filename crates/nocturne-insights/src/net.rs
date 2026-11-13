//! Address arithmetic, and the addresses that only look like a conflict.
//!
//! First-hop redundancy is the reason the naive "two devices with one
//! address" check is useless on a real network: a pair of distribution
//! switches *is supposed* to answer for the same address, and in an ARP table
//! that address has a virtual MAC belonging to no chassis at all. Anycast
//! loopbacks are the same story. Everything that distinguishes a real
//! conflict from a designed one lives here.

use std::collections::HashSet;
use std::net::Ipv4Addr;

use nocturne_drivers::hier::Syntax;

use crate::world::Config;

/// An IPv4 address with its prefix length, as interfaces report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cidr {
    pub address: Ipv4Addr,
    pub prefix: u8,
}

impl Cidr {
    pub fn network(self) -> u32 {
        u32::from(self.address) & self.mask()
    }

    pub fn mask(self) -> u32 {
        if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - self.prefix)
        }
    }

    pub fn last(self) -> u32 {
        self.network() | !self.mask()
    }

    pub fn contains(self, address: Ipv4Addr) -> bool {
        u32::from(address) & self.mask() == self.network()
    }

    /// Do the two networks share any address?
    pub fn overlaps(self, other: Cidr) -> bool {
        self.network() <= other.last() && other.network() <= self.last()
    }

    pub fn network_cidr(self) -> String {
        format!("{}/{}", Ipv4Addr::from(self.network()), self.prefix)
    }
}

impl std::fmt::Display for Cidr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.address, self.prefix)
    }
}

/// `10.0.0.1/24`, `10.0.0.1 255.255.255.0`, or a bare address (taken as /32).
pub fn parse_cidr(text: &str) -> Option<Cidr> {
    let text = text.trim();
    let (address, suffix) = match text.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => match text.split_once(char::is_whitespace) {
            Some((address, mask)) => (address, Some(mask)),
            None => (text, None),
        },
    };
    let address: Ipv4Addr = address.trim().parse().ok()?;
    let prefix = match suffix {
        None => 32,
        Some(suffix) => {
            let suffix = suffix.trim();
            match suffix.parse::<u8>() {
                Ok(prefix) if prefix <= 32 => prefix,
                // A dotted mask: it has to be contiguous to mean anything.
                _ => {
                    let mask = u32::from(suffix.parse::<Ipv4Addr>().ok()?);
                    let ones = mask.leading_ones();
                    (mask.count_ones() == ones).then_some(ones as u8)?
                }
            }
        }
    };
    Some(Cidr { address, prefix })
}

/// An address that says something about the network it is on. Loopback,
/// link-local (including an unconfigured DHCP interface), multicast and the
/// unspecified address never do.
pub fn is_real_host(address: Ipv4Addr) -> bool {
    !(address.is_loopback()
        || address.is_link_local()
        || address.is_multicast()
        || address.is_unspecified()
        || address.is_broadcast())
}

/// 12 lowercase hexadecimal digits, from any vendor's notation.
pub fn normalise_mac(text: &str) -> Option<String> {
    let digits: String = text
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.' | ' '))
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (digits.len() == 12 && digits.chars().all(|c| c.is_ascii_hexdigit())).then_some(digits)
}

/// The protocol a virtual MAC belongs to, if it is one. An ARP entry holding
/// one of these is a redundancy group answering, not a host.
pub fn virtual_mac(mac: &str) -> Option<&'static str> {
    let mac = normalise_mac(mac)?;
    // HSRP v1 0000.0c07.acXX, HSRP v2 0000.0c9f.fXXX, VRRP 0000.5e00.01XX,
    // GLBP 0007.b4XX.XXXX.
    if mac.starts_with("00000c07ac") {
        return Some("HSRP");
    }
    if mac.starts_with("00000c9ff") {
        return Some("HSRP v2");
    }
    if mac.starts_with("00005e0001") {
        return Some("VRRP");
    }
    if mac.starts_with("0007b4") {
        return Some("GLBP");
    }
    None
}

/// Virtual addresses the device's configuration hands to a redundancy group.
///
/// IOS-style only (`standby`/`vrrp`/`glbp` under an interface). On a platform
/// whose grammar this does not read the set is empty, which is why
/// `duplicate_ip` treats a missing configuration as a reason to skip rather
/// than as proof that nothing is virtual.
pub fn virtual_addresses(config: &Config) -> HashSet<Ipv4Addr> {
    let mut addresses = HashSet::new();
    if config.syntax != Syntax::Ios {
        return addresses;
    }
    for section in &config.tree.roots {
        for child in &section.children {
            let words: Vec<&str> = child.text.split_whitespace().collect();
            let group = matches!(
                words.first().copied(),
                Some("standby") | Some("vrrp") | Some("glbp")
            );
            if !group {
                continue;
            }
            // `standby 1 ip 10.0.0.1`, `standby ip 10.0.0.1`,
            // `vrrp 10 ipv4 10.0.0.1`, `standby 1 ip 10.0.0.1 secondary`.
            if let Some(position) = words
                .iter()
                .position(|word| *word == "ip" || *word == "ipv4")
                && let Some(address) = words.get(position + 1)
                && let Ok(address) = address.parse::<Ipv4Addr>()
            {
                addresses.insert(address);
            }
        }
    }
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_drivers::hier::Syntax;

    #[test]
    fn addresses_are_read_in_every_notation() {
        assert_eq!(
            parse_cidr("10.0.0.1/24"),
            Some(Cidr {
                address: Ipv4Addr::new(10, 0, 0, 1),
                prefix: 24
            })
        );
        assert_eq!(
            parse_cidr(" 10.0.0.1 255.255.255.128 ").map(|cidr| cidr.prefix),
            Some(25)
        );
        assert_eq!(parse_cidr("10.0.0.1").map(|cidr| cidr.prefix), Some(32));
        // A non-contiguous mask is not a prefix.
        assert_eq!(parse_cidr("10.0.0.1 255.0.255.0"), None);
        assert_eq!(parse_cidr("not an address"), None);
        assert_eq!(parse_cidr("2001:db8::1/64"), None, "IPv4 only for now");
    }

    #[test]
    fn subnet_arithmetic() {
        let a = parse_cidr("10.0.0.1/24").unwrap();
        let b = parse_cidr("10.0.0.130/25").unwrap();
        assert_eq!(a.network_cidr(), "10.0.0.0/24");
        assert!(a.overlaps(b) && b.overlaps(a));
        assert!(a.contains(Ipv4Addr::new(10, 0, 0, 200)));
        assert!(!b.contains(Ipv4Addr::new(10, 0, 0, 1)));
        assert!(!a.overlaps(parse_cidr("10.0.1.1/24").unwrap()));
        assert!(
            parse_cidr("0.0.0.0/0")
                .unwrap()
                .contains(Ipv4Addr::new(1, 1, 1, 1))
        );
    }

    #[test]
    fn addresses_that_say_nothing_are_recognised() {
        for address in ["127.0.0.1", "169.254.3.4", "224.0.0.5", "0.0.0.0"] {
            assert!(!is_real_host(address.parse().unwrap()), "{address}");
        }
        assert!(is_real_host("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn redundancy_protocols_are_not_duplicates() {
        assert_eq!(virtual_mac("0000.0c07.ac0a"), Some("HSRP"));
        assert_eq!(virtual_mac("00:00:5e:00:01:14"), Some("VRRP"));
        assert_eq!(virtual_mac("0000.0c9f.f00a"), Some("HSRP v2"));
        assert_eq!(virtual_mac("0007.b400.0102"), Some("GLBP"));
        assert_eq!(virtual_mac("aabb.ccdd.eeff"), None);
        assert_eq!(virtual_mac("incomplete"), None);

        let config = Config::parsed(
            1,
            0,
            Syntax::Ios,
            "interface Vlan10\n ip address 10.0.0.2 255.255.255.0\n standby 1 ip 10.0.0.1\n\
             interface Vlan20\n vrrp 20 ip 10.0.20.1\n\
             interface Vlan30\n standby ip 10.0.30.1 secondary\n",
        );
        let virtuals = virtual_addresses(&config);
        assert!(virtuals.contains(&Ipv4Addr::new(10, 0, 0, 1)));
        assert!(virtuals.contains(&Ipv4Addr::new(10, 0, 20, 1)));
        assert!(virtuals.contains(&Ipv4Addr::new(10, 0, 30, 1)));
        assert!(!virtuals.contains(&Ipv4Addr::new(10, 0, 0, 2)));
        assert!(
            virtual_addresses(&Config::parsed(
                1,
                0,
                Syntax::Routeros,
                "/ip address add address=10.0.0.1/24\n"
            ))
            .is_empty()
        );
    }
}
