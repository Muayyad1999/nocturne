//! Access lists: matching packets, hit counters, logging, and the canonical
//! text of an entry (shared by explanations, `show access-lists` and the
//! running configuration).

use std::net::Ipv4Addr;

use crate::device::config::{Acl, AclProto, AclRule, AddrSpec, IcmpClass, PortSpec};
use crate::sim::frame::{Icmp, L4, Packet, TcpFlags};
use crate::sim::fx::Fx;
use crate::sim::node::Node;

pub(crate) enum Verdict {
    Permit,
    /// Denied; the text completes "ACL X on Gi0/0 ...", e.g. ``matches line 10 `deny icmp any any` ``.
    Deny(String),
}

fn ports(packet: &Packet) -> Option<(u16, u16)> {
    match packet.l4 {
        L4::Udp { sport, dport, .. } | L4::Tcp { sport, dport, .. } => Some((sport, dport)),
        _ => None,
    }
}

pub(crate) fn rule_matches(rule: &AclRule, extended: bool, packet: &Packet) -> bool {
    if !rule.src.matches(packet.src) {
        return false;
    }
    if !extended {
        return true;
    }
    let proto_ok = match rule.proto {
        AclProto::Ip => true,
        AclProto::Icmp => matches!(packet.l4, L4::Icmp(_)),
        AclProto::Tcp => matches!(packet.l4, L4::Tcp { .. }),
        AclProto::Udp => matches!(packet.l4, L4::Udp { .. }),
    };
    if !proto_ok || !rule.dst.matches(packet.dst) {
        return false;
    }
    if rule.sport.is_some() || rule.dport.is_some() {
        let Some((sport, dport)) = ports(packet) else {
            return false;
        };
        if rule.sport.is_some_and(|spec| !spec.matches(sport))
            || rule.dport.is_some_and(|spec| !spec.matches(dport))
        {
            return false;
        }
    }
    if let Some(class) = rule.icmp {
        let ok = matches!(
            (class, &packet.l4),
            (IcmpClass::Echo, L4::Icmp(Icmp::EchoRequest { .. }))
                | (IcmpClass::EchoReply, L4::Icmp(Icmp::EchoReply { .. }))
                | (IcmpClass::Unreachable, L4::Icmp(Icmp::Unreachable { .. }))
                | (IcmpClass::TimeExceeded, L4::Icmp(Icmp::TimeExceeded { .. }))
        );
        if !ok {
            return false;
        }
    }
    // `established` = a segment with ACK or RST set, i.e. not an opening SYN.
    !rule.established
        || matches!(
            packet.l4,
            L4::Tcp {
                flags: TcpFlags::SynAck | TcpFlags::Ack | TcpFlags::Rst,
                ..
            }
        )
}

/// Well-known port names as IOS prints (and accepts) them.
const TCP_PORTS: &[(u16, &str)] = &[
    (20, "ftp-data"),
    (21, "ftp"),
    (23, "telnet"),
    (25, "smtp"),
    (53, "domain"),
    (80, "www"),
    (110, "pop3"),
    (179, "bgp"),
];
const UDP_PORTS: &[(u16, &str)] = &[
    (53, "domain"),
    (67, "bootps"),
    (68, "bootpc"),
    (69, "tftp"),
    (123, "ntp"),
    (161, "snmp"),
    (162, "snmptrap"),
    (514, "syslog"),
    (520, "rip"),
];

fn port_table(proto: AclProto) -> &'static [(u16, &'static str)] {
    if proto == AclProto::Udp {
        UDP_PORTS
    } else {
        TCP_PORTS
    }
}

pub(crate) fn port_name(proto: AclProto, port: u16) -> String {
    port_table(proto)
        .iter()
        .find(|(n, _)| *n == port)
        .map_or_else(|| port.to_string(), |(_, name)| (*name).to_string())
}

/// Parse a port given as a number or a well-known name.
pub(crate) fn parse_port(proto: AclProto, text: &str) -> Option<u16> {
    text.parse().ok().or_else(|| {
        port_table(proto)
            .iter()
            .find(|(_, name)| *name == text)
            .map(|(n, _)| *n)
    })
}

fn addr_text(spec: AddrSpec) -> String {
    match spec {
        AddrSpec::Any => "any".to_string(),
        AddrSpec::Host(ip) => format!("host {ip}"),
        AddrSpec::Net(base, wildcard) => format!("{base} {wildcard}"),
    }
}

