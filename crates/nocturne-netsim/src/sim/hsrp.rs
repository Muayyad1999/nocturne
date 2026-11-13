//! HSRP: routers on a subnet share a virtual gateway address.
//!
//! The state machine follows RFC 2281: a group listens, speaks, and then one
//! router becomes active (it owns the virtual IP and MAC address) and another
//! standby. Hellos every three seconds keep the roles alive; when the active
//! router has been silent for the hold time the standby takes over.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::SimTime;
use crate::ifname::IfId;
use crate::sim::frame::{Body, HsrpMsg, L4, Meta, PORT_HSRP, Packet, UdpApp, multicast_mac};
use crate::sim::fx::{Fx, Timer};
use crate::sim::node::Node;
use crate::util::mac::Mac;

pub(crate) const HELLO_S: u64 = 3;
pub(crate) const HOLD_S: u64 = 10;
const SECOND: u64 = 1_000_000;
/// Length of the listen and speak phases when timers are shortened for teaching.
const QUICK_PHASE: u64 = 3 * SECOND;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HsrpState {
    Listen,
    Speak,
    Standby,
    Active,
}

impl HsrpState {
    pub(crate) fn word(self) -> &'static str {
        match self {
            HsrpState::Listen => "Listen",
            HsrpState::Speak => "Speak",
            HsrpState::Standby => "Standby",
            HsrpState::Active => "Active",
        }
    }
}

/// Another router of the group, as last heard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Peer {
    pub(crate) ip: Ipv4Addr,
    pub(crate) priority: u8,
    pub(crate) expires: SimTime,
}

#[derive(Clone, Debug)]
pub(crate) struct Group {
    pub(crate) state: HsrpState,
    pub(crate) active: Option<Peer>,
    pub(crate) standby: Option<Peer>,
    /// When the current listen or speak phase ends.
    phase_ends: SimTime,
    pub(crate) next_hello: SimTime,
    pub(crate) changes: u32,
    pub(crate) last_change: SimTime,
    /// The settings last acted on, kept so a removed group can still resign.
    settings: Option<Settings>,
    token: u32,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Hsrp {
    pub(crate) groups: BTreeMap<(IfId, u16), Group>,
    next_token: u32,
}

/// The settings of one configured group that the protocol needs.
#[derive(Clone, Copy, Debug)]
struct Settings {
    vip: Ipv4Addr,
    priority: u8,
    preempt: bool,
    version: u8,
    own: Ipv4Addr,
}

/// `0000.0c07.acXX` for version 1, `0000.0c9f.fXXX` for version 2.
pub(crate) fn virtual_mac(version: u8, group: u16) -> Mac {
    if version == 2 {
        Mac([
            0x00,
            0x00,
            0x0c,
            0x9f,
            0xf0 | ((group >> 8) & 0x0f) as u8,
            group as u8,
        ])
    } else {
        Mac([0x00, 0x00, 0x0c, 0x07, 0xac, group as u8])
    }
}

impl Node {
    fn hsrp_settings(&self, iface: IfId, group: u16) -> Option<Settings> {
        let cfg = self.cfg.iface(iface)?;
        let entry = cfg.hsrp.iter().find(|g| g.group == group)?;
        let own = self.addr(iface)?.ip;
        Some(Settings {
            vip: entry.ip?,
            priority: entry.priority,
            preempt: entry.preempt,
            version: cfg.hsrp_version,
            own,
        })
    }

    fn hsrp_phase(&self, fx: &Fx) -> u64 {
        if fx.realistic {
            HOLD_S * SECOND
        } else {
            QUICK_PHASE
        }
    }

