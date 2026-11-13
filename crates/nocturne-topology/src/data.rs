//! Everything collected, read once, plus the index that turns what a
//! neighbour calls itself into a device in the inventory.
//!
//! Nothing here reaches a device: it is all `collect::latest_all` and
//! `inventory::list`.

use std::collections::BTreeMap;

use nocturne_app::collect::{self, Stored};
use nocturne_app::error::AppResult;
use nocturne_app::model::{Device, DeviceRow};
use nocturne_app::{App, inventory};
use nocturne_drivers::ifname;
use nocturne_drivers::model::{ArpEntry, Facts, Interface, Kind, MacEntry, Neighbor, Route, Vlan};

/// Rows of one kind from one device, with the time they were read.
#[derive(Clone, Debug, Default)]
pub struct Aged<T> {
    pub at: i64,
    pub rows: Vec<T>,
}

impl<T> Aged<T> {
    fn from(stored: &Stored, rows: Vec<T>) -> Aged<T> {
        Aged {
            at: stored.collected_at,
            rows,
        }
    }
}

type ByDevice<T> = BTreeMap<String, Aged<T>>;

fn load<T: serde::de::DeserializeOwned>(app: &App, kind: Kind) -> AppResult<ByDevice<T>> {
    let mut by_device = BTreeMap::new();
    for stored in collect::latest_all(app, kind)? {
        let rows = stored.typed::<T>();
        by_device.insert(stored.device_id.clone(), Aged::from(&stored, rows));
    }
    Ok(by_device)
}

/// Every device and everything read from it, loaded once per request.
pub struct Collected {
    pub devices: Vec<DeviceRow>,
    pub interfaces: ByDevice<Interface>,
    pub neighbors: ByDevice<Neighbor>,
    pub vlans: ByDevice<Vlan>,
    pub mac_table: ByDevice<MacEntry>,
    pub arp: ByDevice<ArpEntry>,
    pub routes: ByDevice<Route>,
    pub facts: ByDevice<Facts>,
    index: Index,
}

impl Collected {
    /// Everything, for the questions that need the forwarding tables.
    pub fn load(app: &App) -> AppResult<Collected> {
        Collected::load_with(app, true)
    }

    /// `tables` false leaves the MAC and ARP tables unread. They are the
    /// largest thing a campus collects — hundreds of thousands of rows — and
    /// drawing the graph does not need them unless hosts are being drawn.
    pub fn load_with(app: &App, tables: bool) -> AppResult<Collected> {
        let devices = inventory::list(app, &inventory::DeviceFilter::default())?;
        let interfaces = load::<Interface>(app, Kind::Interfaces)?;
        let facts = load::<Facts>(app, Kind::Facts)?;
        let index = Index::build(&devices, &interfaces, &facts);
        Ok(Collected {
            neighbors: load(app, Kind::Neighbors)?,
            vlans: load(app, Kind::Vlans)?,
            mac_table: if tables {
                load(app, Kind::MacTable)?
            } else {
                BTreeMap::new()
            },
            arp: if tables {
                load(app, Kind::Arp)?
            } else {
                BTreeMap::new()
            },
            routes: load(app, Kind::Routes)?,
            devices,
            interfaces,
            facts,
            index,
        })
    }

    pub fn device(&self, id: &str) -> Option<&DeviceRow> {
        self.devices.iter().find(|row| row.device.id == id)
    }

