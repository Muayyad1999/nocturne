//! Addresses: the record, the evidence, and the two held apart.
//!
//! [`compose`] is the one place where a record and the sightings of the same
//! address are put into a single row for display. It never lets one stand in
//! for the other: `documented` and `observed` say which halves exist,
//! `observations` carries the evidence with its device, interface and
//! collection time, and where the two disagree the row says so instead of
//! picking a winner.

use std::collections::BTreeMap;

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};

use crate::db;
use crate::model::{
    Address, AddressRecord, AddressSource, Disagreement, Observation, ObservationKind, Prefix,
    Settings, Status,
};
use crate::net::{self, Addr, Net};
use crate::prefixes::{self, Entry};

/// The most we will hand out in one `ipam.next_address` call.
const MAX_NEXT: usize = 1024;

/// Does this address fall in the prefix's DHCP range?
fn in_dhcp_range(prefix: &Prefix, value: u128, family: u8) -> bool {
    let Some(dhcp) = &prefix.dhcp else {
        return false;
    };
    match (net::addr(&dhcp.start), net::addr(&dhcp.end)) {
        (Ok(start), Ok(end)) => {
            start.family == family && value >= start.value && value <= end.value
        }
        _ => false,
    }
}

/// The sighting that gets to fill in the identity fields when there is no
/// record: an address configured on an interface beats one that merely
/// answered, and the most recently collected beats the rest.
fn strongest(observations: &[Observation]) -> Option<&Observation> {
    observations.iter().max_by_key(|observation| {
        (
            observation.kind == ObservationKind::Interface,
            observation.collected_at,
        )
    })
}

fn same_mac(left: &str, right: &str) -> bool {
    db::bare_mac(left).eq_ignore_ascii_case(&db::bare_mac(right))
}

/// One row for one address.
///
/// `conflict` is decided by [`crate::conflicts`], which can see the whole
/// estate; this function only reports it.
pub fn compose(
    address: &Addr,
    record: Option<&AddressRecord>,
    observations: &[Observation],
    prefix: Option<&Entry>,
    conflict: bool,
) -> Address {
    let live: Vec<&Observation> = observations.iter().collect();
    let observed = !live.is_empty();
    let best = strongest(observations);
    let first_seen = live.iter().map(|o| o.first_seen).min();
    let last_seen = live.iter().map(|o| o.last_seen).max();

    let status = if conflict {
        Status::Conflict
    } else if let Some(record) = record {
        record.status
    } else if observed {
        match prefix {
            Some(entry) if in_dhcp_range(&entry.prefix, address.value, address.family) => {
                Status::Dhcp
            }
            _ => Status::Active,
        }
    } else {
        // Nothing documented it and nothing has answered on it.
        Status::Free
    };

    let source = match (record, best) {
        (Some(_), _) => AddressSource::Manual,
        (None, Some(observation)) => match observation.kind {
            ObservationKind::Interface => AddressSource::Interface,
            ObservationKind::Arp => AddressSource::Arp,
        },
        // Nothing at all: the row exists because it is part of a block's grid.
        (None, None) => AddressSource::Manual,
    };

    let mut disagreements = Vec::new();
    if let Some(record) = record {
        for observation in observations {
            if let Some(documented) = &record.device_id
                && observation.kind == ObservationKind::Interface
                && documented != &observation.device_id
            {
                disagreements.push(Disagreement {
                    field: "device".into(),
                    documented: documented.clone(),
                    observed: observation.device_id.clone(),
                    device_id: observation.device_id.clone(),
                    device: observation.device.clone(),
                    interface: observation.interface.clone(),
                    collected_at: observation.collected_at,
                });
            } else if let Some(documented) = &record.interface
                && observation.kind == ObservationKind::Interface
                && record.device_id.as_deref() == Some(observation.device_id.as_str())
                && !documented.eq_ignore_ascii_case(&observation.interface)
            {
                disagreements.push(Disagreement {
                    field: "interface".into(),
                    documented: documented.clone(),
                    observed: observation.interface.clone(),
                    device_id: observation.device_id.clone(),
                    device: observation.device.clone(),
                    interface: observation.interface.clone(),
                    collected_at: observation.collected_at,
                });
            }
            if let (Some(documented), Some(seen)) = (&record.mac, &observation.mac)
                && !same_mac(documented, seen)
            {
                disagreements.push(Disagreement {
                    field: "mac".into(),
                    documented: documented.clone(),
                    observed: seen.clone(),
                    device_id: observation.device_id.clone(),
                    device: observation.device.clone(),
                    interface: observation.interface.clone(),
                    collected_at: observation.collected_at,
                });
            }
        }
    }

    Address {
        ip: address.text.clone(),
        prefix_id: record
            .and_then(|record| record.prefix_id.clone())
            .or_else(|| prefix.map(|entry| entry.prefix.id.clone())),
        status,
        hostname: record
            .map(|record| record.hostname.clone())
            .filter(|hostname| !hostname.is_empty()),
        description: record
            .map(|record| record.description.clone())
            .filter(|description| !description.is_empty()),
        device_id: record
            .and_then(|record| record.device_id.clone())
            .or_else(|| best.map(|observation| observation.device_id.clone())),
        interface: record
            .and_then(|record| record.interface.clone())
            .or_else(|| best.map(|observation| observation.interface.clone())),
        mac: record
            .and_then(|record| record.mac.clone())
            .or_else(|| best.and_then(|observation| observation.mac.clone())),
        vendor: best.and_then(|observation| observation.vendor.clone()),
        source,
        first_seen,
        last_seen,
        documented_at: record.map(|record| record.updated_at),
        tags: record.map(|record| record.tags.clone()).unwrap_or_default(),
        documented: record.is_some(),
        observed,
        observations: observations.to_vec(),
        disagreements,
    }
}

