//! Putting the graph together: nodes from the inventory, links from every
//! source we have, and the honest bookkeeping about what was missing.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_app::error::AppResult;
use nocturne_app::util::now_ms;
use nocturne_app::{App, settings};
use nocturne_drivers::ifname;
use serde_json::{Value, json};

use crate::data::{self, Collected};
use crate::model::{
    Confidence, DataAge, End, Filter, Graph, Group, GroupKind, Layer, Link, LinkKind, Node,
    NodeKind, SETTINGS_SECTION, Settings, Stats, Status, View, compress_ranges,
};
use crate::{endpoints, l3, manual, physical, views};

/// What to draw.
#[derive(Clone, Debug)]
pub struct Options {
    pub view: String,
    pub layer: Layer,
    pub filter: Filter,
    pub include_endpoints: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            view: crate::model::DEFAULT_VIEW.to_string(),
            layer: Layer::Physical,
            filter: Filter::default(),
            include_endpoints: false,
        }
    }
}

impl Options {
    /// The options a saved view implies, with explicit parameters winning.
    pub fn from_view(view: &View) -> Options {
        Options {
            view: view.id.clone(),
            layer: view.layer,
            filter: view.filter.clone(),
            include_endpoints: view.include_endpoints,
        }
    }
}

/// Call another feature's RPC method, tolerating a build without it.
async fn optional_rpc(app: &App, method: &str, params: Value) -> Option<Value> {
    match app.rpc(method, params).await {
        Ok(value) => Some(value),
        Err(err) => {
            if err.code != "unknown_method" {
                tracing::debug!(method, code = err.code, "topology: {}", err.message);
            }
            None
        }
    }
}

