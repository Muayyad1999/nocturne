//! The cable layer: LLDP, CDP and MNDP reports turned into links.
//!
//! Every neighbour row is one *observation* — "my port X sees Y on its port Z".
//! Two observations that mirror each other are one cable, reported by both
//! ends (`confirmed`). An observation nobody answered is still a cable, but we
//! only have one side's word for it (`one_sided`).

use std::collections::{BTreeMap, BTreeSet};

use nocturne_drivers::ifname;

use crate::data::{Collected, Matched};
use crate::model::{Confidence, End, Link, LinkKind, NodeKind};

/// A neighbour that is not in the inventory.
#[derive(Clone, Debug, Default)]
pub struct Unmanaged {
    pub id: String,
    pub name: String,
    pub address: Option<String>,
    pub capabilities: String,
    pub platform: String,
    pub kind: NodeKind,
}

/// "My port X sees Y on its port Z", as one device told us.
#[derive(Clone, Debug)]
struct Observation {
    from: String,
    from_if: Option<String>,
    to: String,
    to_if: Option<String>,
    protocols: Vec<LinkKind>,
    at: i64,
}

impl Observation {
    fn mirrors(&self, other: &Observation) -> bool {
        self.to == other.from
            && self.from == other.to
            && self.to_if.is_some()
            && self.to_if == other.from_if
            && self.from_if.is_some()
            && self.from_if == other.to_if
    }

    /// A looser match for a report that did not name the remote port.
    fn loosely_mirrors(&self, other: &Observation) -> bool {
        if self.to != other.from || self.from != other.to {
            return false;
        }
        let ports_agree = |mine: &Option<String>, theirs: &Option<String>| match (mine, theirs) {
            (Some(mine), Some(theirs)) => mine == theirs,
            _ => true,
        };
        ports_agree(&self.to_if, &other.from_if) && ports_agree(&self.from_if, &other.to_if)
    }
}

/// What the neighbour protocols say, as links plus the nodes they invented.
pub struct Reported {
    pub links: Vec<Link>,
    pub unmanaged: Vec<Unmanaged>,
    pub warnings: Vec<String>,
    /// Capabilities other devices reported about a node, for guessing its kind.
    pub capabilities: BTreeMap<String, String>,
    /// Interfaces of a device that face another network device.
    pub network_ports: BTreeMap<String, BTreeSet<String>>,
}

fn canonical(name: &str) -> Option<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(ifname::long(trimmed))
}

/// Read every device's neighbour table and work out the cables.
///
/// `scope` is the set of devices the view covers; a link to a managed device
/// outside it is left out, an unmanaged neighbour is always kept.
pub fn build(collected: &Collected, scope: &BTreeSet<String>) -> Reported {
    let mut observations: Vec<Observation> = Vec::new();
    let mut unmanaged: BTreeMap<String, Unmanaged> = BTreeMap::new();
    let mut capabilities: BTreeMap<String, String> = BTreeMap::new();
    let mut warnings: BTreeSet<String> = BTreeSet::new();

    for device_id in scope {
        let Some(aged) = collected.neighbors.get(device_id) else {
            continue;
        };
        for neighbor in &aged.rows {
            let protocol = LinkKind::from_protocol(&neighbor.protocol).unwrap_or(LinkKind::Unknown);
            let from_if = canonical(&neighbor.local_interface);
            let to_if = canonical(&neighbor.remote_interface);
            let (to, remote_name) = match collected
                .index()
                .resolve(&neighbor.device, &neighbor.address)
            {
                Matched::Device { device_id, .. } => (device_id, None),
                Matched::Unmanaged { key, name } => (key.clone(), Some((key, name))),
                Matched::Ambiguous { name } => {
                    warnings.insert(format!(
                        "{} reports a neighbour called `{name}` on {}, and more than one device \
                         answers to that name — the link is left out rather than drawn wrongly",
                        collected.name_of(device_id),
                        from_if.clone().unwrap_or_else(|| "an unnamed port".into())
                    ));
                    continue;
                }
                Matched::Nothing => {
                    warnings.insert(format!(
                        "{} reports a neighbour with neither a name nor an address on {}",
                        collected.name_of(device_id),
                        from_if.clone().unwrap_or_else(|| "an unnamed port".into())
                    ));
                    continue;
                }
            };
            if let Some((key, name)) = remote_name {
                let entry = unmanaged.entry(key.clone()).or_insert_with(|| Unmanaged {
                    id: key,
                    name,
                    ..Unmanaged::default()
                });
                if entry.address.is_none() && !neighbor.address.trim().is_empty() {
                    entry.address = Some(neighbor.address.trim().to_string());
                }
                if entry.platform.is_empty() {
                    entry.platform = neighbor.platform.trim().to_string();
                }
                if entry.capabilities.is_empty() {
                    entry.capabilities = neighbor.capabilities.trim().to_string();
                }
            } else if !neighbor.capabilities.trim().is_empty() {
                capabilities
                    .entry(to.clone())
                    .or_insert_with(|| neighbor.capabilities.trim().to_string());
            }
            // A managed neighbour outside the view is simply not drawn.
            if !to.starts_with("u:") && !scope.contains(&to) {
                continue;
            }
            merge_observation(
                &mut observations,
                Observation {
                    from: device_id.clone(),
                    from_if,
                    to,
                    to_if,
                    protocols: if protocol == LinkKind::Unknown {
                        Vec::new()
                    } else {
                        vec![protocol]
                    },
                    at: aged.at,
                },
            );
        }
    }

    for entry in unmanaged.values_mut() {
        entry.kind = kind_from_hints(&entry.capabilities, &entry.platform);
    }

    let links = pair(&observations);
    let mut network_ports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for link in &links {
        for end in [&link.a, &link.b] {
            if let Some(interface) = &end.interface {
                network_ports
                    .entry(end.node.clone())
                    .or_default()
                    .insert(interface.clone());
            }
        }
    }

    Reported {
        links,
        unmanaged: unmanaged.into_values().collect(),
        warnings: warnings.into_iter().collect(),
        capabilities,
        network_ports,
    }
}

