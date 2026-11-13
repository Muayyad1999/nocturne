//! Reconciliation: the plan on one side, the network on the other.
//!
//! Three calls live here and they are deliberately different things.
//!
//! * [`sync`] refreshes the **evidence**. Collected interface addresses and
//!   ARP entries become observations; an interface address that is no longer
//!   configured disappears; an ARP sighting nobody has refreshed within the
//!   window is forgotten. The only records it writes are `discovered`
//!   prefixes, which say in their own `source` that nobody has confirmed
//!   them, and it never touches a field a person filled in.
//! * [`reconcile`] compares the two and **reports**. Nothing is written. It
//!   also says what it was looking at — which devices were collected and
//!   when — because "nothing has answered on this address" means nothing
//!   without that.
//! * [`learn`] is the one call that turns evidence into records, and it shows
//!   what it would create before it creates anything.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppResult};
use serde::Serialize;

use crate::collected::{self, DeviceCoverage, Harvest};
use crate::conflicts;
use crate::db;
use crate::model::{
    AddressRecord, Conflict, Observation, ObservationKind, Prefix, Settings, Sighting, Source,
    Status, VlanRecord,
};
use crate::net::{self, Net};
use crate::prefixes::{self, Entry};

/// How many rows a report carries before it says it is truncated.
const MAX_ROWS: usize = 500;

// ------------------------------------------------------------------- sync

#[derive(Debug, Serialize)]
pub struct SyncResult {
    pub at: i64,
    /// Prefixes created because an address was seen inside them and no
    /// prefix on record contained it. They carry `source: discovered`.
    pub prefixes_added: u32,
    /// Discovered prefixes whose evidence was refreshed.
    pub prefixes_refreshed: u32,
    /// Observations written (configured addresses and ARP sightings).
    pub addresses_updated: u32,
    /// ARP sightings forgotten because nothing refreshed them in time.
    pub addresses_aged_out: u32,
    /// Interface addresses that are no longer configured anywhere.
    pub addresses_withdrawn: u32,
    /// Distinct VLANs seen. A sync writes no VLAN records.
    pub vlans_updated: u32,
    /// Addresses left out on purpose: link-local, loopback, unspecified.
    pub skipped_addresses: u32,
    pub devices: Vec<DeviceCoverage>,
    pub conflicts: Vec<Conflict>,
}

