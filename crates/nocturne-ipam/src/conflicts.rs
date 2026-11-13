//! Things that cannot all be true: one address on two devices, prefixes that
//! overlap, an address outside the prefix it claims, a gateway nobody is
//! configured with.
//!
//! Every conflict carries what it was decided from, and none of them is ever
//! resolved by this crate. Choosing which side is right is a change to a
//! device or to a record, and both of those are somebody's decision.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppResult};

use crate::db;
use crate::model::{
    AddressRecord, Conflict, ConflictKind, Involved, Observation, ObservationKind, Settings,
};
use crate::net;
use crate::prefixes::{self, Entry};

/// The redundancy protocol a well-known virtual MAC belongs to, if any.
///
/// A virtual address is shared on purpose: two routers answering for it is
/// the feature working, not a duplicate.
pub fn virtual_protocol(mac: &str) -> Option<&'static str> {
    let bare = db::bare_mac(mac).to_ascii_lowercase();
    if bare.len() != 12 {
        return None;
    }
    // Ranges from the protocol specifications: the last byte (or nibbles) is
    // the group number.
    if bare.starts_with("00000c07ac") {
        return Some("HSRP");
    }
    if bare.starts_with("00000c9ff") {
        return Some("HSRPv2");
    }
    if bare.starts_with("000573a00") {
        return Some("HSRPv2 (IPv6)");
    }
    if bare.starts_with("00005e0001") {
        return Some("VRRP");
    }
    if bare.starts_with("00005e0002") {
        return Some("VRRPv3 (IPv6)");
    }
    if bare.starts_with("0007b400") {
        return Some("GLBP");
    }
    None
}

/// How many unfiled records one report names. A plan where thousands of
/// addresses sit in no prefix has one problem, not thousands of them.
const MAX_UNFILED: usize = 500;

/// Every conflict, plus a quick index so an address row can say it is in one.
#[derive(Debug, Default)]
pub struct Report {
    pub conflicts: Vec<Conflict>,
    conflicting: BTreeSet<(String, String)>,
}

impl Report {
    pub fn has(&self, scope: &str, ip: &str) -> bool {
        self.conflicting
            .contains(&(scope.to_string(), ip.to_string()))
    }

    fn add_address(&mut self, scope: &str, ip: &str, conflict: Conflict) {
        self.conflicting.insert((scope.to_string(), ip.to_string()));
        self.conflicts.push(conflict);
    }
}

