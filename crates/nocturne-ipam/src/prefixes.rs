//! The prefix tree, and everything computed rather than typed: what contains
//! what, how full a block is, which addresses in it are free, where the next
//! child of a given size would go.

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use nocturne_tools::{subnet, vlsm};
use serde::{Deserialize, Serialize};

use crate::db;
use crate::model::{DhcpRange, FreeRange, Prefix, PrefixDevice, PrefixRow, Settings, Source};
use crate::net::{self, Addr, Net};

/// How many interfaces a prefix row lists. The count is not capped; the list
/// is, because nobody reads ten thousand of them.
const DEVICES_PER_ROW: usize = 50;
/// How many free runs `ipam.prefix.get` reports.
pub const FREE_RANGES: usize = 200;

/// A prefix with its place in the tree worked out.
#[derive(Clone, Debug)]
pub struct Entry {
    pub prefix: Prefix,
    pub net: Net,
    /// The namespace its addresses live in: `""` or `lab:<lab id>`.
    pub scope: String,
    pub parent_id: Option<String>,
    pub depth: u32,
    pub children: u32,
}

/// A lab's addresses never mix with production utilisation, so the site of a
/// lab prefix *is* its namespace.
pub fn scope_of_site(site: Option<&str>) -> String {
    match site {
        Some(site) if site.starts_with("lab:") => site.to_string(),
        _ => String::new(),
    }
}

/// Every prefix, in address order, with parents linked to children.
pub fn load(app: &App) -> AppResult<Vec<Entry>> {
    let mut entries: Vec<Entry> = Vec::new();
    for prefix in app.store().list::<Prefix>(db::PREFIXES)? {
        // A stored prefix that cannot be read as one is a bug worth seeing
        // rather than a row that silently disappears.
        let net = match net::net(&prefix.cidr) {
            Ok(parsed) => parsed.net,
            Err(err) => {
                tracing::error!(prefix = %prefix.cidr, %err, "ipam: a stored prefix is not a prefix");
                continue;
            }
        };
        entries.push(Entry {
            scope: scope_of_site(prefix.site.as_deref()),
            prefix,
            net,
            parent_id: None,
            depth: 0,
            children: 0,
        });
    }
    entries.sort_by(|a, b| {
        (
            &a.scope,
            a.net.family,
            a.net.base,
            a.net.len,
            &a.prefix.cidr,
        )
            .cmp(&(
                &b.scope,
                b.net.family,
                b.net.base,
                b.net.len,
                &b.prefix.cidr,
            ))
    });
    // Sorted by network then by length, a block's parent is the nearest entry
    // still open above it.
    let mut stack: Vec<usize> = Vec::new();
    let mut children: Vec<u32> = vec![0; entries.len()];
    for index in 0..entries.len() {
        while let Some(&top) = stack.last() {
            let same_scope = entries[top].scope == entries[index].scope;
            if same_scope && entries[top].net.contains_net(&entries[index].net) {
                break;
            }
            stack.pop();
        }
        if let Some(&parent) = stack.last() {
            entries[index].parent_id = Some(entries[parent].prefix.id.clone());
            entries[index].depth = entries[parent].depth + 1;
            children[parent] += 1;
        }
        stack.push(index);
    }
    for (index, count) in children.into_iter().enumerate() {
        entries[index].children = count;
    }
    Ok(entries)
}

pub fn find<'a>(entries: &'a [Entry], id: &str) -> AppResult<&'a Entry> {
    entries
        .iter()
        .find(|entry| entry.prefix.id == id)
        .ok_or_else(|| AppError::not_found(format!("prefix `{id}`")))
}

/// The most specific prefix of `scope` that contains the address.
pub fn containing<'a>(entries: &'a [Entry], scope: &str, address: &Addr) -> Option<&'a Entry> {
    entries
        .iter()
        .filter(|entry| entry.scope == scope && entry.net.contains_addr(address))
        .max_by_key(|entry| entry.net.len)
}

