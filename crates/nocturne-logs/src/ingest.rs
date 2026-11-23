//! From a datagram to a stored row.
//!
//! The path is deliberately short and has exactly one place where it can
//! fall behind: the queue between the sockets and the writer. The sockets
//! never block, never allocate beyond the message they just parsed and never
//! touch the database; the writer takes whatever has piled up, up to a batch,
//! attributes it, redacts it and commits it in one transaction. When the
//! queue is full the line is dropped and counted — `logs.status` says how
//! many — because the alternative is a receiver that eats memory until the
//! machine dies, by which time the logs are gone anyway.
//!
//! The path has two halves. The first is a receiver's own: turning what it
//! took off the wire into rows ([`prepare`] for a syslog message), which is
//! where attribution and redaction happen. The second, [`commit`], is the
//! same for every receiver and for a lab's lines: the unknown-source
//! policy, one transaction, the live tail, the forwarders, the rules. A row
//! that did not go through `commit` is in no index, on no tail and seen by
//! no rule.

use std::collections::HashMap;

use std::sync::atomic::Ordering;

use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult, inventory};
use nocturne_netprobe::syslog::{SyslogMessage, facility_name, severity_name};
use serde_json::{Map, Value, json};

use crate::attribute::Index;
use crate::db::{self, Prepared};
use crate::settings::LogsSettings;
use crate::state::{BATCH_ROWS, LogsState, ReceiverCounters};
use crate::{guard, rules, tail};

/// How long an attribution index is used before it is read afresh. The index
/// is also thrown away the moment the inventory or the collected interfaces
/// change, so this is only the ceiling for changes made behind the app's back.
pub(crate) const INDEX_MAX_AGE_MS: i64 = 30_000;
/// The longest line `logs.ingest` takes; a longer one is cut there, as the
/// receiver cuts a message that arrives over TCP.
const MAX_LAB_LINE: usize = 64 * 1024;
/// The most lines one `logs.ingest` call may hand over.
const MAX_LAB_LINES: usize = 10_000;
/// How often retention runs.
pub const PRUNE_INTERVAL_MS: u64 = 60_000;
/// Rows one retention pass deletes per transaction.
const PRUNE_BATCH: usize = 2_000;

/// Turn a parsed message into the row that will be stored.
///
/// `via` says how the line reached Nocturne: `syslog` for one the device
/// sent, `pull` for one read back out of its own buffer, `lab` for one a
/// simulated device wrote.
pub fn prepare(
    state: &LogsState,
    index: &Index,
    message: &SyslogMessage,
    received_at: i64,
    via: &str,
    known_device: Option<&str>,
) -> Prepared {
    // A line read back out of a device's own buffer, or handed over by the
    // lab that simulates the device, needs no attribution: how it got here
    // says which device it is.
    let attribution = match known_device {
        Some(device_id) => crate::attribute::Attribution {
            device_id: Some(device_id.to_string()),
            reason: String::new(),
        },
        None => index.attribute(&message.source),
    };
    // A line with no priority (a Cisco buffer line, a device that sends none)
    // still carries the severity the vendor wrote into its own mnemonic.
    // Neither present means unknown, which is not the same as `debug`.
    let severity = message
        .severity
        .or_else(|| message.mnemonic.as_ref().and_then(|m| m.severity));
    let mnemonic = message.mnemonic.as_ref().map(|mnemonic| {
        match (mnemonic.name.is_empty(), mnemonic.severity) {
            (true, _) => mnemonic.facility.clone(),
            (false, Some(severity)) => {
                format!("{}-{severity}-{}", mnemonic.facility, mnemonic.name)
            }
            (false, None) => format!("{}-{}", mnemonic.facility, mnemonic.name),
        }
    });
    let text = if message.message.trim().is_empty() {
        &message.raw
    } else {
        &message.message
    };
    let kept = guard::clean(&state.redactor, text);
    let raw = guard::clean(&state.redactor, &message.raw);
    let interface = index.interface_in(attribution.device_id.as_deref(), &kept);

    let mut body = Map::new();
    body.insert("raw".into(), Value::String(raw.into_owned()));
    body.insert("via".into(), Value::String(via.to_string()));
    if let Some(facility) = message.facility {
        body.insert("facility".into(), json!(facility));
        body.insert("facility_name".into(), json!(facility_name(facility)));
    }
    if let Some(severity) = severity {
        body.insert("severity_name".into(), json!(severity_name(severity)));
    }
    for (key, value) in [
        ("app", &message.app),
        ("host", &message.hostname),
        ("procid", &message.procid),
        ("msgid", &message.msgid),
        ("timestamp", &message.timestamp),
    ] {
        if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
            body.insert(key.into(), Value::String(value.clone()));
        }
    }
    if let Some(mnemonic) = &mnemonic {
        body.insert("mnemonic".into(), Value::String(mnemonic.clone()));
    }
    if let Some(sequence) = message.sequence {
        body.insert("sequence".into(), json!(sequence));
    }
    if let Some((canonical, written)) = &interface {
        body.insert("interface".into(), Value::String(canonical.clone()));
        if written != canonical {
            body.insert("interface_text".into(), Value::String(written.clone()));
        }
    }

    Prepared {
        entry: LogEntry {
            id: 0,
            ts: received_at,
            stream: db::STREAM.into(),
            device_id: attribution.device_id.clone(),
            severity,
            source: message.source.clone(),
            message: kept.into_owned(),
            body: Some(Value::Object(body)),
        },
        facility: message.facility,
        app: message.app.clone(),
        mnemonic,
        host: message.hostname.clone(),
        interface: interface.map(|(canonical, _)| canonical),
        reason: attribution.reason,
        fp: db::fingerprint(&message.raw),
    }
}

