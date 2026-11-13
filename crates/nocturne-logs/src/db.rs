//! This feature's own tables, beside the core's `logs`.
//!
//! The core stores a log entry as `(ts, stream, device_id, severity, source,
//! message, body)` and indexes it by stream/time and device/time, which
//! answers "the last 200 lines of this device". A log screen asks more than
//! that: only the `%LINK-3-UPDOWN` lines, only facility `local7`, only the
//! lines about `GigabitEthernet1/0/3`, and the per-severity counts of a
//! histogram. Those live in `logs_syslog`, one row per entry keyed on the
//! core row's id, so the filters are indexed columns rather than
//! `json_extract` over every row in the table.
//!
//! `logs_sources` is the other half of attribution: every address that has
//! ever sent, whether a device in the inventory claims it, and why not when
//! none does. Without it an unattributed line is a dead end — with it the
//! user gets a list of "these addresses are logging at you, this one is not
//! in your inventory".

use nocturne_app::AppResult;
use nocturne_app::store::{LogEntry, Store};
use rusqlite::params;
use serde::{Deserialize, Serialize};

pub const STREAM: &str = "syslog";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS logs_syslog (
    log_id    INTEGER PRIMARY KEY,
    facility  INTEGER,
    app       TEXT,
    mnemonic  TEXT,
    host      TEXT,
    interface TEXT,
    -- Hash of the line as received, so the same line pulled twice from a
    -- device's own buffer is stored once.
    fp        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS logs_syslog_mnemonic ON logs_syslog (mnemonic, log_id);
CREATE INDEX IF NOT EXISTS logs_syslog_facility ON logs_syslog (facility, log_id);
CREATE INDEX IF NOT EXISTS logs_syslog_interface ON logs_syslog (interface, log_id)
    WHERE interface IS NOT NULL;

CREATE TABLE IF NOT EXISTS logs_sources (
    address   TEXT PRIMARY KEY,
    device_id TEXT,
    -- Why this address is not attributed, when it is not.
    reason    TEXT NOT NULL DEFAULT '',
    first_at  INTEGER NOT NULL,
    last_at   INTEGER NOT NULL,
    count     INTEGER NOT NULL DEFAULT 0,
    dropped   INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
";

pub fn init(store: &Store) -> AppResult<()> {
    store.with(|conn| conn.execute_batch(SCHEMA))
}

/// One entry on its way into the store: the core's row plus the parts this
/// feature indexes itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Prepared {
    pub entry: LogEntry,
    pub facility: Option<u8>,
    pub app: Option<String>,
    /// `LINK-3-UPDOWN`, `system,info,account`, … as the vendor writes it.
    pub mnemonic: Option<String>,
    /// The host name the sender put in the message (not the inventory's).
    pub host: Option<String>,
    /// An interface of the attributed device that this line is about.
    pub interface: Option<String>,
    /// Why no device was attributed (empty when one was).
    pub reason: String,
    pub fp: i64,
}

/// A line is identified by its text alone: the same `show logging` buffer
/// read twice must not be stored twice, and the device's own timestamp is
/// part of the text, so two genuinely separate events differ.
pub fn fingerprint(raw: &str) -> i64 {
    // FNV-1a: short, stable across runs and platforms, and good enough to
    // tell log lines apart (a collision costs one skipped duplicate line).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in raw.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash as i64
}

/// What one batch write did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Written {
    pub stored: usize,
}

/// Write a batch of entries, their indexed parts and their sources in one
/// transaction, and return each entry with the id it was given.
///
/// One transaction per batch rather than per line is the whole point: a
/// chattery network sends tens of thousands of lines a minute, and SQLite
/// charges for a commit, not for a row.
pub fn insert_batch(store: &Store, batch: &mut [Prepared]) -> AppResult<Written> {
    if batch.is_empty() {
        return Ok(Written::default());
    }
    let mut ids = Vec::with_capacity(batch.len());
    store.transaction(|tx| {
        {
            let mut entries = tx.prepare_cached(
                "INSERT INTO logs (ts, stream, device_id, severity, source, message, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut meta = tx.prepare_cached(
                "INSERT INTO logs_syslog (log_id, facility, app, mnemonic, host, interface, fp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            let mut source = tx.prepare_cached(
                "INSERT INTO logs_sources (address, device_id, reason, first_at, last_at, count)
                 VALUES (?1, ?2, ?3, ?4, ?4, 1)
                 ON CONFLICT (address) DO UPDATE SET
                     device_id = excluded.device_id,
                     reason    = excluded.reason,
                     last_at   = excluded.last_at,
                     count     = count + 1",
            )?;
            for item in batch.iter() {
                let body = item
                    .entry
                    .body
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|err| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(err) as Box<_>)
                    })?;
                entries.execute(params![
                    item.entry.ts,
                    item.entry.stream,
                    item.entry.device_id,
                    item.entry.severity,
                    item.entry.source,
                    item.entry.message,
                    body,
                ])?;
                let id = tx.last_insert_rowid();
                ids.push(id);
                meta.execute(params![
                    id,
                    item.facility,
                    item.app,
                    item.mnemonic,
                    item.host,
                    item.interface,
                    item.fp,
                ])?;
                if !item.entry.source.is_empty() {
                    source.execute(params![
                        item.entry.source,
                        item.entry.device_id,
                        item.reason,
                        item.entry.ts,
                    ])?;
                }
            }
        }
        Ok(())
    })?;
    for (item, id) in batch.iter_mut().zip(ids) {
        item.entry.id = id;
    }
    Ok(Written {
        stored: batch.len(),
    })
}

