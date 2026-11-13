//! Ethernet switching: VLAN classification, MAC learning and aging, forwarding
//! and flooding, 802.1Q tagging on egress, and port security.

use std::collections::BTreeMap;

use crate::api::{PacketEventKind, SimTime};
use crate::device::config::Violation;
use crate::ifname::IfId;
use crate::sim::frame::{Body, Frame};
use crate::sim::fx::{Fx, Why};
use crate::sim::node::Node;
use crate::sim::port::short_list;
use crate::sim::stp::PortState;
use crate::util::mac::Mac;
use crate::util::rng;

/// Dynamic entries are forgotten after five minutes without traffic.
pub(crate) const MAC_AGING_MICROS: u64 = 300_000_000;
/// A frame that crosses this many bridges is caught in a loop and is discarded.
pub(crate) const MAX_BRIDGE_HOPS: u8 = 48;

#[derive(Clone, Debug)]
pub(crate) struct MacEntry {
    pub(crate) port: IfId,
    pub(crate) seen: SimTime,
    pub(crate) is_static: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MacTable {
    entries: BTreeMap<(u16, Mac), MacEntry>,
}

impl MacTable {
    pub(crate) fn lookup(&self, vlan: u16, mac: Mac, now: SimTime) -> Option<IfId> {
        self.entries
            .get(&(vlan, mac))
            .filter(|e| e.is_static || now.saturating_sub(e.seen) < MAC_AGING_MICROS)
            .map(|e| e.port)
    }

    /// Record `mac` on `port`; returns true if this is new information.
    pub(crate) fn learn(&mut self, vlan: u16, mac: Mac, port: IfId, now: SimTime) -> bool {
        match self.entries.get_mut(&(vlan, mac)) {
            Some(entry) if entry.is_static => false,
            Some(entry) => {
                let fresh = now.saturating_sub(entry.seen) < MAC_AGING_MICROS;
                let moved = entry.port != port;
                entry.port = port;
                entry.seen = now;
                moved || !fresh
            }
            None => {
                self.entries.insert((vlan, mac), MacEntry { port, seen: now, is_static: false });
                true
            }
        }
    }

    pub(crate) fn set_static(&mut self, vlan: u16, mac: Mac, port: IfId, now: SimTime) {
        self.entries.insert((vlan, mac), MacEntry { port, seen: now, is_static: true });
    }

    /// Keep only entries on ports that are still usable.
    pub(crate) fn retain_ports(&mut self, ports: &[IfId]) {
        self.entries.retain(|_, e| ports.contains(&e.port));
    }

    /// Forget dynamic entries of a VLAN, except those on `keep` (topology change).
    pub(crate) fn flush_vlan(&mut self, vlan: u16, keep: Option<IfId>) {
        self.entries.retain(|(v, _), e| *v != vlan || e.is_static || Some(e.port) == keep);
    }

    pub(crate) fn flush_port(&mut self, port: IfId) {
        self.entries.retain(|_, e| e.port != port);
    }

    pub(crate) fn clear_dynamic(&mut self) {
        self.entries.retain(|_, e| e.is_static);
    }

    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Live entries in table order: VLAN, address, entry.
    pub(crate) fn rows(&self, now: SimTime) -> Vec<(u16, Mac, &MacEntry)> {
        self.entries
            .iter()
            .filter(|(_, e)| e.is_static || now.saturating_sub(e.seen) < MAC_AGING_MICROS)
            .map(|((vlan, mac), e)| (*vlan, *mac, e))
            .collect()
    }
}

impl Node {
    /// Which VLAN an incoming frame belongs to on a switch port, or why it is refused.
    fn classify(&self, port: IfId, tag: Option<u16>) -> Result<u16, String> {
        let Some(sw) = self.cfg.iface(port).and_then(|cfg| cfg.l2.as_ref()) else {
            return Err(format!("{} is not a switch port.", port.short()));
        };
        let vlan = if self.is_trunk(port) {
            let vlan = tag.unwrap_or(sw.native_vlan);
            if !sw.allowed.contains(vlan) {
                return Err(format!(
                    "VLAN {vlan} is not in the allowed list of trunk {}, so the switch discards the frame.",
                    port.short()
                ));
            }
            vlan
        } else {
            match tag {
                None => sw.access_vlan,
                Some(v) if sw.voice_vlan == Some(v) => v,
                Some(v) => {
                    return Err(format!(
                        "{} is an access port in VLAN {} and expects untagged frames, but this one carries an 802.1Q tag for VLAN {v}: discarded.",
                        port.short(),
                        sw.access_vlan
                    ));
                }
            }
        };
        if !self.cfg.vlan_active(vlan) {
            return Err(format!(
                "VLAN {vlan} does not exist (or is suspended) on this switch, so frames arriving in it on {} are discarded.",
                port.short()
            ));
        }
        Ok(vlan)
    }

