//! Quality of service: what each class of each policy is doing.
//!
//! A device counts, per class of every policy it has attached, the packets
//! it put in the class and the packets it dropped from it. Read every few
//! minutes over the command line (`Kind::Qos`, a profile's ordinary
//! `collect.qos` steps), those counters become five series per class, a
//! current table that `monitor.qos` pages through, and the ground an alert
//! rule on congestion drops stands on.
//!
//! The same three refusals as the rest of the crate, said for this data:
//!
//! * **A rate is two readings.** The first read of a class, a read after
//!   the device restarted, a read after one that failed, and a counter that
//!   went down while the device stayed up all leave the rate out. Nothing is
//!   written for an interval that was not measured.
//! * **The table holds every class; the series do not have to.** A router
//!   with a queue per subscriber has thousands of classes. Series are
//!   written for the first `qos_max_series` of them in a fixed order, and
//!   the device's coverage says when that cut anything.
//! * **A class is only itself.** Two rows that cannot be told apart by name
//!   get no rate and no series, rather than one of them inheriting the
//!   other's counters.

use std::collections::HashMap;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nocturne_app::model::{Device, Transport};
use nocturne_app::{App, AppError, AppResult, Router, collect, inventory, profiles};
use nocturne_drivers::ifname;
use nocturne_drivers::model::{Direction, Kind, QosClass};
use nocturne_drivers::profile::Platform;
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db::collections;
use crate::model::{DeviceRecord, QosCoverage, QosDeviceCoverage, QosRecord, QosState, QosStatus};
use crate::poll::Measure;
use crate::rates::{self, Counter, Interval, TimeBase};
use crate::settings::{LAB_QOS_INTERVAL_S, MonitorSettings};

// ------------------------------------------------------------- the instance

/// Write `text` so that `reserved` cannot occur in it: the character becomes
/// `%XX`, and so does `%` itself, which keeps the way back exact.
fn escape(text: &str, reserved: char) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        if character == '%' || character == reserved {
            out.push_str(&format!("%{:02X}", u32::from(character)));
        } else {
            out.push(character);
        }
    }
    out
}

/// Undo [`escape`]. A `%` that does not start an escape is kept as it is, so
/// an instance written by hand in a rule still reads.
fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('%') {
        out.push_str(&rest[..at]);
        let code = rest
            .get(at + 1..at + 3)
            .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            .filter(u8::is_ascii);
        match code {
            Some(code) => {
                out.push(char::from(code));
                rest = &rest[at + 3..];
            }
            None => {
                out.push('%');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// The instance of a class's series: `<interface>|<in|out>|<policy>/<class>`.
///
/// Interface names are full of slashes, a policy is often named after one
/// (`AutoQoS-Policy-Se0/0/0`), and RouterOS lets a queue be called anything.
/// The spelling stays one that reads back to exactly the four names it was
/// made from because the separators are found from the outside in:
///
/// * the interface ends at the **first** `|`, so a `|` in an interface name
///   is written `%7C`;
/// * the direction is `in` or `out` and ends at the next `|`;
/// * the class begins after the **last** `/`, so a `/` in a class name is
///   written `%2F`;
/// * the policy is everything in between, exactly as the device spells it,
///   slashes and pipes included.
///
/// A `%` in the interface or the class is written `%25`. Names without those
/// characters — nearly all of them — are spelled as they are.
pub fn instance(interface: &str, direction: Direction, policy: &str, class: &str) -> String {
    format!(
        "{}|{}|{policy}/{}",
        escape(interface, '|'),
        direction.as_str(),
        escape(class, '/')
    )
}

/// The interface, direction, policy and class an instance spells; `None` for
/// text that is not one.
pub fn parse_instance(instance: &str) -> Option<(String, Direction, String, String)> {
    let (interface, rest) = instance.split_once('|')?;
    let (direction, rest) = rest.split_once('|')?;
    let direction = match direction {
        "in" => Direction::In,
        "out" => Direction::Out,
        _ => return None,
    };
    let (policy, class) = rest.rsplit_once('/')?;
    Some((
        unescape(interface),
        direction,
        policy.to_string(),
        unescape(class),
    ))
}

// ------------------------------------------------------------- the counters

/// What tells one class of a device from every other.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClassKey {
    pub interface: String,
    pub direction: Direction,
    pub policy: String,
    pub class: String,
    pub parent: Option<String>,
}

impl ClassKey {
    fn of(class: &QosClass) -> ClassKey {
        ClassKey {
            interface: class.interface.clone(),
            direction: class.direction,
            policy: class.policy.clone(),
            class: class.class.clone(),
            parent: class.parent.clone(),
        }
    }
}

/// The counters of one class as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassCounters {
    pub bytes: Option<Counter>,
    pub packets: Option<Counter>,
    pub dropped: Option<Counter>,
    pub queue_dropped: Option<Counter>,
}

impl ClassCounters {
    fn of(class: &QosClass) -> ClassCounters {
        // Read off the text of a `show` command: no known width, so a
        // counter that went down was cleared, never wrapped.
        ClassCounters {
            bytes: class.bytes.map(Counter::text),
            packets: class.packets.map(Counter::text),
            dropped: class.dropped_packets.map(Counter::text),
            queue_dropped: class.queue_dropped_packets.map(Counter::text),
        }
    }
}

/// What the previous QoS read of one device left behind. In memory only,
/// like the interface counters: a restart of the application starts from a
/// gap rather than from a rate across an unknown stretch.
#[derive(Clone, Debug, Default)]
pub struct QosRuntime {
    /// The device's own notion of time at that read.
    pub time_base: Option<TimeBase>,
    /// The poller's clock at that read ([`crate::clock::Clock::monotonic_ms`]).
    pub read_mono: i64,
    pub classes: HashMap<ClassKey, ClassCounters>,
}

impl QosRuntime {
    /// Drop what the last read left: the next one is a baseline.
    pub fn forget(&mut self) {
        *self = QosRuntime::default();
    }
}

/// Everything one QoS read of one device comes to.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// Every class the device reported, in the stable order.
    pub rows: Vec<QosStatus>,
    /// The samples to write: only for classes with series, only for what
    /// was measured.
    pub measures: Vec<Measure>,
    /// How many classes have series.
    pub series: u64,
    /// More classes could have had series than `qos_max_series` allows.
    pub series_capped: bool,
    /// The device's uptime went backwards since the previous read.
    pub rebooted: bool,
}

