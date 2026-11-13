//! Address arithmetic and the sanitising of anything a person typed.

use std::fmt;
use std::net::Ipv4Addr;

/// An IPv4 network. Small enough that doing the arithmetic here is clearer
/// than passing strings around; every result is checked against
/// `nocturne_tools::subnet` in the tests.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Net {
    /// Network address.
    pub(crate) addr: u32,
    pub(crate) len: u8,
}

impl Net {
    pub(crate) fn new(addr: u32, len: u8) -> Net {
        let len = len.min(32);
        Net {
            addr: addr & mask_of(len),
            len,
        }
    }

    /// Parse `"10.0.0.0/16"`. Anything else — a missing or silly prefix, an
    /// IPv6 address, junk — is `None`; the caller turns that into a sentence.
    pub(crate) fn parse(text: &str) -> Option<Net> {
        let text = text.trim();
        let (addr, len) = text.split_once('/')?;
        let addr: Ipv4Addr = addr.trim().parse().ok()?;
        let len: u8 = len.trim().parse().ok()?;
        if len > 32 {
            return None;
        }
        Some(Net::new(u32::from(addr), len))
    }

    pub(crate) fn size(self) -> u64 {
        1u64 << (32 - u32::from(self.len))
    }

    pub(crate) fn last(self) -> u32 {
        self.addr | !mask_of(self.len)
    }

    /// First address that can be given to a host, by the usual rules
    /// (a /31 has two, a /32 has one).
    pub(crate) fn first_host(self) -> u32 {
        if self.len >= 31 {
            self.addr
        } else {
            self.addr + 1
        }
    }

    pub(crate) fn last_host(self) -> u32 {
        if self.len >= 31 {
            self.last()
        } else {
            self.last().saturating_sub(1)
        }
    }

    pub(crate) fn usable(self) -> u64 {
        match self.len {
            32 => 1,
            31 => 2,
            _ => self.size() - 2,
        }
    }

    pub(crate) fn contains(self, addr: u32) -> bool {
        addr >= self.addr && addr <= self.last()
    }

    pub(crate) fn overlaps(self, other: Net) -> bool {
        self.addr <= other.last() && other.addr <= self.last()
    }

    pub(crate) fn netmask(self) -> String {
        fmt_ip(mask_of(self.len))
    }

    pub(crate) fn wildcard(self) -> String {
        fmt_ip(!mask_of(self.len))
    }

    /// The nth address in the network, or `None` past the end.
    pub(crate) fn nth(self, n: u32) -> Option<u32> {
        let addr = u64::from(self.addr) + u64::from(n);
        (addr <= u64::from(self.last())).then_some(addr as u32)
    }
}

impl fmt::Display for Net {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", fmt_ip(self.addr), self.len)
    }
}

pub(crate) fn mask_of(len: u8) -> u32 {
    if len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(len.min(32)))
    }
}

pub(crate) fn fmt_ip(addr: u32) -> String {
    Ipv4Addr::from(addr).to_string()
}

/// The shortest prefix length whose block holds at least `addresses`.
pub(crate) fn prefix_for_block(addresses: u64) -> u8 {
    let mut len = 32u8;
    while len > 0 && (1u64 << (32 - u32::from(len))) < addresses {
        len -= 1;
    }
    len
}

/// The prefix length of the smallest subnet with at least `hosts` usable
/// addresses, by the usual `2^n - 2` rule. `None` if no IPv4 subnet is big
/// enough.
pub(crate) fn prefix_for_hosts(hosts: u64) -> Option<u8> {
    if hosts <= 1 {
        return Some(32);
    }
    if hosts == 2 {
        return Some(30);
    }
    (0..=29u8).rev().find(|len| {
        let size = 1u64 << (32 - u32::from(*len));
        size - 2 >= hosts
    })
}

// ------------------------------------------------------------- sanitising

