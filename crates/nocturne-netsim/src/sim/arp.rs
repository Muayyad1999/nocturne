//! ARP: the cache, answering requests, and resolving next hops.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::{PacketEventKind, SimTime};
use crate::ifname::IfId;
use crate::sim::frame::{Arp, Body, Frame, Meta};
use crate::sim::fx::{Fx, Why};
use crate::sim::node::Node;
use crate::util::mac::Mac;

/// Entries are refreshed by traffic and dropped after four hours, as on IOS.
pub(crate) const ARP_TIMEOUT_MICROS: u64 = 4 * 3600 * 1_000_000;
/// Do not repeat a request for the same address more often than this.
const REQUEST_INTERVAL_MICROS: u64 = 500_000;

#[derive(Clone, Debug)]
pub(crate) struct ArpEntry {
    pub(crate) mac: Mac,
    pub(crate) iface: IfId,
    pub(crate) learned: SimTime,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ArpCache {
    entries: BTreeMap<Ipv4Addr, ArpEntry>,
    /// Addresses being resolved: when the last request went out.
    pending: BTreeMap<Ipv4Addr, SimTime>,
}

impl ArpCache {
    pub(crate) fn lookup(&self, ip: Ipv4Addr, now: SimTime) -> Option<&ArpEntry> {
        self.entries
            .get(&ip)
            .filter(|e| now.saturating_sub(e.learned) < ARP_TIMEOUT_MICROS)
    }

    /// Returns true when the mapping is new or changed.
    pub(crate) fn insert(&mut self, ip: Ipv4Addr, mac: Mac, iface: IfId, now: SimTime) -> bool {
        self.pending.remove(&ip);
        let changed = self
            .entries
            .get(&ip)
            .is_none_or(|e| e.mac != mac || e.iface != iface);
        self.entries.insert(
            ip,
            ArpEntry {
                mac,
                iface,
                learned: now,
            },
        );
        changed
    }

    pub(crate) fn contains(&self, ip: Ipv4Addr) -> bool {
        self.entries.contains_key(&ip)
    }

    pub(crate) fn retain_ifaces(&mut self, live: &[IfId]) {
        self.entries.retain(|_, e| live.contains(&e.iface));
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.pending.clear();
    }

    pub(crate) fn remove(&mut self, ip: Ipv4Addr) {
        self.entries.remove(&ip);
    }

    pub(crate) fn rows(&self, now: SimTime) -> Vec<(Ipv4Addr, &ArpEntry)> {
        self.entries
            .iter()
            .filter(|(_, e)| now.saturating_sub(e.learned) < ARP_TIMEOUT_MICROS)
            .map(|(ip, e)| (*ip, e))
            .collect()
    }

    /// Has a request for `ip` gone unanswered (used to word host ping failures)?
    pub(crate) fn is_pending(&self, ip: Ipv4Addr) -> bool {
        self.pending.contains_key(&ip)
    }
}

impl Node {
    /// The MAC address this node answers with when asked for `ip` on `iface`,
    /// if the address is its to answer for.
    fn arp_answer(&self, iface: IfId, ip: Ipv4Addr) -> Option<Mac> {
        if self.addrs(iface).iter().any(|a| a.ip == ip) {
            return Some(self.mac_for(iface));
        }
        if let Some(mac) = self.hsrp_virtual_mac(iface, ip) {
            return Some(mac);
        }
        // Translated addresses that live on the outside subnet are ours to answer for.
        (self.nat_owns_global(iface, ip)).then(|| self.mac_for(iface))
    }

