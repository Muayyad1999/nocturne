//! The life a running lab has: a `Lab` in memory, a ticker advancing it, the
//! capture buffer, autosaving and the events that come out of the simulator.
//!
//! Every access to the simulator goes through [`RunningLab::with`], which holds
//! a plain mutex for as long as one synchronous call takes and no longer;
//! nothing in this module awaits while holding it.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use nocturne_netsim::{
    CliKind, Lab, LabEvent, LabSnapshot, LabSpec, Light, LinkId, NodeId, PacketEvent,
};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use serde_json::json;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::devices;
use crate::model::{self, LabRecord, Settings};

/// How often the settings are re-read by a ticker, in ticks.
const SETTINGS_EVERY: u64 = 40;
/// `labs.tick` is published at most this often.
const TICK_EVENT_MS: i64 = 250;
/// `labs.changed` caused by simulator events is published at most this often.
const CHANGED_EVENT_MS: i64 = 250;
/// `labs.packets` batches are published at most this often.
const PACKET_EVENT_MS: i64 = 100;
/// Packet events in one `labs.packets` batch.
const PACKET_BATCH: usize = 500;

// --------------------------------------------------------------------- state

#[derive(Default)]
pub struct LabsState {
    labs: Mutex<HashMap<String, Arc<RunningLab>>>,
    /// Held while a lab is being loaded, so two callers cannot start the same one.
    starting: tokio::sync::Mutex<()>,
}

/// What a node's configuration looked like when it last booted, so that
/// "unsaved" means "a restart would lose this".
#[derive(Clone, Copy)]
struct NodeSave {
    startup: u64,
    /// Hash of the running configuration the node had just after booting.
    baseline: u64,
    running: u64,
}

struct Packets {
    events: VecDeque<PacketEvent>,
    capacity: usize,
    /// One past the highest sequence number seen.
    next_seq: u32,
    /// One past the highest sequence number already sent in a `labs.packets` event.
    sent_seq: u32,
}

pub struct RunningLab {
    pub id: String,
    lab: Mutex<Lab>,
    speed: RwLock<f32>,
    capture: AtomicBool,
    packets: Mutex<Packets>,
    /// Node id → inventory device id, for the events that name a node.
    devices: RwLock<HashMap<NodeId, String>>,
    saves: Mutex<HashMap<NodeId, NodeSave>>,
    /// Nodes whose saved state should be looked at again on the next tick
    /// (somebody typed on their console, and may have typed `write memory`).
    measure: Mutex<Vec<NodeId>>,
    /// One token per open console, so the consoles of a node that is removed
    /// can be closed with it.
    consoles: Mutex<Vec<(NodeId, CancellationToken)>>,
    dirty: AtomicBool,
    /// Bumped at the end of every tick: consoles wake up and drain their output.
    wake: watch::Sender<u64>,
    /// What open consoles are told when `cancel` fires.
    closing: RwLock<String>,
    /// Cancelled when the lab is stopped or deleted, and when the app shuts down.
    pub cancel: CancellationToken,
}

impl RunningLab {
    /// Run `f` against the simulator. Keep it short: this blocks the ticker,
    /// every console of the lab and every other caller.
    pub fn with<T>(&self, f: impl FnOnce(&mut Lab) -> T) -> T {
        let mut lab = self.lab.lock();
        f(&mut lab)
    }

    pub fn speed(&self) -> f32 {
        *self.speed.read()
    }

    pub fn set_speed(&self, speed: f32) {
        *self.speed.write() = speed;
    }

    pub fn capture(&self) -> bool {
        self.capture.load(Ordering::Relaxed)
    }

    pub fn set_capture(&self, on: bool) {
        self.capture.store(on, Ordering::Relaxed);
        self.with(|lab| lab.set_capture(on));
        if !on {
            self.packets.lock().events.clear();
        }
    }

