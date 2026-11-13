//! Derived state: everything that follows from configuration, power and
//! cabling rather than from packets — carrier, negotiated speed/duplex/trunking,
//! EtherChannel bundling, line protocol, and the tables that depend on them.
//!
//! The world owns the cross-node part (a cable has two ends); each node then
//! recomputes its own view in [`Node::derive`]. Both are idempotent, so the
//! rule is simply: change anything, mark the node dirty.

use crate::api::{LabEvent, LinkId, NodeId};
use crate::device::config::{ChannelMode, Duplex, IfConfig, SwMode};
use crate::ifname::{IfId, IfKind};
use crate::sim::World;
use crate::sim::fx::Fx;
use crate::sim::node::{IfState, Node, PeerFacts};

/// What one end of a cable brings to the negotiation.
struct PortFacts {
    usable: bool,
    max_speed: u32,
    speed: Option<u32>,
    duplex: Duplex,
    sw_mode: Option<SwMode>,
    nonegotiate: bool,
    channel: Option<(u32, ChannelMode)>,
}

impl World {
    pub(crate) fn mark_dirty(&mut self, node: NodeId) {
        self.dirty.insert(node);
    }

    /// Recompute derived state for every dirty node until nothing changes.
    pub(crate) fn reconcile(&mut self) {
        let mut budget = 16 * self.nodes.len() + 64;
        loop {
            while let Some(id) = self.dirty.pop_first() {
                if budget == 0 {
                    self.dirty.clear();
                    break;
                }
                budget -= 1;
                self.reconcile_node(id);
            }
            if !self.ospf_dirty {
                break;
            }
            self.ospf_dirty = false;
            crate::sim::ospf::recompute(self);
            if self.dirty.is_empty() || budget == 0 {
                break;
            }
        }
    }

    fn reconcile_node(&mut self, id: NodeId) {
        let Some(node) = self.nodes.get(&id) else { return };
        let links: Vec<LinkId> = node.ifs.values().filter_map(|state| state.link).collect();
        for link in links {
            self.refresh_link(link);
        }
        self.with_node(id, |node, fx| node.derive(fx));
        self.changed.insert(id);
    }

    fn port_facts(&self, end: (NodeId, IfId)) -> Option<PortFacts> {
        let node = self.nodes.get(&end.0)?;
        let cfg = node.cfg.iface(end.1)?;
        let state = node.ifs.get(&end.1)?;
        // A channel member negotiates trunking with its port-channel's settings.
        let l2 = match cfg.channel_group {
            Some((group, _)) => {
                node.cfg.iface(IfId::port_channel(group)).and_then(|po| po.l2.as_ref()).or(cfg.l2.as_ref())
            }
            None => cfg.l2.as_ref(),
        };
        Some(PortFacts {
            usable: node.powered && !cfg.shutdown && state.errdisabled.is_none(),
            max_speed: end.1.kind.max_mbps(),
            speed: cfg.speed,
            duplex: cfg.duplex,
            sw_mode: l2.map(|sw| sw.mode),
            nonegotiate: l2.is_some_and(|sw| sw.nonegotiate),
            channel: cfg.channel_group,
        })
    }

    /// Recompute carrier and negotiation results for both ends of a cable.
    fn refresh_link(&mut self, id: LinkId) {
        let Some(link) = self.links.get(&id) else { return };
        let (a, b) = (link.a, link.b);
        let faulted = link.fault_down || link.flap_down;
        let duplex_fault = link.duplex_fault;
        let was_up = link.up;
        let (Some(fa), Some(fb)) = (self.port_facts(a), self.port_facts(b)) else { return };

        let (speed, speed_ok) = match (fa.speed, fb.speed) {
            (None, None) => (fa.max_speed.min(fb.max_speed), true),
            (Some(s), None) => (s, s <= fb.max_speed),
            (None, Some(s)) => (s, s <= fa.max_speed),
            (Some(s), Some(t)) => (s, s == t),
        };
        let up = !faulted && fa.usable && fb.usable && speed_ok;
        // Autonegotiation against a port with fixed duplex cannot learn the
        // duplex and falls back to half: the classic mismatch.
        let (full_a, full_b) = match (fa.duplex, fb.duplex) {
            _ if duplex_fault => (true, false),
            (Duplex::Auto, Duplex::Auto) | (Duplex::Full, Duplex::Full) => (true, true),
            (Duplex::Full, _) => (true, false),
            (_, Duplex::Full) => (false, true),
            _ => (false, false),
        };

        if let Some(link) = self.links.get_mut(&id) {
            link.up = up;
        }
        for (end, other_end, other, full) in [(a, b, &fb, full_a), (b, a, &fa, full_b)] {
            let peer = PeerFacts {
                node: other_end.0,
                port: other_end.1,
                sw_mode: other.sw_mode,
                nonegotiate: other.nonegotiate,
                channel: other.channel,
            };
            let Some(state) = self.nodes.get_mut(&end.0).and_then(|n| n.ifs.get_mut(&end.1)) else { continue };
            let unchanged = state.carrier == up
                && state.speed == speed
                && state.full_duplex == full
                && state.peer.as_ref() == Some(&peer);
            if !unchanged {
                state.carrier = up;
                state.speed = speed;
                state.full_duplex = full;
                state.peer = Some(peer);
                self.dirty.insert(end.0);
            }
        }
        if was_up != up {
            self.events.push(LabEvent::LinkState { link: id, up });
            self.touch();
        }
    }
}