/// Look for every kind of conflict across the whole estate.
pub fn detect(app: &App, settings: &Settings, entries: &[Entry]) -> AppResult<Report> {
    let mut report = Report::default();
    let now = now_ms();
    let floor = prefixes::arp_floor(settings, now);

    // ------------------------------------------------ one address, two holders
    let interfaces = db::observations_of_kind(app.store(), ObservationKind::Interface)?;
    let mut by_address: BTreeMap<(String, String), Vec<Observation>> = BTreeMap::new();
    for observation in interfaces {
        by_address
            .entry((observation.scope.clone(), observation.ip.clone()))
            .or_default()
            .push(observation);
    }
    for ((scope, ip), observations) in &by_address {
        let devices: BTreeSet<&str> = observations
            .iter()
            .map(|observation| observation.device_id.as_str())
            .collect();
        if devices.len() < 2 {
            continue;
        }
        // A virtual address is shared deliberately; the MAC that answers for
        // it says so.
        let virtual_by = if settings.recognise_virtual_macs {
            db::observations_of_ip(app.store(), scope, ip)?
                .iter()
                .filter(|observation| observation.kind == ObservationKind::Arp)
                .find_map(|observation| observation.mac.as_deref().and_then(virtual_protocol))
        } else {
            None
        };
        if virtual_by.is_some() {
            continue;
        }
        let holders: Vec<String> = observations
            .iter()
            .map(|observation| format!("{} ({})", observation.device, observation.interface))
            .collect();
        report.add_address(
            scope,
            ip,
            Conflict {
                kind: ConflictKind::DuplicateIp,
                subject: ip.clone(),
                detail: format!(
                    "{ip} is configured on {} different devices: {}",
                    devices.len(),
                    holders.join(", ")
                ),
                involved: observations.iter().map(Involved::observation).collect(),
            },
        );
    }

    // -------------------------------- one address, two MACs within the window
    let arp = db::observations_of_kind(app.store(), ObservationKind::Arp)?;
    let window = i64::from(settings.mac_change_window_minutes) * 60 * 1000;
    let mut arp_by_address: BTreeMap<(String, String), Vec<Observation>> = BTreeMap::new();
    for observation in arp {
        if observation.last_seen < floor {
            continue;
        }
        arp_by_address
            .entry((observation.scope.clone(), observation.ip.clone()))
            .or_default()
            .push(observation);
    }
    for ((scope, ip), observations) in &arp_by_address {
        if report.has(scope, ip) {
            continue;
        }
        let mut macs: BTreeMap<String, &Observation> = BTreeMap::new();
        for observation in observations {
            if let Some(mac) = &observation.mac {
                macs.entry(db::bare_mac(mac).to_ascii_lowercase())
                    .or_insert(observation);
            }
        }
        if macs.len() < 2 {
            continue;
        }
        if settings.recognise_virtual_macs && macs.keys().any(|mac| virtual_protocol(mac).is_some())
        {
            continue;
        }
        let seen: Vec<i64> = macs
            .values()
            .map(|observation| observation.last_seen)
            .collect();
        let spread = seen.iter().max().unwrap_or(&0) - seen.iter().min().unwrap_or(&0);
        if spread > window {
            // Far enough apart to be a lease that moved on, not two hosts
            // claiming one address at the same time.
            continue;
        }
        let described: Vec<String> = macs
            .values()
            .map(|observation| {
                format!(
                    "{} on {} ({})",
                    observation.mac.clone().unwrap_or_default(),
                    observation.device,
                    observation.interface
                )
            })
            .collect();
        report.add_address(
            scope,
            ip,
            Conflict {
                kind: ConflictKind::DuplicateIp,
                subject: ip.clone(),
                detail: format!(
                    "{ip} answered with {} different MAC addresses within {} minutes: {}",
                    macs.len(),
                    settings.mac_change_window_minutes,
                    described.join(", ")
                ),
                involved: macs.values().map(|o| Involved::observation(o)).collect(),
            },
        );
    }

    // ------------------------------------------------------ overlapping blocks
    for (index, entry) in entries.iter().enumerate() {
        for other in entries.iter().skip(index + 1) {
            if other.scope != entry.scope || other.net.family != entry.net.family {
                continue;
            }
            if other.net.base > entry.net.last() {
                // Entries are in address order, so nothing further can overlap.
                break;
            }
            if entry.net.contains_net(&other.net) || other.net.contains_net(&entry.net) {
                continue;
            }
            if entry.net.overlaps(&other.net) {
                report.conflicts.push(Conflict {
                    kind: ConflictKind::Overlap,
                    subject: entry.prefix.cidr.clone(),
                    detail: format!(
                        "{} overlaps {} without either containing the other",
                        entry.prefix.cidr, other.prefix.cidr
                    ),
                    involved: vec![
                        Involved {
                            prefix_id: Some(entry.prefix.id.clone()),
                            cidr: Some(entry.prefix.cidr.clone()),
                            ..Involved::record()
                        },
                        Involved {
                            prefix_id: Some(other.prefix.id.clone()),
                            cidr: Some(other.prefix.cidr.clone()),
                            ..Involved::record()
                        },
                    ],
                });
            }
        }
    }

    // ------------------------------------- a record outside the prefix it claims
    for entry in entries {
        for record in db::records_outside(app.store(), &entry.prefix.id, &entry.net)? {
            let Ok(address) = net::addr(&record.ip) else {
                continue;
            };
            report.add_address(
                &record.scope,
                &record.ip,
                Conflict {
                    kind: ConflictKind::OutsideAnyPrefix,
                    subject: record.ip.clone(),
                    detail: format!(
                        "{} is recorded in {} but is not inside it{}",
                        record.ip,
                        entry.prefix.cidr,
                        match prefixes::containing(entries, &record.scope, &address) {
                            Some(real) => format!(" (it is inside {})", real.prefix.cidr),
                            None => String::new(),
                        }
                    ),
                    involved: vec![Involved {
                        ip: Some(record.ip.clone()),
                        prefix_id: Some(entry.prefix.id.clone()),
                        cidr: Some(entry.prefix.cidr.clone()),
                        ..Involved::record()
                    }],
                },
            );
        }
    }
    // A record that names no prefix, and that none contains.
    for record in db::unfiled_records(app.store(), MAX_UNFILED)? {
        let Ok(address) = net::addr(&record.ip) else {
            continue;
        };
        if prefixes::containing(entries, &record.scope, &address).is_some() {
            continue;
        }
        report.add_address(
            &record.scope,
            &record.ip,
            Conflict {
                kind: ConflictKind::OutsideAnyPrefix,
                subject: record.ip.clone(),
                detail: format!(
                    "{} is documented but no prefix on record contains it",
                    record.ip
                ),
                involved: vec![Involved {
                    ip: Some(record.ip.clone()),
                    ..Involved::record()
                }],
            },
        );
    }

    // -------------------------------------------------- the documented gateway
    for entry in entries {
        let Some(gateway) = &entry.prefix.gateway else {
            continue;
        };
        let Ok(documented) = net::addr(gateway) else {
            continue;
        };
        let inside = db::interfaces_in(app.store(), &entry.scope, &entry.net, 200)?;
        if inside.is_empty() {
            // Nothing is configured in this prefix, so there is nothing to
            // disagree with. Saying "mismatch" here would be inventing a fault.
            continue;
        }
        if inside
            .iter()
            .any(|observation| observation.ip == documented.text)
        {
            continue;
        }
        let configured: Vec<String> = inside
            .iter()
            .map(|observation| {
                format!(
                    "{} on {} ({})",
                    observation.ip, observation.device, observation.interface
                )
            })
            .collect();
        report.conflicts.push(Conflict {
            kind: ConflictKind::GatewayMismatch,
            subject: entry.prefix.cidr.clone(),
            detail: format!(
                "{} says its gateway is {}, but nothing in it is configured with that address; what is configured: {}",
                entry.prefix.cidr,
                documented.text,
                configured.join(", ")
            ),
            involved: std::iter::once(Involved {
                prefix_id: Some(entry.prefix.id.clone()),
                cidr: Some(entry.prefix.cidr.clone()),
                ip: Some(documented.text.clone()),
                ..Involved::record()
            })
            .chain(inside.iter().map(Involved::observation))
            .collect(),
        });
    }

    report.conflicts.sort_by(|a, b| {
        (format!("{:?}", a.kind), &a.subject).cmp(&(format!("{:?}", b.kind), &b.subject))
    });
    Ok(report)
}

