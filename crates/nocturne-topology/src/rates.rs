//! The rates monitoring last measured on the ports that links end on.
//!
//! The numbers are the monitor feature's (`monitor.interfaces`); the ports
//! are this feature's. A link end is always spelled the canonical way
//! (`TenGigabitEthernet0/0/0/1`), while monitoring keeps a device's own
//! spelling where its profile says to (`TenGigE0/0/0/1` on IOS XR). So the
//! two are joined here, next to [`ifname`], with both names put through the
//! same function before they are compared — not by every caller with a table
//! of abbreviations of its own.
//!
//! The canonical form is a way to find a port that is spelled differently,
//! not a licence to merge two: a device that has both `eth0` and `Ethernet0`
//! (SONiC's management port and its first front-panel port) has two rows in
//! monitoring's table and one canonical form between them. A port is
//! therefore looked up by the name monitoring keeps first, and by canonical
//! form only when exactly one of the device's ports has it.
//!
//! Nothing here measures anything and nothing is made up: a port monitoring
//! has no row for is left out of the answer, and a rate it could not work out
//! stays absent rather than becoming zero.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_app::App;
use nocturne_app::error::AppResult;
use nocturne_app::util::now_ms;
use nocturne_drivers::ifname;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::paged;

/// How many rows of `monitor.interfaces` are asked for at a time: well inside
/// the five thousand it allows, which is what [`paged::rows`] needs.
const PAGE: usize = 1_000;
const METHOD: &str = "monitor.interfaces";

/// One port as monitoring last saw it.
#[derive(Clone, Debug)]
pub struct Monitored {
    /// The port's name as monitoring keeps it.
    pub name: String,
    pub in_bps: Option<f64>,
    pub out_bps: Option<f64>,
    pub speed_bps: Option<u64>,
    pub util_pct: Option<f64>,
    /// When the poll that produced these ran.
    pub at: Option<i64>,
}

/// What monitoring knows about the ports of one device.
#[derive(Debug, Default)]
struct DevicePorts {
    /// By the name monitoring keeps, letter for letter.
    by_name: BTreeMap<String, Monitored>,
    /// Canonical form → the name of the one port that has it, or `None` when
    /// more than one does and the form alone cannot say which is meant.
    by_key: BTreeMap<String, Option<String>>,
}

impl DevicePorts {
    fn add(&mut self, port: Monitored) {
        // The same row twice is one port, not two with one canonical form: an
        // offset cursor repeats a row when the table changes between pages.
        if self.by_name.contains_key(&port.name) {
            return;
        }
        self.by_key
            .entry(key(&port.name))
            .and_modify(|only| *only = None)
            .or_insert_with(|| Some(port.name.clone()));
        self.by_name.insert(port.name.clone(), port);
    }

    fn get(&self, interface: &str) -> Option<&Monitored> {
        if let Some(port) = self.by_name.get(interface) {
            return Some(port);
        }
        let name = self.by_key.get(&key(interface))?.as_deref()?;
        self.by_name.get(name)
    }
}

/// What monitoring knows about the ports of some devices.
#[derive(Debug, Default)]
pub struct Ports {
    by_device: BTreeMap<String, DevicePorts>,
}

impl Ports {
    /// The port of a device: the one monitoring knows by exactly this name,
    /// or else the only one whose name is another spelling of it. `None` when
    /// there is no such port, and when there are two — picking one would put
    /// one port's rates on another.
    pub fn get(&self, device_id: &str, interface: &str) -> Option<&Monitored> {
        self.by_device.get(device_id)?.get(interface)
    }
}

/// The form two spellings of one port meet in: exactly what [`ifname::same`]
/// compares, written out so that it can be looked up in a map.
fn key(name: &str) -> String {
    ifname::long(name).to_ascii_lowercase()
}