/// Drops as a share of the packets offered between two readings.
///
/// `packets` is what the device classified into the class, counted before
/// anything is dropped from it, so the drops are among them. No share is
/// given of nothing offered, nor where more was dropped than offered: that
/// is a packet counter that leaves the drops out, and a percentage built on
/// it would be a guess at what the device means.
fn drop_share(previous: &ClassCounters, current: &ClassCounters) -> Option<f64> {
    let offered = rates::delta(previous.packets?, current.packets?)?;
    let dropped = rates::delta(previous.dropped?, current.dropped?)?;
    (offered > 0 && dropped <= offered).then(|| dropped as f64 / offered as f64 * 100.0)
}

/// A text inside a [`sort_key`]: its bytes, then a 0. No byte of the text is
/// written as a 0 (0 and 1 become two bytes that start with 1), so the 0 ends
/// it unmistakably and a name that is the start of another sorts first, as
/// it does as a string.
fn push_text(key: &mut Vec<u8>, text: &str) {
    for byte in text.bytes() {
        match byte {
            0 => key.extend_from_slice(&[1, 1]),
            1 => key.extend_from_slice(&[1, 2]),
            byte => key.push(byte),
        }
    }
    key.push(0);
}

/// The order classes are held, capped and paged in, as bytes: interface in
/// natural order (`Gi0/2` before `Gi0/10`, the exact spelling settling what
/// that calls equal), direction, policy, class, parent. Two keys compare
/// byte by byte as the classes do by name, so the order is a function of the
/// names alone — the same classes keep their series from one read to the
/// next whatever order the device lists them in — and it can be stored, so
/// that a page carries on from a place among the names rather than from a
/// position, which moves when a class before it comes or goes.
pub fn sort_key(
    interface: &str,
    direction: Direction,
    policy: &str,
    class: &str,
    parent: Option<&str>,
) -> Vec<u8> {
    let (kind, numbers) = ifname::sort_key(interface);
    let mut key = Vec::with_capacity(2 * interface.len() + policy.len() + class.len() + 32);
    push_text(&mut key, &kind);
    for number in numbers {
        // Each number is marked and the list ends in a 0, so a shorter list
        // that is the start of a longer one comes first: `Gi0/1`, `Gi0/1/0`.
        key.push(1);
        key.extend_from_slice(&number.to_be_bytes());
    }
    key.push(0);
    push_text(&mut key, interface);
    key.push(match direction {
        Direction::In => 1,
        Direction::Out => 2,
    });
    push_text(&mut key, policy);
    push_text(&mut key, class);
    match parent {
        None => key.push(0),
        Some(parent) => {
            key.push(1);
            push_text(&mut key, parent);
        }
    }
    key
}

fn key_of(class: &QosClass) -> Vec<u8> {
    sort_key(
        &class.interface,
        class.direction,
        &class.policy,
        &class.class,
        class.parent.as_deref(),
    )
}

/// The classes in [`sort_key`] order.
fn in_stable_order(classes: &[QosClass]) -> Vec<&QosClass> {
    let mut ordered: Vec<&QosClass> = classes.iter().collect();
    ordered.sort_by_cached_key(|class| key_of(class));
    ordered
}

/// The series instance of each class, `None` where it cannot have one.
///
/// A child policy used under two classes of one parent (`class VLAN10` and
/// `class VLAN20`, each with `service-policy PER-VLAN`) puts two classes of
/// the same policy and name on one interface. Those are told apart by the
/// class their policy sits in, written before the policy:
/// `Gi0/1|out|VLAN10/PER-VLAN/VOICE`. Two that even that does not separate
/// get no instance: a series two classes write to is neither's.
fn instances(ordered: &[&QosClass]) -> Vec<Option<String>> {
    let plain: Vec<String> = ordered
        .iter()
        .map(|class| {
            instance(
                &class.interface,
                class.direction,
                &class.policy,
                &class.class,
            )
        })
        .collect();
    let mut shared: HashMap<&str, usize> = HashMap::new();
    for name in &plain {
        *shared.entry(name.as_str()).or_default() += 1;
    }
    let told_apart: Vec<String> = ordered
        .iter()
        .zip(&plain)
        .map(|(class, name)| match &class.parent {
            Some(parent) if shared[name.as_str()] > 1 => instance(
                &class.interface,
                class.direction,
                &format!("{}/{}", escape(parent, '/'), class.policy),
                &class.class,
            ),
            _ => name.clone(),
        })
        .collect();
    let mut taken: HashMap<&str, usize> = HashMap::new();
    for name in &told_apart {
        *taken.entry(name.as_str()).or_default() += 1;
    }
    told_apart
        .iter()
        .map(|name| (taken[name.as_str()] == 1).then(|| name.clone()))
        .collect()
}

