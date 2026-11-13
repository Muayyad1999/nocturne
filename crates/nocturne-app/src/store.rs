//! Persistence: one SQLite file.
//!
//! Most of what Nocturne keeps — devices, credentials' metadata, changes,
//! playbooks, labs, conversations — is a few thousand small records at most,
//! so it lives in a single **document table** (`collection`, `id`, JSON body).
//! That keeps every feature module free of SQL and makes schema evolution a
//! matter of `#[serde(default)]`. Three things are too voluminous for that and
//! get purpose-built tables: metric **samples** (with roll-ups), the **log**
//! stream (syslog, traps, audit trail, events) and **config** snapshots
//! (compressed, de-duplicated by content hash). Secrets are stored encrypted
//! in their own table and only the [`Vault`](crate::vault::Vault) reads them.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::util::now_ms;

const SCHEMA: &[&str] = &[
    // v1
    "
    CREATE TABLE docs (
        collection TEXT NOT NULL,
        id         TEXT NOT NULL,
        body       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (collection, id)
    ) WITHOUT ROWID;

    CREATE TABLE kv (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE secrets (
        id         TEXT PRIMARY KEY,
        nonce      BLOB NOT NULL,
        ciphertext BLOB NOT NULL,
        updated_at INTEGER NOT NULL
    ) WITHOUT ROWID;

    CREATE TABLE series (
        id        INTEGER PRIMARY KEY,
        device_id TEXT NOT NULL,
        metric    TEXT NOT NULL,
        instance  TEXT NOT NULL DEFAULT '',
        UNIQUE (device_id, metric, instance)
    );

    CREATE TABLE samples (
        series_id INTEGER NOT NULL,
        ts        INTEGER NOT NULL,
        value     REAL NOT NULL,
        PRIMARY KEY (series_id, ts)
    ) WITHOUT ROWID;

    CREATE TABLE rollups (
        series_id INTEGER NOT NULL,
        res       INTEGER NOT NULL,
        bucket    INTEGER NOT NULL,
        min       REAL NOT NULL,
        max       REAL NOT NULL,
        avg       REAL NOT NULL,
        count     INTEGER NOT NULL,
        PRIMARY KEY (series_id, res, bucket)
    ) WITHOUT ROWID;

    CREATE TABLE logs (
        id        INTEGER PRIMARY KEY AUTOINCREMENT,
        ts        INTEGER NOT NULL,
        stream    TEXT NOT NULL,
        device_id TEXT,
        severity  INTEGER,
        source    TEXT NOT NULL DEFAULT '',
        message   TEXT NOT NULL,
        body      TEXT
    );
    CREATE INDEX logs_stream_ts ON logs (stream, ts);
    CREATE INDEX logs_device_ts ON logs (device_id, ts);

    CREATE TABLE configs (
        id        INTEGER PRIMARY KEY AUTOINCREMENT,
        device_id TEXT NOT NULL,
        kind      TEXT NOT NULL,
        sha256    TEXT NOT NULL,
        content   BLOB NOT NULL,
        lines     INTEGER NOT NULL,
        taken_at  INTEGER NOT NULL,
        source    TEXT NOT NULL,
        note      TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX configs_device ON configs (device_id, kind, taken_at);
    ",
];

/// Raw samples older than this are served from 5-minute roll-ups…
const RAW_RETENTION_MS: i64 = 48 * 3_600_000;
/// …and 5-minute roll-ups older than this from hourly ones.
const FINE_RETENTION_MS: i64 = 30 * 86_400_000;
const COARSE_RETENTION_MS: i64 = 400 * 86_400_000;
const FINE_RES_S: i64 = 300;
const COARSE_RES_S: i64 = 3_600;

pub struct Store {
    conn: Mutex<Connection>,
    series_cache: Mutex<HashMap<(String, String, String), i64>>,
}

/// One entry of the log stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    #[serde(default)]
    pub id: i64,
    pub ts: i64,
    /// `syslog`, `trap`, `audit`, `event`, …
    pub stream: String,
    #[serde(default)]
    pub device_id: Option<String>,
    /// Syslog-style severity, 0 (emergency) … 7 (debug).
    #[serde(default)]
    pub severity: Option<u8>,
    #[serde(default)]
    pub source: String,
    pub message: String,
    /// Structured payload, stream-specific.
    #[serde(default)]
    pub body: Option<Value>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LogQuery {
    pub stream: Option<String>,
    pub device_id: Option<String>,
    /// Keep entries at least this severe (numerically ≤).
    pub max_severity: Option<u8>,
    /// Case-insensitive substring of the message or source.
    pub text: Option<String>,
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Page backwards: only entries with a smaller id.
    pub before_id: Option<i64>,
    pub limit: Option<u32>,
}

/// A stored configuration snapshot (without its content).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigVersion {
    pub id: i64,
    pub device_id: String,
    /// `running` or `startup`.
    pub kind: String,
    pub sha256: String,
    pub lines: u32,
    pub taken_at: i64,
    /// `manual`, `scheduled`, `pre-change`, `post-change`, `import`.
    pub source: String,
    pub note: String,
}

/// One point of a metric query: a bucket with min/avg/max (equal for raw samples).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub ts: i64,
    pub min: f64,
    pub avg: f64,
    pub max: f64,
}

