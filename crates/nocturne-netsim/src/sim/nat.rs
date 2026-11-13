//! Network address translation: static, dynamic pool, and port-overloaded
//! (PAT) inside-source NAT, with a translation table that times out.

use std::net::Ipv4Addr;

use crate::api::{NatRow, SimTime};
use crate::device::config::{NatProto, NatSide, NatTarget};
use crate::ifname::IfId;
use crate::sim::frame::{Flow, IP_PROTO_ICMP, IP_PROTO_TCP, IP_PROTO_UDP, Icmp, L4, Packet};
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::util::net::ip_add;

const ICMP_TIMEOUT_S: u64 = 60;
const UDP_TIMEOUT_S: u64 = 300;
const TCP_TIMEOUT_S: u64 = 86_400;
const MAX_ENTRIES: usize = 4096;

#[derive(Clone, Debug)]
pub(crate) struct Translation {
    /// IP protocol number; `None` for a one-to-one address mapping.
    pub(crate) proto: Option<u8>,
    pub(crate) inside_local: (Ipv4Addr, u16),
    pub(crate) inside_global: (Ipv4Addr, u16),
    pub(crate) outside: (Ipv4Addr, u16),
    last_used: SimTime,
}

impl Translation {
    fn timeout_micros(&self) -> u64 {
        1_000_000
            * match self.proto {
                Some(IP_PROTO_ICMP) => ICMP_TIMEOUT_S,
                Some(IP_PROTO_UDP) => UDP_TIMEOUT_S,
                _ => TCP_TIMEOUT_S,
            }
    }

    fn live(&self, now: SimTime) -> bool {
        now.saturating_sub(self.last_used) < self.timeout_micros()
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Nat {
    pub(crate) entries: Vec<Translation>,
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) expired: u64,
}

/// Protocol and the source/destination "ports" NAT keys on (the echo
/// identifier stands in for both ports of an ICMP echo).
fn ports(packet: &Packet) -> Option<(u8, u16, u16)> {
    match packet.flow() {
        Flow::Icmp { id, .. } => Some((IP_PROTO_ICMP, id, id)),
        Flow::Udp { sport, dport } => Some((IP_PROTO_UDP, sport, dport)),
        Flow::Tcp { sport, dport } => Some((IP_PROTO_TCP, sport, dport)),
        Flow::Other => None,
    }
}

fn set_source_port(packet: &mut Packet, port: u16) {
    match &mut packet.l4 {
        L4::Icmp(Icmp::EchoRequest { id, .. } | Icmp::EchoReply { id, .. }) => *id = port,
        L4::Udp { sport, .. } | L4::Tcp { sport, .. } => *sport = port,
        _ => {}
    }
}

fn set_destination_port(packet: &mut Packet, port: u16) {
    match &mut packet.l4 {
        L4::Icmp(Icmp::EchoRequest { id, .. } | Icmp::EchoReply { id, .. }) => *id = port,
        L4::Udp { dport, .. } | L4::Tcp { dport, .. } => *dport = port,
        _ => {}
    }
}

fn proto_name(proto: Option<u8>) -> &'static str {
    match proto {
        Some(IP_PROTO_ICMP) => "icmp",
        Some(IP_PROTO_UDP) => "udp",
        Some(IP_PROTO_TCP) => "tcp",
        _ => "---",
    }
}

impl Node {
    fn nat_expire(&mut self, now: SimTime) {
        let before = self.nat.entries.len();
        self.nat.entries.retain(|e| e.live(now));
        self.nat.expired += (before - self.nat.entries.len()) as u64;
    }

