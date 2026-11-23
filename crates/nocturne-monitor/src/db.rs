//! This feature's own tables.
//!
//! Alerts and the interface table are the two things monitoring keeps that a
//! campus has too many of to filter, sort and page in memory: a few thousand
//! devices with fifty ports each is a hundred thousand rows. Both get a table
//! so the work happens in SQL, and so do the QoS classes, of which one
//! router with a queue per subscriber has thousands. Everything else (rules,
//! maintenance windows, channels, the per-device record) is a handful of
//! documents.

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

CREATE TABLE IF NOT EXISTS monitor_qos (
    device_id       TEXT NOT NULL,
    -- The row's place among the device's classes in their stable order
    -- (interface, direction, policy, class, parent) at the last read.
    seq             INTEGER NOT NULL,
    -- The same order spelled in bytes (`qos::sort_key`), which is what a
    -- page carries on from: a position moves when a class before it comes or
    -- goes between two pages, a class's place among the names does not.
    sort_key        BLOB NOT NULL,
    interface       TEXT NOT NULL,
    direction       TEXT NOT NULL,
    policy          TEXT NOT NULL,
    class           TEXT NOT NULL,
    parent          TEXT,
    -- The instance of the class's series; NULL when it has none.
    instance        TEXT,
    rate_bps        INTEGER,
    bps             REAL,
    drop_ps         REAL,
    queue_drop_ps   REAL,
    drop_pct        REAL,
    queue_len       INTEGER,
    packets         INTEGER,
    dropped_packets INTEGER,
    dropping        INTEGER NOT NULL DEFAULT 0,
    unverified      INTEGER NOT NULL DEFAULT 0,
    at              INTEGER NOT NULL,
    PRIMARY KEY (device_id, seq)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS monitor_qos_order ON monitor_qos (device_id, sort_key, seq);
CREATE INDEX IF NOT EXISTS monitor_qos_dropping ON monitor_qos (dropping, device_id, sort_key, seq);

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

/// The alerts that still owe their firing notice, and only those: what the
/// scheduler's pass over them reads (`alerts::announce_owed`). In an
/// installation where every alert was announced when it was raised this
/// index is empty, and the pass costs one look at it.
///
/// Created after the backfill above, which would otherwise put every alert
/// of an upgraded database into it and take them out again.
fn owed_index() -> String {
    format!(
        "CREATE INDEX IF NOT EXISTS monitor_alerts_owed ON monitor_alerts (started_at, id)
         WHERE {};",
        crate::alerts::OWED
    )
}

/// Make the index of owed notices, and make it again when the one the
/// database holds is not the one this version would create.
///
/// SQLite answers from a partial index only when the query repeats its
/// condition, and the pass names this index so that it can never quietly
/// become a scan. The price of that is paid here: `IF NOT EXISTS` would keep
/// an index left by a version whose idea of "owed" read differently, the
/// pass's statement would then fail to prepare, and every notice held back
/// from then on would stay unsent with only a warning in the log to show
/// for it.
fn ensure_owed_index(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    use rusqlite::OptionalExtension;
    let held: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'index' AND name = 'monitor_alerts_owed'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if held.is_some_and(|sql| !sql.contains(crate::alerts::OWED)) {
        conn.execute_batch("DROP INDEX monitor_alerts_owed;")?;
    }
    conn.execute_batch(&owed_index())
}

/// Drop a `monitor_qos` made before it had `sort_key`, so that `SCHEMA`
/// makes it again. It holds each device's last read and nothing else, and
/// the next read of each device fills it.
fn drop_unkeyed_qos_table(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('monitor_qos')")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if !columns.is_empty() && !columns.iter().any(|name| name == "sort_key") {
        conn.execute_batch("DROP TABLE monitor_qos;")?;
    }
    Ok(())
}

pub fn init(store: &Store) -> AppResult<()> {
    store.with(|conn| {
        drop_unkeyed_qos_table(conn)?;
        conn.execute_batch(SCHEMA)?;
        conn.execute_batch(BACKFILL_NOTIFIED_AT)?;
        ensure_owed_index(conn)
    })
}

