//! Session logs on disk: their index, and listing, reading and deleting them.
//!
//! The files live under `<data_dir>/sessions/YYYY-MM/`. A year of logging
//! every session is tens of thousands of files, so what a listing needs —
//! device, title, start — is kept in a small table of this feature's own and
//! queried there; the directory is only touched for the rows returned.

use std::collections::HashSet;
use std::io::{self, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppBuilder, AppError, AppResult, Router};
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{OptionalExtension, params, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncSeekExt};

use crate::api::audit_unredacted;
use crate::model::kind_name;
use crate::platform;
use crate::text::{incomplete_utf8_suffix, redact_window};
use nocturne_transport::TransportKind;

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS terminal_session_logs (
        name       TEXT PRIMARY KEY,
        device_id  TEXT,
        title      TEXT NOT NULL,
        kind       TEXT NOT NULL,
        started_at INTEGER NOT NULL,
        ended_at   INTEGER
    ) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS terminal_session_logs_started
        ON terminal_session_logs (started_at, name);
    CREATE INDEX IF NOT EXISTS terminal_session_logs_device
        ON terminal_session_logs (device_id, started_at, name);
";

const DEFAULT_LIST: u32 = 100;
const MAX_LIST: u32 = 1000;
const DEFAULT_READ: u64 = 256 * 1024;
const MAX_READ: u64 = 1024 * 1024;
/// How far back from an offset the start of its line is looked for: as far
/// as a page can reach, so that a line a page can hold is always read from
/// its start, wherever in it a read falls. (The writer ends a line itself
/// after 64 KiB of output without a line break, but that is counted in what
/// the device sent, a read at a time, and a byte that is not text is three
/// in the file.)
///
/// Not one byte further, and four short of it. A line too long for a page is
/// handed out in pieces, each a full page less the up to three bytes of a
/// character it would have split; the read after a piece must not find its
/// way back to the start of the line, or it would be given the same piece
/// again for ever.
const LINE_SCAN: u64 = MAX_READ - 4;
/// How much of the file on either side of a page is read, and never
/// returned, to learn whether the page lies inside a private key. A key in a
/// log is a few kilobytes, timestamps included.
const KEY_CONTEXT: u64 = 256 * 1024;
/// How much is read at a time while looking for the end of a long line.
const SCAN_STEP: u64 = 64 * 1024;

/// `YYYY-MM/<file>.log`: exactly one directory and one file, neither able to climb out.
static NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{4}-\d{2}/[^/\\]+\.log$").expect("log name pattern"));

/// Logs that a live session is writing right now.
#[derive(Default)]
pub struct LogsState {
    active: Mutex<HashSet<String>>,
}

/// Marks a log as being written for as long as it exists.
pub(crate) struct ActiveLog {
    state: Arc<LogsState>,
    name: String,
}

impl Drop for ActiveLog {
    fn drop(&mut self) {
        self.state.active.lock().remove(&self.name);
    }
}

/// What the index keeps about a log.
pub(crate) struct LogRecord {
    pub name: String,
    pub device_id: Option<String>,
    pub title: String,
    pub kind: TransportKind,
    pub started_at: i64,
}

/// A session log as `terminal.logs.list` reports it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionLogInfo {
    pub name: String,
    pub device_id: Option<String>,
    pub title: String,
    pub kind: String,
    pub started_at: i64,
    /// `None` while the session runs — and for a session that was cut short
    /// by Nocturne itself ending, where the moment is not known.
    pub ended_at: Option<i64>,
    pub size: u64,
    /// A live session is still writing to it.
    pub active: bool,
}

pub(crate) fn sessions_dir(app: &App) -> Option<PathBuf> {
    app.data_dir().map(|dir| dir.join("sessions"))
}

/// Add a log that is about to be written to the index.
pub(crate) fn record(app: &App, record: &LogRecord) -> AppResult<ActiveLog> {
    app.store().with(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO terminal_session_logs (name, device_id, title, kind, started_at, ended_at)
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                record.name,
                record.device_id,
                record.title,
                kind_name(record.kind),
                record.started_at
            ],
        )
    })?;
    let state = app.state::<LogsState>();
    state.active.lock().insert(record.name.clone());
    Ok(ActiveLog {
        state,
        name: record.name.clone(),
    })
}