    /// Inside → outside: rewrite the source. `Ok(None)` when no rule applies,
    /// `Err` when the packet must be dropped (pool exhausted).
    pub(crate) fn nat_outbound(
        &mut self,
        fx: &mut Fx,
        packet: &mut Packet,
        out: IfId,
    ) -> Result<Option<String>, String> {
        let now = fx.now;
        let local = packet.src;
        let flow = ports(packet);
        // Static port forwards and one-to-one mappings come first.
        for entry in self.cfg.nat.statics.clone() {
            match (entry.ports, flow) {
                (Some((proto, local_port, global_port)), Some((p, sport, _)))
                    if entry.local == local
                        && sport == local_port
                        && p == if proto == NatProto::Tcp {
                            IP_PROTO_TCP
                        } else {
                            IP_PROTO_UDP
                        } =>
                {
                    packet.src = entry.global;
                    set_source_port(packet, global_port);
                    self.nat.hits += 1;
                    return Ok(Some(format!(
                        "NAT (inside to outside): static rule, source {local}:{local_port} becomes {}:{global_port}. ",
                        entry.global
                    )));
                }
                (None, _) if entry.local == local => {
                    packet.src = entry.global;
                    self.nat.hits += 1;
                    return Ok(Some(format!(
                        "NAT (inside to outside): static rule, source {local} becomes {}. ",
                        entry.global
                    )));
                }
                _ => {}
            }
        }
        self.nat_expire(now);
        let rules = self.cfg.nat.rules.clone();
        let Some(rule) = rules
            .into_iter()
            .find(|rule| self.nat_rule_matches(&rule.acl, packet))
        else {
            return Ok(None);
        };
        let pool = match &rule.target {
            NatTarget::Pool(name) => self.cfg.nat.pools.iter().find(|p| p.name == *name).cloned(),
            NatTarget::Interface(_) => None,
        };
        let global_ip = match (&rule.target, &pool) {
            (NatTarget::Interface(iface), _) => {
                let Some(addr) = self.addr(*iface) else {
                    return Err(format!(
                        "NAT cannot translate {local}: {} has no address to translate to.",
                        iface.short()
                    ));
                };
                addr.ip
            }
            (NatTarget::Pool(_), Some(pool)) if rule.overload => pool.start,
            (NatTarget::Pool(name), Some(pool)) => {
                // One global address per inside host.
                let existing = self
                    .nat
                    .entries
                    .iter()
                    .find(|e| e.proto.is_none() && e.inside_local.0 == local)
                    .map(|e| e.inside_global.0);
                let size = u32::from(pool.end).saturating_sub(u32::from(pool.start));
                let free = (0..=size)
                    .filter_map(|i| ip_add(pool.start, i))
                    .find(|ip| !self.nat.entries.iter().any(|e| e.inside_global.0 == *ip));
                match existing.or(free) {
                    Some(ip) => ip,
                    None => {
                        self.nat.misses += 1;
                        return Err(format!(
                            "NAT pool {name} has no free address left for {local}: the packet is dropped."
                        ));
                    }
                }
            }
            (NatTarget::Pool(name), None) => {
                return Err(format!(
                    "NAT pool {name} does not exist: the packet is dropped."
                ));
            }
        };
        let _ = out;
        if !rule.overload && pool.is_some() {
            match self
                .nat
                .entries
                .iter_mut()
                .find(|e| e.proto.is_none() && e.inside_local.0 == local)
            {
                Some(entry) => entry.last_used = now,
                None => {
                    self.nat.entries.push(Translation {
                        proto: None,
                        inside_local: (local, 0),
                        inside_global: (global_ip, 0),
                        outside: (Ipv4Addr::UNSPECIFIED, 0),
                        last_used: now,
                    });
                    fx.changed = true;
                }
            }
            packet.src = global_ip;
            self.nat.hits += 1;
            return Ok(Some(format!(
                "NAT (inside to outside): source {local} becomes {global_ip} from the pool. "
            )));
        }
        // Port address translation.
        let Some((proto, sport, dport)) = flow else {
            return Ok(None);
        };
        let outside = (packet.dst, dport);
        // An ICMP echo has one identifier, not two ports: the far end is
        // identified by its address alone.
        let icmp = proto == IP_PROTO_ICMP;
        let existing = self.nat.entries.iter_mut().find(|e| {
            e.proto == Some(proto)
                && e.inside_local == (local, sport)
                && e.inside_global.0 == global_ip
                && e.outside.0 == outside.0
                && (icmp || e.outside.1 == outside.1)
        });
        let global_port = match existing {
            Some(entry) => {
                entry.last_used = now;
                entry.inside_global.1
            }
            None => {
                // Keep the port if nobody else uses it on the global address.
                let taken = |port: u16, entries: &[Translation]| {
                    entries.iter().any(|e| {
                        e.proto == Some(proto)
                            && e.inside_global == (global_ip, port)
                            && e.inside_local != (local, sport)
                    })
                };
                let port = if taken(sport, &self.nat.entries) {
                    (1024..=u16::MAX)
                        .find(|p| !taken(*p, &self.nat.entries))
                        .unwrap_or(sport)
                } else {
                    sport
                };
                if self.nat.entries.len() < MAX_ENTRIES {
                    self.nat.entries.push(Translation {
                        proto: Some(proto),
                        inside_local: (local, sport),
                        inside_global: (global_ip, port),
                        outside,
                        last_used: now,
                    });
                }
                self.nat.misses += 1;
                fx.changed = true;
                port
            }
        };
        packet.src = global_ip;
        set_source_port(packet, global_port);
        self.nat.hits += 1;
        Ok(Some(format!(
            "NAT (inside to outside, overload): source {local}:{sport} becomes {global_ip}:{global_port}, so many inside hosts can share one address. "
        )))
    }

