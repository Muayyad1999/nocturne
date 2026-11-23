//! Reading a device: what SNMP and the CLI each hand back, in one shape.
//!
//! Nothing here computes a rate — it only reports what the device said, so
//! that the counter arithmetic lives in one place ([`crate::rates`]) and is
//! the same whether the numbers came off an SNMP table or a `show interfaces`.

use std::collections::HashMap;
use std::time::Duration;

use nocturne_app::model::Device;
use nocturne_app::{App, AppResult, collect, connect, profiles};
use nocturne_drivers::model::{Health, Interface, Kind, QosClass};
use nocturne_drivers::profile::{Combine, InterfaceNames, Platform, SnmpEntry, SnmpSpec, SnmpTest};
use nocturne_snmp::{Oid, Value, oids};

use crate::rates::{Counter, TimeBase};
use crate::settings::{QOS_SESSION_WAIT_S, SNMP_TABLE_LIMIT, SNMP_TIMEOUT_S};
use crate::snmp::{self, SharedSnmp, SnmpSession, TableRow};
use crate::state::IfCounters;

/// One interface exactly as the device reported it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawIf {
    pub name: String,
    pub description: String,
    pub admin_up: Option<bool>,
    pub oper_up: Option<bool>,
    pub speed_bps: Option<u64>,
    pub counters: IfCounters,
}

/// Everything one read of a device produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// The device's own clock, against which counter deltas are measured.
    pub time_base: Option<TimeBase>,
    pub uptime_s: Option<u64>,
    pub interfaces: Vec<RawIf>,
    /// Whether the interface table was read at all (an empty table and a
    /// table that could not be read are different things).
    pub interfaces_read: bool,
    pub cpu_pct: Option<f64>,
    pub mem_pct: Option<f64>,
    /// Temperature per sensor; the name is the sensor's table index, or the
    /// entry's `label` with it.
    pub temperatures: Vec<(String, f64)>,
    /// Why a health figure the profile asks for was not made although the
    /// agent answered, one sentence each: a value that cannot be what the
    /// profile says, a table too long to read, two sensors of one name. An
    /// object that is simply not there is not a problem: that figure is
    /// unknown, as it always was.
    pub health_problems: Vec<String>,
}

// ------------------------------------------------------------------- SNMP

/// The IF-MIB columns one poll asks for.
struct IfColumns {
    name: Oid,
    descr: Oid,
    alias: Oid,
    admin: Oid,
    oper: Oid,
    high_speed: Oid,
    speed: Oid,
    hc_in: Oid,
    hc_out: Oid,
    in_octets: Oid,
    out_octets: Oid,
    in_errors: Oid,
    out_errors: Oid,
    in_discards: Oid,
    out_discards: Oid,
}

impl IfColumns {
    fn new() -> IfColumns {
        IfColumns {
            name: snmp::oid(oids::IF_NAME),
            descr: snmp::oid(oids::IF_DESCR),
            alias: snmp::oid(oids::IF_ALIAS),
            admin: snmp::oid(oids::IF_ADMIN_STATUS),
            oper: snmp::oid(oids::IF_OPER_STATUS),
            high_speed: snmp::oid(oids::IF_HIGH_SPEED),
            speed: snmp::oid(oids::IF_SPEED),
            hc_in: snmp::oid(oids::IF_HC_IN_OCTETS),
            hc_out: snmp::oid(oids::IF_HC_OUT_OCTETS),
            in_octets: snmp::oid(oids::IF_IN_OCTETS),
            out_octets: snmp::oid(oids::IF_OUT_OCTETS),
            in_errors: snmp::oid(oids::IF_IN_ERRORS),
            out_errors: snmp::oid(oids::IF_OUT_ERRORS),
            in_discards: snmp::oid(oids::IF_IN_DISCARDS),
            out_discards: snmp::oid(oids::IF_OUT_DISCARDS),
        }
    }

    /// The columns to walk. The 64-bit counters are left out on SNMPv1,
    /// which has no Counter64 at all.
    fn to_walk(&self, counter64: bool) -> Vec<Oid> {
        let mut columns = vec![
            self.name.clone(),
            self.descr.clone(),
            self.alias.clone(),
            self.admin.clone(),
            self.oper.clone(),
            self.high_speed.clone(),
            self.speed.clone(),
            self.in_octets.clone(),
            self.out_octets.clone(),
            self.in_errors.clone(),
            self.out_errors.clone(),
            self.in_discards.clone(),
            self.out_discards.clone(),
        ];
        if counter64 {
            columns.push(self.hc_in.clone());
            columns.push(self.hc_out.clone());
        }
        columns
    }
}

/// Read a device over SNMP.
pub async fn over_snmp(
    app: &App,
    device: &Device,
    transport: &SharedSnmp,
    sessions: &snmp::Sessions,
) -> Result<Reading, String> {
    let target = snmp::target(app, device).map_err(|err| err.message)?;
    let mut lease = snmp::acquire(
        sessions,
        transport,
        &device.id,
        &target,
        Duration::from_secs(SNMP_TIMEOUT_S),
    )
    .await?;
    let platform = profiles::registry(app).get(&device.profile);
    let outcome = read_snmp(lease.session(), platform.as_deref()).await;
    if outcome.is_err() {
        // Anything from a timeout to a changed engine id: start afresh next time.
        lease.discard();
    }
    outcome
}