impl Node {
    /// Recompute this node's derived state from its configuration and the facts
    /// the world last recorded about its cables.
    pub(crate) fn derive(&mut self, fx: &mut Fx) {
        if !self.powered {
            return;
        }
        self.sync_interfaces();
        self.derive_channels();
        self.derive_trunking();
        let came_up = self.derive_line_state(fx);
        self.stp_sync(fx);
        self.prune_tables();
        self.dhcp_client_sync(fx);
        self.rebuild_rib(fx);
        self.neighbors_sync(fx, &came_up);
        self.dhcp_server_sync();
        self.nat_sync();
        self.ospf_sync(fx);
        self.rip_sync(fx);
        self.hsrp_sync(fx);
    }

    /// Make the runtime interface table match the configured interfaces.
    fn sync_interfaces(&mut self) {
        let configured: Vec<IfId> = self.cfg.interfaces.keys().copied().collect();
        self.ifs.retain(|id, _| id.is_physical() || configured.contains(id));
        for id in configured {
            if !self.ifs.contains_key(&id) {
                let mut state = IfState::new(self.mac_for(id), 0);
                state.was_shutdown = self.cfg.iface(id).is_some_and(|c| c.shutdown);
                self.ifs.insert(id, state);
            }
        }
        // A port-channel takes its address from its first member, which can change.
        let channels: Vec<IfId> = self.ifs.keys().filter(|id| id.kind == IfKind::PortChannel).copied().collect();
        for id in channels {
            let mac = self.mac_for(id);
            if let Some(state) = self.ifs.get_mut(&id) {
                state.mac = mac;
            }
        }
    }

    /// Decide which channel-group members are bundled.
    fn derive_channels(&mut self) {
        let groups: std::collections::BTreeSet<u32> =
            self.cfg.interfaces.values().filter_map(|cfg| cfg.channel_group.map(|(g, _)| g)).collect();
        for group in groups {
            let mut partner: Option<(NodeId, u32)> = None;
            let mut speed: Option<u32> = None;
            for member in self.cfg.channel_members(group) {
                let Some((_, mode)) = self.cfg.iface(member).and_then(|c| c.channel_group) else { continue };
                let Some(state) = self.ifs.get(&member) else { continue };
                let mut ok = state.carrier;
                if ok && mode != ChannelMode::On {
                    // A negotiating member needs a compatible far end, and every
                    // member must reach the same partner channel.
                    match state.peer.as_ref().and_then(|p| p.channel.map(|(g, m)| (p.node, g, m))) {
                        Some((node, peer_group, peer_mode)) if mode.compatible(peer_mode) => {
                            ok = *partner.get_or_insert((node, peer_group)) == (node, peer_group);
                        }
                        _ => ok = false,
                    }
                }
                if ok {
                    ok = *speed.get_or_insert(state.speed) == state.speed;
                }
                if let Some(state) = self.ifs.get_mut(&member) {
                    state.bundled = ok;
                }
            }
        }
        for (id, state) in &mut self.ifs {
            if self.cfg.interfaces.get(id).is_none_or(|cfg| cfg.channel_group.is_none()) {
                state.bundled = false;
            }
        }
    }

