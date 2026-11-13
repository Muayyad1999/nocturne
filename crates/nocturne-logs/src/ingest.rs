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

use std::collections::HashMap;

use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppResult};
use nocturne_netprobe::syslog::{SyslogMessage, facility_name, severity_name};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::Receiver;

use crate::attribute::Index;
use crate::db::{self, Prepared};
use crate::settings::LogsSettings;
use crate::state::LogsState;
use crate::{guard, rules, tail};

/// How long an attribution index is used before it is read afresh. The index
/// is also thrown away the moment the inventory or the collected interfaces
/// change, so this is only the ceiling for changes made behind the app's back.
const INDEX_MAX_AGE_MS: i64 = 30_000;
/// How often retention runs.
pub const PRUNE_INTERVAL_MS: u64 = 60_000;
/// Rows one retention pass deletes per transaction.
const PRUNE_BATCH: usize = 2_000;

/// Turn a parsed message into the row that will be stored.
///
/// `via` says how the line reached Nocturne: `syslog` for one the device
/// sent, `pull` for one read back out of its own buffer.
pub fn prepare(
    state: &LogsState,
    index: &Index,
    message: &SyslogMessage,
    received_at: i64,
    via: &str,
    known_device: Option<&str>,
) -> Prepared {
    // A line read back out of a device's own buffer needs no attribution:
    // the session it was read over says which device it is.
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

/// Write one batch: attribute, redact, store, tail, rules.
pub fn write_batch(app: &App, messages: &[SyslogMessage], via: &str) -> AppResult<usize> {
    if messages.is_empty() {
        return Ok(0);
    }
    let state = app.state::<LogsState>();
    let settings = LogsSettings::load(app);
    let now = now_ms();
    let index = crate::attribute::refresh(app, now, INDEX_MAX_AGE_MS)?;

    let mut batch = Vec::with_capacity(messages.len());
    let mut refused: HashMap<String, (u64, String)> = HashMap::new();
    for message in messages {
        let prepared = prepare(&state, &index, message, now, via, None);
        if prepared.entry.device_id.is_none() && settings.drop_unknown() {
            let entry = refused
                .entry(message.source.clone())
                .or_insert((0, prepared.reason.clone()));
            entry.0 += 1;
            continue;
        }
        batch.push(prepared);
    }
    // An address whose lines are all discarded is still listed, with a count:
    // "nothing from this address is being kept" is something the user has to
    // be able to see, or the setting silently makes a device invisible.
    for (address, (count, reason)) in &refused {
        db::count_dropped(app.store(), address, now, reason, *count)?;
        state.counters.dropped(*count);
    }
    if batch.is_empty() {
        return Ok(0);
    }
    let written = db::insert_batch(app.store(), &mut batch)?;
    state.counters.stored(written.stored as u64);
    tail::offer(app, &batch);
    rules::evaluate(app, &index, &batch);
    Ok(written.stored)
}

/// The writer task: everything the sockets hand over, in batches.
pub async fn writer(app: App, mut queue: Receiver<SyslogMessage>, generation: u64) {
    let state = app.state::<LogsState>();
    let mut messages = Vec::with_capacity(crate::state::BATCH_ROWS);
    // `recv_many` returns as soon as there is anything at all, so a quiet
    // network's single line is written straight away and a storm arrives in
    // batches of up to `BATCH_ROWS` without a timer.
    while queue
        .recv_many(&mut messages, crate::state::BATCH_ROWS)
        .await
        > 0
    {
        if state.receiver.lock().generation != generation {
            return;
        }
        if let Err(err) = write_batch(&app, &messages, "syslog") {
            tracing::error!(%err, count = messages.len(), "a batch of syslog lines could not be stored");
        }
        messages.clear();
    }
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
    use std::sync::atomic::Ordering;
    state.counters.last_prune_at.store(now, Ordering::Relaxed);
    if pruned.by_age + pruned.by_rows > 0 {
        state
            .counters
            .pruned_by_age
            .fetch_add(pruned.by_age as u64, Ordering::Relaxed);
        state
            .counters
            .pruned_by_rows
            .fetch_add(pruned.by_rows as u64, Ordering::Relaxed);
        crate::status::publish(app);
    }
    Ok(pruned)
}
