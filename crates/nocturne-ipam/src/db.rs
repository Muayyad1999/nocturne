//! Where records and observations live: two tables of this feature's own.
//!
//! They are separate tables on purpose. A record is written by a person and
//! an observation by a sync, they are read for different questions, and
//! keeping them apart means no query can accidentally return one as the
//! other. Both are indexed on `(scope, family, ip_key)`, where `ip_key` is
//! the address in fixed-width hex — so "every address in this prefix" is one
//! `BETWEEN` over an index rather than a scan, and rows come back in address
//! order, which is the order anybody reading a subnet expects.
//!
//! The `search` column is the lowercased text a free-text query matches: the
//! address, the host name, the description, the device, the interface and the
//! MAC in both notations.

use nocturne_app::store::Store;
use nocturne_app::{AppResult, util};
use rusqlite::{Connection, OptionalExtension, Row, params, params_from_iter};

use crate::model::{AddressRecord, Observation, ObservationKind};
use crate::net::{self, Net};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS ipam_address (
    id         TEXT PRIMARY KEY,
    -- '' for production, 'lab:<lab id>' for a lab: the two never mix.
    scope      TEXT NOT NULL,
    family     INTEGER NOT NULL,
    ip         TEXT NOT NULL,
    -- The address in fixed-width hex, so string order is address order.
    ip_key     TEXT NOT NULL,
    prefix_id  TEXT NOT NULL DEFAULT '',
    status     TEXT NOT NULL,
    search     TEXT NOT NULL DEFAULT '',
    body       TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS ipam_address_range ON ipam_address (scope, family, ip_key);
CREATE INDEX IF NOT EXISTS ipam_address_prefix ON ipam_address (prefix_id);

CREATE TABLE IF NOT EXISTS ipam_observation (
    id           TEXT PRIMARY KEY,
    scope        TEXT NOT NULL,
    family       INTEGER NOT NULL,
    ip           TEXT NOT NULL,
    ip_key       TEXT NOT NULL,
    -- 'interface' or 'arp'.
    kind         TEXT NOT NULL,
    device_id    TEXT NOT NULL,
    interface    TEXT NOT NULL,
    last_seen    INTEGER NOT NULL,
    collected_at INTEGER NOT NULL,
    search       TEXT NOT NULL DEFAULT '',
    body         TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS ipam_observation_range ON ipam_observation (scope, family, ip_key);
CREATE INDEX IF NOT EXISTS ipam_observation_device ON ipam_observation (device_id, kind);
CREATE INDEX IF NOT EXISTS ipam_observation_age ON ipam_observation (kind, last_seen);
";

pub fn init(store: &Store) -> AppResult<()> {
    store.with(|conn| conn.execute_batch(SCHEMA))
}

/// The document collections for the records that are few enough to be
/// documents: a campus has thousands of prefixes, not millions.
pub const PREFIXES: &str = "ipam_prefixes";
pub const VLANS: &str = "ipam_vlans";

// ------------------------------------------------------------------- keys

pub fn record_id(scope: &str, ip: &str) -> String {
    format!("{scope}|{ip}")
}

pub fn observation_id(observation: &Observation) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        observation.scope,
        observation.ip,
        observation.kind.as_str(),
        observation.device_id,
        observation.interface
    )
}

fn record_search(record: &AddressRecord) -> String {
    let mut text = format!(
        "{} {} {} {} {}",
        record.ip,
        record.hostname,
        record.description,
        record.interface.clone().unwrap_or_default(),
        record.tags.join(" ")
    );
    if let Some(mac) = &record.mac {
        text.push(' ');
        text.push_str(mac);
        text.push(' ');
        text.push_str(&bare_mac(mac));
    }
    text.to_lowercase()
}

fn observation_search(observation: &Observation) -> String {
    let mut text = format!(
        "{} {} {} {}",
        observation.ip,
        observation.device,
        observation.interface,
        observation.vendor.clone().unwrap_or_default()
    );
    if let Some(mac) = &observation.mac {
        text.push(' ');
        text.push_str(mac);
        text.push(' ');
        text.push_str(&bare_mac(mac));
    }
    text.to_lowercase()
}