    fn nat_rule_matches(&mut self, acl: &str, packet: &Packet) -> bool {
        match self.cfg.acl(acl) {
            None => false,
            Some(list) if list.extended => {
                let rule = list
                    .rules()
                    .find(|r| crate::sim::acl::rule_matches(r, true, packet))
                    .map(|r| (r.seq, r.permit));
                match rule {
                    Some((seq, permit)) => {
                        *self.acl_hits.entry((acl.to_string(), seq)).or_insert(0) += 1;
                        permit
                    }
                    None => false,
                }
            }
            Some(_) => self.acl_permits_source(acl, packet.src),
        }
    }

    /// Outside → inside: rewrite the destination back to the inside address.
    pub(crate) fn nat_inbound(&mut self, fx: &mut Fx, packet: &mut Packet) -> Option<String> {
        let now = fx.now;
        let global = packet.dst;
        // ICMP errors about a translated packet carry its (translated) header.
        if let L4::Icmp(Icmp::Unreachable { orig, .. } | Icmp::TimeExceeded { orig }) =
            &mut packet.l4
        {
            let key = match orig.flow {
                Flow::Icmp { id, .. } => Some((IP_PROTO_ICMP, id)),
                Flow::Udp { sport, .. } => Some((IP_PROTO_UDP, sport)),
                Flow::Tcp { sport, .. } => Some((IP_PROTO_TCP, sport)),
                Flow::Other => None,
            };
            let entry = key.and_then(|(proto, port)| {
                self.nat
                    .entries
                    .iter()
                    .find(|e| e.proto == Some(proto) && e.inside_global == (orig.src, port))
            });
            let one_to_one = self
                .nat
                .entries
                .iter()
                .find(|e| e.proto.is_none() && e.inside_global.0 == orig.src);
            let (local, port) = match (entry, one_to_one) {
                (Some(entry), _) => (entry.inside_local.0, Some(entry.inside_local.1)),
                (None, Some(entry)) => (entry.inside_local.0, None),
                (None, None) => {
                    let entry = self
                        .cfg
                        .nat
                        .statics
                        .iter()
                        .find(|s| s.ports.is_none() && s.global == orig.src)?;
                    (entry.local, None)
                }
            };
            orig.src = local;
            if let Some(port) = port {
                orig.flow = match orig.flow {
                    Flow::Icmp { seq, .. } => Flow::Icmp { id: port, seq },
                    Flow::Udp { dport, .. } => Flow::Udp { sport: port, dport },
                    Flow::Tcp { dport, .. } => Flow::Tcp { sport: port, dport },
                    Flow::Other => Flow::Other,
                };
            }
            packet.dst = local;
            self.nat.hits += 1;
            return Some(format!(
                "NAT (outside to inside): the error concerns a translated packet, so it goes to the inside host {local}. "
            ));
        }
        let flow = ports(packet);
        for entry in &self.cfg.nat.statics {
            match (entry.ports, flow) {
                (Some((proto, local_port, global_port)), Some((p, _, dport)))
                    if entry.global == global
                        && dport == global_port
                        && p == if proto == NatProto::Tcp {
                            IP_PROTO_TCP
                        } else {
                            IP_PROTO_UDP
                        } =>
                {
                    let local = entry.local;
                    packet.dst = local;
                    set_destination_port(packet, local_port);
                    self.nat.hits += 1;
                    return Some(format!(
                        "NAT (outside to inside): static port forward, destination {global}:{global_port} becomes {local}:{local_port}. "
                    ));
                }
                (None, _) if entry.global == global => {
                    packet.dst = entry.local;
                    self.nat.hits += 1;
                    return Some(format!(
                        "NAT (outside to inside): static rule, destination {global} becomes {}. ",
                        entry.local
                    ));
                }
                _ => {}
            }
        }
        self.nat_expire(now);
        if let Some((proto, sport, dport)) = flow
            && let Some(entry) = self.nat.entries.iter_mut().find(|e| {
                e.proto == Some(proto)
                    && e.inside_global == (global, dport)
                    && e.outside.0 == packet.src
                    && (proto == IP_PROTO_ICMP || e.outside.1 == sport)
            })
        {
            entry.last_used = now;
            let (local, local_port) = entry.inside_local;
            packet.dst = local;
            set_destination_port(packet, local_port);
            self.nat.hits += 1;
            return Some(format!(
                "NAT (outside to inside): the translation table maps {global}:{dport} back to the inside host {local}:{local_port}. "
            ));
        }
        if let Some(entry) = self
            .nat
            .entries
            .iter_mut()
            .find(|e| e.proto.is_none() && e.inside_global.0 == global)
        {
            entry.last_used = now;
            packet.dst = entry.inside_local.0;
            self.nat.hits += 1;
            return Some(format!(
                "NAT (outside to inside): destination {global} is the pool address of inside host {}. ",
                entry.inside_local.0
            ));
        }
        None
    }

