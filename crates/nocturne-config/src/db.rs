//! Storage that is this feature's own.
//!
//! Configuration snapshots live in the core's `configs` table and are written
//! through `store.config_insert` (the change pipeline stores its pre- and
//! post-change snapshots the same way). What that table has no column for —
//! when a version was last seen unchanged, its size, why it is pinned — is
//! kept beside it, keyed by the version id. Compliance runs get tables of
//! their own because a run is one finding per device and rule, and the
//! results screen filters and pages through them.
//!
//! This is also the one place that queries `configs` directly, for what the
//! store does not offer: paging, the previous version, and removing single
//! versions (retention, `config.delete`).

use nocturne_app::AppResult;
use nocturne_app::store::Store;
use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS config_meta (
    version_id INTEGER PRIMARY KEY,
    last_seen  INTEGER NOT NULL,
    bytes      INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS config_pins (
    version_id INTEGER NOT NULL,
    reason     TEXT NOT NULL,
    pinned_at  INTEGER NOT NULL,
    PRIMARY KEY (version_id, reason)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS compliance_runs (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,
    id           TEXT NOT NULL UNIQUE,
    at           INTEGER NOT NULL,
    summary      TEXT NOT NULL,
    has_findings INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS compliance_findings (
    run_seq   INTEGER NOT NULL,
    seq       INTEGER NOT NULL,
    device_id TEXT NOT NULL,
    policy_id TEXT NOT NULL,
    rule_id   TEXT NOT NULL,
    status    TEXT NOT NULL,
    severity  TEXT NOT NULL,
    body      TEXT NOT NULL,
    PRIMARY KEY (run_seq, seq)
) WITHOUT ROWID;
";

pub(crate) fn init(store: &Store) -> AppResult<()> {
    store.with(|conn| conn.execute_batch(SCHEMA))
}

// ------------------------------------------------------------------ versions

/// A row of `configs` with this feature's metadata joined on.
#[derive(Clone, Debug)]
pub(crate) struct VersionRow {
    pub id: i64,
    pub device_id: String,
    pub kind: String,
    pub sha256: String,
    pub lines: u32,
    pub taken_at: i64,
    pub source: String,
    pub note: String,
    /// `None` for a version this feature has not looked at yet (stored by the change pipeline).
    pub last_seen: Option<i64>,
    pub bytes: Option<u64>,
    pub pins: Vec<String>,
}

const VERSION_SELECT: &str = "
    SELECT c.id, c.device_id, c.kind, c.sha256, c.lines, c.taken_at, c.source, c.note, m.last_seen, m.bytes,
           (SELECT group_concat(p.reason, char(10)) FROM config_pins p WHERE p.version_id = c.id)
    FROM configs c LEFT JOIN config_meta m ON m.version_id = c.id";

fn version_row(row: &Row<'_>) -> rusqlite::Result<VersionRow> {
    let pins: Option<String> = row.get(10)?;
    let mut pins: Vec<String> = pins
        .map(|joined| joined.lines().map(str::to_string).collect())
        .unwrap_or_default();
    pins.sort();
    Ok(VersionRow {
        id: row.get(0)?,
        device_id: row.get(1)?,
        kind: row.get(2)?,
        sha256: row.get(3)?,
        lines: row.get(4)?,
        taken_at: row.get(5)?,
        source: row.get(6)?,
        note: row.get(7)?,
        last_seen: row.get(8)?,
        // SQLite has no unsigned integers: a size is stored as the i64 it fits in.
        bytes: row.get::<_, Option<i64>>(9)?.map(|bytes| bytes as u64),
        pins,
    })
}

pub(crate) fn version(store: &Store, id: i64) -> AppResult<Option<VersionRow>> {
    store.with(|conn| {
        conn.query_row(
            &format!("{VERSION_SELECT} WHERE c.id = ?1"),
            params![id],
            version_row,
        )
        .optional()
    })
}

/// Versions of one device and kind, newest first, starting below `before_id`.
pub(crate) fn page(
    store: &Store,
    device_id: &str,
    kind: &str,
    before_id: Option<i64>,
    limit: usize,
) -> AppResult<Vec<VersionRow>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(&format!(
            "{VERSION_SELECT} WHERE c.device_id = ?1 AND c.kind = ?2 AND (?3 IS NULL OR c.id < ?3)
             ORDER BY c.id DESC LIMIT ?4"
        ))?;
        let rows = statement.query_map(
            params![device_id, kind, before_id, limit as i64],
            version_row,
        )?;
        rows.collect()
    })
}