/// The most specific prefix of `scope` that contains a whole block.
pub fn containing_net<'a>(entries: &'a [Entry], scope: &str, block: &Net) -> Option<&'a Entry> {
    entries
        .iter()
        .filter(|entry| entry.scope == scope && entry.net.contains_net(block))
        .max_by_key(|entry| entry.net.len)
}

/// Direct children of an entry.
pub fn children_of<'a>(entries: &'a [Entry], id: &str) -> Vec<&'a Entry> {
    entries
        .iter()
        .filter(|entry| entry.parent_id.as_deref() == Some(id))
        .collect()
}

/// The moment before which an ARP sighting no longer counts as occupancy.
pub fn arp_floor(settings: &Settings, now: i64) -> i64 {
    now - i64::from(settings.arp_stale_days) * 24 * 60 * 60 * 1000
}

/// Everything a whole tree of prefixes needs, read once.
///
/// A campus has two thousand prefixes and a quarter of a million addresses.
/// Two queries per row would be four thousand queries for one screen, so the
/// keys are read once per namespace and each row's range is found by binary
/// search.
pub struct Bulk {
    scopes: std::collections::BTreeMap<String, Scope>,
}

struct Scope {
    records: Vec<db::RecordPoint>,
    observed: Vec<db::ObservedPoint>,
    interfaces: Vec<crate::model::Observation>,
}

/// The half-open index range of `block` in a slice sorted by `(family, key)`.
fn slice_of<T>(
    rows: &[T],
    block: &Net,
    family: impl Fn(&T) -> u8,
    key: impl Fn(&T) -> &str,
) -> (usize, usize) {
    let low = block.key();
    let high = block.last_key();
    let start = rows.partition_point(|row| (family(row), key(row)) < (block.family, low.as_str()));
    let end = rows.partition_point(|row| (family(row), key(row)) <= (block.family, high.as_str()));
    (start, end)
}

impl Bulk {
    /// Read what the given entries need.
    pub fn load(app: &App, settings: &Settings, entries: &[Entry]) -> AppResult<Bulk> {
        let floor = arp_floor(settings, now_ms());
        let mut scopes: std::collections::BTreeMap<String, Scope> =
            std::collections::BTreeMap::new();
        for entry in entries {
            if scopes.contains_key(&entry.scope) {
                continue;
            }
            scopes.insert(
                entry.scope.clone(),
                Scope {
                    records: db::record_points(app.store(), &entry.scope)?,
                    observed: db::observed_points(app.store(), &entry.scope, floor)?,
                    interfaces: db::interface_observations(app.store(), &entry.scope)?,
                },
            );
        }
        Ok(Bulk { scopes })
    }

    fn counts(&self, entry: &Entry) -> db::Counts {
        let Some(scope) = self.scopes.get(&entry.scope) else {
            return db::Counts::default();
        };
        let (start, end) = slice_of(
            &scope.records,
            &entry.net,
            |point| point.family,
            |point| point.key.as_str(),
        );
        let mut counts = db::Counts::default();
        for point in &scope.records[start..end] {
            counts.records += 1;
            if point.reserved {
                counts.reserved += 1;
            }
            if point.in_use {
                counts.in_use_records += 1;
            }
        }
        let (start, end) = slice_of(
            &scope.observed,
            &entry.net,
            |point| point.family,
            |point| point.key.as_str(),
        );
        for point in &scope.observed[start..end] {
            counts.observed += 1;
            if point.undocumented {
                counts.observed_only += 1;
            }
        }
        counts
    }

    fn interfaces(&self, entry: &Entry, limit: usize) -> Vec<crate::model::Observation> {
        let Some(scope) = self.scopes.get(&entry.scope) else {
            return Vec::new();
        };
        scope
            .interfaces
            .iter()
            .filter(|observation| {
                crate::net::addr(&observation.ip)
                    .map(|address| entry.net.contains_addr(&address))
                    .unwrap_or(false)
            })
            .take(limit)
            .cloned()
            .collect()
    }
}