pub(crate) fn mark_ended(app: &App, name: &str) {
    let updated = app.store().with(|conn| {
        conn.execute(
            "UPDATE terminal_session_logs SET ended_at = ?2 WHERE name = ?1",
            params![name, now_ms()],
        )
    });
    if let Err(err) = updated {
        tracing::warn!(%err, name, "the end of a session log could not be recorded");
    }
}

fn forget(app: &App, name: &str) -> AppResult<bool> {
    let removed = app.store().with(|conn| {
        conn.execute(
            "DELETE FROM terminal_session_logs WHERE name = ?1",
            params![name],
        )
    })?;
    Ok(removed > 0)
}

/// The file a log name stands for.
fn path_of(app: &App, name: &str) -> AppResult<PathBuf> {
    if !NAME.is_match(name) {
        return Err(AppError::invalid(format!(
            "`{name}` is not the name of a session log (expected `YYYY-MM/<file>.log`, as `terminal.logs.list` returns)"
        )));
    }
    let Some(root) = sessions_dir(app) else {
        return Err(AppError::not_found(format!(
            "session log `{name}` (this Nocturne instance runs without a data directory and keeps no logs)"
        )));
    };
    let (month, file) = name.split_once('/').unwrap_or_default();
    Ok(root.join(month).join(file))
}

// ---------------------------------------------------------------------- list

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ListParams {
    device_id: Option<String>,
    /// Words that must all appear in the title or file name.
    query: Option<String>,
    limit: Option<u32>,
    /// Continue after this log (the last name of the previous page).
    before: Option<String>,
}

struct Row {
    name: String,
    device_id: Option<String>,
    title: String,
    kind: String,
    started_at: i64,
    ended_at: Option<i64>,
}

/// One page of the index, newest first, strictly older than `cursor`.
fn query_page(
    app: &App,
    filter: &ListParams,
    cursor: Option<&(i64, String)>,
    limit: u32,
) -> AppResult<Vec<Row>> {
    let mut sql = String::from(
        "SELECT name, device_id, title, kind, started_at, ended_at FROM terminal_session_logs WHERE 1=1",
    );
    let mut args: Vec<rusqlite::types::Value> = Vec::new();
    if let Some(device_id) = &filter.device_id {
        sql.push_str(" AND device_id = ?");
        args.push(device_id.clone().into());
    }
    for word in filter
        .query
        .as_deref()
        .unwrap_or_default()
        .split_whitespace()
    {
        let pattern = format!(
            "%{}%",
            word.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        sql.push_str(" AND (title LIKE ? ESCAPE '\\' OR name LIKE ? ESCAPE '\\')");
        args.push(pattern.clone().into());
        args.push(pattern.into());
    }
    if let Some((started_at, name)) = cursor {
        sql.push_str(" AND (started_at, name) < (?, ?)");
        args.push((*started_at).into());
        args.push(name.clone().into());
    }
    sql.push_str(" ORDER BY started_at DESC, name DESC LIMIT ?");
    args.push(i64::from(limit).into());

    app.store().with(|conn| {
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(args.iter()), |row| {
            Ok(Row {
                name: row.get(0)?,
                device_id: row.get(1)?,
                title: row.get(2)?,
                kind: row.get(3)?,
                started_at: row.get(4)?,
                ended_at: row.get(5)?,
            })
        })?;
        rows.collect()
    })
}