/// Every address of a prefix: the records, the sightings, and — for a block
/// small enough — the addresses with neither, so the classic grid can be
/// drawn. A larger block lists only what is known.
pub fn in_prefix(
    app: &App,
    settings: &Settings,
    entry: &Entry,
    entries: &[Entry],
    conflicting: &dyn Fn(&str, &str) -> bool,
) -> AppResult<Vec<Address>> {
    let scope = entry.scope.clone();
    let records = db::records_in(app.store(), &scope, &entry.net)?;
    let observations = db::observations_in(app.store(), &scope, &entry.net)?;
    let floor = prefixes::arp_floor(settings, now_ms());
    // The prefixes inside this one, so that a row's DHCP range and prefix id
    // come from the most specific block without walking the whole tree per
    // address: a /16 grid would otherwise be a quarter of a million compares.
    let inside: Vec<&Entry> = entries
        .iter()
        .filter(|candidate| {
            candidate.scope == scope
                && candidate.prefix.id != entry.prefix.id
                && entry.net.contains_net(&candidate.net)
        })
        .collect();
    let most_specific = |address: &Addr| -> Option<&Entry> {
        inside
            .iter()
            .filter(|candidate| candidate.net.contains_addr(address))
            .max_by_key(|candidate| candidate.net.len)
            .copied()
            .or(Some(entry))
    };

    let mut by_ip: BTreeMap<u128, (Option<AddressRecord>, Vec<Observation>)> = BTreeMap::new();
    for record in records {
        let value = net::addr(&record.ip)?.value;
        by_ip.entry(value).or_default().0 = Some(record);
    }
    for observation in observations {
        if observation.kind == ObservationKind::Arp && observation.last_seen < floor {
            continue;
        }
        let value = net::addr(&observation.ip)?.value;
        by_ip.entry(value).or_default().1.push(observation);
    }

    // The grid: a block with no more addresses than the setting allows is
    // listed in full, so that "nothing here" is visible as rows rather than
    // as a gap.
    let grid = entry.net.total() <= u64::from(settings.grid_max_addresses);
    let mut rows: Vec<Address> = Vec::new();
    if grid {
        let (first, last) = entry.net.host_range();
        // Anything with a record or a sighting outside the host range (a
        // record on the network address, say) is merged in rather than lost.
        let outside: Vec<u128> = by_ip
            .keys()
            .copied()
            .filter(|value| *value < first || *value > last)
            .collect();
        for value in outside {
            let (record, observations) = by_ip.remove(&value).unwrap_or_default();
            let address = Addr::new(entry.net.family, value);
            let conflict = conflicting(&scope, &address.text);
            rows.push(compose(
                &address,
                record.as_ref(),
                &observations,
                most_specific(&address),
                conflict,
            ));
        }
        let mut value = first;
        loop {
            let address = Addr::new(entry.net.family, value);
            let (record, observations) = by_ip.remove(&value).unwrap_or_default();
            let conflict = conflicting(&scope, &address.text);
            rows.push(compose(
                &address,
                record.as_ref(),
                &observations,
                most_specific(&address),
                conflict,
            ));
            if value == last {
                break;
            }
            value += 1;
        }
        // The merged-in rows go to their place in address order.
        rows.sort_by_key(|row| net::addr(&row.ip).map(|address| address.value).unwrap_or(0));
        return Ok(rows);
    }
    // A block too large to grid lists only what is known about it, which is
    // already in address order.
    for (value, (record, observations)) in by_ip {
        let address = Addr::new(entry.net.family, value);
        let conflict = conflicting(&scope, &address.text);
        rows.push(compose(
            &address,
            record.as_ref(),
            &observations,
            most_specific(&address),
            conflict,
        ));
    }
    Ok(rows)
}