impl Store {
    pub fn open(path: &Path) -> AppResult<Store> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Store::init(conn)
    }

    pub fn in_memory() -> AppResult<Store> {
        Store::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> AppResult<Store> {
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version as usize > SCHEMA.len() {
            return Err(AppError::conflict(format!(
                "this data was written by a newer version of Nocturne (schema {version}); upgrade the application"
            )));
        }
        for (index, migration) in SCHEMA.iter().enumerate().skip(version as usize) {
            conn.execute_batch(&format!(
                "BEGIN;\n{migration}\nPRAGMA user_version = {};\nCOMMIT;",
                index + 1
            ))?;
        }
        Ok(Store {
            conn: Mutex::new(conn),
            series_cache: Mutex::new(HashMap::new()),
        })
    }

    /// Run a closure with the connection. Keep it short: every caller waits.
    pub fn with<T>(&self, work: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> AppResult<T> {
        Ok(work(&self.conn.lock())?)
    }

    /// Run a closure inside a transaction; an error rolls everything back.
    pub fn transaction<T>(
        &self,
        work: impl FnOnce(&rusqlite::Transaction<'_>) -> rusqlite::Result<T>,
    ) -> AppResult<T> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let value = work(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    // ------------------------------------------------------------ documents

    /// Insert or replace a document.
    pub fn put<T: Serialize>(&self, collection: &str, id: &str, value: &T) -> AppResult<()> {
        let body = serde_json::to_string(value)?;
        let now = now_ms();
        self.with(|conn| {
            conn.execute(
                "INSERT INTO docs (collection, id, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT (collection, id) DO UPDATE SET body = excluded.body, updated_at = excluded.updated_at",
                params![collection, id, body, now],
            )
        })?;
        Ok(())
    }

    pub fn get<T: DeserializeOwned>(&self, collection: &str, id: &str) -> AppResult<Option<T>> {
        let body: Option<String> = self.with(|conn| {
            conn.query_row(
                "SELECT body FROM docs WHERE collection = ?1 AND id = ?2",
                params![collection, id],
                |row| row.get(0),
            )
            .optional()
        })?;
        body.map(|body| decode(collection, id, &body)).transpose()
    }

    /// Like [`get`](Self::get), but a missing document is a `not_found` error naming `what`.
    pub fn require<T: DeserializeOwned>(
        &self,
        collection: &str,
        id: &str,
        what: &str,
    ) -> AppResult<T> {
        self.get(collection, id)?
            .ok_or_else(|| AppError::not_found(format!("{what} `{id}`")))
    }

    /// Every document of a collection, oldest first.
    pub fn list<T: DeserializeOwned>(&self, collection: &str) -> AppResult<Vec<T>> {
        let rows: Vec<(String, String)> = self.with(|conn| {
            let mut statement = conn.prepare_cached(
                "SELECT id, body FROM docs WHERE collection = ?1 ORDER BY created_at, id",
            )?;
            let rows =
                statement.query_map(params![collection], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })?;
        rows.iter()
            .map(|(id, body)| decode(collection, id, body))
            .collect()
    }

    pub fn delete(&self, collection: &str, id: &str) -> AppResult<bool> {
        let removed = self.with(|conn| {
            conn.execute(
                "DELETE FROM docs WHERE collection = ?1 AND id = ?2",
                params![collection, id],
            )
        })?;
        Ok(removed > 0)
    }

    pub fn count(&self, collection: &str) -> AppResult<u64> {
        let count: i64 = self.with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM docs WHERE collection = ?1",
                params![collection],
                |row| row.get(0),
            )
        })?;
        Ok(count.max(0) as u64)
    }

    // ------------------------------------------------------------- settings

    pub fn kv_get<T: DeserializeOwned>(&self, key: &str) -> AppResult<Option<T>> {
        let value: Option<String> = self.with(|conn| {
            conn.query_row("SELECT value FROM kv WHERE key = ?1", params![key], |row| {
                row.get(0)
            })
            .optional()
        })?;
        value.map(|value| decode("kv", key, &value)).transpose()
    }

    pub fn kv_set<T: Serialize>(&self, key: &str, value: &T) -> AppResult<()> {
        let value = serde_json::to_string(value)?;
        self.with(|conn| {
            conn.execute(
                "INSERT INTO kv (key, value) VALUES (?1, ?2) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )
        })?;
        Ok(())
    }

    pub fn kv_delete(&self, key: &str) -> AppResult<()> {
        self.with(|conn| conn.execute("DELETE FROM kv WHERE key = ?1", params![key]))?;
        Ok(())
    }

    // -------------------------------------------------------------- secrets

    pub(crate) fn secret_put(&self, id: &str, nonce: &[u8], ciphertext: &[u8]) -> AppResult<()> {
        self.with(|conn| {
            conn.execute(
                "INSERT INTO secrets (id, nonce, ciphertext, updated_at) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (id) DO UPDATE SET nonce = excluded.nonce, ciphertext = excluded.ciphertext, updated_at = excluded.updated_at",
                params![id, nonce, ciphertext, now_ms()],
            )
        })?;
        Ok(())
    }

    pub(crate) fn secret_get(&self, id: &str) -> AppResult<Option<(Vec<u8>, Vec<u8>)>> {
        self.with(|conn| {
            conn.query_row(
                "SELECT nonce, ciphertext FROM secrets WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        })
    }

    pub(crate) fn secret_delete(&self, id: &str) -> AppResult<()> {
        self.with(|conn| conn.execute("DELETE FROM secrets WHERE id = ?1", params![id]))?;
        Ok(())
    }

    /// Ids of stored secrets that start with `prefix`.
    pub(crate) fn secret_ids(&self, prefix: &str) -> AppResult<Vec<String>> {
        self.with(|conn| {
            let mut statement = conn
                .prepare_cached("SELECT id FROM secrets WHERE id >= ?1 AND id < ?2 ORDER BY id")?;
            // The smallest string greater than every string with this prefix.
            let upper = format!("{prefix}\u{10FFFF}");
            let rows = statement.query_map(params![prefix, upper], |row| row.get(0))?;
            rows.collect()
        })
    }

    // ----------------------------------------------------------------- logs

    pub fn log_append(&self, entry: &LogEntry) -> AppResult<i64> {
        let body = entry.body.as_ref().map(serde_json::to_string).transpose()?;
        self.with(|conn| {
            conn.execute(
                "INSERT INTO logs (ts, stream, device_id, severity, source, message, body) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![entry.ts, entry.stream, entry.device_id, entry.severity, entry.source, entry.message, body],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// Newest first.
    pub fn log_query(&self, query: &LogQuery) -> AppResult<Vec<LogEntry>> {
        let mut sql = String::from(
            "SELECT id, ts, stream, device_id, severity, source, message, body FROM logs WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let mut bind = |sql: &mut String, clause: &str, value: rusqlite::types::Value| {
            sql.push_str(clause);
            args.push(value);
        };
        if let Some(stream) = &query.stream {
            bind(&mut sql, " AND stream = ?", stream.clone().into());
        }
        if let Some(device_id) = &query.device_id {
            bind(&mut sql, " AND device_id = ?", device_id.clone().into());
        }
        if let Some(severity) = query.max_severity {
            bind(&mut sql, " AND severity <= ?", i64::from(severity).into());
        }
        if let Some(since) = query.since {
            bind(&mut sql, " AND ts >= ?", since.into());
        }
        if let Some(until) = query.until {
            bind(&mut sql, " AND ts <= ?", until.into());
        }
        if let Some(before) = query.before_id {
            bind(&mut sql, " AND id < ?", before.into());
        }
        if let Some(text) = query.text.as_deref().filter(|text| !text.trim().is_empty()) {
            let pattern = format!(
                "%{}%",
                text.trim()
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            sql.push_str(" AND (message LIKE ? ESCAPE '\\' OR source LIKE ? ESCAPE '\\')");
            args.push(pattern.clone().into());
            args.push(pattern.into());
        }
        sql.push_str(" ORDER BY id DESC LIMIT ?");
        args.push(i64::from(query.limit.unwrap_or(200).clamp(1, 5000)).into());

        self.with(|conn| {
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(rusqlite::params_from_iter(args.iter()), |row| {
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
            })?;
            rows.collect()
        })
    }

    /// Delete entries of `stream` older than `before`; returns how many.
    pub fn log_prune(&self, stream: &str, before: i64) -> AppResult<usize> {
        self.with(|conn| {
            conn.execute(
                "DELETE FROM logs WHERE stream = ?1 AND ts < ?2",
                params![stream, before],
            )
        })
    }

    // -------------------------------------------------------------- metrics

    /// The id of a series, creating it on first use.
    pub fn series_id(&self, device_id: &str, metric: &str, instance: &str) -> AppResult<i64> {
        let key = (
            device_id.to_string(),
            metric.to_string(),
            instance.to_string(),
        );
        if let Some(id) = self.series_cache.lock().get(&key) {
            return Ok(*id);
        }
        let id = self.with(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO series (device_id, metric, instance) VALUES (?1, ?2, ?3)",
                params![device_id, metric, instance],
            )?;
            conn.query_row(
                "SELECT id FROM series WHERE device_id = ?1 AND metric = ?2 AND instance = ?3",
                params![device_id, metric, instance],
                |row| row.get(0),
            )
        })?;
        self.series_cache.lock().insert(key, id);
        Ok(id)
    }

    /// `(metric, instance)` pairs recorded for a device.
    pub fn series_of(&self, device_id: &str) -> AppResult<Vec<(String, String)>> {
        self.with(|conn| {
            let mut statement = conn.prepare_cached("SELECT metric, instance FROM series WHERE device_id = ?1 ORDER BY metric, instance")?;
            let rows = statement.query_map(params![device_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
    }

    /// Store a batch of `(series, timestamp, value)` samples in one transaction.
    pub fn samples_insert(&self, samples: &[(i64, i64, f64)]) -> AppResult<()> {
        if samples.is_empty() {
            return Ok(());
        }
        self.transaction(|tx| {
            let mut statement = tx.prepare_cached(
                "INSERT OR REPLACE INTO samples (series_id, ts, value) VALUES (?1, ?2, ?3)",
            )?;
            for (series, ts, value) in samples {
                if value.is_finite() {
                    statement.execute(params![series, ts, value])?;
                }
            }
            Ok(())
        })
    }

    /// Points of one series between `since` and `until`, at the finest
    /// resolution that still exists for that range and yields at most
    /// `max_points` (raw samples are bucketed when there are more).
    pub fn samples_query(
        &self,
        series_id: i64,
        since: i64,
        until: i64,
        max_points: usize,
    ) -> AppResult<Vec<Point>> {
        let max_points = max_points.clamp(2, 5_000) as i64;
        let span = (until - since).max(1);
        let now = now_ms();
        let raw_available = since >= now - RAW_RETENTION_MS;
        let fine_available = since >= now - FINE_RETENTION_MS;
        // Bucket width needed to stay under max_points.
        let wanted_ms = (span / max_points).max(1);

        self.with(|conn| {
            let (sql, bucket_ms): (&str, i64) = if raw_available && wanted_ms < FINE_RES_S * 1000 {
                (
                    "SELECT (ts / ?4) * ?4 AS b, MIN(value), AVG(value), MAX(value) FROM samples
                     WHERE series_id = ?1 AND ts >= ?2 AND ts <= ?3 GROUP BY b ORDER BY b",
                    wanted_ms,
                )
            } else {
                let res = if fine_available && wanted_ms < COARSE_RES_S * 1000 { FINE_RES_S } else { COARSE_RES_S };
                let width = (wanted_ms / (res * 1000)).max(1) * res * 1000;
                (
                    if res == FINE_RES_S {
                        "SELECT (bucket / ?4) * ?4 AS b, MIN(min), SUM(avg * count) / SUM(count), MAX(max) FROM rollups
                         WHERE series_id = ?1 AND res = 300 AND bucket >= ?2 AND bucket <= ?3 GROUP BY b ORDER BY b"
                    } else {
                        "SELECT (bucket / ?4) * ?4 AS b, MIN(min), SUM(avg * count) / SUM(count), MAX(max) FROM rollups
                         WHERE series_id = ?1 AND res = 3600 AND bucket >= ?2 AND bucket <= ?3 GROUP BY b ORDER BY b"
                    },
                    width,
                )
            };
            let mut statement = conn.prepare_cached(sql)?;
            let rows = statement.query_map(params![series_id, since, until, bucket_ms], |row| {
                Ok(Point { ts: row.get(0)?, min: row.get(1)?, avg: row.get(2)?, max: row.get(3)? })
            })?;
            rows.collect()
        })
    }

    /// The most recent sample of a series.
    pub fn sample_latest(&self, series_id: i64) -> AppResult<Option<(i64, f64)>> {
        self.with(|conn| {
            conn.query_row(
                "SELECT ts, value FROM samples WHERE series_id = ?1 ORDER BY ts DESC LIMIT 1",
                params![series_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
        })
    }

    /// Fold raw samples into 5-minute and hourly roll-ups and drop what has
    /// aged out. Idempotent; meant to run every few minutes.
    pub fn samples_maintain(&self, now: i64) -> AppResult<()> {
        self.transaction(|tx| {
            // Complete 5-minute buckets from raw samples (recomputing recent ones is harmless).
            tx.execute(
                "INSERT OR REPLACE INTO rollups (series_id, res, bucket, min, max, avg, count)
                 SELECT series_id, 300, (ts / 300000) * 300000, MIN(value), MAX(value), AVG(value), COUNT(*)
                 FROM samples WHERE ts >= ?1 AND ts < ?2 GROUP BY series_id, ts / 300000",
                params![now - 6 * 3_600_000, (now / 300_000) * 300_000],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO rollups (series_id, res, bucket, min, max, avg, count)
                 SELECT series_id, 3600, (bucket / 3600000) * 3600000, MIN(min), MAX(max), SUM(avg * count) / SUM(count), SUM(count)
                 FROM rollups WHERE res = 300 AND bucket >= ?1 AND bucket < ?2 GROUP BY series_id, bucket / 3600000",
                params![now - 3 * 86_400_000, (now / 3_600_000) * 3_600_000],
            )?;
            tx.execute("DELETE FROM samples WHERE ts < ?1", params![now - RAW_RETENTION_MS])?;
            tx.execute("DELETE FROM rollups WHERE res = 300 AND bucket < ?1", params![now - FINE_RETENTION_MS])?;
            tx.execute("DELETE FROM rollups WHERE res = 3600 AND bucket < ?1", params![now - COARSE_RETENTION_MS])?;
            Ok(())
        })
    }

    /// Remove every series, sample and roll-up of a device.
    pub fn samples_forget_device(&self, device_id: &str) -> AppResult<()> {
        self.transaction(|tx| {
            tx.execute("DELETE FROM samples WHERE series_id IN (SELECT id FROM series WHERE device_id = ?1)", params![device_id])?;
            tx.execute("DELETE FROM rollups WHERE series_id IN (SELECT id FROM series WHERE device_id = ?1)", params![device_id])?;
            tx.execute("DELETE FROM series WHERE device_id = ?1", params![device_id])?;
            Ok(())
        })?;
        self.series_cache
            .lock()
            .retain(|(device, _, _), _| device != device_id);
        Ok(())
    }

    // -------------------------------------------------------------- configs

    /// Store a configuration snapshot. If the content is identical to the
    /// device's latest snapshot of that kind, nothing new is stored and the
    /// existing version is returned with `false`.
    pub fn config_insert(
        &self,
        device_id: &str,
        kind: &str,
        content: &str,
        source: &str,
        note: &str,
    ) -> AppResult<(ConfigVersion, bool)> {
        let sha256 = hex::encode(Sha256::digest(content.as_bytes()));
        if let Some(latest) = self.config_latest(device_id, kind)?
            && latest.sha256 == sha256
        {
            return Ok((latest, false));
        }
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(content.as_bytes())?;
        let compressed = encoder.finish()?;
        let lines = content.lines().count() as u32;
        let taken_at = now_ms();
        let id = self.with(|conn| {
            conn.execute(
                "INSERT INTO configs (device_id, kind, sha256, content, lines, taken_at, source, note) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![device_id, kind, sha256, compressed, lines, taken_at, source, note],
            )?;
            Ok(conn.last_insert_rowid())
        })?;
        let version = ConfigVersion {
            id,
            device_id: device_id.to_string(),
            kind: kind.to_string(),
            sha256,
            lines,
            taken_at,
            source: source.to_string(),
            note: note.to_string(),
        };
        Ok((version, true))
    }

    /// Snapshots of a device, newest first. `kind` of `None` lists every kind.
    pub fn config_versions(
        &self,
        device_id: &str,
        kind: Option<&str>,
    ) -> AppResult<Vec<ConfigVersion>> {
        self.with(|conn| {
            let mut statement = conn.prepare_cached(
                "SELECT id, device_id, kind, sha256, lines, taken_at, source, note FROM configs
                 WHERE device_id = ?1 AND (?2 IS NULL OR kind = ?2) ORDER BY taken_at DESC, id DESC",
            )?;
            let rows = statement.query_map(params![device_id, kind], version_from_row)?;
            rows.collect()
        })
    }

    pub fn config_latest(&self, device_id: &str, kind: &str) -> AppResult<Option<ConfigVersion>> {
        self.with(|conn| {
            conn.query_row(
                "SELECT id, device_id, kind, sha256, lines, taken_at, source, note FROM configs
                 WHERE device_id = ?1 AND kind = ?2 ORDER BY taken_at DESC, id DESC LIMIT 1",
                params![device_id, kind],
                version_from_row,
            )
            .optional()
        })
    }

    /// A snapshot and its text.
    pub fn config_get(&self, id: i64) -> AppResult<(ConfigVersion, String)> {
        let found: Option<(ConfigVersion, Vec<u8>)> = self.with(|conn| {
            conn.query_row(
                "SELECT id, device_id, kind, sha256, lines, taken_at, source, note, content FROM configs WHERE id = ?1",
                params![id],
                |row| Ok((version_from_row(row)?, row.get(8)?)),
            )
            .optional()
        })?;
        let (version, compressed) =
            found.ok_or_else(|| AppError::not_found(format!("configuration snapshot {id}")))?;
        let mut text = String::new();
        flate2::read::ZlibDecoder::new(compressed.as_slice())
            .read_to_string(&mut text)
            .map_err(|err| {
                AppError::internal(format!("configuration snapshot {id} is corrupt: {err}"))
            })?;
        Ok((version, text))
    }

    pub fn config_delete_device(&self, device_id: &str) -> AppResult<()> {
        self.with(|conn| {
            conn.execute(
                "DELETE FROM configs WHERE device_id = ?1",
                params![device_id],
            )
        })?;
        Ok(())
    }
}

fn version_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConfigVersion> {
    Ok(ConfigVersion {
        id: row.get(0)?,
        device_id: row.get(1)?,
        kind: row.get(2)?,
        sha256: row.get(3)?,
        lines: row.get(4)?,
        taken_at: row.get(5)?,
        source: row.get(6)?,
        note: row.get(7)?,
    })
}

fn decode<T: DeserializeOwned>(collection: &str, id: &str, body: &str) -> AppResult<T> {
    serde_json::from_str(body).map_err(|err| {
        AppError::internal(format!(
            "stored record {collection}/{id} cannot be read: {err}"
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Thing {
        name: String,
        #[serde(default)]
        added_later: u32,
    }

    #[test]
    fn documents_round_trip_and_tolerate_schema_growth() {
        let store = Store::in_memory().unwrap();
        store.put("things", "a", &json!({"name": "first"})).unwrap();
        store
            .put(
                "things",
                "b",
                &Thing {
                    name: "second".into(),
                    added_later: 7,
                },
            )
            .unwrap();
        assert_eq!(
            store.get::<Thing>("things", "a").unwrap(),
            Some(Thing {
                name: "first".into(),
                added_later: 0
            })
        );
        assert_eq!(store.list::<Thing>("things").unwrap().len(), 2);
        assert_eq!(store.count("things").unwrap(), 2);
        store
            .put(
                "things",
                "a",
                &Thing {
                    name: "renamed".into(),
                    added_later: 1,
                },
            )
            .unwrap();
        assert_eq!(
            store.require::<Thing>("things", "a", "thing").unwrap().name,
            "renamed"
        );
        assert!(store.delete("things", "a").unwrap());
        assert!(!store.delete("things", "a").unwrap());
        assert_eq!(
            store
                .require::<Thing>("things", "a", "thing")
                .unwrap_err()
                .code,
            "not_found"
        );
        assert!(
            store.get::<Thing>("other", "b").unwrap().is_none(),
            "collections are separate"
        );
    }

    #[test]
    fn settings_and_reopen_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/nocturne.db");
        {
            let store = Store::open(&path).unwrap();
            store.kv_set("theme", &"night").unwrap();
            store
                .put(
                    "things",
                    "x",
                    &Thing {
                        name: "kept".into(),
                        added_later: 0,
                    },
                )
                .unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.kv_get::<String>("theme").unwrap().as_deref(),
            Some("night")
        );
        assert_eq!(store.count("things").unwrap(), 1);
        store.kv_delete("theme").unwrap();
        assert_eq!(store.kv_get::<String>("theme").unwrap(), None);
    }

    #[test]
    fn logs_filter_and_page() {
        let store = Store::in_memory().unwrap();
        for (index, (stream, severity, message)) in [
            (
                "syslog",
                3,
                "%LINK-3-UPDOWN: Interface Gi0/1, changed state to down",
            ),
            ("syslog", 6, "Configured from console"),
            ("audit", 5, "device created"),
        ]
        .into_iter()
        .enumerate()
        {
            store
                .log_append(&LogEntry {
                    id: 0,
                    ts: 1_000 + index as i64,
                    stream: stream.into(),
                    device_id: Some("dev1".into()),
                    severity: Some(severity),
                    source: "10.0.0.1".into(),
                    message: message.into(),
                    body: Some(json!({"n": index})),
                })
                .unwrap();
        }
        let all = store.log_query(&LogQuery::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].message, "device created", "newest first");
        assert_eq!(all[0].body, Some(json!({"n": 2})));

        let syslog = LogQuery {
            stream: Some("syslog".into()),
            ..LogQuery::default()
        };
        assert_eq!(store.log_query(&syslog).unwrap().len(), 2);
        let severe = LogQuery {
            max_severity: Some(3),
            ..LogQuery::default()
        };
        assert_eq!(store.log_query(&severe).unwrap().len(), 1);
        let text = LogQuery {
            text: Some("updown".into()),
            ..LogQuery::default()
        };
        assert_eq!(store.log_query(&text).unwrap().len(), 1);
        let literal_percent = LogQuery {
            text: Some("%".into()),
            ..LogQuery::default()
        };
        assert_eq!(
            store.log_query(&literal_percent).unwrap().len(),
            1,
            "% is matched literally"
        );
        let page = LogQuery {
            before_id: Some(all[0].id),
            limit: Some(1),
            ..LogQuery::default()
        };
        assert_eq!(
            store.log_query(&page).unwrap()[0].message,
            "Configured from console"
        );
        assert_eq!(store.log_prune("syslog", 1_001).unwrap(), 1);
    }

    #[test]
    fn samples_are_bucketed_rolled_up_and_pruned() {
        let store = Store::in_memory().unwrap();
        let series = store.series_id("dev1", "if.in_bps", "Gi0/1").unwrap();
        assert_eq!(
            store.series_id("dev1", "if.in_bps", "Gi0/1").unwrap(),
            series
        );
        assert_ne!(
            store.series_id("dev1", "if.in_bps", "Gi0/2").unwrap(),
            series
        );

        let now = now_ms();
        let start = now - 3_600_000;
        let samples: Vec<(i64, i64, f64)> = (0..360)
            .map(|i| (series, start + i * 10_000, (i % 60) as f64))
            .collect();
        store.samples_insert(&samples).unwrap();
        store.samples_insert(&[(series, now, f64::NAN)]).unwrap();

        let raw = store.samples_query(series, start, now, 1_000).unwrap();
        assert_eq!(raw.len(), 360);
        let coarse = store.samples_query(series, start, now, 36).unwrap();
        assert!(coarse.len() <= 37 && coarse.len() >= 30, "{}", coarse.len());
        assert!(
            coarse
                .iter()
                .all(|point| point.min <= point.avg && point.avg <= point.max)
        );
        assert_eq!(store.sample_latest(series).unwrap().unwrap().1, 59.0);

        store.samples_maintain(now).unwrap();
        let rollups: i64 = store
            .with(|conn| {
                conn.query_row("SELECT COUNT(*) FROM rollups WHERE res = 300", [], |row| {
                    row.get(0)
                })
            })
            .unwrap();
        assert!((11..=13).contains(&rollups), "{rollups}");

        // Far in the future everything raw has aged out, roll-ups answer instead.
        let later = now + RAW_RETENTION_MS + 3_600_000;
        store.samples_maintain(later).unwrap();
        let remaining: i64 = store
            .with(|conn| conn.query_row("SELECT COUNT(*) FROM samples", [], |row| row.get(0)))
            .unwrap();
        assert_eq!(remaining, 0);

        assert_eq!(store.series_of("dev1").unwrap().len(), 2);
        store.samples_forget_device("dev1").unwrap();
        assert!(store.series_of("dev1").unwrap().is_empty());
    }

    #[test]
    fn config_snapshots_deduplicate_and_decompress() {
        let store = Store::in_memory().unwrap();
        let text = "hostname SW1\n!\nvlan 10\n name Staff\n";
        let (first, stored) = store
            .config_insert("dev1", "running", text, "manual", "")
            .unwrap();
        assert!(stored);
        assert_eq!(first.lines, 4);
        let (again, stored) = store
            .config_insert("dev1", "running", text, "scheduled", "")
            .unwrap();
        assert!(!stored, "identical content is not stored twice");
        assert_eq!(again.id, first.id);
        let (second, stored) = store
            .config_insert(
                "dev1",
                "running",
                &format!("{text}vlan 20\n"),
                "post-change",
                "added vlan",
            )
            .unwrap();
        assert!(stored);
        store
            .config_insert("dev1", "startup", text, "manual", "")
            .unwrap();

        let versions = store.config_versions("dev1", Some("running")).unwrap();
        assert_eq!(
            versions
                .iter()
                .map(|version| version.id)
                .collect::<Vec<_>>(),
            vec![second.id, first.id]
        );
        assert_eq!(store.config_versions("dev1", None).unwrap().len(), 3);
        assert_eq!(store.config_get(first.id).unwrap().1, text);
        assert_eq!(
            store
                .config_latest("dev1", "running")
                .unwrap()
                .unwrap()
                .note,
            "added vlan"
        );
        assert_eq!(store.config_get(9_999).unwrap_err().code, "not_found");
        store.config_delete_device("dev1").unwrap();
        assert!(store.config_versions("dev1", None).unwrap().is_empty());
    }
}