/// Pull everything the devices said into observations.
pub fn sync(app: &App, settings: &Settings, device_ids: &[String]) -> AppResult<SyncResult> {
    let now = now_ms();
    let harvest = collected::harvest(app, device_ids)?;
    let mut result = SyncResult {
        at: now,
        prefixes_added: 0,
        prefixes_refreshed: 0,
        addresses_updated: 0,
        addresses_aged_out: 0,
        addresses_withdrawn: 0,
        vlans_updated: 0,
        skipped_addresses: harvest.skipped_addresses,
        devices: harvest.coverage.clone(),
        conflicts: Vec::new(),
    };

    // ---- configured addresses: the interface table is the whole truth
    let mut per_device: BTreeMap<String, Vec<Observation>> = BTreeMap::new();
    for observation in &harvest.interfaces {
        per_device
            .entry(observation.device_id.clone())
            .or_default()
            .push(observation.clone());
    }
    for coverage in &harvest.coverage {
        if coverage.interfaces_at.is_none() {
            // Never collected: what was stored before stays, because a
            // failed read is not an empty one.
            continue;
        }
        let known: BTreeMap<String, Observation> = db::observations_of_device(
            app.store(),
            &coverage.device_id,
            ObservationKind::Interface,
        )?
        .into_iter()
        .map(|observation| (db::observation_id(&observation), observation))
        .collect();
        let mut fresh = per_device.remove(&coverage.device_id).unwrap_or_default();
        for observation in &mut fresh {
            if let Some(earlier) = known.get(&db::observation_id(observation)) {
                observation.first_seen = earlier.first_seen.min(observation.first_seen);
            }
        }
        let withdrawn = known
            .keys()
            .filter(|id| {
                !fresh
                    .iter()
                    .any(|observation| &db::observation_id(observation) == *id)
            })
            .count();
        result.addresses_withdrawn += u32::try_from(withdrawn).unwrap_or(u32::MAX);
        result.addresses_updated += u32::try_from(fresh.len()).unwrap_or(u32::MAX);
        db::replace_device_observations(
            app.store(),
            &coverage.device_id,
            ObservationKind::Interface,
            &fresh,
        )?;
    }

    // ---- ARP: entries come and go, so they age rather than vanish
    let mut arp_per_device: BTreeMap<String, Vec<Observation>> = BTreeMap::new();
    for observation in &harvest.arp {
        arp_per_device
            .entry(observation.device_id.clone())
            .or_default()
            .push(observation.clone());
    }
    for coverage in &harvest.coverage {
        if coverage.arp_at.is_none() {
            continue;
        }
        let known =
            db::observations_of_device(app.store(), &coverage.device_id, ObservationKind::Arp)?;
        let mut fresh = arp_per_device
            .remove(&coverage.device_id)
            .unwrap_or_default();
        let ids: BTreeSet<String> = fresh.iter().map(db::observation_id).collect();
        for observation in &mut fresh {
            if let Some(earlier) = known
                .iter()
                .find(|earlier| db::observation_id(earlier) == db::observation_id(observation))
            {
                observation.first_seen = earlier.first_seen.min(observation.first_seen);
            }
        }
        result.addresses_updated += u32::try_from(fresh.len()).unwrap_or(u32::MAX);
        // What left the table keeps its last_seen and stops being current.
        for mut earlier in known {
            if !ids.contains(&db::observation_id(&earlier)) {
                earlier.current = false;
                fresh.push(earlier);
            }
        }
        db::replace_device_observations(
            app.store(),
            &coverage.device_id,
            ObservationKind::Arp,
            &fresh,
        )?;
    }
    let floor = prefixes::arp_floor(settings, now);
    result.addresses_aged_out =
        u32::try_from(db::prune_stale_arp(app.store(), floor)?).unwrap_or(u32::MAX);

    // ---- prefixes nothing on record covers
    let discovery = discover_prefixes(app, &harvest)?;
    for candidate in discovery {
        match candidate.existing {
            Some(id) => {
                let mut prefix: Prefix = match app.store().get(db::PREFIXES, &id)? {
                    Some(prefix) => prefix,
                    None => continue,
                };
                // A discovered prefix keeps its evidence up to date. A prefix
                // a person has touched is left exactly as they left it.
                if prefix.source == Source::Discovered {
                    prefix.discovered_from = candidate.from;
                    prefix.updated_at = now;
                    app.store().put(db::PREFIXES, &prefix.id, &prefix)?;
                    result.prefixes_refreshed += 1;
                }
            }
            None => {
                let prefix = Prefix {
                    id: db::new_id(),
                    cidr: candidate.cidr,
                    name: String::new(),
                    description: String::new(),
                    site: candidate.site,
                    vlan_id: None,
                    role: None,
                    gateway: None,
                    dhcp: None,
                    tags: Vec::new(),
                    source: Source::Discovered,
                    discovered_from: candidate.from,
                    created_at: now,
                    updated_at: now,
                };
                app.store().put(db::PREFIXES, &prefix.id, &prefix)?;
                result.prefixes_added += 1;
            }
        }
    }

    let vlans = collected::vlan_sightings(app)?;
    let distinct: BTreeSet<(String, u16)> = vlans
        .iter()
        .map(|sighting| (sighting.device.scope.clone(), sighting.vid))
        .collect();
    result.vlans_updated = u32::try_from(distinct.len()).unwrap_or(u32::MAX);

    let entries = prefixes::load(app)?;
    result.conflicts = conflicts::detect(app, settings, &entries)?.conflicts;
    Ok(result)
}

/// A network an interface is configured in that no prefix on record covers.
#[derive(Debug)]
struct Candidate {
    cidr: String,
    site: Option<String>,
    from: Vec<Sighting>,
    /// The id of the prefix with exactly this block, if there is one.
    existing: Option<String>,
}