    /// Does this router answer ARP for `ip` on `iface` because it translates to it?
    pub(crate) fn nat_owns_global(&self, iface: IfId, ip: Ipv4Addr) -> bool {
        let Some(cfg) = self.cfg.iface(iface) else {
            return false;
        };
        if cfg.nat != Some(NatSide::Outside) || !self.addrs(iface).iter().any(|a| a.on_link(ip)) {
            return false;
        }
        let value = u32::from(ip);
        self.cfg.nat.statics.iter().any(|s| s.global == ip)
            || self
                .cfg
                .nat
                .pools
                .iter()
                .any(|p| (u32::from(p.start)..=u32::from(p.end)).contains(&value))
    }

    /// Drop translations whose rules are gone.
    pub(crate) fn nat_sync(&mut self) {
        if self.cfg.nat.rules.is_empty() && !self.nat.entries.is_empty() {
            self.nat.entries.clear();
        }
    }

    pub(crate) fn nat_clear(&mut self) {
        self.nat.entries.clear();
    }

    /// Static and dynamic translations as (protocol, inside global, inside
    /// local, outside local, outside global).
    pub(crate) fn nat_table(&self, now: SimTime) -> Vec<[String; 5]> {
        let pair = |(ip, port): (Ipv4Addr, u16)| format!("{ip}:{port}");
        let mut rows = Vec::new();
        for entry in self.nat.entries.iter().filter(|e| e.live(now)) {
            rows.push(match entry.proto {
                // For ICMP the "ports" are the echo identifier before and after translation.
                Some(IP_PROTO_ICMP) => [
                    "icmp".to_string(),
                    pair(entry.inside_global),
                    pair(entry.inside_local),
                    pair((entry.outside.0, entry.inside_local.1)),
                    pair((entry.outside.0, entry.inside_global.1)),
                ],
                Some(_) => [
                    proto_name(entry.proto).to_string(),
                    pair(entry.inside_global),
                    pair(entry.inside_local),
                    pair(entry.outside),
                    pair(entry.outside),
                ],
                None => [
                    "---".to_string(),
                    entry.inside_global.0.to_string(),
                    entry.inside_local.0.to_string(),
                    "---".to_string(),
                    "---".to_string(),
                ],
            });
        }
        for entry in &self.cfg.nat.statics {
            rows.push(match entry.ports {
                Some((proto, local_port, global_port)) => [
                    if proto == NatProto::Tcp { "tcp" } else { "udp" }.to_string(),
                    format!("{}:{global_port}", entry.global),
                    format!("{}:{local_port}", entry.local),
                    "---".to_string(),
                    "---".to_string(),
                ],
                None => [
                    "---".to_string(),
                    entry.global.to_string(),
                    entry.local.to_string(),
                    "---".to_string(),
                    "---".to_string(),
                ],
            });
        }
        rows
    }

    pub(crate) fn nat_rows(&self, now: SimTime) -> Vec<NatRow> {
        self.nat_table(now)
            .into_iter()
            .map(
                |[
                    protocol,
                    inside_global,
                    inside_local,
                    outside_local,
                    outside_global,
                ]| NatRow {
                    protocol,
                    inside_global,
                    inside_local,
                    outside_local,
                    outside_global,
                },
            )
            .collect()
    }
}