fn newest_on(
    conn: &Connection,
    device_id: &str,
    kind: &str,
    count: usize,
) -> rusqlite::Result<Vec<VersionRow>> {
    // The same order the store uses to decide which version is the latest.
    let mut statement = conn.prepare_cached(&format!(
        "{VERSION_SELECT} WHERE c.device_id = ?1 AND c.kind = ?2 ORDER BY c.taken_at DESC, c.id DESC LIMIT ?3"
    ))?;
    let rows = statement.query_map(params![device_id, kind, count as i64], version_row)?;
    rows.collect()
}

/// The newest `count` versions of one device and kind, newest first.
pub(crate) fn newest(
    store: &Store,
    device_id: &str,
    kind: &str,
    count: usize,
) -> AppResult<Vec<VersionRow>> {
    store.with(|conn| newest_on(conn, device_id, kind, count))
}

/// The latest version of each of `device_ids` (those that have one), in that order.
pub(crate) fn latest_of_each(
    store: &Store,
    device_ids: &[&str],
    kind: &str,
) -> AppResult<Vec<VersionRow>> {
    store.with(|conn| {
        let mut latest = Vec::new();
        for device_id in device_ids {
            latest.extend(newest_on(conn, device_id, kind, 1)?);
        }
        Ok(latest)
    })
}

/// How many versions every device has, per kind: `(device_id, kind, count)`.
pub(crate) fn counts(store: &Store) -> AppResult<Vec<(String, String, u64)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT device_id, kind, COUNT(*) FROM configs GROUP BY device_id, kind",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get::<_, i64>(2)? as u64))
        })?;
        rows.collect()
    })
}

/// Note that a version was seen (again) at `seen`, and how large its text is.
pub(crate) fn record(store: &Store, version_id: i64, seen: i64, bytes: u64) -> AppResult<()> {
    store.with(|conn| {
        conn.execute(
            "INSERT INTO config_meta (version_id, last_seen, bytes) VALUES (?1, ?2, ?3)
             ON CONFLICT (version_id) DO UPDATE SET last_seen = MAX(last_seen, excluded.last_seen), bytes = excluded.bytes",
            params![version_id, seen, bytes as i64],
        )
    })?;
    Ok(())
}

pub(crate) fn pin(store: &Store, version_id: i64, reason: &str, at: i64) -> AppResult<()> {
    store.with(|conn| {
        conn.execute(
            "INSERT OR IGNORE INTO config_pins (version_id, reason, pinned_at) VALUES (?1, ?2, ?3)",
            params![version_id, reason, at],
        )
    })?;
    Ok(())
}

/// Remove one pin of a version, or all of them. Returns how many were removed.
pub(crate) fn unpin(store: &Store, version_id: i64, reason: Option<&str>) -> AppResult<usize> {
    store.with(|conn| {
        conn.execute(
            "DELETE FROM config_pins WHERE version_id = ?1 AND (?2 IS NULL OR reason = ?2)",
            params![version_id, reason],
        )
    })
}

