//! Editing the lab: nodes, cables, power. Everything here ends by marking the
//! affected nodes dirty and reconciling, so derived state is never stale.

use std::net::Ipv4Addr;

use crate::api::{CableKind, CliKind, Fault, LabError, LinkId, NodeId, NodeKind};
use crate::catalog::{self, Model};
use crate::device::config::DeviceConfig;
use crate::ifname::IfId;
use crate::sim::node::Node;
use crate::sim::{ConsoleLink, Link, World};

/// Well-known port names of the out-of-band console cable.
pub(crate) const CONSOLE_PORT: &str = "Console";
pub(crate) const SERIAL_PORT: &str = "RS232";

/// Node names double as hostnames, so they follow hostname rules.
pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn name_prefix(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Router => "R",
        NodeKind::Switch => "SW",
        NodeKind::L3Switch => "MLS",
        NodeKind::Firewall => "FW",
        NodeKind::AccessPoint => "AP",
        NodeKind::Pc => "PC",
        NodeKind::Laptop => "Laptop",
        NodeKind::Server => "Server",
        NodeKind::Printer => "Printer",
        NodeKind::Phone => "Phone",
        NodeKind::Cloud => "Internet",
        NodeKind::Nms => "NMS",
    }
}

impl World {
    pub(crate) fn node_by_name(&self, name: &str) -> Option<NodeId> {
        self.nodes.values().find(|n| n.name == name).map(|n| n.id)
    }

    fn unique_name(&self, model: &Model) -> String {
        let prefix = name_prefix(model.kind);
        // "Internet" reads better without a number while it is the only one.
        if model.kind == NodeKind::Cloud && self.node_by_name(prefix).is_none() {
            return prefix.to_string();
        }
        (1..).map(|n| format!("{prefix}{n}")).find(|name| self.node_by_name(name).is_none()).unwrap_or_default()
    }

    pub(crate) fn add_node(&mut self, model_id: &str, name: Option<&str>, x: f32, y: f32) -> Result<NodeId, LabError> {
        let model = catalog::model(model_id).ok_or_else(|| LabError::UnknownModel(model_id.to_string()))?;
        let name = match name {
            Some(name) if !valid_name(name) => return Err(LabError::InvalidName(name.to_string())),
            Some(name) if self.node_by_name(name).is_some() => return Err(LabError::DuplicateName(name.to_string())),
            Some(name) => name.to_string(),
            None => self.unique_name(model),
        };
        let id = self.next_node;
        self.next_node += 1;
        let mut node = Node::new(id, &name, model, self.seed);
        node.x = x;
        node.y = y;
        if !model.is_network_os() {
            node.host = Some(crate::api::HostConfig::default());
        }
        self.nodes.insert(id, node);
        self.boot(id);
        self.touch();
        self.reconcile();
        Ok(id)
    }

    pub(crate) fn remove_node(&mut self, id: NodeId) -> Result<(), LabError> {
        if !self.nodes.contains_key(&id) {
            return Err(LabError::UnknownNode(id));
        }
        let links: Vec<LinkId> =
            self.links.iter().filter(|(_, l)| l.a.0 == id || l.b.0 == id).map(|(lid, _)| *lid).collect();
        for link in links {
            self.remove_link(link)?;
        }
        self.console_links.retain(|_, l| l.a.0 != id && l.b.0 != id);
        self.nodes.remove(&id);
        self.faults.retain(|_, f| {
            !matches!(f, Fault::InterfaceErrors { node, .. } | Fault::HighCpu { node, .. } | Fault::PowerOff { node } if *node == id)
        });
        self.flows.retain(|_, f| f.src != id);
        self.dirty.remove(&id);
        self.changed.remove(&id);
        self.touch();
        self.reconcile();
        Ok(())
    }

    pub(crate) fn rename_node(&mut self, id: NodeId, name: &str) -> Result<(), LabError> {
        if !valid_name(name) {
            return Err(LabError::InvalidName(name.to_string()));
        }
        if self.node_by_name(name).is_some_and(|other| other != id) {
            return Err(LabError::DuplicateName(name.to_string()));
        }
        let node = self.nodes.get_mut(&id).ok_or(LabError::UnknownNode(id))?;
        // An end host's hostname is its name; a network device keeps whatever
        // `hostname` its configuration says.
        if !node.model.is_network_os() || node.cfg.hostname == node.name {
            node.cfg.hostname = name.to_string();
        }
        node.name = name.to_string();
        self.changed.insert(id);
        self.touch();
        self.reconcile();
        Ok(())
    }