/// What monitoring last measured on every port of the given devices, or
/// `None` when the monitor feature is not in this build.
///
/// Asked for one device at a time and paged to the end: the table has a row
/// for every port of every monitored device, one call returns at most a few
/// thousand of them, and a port in the part that was not read is
/// indistinguishable from one that was never measured.
pub async fn read(app: &App, device_ids: &BTreeSet<String>) -> AppResult<Option<Ports>> {
    if device_ids.is_empty() {
        // Nothing to read, but "is anybody measuring" still has an answer.
        return Ok(paged::page(app, METHOD, json!({"limit": 1}))
            .await?
            .map(|_| Ports::default()));
    }
    let mut ports = Ports::default();
    for device_id in device_ids {
        let params = json!({"device_id": device_id, "sort": "name"});
        let Some(rows) = paged::rows(app, METHOD, &params, PAGE).await? else {
            return Ok(None);
        };
        let known = ports.by_device.entry(device_id.clone()).or_default();
        for row in rows {
            // A row is taken at its word about whose it is, not at the
            // filter's: one that names another device is not this device's.
            if row.get("device_id").and_then(Value::as_str) != Some(device_id.as_str()) {
                continue;
            }
            let Some(name) = row.get("name").and_then(Value::as_str) else {
                continue;
            };
            let number = |field: &str| row.get(field).and_then(Value::as_f64);
            known.add(Monitored {
                name: name.to_string(),
                in_bps: number("in_bps"),
                out_bps: number("out_bps"),
                speed_bps: row.get("speed_bps").and_then(Value::as_u64),
                util_pct: number("util_pct"),
                at: row.get("at").and_then(Value::as_i64),
            });
        }
    }
    Ok(Some(ports))
}

/// `topology.rates` parameters: the ports to answer for.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Params {
    pub ports: Vec<PortRef>,
}

/// One port: a device of the inventory and an interface of it, in whatever
/// spelling the caller has — normally a link end's, as `topology.get` gave it.
#[derive(Debug, Deserialize)]
pub struct PortRef {
    pub device_id: String,
    pub interface: String,
}

/// What monitoring last measured on one of the ports asked about.
#[derive(Debug, Serialize)]
pub struct PortRate {
    pub device_id: String,
    /// The name the port was asked for by, letter for letter, so the caller
    /// can find its answer without knowing anything about interface names.
    pub interface: String,
    /// The name monitoring knows the port by.
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_bps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub out_bps: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<u64>,
    /// When the poll that produced the rates ran.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
}

/// The answer to `topology.rates`.
#[derive(Debug, Serialize)]
pub struct Rates {
    /// This machine's clock when the answer was put together. How old a
    /// reading is, is its `at` taken from this — never from the clock of
    /// whoever is asking, which may be minutes out.
    pub at: i64,
    /// Whether the monitor feature is in this build. When it is not there are
    /// no rates to be had, which is not the same as none having been measured.
    pub monitoring: bool,
    /// One entry for each port asked about that monitoring has a row for, in
    /// the order they were asked. A spelling that could mean either of two of
    /// a device's ports is not answered.
    pub ports: Vec<PortRate>,
}

/// `topology.rates`: the rates on the ports asked about.
pub async fn rates(app: &App, params: Params) -> AppResult<Rates> {
    let devices: BTreeSet<String> = params
        .ports
        .iter()
        .map(|port| port.device_id.clone())
        .collect();
    let Some(known) = read(app, &devices).await? else {
        return Ok(Rates {
            at: now_ms(),
            monitoring: false,
            ports: Vec::new(),
        });
    };
    let mut answered: BTreeSet<(String, String)> = BTreeSet::new();
    let mut ports = Vec::new();
    for port in params.ports {
        let Some(row) = known.get(&port.device_id, &port.interface) else {
            continue;
        };
        if !answered.insert((port.device_id.clone(), port.interface.clone())) {
            continue;
        }
        ports.push(PortRate {
            device_id: port.device_id,
            interface: port.interface,
            name: row.name.clone(),
            in_bps: row.in_bps,
            out_bps: row.out_bps,
            speed_bps: row.speed_bps,
            at: row.at,
        });
    }
    Ok(Rates {
        at: now_ms(),
        monitoring: true,
        ports,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_spellings_of_one_port_have_one_key_and_two_ports_have_two() {
        assert_eq!(key("TenGigE0/0/0/1"), key("TenGigabitEthernet0/0/0/1"));
        assert_eq!(key("Gi0/1"), key("gigabitethernet 0/1"));
        assert_eq!(key("eth0"), key("Ethernet0"));
        assert_ne!(key("Gi0/1"), key("Gi0/10"));
        assert_ne!(key("Gi0/1"), key("Fa0/1"));
        assert!(ifname::same("TenGigE0/0/0/1", "TenGigabitEthernet0/0/0/1"));
    }
}