fn discover_prefixes(app: &App, harvest: &Harvest) -> AppResult<Vec<Candidate>> {
    let entries = prefixes::load(app)?;
    let devices = collected::devices(app)?;
    let mut candidates: BTreeMap<(String, String), Candidate> = BTreeMap::new();
    for observation in &harvest.interfaces {
        // Without a mask there is no network to infer, and guessing one would
        // be exactly the kind of invention this project refuses.
        let Some(len) = observation.prefix_len else {
            continue;
        };
        let Ok(address) = net::addr(&observation.ip) else {
            continue;
        };
        let block = Net {
            family: address.family,
            base: address.value & !net::low_mask(if address.family == 4 { 32 } else { 128 } - len),
            len,
        };
        let scope = observation.scope.clone();
        if prefixes::containing(&entries, &scope, &address).is_some() {
            continue;
        }
        let site = if scope.is_empty() {
            devices
                .iter()
                .find(|device| device.id == observation.device_id)
                .map(|device| device.site.clone())
                .filter(|site| !site.is_empty())
        } else {
            Some(scope.clone())
        };
        let sighting = Sighting {
            device_id: observation.device_id.clone(),
            device: observation.device.clone(),
            interface: observation.interface.clone(),
            cidr: observation
                .cidr
                .clone()
                .unwrap_or_else(|| observation.ip.clone()),
            collected_at: observation.collected_at,
        };
        let key = (scope.clone(), block.cidr());
        match candidates.get_mut(&key) {
            Some(candidate) => candidate.from.push(sighting),
            None => {
                let existing = entries
                    .iter()
                    .find(|entry| entry.scope == scope && entry.net == block)
                    .map(|entry| entry.prefix.id.clone());
                candidates.insert(
                    key,
                    Candidate {
                        cidr: block.cidr(),
                        site,
                        from: vec![sighting],
                        existing,
                    },
                );
            }
        }
    }
    Ok(candidates.into_values().collect())
}

// -------------------------------------------------------------- reconcile

/// What was looked at. Without this, "nothing has answered on that address"
/// is an opinion.
#[derive(Debug, Serialize)]
pub struct Coverage {
    pub devices_total: u32,
    pub devices_with_addresses: u32,
    pub devices_never_collected: u32,
    pub oldest_collection: Option<i64>,
    pub newest_collection: Option<i64>,
    pub devices: Vec<DeviceCoverage>,
}

/// An address the network shows and nobody wrote down.
#[derive(Debug, Serialize)]
pub struct Undocumented {
    pub ip: String,
    pub scope: String,
    pub prefix_id: Option<String>,
    pub prefix: Option<String>,
    /// True when even the prefix it falls in is only `discovered`.
    pub prefix_documented: bool,
    pub observations: Vec<Observation>,
    pub summary: String,
}

/// An address somebody wrote down that the network has never shown.
#[derive(Debug, Serialize)]
pub struct Unobserved {
    pub ip: String,
    pub scope: String,
    pub prefix_id: Option<String>,
    pub prefix: Option<String>,
    pub hostname: String,
    pub status: Status,
    pub documented_at: i64,
    /// `None` means it has never been seen at all.
    pub last_seen: Option<i64>,
    /// Whether anything that could have seen it was ever collected — the
    /// difference between "it is not there" and "we never looked".
    pub had_coverage: bool,
    pub summary: String,
}

/// A record and the network disagreeing about one address.
#[derive(Debug, Serialize)]
pub struct Mismatch {
    pub ip: String,
    pub scope: String,
    pub field: String,
    pub documented: String,
    pub observed: String,
    pub device_id: String,
    pub device: String,
    pub interface: String,
    pub collected_at: i64,
}

/// An interface whose mask is not the length of the prefix it sits in.
#[derive(Debug, Serialize)]
pub struct MaskMismatch {
    pub ip: String,
    pub prefix_id: String,
    pub prefix: String,
    pub prefix_len: u8,
    pub configured_len: u8,
    pub device_id: String,
    pub device: String,
    pub interface: String,
    pub collected_at: i64,
}

