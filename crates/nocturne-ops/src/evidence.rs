//! Evidence: what Nocturne itself read, when, and from where.
//!
//! "This is what SW1 printed at 03:05" — kept, with its source, and
//! attachable to the incident it explains. Contract:
//! `docs/dev/spec-capture.md`.
//!
//! Four things hold for every item, and the module is shaped by them:
//!
//! * **It is redacted before it is stored**, by [`crate::redact`]. There is
//!   no parameter, on any method, that stores text as it came; the
//!   placeholder table is thrown away, so what was replaced cannot be had
//!   back from the item.
//! * **Its source is recorded by this module, not typed by a person.** A
//!   capture method asks the other feature itself — the terminal for the
//!   last lines of a session, the topology for its drawing, the monitor for
//!   its samples — and stores what came back, hashed as stored. The one
//!   method that takes text from the client ([`add_text`]) says so in the
//!   item: its source is `operator`.
//! * **Past a limit a call is refused**, with what to do about it. Nothing
//!   is cut to fit.
//! * **The text is not in the incident.** An incident is rewritten whole on
//!   every save and its search text is its timeline, so an attached item is
//!   a timeline entry holding a reference. What an item *is* lives in the
//!   record store like every other record of this feature; its body lives
//!   in a table of its own, which no list ever reads.
//!
//! Every other feature a capture reads from may be absent from the build.
//! The answer is then `unavailable` with the reason, and nothing is stored.

use std::collections::BTreeSet;
use std::time::Duration;

use nocturne_app::store::Store;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, AppError, AppResult, inventory};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::incidents;
use crate::model::{Entry, EntryKind, Incident, Provenance};
use crate::records::{self, Kind, Query, Record, Tx};
use crate::redact::EvidenceRedactor;
use crate::refs::{Devices, Period, Ref};
use crate::render::{Cell, Content, Exhibit, Table};
use crate::sources::{self, Reason};
use crate::text;
use crate::timefmt::Zone;

const WHAT: &str = "evidence item";

/// The stated limits. Past one, a call is refused.
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_POINTS: usize = 20_000;
pub const MAX_ITEMS: u64 = 5_000;
const MAX_TITLE_CHARS: usize = 200;
const MAX_NOTE_CHARS: usize = 1_000;
/// The most that is recorded of what a capture was asked for. It is part of
/// what an item *is*, so it is copied into every list, event and handover
/// that shows the item: two hundred series fit, a megabyte does not.
pub const MAX_PARAMS_BYTES: usize = 64 * 1024;
/// The longest thing that is taken for the id of a session. Ids made here
/// are a quarter of it.
const MAX_ID_CHARS: usize = 64;
/// The most lines the terminal feature gives of a session. Asking it for
/// more gets this many without a word, which here would be cutting silently.
const MAX_SNAPSHOT_LINES: usize = 10_000;
const DEFAULT_SNAPSHOT_LINES: usize = 200;
/// How much evidence text one exported document carries. Items past it are
/// listed with everything known about them and a statement that their
/// content was left out: a review that cannot be opened helps nobody.
const APPENDIX_BYTES: usize = 4 * 1024 * 1024;

/// How long a capture waits for the feature it reads from. A drawing of a
/// large estate takes longer than the few seconds an overview allows.
const BUDGET: Duration = Duration::from_secs(20);

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS ops_evidence_bodies (
        id   TEXT NOT NULL PRIMARY KEY,
        body TEXT NOT NULL
    );
";

/// Create the table of bodies. Safe to call at every start.
pub fn ensure_schema(store: &Store) -> AppResult<()> {
    store.with(|conn| conn.execute_batch(SCHEMA))
}

// -------------------------------------------------------------------- model

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// The last lines of a terminal session.
    Terminal,
    /// A piece of a session log.
    Log,
    /// Text a person handed in.
    Text,
    /// The topology's own drawing.
    Topology,
    /// Samples of one or more series over a period.
    Chart,
}

impl EvidenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EvidenceKind::Terminal => "terminal",
            EvidenceKind::Log => "log",
            EvidenceKind::Text => "text",
            EvidenceKind::Topology => "topology",
            EvidenceKind::Chart => "chart",
        }
    }
}

/// Where an item came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Source {
    /// The method the content was read with, or `operator` for text a
    /// person handed in.
    pub from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// The device's name when the item was captured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
    /// It comes from, or shows, a simulated device.
    #[serde(default)]
    pub lab: bool,
    /// What was asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// An evidence item without its body: what a list shows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub kind: EvidenceKind,
    pub title: String,
    pub captured_at: i64,
    pub source: Source,
    /// Always `true`. It is a field so that every place an item is shown
    /// can say so without knowing why.
    pub redacted: bool,
    /// Lines of text; absent for a drawing and for a chart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<usize>,
    /// The size of the body as stored.
    pub bytes: usize,
    /// Of the body as stored, in hexadecimal.
    pub sha256: String,
    pub incident_ids: Vec<String>,
}

/// How much of a period a series has samples for, as it was read.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeriesCoverage {
    pub device_id: String,
    /// The device's name when the item was captured.
    pub device: String,
    #[serde(default)]
    pub lab: bool,
    pub metric: String,
    #[serde(default)]
    pub instance: String,
    pub points: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_at: Option<i64>,
    /// The longest stretch between two neighbouring points. A hole in the
    /// samples is this, not a missing number: the points are not filled in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub longest_gap_ms: Option<i64>,
    /// As the source said: the width of a bucket and where the points came
    /// from (`raw`, `5m`, `1h`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_s: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollup: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub since: i64,
    pub until: i64,
    /// Points over all the series.
    pub points: usize,
    /// One entry per series, in the order they were asked for.
    pub series: Vec<SeriesCoverage>,
}

/// The content of an item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Body {
    Text {
        text: String,
    },
    Svg {
        svg: String,
    },
    Chart {
        /// The series exactly as `metrics.query_many` returned them.
        series: Vec<Value>,
        coverage: Coverage,
    },
}

impl Body {
    /// What goes into the table of bodies, and what the hash is of.
    fn stored(&self) -> AppResult<String> {
        Ok(match self {
            Body::Text { text } => text.clone(),
            Body::Svg { svg } => svg.clone(),
            Body::Chart { .. } => serde_json::to_string(self)?,
        })
    }

    fn read(kind: EvidenceKind, stored: String) -> AppResult<Body> {
        Ok(match kind {
            EvidenceKind::Terminal | EvidenceKind::Log | EvidenceKind::Text => {
                Body::Text { text: stored }
            }
            EvidenceKind::Topology => Body::Svg { svg: stored },
            EvidenceKind::Chart => serde_json::from_str(&stored)?,
        })
    }
}

