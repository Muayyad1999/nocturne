//! VLANs, beside the prefixes rather than in a corner: in practice a VLAN and
//! a subnet are one decision.
//!
//! The same split holds here. A [`VlanRecord`] is what somebody wrote down; a
//! collected VLAN table is evidence, and it keeps the name each device gives
//! the VLAN, because two switches disagreeing about what VLAN 30 is called is
//! exactly the sort of thing worth seeing.

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use serde::Deserialize;

use crate::db;
use crate::model::{Source, VlanDevice, VlanPrefix, VlanRecord, VlanRow};
use crate::prefixes::{self, Entry};

/// Every VLAN: documented, seen, or both.
pub fn rows(app: &App, site: Option<&str>, query: Option<&str>) -> AppResult<Vec<VlanRow>> {
    let records: Vec<VlanRecord> = app.store().list(db::VLANS)?;
    let sightings = crate::collected::vlan_sightings(app)?;
    let entries = prefixes::load(app)?;

    let mut rows: Vec<VlanRow> = Vec::new();
    for record in &records {
        let devices = devices_for(&sightings, record.vid, record.site.as_deref());
        rows.push(VlanRow {
            prefixes: prefixes_for(&entries, record),
            observed: !devices.is_empty(),
            documented: true,
            devices,
            vlan: record.clone(),
        });
    }
    // VLANs the network has and nobody documented. They are not records: the
    // id is empty and `documented` is false until somebody saves one.
    for sighting in &sightings {
        let site_of = sighting_site(sighting);
        if records
            .iter()
            .any(|record| record.vid == sighting.vid && record.site == site_of)
        {
            continue;
        }
        if rows
            .iter()
            .any(|row| !row.documented && row.vlan.vid == sighting.vid && row.vlan.site == site_of)
        {
            continue;
        }
        let devices = devices_for(&sightings, sighting.vid, site_of.as_deref());
        let name = devices
            .iter()
            .map(|device| device.name.clone())
            .find(|name| !name.is_empty())
            .unwrap_or_default();
        rows.push(VlanRow {
            vlan: VlanRecord {
                id: String::new(),
                vid: sighting.vid,
                name,
                site: site_of,
                description: String::new(),
                prefix_ids: Vec::new(),
                source: Source::Discovered,
                created_at: 0,
                updated_at: 0,
            },
            devices,
            prefixes: Vec::new(),
            documented: false,
            observed: true,
        });
    }
    // A prefix that names a VLAN nobody documented or collected still ties
    // the two together, so the VLAN is listed.
    for entry in &entries {
        let Some(vid) = entry.prefix.vlan_id else {
            continue;
        };
        if rows.iter().any(|row| row.vlan.vid == vid) {
            continue;
        }
        rows.push(VlanRow {
            vlan: VlanRecord {
                id: String::new(),
                vid,
                name: String::new(),
                site: entry.prefix.site.clone(),
                description: String::new(),
                prefix_ids: Vec::new(),
                source: Source::Discovered,
                created_at: 0,
                updated_at: 0,
            },
            devices: Vec::new(),
            prefixes: vec![VlanPrefix {
                id: entry.prefix.id.clone(),
                cidr: entry.prefix.cidr.clone(),
            }],
            documented: false,
            observed: false,
        });
    }

    if let Some(site) = site {
        rows.retain(|row| row.vlan.site.as_deref() == Some(site));
    }
    if let Some(query) = query.filter(|query| !query.trim().is_empty()) {
        rows.retain(|row| {
            let haystack = format!(
                "{} {} {} {}",
                row.vlan.vid,
                row.vlan.name,
                row.vlan.description,
                row.devices
                    .iter()
                    .map(|device| format!("{} {}", device.device, device.name))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            nocturne_app::util::matches_query(&haystack, query)
        });
    }
    rows.sort_by(|a, b| {
        (a.vlan.site.clone().unwrap_or_default(), a.vlan.vid)
            .cmp(&(b.vlan.site.clone().unwrap_or_default(), b.vlan.vid))
    });
    Ok(rows)
}

fn sighting_site(sighting: &crate::collected::VlanSighting) -> Option<String> {
    if sighting.device.scope.is_empty() {
        Some(sighting.device.site.clone()).filter(|site| !site.is_empty())
    } else {
        Some(sighting.device.scope.clone())
    }
}

fn devices_for(
    sightings: &[crate::collected::VlanSighting],
    vid: u16,
    site: Option<&str>,
) -> Vec<VlanDevice> {
    sightings
        .iter()
        .filter(|sighting| sighting.vid == vid && sighting_site(sighting).as_deref() == site)
        .map(|sighting| VlanDevice {
            device_id: sighting.device.id.clone(),
            device: sighting.device.name.clone(),
            name: sighting.name.clone(),
            status: sighting.status.clone(),
            ports: sighting.ports,
            collected_at: sighting.collected_at,
            lab: sighting.device.lab,
        })
        .collect()
}

fn prefixes_for(entries: &[Entry], record: &VlanRecord) -> Vec<VlanPrefix> {
    let mut prefixes: Vec<VlanPrefix> = entries
        .iter()
        .filter(|entry| {
            record.prefix_ids.contains(&entry.prefix.id)
                || (entry.prefix.vlan_id == Some(record.vid) && entry.prefix.site == record.site)
        })
        .map(|entry| VlanPrefix {
            id: entry.prefix.id.clone(),
            cidr: entry.prefix.cidr.clone(),
        })
        .collect();
    prefixes.sort_by(|a, b| a.cidr.cmp(&b.cidr));
    prefixes.dedup_by(|a, b| a.id == b.id);
    prefixes
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct VlanInput {
    pub id: Option<String>,
    pub vid: Option<u16>,
    pub name: Option<String>,
    pub site: Option<String>,
    pub description: Option<String>,
    pub prefix_ids: Option<Vec<String>>,
}

pub fn save(app: &App, input: VlanInput) -> AppResult<VlanRow> {
    let existing = match &input.id {
        Some(id) if !id.is_empty() => {
            Some(app.store().require::<VlanRecord>(db::VLANS, id, "VLAN")?)
        }
        _ => None,
    };
    let mut record = existing.clone().unwrap_or_else(|| VlanRecord {
        id: db::new_id(),
        created_at: now_ms(),
        ..VlanRecord::default()
    });
    if let Some(vid) = input.vid {
        if !(1..=4094).contains(&vid) {
            return Err(AppError::invalid(format!(
                "{vid} is not a VLAN id (1 to 4094)"
            )));
        }
        record.vid = vid;
    }
    if record.vid == 0 {
        return Err(AppError::invalid("a VLAN record needs a VLAN id"));
    }
    if let Some(name) = input.name {
        record.name = name.trim().to_string();
    }
    if let Some(site) = input.site {
        record.site = (!site.trim().is_empty()).then(|| site.trim().to_string());
    }
    if let Some(description) = input.description {
        record.description = description;
    }
    if let Some(prefix_ids) = input.prefix_ids {
        let entries = prefixes::load(app)?;
        for id in &prefix_ids {
            prefixes::find(&entries, id)?;
        }
        record.prefix_ids = prefix_ids;
    }
    // Two records for one VLAN in one site would make "the" VLAN ambiguous.
    for other in app.store().list::<VlanRecord>(db::VLANS)? {
        if other.id != record.id && other.vid == record.vid && other.site == record.site {
            return Err(AppError::conflict(format!(
                "VLAN {} is already recorded{}",
                record.vid,
                record
                    .site
                    .as_ref()
                    .map(|site| format!(" in {site}"))
                    .unwrap_or_default()
            )));
        }
    }
    if existing.is_some() {
        record.source = Source::Manual;
    }
    record.updated_at = now_ms();
    app.store().put(db::VLANS, &record.id, &record)?;

    let sightings = crate::collected::vlan_sightings(app)?;
    let entries = prefixes::load(app)?;
    let devices = devices_for(&sightings, record.vid, record.site.as_deref());
    Ok(VlanRow {
        prefixes: prefixes_for(&entries, &record),
        observed: !devices.is_empty(),
        documented: true,
        devices,
        vlan: record,
    })
}

pub fn delete(app: &App, id: &str) -> AppResult<(bool, VlanRecord)> {
    let record = app.store().require::<VlanRecord>(db::VLANS, id, "VLAN")?;
    let deleted = app.store().delete(db::VLANS, id)?;
    Ok((deleted, record))
}