fn port_text(proto: AclProto, spec: PortSpec) -> String {
    match spec {
        PortSpec::Eq(p) => format!("eq {}", port_name(proto, p)),
        PortSpec::Neq(p) => format!("neq {}", port_name(proto, p)),
        PortSpec::Gt(p) => format!("gt {}", port_name(proto, p)),
        PortSpec::Lt(p) => format!("lt {}", port_name(proto, p)),
        PortSpec::Range(a, b) => format!("range {} {}", port_name(proto, a), port_name(proto, b)),
    }
}

/// The entry as typed in configuration, without sequence number:
/// `permit tcp any host 10.0.0.5 eq www`.
pub(crate) fn rule_text(rule: &AclRule, extended: bool) -> String {
    let action = if rule.permit { "permit" } else { "deny" };
    if !extended {
        // A standard entry for a single host is written without the keyword.
        let src = match rule.src {
            AddrSpec::Host(ip) => ip.to_string(),
            other => addr_text(other),
        };
        return format!("{action} {src}{}", if rule.log { " log" } else { "" });
    }
    let mut text = format!("{action} {} {}", rule.proto.keyword(), addr_text(rule.src));
    if let Some(spec) = rule.sport {
        text.push_str(&format!(" {}", port_text(rule.proto, spec)));
    }
    text.push_str(&format!(" {}", addr_text(rule.dst)));
    if let Some(spec) = rule.dport {
        text.push_str(&format!(" {}", port_text(rule.proto, spec)));
    }
    if let Some(class) = rule.icmp {
        text.push_str(&format!(" {}", class.keyword()));
    }
    if rule.established {
        text.push_str(" established");
    }
    if rule.log {
        text.push_str(" log");
    }
    text
}

/// The entry as `show access-lists` prints it (standard entries spell out
/// wildcard bits).
pub(crate) fn rule_show_text(rule: &AclRule, extended: bool) -> String {
    if extended {
        return rule_text(rule, true);
    }
    let action = if rule.permit { "permit" } else { "deny  " };
    let src = match rule.src {
        AddrSpec::Any => "any".to_string(),
        AddrSpec::Host(ip) => ip.to_string(),
        AddrSpec::Net(base, wildcard) => format!("{base}, wildcard bits {wildcard}"),
    };
    format!("{action} {src}{}", if rule.log { " log" } else { "" })
}

impl Node {
    /// Run a packet through a named list. A list that does not exist permits
    /// everything, as on real devices.
    pub(crate) fn acl_eval(&mut self, fx: &mut Fx, name: &str, packet: &Packet) -> Verdict {
        let Some(acl) = self.cfg.acl(name) else {
            return Verdict::Permit;
        };
        if acl.rules().next().is_none() {
            return Verdict::Permit;
        }
        let extended = acl.extended;
        let Some(rule) = acl
            .rules()
            .find(|rule| rule_matches(rule, extended, packet))
            .cloned()
        else {
            return Verdict::Deny(
                "matches no entry, so the implicit deny at the end of the list applies".to_string(),
            );
        };
        *self
            .acl_hits
            .entry((name.to_string(), rule.seq))
            .or_insert(0) += 1;
        if rule.log {
            self.acl_log(fx, name, extended, rule.permit, packet);
        }
        if rule.permit {
            Verdict::Permit
        } else {
            Verdict::Deny(format!(
                "matches line {} `{}`",
                rule.seq,
                rule_text(&rule, extended)
            ))
        }
    }

    /// Does a list permit traffic from `src` (vty `access-class`, NAT source lists)?
    pub(crate) fn acl_permits_source(&mut self, name: &str, src: Ipv4Addr) -> bool {
        let Some(acl) = self.cfg.acl(name) else {
            return true;
        };
        if acl.rules().next().is_none() {
            return true;
        }
        match acl
            .rules()
            .find(|rule| rule.src.matches(src))
            .map(|rule| (rule.seq, rule.permit))
        {
            Some((seq, permit)) => {
                *self.acl_hits.entry((name.to_string(), seq)).or_insert(0) += 1;
                permit
            }
            None => false,
        }
    }

