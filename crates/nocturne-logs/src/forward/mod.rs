//! Forwarders: handing what is stored to a log platform.
//!
//! Nocturne keeps a month of the network's logs on one SQLite file for the
//! people who run the network. Years of retention, correlation with what is
//! not a network device, case management — that is a log platform's work,
//! and this module is how it gets the material: syslog over UDP, TCP or TLS
//! as RFC 5424 or JSON, or JSON posted to an HTTP event collector.
//!
//! Four things hold for every forwarder:
//!
//! * **Only what is stored leaves.** The tap is after the write —
//!   `ingest::commit` and `pull::store` for device lines and traps, and for
//!   the `event` and `audit` streams the rows themselves, read back by id
//!   ([`records`]) because every feature writes those through the core. What
//!   a collector receives is the row a person sees on the log screen:
//!   redacted.
//! * **Ingest never waits.** [`offer`] puts an entry on a bounded queue or
//!   counts it as dropped. It does not format, connect, write or sleep; all
//!   of that is the forwarder's own task ([`deliver`]).
//! * **An HTTPS destination is a secret.** The URL of an event collector
//!   and its token are one vault entry, `logs:forward:<id>:secret`. They are
//!   accepted by `logs.forward.save` and returned by nothing: not by the
//!   document, the status, an event, an audit entry, an error or a log line.
//! * **The audit trail leaves only when asked.** It is operator activity,
//!   so no forwarder sends it unless its `streams` names `audit`, and the
//!   audit entry for saving such a forwarder says that it does.

pub mod deliver;
pub mod wire;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use nocturne_app::store::LogEntry;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppError, AppResult, NoParams, Router};
use nocturne_netprobe::syslog::send::{SyslogSender, authorities_from_pem};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::attribute::Index;
use crate::model::{ForwardFormat, ForwardKind, Forwarder, ForwarderStatus};
use crate::state::LogsState;

pub const COLLECTION: &str = "logs_forwarders";
/// Entries that may wait for one forwarder. An entry is the stored row,
/// shared between the forwarders that want it, so this is a few megabytes
/// for a collector that is behind and nothing for one that keeps up.
/// Beyond it entries are dropped and counted: a queue that grows instead
/// only moves the loss to the moment the machine runs out of memory.
pub const QUEUE_LIMIT: usize = 10_000;
/// Each forwarder is a task, a connection and a queue.
pub const MAX_FORWARDERS: usize = 16;
/// The streams a forwarder can send, in the order they are listed.
pub const STREAMS: [&str; 4] = ["syslog", "trap", "event", "audit"];
/// What a forwarder sends when it is not told: everything but the audit
/// trail, which is opt-in.
const DEFAULT_STREAMS: [&str; 3] = ["syslog", "trap", "event"];
/// The streams every feature writes through the core, which are therefore
/// read back rather than tapped.
const RECORD_STREAMS: u8 = bit("event") | bit("audit");
/// How often those are read. Half a second behind is nothing to a log
/// platform, and the read is one indexed range.
const RECORD_POLL: Duration = Duration::from_millis(500);
const RECORD_PAGE: usize = 500;

pub(crate) const IDLE: u8 = 0;
pub(crate) const OK: u8 = 1;
pub(crate) const FAILING: u8 = 2;

const fn bit(stream: &str) -> u8 {
    match stream.as_bytes() {
        b"syslog" => 1,
        b"trap" => 2,
        b"event" => 4,
        b"audit" => 8,
        _ => 0,
    }
}

/// The vault entry holding an HTTPS forwarder's URL and token.
pub fn secret_id(id: &str) -> String {
    format!("logs:forward:{id}:secret")
}

/// An HTTPS forwarder's destination. One secret, because half of it is no
/// use and either half is enough to do harm: the token alone posts to the
/// collector, and a URL often carries a token of its own.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Secret {
    pub url: String,
    #[serde(default)]
    pub token: String,
}

pub(crate) fn read_secret(app: &App, id: &str) -> AppResult<Option<Secret>> {
    let Some(stored) = app.vault().get(&secret_id(id))? else {
        return Ok(None);
    };
    // Not `?`: a parse error quotes what it could not parse.
    serde_json::from_str::<Secret>(&stored)
        .map(Some)
        .map_err(|_| AppError::internal("the stored destination of this forwarder is unreadable"))
}