/// Document collections this feature owns.
pub mod collections {
    pub const RULES: &str = "monitor_rules";
    pub const MAINTENANCE: &str = "monitor_maintenance";
    pub const CHANNELS: &str = "monitor_channels";
    /// One per device: last poll, health and per-source status.
    pub const DEVICES: &str = "monitor_devices";
    /// One per device that has answered SNMP: which ifIndex is which
    /// interface.
    pub const IFINDEX: &str = "monitor_ifindex";
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
        assert_eq!(tables, 5);
    }

    #[test]
    fn the_pass_over_owed_notices_reads_an_index_that_holds_only_those_alerts() {
        // What bounds the cost of `alerts::announce_owed` in an installation
        // with a hundred thousand alerts on record: it never looks at one
        // that was announced, acknowledged or resolved.
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        let plan: Vec<String> = store
            .with(|conn| {
                let mut statement = conn.prepare(&format!(
                    "EXPLAIN QUERY PLAN {}",
                    crate::alerts::owed_query()
                ))?;
                statement
                    .query_map(rusqlite::params![0, "", 200], |row| row.get(3))?
                    .collect()
            })
            .unwrap();
        assert!(
            plan.iter().any(|step| step.contains("monitor_alerts_owed")),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|step| step.contains("TEMP B-TREE")),
            "and in the index's own order, with nothing sorted: {plan:?}"
        );

        // The index holds an alert exactly while a notice is owed for it.
        let insert = |id: &str, state: &str, body: &str| {
            store
                .with(|conn| {
                    conn.execute(
                        "INSERT INTO monitor_alerts
                         (id, dedup_key, state, severity, started_at, last_seen_at, body)
                         VALUES (?1, ?1, ?2, 2, 1, 1, ?3)",
                        rusqlite::params![id, state, body],
                    )
                })
                .unwrap();
        };
        insert("owed", "firing", r#"{"notified_at":null,"notify":null}"#);
        insert("named", "firing", r#"{"notified_at":null,"notify":["c1"]}"#);
        insert("told", "firing", r#"{"notified_at":5,"notify":null}"#);
        insert("nobody", "firing", r#"{"notified_at":null,"notify":[]}"#);
        insert(
            "taken",
            "acknowledged",
            r#"{"notified_at":null,"notify":null}"#,
        );
        insert("over", "resolved", r#"{"notified_at":null,"notify":null}"#);
        let mut indexed: Vec<String> = store
            .with(|conn| {
                let mut statement = conn.prepare(&format!(
                    "SELECT id FROM monitor_alerts INDEXED BY monitor_alerts_owed WHERE {}",
                    crate::alerts::OWED
                ))?;
                statement.query_map([], |row| row.get(0))?.collect()
            })
            .unwrap();
        indexed.sort();
        assert_eq!(indexed, ["named", "owed"]);
    }

    #[test]
    fn an_index_of_owed_notices_left_by_another_version_is_made_again() {
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        let prepares = |store: &Store| {
            store.with(|conn| conn.prepare(&crate::alerts::owed_query()).map(|_| ()))
        };
        prepares(&store).unwrap();

        // The index as a version with another idea of what is owed would
        // have left it: same name, a condition the pass does not repeat.
        store
            .with(|conn| {
                conn.execute_batch(
                    "DROP INDEX monitor_alerts_owed;
                     CREATE INDEX monitor_alerts_owed ON monitor_alerts (started_at, id)
                     WHERE state = 'firing' AND json_extract(body, '$.lab') = 0;",
                )
            })
            .unwrap();
        assert!(
            prepares(&store).is_err(),
            "the pass cannot read from that index, which is what makes this worth a test"
        );

        // The next start.
        init(&store).unwrap();
        prepares(&store).unwrap();
        let condition: String = store
            .with(|conn| {
                conn.query_row(
                    "SELECT sql FROM sqlite_master WHERE name = 'monitor_alerts_owed'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert!(condition.contains(crate::alerts::OWED), "{condition}");
    }

    #[test]
    fn a_qos_table_made_without_its_sort_key_is_made_again() {
        let store = Store::in_memory().unwrap();
        // The table as a build before `sort_key` left it, with a row in it.
        store
            .with(|conn| {
                conn.execute_batch(
                    "CREATE TABLE monitor_qos (
                         device_id TEXT NOT NULL, seq INTEGER NOT NULL, interface TEXT NOT NULL,
                         direction TEXT NOT NULL, policy TEXT NOT NULL, class TEXT NOT NULL,
                         parent TEXT, instance TEXT, rate_bps INTEGER, bps REAL, drop_ps REAL,
                         queue_drop_ps REAL, drop_pct REAL, queue_len INTEGER, packets INTEGER,
                         dropped_packets INTEGER, dropping INTEGER NOT NULL DEFAULT 0,
                         unverified INTEGER NOT NULL DEFAULT 0, at INTEGER NOT NULL,
                         PRIMARY KEY (device_id, seq)) WITHOUT ROWID;
                     INSERT INTO monitor_qos (device_id, seq, interface, direction, policy, class, at)
                     VALUES ('dev1', 0, 'Gi0/1', 'out', 'WAN', 'VOICE', 1);",
                )
            })
            .unwrap();
        init(&store).unwrap();
        let (columns, rows): (Vec<String>, i64) = store
            .with(|conn| {
                let columns = conn
                    .prepare("SELECT name FROM pragma_table_info('monitor_qos')")?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let rows =
                    conn.query_row("SELECT COUNT(*) FROM monitor_qos", [], |row| row.get(0))?;
                Ok((columns, rows))
            })
            .unwrap();
        assert!(columns.iter().any(|name| name == "sort_key"), "{columns:?}");
        assert_eq!(rows, 0, "the next read of the device fills it again");
        // And a table that has it is left alone.
        store
            .with(|conn| {
                conn.execute(
                    "INSERT INTO monitor_qos (device_id, seq, sort_key, interface, direction, policy, class, at)
                     VALUES ('dev1', 0, x'00', 'Gi0/1', 'out', 'WAN', 'VOICE', 1)",
                    [],
                )
            })
            .unwrap();
        init(&store).unwrap();
        let rows: i64 = store
            .with(|conn| conn.query_row("SELECT COUNT(*) FROM monitor_qos", [], |row| row.get(0)))
            .unwrap();
        assert_eq!(rows, 1);
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
