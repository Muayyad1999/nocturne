//! Per-VLAN spanning tree.
//!
//! Every bridge sends configuration BPDUs on its designated ports each hello
//! time and whenever its view changes; ports keep the best vector heard and the
//! usual comparison yields root, designated and alternate roles. Ports moving
//! to forwarding pass through listening and learning. Timers are short unless
//! the lab opted into realistic ones.

use std::collections::BTreeMap;

use crate::api::SimTime;
use crate::device::config::StpMode;
use crate::ifname::{IfId, IfKind};
use crate::sim::frame::{Body, Bpdu, BridgeId, Frame, MAC_PVST, Meta};
use crate::sim::fx::{Fx, Timer};
use crate::sim::node::Node;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortState {
    Blocking,
    Listening,
    Learning,
    Forwarding,
}

impl PortState {
    /// `FWD`, `BLK`, `LIS`, `LRN` as in `show spanning-tree`.
    pub(crate) fn code(self) -> &'static str {
        match self {
            PortState::Blocking => "BLK",
            PortState::Listening => "LIS",
            PortState::Learning => "LRN",
            PortState::Forwarding => "FWD",
        }
    }

    pub(crate) fn word(self) -> &'static str {
        match self {
            PortState::Blocking => "blocking",
            PortState::Listening => "listening",
            PortState::Learning => "learning",
            PortState::Forwarding => "forwarding",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortRole {
    Root,
    Designated,
    Alternate,
    Backup,
}

impl PortRole {
    pub(crate) fn code(self) -> &'static str {
        match self {
            PortRole::Root => "Root",
            PortRole::Designated => "Desg",
            PortRole::Alternate => "Altn",
            PortRole::Backup => "Back",
        }
    }
}

/// A spanning-tree priority vector; smaller is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Vector {
    pub(crate) root: BridgeId,
    pub(crate) cost: u32,
    pub(crate) bridge: BridgeId,
    pub(crate) port: u16,
}