/// The addresses of a block that something claims — a record or a sighting —
/// as numbers, for the free-space arithmetic.
pub fn occupied_values(
    app: &App,
    settings: &Settings,
    scope: &str,
    block: &Net,
) -> AppResult<Vec<u128>> {
    let floor = prefixes::arp_floor(settings, now_ms());
    let mut values = Vec::new();
    for record in db::records_in(app.store(), scope, block)? {
        values.push(net::addr(&record.ip)?.value);
    }
    for observation in db::observations_in(app.store(), scope, block)? {
        if observation.kind == ObservationKind::Arp && observation.last_seen < floor {
            continue;
        }
        values.push(net::addr(&observation.ip)?.value);
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

/// The same, read off rows that have already been composed.
pub fn occupied_in(rows: &[Address]) -> Vec<u128> {
    let mut values: Vec<u128> = rows
        .iter()
        .filter(|row| row.documented || row.observed)
        .filter_map(|row| net::addr(&row.ip).ok().map(|address| address.value))
        .collect();
    values.sort_unstable();
    values.dedup();
    values
}

// ------------------------------------------------------------ next free

/// Why an address was passed over. Saying this is the difference between
/// "here is a free address" and "here is a free address, and here is what I
/// knew when I said so".
#[derive(Debug, Default, serde::Serialize)]
pub struct Skipped {
    pub network: u32,
    pub broadcast: u32,
    /// The Subnet-Router anycast address of an IPv6 subnet.
    pub anycast: u32,
    pub gateway: u32,
    pub dhcp_range: u32,
    pub documented: u32,
    pub seen_on_an_interface: u32,
    pub seen_in_arp: u32,
    pub asked_to_skip: u32,
}

#[derive(Debug, serde::Serialize)]
pub struct NextAddresses {
    pub prefix_id: String,
    pub cidr: String,
    pub addresses: Vec<String>,
    /// Fewer than asked for, because the block ran out.
    pub exhausted: bool,
    pub skipped: Skipped,
    /// What the answer rests on: an address nothing has answered on is not
    /// known to be free, it is known not to have been seen.
    pub basis: String,
}

pub fn next_free(
    app: &App,
    settings: &Settings,
    entry: &Entry,
    count: usize,
    skip_first: u64,
) -> AppResult<NextAddresses> {
    let count = count.clamp(1, MAX_NEXT);
    let floor = prefixes::arp_floor(settings, now_ms());
    let scope = &entry.scope;
    let block = &entry.net;

    let mut skipped = Skipped::default();
    // What is claimed, as a sorted list of numbers with why.
    let mut claimed: BTreeMap<u128, &'static str> = BTreeMap::new();
    for record in db::records_in(app.store(), scope, block)? {
        claimed.insert(net::addr(&record.ip)?.value, "documented");
    }
    let observations = db::observations_in(app.store(), scope, block)?;
    for observation in &observations {
        if observation.kind == ObservationKind::Arp && observation.last_seen < floor {
            continue;
        }
        let value = net::addr(&observation.ip)?.value;
        let why = match observation.kind {
            ObservationKind::Interface => "interface",
            ObservationKind::Arp => "arp",
        };
        // A documented address stays "documented" however it was also seen;
        // between sightings, an address configured on an interface outranks
        // one that merely answered.
        match claimed.get(&value) {
            None => {
                claimed.insert(value, why);
            }
            Some(&"arp") if observation.kind == ObservationKind::Interface => {
                claimed.insert(value, why);
            }
            Some(_) => {}
        }
    }
    if let Some(gateway) = &entry.prefix.gateway {
        claimed.insert(net::addr(gateway)?.value, "gateway");
    }

    let (first, last) = block.host_range();
    let mut value = match first.checked_add(u128::from(skip_first)) {
        Some(start) if start <= last => start,
        _ => {
            skipped.asked_to_skip = u32::try_from(skip_first).unwrap_or(u32::MAX);
            return Ok(NextAddresses {
                prefix_id: entry.prefix.id.clone(),
                cidr: block.cidr(),
                addresses: Vec::new(),
                exhausted: true,
                skipped,
                basis: basis_text(&observations, floor),
            });
        }
    };
    skipped.asked_to_skip = u32::try_from(skip_first).unwrap_or(u32::MAX);
    if block.family == 4 && block.len <= 30 {
        skipped.network = 1;
        skipped.broadcast = 1;
    }
    if block.family == 6 && block.len < 128 {
        skipped.anycast = 1;
    }

    let mut addresses = Vec::new();
    while addresses.len() < count && value <= last {
        if let Some(dhcp) = &entry.prefix.dhcp
            && let (Ok(start), Ok(end)) = (net::addr(&dhcp.start), net::addr(&dhcp.end))
            && value >= start.value
            && value <= end.value
        {
            skipped.dhcp_range += u32::try_from(end.value - value + 1).unwrap_or(u32::MAX);
            match end.value.checked_add(1) {
                Some(next) => value = next,
                None => break,
            }
            continue;
        }
        match claimed.get(&value) {
            Some(&"documented") => skipped.documented += 1,
            Some(&"interface") => skipped.seen_on_an_interface += 1,
            Some(&"arp") => skipped.seen_in_arp += 1,
            Some(&"gateway") => skipped.gateway += 1,
            Some(_) => {}
            None => addresses.push(net::text_of(block.family, value)),
        }
        match value.checked_add(1) {
            Some(next) => value = next,
            None => break,
        }
    }
    Ok(NextAddresses {
        prefix_id: entry.prefix.id.clone(),
        cidr: block.cidr(),
        exhausted: addresses.len() < count,
        addresses,
        skipped,
        basis: basis_text(&observations, floor),
    })
}

fn basis_text(observations: &[Observation], floor: i64) -> String {
    let fresh = observations
        .iter()
        .filter(|observation| {
            observation.kind == ObservationKind::Interface || observation.last_seen >= floor
        })
        .count();
    let newest = observations
        .iter()
        .map(|observation| observation.collected_at)
        .max();
    match (fresh, newest) {
        (0, _) => "nothing has ever been seen in this prefix: these addresses are not documented, which is not the same as known to be free".to_string(),
        (count, Some(at)) => format!(
            "{count} sighting(s) in this prefix, last collected at {at}; an address nothing has answered on is not known to be free"
        ),
        (count, None) => format!("{count} sighting(s) in this prefix"),
    }
}

// ----------------------------------------------------------- save / delete

#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct AddressInput {
    pub ip: Option<String>,
    pub prefix_id: Option<String>,
    pub status: Option<Status>,
    pub hostname: Option<String>,
    pub description: Option<String>,
    pub device_id: Option<String>,
    pub interface: Option<String>,
    pub mac: Option<String>,
    pub tags: Option<Vec<String>>,
    /// `""` (production) or `lab:<lab id>`; taken from the prefix when left out.
    pub scope: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct SavedAddress {
    pub address: Address,
    pub created: bool,
    pub warnings: Vec<String>,
}

/// Write an address record. This is the only way a record is created: no
/// sync, no import of collected data, nothing automatic.
pub fn save(app: &App, settings: &Settings, input: AddressInput) -> AppResult<SavedAddress> {
    let ip = input
        .ip
        .clone()
        .ok_or_else(|| AppError::invalid("an address record needs an address"))?;
    let address = net::addr(&ip)?;
    let entries = prefixes::load(app)?;

    let mut warnings = Vec::new();
    let (scope, prefix_id) = match &input.prefix_id {
        Some(id) if !id.is_empty() => {
            let entry = prefixes::find(&entries, id)?;
            if !entry.net.contains_addr(&address) {
                return Err(AppError::invalid(format!(
                    "{} is not inside {} ({})",
                    address.text,
                    entry.net.cidr(),
                    match prefixes::containing(&entries, &entry.scope, &address) {
                        Some(other) => format!("it is inside {}", other.net.cidr()),
                        None => "no prefix on record contains it".to_string(),
                    }
                )));
            }
            if !entry.net.is_usable_host(address.value) {
                warnings.push(format!(
                    "{} is the network, broadcast or anycast address of {}",
                    address.text,
                    entry.net.cidr()
                ));
            }
            (entry.scope.clone(), Some(id.clone()))
        }
        _ => {
            let scope = input.scope.clone().unwrap_or_default();
            match prefixes::containing(&entries, &scope, &address) {
                Some(entry) => (scope, Some(entry.prefix.id.clone())),
                None => {
                    warnings.push(format!(
                        "no prefix on record contains {}: the address is kept, unfiled",
                        address.text
                    ));
                    (scope, None)
                }
            }
        }
    };

    let existing = db::record(app.store(), &scope, &address.text)?;
    let created = existing.is_none();
    let mut record = existing.unwrap_or_else(|| AddressRecord {
        ip: address.text.clone(),
        scope: scope.clone(),
        created_at: now_ms(),
        ..AddressRecord::default()
    });
    record.prefix_id = prefix_id;
    if let Some(status) = input.status {
        if matches!(status, Status::Free | Status::Conflict) {
            return Err(AppError::invalid(
                "a record can be `active`, `reserved` or `dhcp`: `free` is the absence of one and `conflict` is a finding",
            ));
        }
        record.status = status;
    }
    if let Some(hostname) = input.hostname {
        record.hostname = hostname.trim().to_string();
    }
    if let Some(description) = input.description {
        record.description = description;
    }
    if let Some(device_id) = input.device_id {
        record.device_id = (!device_id.is_empty()).then_some(device_id);
    }
    if let Some(interface) = input.interface {
        record.interface = (!interface.trim().is_empty()).then(|| interface.trim().to_string());
    }
    if let Some(mac) = input.mac {
        record.mac = match mac.trim() {
            "" => None,
            text => Some(
                nocturne_tools::mac::info(text)
                    .map(|info| info.colon)
                    .map_err(net::tool_error)?,
            ),
        };
    }
    if let Some(tags) = input.tags {
        record.tags = tags;
    }
    record.updated_at = now_ms();
    db::put_record(app.store(), &record)?;

    let observations = db::observations_of_ip(app.store(), &scope, &address.text)?;
    let entry = prefix_entry(&entries, &record, &scope, &address);
    let address = compose(&address, Some(&record), &observations, entry, false);
    let _ = settings;
    Ok(SavedAddress {
        address,
        created,
        warnings,
    })
}

fn prefix_entry<'a>(
    entries: &'a [Entry],
    record: &AddressRecord,
    scope: &str,
    address: &Addr,
) -> Option<&'a Entry> {
    record
        .prefix_id
        .as_ref()
        .and_then(|id| entries.iter().find(|entry| &entry.prefix.id == id))
        .or_else(|| prefixes::containing(entries, scope, address))
}

