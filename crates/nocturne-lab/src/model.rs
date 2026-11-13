//! What a lab is when it is not running: the stored record, the settings and
//! the views the RPC surface returns.

use std::collections::BTreeMap;

use nocturne_app::App;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{AppError, AppResult};
use nocturne_netsim::{LabSnapshot, LabSpec};
use serde::{Deserialize, Serialize};

/// Document collection holding one [`LabRecord`] per lab.
pub const COLLECTION: &str = "labs";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Labs that may run at the same time.
    pub max_running: u32,
    /// Real milliseconds between simulation steps.
    pub tick_ms: u64,
    /// Simulated time per step, as a multiple of `tick_ms`. `0` pauses a lab.
    pub default_speed: f32,
    /// Simulated seconds a freshly loaded lab may take to converge.
    pub settle_s: u64,
    /// Use real protocol timers (30 s spanning-tree forward delay) instead of
    /// the simulator's short ones.
    pub realistic_timers: bool,
    /// Packet events kept per lab for the inspector.
    pub capture_events: u32,
    /// Milliseconds of quiet before an edited lab is written back to the store.
    pub autosave_ms: u64,
    /// Address `labs.expose` binds to when the caller does not say.
    pub expose_bind: String,
    /// Nodes `labs.connectivity` will test without an explicit list.
    pub connectivity_max_nodes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_running: 8,
            tick_ms: 50,
            default_speed: 1.0,
            settle_s: 60,
            realistic_timers: false,
            capture_events: 20_000,
            autosave_ms: 1_000,
            expose_bind: "127.0.0.1".into(),
            connectivity_max_nodes: 40,
        }
    }
}

pub fn settings(app: &App) -> Settings {
    nocturne_app::settings::get(app, "labs").unwrap_or_default()
}

/// A lab as it is stored: its specification plus identity and timestamps.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LabRecord {
    pub id: String,
    pub spec: LabSpec,
    pub created_at: i64,
    pub updated_at: i64,
}

impl LabRecord {
    pub fn new(spec: LabSpec) -> LabRecord {
        let now = now_ms();
        LabRecord {
            id: new_id(),
            spec,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn name(&self) -> &str {
        &self.spec.name
    }
}

/// A row of `labs.list`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub nodes: usize,
    pub links: usize,
    pub running: bool,
    pub updated_at: i64,
}

/// Everything the lab screen needs about one lab.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LabView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub running: bool,
    /// Simulated time per real second; `0` while the lab is paused.
    pub speed: f32,
    /// Microseconds of simulated time since the lab was created.
    pub sim_time: u64,
    pub snapshot: LabSnapshot,
    /// Node name → inventory device id, for the nodes that have a command line.
    pub devices: BTreeMap<String, String>,
    /// Nodes whose running configuration would be lost if the lab were stopped.
    pub unsaved_nodes: Vec<String>,
    pub capture: bool,
}

/// One of the simulator's teaching labs, as `labs.examples` lists them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExampleInfo {
    /// What to pass as `labs.create {example}`.
    pub id: String,
    pub name: String,
    pub description: String,
    pub nodes: usize,
    /// The lab's own on-canvas notes: what it is there to show.
    pub teaches: Vec<String>,
}

// ------------------------------------------------------------------- storage

pub fn load(app: &App, id: &str) -> AppResult<LabRecord> {
    app.store()
        .get::<LabRecord>(COLLECTION, id)?
        .ok_or_else(|| AppError::not_found(format!("lab `{id}`")))
}

pub fn save(app: &App, record: &mut LabRecord) -> AppResult<()> {
    record.updated_at = now_ms();
    app.store().put(COLLECTION, &record.id, record)
}

pub fn all(app: &App) -> AppResult<Vec<LabRecord>> {
    let mut labs: Vec<LabRecord> = app.store().list(COLLECTION)?;
    labs.sort_by_key(|lab| lab.spec.name.to_lowercase());
    Ok(labs)
}

/// Refuse a name that another lab already uses: inventory device names are
/// `<lab>/<node>` and must stay unique.
pub fn check_name_free(app: &App, name: &str, except: Option<&str>) -> AppResult<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::invalid("a lab needs a name"));
    }
    if name.contains('/') {
        return Err(AppError::invalid(
            "a lab name cannot contain `/`: device names are `<lab>/<node>`",
        ));
    }
    let taken = all(app)?.into_iter().any(|other| {
        other.spec.name.eq_ignore_ascii_case(name) && Some(other.id.as_str()) != except
    });
    if taken {
        return Err(AppError::conflict(format!(
            "a lab named `{name}` already exists; choose another name"
        )));
    }
    Ok(())
}

/// A name like `Campus 2` that no lab uses yet.
pub fn unique_name(app: &App, wanted: &str) -> AppResult<String> {
    let wanted = wanted.trim();
    let base = if wanted.is_empty() { "Lab" } else { wanted };
    let base = base.replace('/', "-");
    if check_name_free(app, &base, None).is_ok() {
        return Ok(base);
    }
    for n in 2..1000 {
        let candidate = format!("{base} {n}");
        if check_name_free(app, &candidate, None).is_ok() {
            return Ok(candidate);
        }
    }
    Err(AppError::conflict(format!(
        "too many labs are called `{base}`"
    )))
}

/// Turn a simulator error into one the person at the keyboard can act on.
pub fn sim_error(err: nocturne_netsim::LabError) -> AppError {
    use nocturne_netsim::LabError::*;
    match err {
        UnknownNode(_) | UnknownNodeName(_) | UnknownLink(_) => {
            AppError::not_found(err.to_string())
        }
        DuplicateName(_) | InterfaceInUse { .. } => AppError::conflict(err.to_string()),
        Unsupported(_) => AppError::new("unsupported", err.to_string()),
        _ => AppError::invalid(err.to_string()),
    }
}
