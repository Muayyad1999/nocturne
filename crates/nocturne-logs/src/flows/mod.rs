//! The flow collector: NetFlow v5, NetFlow v9 and IPFIX on one UDP socket,
//! added up in memory into the answer to "who is filling this link".
//!
//! The engine (`nocturne_netprobe::flow`) owns the socket and the decoding;
//! what it decoded waits on a queue with a limit, and one task here takes
//! it off and counts it into [`table::Table`]. That is all there is: no
//! flow is stored, published or logged, so the collector costs memory that
//! is bounded and no disk at all — and everything it knows is gone when
//! the application stops, which is why every answer says what it covers.
//!
//! Four things a number from here must not be mistaken for:
//!
//! * **It is the exporter's word.** Flow export is unauthenticated UDP.
//!   Whoever can reach the port can send records about traffic that never
//!   was, from any address. So nothing derived from a flow ever changes a
//!   device, and an exporter no device claims is shown as exactly that.
//! * **It is when it was said, not when it happened.** A record arrives
//!   when the exporter's cache lets the flow go, up to its active timeout
//!   after the traffic; NetFlow v5 and v9 stamp flows relative to the
//!   exporter's boot. Buckets are therefore keyed by the time of receipt.
//! * **It may be a sample.** An exporter that looks at one packet in a
//!   hundred reports what it saw. The rate is reported beside the counts
//!   and never multiplied in.
//! * **It may be seen twice.** A conversation that crosses two exporters
//!   is reported by both. An answer over several exporters adds both up;
//!   `coverage.exporters` says how many it is over.

pub mod label;
pub mod table;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult, Router, inventory};
use nocturne_netprobe::flow::{
    FlowBatch, FlowCollector, FlowCounters, FlowQueue, QueueLimits, SharedDecoder,
};
use parking_lot::Mutex;
use serde::Deserialize;

use self::table::{BUCKET_MS, Counts, Dimension, KEEP_MINUTES, Key, Selection, Summary, Table};
use crate::attribute::Index;
use crate::ingest::INDEX_MAX_AGE_MS;
use crate::model::{
    FlowCounts, FlowCoverage, FlowDrops, FlowExporter, FlowInterface, FlowMemory, FlowRow,
    FlowStatus, FlowTop,
};
use crate::settings::LogsSettings;
use crate::state::{LogsState, RateWindow};
use crate::status;

/// The most rows one answer carries.
const MAX_ROWS: usize = 500;

/// What the collector was last asked to listen on. Only this decides
/// whether the socket is rebound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowBinding {
    pub enabled: bool,
    pub port: u16,
    pub bind: String,
}

/// What the engine's collectors have counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Collected {
    received: u64,
    malformed: u64,
    records: u64,
    sets_without_template: u64,
    queue_full: u64,
}

impl Collected {
    fn of(counters: &FlowCounters) -> Collected {
        let stats = counters.stats();
        Collected {
            received: stats.packets + stats.errors,
            malformed: stats.errors,
            records: stats.records,
            sets_without_template: stats.missing_template_sets,
            queue_full: counters.dropped().records,
        }
    }

    fn plus(self, other: Collected) -> Collected {
        Collected {
            received: self.received + other.received,
            malformed: self.malformed + other.malformed,
            records: self.records + other.records,
            sets_without_template: self.sets_without_template + other.sets_without_template,
            queue_full: self.queue_full + other.queue_full,
        }
    }
}

/// The running collector, or why there is none.
#[derive(Default)]
pub struct FlowReceiver {
    applied: Option<FlowBinding>,
    /// Kept so that the socket closes when it is stopped or replaced.
    collector: Option<FlowCollector>,
    /// The counters of the collector that is running — or of one that is
    /// being stopped, until its last numbers are added to `past`.
    counters: Option<FlowCounters>,
    /// What collectors that have since been stopped had counted. A
    /// collector starts from zero; the status does not.
    past: Collected,
    pub listening: Vec<String>,
    pub error: Option<String>,
    pub enabled: bool,
}