    /// Start, stop and re-evaluate groups after a configuration or interface change.
    pub(crate) fn hsrp_sync(&mut self, fx: &mut Fx) {
        let mut wanted: Vec<(IfId, u16)> = Vec::new();
        if self.routes_packets() {
            for (id, cfg) in &self.cfg.interfaces {
                // The virtual address has to be on the interface's own subnet.
                if let Some(addr) = self
                    .addr(*id)
                    .filter(|_| self.is_l3(*id) && self.is_up(*id))
                {
                    wanted.extend(
                        cfg.hsrp
                            .iter()
                            .filter(|g| g.ip.is_some_and(|vip| addr.on_link(vip) && vip != addr.ip))
                            .map(|g| (*id, g.group)),
                    );
                }
            }
        }
        let gone: Vec<(IfId, u16)> = self
            .hsrp
            .groups
            .keys()
            .filter(|key| !wanted.contains(key))
            .copied()
            .collect();
        for key in gone {
            let Some(group) = self.hsrp.groups.get(&key) else {
                continue;
            };
            let (state, settings) = (group.state, group.settings);
            if let (HsrpState::Active, Some(settings)) = (state, settings) {
                // Tell the standby router it can take over right away.
                self.hsrp_hello(fx, key, settings, true);
            }
            self.hsrp.groups.remove(&key);
            if state != HsrpState::Listen {
                self.log_hsrp(fx, key, state.word(), "Init");
            }
        }
        let phase = self.hsrp_phase(fx);
        for key in wanted {
            if let std::collections::btree_map::Entry::Vacant(slot) = self.hsrp.groups.entry(key) {
                slot.insert(Group {
                    state: HsrpState::Listen,
                    active: None,
                    standby: None,
                    phase_ends: fx.now + phase,
                    next_hello: fx.now,
                    changes: 0,
                    last_change: fx.now,
                    settings: None,
                    token: 0,
                });
                fx.cp_change = true;
            }
            // A raised priority or a new `preempt` may change who should be active.
            self.hsrp_step(fx, key);
        }
    }

    pub(crate) fn hsrp_timer(&mut self, fx: &mut Fx, iface: IfId, group: u16, token: u32) {
        if self
            .hsrp
            .groups
            .get(&(iface, group))
            .is_some_and(|g| g.token == token)
        {
            self.hsrp_step(fx, (iface, group));
        }
    }

    fn log_hsrp(&mut self, fx: &mut Fx, key: (IfId, u16), from: &str, to: &str) {
        fx.cp_change = true;
        fx.changed = true;
        self.log(
            fx,
            5,
            "HSRP",
            "STATECHANGE",
            format!("{} Grp {} state {from} -> {to}", key.0.name(), key.1),
        );
    }

    fn hsrp_set(&mut self, fx: &mut Fx, key: (IfId, u16), to: HsrpState) {
        let phase = self.hsrp_phase(fx);
        let Some(group) = self.hsrp.groups.get_mut(&key) else {
            return;
        };
        let from = group.state;
        if from == to {
            return;
        }
        group.state = to;
        group.changes += 1;
        group.last_change = fx.now;
        group.phase_ends = fx.now + phase;
        // Whoever changes role says so at once.
        group.next_hello = fx.now;
        self.log_hsrp(fx, key, from.word(), to.word());
    }