/// Count a line that was received but not kept, so that a source whose lines
/// are all dropped is still visible to the user.
pub fn count_dropped(
    store: &Store,
    address: &str,
    at: i64,
    reason: &str,
    count: u64,
) -> AppResult<()> {
    store.with(|conn| {
        conn.execute(
            "INSERT INTO logs_sources (address, device_id, reason, first_at, last_at, count, dropped)
             VALUES (?1, NULL, ?2, ?3, ?3, 0, ?4)
             ON CONFLICT (address) DO UPDATE SET
                 reason  = excluded.reason,
                 last_at = excluded.last_at,
                 dropped = dropped + excluded.dropped",
            params![address, reason, at, count as i64],
        )
    })?;
    Ok(())
}

/// One sending address, as `logs.sources` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRow {
    pub address: String,
    pub device_id: Option<String>,
    /// Empty when the address is attributed.
    pub reason: String,
    pub first_at: i64,
    pub last_at: i64,
    pub count: i64,
    /// Lines received from this address and deliberately not kept.
    pub dropped: i64,
}

pub fn sources(store: &Store) -> AppResult<Vec<SourceRow>> {
    store.with(|conn| {
        let mut statement = conn.prepare(
            "SELECT address, device_id, reason, first_at, last_at, count, dropped
             FROM logs_sources ORDER BY last_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(SourceRow {
                address: row.get(0)?,
                device_id: row.get(1)?,
                reason: row.get(2)?,
                first_at: row.get(3)?,
                last_at: row.get(4)?,
                count: row.get(5)?,
                dropped: row.get(6)?,
            })
        })?;
        rows.collect()
    })
}

/// Re-point the stored sources at the devices that now claim them (the
/// inventory changed), so `logs.sources` does not keep saying "unknown" for
/// an address the user has just added.
pub fn reattribute_sources(
    store: &Store,
    resolved: &[(String, Option<String>, String)],
) -> AppResult<()> {
    if resolved.is_empty() {
        return Ok(());
    }
    store.transaction(|tx| {
        let mut update = tx.prepare_cached(
            "UPDATE logs_sources SET device_id = ?2, reason = ?3 WHERE address = ?1",
        )?;
        for (address, device_id, reason) in resolved {
            update.execute(params![address, device_id, reason])?;
        }
        Ok(())
    })
}

/// How many rows of this feature's stream are stored, and the oldest one's time.
pub fn extent(store: &Store) -> AppResult<(i64, Option<i64>)> {
    store.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*), MIN(ts) FROM logs WHERE stream = ?1",
            params![STREAM],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
    })
}

/// What one retention pass removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pruned {
    pub by_age: usize,
    pub by_rows: usize,
}

/// Delete entries older than `before`, then the oldest of whatever is left
/// over `max_rows`, in batches so that a first run against a huge table does
/// not hold the database lock for a minute.
pub fn prune(store: &Store, before: Option<i64>, max_rows: u64, batch: usize) -> AppResult<Pruned> {
    let mut pruned = Pruned::default();
    if let Some(before) = before {
        loop {
            let removed = delete_oldest(store, Some(before), batch)?;
            pruned.by_age += removed;
            if removed < batch {
                break;
            }
        }
    }
    loop {
        let (count, _) = extent(store)?;
        let excess = (count as u64).saturating_sub(max_rows);
        if excess == 0 {
            break;
        }
        let removed = delete_oldest(store, None, batch.min(excess as usize))?;
        pruned.by_rows += removed;
        if removed == 0 {
            break;
        }
    }
    Ok(pruned)
}