async fn list(app: &App, filter: ListParams) -> AppResult<Vec<SessionLogInfo>> {
    let Some(root) = sessions_dir(app) else {
        return Ok(Vec::new());
    };
    let limit = filter.limit.unwrap_or(DEFAULT_LIST).clamp(1, MAX_LIST);
    let mut cursor = match &filter.before {
        None => None,
        Some(name) => {
            let started_at: Option<i64> = app.store().with(|conn| {
                conn.query_row(
                    "SELECT started_at FROM terminal_session_logs WHERE name = ?1",
                    params![name],
                    |row| row.get(0),
                )
                .optional()
            })?;
            let started_at = started_at.ok_or_else(|| {
                AppError::not_found(format!(
                    "session log `{name}` (given as `before`; list again from the start)"
                ))
            })?;
            Some((started_at, name.clone()))
        }
    };

    let state = app.state::<LogsState>();
    let mut logs = Vec::new();
    while logs.len() < limit as usize {
        let wanted = limit - logs.len() as u32;
        let page = query_page(app, &filter, cursor.as_ref(), wanted)?;
        let Some(last) = page.last() else { break };
        cursor = Some((last.started_at, last.name.clone()));
        let exhausted = page.len() < wanted as usize;
        for row in page {
            match tokio::fs::metadata(root.join(&row.name)).await {
                Ok(metadata) => logs.push(SessionLogInfo {
                    active: state.active.lock().contains(&row.name),
                    size: metadata.len(),
                    name: row.name,
                    device_id: row.device_id,
                    title: row.title,
                    kind: row.kind,
                    started_at: row.started_at,
                    ended_at: row.ended_at,
                }),
                // Deleted behind our back: the index follows the directory.
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    forget(app, &row.name)?;
                }
                Err(err) => {
                    return Err(AppError::new(
                        "io",
                        format!("session log `{}` cannot be read: {err}", row.name),
                    ));
                }
            }
        }
        if exhausted {
            break;
        }
    }
    Ok(logs)
}

// ---------------------------------------------------------------------- read

#[derive(Debug, Deserialize)]
struct ReadParams {
    name: String,
    /// Byte offset into the file. The page starts with the line it falls in.
    #[serde(default)]
    offset: u64,
    /// About how many bytes to read: whole lines up to this much, and always
    /// at least one.
    #[serde(default)]
    limit: Option<u64>,
    /// `false` returns the text as written; by default device secrets are hidden.
    #[serde(default)]
    redact: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ReadResult {
    text: String,
    /// Where `text` starts: the start of the line the requested offset fell
    /// in, so at or before it.
    offset: u64,
    /// Size of the file in bytes.
    size: u64,
    eof: bool,
    /// Where to continue reading. Not `offset + text.length`: redaction
    /// changes lengths, and a read ends on a line boundary.
    next_offset: u64,
    redacted: bool,
}

/// Up to `length` bytes of `file` from `from`.
async fn read_range(file: &mut tokio::fs::File, from: u64, length: u64) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if length > 0 {
        file.seek(SeekFrom::Start(from)).await?;
        file.take(length).read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

/// Where the line that `offset` falls in starts, or `None` when that is
/// further back than [`LINE_SCAN`].
async fn line_start(file: &mut tokio::fs::File, offset: u64) -> io::Result<Option<u64>> {
    // No line break in `[searched, offset)` so far.
    let mut searched = offset;
    while searched > 0 {
        let room = LINE_SCAN - (offset - searched);
        if room == 0 {
            return Ok(None);
        }
        // One byte first: a page asked for where the last one ended starts
        // a line, and the byte before it says so.
        let step = if searched == offset { 1 } else { SCAN_STEP };
        let step = step.min(searched).min(room);
        let from = searched - step;
        let chunk = read_range(file, from, step).await?;
        if let Some(newline) = chunk.iter().rposition(|byte| *byte == b'\n') {
            return Ok(Some(from + newline as u64 + 1));
        }
        searched = from;
    }
    Ok(Some(0))
}

/// A page of a log: whole lines, as many as fit in `limit` bytes and never
/// fewer than one.
struct Page {
    /// Where the page starts in the file.
    start: u64,
    bytes: Vec<u8>,
    /// Size of the file.
    size: u64,
}

impl Page {
    fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }
}

/// Read the page that `offset` falls in.
///
/// A page is cut at line breaks at both ends, wherever the caller's offset
/// and limit happen to fall. Half a line cannot be redacted: `enable secret
/// Sup3rS3cret` read from its sixteenth byte is a word with nothing to say
/// what it is. So a page begins with the line the offset is in, and when not
/// even one line fits in `limit` it is that one line.
async fn read_page(path: &Path, offset: u64, limit: u64) -> io::Result<Page> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    let offset = offset.min(size);
    let (mut start, at_line_start) = match line_start(&mut file, offset).await? {
        Some(start) => (start, true),
        // A line longer than anything the writer makes. The page begins
        // where it was asked to; there is nothing better to offer.
        None => (offset, false),
    };
    let mut bytes = read_range(&mut file, start, limit).await?;
    if !at_line_start {
        // The offset may have fallen inside a character: skip its remainder.
        let inside = bytes
            .iter()
            .take(3)
            .take_while(|byte| matches!(byte, 0x80..=0xBF))
            .count();
        bytes.drain(..inside);
        start += inside as u64;
    }
    if start + (bytes.len() as u64) < size {
        match bytes.iter().rposition(|byte| *byte == b'\n') {
            Some(newline) => bytes.truncate(newline + 1),
            None => {
                // Not one whole line in `limit` bytes: read on to the end of
                // this one, for as long as a page may be.
                loop {
                    let have = bytes.len() as u64;
                    if have >= MAX_READ || start + have >= size {
                        break;
                    }
                    let step = SCAN_STEP.min(MAX_READ - have);
                    let more = read_range(&mut file, start + have, step).await?;
                    if more.is_empty() {
                        break;
                    }
                    if let Some(newline) = more.iter().position(|byte| *byte == b'\n') {
                        bytes.extend_from_slice(&more[..=newline]);
                        break;
                    }
                    bytes.extend_from_slice(&more);
                }
                if !bytes.ends_with(b"\n") && start + (bytes.len() as u64) < size {
                    // Longer than a page may be: at least never split a character.
                    let incomplete = incomplete_utf8_suffix(&bytes);
                    bytes.truncate(bytes.len() - incomplete);
                }
            }
        }
    }
    Ok(Page { start, bytes, size })
}