    fn acl_log(&mut self, fx: &mut Fx, name: &str, extended: bool, permit: bool, packet: &Packet) {
        let action = if permit { "permitted" } else { "denied" };
        let (mnemonic, message) = if !extended {
            (
                "IPACCESSLOGNP",
                format!(
                    "list {name} {action} 0 {} -> {}, 1 packet",
                    packet.src, packet.dst
                ),
            )
        } else {
            match &packet.l4 {
                L4::Tcp { sport, dport, .. } => (
                    "IPACCESSLOGP",
                    format!(
                        "list {name} {action} tcp {}({sport}) -> {}({dport}), 1 packet",
                        packet.src, packet.dst
                    ),
                ),
                L4::Udp { sport, dport, .. } => (
                    "IPACCESSLOGP",
                    format!(
                        "list {name} {action} udp {}({sport}) -> {}({dport}), 1 packet",
                        packet.src, packet.dst
                    ),
                ),
                L4::Icmp(icmp) => {
                    let kind = match icmp {
                        Icmp::EchoRequest { .. } => "8/0",
                        Icmp::EchoReply { .. } => "0/0",
                        Icmp::Unreachable { .. } => "3/1",
                        Icmp::TimeExceeded { .. } => "11/0",
                    };
                    (
                        "IPACCESSLOGDP",
                        format!(
                            "list {name} {action} icmp {} -> {} ({kind}), 1 packet",
                            packet.src, packet.dst
                        ),
                    )
                }
                L4::Ospf(_) => (
                    "IPACCESSLOGNP",
                    format!(
                        "list {name} {action} 89 {} -> {}, 1 packet",
                        packet.src, packet.dst
                    ),
                ),
            }
        };
        self.log(fx, 6, "SEC", mnemonic, message);
    }
}

/// Hit counter of one entry.
pub(crate) fn hits(node: &Node, acl: &Acl, seq: u32) -> u64 {
    node.acl_hits
        .get(&(acl.name.clone(), seq))
        .copied()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(permit: bool, proto: AclProto) -> AclRule {
        AclRule {
            seq: 10,
            permit,
            proto,
            src: AddrSpec::Any,
            sport: None,
            dst: AddrSpec::Any,
            dport: None,
            icmp: None,
            established: false,
            log: false,
        }
    }

    fn tcp(flags: TcpFlags, dport: u16) -> Packet {
        Packet {
            src: Ipv4Addr::new(10, 0, 0, 1),
            dst: Ipv4Addr::new(10, 0, 1, 5),
            ttl: 64,
            size: 44,
            l4: L4::Tcp {
                sport: 50000,
                dport,
                flags,
            },
        }
    }

    #[test]
    fn matching() {
        let mut r = rule(true, AclProto::Tcp);
        r.dst = AddrSpec::Host(Ipv4Addr::new(10, 0, 1, 5));
        r.dport = Some(PortSpec::Eq(80));
        assert!(rule_matches(&r, true, &tcp(TcpFlags::Syn, 80)));
        assert!(!rule_matches(&r, true, &tcp(TcpFlags::Syn, 443)));
        r.dport = Some(PortSpec::Range(80, 443));
        assert!(rule_matches(&r, true, &tcp(TcpFlags::Syn, 443)));
        r.established = true;
        assert!(!rule_matches(&r, true, &tcp(TcpFlags::Syn, 80)));
        assert!(rule_matches(&r, true, &tcp(TcpFlags::SynAck, 80)));
        let mut std_rule = rule(false, AclProto::Ip);
        std_rule.src = AddrSpec::Net(Ipv4Addr::new(10, 0, 0, 0), Ipv4Addr::new(0, 0, 0, 255));
        assert!(rule_matches(&std_rule, false, &tcp(TcpFlags::Syn, 80)));
        std_rule.src = AddrSpec::Host(Ipv4Addr::new(10, 0, 0, 9));
        assert!(!rule_matches(&std_rule, false, &tcp(TcpFlags::Syn, 80)));
    }

    #[test]
    fn text_forms() {
        let mut r = rule(true, AclProto::Tcp);
        r.dst = AddrSpec::Host(Ipv4Addr::new(10, 0, 1, 5));
        r.dport = Some(PortSpec::Eq(80));
        assert_eq!(rule_text(&r, true), "permit tcp any host 10.0.1.5 eq www");
        assert_eq!(parse_port(AclProto::Tcp, "www"), Some(80));
        assert_eq!(parse_port(AclProto::Tcp, "8080"), Some(8080));
        assert_eq!(parse_port(AclProto::Udp, "www"), None);
        let mut s = rule(false, AclProto::Ip);
        s.src = AddrSpec::Net(Ipv4Addr::new(192, 168, 1, 0), Ipv4Addr::new(0, 0, 0, 255));
        assert_eq!(rule_text(&s, false), "deny 192.168.1.0 0.0.0.255");
        assert_eq!(
            rule_show_text(&s, false),
            "deny   192.168.1.0, wildcard bits 0.0.0.255"
        );
    }
}
