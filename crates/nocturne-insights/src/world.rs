//! Everything a scan reasons over, loaded once.
//!
//! Checks are pure functions of a [`World`]: they never touch a device, never
//! query the database and never look at the clock, which is what makes them
//! testable one case at a time. Loading is done in bulk (`collect::latest_all`
//! is one query per kind for the whole fleet, not one per device) because a
//! scan runs after every collection.
//!
//! Three things are deliberately *not* in here: metric history and log
//! history (see [`crate::metrics`] and [`crate::logs`], which are queried per
//! device because their volume makes preloading the wrong shape), and
//! anything that needs another feature's RPC (links from `topology.get`,
//! applied changes) — those are fetched before the scan starts and handed in.

use std::collections::HashMap;
use std::sync::Arc;

use nocturne_app::model::{Device, DeviceRow};
use nocturne_app::store::{ConfigVersion, Store};
use nocturne_app::{App, AppResult, collect, inventory, profiles};
use nocturne_drivers::hier::{ConfigTree, Syntax};
use nocturne_drivers::ifname;
use nocturne_drivers::model::{ArpEntry, Facts, Interface, Kind, MacEntry, Neighbor, Route, Vlan};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;

use crate::links::Link;

/// One kind of collected data, with when it was read and by what.
#[derive(Clone, Debug, Default)]
pub struct Data<T> {
    pub rows: Vec<T>,
    pub at: i64,
    pub source: String,
}

impl<T> Data<T> {
    /// The single row of a kind that describes the device as a whole.
    pub fn one(&self) -> Option<&T> {
        self.rows.first()
    }

    pub fn age_ms(&self, now: i64) -> i64 {
        now - self.at
    }
}

/// A stored configuration, parsed, with its interface sections indexed.
#[derive(Clone, Debug)]
pub struct Config {
    pub version_id: i64,
    pub taken_at: i64,
    pub syntax: Syntax,
    pub tree: Arc<ConfigTree>,
    /// Canonical interface name → index into `tree.roots`.
    pub(crate) sections: Arc<HashMap<String, usize>>,
    /// The listing runs to the `end` line an IOS-style device closes a
    /// configuration with. The tree cannot say: its parser drops that line
    /// as display. A check that reasons from what is *absent* needs it, for
    /// a listing that stops early is missing things it never had a chance
    /// to print.
    pub closed: bool,
}

impl Config {
    /// The configuration section of one interface, by any spelling of its name.
    pub fn interface(&self, name: &str) -> Option<&nocturne_drivers::hier::Node> {
        let index = *self.sections.get(&canonical(name))?;
        self.tree.roots.get(index)
    }

    /// The lines of a top-level section whose first line equals `head`.
    pub fn section(&self, head: &str) -> Option<&nocturne_drivers::hier::Node> {
        self.tree.roots.iter().find(|node| node.text == head)
    }

    /// Top-level lines starting with `prefix`.
    pub fn lines_starting(&self, prefix: &str) -> impl Iterator<Item = &str> {
        self.tree
            .roots
            .iter()
            .map(|node| node.text.as_str())
            .filter(move |line| line.starts_with(prefix))
    }

    pub fn has_line_starting(&self, prefix: &str) -> bool {
        self.lines_starting(prefix).next().is_some()
    }
}

/// The configuration the device would come back with after a reload.
#[derive(Clone, Debug)]
pub struct Startup {
    pub version_id: i64,
    pub taken_at: i64,
    pub text: String,
}

/// Everything known about one device at scan time.
#[derive(Clone, Debug, Default)]
pub struct DeviceData {
    pub device: Device,
    pub facts: Option<Data<Facts>>,
    pub previous_facts: Option<Data<Facts>>,
    pub interfaces: Option<Data<Interface>>,
    pub previous_interfaces: Option<Data<Interface>>,
    pub neighbors: Option<Data<Neighbor>>,
    pub previous_neighbors: Option<Data<Neighbor>>,
    pub vlans: Option<Data<Vlan>>,
    pub mac_table: Option<Data<MacEntry>>,
    pub previous_mac_table: Option<Data<MacEntry>>,
    pub arp: Option<Data<ArpEntry>>,
    pub routes: Option<Data<Route>>,
    pub config: Option<Config>,
    pub startup: Option<Startup>,
    /// The grammar of this device's configuration, `Flat` when unknown.
    pub syntax: Syntax,
    /// The platform's own patterns for lines that are display rather than
    /// configuration (timestamps, byte counts): what `config.ignore` in the
    /// profile exists for. Used when running and startup are compared.
    /// Shared, because compiling these per device is the most expensive thing
    /// a scan could do for no reason at all.
    pub config_ignore: Arc<Vec<regex::Regex>>,
    /// Canonical interface name → index into `interfaces.rows`.
    pub(crate) interface_index: HashMap<String, usize>,
}