    /// The lab's specification has changed and should be written back.
    pub fn touch(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn wake(&self) -> watch::Receiver<u64> {
        self.wake.subscribe()
    }

    /// A console was opened on `node`: the token is cancelled when the lab
    /// stops or when that node is removed.
    pub fn register_console(&self, node: NodeId) -> CancellationToken {
        let token = self.cancel.child_token();
        let mut consoles = self.consoles.lock();
        consoles.retain(|(_, open)| !open.is_cancelled());
        consoles.push((node, token.clone()));
        token
    }

    /// Close the consoles open on a node (it is about to disappear).
    pub fn close_consoles_of(&self, node: NodeId) {
        self.consoles.lock().retain(|(open_on, token)| {
            if *open_on == node {
                token.cancel();
                false
            } else {
                true
            }
        });
    }

    /// Stop the ticker and close the consoles, saying why.
    pub fn close(&self, reason: &str) {
        *self.closing.write() = reason.to_string();
        self.cancel.cancel();
    }

    pub fn closing_reason(&self) -> String {
        self.closing.read().clone()
    }

    /// Packet events after `after_seq`, oldest first, and the sequence number
    /// to ask for next time.
    pub fn packets(&self, after_seq: u32, limit: usize) -> (Vec<PacketEvent>, u32) {
        let packets = self.packets.lock();
        let events: Vec<PacketEvent> = packets
            .events
            .iter()
            .filter(|event| event.seq > after_seq)
            .take(limit)
            .cloned()
            .collect();
        let next = events
            .last()
            .map(|event| event.seq.saturating_add(1))
            .unwrap_or(packets.next_seq);
        (events, next)
    }

    /// Nodes whose running configuration a restart would lose.
    pub fn unsaved_nodes(&self) -> Vec<String> {
        let saves = self.saves.lock().clone();
        let mut names: Vec<String> = self.with(|lab| {
            lab.snapshot()
                .nodes
                .into_iter()
                .filter(|node| {
                    saves
                        .get(&node.id)
                        .is_some_and(|save| save.running != save.baseline)
                })
                .map(|node| node.name)
                .collect()
        });
        names.sort();
        names
    }

    /// Look at this node's saved state again on the next tick.
    pub fn measure(&self, node: NodeId) {
        let mut pending = self.measure.lock();
        if !pending.contains(&node) {
            pending.push(node);
        }
    }

    /// Forget what a node's configuration looked like: it has just rebooted,
    /// so the next look re-takes the baseline.
    pub fn forget_baseline(&self, node: NodeId) {
        self.saves.lock().remove(&node);
        self.refresh_saves(&[node]);
    }

    fn refresh_saves(&self, nodes: &[NodeId]) {
        let measured: Vec<(NodeId, Option<(u64, u64)>)> = self.with(|lab| {
            nodes
                .iter()
                .map(|&node| {
                    let measure = match (lab.running_config(node), lab.startup_config(node)) {
                        (Ok(running), Ok(startup)) => Some((hash(&running), hash(&startup))),
                        _ => None,
                    };
                    (node, measure)
                })
                .collect()
        });
        let mut saves = self.saves.lock();
        for (node, measure) in measured {
            let Some((running, startup)) = measure else {
                saves.remove(&node);
                continue;
            };
            let entry = saves.entry(node).or_insert(NodeSave {
                startup,
                baseline: running,
                running,
            });
            if entry.startup != startup {
                // The node was saved (or its startup-config was replaced):
                // from now on this is what a restart would reproduce.
                entry.startup = startup;
                entry.baseline = running;
            }
            entry.running = running;
        }
    }

    /// Take the baseline for every node, after a load or a reset.
    pub fn baseline_all(&self) {
        self.saves.lock().clear();
        let nodes: Vec<NodeId> = self.with(|lab| {
            lab.snapshot()
                .nodes
                .into_iter()
                .filter(|node| node.cli == CliKind::Ios)
                .map(|node| node.id)
                .collect()
        });
        self.refresh_saves(&nodes);
    }
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

// ------------------------------------------------------------------ starting

pub fn running(app: &App, id: &str) -> Option<Arc<RunningLab>> {
    app.state::<LabsState>().labs.lock().get(id).cloned()
}

pub fn running_count(app: &App) -> usize {
    app.state::<LabsState>().labs.lock().len()
}

/// The running lab, or a refusal that says to start it.
pub fn require_running(app: &App, id: &str) -> AppResult<Arc<RunningLab>> {
    if let Some(lab) = running(app, id) {
        return Ok(lab);
    }
    let record = model::load(app, id)?;
    Err(AppError::unavailable(format!(
        "the lab `{}` is not running; start it first",
        record.name()
    )))
}

/// Start a lab if it is not running already. Loading, booting every device and
/// settling the control plane happens off the async runtime.
pub async fn start(app: &App, id: &str) -> AppResult<Arc<RunningLab>> {
    let state = app.state::<LabsState>();
    let _guard = state.starting.lock().await;
    if let Some(lab) = running(app, id) {
        return Ok(lab);
    }
    let record = model::load(app, id)?;
    let settings = model::settings(app);
    let limit = settings.max_running.max(1) as usize;
    if running_count(app) >= limit {
        return Err(AppError::conflict(format!(
            "{limit} labs already run at once; stop one before starting `{}`",
            record.name()
        )));
    }

    let spec = record.spec.clone();
    let settle_us = settings.settle_s.clamp(1, 600).saturating_mul(1_000_000);
    let realistic = settings.realistic_timers;
    let built = tokio::task::spawn_blocking(move || {
        let mut lab = Lab::from_spec(&spec)?;
        lab.set_realistic_timers(realistic);
        lab.settle(settle_us);
        Ok::<Lab, nocturne_netsim::LabError>(lab)
    })
    .await
    .map_err(|err| AppError::internal(format!("the lab could not be loaded: {err}")))?
    .map_err(model::sim_error)?;

    let (wake, _) = watch::channel(0);
    let lab = Arc::new(RunningLab {
        id: record.id.clone(),
        lab: Mutex::new(built),
        speed: RwLock::new(settings.default_speed.clamp(0.0, 16.0)),
        capture: AtomicBool::new(false),
        packets: Mutex::new(Packets {
            events: VecDeque::new(),
            capacity: settings.capture_events.clamp(100, 200_000) as usize,
            next_seq: 0,
            sent_seq: 0,
        }),
        devices: RwLock::new(HashMap::new()),
        saves: Mutex::new(HashMap::new()),
        measure: Mutex::new(Vec::new()),
        consoles: Mutex::new(Vec::new()),
        dirty: AtomicBool::new(false),
        wake,
        closing: RwLock::new("the lab was stopped".to_string()),
        cancel: app.shutdown_token().child_token(),
    });
    lab.baseline_all();
    state.labs.lock().insert(record.id.clone(), lab.clone());
    // The devices may need creating (a lab that was imported while stopped).
    devices::sync(app, &record)?;
    refresh_devices(app, &lab, &record)?;

    app.spawn("labs.ticker", ticker(app.clone(), lab.clone()));
    app.audit(
        "labs.start",
        None,
        format!("Started lab `{}`", record.name()),
        json!({"lab_id": record.id}),
    );
    publish_changed(app, &record.id, "started");
    Ok(lab)
}

/// Stop a lab: its specification is written back first, so what was saved on a
/// console survives and an unsaved running configuration does not.
pub fn stop(app: &App, id: &str) -> AppResult<Vec<String>> {
    let record = model::load(app, id)?;
    let Some(lab) = running(app, id) else {
        return Ok(Vec::new());
    };
    let unsaved = lab.unsaved_nodes();
    save_spec(app, &lab)?;
    app.state::<LabsState>().labs.lock().remove(id);
    lab.close("the lab was stopped");
    app.audit(
        "labs.stop",
        None,
        format!("Stopped lab `{}`", record.name()),
        json!({"lab_id": id, "unsaved_nodes": unsaved}),
    );
    publish_changed(app, id, "stopped");
    Ok(unsaved)
}

/// Stop a lab without writing anything back (it is about to be deleted).
pub fn discard(app: &App, id: &str, reason: &str) {
    let lab = app.state::<LabsState>().labs.lock().remove(id);
    if let Some(lab) = lab {
        lab.close(reason);
    }
}

/// Write the running lab's specification back to the store, keeping the name
/// and description the record holds (the simulator has no setter for the name).
pub fn save_spec(app: &App, lab: &RunningLab) -> AppResult<()> {
    let mut record = model::load(app, &lab.id)?;
    let mut spec = lab.with(|lab| lab.to_spec());
    spec.name = record.spec.name.clone();
    spec.description = record.spec.description.clone();
    if record.spec == spec {
        lab.dirty.store(false, Ordering::Relaxed);
        return Ok(());
    }
    record.spec = spec;
    model::save(app, &mut record)?;
    lab.dirty.store(false, Ordering::Relaxed);
    Ok(())
}

/// Bring the stored specification and the inventory in line with a lab that
/// has just been edited: a running lab is written back first, so the devices
/// are created for the nodes that are really there.
pub fn settle_devices(app: &App, lab_id: &str) -> AppResult<()> {
    if let Some(lab) = running(app, lab_id) {
        save_spec(app, &lab)?;
    }
    let record = model::load(app, lab_id)?;
    devices::sync(app, &record)?;
    if let Some(lab) = running(app, lab_id) {
        refresh_devices(app, &lab, &record)?;
    }
    Ok(())
}

/// Rebuild the node → device map from the inventory.
pub fn refresh_devices(app: &App, lab: &RunningLab, record: &LabRecord) -> AppResult<()> {
    let by_node = devices::device_ids(app, &record.id)?;
    let by_id = lab.with(|sim| {
        by_node
            .iter()
            .filter_map(|(node, device)| Some((sim.node_by_name(node)?, device.clone())))
            .collect()
    });
    *lab.devices.write() = by_id;
    Ok(())
}

// -------------------------------------------------------------------- ticker

/// One link's state as the canvas draws it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LinkLight {
    pub id: LinkId,
    pub up: bool,
    pub a_light: Light,
    pub b_light: Light,
    pub bps_ab: u64,
    pub bps_ba: u64,
}

fn link_lights(snapshot: &LabSnapshot) -> Vec<LinkLight> {
    snapshot
        .links
        .iter()
        .map(|link| LinkLight {
            id: link.id,
            up: link.up,
            a_light: link.a_light,
            b_light: link.b_light,
            bps_ab: link.bps_ab,
            bps_ba: link.bps_ba,
        })
        .collect()
}

struct Step {
    events: Vec<LabEvent>,
    lights: Option<Vec<LinkLight>>,
    sim_time: u64,
}

async fn ticker(app: App, lab: Arc<RunningLab>) {
    let mut settings = model::settings(&app);
    let mut interval = tokio::time::interval(tick_period(&settings));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut ticks: u64 = 0;
    let mut lights: Vec<LinkLight> = Vec::new();
    let mut last_tick_event = 0_i64;
    let mut last_changed_event = 0_i64;
    let mut last_packet_event = 0_i64;
    let mut changed_reason: Option<&'static str> = None;
    let mut dirty_since: Option<i64> = None;

    loop {
        tokio::select! {
            _ = lab.cancel.cancelled() => break,
            _ = interval.tick() => {}
        }
        ticks += 1;
        if ticks.is_multiple_of(SETTINGS_EVERY) {
            let fresh = model::settings(&app);
            if tick_period(&fresh) != tick_period(&settings) {
                interval = tokio::time::interval(tick_period(&fresh));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            }
            settings = fresh;
        }
        let now = now_ms();
        let want_lights = now - last_tick_event >= TICK_EVENT_MS;
        // `advance` counts microseconds; a tick is `tick_ms × speed` of them.
        let micros =
            (settings.tick_ms.clamp(5, 1_000) as f64 * 1_000.0 * f64::from(lab.speed())) as u64;

        let step = lab.with(|sim| {
            if micros > 0 {
                sim.advance(micros);
            }
            let events = sim.take_events();
            Step {
                events,
                lights: want_lights.then(|| link_lights(&sim.snapshot())),
                sim_time: sim.now(),
            }
        });

        // Everything below runs with the simulator unlocked.
        let mut changed_nodes: Vec<NodeId> = Vec::new();
        let mut packets: Vec<PacketEvent> = Vec::new();
        for event in step.events {
            match event {
                LabEvent::Syslog {
                    node,
                    severity,
                    facility,
                    mnemonic,
                    message,
                    line,
                    time,
                } => {
                    let device_id = lab.devices.read().get(&node).cloned();
                    let entry = LogEntry {
                        id: 0,
                        ts: now,
                        stream: "syslog".into(),
                        device_id,
                        severity: Some(severity),
                        source: format!("%{facility}-{severity}-{mnemonic}"),
                        message,
                        body: Some(json!({"line": line, "lab_id": lab.id, "sim_time": time})),
                    };
                    if let Err(err) = app.store().log_append(&entry) {
                        tracing::warn!(%err, "a lab syslog line could not be stored");
                    }
                }
                LabEvent::Packet(event) => packets.push(event),
                LabEvent::NodeChanged { node } => {
                    changed_nodes.push(node);
                    changed_reason = Some("node");
                    lab.touch();
                }
                LabEvent::LinkState { .. } => {
                    changed_reason.get_or_insert("topology");
                    lab.touch();
                }
                LabEvent::TraceDone { trace, ok, summary } => app.publish(
                    "labs.trace_done",
                    &json!({"lab_id": lab.id, "trace_id": trace, "ok": ok, "summary": summary}),
                ),
            }
        }
        // `write memory` changes nothing a device would announce, so the
        // nodes somebody typed on are looked at too, and all of them now and
        // then.
        changed_nodes.extend(lab.measure.lock().drain(..));
        if ticks.is_multiple_of(SETTINGS_EVERY) {
            changed_nodes.extend(lab.saves.lock().keys().copied());
        }
        if !changed_nodes.is_empty() {
            changed_nodes.sort_unstable();
            changed_nodes.dedup();
            lab.refresh_saves(&changed_nodes);
        }
        if !packets.is_empty() {
            let mut buffer = lab.packets.lock();
            for event in packets {
                buffer.next_seq = buffer.next_seq.max(event.seq.saturating_add(1));
                buffer.events.push_back(event);
            }
            while buffer.events.len() > buffer.capacity {
                buffer.events.pop_front();
            }
        }
        if now - last_packet_event >= PACKET_EVENT_MS {
            last_packet_event = now;
            publish_packets(&app, &lab);
        }
        if let Some(fresh) = step.lights {
            last_tick_event = now;
            if fresh != lights {
                lights = fresh;
                app.publish(
                    "labs.tick",
                    &json!({"lab_id": lab.id, "sim_time": step.sim_time, "links": lights}),
                );
            }
        }
        if let Some(reason) = changed_reason
            && now - last_changed_event >= CHANGED_EVENT_MS
        {
            last_changed_event = now;
            changed_reason = None;
            publish_changed(&app, &lab.id, reason);
        }
        // Autosave once the lab has been quiet for a moment.
        match (lab.dirty.load(Ordering::Relaxed), dirty_since) {
            (true, None) => dirty_since = Some(now),
            (true, Some(since)) if now - since >= settings.autosave_ms as i64 => {
                dirty_since = None;
                if let Err(err) = save_spec(&app, &lab) {
                    tracing::warn!(%err, lab = %lab.id, "a lab could not be saved");
                }
            }
            (false, _) => dirty_since = None,
            _ => {}
        }
        lab.wake.send_modify(|seen| *seen = seen.wrapping_add(1));
    }
    // Stopped: let the consoles of this lab shut down tidily.
    lab.wake.send_modify(|seen| *seen = seen.wrapping_add(1));
}

fn tick_period(settings: &Settings) -> Duration {
    Duration::from_millis(settings.tick_ms.clamp(5, 1_000))
}

fn publish_packets(app: &App, lab: &RunningLab) {
    let pending: Vec<PacketEvent> = {
        let mut buffer = lab.packets.lock();
        let from = buffer.sent_seq;
        let batch: Vec<PacketEvent> = buffer
            .events
            .iter()
            .filter(|event| event.seq >= from)
            .take(PACKET_BATCH)
            .cloned()
            .collect();
        if let Some(last) = batch.last() {
            buffer.sent_seq = last.seq.saturating_add(1);
        }
        batch
    };
    if pending.is_empty() {
        return;
    }
    let next_seq = pending
        .last()
        .map(|event| event.seq.saturating_add(1))
        .unwrap_or_default();
    app.publish(
        "labs.packets",
        &json!({"lab_id": lab.id, "events": pending, "next_seq": next_seq}),
    );
}

pub fn publish_changed(app: &App, lab_id: &str, reason: &str) {
    app.publish("labs.changed", &json!({"lab_id": lab_id, "reason": reason}));
}

// --------------------------------------------------------------------- edits

/// Apply a change to a lab whether or not it is running: a running lab is
/// edited in place, a stopped one through a temporary simulator so that every
/// validation rule is the simulator's own.
pub fn edit<T>(app: &App, id: &str, change: impl FnOnce(&mut Lab) -> AppResult<T>) -> AppResult<T> {
    if let Some(lab) = running(app, id) {
        let outcome = lab.with(change)?;
        lab.touch();
        return Ok(outcome);
    }
    let mut record = model::load(app, id)?;
    let mut lab = Lab::from_spec(&record.spec).map_err(model::sim_error)?;
    let outcome = change(&mut lab)?;
    let mut spec = lab.to_spec();
    spec.name = record.spec.name.clone();
    spec.description = record.spec.description.clone();
    record.spec = spec;
    model::save(app, &mut record)?;
    Ok(outcome)
}

/// The lab's current specification: from the running simulator if it is
/// running, from the store if it is not.
pub fn current_spec(app: &App, id: &str) -> AppResult<(LabRecord, LabSpec)> {
    let record = model::load(app, id)?;
    let spec = match running(app, id) {
        Some(lab) => {
            let mut spec = lab.with(|sim| sim.to_spec());
            spec.name = record.spec.name.clone();
            spec.description = record.spec.description.clone();
            spec
        }
        None => record.spec.clone(),
    };
    Ok((record, spec))
}