async fn read_snmp(
    session: &mut dyn SnmpSession,
    platform: Option<&Platform>,
) -> Result<Reading, String> {
    let uptime_oid = snmp::oid(oids::SYS_UP_TIME);
    let uptime = session.get(std::slice::from_ref(&uptime_oid)).await?;
    let ticks = uptime
        .iter()
        .find(|bind| bind.oid == uptime_oid)
        .and_then(|bind| match &bind.value {
            Value::TimeTicks(ticks) => Some(*ticks),
            other => other.as_u64().map(|value| value as u32),
        });

    let columns = IfColumns::new();
    let rows = session
        .table(
            &columns.to_walk(session.supports_counter64()),
            SNMP_TABLE_LIMIT,
        )
        .await?;
    let canonical =
        platform.is_none_or(|platform| platform.profile.interface_names == InterfaceNames::Cisco);
    let interfaces = rows
        .iter()
        .map(|row| interface_of(row, &columns, canonical))
        .collect();

    let mut reading = Reading {
        time_base: ticks.map(TimeBase::Ticks),
        uptime_s: ticks.map(|ticks| u64::from(ticks) / 100),
        interfaces,
        interfaces_read: true,
        ..Reading::default()
    };
    if let Some(platform) = platform {
        health_over_snmp(session, platform, &mut reading).await?;
    }
    Ok(reading)
}

fn interface_of(row: &TableRow, columns: &IfColumns, canonical: bool) -> RawIf {
    let index = row
        .index
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".");
    let reported = row
        .text(&columns.name)
        .filter(|name| !name.trim().is_empty())
        .or_else(|| row.text(&columns.descr))
        .filter(|name| !name.trim().is_empty());
    // Only a name the agent actually gave is canonicalised; a stand-in built
    // from the row index is left exactly as written.
    let name = match reported {
        Some(reported) if canonical => nocturne_drivers::ifname::long(reported.trim()),
        Some(reported) => reported.trim().to_string(),
        None => format!("ifIndex {index}"),
    };
    // ifHighSpeed is in megabits; ifSpeed saturates at 4.29 Gbit/s, which is
    // why anything modern reports the former.
    let speed_bps = row
        .u64(&columns.high_speed)
        .filter(|speed| *speed > 0)
        .map(|megabits| megabits.saturating_mul(1_000_000))
        .or_else(|| row.u64(&columns.speed).filter(|speed| *speed > 0));
    // 64-bit columns where the agent has them, the 32-bit ones otherwise.
    let octets = |hc: &Oid, plain: &Oid| {
        row.u64(hc)
            .map(Counter::bits64)
            .or_else(|| row.u64(plain).map(Counter::bits32))
    };
    RawIf {
        name,
        description: row
            .text(&columns.alias)
            .map(|alias| alias.trim().to_string())
            .unwrap_or_default(),
        admin_up: row.u64(&columns.admin).map(|status| status == 1),
        oper_up: row.u64(&columns.oper).map(|status| status == 1),
        speed_bps,
        counters: IfCounters {
            in_octets: octets(&columns.hc_in, &columns.in_octets),
            out_octets: octets(&columns.hc_out, &columns.out_octets),
            in_errors: row.u64(&columns.in_errors).map(Counter::bits32),
            out_errors: row.u64(&columns.out_errors).map(Counter::bits32),
            in_discards: row.u64(&columns.in_discards).map(Counter::bits32),
            out_discards: row.u64(&columns.out_discards).map(Counter::bits32),
        },
    }
}

/// CPU, memory and temperatures from the objects the device's profile names.
async fn health_over_snmp(
    session: &mut dyn SnmpSession,
    platform: &Platform,
    reading: &mut Reading,
) -> Result<(), String> {
    read_health(session, &platform.profile.snmp, SNMP_TABLE_LIMIT, reading).await
}

/// [`health_over_snmp`], with the most rows one walk may bring back as a
/// parameter so that a test can reach it.
///
/// Every vendor fact is in `spec`: this function knows SNMP, the arithmetic
/// an entry may ask for, and what cannot be a CPU load, a memory figure or a
/// temperature. An error from the agent (a timeout) fails the read as any
/// other request does; everything else that keeps a figure from being made
/// is a gap with its reason in [`Reading::health_problems`].
async fn read_health(
    session: &mut dyn SnmpSession,
    spec: &SnmpSpec,
    limit: usize,
    reading: &mut Reading,
) -> Result<(), String> {
    let mut problems = Vec::new();
    reading.cpu_pct = cpu_figure(session, &spec.cpu, limit, &mut problems).await?;
    reading.mem_pct = memory_figure(session, spec, limit, &mut problems).await?;
    reading.temperatures = sensors(session, &spec.temperature, limit, &mut problems).await?;
    reading.health_problems = problems;
    Ok(())
}

/// One number an entry gave.
struct Found {
    /// The row index of a column, or empty for one object.
    row: String,
    /// After the entry's arithmetic.
    value: f64,
    /// A Gauge32 at its top, which RFC 2578 §7.1.7 has an agent report for
    /// any amount that large or larger: a floor, not a quantity.
    saturated: bool,
}

/// What reading one entry came to.
enum EntryRead {
    /// The numbers it gave; none when the object is not there or no row
    /// passed its tests.
    Found(Vec<Found>),
    /// The walk brought back as many rows as it may, so the rows that would
    /// have counted may lie past where it stopped.
    Cut,
}

/// How an entry is named in a problem: where it stands in the profile, and
/// what it asks for.
fn entry_name(list: &str, index: usize, entry: &SnmpEntry) -> String {
    match entry.target() {
        Some(oid) => format!("snmp.{list}[{index}] ({})", oid.trim()),
        None => format!("snmp.{list}[{index}]"),
    }
}