impl DeviceData {
    pub fn id(&self) -> &str {
        &self.device.id
    }

    pub fn name(&self) -> &str {
        &self.device.name
    }

    pub fn lab(&self) -> bool {
        self.device.lab.is_some()
    }

    /// One interface by any spelling of its name.
    pub fn interface(&self, name: &str) -> Option<&Interface> {
        let index = *self.interface_index.get(&canonical(name))?;
        self.interfaces.as_ref()?.rows.get(index)
    }

    pub fn interface_rows(&self) -> &[Interface] {
        self.interfaces
            .as_ref()
            .map(|data| data.rows.as_slice())
            .unwrap_or_default()
    }

    pub fn previous_interface(&self, name: &str) -> Option<&Interface> {
        let wanted = canonical(name);
        self.previous_interfaces
            .as_ref()?
            .rows
            .iter()
            .find(|interface| canonical(&interface.name) == wanted)
    }

    /// Interfaces that have a neighbour in the latest collection.
    pub fn neighbors_of(&self, name: &str) -> impl Iterator<Item = &Neighbor> {
        let wanted = canonical(name);
        self.neighbors
            .as_ref()
            .map(|data| data.rows.as_slice())
            .unwrap_or_default()
            .iter()
            .filter(move |neighbor| canonical(&neighbor.local_interface) == wanted)
    }

    /// Is this device's data too old to compare with another device's?
    pub fn stale(&self, now: i64, hours: u32) -> bool {
        let newest = [
            self.interfaces.as_ref().map(|data| data.at),
            self.facts.as_ref().map(|data| data.at),
            self.neighbors.as_ref().map(|data| data.at),
        ]
        .into_iter()
        .flatten()
        .max();
        match newest {
            Some(at) => now - at > i64::from(hours) * 3_600_000,
            None => true,
        }
    }
}

/// The scan's view of the network.
pub struct World {
    pub now: i64,
    pub devices: Vec<DeviceData>,
    pub(crate) index: HashMap<String, usize>,
    /// Indices of the devices this scan is allowed to decide for.
    pub(crate) targets: Vec<usize>,
    /// `targets` as a flag per device, so that "is this a target" stays O(1)
    /// when it is asked once per link on a campus-sized graph.
    pub(crate) target_flags: Vec<bool>,
    pub links: Vec<Link>,
    /// Where the links came from, for the coverage report and evidence.
    pub link_source: &'static str,
    /// Whether the configuration feature is registered: without it, "no
    /// backup" means "nothing is taking backups", not "this device is unsaved".
    pub config_feature: bool,
    /// Applied Nocturne changes per device, newest first `(device_id, [(change_id, title, at)])`.
    pub applied_changes: Option<HashMap<String, Vec<AppliedChange>>>,
    /// `(device, metric)` pairs that monitoring records — a series holding a
    /// sample, see [`crate::metrics::present`] — read in one query: asking per
    /// device and metric is thousands of round trips on a campus, for a
    /// question one `DISTINCT` answers.
    pub(crate) series: std::collections::HashSet<(String, String)>,
    pub(crate) store: Arc<Store>,
}

/// A change Nocturne itself applied, used to tell "we did that" from "someone
/// walked up to the device".
#[derive(Clone, Debug)]
pub struct AppliedChange {
    pub id: String,
    pub title: String,
    pub at: i64,
}

impl World {
    pub fn device(&self, id: &str) -> Option<&DeviceData> {
        self.index
            .get(id)
            .and_then(|index| self.devices.get(*index))
    }

    /// The devices this scan decides for.
    pub fn targets(&self) -> impl Iterator<Item = &DeviceData> {
        self.targets
            .iter()
            .filter_map(|index| self.devices.get(*index))
    }

    pub fn is_target(&self, device_id: &str) -> bool {
        self.index
            .get(device_id)
            .is_some_and(|index| self.target_flags.get(*index).copied().unwrap_or(false))
    }

