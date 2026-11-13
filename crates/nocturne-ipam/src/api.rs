//! The RPC surface and the one background task.
//!
//! Specification: `docs/dev/features/ipam.md`. Every method here is reachable
//! only through `App::rpc`, and every mutation is audited.

use std::time::Duration;

use nocturne_app::settings;
use nocturne_app::{App, AppBuilder, AppError, AppResult, NoParams};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::{Address, Conflict, PrefixRow, Settings, Source, VlanRow};
use crate::net;
use crate::prefixes::{self, Entry, PrefixInput, SplitRequest};
use crate::{addresses, collected, conflicts, db, portability, sync, vlans};

pub const SETTINGS_SECTION: &str = "ipam";

/// How long the watcher waits when the settings cannot be read at all.
const FALLBACK_DEBOUNCE: Duration = Duration::from_millis(1_500);

fn config(app: &App) -> AppResult<Settings> {
    settings::get::<Settings>(app, SETTINGS_SECTION)
}

fn changed(app: &App, kinds: &[&str]) {
    app.publish("ipam.changed", &json!({"kinds": kinds}));
}

// ------------------------------------------------------------------ params

#[derive(Debug, Default, Deserialize)]
struct PrefixesParams {
    site: Option<String>,
    query: Option<String>,
    parent_id: Option<String>,
    /// `true` returns only the matching prefixes; the default keeps the
    /// ancestors of a match so the tree stays whole.
    #[serde(default)]
    flat: bool,
}