/// A prefix with its numbers. The counts come from SQL aggregates over the
/// block's range; the device list is the interface sightings inside it.
pub fn row(
    app: &App,
    settings: &Settings,
    entry: &Entry,
    entries: &[Entry],
) -> AppResult<PrefixRow> {
    let counts = db::counts_in(
        app.store(),
        &entry.scope,
        &entry.net,
        arp_floor(settings, now_ms()),
    )?;
    let interfaces = db::interfaces_in(app.store(), &entry.scope, &entry.net, DEVICES_PER_ROW)?;
    Ok(assemble(entry, entries, counts, interfaces))
}

/// The rows of a whole tree, with one bulk read behind them.
pub fn rows(
    app: &App,
    settings: &Settings,
    entries: &[Entry],
    chosen: &[usize],
) -> AppResult<Vec<PrefixRow>> {
    let wanted: Vec<&Entry> = chosen.iter().map(|index| &entries[*index]).collect();
    let bulk = Bulk::load(
        app,
        settings,
        &wanted
            .iter()
            .map(|entry| (*entry).clone())
            .collect::<Vec<_>>(),
    )?;
    Ok(wanted
        .into_iter()
        .map(|entry| {
            let counts = bulk.counts(entry);
            let interfaces = bulk.interfaces(entry, DEVICES_PER_ROW);
            assemble(entry, entries, counts, interfaces)
        })
        .collect())
}

/// Put one row together from its parts.
fn assemble(
    entry: &Entry,
    entries: &[Entry],
    counts: db::Counts,
    interfaces: Vec<crate::model::Observation>,
) -> PrefixRow {
    let usable = entry.net.usable();
    // "In use" is a record that says so, plus an address the network shows
    // that nobody wrote down. An address that is both is counted once.
    let used = counts.in_use_records.saturating_add(counts.observed_only);
    let free = usable
        .saturating_sub(u64::from(used))
        .saturating_sub(u64::from(counts.reserved));
    let exponent = u32::from(entry.net.bits() - entry.net.len);
    let util_known = exponent < 64 && usable > 0;
    let util_pct = if util_known {
        (f64::from(used + counts.reserved) / usable as f64 * 100.0) as f32
    } else {
        0.0
    };
    let mut devices: Vec<PrefixDevice> = interfaces
        .iter()
        .map(|observation| PrefixDevice {
            device_id: observation.device_id.clone(),
            device: observation.device.clone(),
            interface: observation.interface.clone(),
            ip: observation.ip.clone(),
            prefix_len: observation.prefix_len,
            collected_at: observation.collected_at,
            lab: observation.scope.starts_with("lab:"),
        })
        .collect();
    devices.sort_by(|a, b| (&a.device, &a.interface, &a.ip).cmp(&(&b.device, &b.interface, &b.ip)));
    let covered: u128 = children_of(entries, &entry.prefix.id)
        .iter()
        .map(|child| 1u128 << (child.net.bits() - child.net.len))
        .sum();
    let own = 1u128 << (entry.net.bits() - entry.net.len).min(127);
    let child_coverage_pct = if own == 0 {
        0.0
    } else {
        (covered as f64 / own as f64 * 100.0) as f32
    };
    PrefixRow {
        prefix: entry.prefix.clone(),
        parent_id: entry.parent_id.clone(),
        depth: entry.depth,
        children: entry.children,
        usable,
        usable_text: entry.net.usable_text(),
        used,
        reserved: counts.reserved,
        free,
        util_pct,
        util_known,
        documented: counts.records,
        observed: counts.observed,
        child_coverage_pct,
        devices,
    }
}

// --------------------------------------------------------------- free space