    fn physical_port(&self, node: NodeId, name: &str) -> Result<IfId, LabError> {
        let n = self.nodes.get(&node).ok_or(LabError::UnknownNode(node))?;
        let unknown = || LabError::UnknownInterface { node: n.name.clone(), interface: name.to_string() };
        let id = IfId::parse(name).ok_or_else(unknown)?;
        let state = n.ifs.get(&id).filter(|_| id.is_physical() && n.model.ports.contains(&id)).ok_or_else(unknown)?;
        if state.link.is_some() {
            return Err(LabError::InterfaceInUse { node: n.name.clone(), interface: id.name() });
        }
        Ok(id)
    }

    pub(crate) fn add_link(&mut self, a: NodeId, a_if: &str, b: NodeId, b_if: &str, cable: CableKind) -> Result<LinkId, LabError> {
        if a == b {
            return Err(LabError::SelfLink);
        }
        if cable == CableKind::Console {
            return self.add_console_link(a, a_if, b, b_if);
        }
        let port_a = self.physical_port(a, a_if)?;
        let port_b = self.physical_port(b, b_if)?;
        let id = self.next_link;
        self.next_link += 1;
        self.links.insert(
            id,
            Link {
                a: (a, port_a),
                b: (b, port_b),
                cable,
                up: false,
                fault_down: false,
                flap_down: false,
                loss_percent: 0,
                duplex_fault: false,
                bps_ab: 0,
                bps_ba: 0,
            },
        );
        for (node, port) in [(a, port_a), (b, port_b)] {
            if let Some(state) = self.nodes.get_mut(&node).and_then(|n| n.ifs.get_mut(&port)) {
                state.link = Some(id);
            }
            self.mark_dirty(node);
        }
        self.touch();
        self.reconcile();
        Ok(id)
    }

    fn add_console_link(&mut self, a: NodeId, a_if: &str, b: NodeId, b_if: &str) -> Result<LinkId, LabError> {
        for (node, port) in [(a, a_if), (b, b_if)] {
            let n = self.nodes.get(&node).ok_or(LabError::UnknownNode(node))?;
            let expected = match n.model.cli {
                CliKind::Ios => CONSOLE_PORT,
                CliKind::Host => SERIAL_PORT,
                CliKind::None => "",
            };
            if expected.is_empty() || !port.eq_ignore_ascii_case(expected) {
                return Err(LabError::UnknownInterface { node: n.name.clone(), interface: port.to_string() });
            }
            let used = self.console_links.values().any(|l| l.a.0 == node || l.b.0 == node);
            if used {
                return Err(LabError::InterfaceInUse { node: n.name.clone(), interface: expected.to_string() });
            }
        }
        let canonical = |port: &str| {
            if port.eq_ignore_ascii_case(CONSOLE_PORT) { CONSOLE_PORT } else { SERIAL_PORT }.to_string()
        };
        let id = self.next_link;
        self.next_link += 1;
        self.console_links.insert(id, ConsoleLink { a: (a, canonical(a_if)), b: (b, canonical(b_if)) });
        self.touch();
        Ok(id)
    }

    pub(crate) fn remove_link(&mut self, id: LinkId) -> Result<(), LabError> {
        if self.console_links.remove(&id).is_some() {
            self.touch();
            return Ok(());
        }
        let link = self.links.remove(&id).ok_or(LabError::UnknownLink(id))?;
        for (node, port) in [link.a, link.b] {
            if let Some(state) = self.nodes.get_mut(&node).and_then(|n| n.ifs.get_mut(&port)) {
                state.link = None;
                state.peer = None;
                state.carrier = false;
            }
            self.mark_dirty(node);
        }
        self.faults.retain(|_, f| {
            !matches!(
                f,
                Fault::LinkDown { link } | Fault::PacketLoss { link, .. } | Fault::Flapping { link, .. } | Fault::DuplexMismatch { link }
                    if *link == id
            )
        });
        self.touch();
        self.reconcile();
        Ok(())
    }

    /// Power a node off (running configuration and all learned state are lost)
    /// or on (it boots from its startup configuration).
    pub(crate) fn set_power(&mut self, id: NodeId, on: bool) {
        let Some(node) = self.nodes.get_mut(&id) else { return };
        if node.powered == on {
            return;
        }
        node.powered = on;
        if on {
            self.boot(id);
        } else {
            node.epoch = node.epoch.wrapping_add(1);
            node.clear_runtime();
            node.cfg = DeviceConfig::factory(node.model, &node.name);
            for state in node.ifs.values_mut() {
                state.carrier = false;
                state.up = false;
            }
            self.mark_dirty(id);
        }
        self.touch();
        self.reconcile();
    }