/// What one forwarder has done since the process started. Kept by id, so
/// saving a forwarder again does not set its numbers back to zero.
#[derive(Default)]
pub struct Counters {
    pub sent: AtomicU64,
    pub dropped: AtomicU64,
    pub last_at: AtomicI64,
    pub last_error: Mutex<Option<String>>,
    /// [`IDLE`], [`OK`] or [`FAILING`].
    pub outcome: AtomicU8,
}

/// A forwarder that is switched on: its queue and the task behind it.
pub struct Running {
    pub forwarder: Forwarder,
    streams: u8,
    queue: mpsc::Sender<Arc<LogEntry>>,
    pub counters: Arc<Counters>,
    pub(crate) stop: CancellationToken,
    /// Which secret the task was started with (see [`ForwardState::secrets`]).
    secret: u64,
    /// The newest row there was when this forwarder was started. It sends
    /// what is stored after that: the `event` and `audit` rows are read
    /// back in pages, and a page may hold rows older than a forwarder that
    /// was saved a moment ago — an audit entry from before anyone asked for
    /// the audit trail to be sent, for one.
    after: i64,
}

impl Running {
    fn wants(&self, entry: &LogEntry, from_a_lab: impl FnOnce() -> bool) -> bool {
        self.streams & bit(&entry.stream) != 0
            && entry.id > self.after
            && self.severe_enough(entry)
            && (self.forwarder.lab || !from_a_lab())
    }

    /// The severity floor, and the two kinds of entry it does not apply to.
    ///
    /// No severity is not "less severe": a trap has none by nature. And
    /// every audit entry is written with the same severity, so a floor
    /// would not thin the audit trail out, it would silence it — for a
    /// forwarder that was asked, by name, to send it.
    fn severe_enough(&self, entry: &LogEntry) -> bool {
        entry.stream == crate::AUDIT_STREAM
            || self
                .forwarder
                .min_severity
                .is_none_or(|floor| entry.severity.is_none_or(|severity| severity <= floor))
    }

    /// Entries waiting to be sent right now.
    pub fn queued(&self) -> usize {
        QUEUE_LIMIT.saturating_sub(self.queue.capacity())
    }
}

#[derive(Default)]
pub struct ForwardState {
    /// The forwarders that are switched on, as the ingest path reads them.
    running: RwLock<Arc<Vec<Arc<Running>>>>,
    counters: Mutex<HashMap<String, Arc<Counters>>>,
    /// Counts the changes to each forwarder's secret. The document does
    /// not change when only the URL or the token does, so this is what
    /// tells [`apply`] that a running task holds an old destination.
    secrets: Mutex<HashMap<String, u64>>,
    /// The id of the last `event` / `audit` row offered. `None` while no
    /// forwarder wants those streams; set to the newest row when one first
    /// does, so that it gets what is recorded from then on and not the
    /// month before.
    cursor: Mutex<Option<i64>>,
    /// One `apply` at a time.
    applying: Mutex<()>,
}

impl ForwardState {
    pub fn running(&self) -> Arc<Vec<Arc<Running>>> {
        self.running.read().clone()
    }

    fn counters_of(&self, id: &str) -> Arc<Counters> {
        self.counters
            .lock()
            .entry(id.to_string())
            .or_default()
            .clone()
    }
}

/// Whether an entry is about a lab's simulated device. The audit trail is
/// what people did, whatever they did it to, and is never a lab's.
fn from_a_lab(index: &Index, entry: &LogEntry) -> bool {
    entry.stream != crate::AUDIT_STREAM
        && entry
            .device_id
            .as_deref()
            .and_then(|id| index.device(id))
            .is_some_and(|device| device.lab)
}