/// A byte limit that says the setting's default until the settings are read.
struct ByteLimit(AtomicU64);

impl Default for ByteLimit {
    fn default() -> Self {
        ByteLimit(AtomicU64::new(crate::state::DEFAULT_QUEUE_BYTES as u64))
    }
}

/// Everything the flow collector owns.
#[derive(Default)]
pub struct FlowState {
    pub receiver: Mutex<FlowReceiver>,
    /// One `apply` at a time.
    applying: tokio::sync::Mutex<()>,
    pub table: Mutex<Table>,
    rate: Mutex<RateWindow>,
    last_at: AtomicI64,
    /// Flow records added to the buckets.
    counted: AtomicU64,
    /// Flow records dropped because the inventory could not be read.
    unreadable: AtomicU64,
    /// The `unknown_sources: "drop"` setting, as the counting task reads it
    /// for every datagram.
    drop_unclaimed: AtomicBool,
    /// The `queue_limit_bytes` setting as the running collector was started
    /// with it — like `queue_limit`, it is read when a collector starts —
    /// or, with none running, as one would be.
    max_queued_bytes: ByteLimit,
    /// What the exporters announced — their templates, the sampling rates in
    /// their options — handed to every collector this application starts.
    /// It is theirs, not a socket's: a collector that is moved or switched
    /// off and on again reads what they send next with what they said
    /// before, instead of counting their sampled records as unsampled until
    /// they repeat their options.
    decoder: SharedDecoder,
}

impl FlowState {
    pub fn status(&self, now: i64) -> FlowStatus {
        let (enabled, listening, error, collected, queued, queued_bytes) = {
            let receiver = self.receiver.lock();
            let waiting = |of: fn(&FlowCounters) -> usize| {
                receiver.counters.as_ref().map(of).unwrap_or_default() as u64
            };
            (
                receiver.enabled,
                receiver.listening.clone(),
                receiver.error.clone(),
                receiver.past.plus(
                    receiver
                        .counters
                        .as_ref()
                        .map(Collected::of)
                        .unwrap_or_default(),
                ),
                waiting(FlowCounters::waiting),
                waiting(FlowCounters::waiting_bytes),
            )
        };
        let (held, released, exporters) = {
            let mut table = self.table.lock();
            // Nothing else runs on a timer here: what is a day old goes
            // when somebody looks, or when the next datagram arrives.
            table.tidy(now);
            (table.held(), table.released(), table.exporters())
        };
        let drops = FlowDrops {
            queue_full: collected.queue_full,
            no_room: released.refused_records,
            unclaimed: released.unclaimed_records,
            unreadable: self.unreadable.load(Ordering::Relaxed),
        };
        FlowStatus {
            enabled,
            implemented: true,
            listening,
            error,
            received: collected.received,
            malformed: collected.malformed,
            records: collected.records,
            counted: self.counted.load(Ordering::Relaxed),
            dropped: drops.queue_full + drops.no_room + drops.unclaimed + drops.unreadable,
            drops,
            queued,
            queued_bytes,
            max_queued_bytes: self.max_queued_bytes.0.load(Ordering::Relaxed),
            rate_per_minute: self.rate.lock().per_minute(now),
            last_at: match self.last_at.load(Ordering::Relaxed) {
                0 => None,
                at => Some(at),
            },
            sets_without_template: collected.sets_without_template,
            exporters: held.exporters as u64,
            unattributed_exporters: exporters
                .iter()
                .filter(|exporter| exporter.device_id.is_none())
                .count() as u64,
            memory: FlowMemory {
                minutes: held.minutes as u64,
                keys: held.keys as u64,
                max_keys: table::MAX_KEYS as u64,
                evicted_minutes: released.evicted_minutes,
                oldest_at: held.oldest_at,
            },
            lab: "A simulated device exports no flows, so nothing counted here came from a lab."
                .to_string(),
        }
    }
}