/// Build the graph the options ask for.
pub async fn build(app: &App, options: &Options) -> AppResult<Graph> {
    let config: Settings = settings::get(app, SETTINGS_SECTION)?;
    // The MAC and ARP tables are only read when hosts are being drawn.
    let collected = Collected::load_with(app, options.include_endpoints)?;
    let scope: BTreeSet<String> = collected
        .devices
        .iter()
        .filter(|row| options.filter.matches(&row.device))
        .map(|row| row.device.id.clone())
        .collect();

    let alerts = open_alerts(app).await;
    let mut warnings: Vec<String> = Vec::new();
    let mut links: Vec<Link>;
    let mut groups: Vec<Group> = Vec::new();
    let mut nodes: Vec<Node> = Vec::new();
    let mut unmanaged: Vec<physical::Unmanaged> = Vec::new();
    let mut capabilities: BTreeMap<String, String> = BTreeMap::new();
    let mut network_ports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut endpoint_count = 0;

    match options.layer {
        Layer::Physical => {
            let reported = physical::build(&collected, &scope);
            warnings.extend(reported.warnings);
            capabilities = reported.capabilities;
            network_ports = reported.network_ports;
            links = reported.links;
            unmanaged = reported.unmanaged;

            if config.use_lab_links {
                let (lab_links, lab_warnings) = lab_links(app, &collected, &scope).await;
                if !lab_links.is_empty() {
                    let lab_pairs: BTreeSet<(String, String)> = lab_links
                        .iter()
                        .map(|link| pair_key(&link.a.node, &link.b.node))
                        .collect();
                    // The lab knows its own wiring; reported links between two
                    // lab devices it already describes are dropped as duplicates.
                    links.retain(|link| !lab_pairs.contains(&pair_key(&link.a.node, &link.b.node)));
                    links.extend(lab_links);
                }
                warnings.extend(lab_warnings);
            }

            let (manual_links, manual_nodes) = manual::links(app, &collected, &scope)?;
            links.extend(manual_links);
            nodes.extend(manual_nodes);

            if config.fold_port_channels {
                links = fold_aggregates(links, &collected);
            }
        }
        Layer::L3 => {
            let addressing = l3::build(&collected, &scope);
            warnings.extend(addressing.warnings);
            links = addressing.links;
            groups = addressing.groups;
        }
    }

    // Nodes for every device in scope.
    let stale_before = now_ms() - i64::from(config.stale_after_hours) * 3_600_000;
    let mut stale_devices = 0;
    let mut no_data: Vec<String> = Vec::new();
    let mut oldest: Option<i64> = None;
    let mut newest: Option<i64> = None;
    for row in &collected.devices {
        if !scope.contains(&row.device.id) {
            continue;
        }
        let device = &row.device;
        let collected_at = match options.layer {
            Layer::Physical => collected.neighbors.get(&device.id).map(|aged| aged.at),
            Layer::L3 => collected.interfaces.get(&device.id).map(|aged| aged.at),
        };
        match collected_at {
            Some(at) => {
                oldest = Some(oldest.map_or(at, |known: i64| known.min(at)));
                newest = Some(newest.map_or(at, |known: i64| known.max(at)));
            }
            None => no_data.push(device.name.clone()),
        }
        let stale = collected_at.is_some_and(|at| at < stale_before);
        if stale {
            stale_devices += 1;
        }
        let facts = collected
            .facts
            .get(&device.id)
            .and_then(|aged| aged.rows.first());
        nodes.push(Node {
            id: device.id.clone(),
            kind: device_kind(&collected, device, capabilities.get(&device.id)),
            device_id: Some(device.id.clone()),
            name: device.name.clone(),
            address: Some(device.address.clone()).filter(|address| !address.is_empty()),
            site: data::site_of(device),
            role: Some(device.role.clone()).filter(|role| !role.is_empty()),
            profile: Some(device.profile.clone()).filter(|profile| !profile.is_empty()),
            model: facts
                .map(|facts| facts.model.clone())
                .filter(|model| !model.is_empty()),
            lab: device.lab.is_some(),
            status: data::status_of(row),
            unmanaged: false,
            alerts: alerts.get(&device.id).copied(),
            collected_at,
            stale,
            ..Node::default()
        });
    }

    for neighbour in unmanaged {
        nodes.push(Node {
            id: neighbour.id.clone(),
            kind: neighbour.kind,
            name: neighbour.name.clone(),
            address: neighbour.address.clone(),
            model: Some(neighbour.platform.clone()).filter(|platform| !platform.is_empty()),
            status: Status::Unknown,
            unmanaged: true,
            ..Node::default()
        });
    }

    if options.include_endpoints && options.layer == Layer::Physical {
        let found = endpoints::build(
            &collected,
            &scope,
            &network_ports,
            config.endpoint_max_macs as usize,
        );
        endpoint_count = found.nodes.len();
        warnings.extend(found.warnings);
        nodes.extend(found.nodes);
        links.extend(found.links);
    }

    // Only keep links whose ends we actually have nodes for.
    let known: BTreeSet<String> = nodes.iter().map(|node| node.id.clone()).collect();
    links.retain(|link| known.contains(&link.a.node) && known.contains(&link.b.node));
    decorate(app, &mut links, &collected, &config).await;
    links.sort_by_key(Link::signature);
    links.dedup_by(|a, b| a.signature() == b.signature());

    // Groups: sites, then labs, then whatever the layer added.
    let mut site_groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut lab_groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in &nodes {
        if let Some(site) = &node.site {
            site_groups
                .entry(site.clone())
                .or_default()
                .push(node.id.clone());
        }
        if let Some(device_id) = &node.device_id
            && let Some(lab) = collected
                .device(device_id)
                .and_then(|row| row.device.lab.as_ref())
        {
            lab_groups
                .entry(lab.lab_id.clone())
                .or_default()
                .push(node.id.clone());
        }
    }
    let mut all_groups: Vec<Group> = site_groups
        .into_iter()
        .map(|(site, members)| Group {
            id: format!("site:{site}"),
            kind: GroupKind::Site,
            label: site,
            nodes: members,
        })
        .collect();
    all_groups.extend(lab_groups.into_iter().map(|(lab_id, members)| Group {
        id: format!("lab:{lab_id}"),
        kind: GroupKind::Lab,
        label: format!("Lab {lab_id}"),
        nodes: members,
    }));
    all_groups.append(&mut groups);

    if stale_devices > 0 {
        let names: Vec<&str> = nodes
            .iter()
            .filter(|node| node.stale)
            .map(|node| node.name.as_str())
            .collect();
        warnings.push(format!(
            "{stale_devices} device(s) have not been collected from for more than {} hours and \
             may be drawn out of date: {}",
            config.stale_after_hours,
            summarise(&names)
        ));
    }
    if !no_data.is_empty() {
        let kind = match options.layer {
            Layer::Physical => "neighbour",
            Layer::L3 => "interface",
        };
        let names: Vec<&str> = no_data.iter().map(String::as_str).collect();
        warnings.push(format!(
            "no {kind} data has been collected from {} device(s), so anything connected only to \
             them is missing: {}",
            no_data.len(),
            summarise(&names)
        ));
    }

    nodes.sort_by_key(|node| crate::graph_sort_key(&node.name));
    nodes.dedup_by(|a, b| a.id == b.id);
    let unmanaged_count = nodes.iter().filter(|node| node.unmanaged).count() - endpoint_count;
    let device_count = nodes.iter().filter(|node| node.device_id.is_some()).count();

    let mut graph = Graph {
        stats: Stats {
            devices: device_count,
            links: links.len(),
            unmanaged: unmanaged_count,
            stale_devices,
            endpoints: endpoint_count,
        },
        nodes,
        links,
        groups: all_groups,
        warnings,
        generated_at: now_ms(),
        data_age: DataAge {
            oldest_at: oldest,
            newest_at: newest,
        },
        layer: options.layer,
        view: options.view.clone(),
    };
    views::apply_positions(app, &options.view, &mut graph)?;
    Ok(graph)
}

fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// At most four names, then "and N more".
fn summarise(names: &[&str]) -> String {
    if names.len() <= 4 {
        return names.join(", ");
    }
    format!("{}, and {} more", names[..4].join(", "), names.len() - 4)
}

/// What a managed device is: its role first, then what its neighbours say
/// about it, then its platform.
fn device_kind(
    collected: &Collected,
    device: &nocturne_app::model::Device,
    capabilities: Option<&String>,
) -> NodeKind {
    let role = device.role.to_ascii_lowercase();
    let routes_beyond_connected = collected.routes.get(&device.id).is_some_and(|aged| {
        aged.rows.iter().any(|route| {
            let protocol = route.protocol.to_ascii_lowercase();
            !protocol.is_empty() && protocol != "connected" && protocol != "local"
        })
    });
    let by_role = match role.as_str() {
        "router" | "wan" | "edge-router" => Some(NodeKind::Router),
        "firewall" | "security" => Some(NodeKind::Firewall),
        "wireless" | "ap" | "access-point" | "wifi" => Some(NodeKind::AccessPoint),
        "server" | "hypervisor" => Some(NodeKind::Server),
        "host" | "endpoint" => Some(NodeKind::Host),
        "internet" | "isp" | "cloud" => Some(NodeKind::Cloud),
        "core" | "distribution" | "dist" | "aggregation" => Some(if routes_beyond_connected {
            NodeKind::L3Switch
        } else {
            NodeKind::Switch
        }),
        "access" | "edge" => Some(NodeKind::Switch),
        _ => None,
    };
    if let Some(kind) = by_role {
        return kind;
    }
    let platform = collected
        .facts
        .get(&device.id)
        .and_then(|aged| aged.rows.first())
        .map(|facts| format!("{} {}", facts.vendor, facts.model))
        .unwrap_or_default();
    match capabilities {
        Some(capabilities) => physical::kind_from_hints(capabilities, &platform),
        None => {
            let kind = physical::kind_from_platform(&platform);
            if kind == NodeKind::Unknown && routes_beyond_connected {
                NodeKind::Router
            } else {
                kind
            }
        }
    }
}

/// Ask the lab feature for the wiring it owns.
async fn lab_links(
    app: &App,
    collected: &Collected,
    scope: &BTreeSet<String>,
) -> (Vec<Link>, Vec<String>) {
    let Some(rows) = optional_rpc(app, "labs.links", json!({})).await else {
        return (Vec::new(), Vec::new());
    };
    let Some(rows) = rows.as_array() else {
        return (Vec::new(), Vec::new());
    };
    let mut links = Vec::new();
    let mut warnings = BTreeSet::new();
    for row in rows {
        let end = |side: &str| -> Option<(String, Option<String>)> {
            let value = row.get(side)?;
            let device_id = value.get("device_id")?.as_str()?.to_string();
            let interface = value
                .get("interface")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(ifname::long);
            Some((device_id, interface))
        };
        let (Some((a_id, a_if)), Some((b_id, b_if))) = (end("a"), end("b")) else {
            if let Some(lab) = row.get("lab").and_then(Value::as_str) {
                warnings.insert(format!(
                    "lab `{lab}` has a link to a node that is not in the inventory, so it is not drawn"
                ));
            }
            continue;
        };
        if !scope.contains(&a_id) || !scope.contains(&b_id) {
            continue;
        }
        let up = row.get("up").and_then(Value::as_bool);
        let running = row.get("running").and_then(Value::as_bool).unwrap_or(false);
        let mut ends = [
            End::with_interface(a_id, a_if),
            End::with_interface(b_id, b_if),
        ];
        ends.sort_by(|a, b| {
            (&a.node, a.interface.as_deref().unwrap_or(""))
                .cmp(&(&b.node, b.interface.as_deref().unwrap_or("")))
        });
        let [a, b] = ends;
        let mut link = Link {
            a,
            b,
            kind: LinkKind::Lab,
            confidence: Confidence::Confirmed,
            status: if running {
                Status::from_flag(up)
            } else {
                Status::Unknown
            },
            label: row
                .get("lab")
                .and_then(Value::as_str)
                .map(|lab| format!("lab {lab}")),
            ..Link::default()
        };
        link.id = crate::id_of(&link.signature());
        links.push(link);
    }
    let _ = collected;
    (links, warnings.into_iter().collect())
}