/// Addresses inside a block that nothing claims: no record, no sighting, and
/// not the network, broadcast or anycast address.
///
/// This is computed from the evidence there is. A free run means "nothing
/// documented it and nothing has answered on it", which is why
/// `ipam.prefix.get` reports it next to the collection coverage.
pub fn free_ranges(block: &Net, occupied: &[u128], limit: usize) -> Vec<FreeRange> {
    let (first, last) = block.host_range();
    if last < first {
        return Vec::new();
    }
    let mut taken: Vec<u128> = occupied
        .iter()
        .copied()
        .filter(|value| *value >= first && *value <= last)
        .collect();
    taken.sort_unstable();
    taken.dedup();
    let mut ranges = Vec::new();
    let mut cursor = first;
    for value in taken {
        if value > cursor {
            ranges.push(range_of(block.family, cursor, value - 1));
        }
        // `value == last` and `value + 1` would overflow only at the very top
        // of the address space, where there is nothing left to describe.
        match value.checked_add(1) {
            Some(next) => cursor = next,
            None => return ranges,
        }
        if cursor > last {
            return ranges;
        }
        if ranges.len() >= limit {
            return ranges;
        }
    }
    if cursor <= last {
        ranges.push(range_of(block.family, cursor, last));
    }
    ranges.truncate(limit);
    ranges
}

fn range_of(family: u8, start: u128, end: u128) -> FreeRange {
    let count = end - start + 1;
    FreeRange {
        start: net::text_of(family, start),
        end: net::text_of(family, end),
        count: u64::try_from(count).unwrap_or(u64::MAX),
        count_text: count.to_string(),
    }
}

/// Child blocks of the prefix that no child prefix covers, as CIDRs — the
/// free space of the tree rather than of the addresses.
pub fn free_child_space(entry: &Entry, entries: &[Entry]) -> AppResult<Vec<String>> {
    let used: Vec<String> = children_of(entries, &entry.prefix.id)
        .iter()
        .map(|child| child.net.cidr())
        .collect();
    if used.is_empty() {
        return Ok(vec![entry.net.cidr()]);
    }
    subnet::free_space(&entry.net.cidr(), &used).map_err(net::tool_error)
}

/// The first free child subnet of a given size, which is what
/// `ipam.next_prefix` answers. Nothing is created.
pub fn next_free_child(entry: &Entry, entries: &[Entry], len: u8) -> AppResult<Option<String>> {
    if len < entry.net.len {
        return Err(AppError::invalid(format!(
            "/{len} is larger than the prefix it would come out of ({})",
            entry.net.cidr()
        )));
    }
    if len > entry.net.bits() {
        return Err(AppError::invalid(format!(
            "/{len} is not a valid length for IPv{}",
            entry.net.family
        )));
    }
    let used: Vec<String> = children_of(entries, &entry.prefix.id)
        .iter()
        .map(|child| child.net.cidr())
        .collect();
    subnet::next_free(&entry.net.cidr(), len, &used).map_err(net::tool_error)
}

// ------------------------------------------------------------ save / delete

/// What a save wants to change. Everything is optional: a field left out
/// keeps the value it had, which is what lets a sync refresh a prefix without
/// touching anything a person wrote.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PrefixInput {
    pub id: Option<String>,
    pub cidr: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub site: Option<String>,
    pub vlan_id: Option<u16>,
    pub role: Option<crate::model::Role>,
    pub gateway: Option<String>,
    pub dhcp: Option<DhcpRange>,
    pub tags: Option<Vec<String>>,
}

/// A saved prefix and what was odd about it. An overlap that is not
/// containment is a warning, never a refusal: real networks have them, and
/// hiding one would be worse than saying so.
#[derive(Debug, Serialize)]
pub struct Saved {
    pub prefix: PrefixRow,
    pub warnings: Vec<String>,
    pub created: bool,
    /// The block was already there as a `discovered` prefix and this save
    /// adopted it, which is how a sighting becomes somebody's plan.
    pub adopted: bool,
}