/// Stop whatever is running, then start what the settings ask for.
///
/// Never returns an error for a port that cannot be bound: that is a state
/// of the collector, not a failure of the call that tried to start it.
/// What has been counted stays: the buckets belong to the application, not
/// to the socket.
pub async fn apply(app: &App) -> AppResult<()> {
    let state = app.state::<LogsState>();
    let flows = &state.flows;
    // One at a time: two calls that each stop the old socket and bind a new
    // one would find the port taken by each other.
    let _turn = flows.applying.lock().await;
    let settings = LogsSettings::load(app);
    // Read for every datagram, so changing it needs no new socket.
    flows
        .drop_unclaimed
        .store(settings.drop_unknown(), Ordering::Relaxed);

    let wanted = FlowBinding {
        enabled: settings.flows.enabled,
        port: settings.flows.port,
        bind: settings.flows.bind.trim().to_string(),
    };
    let previous = {
        let mut receiver = flows.receiver.lock();
        // Nothing to do when the collector is already running exactly this:
        // rebinding would lose what arrives meanwhile. One that could not
        // start is tried again — whatever held the port may have gone.
        if receiver.applied.as_ref() == Some(&wanted)
            && (receiver.collector.is_some() || !wanted.enabled)
        {
            return Ok(());
        }
        receiver.applied = Some(wanted.clone());
        receiver.enabled = wanted.enabled;
        receiver.listening.clear();
        receiver.error = None;
        receiver.collector.take()
    };
    if let Some(previous) = previous {
        // The old socket is closed before the new one is bound: the port
        // may well be the same one. What it had decoded is still on its
        // queue and is counted by the task that reads it.
        previous.stop().await;
        let mut receiver = flows.receiver.lock();
        if let Some(counters) = receiver.counters.take() {
            // Final now that it has stopped.
            receiver.past = receiver.past.plus(Collected::of(&counters));
        }
    }

    flows
        .max_queued_bytes
        .0
        .store(settings.queue_limit_bytes, Ordering::Relaxed);
    if !wanted.enabled {
        status::publish(app);
        return Ok(());
    }
    let address: IpAddr = match wanted.bind.parse() {
        Ok(address) => address,
        Err(_) => {
            fail(
                app,
                format!(
                    "`{}` is not an address this machine can listen on (try 0.0.0.0 for every interface)",
                    wanted.bind
                ),
            );
            return Ok(());
        }
    };
    let limits = QueueLimits {
        records: settings.queue_limit as usize,
        bytes: settings.queue_limit_bytes as usize,
    };
    let (collector, queue) = match FlowCollector::start(
        SocketAddr::new(address, wanted.port),
        limits,
        flows.decoder.clone(),
    )
    .await
    {
        Ok(started) => started,
        Err(err) => {
            // The engine's message says which address and what to do.
            fail(app, err.to_string());
            return Ok(());
        }
    };
    let listening = vec![format!("udp {}", collector.local_addr())];
    {
        let mut receiver = flows.receiver.lock();
        receiver.listening = listening.clone();
        receiver.counters = Some(collector.counters());
        receiver.collector = Some(collector);
    }
    app.spawn("logs.flows.count", count(app.clone(), queue));
    app.audit(
        "logs.receiver.start",
        None,
        format!("Flow collector listening on {}", listening.join(", ")),
        serde_json::json!({"receiver": "flows", "listening": listening}),
    );
    status::publish(app);
    Ok(())
}

fn fail(app: &App, error: String) {
    tracing::warn!(%error, "the flow collector could not start");
    app.state::<LogsState>().flows.receiver.lock().error = Some(error);
    status::publish(app);
}