/// The addresses *inside one block* that two devices both have configured, or
/// that answered with two different MACs at once.
///
/// This is the part of [`detect`] a prefix's address list needs, and it is
/// one range query rather than a walk of the estate.
pub fn in_block(
    app: &App,
    settings: &Settings,
    scope: &str,
    block: &crate::net::Net,
) -> AppResult<BTreeSet<String>> {
    let floor = prefixes::arp_floor(settings, now_ms());
    let mut devices_by_ip: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut macs_by_ip: BTreeMap<String, BTreeMap<String, i64>> = BTreeMap::new();
    for observation in db::observations_in(app.store(), scope, block)? {
        match observation.kind {
            ObservationKind::Interface => {
                devices_by_ip
                    .entry(observation.ip.clone())
                    .or_default()
                    .insert(observation.device_id.clone());
            }
            ObservationKind::Arp => {
                if observation.last_seen < floor {
                    continue;
                }
                if let Some(mac) = &observation.mac {
                    macs_by_ip
                        .entry(observation.ip.clone())
                        .or_default()
                        .insert(
                            db::bare_mac(mac).to_ascii_lowercase(),
                            observation.last_seen,
                        );
                }
            }
        }
    }
    let window = i64::from(settings.mac_change_window_minutes) * 60 * 1000;
    let mut conflicting = BTreeSet::new();
    for (ip, devices) in devices_by_ip {
        if devices.len() < 2 {
            continue;
        }
        if settings.recognise_virtual_macs
            && macs_by_ip
                .get(&ip)
                .is_some_and(|macs| macs.keys().any(|mac| virtual_protocol(mac).is_some()))
        {
            continue;
        }
        conflicting.insert(ip);
    }
    for (ip, macs) in macs_by_ip {
        if macs.len() < 2 || conflicting.contains(&ip) {
            continue;
        }
        if settings.recognise_virtual_macs && macs.keys().any(|mac| virtual_protocol(mac).is_some())
        {
            continue;
        }
        let seen: Vec<i64> = macs.values().copied().collect();
        let spread = seen.iter().max().unwrap_or(&0) - seen.iter().min().unwrap_or(&0);
        if spread <= window {
            conflicting.insert(ip);
        }
    }
    Ok(conflicting)
}