async fn read_entry(
    session: &mut dyn SnmpSession,
    entry: &SnmpEntry,
    limit: usize,
) -> Result<EntryRead, String> {
    let nothing = || Ok(EntryRead::Found(Vec::new()));
    let Some(text) = entry.target() else {
        return nothing();
    };
    let Ok(oid) = text.trim().parse::<Oid>() else {
        // Only a bare string can get here (a mapping's OIDs are checked when
        // the profile loads), and a bare string that does not parse has
        // always been skipped rather than allowed to fail the poll.
        tracing::warn!(oid = %text, "ignoring an OID that does not parse");
        return nothing();
    };
    if !entry.is_column() {
        let answer = session.get_each(std::slice::from_ref(&oid)).await?;
        let found = answer.into_iter().flatten().next().and_then(|value| {
            Some(Found {
                row: String::new(),
                value: arithmetic(entry, raw_number(entry, &value)?, None, None)?,
                saturated: !entry.signed && value == Value::Gauge32(u32::MAX),
            })
        });
        return Ok(EntryRead::Found(found.into_iter().collect()));
    }

    // The profile was checked when it loaded, so these parse. If one did
    // not, no row could be judged, and the entry gives nothing.
    let column = |text: &Option<String>| {
        text.as_deref()
            .map(|text| text.trim().parse::<Oid>().map_err(|_| ()))
            .transpose()
    };
    let tests: Option<Vec<(Oid, &SnmpTest)>> = entry
        .tests
        .iter()
        .map(|test| Some((test.oid.trim().parse::<Oid>().ok()?, test)))
        .collect();
    let (Some(tests), Ok(si_scale), Ok(precision)) =
        (tests, column(&entry.si_scale), column(&entry.precision))
    else {
        return nothing();
    };
    // One walk for the value and every column a row is judged by: the
    // session pivots them by index, so each row arrives whole.
    let mut columns = vec![oid.clone()];
    for column in tests
        .iter()
        .map(|(column, _)| column)
        .chain(si_scale.iter())
        .chain(precision.iter())
    {
        if !columns.contains(column) {
            columns.push(column.clone());
        }
    }
    let rows = session.table(&columns, limit).await?;
    if rows.len() >= limit {
        return Ok(EntryRead::Cut);
    }
    let mut found = Vec::new();
    for row in &rows {
        let passes = tests.iter().all(|(column, test)| {
            row.get(column)
                .and_then(plain_text)
                .is_some_and(|text| test.holds(&text))
        });
        if !passes {
            continue;
        }
        let Some(cell) = row.get(&oid) else {
            continue;
        };
        let Some(raw) = raw_number(entry, cell) else {
            continue;
        };
        // A sensor whose scale or precision is missing, or is not one, cannot
        // be read: guessing units would be off by powers of ten.
        let exponent = match &si_scale {
            Some(column) => match row.get(column).and_then(Value::as_i64) {
                Some(scale) if (1..=17).contains(&scale) => Some(scale),
                _ => continue,
            },
            None => None,
        };
        let places = match &precision {
            Some(column) => match row.get(column).and_then(Value::as_i64) {
                Some(places) if (-8..=9).contains(&places) => Some(places),
                _ => continue,
            },
            None => None,
        };
        let Some(value) = arithmetic(entry, raw, exponent, places) else {
            continue;
        };
        found.push(Found {
            row: row
                .index
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join("."),
            value,
            saturated: !entry.signed && *cell == Value::Gauge32(u32::MAX),
        });
    }
    Ok(EntryRead::Found(found))
}

/// A cell as the number the agent meant. Where the entry is `signed`, the
/// agent puts a signed 32-bit object in an unsigned type (RouterOS before
/// 7.12 sends -1 °C as 4294967295), and the bits are read back as what they
/// are, two's complement, rather than refused as an impossible value.
fn raw_number(entry: &SnmpEntry, value: &Value) -> Option<f64> {
    match value {
        Value::Counter32(bits) | Value::Gauge32(bits) | Value::TimeTicks(bits) if entry.signed => {
            Some(f64::from(*bits as i32))
        }
        _ => numeric(value),
    }
}

/// raw (read as signed where the entry says so) → `absent` → `si_scale` →
/// `precision` → `scale` → `complement`. `None` for a raw value the profile
/// says means "no reading".
fn arithmetic(
    entry: &SnmpEntry,
    raw: f64,
    exponent: Option<i64>,
    places: Option<i64>,
) -> Option<f64> {
    if entry.absent.contains(&raw) {
        return None;
    }
    let mut value = raw;
    if let Some(exponent) = exponent {
        // RFC 3433 EntitySensorDataScale: units(9) is 10^0, each step 10^3.
        // Dividing by a power of ten that is exact keeps 41500 milli at 41.5;
        // multiplying by an inexact 10^-3 need not.
        let power = 3 * (exponent as i32 - 9);
        value = if power >= 0 {
            value * 10f64.powi(power)
        } else {
            value / 10f64.powi(-power)
        };
    }
    if let Some(places) = places.filter(|places| *places > 0) {
        // EntitySensorPrecision 1..9: decimal places of a fixed-point
        // number. Zero and below say how many digits are accurate, and
        // move nothing.
        value /= 10f64.powi(places as i32);
    }
    if let Some(scale) = entry.scale {
        value *= scale;
    }
    if let Some(complement) = entry.complement {
        value = complement - value;
    }
    Some(value)
}