#[derive(Clone, Debug)]
pub(crate) struct StpPort {
    pub(crate) state: PortState,
    pub(crate) role: PortRole,
    /// Best information received on the port, and when it expires.
    pub(crate) heard: Option<(Vector, SimTime)>,
    /// PortFast in effect (lost when a BPDU arrives).
    pub(crate) edge: bool,
    /// Send the topology-change flag until this time.
    pub(crate) tc_until: SimTime,
    token: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct StpVlan {
    pub(crate) root: BridgeId,
    pub(crate) root_cost: u32,
    pub(crate) root_port: Option<IfId>,
    pub(crate) ports: BTreeMap<IfId, StpPort>,
    pub(crate) topology_changes: u32,
    pub(crate) last_change: Option<SimTime>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Stp {
    pub(crate) vlans: BTreeMap<u16, StpVlan>,
    hello_armed: bool,
    next_token: u32,
}

/// Hello, forward delay (per phase) and max age, in microseconds.
pub(crate) struct StpTimers {
    pub(crate) hello: u64,
    pub(crate) forward_delay: u64,
    pub(crate) max_age: u64,
}

impl Stp {
    /// State of a port in a VLAN. Ports spanning tree does not know are not
    /// forwarding: a switch never forwards on a port it has not vetted.
    pub(crate) fn state(&self, vlan: u16, port: IfId) -> PortState {
        self.vlans.get(&vlan).and_then(|v| v.ports.get(&port)).map_or(PortState::Blocking, |p| p.state)
    }

    /// Is the port forwarding in every VLAN it carries?
    pub(crate) fn all_forwarding(&self, port: IfId) -> bool {
        self.vlans.values().filter_map(|v| v.ports.get(&port)).all(|p| p.state == PortState::Forwarding)
    }

    /// Any port still listening or learning (used to judge quiescence).
    pub(crate) fn converging(&self) -> bool {
        self.vlans
            .values()
            .flat_map(|v| v.ports.values())
            .any(|p| matches!(p.state, PortState::Listening | PortState::Learning))
    }

    pub(crate) fn reset(&mut self) {
        *self = Stp::default();
    }
}

impl Node {
    pub(crate) fn stp_timers(&self, fx: &Fx) -> StpTimers {
        let rapid = self.cfg.stp.mode == StpMode::RapidPvst;
        let (forward_delay, max_age) = match (fx.realistic, rapid) {
            // Rapid spanning tree converges by handshake; model that as a short delay.
            (_, true) => (100_000, 6_000_000),
            (true, false) => (15_000_000, 20_000_000),
            (false, false) => (1_000_000, 6_000_000),
        };
        StpTimers { hello: 2_000_000, forward_delay, max_age }
    }

    pub(crate) fn bridge_id(&self, vlan: u16) -> BridgeId {
        BridgeId { priority: self.cfg.stp.priority_of(vlan).saturating_add(vlan), mac: self.base_mac }
    }

    /// Port identifier: priority in the high byte, port number in the low byte.
    pub(crate) fn stp_port_id(&self, port: IfId) -> u16 {
        let number = if port.kind == IfKind::PortChannel {
            200 + port.number().min(55)
        } else {
            self.model.ports.iter().position(|p| *p == port).map_or(0, |i| i as u32 + 1)
        };
        let priority = self.cfg.iface(port).and_then(|c| c.stp.port_priority).unwrap_or(128);
        u16::from(priority) << 8 | (number as u16 & 0xff)
    }

    pub(crate) fn stp_path_cost(&self, port: IfId) -> u32 {
        if let Some(cost) = self.cfg.iface(port).and_then(|c| c.stp.cost) {
            return cost;
        }
        let speed: u32 = if port.kind == IfKind::PortChannel {
            self.members_of(port).iter().filter_map(|m| self.ifs.get(m)).map(|s| s.speed).sum()
        } else {
            self.ifs.get(&port).map_or(0, |s| s.speed)
        };
        match speed {
            0..=10 => 100,
            11..=100 => 19,
            101..=199 => 15,
            200..=999 => 12,
            1000..=1999 => 4,
            2000..=9999 => 3,
            10_000..=19_999 => 2,
            _ => 1,
        }
    }

    /// Is PortFast in effect on the port by configuration?
    pub(crate) fn stp_portfast(&self, port: IfId) -> bool {
        let Some(cfg) = self.cfg.iface(port) else { return false };
        cfg.stp.portfast.unwrap_or(self.cfg.stp.portfast_default && !self.is_trunk(port))
    }

    fn stp_bpduguard(&self, port: IfId) -> bool {
        let Some(cfg) = self.cfg.iface(port) else { return false };
        cfg.stp.bpduguard.unwrap_or(self.cfg.stp.bpduguard_default && self.stp_portfast(port))
    }

    /// Align the per-VLAN port sets with the up layer-2 ports, then recompute.
    pub(crate) fn stp_sync(&mut self, fx: &mut Fx) {
        if !self.model.is_switch() {
            return;
        }
        let ports = self.l2_ports();
        let vlans: Vec<u16> = self.cfg.vlans.iter().filter(|(_, v)| v.active).map(|(id, _)| *id).collect();
        self.stp.vlans.retain(|id, _| vlans.contains(id));
        let timers = self.stp_timers(fx);
        for vlan in vlans {
            let own = self.bridge_id(vlan);
            let wanted: Vec<IfId> = ports.iter().copied().filter(|p| self.port_carries(*p, vlan)).collect();
            let edges: Vec<bool> = wanted.iter().map(|p| self.stp_portfast(*p)).collect();
            let stp = &mut self.stp;
            let instance = stp.vlans.entry(vlan).or_insert_with(|| StpVlan {
                root: own,
                root_cost: 0,
                root_port: None,
                ports: BTreeMap::new(),
                topology_changes: 0,
                last_change: None,
            });
            // Losing a forwarding port changes the active topology.
            let lost_forwarding = instance
                .ports
                .iter()
                .any(|(id, p)| !wanted.contains(id) && p.state == PortState::Forwarding && !p.edge);
            let before = instance.ports.len();
            instance.ports.retain(|id, _| wanted.contains(id));
            let mut ports_changed = instance.ports.len() != before;
            for (port, edge) in wanted.iter().zip(edges) {
                match instance.ports.get_mut(port) {
                    Some(existing) => {
                        // Turning PortFast on takes effect at once; turning it off
                        // only matters the next time the port comes up.
                        if edge && !existing.edge && existing.heard.is_none() {
                            existing.edge = true;
                            existing.state = PortState::Forwarding;
                        }
                    }
                    None => {
                        stp.next_token += 1;
                        let token = stp.next_token;
                        let state = if edge { PortState::Forwarding } else { PortState::Listening };
                        instance.ports.insert(
                            *port,
                            StpPort { state, role: PortRole::Designated, heard: None, edge, tc_until: 0, token },
                        );
                        if !edge {
                            fx.arm(timers.forward_delay, Timer::StpForward { vlan, port: *port, token });
                        }
                        ports_changed = true;
                    }
                }
            }
            if ports_changed {
                fx.cp_change = true;
            }
            self.stp_recompute(fx, vlan, ports_changed);
            if lost_forwarding {
                self.stp_topology_change(fx, vlan, None);
            }
        }
        if !self.stp.hello_armed {
            self.stp.hello_armed = true;
            fx.arm(timers.hello, Timer::StpHello);
        }
    }

    /// Recompute root, roles and states for one VLAN; send BPDUs if `announce`
    /// or if this bridge's view changed.
    fn stp_recompute(&mut self, fx: &mut Fx, vlan: u16, announce: bool) {
        let own = self.bridge_id(vlan);
        let timers = self.stp_timers(fx);
        let Some(instance) = self.stp.vlans.get(&vlan) else { return };
        let ports: Vec<IfId> = instance.ports.keys().copied().collect();
        let costs: Vec<u32> = ports.iter().map(|p| self.stp_path_cost(*p)).collect();
        let ids: Vec<u16> = ports.iter().map(|p| self.stp_port_id(*p)).collect();

        // Best path to a root learned from a neighbour, if any beats being root ourselves.
        let mut best: Option<(Vector, u16, IfId)> = None;
        for ((port, cost), id) in ports.iter().zip(&costs).zip(&ids) {
            let Some((heard, _)) = instance.ports.get(port).and_then(|p| p.heard) else { continue };
            if heard.bridge == own {
                continue;
            }
            let candidate = (Vector { cost: heard.cost.saturating_add(*cost), ..heard }, *id, *port);
            if best.as_ref().is_none_or(|b| (candidate.0, candidate.1) < (b.0, b.1)) {
                best = Some(candidate);
            }
        }
        let (root, root_cost, root_port) = match best {
            Some((vector, _, port)) if vector.root < own => (vector.root, vector.cost, Some(port)),
            _ => (own, 0, None),
        };

        let mut topology_change = false;
        let mut changed = false;
        let mut arm = Vec::new();
        let stp = &mut self.stp;
        let Some(instance) = stp.vlans.get_mut(&vlan) else { return };
        if (instance.root, instance.root_cost, instance.root_port) != (root, root_cost, root_port) {
            instance.root = root;
            instance.root_cost = root_cost;
            instance.root_port = root_port;
            changed = true;
        }
        for (port, id) in ports.iter().zip(&ids) {
            let Some(p) = instance.ports.get_mut(port) else { continue };
            let ours = Vector { root, cost: root_cost, bridge: own, port: *id };
            let role = if Some(*port) == root_port {
                PortRole::Root
            } else {
                match p.heard {
                    Some((heard, _)) if heard < ours => {
                        if heard.bridge == own { PortRole::Backup } else { PortRole::Alternate }
                    }
                    _ => PortRole::Designated,
                }
            };
            if role != p.role {
                p.role = role;
                changed = true;
            }
            match role {
                PortRole::Root | PortRole::Designated => {
                    if p.state == PortState::Blocking {
                        stp.next_token += 1;
                        p.token = stp.next_token;
                        p.state = PortState::Listening;
                        arm.push((timers.forward_delay, Timer::StpForward { vlan, port: *port, token: p.token }));
                        changed = true;
                    }
                }
                PortRole::Alternate | PortRole::Backup => {
                    if p.state != PortState::Blocking {
                        topology_change |= p.state == PortState::Forwarding;
                        p.state = PortState::Blocking;
                        changed = true;
                    }
                }
            }
        }
        for (delay, timer) in arm {
            fx.arm(delay, timer);
        }
        if changed {
            fx.cp_change = true;
            fx.changed = true;
        }
        if topology_change {
            self.stp_topology_change(fx, vlan, None);
        }
        if changed || announce {
            self.stp_send(fx, vlan, None);
        }
    }

    /// Transmit this bridge's BPDU on the designated ports of a VLAN (or one port).
    fn stp_send(&mut self, fx: &mut Fx, vlan: u16, only: Option<IfId>) {
        let own = self.bridge_id(vlan);
        let Some(instance) = self.stp.vlans.get(&vlan) else { return };
        let targets: Vec<(IfId, bool)> = instance
            .ports
            .iter()
            .filter(|(id, p)| p.role == PortRole::Designated && only.is_none_or(|o| o == **id))
            .map(|(id, p)| (*id, p.tc_until > fx.now))
            .collect();
        let (root, cost) = (instance.root, instance.root_cost);
        for (port, topology_change) in targets {
            let Some(sw) = self.cfg.iface(port).and_then(|c| c.l2.as_ref()) else { continue };
            let tag = (self.is_trunk(port) && sw.native_vlan != vlan).then_some(vlan);
            let bpdu = Bpdu { vlan, root, cost, bridge: own, port: self.stp_port_id(port), topology_change };
            let mut frame = Frame {
                src: self.mac_for(port),
                dst: MAC_PVST,
                tag,
                body: Body::Bpdu(bpdu),
                meta: Meta { pdu: fx.new_pdu(), ..Meta::default() },
            };
            frame.meta.via = fx.note(self.id, crate::api::PacketEventKind::Created, &frame, Some(port), None, || {
                format!(
                    "The switch is the designated bridge on {} for VLAN {vlan}, so it advertises the root {root} at cost {cost}.",
                    port.short()
                )
            });
            self.control_tx(fx, port, frame);
        }
    }

    /// A BPDU arrived on layer-2 port `port` (physical member `phys`).
    pub(crate) fn stp_rx(&mut self, fx: &mut Fx, port: IfId, phys: IfId, bpdu: Bpdu, frame: &Frame) {
        let Some(sw) = self.cfg.iface(port).and_then(|c| c.l2.as_ref()) else { return };
        let vlan = if self.is_trunk(port) { frame.tag.unwrap_or(sw.native_vlan) } else { sw.access_vlan };
        if self.stp_bpduguard(port) {
            let short = phys.short();
            self.log(
                fx,
                2,
                "SPANTREE",
                "BLOCK_BPDUGUARD",
                format!("Received BPDU on port {short} with BPDU Guard enabled. Disabling port."),
            );
            self.log(
                fx,
                4,
                "PM",
                "ERR_DISABLE",
                format!("bpduguard error detected on {short}, putting {short} in err-disable state"),
            );
            if let Some(state) = self.ifs.get_mut(&phys) {
                state.errdisabled = Some("bpduguard");
            }
            fx.dirty = true;
            fx.dropped(self.id, frame, Some(port), crate::sim::fx::Why::Fatal, || {
                format!(
                    "BPDU guard is enabled on {short}: receiving a BPDU means a switch was plugged into an edge port, so the port is err-disabled."
                )
            });
            return;
        }
        let own = self.bridge_id(vlan);
        let own_port_id = self.stp_port_id(port);
        let timers = self.stp_timers(fx);
        let Some(instance) = self.stp.vlans.get_mut(&vlan) else { return };
        let (root, root_cost) = (instance.root, instance.root_cost);
        let Some(p) = instance.ports.get_mut(&port) else { return };
        let heard = Vector { root: bpdu.root, cost: bpdu.cost, bridge: bpdu.bridge, port: bpdu.port };
        // A port that hears a BPDU is not an edge port, whatever the configuration says.
        p.edge = false;
        p.heard = Some((heard, fx.now.saturating_add(timers.max_age)));
        let accepts_tc = matches!(p.role, PortRole::Root | PortRole::Designated) && p.state == PortState::Forwarding;
        let ours = Vector { root, cost: root_cost, bridge: own, port: own_port_id };
        fx.note(self.id, crate::api::PacketEventKind::Delivered, frame, Some(port), None, || {
            format!(
                "The switch compares the BPDU (root {}, cost {}) with its own information for VLAN {vlan} (root {root}, cost {root_cost}): the neighbour's is {}.",
                bpdu.root,
                bpdu.cost,
                if heard < ours { "better, so this port leads towards the root or must block" } else { "worse, so this port stays designated" }
            )
        });
        if bpdu.topology_change && accepts_tc {
            self.stp_topology_change(fx, vlan, Some(port));
        }
        self.stp_recompute(fx, vlan, false);
        // Still designated after hearing a worse BPDU: answer so the neighbour learns.
        let still_designated =
            self.stp.vlans.get(&vlan).and_then(|v| v.ports.get(&port)).is_some_and(|p| p.role == PortRole::Designated);
        if still_designated && ours < heard {
            self.stp_send(fx, vlan, Some(port));
        }
    }

    /// The active topology of a VLAN changed: flush learned addresses and tell
    /// the neighbours (except the one that told us).
    fn stp_topology_change(&mut self, fx: &mut Fx, vlan: u16, from: Option<IfId>) {
        self.macs.flush_vlan(vlan, from);
        let hello = self.stp_timers(fx).hello;
        let Some(instance) = self.stp.vlans.get_mut(&vlan) else { return };
        if from.is_none() {
            instance.topology_changes += 1;
            instance.last_change = Some(fx.now);
        }
        let until = fx.now.saturating_add(2 * hello);
        let mut tell = Vec::new();
        for (id, p) in &mut instance.ports {
            if Some(*id) != from && !p.edge && p.tc_until <= fx.now {
                p.tc_until = until;
                tell.push(*id);
            }
        }
        fx.changed = true;
        for port in tell {
            self.stp_send(fx, vlan, Some(port));
        }
    }

    /// Periodic hello: age out stale information and re-advertise.
    pub(crate) fn stp_hello(&mut self, fx: &mut Fx) {
        let timers = self.stp_timers(fx);
        fx.arm(timers.hello, Timer::StpHello);
        let vlans: Vec<u16> = self.stp.vlans.keys().copied().collect();
        for vlan in vlans {
            let mut expired = false;
            if let Some(instance) = self.stp.vlans.get_mut(&vlan) {
                for p in instance.ports.values_mut() {
                    if p.heard.is_some_and(|(_, until)| until <= fx.now) {
                        p.heard = None;
                        expired = true;
                    }
                }
            }
            if expired {
                fx.cp_change = true;
            }
            self.stp_recompute(fx, vlan, true);
        }
    }

    /// Listening → learning → forwarding.
    pub(crate) fn stp_forward_timer(&mut self, fx: &mut Fx, vlan: u16, port: IfId, token: u32) {
        let delay = self.stp_timers(fx).forward_delay;
        let Some(p) = self.stp.vlans.get_mut(&vlan).and_then(|v| v.ports.get_mut(&port)) else { return };
        if p.token != token {
            return;
        }
        match p.state {
            PortState::Listening => {
                p.state = PortState::Learning;
                fx.arm(delay, Timer::StpForward { vlan, port, token });
                fx.changed = true;
            }
            PortState::Learning => {
                p.state = PortState::Forwarding;
                let edge = p.edge;
                fx.cp_change = true;
                fx.changed = true;
                if !edge {
                    self.stp_topology_change(fx, vlan, None);
                }
            }
            PortState::Blocking | PortState::Forwarding => {}
        }
    }
}
