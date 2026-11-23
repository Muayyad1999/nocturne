//! Reading a device: what SNMP and the CLI each hand back, in one shape.
//!
//! Nothing here computes a rate — it only reports what the device said, so
//! that the counter arithmetic lives in one place ([`crate::rates`]) and is
//! the same whether the numbers came off an SNMP table or a `show interfaces`.

use std::time::Duration;

use nocturne_app::model::Device;
use nocturne_app::{App, AppResult, collect, profiles};
use nocturne_drivers::model::{Health, Interface, Kind};
use nocturne_drivers::profile::{InterfaceNames, Platform};
use nocturne_snmp::{Oid, Value, oids};

use crate::rates::{Counter, TimeBase};
use crate::settings::{SNMP_TABLE_LIMIT, SNMP_TIMEOUT_S};
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
    /// Temperature per sensor; the name is the sensor's table index.
    pub temperatures: Vec<(String, f64)>,
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

/// CPU, memory and temperatures from the OIDs the device's profile names.
async fn health_over_snmp(
    session: &mut dyn SnmpSession,
    platform: &Platform,
    reading: &mut Reading,
) -> Result<(), String> {
    let spec = &platform.profile.snmp;
    let cpu = numbers(session, &spec.cpu).await?;
    if !cpu.is_empty() {
        // Several processors: the device's load is their mean.
        reading.cpu_pct = Some(cpu.iter().map(|(_, value)| value).sum::<f64>() / cpu.len() as f64);
    }
    // Memory objects name one pool, so the first reading is the pool's.
    let used = numbers(session, &spec.memory_used).await?;
    let free = numbers(session, &spec.memory_free).await?;
    let total = numbers(session, &spec.memory_total).await?;
    let first = |values: &[(String, f64)]| values.first().map(|(_, value)| *value);
    reading.mem_pct = match (first(&used), first(&total), first(&free)) {
        (Some(used), Some(total), _) if total > 0.0 => Some(used / total * 100.0),
        (Some(used), _, Some(free)) if used + free > 0.0 => Some(used / (used + free) * 100.0),
        (_, Some(total), Some(free)) if total > 0.0 => Some((total - free) / total * 100.0),
        _ => None,
    };
    reading.temperatures = numbers(session, &spec.temperature).await?;
    Ok(())
}

/// Read a profile's OIDs, whether they are scalars or columns, as numbers
/// labelled with the instance they came from.
async fn numbers(
    session: &mut dyn SnmpSession,
    oid_texts: &[String],
) -> Result<Vec<(String, f64)>, String> {
    let mut scalars: Vec<Oid> = Vec::new();
    let mut columns: Vec<Oid> = Vec::new();
    for text in oid_texts {
        let Ok(oid) = text.trim().parse::<Oid>() else {
            // A profile with an unparseable OID is a profile bug, not a
            // device problem: skip it rather than failing the whole poll.
            tracing::warn!(oid = %text, "ignoring an OID that does not parse");
            continue;
        };
        if text.trim().ends_with(".0") {
            scalars.push(oid);
        } else {
            columns.push(oid);
        }
    }
    let mut found: Vec<(String, f64)> = Vec::new();
    if !scalars.is_empty() {
        for bind in session.get(&scalars).await? {
            if let Some(value) = numeric(&bind.value) {
                found.push((String::new(), value));
            }
        }
    }
    for column in columns {
        for row in session.table(std::slice::from_ref(&column), 512).await? {
            let label = row
                .index
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(".");
            if let Some(value) = row.get(&column).and_then(numeric) {
                found.push((label, value));
            }
        }
    }
    Ok(found)
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

/// Read a device over its command line, for devices with no SNMP and for
/// simulated ones. Uses the core collector, so the profile decides which
/// commands are run and everything read is stored for the other features.
pub async fn over_cli(app: &App, device_id: &str) -> AppResult<Reading> {
    let snapshot = collect::run(app, device_id, &[Kind::Interfaces, Kind::Health]).await?;
    let interfaces: Option<Vec<Interface>> = snapshot
        .rows(Kind::Interfaces)
        .map(|rows| typed_rows(&rows));
    let health: Option<Health> = snapshot
        .rows(Kind::Health)
        .and_then(|rows| typed_rows::<Health>(&rows).into_iter().next());

    let mut reading = Reading {
        interfaces_read: interfaces.is_some(),
        interfaces: interfaces
            .unwrap_or_default()
            .into_iter()
            .map(from_model)
            .collect(),
        ..Reading::default()
    };
    if let Some(health) = health {
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
    // A device that reports its uptime gives counter deltas a trustworthy
    // base; one that does not leaves only the poller's own clock.
    reading.time_base = Some(reading.uptime_s.map_or(TimeBase::Wall, TimeBase::Seconds));
    Ok(reading)
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