    /// Ports (other than `except`) that forward `vlan` right now.
    fn flood_ports(&self, vlan: u16, except: Option<IfId>) -> Vec<IfId> {
        self.l2_ports()
            .into_iter()
            .filter(|p| Some(*p) != except && self.port_carries(*p, vlan) && self.stp.state(vlan, *p) == PortState::Forwarding)
            .collect()
    }

    /// A frame arrived on layer-2 port `port` (physical member `phys`).
    pub(crate) fn switch_rx(&mut self, fx: &mut Fx, port: IfId, phys: IfId, mut frame: Frame) {
        if let Body::Bpdu(bpdu) = frame.body {
            self.stp_rx(fx, port, phys, bpdu, &frame);
            return;
        }
        frame.meta.hops = frame.meta.hops.saturating_add(1);
        if frame.meta.hops > MAX_BRIDGE_HOPS {
            fx.dropped(self.id, &frame, Some(port), Why::Fatal, || {
                "The frame has crossed far too many switches: it is circulating in a layer-2 loop and is discarded.".to_string()
            });
            return;
        }
        let vlan = match self.classify(port, frame.tag) {
            Ok(vlan) => vlan,
            Err(reason) => {
                if let Some(state) = self.ifs.get_mut(&phys) {
                    state.counters.in_discards += 1;
                }
                fx.dropped(self.id, &frame, Some(port), Why::Fatal, || reason);
                return;
            }
        };
        let state = self.stp.state(vlan, port);
        if !matches!(state, PortState::Forwarding | PortState::Learning) {
            fx.dropped(self.id, &frame, Some(port), Why::Blocked, || {
                format!(
                    "Spanning tree holds {} in the {} state for VLAN {vlan}, so the switch discards frames arriving there.",
                    port.short(),
                    state.word()
                )
            });
            return;
        }
        if !self.port_security_admits(fx, phys, vlan, &frame) {
            return;
        }
        let learned = !frame.src.is_multicast() && self.macs.learn(vlan, frame.src, port, fx.now);
        if learned {
            fx.changed = true;
        }
        if state == PortState::Learning {
            fx.dropped(self.id, &frame, Some(port), Why::Blocked, || {
                format!(
                    "{} is still in the spanning-tree learning state for VLAN {vlan}: the switch records the source MAC but does not forward yet.",
                    port.short()
                )
            });
            return;
        }
        self.forwarded += 1;
        frame.tag = None;

        // The switch's own layer-3 interface for the VLAN, if any.
        let svi = IfId::vlan(vlan);
        if self.is_up(svi) && self.cfg.iface(svi).is_some() {
            let to_cpu = frame.dst == self.mac_for(svi) || self.hsrp_owns_mac(svi, frame.dst);
            if to_cpu || frame.dst.is_multicast() {
                let mut copy = frame.clone();
                if to_cpu {
                    copy.meta.via = fx.note(self.id, PacketEventKind::Forwarded, &copy, Some(svi), None, || {
                        format!(
                            "The destination MAC {} belongs to the switch's own interface Vlan{vlan}, so the frame is handed to its routing engine.",
                            copy.dst
                        )
                    });
                }
                self.l3_rx(fx, svi, copy);
            }
            if to_cpu {
                return;
            }
        }

        let known = (!frame.dst.is_multicast()).then(|| self.macs.lookup(vlan, frame.dst, fx.now)).flatten();
        let out = match known {
            Some(p) if p == port => {
                fx.dropped(self.id, &frame, Some(port), Why::Ignored, || {
                    format!(
                        "The MAC table says {} lives on {}, the port the frame came in on, so there is nothing to forward.",
                        frame.dst,
                        port.short()
                    )
                });
                return;
            }
            Some(p) if self.stp.state(vlan, p) == PortState::Forwarding && self.port_carries(p, vlan) => vec![p],
            Some(_) => Vec::new(),
            None => self.flood_ports(vlan, Some(port)),
        };
        let src = frame.src;
        frame.meta.via = fx.note(self.id, PacketEventKind::Forwarded, &frame, None, None, || {
            let learn = if learned {
                format!("The switch learns that {src} is reachable through {} in VLAN {vlan}. ", port.short())
            } else {
                String::new()
            };
            let action = if frame.dst.is_multicast() {
                format!(
                    "The destination {} is a {} address, so the frame is flooded out of every other port in VLAN {vlan} ({}).",
                    frame.dst,
                    if frame.dst.is_broadcast() { "broadcast" } else { "multicast" },
                    list_or_none(&out)
                )
            } else if known.is_some() {
                format!(
                    "It looks up {} in the MAC table for VLAN {vlan}: found on {}, so the frame is forwarded out of that port only.",
                    frame.dst,
                    list_or_none(&out)
                )
            } else {
                format!(
                    "It looks up {} in the MAC table for VLAN {vlan}: not found, so it floods the frame out of every other port in VLAN {vlan} ({}).",
                    frame.dst,
                    list_or_none(&out)
                )
            };
            format!("{learn}{action}{}", self.tag_remark(&out, vlan))
        });
        for p in out {
            self.egress(fx, p, vlan, frame.clone());
        }
    }