/// Hand stored entries to the forwarders that want them.
///
/// This is on the ingest path and does nothing that can wait: an entry goes
/// on a forwarder's queue, or the queue is full and the forwarder's
/// `dropped` goes up by one. With no forwarder switched on it is one read
/// of an empty list.
pub fn offer<'a>(app: &App, index: &Index, entries: impl IntoIterator<Item = &'a LogEntry>) {
    let running = app.state::<LogsState>().forward.running();
    if running.is_empty() {
        return;
    }
    for entry in entries {
        // Cloned once, and only if somebody has room for it.
        let mut shared: Option<Arc<LogEntry>> = None;
        let mut lab: Option<bool> = None;
        for forwarder in running.iter() {
            if !forwarder.wants(entry, || {
                *lab.get_or_insert_with(|| from_a_lab(index, entry))
            }) {
                continue;
            }
            if forwarder.queue.capacity() == 0 {
                forwarder.counters.dropped.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let entry = shared
                .get_or_insert_with(|| Arc::new(entry.clone()))
                .clone();
            if forwarder.queue.try_send(entry).is_err() {
                forwarder.counters.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// What has to be the same for a running task to be left alone. The name
/// is not part of it: renaming a forwarder must not throw away its queue.
fn same_delivery(a: &Forwarder, b: &Forwarder) -> bool {
    a.kind == b.kind
        && a.host == b.host
        && a.port == b.port
        && a.format == b.format
        && a.streams == b.streams
        && a.min_severity == b.min_severity
        && a.lab == b.lab
        && a.ca_pem == b.ca_pem
}

fn newest_row(app: &App) -> AppResult<i64> {
    app.store().with(|conn| {
        conn.query_row("SELECT COALESCE(MAX(id), 0) FROM logs", [], |row| {
            row.get(0)
        })
    })
}

/// Bring the running tasks in line with the stored forwarders: start what
/// is new or changed, stop what is gone or switched off, leave the rest and
/// their queues alone.
pub fn apply(app: &App) -> AppResult<()> {
    let state = app.state::<LogsState>();
    let forward = &state.forward;
    let _one_at_a_time = forward.applying.lock();
    let mut wanted: Vec<Forwarder> = app.store().list(COLLECTION)?;
    wanted.retain(|forwarder| forwarder.enabled);

    let current = forward.running();
    let newest = newest_row(app)?;
    let mut next: Vec<Arc<Running>> = Vec::with_capacity(wanted.len());
    for forwarder in wanted {
        let secret = forward
            .secrets
            .lock()
            .get(&forwarder.id)
            .copied()
            .unwrap_or_default();
        let unchanged = current.iter().find(|running| {
            running.forwarder.id == forwarder.id
                && running.secret == secret
                && same_delivery(&running.forwarder, &forwarder)
        });
        if let Some(running) = unchanged {
            next.push(running.clone());
            continue;
        }
        let (queue, waiting) = mpsc::channel(QUEUE_LIMIT);
        let running = Arc::new(Running {
            streams: forwarder
                .streams
                .iter()
                .fold(0, |mask, stream| mask | bit(stream)),
            counters: forward.counters_of(&forwarder.id),
            forwarder,
            queue,
            stop: CancellationToken::new(),
            secret,
            after: newest,
        });
        // A forwarder that was failing with its old settings has not failed
        // with these.
        running.counters.outcome.store(IDLE, Ordering::Relaxed);
        *running.counters.last_error.lock() = None;
        app.spawn(
            "logs.forward",
            deliver::run(app.clone(), running.clone(), waiting),
        );
        next.push(running);
    }
    for running in current.iter() {
        if !next.iter().any(|kept| Arc::ptr_eq(kept, running)) {
            running.stop.cancel();
        }
    }

    let wants_records = next
        .iter()
        .any(|running| running.streams & RECORD_STREAMS != 0);
    {
        let mut cursor = forward.cursor.lock();
        match (wants_records, *cursor) {
            (true, None) => *cursor = Some(newest),
            (false, Some(_)) => *cursor = None,
            _ => {}
        }
    }
    *forward.running.write() = Arc::new(next);
    Ok(())
}

/// Start the forwarders and the task that reads the `event` and `audit`
/// streams for them.
pub fn start(app: &App) {
    if let Err(err) = apply(app) {
        tracing::error!(%err, "the log forwarders could not be started");
    }
    app.spawn("logs.forward.records", records(app.clone()));
}

/// Read what has been recorded in the `event` and `audit` streams since the
/// last look and offer it to the forwarders.
///
/// Every feature writes those streams through the core, so there is no
/// batch of this crate's to tap. Reading the rows back by id is the same
/// promise kept another way: what leaves is what was stored.
pub async fn records(app: App) {
    loop {
        tokio::time::sleep(RECORD_POLL).await;
        if let Err(err) = offer_records(&app) {
            tracing::error!(%err, "the event and audit streams could not be read for forwarding");
        }
    }
}

/// One look. Public so that a test can take it without waiting.
pub fn offer_records(app: &App) -> AppResult<()> {
    let state = app.state::<LogsState>();
    loop {
        let Some(after) = *state.forward.cursor.lock() else {
            return Ok(());
        };
        // The upper end is read first: the rows between the cursor and it
        // are mostly device lines, and without it a quiet audit trail would
        // have every look walk further over them than the last.
        let newest = newest_row(app)?;
        if newest <= after {
            return Ok(());
        }
        let rows: Vec<LogEntry> = app.store().with(|conn| {
            let mut statement = conn.prepare_cached(
                "SELECT id, ts, stream, device_id, severity, source, message, body FROM logs
                 WHERE id > ?1 AND id <= ?2 AND stream IN ('event', 'audit')
                 ORDER BY id LIMIT ?3",
            )?;
            let rows = statement.query_map(
                rusqlite::params![after, newest, RECORD_PAGE as i64],
                |row| {
                    let body: Option<String> = row.get(7)?;
                    Ok(LogEntry {
                        id: row.get(0)?,
                        ts: row.get(1)?,
                        stream: row.get(2)?,
                        device_id: row.get(3)?,
                        severity: row.get(4)?,
                        source: row.get(5)?,
                        message: row.get(6)?,
                        body: body.and_then(|body| serde_json::from_str(&body).ok()),
                    })
                },
            )?;
            rows.collect()
        })?;
        let full_page = rows.len() == RECORD_PAGE;
        let reached = match rows.last() {
            Some(last) if full_page => last.id,
            _ => newest,
        };
        if !rows.is_empty() {
            let now = now_ms();
            let mut index = crate::attribute::refresh(app, now, crate::ingest::INDEX_MAX_AGE_MS)?;
            let unknown = rows.iter().any(|row| {
                row.device_id
                    .as_deref()
                    .is_some_and(|id| index.device(id).is_none())
            });
            if unknown {
                // A device added a moment ago — a lab's, perhaps, which is
                // exactly what must be known before its events are sent on.
                index = crate::attribute::refresh(app, now, 0)?;
            }
            offer(app, &index, rows.iter());
        }
        let mut cursor = state.forward.cursor.lock();
        // Unless `apply` moved or dropped it in the meantime.
        if *cursor == Some(after) {
            *cursor = Some(reached);
        }
        if !full_page {
            return Ok(());
        }
    }
}

/// `logs.status`: every forwarder, by name.
pub fn status(app: &App) -> AppResult<Vec<ForwarderStatus>> {
    let state = app.state::<LogsState>();
    let mut forwarders: Vec<Forwarder> = app.store().list(COLLECTION)?;
    forwarders.sort_by_key(|forwarder| forwarder.name.to_lowercase());
    let counters = state.forward.counters.lock();
    Ok(forwarders
        .into_iter()
        .map(|forwarder| {
            let counted = counters.get(&forwarder.id);
            let load = |value: fn(&Counters) -> &AtomicU64| {
                counted
                    .map(|counters| value(counters).load(Ordering::Relaxed))
                    .unwrap_or_default()
            };
            let outcome = counted
                .map(|counters| counters.outcome.load(Ordering::Relaxed))
                .unwrap_or(IDLE);
            let state = match (forwarder.enabled, outcome) {
                (false, _) => "disabled",
                (true, OK) => "ok",
                (true, FAILING) => "failing",
                (true, _) => "idle",
            };
            ForwarderStatus {
                sent: load(|counters| &counters.sent),
                dropped: load(|counters| &counters.dropped),
                last_error: counted
                    .filter(|_| forwarder.enabled)
                    .and_then(|counters| counters.last_error.lock().clone()),
                last_at: counted
                    .map(|counters| counters.last_at.load(Ordering::Relaxed))
                    .filter(|at| *at != 0),
                id: forwarder.id,
                name: forwarder.name,
                state: state.to_string(),
            }
        })
        .collect())
}

// -------------------------------------------------------------------- RPC

/// `logs.forward.save`. The document is replaced by what is sent, and what
/// is left out takes its default — the careful one each time: no audit
/// trail, nothing from a lab. Two things are kept when left out instead:
/// `enabled`, so that renaming a forwarder cannot switch it on, and the
/// secret, which is write-only and could not be sent back.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct SaveParams {
    pub id: Option<String>,
    pub name: String,
    pub enabled: Option<bool>,
    pub kind: Option<ForwardKind>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub format: Option<ForwardFormat>,
    pub streams: Option<Vec<String>>,
    pub min_severity: Option<u8>,
    pub lab: bool,
    pub ca_pem: Option<String>,
    /// `https_json`: where to post. Absent leaves what is stored; an empty
    /// string clears it. Taken as any JSON so that a value of the wrong
    /// type is refused here, in our words, and not by a parser that quotes
    /// what it was given.
    pub url: Option<Value>,
    /// `https_json`: the collector's token. Same rules.
    pub token: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct IdParams {
    id: String,
}

/// A secret parameter as text, without ever repeating it.
fn secret_text(value: &Option<Value>, what: &str) -> AppResult<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.trim().to_string())),
        Some(_) => Err(AppError::invalid(format!("the {what} has to be text"))),
    }
}

/// The rule a notification channel's webhook URL has to pass
/// (`nocturne-monitor`, `notify.rs`): `https://`, or `http://` for a
/// collector on one's own network. Written again here because a feature
/// does not link to another; if the rule ever grows — an address policy for
/// server mode, say — it belongs in the core, for both.
///
/// It goes one step further in what it reads, not in what it allows: the
/// text has to be a URL with a host, so that a typing mistake is found when
/// the forwarder is saved rather than as a failing forwarder afterwards.
/// No message here repeats any part of the URL.
fn check_url(url: &str) -> AppResult<()> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(AppError::invalid(
            "the collector's URL has to start with https:// (or http:// on your own network)",
        ));
    }
    let parsed = reqwest::Url::parse(url)
        .ok()
        .filter(|parsed| parsed.host_str().is_some_and(|host| !host.is_empty()));
    if parsed.is_none() || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(AppError::invalid(
            "the collector's URL is not a complete URL (https://host[:port]/path)",
        ));
    }
    Ok(())
}