/// Ids of the versions of one device and kind that retention may remove:
/// everything older than the newest `keep` that is not pinned.
pub(crate) fn expired(
    store: &Store,
    device_id: &str,
    kind: &str,
    keep: u32,
) -> AppResult<Vec<i64>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT c.id FROM configs c
             WHERE c.device_id = ?1 AND c.kind = ?2
               AND c.id NOT IN (
                   SELECT k.id FROM configs k WHERE k.device_id = ?1 AND k.kind = ?2
                   ORDER BY k.taken_at DESC, k.id DESC LIMIT ?3
               )
               AND NOT EXISTS (SELECT 1 FROM config_pins p WHERE p.version_id = c.id)
             ORDER BY c.id",
        )?;
        let rows = statement.query_map(params![device_id, kind, keep], |row| row.get(0))?;
        rows.collect()
    })
}

/// `(device_id, kind)` of every history longer than `keep`.
pub(crate) fn crowded(store: &Store, keep: u32) -> AppResult<Vec<(String, String)>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT device_id, kind FROM configs GROUP BY device_id, kind HAVING COUNT(*) > ?1",
        )?;
        let rows = statement.query_map(params![keep], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })
}

/// Ids of a device's versions of every kind, split into `(removable, pinned)`.
pub(crate) fn of_device(store: &Store, device_id: &str) -> AppResult<(Vec<i64>, Vec<i64>)> {
    let rows: Vec<(i64, bool)> = store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT c.id, EXISTS (SELECT 1 FROM config_pins p WHERE p.version_id = c.id)
             FROM configs c WHERE c.device_id = ?1 ORDER BY c.id",
        )?;
        let rows = statement.query_map(params![device_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    })?;
    let (pinned, removable): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(_, pinned)| *pinned);
    let ids = |rows: Vec<(i64, bool)>| rows.into_iter().map(|(id, _)| id).collect();
    Ok((ids(removable), ids(pinned)))
}

/// Remove versions and everything recorded about them.
pub(crate) fn delete(store: &Store, ids: &[i64]) -> AppResult<usize> {
    store.transaction(|tx| {
        let mut removed = 0;
        for id in ids {
            removed += tx.execute("DELETE FROM configs WHERE id = ?1", params![id])?;
            tx.execute("DELETE FROM config_meta WHERE version_id = ?1", params![id])?;
            tx.execute("DELETE FROM config_pins WHERE version_id = ?1", params![id])?;
        }
        Ok(removed)
    })
}

/// Drop metadata of versions that are gone (the inventory removes a deleted
/// device's snapshots itself).
pub(crate) fn sweep(store: &Store) -> AppResult<()> {
    store.transaction(|tx| {
        tx.execute(
            "DELETE FROM config_meta WHERE version_id NOT IN (SELECT id FROM configs)",
            [],
        )?;
        tx.execute(
            "DELETE FROM config_pins WHERE version_id NOT IN (SELECT id FROM configs)",
            [],
        )?;
        Ok(())
    })
}

// ---------------------------------------------------------------- compliance

#[derive(Clone, Debug)]
pub(crate) struct RunRow {
    pub seq: i64,
    pub id: String,
    pub at: i64,
    /// JSON of the run's summary.
    pub summary: String,
    pub has_findings: bool,
}

fn run_row(row: &Row<'_>) -> rusqlite::Result<RunRow> {
    Ok(RunRow {
        seq: row.get(0)?,
        id: row.get(1)?,
        at: row.get(2)?,
        summary: row.get(3)?,
        has_findings: row.get(4)?,
    })
}

/// One finding as stored: the columns it can be filtered by, and its JSON.
pub(crate) struct FindingRow {
    pub device_id: String,
    pub policy_id: String,
    pub rule_id: String,
    pub status: &'static str,
    pub severity: &'static str,
    pub body: String,
}