/// The counting task: everything the collector decoded, into the buckets.
///
/// It ends when its queue is closed and empty — after the collector it
/// belongs to has been stopped *and* everything that collector had decoded
/// is counted.
async fn count(app: App, mut queue: FlowQueue) {
    while let Some(batch) = queue.recv().await {
        take(&app, &batch, now_ms());
    }
}

/// Count what one datagram held, received at `now`.
///
/// This is the only way into the buckets. The counting task calls it with
/// the clock; a test that needs a datagram to have arrived yesterday calls
/// it with yesterday. `None` when the inventory could not be read to say
/// whose the exporter is: the records are then counted as dropped.
pub fn take(app: &App, batch: &FlowBatch, now: i64) -> Option<table::Taken> {
    let state = app.state::<LogsState>();
    let flows = &state.flows;
    let index = match crate::attribute::refresh(app, now, INDEX_MAX_AGE_MS) {
        Ok(index) => index,
        Err(err) => {
            tracing::error!(%err, "the inventory could not be read to attribute a flow exporter");
            flows
                .unreadable
                .fetch_add(batch.records.len() as u64, Ordering::Relaxed);
            return None;
        }
    };
    let taken = flows.table.lock().add(
        batch,
        now,
        index.built_at,
        || index.attribute(&batch.exporter.to_string()),
        flows.drop_unclaimed.load(Ordering::Relaxed),
    );
    if taken != table::Taken::NoRoom {
        // An exporter that is not followed does not move the clock either:
        // "last heard" is about the ones that are listed.
        flows.last_at.fetch_max(now, Ordering::Relaxed);
        flows.rate.lock().record(now, 1);
    }
    if taken == table::Taken::Counted {
        flows
            .counted
            .fetch_add(batch.records.len() as u64, Ordering::Relaxed);
    }
    Some(taken)
}