fn check_token(token: &str) -> AppResult<()> {
    let sendable = token.len() <= 4_096
        && deliver::authorization(token)
            .is_none_or(|value| reqwest::header::HeaderValue::from_str(&value).is_ok());
    if !sendable {
        return Err(AppError::invalid(
            "the token cannot be sent in an HTTP header: it is too long or holds a character a header may not",
        ));
    }
    Ok(())
}

fn streams_of(given: Option<Vec<String>>) -> AppResult<Vec<String>> {
    let Some(given) = given else {
        return Ok(DEFAULT_STREAMS.map(str::to_string).to_vec());
    };
    let given: Vec<String> = given
        .iter()
        .map(|stream| stream.trim().to_ascii_lowercase())
        .collect();
    if let Some(unknown) = given.iter().find(|stream| bit(stream) == 0) {
        let unknown: String = unknown.chars().take(40).collect();
        return Err(AppError::invalid(format!(
            "there is no stream called `{unknown}`: a forwarder can send syslog, trap, event and audit"
        )));
    }
    let streams: Vec<String> = STREAMS
        .iter()
        .filter(|stream| given.iter().any(|given| given == *stream))
        .map(|stream| stream.to_string())
        .collect();
    if streams.is_empty() {
        return Err(AppError::invalid(
            "choose at least one stream for the forwarder to send",
        ));
    }
    Ok(streams)
}

