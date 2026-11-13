//! The `labs.*` RPC surface (`docs/dev/features/lab.md`).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use nocturne_app::{App, AppBuilder, AppError, AppResult, NoParams};
use nocturne_netsim::{
    CableKind, CliKind, Fault, FlowSpec, HostConfig, Lab, LabSpec, LinkView, NodeId, NodeView,
    NoteSpec, PduKind, ProbeResult, catalog, demo_campus, examples,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{self, ExampleInfo, LabRecord, LabSummary, LabView};
use crate::runtime;
use crate::{devices, expose, twin};

// ------------------------------------------------------------------ helpers

fn node_of(sim: &Lab, name: &str) -> AppResult<NodeId> {
    sim.node_by_name(name.trim())
        .ok_or_else(|| AppError::not_found(format!("node `{name}` in this lab")))
}

/// Read the lab whether it runs or not: a stopped lab is loaded into a
/// throw-away simulator, which shows it exactly as a start would.
fn read<T>(app: &App, id: &str, look: impl FnOnce(&mut Lab) -> AppResult<T>) -> AppResult<T> {
    match runtime::running(app, id) {
        Some(lab) => lab.with(look),
        None => {
            let record = model::load(app, id)?;
            let mut sim = Lab::from_spec(&record.spec).map_err(model::sim_error)?;
            look(&mut sim)
        }
    }
}

fn view(app: &App, id: &str) -> AppResult<LabView> {
    let record = model::load(app, id)?;
    let running = runtime::running(app, id);
    let (mut snapshot, sim_time, unsaved, speed, capture) = match &running {
        Some(lab) => {
            let (snapshot, sim_time) = lab.with(|sim| (sim.snapshot(), sim.now()));
            (
                snapshot,
                sim_time,
                lab.unsaved_nodes(),
                lab.speed(),
                lab.capture(),
            )
        }
        None => {
            let sim = Lab::from_spec(&record.spec).map_err(model::sim_error)?;
            (
                sim.snapshot(),
                0,
                Vec::new(),
                model::settings(app).default_speed,
                false,
            )
        }
    };
    snapshot.name = record.spec.name.clone();
    snapshot.description = record.spec.description.clone();
    Ok(LabView {
        id: record.id.clone(),
        name: record.spec.name.clone(),
        description: record.spec.description.clone(),
        running: running.is_some(),
        speed,
        sim_time,
        snapshot,
        devices: devices::device_ids(app, id)?,
        unsaved_nodes: unsaved,
        capture,
    })
}

fn node_view(sim: &Lab, node: NodeId) -> AppResult<NodeView> {
    sim.snapshot()
        .nodes
        .into_iter()
        .find(|view| view.id == node)
        .ok_or_else(|| AppError::internal("the node disappeared while it was being added"))
}

/// Store a new lab, create its devices and announce it.
pub(crate) fn create_record(app: &App, spec: LabSpec, action: &str) -> AppResult<LabView> {
    let mut record = LabRecord::new(spec);
    model::save(app, &mut record)?;
    if let Err(err) = devices::sync(app, &record) {
        // Leave nothing half-made behind.
        let _ = devices::remove_all(app, &record.id);
        let _ = app.store().delete(model::COLLECTION, &record.id);
        return Err(err);
    }
    app.audit(
        action,
        None,
        format!(
            "Created lab `{}` ({} nodes)",
            record.name(),
            record.spec.nodes.len()
        ),
        json!({"lab_id": record.id}),
    );
    runtime::publish_changed(app, &record.id, "created");
    view(app, &record.id)
}

/// The address a node is reached on inside the lab.
fn address_of(sim: &Lab, node: NodeId) -> Option<Ipv4Addr> {
    let view = sim.snapshot().nodes.into_iter().find(|v| v.id == node)?;
    node_address(&view)
}

fn node_address(view: &NodeView) -> Option<Ipv4Addr> {
    let from_mgmt = view.mgmt_ip.as_deref().and_then(parse_address);
    from_mgmt.or_else(|| {
        view.interfaces
            .iter()
            .filter_map(|interface| interface.ip.as_deref())
            .find_map(parse_address)
    })
}

fn parse_address(text: &str) -> Option<Ipv4Addr> {
    text.split('/').next()?.trim().parse().ok()
}

/// `to` is a node name or an address.
fn target_address(sim: &Lab, to: &str) -> AppResult<Ipv4Addr> {
    let to = to.trim();
    if let Ok(address) = to.parse::<Ipv4Addr>() {
        return Ok(address);
    }
    let node = node_of(sim, to)?;
    address_of(sim, node).ok_or_else(|| {
        AppError::invalid(format!(
            "`{to}` has no IPv4 address in this lab, so there is nothing to probe"
        ))
    })
}

fn slug(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn example_spec(id: &str) -> Option<LabSpec> {
    let id = id.trim();
    if id.eq_ignore_ascii_case("demo") || id.eq_ignore_ascii_case("campus") {
        return Some(demo_campus());
    }
    examples()
        .into_iter()
        .find(|spec| slug(&spec.name) == slug(id) || spec.name.eq_ignore_ascii_case(id))
}

fn example_list() -> Vec<ExampleInfo> {
    std::iter::once(demo_campus())
        .chain(examples())
        .map(|spec| ExampleInfo {
            id: if spec.name == "Campus" {
                "demo".into()
            } else {
                slug(&spec.name)
            },
            name: spec.name.clone(),
            description: spec.description.clone(),
            nodes: spec.nodes.len(),
            teaches: spec.notes.iter().map(|note| note.text.clone()).collect(),
        })
        .collect()
}

// ------------------------------------------------------------------- params

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct LabParams {
    lab_id: String,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CreateParams {
    name: Option<String>,
    /// An id from `labs.examples`; `demo` is the campus.
    example: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct UpdateParams {
    id: String,
    name: Option<String>,
    description: Option<String>,
    notes: Option<Vec<NoteSpec>>,
}

#[derive(Deserialize)]
struct DuplicateParams {
    id: String,
    name: String,
}

/// A specification as an object, or as JSON or YAML text.
#[derive(Deserialize)]
#[serde(untagged)]
enum SpecInput {
    Text(String),
    Spec(Box<LabSpec>),
}

#[derive(Deserialize)]
struct ImportParams {
    #[serde(default)]
    name: Option<String>,
    spec: SpecInput,
}

#[derive(Deserialize)]
struct ExportParams {
    id: String,
    #[serde(default = "default_format")]
    format: String,
}

fn default_format() -> String {
    "json".into()
}

#[derive(Deserialize)]
struct SpeedParams {
    id: String,
    speed: f32,
}

#[derive(Deserialize)]
struct NodeAddParams {
    lab_id: String,
    model: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    x: f32,
    #[serde(default)]
    y: f32,
}

#[derive(Deserialize)]
struct NodeUpdateParams {
    lab_id: String,
    node: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    x: Option<f32>,
    #[serde(default)]
    y: Option<f32>,
    #[serde(default)]
    powered: Option<bool>,
    #[serde(default)]
    host: Option<HostConfig>,
}

#[derive(Deserialize)]
struct Position {
    x: f32,
    y: f32,
}

#[derive(Deserialize)]
struct NodeMoveParams {
    lab_id: String,
    positions: BTreeMap<String, Position>,
}

#[derive(Deserialize)]
struct NodeParams {
    lab_id: String,
    node: String,
}

#[derive(Deserialize)]
struct ConfigParams {
    lab_id: String,
    node: String,
    #[serde(default = "running_kind")]
    kind: String,
}

fn running_kind() -> String {
    "running".into()
}

#[derive(Deserialize)]
struct ConfigSetParams {
    lab_id: String,
    node: String,
    startup: String,
}

#[derive(Deserialize)]
struct LinkEnd {
    node: String,
    #[serde(default)]
    interface: Option<String>,
}

#[derive(Deserialize)]
struct LinkAddParams {
    lab_id: String,
    a: LinkEnd,
    b: LinkEnd,
    #[serde(default)]
    cable: CableKind,
}

#[derive(Deserialize)]
struct LinkRemoveParams {
    lab_id: String,
    link: u32,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct LinksParams {
    lab_id: Option<String>,
}

#[derive(Deserialize)]
struct ProbeParams {
    lab_id: String,
    from: String,
    to: String,
    #[serde(default = "ping_kind")]
    kind: String,
    #[serde(default)]
    port: Option<u16>,
}

fn ping_kind() -> String {
    "ping".into()
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ConnectivityParams {
    lab_id: String,
    nodes: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct CaptureParams {
    lab_id: String,
    on: bool,
}

#[derive(Deserialize)]
struct TraceParams {
    lab_id: String,
    from: String,
    to: String,
    #[serde(default = "icmp_pdu")]
    pdu: String,
    #[serde(default)]
    port: Option<u16>,
}

fn icmp_pdu() -> String {
    "icmp".into()
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct PacketsParams {
    lab_id: String,
    after_seq: Option<u32>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct FlowAddParams {
    lab_id: String,
    flow: FlowSpec,
}

#[derive(Deserialize)]
struct FlowRemoveParams {
    lab_id: String,
    flow_id: u32,
}

#[derive(Deserialize)]
struct FaultInjectParams {
    lab_id: String,
    fault: Fault,
}

#[derive(Deserialize)]
struct FaultClearParams {
    lab_id: String,
    fault_id: u32,
}

// ------------------------------------------------------------------ results

#[derive(Serialize)]
struct StopResult {
    id: String,
    running: bool,
    /// Nodes whose running configuration has just been lost.
    unsaved_nodes: Vec<String>,
}

#[derive(Serialize)]
struct ConfigResult {
    text: String,
}

#[derive(Serialize)]
struct ConfigSetResult {
    /// The startup configuration the device now holds.
    text: String,
    accepted: usize,
    rejected: Vec<RejectedLine>,
}

#[derive(Clone, Debug, Serialize)]
pub struct RejectedLine {
    pub line: String,
    pub error: String,
}

#[derive(Serialize)]
struct LinkEndRow {
    device_id: Option<String>,
    node: String,
    interface: String,
}

#[derive(Serialize)]
struct LinkRow {
    lab_id: String,
    lab: String,
    link: u32,
    a: LinkEndRow,
    b: LinkEndRow,
    up: bool,
    running: bool,
}

#[derive(Serialize)]
struct Cell {
    ok: bool,
    reason: String,
}

#[derive(Serialize)]
struct Connectivity {
    nodes: Vec<String>,
    matrix: Vec<Vec<Cell>>,
    reachable_pct: f32,
}

// --------------------------------------------------------------- operations

async fn create(app: &App, params: CreateParams) -> AppResult<LabView> {
    let mut spec = match params.example.as_deref() {
        Some(example) => example_spec(example).ok_or_else(|| {
            AppError::invalid(format!(
                "there is no example lab called `{example}`; `labs.examples` lists them"
            ))
        })?,
        None => LabSpec {
            name: "Lab".into(),
            ..LabSpec::default()
        },
    };
    spec.name = match params.name {
        Some(name) => {
            model::check_name_free(app, &name, None)?;
            name.trim().to_string()
        }
        None => model::unique_name(app, &spec.name)?,
    };
    // Refuse a specification the simulator cannot load before it is stored.
    Lab::from_spec(&spec).map_err(model::sim_error)?;
    create_record(app, spec, "labs.create")
}

async fn import(app: &App, params: ImportParams) -> AppResult<LabView> {
    let mut spec = match params.spec {
        SpecInput::Spec(spec) => *spec,
        SpecInput::Text(text) => parse_spec(&text)?,
    };
    if let Some(name) = params.name {
        spec.name = name;
    }
    spec.name = model::unique_name(app, &spec.name)?;
    Lab::from_spec(&spec).map_err(model::sim_error)?;
    create_record(app, spec, "labs.import")
}

fn parse_spec(text: &str) -> AppResult<LabSpec> {
    let text = text.trim();
    if text.is_empty() {
        return Err(AppError::invalid("the lab file is empty"));
    }
    if text.starts_with('{') {
        return serde_json::from_str(text)
            .map_err(|err| AppError::invalid(format!("this is not a lab file: {err}")));
    }
    serde_yaml_ng::from_str(text)
        .map_err(|err| AppError::invalid(format!("this is not a lab file: {err}")))
}

async fn update(app: &App, params: UpdateParams) -> AppResult<LabView> {
    let mut record = model::load(app, &params.id)?;
    if let Some(name) = params.name {
        let name = name.trim().to_string();
        if !name.eq_ignore_ascii_case(record.name()) {
            model::check_name_free(app, &name, Some(&record.id))?;
        }
        record.spec.name = name;
    }
    if let Some(description) = params.description {
        record.spec.description = description;
    }
    if let Some(notes) = params.notes {
        record.spec.notes = notes.clone();
        if let Some(lab) = runtime::running(app, &record.id) {
            lab.with(|sim| sim.set_notes(notes));
        }
    }
    if let Some(lab) = runtime::running(app, &record.id) {
        let description = record.spec.description.clone();
        lab.with(|sim| sim.set_description(&description));
    }
    model::save(app, &mut record)?;
    // The devices are named after the lab.
    runtime::settle_devices(app, &record.id)?;
    app.audit(
        "labs.update",
        None,
        format!("Updated lab `{}`", record.name()),
        json!({"lab_id": record.id}),
    );
    runtime::publish_changed(app, &record.id, "updated");
    view(app, &record.id)
}

async fn delete(app: &App, id: &str) -> AppResult<Value> {
    let record = model::load(app, id)?;
    runtime::discard(app, id, "the lab was deleted");
    expose::close_lab(app, id).await;
    let removed = devices::remove_all(app, id)?;
    app.store().delete(model::COLLECTION, id)?;
    app.audit(
        "labs.delete",
        None,
        format!(
            "Deleted lab `{}` and its {removed} simulated devices",
            record.name()
        ),
        json!({"lab_id": id}),
    );
    runtime::publish_changed(app, id, "deleted");
    Ok(json!({"deleted": true, "devices_removed": removed}))
}

async fn duplicate(app: &App, params: DuplicateParams) -> AppResult<LabView> {
    let (_, mut spec) = runtime::current_spec(app, &params.id)?;
    spec.name = model::unique_name(app, &params.name)?;
    create_record(app, spec, "labs.duplicate")
}

fn reset(app: &App, id: &str) -> AppResult<LabView> {
    let settle = model::settings(app).settle_s.clamp(1, 600) * 1_000_000;
    if let Some(lab) = runtime::running(app, id) {
        lab.with(|sim| {
            let nodes: Vec<NodeId> = sim.snapshot().nodes.iter().map(|node| node.id).collect();
            let powered: Vec<NodeId> = sim
                .snapshot()
                .nodes
                .iter()
                .filter(|node| node.powered)
                .map(|node| node.id)
                .collect();
            for node in &nodes {
                sim.set_power(*node, false);
            }
            for node in powered {
                sim.set_power(node, true);
            }
            sim.settle(settle);
        });
        lab.baseline_all();
        lab.touch();
    } else {
        model::load(app, id)?;
    }
    app.audit(
        "labs.reset",
        None,
        format!("Reset lab `{}`", model::load(app, id)?.name()),
        json!({"lab_id": id}),
    );
    runtime::publish_changed(app, id, "topology");
    view(app, id)
}

fn node_config(app: &App, params: &ConfigParams) -> AppResult<ConfigResult> {
    let startup = match params.kind.as_str() {
        "running" => false,
        "startup" => true,
        other => {
            return Err(AppError::invalid(format!(
                "`{other}` is not a configuration kind; use `running` or `startup`"
            )));
        }
    };
    let node = params.node.clone();
    read(app, &params.lab_id, move |sim| {
        let id = node_of(sim, &node)?;
        let text = if startup {
            sim.startup_config(id)
        } else {
            sim.running_config(id)
        };
        Ok(ConfigResult {
            text: text.map_err(model::sim_error)?,
        })
    })
}

/// Replace a node's startup configuration and reboot it.
///
/// The simulator has no setter for a startup configuration, so this does what
/// an engineer would: erase NVRAM, reload the node, type the configuration in
/// and save it. That also reports, line by line, what the device refused.
fn set_node_config(app: &App, params: ConfigSetParams) -> AppResult<ConfigSetResult> {
    let text = params.startup.clone();
    let node_name = params.node.clone();
    let result = runtime::edit(app, &params.lab_id, move |sim| {
        let node = node_of(sim, &node_name)?;
        let lines = config_lines(&text);
        sim.exec(node, "erase startup-config")
            .map_err(model::sim_error)?;
        sim.exec(node, "reload").map_err(model::sim_error)?;
        let outcome = sim.configure(node, &lines).map_err(model::sim_error)?;
        sim.exec(node, "write memory").map_err(model::sim_error)?;
        let rejected: Vec<RejectedLine> = outcome
            .lines
            .iter()
            .filter(|line| line.error)
            .map(|line| RejectedLine {
                line: line.line.clone(),
                error: line.output.trim().to_string(),
            })
            .collect();
        Ok(ConfigSetResult {
            text: sim.startup_config(node).map_err(model::sim_error)?,
            accepted: outcome.lines.len() - rejected.len(),
            rejected,
        })
    })?;
    if let Some(lab) = runtime::running(app, &params.lab_id)
        && let Some(node) = lab.with(|sim| sim.node_by_name(&params.node))
    {
        lab.forget_baseline(node);
    }
    app.audit(
        "labs.node.config_set",
        None,
        format!(
            "Replaced the startup configuration of lab node `{}`",
            params.node
        ),
        json!({"lab_id": params.lab_id, "rejected": result.rejected.len()}),
    );
    runtime::publish_changed(app, &params.lab_id, "config");
    Ok(result)
}

/// Configuration text as the device would read it: stop at `end`, as a boot does.
fn config_lines(text: &str) -> Vec<String> {
    text.lines()
        .take_while(|line| line.trim() != "end")
        .map(str::to_string)
        .collect()
}

fn add_link(app: &App, params: LinkAddParams) -> AppResult<LinkView> {
    let cable = params.cable;
    let (a_name, b_name) = (params.a.node.clone(), params.b.node.clone());
    let (a_if, b_if) = (params.a.interface.clone(), params.b.interface.clone());
    runtime::edit(app, &params.lab_id, move |sim| {
        let a = node_of(sim, &a_name)?;
        let b = node_of(sim, &b_name)?;
        let a_if = pick_interface(sim, a, &a_name, a_if, cable)?;
        let b_if = pick_interface(sim, b, &b_name, b_if, cable)?;
        let id = sim
            .add_link(a, &a_if, b, &b_if, cable)
            .map_err(model::sim_error)?;
        sim.snapshot()
            .links
            .into_iter()
            .find(|link| link.id == id)
            .ok_or_else(|| AppError::internal("the link disappeared as it was made"))
    })
}

/// The interface the caller named, or the first free one that suits the cable.
fn pick_interface(
    sim: &Lab,
    node: NodeId,
    name: &str,
    wanted: Option<String>,
    cable: CableKind,
) -> AppResult<String> {
    if let Some(wanted) = wanted.filter(|wanted| !wanted.trim().is_empty()) {
        return Ok(wanted);
    }
    if cable == CableKind::Console {
        let view = sim.snapshot().nodes.into_iter().find(|v| v.id == node);
        return match view.map(|view| view.cli) {
            Some(CliKind::Ios) => Ok("Console".into()),
            Some(CliKind::Host) => Ok("RS232".into()),
            _ => Err(AppError::invalid(format!(
                "`{name}` has no console port: it has no command line"
            ))),
        };
    }
    let free = sim.free_interfaces(node);
    let fibre_first = free
        .iter()
        .find(|port| port.starts_with("TenGigabit") || port.starts_with("Gigabit"));
    let chosen = match cable {
        CableKind::Fiber => fibre_first.or(free.first()),
        _ => free.first(),
    };
    chosen.cloned().ok_or_else(|| {
        AppError::conflict(format!(
            "`{name}` has no free interface left; remove a cable or use a model with more ports"
        ))
    })
}

fn links(app: &App, lab_id: Option<&str>) -> AppResult<Vec<LinkRow>> {
    let records = match lab_id {
        Some(id) => vec![model::load(app, id)?],
        None => model::all(app)?,
    };
    let mut rows = Vec::new();
    for record in records {
        let device_ids = devices::device_ids(app, &record.id)?;
        let running = runtime::running(app, &record.id);
        let live: Vec<LinkView> = running
            .as_ref()
            .map(|lab| lab.with(|sim| sim.snapshot().links))
            .unwrap_or_default();
        let names: BTreeMap<NodeId, String> = running
            .as_ref()
            .map(|lab| {
                lab.with(|sim| {
                    sim.snapshot()
                        .nodes
                        .into_iter()
                        .map(|node| (node.id, node.name))
                        .collect()
                })
            })
            .unwrap_or_default();
        let (_, spec) = runtime::current_spec(app, &record.id)?;
        for (index, link) in spec.links.iter().enumerate() {
            let id = index as u32 + 1;
            let found = live.iter().find(|live| {
                names.get(&live.a).map(String::as_str) == Some(link.a.node.as_str())
                    && live.a_if == link.a.interface
            });
            let end = |node: &str, interface: &str| LinkEndRow {
                device_id: device_ids.get(node).cloned(),
                node: node.to_string(),
                interface: interface.to_string(),
            };
            rows.push(LinkRow {
                lab_id: record.id.clone(),
                lab: record.name().to_string(),
                link: found.map(|link| link.id).unwrap_or(id),
                a: end(&link.a.node, &link.a.interface),
                b: end(&link.b.node, &link.b.interface),
                up: found.is_some_and(|link| link.up),
                running: running.is_some(),
            });
        }
    }
    Ok(rows)
}

fn probe(app: &App, params: ProbeParams) -> AppResult<ProbeResult> {
    let lab = runtime::require_running(app, &params.lab_id)?;
    lab.with(|sim| {
        let from = node_of(sim, &params.from)?;
        let to = target_address(sim, &params.to)?;
        match params.kind.as_str() {
            "ping" => Ok(sim.probe_ping(from, to)),
            "tcp" => {
                let port = params.port.ok_or_else(|| {
                    AppError::invalid("a tcp probe needs a port, e.g. `{\"port\": 80}`")
                })?;
                Ok(sim.probe_tcp(from, to, port))
            }
            other => Err(AppError::invalid(format!(
                "`{other}` is not a probe kind; use `ping` or `tcp`"
            ))),
        }
    })
}

async fn connectivity(app: &App, params: ConnectivityParams) -> AppResult<Connectivity> {
    let lab = runtime::require_running(app, &params.lab_id)?;
    let limit = model::settings(app).connectivity_max_nodes.max(2) as usize;

    // Everything with an address, or exactly what the caller asked for, in
    // the order they asked for it.
    let targets: Vec<(NodeId, String, Ipv4Addr)> = lab.with(|sim| {
        let snapshot = sim.snapshot();
        let addressable = |node: &NodeView| {
            node.powered
                .then(|| Some((node.id, node.name.clone(), node_address(node)?)))
                .flatten()
        };
        match &params.nodes {
            None => snapshot.nodes.iter().filter_map(addressable).collect(),
            Some(wanted) => wanted
                .iter()
                .filter_map(|name| {
                    let node = snapshot
                        .nodes
                        .iter()
                        .find(|node| node.name.eq_ignore_ascii_case(name))?;
                    addressable(node)
                })
                .collect(),
        }
    });
    if let Some(wanted) = &params.nodes {
        for name in wanted {
            if !targets.iter().any(|(_, node, _)| node == name) {
                return Err(AppError::invalid(format!(
                    "`{name}` is not a powered node with an address in this lab"
                )));
            }
        }
    }
    if targets.len() > limit {
        return Err(AppError::invalid(format!(
            "this lab has {} addressable nodes and the matrix is capped at {limit}; ask for a list of nodes",
            targets.len()
        )));
    }

    let names: Vec<String> = targets.iter().map(|(_, name, _)| name.clone()).collect();
    let mut matrix = Vec::with_capacity(targets.len());
    let (mut ok, mut total) = (0_usize, 0_usize);
    for (from, _, _) in &targets {
        let mut row = Vec::with_capacity(targets.len());
        for (to, _, address) in &targets {
            if from == to {
                row.push(Cell {
                    ok: true,
                    reason: String::new(),
                });
                continue;
            }
            // One probe per lock: the ticker and the consoles keep running.
            let result = lab.with(|sim| sim.probe_ping(*from, *address));
            total += 1;
            ok += usize::from(result.ok);
            row.push(Cell {
                ok: result.ok,
                reason: result.reason,
            });
        }
        matrix.push(row);
        tokio::task::yield_now().await;
    }
    Ok(Connectivity {
        nodes: names,
        matrix,
        reachable_pct: if total == 0 {
            100.0
        } else {
            (ok as f32 * 100.0 / total as f32 * 10.0).round() / 10.0
        },
    })
}

fn trace(app: &App, params: TraceParams) -> AppResult<Value> {
    let lab = runtime::require_running(app, &params.lab_id)?;
    let pdu = match (params.pdu.as_str(), params.port) {
        ("icmp", _) => PduKind::Icmp,
        ("tcp", Some(port)) => PduKind::Tcp { port },
        ("udp", Some(port)) => PduKind::Udp { port },
        ("tcp" | "udp", None) => {
            return Err(AppError::invalid("a tcp or udp trace needs a port"));
        }
        (other, _) => {
            return Err(AppError::invalid(format!(
                "`{other}` is not a kind of packet; use `icmp`, `tcp` or `udp`"
            )));
        }
    };
    let trace = lab.with(|sim| {
        let from = node_of(sim, &params.from)?;
        let to = target_address(sim, &params.to)?;
        sim.start_trace(from, to, pdu).map_err(model::sim_error)
    })?;
    Ok(json!({"trace_id": trace}))
}

// ----------------------------------------------------------------- wiring up

pub fn init(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("labs.list", |app: App, _: NoParams| async move {
        let mut rows = Vec::new();
        for record in model::all(&app)? {
            rows.push(LabSummary {
                nodes: record.spec.nodes.len(),
                links: record.spec.links.len(),
                running: runtime::running(&app, &record.id).is_some(),
                id: record.id,
                name: record.spec.name,
                description: record.spec.description,
                updated_at: record.updated_at,
            });
        }
        Ok(rows)
    });
    router.add("labs.create", |app: App, params: CreateParams| async move {
        create(&app, params).await
    });
    router.add("labs.get", |app: App, params: IdParams| async move {
        view(&app, &params.id)
    });
    router.add("labs.update", |app: App, params: UpdateParams| async move {
        update(&app, params).await
    });
    router.add("labs.delete", |app: App, params: IdParams| async move {
        delete(&app, &params.id).await
    });
    router.add(
        "labs.duplicate",
        |app: App, params: DuplicateParams| async move { duplicate(&app, params).await },
    );
    router.add("labs.import", |app: App, params: ImportParams| async move {
        import(&app, params).await
    });
    router.add("labs.export", |app: App, params: ExportParams| async move {
        let (_, spec) = runtime::current_spec(&app, &params.id)?;
        let text = match params.format.as_str() {
            "json" => serde_json::to_string_pretty(&spec)?,
            "yaml" | "yml" => serde_yaml_ng::to_string(&spec)
                .map_err(|err| AppError::internal(format!("the lab cannot be written: {err}")))?,
            other => {
                return Err(AppError::invalid(format!(
                    "`{other}` is not a lab file format; use `json` or `yaml`"
                )));
            }
        };
        Ok(json!({ "text": text }))
    });

    router.add("labs.start", |app: App, params: IdParams| async move {
        runtime::start(&app, &params.id).await?;
        view(&app, &params.id)
    });
    router.add("labs.stop", |app: App, params: IdParams| async move {
        let unsaved = runtime::stop(&app, &params.id)?;
        Ok(StopResult {
            id: params.id,
            running: false,
            unsaved_nodes: unsaved,
        })
    });
    router.add("labs.speed", |app: App, params: SpeedParams| async move {
        if !(0.0..=16.0).contains(&params.speed) {
            return Err(AppError::invalid(
                "the speed is 0 (paused) or between 0.25 and 16 times real time",
            ));
        }
        let speed = if params.speed == 0.0 {
            0.0
        } else {
            params.speed.clamp(0.25, 16.0)
        };
        let lab = runtime::require_running(&app, &params.id)?;
        lab.set_speed(speed);
        runtime::publish_changed(&app, &params.id, "updated");
        Ok(json!({"id": params.id, "speed": speed}))
    });
    router.add("labs.reset", |app: App, params: IdParams| async move {
        reset(&app, &params.id)
    });
    router.add("labs.catalog", |_app: App, _: NoParams| async move {
        Ok(catalog())
    });
    router.add("labs.examples", |_app: App, _: NoParams| async move {
        Ok(example_list())
    });

    router.add(
        "labs.node.add",
        |app: App, params: NodeAddParams| async move {
            let (model_id, name) = (params.model.clone(), params.name.clone());
            let (x, y) = (params.x, params.y);
            let node = runtime::edit(&app, &params.lab_id, move |sim| {
                let node = sim
                    .add_node(&model_id, name.as_deref(), x, y)
                    .map_err(model::sim_error)?;
                node_view(sim, node)
            })?;
            runtime::settle_devices(&app, &params.lab_id)?;
            let record = model::load(&app, &params.lab_id)?;
            if let Some(lab) = runtime::running(&app, &params.lab_id) {
                lab.forget_baseline(node.id);
            }
            app.audit(
                "labs.node.add",
                None,
                format!("Added `{}` to lab `{}`", node.name, record.name()),
                json!({"lab_id": record.id, "model": params.model}),
            );
            runtime::publish_changed(&app, &params.lab_id, "topology");
            Ok(node)
        },
    );
    router.add(
        "labs.node.update",
        |app: App, params: NodeUpdateParams| async move {
            let record = model::load(&app, &params.lab_id)?;
            let renamed = params
                .name
                .as_ref()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.eq_ignore_ascii_case(&params.node));
            let (node_name, rename) = (params.node.clone(), renamed.clone());
            let (x, y, powered, host) = (params.x, params.y, params.powered, params.host.clone());
            let node = runtime::edit(&app, &params.lab_id, move |sim| {
                let node = node_of(sim, &node_name)?;
                if let Some(name) = rename {
                    sim.rename_node(node, &name).map_err(model::sim_error)?;
                }
                if x.is_some() || y.is_some() {
                    let at = node_view(sim, node)?;
                    sim.move_node(node, x.unwrap_or(at.x), y.unwrap_or(at.y));
                }
                if let Some(host) = host {
                    sim.set_host_config(node, host).map_err(model::sim_error)?;
                }
                if let Some(powered) = powered {
                    sim.set_power(node, powered);
                }
                node_view(sim, node)
            })?;
            if let Some(name) = &renamed {
                devices::rename_node(&app, &record, &params.node, name)?;
                runtime::settle_devices(&app, &params.lab_id)?;
            }
            if let Some(lab) = runtime::running(&app, &params.lab_id)
                && params.powered.is_some()
            {
                lab.forget_baseline(node.id);
            }
            runtime::publish_changed(&app, &params.lab_id, "node");
            Ok(node)
        },
    );
    router.add(
        "labs.node.move",
        |app: App, params: NodeMoveParams| async move {
            let positions = params.positions;
            let moved = runtime::edit(&app, &params.lab_id, move |sim| {
                let mut moved = 0;
                for (name, position) in &positions {
                    let node = node_of(sim, name)?;
                    sim.move_node(node, position.x, position.y);
                    moved += 1;
                }
                Ok(moved)
            })?;
            // No event: a drag would flood the bus. The autosave is debounced.
            Ok(json!({ "moved": moved }))
        },
    );
    router.add(
        "labs.node.remove",
        |app: App, params: NodeParams| async move {
            let node_name = params.node.clone();
            let running = runtime::running(&app, &params.lab_id);
            let removed = running
                .as_ref()
                .and_then(|lab| lab.with(|sim| sim.node_by_name(&node_name)));
            runtime::edit(&app, &params.lab_id, move |sim| {
                let node = node_of(sim, &node_name)?;
                sim.remove_node(node).map_err(model::sim_error)
            })?;
            // The simulator closed its side of any console on that node;
            // close ours too, rather than leaving a terminal hanging.
            if let (Some(lab), Some(node)) = (running, removed) {
                lab.close_consoles_of(node);
            }
            runtime::settle_devices(&app, &params.lab_id)?;
            let record = model::load(&app, &params.lab_id)?;
            app.audit(
                "labs.node.remove",
                None,
                format!("Removed `{}` from lab `{}`", params.node, record.name()),
                json!({"lab_id": record.id}),
            );
            runtime::publish_changed(&app, &params.lab_id, "topology");
            Ok(json!({"removed": true}))
        },
    );
    router.add(
        "labs.node.config",
        |app: App, params: ConfigParams| async move { node_config(&app, &params) },
    );
    router.add(
        "labs.node.config_set",
        |app: App, params: ConfigSetParams| async move { set_node_config(&app, params) },
    );
    router.add(
        "labs.node.tables",
        |app: App, params: NodeParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            lab.with(|sim| {
                let node = node_of(sim, &params.node)?;
                sim.node_tables(node).map_err(model::sim_error)
            })
        },
    );
    router.add(
        "labs.node.metrics",
        |app: App, params: NodeParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            lab.with(|sim| {
                let node = node_of(sim, &params.node)?;
                sim.metrics(node).map_err(model::sim_error)
            })
        },
    );

    router.add(
        "labs.link.add",
        |app: App, params: LinkAddParams| async move {
            let lab_id = params.lab_id.clone();
            let link = add_link(&app, params)?;
            runtime::publish_changed(&app, &lab_id, "topology");
            Ok(link)
        },
    );
    router.add(
        "labs.link.remove",
        |app: App, params: LinkRemoveParams| async move {
            let link = params.link;
            runtime::edit(&app, &params.lab_id, move |sim| {
                sim.remove_link(link).map_err(model::sim_error)
            })?;
            runtime::publish_changed(&app, &params.lab_id, "topology");
            Ok(json!({"removed": true}))
        },
    );
    router.add("labs.links", |app: App, params: LinksParams| async move {
        links(&app, params.lab_id.as_deref())
    });

    router.add("labs.probe", |app: App, params: ProbeParams| async move {
        probe(&app, params)
    });
    router.add(
        "labs.connectivity",
        |app: App, params: ConnectivityParams| async move { connectivity(&app, params).await },
    );
    router.add(
        "labs.capture",
        |app: App, params: CaptureParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            lab.set_capture(params.on);
            Ok(json!({"capture": params.on}))
        },
    );
    router.add("labs.trace", |app: App, params: TraceParams| async move {
        trace(&app, params)
    });
    router.add(
        "labs.packets",
        |app: App, params: PacketsParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            let limit = params.limit.unwrap_or(500).clamp(1, 5_000) as usize;
            let (events, next_seq) = lab.packets(params.after_seq.unwrap_or(0), limit);
            Ok(json!({"events": events, "next_seq": next_seq}))
        },
    );

    router.add(
        "labs.flow.add",
        |app: App, params: FlowAddParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            let id = lab.with(|sim| sim.add_flow(params.flow).map_err(model::sim_error))?;
            lab.touch();
            Ok(json!({"flow_id": id}))
        },
    );
    router.add(
        "labs.flow.remove",
        |app: App, params: FlowRemoveParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            lab.with(|sim| sim.remove_flow(params.flow_id));
            lab.touch();
            Ok(json!({"removed": true}))
        },
    );
    router.add("labs.flow.list", |app: App, params: LabParams| async move {
        let lab = runtime::require_running(&app, &params.lab_id)?;
        let flows = lab.with(|sim| sim.flows());
        Ok(flows
            .into_iter()
            .map(|(id, spec, state)| json!({"id": id, "flow": spec, "state": state}))
            .collect::<Vec<Value>>())
    });

    router.add(
        "labs.fault.inject",
        |app: App, params: FaultInjectParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            let fault = params.fault.clone();
            let id = lab.with(|sim| sim.inject(fault).map_err(model::sim_error))?;
            app.audit(
                "labs.fault.inject",
                None,
                format!(
                    "Injected a fault into lab `{}`",
                    model::load(&app, &params.lab_id)?.name()
                ),
                json!({"lab_id": params.lab_id, "fault": params.fault}),
            );
            runtime::publish_changed(&app, &params.lab_id, "topology");
            Ok(json!({"fault_id": id}))
        },
    );
    router.add(
        "labs.fault.clear",
        |app: App, params: FaultClearParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            lab.with(|sim| sim.clear_fault(params.fault_id));
            runtime::publish_changed(&app, &params.lab_id, "topology");
            Ok(json!({"cleared": true}))
        },
    );
    router.add(
        "labs.fault.list",
        |app: App, params: LabParams| async move {
            let lab = runtime::require_running(&app, &params.lab_id)?;
            let faults = lab.with(|sim| sim.faults());
            Ok(faults
                .into_iter()
                .map(|(id, fault)| json!({"id": id, "fault": fault}))
                .collect::<Vec<Value>>())
        },
    );

    expose::init(router);
    twin::init(router);
}