impl Evidence {
    /// What the item is, in the words of a timeline's evidence column.
    fn what(&self) -> String {
        let name = |value: &Option<String>| value.clone().unwrap_or_default();
        let what = match self.kind {
            EvidenceKind::Terminal => {
                format!("terminal session `{}`", name(&self.source.session_title))
            }
            EvidenceKind::Log => format!(
                "session log `{}`",
                self.source
                    .params
                    .as_ref()
                    .and_then(|params| params.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            ),
            EvidenceKind::Text => "text supplied by the operator".to_string(),
            EvidenceKind::Topology => "the topology's drawing".to_string(),
            EvidenceKind::Chart => "stored samples".to_string(),
        };
        format!(
            "{what}{}, {}, redacted, sha256 {}",
            if self.source.lab { " (lab)" } else { "" },
            self.extent(),
            self.sha256
        )
    }

    /// `200 lines`, or the size of something that has no lines.
    fn extent(&self) -> String {
        match self.lines {
            Some(1) => "1 line".to_string(),
            Some(lines) => format!("{lines} lines"),
            None => text::size(self.bytes),
        }
    }

    /// The devices the item is about: its own, and every one a chart shows.
    fn device_ids(&self) -> BTreeSet<String> {
        let mut ids: BTreeSet<String> = self.source.device_id.iter().cloned().collect();
        if let Some(series) = self
            .source
            .params
            .as_ref()
            .and_then(|params| params.get("series"))
            .and_then(Value::as_array)
        {
            ids.extend(
                series
                    .iter()
                    .filter_map(|one| one.get("device_id").and_then(Value::as_str))
                    .map(str::to_string),
            );
        }
        ids
    }

    fn search_text(&self) -> String {
        let source = &self.source;
        [
            Some(self.title.as_str()),
            Some(self.kind.as_str()),
            Some(source.from.as_str()),
            source.session_title.as_deref(),
            source.device.as_deref(),
            source.peer.as_deref(),
            source.note.as_deref(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
    }

    fn record(&self) -> AppResult<Record> {
        let mut links: Vec<String> = self
            .device_ids()
            .into_iter()
            .map(|id| format!("device:{id}"))
            .collect();
        links.extend(self.incident_ids.iter().map(|id| format!("incident:{id}")));
        let stored = serde_json::to_string(self)?;
        Ok(Record {
            id: self.id.clone(),
            sort: self.captured_at,
            facet: self.kind.as_str().to_string(),
            head: stored.clone(),
            search: self.search_text(),
            body: stored,
            links,
        })
    }

    /// The timeline entry an incident gets for the item: at the time it was
    /// captured, holding a reference and the title, never the text.
    fn entry(&self, author: &str, now: i64) -> Entry {
        Entry {
            id: entry_id(&self.id),
            ts: self.captured_at,
            kind: EntryKind::Evidence,
            text: self.title.clone(),
            reference: Some(Ref::new("evidence", self.id.clone())),
            author: author.to_string(),
            source: Provenance::read(
                &self.source.from,
                format!("evidence `{}`: {}", self.id, self.what()),
                now,
            ),
        }
    }

    /// What is left on a timeline when the item is deleted: the same entry,
    /// at the same time, saying that there was evidence here and when it
    /// went. The hash stays, so a copy somebody exported can still be
    /// matched against what the incident once pointed at.
    fn tombstone(&self, entry: &Entry, author: &str, now: i64) -> Entry {
        Entry {
            id: entry.id.clone(),
            ts: entry.ts,
            kind: EntryKind::Evidence,
            text: format!("{} — evidence deleted on {}", self.title, utc_date(now)),
            reference: None,
            author: author.to_string(),
            source: Provenance::read(
                "evidence.delete",
                format!("evidence `{}` was deleted; it was {}", self.id, self.what()),
                now,
            ),
        }
    }
}

fn entry_id(evidence_id: &str) -> String {
    format!("evidence:{evidence_id}")
}

/// `2026-11-22 (UTC)`. A stored sentence cannot follow its reader's clock,
/// so it names the one it used.
fn utc_date(at: i64) -> String {
    match chrono::DateTime::<chrono::Utc>::from_timestamp_millis(at) {
        Some(time) => format!("{} (UTC)", time.format("%Y-%m-%d")),
        None => format!("{at} ms"),
    }
}

fn sha256(stored: &str) -> String {
    hex::encode(Sha256::digest(stored.as_bytes()))
}

// ------------------------------------------------------------------ storage

fn parse(body: &str) -> AppResult<Evidence> {
    serde_json::from_str(body).map_err(AppError::from)
}

fn missing(id: &str) -> AppError {
    AppError::not_found(format!("{WHAT} `{id}`"))
}

pub fn load(app: &App, id: &str) -> AppResult<Evidence> {
    let body = records::body(app.store(), Kind::Evidence, id)?.ok_or_else(|| missing(id))?;
    parse(&body)
}

fn load_in(tx: &Tx<'_>, id: &str) -> AppResult<Evidence> {
    let body = tx.body(Kind::Evidence, id)?.ok_or_else(|| missing(id))?;
    parse(&body)
}

pub fn load_body(app: &App, evidence: &Evidence) -> AppResult<Body> {
    let stored: Option<String> = app.store().with(|conn| {
        conn.query_row(
            "SELECT body FROM ops_evidence_bodies WHERE id = ?1",
            params![evidence.id],
            |row| row.get(0),
        )
        .optional()
    })?;
    let stored = stored.ok_or_else(|| {
        AppError::internal(format!(
            "{WHAT} `{}` is recorded but its content is not stored",
            evidence.id
        ))
    })?;
    Body::read(evidence.kind, stored)
}

/// An item with its body, as `get` and the capture methods return it.
fn with_body(evidence: &Evidence, body: &Body) -> AppResult<Value> {
    let mut value = serde_json::to_value(evidence)?;
    value["body"] = serde_json::to_value(body)?;
    Ok(value)
}

fn refuse(limit: &str, max: usize, actual: usize, message: String) -> AppError {
    AppError::invalid(format!("{message} Nothing was stored."))
        .with_details(json!({"limit": limit, "max": max, "actual": actual}))
}

/// Refuse text that is longer than an item holds. `instead` is what the
/// caller can do about it.
fn text_fits(bytes: usize, instead: &str) -> AppResult<()> {
    if bytes <= MAX_TEXT_BYTES {
        return Ok(());
    }
    Err(refuse(
        "text_bytes",
        MAX_TEXT_BYTES,
        bytes,
        format!(
            "This is {} of text and an evidence item holds at most {}: {instead}.",
            text::size(bytes),
            text::size(MAX_TEXT_BYTES)
        ),
    ))
}

/// Refuse a request too large to be recorded with the item it makes.
fn params_fit(asked: &Value, instead: &str) -> AppResult<()> {
    let bytes = asked.to_string().len();
    if bytes <= MAX_PARAMS_BYTES {
        return Ok(());
    }
    Err(refuse(
        "params_bytes",
        MAX_PARAMS_BYTES,
        bytes,
        format!(
            "What was asked for is {} and at most {} of it is recorded with an evidence item: {instead}.",
            text::size(bytes),
            text::size(MAX_PARAMS_BYTES)
        ),
    ))
}

/// Whether `id` can be an id at all. What a client names as a session is
/// recorded in the item, its event and its audit entry whether or not the
/// session can still be found, so it has to be shaped like one: short, and
/// nothing a sentence or a pasted line could be.
fn id_shaped(id: &str) -> bool {
    id.chars().count() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
}

/// What a new item is made of.
struct Draft {
    kind: EvidenceKind,
    title: String,
    source: Source,
    body: Body,
    /// The audit entry: its action and its sentence.
    action: &'static str,
    message: String,
}

/// Store a new item, announce it and record that it was made.
fn keep(app: &App, draft: Draft) -> AppResult<Value> {
    let stored = draft.body.stored()?;
    let (lines, points) = match &draft.body {
        Body::Text { text } => (Some(text.lines().count()), None),
        Body::Svg { .. } => (None, None),
        Body::Chart { coverage, .. } => (None, Some(coverage.points)),
    };
    let evidence = Evidence {
        id: new_id(),
        kind: draft.kind,
        title: draft.title,
        captured_at: now_ms(),
        source: draft.source,
        redacted: true,
        lines,
        bytes: stored.len(),
        sha256: sha256(&stored),
        incident_ids: Vec::new(),
    };
    let record = evidence.record()?;
    records::transaction(app.store(), |tx| {
        let have = tx.count(Kind::Evidence)?;
        if have >= MAX_ITEMS {
            return Err(refuse(
                "items",
                MAX_ITEMS as usize,
                have as usize,
                format!(
                    "There are already {have} evidence items, the most that are kept: delete the ones that are no longer needed (`evidence.delete`)."
                ),
            ));
        }
        tx.put(Kind::Evidence, &record)?;
        tx.conn().execute(
            "INSERT INTO ops_evidence_bodies (id, body) VALUES (?1, ?2)",
            params![evidence.id, stored],
        )?;
        Ok(())
    })?;
    app.publish("evidence.changed", &evidence);
    app.audit(
        draft.action,
        evidence.source.device_id.as_deref(),
        draft.message,
        json!({
            "evidence_id": evidence.id,
            "kind": evidence.kind,
            "from": evidence.source.from,
            "session_id": evidence.source.session_id,
            "lines": lines,
            "points": points,
            "bytes": evidence.bytes,
            "sha256": evidence.sha256,
            "lab": evidence.source.lab,
        }),
    );
    with_body(&evidence, &draft.body)
}

/// The title of a new item: the one given, or `otherwise`. It is redacted
/// like everything else a person or a device supplied — which is why the
/// titles made here join a name to what follows with a dash: `SW1: last`
/// would be `name: value` to a rule that looks for exactly that.
fn title_of(
    given: Option<&str>,
    otherwise: impl FnOnce() -> String,
    redactor: &EvidenceRedactor,
) -> AppResult<String> {
    let title = match given.map(text::one_line).filter(|title| !title.is_empty()) {
        Some(title) => title,
        None => otherwise(),
    };
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(AppError::invalid(format!(
            "a title has at most {MAX_TITLE_CHARS} characters; what it describes goes in the item itself"
        )));
    }
    Ok(redactor.line(&title))
}

// ------------------------------------------------------- the other features

/// Ask another feature, which may not be in this build.
///
/// A feature that is absent or silent makes the capture `unavailable`, with
/// the reason. One that answers with a refusal of its own — no such
/// session, no such log — is passed on as it said it.
async fn ask(app: &App, method: &str, params: Value, what: &str) -> AppResult<Value> {
    sources::call(app, method, params, BUDGET)
        .await
        .map_err(|gap| match (gap.reason, gap.code) {
            (Reason::Failed, Some(code)) => AppError::new(code, gap.message),
            _ => AppError::unavailable(format!("{what}: {}", gap.message)),
        })
}

fn malformed(method: &str, detail: &str, what: &str) -> AppError {
    AppError::unavailable(format!(
        "{what}: `{method}` answered with something this version cannot read ({detail})"
    ))
}

/// A terminal session, as the terminal feature describes it.
struct Session {
    title: String,
    device_id: Option<String>,
    lab: bool,
    peer: Option<String>,
}

async fn session(app: &App, session_id: &str, what: &str) -> AppResult<Session> {
    let answer = ask(app, "terminal.list", json!({}), what).await?;
    let sessions = sources::list("terminal.list", answer, "sessions")
        .map_err(|gap| AppError::unavailable(format!("{what}: {}", gap.message)))?;
    let found = sessions
        .iter()
        .find(|session| session.get("id").and_then(Value::as_str) == Some(session_id))
        .ok_or_else(|| AppError::not_found(format!("terminal session `{session_id}`")))?;
    // Whether the far end is simulated is not something to guess at: a
    // description without it is not one this can label evidence from.
    let lab = found
        .get("lab")
        .and_then(Value::as_bool)
        .ok_or_else(|| malformed("terminal.list", "a session without `lab`", what))?;
    let title = found
        .get("title")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("terminal.list", "a session without `title`", what))?;
    Ok(Session {
        title: title.to_string(),
        device_id: found
            .get("device_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        lab,
        peer: found
            .pointer("/info/peer")
            .and_then(Value::as_str)
            .filter(|peer| !peer.is_empty())
            .map(str::to_string),
    })
}

/// A device's name, when the inventory still has it.
fn device_name(app: &App, device_id: Option<&str>) -> Option<String> {
    inventory::get(app, device_id?)
        .ok()
        .map(|device| device.name)
}

// --------------------------------------------------------- capture_terminal

#[derive(Debug, Deserialize)]
pub struct CaptureTerminalParams {
    pub session_id: String,
    /// Lines from the end of the session; 200 when left out.
    #[serde(default)]
    pub lines: Option<usize>,
    #[serde(default)]
    pub title: Option<String>,
}

pub async fn capture_terminal(app: &App, params: CaptureTerminalParams) -> AppResult<Value> {
    const WHAT: &str = "evidence cannot be captured from a terminal session";
    let lines = params.lines.unwrap_or(DEFAULT_SNAPSHOT_LINES);
    if !(1..=MAX_SNAPSHOT_LINES).contains(&lines) {
        return Err(AppError::invalid(format!(
            "`lines` is between 1 and {MAX_SNAPSHOT_LINES}; for more of a session, capture from its log with `evidence.capture_log`"
        )));
    }
    let session = session(app, &params.session_id, WHAT).await?;
    let answer = ask(
        app,
        "terminal.snapshot",
        json!({"session_id": params.session_id, "lines": lines, "redact": true}),
        WHAT,
    )
    .await?;
    let printed = answer
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("terminal.snapshot", "no `text`", WHAT))?;
    if printed.trim().is_empty() {
        return Err(AppError::invalid(
            "the session has printed nothing yet, so there is nothing to keep",
        ));
    }
    const INSTEAD: &str =
        "capture fewer lines (`lines`), or the part that matters from the session's log";
    // Hiding secrets does not halve a transcript: one twice too long is
    // refused before the work of going through it.
    if printed.len() > 2 * MAX_TEXT_BYTES {
        text_fits(printed.len(), INSTEAD)?;
    }
    let redactor = EvidenceRedactor::open(app);
    let text = redactor.text(printed);
    text_fits(text.len(), INSTEAD)?;
    let kept = text.lines().count();
    let title = title_of(
        params.title.as_deref(),
        || format!("{} — last {kept} lines", session.title),
        &redactor,
    )?;
    let device = device_name(app, session.device_id.as_deref());
    // A session is called what it was opened as, and that goes into the
    // item, its event and the audit trail: it is searched like the rest.
    let session_title = redactor.line(&session.title);
    keep(
        app,
        Draft {
            kind: EvidenceKind::Terminal,
            title,
            message: format!(
                "Kept the last {kept} lines of the terminal session to {session_title} as evidence"
            ),
            source: Source {
                from: "terminal.snapshot".into(),
                session_id: Some(params.session_id),
                session_title: Some(session_title),
                device_id: session.device_id,
                device,
                peer: session.peer.as_deref().map(|peer| redactor.line(peer)),
                lab: session.lab,
                params: Some(json!({"lines": lines})),
                note: None,
            },
            body: Body::Text { text },
            action: "evidence.capture_terminal",
        },
    )
}

// -------------------------------------------------------------- capture_log

#[derive(Debug, Deserialize)]
pub struct CaptureLogParams {
    pub name: String,
    /// Byte offset into the log.
    #[serde(default)]
    pub offset: u64,
    /// Most bytes to read; as much as an item holds when left out.
    #[serde(default)]
    pub limit: Option<u64>,
    #[serde(default)]
    pub title: Option<String>,
}

pub async fn capture_log(app: &App, params: CaptureLogParams) -> AppResult<Value> {
    const WHAT: &str = "evidence cannot be captured from a session log";
    let limit = params.limit.unwrap_or(MAX_TEXT_BYTES as u64);
    if !(1..=MAX_TEXT_BYTES as u64).contains(&limit) {
        return Err(AppError::invalid(format!(
            "`limit` is between 1 and {MAX_TEXT_BYTES} bytes ({}), the most an evidence item holds; keep a longer stretch of a log as several items, continuing at the `offset` where the one before ended",
            text::size(MAX_TEXT_BYTES)
        )));
    }
    // What the terminal's index says about the log: which session wrote it.
    // The file itself is never opened from here — it is unredacted on disk.
    let file = params.name.rsplit('/').next().unwrap_or(&params.name);
    let listed = ask(
        app,
        "terminal.logs.list",
        json!({"query": file, "limit": 50}),
        WHAT,
    )
    .await?;
    let listed = sources::list("terminal.logs.list", listed, "logs")
        .map_err(|gap| AppError::unavailable(format!("{WHAT}: {}", gap.message)))?;
    let described = listed
        .iter()
        .find(|log| log.get("name").and_then(Value::as_str) == Some(params.name.as_str()))
        .ok_or_else(|| AppError::not_found(format!("session log `{}`", params.name)))?;
    let device_id = described
        .get("device_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let session_title = described
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string);
    let device = device_id
        .as_deref()
        .and_then(|id| inventory::get(app, id).ok());
    // A lab session is logged as one over the simulator's transport, which
    // still says so after its device has been deleted.
    let lab = described.get("kind").and_then(Value::as_str) == Some("sim")
        || device.as_ref().is_some_and(|device| device.lab.is_some());

    let answer = ask(
        app,
        "terminal.logs.read",
        json!({"name": params.name, "offset": params.offset, "limit": limit, "redact": true}),
        WHAT,
    )
    .await?;
    let read = answer
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed("terminal.logs.read", "no `text`", WHAT))?;
    if read.trim().is_empty() {
        return Err(AppError::invalid(format!(
            "session log `{}` has nothing at offset {}",
            params.name, params.offset
        )));
    }
    let redactor = EvidenceRedactor::open(app);
    let text = redactor.text(read);
    text_fits(text.len(), "read a smaller piece of the log (`limit`)")?;
    let kept = text.lines().count();
    let title = title_of(
        params.title.as_deref(),
        || {
            format!(
                "{} — {kept} lines of the session log",
                session_title.as_deref().unwrap_or(file)
            )
        },
        &redactor,
    )?;
    // Which bytes of the log this is, as the read itself reported them: a
    // page starts at the beginning of the line the offset falls in, which can
    // be earlier than the offset asked for.
    let note = match (
        answer.get("offset").and_then(Value::as_u64),
        answer.get("next_offset").and_then(Value::as_u64),
        answer.get("size").and_then(Value::as_u64),
    ) {
        (Some(from), Some(next), Some(size)) => Some(format!(
            "bytes {from} to {next} of {size}{}",
            if answer.get("eof").and_then(Value::as_bool) == Some(true) {
                ", the end of the log"
            } else {
                ""
            }
        )),
        _ => None,
    };
    keep(
        app,
        Draft {
            kind: EvidenceKind::Log,
            title,
            message: format!(
                "Kept {kept} lines of session log `{}` as evidence",
                params.name
            ),
            source: Source {
                from: "terminal.logs.read".into(),
                session_id: None,
                session_title: session_title.map(|title| redactor.line(&title)),
                device_id,
                device: device.map(|device| device.name),
                peer: None,
                lab,
                params: Some(json!({"name": params.name, "offset": params.offset, "limit": limit})),
                note,
            },
            body: Body::Text { text },
            action: "evidence.capture_log",
        },
    )
}

