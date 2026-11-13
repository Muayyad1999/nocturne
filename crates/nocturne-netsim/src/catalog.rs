//! The device models a lab can contain.

use std::sync::OnceLock;

use crate::api::{CliKind, ModelInfo, NodeKind};
use crate::ifname::{IfId, IfKind};

/// How a node behaves on the wire, independent of its icon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    /// Routed ports only; forwards IPv4.
    Router,
    /// VLAN-aware bridge with a management SVI; never routes.
    L2Switch,
    /// Bridge that can also route (`ip routing`, SVIs, `no switchport`).
    L3Switch,
    /// Single-homed end system.
    Host,
    /// Multi-port end system that forwards between its ports ("the Internet").
    Cloud,
    /// Transparent bridge without spanning tree or CLI (access point).
    Bridge,
}

#[derive(Debug)]
pub(crate) struct Model {
    pub(crate) id: &'static str,
    pub(crate) kind: NodeKind,
    pub(crate) role: Role,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) ports: Vec<IfId>,
    pub(crate) cli: CliKind,
    /// Hardware name shown by `show version`, `show inventory` and CDP.
    pub(crate) platform: &'static str,
    /// Software image family, e.g. `NVOS-L3-UNIVERSALK9`.
    pub(crate) image: &'static str,
    pub(crate) version: &'static str,
    /// CDP capability letters.
    pub(crate) capabilities: &'static str,
}

impl Model {
    pub(crate) fn is_switch(&self) -> bool {
        matches!(self.role, Role::L2Switch | Role::L3Switch)
    }

    /// Runs the IOS-style network operating system.
    pub(crate) fn is_network_os(&self) -> bool {
        matches!(self.role, Role::Router | Role::L2Switch | Role::L3Switch)
    }
}

fn ports(kind: IfKind, prefix: &[u32], range: std::ops::RangeInclusive<u32>) -> Vec<IfId> {
    range
        .map(|n| {
            let mut path = prefix.to_vec();
            path.push(n);
            IfId::new(kind, &path)
        })
        .collect()
}

fn build() -> Vec<Model> {
    use IfKind::{Dot11Radio, Ethernet, FastEthernet, GigabitEthernet, TenGigabitEthernet};
    let host = |id, kind, label, description, cli, platform| Model {
        id,
        kind,
        role: Role::Host,
        label,
        description,
        ports: ports(Ethernet, &[], 0..=0),
        cli,
        platform,
        image: "",
        version: "",
        capabilities: "H",
    };
    let router = |id, kind, label, description, count: u32, platform| Model {
        id,
        kind,
        role: Role::Router,
        label,
        description,
        ports: ports(GigabitEthernet, &[0], 0..=count - 1),
        cli: CliKind::Ios,
        platform,
        image: "NVOS-L3-UNIVERSALK9",
        version: "15.9(3)M",
        capabilities: "R",
    };
    vec![
        router(
            "router",
            NodeKind::Router,
            "Router (4 × GE)",
            "Branch router with four routed Gigabit Ethernet ports: static routing, OSPF, RIP, NAT, ACLs, DHCP, HSRP.",
            4,
            "NV-ROUTER-4GE",
        ),
        router(
            "router-8",
            NodeKind::Router,
            "Router (8 × GE)",
            "Aggregation router with eight routed Gigabit Ethernet ports.",
            8,
            "NV-ROUTER-8GE",
        ),
        Model {
            id: "switch",
            kind: NodeKind::Switch,
            role: Role::L2Switch,
            label: "Switch (24 × FE + 2 × GE)",
            description: "Layer-2 access switch: VLANs, 802.1Q trunks, spanning tree, EtherChannel, port security. \
                          Managed through interface Vlan1 and ip default-gateway.",
            ports: [ports(FastEthernet, &[0], 1..=24), ports(GigabitEthernet, &[0], 1..=2)].concat(),
            cli: CliKind::Ios,
            platform: "NV-SWITCH-24TT",
            image: "NVOS-L2-LANBASEK9",
            version: "15.2(7)E",
            capabilities: "S I",
        },
        Model {
            id: "switch-l3",
            kind: NodeKind::L3Switch,
            role: Role::L3Switch,
            label: "Layer-3 switch (24 × GE + 4 × 10GE)",
            description: "Multilayer distribution switch: everything the access switch does plus ip routing, SVIs, \
                          routed ports, OSPF and HSRP.",
            ports: [ports(GigabitEthernet, &[1, 0], 1..=24), ports(TenGigabitEthernet, &[1, 1], 1..=4)].concat(),
            cli: CliKind::Ios,
            platform: "NV-SWITCH-L3-24T4X",
            image: "NVOS-L3-IPSERVICESK9",
            version: "15.2(7)E",
            capabilities: "R S I",
        },
        router(
            "firewall",
            NodeKind::Firewall,
            "Firewall (4 × GE)",
            "Edge firewall with a router personality: access lists and NAT on four routed ports.",
            4,
            "NV-FIREWALL-4GE",
        ),
        Model {
            id: "ap",
            kind: NodeKind::AccessPoint,
            role: Role::Bridge,
            label: "Access point",
            description: "Transparent bridge between its wired uplink and its radio. No command line.",
            ports: vec![IfId::new(GigabitEthernet, &[0]), IfId::new(Dot11Radio, &[0])],
            cli: CliKind::None,
            platform: "NV-AP",
            image: "",
            version: "",
            capabilities: "T",
        },
        host("pc", NodeKind::Pc, "PC", "Desktop computer with a command prompt.", CliKind::Host, "NV-PC"),
        host("laptop", NodeKind::Laptop, "Laptop", "Laptop computer with a command prompt.", CliKind::Host, "NV-LAPTOP"),
        host(
            "server",
            NodeKind::Server,
            "Server",
            "Server with a command prompt and optional services (http, dns, dhcp, tftp, syslog, ntp, ssh, telnet).",
            CliKind::Host,
            "NV-SERVER",
        ),
        host("printer", NodeKind::Printer, "Printer", "Network printer. No command line.", CliKind::None, "NV-PRINTER"),
        host("phone", NodeKind::Phone, "IP phone", "IP telephone. No command line.", CliKind::None, "NV-PHONE"),
        Model {
            id: "cloud",
            kind: NodeKind::Cloud,
            role: Role::Cloud,
            label: "Internet cloud",
            description: "Stands in for the Internet: answers for its extra addresses and routes between its ports.",
            ports: ports(Ethernet, &[], 0..=3),
            cli: CliKind::None,
            platform: "NV-CLOUD",
            image: "",
            version: "",
            capabilities: "R",
        },
        host(
            "nms",
            NodeKind::Nms,
            "Nocturne management station",
            "The Nocturne NOC itself, as seen from inside the lab.",
            CliKind::Host,
            "NV-NMS",
        ),
    ]
}