/// A MAC with every separator removed, so `aabb.ccdd.eeff` is found by
/// `aa:bb:cc`, `aabbcc` or `AABB.CC`.
pub fn bare_mac(mac: &str) -> String {
    mac.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

// ---------------------------------------------------------------- records

fn record_of(row: &Row<'_>) -> rusqlite::Result<AddressRecord> {
    let body: String = row.get(0)?;
    serde_json::from_str(&body).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn write_record(conn: &Connection, record: &AddressRecord) -> rusqlite::Result<()> {
    let address = net::addr(&record.ip).map_err(|err| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(err.message)))
    })?;
    let body = serde_json::to_string(record)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    conn.execute(
        "INSERT INTO ipam_address (id, scope, family, ip, ip_key, prefix_id, status, search, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT (id) DO UPDATE SET prefix_id = excluded.prefix_id, status = excluded.status,
             search = excluded.search, body = excluded.body",
        params![
            record_id(&record.scope, &record.ip),
            record.scope,
            address.family,
            address.text,
            address.key(),
            record.prefix_id.clone().unwrap_or_default(),
            serde_json::to_value(record.status)
                .map(|value| value.as_str().unwrap_or_default().to_string())
                .unwrap_or_default(),
            record_search(record),
            body,
        ],
    )?;
    Ok(())
}

pub fn put_record(store: &Store, record: &AddressRecord) -> AppResult<()> {
    store.with(|conn| write_record(conn, record))
}

pub fn put_records(store: &Store, records: &[AddressRecord]) -> AppResult<()> {
    store.transaction(|tx| {
        for record in records {
            write_record(tx, record)?;
        }
        Ok(())
    })
}

pub fn record(store: &Store, scope: &str, ip: &str) -> AppResult<Option<AddressRecord>> {
    store.with(|conn| {
        conn.query_row(
            "SELECT body FROM ipam_address WHERE id = ?1",
            params![record_id(scope, ip)],
            record_of,
        )
        .optional()
    })
}

pub fn delete_record(store: &Store, scope: &str, ip: &str) -> AppResult<bool> {
    let removed = store.with(|conn| {
        conn.execute(
            "DELETE FROM ipam_address WHERE id = ?1",
            params![record_id(scope, ip)],
        )
    })?;
    Ok(removed > 0)
}

/// Every record in a prefix's range, in address order.
pub fn records_in(store: &Store, scope: &str, block: &Net) -> AppResult<Vec<AddressRecord>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM ipam_address
              WHERE scope = ?1 AND family = ?2 AND ip_key BETWEEN ?3 AND ?4
              ORDER BY ip_key",
        )?;
        let rows = statement.query_map(
            params![scope, block.family, block.key(), block.last_key()],
            record_of,
        )?;
        rows.collect()
    })
}

/// Records whose `prefix_id` points at a prefix, wherever they actually are —
/// which is how an address that claims a prefix it is not inside is found.
pub fn records_claiming(store: &Store, prefix_id: &str) -> AppResult<Vec<AddressRecord>> {
    store.with(|conn| {
        let mut statement = conn
            .prepare_cached("SELECT body FROM ipam_address WHERE prefix_id = ?1 ORDER BY ip_key")?;
        let rows = statement.query_map(params![prefix_id], record_of)?;
        rows.collect()
    })
}

/// Records that name a prefix but are not inside it — the whole check as one
/// indexed query per prefix, because loading a quarter of a million records
/// to find none of them is not a check, it is a stall.
pub fn records_outside(
    store: &Store,
    prefix_id: &str,
    block: &Net,
) -> AppResult<Vec<AddressRecord>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM ipam_address
              WHERE prefix_id = ?1
                AND (family != ?2 OR ip_key < ?3 OR ip_key > ?4)
              ORDER BY ip_key",
        )?;
        let rows = statement.query_map(
            params![prefix_id, block.family, block.key(), block.last_key()],
            record_of,
        )?;
        rows.collect()
    })
}

/// Records that name no prefix at all. A campus has a handful; they are the
/// ones nothing on record contains.
pub fn unfiled_records(store: &Store, limit: usize) -> AppResult<Vec<AddressRecord>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM ipam_address WHERE prefix_id = '' ORDER BY family, ip_key LIMIT ?1",
        )?;
        let rows = statement.query_map(params![limit as i64], record_of)?;
        rows.collect()
    })
}