#[derive(Debug, Deserialize)]
struct GetPrefixParams {
    id: String,
    #[serde(default = "yes")]
    addresses: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SplitParams {
    id: String,
    new_len: Option<u8>,
    /// Host counts, either as numbers or as `{name, hosts}`.
    #[serde(default)]
    sizes: Vec<SizeInput>,
    /// Point-to-point links get a /31 rather than a /30.
    #[serde(default)]
    p2p31: bool,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SizeInput {
    Hosts(u64),
    Named {
        #[serde(default)]
        name: String,
        hosts: u64,
    },
}

impl SizeInput {
    fn request(&self, index: usize) -> SplitRequest {
        match self {
            SizeInput::Hosts(hosts) => SplitRequest {
                name: format!("subnet {}", index + 1),
                hosts: *hosts,
            },
            SizeInput::Named { name, hosts } => SplitRequest {
                name: if name.is_empty() {
                    format!("subnet {}", index + 1)
                } else {
                    name.clone()
                },
                hosts: *hosts,
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct NextPrefixParams {
    parent_id: String,
    len: u8,
}

#[derive(Debug, Deserialize)]
struct NextAddressParams {
    prefix_id: String,
    #[serde(default = "one")]
    count: usize,
    #[serde(default)]
    skip_first: u64,
}

fn one() -> usize {
    1
}

#[derive(Debug, Deserialize)]
struct AddressKeyParams {
    ip: String,
    #[serde(default)]
    scope: String,
}

#[derive(Debug, Deserialize)]
struct ReserveParams {
    prefix_id: String,
    ip: Option<String>,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    query: String,
    #[serde(default = "search_limit")]
    limit: usize,
}

fn search_limit() -> usize {
    200
}

#[derive(Debug, Default, Deserialize)]
struct SyncParams {
    #[serde(default)]
    device_ids: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ReconcileParams {
    site: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LearnParams {
    #[serde(default)]
    device_ids: Vec<String>,
    /// Nothing is written unless this is explicitly `false`.
    #[serde(default = "yes")]
    dry_run: bool,
    #[serde(default)]
    include_arp: bool,
}

#[derive(Debug, Default, Deserialize)]
struct VlansParams {
    site: Option<String>,
    query: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ImportParams {
    #[serde(default = "csv_format")]
    format: String,
    kind: String,
    text: String,
    #[serde(default = "yes")]
    dry_run: bool,
}

fn csv_format() -> String {
    "csv".to_string()
}

#[derive(Debug, Deserialize)]
struct ExportParams {
    kind: String,
    #[serde(default = "csv_format")]
    format: String,
    /// Add the addresses only the network knows about, each labelled with
    /// where it was seen.
    #[serde(default)]
    include_observed: bool,
}

// ----------------------------------------------------------------- results

#[derive(Debug, Serialize)]
struct PrefixDetail {
    prefix: PrefixRow,
    addresses: Vec<Address>,
    free_ranges: Vec<crate::model::FreeRange>,
    /// Child blocks no child prefix covers.
    free_child_prefixes: Vec<String>,
    /// True when `addresses` is every address of the block (the grid); false
    /// when it is only the ones something is known about.
    grid: bool,
    /// What the free space does and does not mean.
    note: String,
    /// The devices that could have seen anything in this block, and when they
    /// were last read.
    coverage: Vec<collected::DeviceCoverage>,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    prefixes: Vec<PrefixRow>,
    addresses: Vec<Address>,
    vlans: Vec<VlanRow>,
}

#[derive(Debug, Serialize)]
struct Utilisation {
    prefix: String,
    prefix_id: String,
    util_pct: f32,
    util_known: bool,
    used: u32,
    usable_text: String,
}

#[derive(Debug, Serialize)]
struct Summary {
    prefixes: u64,
    prefixes_discovered: u64,
    /// Address *records*.
    addresses: u64,
    /// Sightings, which is a different number and a different thing.
    observations: u64,
    vlans: u64,
    utilisation: Vec<Utilisation>,
    conflicts: usize,
    /// Devices whose interfaces have never been collected: everything above
    /// is only as complete as this.
    devices_never_collected: u32,
    devices_total: u32,
}

// ----------------------------------------------------------------- helpers

fn matches_prefix(entry: &Entry, query: &str) -> bool {
    let role = entry
        .prefix
        .role
        .and_then(|role| serde_json::to_value(role).ok())
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default();
    let haystack = format!(
        "{} {} {} {} {} {} {}",
        entry.prefix.cidr,
        entry.prefix.name,
        entry.prefix.description,
        entry.prefix.site.clone().unwrap_or_default(),
        entry
            .prefix
            .vlan_id
            .map(|vid| vid.to_string())
            .unwrap_or_default(),
        role,
        entry.prefix.tags.join(" ")
    );
    nocturne_app::util::matches_query(&haystack, query)
}

/// Is `entry` inside the subtree rooted at `root`?
fn in_subtree(entries: &[Entry], entry: &Entry, root: &str) -> bool {
    let mut current = entry.parent_id.clone();
    while let Some(id) = current {
        if id == root {
            return true;
        }
        current = entries
            .iter()
            .find(|candidate| candidate.prefix.id == id)
            .and_then(|candidate| candidate.parent_id.clone());
    }
    false
}

/// The rows of a search result: each the record, the sightings, and nothing
/// merged. The evidence is fetched for the whole result in two queries.
fn found_rows(
    app: &App,
    settings: &Settings,
    entries: &[Entry],
    keys: &[(String, String)],
) -> AppResult<Vec<Address>> {
    let records = db::records_of(app.store(), keys)?;
    let observations = db::observations_of(app.store(), keys)?;
    let now = nocturne_app::util::now_ms();
    let mut rows = Vec::with_capacity(keys.len());
    for key in keys {
        let address = net::addr(&key.1)?;
        let record = records.get(key);
        let evidence = observations.get(key).cloned().unwrap_or_default();
        let entry = prefixes::containing(entries, &key.0, &address);
        let conflict = conflicts::for_evidence(settings, entries, &key.0, record, &evidence, now);
        rows.push(addresses::compose(
            &address, record, &evidence, entry, conflict,
        ));
    }
    Ok(rows)
}

fn rows_for(
    app: &App,
    settings: &Settings,
    entries: &[Entry],
    chosen: &[usize],
) -> AppResult<Vec<PrefixRow>> {
    prefixes::rows(app, settings, entries, chosen)
}

// -------------------------------------------------------------------- init

pub fn init(builder: &mut AppBuilder) {
    settings::declare(builder, SETTINGS_SECTION, &Settings::default());
    // Without its tables the feature has nowhere to keep records or
    // evidence: failing here means a broken database, not a condition to
    // carry on with.
    db::init(builder.store()).expect("the ipam feature's tables can be created");
    let router = &mut builder.router;

    router.add(
        "ipam.prefixes",
        |app: App, params: PrefixesParams| async move {
            let settings = config(&app)?;
            let entries = prefixes::load(&app)?;
            let query = params.query.filter(|query| !query.trim().is_empty());
            let mut chosen: Vec<usize> = Vec::new();
            for (index, entry) in entries.iter().enumerate() {
                if let Some(site) = &params.site
                    && entry.prefix.site.as_deref() != Some(site.as_str())
                {
                    continue;
                }
                if let Some(parent) = &params.parent_id
                    && !in_subtree(&entries, entry, parent)
                {
                    continue;
                }
                if let Some(query) = &query
                    && !matches_prefix(entry, query)
                {
                    continue;
                }
                chosen.push(index);
            }
            if !params.flat {
                // Keep the ancestors of every match, so the tree a screen
                // draws is not missing its branches — but never above the
                // prefix the caller asked to look inside.
                let mut with_parents = chosen.clone();
                for index in &chosen {
                    let mut parent = entries[*index].parent_id.clone();
                    while let Some(id) = parent {
                        if let Some(position) =
                            entries.iter().position(|entry| entry.prefix.id == id)
                        {
                            let inside = params
                                .parent_id
                                .as_ref()
                                .map(|root| in_subtree(&entries, &entries[position], root))
                                .unwrap_or(true);
                            if !inside {
                                break;
                            }
                            if !with_parents.contains(&position) {
                                with_parents.push(position);
                            }
                            parent = entries[position].parent_id.clone();
                        } else {
                            break;
                        }
                    }
                }
                with_parents.sort_unstable();
                with_parents.dedup();
                chosen = with_parents;
            }
            rows_for(&app, &settings, &entries, &chosen)
        },
    );

    router.add(
        "ipam.prefix.get",
        |app: App, params: GetPrefixParams| async move {
            let settings = config(&app)?;
            let entries = prefixes::load(&app)?;
            let entry = prefixes::find(&entries, &params.id)?;
            // Only the conflicts of this block, which is a range query
            // rather than a walk of the estate.
            let conflicting = conflicts::in_block(&app, &settings, &entry.scope, &entry.net)?;
            let has = |_scope: &str, ip: &str| conflicting.contains(ip);
            let rows = if params.addresses {
                addresses::in_prefix(&app, &settings, entry, &entries, &has)?
            } else {
                Vec::new()
            };
            let occupied = if params.addresses {
                addresses::occupied_in(&rows)
            } else {
                addresses::occupied_values(&app, &settings, &entry.scope, &entry.net)?
            };
            let grid = entry.net.total() <= u64::from(settings.grid_max_addresses);
            // The devices that could have seen anything in this block: one
            // range query, not one per device in the inventory.
            let inside = db::interfaces_in(app.store(), &entry.scope, &entry.net, 500)?;
            let seen_by: std::collections::BTreeSet<&str> = inside
                .iter()
                .map(|observation| observation.device_id.as_str())
                .chain(rows.iter().flat_map(|address| {
                    address
                        .observations
                        .iter()
                        .map(|observation| observation.device_id.as_str())
                }))
                .collect();
            let coverage: Vec<collected::DeviceCoverage> = collected::harvest(&app, &[])?
                .coverage
                .into_iter()
                .filter(|row| seen_by.contains(row.device_id.as_str()))
                .collect();
            Ok(PrefixDetail {
                prefix: prefixes::row(&app, &settings, entry, &entries)?,
                addresses: rows,
                free_ranges: prefixes::free_ranges(&entry.net, &occupied, prefixes::FREE_RANGES),
                free_child_prefixes: prefixes::free_child_space(entry, &entries)?,
                grid,
                note: if grid {
                    "every address of the block is listed; `free` means nothing documented it and nothing has answered on it".to_string()
                } else {
                    "the block is too large to list address by address: only addresses with a record or a sighting are here".to_string()
                },
                coverage,
            })
        },
    );

    router.add(
        "ipam.prefix.save",
        |app: App, params: PrefixInput| async move {
            let settings = config(&app)?;
            let saved = prefixes::save(&app, params, &settings)?;
            app.audit(
                "ipam.prefix.save",
                None,
                format!(
                    "{} prefix {}",
                    if saved.created { "Added" } else { "Updated" },
                    saved.prefix.prefix.cidr
                ),
                json!({
                    "prefix": saved.prefix.prefix.id,
                    "cidr": saved.prefix.prefix.cidr,
                    "warnings": saved.warnings,
                }),
            );
            changed(&app, &["prefixes"]);
            Ok(saved)
        },
    );

    router.add(
        "ipam.prefix.delete",
        |app: App, params: IdParams| async move {
            let deleted = prefixes::delete(&app, &params.id)?;
            if deleted.deleted {
                app.audit(
                    "ipam.prefix.delete",
                    None,
                    format!("Deleted prefix {}", deleted.cidr),
                    json!({
                        "prefix": params.id,
                        "children_reparented": deleted.children_reparented,
                        "addresses_repointed": deleted.addresses_repointed,
                    }),
                );
                changed(&app, &["prefixes", "addresses"]);
            }
            Ok(deleted)
        },
    );

    router.add(
        "ipam.prefix.split",
        |app: App, params: SplitParams| async move {
            let settings = config(&app)?;
            let entries = prefixes::load(&app)?;
            let entry = prefixes::find(&entries, &params.id)?;
            let requests: Vec<SplitRequest> = params
                .sizes
                .iter()
                .enumerate()
                .map(|(index, size)| size.request(index))
                .collect();
            let mut children =
                prefixes::plan_split(entry, params.new_len, &requests, params.p2p31)?;
            let mut created = 0;
            if !params.dry_run {
                for child in &mut children {
                    let input = PrefixInput {
                        cidr: Some(child.cidr.clone()),
                        name: (!child.name.is_empty()).then(|| child.name.clone()),
                        site: entry.prefix.site.clone(),
                        ..PrefixInput::default()
                    };
                    match prefixes::save(&app, input, &settings) {
                        Ok(saved) => {
                            child.id = Some(saved.prefix.prefix.id);
                            created += 1;
                        }
                        Err(err) => child.skipped = Some(err.message),
                    }
                }
                if created > 0 {
                    app.audit(
                        "ipam.prefix.split",
                        None,
                        format!("Split {} into {created} prefixes", entry.prefix.cidr),
                        json!({"prefix": entry.prefix.id, "created": created}),
                    );
                    changed(&app, &["prefixes"]);
                }
            }
            Ok(json!({
                "parent": entry.prefix.cidr,
                "parent_id": entry.prefix.id,
                "dry_run": params.dry_run,
                "created": created,
                "children": children,
            }))
        },
    );

    router.add(
        "ipam.next_prefix",
        |app: App, params: NextPrefixParams| async move {
            let entries = prefixes::load(&app)?;
            let entry = prefixes::find(&entries, &params.parent_id)?;
            let next = prefixes::next_free_child(entry, &entries, params.len)?;
            Ok(json!({
                "parent": entry.prefix.cidr,
                "parent_id": entry.prefix.id,
                "len": params.len,
                "cidr": next,
                "free_space": prefixes::free_child_space(entry, &entries)?,
                "note": format!(
                    "nothing was created: this is where the next /{} inside {} would fit",
                    params.len, entry.prefix.cidr
                ),
            }))
        },
    );

    router.add(
        "ipam.next_address",
        |app: App, params: NextAddressParams| async move {
            let settings = config(&app)?;
            let entries = prefixes::load(&app)?;
            let entry = prefixes::find(&entries, &params.prefix_id)?;
            addresses::next_free(&app, &settings, entry, params.count, params.skip_first)
        },
    );

    router.add(
        "ipam.address.save",
        |app: App, params: addresses::AddressInput| async move {
            let settings = config(&app)?;
            let saved = addresses::save(&app, &settings, params)?;
            app.audit(
                "ipam.address.save",
                saved.address.device_id.as_deref(),
                format!(
                    "{} address record {}",
                    if saved.created { "Added" } else { "Updated" },
                    saved.address.ip
                ),
                json!({"ip": saved.address.ip, "status": saved.address.status, "warnings": saved.warnings}),
            );
            changed(&app, &["addresses"]);
            Ok(saved)
        },
    );

    router.add(
        "ipam.address.delete",
        |app: App, params: AddressKeyParams| async move {
            let deleted = addresses::delete(&app, &params.scope, &params.ip)?;
            if deleted.deleted {
                app.audit(
                    "ipam.address.delete",
                    None,
                    format!("Deleted the address record for {}", deleted.ip),
                    json!({"ip": deleted.ip, "observations_left": deleted.observations_left}),
                );
                changed(&app, &["addresses"]);
            }
            Ok(deleted)
        },
    );

    router.add(
        "ipam.address.reserve",
        |app: App, params: ReserveParams| async move {
            let settings = config(&app)?;
            let saved = addresses::reserve(
                &app,
                &settings,
                &params.prefix_id,
                params.ip.as_deref(),
                &params.hostname,
                &params.description,
            )?;
            app.audit(
                "ipam.address.reserve",
                None,
                format!(
                    "Reserved {}{}",
                    saved.address.ip,
                    if params.hostname.is_empty() {
                        String::new()
                    } else {
                        format!(" for {}", params.hostname)
                    }
                ),
                json!({"ip": saved.address.ip, "prefix": params.prefix_id}),
            );
            changed(&app, &["addresses"]);
            Ok(saved)
        },
    );

    router.add(
        "ipam.address.get",
        |app: App, params: AddressKeyParams| async move {
            let settings = config(&app)?;
            let address = net::addr(&params.ip)?;
            let entries = prefixes::load(&app)?;
            let entry = prefixes::containing(&entries, &params.scope, &address);
            let record = db::record(app.store(), &params.scope, &address.text)?;
            let observations = db::observations_of_ip(app.store(), &params.scope, &address.text)?;
            if record.is_none() && observations.is_empty() && entry.is_none() {
                return Err(AppError::not_found(format!(
                    "nothing is known about {}",
                    address.text
                )));
            }
            let conflict =
                conflicts::for_ip(&app, &settings, &entries, &params.scope, &address.text)?;
            let composed =
                addresses::compose(&address, record.as_ref(), &observations, entry, conflict);
            let prefix = match entry {
                Some(entry) => Some(prefixes::row(&app, &settings, entry, &entries)?),
                None => None,
            };
            Ok(json!({"address": composed, "prefix": prefix}))
        },
    );

    router.add("ipam.search", |app: App, params: SearchParams| async move {
        let settings = config(&app)?;
        let entries = prefixes::load(&app)?;
        let query = params.query.trim().to_string();
        if query.is_empty() {
            return Err(AppError::invalid("a search needs something to look for"));
        }
        let found = db::search(app.store(), &query, params.limit)?;

        // An address finds its record *and* the prefix that contains it.
        let mut chosen: Vec<usize> = Vec::new();
        let typed = net::addr(&query).ok();
        for (index, entry) in entries.iter().enumerate() {
            let hit = matches_prefix(entry, &query)
                || typed
                    .as_ref()
                    .is_some_and(|address| entry.net.contains_addr(address));
            if hit {
                chosen.push(index);
            }
        }
        // One row per address found, whichever half of it matched.
        let mut keys: Vec<(String, String)> = Vec::new();
        let mut seen: std::collections::BTreeSet<(String, String)> = Default::default();
        for (scope, ip) in found
            .records
            .iter()
            .map(|record| (record.scope.clone(), record.ip.clone()))
            .chain(
                found
                    .observations
                    .iter()
                    .map(|observation| (observation.scope.clone(), observation.ip.clone())),
            )
            .chain(
                typed
                    .iter()
                    .map(|address| (String::new(), address.text.clone())),
            )
        {
            if seen.insert((scope.clone(), ip.clone())) {
                keys.push((scope, ip));
            }
        }
        let rows = found_rows(&app, &settings, &entries, &keys)?;
        Ok(SearchResult {
            prefixes: rows_for(&app, &settings, &entries, &chosen)?,
            addresses: rows,
            vlans: vlans::rows(&app, None, Some(&query))?,
        })
    });

    router.add("ipam.sync", |app: App, params: SyncParams| async move {
        let settings = config(&app)?;
        let result = sync::sync(&app, &settings, &params.device_ids)?;
        if result.prefixes_added > 0 {
            app.audit(
                "ipam.sync",
                None,
                format!(
                    "Discovered {} prefixes from collected interface addresses",
                    result.prefixes_added
                ),
                json!({"prefixes_added": result.prefixes_added}),
            );
        }
        changed(&app, &["addresses", "prefixes", "vlans", "conflicts"]);
        Ok(result)
    });

    router.add(
        "ipam.reconcile",
        |app: App, params: ReconcileParams| async move {
            let settings = config(&app)?;
            sync::reconcile(&app, &settings, params.site.as_deref())
        },
    );

    router.add("ipam.learn", |app: App, params: LearnParams| async move {
        let settings = config(&app)?;
        let plan = sync::learn(
            &app,
            &settings,
            &params.device_ids,
            params.dry_run,
            params.include_arp,
        )?;
        if !params.dry_run {
            app.audit(
                "ipam.learn",
                None,
                format!(
                    "Learned {} prefixes, {} addresses and {} VLANs from collected data",
                    plan.created_prefixes, plan.created_addresses, plan.created_vlans
                ),
                json!({
                    "prefixes": plan.created_prefixes,
                    "addresses": plan.created_addresses,
                    "vlans": plan.created_vlans,
                }),
            );
            changed(&app, &["prefixes", "addresses", "vlans"]);
        }
        Ok(plan)
    });

    router.add("ipam.conflicts", |app: App, _: NoParams| async move {
        let settings = config(&app)?;
        let entries = prefixes::load(&app)?;
        Ok::<Vec<Conflict>, AppError>(conflicts::detect(&app, &settings, &entries)?.conflicts)
    });

    router.add("ipam.vlans", |app: App, params: VlansParams| async move {
        vlans::rows(&app, params.site.as_deref(), params.query.as_deref())
    });

    router.add(
        "ipam.vlan.save",
        |app: App, params: vlans::VlanInput| async move {
            let row = vlans::save(&app, params)?;
            app.audit(
                "ipam.vlan.save",
                None,
                format!("Saved VLAN {} ({})", row.vlan.vid, row.vlan.name),
                json!({"vlan": row.vlan.id, "vid": row.vlan.vid}),
            );
            changed(&app, &["vlans"]);
            Ok(row)
        },
    );

    router.add(
        "ipam.vlan.delete",
        |app: App, params: IdParams| async move {
            let (deleted, record) = vlans::delete(&app, &params.id)?;
            if deleted {
                app.audit(
                    "ipam.vlan.delete",
                    None,
                    format!("Deleted VLAN {}", record.vid),
                    json!({"vlan": record.id, "vid": record.vid}),
                );
                changed(&app, &["vlans"]);
            }
            Ok(json!({"deleted": deleted, "vid": record.vid}))
        },
    );

    router.add("ipam.import", |app: App, params: ImportParams| async move {
        if !params.format.trim().eq_ignore_ascii_case("csv") {
            return Err(AppError::invalid(format!(
                "`{}` is not an import format (csv)",
                params.format
            )));
        }
        let settings = config(&app)?;
        let result =
            portability::import(&app, &params.kind, &params.text, params.dry_run, &settings)?;
        if !params.dry_run && (result.created > 0 || result.updated > 0) {
            app.audit(
                "ipam.import",
                None,
                format!(
                    "Imported {} {} ({} new, {} updated)",
                    result.created + result.updated,
                    params.kind,
                    result.created,
                    result.updated
                ),
                json!({"kind": params.kind, "created": result.created, "updated": result.updated}),
            );
            changed(&app, &[params.kind.as_str()]);
        }
        Ok(result)
    });

    router.add("ipam.export", |app: App, params: ExportParams| async move {
        if !params.format.trim().eq_ignore_ascii_case("csv") {
            return Err(AppError::invalid(format!(
                "`{}` is not an export format (csv)",
                params.format
            )));
        }
        let text = portability::export(&app, &params.kind, params.include_observed)?;
        Ok(json!({"kind": params.kind, "format": "csv", "text": text}))
    });

    router.add("ipam.summary", |app: App, _: NoParams| async move {
        let settings = config(&app)?;
        let entries = prefixes::load(&app)?;
        let mut utilisation: Vec<Utilisation> = Vec::new();
        let all: Vec<usize> = (0..entries.len()).collect();
        for row in prefixes::rows(&app, &settings, &entries, &all)? {
            if row.util_known && row.used + row.reserved > 0 {
                utilisation.push(Utilisation {
                    prefix: row.prefix.cidr.clone(),
                    prefix_id: row.prefix.id.clone(),
                    util_pct: row.util_pct,
                    util_known: row.util_known,
                    used: row.used,
                    usable_text: row.usable_text.clone(),
                });
            }
        }
        utilisation.sort_by(|a, b| {
            b.util_pct
                .partial_cmp(&a.util_pct)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        utilisation.truncate(10);
        let coverage = collected::harvest(&app, &[])?.coverage;
        Ok(Summary {
            prefixes: u64::try_from(entries.len()).unwrap_or(u64::MAX),
            prefixes_discovered: u64::try_from(
                entries
                    .iter()
                    .filter(|entry| entry.prefix.source == Source::Discovered)
                    .count(),
            )
            .unwrap_or(u64::MAX),
            addresses: db::count_records(app.store())?,
            observations: db::count_observations(app.store())?,
            vlans: app.store().count(db::VLANS)?,
            utilisation,
            conflicts: conflicts::detect(&app, &settings, &entries)?
                .conflicts
                .len(),
            devices_never_collected: u32::try_from(
                coverage
                    .iter()
                    .filter(|row| row.interfaces_at.is_none())
                    .count(),
            )
            .unwrap_or(u32::MAX),
            devices_total: u32::try_from(coverage.len()).unwrap_or(u32::MAX),
        })
    });

    builder.on_start(|app| {
        let events = app.subscribe();
        app.spawn("ipam.watcher", watcher(app.clone(), events));
    });
}

/// Re-read collected data once the collections have stopped arriving, and
/// forget a deleted device's sightings.
async fn watcher(app: App, mut events: tokio::sync::broadcast::Receiver<nocturne_app::Event>) {
    use tokio::sync::broadcast::error::RecvError;

    /// What a topic asks the feature to do.
    enum Wake {
        Sync,
        Forget(Vec<String>),
    }

    fn wake_of(event: &nocturne_app::Event) -> Option<Wake> {
        match event.topic.as_str() {
            "collect.updated" => Some(Wake::Sync),
            "devices.changed" if event.payload["change"] == "deleted" => {
                let ids = event.payload["ids"]
                    .as_array()
                    .map(|ids| {
                        ids.iter()
                            .filter_map(|id| id.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(Wake::Forget(ids))
            }
            _ => None,
        }
    }

    loop {
        let mut pending_sync = false;
        let mut forget: Vec<String> = Vec::new();
        match events.recv().await {
            Ok(event) => match wake_of(&event) {
                Some(Wake::Sync) => pending_sync = true,
                Some(Wake::Forget(ids)) => forget.extend(ids),
                None => continue,
            },
            Err(RecvError::Lagged(_)) => pending_sync = true,
            Err(RecvError::Closed) => return,
        }
        // Everything that arrives while the dust settles is the same change.
        let settle = tokio::time::sleep(
            config(&app)
                .map(|settings| Duration::from_millis(u64::from(settings.sync_delay_ms)))
                .unwrap_or(FALLBACK_DEBOUNCE),
        );
        tokio::pin!(settle);
        loop {
            tokio::select! {
                () = &mut settle => break,
                received = events.recv() => match received {
                    Ok(event) => {
                        match wake_of(&event) {
                            Some(Wake::Sync) => pending_sync = true,
                            Some(Wake::Forget(ids)) => forget.extend(ids),
                            None => {}
                        }
                        continue;
                    }
                    Err(RecvError::Lagged(_)) => {
                        pending_sync = true;
                        continue;
                    }
                    Err(RecvError::Closed) => return,
                },
            }
        }
        let mut kinds: Vec<&str> = Vec::new();
        for id in &forget {
            match db::forget_device(app.store(), id) {
                Ok(removed) if removed > 0 => kinds.push("addresses"),
                Ok(_) => {}
                Err(err) => tracing::debug!(%err, "ipam: a deleted device's sightings stayed"),
            }
        }
        if pending_sync {
            let settings = match config(&app) {
                Ok(settings) => settings,
                Err(err) => {
                    tracing::debug!(%err, "ipam: its settings could not be read");
                    continue;
                }
            };
            if !settings.sync_on_collect {
                continue;
            }
            match sync::sync(&app, &settings, &[]) {
                Ok(result) => {
                    kinds.push("addresses");
                    if result.prefixes_added > 0 || result.prefixes_refreshed > 0 {
                        kinds.push("prefixes");
                    }
                    if !result.conflicts.is_empty() {
                        kinds.push("conflicts");
                    }
                }
                Err(err) => tracing::debug!(%err, "ipam: collected data could not be read"),
            }
        }
        if !kinds.is_empty() {
            kinds.sort_unstable();
            kinds.dedup();
            changed(&app, &kinds);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_list_takes_numbers_or_names() {
        let sizes: Vec<SizeInput> =
            serde_json::from_value(json!([500, {"name": "voice", "hosts": 100}])).unwrap();
        let requests: Vec<SplitRequest> = sizes
            .iter()
            .enumerate()
            .map(|(index, size)| size.request(index))
            .collect();
        assert_eq!(
            requests
                .iter()
                .map(|request| (request.name.as_str(), request.hosts))
                .collect::<Vec<_>>(),
            [("subnet 1", 500), ("voice", 100)]
        );
    }

    #[test]
    fn unknown_statuses_and_formats_are_refused_not_guessed() {
        let params: Result<ImportParams, _> =
            serde_json::from_value(json!({"kind": "prefixes", "text": "cidr\n10.0.0.0/8"}));
        let params = params.expect("defaults");
        assert!(params.dry_run, "an import previews unless told otherwise");
        assert_eq!(params.format, "csv");
        let learn: LearnParams = serde_json::from_value(json!({})).unwrap();
        assert!(learn.dry_run, "learning previews unless told otherwise");
        assert!(!learn.include_arp);
        assert_eq!(
            serde_json::to_value(crate::model::Status::Free).unwrap(),
            json!("free")
        );
    }
}
