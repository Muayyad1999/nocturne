//! CDP and LLDP: periodic link-local advertisements and the neighbour tables
//! built from them.

use crate::api::{NeighborRow, PacketEventKind, SimTime};
use crate::catalog::Role;
use crate::ifname::IfId;
use crate::sim::frame::{Advert, Body, Frame, MAC_CDP, MAC_LLDP, Meta};
use crate::sim::fx::{Fx, Timer, Why};
use crate::sim::node::Node;

pub(crate) const CDP_INTERVAL_S: u32 = 60;
pub(crate) const CDP_HOLD_S: u32 = 180;
pub(crate) const LLDP_HOLD_S: u32 = 120;
/// LLDP goes out every tick, CDP every second one.
const TICK_MICROS: u64 = 30_000_000;

#[derive(Clone, Debug)]
pub(crate) struct Neighbor {
    pub(crate) local: IfId,
    pub(crate) advert: Advert,
    pub(crate) heard: SimTime,
}

impl Neighbor {
    /// Seconds until the entry expires.
    pub(crate) fn hold_left(&self, now: SimTime) -> u64 {
        u64::from(self.advert.hold_s).saturating_sub(now.saturating_sub(self.heard) / 1_000_000)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Neighbors {
    pub(crate) list: Vec<Neighbor>,
    armed: bool,
    ticks: u32,
    /// Protocols that were running when last synchronised (CDP, LLDP).
    was_running: (bool, bool),
}

impl Neighbors {
    /// Live entries of one protocol, ordered by local interface.
    pub(crate) fn entries(&self, lldp: bool, now: SimTime) -> Vec<&Neighbor> {
        let mut out: Vec<&Neighbor> =
            self.list.iter().filter(|n| n.advert.lldp == lldp && n.hold_left(now) > 0).collect();
        out.sort_by_key(|n| n.local);
        out
    }
}

impl Node {
    fn cdp_on(&self, port: IfId) -> bool {
        self.cfg.cdp_run && self.cfg.iface(port).is_some_and(|c| c.cdp_enable)
    }

    fn advert(&self, port: IfId, lldp: bool) -> Advert {
        let l2_port = self.l2_port(port).unwrap_or(port);
        let native_vlan = self
            .cfg
            .iface(l2_port)
            .and_then(|c| c.l2.as_ref())
            .map(|sw| if self.is_trunk(l2_port) { sw.native_vlan } else { sw.access_vlan });
        let capabilities = match (lldp, self.role()) {
            (true, Role::Router) => "R",
            (true, Role::L2Switch) => "B",
            (true, Role::L3Switch) => "B,R",
            (true, _) => "S",
            (false, _) => self.model.capabilities,
        };
        let description = self.cfg.iface(port).map(|c| c.description.clone()).filter(|d| !d.is_empty());
        Advert {
            lldp,
            device: self.hostname().to_string(),
            port: if lldp { port.short() } else { port.name() },
            port_description: description.unwrap_or_else(|| port.name()),
            platform: format!("Nocturne {}", self.model.platform),
            capabilities: capabilities.to_string(),
            software: format!(
                "Nocturne Virtual Software ({}), Version {}, RELEASE SOFTWARE (fc1)",
                self.model.image, self.model.version
            ),
            mgmt_ip: self.mgmt_ip(),
            native_vlan,
            full_duplex: self.ifs.get(&port).is_some_and(|s| s.full_duplex),
            chassis: self.base_mac,
            hold_s: if lldp { LLDP_HOLD_S } else { CDP_HOLD_S },
        }
    }

    fn send_adverts(&mut self, fx: &mut Fx, ports: &[IfId], cdp: bool, lldp: bool) {
        for port in ports {
            let Some(cfg) = self.cfg.iface(*port) else { continue };
            if !port.is_physical() || !self.is_up(*port) {
                continue;
            }
            let lldp_tx = self.cfg.lldp_run && cfg.lldp_transmit;
            for (is_lldp, enabled) in [(false, cdp && self.cdp_on(*port)), (true, lldp && lldp_tx)] {
                if !enabled {
                    continue;
                }
                let advert = self.advert(*port, is_lldp);
                let mut frame = Frame {
                    src: self.mac_for(*port),
                    dst: if is_lldp { MAC_LLDP } else { MAC_CDP },
                    tag: None,
                    body: Body::Advert(Box::new(advert)),
                    meta: Meta { pdu: fx.new_pdu(), ..Meta::default() },
                };
                let noun = self.noun();
                frame.meta.via = fx.note(self.id, PacketEventKind::Created, &frame, Some(*port), None, || {
                    format!(
                        "The {noun} announces itself to whatever is directly attached to {} with a {} advertisement.",
                        port.short(),
                        if is_lldp { "LLDP" } else { "CDP" }
                    )
                });
                fx.tx.push((*port, frame));
            }
        }
    }

    /// Advertise on ports that just came up, forget neighbours on ports that
    /// went down, and keep the periodic timer running.
    pub(crate) fn neighbors_sync(&mut self, fx: &mut Fx, came_up: &[IfId]) {
        if !self.model.is_network_os() {
            return;
        }
        let up: Vec<IfId> = self.ifs.iter().filter(|(id, s)| id.is_physical() && s.up).map(|(id, _)| *id).collect();
        let running = (self.cfg.cdp_run, self.cfg.lldp_run);
        self.neighbors.list.retain(|n| up.contains(&n.local) && if n.advert.lldp { running.1 } else { running.0 });
        // A protocol that was just switched on introduces itself everywhere.
        let (was_cdp, was_lldp) = std::mem::replace(&mut self.neighbors.was_running, running);
        self.send_adverts(fx, &up, running.0 && !was_cdp, running.1 && !was_lldp);
        let fresh: Vec<IfId> = came_up.iter().copied().filter(|id| id.is_physical()).collect();
        self.send_adverts(fx, &fresh, was_cdp, was_lldp);
        if !self.neighbors.armed {
            self.neighbors.armed = true;
            fx.arm(TICK_MICROS, Timer::Advert);
        }
    }

    pub(crate) fn advert_timer(&mut self, fx: &mut Fx) {
        fx.arm(TICK_MICROS, Timer::Advert);
        self.neighbors.ticks = self.neighbors.ticks.wrapping_add(1);
        let before = self.neighbors.list.len();
        let now = fx.now;
        self.neighbors.list.retain(|n| n.hold_left(now) > 0);
        if self.neighbors.list.len() != before {
            fx.changed = true;
        }
        let ports: Vec<IfId> = self.model.ports.clone();
        let cdp_due = self.neighbors.ticks.is_multiple_of(2);
        self.send_adverts(fx, &ports, cdp_due, true);
    }

    pub(crate) fn advert_rx(&mut self, fx: &mut Fx, port: IfId, advert: &Advert, frame: &Frame) {
        let listening = self.model.is_network_os()
            && if advert.lldp {
                self.cfg.lldp_run && self.cfg.iface(port).is_some_and(|c| c.lldp_receive)
            } else {
                self.cdp_on(port)
            };
        let protocol = if advert.lldp { "LLDP" } else { "CDP" };
        if !listening {
            let noun = self.noun();
            fx.dropped(self.id, frame, Some(port), Why::Ignored, || {
                format!("The {noun} does not run {protocol} on {}: the advertisement is ignored.", port.short())
            });
            return;
        }
        fx.note(self.id, PacketEventKind::Delivered, frame, Some(port), None, || {
            format!(
                "The advertisement is link-local and is never forwarded. {} port {} is recorded as the {protocol} neighbour on {}.",
                advert.device,
                advert.port,
                port.short()
            )
        });
        match self.neighbors.list.iter_mut().find(|n| n.advert.lldp == advert.lldp && n.local == port) {
            Some(entry) => {
                fx.changed |= entry.advert.device != advert.device;
                entry.advert = advert.clone();
                entry.heard = fx.now;
            }
            None => {
                self.neighbors.list.push(Neighbor { local: port, advert: advert.clone(), heard: fx.now });
                fx.changed = true;
            }
        }
        if !advert.lldp {
            self.cdp_checks(fx, port, advert);
        }
    }

    /// CDP notices when the two ends of a cable disagree.
    fn cdp_checks(&mut self, fx: &mut Fx, port: IfId, advert: &Advert) {
        let own = self.advert(port, false);
        if let (Some(mine), Some(theirs)) = (own.native_vlan, advert.native_vlan)
            && mine != theirs
        {
            let message = format!(
                "Native VLAN mismatch discovered on {} ({mine}), with {} {} ({theirs}).",
                port.name(),
                advert.device,
                advert.port
            );
            self.log(fx, 4, "CDP", "NATIVE_VLAN_MISMATCH", message);
        }
        if own.full_duplex != advert.full_duplex {
            let word = |full: bool| if full { "not half duplex" } else { "half duplex" };
            let message = format!(
                "duplex mismatch discovered on {} ({}), with {} {} ({}).",
                port.name(),
                word(own.full_duplex),
                advert.device,
                advert.port,
                word(advert.full_duplex)
            );
            self.log(fx, 4, "CDP", "DUPLEX_MISMATCH", message);
        }
    }

    pub(crate) fn neighbor_rows(&self, now: SimTime) -> Vec<NeighborRow> {
        let mut rows = Vec::new();
        for lldp in [false, true] {
            for n in self.neighbors.entries(lldp, now) {
                rows.push(NeighborRow {
                    protocol: if lldp { "LLDP" } else { "CDP" }.to_string(),
                    local_if: n.local.name(),
                    device: n.advert.device.clone(),
                    remote_if: n.advert.port.clone(),
                    platform: n.advert.platform.clone(),
                    mgmt_ip: n.advert.mgmt_ip.map(|ip| ip.to_string()).unwrap_or_default(),
                });
            }
        }
        rows
    }
}