pub fn all_records(store: &Store) -> AppResult<Vec<AddressRecord>> {
    store.with(|conn| {
        let mut statement =
            conn.prepare_cached("SELECT body FROM ipam_address ORDER BY scope, family, ip_key")?;
        let rows = statement.query_map([], record_of)?;
        rows.collect()
    })
}

pub fn count_records(store: &Store) -> AppResult<u64> {
    let count: i64 = store
        .with(|conn| conn.query_row("SELECT COUNT(*) FROM ipam_address", [], |row| row.get(0)))?;
    Ok(count.max(0) as u64)
}

// ----------------------------------------------------------- observations

fn observation_of(row: &Row<'_>) -> rusqlite::Result<Observation> {
    let body: String = row.get(0)?;
    serde_json::from_str(&body).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

fn write_observation(conn: &Connection, observation: &Observation) -> rusqlite::Result<()> {
    let address = net::addr(&observation.ip).map_err(|err| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(err.message)))
    })?;
    let body = serde_json::to_string(observation)
        .map_err(|err| rusqlite::Error::ToSqlConversionFailure(Box::new(err)))?;
    conn.execute(
        "INSERT INTO ipam_observation
             (id, scope, family, ip, ip_key, kind, device_id, interface, last_seen, collected_at, search, body)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT (id) DO UPDATE SET last_seen = excluded.last_seen,
             collected_at = excluded.collected_at, search = excluded.search, body = excluded.body",
        params![
            observation_id(observation),
            observation.scope,
            address.family,
            address.text,
            address.key(),
            observation.kind.as_str(),
            observation.device_id,
            observation.interface,
            observation.last_seen,
            observation.collected_at,
            observation_search(observation),
            body,
        ],
    )?;
    Ok(())
}

/// Replace a device's sightings of one kind, keeping the `first_seen` of
/// anything already known. An interface table is authoritative — an address
/// that is no longer on it is gone, not stale — so the old rows of that kind
/// are deleted in the same transaction.
pub fn replace_device_observations(
    store: &Store,
    device_id: &str,
    kind: ObservationKind,
    observations: &[Observation],
) -> AppResult<()> {
    store.transaction(|tx| {
        tx.execute(
            "DELETE FROM ipam_observation WHERE device_id = ?1 AND kind = ?2",
            params![device_id, kind.as_str()],
        )?;
        for observation in observations {
            write_observation(tx, observation)?;
        }
        Ok(())
    })
}

pub fn put_observations(store: &Store, observations: &[Observation]) -> AppResult<()> {
    store.transaction(|tx| {
        for observation in observations {
            write_observation(tx, observation)?;
        }
        Ok(())
    })
}

pub fn observations_of_device(
    store: &Store,
    device_id: &str,
    kind: ObservationKind,
) -> AppResult<Vec<Observation>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM ipam_observation WHERE device_id = ?1 AND kind = ?2 ORDER BY ip_key",
        )?;
        let rows = statement.query_map(params![device_id, kind.as_str()], observation_of)?;
        rows.collect()
    })
}

pub fn observations_in(store: &Store, scope: &str, block: &Net) -> AppResult<Vec<Observation>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM ipam_observation
              WHERE scope = ?1 AND family = ?2 AND ip_key BETWEEN ?3 AND ?4
              ORDER BY ip_key",
        )?;
        let rows = statement.query_map(
            params![scope, block.family, block.key(), block.last_key()],
            observation_of,
        )?;
        rows.collect()
    })
}

/// Interface sightings inside a block — the devices a prefix row shows.
/// Capped, because a /8 with ten thousand interfaces in it is a list nobody
/// reads; the count in `observed` is not capped.
pub fn interfaces_in(
    store: &Store,
    scope: &str,
    block: &Net,
    limit: usize,
) -> AppResult<Vec<Observation>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM ipam_observation
              WHERE scope = ?1 AND family = ?2 AND ip_key BETWEEN ?3 AND ?4 AND kind = 'interface'
              ORDER BY ip_key LIMIT ?5",
        )?;
        let rows = statement.query_map(
            params![
                scope,
                block.family,
                block.key(),
                block.last_key(),
                limit as i64
            ],
            observation_of,
        )?;
        rows.collect()
    })
}