/// Hold an address back. An address that is already in use is a conflict
/// naming who has it — the reservation is refused rather than written over
/// what the network says.
pub fn reserve(
    app: &App,
    settings: &Settings,
    prefix_id: &str,
    ip: Option<&str>,
    hostname: &str,
    description: &str,
) -> AppResult<SavedAddress> {
    let entries = prefixes::load(app)?;
    let entry = prefixes::find(&entries, prefix_id)?;
    let address = match ip {
        Some(ip) => net::addr(ip)?,
        None => {
            let next = next_free(app, settings, entry, 1, 0)?;
            let first = next.addresses.first().ok_or_else(|| {
                AppError::conflict(format!(
                    "{} has no address left that is neither documented nor seen",
                    entry.net.cidr()
                ))
            })?;
            net::addr(first)?
        }
    };
    if !entry.net.contains_addr(&address) {
        return Err(AppError::invalid(format!(
            "{} is not inside {}",
            address.text,
            entry.net.cidr()
        )));
    }
    // A record of any kind means somebody has already spoken for the
    // address; changing what it says is `ipam.address.save`, not a second
    // reservation quietly winning.
    if let Some(existing) = db::record(app.store(), &entry.scope, &address.text)? {
        return Err(AppError::conflict(format!(
            "{} is already recorded{}{}",
            address.text,
            match (&existing.hostname, &existing.device_id) {
                (hostname, _) if !hostname.is_empty() => format!(" as `{hostname}`"),
                (_, Some(device)) => format!(" against device `{device}`"),
                _ => String::new(),
            },
            match existing.status {
                Status::Reserved => " (reserved)",
                Status::Dhcp => " (in the DHCP range)",
                _ => "",
            }
        ))
        .with_details(serde_json::json!({"kind": "duplicate_ip", "ip": address.text})));
    }
    let observations = db::observations_of_ip(app.store(), &entry.scope, &address.text)?;
    let floor = prefixes::arp_floor(settings, now_ms());
    if let Some(holder) = observations.iter().find(|observation| {
        observation.kind == ObservationKind::Interface || observation.last_seen >= floor
    }) {
        return Err(AppError::conflict(format!(
            "{} is in use: {} has it on {} ({}), collected at {}",
            address.text,
            holder.device,
            holder.interface,
            holder.kind.as_str(),
            holder.collected_at
        ))
        .with_details(serde_json::json!({
            "kind": "duplicate_ip",
            "ip": address.text,
            "involved": observations,
        })));
    }
    save(
        app,
        settings,
        AddressInput {
            ip: Some(address.text),
            prefix_id: Some(prefix_id.to_string()),
            status: Some(Status::Reserved),
            hostname: Some(hostname.to_string()),
            description: Some(description.to_string()),
            ..AddressInput::default()
        },
    )
}