/// The same cable reported twice from one end (LLDP *and* CDP) is one
/// observation with two protocols.
fn merge_observation(observations: &mut Vec<Observation>, new: Observation) {
    if let Some(existing) = observations.iter_mut().find(|existing| {
        existing.from == new.from
            && existing.from_if == new.from_if
            && existing.to == new.to
            && existing.to_if == new.to_if
    }) {
        for protocol in new.protocols {
            if !existing.protocols.contains(&protocol) {
                existing.protocols.push(protocol);
            }
        }
        existing.at = existing.at.max(new.at);
        return;
    }
    // A report that names the remote port supersedes one from the same port
    // that did not (some platforms answer LLDP with only a chassis id).
    if new.to_if.is_some()
        && let Some(vague) = observations.iter_mut().find(|existing| {
            existing.from == new.from
                && existing.from_if == new.from_if
                && existing.to == new.to
                && existing.to_if.is_none()
        })
    {
        vague.to_if = new.to_if;
        for protocol in new.protocols {
            if !vague.protocols.contains(&protocol) {
                vague.protocols.push(protocol);
            }
        }
        vague.at = vague.at.max(new.at);
        return;
    }
    if new.to_if.is_none()
        && observations.iter().any(|existing| {
            existing.from == new.from
                && existing.from_if == new.from_if
                && existing.to == new.to
                && existing.to_if.is_some()
        })
    {
        return;
    }
    observations.push(new);
}

/// Turn observations into links, pairing the ones that mirror each other.
fn pair(observations: &[Observation]) -> Vec<Link> {
    let mut used = vec![false; observations.len()];
    let mut links: Vec<Link> = Vec::new();

    // Exact mirrors first, so parallel links between one pair of devices are
    // matched port to port before anything vaguer is considered.
    for index in 0..observations.len() {
        if used[index] {
            continue;
        }
        let one = &observations[index];
        let Some(other) = (index + 1..observations.len())
            .find(|&other| !used[other] && one.mirrors(&observations[other]))
        else {
            continue;
        };
        used[index] = true;
        used[other] = true;
        links.push(link_of(one, Some(&observations[other])));
    }
    for index in 0..observations.len() {
        if used[index] {
            continue;
        }
        let one = &observations[index];
        match (index + 1..observations.len())
            .find(|&other| !used[other] && one.loosely_mirrors(&observations[other]))
        {
            Some(other) => {
                used[index] = true;
                used[other] = true;
                links.push(link_of(one, Some(&observations[other])));
            }
            None => {
                used[index] = true;
                links.push(link_of(one, None));
            }
        }
    }
    links.sort_by_key(Link::signature);
    links
}