/// One QoS read of one device, as a poll hands it over.
#[derive(Clone, Copy, Debug)]
pub struct QosReading<'a> {
    pub device_id: &'a str,
    pub classes: &'a [QosClass],
    /// The device's uptime at this read.
    pub base: TimeBase,
    /// The poller's own clock at this read, one that is never set back.
    pub tick: i64,
    /// The time of day the rows are dated with.
    pub at: i64,
    pub unverified: bool,
}

/// Turn a QoS read plus what the previous one left into rates, and leave the
/// new counters behind for the next.
///
/// The interval is the device's own, by [`rates::interval`]: the same rule
/// the interface rates are computed by.
pub fn apply(runtime: &mut QosRuntime, reading: &QosReading<'_>, max_series: usize) -> Applied {
    let QosReading {
        device_id,
        classes,
        base,
        tick,
        at,
        unverified,
    } = *reading;
    let elapsed = match runtime.time_base {
        Some(previous) => rates::interval(previous, base, tick - runtime.read_mono),
        None => Interval::Unusable,
    };
    let seconds = match elapsed {
        Interval::Elapsed(seconds) => Some(seconds),
        _ => None,
    };

    let ordered = in_stable_order(classes);
    let names = instances(&ordered);
    let mut same: HashMap<ClassKey, usize> = HashMap::new();
    for class in &ordered {
        *same.entry(ClassKey::of(class)).or_default() += 1;
    }

    let mut applied = Applied {
        rebooted: elapsed == Interval::Rebooted,
        ..Applied::default()
    };
    let mut held: HashMap<ClassKey, ClassCounters> = HashMap::new();
    for (class, name) in ordered.iter().zip(names) {
        let key = ClassKey::of(class);
        let current = ClassCounters::of(class);
        // Two rows with one name in every respect: which of them a counter
        // held from last time belongs to is not known, so neither is paired
        // with it and neither leaves one behind.
        let unique = same[&key] == 1;
        let previous = match (unique, seconds) {
            (true, Some(_)) => runtime.classes.get(&key).copied(),
            _ => None,
        };
        let rate = |take: fn(&ClassCounters) -> Option<Counter>| {
            rates::rate(take(&previous?)?, take(&current)?, seconds?)
        };
        let bps = rate(|counters| counters.bytes).map(|octets| octets * 8.0);
        let drop_ps = rate(|counters| counters.dropped);
        let queue_drop_ps = rate(|counters| counters.queue_dropped);
        let drop_pct = previous.and_then(|previous| drop_share(&previous, &current));

        // Whether this row says which of its drops are congestion. A class
        // with a policer and no queue prints the policer's and nothing for
        // a queue it does not have: its drops are accounted for.
        let separated =
            class.queue_dropped_packets.is_some() || class.police_dropped_packets.is_some();
        let dropping = match queue_drop_ps {
            Some(rate) => rate > 0.0,
            None => !separated && drop_ps.is_some_and(|rate| rate > 0.0),
        };

        // The first so many classes that can have series, in the stable
        // order, get them; the rest are in the table only.
        let name = match name {
            Some(name) if (applied.series as usize) < max_series => {
                applied.series += 1;
                Some(name)
            }
            Some(_) => {
                applied.series_capped = true;
                None
            }
            None => None,
        };
        if let Some(name) = &name {
            for (metric, value) in [
                ("qos.bps", bps),
                ("qos.drop_ps", drop_ps),
                ("qos.queue_drop_ps", queue_drop_ps),
                ("qos.drop_pct", drop_pct),
                // A gauge: it needs no interval, and is there from the
                // first read.
                (
                    "qos.queue_len",
                    class.queued_packets.map(|depth| depth as f64),
                ),
            ] {
                // `None` is a gap, not a zero: nothing is written.
                if let Some(value) = value {
                    applied.measures.push(Measure::new(metric, name, value));
                }
            }
        }
        if unique {
            held.insert(key, current);
        }
        applied.rows.push(QosStatus {
            device_id: device_id.to_string(),
            interface: class.interface.clone(),
            direction: class.direction,
            policy: class.policy.clone(),
            class: class.class.clone(),
            parent: class.parent.clone(),
            instance: name,
            rate_bps: class.rate_bps,
            bps,
            drop_ps,
            queue_drop_ps,
            drop_pct,
            queue_len: class.queued_packets,
            packets: class.packets,
            dropped_packets: class.dropped_packets,
            dropping,
            at,
            unverified,
        });
    }

    runtime.classes = held;
    runtime.time_base = Some(base);
    runtime.read_mono = tick;
    applied
}

// ---------------------------------------------------------------- when to ask

/// Whether a platform's profile reads QoS at all.
pub fn reads(platform: &Platform) -> bool {
    collect::supported_kinds(platform).contains(&Kind::Qos)
}

/// Whether the QoS read of a device that can be asked is due.
///
/// Its own cadence, and deliberately blind to whether SNMP answered: SNMP
/// carries no QoS, so a device whose interfaces are polled over SNMP is
/// still asked on its command line this often.
pub fn due(lab: bool, settings: &MonitorSettings, last_mono: Option<i64>, tick: i64) -> bool {
    let cadence = if lab {
        LAB_QOS_INTERVAL_S
    } else {
        settings.qos_interval_s
    };
    last_mono.is_none_or(|last| tick - last >= i64::from(cadence) * 1_000)
}

// ------------------------------------------------------------------ the table