    pub fn target_count(&self) -> usize {
        self.targets.len()
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Re-read which metrics are recorded. A scan reads this once while
    /// loading; the per-check tests insert their samples after the world is
    /// built and call this to say so.
    #[cfg(test)]
    pub(crate) fn reindex_series(&mut self) {
        self.series = crate::metrics::present(&self.store).unwrap_or_default();
    }

    /// Does monitoring record this metric for this device — has any series of
    /// it a sample? The difference between "nothing is over the threshold"
    /// and "nothing was measured". It says nothing about *when*: a check that
    /// reasons about now still has to find samples in its own window.
    pub fn has_metric(&self, device_id: &str, metric: &str) -> bool {
        self.series
            .contains(&(device_id.to_string(), metric.to_string()))
    }

    /// Links with at least one end among the scan's targets.
    pub fn target_links(&self) -> impl Iterator<Item = &Link> {
        self.links
            .iter()
            .filter(|link| self.is_target(&link.a.device_id) || self.is_target(&link.b.device_id))
    }
}

/// Canonical form of an interface name for joining data from different commands.
pub fn canonical(name: &str) -> String {
    ifname::long(name).to_ascii_lowercase()
}

/// What parsing a stored configuration yields, shared between scans.
#[derive(Clone)]
pub(crate) struct Parsed {
    pub(crate) tree: Arc<ConfigTree>,
    pub(crate) sections: Arc<HashMap<String, usize>>,
    pub(crate) closed: bool,
}

impl Parsed {
    pub(crate) fn of(text: &str, syntax: Syntax) -> Parsed {
        let tree = ConfigTree::parse(text, syntax);
        Parsed {
            sections: Arc::new(interface_sections(&tree)),
            tree: Arc::new(tree),
            closed: closes_with_end(text),
        }
    }
}

/// Is the last line of the listing `end`, at the left margin? Blank lines
/// and `!` separators after it are display, as they are everywhere else in
/// the listing. An indented `end` is a line of some section, not the close.
pub(crate) fn closes_with_end(text: &str) -> bool {
    text.lines().map(str::trim_end).rfind(|line| {
        let content = line.trim_start();
        !content.is_empty() && !content.starts_with('!')
    }) == Some("end")
}

/// Parsed configurations, kept between scans. A campus has hundreds of
/// configurations and a scan runs after every collection; re-parsing them all
/// each time is the single most expensive thing this feature could do.
#[derive(Default)]
pub struct ConfigCache {
    trees: Mutex<HashMap<i64, Parsed>>,
}

impl ConfigCache {
    /// At most this many parsed configurations are kept (a few MB).
    const CAPACITY: usize = 2_000;

    fn get_or_parse(&self, version_id: i64, syntax: Syntax, text: &str) -> Parsed {
        let cached = self.trees.lock().get(&version_id).cloned();
        if let Some(found) = cached {
            return found;
        }
        let parsed = Parsed::of(text, syntax);
        let mut trees = self.trees.lock();
        // Configuration versions are append-only, so eviction can be crude:
        // what matters is that memory stays bounded.
        if trees.len() >= Self::CAPACITY {
            trees.clear();
        }
        trees.insert(version_id, parsed.clone());
        parsed
    }
}

pub(crate) fn interface_sections(tree: &ConfigTree) -> HashMap<String, usize> {
    let mut sections = HashMap::new();
    for (index, node) in tree.roots.iter().enumerate() {
        // `interface GigabitEthernet0/1`, `interface ether1`.
        if let Some(name) = node.text.strip_prefix("interface ") {
            sections.entry(canonical(name)).or_insert(index);
        }
    }
    sections
}

/// What a scan was asked to look at.
pub struct LoadRequest<'a> {
    /// Empty = every device.
    pub device_ids: &'a [String],
    pub include_lab: bool,
    pub links: Vec<Link>,
    pub link_source: &'static str,
    pub applied_changes: Option<HashMap<String, Vec<AppliedChange>>>,
    pub now: i64,
}