fn link_of(one: &Observation, other: Option<&Observation>) -> Link {
    let mut protocols: Vec<LinkKind> = one.protocols.clone();
    if let Some(other) = other {
        for protocol in &other.protocols {
            if !protocols.contains(protocol) {
                protocols.push(*protocol);
            }
        }
    }
    protocols.sort_unstable();
    let kind = protocols.first().copied().unwrap_or(LinkKind::Unknown);
    // The other end's view fills in a port this end did not name.
    let one_to_if = one
        .to_if
        .clone()
        .or_else(|| other.and_then(|other| other.from_if.clone()));
    let one_from_if = one
        .from_if
        .clone()
        .or_else(|| other.and_then(|other| other.to_if.clone()));
    let mut ends = [
        End::with_interface(one.from.clone(), one_from_if),
        End::with_interface(one.to.clone(), one_to_if),
    ];
    ends.sort_by(|a, b| {
        (&a.node, a.interface.as_deref().unwrap_or(""))
            .cmp(&(&b.node, b.interface.as_deref().unwrap_or("")))
    });
    let [a, b] = ends;
    let mut link = Link {
        a,
        b,
        kind,
        confidence: if other.is_some() {
            Confidence::Confirmed
        } else {
            Confidence::OneSided
        },
        protocols: protocols
            .iter()
            .map(|protocol| protocol.as_str().to_string())
            .collect(),
        ..Link::default()
    };
    link.id = crate::id_of(&link.signature());
    link
}

/// What a device is, from the capability and platform strings LLDP and CDP
/// carry. Nothing is guessed from a name.
pub fn kind_from_hints(capabilities: &str, platform: &str) -> NodeKind {
    let caps = capabilities.to_ascii_lowercase();
    let has = |needle: &str| caps.contains(needle);
    // Both protocols also answer in single letters (`B,R` / `R S I`). `S` is
    // Switch in CDP and Station in LLDP; CDP's reading wins because the letter
    // form is almost always CDP's.
    let letters: Vec<&str> = caps
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() == 1)
        .collect();
    let letter = |wanted: &str| letters.contains(&wanted);
    let wireless = has("wlan") || has("access point") || has("access-point") || has("wireless");
    let phone = has("phone") || has("telephone") || letter("p");
    let router = has("router") || letter("r");
    let bridge = has("bridge") || has("switch") || letter("b") || letter("s") || letter("t");
    if wireless || letter("w") {
        return NodeKind::AccessPoint;
    }
    if phone {
        return NodeKind::Host;
    }
    match (router, bridge) {
        (true, true) => NodeKind::L3Switch,
        (true, false) => NodeKind::Router,
        (false, true) => NodeKind::Switch,
        (false, false) => {
            if has("host") || has("station") || letter("h") {
                NodeKind::Host
            } else {
                kind_from_platform(platform)
            }
        }
    }
}

/// A last resort: the platform string the protocol carried.
pub fn kind_from_platform(platform: &str) -> NodeKind {
    let text = platform.to_ascii_lowercase();
    let any = |needles: &[&str]| needles.iter().any(|needle| text.contains(needle));
    if text.is_empty() {
        return NodeKind::Unknown;
    }
    if any(&["air-", "aironet", "unifi", "uap", "aruba ap", "meraki mr"]) {
        return NodeKind::AccessPoint;
    }
    if any(&[
        "asa",
        "firepower",
        "palo alto",
        "fortigate",
        "pfsense",
        "srx",
    ]) {
        return NodeKind::Firewall;
    }
    if any(&["ip phone", "voip"]) {
        return NodeKind::Host;
    }
    if any(&[
        "catalyst", "ws-c", "nexus", "procurve", "ex4", "ex2", "onv", "mes",
    ]) {
        return NodeKind::Switch;
    }
    if any(&[
        "isr",
        "asr",
        "routerboard",
        "ccr",
        "mx",
        "edgerouter",
        "vyos",
    ]) {
        return NodeKind::Router;
    }
    NodeKind::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_name_the_kind() {
        assert_eq!(
            kind_from_hints("Router Switch IGMP", ""),
            NodeKind::L3Switch
        );
        assert_eq!(kind_from_hints("Switch IGMP", ""), NodeKind::Switch);
        assert_eq!(kind_from_hints("Router", ""), NodeKind::Router);
        assert_eq!(
            kind_from_hints("Bridge, WLAN Access Point", ""),
            NodeKind::AccessPoint
        );
        assert_eq!(kind_from_hints("Host", ""), NodeKind::Host);
        assert_eq!(
            kind_from_hints("", "cisco AIR-CAP2702I"),
            NodeKind::AccessPoint
        );
        assert_eq!(kind_from_hints("", "WS-C2960-24TT-L"), NodeKind::Switch);
        assert_eq!(kind_from_hints("", ""), NodeKind::Unknown);
    }
}