/// Whether this one address is in a conflict: two devices with it configured,
/// two MACs answering for it, or a record that claims a prefix it is not in.
///
/// Pure: it is given the evidence rather than fetching it, so a list of two
/// hundred search hits costs two hundred queries and not six hundred.
pub fn for_evidence(
    settings: &Settings,
    entries: &[Entry],
    scope: &str,
    record: Option<&AddressRecord>,
    observations: &[Observation],
    now: i64,
) -> bool {
    let floor = prefixes::arp_floor(settings, now);
    let devices: BTreeSet<&str> = observations
        .iter()
        .filter(|observation| observation.kind == ObservationKind::Interface)
        .map(|observation| observation.device_id.as_str())
        .collect();
    let macs: BTreeMap<String, i64> = observations
        .iter()
        .filter(|observation| {
            observation.kind == ObservationKind::Arp && observation.last_seen >= floor
        })
        .filter_map(|observation| {
            observation.mac.as_ref().map(|mac| {
                (
                    db::bare_mac(mac).to_ascii_lowercase(),
                    observation.last_seen,
                )
            })
        })
        .collect();
    let virtual_address =
        settings.recognise_virtual_macs && macs.keys().any(|mac| virtual_protocol(mac).is_some());
    if devices.len() >= 2 && !virtual_address {
        return true;
    }
    if macs.len() >= 2 && !virtual_address {
        let window = i64::from(settings.mac_change_window_minutes) * 60 * 1000;
        let seen: Vec<i64> = macs.values().copied().collect();
        if seen.iter().max().unwrap_or(&0) - seen.iter().min().unwrap_or(&0) <= window {
            return true;
        }
    }
    if let Some(record) = record
        && let Ok(address) = net::addr(&record.ip)
    {
        match &record.prefix_id {
            Some(id) => {
                if let Some(entry) = entries.iter().find(|entry| &entry.prefix.id == id)
                    && !entry.net.contains_addr(&address)
                {
                    return true;
                }
            }
            None => {
                if prefixes::containing(entries, scope, &address).is_none() {
                    return true;
                }
            }
        }
    }
    false
}

/// [`for_evidence`], fetching the evidence itself.
pub fn for_ip(
    app: &App,
    settings: &Settings,
    entries: &[Entry],
    scope: &str,
    ip: &str,
) -> AppResult<bool> {
    let record = db::record(app.store(), scope, ip)?;
    let observations = db::observations_of_ip(app.store(), scope, ip)?;
    Ok(for_evidence(
        settings,
        entries,
        scope,
        record.as_ref(),
        &observations,
        now_ms(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_well_known_virtual_macs_are_recognised() {
        assert_eq!(virtual_protocol("0000.0c07.ac0a"), Some("HSRP"));
        assert_eq!(virtual_protocol("00:00:0c:9f:f0:01"), Some("HSRPv2"));
        assert_eq!(virtual_protocol("0005.73a0.0001"), Some("HSRPv2 (IPv6)"));
        assert_eq!(virtual_protocol("00-00-5E-00-01-2A"), Some("VRRP"));
        assert_eq!(virtual_protocol("0000.5e00.0201"), Some("VRRPv3 (IPv6)"));
        assert_eq!(virtual_protocol("0007.b400.0a01"), Some("GLBP"));
        // A real card is not a virtual address.
        assert_eq!(virtual_protocol("0000.0c12.3456"), None);
        assert_eq!(virtual_protocol("aa:bb:cc:dd:ee:ff"), None);
        assert_eq!(virtual_protocol("nonsense"), None);
    }
}
