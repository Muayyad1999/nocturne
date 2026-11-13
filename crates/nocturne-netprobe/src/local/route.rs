//! Parsers for the default route(s) as each operating system prints them.

use std::net::{IpAddr, Ipv4Addr};

use super::LocalRoute;
use crate::util::parse_ip_literal;

const DEFAULT_V4: &str = "0.0.0.0/0";
const DEFAULT_V6: &str = "::/0";

fn route(gateway: IpAddr, interface: &str) -> LocalRoute {
    LocalRoute {
        destination: match gateway {
            IpAddr::V4(_) => DEFAULT_V4,
            IpAddr::V6(_) => DEFAULT_V6,
        }
        .to_owned(),
        gateway: gateway.to_string(),
        interface: interface.to_owned(),
    }
}

/// Linux `ip route show default` and `ip -6 route show default`, including multipath routes:
///
/// ```text
/// default via 192.168.1.1 dev wlan0 proto dhcp src 192.168.1.10 metric 600
/// default proto static metric 1024
///         nexthop via 10.0.0.1 dev eth0 weight 1
/// ```
///
/// Default routes without a gateway (point-to-point links, `default dev tun0`) are skipped:
/// there is no next hop to report.
pub(crate) fn parse_ip_route(text: &str) -> Vec<LocalRoute> {
    let mut routes = Vec::new();
    let mut in_default = false;
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        match tokens.first() {
            Some(&"default") => in_default = true,
            Some(&"nexthop") if in_default => {}
            _ => {
                in_default = false;
                continue;
            }
        }
        let after = |keyword: &str| {
            tokens
                .iter()
                .position(|token| *token == keyword)
                .and_then(|index| tokens.get(index + 1))
                .copied()
        };
        let (Some(gateway), Some(interface)) = (after("via"), after("dev")) else {
            continue;
        };
        // "via inet6 fe80::1" (an IPv4 route over an IPv6 next hop) names the family first.
        let gateway = if gateway == "inet6" || gateway == "inet" {
            tokens
                .iter()
                .position(|token| *token == "via")
                .and_then(|index| tokens.get(index + 2))
                .copied()
                .unwrap_or(gateway)
        } else {
            gateway
        };
        if let Some(gateway) = parse_ip_literal(gateway) {
            routes.push(route(gateway, interface));
        }
    }
    routes
}

/// Linux `/proc/net/route` (IPv4 only), used when `ip` is not installed. Addresses are
/// hexadecimal in host (little-endian) byte order:
/// `eth0    00000000        010200C0        0003    0       0       0       00000000 …`
pub(crate) fn parse_proc_route(text: &str) -> Vec<LocalRoute> {
    const RTF_UP: u32 = 0x1;
    const RTF_GATEWAY: u32 = 0x2;
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let hex = |index: usize| u32::from_str_radix(fields.get(index)?, 16).ok();
            let (destination, gateway, flags, mask) = (hex(1)?, hex(2)?, hex(3)?, hex(7)?);
            let wanted = RTF_UP | RTF_GATEWAY;
            if destination != 0 || mask != 0 || flags & wanted != wanted {
                return None;
            }
            let gateway = Ipv4Addr::from(gateway.swap_bytes());
            Some(route(IpAddr::V4(gateway), fields[0]))
        })
        .collect()
}

/// macOS / BSD `netstat -rn`:
///
/// ```text
/// Destination        Gateway            Flags               Netif Expire
/// default            192.168.1.1        UGScg                 en0
/// default            fe80::1%en0        UGcg                  en0
/// ```
pub(crate) fn parse_netstat_rn(text: &str) -> Vec<LocalRoute> {
    let mut routes: Vec<LocalRoute> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let [destination, gateway, _flags, rest @ ..] = &fields[..] else {
            continue;
        };
        if *destination != "default" {
            continue;
        }
        // "link#17" gateways are directly connected, and "fe80::%utun0" is how macOS prints
        // the default route of a tunnel: neither names a next hop.
        let Some(gateway) =
            parse_ip_literal(gateway).filter(|gateway| !is_bare_link_local(gateway))
        else {
            continue;
        };
        // macOS/FreeBSD: Netif right after Flags; NetBSD/OpenBSD have Refs/Use/Mtu before it.
        let Some(interface) = rest.iter().find(|field| !is_number(field)) else {
            continue;
        };
        let found = route(gateway, interface);
        if !routes.contains(&found) {
            routes.push(found);
        }
    }
    routes
}