/// Read the fleet's collected data, configurations and device records.
pub fn load(app: &App, request: LoadRequest<'_>) -> AppResult<World> {
    let rows: Vec<DeviceRow> = inventory::list(app, &Default::default())?;
    let cache = app.state::<ConfigCache>();

    let mut devices: Vec<DeviceData> = Vec::with_capacity(rows.len());
    let mut index = HashMap::with_capacity(rows.len());
    let mut compiled = HashMap::new();
    for row in rows {
        if row.device.lab.is_some() && !request.include_lab {
            continue;
        }
        let (syntax, config_ignore) = platform_of(app, &row, &mut compiled);
        index.insert(row.device.id.clone(), devices.len());
        devices.push(DeviceData {
            device: row.device,
            syntax,
            config_ignore,
            ..DeviceData::default()
        });
    }

    // Collected data: one query per kind for the whole fleet.
    fill(app, &mut devices, &index, Kind::Facts, |data, rows| {
        data.facts = Some(rows)
    })?;
    fill(app, &mut devices, &index, Kind::Interfaces, |data, rows| {
        data.interfaces = Some(rows)
    })?;
    fill(app, &mut devices, &index, Kind::Neighbors, |data, rows| {
        data.neighbors = Some(rows)
    })?;
    fill(app, &mut devices, &index, Kind::Vlans, |data, rows| {
        data.vlans = Some(rows)
    })?;
    fill(app, &mut devices, &index, Kind::MacTable, |data, rows| {
        data.mac_table = Some(rows)
    })?;
    fill(app, &mut devices, &index, Kind::Arp, |data, rows| {
        data.arp = Some(rows)
    })?;
    fill(app, &mut devices, &index, Kind::Routes, |data, rows| {
        data.routes = Some(rows)
    })?;

    for data in &mut devices {
        data.interface_index = data
            .interfaces
            .iter()
            .flat_map(|interfaces| interfaces.rows.iter().enumerate())
            .map(|(position, interface)| (canonical(&interface.name), position))
            .collect();
    }

    // Which devices this scan decides for.
    let targets: Vec<usize> = if request.device_ids.is_empty() {
        (0..devices.len()).collect()
    } else {
        request
            .device_ids
            .iter()
            .filter_map(|id| index.get(id).copied())
            .collect()
    };

    let mut target_flags = vec![false; devices.len()];
    for position in &targets {
        if let Some(flag) = target_flags.get_mut(*position) {
            *flag = true;
        }
    }

    // The previous generation and the stored configurations are only needed
    // for the devices a verdict is reached for — except running configurations,
    // which the far end of a link is compared against.
    for position in 0..devices.len() {
        let device_id = devices[position].device.id.clone();
        if let Some(config) = running_config(app, &cache, &device_id, devices[position].syntax)? {
            devices[position].config = Some(config);
        }
        if !target_flags[position] {
            continue;
        }
        devices[position].previous_interfaces = previous(app, &device_id, Kind::Interfaces)?;
        devices[position].previous_facts = previous(app, &device_id, Kind::Facts)?;
        devices[position].previous_mac_table = previous(app, &device_id, Kind::MacTable)?;
        devices[position].previous_neighbors = previous(app, &device_id, Kind::Neighbors)?;
        devices[position].startup = startup_config(app, &device_id)?;
    }

    Ok(World {
        now: request.now,
        devices,
        index,
        targets,
        target_flags,
        links: request.links,
        link_source: request.link_source,
        config_feature: app.methods().contains(&"config.latest"),
        applied_changes: request.applied_changes,
        series: crate::metrics::present(app.store())?,
        store: app.store().clone(),
    })
}

/// The configuration grammar of a device, and the lines its platform says
/// are display rather than configuration — compiled once per profile, not
/// once per device.
fn platform_of(
    app: &App,
    row: &DeviceRow,
    compiled: &mut HashMap<String, (Syntax, Arc<Vec<regex::Regex>>)>,
) -> (Syntax, Arc<Vec<regex::Regex>>) {
    let profile = if row.device.profile.is_empty() {
        row.state.detected_profile.clone().unwrap_or_default()
    } else {
        row.device.profile.clone()
    };
    if profile.is_empty() {
        return (Syntax::Flat, Arc::default());
    }
    if let Some(found) = compiled.get(&profile) {
        return found.clone();
    }
    let platform = match profiles::platform(app, &profile) {
        Ok(platform) => (
            platform.profile.config.syntax,
            Arc::new(
                platform
                    .profile
                    .config
                    .ignore
                    .iter()
                    .filter_map(|pattern| regex::Regex::new(pattern).ok())
                    .collect(),
            ),
        ),
        Err(_) => (Syntax::Flat, Arc::default()),
    };
    compiled.insert(profile, platform.clone());
    platform
}

/// The rows of a stored kind as model structs. `Stored::typed` clones every
/// row to deserialise it; a campus scan reads seventy thousand of them, so
/// here they are read by reference instead.
fn typed_rows<T: DeserializeOwned>(stored: &collect::Stored) -> Vec<T> {
    stored
        .rows
        .iter()
        .filter_map(|row| T::deserialize(row).ok())
        .collect()
}

fn fill<T: DeserializeOwned>(
    app: &App,
    devices: &mut [DeviceData],
    index: &HashMap<String, usize>,
    kind: Kind,
    set: impl Fn(&mut DeviceData, Data<T>),
) -> AppResult<()> {
    for stored in collect::latest_all(app, kind)? {
        let Some(position) = index.get(&stored.device_id) else {
            continue;
        };
        let Some(data) = devices.get_mut(*position) else {
            continue;
        };
        set(
            data,
            Data {
                rows: typed_rows(&stored),
                at: stored.collected_at,
                source: stored.source,
            },
        );
    }
    Ok(())
}