/// A cell as a `where` test compares it: integers in decimal, object
/// identifiers dotted with no leading dot, strings trimmed. Anything else
/// has no plain rendering, and fails every test.
fn plain_text(value: &Value) -> Option<String> {
    match value {
        Value::Integer(number) => Some(number.to_string()),
        Value::Counter32(number) | Value::Gauge32(number) | Value::TimeTicks(number) => {
            Some(number.to_string())
        }
        Value::Counter64(number) => Some(number.to_string()),
        Value::Oid(oid) => Some(oid.to_string()),
        Value::IpAddress(_) => Some(value.display()),
        Value::OctetString(_) => value.as_text().map(|text| text.trim().to_string()),
        _ => None,
    }
}

/// `cpu`: the first entry that gives a number is the reading.
async fn cpu_figure(
    session: &mut dyn SnmpSession,
    entries: &[SnmpEntry],
    limit: usize,
    problems: &mut Vec<String>,
) -> Result<Option<f64>, String> {
    for (index, entry) in entries.iter().enumerate() {
        let rows = match read_entry(session, entry, limit).await? {
            EntryRead::Cut => {
                problems.push(format!(
                    "{}: the table has {limit} rows or more, more than one read takes, so no CPU figure was made",
                    entry_name("cpu", index, entry)
                ));
                return Ok(None);
            }
            EntryRead::Found(rows) if rows.is_empty() => continue,
            EntryRead::Found(rows) => rows,
        };
        // Judged row by row: a mean can hide one impossible value among
        // plausible ones.
        if let Some(bad) = rows.iter().find(|row| !(0.0..=100.0).contains(&row.value)) {
            problems.push(format!(
                "{}: {} is {}, which is not a percentage, so no CPU figure was stored",
                entry_name("cpu", index, entry),
                which(&bad.row),
                bad.value
            ));
            return Ok(None);
        }
        let values = rows.iter().map(|row| row.value);
        return Ok(Some(match entry.combine.unwrap_or_default() {
            Combine::Mean => values.sum::<f64>() / rows.len() as f64,
            Combine::Max => values.fold(f64::MIN, f64::max),
        }));
    }
    Ok(None)
}

/// "row 5", or "the value" for one object.
fn which(row: &str) -> String {
    if row.is_empty() {
        "the value".to_string()
    } else {
        format!("row {row}")
    }
}

/// One memory list: the first number of the first entry that gives one.
async fn quantity(
    session: &mut dyn SnmpSession,
    list: &str,
    entries: &[SnmpEntry],
    limit: usize,
    problems: &mut Vec<String>,
) -> Result<Option<f64>, String> {
    for (index, entry) in entries.iter().enumerate() {
        let first = match read_entry(session, entry, limit).await? {
            EntryRead::Cut => {
                problems.push(format!(
                    "{}: the table has {limit} rows or more, more than one read takes, so it gave no memory figure",
                    entry_name(list, index, entry)
                ));
                return Ok(None);
            }
            EntryRead::Found(rows) => match rows.into_iter().next() {
                Some(first) => first,
                None => continue,
            },
        };
        if first.saturated {
            problems.push(format!(
                "{}: {} is 4294967295, the most a Gauge32 holds, which an agent reports for any amount that large or larger; no memory figure was made from it",
                entry_name(list, index, entry),
                which(&first.row)
            ));
            return Ok(None);
        }
        if first.value < 0.0 {
            problems.push(format!(
                "{}: {} is {}, and an amount of memory cannot be negative; no memory figure was made from it",
                entry_name(list, index, entry),
                which(&first.row),
                first.value
            ));
            return Ok(None);
        }
        return Ok(Some(first.value));
    }
    Ok(None)
}

async fn memory_figure(
    session: &mut dyn SnmpSession,
    spec: &SnmpSpec,
    limit: usize,
    problems: &mut Vec<String>,
) -> Result<Option<f64>, String> {
    let used = quantity(session, "memory_used", &spec.memory_used, limit, problems).await?;
    let free = quantity(session, "memory_free", &spec.memory_free, limit, problems).await?;
    let total = quantity(session, "memory_total", &spec.memory_total, limit, problems).await?;
    let from_quantities = match (used, total, free) {
        (Some(used), Some(total), _) if total > 0.0 => Some(used / total * 100.0),
        (Some(used), _, Some(free)) if used + free > 0.0 => Some(used / (used + free) * 100.0),
        (_, Some(total), Some(free)) if total > 0.0 => Some((total - free) / total * 100.0),
        _ => None,
    };
    match from_quantities {
        Some(figure) if (0.0..=100.0).contains(&figure) => return Ok(Some(figure)),
        Some(figure) => problems.push(format!(
            "the memory quantities give {figure:.1} % in use (used {}, free {}, total {}), which cannot be, so no memory figure was made from them",
            shown(used),
            shown(free),
            shown(total)
        )),
        None => {}
    }
    // A ready-made percentage only where the quantities gave no figure:
    // bytes, where an agent has them, say more than its own rounding.
    for (index, entry) in spec.memory_percent.iter().enumerate() {
        let first = match read_entry(session, entry, limit).await? {
            EntryRead::Cut => {
                problems.push(format!(
                    "{}: the table has {limit} rows or more, more than one read takes, so no memory figure was made",
                    entry_name("memory_percent", index, entry)
                ));
                return Ok(None);
            }
            EntryRead::Found(rows) => match rows.into_iter().next() {
                Some(first) => first,
                None => continue,
            },
        };
        if !(0.0..=100.0).contains(&first.value) {
            problems.push(format!(
                "{}: {} is {}, which is not a percentage, so no memory figure was stored",
                entry_name("memory_percent", index, entry),
                which(&first.row),
                first.value
            ));
            return Ok(None);
        }
        return Ok(Some(first.value));
    }
    Ok(None)
}