    pub fn name_of(&self, id: &str) -> String {
        self.device(id)
            .map(|row| row.device.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    /// The interface of a device by any spelling of its name.
    pub fn interface(&self, device_id: &str, name: &str) -> Option<&Interface> {
        self.interfaces
            .get(device_id)?
            .rows
            .iter()
            .find(|interface| ifname::same(&interface.name, name))
    }

    /// The device that owns an IP address, by its collected interfaces or its
    /// inventory address.
    pub fn device_with_address(&self, address: &str) -> Option<&str> {
        self.index
            .by_address
            .get(&normalise_address(address))?
            .one()
    }

    /// Every address of a device, longest prefix first, for display.
    pub fn addresses_of(&self, device_id: &str) -> Vec<String> {
        let mut addresses: Vec<String> = Vec::new();
        if let Some(aged) = self.interfaces.get(device_id) {
            for interface in &aged.rows {
                for cidr in &interface.ipv4 {
                    if let Some(host) = host_of(cidr) {
                        addresses.push(host);
                    }
                }
            }
        }
        if let Some(row) = self.device(device_id)
            && !row.device.address.is_empty()
            && !addresses.contains(&row.device.address)
        {
            addresses.push(row.device.address.clone());
        }
        addresses
    }
}

/// `10.0.0.1/30` → `10.0.0.1`; a bare address is returned as it is.
pub fn host_of(cidr: &str) -> Option<String> {
    let text = cidr.trim();
    if text.is_empty() {
        return None;
    }
    let host = text.split('/').next().unwrap_or(text).trim();
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn normalise_address(address: &str) -> String {
    address.trim().to_ascii_lowercase()
}

/// A lookup that refuses to answer when two devices claim the same key.
#[derive(Debug, Default, Clone)]
pub struct Claim {
    ids: Vec<String>,
}

impl Claim {
    fn add(&mut self, id: &str) {
        if !self.ids.iter().any(|known| known == id) {
            self.ids.push(id.to_string());
        }
    }

    /// The one device that claims this key, or `None` when several do.
    pub fn one(&self) -> Option<&str> {
        match self.ids.as_slice() {
            [only] => Some(only.as_str()),
            _ => None,
        }
    }
}

/// How a neighbour report was matched to the inventory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Matched {
    /// A device, and the sentence explaining which field matched.
    Device { device_id: String, how: String },
    /// Not in the inventory: a stable key and a name to show.
    Unmanaged { key: String, name: String },
    /// Two devices answer to this name; refusing is better than guessing.
    Ambiguous { name: String },
    /// The report named nobody at all.
    Nothing,
}

/// Name → device, address → device, chassis MAC → device.
#[derive(Debug, Default)]
pub struct Index {
    pub by_name: BTreeMap<String, Claim>,
    pub by_label: BTreeMap<String, Claim>,
    pub by_address: BTreeMap<String, Claim>,
    pub by_mac: BTreeMap<String, Claim>,
}

impl Index {
    fn build(
        devices: &[DeviceRow],
        interfaces: &ByDevice<Interface>,
        facts: &ByDevice<Facts>,
    ) -> Index {
        let mut index = Index::default();
        for row in devices {
            let device = &row.device;
            index.name(&device.name, &device.id);
            if !device.address.is_empty() {
                index
                    .by_address
                    .entry(normalise_address(&device.address))
                    .or_default()
                    .add(&device.id);
            }
            if let Some(hostname) = facts
                .get(&device.id)
                .and_then(|aged| aged.rows.first())
                .map(|facts| facts.hostname.as_str())
                .filter(|hostname| !hostname.is_empty())
            {
                index.name(hostname, &device.id);
            }
            if let Some(aged) = interfaces.get(&device.id) {
                for interface in &aged.rows {
                    for cidr in interface.ipv4.iter().chain(interface.ipv6.iter()) {
                        if let Some(host) = host_of(cidr) {
                            index.by_address.entry(host).or_default().add(&device.id);
                        }
                    }
                    if let Some(mac) = bare_mac(&interface.mac) {
                        index.by_mac.entry(mac).or_default().add(&device.id);
                    }
                }
            }
        }
        index
    }

    fn name(&mut self, name: &str, device_id: &str) {
        let full = normalise_name(name);
        if full.is_empty() {
            return;
        }
        self.by_name.entry(full.clone()).or_default().add(device_id);
        let label = first_label(&full);
        if label != full {
            self.by_label.entry(label).or_default().add(device_id);
        }
    }

    /// Match what a neighbour protocol reported to a device in the inventory.
    ///
    /// Exact matches only, in order of how much they prove: chassis MAC, full
    /// name, first DNS label, management address. Never by similarity.
    pub fn resolve(&self, reported: &str, address: &str) -> Matched {
        let name = normalise_name(reported);
        if let Some(mac) = bare_mac(&name)
            && let Some(claim) = self.by_mac.get(&mac)
        {
            if let Some(device_id) = claim.one() {
                return Matched::Device {
                    device_id: device_id.to_string(),
                    how: format!(
                        "chassis MAC {} is an interface of this device",
                        pretty(&mac)
                    ),
                };
            }
            return Matched::Ambiguous { name: pretty(&mac) };
        }
        if !name.is_empty() {
            if let Some(claim) = self.by_name.get(&name) {
                if let Some(device_id) = claim.one() {
                    return Matched::Device {
                        device_id: device_id.to_string(),
                        how: format!("reported name `{name}` is the device's name"),
                    };
                }
                return Matched::Ambiguous { name };
            }
            let label = first_label(&name);
            if let Some(claim) = self
                .by_label
                .get(&label)
                .or_else(|| self.by_name.get(&label))
            {
                if let Some(device_id) = claim.one() {
                    return Matched::Device {
                        device_id: device_id.to_string(),
                        how: format!("`{name}` is the fully qualified name of `{label}`"),
                    };
                }
                return Matched::Ambiguous { name };
            }
        }
        let address_key = normalise_address(address);
        if !address_key.is_empty()
            && let Some(claim) = self.by_address.get(&address_key)
        {
            if let Some(device_id) = claim.one() {
                return Matched::Device {
                    device_id: device_id.to_string(),
                    how: format!("management address {address_key} belongs to this device"),
                };
            }
            return Matched::Ambiguous { name: address_key };
        }
        if name.is_empty() && address_key.is_empty() {
            return Matched::Nothing;
        }
        let (key, display) = unmanaged_identity(&name, &address_key);
        Matched::Unmanaged { key, name: display }
    }
}

/// A stable key and a display name for a neighbour that is not in the inventory.
fn unmanaged_identity(name: &str, address: &str) -> (String, String) {
    if !name.is_empty() {
        if let Some(mac) = bare_mac(name) {
            let pretty_mac = pretty(&mac);
            let display = match nocturne_tools::mac::vendor(&pretty_mac) {
                Some(vendor) => format!("{vendor} {}", &pretty_mac[9..]),
                None => pretty_mac,
            };
            return (format!("u:{mac}"), display);
        }
        let label = first_label(name);
        return (format!("u:{label}"), label);
    }
    (format!("u:{address}"), address.to_string())
}

/// `CORE1.lab.example` → `core1.lab.example`; `SW1(FOC1234X5)` → `sw1`.
pub fn normalise_name(name: &str) -> String {
    let mut text = name.trim();
    if let Some(open) = text.find('(')
        && text.ends_with(')')
        && open > 0
    {
        text = text[..open].trim_end();
    }
    text.trim_end_matches('.').to_ascii_lowercase()
}

/// The first DNS label: `core1.lab.example` → `core1`.
pub fn first_label(name: &str) -> String {
    name.split('.').next().unwrap_or(name).to_string()
}

/// A MAC address in any notation as 12 lowercase hex digits, or `None` when
/// the text is not one. Bare 12-digit input is only read as a MAC when it
/// really is all hexadecimal.
pub fn bare_mac(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.len() < 12 {
        return None;
    }
    let info = nocturne_tools::mac::info(trimmed).ok()?;
    Some(info.bare)
}

/// 12 hex digits as `aa:bb:cc:dd:ee:ff`.
pub fn pretty(bare: &str) -> String {
    bare.as_bytes()
        .chunks(2)
        .map(|pair| String::from_utf8_lossy(pair).to_string())
        .collect::<Vec<_>>()
        .join(":")
}

/// Whether a device is reachable, as the graph shows it.
pub fn status_of(row: &DeviceRow) -> crate::model::Status {
    crate::model::Status::from_flag(row.state.reachable)
}

/// The site of a device, or `None` when it has none.
pub fn site_of(device: &Device) -> Option<String> {
    Some(device.site.clone()).filter(|site| !site.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_normalised_the_way_devices_report_them() {
        assert_eq!(normalise_name("  CORE1.lab.example "), "core1.lab.example");
        assert_eq!(normalise_name("SW1(FOC1234X5)"), "sw1");
        assert_eq!(normalise_name("SW1."), "sw1");
        assert_eq!(first_label("core1.lab.example"), "core1");
        assert_eq!(first_label("core1"), "core1");
    }

    #[test]
    fn macs_are_recognised_in_every_notation() {
        for text in [
            "00:1b:21:aa:bb:cc",
            "001b.21aa.bbcc",
            "00-1B-21-AA-BB-CC",
            "001b21aabbcc",
        ] {
            assert_eq!(bare_mac(text).as_deref(), Some("001b21aabbcc"), "{text}");
        }
        assert_eq!(bare_mac("core1"), None);
        assert_eq!(bare_mac("switch-upstairs"), None);
        assert_eq!(pretty("001b21aabbcc"), "00:1b:21:aa:bb:cc");
    }

    #[test]
    fn host_addresses_come_out_of_cidr() {
        assert_eq!(host_of("10.0.0.1/30").as_deref(), Some("10.0.0.1"));
        assert_eq!(host_of("10.0.0.1").as_deref(), Some("10.0.0.1"));
        assert_eq!(host_of("  "), None);
    }
}