/// Delete the *record*. The sightings stay: the network does not stop saying
/// what it says because a plan was corrected.
#[derive(Debug, serde::Serialize)]
pub struct DeletedAddress {
    pub deleted: bool,
    pub ip: String,
    /// Sightings left behind, which is why the address may still be listed.
    pub observations_left: usize,
}

pub fn delete(app: &App, scope: &str, ip: &str) -> AppResult<DeletedAddress> {
    let address = net::addr(ip)?;
    let deleted = db::delete_record(app.store(), scope, &address.text)?;
    let left = db::observations_of_ip(app.store(), scope, &address.text)?.len();
    Ok(DeletedAddress {
        deleted,
        ip: address.text,
        observations_left: left,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(ip: &str, kind: ObservationKind, device: &str, interface: &str) -> Observation {
        Observation {
            ip: ip.to_string(),
            kind,
            device_id: format!("id-{device}"),
            device: device.to_string(),
            interface: interface.to_string(),
            prefix_len: (kind == ObservationKind::Interface).then_some(24),
            cidr: (kind == ObservationKind::Interface).then(|| format!("{ip}/24")),
            mac: None,
            vendor: None,
            scope: String::new(),
            first_seen: 1_000,
            last_seen: 2_000,
            collected_at: 2_000,
            collector: "cli".into(),
            current: true,
        }
    }

    #[test]
    fn an_address_nobody_has_seen_or_written_down_is_free_not_available() {
        let address = net::addr("10.1.10.50").unwrap();
        let row = compose(&address, None, &[], None, false);
        assert_eq!(row.status, Status::Free);
        assert!(!row.documented && !row.observed);
        assert_eq!((row.first_seen, row.last_seen), (None, None));
        assert!(row.observations.is_empty(), "there is no evidence to show");
    }

    #[test]
    fn a_record_and_a_sighting_stay_distinguishable_in_one_row() {
        let address = net::addr("10.1.10.5").unwrap();
        let record = AddressRecord {
            ip: "10.1.10.5".into(),
            hostname: "printer-3".into(),
            status: Status::Active,
            updated_at: 500,
            ..AddressRecord::default()
        };
        let seen = observation("10.1.10.5", ObservationKind::Arp, "SW1", "Vlan10");

        let documented_only = compose(&address, Some(&record), &[], None, false);
        assert!(documented_only.documented && !documented_only.observed);
        assert_eq!(documented_only.source, AddressSource::Manual);
        assert_eq!(
            documented_only.last_seen, None,
            "nothing has answered on it"
        );
        assert_eq!(documented_only.documented_at, Some(500));

        let observed_only = compose(&address, None, std::slice::from_ref(&seen), None, false);
        assert!(!observed_only.documented && observed_only.observed);
        assert_eq!(observed_only.source, AddressSource::Arp);
        assert_eq!(observed_only.last_seen, Some(2_000));
        assert_eq!(observed_only.hostname, None, "nobody named it");

        let both = compose(&address, Some(&record), &[seen], None, false);
        assert!(both.documented && both.observed);
        assert_eq!(
            both.source,
            AddressSource::Manual,
            "the record is the identity"
        );
        assert_eq!(both.hostname.as_deref(), Some("printer-3"));
        assert_eq!(both.observations.len(), 1, "the evidence is still there");
    }

    #[test]
    fn a_record_that_contradicts_the_network_is_reported_not_overwritten() {
        let address = net::addr("10.1.10.1").unwrap();
        let record = AddressRecord {
            ip: "10.1.10.1".into(),
            device_id: Some("id-SW1".into()),
            interface: Some("Vlan10".into()),
            mac: Some("aa:bb:cc:dd:ee:ff".into()),
            ..AddressRecord::default()
        };
        let mut seen = observation("10.1.10.1", ObservationKind::Interface, "SW2", "Vlan10");
        seen.mac = Some("00:11:22:33:44:55".into());
        let row = compose(&address, Some(&record), &[seen], None, false);
        assert_eq!(
            row.device_id.as_deref(),
            Some("id-SW1"),
            "the record still says SW1"
        );
        let fields: Vec<&str> = row
            .disagreements
            .iter()
            .map(|disagreement| disagreement.field.as_str())
            .collect();
        assert_eq!(fields, ["device", "mac"]);
        assert_eq!(row.disagreements[0].observed, "id-SW2");

        // The same device on another interface is an interface disagreement.
        let moved = observation("10.1.10.1", ObservationKind::Interface, "SW1", "Vlan20");
        let row = compose(&address, Some(&record), &[moved], None, false);
        assert_eq!(row.disagreements.len(), 1);
        assert_eq!(row.disagreements[0].field, "interface");

        // A MAC in another notation is the same MAC.
        let mut same = observation("10.1.10.1", ObservationKind::Interface, "SW1", "Vlan10");
        same.mac = Some("AABB.CCDD.EEFF".into());
        assert!(
            compose(&address, Some(&record), &[same], None, false)
                .disagreements
                .is_empty()
        );
    }

    #[test]
    fn the_interface_sighting_outranks_the_arp_one() {
        let address = net::addr("10.1.10.1").unwrap();
        let arp = observation("10.1.10.1", ObservationKind::Arp, "SW2", "Vlan10");
        let configured = observation("10.1.10.1", ObservationKind::Interface, "SW1", "Vlan10");
        let row = compose(&address, None, &[arp, configured], None, false);
        assert_eq!(row.source, AddressSource::Interface);
        assert_eq!(
            row.device_id.as_deref(),
            Some("id-SW1"),
            "the device that has the address configured, not the one that saw it"
        );
        assert_eq!(row.observations.len(), 2, "both sightings are kept");
    }
}