#[derive(Debug, Serialize)]
pub struct PrefixNote {
    pub id: String,
    pub cidr: String,
    pub site: Option<String>,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct Reconciliation {
    pub at: i64,
    pub coverage: Coverage,
    pub undocumented: Vec<Undocumented>,
    pub unobserved: Vec<Unobserved>,
    pub mismatches: Vec<Mismatch>,
    pub mask_mismatches: Vec<MaskMismatch>,
    /// Prefixes that exist only because something was seen inside them.
    pub prefixes_not_documented: Vec<PrefixNote>,
    /// Documented prefixes nothing has ever been seen in.
    pub prefixes_without_evidence: Vec<PrefixNote>,
    pub conflicts: Vec<Conflict>,
    /// Any list that hit its cap.
    pub truncated: Vec<String>,
    /// What this report will not decide, and why.
    pub refused: Vec<String>,
}

/// Compare the records with the evidence. Writes nothing.
pub fn reconcile(app: &App, settings: &Settings, site: Option<&str>) -> AppResult<Reconciliation> {
    let now = now_ms();
    let floor = prefixes::arp_floor(settings, now);
    let entries = prefixes::load(app)?;
    let conflicts = conflicts::detect(app, settings, &entries)?;
    let coverage_rows = collected::harvest(app, &[])?.coverage;

    let in_site = |entry: &Entry| match site {
        Some(site) => entry.prefix.site.as_deref() == Some(site),
        None => true,
    };

    let mut report = Reconciliation {
        at: now,
        coverage: Coverage {
            devices_total: u32::try_from(coverage_rows.len()).unwrap_or(u32::MAX),
            devices_with_addresses: u32::try_from(
                coverage_rows
                    .iter()
                    .filter(|row| row.addresses_configured > 0)
                    .count(),
            )
            .unwrap_or(u32::MAX),
            devices_never_collected: u32::try_from(
                coverage_rows
                    .iter()
                    .filter(|row| row.interfaces_at.is_none())
                    .count(),
            )
            .unwrap_or(u32::MAX),
            oldest_collection: coverage_rows.iter().filter_map(|row| row.interfaces_at).min(),
            newest_collection: coverage_rows.iter().filter_map(|row| row.interfaces_at).max(),
            devices: coverage_rows,
        },
        undocumented: Vec::new(),
        unobserved: Vec::new(),
        mismatches: Vec::new(),
        mask_mismatches: Vec::new(),
        prefixes_not_documented: Vec::new(),
        prefixes_without_evidence: Vec::new(),
        conflicts: conflicts.conflicts.clone(),
        truncated: Vec::new(),
        refused: vec![
            "An address configured on a device but absent from the plan may be a device to reconfigure or a plan to update: which one is a decision, so it is only reported.".to_string(),
            "A record the network has never shown is not deleted; it may be a host that is switched off, or a plan for next week.".to_string(),
            "Where a record and a device disagree, the record is left exactly as it was written.".to_string(),
            "A `discovered` prefix is never promoted to a documented one by itself.".to_string(),
        ],
    };

    // ---- observations with no record
    let mut by_address: BTreeMap<(String, String), Vec<Observation>> = BTreeMap::new();
    for observation in db::all_observations(app.store())? {
        if observation.kind == ObservationKind::Arp && observation.last_seen < floor {
            continue;
        }
        by_address
            .entry((observation.scope.clone(), observation.ip.clone()))
            .or_default()
            .push(observation);
    }
    for ((scope, ip), observations) in &by_address {
        let Ok(address) = net::addr(ip) else { continue };
        let entry = prefixes::containing(&entries, scope, &address);
        if let Some(entry) = entry
            && !in_site(entry)
        {
            continue;
        }
        if entry.is_none() && site.is_some() {
            continue;
        }
        // The mask the interface carries against the prefix it falls in.
        if let Some(entry) = entry {
            for observation in observations {
                if let Some(len) = observation.prefix_len
                    && len != entry.net.len
                    && report.mask_mismatches.len() < MAX_ROWS
                {
                    report.mask_mismatches.push(MaskMismatch {
                        ip: ip.clone(),
                        prefix_id: entry.prefix.id.clone(),
                        prefix: entry.prefix.cidr.clone(),
                        prefix_len: entry.net.len,
                        configured_len: len,
                        device_id: observation.device_id.clone(),
                        device: observation.device.clone(),
                        interface: observation.interface.clone(),
                        collected_at: observation.collected_at,
                    });
                }
            }
        }
        let record = db::record(app.store(), scope, ip)?;
        match record {
            None => {
                if report.undocumented.len() >= MAX_ROWS {
                    continue;
                }
                let configured: Vec<&Observation> = observations
                    .iter()
                    .filter(|observation| observation.kind == ObservationKind::Interface)
                    .collect();
                let summary = match configured.first() {
                    Some(observation) => format!(
                        "{} is configured on {} ({}) and is not in the plan",
                        ip, observation.device, observation.interface
                    ),
                    None => format!(
                        "{} answered on {} and is not in the plan",
                        ip,
                        observations
                            .iter()
                            .map(|observation| observation.device.as_str())
                            .collect::<BTreeSet<_>>()
                            .into_iter()
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                };
                report.undocumented.push(Undocumented {
                    ip: ip.clone(),
                    scope: scope.clone(),
                    prefix_id: entry.map(|entry| entry.prefix.id.clone()),
                    prefix: entry.map(|entry| entry.prefix.cidr.clone()),
                    prefix_documented: entry
                        .map(|entry| entry.prefix.source == Source::Manual)
                        .unwrap_or(false),
                    observations: observations.clone(),
                    summary,
                });
            }
            Some(record) => {
                let composed = crate::addresses::compose(
                    &address,
                    Some(&record),
                    observations,
                    entry,
                    conflicts.has(scope, ip),
                );
                for disagreement in composed.disagreements {
                    if report.mismatches.len() >= MAX_ROWS {
                        break;
                    }
                    report.mismatches.push(Mismatch {
                        ip: ip.clone(),
                        scope: scope.clone(),
                        field: disagreement.field,
                        documented: disagreement.documented,
                        observed: disagreement.observed,
                        device_id: disagreement.device_id,
                        device: disagreement.device,
                        interface: disagreement.interface,
                        collected_at: disagreement.collected_at,
                    });
                }
            }
        }
    }

    // ---- records nothing has shown
    for record in db::all_records(app.store())? {
        let Ok(address) = net::addr(&record.ip) else {
            continue;
        };
        let entry = prefixes::containing(&entries, &record.scope, &address);
        if let Some(entry) = entry
            && !in_site(entry)
        {
            continue;
        }
        if entry.is_none() && site.is_some() {
            continue;
        }
        let observations = db::observations_of_ip(app.store(), &record.scope, &record.ip)?;
        let last_seen = observations
            .iter()
            .filter(|observation| {
                observation.kind == ObservationKind::Interface || observation.last_seen >= floor
            })
            .map(|observation| observation.last_seen)
            .max();
        if last_seen.is_some() {
            continue;
        }
        // Could anything have seen it? Only a device configured in the same
        // prefix is in a position to.
        let had_coverage = match entry {
            Some(entry) => {
                !db::interfaces_in(app.store(), &record.scope, &entry.net, 1)?.is_empty()
            }
            None => false,
        };
        let ever = observations.iter().map(|o| o.last_seen).max();
        let summary = match (had_coverage, ever) {
            (false, _) => format!(
                "{} is in the plan; nothing that could see it has been collected, so there is no evidence either way",
                record.ip
            ),
            (true, None) => format!(
                "{} is in the plan and nothing has ever answered on it",
                record.ip
            ),
            (true, Some(at)) => format!(
                "{} is in the plan; the last sighting was at {at}, which is older than the {}-day window",
                record.ip, settings.arp_stale_days
            ),
        };
        if report.unobserved.len() >= MAX_ROWS {
            continue;
        }
        report.unobserved.push(Unobserved {
            ip: record.ip.clone(),
            scope: record.scope.clone(),
            prefix_id: record.prefix_id.clone(),
            prefix: entry.map(|entry| entry.prefix.cidr.clone()),
            hostname: record.hostname.clone(),
            status: record.status,
            documented_at: record.updated_at,
            last_seen: ever,
            had_coverage,
            summary,
        });
    }

    // ---- prefixes
    for entry in entries.iter().filter(|entry| in_site(entry)) {
        if entry.prefix.source == Source::Discovered {
            let from: Vec<String> = entry
                .prefix
                .discovered_from
                .iter()
                .map(|sighting| format!("{} ({})", sighting.device, sighting.interface))
                .collect();
            report.prefixes_not_documented.push(PrefixNote {
                id: entry.prefix.id.clone(),
                cidr: entry.prefix.cidr.clone(),
                site: entry.prefix.site.clone(),
                detail: if from.is_empty() {
                    format!(
                        "{} was discovered; nobody has documented it",
                        entry.prefix.cidr
                    )
                } else {
                    format!(
                        "{} exists because an address was seen on {}; nobody has documented it",
                        entry.prefix.cidr,
                        from.join(", ")
                    )
                },
            });
            continue;
        }
        let counts = db::counts_in(app.store(), &entry.scope, &entry.net, floor)?;
        if counts.observed == 0 {
            report.prefixes_without_evidence.push(PrefixNote {
                id: entry.prefix.id.clone(),
                cidr: entry.prefix.cidr.clone(),
                site: entry.prefix.site.clone(),
                detail: format!(
                    "{} is documented and nothing has ever been seen in it",
                    entry.prefix.cidr
                ),
            });
        }
    }

    for (name, length) in [
        ("undocumented", report.undocumented.len()),
        ("unobserved", report.unobserved.len()),
        ("mismatches", report.mismatches.len()),
        ("mask_mismatches", report.mask_mismatches.len()),
    ] {
        if length >= MAX_ROWS {
            report.truncated.push(name.to_string());
        }
    }
    Ok(report)
}

// ------------------------------------------------------------------ learn

#[derive(Clone, Debug, Serialize)]
pub struct LearnPrefix {
    pub cidr: String,
    pub site: Option<String>,
    pub from: Vec<Sighting>,
    /// `create` or `exists`.
    pub action: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LearnAddress {
    pub ip: String,
    pub scope: String,
    pub prefix: Option<String>,
    pub device_id: String,
    pub device: String,
    pub interface: String,
    pub kind: ObservationKind,
    pub mac: Option<String>,
    pub collected_at: i64,
    /// `create`, `exists` (a record is already there) or `skip`.
    pub action: String,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LearnVlan {
    pub vid: u16,
    pub name: String,
    pub site: Option<String>,
    pub devices: Vec<String>,
    pub action: String,
}

#[derive(Debug, Serialize)]
pub struct LearnPlan {
    /// True when nothing was written, which is the default.
    pub dry_run: bool,
    pub prefixes: Vec<LearnPrefix>,
    pub addresses: Vec<LearnAddress>,
    pub vlans: Vec<LearnVlan>,
    pub created_prefixes: u32,
    pub created_addresses: u32,
    pub created_vlans: u32,
    /// What the plan was built from.
    pub coverage: Coverage,
    pub notes: Vec<String>,
}

/// Import what is already on the network as records: one call that shows what
/// it would create, and only writes when asked twice.
pub fn learn(
    app: &App,
    settings: &Settings,
    device_ids: &[String],
    dry_run: bool,
    include_arp: bool,
) -> AppResult<LearnPlan> {
    let now = now_ms();
    let floor = prefixes::arp_floor(settings, now);
    let harvest = collected::harvest(app, device_ids)?;
    let coverage_rows = harvest.coverage.clone();
    let mut plan = LearnPlan {
        dry_run,
        prefixes: Vec::new(),
        addresses: Vec::new(),
        vlans: Vec::new(),
        created_prefixes: 0,
        created_addresses: 0,
        created_vlans: 0,
        coverage: Coverage {
            devices_total: u32::try_from(coverage_rows.len()).unwrap_or(u32::MAX),
            devices_with_addresses: u32::try_from(
                coverage_rows
                    .iter()
                    .filter(|row| row.addresses_configured > 0)
                    .count(),
            )
            .unwrap_or(u32::MAX),
            devices_never_collected: u32::try_from(
                coverage_rows
                    .iter()
                    .filter(|row| row.interfaces_at.is_none())
                    .count(),
            )
            .unwrap_or(u32::MAX),
            oldest_collection: coverage_rows.iter().filter_map(|row| row.interfaces_at).min(),
            newest_collection: coverage_rows.iter().filter_map(|row| row.interfaces_at).max(),
            devices: coverage_rows,
        },
        notes: vec![
            "A record created here says where it came from; it does not become the truth about the device.".to_string(),
            "Existing records are never overwritten.".to_string(),
        ],
    };
    if !include_arp {
        plan.notes.push(
            "ARP sightings were left out: they are hosts that answered once, not addresses anybody has assigned. Ask for them with `include_arp`.".to_string(),
        );
    }

    // ---- prefixes
    for candidate in discover_prefixes(app, &harvest)? {
        let existing = candidate.existing.clone();
        let mut row = LearnPrefix {
            cidr: candidate.cidr.clone(),
            site: candidate.site.clone(),
            from: candidate.from.clone(),
            action: if existing.is_some() {
                "exists"
            } else {
                "create"
            }
            .to_string(),
            id: existing.clone(),
        };
        if existing.is_none() && !dry_run {
            let prefix = Prefix {
                id: db::new_id(),
                cidr: candidate.cidr.clone(),
                site: candidate.site.clone(),
                source: Source::Discovered,
                discovered_from: candidate.from.clone(),
                created_at: now,
                updated_at: now,
                ..Prefix::default()
            };
            app.store().put(db::PREFIXES, &prefix.id, &prefix)?;
            row.id = Some(prefix.id);
            plan.created_prefixes += 1;
        }
        plan.prefixes.push(row);
    }

    let entries = prefixes::load(app)?;

    // ---- addresses
    let mut sightings: Vec<&Observation> = harvest.interfaces.iter().collect();
    if include_arp {
        sightings.extend(
            harvest
                .arp
                .iter()
                .filter(|observation| observation.last_seen >= floor),
        );
    }
    let mut records: Vec<AddressRecord> = Vec::new();
    for observation in sightings {
        let Ok(address) = net::addr(&observation.ip) else {
            continue;
        };
        let entry = prefixes::containing(&entries, &observation.scope, &address);
        let existing = db::record(app.store(), &observation.scope, &address.text)?;
        let mut row = LearnAddress {
            ip: address.text.clone(),
            scope: observation.scope.clone(),
            prefix: entry.map(|entry| entry.prefix.cidr.clone()),
            device_id: observation.device_id.clone(),
            device: observation.device.clone(),
            interface: observation.interface.clone(),
            kind: observation.kind,
            mac: observation.mac.clone(),
            collected_at: observation.collected_at,
            action: "create".to_string(),
            reason: None,
        };
        if existing.is_some() {
            row.action = "exists".to_string();
            row.reason = Some("a record for this address is already there".to_string());
        } else if records
            .iter()
            .any(|record| record.ip == address.text && record.scope == observation.scope)
        {
            row.action = "skip".to_string();
            row.reason = Some("another device in this run reported the same address".to_string());
        } else {
            records.push(AddressRecord {
                ip: address.text.clone(),
                scope: observation.scope.clone(),
                prefix_id: entry.map(|entry| entry.prefix.id.clone()),
                status: Status::Active,
                hostname: String::new(),
                description: format!(
                    "learned from {} ({}), collected at {}",
                    observation.device, observation.interface, observation.collected_at
                ),
                device_id: Some(observation.device_id.clone()),
                interface: Some(observation.interface.clone()),
                mac: observation.mac.clone(),
                tags: vec!["learned".to_string()],
                created_at: now,
                updated_at: now,
            });
        }
        plan.addresses.push(row);
    }
    if !dry_run && !records.is_empty() {
        db::put_records(app.store(), &records)?;
        plan.created_addresses = u32::try_from(records.len()).unwrap_or(u32::MAX);
    }

    // ---- VLANs
    let documented: Vec<VlanRecord> = app.store().list(db::VLANS)?;
    let sightings = collected::vlan_sightings(app)?;
    let mut by_vlan: BTreeMap<(String, u16), Vec<&collected::VlanSighting>> = BTreeMap::new();
    for sighting in &sightings {
        if !device_ids.is_empty() && !device_ids.contains(&sighting.device.id) {
            continue;
        }
        by_vlan
            .entry((sighting.device.scope.clone(), sighting.vid))
            .or_default()
            .push(sighting);
    }
    for ((scope, vid), group) in by_vlan {
        let site = group
            .first()
            .map(|sighting| {
                if scope.is_empty() {
                    sighting.device.site.clone()
                } else {
                    scope.clone()
                }
            })
            .filter(|site| !site.is_empty());
        let exists = documented
            .iter()
            .any(|record| record.vid == vid && record.site == site);
        let name = group
            .iter()
            .map(|sighting| sighting.name.clone())
            .find(|name| !name.is_empty())
            .unwrap_or_default();
        let mut row = LearnVlan {
            vid,
            name: name.clone(),
            site: site.clone(),
            devices: group
                .iter()
                .map(|sighting| sighting.device.name.clone())
                .collect(),
            action: if exists { "exists" } else { "create" }.to_string(),
        };
        if !exists && !dry_run {
            let record = VlanRecord {
                id: db::new_id(),
                vid,
                name,
                site: site.clone(),
                description: String::new(),
                prefix_ids: Vec::new(),
                source: Source::Discovered,
                created_at: now,
                updated_at: now,
            };
            app.store().put(db::VLANS, &record.id, &record)?;
            plan.created_vlans += 1;
            row.action = "create".to_string();
        }
        plan.vlans.push(row);
    }
    Ok(plan)
}