    /// (Re)start a node from its saved configuration.
    pub(crate) fn boot(&mut self, id: NodeId) {
        let now = self.now;
        let Some(node) = self.nodes.get_mut(&id) else { return };
        node.epoch = node.epoch.wrapping_add(1);
        node.boot_time = now;
        node.clock_offset_ms = None;
        node.clear_runtime();
        node.cfg = DeviceConfig::factory(node.model, &node.name);
        if node.model.is_network_os() {
            if let Some(vlans) = node.vlan_dat.clone() {
                node.cfg.vlans = vlans;
            }
            crate::device::ios::boot::apply_startup(self, id);
        } else {
            crate::device::hostcfg::apply(node);
        }
        let Some(node) = self.nodes.get_mut(&id) else { return };
        // Interfaces that start out shut down should not log a transition.
        let shut: Vec<(IfId, bool)> = node.cfg.interfaces.iter().map(|(i, c)| (*i, c.shutdown)).collect();
        for (iface, shutdown) in shut {
            if let Some(state) = node.ifs.get_mut(&iface) {
                state.was_shutdown = shutdown;
            }
        }
        self.with_node(id, |node, fx| {
            node.log(fx, 5, "SYS", "RESTART", "System restarted --".to_string());
        });
        self.mark_dirty(id);
        self.changed.insert(id);
    }

    /// The node that owns `ip` on an up interface (or, failing that, has it configured).
    pub(crate) fn node_by_ip(&self, ip: Ipv4Addr) -> Option<NodeId> {
        self.nodes
            .values()
            .find(|n| n.powered && n.rib.addrs.iter().any(|(_, a)| a.ip == ip))
            .or_else(|| {
                self.nodes.values().find(|n| {
                    n.cfg.interfaces.values().any(|c| c.static_addr().is_some_and(|a| a.ip == ip) || c.secondary.iter().any(|a| a.ip == ip))
                })
            })
            .map(|n| n.id)
    }

    /// Rebuild the records served by nodes that run the `dns` service.
    pub(crate) fn refresh_dns(&mut self) {
        if !self.nodes.values().any(|n| n.has_service("dns")) {
            return;
        }
        let mut zone: Vec<(String, Ipv4Addr)> = Vec::new();
        for node in self.nodes.values() {
            let Some(ip) = node.mgmt_ip() else { continue };
            zone.push((node.name.clone(), ip));
            if node.cfg.hostname != node.name {
                zone.push((node.cfg.hostname.clone(), ip));
            }
        }
        for node in self.nodes.values_mut().filter(|n| n.has_service("dns")) {
            node.dns_zone = zone.clone();
        }
    }
}

impl Node {
    /// Forget everything learned at run time (power loss or reload).
    pub(crate) fn clear_runtime(&mut self) {
        self.macs.clear();
        self.arp.clear();
        self.rib.clear();
        self.stp.reset();
        self.neighbors = Default::default();
        self.ospf = Default::default();
        self.rip = Default::default();
        self.dhcpd = Default::default();
        self.nat = Default::default();
        self.hsrp = Default::default();
        self.acl_hits.clear();
        self.procs.clear();
        self.log.clear();
        self.forwarded = 0;
        self.last_config_change = None;
        self.ifs.retain(|id, _| id.is_physical());
        for state in self.ifs.values_mut() {
            let (mac, link, speed) = (state.mac, state.link, state.speed);
            *state = crate::sim::node::IfState::new(mac, speed);
            state.link = link;
        }
    }

    /// Best address to manage the device on: a loopback, else a switch's
    /// management SVI, else the first usable interface address.
    pub(crate) fn mgmt_ip(&self) -> Option<Ipv4Addr> {
        use crate::ifname::IfKind;
        let candidates: Vec<(IfId, Ipv4Addr, bool)> = self
            .cfg
            .interfaces
            .keys()
            .filter(|id| self.is_l3(**id))
            .filter_map(|id| self.addr(*id).map(|a| (*id, a.ip, self.is_up(*id))))
            .collect();
        let pick = |kind: Option<IfKind>, need_up: bool| {
            candidates
                .iter()
                .find(|(id, _, up)| kind.is_none_or(|k| id.kind == k) && (*up || !need_up))
                .map(|(_, ip, _)| *ip)
        };
        pick(Some(IfKind::Loopback), true)
            .or_else(|| pick(Some(IfKind::Vlan), true))
            .or_else(|| pick(None, true))
            .or_else(|| pick(None, false))
    }
}
