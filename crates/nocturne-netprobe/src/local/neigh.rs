//! Parsers for the neighbour (ARP / NDP) table as each operating system prints it.

use std::net::IpAddr;
use std::sync::LazyLock;

use regex::Regex;

use super::Neighbour;
use crate::util::{format_mac, parse_ip_literal, parse_mac};

/// Builds an entry, dropping what is not a neighbour: unresolved entries (no MAC) and the
/// multicast / broadcast mappings some systems list alongside real hosts.
fn entry(address: &str, mac: &str, interface: &str, state: &str) -> Option<Neighbour> {
    let address = parse_ip_literal(address)?;
    let mac = parse_mac(mac)?;
    let group_address = mac[0] & 0x01 != 0;
    if group_address || mac == [0; 6] || address.is_multicast() {
        return None;
    }
    Some(Neighbour {
        address: address.to_string(),
        mac: format_mac(mac),
        interface: interface.to_owned(),
        state: state.to_owned(),
    })
}

/// Linux `ip neigh show`:
/// `192.0.2.1 dev eth0 lladdr 02:fc:00:00:00:05 REACHABLE`
pub(crate) fn parse_ip_neigh(text: &str) -> Vec<Neighbour> {
    text.lines()
        .filter_map(|line| {
            let tokens: Vec<&str> = line.split_whitespace().collect();
            let after = |keyword: &str| {
                tokens
                    .iter()
                    .position(|token| *token == keyword)
                    .and_then(|index| tokens.get(index + 1))
                    .copied()
            };
            let state = tokens.last()?.to_ascii_lowercase();
            if matches!(state.as_str(), "failed" | "incomplete" | "none") {
                return None;
            }
            entry(tokens.first()?, after("lladdr")?, after("dev")?, &state)
        })
        .collect()
}

/// Linux `/proc/net/arp` (IPv4 only), used when `ip` is not installed:
/// `192.0.2.1        0x1         0x2         02:fc:00:00:00:05     *        eth0`
pub(crate) fn parse_proc_arp(text: &str) -> Vec<Neighbour> {
    const ATF_COM: u32 = 0x02; // completed entry
    const ATF_PERM: u32 = 0x04;
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [address, _hw_type, flags, mac, _mask, interface] = fields[..] else {
                return None;
            };
            let flags = u32::from_str_radix(flags.trim_start_matches("0x"), 16).ok()?;
            if flags & ATF_COM == 0 {
                return None;
            }
            let state = if flags & ATF_PERM != 0 {
                "permanent"
            } else {
                "dynamic"
            };
            entry(address, mac, interface, state)
        })
        .collect()
}

/// `arp -an` on macOS, the BSDs and Linux net-tools:
/// `? (192.168.1.1) at a4:2b:b0:c1:d2:e3 on en0 ifscope [ethernet]`
/// `? (192.0.2.1) at 02:fc:00:00:00:05 [ether] on eth0`
pub(crate) fn parse_arp_an(text: &str) -> Vec<Neighbour> {
    // Between the MAC and "on": the hardware type and flags of net-tools ("[ether] PERM").
    static LINE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\(([^)]+)\) at (\S+)((?: \[\w+\]| PERM| PUB)*) on (\S+)(.*)$").unwrap()
    });
    text.lines()
        .filter_map(|line| {
            let captures = LINE.captures(line.trim())?;
            let state = if captures[3].contains("PERM") || captures[5].contains("permanent") {
                "permanent"
            } else {
                "dynamic"
            };
            entry(&captures[1], &captures[2], &captures[4], state)
        })
        .collect()
}

/// macOS / BSD `ndp -an`:
/// `fe80::1%en0   a4:2b:b0:c1:d2:e3   en0 23h59m58s S  R`
pub(crate) fn parse_ndp_an(text: &str) -> Vec<Neighbour> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [address, mac, interface, _expire, state, ..] = fields[..] else {
                return None;
            };
            let state = match state {
                "R" => "reachable",
                "S" => "stale",
                "D" => "delay",
                "P" => "probe",
                "I" | "N" | "W" => return None,
                _ => "reachable",
            };
            entry(address, mac, interface, state)
        })
        .collect()
}