/// Whether a PEM text has a block that is not a certificate.
fn holds_more_than_certificates(pem: &str) -> bool {
    pem.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("-----BEGIN "))
        .any(|line| line != "-----BEGIN CERTIFICATE-----")
}

/// Whether a syslog forwarder points at the syslog receiver of this very
/// application. Its lines would come back as new lines, be stored and be
/// sent again, as fast as the loopback carries them.
///
/// This catches the direct case — an address of this machine and a port
/// the receiver is listening on right now. A loop through another relay, or
/// a receiver switched on afterwards, it cannot see.
fn sends_to_itself(app: &App, forwarder: &Forwarder) -> bool {
    let transport = match forwarder.kind {
        ForwardKind::SyslogUdp => "udp",
        ForwardKind::SyslogTcp => "tcp",
        // The receiver speaks neither TLS nor HTTP.
        ForwardKind::SyslogTls | ForwardKind::HttpsJson => return false,
    };
    let port = format!(":{}", forwarder.port);
    let listening = app
        .state::<LogsState>()
        .syslog
        .receiver
        .lock()
        .listening
        .iter()
        .any(|bound| bound.starts_with(transport) && bound.ends_with(&port));
    if !listening {
        return false;
    }
    let host = forwarder
        .host
        .trim()
        .trim_matches(['[', ']'])
        .to_ascii_lowercase();
    if host == "localhost" {
        return true;
    }
    let Ok(address) = host.parse::<std::net::IpAddr>() else {
        return false;
    };
    address.is_loopback()
        || address.is_unspecified()
        || nocturne_netprobe::local::interfaces()
            .iter()
            .flat_map(|interface| &interface.addresses)
            .filter_map(|own| own.split('/').next()?.parse::<std::net::IpAddr>().ok())
            .any(|own| own == address)
}

