//! Hosts: what the MAC tables say is plugged into an edge port.
//!
//! A port is an edge port when no neighbour protocol sees another network
//! device on it and it carries no more MAC addresses than the setting allows.
//! Everything here is `inferred`: a MAC table says where frames came from, not
//! what is at the other end of the cable.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_drivers::ifname;
use nocturne_drivers::model::Interface;

use crate::data::{Collected, bare_mac, pretty};
use crate::model::{Confidence, End, Link, LinkKind, Node, NodeKind, Status};

/// What kind of port a MAC was seen on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortKind {
    /// No other network device is seen here and few MACs are learnt: an edge port.
    Access,
    /// Carries many VLANs or many MACs.
    Trunk,
    /// A neighbour protocol sees another network device here.
    Uplink,
}

impl PortKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PortKind::Access => "access",
            PortKind::Trunk => "trunk",
            PortKind::Uplink => "uplink",
        }
    }
}

/// One MAC seen on one port of one device.
#[derive(Clone, Debug)]
pub struct Sighting {
    pub device_id: String,
    pub interface: String,
    pub vlan: Option<u64>,
    pub kind: PortKind,
    /// How many distinct MAC addresses that port has learnt.
    pub macs_on_port: usize,
    pub seen_at: i64,
    pub entry_kind: String,
    pub why: String,
}

/// Decide what a port is, from the neighbours seen on it, its configured mode
/// and how many MACs it has learnt.
pub fn classify(
    interface: Option<&Interface>,
    has_network_neighbour: bool,
    macs_on_port: usize,
    max_access_macs: usize,
) -> (PortKind, String) {
    if has_network_neighbour {
        return (
            PortKind::Uplink,
            "a neighbour protocol sees another network device on this port".to_string(),
        );
    }
    let mode = interface
        .map(|interface| interface.mode.to_ascii_lowercase())
        .unwrap_or_default();
    if mode == "trunk" {
        return (
            PortKind::Trunk,
            "the port is configured as a trunk".to_string(),
        );
    }
    let canonical = interface
        .map(|interface| ifname::long(&interface.name))
        .unwrap_or_default();
    if canonical.starts_with("Port-channel") || canonical.starts_with("Bundle-Ether") {
        return (PortKind::Trunk, "the port is a link aggregate".to_string());
    }
    if macs_on_port > max_access_macs {
        return (
            PortKind::Trunk,
            format!(
                "{macs_on_port} MAC addresses are learnt here, more than an access port normally has"
            ),
        );
    }
    let detail = if mode == "access" {
        "the port is configured as an access port and no network neighbour is seen on it"
    } else {
        "no network neighbour is seen on this port and it has learnt few MAC addresses"
    };
    (PortKind::Access, detail.to_string())
}

/// MAC address → every port it has been seen on, across every device in scope.
pub fn sightings(
    collected: &Collected,
    scope: &BTreeSet<String>,
    network_ports: &BTreeMap<String, BTreeSet<String>>,
    max_access_macs: usize,
) -> BTreeMap<String, Vec<Sighting>> {
    let mut found: BTreeMap<String, Vec<Sighting>> = BTreeMap::new();
    for device_id in scope {
        let Some(aged) = collected.mac_table.get(device_id) else {
            continue;
        };
        let mut per_port: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entry in &aged.rows {
            let Some(mac) = bare_mac(&entry.mac) else {
                continue;
            };
            if entry.interface.trim().is_empty() {
                continue;
            }
            per_port
                .entry(ifname::long(&entry.interface))
                .or_default()
                .insert(mac);
        }
        let empty = BTreeSet::new();
        let ports = network_ports.get(device_id).unwrap_or(&empty);
        for entry in &aged.rows {
            let Some(mac) = bare_mac(&entry.mac) else {
                continue;
            };
            if entry.interface.trim().is_empty() {
                continue;
            }
            let interface = ifname::long(&entry.interface);
            let macs_on_port = per_port.get(&interface).map_or(0, BTreeSet::len);
            let (kind, why) = classify(
                collected.interface(device_id, &interface),
                ports.contains(&interface),
                macs_on_port,
                max_access_macs,
            );
            found.entry(mac).or_default().push(Sighting {
                device_id: device_id.clone(),
                interface,
                vlan: entry.vlan,
                kind,
                macs_on_port,
                seen_at: aged.at,
                entry_kind: entry.kind.clone(),
                why,
            });
        }
    }
    for list in found.values_mut() {
        list.sort_by(|a, b| {
            (&a.device_id, ifname::sort_key(&a.interface))
                .cmp(&(&b.device_id, ifname::sort_key(&b.interface)))
        });
        list.dedup_by(|a, b| a.device_id == b.device_id && a.interface == b.interface);
    }
    found
}

/// The sighting that is most likely the port the host is actually plugged
/// into: an access port, then the one that has learnt the fewest MACs.
pub fn best_sighting(sightings: &[Sighting]) -> Option<usize> {
    sightings
        .iter()
        .enumerate()
        .min_by_key(|(index, sighting)| {
            (
                sighting.kind,
                sighting.macs_on_port,
                sighting.device_id.clone(),
                ifname::sort_key(&sighting.interface),
                *index,
            )
        })
        .map(|(index, _)| index)
}