    pub(crate) fn arp_rx(&mut self, fx: &mut Fx, iface: IfId, arp: Arp, meta: Meta) {
        let frame = Frame {
            src: arp.sender_mac,
            dst: arp.target_mac,
            tag: None,
            body: Body::Arp(arp),
            meta,
        };
        let on_link = self.addrs(iface).iter().any(|a| a.on_link(arp.sender_ip));
        let answer = self.arp_answer(iface, arp.target_ip);
        let noun = self.noun();
        if !on_link || arp.sender_ip.is_unspecified() {
            fx.dropped(self.id, &frame, Some(iface), Why::Ignored, || {
                format!(
                    "The ARP sender {} is not on the subnet of {}: ignored.",
                    arp.sender_ip,
                    iface.short()
                )
            });
            return;
        }
        // Learn from ARP traffic addressed to us; refresh what we already know.
        if answer.is_some() || self.arp.contains(arp.sender_ip) {
            if self
                .arp
                .insert(arp.sender_ip, arp.sender_mac, iface, fx.now)
            {
                fx.changed = true;
            }
        }
        match (arp.request, answer) {
            (true, Some(mac)) => {
                fx.note(self.id, PacketEventKind::Delivered, &frame, Some(iface), None, || {
                    format!(
                        "The request asks for {}, which this {noun} owns. It records {} → {} in its ARP table and replies with its own MAC address {mac}.",
                        arp.target_ip, arp.sender_ip, arp.sender_mac
                    )
                });
                let reply = Arp {
                    request: false,
                    sender_mac: mac,
                    sender_ip: arp.target_ip,
                    target_mac: arp.sender_mac,
                    target_ip: arp.sender_ip,
                };
                let mut out = Frame {
                    src: mac,
                    dst: arp.sender_mac,
                    tag: None,
                    body: Body::Arp(reply),
                    meta: self.caused(fx, &meta),
                };
                out.meta.via = fx.note(
                    self.id,
                    PacketEventKind::Created,
                    &out,
                    Some(iface),
                    None,
                    || {
                        format!(
                            "The {noun} sends the ARP reply straight back to {}.",
                            arp.sender_mac
                        )
                    },
                );
                self.l3_tx(fx, iface, mac, arp.sender_mac, out.body, out.meta);
            }
            (false, _) => {
                fx.note(self.id, PacketEventKind::Delivered, &frame, Some(iface), None, || {
                    format!(
                        "The reply tells this {noun} that {} is at {}; the mapping goes into its ARP table and waiting traffic can now be sent.",
                        arp.sender_ip, arp.sender_mac
                    )
                });
            }
            (true, None) => fx.dropped(self.id, &frame, Some(iface), Why::Ignored, || {
                format!(
                    "The request asks for {}, which is not this {noun}'s address: ignored.",
                    arp.target_ip
                )
            }),
        }
    }

    /// Metadata for a PDU created in response to another one: new identity,
    /// same trace, chained to the event that caused it.
    pub(crate) fn caused(&self, fx: &mut Fx, cause: &Meta) -> Meta {
        Meta {
            pdu: fx.new_pdu(),
            trace: cause.trace,
            via: cause.via,
            hops: 0,
        }
    }

    /// MAC address of `next_hop` on `iface`. On a miss an ARP request goes out
    /// (rate-limited) and the caller must drop its packet.
    pub(crate) fn arp_resolve(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        next_hop: Ipv4Addr,
        cause: &Meta,
    ) -> Option<Mac> {
        if let Some(entry) = self.arp.lookup(next_hop, fx.now) {
            return Some(entry.mac);
        }
        let recently = self
            .arp
            .pending
            .get(&next_hop)
            .is_some_and(|at| fx.now.saturating_sub(*at) < REQUEST_INTERVAL_MICROS);
        if recently {
            return None;
        }
        self.arp.pending.insert(next_hop, fx.now);
        let Some(source) = self
            .addrs(iface)
            .into_iter()
            .find(|a| a.on_link(next_hop))
            .or_else(|| self.addr(iface))
        else {
            return None;
        };
        let mac = self.mac_for(iface);
        let request = Arp {
            request: true,
            sender_mac: mac,
            sender_ip: source.ip,
            target_mac: Mac::ZERO,
            target_ip: next_hop,
        };
        let mut frame = Frame {
            src: mac,
            dst: Mac::BROADCAST,
            tag: None,
            body: Body::Arp(request),
            meta: self.caused(fx, cause),
        };
        let noun = self.noun();
        frame.meta.via = fx.note(self.id, PacketEventKind::Created, &frame, Some(iface), None, || {
            format!(
                "The {noun} has no ARP entry for {next_hop}, so it broadcasts an ARP request out of {} asking who owns that address.",
                iface.short()
            )
        });
        self.l3_tx(fx, iface, mac, Mac::BROADCAST, frame.body, frame.meta);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_expires_after_four_hours() {
        let iface = IfId::parse("g0/0").unwrap();
        let ip = Ipv4Addr::new(10, 0, 0, 2);
        let mut cache = ArpCache::default();
        assert!(cache.insert(ip, Mac([2, 0, 0, 0, 0, 1]), iface, 0));
        assert!(!cache.insert(ip, Mac([2, 0, 0, 0, 0, 1]), iface, 10));
        assert!(cache.insert(ip, Mac([2, 0, 0, 0, 0, 2]), iface, 20));
        assert!(cache.lookup(ip, ARP_TIMEOUT_MICROS - 1).is_some());
        assert!(cache.lookup(ip, ARP_TIMEOUT_MICROS + 20).is_none());
        cache.retain_ifaces(&[]);
        assert!(cache.rows(0).is_empty());
    }
}