    /// Act on everything that is due for a group, then arm its next timer.
    fn hsrp_step(&mut self, fx: &mut Fx, key: (IfId, u16)) {
        let Some(settings) = self.hsrp_settings(key.0, key.1) else {
            return;
        };
        let now = fx.now;
        let rank = (settings.priority, settings.own);
        for _ in 0..4 {
            let Some(group) = self.hsrp.groups.get_mut(&key) else {
                return;
            };
            group.settings = Some(settings);
            if group.active.is_some_and(|p| p.expires <= now) {
                group.active = None;
            }
            if group.standby.is_some_and(|p| p.expires <= now) {
                group.standby = None;
            }
            let outranked_active = group.active.is_some_and(|p| (p.priority, p.ip) < rank);
            let next = match group.state {
                HsrpState::Active => HsrpState::Active,
                _ if settings.preempt && outranked_active => HsrpState::Active,
                HsrpState::Standby if group.active.is_none() => HsrpState::Active,
                HsrpState::Speak if now >= group.phase_ends && group.standby.is_none() => {
                    HsrpState::Standby
                }
                HsrpState::Listen
                    if now >= group.phase_ends
                        && (group.active.is_none() || group.standby.is_none()) =>
                {
                    HsrpState::Speak
                }
                state => state,
            };
            if next == group.state {
                break;
            }
            if next == HsrpState::Active {
                group.active = None;
            }
            self.hsrp_set(fx, key, next);
        }
        let Some(group) = self.hsrp.groups.get_mut(&key) else {
            return;
        };
        if group.state >= HsrpState::Speak && group.next_hello <= now {
            group.next_hello = now + HELLO_S * SECOND;
            self.hsrp_hello(fx, key, settings, false);
        }
        self.hsrp.next_token = self.hsrp.next_token.wrapping_add(1);
        let token = self.hsrp.next_token;
        let Some(group) = self.hsrp.groups.get_mut(&key) else {
            return;
        };
        group.token = token;
        let mut deadlines: Vec<SimTime> = group
            .active
            .iter()
            .chain(group.standby.iter())
            .map(|p| p.expires)
            .collect();
        if group.state >= HsrpState::Speak {
            deadlines.push(group.next_hello);
        }
        if matches!(group.state, HsrpState::Listen | HsrpState::Speak) && group.phase_ends > now {
            deadlines.push(group.phase_ends);
        }
        if let Some(at) = deadlines.into_iter().min() {
            fx.arm(
                at.saturating_sub(now).max(1),
                Timer::Hsrp {
                    iface: key.0,
                    group: key.1,
                    token,
                },
            );
        }
    }

    fn hsrp_hello(&mut self, fx: &mut Fx, key: (IfId, u16), settings: Settings, resign: bool) {
        let Some(group) = self.hsrp.groups.get(&key) else {
            return;
        };
        let (iface, number) = key;
        if !self.is_up(iface) {
            return;
        }
        let state = group.state;
        let msg = HsrpMsg {
            group: number,
            version: settings.version,
            active: state == HsrpState::Active,
            standby: state == HsrpState::Standby,
            resign,
            priority: settings.priority,
            vip: settings.vip,
        };
        let dst = if settings.version == 2 {
            Ipv4Addr::new(224, 0, 0, 102)
        } else {
            Ipv4Addr::new(224, 0, 0, 2)
        };
        let packet = Packet {
            src: settings.own,
            dst,
            ttl: 1,
            size: 48,
            l4: L4::Udp {
                sport: PORT_HSRP,
                dport: PORT_HSRP,
                app: UdpApp::Hsrp(msg),
            },
        };
        // The active router speaks from the virtual MAC address, which is how
        // the switches in between learn where the virtual gateway lives.
        let src_mac = if state == HsrpState::Active {
            virtual_mac(settings.version, number)
        } else {
            self.mac_for(iface)
        };
        let dst_mac = multicast_mac(dst);
        let mut meta = Meta {
            pdu: fx.new_pdu(),
            ..Meta::default()
        };
        let noun = self.noun();
        meta.via = self.ip_note(fx, crate::api::PacketEventKind::Created, &packet, &meta, (src_mac, dst_mac), Some(iface), None, || {
            format!(
                "The {noun} sends an HSRP hello for group {number} ({}, priority {}) so the other gateways for {} know it is alive.",
                state.word().to_lowercase(),
                settings.priority,
                settings.vip
            )
        });
        self.l3_tx(fx, iface, src_mac, dst_mac, Body::Ip(packet), meta);
    }