/// What is inside a block, counted in SQL. A campus has a quarter of a
/// million addresses and a tree of two thousand prefixes: the numbers on
/// every row have to come from an index, not from loading the rows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    /// Address records of any status.
    pub records: u32,
    pub reserved: u32,
    /// Records that are `active` or `dhcp`.
    pub in_use_records: u32,
    /// Distinct addresses seen on the network (within the ARP window).
    pub observed: u32,
    /// Of those, the ones nobody has a record for.
    pub observed_only: u32,
}

pub fn counts_in(store: &Store, scope: &str, block: &Net, arp_floor: i64) -> AppResult<Counts> {
    let (records, reserved, in_use_records) = store.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(status = 'reserved'), 0),
                    COALESCE(SUM(status IN ('active', 'dhcp')), 0)
               FROM ipam_address
              WHERE scope = ?1 AND family = ?2 AND ip_key BETWEEN ?3 AND ?4",
            params![scope, block.family, block.key(), block.last_key()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
    })?;
    let (observed, observed_only) = store.with(|conn| {
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(undocumented), 0) FROM (
                 SELECT o.ip,
                        NOT EXISTS (SELECT 1 FROM ipam_address a WHERE a.id = o.scope || '|' || o.ip) AS undocumented
                   FROM ipam_observation o
                  WHERE o.scope = ?1 AND o.family = ?2 AND o.ip_key BETWEEN ?3 AND ?4
                    AND (o.kind = 'interface' OR o.last_seen >= ?5)
                  GROUP BY o.ip)",
            params![
                scope,
                block.family,
                block.key(),
                block.last_key(),
                arp_floor
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
    })?;
    Ok(Counts {
        records: records.max(0) as u32,
        reserved: reserved.max(0) as u32,
        in_use_records: in_use_records.max(0) as u32,
        observed: observed.max(0) as u32,
        observed_only: observed_only.max(0) as u32,
    })
}

/// One record, as little of it as a count needs.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordPoint {
    pub family: u8,
    pub key: String,
    pub reserved: bool,
    /// `active` or `dhcp`.
    pub in_use: bool,
}

/// One address the network has shown, and whether anybody wrote it down.
#[derive(Clone, Debug, PartialEq)]
pub struct ObservedPoint {
    pub family: u8,
    pub key: String,
    pub undocumented: bool,
}

/// Every record of a namespace as a sort key and two flags, in address order.
///
/// This is what makes a whole prefix tree cost two queries instead of two per
/// row: the ranges are then found by binary search in memory. Nothing is
/// deserialised, so a quarter of a million records is a few megabytes of
/// keys rather than a few hundred of JSON.
pub fn record_points(store: &Store, scope: &str) -> AppResult<Vec<RecordPoint>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT family, ip_key, status FROM ipam_address WHERE scope = ?1
              ORDER BY family, ip_key",
        )?;
        let rows = statement.query_map(params![scope], |row| {
            let status: String = row.get(2)?;
            Ok(RecordPoint {
                family: row.get::<_, i64>(0)? as u8,
                key: row.get(1)?,
                reserved: status == "reserved",
                in_use: status == "active" || status == "dhcp",
            })
        })?;
        rows.collect()
    })
}

/// The same for sightings: one row per address, with whether a record exists.
pub fn observed_points(
    store: &Store,
    scope: &str,
    arp_floor: i64,
) -> AppResult<Vec<ObservedPoint>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT o.family, o.ip_key,
                    NOT EXISTS (SELECT 1 FROM ipam_address a WHERE a.id = o.scope || '|' || o.ip)
               FROM ipam_observation o
              WHERE o.scope = ?1 AND (o.kind = 'interface' OR o.last_seen >= ?2)
              GROUP BY o.family, o.ip_key
              ORDER BY o.family, o.ip_key",
        )?;
        let rows = statement.query_map(params![scope, arp_floor], |row| {
            Ok(ObservedPoint {
                family: row.get::<_, i64>(0)? as u8,
                key: row.get(1)?,
                undocumented: row.get::<_, i64>(2)? != 0,
            })
        })?;
        rows.collect()
    })
}

