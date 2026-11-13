//! Which port is cabled to which port.
//!
//! Half the catalogue is about two ends of a link disagreeing, so a link has
//! to be something better than a guess. When the topology feature is present
//! its graph is authoritative (`topology.get`). When it is not, links are
//! derived here from collected neighbours (LLDP/CDP/MNDP), with the matching
//! deliberately narrow, because a wrong link is a false positive on every
//! check that uses it.
//!
//! **What the matcher does.** A neighbour row names a remote device and a
//! remote port. The remote device is resolved against the inventory by, in
//! order: the device's own name, its collected hostname (`Facts`), either of
//! those without the DNS domain, its management address, or any IPv4 address
//! collected from its interfaces. A key that two devices share is dropped
//! rather than guessed at. The remote port must exist in the remote device's
//! collected interfaces, in some spelling `ifname` understands.
//!
//! **What it refuses to do, and so what the checks never see.**
//!
//! * Remote ports reported as a MAC address (LLDP port-id subtype 3, common
//!   on APs and phones) are not resolvable to an interface: no link.
//! * A local port that reports neighbours on *two different* devices is a hub,
//!   a shared segment, or a phone with a switch behind it: no link, because
//!   "the other end" is not one port and comparing duplex or VLANs with
//!   either of them is meaningless.
//! * Two reports that disagree about where a port goes produce no link at
//!   all; the matcher never picks a winner.
//! * Neighbours that are not in the inventory are not links. Nocturne cannot
//!   read their side, so nothing can be compared.
//! * A port-channel and its members both appear if both are reported; checks
//!   that would double-count say so themselves.

use std::collections::{BTreeMap, HashMap, HashSet};

use nocturne_drivers::ifname;
use serde_json::Value;

use crate::world::{DeviceData, canonical};

/// One end of a link.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct End {
    pub device_id: String,
    pub device: String,
    /// Canonical long form, as the device itself spells it where known.
    pub interface: String,
}

/// A cable between two managed devices.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Link {
    /// The end with the smaller device id: links are the same in every scan.
    pub a: End,
    pub b: End,
    /// Both ends reported each other.
    pub confirmed: bool,
    /// `lldp`, `cdp`, `topology`, …
    pub how: String,
}

impl Link {
    /// The end on `device_id`, and the other one.
    pub fn ends_from(&self, device_id: &str) -> Option<(&End, &End)> {
        if self.a.device_id == device_id {
            Some((&self.a, &self.b))
        } else if self.b.device_id == device_id {
            Some((&self.b, &self.a))
        } else {
            None
        }
    }

    /// The end that owns a finding about the whole link: deterministic, so a
    /// finding keeps its id whichever end a scan started from.
    pub fn owner(&self) -> &End {
        &self.a
    }
}

fn ordered(one: End, other: End, confirmed: bool, how: String) -> Link {
    let (a, b) = if (&one.device_id, &one.interface) <= (&other.device_id, &other.interface) {
        (one, other)
    } else {
        (other, one)
    };
    Link {
        a,
        b,
        confirmed,
        how,
    }
}

/// Looks like a MAC address rather than a port name.
fn looks_like_mac(text: &str) -> bool {
    let digits: String = text
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | '.' | ' '))
        .collect();
    digits.len() == 12 && digits.chars().all(|c| c.is_ascii_hexdigit())
}

/// Names and addresses a device can be recognised by. Keys shared by two
/// devices are removed: an ambiguous match is not a match.
fn identity_index(devices: &[DeviceData]) -> HashMap<String, usize> {
    let mut seen: HashMap<String, Option<usize>> = HashMap::new();
    let add = |key: &str, position: usize, seen: &mut HashMap<String, Option<usize>>| {
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return;
        }
        match seen.get(&key) {
            Some(Some(existing)) if *existing != position => {
                seen.insert(key, None);
            }
            Some(_) => {}
            None => {
                seen.insert(key, Some(position));
            }
        }
    };
    for (position, data) in devices.iter().enumerate() {
        add(&data.device.name, position, &mut seen);
        if let Some(label) = data.device.name.split('.').next() {
            add(label, position, &mut seen);
        }
        if !data.device.address.is_empty() {
            add(&data.device.address, position, &mut seen);
        }
        if let Some(facts) = data.facts.as_ref().and_then(|data| data.one()) {
            add(&facts.hostname, position, &mut seen);
            if let Some(label) = facts.hostname.split('.').next() {
                add(label, position, &mut seen);
            }
        }
        for interface in data.interface_rows() {
            for address in &interface.ipv4 {
                add(
                    address.split('/').next().unwrap_or(address),
                    position,
                    &mut seen,
                );
            }
        }
    }
    seen.into_iter()
        .filter_map(|(key, position)| position.map(|position| (key, position)))
        .collect()
}