/// The address the collector is listening on, for tests and for the
/// "point your devices here" hint.
pub fn listening(app: &App) -> Vec<String> {
    app.state::<LogsState>()
        .flows
        .receiver
        .lock()
        .listening
        .clone()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct TopParams {
    device_id: Option<String>,
    /// One exporter by its address: the way to look at one no device claims.
    exporter: Option<String>,
    minutes: Option<i64>,
    by: Option<String>,
    limit: Option<usize>,
}

pub fn register(router: &mut Router) {
    router.add("flows.top", |app: App, params: TopParams| async move {
        top(&app, params, now_ms()).await
    });
    router.add(
        "flows.exporters",
        |app: App, _: nocturne_app::NoParams| async move { exporters(&app, now_ms()) },
    );
}

fn counts(counts: Counts) -> FlowCounts {
    FlowCounts {
        bytes: counts.bytes,
        packets: counts.packets,
        flows: counts.flows,
    }
}

/// The current index, and every exporter attributed by it.
fn attributed(app: &App, now: i64) -> AppResult<Arc<Index>> {
    let index = crate::attribute::refresh(app, now, INDEX_MAX_AGE_MS)?;
    let state = app.state::<LogsState>();
    state
        .flows
        .table
        .lock()
        .attribute_all(index.built_at, |address| {
            index.attribute(&address.to_string())
        });
    Ok(index)
}

/// `flows.exporters`: who has sent flow export, known to the inventory or not.
fn exporters(app: &App, now: i64) -> AppResult<Vec<FlowExporter>> {
    let index = attributed(app, now)?;
    let state = app.state::<LogsState>();
    let listed = {
        let mut table = state.flows.table.lock();
        table.tidy(now);
        table.exporters()
    };
    Ok(listed
        .into_iter()
        .map(|exporter| FlowExporter {
            address: exporter.address.to_string(),
            device: exporter
                .device_id
                .as_deref()
                .map(|id| index.name_of(id))
                .filter(|name| !name.is_empty()),
            device_id: exporter.device_id,
            reason: exporter.reason,
            version: exporter.version,
            first_at: exporter.first_at,
            last_at: exporter.last_at,
            datagrams: exporter.datagrams,
            records: exporter.records,
            dropped: exporter.dropped,
            templates: exporter.templates,
            sets_without_template: exporter.sets_without_template,
            sampling: exporter.rates,
        })
        .collect())
}

/// One way of looking, added up over a window: per key, what there was
/// besides, and what the window covers.
#[derive(Default)]
struct Folded {
    /// Keyed by the exporter (its place in the slices) as well when the key
    /// only means something per exporter: an ifIndex is a device's own.
    keys: HashMap<(usize, Key), Counts>,
    other: Counts,
    total: Counts,
    records: u64,
    sets_without_template: u64,
    sampled_records: u64,
    rates: Vec<u32>,
    first_minute: Option<i64>,
    exporters: u64,
}

impl Folded {
    fn minute(&mut self, place: usize, summary: &Summary, keys: &[(Key, Counts)], other: Counts) {
        for (key, counts) in keys {
            self.keys.entry((place, *key)).or_default().add(*counts);
        }
        self.other.add(other);
        self.total.add(summary.total);
        self.records += summary.records;
        self.sets_without_template += summary.sets_without_template;
        self.sampled_records += summary.sampled_records;
        for rate in &summary.rates {
            table::note_rate(&mut self.rates, *rate);
        }
        self.first_minute = Some(
            self.first_minute
                .map_or(summary.minute, |first| first.min(summary.minute)),
        );
    }
}

/// `flows.top`: the biggest talkers of the last `minutes`, one way of
/// looking at them, and what the answer covers.
async fn top(app: &App, params: TopParams, now: i64) -> AppResult<FlowTop> {
    let ways = || Dimension::ALL.map(|dimension| dimension.name()).join(", ");
    let Some(by) = params
        .by
        .as_deref()
        .map(str::trim)
        .filter(|by| !by.is_empty())
    else {
        return Err(AppError::invalid(format!(
            "`flows.top` needs `by`: one of {}",
            ways()
        )));
    };
    let Some(dimension) = Dimension::parse(by) else {
        return Err(AppError::invalid(format!(
            "`{}` is not something flows are added up by: `by` is one of {}",
            crate::rules::plain(by, 40),
            ways()
        )));
    };
    let minutes = params.minutes.unwrap_or(15);
    if !(1..=KEEP_MINUTES).contains(&minutes) {
        return Err(AppError::invalid(format!(
            "`minutes` is 1 to {KEEP_MINUTES}: flows are kept for a day, in memory"
        )));
    }
    let limit = params.limit.unwrap_or(20).clamp(1, MAX_ROWS);
    let mut selection = Selection::default();
    if let Some(reference) = params
        .device_id
        .as_deref()
        .filter(|id| !id.trim().is_empty())
    {
        // By id, name or address, as everywhere; an unknown device is an
        // error, not an empty answer that reads as "no traffic".
        selection.device_id = Some(inventory::find(app, reference)?.id);
    }
    if let Some(address) = params
        .exporter
        .as_deref()
        .map(str::trim)
        .filter(|address| !address.is_empty())
    {
        selection.exporter = Some(address.parse::<IpAddr>().map_err(|_| {
            AppError::invalid("`exporter` is the address a flow exporter sends from, as `flows.exporters` lists it")
        })?);
    }

    let index = attributed(app, now)?;
    let state = app.state::<LogsState>();
    let first_minute = now.div_euclid(BUCKET_MS) - minutes;
    let slices = {
        let mut table = state.flows.table.lock();
        table.tidy(now);
        table.slices(&selection, dimension, first_minute)
    };
    // Added up outside the lock: the receiver is not kept waiting for an
    // answer over a day of every exporter.
    let per_exporter = dimension == Dimension::Interface;
    let mut folded = Folded::default();
    for (place, slice) in slices.iter().enumerate() {
        let place = if per_exporter { place } else { 0 };
        for minute in &slice.minutes {
            let (keys, other) = minute.talkers(dimension);
            folded.minute(place, &minute.summary, keys, other);
        }
        if let Some((summary, keys, other)) = &slice.running {
            folded.minute(place, summary, keys, *other);
        }
        folded.exporters += 1;
    }

    let mut biggest: Vec<((usize, Key), Counts)> = folded.keys.into_iter().collect();
    biggest.sort_by(|a, b| {
        b.1.bytes
            .cmp(&a.1.bytes)
            .then(b.1.packets.cmp(&a.1.packets))
            .then(b.1.flows.cmp(&a.1.flows))
            .then_with(|| a.0.cmp(&b.0))
    });
    biggest.truncate(limit);

    let mut rows = Vec::with_capacity(biggest.len());
    for ((place, key), found) in biggest {
        let (key_text, label_text, interface) = match key {
            Key::Interface(if_index, direction) => {
                let slice = &slices[place];
                let interface = interface_of(app, &index, slice, if_index, direction, now).await;
                let name = match (&interface.name, if_index) {
                    (Some(name), _) => name.clone(),
                    (None, 0) => "no interface reported".to_string(),
                    (None, _) => format!("ifIndex {if_index}"),
                };
                // Whose interface, when the answer is over more than one
                // exporter: every router has a port 3.
                let whose = (slices.len() > 1).then(|| {
                    slice
                        .device_id
                        .as_deref()
                        .map(|id| index.name_of(id))
                        .filter(|device| !device.is_empty())
                        .unwrap_or_else(|| slice.address.to_string())
                });
                let label = match whose {
                    Some(whose) => format!("{whose} {name} {}", interface.direction),
                    None => format!("{name} {}", interface.direction),
                };
                (
                    format!("{} {if_index} {}", slice.address, interface.direction),
                    label,
                    Some(interface),
                )
            }
            Key::Application(protocol, port) => (
                label::application_key(protocol, port),
                label::application_label(protocol, port),
                None,
            ),
            Key::Dscp(dscp) => (dscp.to_string(), label::dscp_label(dscp), None),
            other => {
                let text = label::key(&other);
                (text.clone(), text, None)
            }
        };
        rows.push(FlowRow {
            key: key_text,
            label: label_text,
            bytes: found.bytes,
            packets: found.packets,
            flows: found.flows,
            interface,
        });
    }

    let collecting = state.flows.receiver.lock().collector.is_some();
    Ok(FlowTop {
        rows,
        total: counts(folded.total),
        other: counts(folded.other),
        coverage: FlowCoverage {
            collecting,
            window_start: first_minute * BUCKET_MS,
            until: now,
            since: folded.first_minute.map(|minute| minute * BUCKET_MS),
            records: folded.records,
            sets_without_template: folded.sets_without_template,
            sampling: folded.rates,
            sampled_records: folded.sampled_records,
            exporters: folded.exporters,
        },
    })
}

/// The interface behind an ifIndex of one exporter: by name when the
/// monitoring feature has a map of that device, by number otherwise. No
/// name is made up, and index 0 — "the exporter did not say" — has none.
async fn interface_of(
    app: &App,
    index: &Index,
    slice: &table::Slice,
    if_index: u32,
    direction: table::Direction,
    now: i64,
) -> FlowInterface {
    let mut name = None;
    if if_index != 0
        && let Some(device_id) = slice.device_id.as_deref()
        && let Some(names) = crate::traps::interface_names(app, device_id, now).await
        && let Some(found) = names.get(&if_index)
    {
        // In the device's own collected spelling when it has one, so that
        // a flow, a trap and a syslog line about a port carry one name.
        name = Some(
            index
                .interface_in(Some(device_id), found)
                .map(|(canonical, _)| canonical)
                .unwrap_or_else(|| found.clone()),
        );
    }
    FlowInterface {
        exporter: slice.address.to_string(),
        device_id: slice.device_id.clone(),
        if_index,
        name,
        direction: label::direction_word(direction).to_string(),
    }
}
