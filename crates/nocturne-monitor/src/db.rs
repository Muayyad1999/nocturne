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
";

pub fn init(store: &Store) -> AppResult<()> {
    store.with(|conn| conn.execute_batch(SCHEMA))
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
        assert_eq!(tables, 2);
    }

    #[test]
    fn a_channel_secret_lives_under_its_own_vault_id() {
        assert_eq!(channel_secret_id("abc"), "monitor:channel:abc:secret");
    }
}
