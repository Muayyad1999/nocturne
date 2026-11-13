//! `labs.twin`: a lab built from what Nocturne has collected from real devices.
//!
//! Nodes come from collected facts and interfaces, links from collected
//! neighbours, and each node's startup configuration from the device's latest
//! backup — **as far as the simulator accepts it**. The result says, per node,
//! which model it was put on, which interfaces that model does not have, and
//! every configuration line the simulated device refused, so nobody has to
//! guess how faithful the twin is.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_app::jobs::{self, JobSpec};
use nocturne_app::model::Device;
use nocturne_app::{App, AppError, AppResult, Router};
use nocturne_drivers::model::{Facts, Interface, Kind, Neighbor};
use nocturne_netsim::{
    CableKind, CliKind, Endpoint, Lab, LabSpec, LinkSpec, ModelInfo, NodeSpec, catalog,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::api::RejectedLine;
use crate::model::{self, LabRecord};
use crate::{devices, runtime};

/// Interfaces that exist only in the configuration, not as ports.
fn is_virtual(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["vlan", "loopback", "port-channel", "tunnel", "bvi", "null"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

#[derive(Deserialize)]
struct TwinParams {
    #[serde(default)]
    device_ids: Vec<String>,
    #[serde(default)]
    name: Option<String>,
}

/// What became of one real device.
#[derive(Clone, Debug, Serialize)]
pub struct TwinNode {
    pub device_id: String,
    pub device: String,
    /// The node's name in the lab.
    pub node: String,
    /// The catalog model it was put on.
    pub model: String,
    /// Where the configuration came from: `running` or `startup`.
    pub config_source: String,
    pub config_lines: usize,
    pub accepted: usize,
    pub rejected: Vec<RejectedLine>,
    /// Real interfaces the chosen model has.
    pub interfaces_mapped: Vec<String>,
    /// Real interfaces no model in the catalog has: anything configured on
    /// them is lost.
    pub interfaces_unmapped: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TwinRefusal {
    pub device_id: String,
    pub device: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct TwinLink {
    pub a: String,
    pub a_interface: String,
    pub b: String,
    pub b_interface: String,
    /// Which neighbour table it came from.
    pub protocol: String,
}

struct Source {
    device: Device,
    facts: Option<Facts>,
    interfaces: Vec<Interface>,
    neighbors: Vec<Neighbor>,
    config: String,
    config_source: String,
}

fn gather(app: &App, device: Device) -> Result<Source, TwinRefusal> {
    let refuse = |reason: &str| TwinRefusal {
        device_id: device.id.clone(),
        device: device.name.clone(),
        reason: reason.to_string(),
    };
    if device.lab.is_some() {
        return Err(refuse(
            "this is already a simulated device; twin the real one instead",
        ));
    }
    let interfaces: Vec<Interface> =
        nocturne_app::collect::latest(app, &device.id, Kind::Interfaces)
            .map_err(|err| refuse(&err.message))?
            .map(|stored| stored.typed())
            .unwrap_or_default();
    if interfaces.is_empty() {
        return Err(refuse(
            "no interfaces have been collected from it; run `collect.run` on it first",
        ));
    }
    let (config, config_source) = match latest_config(app, &device.id) {
        Some(found) => found,
        None => {
            return Err(refuse(
                "it has no configuration backup; back it up first and try again",
            ));
        }
    };
    let facts = nocturne_app::collect::latest(app, &device.id, Kind::Facts)
        .map_err(|err| refuse(&err.message))?
        .and_then(|stored| stored.typed::<Facts>().into_iter().next());
    let neighbors: Vec<Neighbor> = nocturne_app::collect::latest(app, &device.id, Kind::Neighbors)
        .map_err(|err| refuse(&err.message))?
        .map(|stored| stored.typed())
        .unwrap_or_default();
    Ok(Source {
        device,
        facts,
        interfaces,
        neighbors,
        config,
        config_source,
    })
}

fn latest_config(app: &App, device_id: &str) -> Option<(String, String)> {
    for kind in ["running", "startup"] {
        if let Ok(Some(version)) = app.store().config_latest(device_id, kind)
            && let Ok((_, text)) = app.store().config_get(version.id)
            && !text.trim().is_empty()
        {
            return Some((text, kind.to_string()));
        }
    }
    None
}

/// Models that can run a configuration, best first for a tie.
fn simulated_models() -> Vec<ModelInfo> {
    catalog()
        .into_iter()
        .filter(|info| info.cli == CliKind::Ios)
        .collect()
}

/// The model whose ports cover most of the device's real ports.
fn choose_model(interfaces: &[Interface]) -> (ModelInfo, Vec<String>, Vec<String>) {
    let physical: Vec<String> = interfaces
        .iter()
        .map(|interface| interface.name.clone())
        .filter(|name| !is_virtual(name))
        .collect();
    let models = simulated_models();
    let score = |info: &ModelInfo| {
        physical
            .iter()
            .filter(|name| {
                info.interfaces
                    .iter()
                    .any(|port| port.eq_ignore_ascii_case(name))
            })
            .count()
    };
    // The catalog is in palette order, so on a tie the plainest model wins
    // (a router before a firewall, an access switch before a layer-3 one).
    let mut best: Option<&ModelInfo> = None;
    for info in &models {
        let better = match best {
            None => true,
            Some(current) => {
                let (theirs, ours) = (score(current), score(info));
                ours > theirs
                    || (ours == theirs && info.interfaces.len() < current.interfaces.len())
            }
        };
        if better {
            best = Some(info);
        }
    }
    let best = best.cloned().unwrap_or_else(|| models[0].clone());
    let (mapped, unmapped): (Vec<String>, Vec<String>) = physical.into_iter().partition(|name| {
        best.interfaces
            .iter()
            .any(|port| port.eq_ignore_ascii_case(name))
    });
    (best, mapped, unmapped)
}

/// A node name the simulator accepts, unique in the lab.
fn node_name(source: &Source, taken: &mut BTreeSet<String>) -> String {
    let raw = source
        .facts
        .as_ref()
        .map(|facts| facts.hostname.clone())
        .filter(|hostname| !hostname.is_empty())
        .unwrap_or_else(|| source.device.name.clone());
    let raw = raw.split('.').next().unwrap_or(&raw).to_string();
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .take(63)
        .collect();
    let base = if cleaned.is_empty() {
        "device".to_string()
    } else {
        cleaned
    };
    let mut name = base.clone();
    let mut n = 2;
    while taken.contains(&name.to_lowercase()) {
        name = format!("{base}-{n}");
        n += 1;
    }
    taken.insert(name.to_lowercase());
    name
}

/// Does this neighbour row point at another device in the twin?
fn match_node(neighbor: &Neighbor, names: &BTreeMap<String, String>) -> Option<String> {
    let device = neighbor
        .device
        .split('.')
        .next()
        .unwrap_or(&neighbor.device);
    names.get(&device.to_lowercase()).cloned()
}

fn build(sources: Vec<Source>, name: &str) -> AppResult<(LabSpec, Vec<TwinNode>, Vec<TwinLink>)> {
    let mut taken = BTreeSet::new();
    let mut nodes = Vec::new();
    let mut plan: Vec<(String, Source, ModelInfo, Vec<String>, Vec<String>)> = Vec::new();
    for source in sources {
        let node = node_name(&source, &mut taken);
        let (model, mapped, unmapped) = choose_model(&source.interfaces);
        plan.push((node, source, model, mapped, unmapped));
    }

    // Every name a neighbour row could use for one of our nodes.
    let mut aliases: BTreeMap<String, String> = BTreeMap::new();
    for (node, source, _, _, _) in &plan {
        aliases.insert(node.to_lowercase(), node.clone());
        aliases.insert(source.device.name.to_lowercase(), node.clone());
        if let Some(facts) = &source.facts
            && !facts.hostname.is_empty()
        {
            let short = facts
                .hostname
                .split('.')
                .next()
                .unwrap_or(&facts.hostname)
                .to_lowercase();
            aliases.insert(short, node.clone());
        }
    }

    for (index, (node, source, model, _, _)) in plan.iter().enumerate() {
        nodes.push(NodeSpec {
            name: node.clone(),
            model: model.id.clone(),
            x: 180.0 + (index % 4) as f32 * 240.0,
            y: 160.0 + (index / 4) as f32 * 200.0,
            startup_config: Some(source.config.clone()),
            ..NodeSpec::default()
        });
    }

    let mut links: Vec<TwinLink> = Vec::new();
    // An interface takes one cable: the first neighbour row that names it wins.
    let mut cabled: BTreeSet<String> = BTreeSet::new();
    for (node, source, model, _, _) in &plan {
        for neighbor in &source.neighbors {
            let Some(other) = match_node(neighbor, &aliases) else {
                continue;
            };
            if &other == node {
                continue;
            }
            let local = nocturne_drivers::ifname::long(&neighbor.local_interface);
            let remote = nocturne_drivers::ifname::long(&neighbor.remote_interface);
            let Some(other_entry) = plan.iter().find(|(name, ..)| name == &other) else {
                continue;
            };
            let has = |info: &ModelInfo, port: &str| {
                info.interfaces
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(port))
            };
            if !has(model, &local) || !has(&other_entry.2, &remote) {
                continue;
            }
            let (near, far) = (
                format!("{node}:{local}").to_lowercase(),
                format!("{other}:{remote}").to_lowercase(),
            );
            if cabled.contains(&near) || cabled.contains(&far) {
                continue;
            }
            cabled.insert(near);
            cabled.insert(far);
            links.push(TwinLink {
                a: node.clone(),
                a_interface: local,
                b: other,
                b_interface: remote,
                protocol: neighbor.protocol.clone(),
            });
        }
    }

    let spec = LabSpec {
        name: name.to_string(),
        description: format!(
            "Twin of {} device{} built from collected data and backups.",
            plan.len(),
            if plan.len() == 1 { "" } else { "s" }
        ),
        nodes,
        links: links
            .iter()
            .map(|link| LinkSpec {
                a: Endpoint {
                    node: link.a.clone(),
                    interface: link.a_interface.clone(),
                },
                b: Endpoint {
                    node: link.b.clone(),
                    interface: link.b_interface.clone(),
                },
                cable: CableKind::Auto,
            })
            .collect(),
        ..LabSpec::default()
    };

    // Boot it once to find out what the simulator made of each configuration.
    let sim = Lab::from_spec(&spec).map_err(model::sim_error)?;
    let mut report = Vec::new();
    for (node, source, chosen, mapped, unmapped) in plan {
        let id = sim.node_by_name(&node);
        let rejected: Vec<RejectedLine> = id
            .map(|id| sim.startup_errors(id))
            .unwrap_or_default()
            .into_iter()
            .map(|(line, error)| RejectedLine {
                line,
                error: error.trim().to_string(),
            })
            .collect();
        let total = source
            .config
            .lines()
            .filter(|line| !line.trim().is_empty() && !line.trim().starts_with('!'))
            .count();
        report.push(TwinNode {
            device_id: source.device.id.clone(),
            device: source.device.name.clone(),
            node,
            model: chosen.id.clone(),
            config_source: source.config_source.clone(),
            config_lines: total,
            accepted: total.saturating_sub(rejected.len()),
            rejected,
            interfaces_mapped: mapped,
            interfaces_unmapped: unmapped,
        });
    }
    Ok((spec, report, links))
}

pub fn init(router: &mut Router) {
    router.add("labs.twin", |app: App, params: TwinParams| async move {
        if params.device_ids.is_empty() {
            return Err(AppError::invalid("no devices given"));
        }
        let mut devices_wanted = Vec::new();
        for reference in &params.device_ids {
            let device = nocturne_app::inventory::find(&app, reference)?;
            if !devices_wanted.iter().any(|known: &Device| known.id == device.id) {
                devices_wanted.push(device);
            }
        }
        let wanted_name = params.name.clone();
        let ids: Vec<String> = devices_wanted.iter().map(|device| device.id.clone()).collect();
        let title = format!("Build a lab from {} devices", devices_wanted.len());
        let spec = JobSpec::new("labs.twin", title).devices(&ids);
        let job = jobs::start(&app, spec, move |ctx| async move {
            let app = ctx.app().clone();
            let mut sources = Vec::new();
            let mut refused = Vec::new();
            for device in devices_wanted {
                ctx.advance(format!("Reading what is known about {}", device.name));
                match gather(&app, device) {
                    Ok(source) => sources.push(source),
                    Err(refusal) => {
                        ctx.device(&refusal.device_id, "warn", format!("{}: {}", refusal.device, refusal.reason));
                        refused.push(refusal);
                    }
                }
            }
            if sources.is_empty() {
                return Err(AppError::invalid(
                    "none of these devices can be twinned yet: collect from them and back them up first",
                )
                .with_details(json!({"refused": refused})));
            }
            let name = model::unique_name(&app, wanted_name.as_deref().unwrap_or("Twin"))?;
            ctx.status("Building the lab");
            let (spec, nodes, links) = build(sources, &name)?;
            let mut record = LabRecord::new(spec);
            model::save(&app, &mut record)?;
            devices::sync(&app, &record)?;
            for node in &nodes {
                let level = if node.rejected.is_empty() { "info" } else { "warn" };
                ctx.device(
                    &node.device_id,
                    level,
                    format!(
                        "{} → {} ({}): {} of {} configuration lines accepted{}",
                        node.device,
                        node.node,
                        node.model,
                        node.accepted,
                        node.config_lines,
                        if node.interfaces_unmapped.is_empty() {
                            String::new()
                        } else {
                            format!(
                                ", {} interface(s) the model does not have",
                                node.interfaces_unmapped.len()
                            )
                        }
                    ),
                );
            }
            app.audit(
                "labs.twin",
                None,
                format!("Built lab `{}` from {} devices", record.name(), nodes.len()),
                json!({"lab_id": record.id}),
            );
            runtime::publish_changed(&app, &record.id, "created");
            Ok(json!({
                "lab_id": record.id,
                "name": record.name(),
                "nodes": nodes,
                "links": links,
                "refused": refused,
            }))
        });
        Ok(jobs::started(&job))
    });
}