pub fn save(app: &App, params: SaveParams) -> AppResult<Forwarder> {
    let state = app.state::<LogsState>();
    let existing: Option<Forwarder> = match params.id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => Some(app.store().require(COLLECTION, id, "log forwarder")?),
        None => None,
    };
    if existing.is_none() && app.store().list::<Forwarder>(COLLECTION)?.len() >= MAX_FORWARDERS {
        return Err(AppError::invalid(format!(
            "there are already {MAX_FORWARDERS} forwarders, which is as many as there can be"
        )));
    }
    let name = params.name.trim().to_string();
    if name.is_empty() || name.chars().count() > 100 {
        return Err(AppError::invalid(
            "give the forwarder a name (at most 100 characters)",
        ));
    }
    let kind = params
        .kind
        .ok_or_else(|| AppError::invalid("say what kind of forwarder this is (`kind`: syslog_udp, syslog_tcp, syslog_tls or https_json)"))?;
    let streams = streams_of(params.streams)?;
    if params.min_severity.is_some_and(|severity| severity > 7) {
        return Err(AppError::invalid(
            "a severity is a number from 0 (emergency) to 7 (debug)",
        ));
    }
    let ca_pem = params
        .ca_pem
        .map(|pem| pem.trim().to_string())
        .filter(|pem| !pem.is_empty());
    if let Some(pem) = &ca_pem {
        if !kind.is_encrypted() {
            return Err(AppError::invalid(
                "only a TLS or an HTTPS forwarder checks a certificate: leave `ca_pem` out for this kind",
            ));
        }
        authorities_from_pem(pem).map_err(|err| AppError::invalid(err.to_string()))?;
        // A certificate and its private key are often kept in one file,
        // and a parser that wants certificates reads past the key without
        // a word. This text is kept in the forwarder's document and shown
        // again by `logs.forward.list`; a key must not be.
        if holds_more_than_certificates(pem) {
            return Err(AppError::invalid(
                "`ca_pem` holds something that is not a certificate (a private key, perhaps). Give only the authority's certificate: this text is stored with the forwarder and shown again",
            ));
        }
    }
    let url = secret_text(&params.url, "URL")?;
    let token = secret_text(&params.token, "token")?;
    let id = existing
        .as_ref()
        .map(|forwarder| forwarder.id.clone())
        .unwrap_or_else(new_id);

    let mut forwarder = Forwarder {
        id: id.clone(),
        name,
        enabled: params
            .enabled
            .or(existing.as_ref().map(|forwarder| forwarder.enabled))
            .unwrap_or(true),
        kind,
        host: String::new(),
        port: 0,
        format: ForwardFormat::Json,
        streams,
        min_severity: params.min_severity,
        lab: params.lab,
        ca_pem,
        has_secret: false,
        updated_at: now_ms(),
    };

    // Everything is checked before anything is written.
    let mut secret: Option<Secret> = None;
    let mut secret_changed = false;
    if kind.is_syslog() {
        if url.as_deref().is_some_and(|url| !url.is_empty())
            || token.as_deref().is_some_and(|token| !token.is_empty())
        {
            return Err(AppError::invalid(
                "a syslog forwarder has a host and a port, not a URL and a token",
            ));
        }
        forwarder.host = params.host.unwrap_or_default().trim().to_string();
        forwarder.port = match params.port {
            Some(port) if port != 0 => port,
            // RFC 5425 gives syslog over TLS its own port.
            _ if kind == ForwardKind::SyslogTls => 6514,
            _ => 514,
        };
        forwarder.format = params.format.unwrap_or(ForwardFormat::Rfc5424);
        // In our words, not the engine's: its message quotes the host, and
        // a URL pasted into the wrong field would be quoted with it — into
        // an error, and from there into the log of refused calls.
        let usable = deliver::syslog_target(&forwarder)
            .ok()
            .is_some_and(|target| SyslogSender::new(target).is_ok());
        if !usable {
            return Err(AppError::invalid(
                "a syslog forwarder needs the collector's host name or IP address (`host`), without a scheme, a path or spaces",
            ));
        }
        if sends_to_itself(app, &forwarder) {
            return Err(AppError::invalid(
                "that is this application's own syslog receiver: it would be sent its own lines, store them and send them again, for ever",
            ));
        }
    } else {
        if params.format == Some(ForwardFormat::Rfc5424) {
            return Err(AppError::invalid(
                "an HTTPS forwarder posts JSON; RFC 5424 is for the syslog kinds",
            ));
        }
        if params
            .host
            .as_deref()
            .is_some_and(|host| !host.trim().is_empty())
        {
            return Err(AppError::invalid(
                "an HTTPS forwarder is given a URL (`url`), not a host and a port",
            ));
        }
        let stored = read_secret(app, &id)?;
        secret = stored.clone();
        if let Some(url) = url {
            if url.is_empty() {
                secret = None;
            } else {
                check_url(&url)?;
                // A token was given for one destination. Pointed somewhere
                // else, the forwarder would hand it to whoever is there —
                // which would make a write-only secret readable by anyone
                // who may save a forwarder. A new URL starts without one.
                let token = stored
                    .as_ref()
                    .filter(|stored| stored.url == url)
                    .map(|stored| stored.token.clone())
                    .unwrap_or_default();
                secret = Some(Secret { url, token });
            }
        }
        if let Some(token) = token {
            check_token(&token)?;
            match &mut secret {
                Some(secret) => secret.token = token,
                None if token.is_empty() => {}
                None => {
                    return Err(AppError::invalid(
                        "a token needs the URL it is sent to: give `url` as well",
                    ));
                }
            }
        }
        if secret.is_none() {
            return Err(AppError::invalid(
                "an HTTPS forwarder needs the URL it posts to (`url`)",
            ));
        }
        secret_changed = secret != stored;
    }

    match &secret {
        Some(secret) if secret_changed => {
            let text = serde_json::to_string(secret)
                .map_err(|_| AppError::internal("the destination could not be stored"))?;
            app.vault().put(&secret_id(&id), &text)?;
        }
        Some(_) => {}
        // A forwarder that became a syslog one keeps no URL behind.
        None => app.vault().delete(&secret_id(&id))?,
    }
    if secret_changed {
        *state.forward.secrets.lock().entry(id.clone()).or_default() += 1;
    }
    forwarder.has_secret = secret.is_some();
    app.store().put(COLLECTION, &id, &forwarder)?;
    apply(app)?;

    let sends_audit = forwarder
        .streams
        .iter()
        .any(|stream| stream == crate::AUDIT_STREAM);
    let mut detail = json!({
        "id": forwarder.id,
        "kind": forwarder.kind,
        "enabled": forwarder.enabled,
        "format": forwarder.format,
        "streams": forwarder.streams,
        "audit": sends_audit,
        "lab": forwarder.lab,
    });
    if kind.is_syslog() {
        detail["host"] = json!(forwarder.host);
        detail["port"] = json!(forwarder.port);
    } else {
        // That it changed, never what to.
        detail["secret_changed"] = json!(secret_changed);
    }
    app.audit(
        "logs.forward.save",
        None,
        format!(
            "Saved the log forwarder `{}`{}",
            forwarder.name,
            if sends_audit {
                ", which sends the audit trail off this machine"
            } else {
                ""
            }
        ),
        detail,
    );
    crate::status::publish(app);
    Ok(forwarder)
}

