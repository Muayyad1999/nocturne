//! "Which switch port is this plugged into?"
//!
//! An IP becomes a MAC through the collected ARP tables, the MAC is looked up
//! in every collected MAC table, and the sighting that is an edge port wins —
//! the trunks and uplinks where the same MAC was seen are the path to it, not
//! where it is. Every step is reported in `how`, because an answer nobody can
//! check is worth very little.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_app::App;
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::settings;
use nocturne_drivers::ifname;
use serde::{Deserialize, Serialize};

use crate::data::{Collected, bare_mac, pretty};
use crate::endpoints::{self, PortKind, Sighting};
use crate::export::timestamp;
use crate::model::{SETTINGS_SECTION, Settings};
use crate::physical;

#[derive(Debug, Deserialize)]
pub struct Params {
    /// An IP address, a MAC address in any notation, or a device name.
    pub query: String,
}

/// One place the address was seen.
#[derive(Clone, Debug, Serialize)]
pub struct Location {
    pub device_id: String,
    pub device: String,
    pub interface: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlan: Option<u64>,
    /// `access`, `trunk` or `uplink`.
    pub kind: String,
    /// When the MAC table it came from was collected.
    pub seen_at: i64,
    /// How many MACs that port has learnt.
    pub macs_on_port: usize,
    /// `dynamic`, `static`, … as the device labelled the entry.
    pub entry_kind: String,
    pub why: String,
}

#[derive(Debug, Serialize)]
pub struct Located {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// Every sighting, in device then interface order.
    pub locations: Vec<Location>,
    /// The index in `locations` of the port the host is most likely on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best: Option<usize>,
    /// The reasoning, step by step.
    pub how: Vec<String>,
}

/// Every device, as the scope for a search that is not limited to a view.
fn everything(collected: &Collected) -> BTreeSet<String> {
    collected
        .devices
        .iter()
        .map(|row| row.device.id.clone())
        .collect()
}

/// An address resolved to a MAC, with the sentences that explain how.
struct Resolved {
    mac: Option<String>,
    ip: Option<String>,
    how: Vec<String>,
}

fn resolve(collected: &Collected, query: &str) -> AppResult<Resolved> {
    let query = query.trim();
    if query.is_empty() {
        return Err(AppError::invalid(
            "give an IP address, a MAC address or a device name to look for",
        ));
    }
    let mut how = Vec::new();

    if let Some(mac) = bare_mac(query) {
        how.push(format!(
            "`{query}` read as the MAC address {}",
            pretty(&mac)
        ));
        let ip = endpoints::arp_index(collected)
            .get(&mac)
            .and_then(|ips| ips.iter().next().cloned());
        if let Some(ip) = &ip {
            how.push(format!(
                "a collected ARP table gives {} the address {ip}",
                pretty(&mac)
            ));
        }
        return Ok(Resolved {
            mac: Some(mac),
            ip,
            how,
        });
    }

    if query.parse::<std::net::IpAddr>().is_ok() {
        let wanted = query.to_ascii_lowercase();
        let mut found: Vec<(String, String, i64)> = Vec::new();
        for (device_id, aged) in &collected.arp {
            for entry in &aged.rows {
                if entry.ip.trim().eq_ignore_ascii_case(&wanted)
                    && let Some(mac) = bare_mac(&entry.mac)
                {
                    found.push((mac, device_id.clone(), aged.at));
                }
            }
        }
        found.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
        let Some((mac, device_id, at)) = found.first().cloned() else {
            return Err(AppError::not_found(format!(
                "no collected ARP table has an entry for {query}. Collect `arp` from the router or \
                 switch that owns that subnet, then ask again"
            ))
            .with_details(serde_json::json!({"query": query, "step": "arp"})));
        };
        how.push(format!(
            "{query} is at {} in the ARP table of {} (collected {})",
            pretty(&mac),
            collected.name_of(&device_id),
            timestamp(at)
        ));
        let distinct: BTreeSet<&String> = found.iter().map(|(mac, ..)| mac).collect();
        if distinct.len() > 1 {
            how.push(format!(
                "{} different MAC addresses are recorded for {query}; the most recently collected \
                 one is used",
                distinct.len()
            ));
        }
        return Ok(Resolved {
            mac: Some(mac),
            ip: Some(wanted),
            how,
        });
    }

    // Names are handled by `resolve_by_name` before this point; anything left
    // is something Nocturne cannot turn into an address, and it says so.
    Err(AppError::not_found(format!(
        "`{query}` is not an IP address or a MAC address, and no device of that name is in the \
         inventory. Nocturne does not resolve names, so give an address"
    )))
}

