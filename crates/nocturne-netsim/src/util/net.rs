//! IPv4 arithmetic: prefixes, masks, wildcards.

use std::net::Ipv4Addr;

pub(crate) fn mask_from_len(len: u8) -> Ipv4Addr {
    let len = u32::from(len.min(32));
    Ipv4Addr::from(if len == 0 { 0 } else { u32::MAX << (32 - len) })
}

/// Prefix length of a contiguous subnet mask.
pub(crate) fn len_from_mask(mask: Ipv4Addr) -> Option<u8> {
    let m = u32::from(mask);
    (m.leading_ones() + m.trailing_zeros() == 32).then(|| m.leading_ones() as u8)
}

/// Strict dotted-quad parser (four decimal octets, nothing else).
pub(crate) fn parse_ip(s: &str) -> Option<Ipv4Addr> {
    let mut octets = [0u8; 4];
    let mut parts = s.split('.');
    for octet in &mut octets {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *octet = part.parse().ok()?;
    }
    parts.next().is_none().then(|| Ipv4Addr::from(octets))
}

/// `"10.0.0.1/24"` → address and prefix length.
pub(crate) fn parse_cidr(s: &str) -> Option<(Ipv4Addr, u8)> {
    let (ip, len) = s.split_once('/')?;
    let len: u8 = len.parse().ok()?;
    (len <= 32).then_some(())?;
    Some((parse_ip(ip)?, len))
}

/// A network: address with host bits cleared, and prefix length.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(crate) struct Prefix {
    pub(crate) net: Ipv4Addr,
    pub(crate) len: u8,
}

impl Prefix {
    pub(crate) const DEFAULT: Prefix = Prefix {
        net: Ipv4Addr::UNSPECIFIED,
        len: 0,
    };

    /// Host bits of `addr` are cleared.
    pub(crate) fn new(addr: Ipv4Addr, len: u8) -> Prefix {
        let len = len.min(32);
        Prefix {
            net: Ipv4Addr::from(u32::from(addr) & u32::from(mask_from_len(len))),
            len,
        }
    }

    pub(crate) fn from_mask(addr: Ipv4Addr, mask: Ipv4Addr) -> Option<Prefix> {
        Some(Prefix::new(addr, len_from_mask(mask)?))
    }

    pub(crate) fn mask(self) -> Ipv4Addr {
        mask_from_len(self.len)
    }

    pub(crate) fn contains(self, ip: Ipv4Addr) -> bool {
        u32::from(ip) & u32::from(self.mask()) == u32::from(self.net)
    }

    pub(crate) fn broadcast(self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.net) | !u32::from(self.mask()))
    }

    /// `other` lies entirely within `self`.
    pub(crate) fn covers(self, other: Prefix) -> bool {
        self.len <= other.len && self.contains(other.net)
    }

    /// Classful network of an address (used by RIP `network` and route display).
    pub(crate) fn classful(ip: Ipv4Addr) -> Prefix {
        let first = ip.octets()[0];
        let len = if first < 128 {
            8
        } else if first < 192 {
            16
        } else {
            24
        };
        Prefix::new(ip, len)
    }
}

impl std::fmt::Display for Prefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.net, self.len)
    }
}

/// An interface address: host address plus the prefix length of its subnet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct IfAddr {
    pub(crate) ip: Ipv4Addr,
    pub(crate) len: u8,
}

impl IfAddr {
    pub(crate) fn new(ip: Ipv4Addr, len: u8) -> IfAddr {
        IfAddr {
            ip,
            len: len.min(32),
        }
    }

    pub(crate) fn subnet(self) -> Prefix {
        Prefix::new(self.ip, self.len)
    }

    pub(crate) fn mask(self) -> Ipv4Addr {
        mask_from_len(self.len)
    }

    /// `ip` is on this address's subnet.
    pub(crate) fn on_link(self, ip: Ipv4Addr) -> bool {
        self.subnet().contains(ip)
    }
}

impl std::fmt::Display for IfAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.ip, self.len)
    }
}

/// ACL-style match: bits set in `wildcard` are "don't care".
pub(crate) fn wildcard_match(ip: Ipv4Addr, base: Ipv4Addr, wildcard: Ipv4Addr) -> bool {
    let care = !u32::from(wildcard);
    u32::from(ip) & care == u32::from(base) & care
}

pub(crate) fn invert(ip: Ipv4Addr) -> Ipv4Addr {
    Ipv4Addr::from(!u32::from(ip))
}

pub(crate) fn is_multicast(ip: Ipv4Addr) -> bool {
    ip.octets()[0] & 0xf0 == 0xe0
}

/// The address `n` above `ip`, if it does not overflow.
pub(crate) fn ip_add(ip: Ipv4Addr, n: u32) -> Option<Ipv4Addr> {
    u32::from(ip).checked_add(n).map(Ipv4Addr::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks() {
        assert_eq!(mask_from_len(0), Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(mask_from_len(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(mask_from_len(32), Ipv4Addr::BROADCAST);
        assert_eq!(len_from_mask(Ipv4Addr::new(255, 255, 252, 0)), Some(22));
        assert_eq!(len_from_mask(Ipv4Addr::new(0, 0, 0, 0)), Some(0));
        assert_eq!(len_from_mask(Ipv4Addr::new(255, 0, 255, 0)), None);
    }

    #[test]
    fn parsing() {
        assert_eq!(parse_ip("10.0.0.1"), Some(Ipv4Addr::new(10, 0, 0, 1)));
        for bad in [
            "",
            "10.0.0",
            "10.0.0.1.2",
            "10.0.0.256",
            "a.b.c.d",
            "10..0.1",
            "+1.2.3.4",
            "1.2.3.4 ",
        ] {
            assert_eq!(parse_ip(bad), None, "{bad}");
        }
        assert_eq!(
            parse_cidr("8.8.8.8/32"),
            Some((Ipv4Addr::new(8, 8, 8, 8), 32))
        );
        assert_eq!(parse_cidr("8.8.8.8/33"), None);
    }

    #[test]
    fn prefixes() {
        let p = Prefix::new(Ipv4Addr::new(10, 1, 2, 3), 24);
        assert_eq!(p.to_string(), "10.1.2.0/24");
        assert!(p.contains(Ipv4Addr::new(10, 1, 2, 200)));
        assert!(!p.contains(Ipv4Addr::new(10, 1, 3, 1)));
        assert_eq!(p.broadcast(), Ipv4Addr::new(10, 1, 2, 255));
        assert!(Prefix::DEFAULT.covers(p) && !p.covers(Prefix::DEFAULT));
        assert_eq!(
            Prefix::classful(Ipv4Addr::new(172, 16, 5, 1)).to_string(),
            "172.16.0.0/16"
        );
        assert!(wildcard_match(
            Ipv4Addr::new(10, 1, 2, 3),
            Ipv4Addr::new(10, 1, 0, 0),
            Ipv4Addr::new(0, 0, 255, 255)
        ));
        assert!(!wildcard_match(
            Ipv4Addr::new(10, 2, 2, 3),
            Ipv4Addr::new(10, 1, 0, 0),
            Ipv4Addr::new(0, 0, 255, 255)
        ));
    }
}