// --------------------------------------------------------- capture_topology

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct CaptureTopologyParams {
    pub view: Option<String>,
    pub layer: Option<Value>,
    pub filter: Option<Value>,
    pub include_endpoints: Option<bool>,
    pub title: Option<String>,
}

/// Whether a drawing made with `filter` can show a simulated device.
///
/// The drawing itself marks its lab nodes, but an item is labelled without
/// being opened. A saved view narrows with a filter of its own that only the
/// topology feature knows, so the answer errs towards "it may": a lab label
/// on a drawing of hardware misleads nobody, the other way round does.
fn may_show_lab(devices: &Devices, filter: Option<&Value>) -> bool {
    let named = |key: &str| {
        filter
            .and_then(|filter| filter.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    if named("lab_id").is_some() {
        return true;
    }
    let only: Vec<&str> = filter
        .and_then(|filter| filter.get("device_ids"))
        .and_then(Value::as_array)
        .map(|ids| ids.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    devices
        .rows()
        .iter()
        .map(|row| &row.device)
        .filter(|device| device.lab.is_some())
        .any(|device| {
            (only.is_empty() || only.contains(&device.id.as_str()))
                && named("site").is_none_or(|site| device.site.eq_ignore_ascii_case(site))
                && named("role").is_none_or(|role| device.role.eq_ignore_ascii_case(role))
                && named("tag").is_none_or(|tag| {
                    device
                        .tags
                        .iter()
                        .any(|known| known.eq_ignore_ascii_case(tag))
                })
        })
}

pub async fn capture_topology(app: &App, params: CaptureTopologyParams) -> AppResult<Value> {
    const WHAT: &str = "evidence cannot be captured from the topology";
    // Only what was said is sent and recorded: what was left out is the
    // topology feature's default, not this one's.
    let mut asked = serde_json::Map::new();
    if let Some(view) = params.view.as_ref().filter(|view| !view.is_empty()) {
        asked.insert("view".into(), json!(view));
    }
    if let Some(layer) = params.layer.as_ref().filter(|layer| !layer.is_null()) {
        asked.insert("layer".into(), layer.clone());
    }
    if let Some(filter) = params.filter.as_ref().filter(|filter| !filter.is_null()) {
        asked.insert("filter".into(), filter.clone());
    }
    if let Some(include) = params.include_endpoints {
        asked.insert("include_endpoints".into(), json!(include));
    }
    params_fit(
        &Value::Object(asked.clone()),
        "name a saved `view`, or narrow the `filter` to what the drawing is about",
    )?;
    let mut request = asked.clone();
    request.insert("format".into(), json!("svg"));
    let answer = ask(app, "topology.export", Value::Object(request), WHAT).await?;
    let drawn = answer
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| text.contains("<svg"))
        .ok_or_else(|| malformed("topology.export", "no SVG in `text`", WHAT))?;
    if answer
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format != "svg")
    {
        return Err(malformed("topology.export", "not the SVG format", WHAT));
    }
    let fits = |bytes: usize| {
        if bytes <= MAX_SVG_BYTES {
            return Ok(());
        }
        Err(refuse(
            "svg_bytes",
            MAX_SVG_BYTES,
            bytes,
            format!(
                "The drawing is {} and an evidence item holds at most {} of SVG: narrow it with a `filter`, or leave the endpoints out.",
                text::size(bytes),
                text::size(MAX_SVG_BYTES)
            ),
        ))
    };
    // Hiding secrets does not halve a drawing: one twice too large is
    // refused before the work of going through it.
    if drawn.len() > 2 * MAX_SVG_BYTES {
        fits(drawn.len())?;
    }
    let redactor = EvidenceRedactor::open(app);
    let svg = redactor.svg(drawn);
    fits(svg.len())?;
    let title = title_of(
        params.title.as_deref(),
        || match &params.view {
            Some(view) if !view.is_empty() => format!("Topology drawing — {view}"),
            _ => "Topology drawing".to_string(),
        },
        &redactor,
    )?;
    let lab = may_show_lab(&Devices::load(app)?, params.filter.as_ref());
    keep(
        app,
        Draft {
            kind: EvidenceKind::Topology,
            title,
            message: "Kept the topology's drawing as evidence".into(),
            source: Source {
                from: "topology.export".into(),
                lab,
                params: Some(redactor.json(&Value::Object(asked))),
                ..Source::default()
            },
            body: Body::Svg { svg },
            action: "evidence.capture_topology",
        },
    )
}

// ------------------------------------------------------------ capture_chart

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SeriesRef {
    pub device_id: String,
    pub metric: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CaptureChartParams {
    pub series: Vec<SeriesRef>,
    pub since: i64,
    pub until: i64,
    #[serde(default)]
    pub step_s: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
}

/// The times of a series' points, in the order they came. `None` when a
/// point is not `[time, value]`.
fn times(series: &Value) -> Option<Vec<i64>> {
    series
        .get("points")?
        .as_array()?
        .iter()
        .map(|point| match point.as_array()?.as_slice() {
            [at, _value] => at.as_i64(),
            _ => None,
        })
        .collect()
}

pub async fn capture_chart(app: &App, params: CaptureChartParams) -> AppResult<Value> {
    const WHAT: &str = "evidence cannot be captured from the stored samples";
    if params.series.is_empty() {
        return Err(AppError::invalid(
            "name at least one series to keep: `series: [{device_id, metric, instance?}]`",
        ));
    }
    if params.until <= params.since {
        return Err(AppError::invalid(
            "a chart's period needs `until` after `since`",
        ));
    }
    // Every device has to be one the inventory knows: its name goes into the
    // item, and whether it is simulated decides the item's lab label.
    let devices = Devices::load(app)?;
    for wanted in &params.series {
        if !devices.contains(&wanted.device_id) {
            return Err(AppError::not_found(format!(
                "device `{}`",
                wanted.device_id
            )));
        }
    }
    let mut asked = json!({
        "series": params.series,
        "since": params.since,
        "until": params.until,
    });
    if let Some(step) = params.step_s {
        asked["step_s"] = json!(step);
    }
    params_fit(&asked, "keep fewer series in one item")?;
    let answer = ask(app, "metrics.query_many", asked.clone(), WHAT).await?;
    let answered = sources::list("metrics.query_many", answer, "series")
        .map_err(|gap| AppError::unavailable(format!("{WHAT}: {}", gap.message)))?;
    if answered.len() != params.series.len() {
        return Err(malformed(
            "metrics.query_many",
            &format!(
                "{} series for the {} asked for",
                answered.len(),
                params.series.len()
            ),
            WHAT,
        ));
    }

    let redactor = EvidenceRedactor::open(app);
    let mut coverage = Coverage {
        since: params.since,
        until: params.until,
        points: 0,
        series: Vec::with_capacity(answered.len()),
    };
    for (wanted, series) in params.series.iter().zip(&answered) {
        let times = times(series).ok_or_else(|| {
            malformed(
                "metrics.query_many",
                "a series whose `points` are not `[time, value]` pairs",
                WHAT,
            )
        })?;
        coverage.points += times.len();
        let row = devices.get(&wanted.device_id);
        coverage.series.push(SeriesCoverage {
            device_id: wanted.device_id.clone(),
            device: devices.name(&wanted.device_id),
            lab: row.is_some_and(|row| row.device.lab.is_some()),
            // The names are the caller's, and are stored with the item like
            // the rest of what was asked for: they are searched like it.
            metric: redactor.line(&wanted.metric),
            instance: redactor.line(wanted.instance.as_deref().unwrap_or_default()),
            points: times.len(),
            first_at: times.iter().min().copied(),
            last_at: times.iter().max().copied(),
            // The times are the source's, and nothing says they are sane.
            longest_gap_ms: times
                .windows(2)
                .map(|pair| pair[1].saturating_sub(pair[0]))
                .max(),
            step_s: series.get("step_s").and_then(Value::as_i64),
            rollup: series
                .get("rollup")
                .and_then(Value::as_str)
                .map(|rollup| redactor.line(rollup)),
        });
    }
    if coverage.points > MAX_POINTS {
        return Err(refuse(
            "points",
            MAX_POINTS,
            coverage.points,
            format!(
                "That is {} points and an evidence item holds at most {MAX_POINTS}: ask for a larger `step_s`, a shorter period or fewer series.",
                coverage.points
            ),
        ));
    }

    // Samples that are not there stay not there: the series go in as they
    // came, holes included, and nothing is drawn across them.
    let series: Vec<Value> = answered.iter().map(|one| redactor.json(one)).collect();
    let one_device: BTreeSet<&str> = params
        .series
        .iter()
        .map(|wanted| wanted.device_id.as_str())
        .collect();
    let device_id = (one_device.len() == 1).then(|| params.series[0].device_id.clone());
    let title = title_of(
        params.title.as_deref(),
        || {
            let mut metrics: Vec<&str> = Vec::new();
            for wanted in &params.series {
                if !metrics.contains(&wanted.metric.as_str()) {
                    metrics.push(&wanted.metric);
                }
            }
            let shown = metrics
                .iter()
                .take(3)
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            let more = if metrics.len() > 3 { ", …" } else { "" };
            match &device_id {
                Some(id) => format!("{shown}{more} on {}", devices.name(id)),
                None => format!("{shown}{more} on {} devices", one_device.len()),
            }
        },
        &redactor,
    )?;
    let count = coverage.series.len();
    keep(
        app,
        Draft {
            kind: EvidenceKind::Chart,
            title,
            message: format!(
                "Kept {} points of {count} series as evidence",
                coverage.points
            ),
            source: Source {
                from: "metrics.query_many".into(),
                device: device_id.as_deref().map(|id| devices.name(id)),
                device_id,
                lab: coverage.series.iter().any(|series| series.lab),
                params: Some(redactor.json(&asked)),
                ..Source::default()
            },
            body: Body::Chart { series, coverage },
            action: "evidence.capture_chart",
        },
    )
}

// ----------------------------------------------------------------- add_text

#[derive(Debug, Deserialize)]
pub struct AddTextParams {
    pub text: String,
    pub title: String,
    /// The session the person says the text is from.
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

pub async fn add_text(app: &App, params: AddTextParams) -> AppResult<Value> {
    const INSTEAD: &str = "keep a smaller selection, or keep it as several items";
    if params.text.trim().is_empty() {
        return Err(AppError::invalid("there is no text to keep"));
    }
    if params.title.trim().is_empty() {
        return Err(AppError::invalid(
            "evidence a person hands in needs a title: say what it is",
        ));
    }
    // Before any work is done on it, and again once it is redacted: a
    // placeholder can be longer than what it replaced.
    text_fits(params.text.len(), INSTEAD)?;
    if let Some(note) = &params.note
        && note.chars().count() > MAX_NOTE_CHARS
    {
        return Err(AppError::invalid(format!(
            "a note on an evidence item has at most {MAX_NOTE_CHARS} characters"
        )));
    }

    if let Some(id) = params.session_id.as_deref()
        && !id_shaped(id)
    {
        return Err(AppError::invalid(
            "`session_id` is not the id of a terminal session; what the text is and where it is from go in `title` and `note`",
        ));
    }

    // Which session the text is said to be from. This is the person's word:
    // the session is looked up for its name and its device, and the item
    // still says `operator`.
    let session = match params.session_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => session(app, id, "the session could not be looked up")
            .await
            .ok(),
        None => None,
    };
    let device_id = match (
        params.device_id.as_deref().filter(|id| !id.is_empty()),
        session
            .as_ref()
            .and_then(|session| session.device_id.as_deref()),
    ) {
        (Some(said), Some(of_session)) if said != of_session => {
            return Err(AppError::invalid(
                "`device_id` is not the device of that session; send one of the two",
            ));
        }
        (Some(said), _) => Some(said.to_string()),
        (None, of_session) => of_session.map(str::to_string),
    };
    let device = match &device_id {
        Some(id) => Some(inventory::get(app, id)?),
        None => None,
    };
    let lab = session.as_ref().is_some_and(|session| session.lab)
        || device.as_ref().is_some_and(|device| device.lab.is_some());

    let redactor = EvidenceRedactor::open(app);
    let text = redactor.text(&params.text);
    text_fits(text.len(), INSTEAD)?;
    let title = title_of(Some(params.title.as_str()), String::new, &redactor)?;
    let kept = text.lines().count();
    let mut note = "text as sent by the client, redacted on arrival".to_string();
    let session_title = session
        .as_ref()
        .map(|session| redactor.line(&session.title));
    if let Some(title) = &session_title {
        note.push_str(&format!("; said to be from session `{title}`"));
    }
    if let Some(said) = params
        .note
        .as_deref()
        .map(text::one_line)
        .filter(|said| !said.is_empty())
    {
        note.push_str(&format!("; the operator's note: {}", redactor.line(&said)));
    }
    keep(
        app,
        Draft {
            kind: EvidenceKind::Text,
            title,
            message: format!("Kept {kept} lines of text supplied by the operator as evidence"),
            source: Source {
                from: "operator".into(),
                session_id: params.session_id.filter(|id| !id.is_empty()),
                session_title,
                device_id,
                device: device.map(|device| device.name),
                peer: session
                    .and_then(|session| session.peer)
                    .map(|peer| redactor.line(&peer)),
                lab,
                params: None,
                note: Some(note),
            },
            body: Body::Text { text },
            action: "evidence.add_text",
        },
    )
}

// --------------------------------------------------------------- list / get

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListParams {
    pub kind: Option<EvidenceKind>,
    pub device_id: Option<String>,
    pub incident_id: Option<String>,
    /// Words that must all occur in the title, the device, the session or
    /// the note. The content is not searched.
    pub query: Option<String>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Listed {
    pub items: Vec<Evidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

pub fn list(app: &App, params: &ListParams) -> AppResult<Listed> {
    let mut links: Vec<String> = Vec::new();
    if let Some(id) = &params.device_id {
        links.push(format!("device:{id}"));
    }
    if let Some(id) = &params.incident_id {
        links.push(format!("incident:{id}"));
    }
    let page = records::page(
        app.store(),
        Kind::Evidence,
        &Query {
            facets: params
                .kind
                .iter()
                .map(|kind| kind.as_str().to_string())
                .collect(),
            words: params
                .query
                .as_deref()
                .map(records::words)
                .unwrap_or_default(),
            links,
            cursor: params.cursor.clone(),
            limit: params.limit.unwrap_or(50).clamp(1, 200),
        },
    )?;
    Ok(Listed {
        items: page
            .heads
            .iter()
            .map(|head| parse(head))
            .collect::<AppResult<_>>()?,
        next_cursor: page.next_cursor,
    })
}

/// The items captured in `[since, until)`, oldest first, for the handover.
pub fn captured_in(app: &App, period: Period) -> AppResult<Vec<Evidence>> {
    // Newest first from the end of the period, a page at a time, and no
    // further back than its start: a handover is made every shift and is
    // not to read every item there has ever been to find the few it wants.
    // A position of `until` with no id is "everything captured before
    // `until`", which is the half-open period's own end.
    let mut items: Vec<Evidence> = Vec::new();
    let mut cursor = Some(format!("{}:", period.until));
    'pages: while let Some(from) = cursor {
        let page = records::page(
            app.store(),
            Kind::Evidence,
            &Query {
                cursor: Some(from),
                limit: 200,
                ..Query::default()
            },
        )?;
        for head in &page.heads {
            let item = parse(head)?;
            if !period.contains(item.captured_at) {
                break 'pages;
            }
            items.push(item);
        }
        cursor = page.next_cursor;
    }
    items.reverse();
    Ok(items)
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
    /// Who is doing it, for the tombstone a deleted item leaves.
    #[serde(default)]
    pub author: String,
}

pub fn get(app: &App, id: &str) -> AppResult<Value> {
    let evidence = load(app, id)?;
    with_body(&evidence, &load_body(app, &evidence)?)
}

// ---------------------------------------------------------- rename / attach

#[derive(Debug, Deserialize)]
pub struct RenameParams {
    pub id: String,
    pub title: String,
}

/// The incidents that changed in a step, announced once it is committed.
fn announce(app: &App, incidents: &[Incident]) {
    for incident in incidents {
        app.publish("incidents.changed", &incident.head());
    }
}

pub fn rename(app: &App, params: &RenameParams) -> AppResult<Evidence> {
    if params.title.trim().is_empty() {
        return Err(AppError::invalid("an evidence item needs a title"));
    }
    let title = title_of(
        Some(params.title.as_str()),
        String::new,
        &EvidenceRedactor::open(app),
    )?;
    let (evidence, was, incidents) = records::transaction(app.store(), |tx| {
        let mut evidence = load_in(tx, &params.id)?;
        let was = std::mem::replace(&mut evidence.title, title.clone());
        if was == evidence.title {
            return Ok((evidence, was, Vec::new()));
        }
        tx.put(Kind::Evidence, &evidence.record()?)?;
        // The timeline shows the title, so it follows the item.
        let mut incidents = Vec::new();
        for incident_id in &evidence.incident_ids {
            if tx.body(Kind::Incident, incident_id)?.is_none() {
                continue;
            }
            let (incident, ()) = incidents::modify_in(tx, incident_id, |incident| {
                for entry in &mut incident.timeline {
                    if entry.id == entry_id(&evidence.id) && entry.reference.is_some() {
                        entry.text = evidence.title.clone();
                    }
                }
                Ok(())
            })?;
            incidents.push(incident);
        }
        Ok((evidence, was, incidents))
    })?;
    if was != evidence.title {
        app.publish("evidence.changed", &evidence);
        announce(app, &incidents);
        app.audit(
            "evidence.rename",
            evidence.source.device_id.as_deref(),
            format!("Renamed evidence `{was}` to `{}`", evidence.title),
            json!({"evidence_id": evidence.id, "sha256": evidence.sha256}),
        );
    }
    Ok(evidence)
}

#[derive(Debug, Deserialize)]
pub struct AttachParams {
    pub id: String,
    pub incident_id: String,
    /// Who is attaching it; it goes on the timeline entry.
    #[serde(default)]
    pub author: String,
}

/// Put the item on an incident's timeline.
///
/// The item and the incident change in one transaction, and the incident is
/// read inside it: a note added a moment before is in what is written back.
pub fn attach(app: &App, params: &AttachParams) -> AppResult<Evidence> {
    let now = now_ms();
    let (evidence, incident, added) = records::transaction(app.store(), |tx| {
        let mut evidence = load_in(tx, &params.id)?;
        let entry = evidence.entry(&params.author, now);
        let (incident, added) = incidents::modify_in(tx, &params.incident_id, |incident| {
            Ok(incident.absorb(vec![entry]) > 0)
        })?;
        if !evidence.incident_ids.contains(&params.incident_id) {
            evidence.incident_ids.push(params.incident_id.clone());
            tx.put(Kind::Evidence, &evidence.record()?)?;
        }
        Ok((evidence, incident, added))
    })?;
    if added {
        app.publish("evidence.changed", &evidence);
        announce(app, std::slice::from_ref(&incident));
        app.audit(
            "evidence.attach",
            evidence.source.device_id.as_deref(),
            format!(
                "Attached evidence `{}` to incident `{}`",
                evidence.title, incident.title
            ),
            json!({
                "evidence_id": evidence.id,
                "incident_id": incident.id,
                "kind": evidence.kind,
                "sha256": evidence.sha256,
                "by": params.author,
            }),
        );
    }
    Ok(evidence)
}

/// Take the item off an incident's timeline. The item itself stays.
pub fn detach(app: &App, params: &AttachParams) -> AppResult<Evidence> {
    let (evidence, incident, removed) = records::transaction(app.store(), |tx| {
        let mut evidence = load_in(tx, &params.id)?;
        let listed = evidence.incident_ids.contains(&params.incident_id);
        evidence.incident_ids.retain(|id| *id != params.incident_id);
        // An incident that has been deleted since has no timeline to take
        // the entry from; the item's own list is still put right. And an
        // incident the item is not on is left exactly as it is.
        let on_timeline = match tx.body(Kind::Incident, &params.incident_id)? {
            Some(body) => incidents::parse(&body)?
                .timeline
                .iter()
                .any(|entry| entry.id == entry_id(&evidence.id)),
            None => false,
        };
        let incident = if on_timeline {
            let (incident, ()) = incidents::modify_in(tx, &params.incident_id, |incident| {
                incident
                    .timeline
                    .retain(|entry| entry.id != entry_id(&evidence.id));
                Ok(())
            })?;
            Some(incident)
        } else {
            None
        };
        if listed {
            tx.put(Kind::Evidence, &evidence.record()?)?;
        }
        let removed = listed || incident.is_some();
        Ok((evidence, incident, removed))
    })?;
    if removed {
        app.publish("evidence.changed", &evidence);
        announce(app, incident.as_slice());
        app.audit(
            "evidence.detach",
            evidence.source.device_id.as_deref(),
            format!(
                "Detached evidence `{}` from {}",
                evidence.title,
                match &incident {
                    Some(incident) => format!("incident `{}`", incident.title),
                    None => "an incident that no longer exists".to_string(),
                }
            ),
            json!({
                "evidence_id": evidence.id,
                "incident_id": params.incident_id,
                "sha256": evidence.sha256,
                "by": params.author,
            }),
        );
    }
    Ok(evidence)
}

// ------------------------------------------------------------------- delete

/// Delete an item for good. Every incident that refers to it keeps the
/// entry, as a tombstone.
pub fn delete(app: &App, params: &IdParams) -> AppResult<Value> {
    let now = now_ms();
    let (evidence, incidents) = records::transaction(app.store(), |tx| {
        let evidence = load_in(tx, &params.id)?;
        let mut incidents = Vec::new();
        for incident_id in &evidence.incident_ids {
            if tx.body(Kind::Incident, incident_id)?.is_none() {
                continue;
            }
            let (incident, left) = incidents::modify_in(tx, incident_id, |incident| {
                let mut left = false;
                for entry in &mut incident.timeline {
                    if entry.id == entry_id(&evidence.id) && entry.reference.is_some() {
                        *entry = evidence.tombstone(entry, &params.author, now);
                        left = true;
                    }
                }
                Ok(left)
            })?;
            if left {
                incidents.push(incident);
            }
        }
        tx.delete(Kind::Evidence, &evidence.id)?;
        tx.conn().execute(
            "DELETE FROM ops_evidence_bodies WHERE id = ?1",
            params![evidence.id],
        )?;
        Ok((evidence, incidents))
    })?;
    let tombstones: Vec<&str> = incidents
        .iter()
        .map(|incident| incident.id.as_str())
        .collect();
    app.publish(
        "evidence.changed",
        &json!({"id": evidence.id, "deleted": true}),
    );
    announce(app, &incidents);
    app.audit(
        "evidence.delete",
        evidence.source.device_id.as_deref(),
        format!("Deleted evidence `{}`", evidence.title),
        json!({
            "evidence_id": evidence.id,
            "kind": evidence.kind,
            "lines": evidence.lines,
            "bytes": evidence.bytes,
            "sha256": evidence.sha256,
            "tombstones": tombstones,
            "by": params.author,
        }),
    );
    Ok(json!({"deleted": true, "tombstones": tombstones}))
}

/// Forget a deleted incident on every item that was attached to it. Part of
/// the step that deletes the incident.
pub(crate) fn forget_incident(tx: &Tx<'_>, incident_id: &str) -> AppResult<Vec<Evidence>> {
    let mut changed = Vec::new();
    for id in tx.linked(Kind::Evidence, &format!("incident:{incident_id}"))? {
        let mut evidence = load_in(tx, &id)?;
        evidence.incident_ids.retain(|known| known != incident_id);
        tx.put(Kind::Evidence, &evidence.record()?)?;
        changed.push(evidence);
    }
    Ok(changed)
}

// ----------------------------------------------------------- for documents

/// The evidence on an incident's timeline as an appendix: each item with
/// what is known about it and its content.
pub fn exhibits(app: &App, incident: &Incident, zone: &Zone) -> AppResult<Vec<Exhibit>> {
    let mut budget = APPENDIX_BYTES;
    let mut exhibits = Vec::new();
    for entry in &incident.timeline {
        let Some(reference) = entry
            .reference
            .as_ref()
            .filter(|_| entry.kind == EntryKind::Evidence)
        else {
            continue;
        };
        let evidence = match load(app, &reference.id) {
            Ok(evidence) => evidence,
            Err(err) => {
                exhibits.push(Exhibit {
                    title: entry.text.clone(),
                    facts: vec![("Evidence".into(), reference.id.clone())],
                    content: Content::Omitted(format!(
                        "the item could not be read: {}",
                        err.message
                    )),
                });
                continue;
            }
        };
        let source = &evidence.source;
        let mut facts: Vec<(String, String)> = vec![
            ("Evidence".into(), evidence.id.clone()),
            ("Kind".into(), evidence.kind.as_str().into()),
            ("Captured".into(), zone.rfc3339(evidence.captured_at)),
            (
                "Source".into(),
                match &source.note {
                    Some(note) => format!("{} — {note}", source.from),
                    None => source.from.clone(),
                },
            ),
        ];
        if let Some(title) = &source.session_title {
            facts.push(("Session".into(), title.clone()));
        }
        if let Some(device) = source.device.as_ref().or(source.device_id.as_ref()) {
            facts.push(("Device".into(), device.clone()));
        }
        if source.lab {
            facts.push(("Lab".into(), "yes — a simulated device".into()));
        }
        if let Some(lines) = evidence.lines {
            facts.push(("Lines".into(), lines.to_string()));
        }
        facts.push(("Size".into(), format!("{} bytes", evidence.bytes)));
        facts.push(("SHA-256".into(), evidence.sha256.clone()));
        facts.push((
            "Redacted".into(),
            "yes — secrets were replaced before it was stored".into(),
        ));

        let content = if evidence.bytes > budget {
            Content::Omitted(format!(
                "this document carries at most {} of evidence and this item is {}; open it in Nocturne",
                text::size(APPENDIX_BYTES),
                text::size(evidence.bytes)
            ))
        } else {
            match load_body(app, &evidence) {
                Ok(body) => {
                    budget -= evidence.bytes;
                    match body {
                        Body::Text { text } => Content::Pre(text),
                        Body::Svg { svg } => Content::Drawing(svg),
                        Body::Chart { series, coverage } => {
                            Content::Table(chart_table(&series, &coverage, zone))
                        }
                    }
                }
                Err(err) => {
                    Content::Omitted(format!("its content could not be read: {}", err.message))
                }
            }
        };
        exhibits.push(Exhibit {
            title: evidence.title.clone(),
            facts,
            content,
        });
    }
    Ok(exhibits)
}

/// A chart's series as rows: what was measured, how much of it there is and
/// between which values it moved. The points themselves stay in the item.
fn chart_table(series: &[Value], coverage: &Coverage, zone: &Zone) -> Table {
    let mut table = Table::new(&[
        ("device", "Device"),
        ("metric", "Metric"),
        ("instance", "Instance"),
        ("points", "Points"),
        ("first_at", "First"),
        ("last_at", "Last"),
        ("min", "Lowest"),
        ("max", "Highest"),
        ("longest_gap", "Longest gap"),
    ]);
    for (covered, one) in coverage.series.iter().zip(series) {
        let values: Vec<f64> = one
            .get("points")
            .and_then(Value::as_array)
            .map(|points| {
                points
                    .iter()
                    .filter_map(|point| point.get(1).and_then(Value::as_f64))
                    .collect()
            })
            .unwrap_or_default();
        let extreme = |pick: fn(f64, f64) -> f64| {
            values
                .iter()
                .copied()
                .reduce(pick)
                .map(|value| Cell::text(format!("{value}")))
        };
        table.push(vec![
            Cell::text(&covered.device),
            Cell::text(&covered.metric),
            Cell::text(&covered.instance),
            Cell::count(covered.points),
            Cell::maybe(covered.first_at.map(|at| Cell::time(zone, at))),
            Cell::maybe(covered.last_at.map(|at| Cell::time(zone, at))),
            Cell::maybe(extreme(f64::min)),
            Cell::maybe(extreme(f64::max)),
            Cell::maybe(covered.longest_gap_ms.map(Cell::duration)),
        ]);
    }
    table
}

// ----------------------------------------------------------------- registry

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add(
        "evidence.capture_terminal",
        |app: App, params: CaptureTerminalParams| async move {
            capture_terminal(&app, params).await
        },
    );
    router.add(
        "evidence.capture_log",
        |app: App, params: CaptureLogParams| async move { capture_log(&app, params).await },
    );
    router.add(
        "evidence.capture_topology",
        |app: App, params: CaptureTopologyParams| async move {
            capture_topology(&app, params).await
        },
    );
    router.add(
        "evidence.capture_chart",
        |app: App, params: CaptureChartParams| async move { capture_chart(&app, params).await },
    );
    router.add(
        "evidence.add_text",
        |app: App, params: AddTextParams| async move { add_text(&app, params).await },
    );
    router.add("evidence.list", |app: App, params: ListParams| async move {
        list(&app, &params)
    });
    router.add("evidence.get", |app: App, params: IdParams| async move {
        get(&app, &params.id)
    });
    router.add(
        "evidence.rename",
        |app: App, params: RenameParams| async move { rename(&app, &params) },
    );
    router.add(
        "evidence.attach",
        |app: App, params: AttachParams| async move { attach(&app, &params) },
    );
    router.add(
        "evidence.detach",
        |app: App, params: AttachParams| async move { detach(&app, &params) },
    );
    router.add("evidence.delete", |app: App, params: IdParams| async move {
        delete(&app, &params)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn item(kind: EvidenceKind) -> Evidence {
        Evidence {
            id: "e1".into(),
            kind,
            title: "show interfaces on SW1".into(),
            captured_at: 1_000,
            source: Source {
                from: "terminal.snapshot".into(),
                session_title: Some("SW1".into()),
                device_id: Some("d1".into()),
                device: Some("SW1".into()),
                lab: true,
                ..Source::default()
            },
            redacted: true,
            lines: Some(12),
            bytes: 640,
            sha256: "ab".repeat(32),
            incident_ids: vec!["i1".into()],
        }
    }

    #[test]
    fn a_timeline_entry_points_at_the_item_and_never_carries_its_text() {
        let evidence = item(EvidenceKind::Terminal);
        let entry = evidence.entry("mo", 9_000);
        assert_eq!(entry.id, "evidence:e1");
        assert_eq!(entry.ts, 1_000, "at the time of capture, not of attaching");
        assert_eq!(entry.kind, EntryKind::Evidence);
        assert_eq!(entry.text, "show interfaces on SW1");
        assert_eq!(entry.reference, Some(Ref::new("evidence", "e1")));
        assert_eq!(entry.source.from, "terminal.snapshot");
        assert_eq!(
            entry.source.evidence,
            format!(
                "evidence `e1`: terminal session `SW1` (lab), 12 lines, redacted, sha256 {}",
                "ab".repeat(32)
            )
        );

        // 2026-11-13 14:05:09 UTC.
        let deleted_at = (20_770 * 86_400 + 14 * 3_600 + 5 * 60 + 9) * 1000;
        let tombstone = evidence.tombstone(&entry, "zahra", deleted_at);
        assert_eq!(
            (tombstone.id.as_str(), tombstone.ts),
            ("evidence:e1", 1_000)
        );
        assert_eq!(
            tombstone.text,
            "show interfaces on SW1 — evidence deleted on 2026-11-13 (UTC)"
        );
        assert_eq!(tombstone.reference, None, "it points at nothing any more");
        assert_eq!(tombstone.source.from, "evidence.delete");
        assert!(
            tombstone.source.evidence.contains(&"ab".repeat(32)),
            "the hash outlives the item"
        );
    }

    #[test]
    fn an_item_is_found_by_its_devices_and_incidents_and_not_by_its_content() {
        let mut evidence = item(EvidenceKind::Chart);
        evidence.source.params = Some(json!({"series": [
            {"device_id": "d1", "metric": "up"},
            {"device_id": "d2", "metric": "up"},
        ]}));
        let record = evidence.record().unwrap();
        assert_eq!(record.links, ["device:d1", "device:d2", "incident:i1"]);
        assert_eq!(record.facet, "chart");
        assert_eq!(record.sort, 1_000);
        assert_eq!(
            record.search,
            "show interfaces on sw1 chart terminal.snapshot sw1 sw1"
        );
        assert_eq!(record.head, record.body);
        assert!(!record.head.contains("\"body\""));
    }

    #[test]
    fn a_drawing_of_everything_is_labelled_lab_when_the_estate_has_a_lab_in_it() {
        use nocturne_app::model::{Device, DeviceRow, DeviceState, LabRef};
        let row = |id: &str, site: &str, lab: bool| DeviceRow {
            device: Device {
                id: id.into(),
                name: id.into(),
                site: site.into(),
                lab: lab.then(|| LabRef {
                    lab_id: "l1".into(),
                    node: id.into(),
                }),
                ..Device::default()
            },
            state: DeviceState::default(),
        };
        let devices = Devices::from_rows(vec![row("hw", "HQ", false), row("sim", "Lab", true)]);
        assert!(may_show_lab(&devices, None));
        assert!(may_show_lab(&devices, Some(&json!({"site": "lab"}))));
        assert!(!may_show_lab(&devices, Some(&json!({"site": "HQ"}))));
        assert!(!may_show_lab(
            &devices,
            Some(&json!({"device_ids": ["hw"]}))
        ));
        assert!(may_show_lab(
            &devices,
            Some(&json!({"device_ids": ["hw", "sim"]}))
        ));
        // A filter that names a lab is about a lab, whatever else it says.
        assert!(may_show_lab(
            &Devices::empty(),
            Some(&json!({"lab_id": "l1"}))
        ));
        assert!(!may_show_lab(&Devices::empty(), None));
    }

    #[test]
    fn a_chart_body_is_stored_and_read_back_as_the_same_thing() {
        let body = Body::Chart {
            series: vec![
                json!({"device_id": "d1", "metric": "up", "points": [[1, 1.0], [3, 0.0]]}),
            ],
            coverage: Coverage {
                since: 0,
                until: 10,
                points: 2,
                series: vec![SeriesCoverage {
                    device_id: "d1".into(),
                    device: "SW1".into(),
                    lab: false,
                    metric: "up".into(),
                    instance: String::new(),
                    points: 2,
                    first_at: Some(1),
                    last_at: Some(3),
                    longest_gap_ms: Some(2),
                    step_s: Some(60),
                    rollup: Some("raw".into()),
                }],
            },
        };
        let stored = body.stored().unwrap();
        assert_eq!(Body::read(EvidenceKind::Chart, stored).unwrap(), body);
        let text = Body::Text {
            text: "SW1#show clock".into(),
        };
        assert_eq!(text.stored().unwrap(), "SW1#show clock");
        assert_eq!(
            serde_json::to_value(&text).unwrap(),
            json!({"type": "text", "text": "SW1#show clock"})
        );
        assert_eq!(
            times(&json!({"points": [[5, 1.5], [9, null]]})),
            Some(vec![5, 9])
        );
        assert_eq!(times(&json!({"points": [[5]]})), None);
        assert_eq!(times(&json!({"points": "none"})), None);
    }
}