/// A counter as SQLite holds it. Nothing a device prints comes near the top
/// of a signed 64-bit integer; one that did is held at the top, not wrapped
/// to a negative.
fn stored(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Replace everything held about one device's classes.
fn replace(app: &App, device_id: &str, rows: &[QosStatus]) -> AppResult<()> {
    app.store().transaction(|tx| {
        tx.execute(
            "DELETE FROM monitor_qos WHERE device_id = ?1",
            params![device_id],
        )?;
        let mut insert = tx.prepare_cached(
            "INSERT INTO monitor_qos
             (device_id, seq, sort_key, interface, direction, policy, class, parent, instance,
              rate_bps, bps, drop_ps, queue_drop_ps, drop_pct, queue_len, packets,
              dropped_packets, dropping, unverified, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
                     ?17, ?18, ?19, ?20)",
        )?;
        for (seq, status) in rows.iter().enumerate() {
            insert.execute(params![
                device_id,
                seq as i64,
                sort_key(
                    &status.interface,
                    status.direction,
                    &status.policy,
                    &status.class,
                    status.parent.as_deref(),
                ),
                status.interface,
                status.direction.as_str(),
                status.policy,
                status.class,
                status.parent,
                status.instance,
                status.rate_bps.map(stored),
                status.bps,
                status.drop_ps,
                status.queue_drop_ps,
                status.drop_pct,
                status.queue_len.map(stored),
                status.packets.map(stored),
                status.dropped_packets.map(stored),
                status.dropping,
                status.unverified,
                status.at,
            ])?;
        }
        Ok(())
    })
}

/// Take the rates off a device's rows and keep the rows.
///
/// After a read that did not work the classes are still what was last known
/// to be configured, and their counters still what was read at `at`. What
/// they are doing *now* is not known, and a rate left standing would go on
/// saying "dropping 40 a second" for as long as the read kept failing.
fn clear_rates(app: &App, device_id: &str) -> AppResult<()> {
    app.store().with(|conn| {
        conn.execute(
            "UPDATE monitor_qos
             SET bps = NULL, drop_ps = NULL, queue_drop_ps = NULL, drop_pct = NULL, dropping = 0
             WHERE device_id = ?1",
            params![device_id],
        )
    })?;
    Ok(())
}

/// Forget a device's classes (it left the inventory, or can no longer be asked).
pub fn forget(app: &App, device_id: &str) -> AppResult<()> {
    app.store().with(|conn| {
        conn.execute(
            "DELETE FROM monitor_qos WHERE device_id = ?1",
            params![device_id],
        )
    })?;
    Ok(())
}

// ------------------------------------------------------- what a poll learned

/// How the QoS part of one poll ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The device was read.
    Read {
        applied: Applied,
        /// What the read could not place and left out (a row that does not
        /// say which way it faces), in the collector's words.
        left_out: Option<String>,
        unverified: bool,
    },
    /// The device was asked and the read did not work.
    Failed { error: String, unverified: bool },
    /// The read was due and the device was not answering, so it was not
    /// asked.
    NotReached { error: String, unverified: bool },
    /// The device cannot be asked (any more): its profile has no QoS read,
    /// or it has no command line.
    Unsupported,
}

/// Write down what a poll learned: the table, and the device's record.
/// Returns whether what `monitor.qos` answers for the device changed.
pub fn settle(
    app: &App,
    record: &mut QosRecord,
    device_id: &str,
    at: i64,
    outcome: &Outcome,
) -> AppResult<bool> {
    match outcome {
        Outcome::Read {
            applied,
            left_out,
            unverified,
        } => {
            replace(app, device_id, &applied.rows)?;
            *record = QosRecord {
                state: Some(QosState::Ok),
                at: Some(at),
                error: left_out.clone(),
                unverified: *unverified,
                series_capped: applied.series_capped,
                classes: applied.rows.len() as u64,
                series: applied.series,
            };
            Ok(true)
        }
        Outcome::Failed { error, unverified } => {
            clear_rates(app, device_id)?;
            record.state = Some(QosState::Failed);
            record.at = Some(at);
            record.error = Some(error.clone());
            record.unverified = *unverified;
            Ok(true)
        }
        Outcome::NotReached { error, unverified } => {
            // A device that is down is asked nothing at every poll until it
            // is back. That is one change — the rates it had are no longer
            // what it is doing — and not one a minute.
            if record.state != Some(QosState::Ok) {
                return Ok(false);
            }
            clear_rates(app, device_id)?;
            record.state = Some(QosState::Failed);
            record.at = Some(at);
            record.error = Some(error.clone());
            record.unverified = *unverified;
            Ok(true)
        }
        Outcome::Unsupported => {
            if record.state.is_none() {
                return Ok(false);
            }
            forget(app, device_id)?;
            *record = QosRecord::default();
            Ok(true)
        }
    }
}