/// Every IP address the collected ARP tables have for a MAC address.
pub fn arp_index(collected: &Collected) -> BTreeMap<String, BTreeSet<String>> {
    let mut index: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for aged in collected.arp.values() {
        for entry in &aged.rows {
            if let Some(mac) = bare_mac(&entry.mac)
                && !entry.ip.trim().is_empty()
            {
                index
                    .entry(mac)
                    .or_default()
                    .insert(entry.ip.trim().to_ascii_lowercase());
            }
        }
    }
    index
}

pub struct Endpoints {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub warnings: Vec<String>,
}

/// Turn edge-port MAC sightings into host nodes.
pub fn build(
    collected: &Collected,
    scope: &BTreeSet<String>,
    network_ports: &BTreeMap<String, BTreeSet<String>>,
    max_access_macs: usize,
) -> Endpoints {
    let seen = sightings(collected, scope, network_ports, max_access_macs);
    let addresses = arp_index(collected);
    let mut nodes = Vec::new();
    let mut links = Vec::new();
    let mut warnings: BTreeSet<String> = BTreeSet::new();

    for (mac, list) in &seen {
        // A MAC that belongs to a managed device is that device, not a host.
        if collected.index().by_mac.contains_key(mac) {
            continue;
        }
        let access: Vec<&Sighting> = list
            .iter()
            .filter(|sighting| sighting.kind == PortKind::Access)
            .collect();
        let Some(port) = access.first() else {
            continue;
        };
        if access.len() > 1 {
            warnings.insert(format!(
                "{} is learnt on {} access ports ({}); it is drawn on {} {}",
                pretty(mac),
                access.len(),
                access
                    .iter()
                    .map(|sighting| format!(
                        "{} {}",
                        collected.name_of(&sighting.device_id),
                        sighting.interface
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                collected.name_of(&port.device_id),
                port.interface
            ));
        }
        let ip = addresses
            .get(mac)
            .and_then(|ips| ips.iter().next().cloned());
        let vendor = nocturne_tools::mac::vendor(&pretty(mac));
        let node_id = format!("h:{mac}");
        nodes.push(Node {
            id: node_id.clone(),
            kind: NodeKind::Host,
            name: host_name(mac, ip.as_deref(), vendor.as_deref()),
            address: ip.clone(),
            model: vendor,
            status: Status::Unknown,
            unmanaged: true,
            collected_at: Some(port.seen_at),
            ..Node::default()
        });
        let mut link = Link {
            a: End::with_interface(port.device_id.clone(), Some(port.interface.clone())),
            b: End {
                node: node_id,
                interface: None,
                ip,
            },
            kind: LinkKind::Mac,
            confidence: Confidence::Inferred,
            status: Status::Up,
            vlans: port.vlan.map(|vlan| vlan.to_string()),
            label: port.vlan.map(|vlan| format!("VLAN {vlan}")),
            ..Link::default()
        };
        link.id = crate::id_of(&link.signature());
        links.push(link);
    }

    Endpoints {
        nodes,
        links,
        warnings: warnings.into_iter().collect(),
    }
}

/// What to call a host: its address if ARP knows it, else who made its card.
pub fn host_name(mac: &str, ip: Option<&str>, vendor: Option<&str>) -> String {
    if let Some(ip) = ip {
        return ip.to_string();
    }
    let tail = &pretty(mac)[9..];
    match vendor {
        Some(vendor) => format!("{vendor} {tail}"),
        None => format!("host {tail}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interface(name: &str, mode: &str) -> Interface {
        Interface {
            name: name.to_string(),
            mode: mode.to_string(),
            ..Interface::default()
        }
    }

    #[test]
    fn ports_are_classified_by_what_is_on_them() {
        let access = interface("Gi1/0/5", "access");
        assert_eq!(classify(Some(&access), false, 1, 3).0, PortKind::Access);
        assert_eq!(classify(Some(&access), true, 1, 3).0, PortKind::Uplink);
        let trunk = interface("Gi1/0/48", "trunk");
        assert_eq!(classify(Some(&trunk), false, 1, 3).0, PortKind::Trunk);
        // Many MACs on a port nobody configured as a trunk is still not an edge.
        assert_eq!(classify(None, false, 40, 3).0, PortKind::Trunk);
        assert_eq!(classify(None, false, 2, 3).0, PortKind::Access);
        assert_eq!(
            classify(Some(&interface("Po1", "")), false, 1, 3).0,
            PortKind::Trunk
        );
    }

    #[test]
    fn hosts_are_named_from_what_is_known() {
        assert_eq!(
            host_name("001b21aabbcc", Some("10.1.10.5"), Some("Intel")),
            "10.1.10.5"
        );
        assert_eq!(
            host_name("001b21aabbcc", None, Some("Intel")),
            "Intel aa:bb:cc"
        );
        assert_eq!(host_name("001b21aabbcc", None, None), "host aa:bb:cc");
    }
}