pub fn save(app: &App, input: PrefixInput, settings: &Settings) -> AppResult<Saved> {
    let entries = load(app)?;
    let named = match &input.id {
        Some(id) if !id.is_empty() => Some(find(&entries, id)?.prefix.clone()),
        _ => None,
    };
    let cidr_text = input
        .cidr
        .clone()
        .or_else(|| named.as_ref().map(|prefix| prefix.cidr.clone()))
        .ok_or_else(|| AppError::invalid("a prefix needs a CIDR block"))?;
    let (cidr, block) = net::strict_net(&cidr_text)?;
    let site = match (&input.site, &named) {
        (Some(site), _) => (!site.trim().is_empty()).then(|| site.trim().to_string()),
        (None, Some(prefix)) => prefix.site.clone(),
        (None, None) => None,
    };
    // Documenting a block a sync already discovered adopts that record
    // instead of refusing: the discovery was never anybody's plan, and two
    // records for one block in one site would make "the" prefix ambiguous.
    let adopted = named.is_none()
        && entries.iter().any(|entry| {
            entry.net == block
                && entry.prefix.site == site
                && entry.prefix.source == Source::Discovered
        });
    let existing = named.or_else(|| {
        entries
            .iter()
            .find(|entry| {
                entry.net == block
                    && entry.prefix.site == site
                    && entry.prefix.source == Source::Discovered
            })
            .map(|entry| entry.prefix.clone())
    });
    let created = existing.is_none();
    let mut prefix = existing.clone().unwrap_or_else(|| Prefix {
        id: db::new_id(),
        created_at: now_ms(),
        ..Prefix::default()
    });
    prefix.cidr = cidr;
    if let Some(name) = input.name {
        prefix.name = name.trim().to_string();
    }
    if let Some(description) = input.description {
        prefix.description = description;
    }
    prefix.site = site;
    if let Some(vlan_id) = input.vlan_id {
        if !(1..=4094).contains(&vlan_id) {
            return Err(AppError::invalid(format!(
                "{vlan_id} is not a VLAN id (1 to 4094)"
            )));
        }
        prefix.vlan_id = Some(vlan_id);
    }
    if let Some(role) = input.role {
        prefix.role = Some(role);
    }
    if let Some(tags) = input.tags {
        prefix.tags = tags;
    }
    let mut warnings = Vec::new();
    if let Some(gateway) = input.gateway {
        if gateway.trim().is_empty() {
            prefix.gateway = None;
        } else {
            let address = net::addr(&gateway)?;
            if !block.contains_addr(&address) {
                return Err(AppError::invalid(format!(
                    "the gateway {} is not inside {}",
                    address.text, prefix.cidr
                )));
            }
            if !block.is_usable_host(address.value) {
                warnings.push(format!(
                    "{} is the network or broadcast address of {}",
                    address.text, prefix.cidr
                ));
            }
            prefix.gateway = Some(address.text);
        }
    }
    if let Some(dhcp) = input.dhcp {
        if dhcp.start.trim().is_empty() && dhcp.end.trim().is_empty() {
            prefix.dhcp = None;
        } else {
            let start = net::addr(&dhcp.start)?;
            let end = net::addr(&dhcp.end)?;
            if start.family != end.family {
                return Err(AppError::invalid(
                    "the DHCP range starts and ends in different address families",
                ));
            }
            if start.value > end.value {
                return Err(AppError::invalid(format!(
                    "the DHCP range {} to {} runs backwards",
                    start.text, end.text
                )));
            }
            if !block.contains_addr(&start) || !block.contains_addr(&end) {
                return Err(AppError::invalid(format!(
                    "the DHCP range {} to {} is not inside {}",
                    start.text, end.text, prefix.cidr
                )));
            }
            prefix.dhcp = Some(DhcpRange {
                start: start.text,
                end: end.text,
            });
        }
    }
    // A person touching a discovered prefix adopts it.
    if existing.is_some() {
        prefix.source = Source::Manual;
    }
    prefix.updated_at = now_ms();

    let scope = scope_of_site(prefix.site.as_deref());
    for entry in &entries {
        if entry.prefix.id == prefix.id || entry.scope != scope {
            continue;
        }
        if entry.net == block && entry.prefix.site == prefix.site {
            return Err(AppError::conflict(format!(
                "{} already exists{} (`{}`)",
                prefix.cidr,
                prefix
                    .site
                    .as_ref()
                    .map(|site| format!(" in {site}"))
                    .unwrap_or_default(),
                if entry.prefix.name.is_empty() {
                    entry.prefix.id.as_str()
                } else {
                    entry.prefix.name.as_str()
                }
            )));
        }
        if entry.net.overlaps(&block)
            && !entry.net.contains_net(&block)
            && !block.contains_net(&entry.net)
        {
            warnings.push(format!(
                "{} overlaps {} without containing it",
                prefix.cidr,
                entry.net.cidr()
            ));
        }
    }

    app.store().put(db::PREFIXES, &prefix.id, &prefix)?;
    let entries = load(app)?;
    let entry = find(&entries, &prefix.id)?;
    Ok(Saved {
        prefix: row(app, settings, entry, &entries)?,
        warnings,
        created,
        adopted,
    })
}