fn previous<T: DeserializeOwned>(
    app: &App,
    device_id: &str,
    kind: Kind,
) -> AppResult<Option<Data<T>>> {
    Ok(collect::previous(app, device_id, kind)?.map(|stored| Data {
        rows: typed_rows(&stored),
        at: stored.collected_at,
        source: stored.source,
    }))
}

fn running_config(
    app: &App,
    cache: &ConfigCache,
    device_id: &str,
    syntax: Syntax,
) -> AppResult<Option<Config>> {
    let Some(version) = app.store().config_latest(device_id, "running")? else {
        return Ok(None);
    };
    Ok(Some(config_of(app, cache, &version, syntax)?))
}

fn config_of(
    app: &App,
    cache: &ConfigCache,
    version: &ConfigVersion,
    syntax: Syntax,
) -> AppResult<Config> {
    // The lookup is in a scope of its own: holding the cache's lock across the
    // read that fills it would deadlock on itself.
    let cached = cache.trees.lock().get(&version.id).cloned();
    let parsed = match cached {
        Some(found) => found,
        None => {
            let (_, text) = app.store().config_get(version.id)?;
            cache.get_or_parse(version.id, syntax, &text)
        }
    };
    Ok(Config {
        version_id: version.id,
        taken_at: version.taken_at,
        syntax,
        tree: parsed.tree,
        sections: parsed.sections,
        closed: parsed.closed,
    })
}

fn startup_config(app: &App, device_id: &str) -> AppResult<Option<Startup>> {
    let Some(version) = app.store().config_latest(device_id, "startup")? else {
        return Ok(None);
    };
    let (_, text) = app.store().config_get(version.id)?;
    Ok(Some(Startup {
        version_id: version.id,
        taken_at: version.taken_at,
        text,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_sections_are_found_by_any_spelling() {
        let parsed = Parsed::of(
            "interface GigabitEthernet0/1\n switchport mode trunk\ninterface Vlan10\n ip address 10.0.0.1 255.255.255.0\n",
            Syntax::Ios,
        );
        let config = Config {
            version_id: 1,
            taken_at: 0,
            syntax: Syntax::Ios,
            sections: parsed.sections,
            tree: parsed.tree,
            closed: parsed.closed,
        };
        assert!(config.interface("Gi0/1").is_some());
        assert!(config.interface("gigabitethernet 0/1").is_some());
        assert!(config.interface("Vl10").is_some());
        assert!(config.interface("Gi0/2").is_none());
    }

    #[test]
    fn the_cache_parses_a_configuration_once_and_stays_bounded() {
        let cache = ConfigCache::default();
        let first = cache.get_or_parse(1, Syntax::Ios, "hostname SW1\n");
        let again = cache.get_or_parse(1, Syntax::Ios, "hostname OTHER\n");
        assert!(Arc::ptr_eq(&first.tree, &again.tree), "the parse is reused");
        for version in 0..ConfigCache::CAPACITY as i64 + 5 {
            cache.get_or_parse(version + 10, Syntax::Ios, "hostname SW\n");
        }
        assert!(cache.trees.lock().len() <= ConfigCache::CAPACITY);
    }

    #[test]
    fn a_listing_is_closed_only_when_its_last_line_is_end() {
        // What a device prints after the last section: `!`, `end`, and
        // sometimes a blank line before the prompt comes back.
        assert!(closes_with_end("hostname SW1\n!\nend\n"));
        assert!(closes_with_end("hostname SW1\r\n!\r\nend\r\n\r\n"));
        assert!(closes_with_end("hostname SW1\nend\n!\n"));
        // Stopped early, at any point.
        assert!(!closes_with_end(
            "hostname SW1\npolicy-map WAN\n class VOICE\n"
        ));
        assert!(!closes_with_end("hostname SW1\n!\n"));
        assert!(!closes_with_end(""));
        // A word that merely starts or ends that way is not the line, and
        // neither is the word inside a section.
        assert!(!closes_with_end("hostname SW1\nend-policy-map\n"));
        assert!(!closes_with_end("hostname SW1\n description the end\n"));
        assert!(!closes_with_end("hostname SW1\nmacro name LEAVE\n end\n"));
        // The cache remembers it with the tree.
        let cache = ConfigCache::default();
        assert!(
            cache
                .get_or_parse(1, Syntax::Ios, "hostname SW1\nend\n")
                .closed
        );
        assert!(!cache.get_or_parse(2, Syntax::Ios, "hostname SW1\n").closed);
        assert!(cache.get_or_parse(1, Syntax::Ios, "ignored: cached").closed);
    }
}