/// Write one batch of syslog messages the receiver took off its sockets:
/// attribute, redact, then [`commit`].
///
/// Every one of them ends in the receiver's `stored` or `dropped`, also when
/// the database will not take the batch: a line that was received and is in
/// no counter is a loss nobody can see.
pub fn write_batch(app: &App, messages: &[SyslogMessage], via: &str) -> AppResult<usize> {
    if messages.is_empty() {
        return Ok(0);
    }
    let state = app.state::<LogsState>();
    let counters = &state.syslog.counters;
    let now = now_ms();
    let index = crate::attribute::refresh(app, now, INDEX_MAX_AGE_MS)
        .inspect_err(|_| counters.dropped(messages.len() as u64))?;
    let batch = messages
        .iter()
        .map(|message| prepare(&state, &index, message, now, via, None))
        .collect();
    match commit(app, &index, counters, batch) {
        Ok(kept) => Ok(kept.len()),
        Err(unwritten) => {
            counters.dropped(unwritten.rows as u64);
            Err(unwritten.error)
        }
    }
}

/// A batch [`commit`] could not write: why, and how many of its rows that
/// leaves in no counter. The rows the unknown-source policy turned away are
/// not among them; `commit` has counted those as dropped already.
#[derive(Debug)]
pub struct Unwritten {
    pub error: AppError,
    pub rows: usize,
}

/// The unknown-source policy: whether a row is kept at all. A row no device
/// claims is kept unless the settings say to drop those.
///
/// It is its own function because a receiver that owes its sender an
/// answer — an inform's acknowledgement — must know whether [`commit`]
/// kept the row, and must not decide that by a rule of its own.
pub fn keeps(settings: &LogsSettings, prepared: &Prepared) -> bool {
    prepared.entry.device_id.is_some() || !settings.drop_unknown()
}

/// The half of the path every receiver shares: the unknown-source policy,
/// one transaction, the live tail, the rules. Returns the rows that were
/// kept, each with the id it was given.
///
/// `counters` are those of the receiver the rows came from, so that
/// `logs.status` attributes what was stored and what was refused to it.
/// When the database will not take the batch, the error says how many rows
/// are lost ([`Unwritten`]); whether those were received, and so belong in
/// `dropped`, is the caller's to know — a lab's lines never crossed a socket.
pub fn commit(
    app: &App,
    index: &Index,
    counters: &ReceiverCounters,
    batch: Vec<Prepared>,
) -> Result<Vec<Prepared>, Unwritten> {
    let settings = LogsSettings::load(app);
    let now = now_ms();
    let mut kept = Vec::with_capacity(batch.len());
    let mut refused: HashMap<String, (u64, String)> = HashMap::new();
    for prepared in batch {
        if !keeps(&settings, &prepared) {
            let entry = refused
                .entry(prepared.entry.source.clone())
                .or_insert((0, prepared.reason.clone()));
            entry.0 += 1;
            continue;
        }
        kept.push(prepared);
    }
    // Counted before anything here can fail: they are dropped by the
    // setting whether or not the database then takes the list below.
    counters.dropped(refused.values().map(|(count, _)| count).sum());
    let unwritten = |error: AppError, rows: usize| Unwritten { error, rows };
    // An address whose lines are all discarded is still listed, with a count:
    // "nothing from this address is being kept" is something the user has to
    // be able to see, or the setting silently makes a device invisible.
    for (address, (count, reason)) in &refused {
        db::count_dropped(app.store(), address, now, reason, *count)
            .map_err(|error| unwritten(error, kept.len()))?;
    }
    if kept.is_empty() {
        return Ok(kept);
    }
    let rows = kept.len();
    let written =
        db::insert_batch(app.store(), &mut kept).map_err(|error| unwritten(error, rows))?;
    counters.stored(written.stored as u64);
    tail::offer(app, &kept);
    // After the write, never before: what a forwarder sends on is the row
    // that was kept, redacted. This puts entries on queues and returns; a
    // collector that is slow or gone costs a counter, not a wait.
    crate::forward::offer(app, index, kept.iter().map(|item| &item.entry));
    rules::evaluate(app, index, &kept);
    Ok(kept)
}