fn resolve<'a>(
    index: &HashMap<String, usize>,
    devices: &'a [DeviceData],
    name: &str,
    address: &str,
) -> Option<(usize, &'a DeviceData)> {
    let candidates = [
        name.trim().to_ascii_lowercase(),
        name.split('.')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase(),
        // CDP device ids sometimes carry the serial: `SW1(FOC1234X5YZ)`.
        name.split('(')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase(),
        address.trim().to_ascii_lowercase(),
    ];
    for candidate in candidates {
        if candidate.is_empty() {
            continue;
        }
        if let Some(position) = index.get(&candidate) {
            return devices.get(*position).map(|data| (*position, data));
        }
    }
    None
}

/// Derive links from collected neighbours. See the module documentation for
/// what this deliberately will not infer.
pub fn from_neighbors(devices: &[DeviceData]) -> Vec<Link> {
    let index = identity_index(devices);
    // (local device, local interface) → the one remote end it may have.
    let mut reported: BTreeMap<(String, String), Option<(End, String)>> = BTreeMap::new();
    let mut local_names: HashMap<(String, String), String> = HashMap::new();

    for data in devices {
        let Some(neighbors) = data.neighbors.as_ref() else {
            continue;
        };
        for neighbor in &neighbors.rows {
            if neighbor.local_interface.trim().is_empty() {
                continue;
            }
            let local_key = (data.device.id.clone(), canonical(&neighbor.local_interface));
            local_names
                .entry(local_key.clone())
                .or_insert_with(|| ifname::long(&neighbor.local_interface));
            let Some((_, remote)) = resolve(&index, devices, &neighbor.device, &neighbor.address)
            else {
                // Not in the inventory: nothing of its side can be read.
                continue;
            };
            if remote.device.id == data.device.id {
                // A device seeing itself (a loop, or a mirrored port): never a link.
                reported.insert(local_key, None);
                continue;
            }
            if neighbor.remote_interface.trim().is_empty()
                || looks_like_mac(&neighbor.remote_interface)
            {
                reported.insert(local_key, None);
                continue;
            }
            // The far port has to be a port the far device actually has.
            let far = match remote.interface(&neighbor.remote_interface) {
                Some(interface) => interface.name.clone(),
                None if remote.interfaces.is_none() => ifname::long(&neighbor.remote_interface),
                None => {
                    reported.insert(local_key, None);
                    continue;
                }
            };
            let end = End {
                device_id: remote.device.id.clone(),
                device: remote.device.name.clone(),
                interface: far,
            };
            let protocol = if neighbor.protocol.is_empty() {
                "neighbor".to_string()
            } else {
                neighbor.protocol.clone()
            };
            match reported.get(&local_key) {
                // A second, different neighbour on the same port: a hub, a
                // shared segment or a phone with a switch behind it.
                Some(Some((existing, _))) if *existing != end => {
                    reported.insert(local_key, None);
                }
                Some(_) => {}
                None => {
                    reported.insert(local_key, Some((end, protocol)));
                }
            }
        }
    }

    let names: HashMap<&str, &str> = devices
        .iter()
        .map(|data| (data.device.id.as_str(), data.device.name.as_str()))
        .collect();
    let mut links: BTreeMap<(End, End), (bool, String)> = BTreeMap::new();
    let mut unusable: HashSet<(String, String)> = HashSet::new();
    for (local_key, remote) in &reported {
        if remote.is_none() {
            unusable.insert(local_key.clone());
        }
    }
    for (local_key, remote) in &reported {
        let Some((far, protocol)) = remote else {
            continue;
        };
        let far_key = (far.device_id.clone(), canonical(&far.interface));
        if unusable.contains(&far_key) {
            // The other end says this port goes somewhere else, or to more
            // than one place. Refuse to pick a winner.
            continue;
        }
        let near = End {
            device_id: local_key.0.clone(),
            // Filled in now, not later: an end must be spelled the same
            // whichever side of the link it was seen from, or one cable
            // becomes two links.
            device: names
                .get(local_key.0.as_str())
                .copied()
                .unwrap_or_default()
                .to_string(),
            interface: local_names
                .get(local_key)
                .cloned()
                .unwrap_or_else(|| local_key.1.clone()),
        };
        // Did the far end report this one back?
        let confirmed = reported
            .get(&far_key)
            .and_then(|reported| reported.as_ref())
            .is_some_and(|(back, _)| {
                back.device_id == near.device_id && canonical(&back.interface) == local_key.1
            });
        let link = ordered(near, far.clone(), confirmed, protocol.clone());
        let entry = links
            .entry((link.a.clone(), link.b.clone()))
            .or_insert((confirmed, link.how.clone()));
        entry.0 |= confirmed;
    }

    links
        .into_iter()
        .map(|((a, b), (confirmed, how))| Link {
            a,
            b,
            confirmed,
            how,
        })
        .collect()
}