/// The lines of a piece of the file. Bytes that are not UTF-8 (a device can
/// print anything) become replacement characters, as in the page itself.
fn lines_of(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split('\n')
        .map(str::to_string)
        .collect()
}

/// The page with its secrets hidden.
///
/// A page of whole lines is enough for every secret but a private key, which
/// is longer than a line: a page can hold lines of one and neither of its
/// markers. So the file is read a little way to both sides of the page, and
/// what is found there decides whether the page's lines are key material.
async fn redact_page(
    path: &Path,
    page: &Page,
    rules: &[String],
) -> io::Result<crate::text::Redaction> {
    let mut file = tokio::fs::File::open(path).await?;
    let before_from = page.start.saturating_sub(KEY_CONTEXT);
    let before = read_range(&mut file, before_from, page.start - before_from).await?;
    let after = read_range(
        &mut file,
        page.end(),
        KEY_CONTEXT.min(page.size - page.end()),
    )
    .await?;

    let mut lines = lines_of(&before);
    if before.is_empty() || before.ends_with(b"\n") {
        // The page starts a line of its own: what `split` left after the last
        // line break before it is not a line.
        lines.pop();
    }
    let first = lines.len();
    lines.extend(lines_of(&page.bytes));
    let last = lines.len();
    lines.extend(lines_of(&after));
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    Ok(redact_window(&lines, first..last, rules))
}