pub(crate) fn models() -> &'static [Model] {
    static MODELS: OnceLock<Vec<Model>> = OnceLock::new();
    MODELS.get_or_init(build)
}

pub(crate) fn model(id: &str) -> Option<&'static Model> {
    models().iter().find(|m| m.id == id)
}

/// Every model a lab can contain, in palette order.
pub fn catalog() -> Vec<ModelInfo> {
    models()
        .iter()
        .map(|m| ModelInfo {
            id: m.id.to_string(),
            kind: m.kind,
            label: m.label.to_string(),
            description: m.description.to_string(),
            interfaces: m.ports.iter().map(|p| p.name()).collect(),
            cli: m.cli,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_matches_the_spec_table() {
        let cat = catalog();
        let get = |id: &str| cat.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("model {id}"));
        assert_eq!(get("router").interfaces, ["GigabitEthernet0/0", "GigabitEthernet0/1", "GigabitEthernet0/2", "GigabitEthernet0/3"]);
        assert_eq!(get("router-8").interfaces.len(), 8);
        let sw = get("switch");
        assert_eq!(sw.interfaces.len(), 26);
        assert_eq!((sw.interfaces[0].as_str(), sw.interfaces[25].as_str()), ("FastEthernet0/1", "GigabitEthernet0/2"));
        let l3 = get("switch-l3");
        assert_eq!(l3.interfaces.len(), 28);
        assert_eq!((l3.interfaces[0].as_str(), l3.interfaces[27].as_str()), ("GigabitEthernet1/0/1", "TenGigabitEthernet1/1/4"));
        assert_eq!(get("ap").interfaces, ["GigabitEthernet0", "Dot11Radio0"]);
        assert_eq!(get("cloud").interfaces, ["Ethernet0", "Ethernet1", "Ethernet2", "Ethernet3"]);
        for id in ["pc", "laptop", "server", "printer", "phone", "nms"] {
            assert_eq!(get(id).interfaces, ["Ethernet0"], "{id}");
        }
        assert_eq!(get("printer").cli, CliKind::None);
        assert_eq!(get("firewall").kind, NodeKind::Firewall);
        assert_eq!(get("pc").cli, CliKind::Host);
    }
}