/// A short, safe tag for device names, VLAN names and interface descriptions.
///
/// Nothing a person typed ever reaches a configuration line unchanged: only
/// `A-Z`, `0-9` and `-` survive, runs of separators collapse, and the result
/// is capped. An input with no usable characters — empty, all punctuation, a
/// newline and a configuration command — yields the empty string, and the
/// caller substitutes a generated name.
pub(crate) fn tag(input: &str, max: usize) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= max {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

/// One line of free text for a `description`, a banner or a note: control
/// characters (a newline above all, which would end the command and let the
/// rest be read as configuration) are replaced by spaces, and the length is
/// capped well inside what the CLI accepts.
pub(crate) fn one_line(input: &str, max: usize) -> String {
    let mut out = String::new();
    let mut spaced = false;
    for ch in input.chars() {
        let printable = !ch.is_control() && ch.is_ascii() && ch != '"' && ch != '?';
        if printable && !(ch == ' ' && (out.is_empty() || spaced)) {
            out.push(ch);
            spaced = ch == ' ';
        } else if !out.is_empty() && !spaced {
            out.push(' ');
            spaced = true;
        }
        if out.len() >= max {
            break;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Text for a `banner` command, whose delimiter would otherwise end the
/// command and let the rest of the line be read as configuration.
pub(crate) fn banner_text(input: &str, max: usize) -> String {
    one_line(
        &one_line(input, max).replace(['#', '^', '%', '!'], " "),
        max,
    )
}

/// `x` rounded up after adding `pct` per cent, without overflowing.
pub(crate) fn with_growth(x: u64, pct: u32) -> u64 {
    let pct = u64::from(pct.min(10_000));
    x.saturating_mul(100 + pct).div_ceil(100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nets_do_the_usual_arithmetic() {
        let n = Net::parse("10.20.0.0/16").unwrap();
        assert_eq!(n.size(), 65_536);
        assert_eq!(fmt_ip(n.last()), "10.20.255.255");
        assert_eq!(fmt_ip(n.first_host()), "10.20.0.1");
        assert_eq!(n.netmask(), "255.255.0.0");
        assert_eq!(n.wildcard(), "0.0.255.255");
        let p = Net::parse("10.0.0.4/30").unwrap();
        assert_eq!(
            (fmt_ip(p.first_host()), fmt_ip(p.last_host())),
            ("10.0.0.5".into(), "10.0.0.6".into())
        );
        let p31 = Net::parse("10.0.0.4/31").unwrap();
        assert_eq!(
            (fmt_ip(p31.first_host()), fmt_ip(p31.last_host())),
            ("10.0.0.4".into(), "10.0.0.5".into())
        );
        assert_eq!(p31.usable(), 2);
        // A host address is snapped to its network.
        assert_eq!(
            Net::parse("10.20.3.7/24").unwrap().to_string(),
            "10.20.3.0/24"
        );
    }

    #[test]
    fn bad_blocks_are_refused_rather_than_guessed() {
        for text in [
            "",
            "10.0.0.0",
            "10.0.0.0/33",
            "2001:db8::/32",
            "10.0.0.0/-1",
            "hello",
            "999.0.0.0/8",
        ] {
            assert!(Net::parse(text).is_none(), "{text}");
        }
        assert_eq!(Net::parse("0.0.0.0/0").unwrap().size(), 1u64 << 32);
    }

    #[test]
    fn prefixes_are_sized_the_classic_way() {
        assert_eq!(prefix_for_hosts(1), Some(32));
        assert_eq!(prefix_for_hosts(2), Some(30));
        assert_eq!(prefix_for_hosts(3), Some(29));
        assert_eq!(prefix_for_hosts(30), Some(27));
        assert_eq!(prefix_for_hosts(31), Some(26));
        assert_eq!(prefix_for_hosts(254), Some(24));
        assert_eq!(prefix_for_hosts(255), Some(23));
        assert_eq!(prefix_for_hosts(4_294_967_294), Some(0));
        assert_eq!(prefix_for_hosts(4_294_967_295), None);
        assert_eq!(prefix_for_block(1), 32);
        assert_eq!(prefix_for_block(5), 29);
    }

    #[test]
    fn typed_names_cannot_become_configuration() {
        assert_eq!(tag("Acme branch", 12), "ACME-BRANCH");
        assert_eq!(tag("HQ\nhostname EVIL\n", 12), "HQ-HOSTNAME");
        assert_eq!(tag("   ", 12), "");
        assert_eq!(tag("...", 12), "");
        assert_eq!(tag("Büro Köln", 12), "B-RO-K-LN");
        assert_eq!(tag("a".repeat(100).as_str(), 8).len(), 8);
        assert_eq!(one_line("two\nlines\there", 60), "two lines here");
        assert_eq!(one_line("  spaced   out  ", 60), "spaced out");
        assert!(one_line("a\u{202e}b", 60).is_ascii());
        // A banner's delimiter can never appear inside it.
        assert_eq!(
            banner_text("Acme # hostname EVIL #", 60),
            "Acme hostname EVIL"
        );
        assert!(!banner_text("a#b^c%d!e", 60).contains(['#', '^', '%', '!']));
    }

    #[test]
    fn growth_rounds_up_and_never_overflows() {
        assert_eq!(with_growth(120, 25), 150);
        assert_eq!(with_growth(1, 25), 2);
        assert_eq!(with_growth(0, 25), 0);
        // Saturates rather than wrapping; the callers clamp to what fits.
        assert_eq!(with_growth(u64::MAX, 25), u64::MAX / 100 + 1);
        assert_eq!(with_growth(100, u32::MAX), 10_100);
    }
}