async fn read(app: &App, request: ReadParams) -> AppResult<ReadResult> {
    let name = request.name.as_str();
    let path = path_of(app, name)?;
    let limit = request.limit.unwrap_or(DEFAULT_READ).clamp(1, MAX_READ);
    let unreadable = |err: io::Error| match err.kind() {
        io::ErrorKind::NotFound => AppError::not_found(format!("session log `{name}`")),
        _ => AppError::new("io", format!("session log `{name}` cannot be read: {err}")),
    };
    let page = read_page(&path, request.offset, limit)
        .await
        .map_err(unreadable)?;
    let next_offset = page.end();

    let indexed: Option<(Option<String>, String)> = app.store().with(|conn| {
        conn.query_row(
            "SELECT device_id, title FROM terminal_session_logs WHERE name = ?1",
            params![name],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
    })?;
    let device_id = indexed
        .as_ref()
        .and_then(|(device_id, _)| device_id.clone());

    let (text, redacted) = if request.redact == Some(false) {
        // A read past the end shows nothing, so there is nothing to record:
        // following a log that is still being written asks there again and again.
        if !page.bytes.is_empty() {
            let title = indexed.as_ref().map_or(name, |(_, title)| title.as_str());
            audit_unredacted(
                app,
                device_id.as_deref(),
                "session log",
                title,
                json!({
                    "source": "log",
                    "name": name,
                    "offset": page.start,
                    "bytes": page.bytes.len(),
                }),
            );
        }
        (String::from_utf8_lossy(&page.bytes).into_owned(), false)
    } else {
        let rules = platform::secret_rules(app, device_id.as_deref());
        let redaction = redact_page(&path, &page, &rules)
            .await
            .map_err(unreadable)?;
        (redaction.text, redaction.redacted)
    };
    Ok(ReadResult {
        text,
        offset: page.start,
        size: page.size,
        eof: next_offset >= page.size,
        next_offset,
        redacted,
    })
}

// -------------------------------------------------------------------- delete

#[derive(Debug, Deserialize)]
struct DeleteParams {
    names: Vec<String>,
}

#[derive(Debug, Serialize)]
struct NotDeleted {
    name: String,
    code: &'static str,
    message: String,
}

#[derive(Debug, Serialize)]
struct DeleteResult {
    deleted: usize,
    failed: Vec<NotDeleted>,
}

async fn delete_one(app: &App, name: &str) -> AppResult<()> {
    let path = path_of(app, name)?;
    if app.state::<LogsState>().active.lock().contains(name) {
        return Err(AppError::conflict(format!(
            "`{name}` is being written by an open session; turn its logging off or close it first"
        )));
    }
    let known = forget(app, name)?;
    match tokio::fs::remove_file(&path).await {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound && known => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(AppError::not_found(format!("session log `{name}`")));
        }
        Err(err) => {
            return Err(AppError::new(
                "io",
                format!("session log `{name}` cannot be deleted: {err}"),
            ));
        }
    }
    // The month's directory goes with its last log; it is refused while files remain.
    if let Some(month) = path.parent() {
        let _ = tokio::fs::remove_dir(month).await;
    }
    Ok(())
}

async fn delete(app: &App, request: DeleteParams) -> AppResult<DeleteResult> {
    if request.names.is_empty() {
        return Err(AppError::invalid("no session logs named to delete"));
    }
    let mut deleted = Vec::new();
    let mut failed = Vec::new();
    for name in request.names {
        match delete_one(app, &name).await {
            Ok(()) => deleted.push(name),
            Err(err) => failed.push(NotDeleted {
                name,
                code: err.code,
                message: err.message,
            }),
        }
    }
    if !deleted.is_empty() {
        let count = deleted.len();
        app.audit(
            "terminal.logs.delete",
            None,
            format!(
                "Deleted {count} session log{}",
                if count == 1 { "" } else { "s" }
            ),
            json!({"names": deleted}),
        );
        app.publish("terminal.logs.changed", &json!({"deleted": deleted}));
    }
    Ok(DeleteResult {
        deleted: deleted.len(),
        failed,
    })
}

pub(crate) fn init(builder: &mut AppBuilder) {
    if let Err(err) = builder.store().with(|conn| conn.execute_batch(SCHEMA)) {
        // Sessions still work; asking for a log then reports this database problem.
        tracing::error!(%err, "the index of session logs could not be prepared");
    }
    builder.state(LogsState::default());
}

pub(crate) fn register(router: &mut Router) {
    router.add(
        "terminal.logs.list",
        |app: App, params: ListParams| async move { list(&app, params).await },
    );
    router.add(
        "terminal.logs.read",
        |app: App, params: ReadParams| async move { read(&app, params).await },
    );
    router.add(
        "terminal.logs.delete",
        |app: App, params: DeleteParams| async move { delete(&app, params).await },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_names_cannot_leave_the_sessions_directory() {
        for good in [
            "2026-11/SW1-20261113-052405.log",
            "2026-01/admin@10.0.0.1-20260101-000000-2.log",
        ] {
            assert!(NAME.is_match(good), "{good}");
        }
        for bad in [
            "../secrets.log",
            "2026-11/../../etc/passwd.log",
            "2026-11/sub/SW1.log",
            "2026-11\\SW1.log",
            "2026-11/SW1.txt",
            "/etc/passwd",
            "2026-11/",
            "SW1.log",
            "",
        ] {
            assert!(!NAME.is_match(bad), "{bad}");
        }
    }
}