    /// A sentence about 802.1Q tagging when any egress port is a trunk.
    fn tag_remark(&self, out: &[IfId], vlan: u16) -> String {
        let tagged: Vec<IfId> = out
            .iter()
            .copied()
            .filter(|p| {
                self.is_trunk(*p)
                    && self.cfg.iface(*p).and_then(|c| c.l2.as_ref()).is_some_and(|sw| sw.native_vlan != vlan)
            })
            .collect();
        if tagged.is_empty() {
            String::new()
        } else {
            format!(" On trunk {} the frame carries an 802.1Q tag for VLAN {vlan}.", short_list(&tagged))
        }
    }

    /// Send a frame of `vlan` out of layer-2 port `port`, tagging as required.
    fn egress(&mut self, fx: &mut Fx, port: IfId, vlan: u16, mut frame: Frame) {
        let Some(sw) = self.cfg.iface(port).and_then(|cfg| cfg.l2.as_ref()) else { return };
        frame.tag = if self.is_trunk(port) {
            (sw.native_vlan != vlan).then_some(vlan)
        } else {
            (sw.voice_vlan == Some(vlan) && sw.access_vlan != vlan).then_some(vlan)
        };
        let members = self.members_of(port);
        if members.is_empty() {
            return;
        }
        // Per-flow load sharing across the members of a port-channel.
        let key = rng::hash(&[mac_bits(frame.src), mac_bits(frame.dst)]);
        let member = members[(key % members.len() as u64) as usize];
        fx.tx.push((member, frame));
    }

    /// A frame originated by the switch's own interface `VlanN`.
    pub(crate) fn switch_tx_from_cpu(&mut self, fx: &mut Fx, vlan: u16, frame: Frame) {
        let known = (!frame.dst.is_multicast()).then(|| self.macs.lookup(vlan, frame.dst, fx.now)).flatten();
        let out = match known {
            Some(p) if self.stp.state(vlan, p) == PortState::Forwarding && self.port_carries(p, vlan) => vec![p],
            Some(_) => Vec::new(),
            None => self.flood_ports(vlan, None),
        };
        for p in out {
            self.egress(fx, p, vlan, frame.clone());
        }
    }

    /// Send a link-local control frame (BPDU) out of a layer-2 port.
    pub(crate) fn control_tx(&mut self, fx: &mut Fx, port: IfId, frame: Frame) {
        for member in self.members_of(port).into_iter().take(1) {
            fx.tx.push((member, frame.clone()));
        }
    }