/// What deleting a prefix did to what pointed at it.
#[derive(Debug, Serialize)]
pub struct Deleted {
    pub deleted: bool,
    pub cidr: String,
    /// Children that now hang off this prefix's parent.
    pub children_reparented: u32,
    /// Address records that claimed it and now claim its parent, or nothing.
    pub addresses_repointed: u32,
}

pub fn delete(app: &App, id: &str) -> AppResult<Deleted> {
    let entries = load(app)?;
    let entry = find(&entries, id)?;
    let children = entry.children;
    let parent = entry.parent_id.clone();
    let cidr = entry.prefix.cidr.clone();
    let mut repointed = 0;
    for mut record in db::records_claiming(app.store(), id)? {
        record.prefix_id = parent.clone();
        record.updated_at = now_ms();
        db::put_record(app.store(), &record)?;
        repointed += 1;
    }
    let deleted = app.store().delete(db::PREFIXES, id)?;
    Ok(Deleted {
        deleted,
        cidr,
        children_reparented: children,
        addresses_repointed: repointed,
    })
}

// ------------------------------------------------------------------- splits

/// One child a split would create, or did.
#[derive(Clone, Debug, Serialize)]
pub struct SplitChild {
    pub cidr: String,
    pub name: String,
    /// Hosts the request asked for (VLSM only).
    pub hosts_requested: Option<u64>,
    pub usable: u64,
    /// Spare addresses the allocation leaves (VLSM only).
    pub spare: Option<u64>,
    /// The id it was created as; absent in a dry run or when it already existed.
    pub id: Option<String>,
    /// Why it was not created.
    pub skipped: Option<String>,
}