/// Windows `arp -a`. Works in any UI language: interface headers are recognised by their
/// `--- 0x<index>` suffix, entries by an address followed by a dashed MAC.
///
/// ```text
/// Interface: 192.168.1.10 --- 0xb
///   Internet Address      Physical Address      Type
///   192.168.1.1           a4-2b-b0-c1-d2-e3     dynamic
/// ```
pub(crate) fn parse_arp_windows(text: &str) -> Vec<Neighbour> {
    static HEADER: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{1,3}(?:\.\d{1,3}){3})\s+---\s+0x[0-9a-fA-F]+").unwrap());
    let mut interface = String::new();
    let mut neighbours = Vec::new();
    for line in text.lines() {
        if let Some(captures) = HEADER.captures(line) {
            interface = captures[1].to_owned();
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [address, mac, kind, ..] = fields[..] else {
            continue;
        };
        if let Some(neighbour) = entry(address, mac, &interface, windows_state(kind)) {
            neighbours.push(neighbour);
        }
    }
    neighbours
}

/// "dynamic" / "static" in the UI language (dynamisch, dynamique, dinámico, estático, …).
fn windows_state(word: &str) -> &'static str {
    match word.chars().next().map(|c| c.to_ascii_lowercase()) {
        Some('d') => "dynamic",
        Some('s' | 'e') => "static",
        _ => "unknown",
    }
}