    /// Port security: may this source address use the port?
    fn port_security_admits(&mut self, fx: &mut Fx, phys: IfId, vlan: u16, frame: &Frame) -> bool {
        let Some(ps) = self.cfg.iface(phys).and_then(|c| c.l2.as_ref()).map(|sw| &sw.port_security) else {
            return true;
        };
        if !ps.enabled || frame.src.is_multicast() {
            return true;
        }
        let src = frame.src;
        let (maximum, violation, sticky) = (ps.maximum, ps.violation, ps.sticky);
        let configured = ps.macs.len();
        let known_static = ps.macs.iter().any(|(m, _)| *m == src);
        let Some(state) = self.ifs.get_mut(&phys) else { return true };
        if known_static || state.secure_macs.contains(&(src, vlan)) {
            state.last_secure_src = Some((src, vlan));
            return true;
        }
        if configured + state.secure_macs.len() < maximum as usize {
            state.last_secure_src = Some((src, vlan));
            if sticky {
                if let Some(sw) = self.cfg.interfaces.get_mut(&phys).and_then(|c| c.l2.as_mut()) {
                    sw.port_security.macs.push((src, true));
                }
            } else {
                state.secure_macs.push((src, vlan));
            }
            self.macs.set_static(vlan, src, phys, fx.now);
            fx.changed = true;
            return true;
        }
        state.last_secure_src = Some((src, vlan));
        if violation != Violation::Protect {
            state.security_violations += 1;
        }
        let name = phys.name();
        match violation {
            Violation::Shutdown => {
                state.errdisabled = Some("psecure-violation");
                fx.dirty = true;
                self.log(
                    fx,
                    4,
                    "PM",
                    "ERR_DISABLE",
                    format!(
                        "psecure-violation error detected on {}, putting {} in err-disable state",
                        phys.short(),
                        phys.short()
                    ),
                );
                self.log(
                    fx,
                    2,
                    "PORT_SECURITY",
                    "PSECURE_VIOLATION",
                    format!("Security violation occurred, caused by MAC address {src} on port {name}."),
                );
            }
            Violation::Restrict => self.log(
                fx,
                2,
                "PORT_SECURITY",
                "PSECURE_VIOLATION",
                format!("Security violation occurred, caused by MAC address {src} on port {name}."),
            ),
            Violation::Protect => {}
        }
        fx.dropped(self.id, frame, Some(phys), Why::Fatal, || {
            format!(
                "Port security on {} allows {maximum} secure MAC address(es) and {src} is not one of them: the frame is dropped{}.",
                phys.short(),
                match violation {
                    Violation::Shutdown => " and the port is err-disabled",
                    Violation::Restrict => " and the violation is logged",
                    Violation::Protect => " silently",
                }
            )
        });
        false
    }
}

fn list_or_none(ports: &[IfId]) -> String {
    if ports.is_empty() { "no port qualifies".to_string() } else { short_list(ports) }
}

fn mac_bits(mac: Mac) -> u64 {
    mac.0.iter().fold(0, |acc, b| acc << 8 | u64::from(*b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ifname::IfId;

    #[test]
    fn mac_table_learns_moves_and_ages() {
        let (a, b) = (IfId::parse("fa0/1").unwrap(), IfId::parse("fa0/2").unwrap());
        let mac = Mac([0, 1, 2, 3, 4, 5]);
        let mut table = MacTable::default();
        assert!(table.learn(10, mac, a, 0));
        assert!(!table.learn(10, mac, a, 1_000_000));
        assert_eq!(table.lookup(10, mac, 2_000_000), Some(a));
        assert_eq!(table.lookup(20, mac, 2_000_000), None);
        assert!(table.learn(10, mac, b, 3_000_000));
        assert_eq!(table.lookup(10, mac, 3_000_000), Some(b));
        assert_eq!(table.lookup(10, mac, 3_000_000 + MAC_AGING_MICROS), None);
        assert!(table.rows(3_000_000 + MAC_AGING_MICROS).is_empty());
        table.set_static(10, mac, a, 0);
        assert_eq!(table.lookup(10, mac, u64::MAX), Some(a));
        table.clear_dynamic();
        assert_eq!(table.rows(0).len(), 1);
        table.flush_port(a);
        assert!(table.rows(0).is_empty());
    }
}