pub(crate) fn insert_run(
    store: &Store,
    id: &str,
    at: i64,
    summary: &str,
    findings: &[FindingRow],
) -> AppResult<()> {
    store.transaction(|tx| {
        tx.execute(
            "INSERT INTO compliance_runs (id, at, summary) VALUES (?1, ?2, ?3)",
            params![id, at, summary],
        )?;
        let run_seq = tx.last_insert_rowid();
        let mut statement = tx.prepare_cached(
            "INSERT INTO compliance_findings (run_seq, seq, device_id, policy_id, rule_id, status, severity, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for (seq, finding) in findings.iter().enumerate() {
            statement.execute(params![
                run_seq,
                seq as i64 + 1,
                finding.device_id,
                finding.policy_id,
                finding.rule_id,
                finding.status,
                finding.severity,
                finding.body
            ])?;
        }
        Ok(())
    })
}

/// A run by id, or the most recent one.
pub(crate) fn run(store: &Store, id: Option<&str>) -> AppResult<Option<RunRow>> {
    store.with(|conn| {
        conn.query_row(
            "SELECT seq, id, at, summary, has_findings FROM compliance_runs
             WHERE (?1 IS NULL OR id = ?1) ORDER BY seq DESC LIMIT 1",
            params![id],
            run_row,
        )
        .optional()
    })
}

/// Runs, newest first, starting below `before_seq`.
pub(crate) fn runs(store: &Store, before_seq: Option<i64>, limit: usize) -> AppResult<Vec<RunRow>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT seq, id, at, summary, has_findings FROM compliance_runs
             WHERE (?1 IS NULL OR seq < ?1) ORDER BY seq DESC LIMIT ?2",
        )?;
        let rows = statement.query_map(params![before_seq, limit as i64], run_row)?;
        rows.collect()
    })
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FindingFilter {
    pub status: Option<String>,
    pub severity: Option<String>,
    pub device_id: Option<String>,
    pub policy_id: Option<String>,
    pub rule_id: Option<String>,
}

/// Findings of a run as `(seq, json)`, in the order they were produced, after `after_seq`.
pub(crate) fn findings(
    store: &Store,
    run_seq: i64,
    filter: &FindingFilter,
    after_seq: i64,
    limit: usize,
) -> AppResult<Vec<(i64, String)>> {
    let mut sql =
        String::from("SELECT seq, body FROM compliance_findings WHERE run_seq = ? AND seq > ?");
    let mut args: Vec<SqlValue> = vec![run_seq.into(), after_seq.into()];
    for (column, value) in [
        ("status", &filter.status),
        ("severity", &filter.severity),
        ("device_id", &filter.device_id),
        ("policy_id", &filter.policy_id),
        ("rule_id", &filter.rule_id),
    ] {
        if let Some(value) = value {
            sql.push_str(&format!(" AND {column} = ?"));
            args.push(value.clone().into());
        }
    }
    sql.push_str(" ORDER BY seq LIMIT ?");
    args.push((limit as i64).into());
    store.with(|conn| {
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(args.iter()), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect()
    })
}

/// Keep the findings of the newest `keep_findings` runs and the summaries of
/// the newest `keep_runs`.
pub(crate) fn trim_runs(store: &Store, keep_findings: u32, keep_runs: u32) -> AppResult<()> {
    store.transaction(|tx| {
        let cutoff = |keep: u32| -> rusqlite::Result<Option<i64>> {
            tx.query_row(
                "SELECT seq FROM compliance_runs ORDER BY seq DESC LIMIT 1 OFFSET ?1",
                params![keep],
                |row| row.get(0),
            )
            .optional()
        };
        if let Some(oldest_without) = cutoff(keep_findings)? {
            tx.execute(
                "DELETE FROM compliance_findings WHERE run_seq <= ?1",
                params![oldest_without],
            )?;
            tx.execute(
                "UPDATE compliance_runs SET has_findings = 0 WHERE seq <= ?1",
                params![oldest_without],
            )?;
        }
        if let Some(oldest_gone) = cutoff(keep_runs)? {
            tx.execute(
                "DELETE FROM compliance_runs WHERE seq <= ?1",
                params![oldest_gone],
            )?;
        }
        Ok(())
    })
}