// ----------------------------------------------------------------- monitor.qos

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QosQuery {
    pub device_id: Option<String>,
    /// Only the classes of policies attached to this interface, by any
    /// spelling of its name. Empty: the classes attached to no interface.
    pub interface: Option<String>,
    /// Only classes that are losing packets to congestion (see
    /// [`QosStatus::dropping`]).
    pub problem_only: bool,
    pub limit: Option<u32>,
    /// `next_cursor` of the page before.
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QosPage {
    pub rows: Vec<QosStatus>,
    /// What to pass as `cursor` for the rows after these; `null` after the
    /// last.
    pub next_cursor: Option<String>,
    pub coverage: QosCoverage,
}

/// Where a page stopped: its last row's device and place among the device's
/// classes ([`sort_key`]), and its position, which tells apart two classes
/// nothing else does. The place, not the position, is what the next page
/// carries on from: the table is replaced at every read, and a class that
/// came or went before the place between two pages moves every position
/// after it. The device id goes last because it is not ours to constrain;
/// the rest holds no `@`.
fn cursor_of(device_id: &str, key: &[u8], seq: i64) -> String {
    format!("{seq}.{}@{device_id}", URL_SAFE_NO_PAD.encode(key))
}

/// A cursor as `(device_id, sort_key, seq)`.
type Cursor = (String, Vec<u8>, i64);

fn parse_cursor(cursor: &str) -> AppResult<Cursor> {
    let invalid = || AppError::invalid("the cursor is the `next_cursor` of the page before");
    let (place, device_id) = cursor.split_once('@').ok_or_else(invalid)?;
    let (seq, key) = place.split_once('.').ok_or_else(invalid)?;
    let seq = seq.parse::<i64>().map_err(|_| invalid())?;
    let key = URL_SAFE_NO_PAD.decode(key).map_err(|_| invalid())?;
    Ok((device_id.to_string(), key, seq))
}

/// One page of the table, in device and then stable class order.
pub fn rows(app: &App, query: &QosQuery) -> AppResult<(Vec<QosStatus>, Option<String>)> {
    let limit = query.limit.unwrap_or(200).clamp(1, 5_000) as usize;
    let after: Option<Cursor> = match query.cursor.as_deref() {
        Some(cursor) if !cursor.is_empty() => Some(parse_cursor(cursor)?),
        _ => None,
    };
    // The table holds the name as the collector spells it; a caller may hold
    // the short form (`Gi0/1`). Both are tried, and the name as given first,
    // because the long form of a name that is not an abbreviation is a guess.
    let long = query.interface.as_deref().map(ifname::long);

    let mut sql = String::from(
        "SELECT device_id, seq, interface, direction, policy, class, parent, instance, rate_bps,
                bps, drop_ps, queue_drop_ps, drop_pct, queue_len, packets, dropped_packets,
                dropping, unverified, at, sort_key
         FROM monitor_qos WHERE 1 = 1",
    );
    if query.device_id.is_some() {
        sql.push_str(" AND device_id = :device_id");
    }
    if query.interface.is_some() {
        sql.push_str(
            " AND (interface = :interface COLLATE NOCASE OR interface = :long COLLATE NOCASE)",
        );
    }
    if query.problem_only {
        sql.push_str(" AND dropping = 1");
    }
    if after.is_some() {
        sql.push_str(" AND (device_id, sort_key, seq) > (:after_device, :after_key, :after_seq)");
    }
    sql.push_str(" ORDER BY device_id, sort_key, seq LIMIT :limit");

    // One row more than the page, to know whether there is another.
    let fetch = limit as i64 + 1;
    let mut found: Vec<(i64, Vec<u8>, QosStatus)> = app.store().with(|conn| {
        let mut statement = conn.prepare_cached(&sql)?;
        let mut bound: Vec<(&str, &dyn rusqlite::ToSql)> = vec![(":limit", &fetch)];
        if let Some(device_id) = &query.device_id {
            bound.push((":device_id", device_id));
        }
        if let (Some(interface), Some(long)) = (&query.interface, &long) {
            bound.push((":interface", interface));
            bound.push((":long", long));
        }
        if let Some((device_id, key, seq)) = &after {
            bound.push((":after_device", device_id));
            bound.push((":after_key", key));
            bound.push((":after_seq", seq));
        }
        statement
            .query_map(bound.as_slice(), row_to_status)?
            .collect()
    })?;
    let more = found.len() > limit;
    found.truncate(limit);
    let next_cursor = match found.last() {
        Some((seq, key, last)) if more => Some(cursor_of(&last.device_id, key, *seq)),
        _ => None,
    };
    Ok((
        found.into_iter().map(|(_, _, status)| status).collect(),
        next_cursor,
    ))
}

fn row_to_status(row: &Row<'_>) -> rusqlite::Result<(i64, Vec<u8>, QosStatus)> {
    let count = |index: usize| -> rusqlite::Result<Option<u64>> {
        Ok(row
            .get::<_, Option<i64>>(index)?
            .map(|value| value.max(0) as u64))
    };
    Ok((
        row.get(1)?,
        row.get(19)?,
        QosStatus {
            device_id: row.get(0)?,
            interface: row.get(2)?,
            // Only ever written by `replace`, from a `Direction`.
            direction: match row.get::<_, String>(3)?.as_str() {
                "out" => Direction::Out,
                _ => Direction::In,
            },
            policy: row.get(4)?,
            class: row.get(5)?,
            parent: row.get(6)?,
            instance: row.get(7)?,
            rate_bps: count(8)?,
            bps: row.get(9)?,
            drop_ps: row.get(10)?,
            queue_drop_ps: row.get(11)?,
            drop_pct: row.get(12)?,
            queue_len: count(13)?,
            packets: count(14)?,
            dropped_packets: count(15)?,
            dropping: row.get(16)?,
            unverified: row.get(17)?,
            at: row.get(18)?,
        },
    ))
}

/// What one device's rows, or their absence, mean.
fn coverage_of(app: &App, device: &Device, record: Option<&QosRecord>) -> QosDeviceCoverage {
    let platform = (device.transport != Transport::None)
        .then(|| profiles::registry(app).get(&device.profile))
        .flatten()
        .filter(|platform| reads(platform));
    let Some(platform) = platform else {
        return QosDeviceCoverage {
            device_id: device.id.clone(),
            state: QosState::Unsupported,
            ..QosDeviceCoverage::default()
        };
    };
    match record.and_then(|record| record.state.map(|state| (state, record))) {
        Some((state, record)) => QosDeviceCoverage {
            device_id: device.id.clone(),
            state,
            at: record.at,
            unverified: record.unverified,
            series_capped: record.series_capped,
            error: record.error.clone(),
            classes: record.classes,
            series: record.series,
        },
        None => QosDeviceCoverage {
            device_id: device.id.clone(),
            state: QosState::Never,
            // Said before anything was read, so that a screen can say it
            // above an empty table too.
            unverified: platform.profile.is_unverified(Kind::Qos),
            ..QosDeviceCoverage::default()
        },
    }
}

/// The coverage of a query: the device it names, or every device monitoring
/// watches and any other it still holds a QoS read of.
pub fn coverage(app: &App, device_id: Option<&str>) -> AppResult<QosCoverage> {
    let devices = match device_id {
        Some(device_id) => {
            let device = inventory::get(app, device_id)?;
            let record: Option<DeviceRecord> = app.store().get(collections::DEVICES, device_id)?;
            vec![coverage_of(
                app,
                &device,
                record.as_ref().map(|record| &record.qos),
            )]
        }
        None => {
            let records: HashMap<String, DeviceRecord> = app
                .store()
                .list::<DeviceRecord>(collections::DEVICES)?
                .into_iter()
                .map(|record| (record.device_id.clone(), record))
                .collect();
            inventory::list(app, &inventory::DeviceFilter::default())?
                .iter()
                .filter_map(|row| {
                    let record = records.get(&row.device.id).map(|record| &record.qos);
                    let read_before = record.is_some_and(|record| record.state.is_some());
                    (row.device.monitor.enabled || read_before)
                        .then(|| coverage_of(app, &row.device, record))
                })
                .collect()
        }
    };
    Ok(QosCoverage { devices })
}

pub fn page(app: &App, query: &QosQuery) -> AppResult<QosPage> {
    // `not_found` for a device that does not exist, before anything else.
    let coverage = coverage(app, query.device_id.as_deref())?;
    let (rows, next_cursor) = rows(app, query)?;
    Ok(QosPage {
        rows,
        next_cursor,
        coverage,
    })
}

/// Tell whoever is showing the table that it changed.
pub fn announce(app: &App, device_id: &str) {
    app.publish("monitor.qos.updated", &json!({"device_id": device_id}));
}

pub fn register(router: &mut Router) {
    router.add("monitor.qos", |app: App, params: QosQuery| async move {
        page(&app, &params)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(interface: &str, direction: Direction, policy: &str, name: &str) -> QosClass {
        QosClass {
            interface: interface.into(),
            direction,
            policy: policy.into(),
            class: name.into(),
            ..QosClass::default()
        }
    }

    #[test]
    fn an_ordinary_class_is_spelled_as_the_specification_spells_it() {
        assert_eq!(
            instance("GigabitEthernet0/0", Direction::Out, "WAN-QUEUE", "VOICE"),
            "GigabitEthernet0/0|out|WAN-QUEUE/VOICE"
        );
        // A RouterOS queue on addresses has no interface, and says so by
        // leaving the place empty.
        assert_eq!(
            instance("", Direction::In, "queue simple", "office"),
            "|in|queue simple/office"
        );
        // A policy named after an interface keeps its slashes.
        assert_eq!(
            instance(
                "Serial0/0/0",
                Direction::Out,
                "AutoQoS-Policy-Se0/0/0",
                "class-default"
            ),
            "Serial0/0/0|out|AutoQoS-Policy-Se0/0/0/class-default"
        );
    }

    #[test]
    fn pipes_and_slashes_in_names_read_back_as_the_names_they_were() {
        // Every separator, in every name, and the escape character itself.
        let awkward = [
            ("ether1|wan", "up/down", "gold/silver"),
            ("a|b|c", "p|q", "x|y"),
            ("50%", "100%/policy", "50%/50%"),
            ("", "", ""),
            ("vlan10@bond1", "queue tree", "voip %2F"),
            ("%7C", "|out|", "/"),
            ("in", "out", "in|out"),
        ];
        for (interface, policy, name) in awkward {
            for direction in [Direction::In, Direction::Out] {
                let spelled = instance(interface, direction, policy, name);
                assert_eq!(
                    parse_instance(&spelled),
                    Some((
                        interface.to_string(),
                        direction,
                        policy.to_string(),
                        name.to_string()
                    )),
                    "{spelled}"
                );
            }
        }
        // And two classes that differ only in where the separator falls are
        // two instances.
        assert_ne!(
            instance("e1", Direction::Out, "a/b", "c"),
            instance("e1", Direction::Out, "a", "b/c")
        );
        assert_ne!(
            instance("e1|out", Direction::Out, "p", "c"),
            instance("e1", Direction::Out, "out|p", "c")
        );
        assert_eq!(parse_instance("wan-uplink"), None);
        assert_eq!(parse_instance("Gi0/1|sideways|P/C"), None);
    }

    #[test]
    fn the_order_is_by_name_whatever_order_the_device_lists_in() {
        let listed = vec![
            class("GigabitEthernet0/10", Direction::Out, "WAN", "VOICE"),
            class("GigabitEthernet0/2", Direction::Out, "WAN", "class-default"),
            class("GigabitEthernet0/2", Direction::Out, "WAN", "VOICE"),
            class("GigabitEthernet0/2", Direction::In, "LAN", "GUEST"),
            class("", Direction::Out, "queue tree", "voip"),
        ];
        let names = |classes: &[QosClass]| -> Vec<String> {
            in_stable_order(classes)
                .iter()
                .map(|class| {
                    instance(
                        &class.interface,
                        class.direction,
                        &class.policy,
                        &class.class,
                    )
                })
                .collect()
        };
        let expected = [
            "|out|queue tree/voip",
            "GigabitEthernet0/2|in|LAN/GUEST",
            "GigabitEthernet0/2|out|WAN/VOICE",
            "GigabitEthernet0/2|out|WAN/class-default",
            "GigabitEthernet0/10|out|WAN/VOICE",
        ];
        assert_eq!(names(&listed), expected);
        let mut reversed = listed.clone();
        reversed.reverse();
        assert_eq!(names(&reversed), expected);
    }

    /// A read of `classes` at `uptime_s` of device uptime, printed to the
    /// second, on a poller whose clock agrees.
    fn read(runtime: &mut QosRuntime, classes: &[QosClass], uptime_s: u64) -> Applied {
        apply(
            runtime,
            &QosReading {
                device_id: "d1",
                classes,
                base: TimeBase::Seconds(uptime_s),
                tick: uptime_s as i64 * 1_000,
                at: uptime_s as i64 * 1_000,
                unverified: false,
            },
            256,
        )
    }

    fn counted(mut class: QosClass, packets: u64, bytes: u64, dropped: u64) -> QosClass {
        class.packets = Some(packets);
        class.bytes = Some(bytes);
        class.dropped_packets = Some(dropped);
        class.queue_dropped_packets = Some(dropped);
        class
    }

    #[test]
    fn a_child_policy_used_twice_on_one_interface_is_two_classes_with_two_series() {
        let voice_under = |parent: &str, packets: u64| QosClass {
            parent: Some(parent.into()),
            ..counted(
                class("GigabitEthernet0/1", Direction::Out, "PER-VLAN", "VOICE"),
                packets,
                packets * 100,
                0,
            )
        };
        let mut runtime = QosRuntime::default();
        let first = read(
            &mut runtime,
            &[voice_under("VLAN10", 1_000), voice_under("VLAN20", 5_000)],
            1_000,
        );
        assert_eq!(
            first
                .rows
                .iter()
                .map(|row| row.instance.as_deref())
                .collect::<Vec<_>>(),
            [
                Some("GigabitEthernet0/1|out|VLAN10/PER-VLAN/VOICE"),
                Some("GigabitEthernet0/1|out|VLAN20/PER-VLAN/VOICE")
            ],
            "told apart by the class their policy sits in"
        );
        // Each is subtracted from its own earlier reading, not the other's.
        let second = read(
            &mut runtime,
            &[voice_under("VLAN20", 5_300), voice_under("VLAN10", 1_600)],
            1_301,
        );
        assert_eq!(
            second.rows.iter().map(|row| row.bps).collect::<Vec<_>>(),
            [Some(60_000.0 / 301.0 * 8.0), Some(30_000.0 / 301.0 * 8.0)],
            "600 packets of 100 bytes under VLAN10, 300 under VLAN20, in 301 s"
        );
    }

    #[test]
    fn where_snmp_gave_the_uptime_the_interval_is_measured_in_its_ticks() {
        // A device whose interfaces are polled over SNMP: the QoS read is a
        // CLI session, and the device's time is the `sysUpTime` the same
        // poll read. 300 s of it passed; the poller, held up, counted 330.
        let voice = |packets: u64| {
            counted(
                class("GigabitEthernet0/1", Direction::Out, "WAN", "VOICE"),
                packets,
                packets * 100,
                packets / 100,
            )
        };
        let mut runtime = QosRuntime::default();
        let mut timed = |base: TimeBase, tick: i64, packets: u64| {
            apply(
                &mut runtime,
                &QosReading {
                    device_id: "d1",
                    classes: &[voice(packets)],
                    base,
                    tick,
                    at: tick,
                    unverified: false,
                },
                256,
            )
        };
        timed(TimeBase::Ticks(1_000_000), 0, 10_000);
        let second = timed(TimeBase::Ticks(1_030_000), 330_000, 40_000);
        let row = &second.rows[0];
        assert_eq!(row.bps, Some(3_000_000.0 / 300.0 * 8.0));
        assert_eq!(row.drop_ps, Some(1.0));
        assert_eq!(row.queue_drop_ps, Some(1.0));
        assert_eq!(row.drop_pct, Some(1.0));

        // The agent restarted with the device: ticks went backwards.
        let third = timed(TimeBase::Ticks(3_000), 660_000, 400);
        assert!(third.rebooted);
        assert_eq!(third.rows[0].bps, None);
        assert!(third.measures.is_empty(), "{:?}", third.measures);

        // And a read timed by the CLI's uptime after one timed by SNMP's
        // (the agent stopped answering) is not comparable with it.
        let mixed = timed(TimeBase::Seconds(700), 990_000, 30_400);
        assert!(!mixed.rebooted);
        assert_eq!(mixed.rows[0].bps, None);
    }

    #[test]
    fn a_share_is_of_packets_offered_and_of_nothing_is_no_share() {
        let reading = |packets: u64, dropped: u64| ClassCounters {
            packets: Some(Counter::text(packets)),
            dropped: Some(Counter::text(dropped)),
            ..ClassCounters::default()
        };
        assert_eq!(
            drop_share(&reading(1_000, 10), &reading(3_000, 60)),
            Some(2.5)
        );
        assert_eq!(
            drop_share(&reading(1_000, 10), &reading(3_000, 10)),
            Some(0.0)
        );
        // An idle class: nothing offered, so nothing to take a share of.
        assert_eq!(drop_share(&reading(1_000, 10), &reading(1_000, 10)), None);
        // More dropped than offered: the packet counter leaves drops out,
        // and what share that makes is not this code's to guess.
        assert_eq!(drop_share(&reading(1_000, 10), &reading(1_100, 510)), None);
        // A counter cleared, or one the platform does not print.
        assert_eq!(drop_share(&reading(1_000, 10), &reading(50, 0)), None);
        let no_drops = ClassCounters {
            packets: Some(Counter::text(5)),
            ..ClassCounters::default()
        };
        assert_eq!(drop_share(&no_drops, &reading(3_000, 60)), None);
    }

    #[test]
    fn two_classes_nothing_tells_apart_get_no_rate_and_no_series() {
        // The same policy under a class of the same name twice: three
        // levels, with only the innermost parent on the row.
        let twin = |packets: u64| QosClass {
            parent: Some("class-default".into()),
            ..counted(
                class("GigabitEthernet0/1", Direction::Out, "INNER", "VOICE"),
                packets,
                packets * 100,
                0,
            )
        };
        let other = counted(
            class("GigabitEthernet0/1", Direction::Out, "INNER", "DATA"),
            10,
            1_000,
            0,
        );
        let mut runtime = QosRuntime::default();
        read(&mut runtime, &[twin(100), twin(900), other.clone()], 1_000);
        let second = read(
            &mut runtime,
            &[twin(200), twin(1_000), counted(other, 310, 31_000, 0)],
            1_301,
        );
        assert_eq!(second.rows.len(), 3, "the table still holds all three");
        for row in &second.rows[1..] {
            assert_eq!(row.class, "VOICE");
            assert_eq!(row.instance, None);
            assert_eq!(row.bps, None, "which 100 became which 200?");
            assert!(row.packets.is_some(), "the counters as read");
        }
        assert_eq!(second.rows[0].class, "DATA");
        assert_eq!(second.rows[0].bps, Some(30_000.0 / 301.0 * 8.0));
        assert!(
            second
                .measures
                .iter()
                .all(|measure| measure.instance.ends_with("/DATA")),
            "{:?}",
            second.measures
        );
        assert_eq!((second.series, second.series_capped), (1, false));
    }

    #[test]
    fn the_qos_read_is_due_on_its_own_cadence_and_sooner_in_a_lab() {
        let settings = MonitorSettings::default();
        // Never read: due at once.
        assert!(due(false, &settings, None, 0));
        // A device on the real network: every five minutes, and not before.
        assert!(!due(false, &settings, Some(1_000), 1_000 + 299_999));
        assert!(due(false, &settings, Some(1_000), 1_000 + 300_000));
        // A lab device: every thirty seconds.
        assert!(!due(true, &settings, Some(1_000), 1_000 + 29_999));
        assert!(due(true, &settings, Some(1_000), 1_000 + 30_000));
        // And as often as the settings say.
        let hourly = MonitorSettings {
            qos_interval_s: 3_600,
            ..MonitorSettings::default()
        };
        assert!(!due(false, &hourly, Some(0), 3_599_000));
        assert!(due(false, &hourly, Some(0), 3_600_000));
        assert!(due(true, &hourly, Some(0), 30_000), "a lab keeps its own");
    }

    #[test]
    fn a_cursor_reads_back_whatever_the_device_id_and_the_names_hold() {
        let awkward = sort_key("a|b/c@d", Direction::Out, "p.q@r", "\0\u{1}%", Some("x"));
        for device_id in ["piu6a4nkhh2a6oyu", "a@b", "7@", "", "a.b"] {
            for key in [Vec::new(), vec![0, 1, 255, b'@', b'.'], awkward.clone()] {
                assert_eq!(
                    parse_cursor(&cursor_of(device_id, &key, 41)).unwrap(),
                    (device_id.to_string(), key.clone(), 41)
                );
            }
        }
        for bad in ["later", "@", "x@dev", "12", "x.AAAA@dev", "1.!!@dev"] {
            assert_eq!(parse_cursor(bad).unwrap_err().code, "invalid", "{bad}");
        }
    }

    #[test]
    fn the_stored_order_is_the_order_of_the_names() {
        // What `sort_key` must agree with, written out as the comparison it
        // stands for: natural interface order, the exact spelling, then the
        // other names as strings, no parent before any.
        let by_names = |class: &QosClass| {
            (
                ifname::sort_key(&class.interface),
                class.interface.clone(),
                class.direction,
                class.policy.clone(),
                class.class.clone(),
                class.parent.clone(),
            )
        };
        let mut classes = Vec::new();
        for interface in [
            "",
            "Gi0/1",
            "Gi0/1/0",
            "Gi0/1.100",
            "gi0/1",
            "GigabitEthernet0/10",
            "Gi0/2",
            "ether1",
            "ether10",
            "ether2",
            "a|b",
            "Gi0/18446744073709551615",
        ] {
            for direction in [Direction::In, Direction::Out] {
                for policy in ["", "P", "P\0", "P\u{1}", "P\u{1}x", "PQ", "P/Q"] {
                    for name in ["", "VOICE", "VOICE\0", "class-default"] {
                        for parent in [None, Some(""), Some("class-default")] {
                            classes.push(QosClass {
                                parent: parent.map(str::to_string),
                                ..class(interface, direction, policy, name)
                            });
                        }
                    }
                }
            }
        }
        let mut expected = classes.clone();
        expected.sort_by_key(by_names);
        let stored: Vec<QosClass> = in_stable_order(&classes).into_iter().cloned().collect();
        assert_eq!(stored, expected);
        // And no two classes with different names share a key.
        let keys: std::collections::HashSet<Vec<u8>> = classes.iter().map(key_of).collect();
        assert_eq!(keys.len(), classes.len());
    }
}