/// Delete at most `limit` of the oldest entries (optionally only those before
/// a time), and the rows this feature keeps beside them.
fn delete_oldest(store: &Store, before: Option<i64>, limit: usize) -> AppResult<usize> {
    if limit == 0 {
        return Ok(0);
    }
    store.transaction(|tx| {
        let ids: Vec<i64> = {
            let mut statement = tx.prepare_cached(
                "SELECT id FROM logs WHERE stream = ?1 AND (?2 IS NULL OR ts < ?2)
                 ORDER BY id LIMIT ?3",
            )?;
            let rows =
                statement.query_map(params![STREAM, before, limit as i64], |row| row.get(0))?;
            rows.collect::<rusqlite::Result<Vec<i64>>>()?
        };
        if ids.is_empty() {
            return Ok(0);
        }
        // Deleted by their exact ids: a pulled entry can carry an older
        // timestamp than the rows written around it, so "everything between
        // the first and the last id" is not the same set as the one chosen.
        let places = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let removed = tx.execute(
            &format!("DELETE FROM logs WHERE id IN ({places})"),
            rusqlite::params_from_iter(ids.iter()),
        )?;
        tx.execute(
            &format!("DELETE FROM logs_syslog WHERE log_id IN ({places})"),
            rusqlite::params_from_iter(ids.iter()),
        )?;
        Ok(removed)
    })
}

/// Fingerprints of the lines already stored for a device, newest first: what
/// a pull from the device's own buffer must not store a second time.
pub fn fingerprints_of(store: &Store, device_id: &str, limit: usize) -> AppResult<Vec<i64>> {
    store.with(|conn| {
        let mut statement = conn.prepare(
            "SELECT s.fp FROM logs l JOIN logs_syslog s ON s.log_id = l.id
             WHERE l.device_id = ?1 AND l.stream = ?2
             ORDER BY l.id DESC LIMIT ?3",
        )?;
        let rows =
            statement.query_map(params![device_id, STREAM, limit as i64], |row| row.get(0))?;
        rows.collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::store::LogQuery;

    fn prepared(message: &str, ts: i64, source: &str) -> Prepared {
        Prepared {
            entry: LogEntry {
                id: 0,
                ts,
                stream: STREAM.into(),
                device_id: None,
                severity: Some(5),
                source: source.into(),
                message: message.into(),
                body: Some(serde_json::json!({"raw": message})),
            },
            facility: Some(23),
            app: None,
            mnemonic: Some("LINK-3-UPDOWN".into()),
            host: Some("SW1".into()),
            interface: None,
            reason: "no device in the inventory has this address".into(),
            fp: fingerprint(message),
        }
    }

    #[test]
    fn the_schema_is_idempotent() {
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        init(&store).unwrap();
        let tables: i64 = store
            .with(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'logs_%'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(tables, 2);
    }

    #[test]
    fn a_batch_is_written_with_its_index_rows_and_its_sources() {
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        let mut batch = vec![
            prepared("first", 1_000, "10.0.0.1"),
            prepared("second", 1_001, "10.0.0.1"),
        ];
        assert_eq!(insert_batch(&store, &mut batch).unwrap().stored, 2);
        assert!(batch[0].entry.id > 0 && batch[1].entry.id > batch[0].entry.id);

        let listed = sources(&store).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!((listed[0].count, listed[0].dropped), (2, 0));
        assert!(listed[0].device_id.is_none());
        assert!(listed[0].reason.contains("inventory"));

        count_dropped(&store, "10.0.0.9", 1_002, "unknown source", 3).unwrap();
        let after = sources(&store).unwrap();
        let dropped = after
            .iter()
            .find(|row| row.address == "10.0.0.9")
            .expect("the address that was dropped is still listed");
        assert_eq!((dropped.count, dropped.dropped), (0, 3));
    }

    #[test]
    fn retention_removes_the_oldest_by_age_then_by_row_count() {
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        let mut batch: Vec<Prepared> = (0..50)
            .map(|index| prepared(&format!("line {index}"), 1_000 + index, "10.0.0.1"))
            .collect();
        insert_batch(&store, &mut batch).unwrap();

        let pruned = prune(&store, Some(1_010), 1_000, 7).unwrap();
        assert_eq!((pruned.by_age, pruned.by_rows), (10, 0));
        assert_eq!(extent(&store).unwrap().0, 40);

        let pruned = prune(&store, None, 25, 7).unwrap();
        assert_eq!((pruned.by_age, pruned.by_rows), (0, 15));
        let left = store
            .log_query(&LogQuery {
                stream: Some(STREAM.into()),
                limit: Some(100),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(left.len(), 25);
        assert_eq!(left.last().unwrap().message, "line 25", "the oldest went");

        let orphans: i64 = store
            .with(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM logs_syslog WHERE log_id NOT IN (SELECT id FROM logs)",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(orphans, 0, "the index rows go with the entries");
    }

    #[test]
    fn fingerprints_tell_lines_apart_and_are_listed_per_device() {
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        assert_ne!(fingerprint("a"), fingerprint("b"));
        assert_eq!(fingerprint("same line"), fingerprint("same line"));

        let mut batch = vec![prepared("one", 10, "10.0.0.1")];
        batch[0].entry.device_id = Some("dev1".into());
        insert_batch(&store, &mut batch).unwrap();
        assert_eq!(
            fingerprints_of(&store, "dev1", 10).unwrap(),
            vec![fingerprint("one")]
        );
        assert!(fingerprints_of(&store, "dev2", 10).unwrap().is_empty());
    }
}