/// Find where a host is.
pub fn locate(app: &App, params: &Params) -> AppResult<Located> {
    let config: Settings = settings::get(app, SETTINGS_SECTION)?;
    let collected = Collected::load(app)?;
    let query = params.query.trim().to_string();

    // A device name resolves to the device's own interface MACs.
    let resolved = match resolve_by_name(app, &collected, &query)? {
        Some(resolved) => resolved,
        None => resolve(&collected, &query)?,
    };
    let Resolved { mac, ip, mut how } = resolved;
    let Some(mac) = mac else {
        return Ok(Located {
            mac: None,
            ip,
            vendor: None,
            locations: Vec::new(),
            best: None,
            how,
        });
    };

    let scope = everything(&collected);
    let reported = physical::build(&collected, &scope);
    let seen = endpoints::sightings(
        &collected,
        &scope,
        &reported.network_ports,
        config.endpoint_max_macs as usize,
    );
    let sightings: Vec<Sighting> = seen.get(&mac).cloned().unwrap_or_default();
    if sightings.is_empty() {
        how.push(format!(
            "{} is in no collected MAC table. Collect `mac_table` from the access switches of that \
             VLAN and ask again",
            pretty(&mac)
        ));
        return Ok(Located {
            vendor: nocturne_tools::mac::vendor(&pretty(&mac)),
            mac: Some(pretty(&mac)),
            ip,
            locations: Vec::new(),
            best: None,
            how,
        });
    }

    how.push(format!(
        "{} is in the MAC table of {} port{}",
        pretty(&mac),
        sightings.len(),
        if sightings.len() == 1 { "" } else { "s" }
    ));
    let best = endpoints::best_sighting(&sightings);
    if let Some(index) = best {
        let chosen = &sightings[index];
        how.push(format!(
            "{} {} is the answer: {}",
            collected.name_of(&chosen.device_id),
            chosen.interface,
            chosen.why
        ));
        let elsewhere: Vec<String> = sightings
            .iter()
            .enumerate()
            .filter(|(other, sighting)| *other != index && sighting.kind != PortKind::Access)
            .map(|(_, sighting)| {
                format!(
                    "{} {} ({})",
                    collected.name_of(&sighting.device_id),
                    ifname::short(&sighting.interface),
                    sighting.kind.as_str()
                )
            })
            .collect();
        if !elsewhere.is_empty() {
            how.push(format!(
                "it is also learnt on {} — that is the path frames take to reach it, not where it \
                 is plugged in",
                elsewhere.join(", ")
            ));
        }
        if chosen.kind != PortKind::Access {
            how.push(
                "no port looks like an edge port, so this is the best guess, not an answer: \
                 collect `neighbors` and `interfaces` from these switches to tell uplinks from \
                 access ports"
                    .to_string(),
            );
        }
    }

    let locations = sightings
        .iter()
        .map(|sighting| Location {
            device: collected.name_of(&sighting.device_id),
            device_id: sighting.device_id.clone(),
            interface: sighting.interface.clone(),
            vlan: sighting.vlan,
            kind: sighting.kind.as_str().to_string(),
            seen_at: sighting.seen_at,
            macs_on_port: sighting.macs_on_port,
            entry_kind: sighting.entry_kind.clone(),
            why: sighting.why.clone(),
        })
        .collect();

    Ok(Located {
        vendor: nocturne_tools::mac::vendor(&pretty(&mac)),
        mac: Some(pretty(&mac)),
        ip,
        locations,
        best,
        how,
    })
}

/// A device name: look for the MAC addresses of its own interfaces.
fn resolve_by_name(app: &App, collected: &Collected, query: &str) -> AppResult<Option<Resolved>> {
    if query.parse::<std::net::IpAddr>().is_ok() || bare_mac(query).is_some() {
        return Ok(None);
    }
    let Ok(device) = nocturne_app::inventory::find(app, query) else {
        return Ok(None);
    };
    let mut how = vec![format!(
        "`{query}` is the device {} in the inventory",
        device.name
    )];
    let macs: Vec<String> = collected
        .interfaces
        .get(&device.id)
        .map(|aged| {
            aged.rows
                .iter()
                .filter_map(|interface| bare_mac(&interface.mac))
                .collect()
        })
        .unwrap_or_default();
    // The MAC that another switch has learnt is the one we want.
    let elsewhere: BTreeMap<String, usize> = macs
        .iter()
        .map(|mac| {
            let count = collected
                .mac_table
                .iter()
                .filter(|(device_id, _)| *device_id != &device.id)
                .flat_map(|(_, aged)| aged.rows.iter())
                .filter(|entry| bare_mac(&entry.mac).as_deref() == Some(mac.as_str()))
                .count();
            (mac.clone(), count)
        })
        .collect();
    let chosen = elsewhere
        .iter()
        .filter(|(_, count)| **count > 0)
        .max_by_key(|(mac, count)| (**count, std::cmp::Reverse((*mac).clone())))
        .map(|(mac, _)| mac.clone())
        .or_else(|| macs.first().cloned());
    match chosen {
        Some(mac) => {
            how.push(format!(
                "its interfaces have the MAC address {}, which is what the MAC tables are searched \
                 for",
                pretty(&mac)
            ));
            Ok(Some(Resolved {
                mac: Some(mac),
                ip: Some(device.address.clone()).filter(|address| !address.is_empty()),
                how,
            }))
        }
        None => Err(AppError::not_found(format!(
            "no interface MAC address has been collected from {}, so there is nothing to look for. \
             Collect `interfaces` from it first",
            device.name
        ))),
    }
}