fn shown(quantity: Option<f64>) -> String {
    quantity.map_or_else(|| "unknown".to_string(), |value| value.to_string())
}

/// `temperature`: every entry is read, and every row it gives is a sensor,
/// except that a `fallback` entry is read only when no entry above it that
/// is not one gave a sensor.
async fn sensors(
    session: &mut dyn SnmpSession,
    entries: &[SnmpEntry],
    limit: usize,
    problems: &mut Vec<String>,
) -> Result<Vec<(String, f64)>, String> {
    // (name, °C, which entry)
    let mut read: Vec<(String, f64, usize)> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let answered_above = read.iter().any(|(_, _, above)| !entries[*above].fallback);
        if entry.fallback && answered_above {
            continue;
        }
        match read_entry(session, entry, limit).await? {
            EntryRead::Cut => problems.push(format!(
                "{}: the table has {limit} rows or more, more than one read takes, so none of its sensors was read",
                entry_name("temperature", index, entry)
            )),
            EntryRead::Found(rows) => {
                for row in rows {
                    let name = match (&entry.label, row.row.is_empty()) {
                        (Some(label), true) => label.clone(),
                        (Some(label), false) => format!("{label}.{}", row.row),
                        (None, _) => row.row,
                    };
                    read.push((name, row.value, index));
                }
            }
        }
    }
    // A name is a series and an alert key. Two readings under one name
    // would be one series that keeps whichever was written last, and one
    // alert raised and resolved by the same poll: neither is kept, whatever
    // their values, so the outcome does not change from poll to poll.
    //
    // Counted in one pass, and said once per group rather than once per
    // sensor: a table of twenty thousand rows read twice, or read in the
    // wrong unit, is one mistake, and comparing every name with every other
    // held the poll for seconds and filled the source's error with megabytes.
    let mut sharing: HashMap<&str, Vec<usize>> = HashMap::new();
    for (name, _, index) in &read {
        sharing.entry(name.as_str()).or_default().push(*index);
    }
    // (the entries that share names, how many names, the first of them)
    let mut collisions: Vec<(Vec<usize>, usize, &str)> = Vec::new();
    // (the entry, how many of its sensors, the first of them and its value)
    let mut refused: Vec<(usize, usize, &str, f64)> = Vec::new();
    let mut kept = Vec::new();
    for (name, celsius, index) in &read {
        let mut sources = sharing[name.as_str()].clone();
        if sources.len() > 1 {
            sources.dedup();
            match collisions.iter_mut().find(|(at, _, _)| *at == sources) {
                // Every name is counted once, at its first reading.
                Some(group) if sharing[name.as_str()][0] == *index => group.1 += 1,
                Some(_) => {}
                None => collisions.push((sources, 1, name.as_str())),
            }
            continue;
        }
        if !(-100.0..=300.0).contains(celsius) {
            match refused.iter_mut().find(|(at, ..)| at == index) {
                Some(group) => group.1 += 1,
                None => refused.push((*index, 1, name.as_str(), *celsius)),
            }
            continue;
        }
        kept.push((name.clone(), *celsius));
    }
    let shown = |name: &str| {
        if name.is_empty() {
            "an unnamed one".to_string()
        } else {
            format!("`{name}`")
        }
    };
    for (at, count, first) in collisions {
        let sources: Vec<String> = at
            .iter()
            .map(|index| entry_name("temperature", *index, &entries[*index]))
            .collect();
        problems.push(if count == 1 {
            format!(
                "{} give a sensor of the same name ({}), so none of them was stored: give the entries distinct `label`s",
                sources.join(" and "),
                shown(first)
            )
        } else {
            format!(
                "{} give {count} sensors of the same names ({} and {} more), so none of them was stored: give the entries distinct `label`s",
                sources.join(" and "),
                shown(first),
                count - 1
            )
        });
    }
    for (index, count, first, celsius) in refused {
        let entry = entry_name("temperature", index, &entries[index]);
        let sensor = if first.is_empty() {
            "(unnamed)".to_string()
        } else {
            format!("`{first}`")
        };
        problems.push(if count == 1 {
            format!("{entry}: sensor {sensor} reads {celsius} °C, which no sensor does; it was not stored")
        } else {
            format!(
                "{entry}: sensor {sensor} reads {celsius} °C, and {} more of its sensors read values no sensor does; none of them was stored",
                count - 1
            )
        });
    }
    Ok(kept)
}

fn numeric(value: &Value) -> Option<f64> {
    if value.is_exception() {
        return None;
    }
    if let Some(number) = value.as_i64() {
        return Some(number as f64);
    }
    // Some agents report gauges as text ("42" or "42 C").
    let text = value.as_text()?;
    let head: String = text
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
        .collect();
    head.parse().ok()
}

// -------------------------------------------------------------------- CLI

/// What one command-line session of a poll is asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Want {
    /// The interface table and the device's health: what SNMP reads where
    /// there is SNMP.
    pub interfaces: bool,
    /// The per-class QoS statistics, which only the command line has.
    pub qos: bool,
    /// The device's uptime, for a QoS read with no other account of the
    /// device's time (`interfaces` reads it anyway; an SNMP poll that
    /// answered has `sysUpTime`).
    pub uptime: bool,
}

/// What a device answered when asked for its QoS statistics.
#[derive(Clone, Debug, PartialEq)]
pub enum QosRead {
    /// The read worked. No classes is an answer: no policy is attached.
    Classes {
        classes: Vec<QosClass>,
        /// What the collector could not place and left out, in its words.
        left_out: Option<String>,
    },
    /// The read did not work, and why.
    Failed(String),
}