/// Windows `netsh interface ipv6 show neighbors`:
///
/// ```text
/// Interface 11: Ethernet
///
/// Internet Address                              Physical Address   Type
/// --------------------------------------------  -----------------  -----------
/// fe80::1                                       a4-2b-b0-c1-d2-e3  Reachable (Router)
/// ```
pub(crate) fn parse_netsh_neighbors(text: &str) -> Vec<Neighbour> {
    static HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\S+ \d+: (.+)$").unwrap());
    let mut interface = String::new();
    let mut neighbours = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(captures) = HEADER.captures(line) {
            interface = captures[1].trim().to_owned();
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [address, mac, kind, ..] = fields[..] else {
            continue;
        };
        if !address.contains(':') || address.parse::<IpAddr>().is_err() {
            continue;
        }
        let state = match kind.to_ascii_lowercase().as_str() {
            "unreachable" | "incomplete" => continue,
            // The state names are not translated consistently; keep the English ones we know.
            known @ ("reachable" | "stale" | "permanent" | "delay" | "probe") => known.to_owned(),
            _ => "reachable".to_owned(),
        };
        if let Some(neighbour) = entry(address, mac, &interface, &state) {
            neighbours.push(neighbour);
        }
    }
    neighbours
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn neighbour(address: &str, mac: &str, interface: &str, state: &str) -> Neighbour {
        Neighbour {
            address: address.into(),
            mac: mac.into(),
            interface: interface.into(),
            state: state.into(),
        }
    }

    #[test]
    fn linux_ip_neigh() {
        let text = "192.0.2.1 dev eth0 lladdr 02:fc:00:00:00:05 REACHABLE\n\
                    10.0.0.7 dev eth1 lladdr 00:1A:2B:3C:4D:5E STALE\n\
                    10.0.0.9 dev eth1  FAILED\n\
                    10.0.0.11 dev eth1 INCOMPLETE\n\
                    fe80::1 dev eth0 lladdr 00:11:22:33:44:55 router STALE\n\
                    2001:db8::7 dev eth0 lladdr 00:11:22:33:44:66 router DELAY\n\
                    10.0.0.200 dev br0 lladdr 52:54:00:12:34:56 PERMANENT\n\
                    ff02::16 dev eth0 lladdr 33:33:00:00:00:16 NOARP\n\
                    \n";
        assert_eq!(
            parse_ip_neigh(text),
            vec![
                neighbour("192.0.2.1", "02:fc:00:00:00:05", "eth0", "reachable"),
                neighbour("10.0.0.7", "00:1a:2b:3c:4d:5e", "eth1", "stale"),
                neighbour("fe80::1", "00:11:22:33:44:55", "eth0", "stale"),
                neighbour("2001:db8::7", "00:11:22:33:44:66", "eth0", "delay"),
                neighbour("10.0.0.200", "52:54:00:12:34:56", "br0", "permanent"),
            ]
        );
    }

    #[test]
    fn linux_proc_net_arp() {
        let text = "IP address       HW type     Flags       HW address            Mask     Device\n\
                    192.0.2.1        0x1         0x2         02:fc:00:00:00:05     *        eth0\n\
                    192.0.2.9        0x1         0x0         00:00:00:00:00:00     *        eth0\n\
                    192.0.2.20       0x1         0x6         52:54:00:aa:bb:cc     *        eth0\n";
        assert_eq!(
            parse_proc_arp(text),
            vec![
                neighbour("192.0.2.1", "02:fc:00:00:00:05", "eth0", "dynamic"),
                neighbour("192.0.2.20", "52:54:00:aa:bb:cc", "eth0", "permanent"),
            ]
        );
    }

    #[test]
    fn macos_arp() {
        let text = "? (192.168.1.1) at a4:2b:b0:c1:d2:e3 on en0 ifscope [ethernet]\n\
                    ? (192.168.1.23) at 0:c:29:a:b:c on en0 ifscope [ethernet]\n\
                    ? (192.168.1.50) at (incomplete) on en0 ifscope [ethernet]\n\
                    ? (192.168.1.255) at ff:ff:ff:ff:ff:ff on en0 ifscope [ethernet]\n\
                    ? (224.0.0.251) at 1:0:5e:0:0:fb on en0 ifscope permanent [ethernet]\n\
                    ? (192.168.64.1) at 3e:22:fb:1:2:64 on bridge100 ifscope permanent [bridge]\n";
        assert_eq!(
            parse_arp_an(text),
            vec![
                neighbour("192.168.1.1", "a4:2b:b0:c1:d2:e3", "en0", "dynamic"),
                neighbour("192.168.1.23", "00:0c:29:0a:0b:0c", "en0", "dynamic"),
                neighbour("192.168.64.1", "3e:22:fb:01:02:64", "bridge100", "permanent"),
            ]
        );
    }

    #[test]
    fn linux_and_freebsd_arp() {
        let linux = "? (192.0.2.1) at 02:fc:00:00:00:05 [ether] on eth0\n\
                     ? (10.0.0.9) at <incomplete> on eth0\n\
                     ? (10.0.0.3) at 52:54:00:12:34:56 [ether] PERM on eth0\n";
        assert_eq!(
            parse_arp_an(linux),
            vec![
                neighbour("192.0.2.1", "02:fc:00:00:00:05", "eth0", "dynamic"),
                neighbour("10.0.0.3", "52:54:00:12:34:56", "eth0", "permanent"),
            ]
        );
        let freebsd = "? (192.168.1.1) at a4:2b:b0:c1:d2:e3 on em0 expires in 1195 seconds [ethernet]\n\
                       ? (192.168.1.10) at 00:0c:29:aa:bb:cc on em0 permanent [ethernet]\n";
        assert_eq!(
            parse_arp_an(freebsd),
            vec![
                neighbour("192.168.1.1", "a4:2b:b0:c1:d2:e3", "em0", "dynamic"),
                neighbour("192.168.1.10", "00:0c:29:aa:bb:cc", "em0", "permanent"),
            ]
        );
    }

    #[test]
    fn macos_ndp() {
        let text = "Neighbor                                Linklayer Address  Netif Expire    St Flgs Prbs\n\
                    fe80::1%en0                             a4:2b:b0:c1:d2:e3    en0 23h59m58s S  R\n\
                    fe80::a683:e7ff:fe12:3456%en0           a4:83:e7:12:34:56    en0 permanent R\n\
                    2001:db8::5                             (incomplete)         en0 expired   N\n\
                    2001:db8::9                             0:1b:21:a:b:c        en0 5s        D\n";
        assert_eq!(
            parse_ndp_an(text),
            vec![
                neighbour("fe80::1", "a4:2b:b0:c1:d2:e3", "en0", "stale"),
                neighbour("fe80::a683:e7ff:fe12:3456", "a4:83:e7:12:34:56", "en0", "reachable"),
                neighbour("2001:db8::9", "00:1b:21:0a:0b:0c", "en0", "delay"),
            ]
        );
    }

    #[test]
    fn windows_arp_english_and_german() {
        let english = "\r\nInterface: 192.168.1.10 --- 0xb\r\n\
                       \x20 Internet Address      Physical Address      Type\r\n\
                       \x20 192.168.1.1           a4-2b-b0-c1-d2-e3     dynamic   \r\n\
                       \x20 192.168.1.42          00-1a-2b-3c-4d-5e     static    \r\n\
                       \x20 192.168.1.255         ff-ff-ff-ff-ff-ff     static    \r\n\
                       \x20 224.0.0.22            01-00-5e-00-00-16     static    \r\n\
                       \r\nInterface: 10.8.0.2 --- 0x15\r\n\
                       \x20 Internet Address      Physical Address      Type\r\n\
                       \x20 10.8.0.1              00-ff-aa-bb-cc-dd     dynamic   \r\n";
        assert_eq!(
            parse_arp_windows(english),
            vec![
                neighbour("192.168.1.1", "a4:2b:b0:c1:d2:e3", "192.168.1.10", "dynamic"),
                neighbour("192.168.1.42", "00:1a:2b:3c:4d:5e", "192.168.1.10", "static"),
                neighbour("10.8.0.1", "00:ff:aa:bb:cc:dd", "10.8.0.2", "dynamic"),
            ]
        );

        let german = "\r\nSchnittstelle: 192.168.1.10 --- 0xb\r\n\
                      \x20 Internetadresse       Physische Adresse     Typ\r\n\
                      \x20 192.168.1.1           a4-2b-b0-c1-d2-e3     dynamisch \r\n\
                      \x20 192.168.1.255         ff-ff-ff-ff-ff-ff     statisch  \r\n";
        assert_eq!(
            parse_arp_windows(german),
            vec![neighbour("192.168.1.1", "a4:2b:b0:c1:d2:e3", "192.168.1.10", "dynamic")]
        );

        let spanish = "\r\nInterfaz: 192.168.1.10 --- 0xb\r\n\
                       \x20 Dirección de Internet          Dirección física      Tipo\r\n\
                       \x20 192.168.1.1           a4-2b-b0-c1-d2-e3     dinámico  \r\n\
                       \x20 192.168.1.2           a4-2b-b0-c1-d2-e4     estático  \r\n";
        let states: Vec<String> = parse_arp_windows(spanish).into_iter().map(|n| n.state).collect();
        assert_eq!(states, ["dynamic", "static"]);
    }

    #[test]
    fn windows_netsh_ipv6() {
        let text = "\r\nInterface 11: Ethernet\r\n\r\n\r\n\
                    Internet Address                              Physical Address   Type\r\n\
                    --------------------------------------------  -----------------  -----------\r\n\
                    fe80::1                                       a4-2b-b0-c1-d2-e3  Reachable (Router)\r\n\
                    fe80::2c3:d4ff:fee5:f607                      00-c3-d4-e5-f6-07  Stale\r\n\
                    2001:db8::99                                  00-00-00-00-00-00  Unreachable\r\n\
                    ff02::1                                       33-33-00-00-00-01  Permanent\r\n\
                    \r\nInterface 1: Loopback Pseudo-Interface 1\r\n\r\n\r\n\
                    Internet Address                              Physical Address   Type\r\n\
                    --------------------------------------------  -----------------  -----------\r\n\
                    ff02::c                                                          Permanent\r\n";
        assert_eq!(
            parse_netsh_neighbors(text),
            vec![
                neighbour("fe80::1", "a4:2b:b0:c1:d2:e3", "Ethernet", "reachable"),
                neighbour("fe80::2c3:d4ff:fee5:f607", "00:c3:d4:e5:f6:07", "Ethernet", "stale"),
            ]
        );
    }

    #[test]
    fn garbage_yields_nothing() {
        for parser in [
            parse_ip_neigh,
            parse_proc_arp,
            parse_arp_an,
            parse_ndp_an,
            parse_arp_windows,
            parse_netsh_neighbors,
        ] {
            assert_eq!(parser(""), vec![]);
            assert_eq!(parser("\u{0}\u{1} dev lladdr at on ( ) --- 0x\n\n: : :\n"), vec![]);
            assert_eq!(parser("command not found\n"), vec![]);
        }
    }
}