/// Interfaces of a device that are link aggregates.
fn aggregate_interfaces(collected: &Collected, device_id: &str) -> Vec<String> {
    let Some(aged) = collected.interfaces.get(device_id) else {
        return Vec::new();
    };
    aged.rows
        .iter()
        .map(|interface| ifname::long(&interface.name))
        .filter(|name| {
            name.starts_with("Port-channel")
                || name.starts_with("Bundle-Ether")
                || name.starts_with("bond")
                || name.starts_with("ae")
        })
        .collect()
}

/// Fold the members of a port-channel into one link.
///
/// Only when it can be justified: two or more parallel links between the same
/// pair of managed devices, and exactly one aggregate interface at each end
/// that is not itself a link. The collected model carries no membership list,
/// so anything less certain is left as separate links.
fn fold_aggregates(links: Vec<Link>, collected: &Collected) -> Vec<Link> {
    let used: BTreeSet<(String, String)> = links
        .iter()
        .flat_map(|link| [&link.a, &link.b])
        .filter_map(|end| {
            end.interface
                .as_ref()
                .map(|interface| (end.node.clone(), interface.clone()))
        })
        .collect();
    let mut by_pair: BTreeMap<(String, String), Vec<Link>> = BTreeMap::new();
    let mut kept: Vec<Link> = Vec::new();
    for link in links {
        if matches!(link.kind, LinkKind::Lldp | LinkKind::Cdp | LinkKind::Mndp)
            && link.a.node != link.b.node
            && link.a.interface.is_some()
            && link.b.interface.is_some()
        {
            by_pair
                .entry(pair_key(&link.a.node, &link.b.node))
                .or_default()
                .push(link);
        } else {
            kept.push(link);
        }
    }
    for ((a_node, b_node), members) in by_pair {
        if members.len() < 2 {
            kept.extend(members);
            continue;
        }
        let free = |node: &str| -> Option<String> {
            let candidates: Vec<String> = aggregate_interfaces(collected, node)
                .into_iter()
                .filter(|interface| !used.contains(&(node.to_string(), interface.clone())))
                .collect();
            match candidates.as_slice() {
                [only] => Some(only.clone()),
                _ => None,
            }
        };
        let (Some(a_aggregate), Some(b_aggregate)) = (free(&a_node), free(&b_node)) else {
            kept.extend(members);
            continue;
        };
        let count = members.len();
        let mut member_labels: Vec<String> = members
            .iter()
            .map(|link| {
                let (one, other) = if link.a.node == a_node {
                    (&link.a, &link.b)
                } else {
                    (&link.b, &link.a)
                };
                format!(
                    "{} ↔ {}",
                    ifname::short(one.interface.as_deref().unwrap_or("")),
                    ifname::short(other.interface.as_deref().unwrap_or(""))
                )
            })
            .collect();
        member_labels.sort();
        let confidence = members
            .iter()
            .map(|link| link.confidence)
            .max()
            .unwrap_or(Confidence::OneSided);
        let status = if members.iter().any(|link| link.status == Status::Up) {
            Status::Up
        } else if members.iter().all(|link| link.status == Status::Down) {
            Status::Down
        } else {
            Status::Unknown
        };
        let mut protocols: Vec<String> = members
            .iter()
            .flat_map(|link| link.protocols.clone())
            .collect();
        protocols.sort();
        protocols.dedup();
        let mut folded = Link {
            a: End::with_interface(a_node.clone(), Some(a_aggregate.clone())),
            b: End::with_interface(b_node.clone(), Some(b_aggregate.clone())),
            kind: members[0].kind,
            confidence,
            status,
            aggregate: Some(format!(
                "{} \u{2194} {} ({count} members)",
                ifname::short(&a_aggregate),
                ifname::short(&b_aggregate)
            )),
            members: member_labels,
            protocols,
            ..Link::default()
        };
        folded.id = crate::id_of(&folded.signature());
        kept.push(folded);
    }
    kept
}

