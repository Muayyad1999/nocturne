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

use crate::model::kind_name;
use crate::platform;
use crate::text::{incomplete_utf8_suffix, redact_transcript};
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
    /// Byte offset into the file.
    #[serde(default)]
    offset: u64,
    /// Most bytes to read.
    #[serde(default)]
    limit: Option<u64>,
    /// `false` returns the text as written; by default device secrets are hidden.
    #[serde(default)]
    redact: Option<bool>,
}

#[derive(Debug, Serialize)]
struct ReadResult {
    text: String,
    /// Size of the file in bytes.
    size: u64,
    eof: bool,
    /// Where to continue reading. Not `offset + text.length`: redaction
    /// changes lengths, and a read ends on a line boundary.
    next_offset: u64,
    redacted: bool,
}

/// Up to `limit` bytes of `path` from `offset`, and the size of the file.
async fn read_chunk(path: &Path, offset: u64, limit: u64) -> io::Result<(Vec<u8>, u64)> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    let mut bytes = Vec::new();
    if offset < size {
        file.seek(SeekFrom::Start(offset)).await?;
        file.take(limit).read_to_end(&mut bytes).await?;
    }
    Ok((bytes, size))
}

/// Cut a chunk read at an arbitrary place down to whole characters and, when
/// more of the file follows, whole lines. Returns how many bytes were dropped
/// at the start, and the text.
fn whole_lines(
    mut bytes: Vec<u8>,
    starts_inside_file: bool,
    more_follows: bool,
) -> (usize, Vec<u8>) {
    let skipped = if starts_inside_file {
        bytes
            .iter()
            .take(3)
            .take_while(|byte| matches!(byte, 0x80..=0xBF))
            .count()
    } else {
        0
    };
    bytes.drain(..skipped);
    if more_follows {
        match bytes.iter().rposition(|byte| *byte == b'\n') {
            Some(newline) => bytes.truncate(newline + 1),
            None => {
                let incomplete = incomplete_utf8_suffix(&bytes);
                bytes.truncate(bytes.len() - incomplete);
            }
        }
    }
    (skipped, bytes)
}

async fn read(app: &App, request: ReadParams) -> AppResult<ReadResult> {
    let name = request.name.as_str();
    let path = path_of(app, name)?;
    let limit = request.limit.unwrap_or(DEFAULT_READ).clamp(1, MAX_READ);
    let (bytes, size) = read_chunk(&path, request.offset, limit)
        .await
        .map_err(|err| match err.kind() {
            io::ErrorKind::NotFound => AppError::not_found(format!("session log `{name}`")),
            _ => AppError::new("io", format!("session log `{name}` cannot be read: {err}")),
        })?;
    let offset = request.offset.min(size);
    let more_follows = offset + (bytes.len() as u64) < size;
    let (skipped, bytes) = whole_lines(bytes, offset > 0, more_follows);
    let next_offset = offset + (skipped + bytes.len()) as u64;

    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (text, redacted) = if request.redact == Some(false) {
        (text, false)
    } else {
        let device_id: Option<String> = app.store().with(|conn| {
            conn.query_row(
                "SELECT device_id FROM terminal_session_logs WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .map(Option::flatten)
        })?;
        let redaction =
            redact_transcript(&text, &platform::secret_rules(app, device_id.as_deref()));
        (redaction.text, redaction.redacted)
    };
    Ok(ReadResult {
        text,
        size,
        eof: next_offset >= size,
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

    #[test]
    fn chunks_are_cut_to_whole_lines_and_characters() {
        // More follows: stop after the last complete line.
        let (skipped, text) = whole_lines(b"one\ntwo\nthr".to_vec(), false, true);
        assert_eq!((skipped, text.as_slice()), (0, &b"one\ntwo\n"[..]));
        // The end of the file is returned as it is.
        let (_, text) = whole_lines(b"one\ntwo\nthr".to_vec(), false, false);
        assert_eq!(text, b"one\ntwo\nthr");
        // A line longer than the chunk: at least never split a character.
        let (_, text) = whole_lines("caf\u{e9}".as_bytes()[..4].to_vec(), false, true);
        assert_eq!(text, b"caf");
        // A read that starts inside a character skips its remainder.
        let (skipped, text) = whole_lines("\u{2603}now\n".as_bytes()[1..].to_vec(), true, true);
        assert_eq!((skipped, text.as_slice()), (2, &b"now\n"[..]));
    }
}