fn is_bare_link_local(address: &IpAddr) -> bool {
    matches!(address, IpAddr::V6(v6) if v6.segments() == [0xfe80, 0, 0, 0, 0, 0, 0, 0])
}

fn is_number(field: &str) -> bool {
    field == "-" || field.chars().all(|c| c.is_ascii_digit())
}

/// Windows `route print`, in any UI language: the IPv4 default route is the row whose first two
/// columns are `0.0.0.0 0.0.0.0`, the IPv6 one the row with destination `::/0`.
///
/// ```text
///           0.0.0.0          0.0.0.0      192.168.1.1     192.168.1.10     25
///  11    281 ::/0                     fe80::1
/// ```
///
/// The interface is what Windows prints: the interface's address (IPv4) or index (IPv6).
pub(crate) fn parse_route_print(text: &str) -> Vec<LocalRoute> {
    let mut routes: Vec<LocalRoute> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        let found = match fields[..] {
            ["0.0.0.0", "0.0.0.0", gateway, interface, _metric] => gateway
                .parse::<Ipv4Addr>()
                .ok()
                .map(|gateway| route(IpAddr::V4(gateway), interface)),
            [index, _metric, "::/0", gateway] => parse_ip_literal(gateway)
                .filter(IpAddr::is_ipv6)
                .map(|gateway| route(gateway, index)),
            _ => None,
        };
        if let Some(found) = found {
            if !routes.contains(&found) {
                routes.push(found);
            }
        }
    }
    routes
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn default_route(gateway: &str, interface: &str) -> LocalRoute {
        route(gateway.parse().unwrap(), interface)
    }

    #[test]
    fn linux_ip_route() {
        let v4 = "default via 192.168.1.1 dev wlan0 proto dhcp src 192.168.1.10 metric 600 \n\
                  default via 10.0.0.1 dev eth0 proto static metric 100 \n\
                  default dev tun0 scope link \n\
                  10.0.0.0/24 dev eth0 proto kernel scope link src 10.0.0.5 \n";
        assert_eq!(
            parse_ip_route(v4),
            vec![
                default_route("192.168.1.1", "wlan0"),
                default_route("10.0.0.1", "eth0")
            ]
        );
        assert_eq!(
            parse_ip_route("default via 192.0.2.1 dev eth0 \n"),
            vec![default_route("192.0.2.1", "eth0")]
        );

        let v6 = "default via fe80::1 dev eth0 proto ra metric 100 expires 1798sec pref medium\n";
        let routes = parse_ip_route(v6);
        assert_eq!(routes, vec![default_route("fe80::1", "eth0")]);
        assert_eq!(routes[0].destination, "::/0");

        let multipath = "default proto static metric 1024 \n\
                         \tnexthop via 10.0.0.1 dev eth0 weight 1 \n\
                         \tnexthop via 10.0.1.1 dev eth1 weight 1 \n\
                         192.168.5.0/24 proto static \n\
                         \tnexthop via 10.0.0.9 dev eth0 weight 1 \n";
        assert_eq!(
            parse_ip_route(multipath),
            vec![
                default_route("10.0.0.1", "eth0"),
                default_route("10.0.1.1", "eth1")
            ]
        );

        let over_v6 = "default via inet6 fe80::1 dev eth0 \n";
        assert_eq!(
            parse_ip_route(over_v6),
            vec![default_route("fe80::1", "eth0")]
        );
    }

    #[test]
    fn linux_proc_net_route() {
        let text = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
                    eth0\t00000000\t010200C0\t0003\t0\t0\t0\t00000000\t0\t0\t0\n\
                    eth0\t000200C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
                    eth1\t00000000\t00000000\t0001\t0\t0\t50\t00000000\t0\t0\t0\n";
        // 0x010200C0 little-endian is c0.00.02.01 = 192.0.2.1.
        assert_eq!(
            parse_proc_route(text),
            vec![default_route("192.0.2.1", "eth0")]
        );
    }

    #[test]
    fn macos_netstat() {
        let text = "Routing tables\n\
                    \n\
                    Internet:\n\
                    Destination        Gateway            Flags               Netif Expire\n\
                    default            192.168.1.1        UGScg                 en0       \n\
                    default            link#17            UCSIg             bridge100      !\n\
                    127                127.0.0.1          UCS                   lo0       \n\
                    192.168.1          link#6             UCS                   en0      !\n\
                    \n\
                    Internet6:\n\
                    Destination                             Gateway                                 Flags               Netif Expire\n\
                    default                                 fe80::1%en0                             UGcg                  en0       \n\
                    default                                 fe80::%utun0                            UGcIg               utun0       \n\
                    ::1                                     ::1                                     UHL                   lo0       \n";
        assert_eq!(
            parse_netstat_rn(text),
            vec![
                default_route("192.168.1.1", "en0"),
                default_route("fe80::1", "en0")
            ]
        );
    }

    #[test]
    fn openbsd_netstat_has_extra_columns() {
        let text = "Internet:\n\
                    Destination        Gateway            Flags   Refs      Use   Mtu  Prio Iface\n\
                    default            10.0.2.2           UGS        5      120     -     8 em0  \n";
        assert_eq!(
            parse_netstat_rn(text),
            vec![default_route("10.0.2.2", "em0")]
        );
    }

    #[test]
    fn windows_route_print() {
        let text = "===========================================================================\r\n\
                    Interface List\r\n\
                    \x2011...a4 83 e7 12 34 56 ......Intel(R) Ethernet Connection\r\n\
                    \x20 1...........................Software Loopback Interface 1\r\n\
                    ===========================================================================\r\n\
                    \r\n\
                    IPv4 Route Table\r\n\
                    ===========================================================================\r\n\
                    Active Routes:\r\n\
                    Network Destination        Netmask          Gateway       Interface  Metric\r\n\
                    \x20         0.0.0.0          0.0.0.0      192.168.1.1     192.168.1.10     25\r\n\
                    \x20       127.0.0.0        255.0.0.0         On-link         127.0.0.1    331\r\n\
                    \x20     192.168.1.0    255.255.255.0         On-link      192.168.1.10    281\r\n\
                    ===========================================================================\r\n\
                    Persistent Routes:\r\n\
                    \x20 Network Address          Netmask  Gateway Address  Metric\r\n\
                    \x20         0.0.0.0          0.0.0.0      192.168.1.1  Default \r\n\
                    ===========================================================================\r\n\
                    \r\n\
                    IPv6 Route Table\r\n\
                    ===========================================================================\r\n\
                    Active Routes:\r\n\
                    \x20If Metric Network Destination      Gateway\r\n\
                    \x2011    281 ::/0                     fe80::1\r\n\
                    \x20 1    331 ::1/128                  On-link\r\n\
                    \x2011    281 fe80::/64                On-link\r\n\
                    ===========================================================================\r\n";
        assert_eq!(
            parse_route_print(text),
            vec![
                default_route("192.168.1.1", "192.168.1.10"),
                default_route("fe80::1", "11")
            ]
        );
        // A VPN default route without a next hop.
        let on_link =
            "          0.0.0.0          0.0.0.0         On-link         10.8.0.2     35\r\n";
        assert_eq!(parse_route_print(on_link), vec![]);
        // The same table on a German system: only the words differ.
        let german = "Aktive Routen:\r\n\
                      \x20    Netzwerkziel    Netzwerkmaske          Gateway    Schnittstelle Metrik\r\n\
                      \x20         0.0.0.0          0.0.0.0      192.168.1.1     192.168.1.10     25\r\n\
                      \x20       127.0.0.0        255.0.0.0   Auf Verbindung         127.0.0.1    331\r\n";
        assert_eq!(
            parse_route_print(german),
            vec![default_route("192.168.1.1", "192.168.1.10")]
        );
    }

    #[test]
    fn garbage_yields_nothing() {
        for parser in [
            parse_ip_route,
            parse_proc_route,
            parse_netstat_rn,
            parse_route_print,
        ] {
            assert_eq!(parser(""), vec![]);
            assert_eq!(
                parser("default\ndefault via\ndefault via x dev\n0.0.0.0 0.0.0.0\n::/0\n"),
                vec![]
            );
            assert_eq!(parser("\u{0}\u{7f}\n\t\t\t\n"), vec![]);
        }
    }
}