/// `logs.ingest`: the lines a lab's simulated device wrote.
///
/// A simulated device has no socket to send from, so the lab hands its
/// lines over by name. From here on they are a datagram's equal — parsed,
/// redacted, indexed, tailed, run through the rules — and say `via: lab`.
///
/// It is refused for anything but a lab device. Otherwise this would be a
/// way to write a log line for a real switch through the API, and a log
/// line is what an alert, a backup and a detection are decided from.
pub fn from_lab(app: &App, device_reference: &str, lines: &[String]) -> AppResult<usize> {
    // The inventory finds a device by id, name or address, and a simulated
    // device has no address: an empty reference would "find" one of them.
    if device_reference.trim().is_empty() {
        return Err(AppError::invalid(
            "`logs.ingest` needs the device the lines are from (`device_id`)",
        ));
    }
    // However the device is referred to, it is the device that was found
    // that is checked, and the same one the lines are stored for: nothing
    // in the parameters can say "lab" on its behalf.
    let device = inventory::find(app, device_reference)?;
    let lab = device
        .lab
        .as_ref()
        .filter(|_| crate::attribute::is_simulated(&device));
    let Some(lab) = lab else {
        return Err(AppError::forbidden(format!(
            "`{}` is not a lab device: `logs.ingest` only takes the lines of simulated devices. \
             A real device's lines arrive at the syslog receiver or are read with `logs.pull`",
            device.name
        )));
    };
    if lines.len() > MAX_LAB_LINES {
        return Err(AppError::invalid(format!(
            "`logs.ingest` takes at most {MAX_LAB_LINES} lines in one call, not {}",
            lines.len()
        )));
    }
    let state = app.state::<LogsState>();
    let now = now_ms();
    let mut index = crate::attribute::refresh(app, now, INDEX_MAX_AGE_MS)?;
    if index.device(&device.id).is_none() {
        // A lab adds its devices and they start talking in the same
        // breath, before the index has heard of them; without the device
        // the rules could not tell that this is a lab.
        index = crate::attribute::refresh(app, now, 0)?;
    }
    // No socket, so no address the line was sent from: the source stays
    // empty rather than naming one the device never used.
    let nowhere = "0.0.0.0:0".parse().expect("a valid socket address");
    let mut batch = Vec::with_capacity(lines.len());
    for line in lines {
        let line = line.trim_end_matches(['\r', '\n']);
        if line.trim().is_empty() {
            continue;
        }
        let mut cut = line.len().min(MAX_LAB_LINE);
        while !line.is_char_boundary(cut) {
            cut -= 1;
        }
        let mut message = nocturne_netprobe::syslog::parse(&line.as_bytes()[..cut], nowhere);
        message.source = String::new();
        batch.push(prepare(
            &state,
            &index,
            &message,
            now,
            "lab",
            Some(&device.id),
        ));
    }
    let mut stored = 0;
    while !batch.is_empty() {
        let rest = batch.split_off(batch.len().min(BATCH_ROWS));
        // A lab's lines were never received, so lost ones are no `dropped`:
        // the lab is told by the error instead.
        stored += commit(app, &index, &state.syslog.counters, batch)
            .map_err(|unwritten| unwritten.error)?
            .len();
        batch = rest;
    }
    state
        .syslog
        .from_lab
        .fetch_add(stored as u64, Ordering::Relaxed);
    // Once per device, not once per call: a lab hands lines over many times
    // a second, and the audit trail is for "this device's log is being
    // written through the API", not for every line of it.
    if stored > 0 && state.syslog.lab_devices.lock().insert(device.id.clone()) {
        app.audit(
            "logs.ingest",
            Some(&device.id),
            format!(
                "Lab device `{}` started handing over its log lines",
                device.name
            ),
            json!({"lab_id": lab.lab_id, "node": lab.node}),
        );
    }
    Ok(stored)
}

/// The retention task: age first, then the row cap.
pub async fn retention(app: App) {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(PRUNE_INTERVAL_MS)).await;
        if let Err(err) = prune_once(&app) {
            tracing::error!(%err, "log retention failed");
        }
    }
}

/// One retention pass. Returns what it removed.
pub fn prune_once(app: &App) -> AppResult<db::Pruned> {
    let settings = LogsSettings::load(app);
    let state = app.state::<LogsState>();
    let now = now_ms();
    let before = (settings.retention_days > 0)
        .then(|| now - i64::from(settings.retention_days) * 86_400_000);
    let pruned = db::prune(app.store(), before, settings.max_rows, PRUNE_BATCH)?;
    state.retention.last_prune_at.store(now, Ordering::Relaxed);
    if pruned.by_age + pruned.by_rows > 0 {
        state
            .retention
            .pruned_by_age
            .fetch_add(pruned.by_age as u64, Ordering::Relaxed);
        state
            .retention
            .pruned_by_rows
            .fetch_add(pruned.by_rows as u64, Ordering::Relaxed);
        crate::status::publish(app);
    }
    Ok(pruned)
}