/// Plan a split, either into equal blocks or by host counts (VLSM). Nothing
/// is created here, which is what makes `dry_run` honest.
pub fn plan_split(
    entry: &Entry,
    new_len: Option<u8>,
    sizes: &[SplitRequest],
    p2p31: bool,
) -> AppResult<Vec<SplitChild>> {
    match (new_len, sizes.is_empty()) {
        (Some(len), true) => {
            if len <= entry.net.len {
                return Err(AppError::invalid(format!(
                    "/{len} is not smaller than {}",
                    entry.net.cidr()
                )));
            }
            let result = subnet::split(&entry.net.cidr(), len, 4096).map_err(net::tool_error)?;
            if result.truncated {
                return Err(AppError::invalid(format!(
                    "splitting {} into /{len} makes {} subnets, which is more than Nocturne will create at once",
                    entry.net.cidr(),
                    result.total
                )));
            }
            Ok(result
                .subnets
                .into_iter()
                .map(|info| SplitChild {
                    cidr: info.cidr,
                    name: String::new(),
                    hosts_requested: None,
                    usable: info.usable_hosts.parse().unwrap_or(0),
                    spare: None,
                    id: None,
                    skipped: None,
                })
                .collect())
        }
        (None, false) => {
            let requests: Vec<vlsm::Request> = sizes
                .iter()
                .map(|request| vlsm::Request {
                    name: request.name.clone(),
                    hosts: request.hosts,
                })
                .collect();
            let plan = vlsm::plan(&entry.net.cidr(), &requests, p2p31).map_err(net::tool_error)?;
            Ok(plan
                .allocations
                .into_iter()
                .map(|allocation| SplitChild {
                    cidr: allocation.cidr,
                    name: allocation.name,
                    hosts_requested: Some(allocation.hosts_requested),
                    usable: allocation.usable_hosts,
                    spare: Some(allocation.spare),
                    id: None,
                    skipped: None,
                })
                .collect())
        }
        (Some(_), false) => Err(AppError::invalid(
            "a split takes either one new length or a list of host counts, not both",
        )),
        (None, true) => Err(AppError::invalid("a split needs `new_len` or `sizes`")),
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct SplitRequest {
    #[serde(default)]
    pub name: String,
    pub hosts: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Role;

    fn entry(cidr: &str, site: Option<&str>) -> Entry {
        let net = net::net(cidr).unwrap().net;
        Entry {
            prefix: Prefix {
                id: cidr.to_string(),
                cidr: net.cidr(),
                site: site.map(str::to_string),
                ..Prefix::default()
            },
            scope: scope_of_site(site),
            net,
            parent_id: None,
            depth: 0,
            children: 0,
        }
    }

    #[test]
    fn free_runs_are_the_gaps_between_what_is_claimed() {
        let block = net::net("10.1.10.0/24").unwrap().net;
        let occupied = [
            net::addr("10.1.10.1").unwrap().value,
            net::addr("10.1.10.2").unwrap().value,
            net::addr("10.1.10.254").unwrap().value,
        ];
        let ranges = free_ranges(&block, &occupied, 200);
        assert_eq!(ranges.len(), 1);
        assert_eq!(
            (
                ranges[0].start.as_str(),
                ranges[0].end.as_str(),
                ranges[0].count
            ),
            ("10.1.10.3", "10.1.10.253", 251)
        );
        // Nothing claimed: one run of every host address, network and
        // broadcast left out.
        let whole = free_ranges(&block, &[], 200);
        assert_eq!(
            (
                whole[0].start.as_str(),
                whole[0].end.as_str(),
                whole[0].count
            ),
            ("10.1.10.1", "10.1.10.254", 254)
        );
    }

    #[test]
    fn a_thirty_one_and_a_thirty_two_have_free_runs_too() {
        let p31 = net::net("192.168.1.0/31").unwrap().net;
        let ranges = free_ranges(&p31, &[], 10);
        assert_eq!(
            (
                ranges[0].start.as_str(),
                ranges[0].end.as_str(),
                ranges[0].count
            ),
            ("192.168.1.0", "192.168.1.1", 2)
        );
        let taken = [net::addr("192.168.1.0").unwrap().value];
        let ranges = free_ranges(&p31, &taken, 10);
        assert_eq!(
            (ranges[0].start.as_str(), ranges[0].count),
            ("192.168.1.1", 1)
        );
        let p32 = net::net("192.168.1.7/32").unwrap().net;
        assert_eq!(free_ranges(&p32, &[], 10)[0].count, 1);
        assert!(
            free_ranges(&p32, &[net::addr("192.168.1.7").unwrap().value], 10).is_empty(),
            "a /32 whose one address is taken has no free run"
        );
    }

    #[test]
    fn the_top_of_a_v6_block_does_not_overflow() {
        let block = net::net("2001:db8::/126").unwrap().net;
        let occupied: Vec<u128> = (0..4).map(|offset| block.base + offset).collect();
        assert!(free_ranges(&block, &occupied, 10).is_empty());
        let whole = net::net("::/0").unwrap().net;
        let ranges = free_ranges(&whole, &[], 4);
        assert_eq!(ranges[0].start, "::1");
        assert_eq!(ranges[0].count, u64::MAX, "saturated; the text is exact");
        assert_eq!(
            ranges[0].count_text,
            "340282366920938463463374607431768211455"
        );
    }

    #[test]
    fn a_split_matches_a_plan_computed_by_hand() {
        let parent = entry("10.1.0.0/22", None);
        let children = plan_split(&parent, Some(24), &[], false).unwrap();
        assert_eq!(
            children
                .iter()
                .map(|child| child.cidr.as_str())
                .collect::<Vec<_>>(),
            ["10.1.0.0/24", "10.1.1.0/24", "10.1.2.0/24", "10.1.3.0/24"]
        );
        assert_eq!(children[0].usable, 254);
        // VLSM: largest first, aligned.
        let sizes = [
            SplitRequest {
                name: "users".into(),
                hosts: 500,
            },
            SplitRequest {
                name: "voice".into(),
                hosts: 200,
            },
            SplitRequest {
                name: "link".into(),
                hosts: 2,
            },
        ];
        let plan = plan_split(&parent, None, &sizes, false).unwrap();
        assert_eq!(
            plan.iter()
                .map(|child| (child.name.as_str(), child.cidr.as_str()))
                .collect::<Vec<_>>(),
            [
                ("users", "10.1.0.0/23"),
                ("voice", "10.1.2.0/24"),
                ("link", "10.1.3.0/30")
            ]
        );
        assert_eq!(plan[2].usable, 2);
        // And with /31 point-to-point links.
        let plan = plan_split(&parent, None, &sizes[2..], true).unwrap();
        assert_eq!(plan[0].cidr, "10.1.0.0/31");
        assert_eq!(
            plan_split(&parent, Some(22), &[], false).unwrap_err().code,
            "invalid"
        );
        assert_eq!(
            plan_split(&parent, Some(24), &sizes, false)
                .unwrap_err()
                .code,
            "invalid"
        );
        assert_eq!(
            plan_split(&parent, None, &[], false).unwrap_err().code,
            "invalid"
        );
    }

    #[test]
    fn the_next_free_child_skips_what_is_already_there() {
        let parent = entry("10.1.0.0/22", None);
        let mut entries = vec![parent.clone()];
        for (index, cidr) in ["10.1.0.0/24", "10.1.1.0/24"].into_iter().enumerate() {
            let mut child = entry(cidr, None);
            child.parent_id = Some(parent.prefix.id.clone());
            child.depth = 1;
            child.prefix.id = format!("child{index}");
            entries.push(child);
        }
        assert_eq!(
            next_free_child(&parent, &entries, 24).unwrap().as_deref(),
            Some("10.1.2.0/24")
        );
        assert_eq!(
            next_free_child(&parent, &entries, 25).unwrap().as_deref(),
            Some("10.1.2.0/25")
        );
        assert_eq!(
            next_free_child(&parent, &entries, 21).unwrap_err().code,
            "invalid"
        );
        assert_eq!(
            next_free_child(&parent, &entries, 40).unwrap_err().code,
            "invalid"
        );
        // Full: nothing comes back, which is not an error.
        let mut full = vec![parent.clone()];
        for (index, cidr) in ["10.1.0.0/23", "10.1.2.0/23"].into_iter().enumerate() {
            let mut child = entry(cidr, None);
            child.parent_id = Some(parent.prefix.id.clone());
            child.prefix.id = format!("half{index}");
            full.push(child);
        }
        assert_eq!(next_free_child(&parent, &full, 24).unwrap(), None);
        assert_eq!(
            free_child_space(&parent, &full).unwrap(),
            Vec::<String>::new()
        );
        assert_eq!(
            free_child_space(&parent, std::slice::from_ref(&parent)).unwrap(),
            ["10.1.0.0/22"]
        );
    }

    #[test]
    fn roles_and_sites_are_part_of_a_prefixs_identity() {
        // A lab's site is its namespace; a production site is not.
        assert_eq!(scope_of_site(Some("lab:abc")), "lab:abc");
        assert_eq!(scope_of_site(Some("HQ")), "");
        assert_eq!(scope_of_site(None), "");
        let value = serde_json::to_value(Role::P2p).unwrap();
        assert_eq!(value, serde_json::json!("p2p"));
    }
}
