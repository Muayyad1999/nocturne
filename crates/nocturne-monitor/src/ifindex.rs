//! Which ifIndex is which interface, as a device's SNMP agent last said.
//!
//! A trap says `ifIndex=3` and a flow record says "input interface 3";
//! neither says `GigabitEthernet0/3`. The poller reads the interface table
//! by that index, and this module keeps the pairing where the features that
//! decode traps and flows can ask for it (`monitor.ifindex`). It is stored,
//! not remembered: a trap arrives when it likes, including in the minute
//! after a restart, before the device has been polled again.
//!
//! Nothing in the map is worked out. A pair is there only because the agent
//! gave that row a name *and* the poll that had just run knows an interface
//! by that name; a device that has never answered SNMP has no map at all
//! (`at` absent), which is a different answer from a map with nothing in it.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use nocturne_app::model::Device;
use nocturne_app::{App, AppResult, Router, inventory, profiles};
use nocturne_drivers::profile::InterfaceNames;
use nocturne_snmp::oids;
use serde::{Deserialize, Serialize};

use crate::db::collections::IFINDEX;
use crate::read::Reading;
use crate::settings::{SNMP_TABLE_LIMIT, SNMP_TIMEOUT_S};
use crate::snmp;
use crate::state::MonitorState;

/// How long a map stands without being read again, when nothing suggests it
/// has changed. An agent may renumber its interfaces only when it restarts
/// (RFC 2863), and a restart, like a different set of names, is seen by the
/// poll and asked about at once — so this is a backstop, not the cadence
/// anything depends on, and it keeps the cost to two columns every five
/// minutes instead of two more on every poll.
const REFRESH_MS: i64 = 300_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IfIndexEntry {
    pub index: u32,
    /// Spelled as the interface table and the metrics spell it.
    pub name: String,
}

/// The answer to `monitor.ifindex`, and the stored document.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IfIndexMap {
    /// When the map was read off the device. Absent when it never was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
    pub interfaces: Vec<IfIndexEntry>,
}

/// What is stored for a device: its map, or the empty answer of a device
/// that has never been read over SNMP.
pub fn stored(app: &App, device_id: &str) -> AppResult<IfIndexMap> {
    Ok(app.store().get(IFINDEX, device_id)?.unwrap_or_default())
}

/// The interface names of a reading, as one number: enough to notice that a
/// port has appeared, gone or been renamed since the map was read.
fn fingerprint(reading: &Reading) -> u64 {
    let mut names: Vec<&str> = reading
        .interfaces
        .iter()
        .map(|raw| raw.name.as_str())
        .collect();
    names.sort_unstable();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut hasher);
    hasher.finish()
}

/// Bring a device's map up to date after a poll that read it over SNMP.
///
/// Asked again when the device restarted, when the poll read other
/// interface names than last time, and otherwise every [`REFRESH_MS`]. A
/// read that fails leaves the stored map as it was — still the last thing
/// the agent said, with the time it said it — and never fails the poll.
pub async fn refresh(
    app: &App,
    state: &MonitorState,
    device: &Device,
    reading: &Reading,
    rebooted: bool,
    tick: i64,
    at: i64,
) {
    let names = fingerprint(reading);
    let due = state.device(&device.id, |runtime| {
        let due = rebooted
            || runtime.ifindex_names != names
            || runtime
                .last_ifindex_mono
                .is_none_or(|last| tick - last >= REFRESH_MS);
        if due {
            // Whatever comes of it: an agent that does not answer this
            // question is not asked it again on every poll.
            runtime.last_ifindex_mono = Some(tick);
            runtime.ifindex_names = names;
        }
        due
    });
    if !due {
        return;
    }
    match read(app, state, device, reading).await {
        Ok(interfaces) => {
            let map = IfIndexMap {
                at: Some(at),
                interfaces,
            };
            if let Err(err) = app.store().put(IFINDEX, &device.id, &map) {
                tracing::warn!(device = %device.id, %err, "the interface index map could not be stored");
            }
        }
        Err(error) => {
            tracing::debug!(device = %device.id, %error, "the interface index map could not be read");
        }
    }
}

/// Ask the agent for the name columns of its interface table.
///
/// The poll's own walk of that table (`read::over_snmp`) has every row's
/// index in its hands and returns interfaces by name only, so this asks for
/// the two name columns once more, on the session the poll has just used.
/// The names are spelled by the rule the poll spells them by, and one is
/// kept only if the reading has an interface of that name: should the two
/// rules ever drift apart, the map loses entries instead of gaining names
/// that nothing else in the application uses.
async fn read(
    app: &App,
    state: &MonitorState,
    device: &Device,
    reading: &Reading,
) -> Result<Vec<IfIndexEntry>, String> {
    let target = snmp::target(app, device).map_err(|err| err.message)?;
    let mut lease = snmp::acquire(
        &state.sessions,
        &state.snmp,
        &device.id,
        &target,
        Duration::from_secs(SNMP_TIMEOUT_S),
    )
    .await?;
    let (name, descr) = (snmp::oid(oids::IF_NAME), snmp::oid(oids::IF_DESCR));
    let rows = match lease
        .session()
        .table(&[name.clone(), descr.clone()], SNMP_TABLE_LIMIT)
        .await
    {
        Ok(rows) => rows,
        Err(error) => {
            lease.discard();
            return Err(error);
        }
    };
    drop(lease);

    let canonical = profiles::registry(app)
        .get(&device.profile)
        .is_none_or(|platform| platform.profile.interface_names == InterfaceNames::Cisco);
    let known: HashSet<&str> = reading
        .interfaces
        .iter()
        .map(|raw| raw.name.as_str())
        .collect();
    let mut interfaces: Vec<IfIndexEntry> = rows
        .iter()
        .filter_map(|row| {
            // ifIndex is one number; a row indexed by anything else is not
            // a row of this table.
            let [index] = row.index.as_slice() else {
                return None;
            };
            // A row the agent gave no name is left out: the poll calls it
            // `ifIndex 7`, which is a label for a screen, not a name.
            let reported = row
                .text(&name)
                .filter(|text| !text.trim().is_empty())
                .or_else(|| row.text(&descr))
                .filter(|text| !text.trim().is_empty())?;
            let spelled = if canonical {
                nocturne_drivers::ifname::long(reported.trim())
            } else {
                reported.trim().to_string()
            };
            known.contains(spelled.as_str()).then_some(IfIndexEntry {
                index: *index,
                name: spelled,
            })
        })
        .collect();
    interfaces.sort_by_key(|entry| entry.index);
    interfaces.dedup_by_key(|entry| entry.index);
    Ok(interfaces)
}

#[derive(Debug, Deserialize)]
pub struct DeviceParams {
    pub device_id: String,
}

pub fn register(router: &mut Router) {
    router.add(
        "monitor.ifindex",
        |app: App, params: DeviceParams| async move {
            inventory::get(&app, &params.device_id)?;
            stored(&app, &params.device_id)
        },
    );
}