pub fn delete(app: &App, id: &str) -> AppResult<()> {
    let state = app.state::<LogsState>();
    let forwarder: Forwarder = app.store().require(COLLECTION, id, "log forwarder")?;
    app.store().delete(COLLECTION, id)?;
    app.vault().delete(&secret_id(id))?;
    apply(app)?;
    state.forward.counters.lock().remove(id);
    state.forward.secrets.lock().remove(id);
    app.audit(
        "logs.forward.delete",
        None,
        format!("Deleted the log forwarder `{}`", forwarder.name),
        json!({"id": forwarder.id, "kind": forwarder.kind}),
    );
    crate::status::publish(app);
    Ok(())
}

pub fn list(app: &App) -> AppResult<Vec<Forwarder>> {
    let mut forwarders: Vec<Forwarder> = app.store().list(COLLECTION)?;
    forwarders.sort_by_key(|forwarder| forwarder.name.to_lowercase());
    Ok(forwarders)
}

pub fn register(router: &mut Router) {
    router.add("logs.forward.list", |app: App, _: NoParams| async move {
        list(&app)
    });
    router.add(
        "logs.forward.save",
        |app: App, params: SaveParams| async move { save(&app, params) },
    );
    router.add(
        "logs.forward.delete",
        |app: App, params: IdParams| async move {
            delete(&app, &params.id)?;
            Ok(json!({"deleted": params.id}))
        },
    );
    router.add(
        "logs.forward.test",
        |app: App, params: IdParams| async move {
            let forwarder: Forwarder =
                app.store()
                    .require(COLLECTION, &params.id, "log forwarder")?;
            let outcome = deliver::test(&app, &forwarder).await;
            app.audit(
                "logs.forward.test",
                None,
                format!(
                    "Sent a test event to the log forwarder `{}`",
                    forwarder.name
                ),
                json!({"id": forwarder.id, "ok": outcome.is_ok()}),
            );
            match outcome {
                Ok(()) => Ok(json!({"sent": true})),
                Err(reason) => Err(AppError::unavailable(reason)),
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_that_is_refused_is_never_repeated_in_the_refusal() {
        for url in [
            "ftp://collector.example.net/secret-path",
            "collector.example.net/secret-path",
            "https://",
            "https://collector.example.net/secret path",
        ] {
            let refused = check_url(url).unwrap_err();
            assert_eq!(refused.code, "invalid");
            assert!(
                !refused.message.contains("secret") && !refused.message.contains("collector.ex"),
                "{refused}"
            );
        }
        check_url("https://collector.example.net:8088/services/collector/event").unwrap();
        check_url("http://10.0.0.5:8088/services/collector").unwrap();
    }

    #[test]
    fn a_token_that_cannot_be_a_header_is_refused_without_being_repeated() {
        check_token("").unwrap();
        check_token("11111111-2222-3333-4444-555555555555").unwrap();
        check_token("Bearer abc.def").unwrap();
        let refused = check_token("line\nbreak-secret").unwrap_err();
        assert!(!refused.message.contains("break-secret"), "{refused}");
        assert!(check_token(&"x".repeat(5_000)).is_err());
    }

    #[test]
    fn a_pem_with_anything_but_certificates_in_it_is_told_apart() {
        let certificate = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----";
        assert!(!holds_more_than_certificates(certificate));
        assert!(!holds_more_than_certificates(&format!(
            "subject=CN = an authority\n{certificate}\n  {certificate}\n"
        )));
        for label in [
            "PRIVATE KEY",
            "RSA PRIVATE KEY",
            "EC PRIVATE KEY",
            "ENCRYPTED PRIVATE KEY",
        ] {
            assert!(
                holds_more_than_certificates(&format!(
                    "{certificate}\n-----BEGIN {label}-----\nMIIE\n-----END {label}-----\n"
                )),
                "{label}"
            );
        }
    }

    #[test]
    fn the_audit_trail_is_not_among_the_streams_unless_it_is_named() {
        assert_eq!(streams_of(None).unwrap(), vec!["syslog", "trap", "event"]);
        assert_eq!(
            streams_of(Some(vec![
                "Audit".into(),
                " syslog ".into(),
                "audit".into()
            ]))
            .unwrap(),
            vec!["syslog", "audit"],
            "in the listed order, once each"
        );
        assert!(streams_of(Some(vec![])).is_err());
        let refused = streams_of(Some(vec!["netflow".into()])).unwrap_err();
        assert!(refused.message.contains("netflow"), "{refused}");
    }

    #[test]
    fn a_secret_of_the_wrong_type_is_refused_without_being_repeated() {
        let refused = secret_text(&Some(json!(123_456_789)), "token").unwrap_err();
        assert!(!refused.message.contains("123"), "{refused}");
        assert_eq!(secret_text(&None, "token").unwrap(), None);
        assert_eq!(secret_text(&Some(Value::Null), "token").unwrap(), None);
        assert_eq!(
            secret_text(&Some(json!(" abc ")), "token")
                .unwrap()
                .as_deref(),
            Some("abc")
        );
    }
}