    /// Operational access/trunk mode of every switch port (a simple DTP outcome).
    fn derive_trunking(&mut self) {
        let ports: Vec<IfId> =
            self.cfg.interfaces.iter().filter(|(_, cfg)| cfg.l2.is_some()).map(|(id, _)| *id).collect();
        for port in ports {
            let Some(sw) = self.cfg.iface(port).and_then(|cfg| cfg.l2.as_ref()) else { continue };
            // A port-channel negotiates through its first bundled member.
            let via = if port.kind == IfKind::PortChannel {
                self.cfg.channel_members(port.number()).into_iter().find(|m| self.ifs.get(m).is_some_and(|s| s.bundled))
            } else {
                Some(port)
            };
            let state = via.and_then(|v| self.ifs.get(&v));
            let carrier = state.is_some_and(|s| s.carrier);
            let peer = state.and_then(|s| s.peer.as_ref());
            let trunking = match sw.mode {
                SwMode::Trunk => true,
                SwMode::Access => false,
                dynamic => {
                    carrier
                        && peer.is_some_and(|p| {
                            !p.nonegotiate
                                && matches!(
                                    (dynamic, p.sw_mode),
                                    (SwMode::DynamicDesirable, Some(_))
                                        | (SwMode::DynamicAuto, Some(SwMode::Trunk | SwMode::DynamicDesirable))
                                )
                                && p.sw_mode != Some(SwMode::Access)
                        })
                }
            };
            if let Some(state) = self.ifs.get_mut(&port) {
                state.trunking = trunking;
            }
        }
    }

    /// Line protocol of every interface, with the syslog messages a real device
    /// prints on each transition. Returns the interfaces that just came up.
    fn derive_line_state(&mut self, fx: &mut Fx) -> Vec<IfId> {
        let mut came_up = Vec::new();
        let order: Vec<IfId> = {
            // Physical ports first, then port-channels, then everything layered on them.
            let mut ids: Vec<IfId> = self.cfg.interfaces.keys().copied().collect();
            ids.sort_by_key(|id| match (id.is_physical(), id.kind) {
                (true, _) => 0,
                (_, IfKind::PortChannel) => 1,
                _ => 2,
            });
            ids
        };
        for id in order {
            let Some(cfg) = self.cfg.iface(id) else { continue };
            let shutdown = cfg.shutdown;
            let carrier = self.carrier_of(id, cfg);
            let Some(state) = self.ifs.get_mut(&id) else { continue };
            if !id.is_physical() {
                state.carrier = carrier;
            }
            let up = state.carrier && !shutdown && state.errdisabled.is_none();
            let (was_up, was_shutdown) = (state.up, state.was_shutdown);
            state.up = up;
            state.was_shutdown = shutdown;
            if up == was_up && shutdown == was_shutdown {
                continue;
            }
            state.last_change = fx.now;
            if !up {
                state.dhcp.release();
            }
            fx.cp_change = true;
            fx.changed = true;
            let name = id.name();
            if shutdown && !was_shutdown {
                self.log(fx, 5, "LINK", "CHANGED", format!("Interface {name}, changed state to administratively down"));
            } else if up != was_up || !shutdown && was_shutdown {
                let word = if up { "up" } else { "down" };
                self.log(fx, 3, "LINK", "UPDOWN", format!("Interface {name}, changed state to {word}"));
            }
            if up != was_up {
                let word = if up { "up" } else { "down" };
                self.log(
                    fx,
                    5,
                    "LINEPROTO",
                    "UPDOWN",
                    format!("Line protocol on Interface {name}, changed state to {word}"),
                );
                if up {
                    came_up.push(id);
                }
            }
        }
        came_up
    }

    /// Whether the resource under a logical interface is present.
    fn carrier_of(&self, id: IfId, cfg: &IfConfig) -> bool {
        match id.kind {
            _ if id.is_sub() => self.is_up(id.parent()) && cfg.dot1q.is_some(),
            IfKind::Loopback => true,
            IfKind::PortChannel => {
                self.cfg.channel_members(id.number()).iter().any(|m| self.ifs.get(m).is_some_and(|s| s.bundled))
            }
            // An SVI is up while its VLAN exists and some up port carries it.
            IfKind::Vlan => {
                let vlan = id.number() as u16;
                self.cfg.vlan_active(vlan) && self.l2_ports().iter().any(|port| self.port_carries(*port, vlan))
            }
            _ => self.ifs.get(&id).is_some_and(|s| s.carrier),
        }
    }

    /// Forget dynamic state that referred to interfaces which are now down.
    fn prune_tables(&mut self) {
        let l2_ports = self.l2_ports();
        self.macs.retain_ports(&l2_ports);
        let live: Vec<IfId> = self.ifs.iter().filter(|(_, s)| s.up).map(|(id, _)| *id).collect();
        self.arp.retain_ifaces(&live);
        for (id, state) in &mut self.ifs {
            if !state.up && !self.cfg.interfaces.get(id).is_some_and(|c| c.l2.as_ref().is_some_and(|sw| sw.port_security.sticky))
            {
                state.secure_macs.clear();
            }
        }
    }
}