    pub(crate) fn hsrp_rx(&mut self, fx: &mut Fx, iface: IfId, src: Ipv4Addr, msg: HsrpMsg) {
        let key = (iface, msg.group);
        let Some(settings) = self.hsrp_settings(iface, msg.group) else {
            return;
        };
        if msg.version != settings.version || msg.vip != settings.vip || src == settings.own {
            return;
        }
        let now = fx.now;
        let rank = (settings.priority, settings.own);
        let theirs = (msg.priority, src);
        let peer = Peer {
            ip: src,
            priority: msg.priority,
            expires: now + HOLD_S * SECOND,
        };
        let Some(group) = self.hsrp.groups.get_mut(&key) else {
            return;
        };
        let state = group.state;
        let mut next = state;
        let mut answer = false;
        if msg.resign {
            if group.active.is_some_and(|p| p.ip == src) {
                group.active = None;
            }
        } else if msg.active {
            if group.standby.is_some_and(|p| p.ip == src) {
                group.standby = None;
            }
            if state == HsrpState::Active {
                if theirs > rank {
                    // Two active routers: the lower priority one stands down.
                    group.active = Some(peer);
                    next = HsrpState::Speak;
                } else {
                    answer = true;
                }
            } else {
                group.active = Some(peer);
            }
        } else if msg.standby {
            if group.active.is_some_and(|p| p.ip == src) {
                group.active = None;
            }
            match state {
                HsrpState::Speak | HsrpState::Standby if theirs > rank => {
                    group.standby = Some(peer);
                    next = HsrpState::Listen;
                }
                // A better candidate takes the standby role over.
                HsrpState::Speak => next = HsrpState::Standby,
                HsrpState::Standby => answer = true,
                _ => group.standby = Some(peer),
            }
        } else if theirs > rank && matches!(state, HsrpState::Speak | HsrpState::Standby) {
            // A higher priority router is speaking up: let it have the role.
            next = HsrpState::Listen;
        }
        if next != state {
            self.hsrp_set(fx, key, next);
        } else if answer && let Some(group) = self.hsrp.groups.get_mut(&key) {
            group.next_hello = now;
        }
        self.hsrp_step(fx, key);
    }

    /// Is `mac` the virtual address of a group this router is active for on `iface`?
    pub(crate) fn hsrp_owns_mac(&self, iface: IfId, mac: Mac) -> bool {
        self.hsrp.groups.iter().any(|((id, number), group)| {
            *id == iface
                && group.state == HsrpState::Active
                && self
                    .cfg
                    .iface(iface)
                    .is_some_and(|cfg| virtual_mac(cfg.hsrp_version, *number) == mac)
        })
    }

    fn hsrp_active_for(&self, ip: Ipv4Addr) -> Option<(IfId, u16)> {
        self.hsrp
            .groups
            .iter()
            .filter(|(_, group)| group.state == HsrpState::Active)
            .map(|(key, _)| *key)
            .find(|(iface, number)| {
                self.hsrp_settings(*iface, *number)
                    .is_some_and(|s| s.vip == ip)
            })
    }

    /// Is `ip` a virtual address this router currently answers for?
    pub(crate) fn hsrp_owns_ip(&self, ip: Ipv4Addr) -> bool {
        self.hsrp_active_for(ip).is_some()
    }

    /// The virtual MAC to answer ARP with, if `ip` is a virtual address we are active for on `iface`.
    pub(crate) fn hsrp_virtual_mac(&self, iface: IfId, ip: Ipv4Addr) -> Option<Mac> {
        let (on, number) = self.hsrp_active_for(ip)?;
        (on == iface).then(|| {
            virtual_mac(
                self.cfg.iface(iface).map_or(1, |cfg| cfg.hsrp_version),
                number,
            )
        })
    }

    /// Is a group still electing, or waiting for a silent peer to time out?
    pub(crate) fn hsrp_converging(&self, now: SimTime) -> bool {
        let grace = (HOLD_S - HELLO_S) * SECOND - 1_500_000;
        self.hsrp.groups.values().any(|group| {
            let electing = match group.state {
                HsrpState::Speak => true,
                HsrpState::Listen => group.active.is_none() || group.standby.is_none(),
                HsrpState::Standby => group.active.is_none(),
                HsrpState::Active => false,
            };
            let overdue = group
                .active
                .iter()
                .chain(group.standby.iter())
                .any(|p| p.expires.saturating_sub(now) < grace);
            electing || overdue
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_mac_addresses() {
        assert_eq!(virtual_mac(1, 1).dotted(), "0000.0c07.ac01");
        assert_eq!(virtual_mac(1, 10).dotted(), "0000.0c07.ac0a");
        assert_eq!(virtual_mac(2, 10).dotted(), "0000.0c9f.f00a");
        assert_eq!(virtual_mac(2, 0xabc).dotted(), "0000.0c9f.fabc");
    }
}