/// Every interface sighting of a namespace, in address order.
pub fn interface_observations(store: &Store, scope: &str) -> AppResult<Vec<Observation>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM ipam_observation WHERE scope = ?1 AND kind = 'interface'
              ORDER BY family, ip_key",
        )?;
        let rows = statement.query_map(params![scope], observation_of)?;
        rows.collect()
    })
}

pub fn observations_of_ip(store: &Store, scope: &str, ip: &str) -> AppResult<Vec<Observation>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            // The strongest evidence first: an address configured on an
            // interface says more than one that merely answered.
            "SELECT body FROM ipam_observation WHERE scope = ?1 AND ip = ?2
              ORDER BY CASE kind WHEN 'interface' THEN 0 ELSE 1 END, device_id, interface",
        )?;
        let rows = statement.query_map(params![scope, ip], observation_of)?;
        rows.collect()
    })
}

/// The records and the sightings of a set of addresses, in two queries
/// instead of two per address — what a search result needs to be composed.
pub fn records_of(
    store: &Store,
    keys: &[(String, String)],
) -> AppResult<std::collections::BTreeMap<(String, String), AddressRecord>> {
    let mut found = std::collections::BTreeMap::new();
    for chunk in keys.chunks(400) {
        let ids: Vec<String> = chunk
            .iter()
            .map(|(scope, ip)| record_id(scope, ip))
            .collect();
        let placeholders = vec!["?"; ids.len()].join(",");
        let rows: Vec<AddressRecord> = store.with(|conn| {
            let sql = format!("SELECT body FROM ipam_address WHERE id IN ({placeholders})");
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(ids.iter()), record_of)?;
            rows.collect()
        })?;
        for record in rows {
            found.insert((record.scope.clone(), record.ip.clone()), record);
        }
    }
    Ok(found)
}

pub fn observations_of(
    store: &Store,
    keys: &[(String, String)],
) -> AppResult<std::collections::BTreeMap<(String, String), Vec<Observation>>> {
    let mut found: std::collections::BTreeMap<(String, String), Vec<Observation>> =
        std::collections::BTreeMap::new();
    for chunk in keys.chunks(400) {
        let ids: Vec<String> = chunk
            .iter()
            .map(|(scope, ip)| format!("{scope}|{ip}"))
            .collect();
        let placeholders = vec!["?"; ids.len()].join(",");
        let rows: Vec<Observation> = store.with(|conn| {
            let sql = format!(
                "SELECT body FROM ipam_observation WHERE scope || '|' || ip IN ({placeholders})
                  ORDER BY CASE kind WHEN 'interface' THEN 0 ELSE 1 END, device_id, interface"
            );
            let mut statement = conn.prepare(&sql)?;
            let rows = statement.query_map(params_from_iter(ids.iter()), observation_of)?;
            rows.collect()
        })?;
        for observation in rows {
            found
                .entry((observation.scope.clone(), observation.ip.clone()))
                .or_default()
                .push(observation);
        }
    }
    Ok(found)
}

pub fn all_observations(store: &Store) -> AppResult<Vec<Observation>> {
    store.with(|conn| {
        let mut statement = conn
            .prepare_cached("SELECT body FROM ipam_observation ORDER BY scope, family, ip_key")?;
        let rows = statement.query_map([], observation_of)?;
        rows.collect()
    })
}

/// Interface sightings across the whole estate, in address order — what
/// prefix discovery and the duplicate-address check walk.
pub fn observations_of_kind(store: &Store, kind: ObservationKind) -> AppResult<Vec<Observation>> {
    store.with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM ipam_observation WHERE kind = ?1 ORDER BY scope, family, ip_key",
        )?;
        let rows = statement.query_map(params![kind.as_str()], observation_of)?;
        rows.collect()
    })
}

/// Forget ARP sightings nobody has refreshed within the window. An interface
/// address is never aged out: it is removed the moment the device stops
/// reporting it.
pub fn prune_stale_arp(store: &Store, before: i64) -> AppResult<usize> {
    store.with(|conn| {
        conn.execute(
            "DELETE FROM ipam_observation WHERE kind = 'arp' AND last_seen < ?1",
            params![before],
        )
    })
}

