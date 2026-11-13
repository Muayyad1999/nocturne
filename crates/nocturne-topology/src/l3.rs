//! The addressing layer: which interfaces share a subnet.
//!
//! Two interfaces in one subnet are a link when there are only two of them,
//! and a group when there are more. Loopbacks are addresses, not cables, and
//! an address that two devices both claim is a shared virtual address
//! (HSRP/VRRP), not a link.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_drivers::ifname;

use crate::data::{Collected, host_of};
use crate::model::{Confidence, End, Group, GroupKind, Link, LinkKind, Status};

/// One interface address on one device.
#[derive(Clone, Debug)]
struct Member {
    device_id: String,
    interface: String,
    ip: String,
    network: String,
    status: Status,
}

pub struct Addressing {
    pub links: Vec<Link>,
    pub groups: Vec<Group>,
    pub warnings: Vec<String>,
}

/// Interfaces that carry an address but never a cable we would draw.
fn is_virtual_interface(name: &str) -> bool {
    let canonical = ifname::long(name);
    canonical.starts_with("Loopback") || canonical.starts_with("Null")
}

pub fn build(collected: &Collected, scope: &BTreeSet<String>) -> Addressing {
    let mut members: Vec<Member> = Vec::new();
    let mut claimed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();

    for device_id in scope {
        let Some(aged) = collected.interfaces.get(device_id) else {
            continue;
        };
        for interface in &aged.rows {
            if is_virtual_interface(&interface.name) {
                continue;
            }
            for cidr in interface.ipv4.iter().chain(interface.ipv6.iter()) {
                let Ok(info) = nocturne_tools::subnet::info(cidr) else {
                    continue;
                };
                // A /32 or /128 is an address, not a shared segment.
                if (info.version == 4 && info.prefix >= 32)
                    || (info.version == 6 && info.prefix >= 128)
                {
                    continue;
                }
                if info.scope.contains("link-local") {
                    continue;
                }
                let Some(ip) = host_of(cidr) else { continue };
                claimed
                    .entry(ip.clone())
                    .or_default()
                    .insert(device_id.clone());
                members.push(Member {
                    device_id: device_id.clone(),
                    interface: ifname::long(&interface.name),
                    ip,
                    network: info.cidr,
                    status: Status::from_flag(interface.oper_up),
                });
            }
        }
    }

    let mut warnings: BTreeSet<String> = BTreeSet::new();
    let shared: BTreeSet<String> = claimed
        .iter()
        .filter(|(_, devices)| devices.len() > 1)
        .map(|(ip, _)| ip.clone())
        .collect();
    for ip in &shared {
        let names: Vec<String> = claimed[ip]
            .iter()
            .map(|device_id| collected.name_of(device_id))
            .collect();
        warnings.insert(format!(
            "{ip} is configured on {} — treated as a shared virtual address (HSRP/VRRP) and not \
             drawn as a link",
            names.join(" and ")
        ));
    }
    members.retain(|member| !shared.contains(&member.ip));

    let mut by_network: BTreeMap<String, Vec<Member>> = BTreeMap::new();
    for member in members {
        by_network
            .entry(member.network.clone())
            .or_default()
            .push(member);
    }

    let mut links = Vec::new();
    let mut groups = Vec::new();
    for (network, mut in_network) in by_network {
        in_network.sort_by(|a, b| {
            (&a.device_id, ifname::sort_key(&a.interface))
                .cmp(&(&b.device_id, ifname::sort_key(&b.interface)))
        });
        let devices: BTreeSet<&String> =
            in_network.iter().map(|member| &member.device_id).collect();
        if devices.len() < 2 {
            continue;
        }
        if in_network.len() == 2 {
            let (one, other) = (&in_network[0], &in_network[1]);
            let status = match (one.status, other.status) {
                (Status::Down, _) | (_, Status::Down) => Status::Down,
                (Status::Up, _) | (_, Status::Up) => Status::Up,
                _ => Status::Unknown,
            };
            let mut link = Link {
                a: End {
                    node: one.device_id.clone(),
                    interface: Some(one.interface.clone()),
                    ip: Some(one.ip.clone()),
                },
                b: End {
                    node: other.device_id.clone(),
                    interface: Some(other.interface.clone()),
                    ip: Some(other.ip.clone()),
                },
                kind: LinkKind::Subnet,
                confidence: Confidence::Inferred,
                status,
                label: Some(network.clone()),
                ..Link::default()
            };
            link.id = crate::id_of(&link.signature());
            links.push(link);
            continue;
        }
        groups.push(Group {
            id: format!("subnet:{network}"),
            kind: GroupKind::Subnet,
            label: format!("{network} ({} devices)", devices.len()),
            nodes: devices.into_iter().cloned().collect(),
        });
    }

    let mut warnings: Vec<String> = warnings.into_iter().collect();
    if links.is_empty() && groups.is_empty() {
        let without = scope
            .iter()
            .filter(|device_id| {
                collected.interfaces.get(*device_id).is_none_or(|aged| {
                    aged.rows
                        .iter()
                        .all(|interface| interface.ipv4.is_empty() && interface.ipv6.is_empty())
                })
            })
            .count();
        if without > 0 {
            warnings.push(format!(
                "no interface addresses have been collected from {without} of the {} devices in                  this view, so there is nothing to join them by. Collect `interfaces` from them",
                scope.len()
            ));
        }
    }

    Addressing {
        links,
        groups,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopbacks_are_not_cables() {
        assert!(is_virtual_interface("Lo0"));
        assert!(is_virtual_interface("Loopback1"));
        assert!(is_virtual_interface("Null0"));
        assert!(!is_virtual_interface("Vlan10"));
        assert!(!is_virtual_interface("Gi0/1"));
    }
}