/// Links from the topology feature's graph, when it is registered.
///
/// Only links between two devices in the inventory, both ends naming an
/// interface, are usable here; endpoint and subnet links are not cables.
pub fn from_topology(graph: &Value, devices: &[DeviceData]) -> Vec<Link> {
    let by_node: HashMap<&str, (&str, &str)> = graph["nodes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|node| {
            Some((
                node["id"].as_str()?,
                (
                    node["device_id"].as_str()?,
                    node["name"].as_str().unwrap_or(""),
                ),
            ))
        })
        .collect();
    let known: HashSet<&str> = devices.iter().map(|data| data.device.id.as_str()).collect();

    let mut links = Vec::new();
    for link in graph["links"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let kind = link["kind"].as_str().unwrap_or("");
        if matches!(kind, "subnet" | "mac") {
            continue;
        }
        let end = |side: &str| -> Option<End> {
            let node = link[side]["node"].as_str()?;
            let interface = link[side]["interface"].as_str()?;
            let (device_id, name) = by_node.get(node)?;
            known.contains(device_id).then(|| End {
                device_id: (*device_id).to_string(),
                device: (*name).to_string(),
                interface: ifname::long(interface),
            })
        };
        let (Some(a), Some(b)) = (end("a"), end("b")) else {
            continue;
        };
        if a.device_id == b.device_id {
            continue;
        }
        let confirmed = link["confidence"].as_str() == Some("confirmed");
        links.push(ordered(a, b, confirmed, format!("topology/{kind}")));
    }
    links.sort();
    links.dedup();
    links
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdata::{device, iface, neighbor};

    #[test]
    fn a_link_both_ends_report_is_confirmed() {
        let devices = vec![
            device("a", "SW1")
                .interfaces(vec![iface("GigabitEthernet0/1")])
                .neighbors(vec![neighbor(
                    "GigabitEthernet0/1",
                    "SW2",
                    "GigabitEthernet0/2",
                )]),
            device("b", "SW2")
                .interfaces(vec![iface("GigabitEthernet0/2")])
                .neighbors(vec![neighbor(
                    "GigabitEthernet0/2",
                    "SW1",
                    "GigabitEthernet0/1",
                )]),
        ];
        let links = from_neighbors(&devices);
        assert_eq!(links.len(), 1);
        assert!(links[0].confirmed);
        assert_eq!(links[0].a.device_id, "a");
        assert_eq!(links[0].b.interface, "GigabitEthernet0/2");
    }

    #[test]
    fn one_sided_reports_still_make_a_link() {
        let devices = vec![
            device("a", "SW1")
                .interfaces(vec![iface("GigabitEthernet0/1")])
                .neighbors(vec![neighbor(
                    "GigabitEthernet0/1",
                    "SW2.lab.example",
                    "Gi0/2",
                )]),
            device("b", "SW2").interfaces(vec![iface("GigabitEthernet0/2")]),
        ];
        let links = from_neighbors(&devices);
        assert_eq!(links.len(), 1);
        assert!(!links[0].confirmed);
        assert_eq!(
            links[0].b.interface, "GigabitEthernet0/2",
            "the far spelling is the far device's own"
        );
    }

    #[test]
    fn what_the_matcher_refuses_to_infer() {
        // Two neighbours on one port: a hub or a phone with a PC behind it.
        let devices = vec![
            device("a", "SW1")
                .interfaces(vec![iface("GigabitEthernet0/1")])
                .neighbors(vec![
                    neighbor("GigabitEthernet0/1", "SW2", "Gi0/2"),
                    neighbor("GigabitEthernet0/1", "SW3", "Gi0/3"),
                ]),
            device("b", "SW2").interfaces(vec![iface("GigabitEthernet0/2")]),
            device("c", "SW3").interfaces(vec![iface("GigabitEthernet0/3")]),
        ];
        assert!(from_neighbors(&devices).is_empty());

        // A remote port reported as a MAC address.
        let devices = vec![
            device("a", "SW1")
                .interfaces(vec![iface("GigabitEthernet0/1")])
                .neighbors(vec![neighbor(
                    "GigabitEthernet0/1",
                    "SW2",
                    "00:11:22:33:44:55",
                )]),
            device("b", "SW2").interfaces(vec![iface("GigabitEthernet0/2")]),
        ];
        assert!(from_neighbors(&devices).is_empty());

        // A remote port the remote device does not have.
        let devices = vec![
            device("a", "SW1")
                .interfaces(vec![iface("GigabitEthernet0/1")])
                .neighbors(vec![neighbor("GigabitEthernet0/1", "SW2", "Gi9/9")]),
            device("b", "SW2").interfaces(vec![iface("GigabitEthernet0/2")]),
        ];
        assert!(from_neighbors(&devices).is_empty());

        // A neighbour that is not in the inventory.
        let devices = vec![
            device("a", "SW1")
                .interfaces(vec![iface("GigabitEthernet0/1")])
                .neighbors(vec![neighbor("GigabitEthernet0/1", "printer-3", "eth0")]),
        ];
        assert!(from_neighbors(&devices).is_empty());

        // Two devices that share a short name: ambiguous, so not a match.
        let devices = vec![
            device("a", "SW1")
                .interfaces(vec![iface("GigabitEthernet0/1")])
                .neighbors(vec![neighbor("GigabitEthernet0/1", "core", "Gi1/0/1")]),
            device("b", "core.site-a").interfaces(vec![iface("GigabitEthernet1/0/1")]),
            device("c", "core.site-b").interfaces(vec![iface("GigabitEthernet1/0/1")]),
        ];
        assert!(from_neighbors(&devices).is_empty());
    }

    #[test]
    fn ends_that_disagree_produce_nothing() {
        let devices = vec![
            device("a", "SW1")
                .interfaces(vec![iface("GigabitEthernet0/1")])
                .neighbors(vec![neighbor("GigabitEthernet0/1", "SW2", "Gi0/2")]),
            device("b", "SW2")
                .interfaces(vec![
                    iface("GigabitEthernet0/2"),
                    iface("GigabitEthernet0/3"),
                ])
                .neighbors(vec![
                    neighbor("GigabitEthernet0/2", "SW1", "Gi0/1"),
                    neighbor("GigabitEthernet0/2", "SW3", "Gi0/9"),
                ]),
            device("c", "SW3").interfaces(vec![iface("GigabitEthernet0/9")]),
        ];
        assert!(from_neighbors(&devices).is_empty());
    }

    #[test]
    fn a_topology_graph_is_read_when_the_feature_is_there() {
        let graph = serde_json::json!({
            "nodes": [
                {"id": "n1", "device_id": "a", "name": "SW1"},
                {"id": "n2", "device_id": "b", "name": "SW2"},
                {"id": "n3", "name": "unmanaged-ap"}
            ],
            "links": [
                {"a": {"node": "n1", "interface": "Gi0/1"}, "b": {"node": "n2", "interface": "Gi0/2"},
                 "kind": "lldp", "confidence": "confirmed"},
                {"a": {"node": "n1", "interface": "Gi0/5"}, "b": {"node": "n3", "interface": "eth0"},
                 "kind": "cdp", "confidence": "one_sided"},
                {"a": {"node": "n1"}, "b": {"node": "n2"}, "kind": "subnet", "confidence": "inferred"}
            ]
        });
        let devices = vec![device("a", "SW1"), device("b", "SW2")];
        let links = from_topology(&graph, &devices);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].a.interface, "GigabitEthernet0/1");
        assert!(links[0].confirmed);
    }
}