pub fn forget_device(store: &Store, device_id: &str) -> AppResult<usize> {
    store.with(|conn| {
        conn.execute(
            "DELETE FROM ipam_observation WHERE device_id = ?1",
            params![device_id],
        )
    })
}

pub fn count_observations(store: &Store) -> AppResult<u64> {
    let count: i64 = store.with(|conn| {
        conn.query_row("SELECT COUNT(*) FROM ipam_observation", [], |row| {
            row.get(0)
        })
    })?;
    Ok(count.max(0) as u64)
}

// ----------------------------------------------------------------- search

/// What a free-text search found, kept apart by what it is.
#[derive(Debug, Default)]
pub struct Found {
    pub records: Vec<AddressRecord>,
    pub observations: Vec<Observation>,
}

/// Records and sightings matching every word of `query`, in SQL, with a cap.
///
/// A query that is an address or the start of one is matched against the
/// address column as a prefix, which an index can serve; anything else goes
/// to the `search` column.
pub fn search(store: &Store, query: &str, limit: usize) -> AppResult<Found> {
    let words: Vec<String> = query
        .split_whitespace()
        .map(|word| word.to_lowercase())
        .collect();
    if words.is_empty() {
        return Ok(Found::default());
    }
    let mut clauses = Vec::new();
    let mut values: Vec<String> = Vec::new();
    for word in &words {
        // A MAC typed in any notation becomes its bare digits, which is also
        // in `search`; everything else is matched as typed.
        let needle = if looks_like_a_mac(word) {
            bare_mac(word)
        } else {
            word.clone()
        };
        clauses.push("search LIKE ? ESCAPE '\\'".to_string());
        values.push(format!("%{}%", escape_like(&needle)));
    }
    let where_clause = clauses.join(" AND ");
    let records = store.with(|conn| {
        let sql = format!(
            "SELECT body FROM ipam_address WHERE {where_clause} ORDER BY family, ip_key LIMIT {limit}"
        );
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), record_of)?;
        rows.collect::<rusqlite::Result<Vec<AddressRecord>>>()
    })?;
    let observations = store.with(|conn| {
        let sql = format!(
            "SELECT body FROM ipam_observation WHERE {where_clause} ORDER BY family, ip_key LIMIT {limit}"
        );
        let mut statement = conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), observation_of)?;
        rows.collect::<rusqlite::Result<Vec<Observation>>>()
    })?;
    Ok(Found {
        records,
        observations,
    })
}

/// `LIKE` treats these as wildcards; a search for `10.1.1%` means the text.
/// A MAC, or something that only looks like one?
///
/// This decides whether a search term is matched as typed or stripped to bare
/// hexadecimal digits, and getting it wrong is expensive: the rule used to be
/// "six or more hexadecimal digits once the punctuation is gone", and
/// `10.10.10` strips to `101010`, which is six hexadecimal digits. So an
/// address was searched for as `%101010%`, matched nothing, and an IPAM
/// answered "nothing is known about that address" about an address it had a
/// record of. `10.10.1` was fine, because it strips to five.
///
/// The separators decide it instead, because the notations do not overlap:
///
/// * nothing at all — `aabbccddeeff`: bare digits, so a MAC;
/// * `:` or `-` in pairs — `aa:bb:cc`, `aa-bb-cc`: a MAC, whole or a prefix;
/// * `.` in fours — `aabb.ccdd.eeff`: Cisco's notation, a MAC;
/// * `.` in anything else — `10.10.10`, `10.10.10.5`: an address;
/// * `:` in groups that are not pairs — `2001:db8`: an IPv6 address.
fn looks_like_a_mac(word: &str) -> bool {
    let bare = bare_mac(word);
    // A MAC is twelve digits; fewer than six is too little of one to be sure.
    if !(6..=12).contains(&bare.len()) || !bare.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let groups: Vec<&str> = word.split(['.', ':', '-']).collect();
    if groups.len() == 1 {
        return true;
    }
    let width = if word.contains('.') && !word.contains(':') {
        // Cisco writes a MAC in three groups of four; dotted decimal does not.
        4
    } else {
        2
    };
    groups.iter().all(|group| group.len() == width)
}