/// What one command-line session brought back.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliAnswer {
    /// The interfaces and health, when they were asked for.
    pub reading: Option<Reading>,
    /// What the device said about its uptime in this session, when the
    /// session asked: its own figure, or [`TimeBase::Wall`] where it gives
    /// none.
    pub time_base: Option<TimeBase>,
    /// The QoS statistics, when they were asked for.
    pub qos: Option<QosRead>,
}

/// Read a device over its command line, for devices with no SNMP, for
/// simulated ones, and for what SNMP does not carry. Uses the core
/// collector, so the profile decides which commands are run and everything
/// read is stored for the other features.
///
/// Everything a poll wants goes into this one session: a device is logged
/// in to once, not once per kind.
pub async fn over_cli(app: &App, device_id: &str, want: Want) -> AppResult<CliAnswer> {
    let health_wanted = want.interfaces || want.uptime;
    let mut kinds = Vec::new();
    if want.interfaces {
        kinds.push(Kind::Interfaces);
    }
    if health_wanted {
        kinds.push(Kind::Health);
    }
    if want.qos {
        kinds.push(Kind::Qos);
    }
    // A session that stands in for SNMP is the device's only read, and waits
    // for another automated session on the device (a change, a backup) as
    // long as anything does. One that is only for QoS is an extra in a poll
    // that has usually read the device over SNMP already. Waiting the
    // ordinary minute and a half there would hold that poll's results, its
    // slot and its share of the concurrency behind somebody's change, every
    // time QoS fell due during one; it waits a moment instead, and is
    // otherwise a failed read: a gap, with the reason in the coverage.
    let options = if want.interfaces {
        connect::CliOptions::default()
    } else {
        connect::CliOptions {
            wait: Duration::from_secs(QOS_SESSION_WAIT_S),
            ..connect::CliOptions::default()
        }
    };
    let mut cli = connect::cli(app, device_id, options).await?;
    let snapshot = collect::run_on(app, &mut cli, &kinds).await;
    cli.close().await;
    let snapshot = snapshot?;
    let health: Option<Health> = snapshot
        .rows(Kind::Health)
        .and_then(|rows| typed_rows::<Health>(&rows).into_iter().next());
    let uptime_s = health.as_ref().and_then(|health| health.uptime_s);
    // A device that reports its uptime gives counter deltas a trustworthy
    // base; one that does not leaves only the poller's own clock.
    let time_base = health_wanted.then(|| uptime_s.map_or(TimeBase::Wall, TimeBase::Seconds));

    let reading = want.interfaces.then(|| {
        let interfaces: Option<Vec<Interface>> = snapshot
            .rows(Kind::Interfaces)
            .map(|rows| typed_rows(&rows));
        let mut reading = Reading {
            interfaces_read: interfaces.is_some(),
            interfaces: interfaces
                .unwrap_or_default()
                .into_iter()
                .map(from_model)
                .collect(),
            time_base,
            ..Reading::default()
        };
        if let Some(health) = &health {
            reading.cpu_pct = health.cpu_pct;
            reading.mem_pct = match (health.memory_used, health.memory_total) {
                (Some(used), Some(total)) if total > 0 => Some(used as f64 / total as f64 * 100.0),
                _ => None,
            };
            reading.temperatures = health
                .temperature_c
                .map(|celsius| vec![(String::new(), celsius)])
                .unwrap_or_default();
            reading.uptime_s = health.uptime_s;
        }
        reading
    });

    let qos = want.qos.then(|| {
        // The collector's own account of the kind: why it has no rows, or
        // which rows it dropped from the ones it has.
        let said = snapshot
            .problems
            .iter()
            .filter(|problem| problem.kind == Kind::Qos)
            .map(|problem| problem.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        match &snapshot.qos {
            Some(classes) => QosRead::Classes {
                classes: classes.clone(),
                left_out: (!said.is_empty()).then_some(said),
            },
            None if said.is_empty() => {
                QosRead::Failed("the QoS statistics were asked for and not answered".into())
            }
            None => QosRead::Failed(said),
        }
    });

    Ok(CliAnswer {
        reading,
        time_base,
        qos,
    })
}

fn typed_rows<T: serde::de::DeserializeOwned>(rows: &[serde_json::Value]) -> Vec<T> {
    rows.iter()
        .filter_map(|row| serde_json::from_value(row.clone()).ok())
        .collect()
}

fn from_model(interface: Interface) -> RawIf {
    let counters = &interface.counters;
    RawIf {
        name: interface.name,
        description: interface.description,
        admin_up: interface.admin_up,
        oper_up: interface.oper_up,
        speed_bps: interface.speed_bps,
        counters: IfCounters {
            // A CLI counter is whatever width the device prints, and the
            // text does not say which. With no wrap point of its own, one
            // that goes down is a gap: the device's uptime guards against a
            // restart, but not against `clear counters`.
            in_octets: counters.in_octets.map(Counter::text),
            out_octets: counters.out_octets.map(Counter::text),
            in_errors: counters.in_errors.map(Counter::text),
            out_errors: counters.out_errors.map(Counter::text),
            in_discards: counters.in_discards.map(Counter::text),
            out_discards: counters.out_discards.map(Counter::text),
        },
    }
}

/// The interfaces of a reading as the collected-data store keeps them, so
/// topology and IPAM see the ports of devices that have no CLI.
pub fn as_collected(interfaces: &[RawIf]) -> Vec<serde_json::Value> {
    interfaces
        .iter()
        .map(|raw| {
            let mut interface = Interface {
                name: raw.name.clone(),
                description: raw.description.clone(),
                admin_up: raw.admin_up,
                oper_up: raw.oper_up,
                speed_bps: raw.speed_bps,
                ..Interface::default()
            };
            interface.counters.in_octets = raw.counters.in_octets.map(|counter| counter.value);
            interface.counters.out_octets = raw.counters.out_octets.map(|counter| counter.value);
            interface.counters.in_errors = raw.counters.in_errors.map(|counter| counter.value);
            interface.counters.out_errors = raw.counters.out_errors.map(|counter| counter.value);
            interface.counters.in_discards = raw.counters.in_discards.map(|counter| counter.value);
            interface.counters.out_discards =
                raw.counters.out_discards.map(|counter| counter.value);
            serde_json::to_value(interface).unwrap_or(serde_json::Value::Null)
        })
        .filter(|row| !row.is_null())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeAgentSession;
    use nocturne_snmp::VarBind;

    fn row(index: u32, values: Vec<(Oid, Value)>) -> TableRow {
        TableRow {
            index: vec![index],
            values,
        }
    }

    #[test]
    fn an_interface_row_prefers_ifname_ifalias_and_the_high_speed_column() {
        let columns = IfColumns::new();
        let raw = interface_of(
            &row(
                3,
                vec![
                    (columns.name.clone(), Value::from("Gi0/3")),
                    (columns.descr.clone(), Value::from("GigabitEthernet0/3")),
                    (columns.alias.clone(), Value::from("desk 14")),
                    (columns.admin.clone(), Value::Integer(1)),
                    (columns.oper.clone(), Value::Integer(2)),
                    (columns.high_speed.clone(), Value::Gauge32(1_000)),
                    (columns.speed.clone(), Value::Gauge32(4_294_967_295)),
                    (columns.hc_in.clone(), Value::Counter64(12)),
                    (columns.in_octets.clone(), Value::Counter32(9)),
                ],
            ),
            &columns,
            true,
        );
        assert_eq!(raw.name, "GigabitEthernet0/3");
        assert_eq!(raw.description, "desk 14");
        assert_eq!((raw.admin_up, raw.oper_up), (Some(true), Some(false)));
        assert_eq!(raw.speed_bps, Some(1_000_000_000));
        assert_eq!(raw.counters.in_octets, Some(Counter::bits64(12)));
    }

    #[test]
    fn an_agent_without_the_64_bit_columns_falls_back_to_the_32_bit_ones() {
        let columns = IfColumns::new();
        let raw = interface_of(
            &row(
                1,
                vec![
                    (columns.descr.clone(), Value::from("eth0")),
                    (columns.in_octets.clone(), Value::Counter32(4_000)),
                    (columns.out_octets.clone(), Value::Counter32(5_000)),
                ],
            ),
            &columns,
            false,
        );
        assert_eq!(raw.name, "eth0");
        assert_eq!(raw.counters.in_octets, Some(Counter::bits32(4_000)));
        assert_eq!(raw.counters.out_octets, Some(Counter::bits32(5_000)));
        assert_eq!(raw.speed_bps, None);
        // Nothing invented for the columns the agent did not answer.
        assert_eq!(raw.admin_up, None);
        assert_eq!(raw.counters.in_errors, None);
    }

    #[test]
    fn a_row_with_nothing_but_an_index_still_gets_a_name() {
        let columns = IfColumns::new();
        let raw = interface_of(&row(7, vec![]), &columns, true);
        assert_eq!(raw.name, "ifIndex 7");
    }

    #[test]
    fn numbers_come_out_of_gauges_integers_and_text() {
        assert_eq!(numeric(&Value::Gauge32(42)), Some(42.0));
        assert_eq!(numeric(&Value::Integer(-5)), Some(-5.0));
        assert_eq!(numeric(&Value::from("37 C")), Some(37.0));
        assert_eq!(numeric(&Value::NoSuchInstance), None);
        assert_eq!(numeric(&Value::from("warm")), None);
    }

    fn spec(yaml: &str) -> SnmpSpec {
        let mut registry = nocturne_drivers::profile::Registry::empty();
        registry
            .add(yaml, nocturne_drivers::profile::Origin::User)
            .expect("the profile loads")
            .profile
            .snmp
            .clone()
    }

    /// A column of `rows` rows, numbered from 1, each `value`.
    fn session_with_rows(column: &str, rows: u32, value: i64) -> FakeAgentSession {
        let column: Oid = column.parse().unwrap();
        FakeAgentSession {
            rows: (1..=rows)
                .map(|index| row(index, vec![(column.clone(), Value::Integer(value))]))
                .collect(),
            ..FakeAgentSession::default()
        }
    }

    #[tokio::test]
    async fn a_table_as_long_as_one_walk_takes_is_a_problem_not_a_figure() {
        let spec = spec(
            "id: t\nsnmp:\n  cpu: [1.3.6.1.2.1.25.3.3.1.2]\n  temperature: [1.3.6.1.4.1.9.9.13.1.3.1.3]\n",
        );
        // Three rows, and a walk that may bring back three: the fourth, the
        // one that might have made the figure, may be past the cut.
        let mut short = session_with_rows("1.3.6.1.2.1.25.3.3.1.2", 3, 10);
        let mut reading = Reading::default();
        read_health(&mut short, &spec, 3, &mut reading)
            .await
            .unwrap();
        assert_eq!(reading.cpu_pct, None);
        assert_eq!(
            reading.health_problems.len(),
            1,
            "{:?}",
            reading.health_problems
        );
        assert!(
            reading.health_problems[0]
                .contains("snmp.cpu[0] (1.3.6.1.2.1.25.3.3.1.2): the table has 3 rows or more"),
            "{:?}",
            reading.health_problems
        );
        // One row fewer than the walk takes is a whole table.
        let mut whole = session_with_rows("1.3.6.1.2.1.25.3.3.1.2", 2, 10);
        let mut reading = Reading::default();
        read_health(&mut whole, &spec, 3, &mut reading)
            .await
            .unwrap();
        assert_eq!(reading.cpu_pct, Some(10.0));
        assert!(reading.health_problems.is_empty());

        // An agent with 100,000 sensors, against the limit the poller uses.
        let mut flood = session_with_rows("1.3.6.1.4.1.9.9.13.1.3.1.3", 100_000, 40);
        let mut reading = Reading::default();
        read_health(&mut flood, &spec, SNMP_TABLE_LIMIT, &mut reading)
            .await
            .unwrap();
        assert!(reading.temperatures.is_empty());
        assert!(
            reading.health_problems[0].contains("none of its sensors was read"),
            "{:?}",
            reading.health_problems
        );
    }

    #[tokio::test]
    async fn a_large_table_read_in_the_wrong_unit_is_one_problem_not_one_per_row() {
        const COLUMN: &str = "1.3.6.1.4.1.9.9.13.1.3.1.3";
        // Five thousand rows that no thermometer reads: a column of some
        // other unit listed under `temperature`.
        let once = spec(&format!("id: t\nsnmp:\n  temperature: [{COLUMN}]\n"));
        let mut session = session_with_rows(COLUMN, 5_000, 400);
        let mut reading = Reading::default();
        read_health(&mut session, &once, SNMP_TABLE_LIMIT, &mut reading)
            .await
            .unwrap();
        assert!(reading.temperatures.is_empty());
        assert_eq!(
            reading.health_problems,
            [format!(
                "snmp.temperature[0] ({COLUMN}): sensor `1` reads 400 °C, and 4999 more of its sensors read values no sensor does; none of them was stored"
            )]
        );

        // The same table listed twice: every one of its names is shared.
        let twice = spec(&format!(
            "id: t\nsnmp:\n  temperature: [{COLUMN}, {COLUMN}]\n"
        ));
        let mut session = session_with_rows(COLUMN, 5_000, 40);
        let mut reading = Reading::default();
        read_health(&mut session, &twice, SNMP_TABLE_LIMIT, &mut reading)
            .await
            .unwrap();
        assert!(reading.temperatures.is_empty());
        assert_eq!(
            reading.health_problems,
            [format!(
                "snmp.temperature[0] ({COLUMN}) and snmp.temperature[1] ({COLUMN}) give 5000 sensors of the same names (`1` and 4999 more), so none of them was stored: give the entries distinct `label`s"
            )]
        );
    }

    /// Answers like `inner`, except that any request naming `broken` times
    /// out.
    struct TimesOut {
        inner: FakeAgentSession,
        broken: Oid,
    }

    #[async_trait::async_trait]
    impl SnmpSession for TimesOut {
        async fn get(&mut self, oids: &[Oid]) -> Result<Vec<VarBind>, String> {
            if oids.contains(&self.broken) {
                return Err("request timed out".into());
            }
            self.inner.get(oids).await
        }

        async fn table(&mut self, columns: &[Oid], limit: usize) -> Result<Vec<TableRow>, String> {
            if columns.contains(&self.broken) {
                return Err("request timed out".into());
            }
            self.inner.table(columns, limit).await
        }

        fn supports_counter64(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn a_health_request_that_times_out_fails_the_read_as_any_request_does() {
        let mut registry = nocturne_drivers::profile::Registry::empty();
        let platform = registry
            .add(
                "id: t\nsnmp:\n  memory_total: [1.3.6.1.4.1.2021.4.5.0]\n  temperature: [1.3.6.1.4.1.9.9.13.1.3.1.3]\n",
                nocturne_drivers::profile::Origin::User,
            )
            .unwrap();
        for broken in ["1.3.6.1.4.1.2021.4.5.0", "1.3.6.1.4.1.9.9.13.1.3.1.3"] {
            let mut inner = FakeAgentSession::default();
            inner.scalars.push(VarBind {
                oid: snmp::oid(oids::SYS_UP_TIME),
                value: Value::TimeTicks(100),
            });
            let mut session = TimesOut {
                inner,
                broken: broken.parse().unwrap(),
            };
            let outcome = read_snmp(&mut session, Some(&platform)).await;
            assert_eq!(
                outcome,
                Err("request timed out".to_string()),
                "a timeout is not an object the agent lacks: {broken}"
            );
        }
    }

    #[tokio::test]
    async fn uptime_and_interfaces_come_back_from_a_session() {
        let mut session = FakeAgentSession::default();
        session.scalars.push(VarBind {
            oid: snmp::oid(oids::SYS_UP_TIME),
            value: Value::TimeTicks(123_456),
        });
        let columns = IfColumns::new();
        session.rows.push(row(
            1,
            vec![
                (columns.name.clone(), Value::from("Gi0/1")),
                (columns.oper.clone(), Value::Integer(1)),
                (columns.hc_in.clone(), Value::Counter64(1_000)),
            ],
        ));
        let reading = read_snmp(&mut session, None).await.unwrap();
        assert_eq!(reading.time_base, Some(TimeBase::Ticks(123_456)));
        assert_eq!(reading.uptime_s, Some(1_234));
        assert!(reading.interfaces_read);
        assert_eq!(reading.interfaces.len(), 1);
        assert_eq!(reading.interfaces[0].name, "GigabitEthernet0/1");
    }
}
