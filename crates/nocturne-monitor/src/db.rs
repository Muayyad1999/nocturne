//! This feature's own tables.
//!
//! Alerts and the interface table are the two things monitoring keeps that a
//! campus has too many of to filter, sort and page in memory: a few thousand
//! devices with fifty ports each is a hundred thousand rows. Both get a table
//! so the work happens in SQL. Everything else (rules, maintenance windows,
//! channels, the per-device record) is a handful of documents.

use nocturne_app::AppResult;
use nocturne_app::store::Store;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS monitor_interfaces (
    device_id    TEXT NOT NULL,
    device       TEXT NOT NULL DEFAULT '',
    name         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    admin_up     INTEGER,
    oper_up      INTEGER,
    speed_bps    INTEGER,
    in_bps       REAL,
    out_bps      REAL,
    util_pct     REAL,
    in_err_ps    REAL,
    out_err_ps   REAL,
    in_disc_ps   REAL,
    out_disc_ps  REAL,
    uplink       INTEGER NOT NULL DEFAULT 0,
    lab          INTEGER NOT NULL DEFAULT 0,
    at           INTEGER NOT NULL,
    PRIMARY KEY (device_id, name)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS monitor_interfaces_problem ON monitor_interfaces (oper_up, admin_up);

CREATE TABLE IF NOT EXISTS monitor_alerts (
    id           TEXT PRIMARY KEY,
    dedup_key    TEXT NOT NULL,
    state        TEXT NOT NULL,
    severity     INTEGER NOT NULL,
    device_id    TEXT,
    started_at   INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL,
    body         TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS monitor_alerts_state ON monitor_alerts (state, started_at);
CREATE INDEX IF NOT EXISTS monitor_alerts_dedup ON monitor_alerts (dedup_key, state);
CREATE INDEX IF NOT EXISTS monitor_alerts_device ON monitor_alerts (device_id, started_at);

CREATE TABLE IF NOT EXISTS monitor_checks (
    id           TEXT PRIMARY KEY,
    device_id    TEXT NOT NULL,
    name         TEXT NOT NULL,
    kind         TEXT NOT NULL,
    enabled      INTEGER NOT NULL DEFAULT 1,
    interval_s   INTEGER NOT NULL,
    -- The definition, as `ServiceCheck`.
    body         TEXT NOT NULL,
    -- The live state. `status` and `since` are columns because `monitor.status`
    -- counts them and a NOC list filters on them; `state` carries the whole
    -- record. `state` is NULL until the first run, which is what `unknown`
    -- means: a check that has never run has found nothing.
    status       TEXT NOT NULL DEFAULT 'unknown',
    since        INTEGER,
    last_run_at  INTEGER,
    state        TEXT,
    created_at   INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL
) WITHOUT ROWID;
CREATE UNIQUE INDEX IF NOT EXISTS monitor_checks_name
    ON monitor_checks (device_id, name COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS monitor_checks_status ON monitor_checks (status, enabled);

CREATE TABLE IF NOT EXISTS monitor_check_events (
    id        INTEGER PRIMARY KEY,
    check_id  TEXT NOT NULL,
    device_id TEXT NOT NULL,
    at        INTEGER NOT NULL,
    status    TEXT NOT NULL,
    previous  TEXT NOT NULL,
    -- How long `previous` had lasted; NULL when that is not known.
    held_s    INTEGER,
    code      TEXT NOT NULL,
    message   TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS monitor_check_events_check ON monitor_check_events (check_id, at);
";

/// Alerts written before `notified_at` existed do not say whether their
/// firing notice went out. Left as they are they read as "nobody was told":
/// every alert open across the upgrade would be announced a second time at
/// its next sighting, and none of them would have its resolve announced. What
/// the old code did is known — it sent the notice when the alert was raised,
/// unless a maintenance window muted it — so that is written down once. The
/// flag an old alert carries is the one from its last sighting, which is the
/// nearest thing on record to the one from its raise.
///
/// Runs at every start and finds nothing after the first: a body that has
/// the key, even as `null`, is left alone.
const BACKFILL_NOTIFIED_AT: &str = "
UPDATE monitor_alerts
SET body = json_set(
    body,
    '$.notified_at',
    CASE WHEN json_extract(body, '$.muted_by_maintenance') THEN NULL
         ELSE json_extract(body, '$.started_at') END)
WHERE json_type(body, '$.notified_at') IS NULL;
";

pub fn init(store: &Store) -> AppResult<()> {
    store.with(|conn| {
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(BACKFILL_NOTIFIED_AT)
    })
}

/// Document collections this feature owns.
pub mod collections {
    pub const RULES: &str = "monitor_rules";
    pub const MAINTENANCE: &str = "monitor_maintenance";
    pub const CHANNELS: &str = "monitor_channels";
    /// One per device: last poll, health and per-source status.
    pub const DEVICES: &str = "monitor_devices";
}

/// The vault id of a channel's secret (its URL, or its SMTP password).
pub fn channel_secret_id(channel_id: &str) -> String {
    format!("monitor:channel:{channel_id}:secret")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_schema_is_idempotent() {
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        init(&store).unwrap();
        let tables: i64 = store
            .with(|conn| {
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name LIKE 'monitor_%'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(tables, 4);
    }

    #[test]
    fn two_checks_on_one_device_cannot_share_a_name() {
        // One name, one alert key, one line a person can read: the database
        // is where that is made true, not the save handler alone.
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        let insert = |id: &str, name: &str| {
            store.with(|conn| {
                conn.execute(
                    "INSERT INTO monitor_checks
                        (id, device_id, name, kind, enabled, interval_s, body, created_at, updated_at)
                     VALUES (?1, 'dev1', ?2, 'tcp', 1, 60, '{}', 0, 0)",
                    rusqlite::params![id, name],
                )
            })
        };
        insert("c1", "web").unwrap();
        assert!(insert("c2", "WEB").is_err(), "the same name in other case");
        insert("c3", "mail").unwrap();
    }

    #[test]
    fn a_channel_secret_lives_under_its_own_vault_id() {
        assert_eq!(channel_secret_id("abc"), "monitor:channel:abc:secret");
    }
}