/// Fill in what the collected interfaces (and the monitor, when it is in the
/// build) know about each link.
async fn decorate(app: &App, links: &mut [Link], collected: &Collected, config: &Settings) {
    let utilisation = if config.use_monitor_utilisation {
        monitor_utilisation(app).await
    } else {
        BTreeMap::new()
    };
    for link in links.iter_mut() {
        let mut speeds: Vec<u64> = Vec::new();
        let mut states: Vec<Option<bool>> = Vec::new();
        let mut vlans: Option<String> = None;
        let mut util: Option<f64> = None;
        for end in [&link.a, &link.b] {
            let Some(name) = &end.interface else { continue };
            if !collected.interfaces.contains_key(&end.node) {
                continue;
            }
            if let Some(interface) = collected.interface(&end.node, name) {
                if let Some(speed) = interface.speed_bps.filter(|speed| *speed > 0) {
                    speeds.push(speed);
                }
                states.push(interface.oper_up);
                if vlans.is_none() {
                    vlans = vlans_of(collected, &end.node, name, interface);
                }
            }
            if let Some(value) = utilisation.get(&(end.node.clone(), name.clone())) {
                util = Some(util.map_or(*value, |known: f64| known.max(*value)));
            }
        }
        if link.speed_bps.is_none() && !speeds.is_empty() {
            link.speed_bps = Some(if link.aggregate.is_some() {
                speeds.iter().sum()
            } else {
                speeds.into_iter().min().unwrap_or_default()
            });
        }
        if link.util_pct.is_none() {
            link.util_pct = util;
        }
        if link.vlans.is_none() {
            link.vlans = vlans;
        }
        if link.status == Status::Unknown && !states.is_empty() {
            link.status = if states.contains(&Some(false)) {
                Status::Down
            } else if states.contains(&Some(true)) {
                Status::Up
            } else {
                Status::Unknown
            };
        }
    }
}

/// The VLANs an interface carries, from the collected VLAN table, falling back
/// to the interface's own access VLAN.
fn vlans_of(
    collected: &Collected,
    device_id: &str,
    name: &str,
    interface: &nocturne_drivers::model::Interface,
) -> Option<String> {
    if let Some(aged) = collected.vlans.get(device_id) {
        let ids: Vec<u64> = aged
            .rows
            .iter()
            .filter(|vlan| {
                vlan.interfaces
                    .iter()
                    .any(|member| ifname::same(member, name))
            })
            .filter_map(|vlan| vlan.id)
            .collect();
        if !ids.is_empty() {
            return Some(compress_ranges(&ids));
        }
    }
    interface.vlan.map(|vlan| vlan.to_string())
}

/// How many alerts are open against each device, when the monitor feature is
/// in the build. Absent is "nobody is watching", not "none".
async fn open_alerts(app: &App) -> BTreeMap<String, u32> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    let Some(rows) =
        optional_rpc(app, "alerts.list", json!({"state": "open", "limit": 2000})).await
    else {
        return counts;
    };
    let rows = match rows.get("rows").and_then(Value::as_array) {
        Some(rows) => rows.clone(),
        None => rows.as_array().cloned().unwrap_or_default(),
    };
    for row in rows {
        if let Some(device_id) = row.get("device_id").and_then(Value::as_str) {
            *counts.entry(device_id.to_string()).or_default() += 1;
        }
    }
    counts
}

/// `monitor.interfaces`, when the monitor feature is in the build.
async fn monitor_utilisation(app: &App) -> BTreeMap<(String, String), f64> {
    let mut by_interface = BTreeMap::new();
    let Some(rows) = optional_rpc(app, "monitor.interfaces", json!({"limit": 5000})).await else {
        return by_interface;
    };
    let rows = match rows.get("rows").and_then(Value::as_array) {
        Some(rows) => rows.clone(),
        None => rows.as_array().cloned().unwrap_or_default(),
    };
    for row in rows {
        let (Some(device_id), Some(name), Some(util)) = (
            row.get("device_id").and_then(Value::as_str),
            row.get("name").and_then(Value::as_str),
            row.get("util_pct").and_then(Value::as_f64),
        ) else {
            continue;
        };
        by_interface.insert((device_id.to_string(), ifname::long(name)), util);
    }
    by_interface
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lists_of_names_are_summarised() {
        assert_eq!(summarise(&["a", "b"]), "a, b");
        assert_eq!(
            summarise(&["a", "b", "c", "d", "e", "f"]),
            "a, b, c, d, and 2 more"
        );
    }
}