/// Escape, not delete.
///
/// `_` and `%` are LIKE's wildcards and have to be escaped — but deleting
/// them silently changes the question, so a search for `desk_12` quietly
/// became a search for `desk12` and answered about something else. The
/// clauses that use this carry `ESCAPE '\\'`.
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(character);
    }
    out
}

/// A stable id for a new record document.
pub fn new_id() -> String {
    util::new_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Status;

    fn store() -> Store {
        let store = Store::in_memory().unwrap();
        init(&store).unwrap();
        store
    }

    fn record(ip: &str, hostname: &str) -> AddressRecord {
        AddressRecord {
            ip: ip.to_string(),
            hostname: hostname.to_string(),
            status: Status::Active,
            ..AddressRecord::default()
        }
    }

    #[test]
    fn a_prefixs_addresses_come_back_in_address_order() {
        let store = store();
        for ip in ["10.1.10.10", "10.1.10.9", "10.1.11.1", "10.1.10.254"] {
            put_record(&store, &record(ip, "host")).unwrap();
        }
        let block = net::net("10.1.10.0/24").unwrap().net;
        let found: Vec<String> = records_in(&store, "", &block)
            .unwrap()
            .into_iter()
            .map(|record| record.ip)
            .collect();
        assert_eq!(found, ["10.1.10.9", "10.1.10.10", "10.1.10.254"]);
    }

    #[test]
    fn a_lab_and_production_keep_their_own_namespaces() {
        let store = store();
        put_record(&store, &record("10.1.10.5", "production")).unwrap();
        put_record(
            &store,
            &AddressRecord {
                scope: "lab:one".into(),
                ..record("10.1.10.5", "simulated")
            },
        )
        .unwrap();
        let block = net::net("10.1.10.0/24").unwrap().net;
        assert_eq!(records_in(&store, "", &block).unwrap().len(), 1);
        assert_eq!(
            records_in(&store, "lab:one", &block).unwrap()[0].hostname,
            "simulated"
        );
        assert_eq!(count_records(&store).unwrap(), 2);
    }

    #[test]
    fn search_finds_an_address_by_part_of_it_or_by_a_mac_in_any_notation() {
        let store = store();
        put_record(
            &store,
            &AddressRecord {
                mac: Some("aa:bb:cc:dd:ee:ff".into()),
                ..record("10.1.10.5", "printer-3")
            },
        )
        .unwrap();
        assert_eq!(search(&store, "10.1.10.", 50).unwrap().records.len(), 1);
        assert_eq!(search(&store, "printer", 50).unwrap().records.len(), 1);
        assert_eq!(
            search(&store, "aabb.ccdd.eeff", 50).unwrap().records.len(),
            1
        );
        assert_eq!(
            search(&store, "AA-BB-CC-DD-EE-FF", 50)
                .unwrap()
                .records
                .len(),
            1
        );
        assert_eq!(search(&store, "nothing", 50).unwrap().records.len(), 0);
        // An address is an address, even when its digits are all
        // hexadecimal: `10.10.10` strips to `101010`, and the old rule read
        // that as a MAC and found nothing.
        put_record(&store, &record("10.10.10.5", "desk_12")).unwrap();
        assert_eq!(search(&store, "10.10.10", 50).unwrap().records.len(), 1);
        assert_eq!(search(&store, "10.10.10.5", 50).unwrap().records.len(), 1);

        // A wildcard is text, not a pattern. This used to assert `1`, which
        // was the bug rather than the intent: `%` was *deleted* from the
        // term, so the search became `%%` and matched everything. Escaped, it
        // asks for a literal percent sign, and nothing here has one.
        assert_eq!(search(&store, "%", 50).unwrap().records.len(), 0);
        // `_` is LIKE's single-character wildcard. Escaped it asks for a
        // literal underscore, and `desk_12` above has one — so this finds
        // exactly that record and not the other two.
        assert_eq!(search(&store, "_", 50).unwrap().records.len(), 1);
        // And a term that really does contain one is found by it.
        put_record(&store, &record("10.1.10.9", "100%-used")).unwrap();
        assert_eq!(search(&store, "100%", 50).unwrap().records.len(), 1);
        assert_eq!(search(&store, "desk_12", 50).unwrap().records.len(), 1);
        assert_eq!(search(&store, "deskX12", 50).unwrap().records.len(), 0);
    }

    #[test]
    fn replacing_a_devices_interface_sightings_removes_what_it_no_longer_reports() {
        let store = store();
        let base = Observation {
            ip: "10.1.10.1".into(),
            kind: ObservationKind::Interface,
            device_id: "d1".into(),
            device: "SW1".into(),
            interface: "Vlan10".into(),
            prefix_len: Some(24),
            cidr: Some("10.1.10.1/24".into()),
            mac: None,
            vendor: None,
            scope: String::new(),
            first_seen: 100,
            last_seen: 100,
            collected_at: 100,
            collector: "cli".into(),
            current: true,
        };
        let second = Observation {
            ip: "10.1.20.1".into(),
            interface: "Vlan20".into(),
            cidr: Some("10.1.20.1/24".into()),
            ..base.clone()
        };
        replace_device_observations(
            &store,
            "d1",
            ObservationKind::Interface,
            &[base.clone(), second],
        )
        .unwrap();
        assert_eq!(
            observations_of_device(&store, "d1", ObservationKind::Interface)
                .unwrap()
                .len(),
            2
        );
        // The next collection no longer has Vlan20.
        replace_device_observations(&store, "d1", ObservationKind::Interface, &[base]).unwrap();
        let left = observations_of_device(&store, "d1", ObservationKind::Interface).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].ip, "10.1.10.1");
    }

    #[test]
    fn stale_arp_is_forgotten_and_interface_sightings_are_not() {
        let store = store();
        let arp = Observation {
            ip: "10.1.10.50".into(),
            kind: ObservationKind::Arp,
            device_id: "d1".into(),
            device: "SW1".into(),
            interface: "Vlan10".into(),
            prefix_len: None,
            cidr: None,
            mac: Some("aa:bb:cc:00:00:01".into()),
            vendor: None,
            scope: String::new(),
            first_seen: 1_000,
            last_seen: 1_000,
            collected_at: 1_000,
            collector: "cli".into(),
            current: false,
        };
        let configured = Observation {
            ip: "10.1.10.1".into(),
            kind: ObservationKind::Interface,
            last_seen: 1_000,
            ..arp.clone()
        };
        put_observations(&store, &[arp, configured]).unwrap();
        assert_eq!(prune_stale_arp(&store, 5_000).unwrap(), 1);
        assert_eq!(count_observations(&store).unwrap(), 1);
        assert_eq!(
            observations_of_device(&store, "d1", ObservationKind::Interface)
                .unwrap()
                .len(),
            1
        );
    }

    /// The rule that broke address search: `10.10.10` strips to `101010`,
    /// which is six hexadecimal digits, so it was searched for as a MAC and
    /// matched nothing. `10.10.1` strips to five and worked, which is why it
    /// went unnoticed.
    #[test]
    fn an_address_is_never_mistaken_for_a_mac() {
        for address in [
            "10.10.10",
            "10.10.10.5",
            "10.1.1",
            "192.168.100.254",
            "172.16.0.0/12",
            "2001:db8",
            "2001:db8::1",
            "fe80::1",
        ] {
            assert!(
                !looks_like_a_mac(address),
                "{address} is an address, not a MAC"
            );
        }
        for mac in [
            "aabbccddeeff",
            "aa:bb:cc:dd:ee:ff",
            "aa-bb-cc-dd-ee-ff",
            "aabb.ccdd.eeff",
            "aa:bb:cc",
            "001122",
        ] {
            assert!(looks_like_a_mac(mac), "{mac} is a MAC");
        }
        // Too little to tell, so matched as typed rather than guessed at.
        for short in ["aa:bb", "abcd", "10"] {
            assert!(!looks_like_a_mac(short), "{short} is too short to be sure");
        }
    }

    /// LIKE's wildcards are escaped, not deleted: deleting them answers a
    /// different question from the one asked.
    #[test]
    fn a_wildcard_in_a_search_is_escaped_rather_than_dropped() {
        assert_eq!(escape_like("desk_12"), r"desk\_12");
        assert_eq!(escape_like("100%"), r"100\%");
        assert_eq!(escape_like(r"a\b"), r"a\\b");
        assert_eq!(escape_like("10.10.10.5"), "10.10.10.5");
    }
}
