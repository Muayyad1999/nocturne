//! Turning what devices reported into evidence.
//!
//! Two collected tables matter here: the interfaces (what is *configured*)
//! and the ARP tables (what *answered*). Each row becomes an [`Observation`]
//! that names the device, the interface and the moment the table was read.
//! Nothing in this file writes a record, and a device that was never
//! collected produces no rows and one honest line of coverage instead.

use nocturne_app::collect::{self, Stored};
use nocturne_app::inventory::{self, DeviceFilter};
use nocturne_app::model::DeviceRow;
use nocturne_app::{App, AppResult};
use nocturne_drivers::model::{ArpEntry, Interface, Kind, Vlan as CollectedVlan};

use crate::model::{Observation, ObservationKind};
use crate::net;

/// The device an observation came from, with the namespace its addresses live
/// in. A lab device's addresses are kept apart from production.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub site: String,
    pub scope: String,
    pub lab: bool,
}

impl DeviceInfo {
    fn of(row: &DeviceRow) -> DeviceInfo {
        let lab = row.device.lab.clone();
        DeviceInfo {
            id: row.device.id.clone(),
            name: row.device.name.clone(),
            site: row.device.site.clone(),
            scope: match &lab {
                Some(reference) => format!("lab:{}", reference.lab_id),
                None => String::new(),
            },
            lab: lab.is_some(),
        }
    }
}

/// What is known about one device's contribution — including when it is
/// nothing, and why.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DeviceCoverage {
    pub device_id: String,
    pub device: String,
    pub site: String,
    pub lab: bool,
    /// When the interface table was read; `None` means it never was.
    pub interfaces_at: Option<i64>,
    pub arp_at: Option<i64>,
    pub addresses_configured: u32,
    pub arp_entries: u32,
    /// Why this device says nothing about addresses.
    pub reason: Option<String>,
}

/// Everything read out of collected data in one pass.
#[derive(Debug, Default)]
pub struct Harvest {
    pub interfaces: Vec<Observation>,
    pub arp: Vec<Observation>,
    pub coverage: Vec<DeviceCoverage>,
    /// Addresses deliberately left out: link-local, loopback, unspecified.
    pub skipped_addresses: u32,
}

pub fn devices(app: &App) -> AppResult<Vec<DeviceInfo>> {
    Ok(inventory::list(app, &DeviceFilter::default())?
        .iter()
        .map(DeviceInfo::of)
        .collect())
}

/// Addresses that exist on every device and mean nothing to a plan.
fn is_uninteresting(family: u8, value: u128) -> bool {
    if family == 4 {
        let octets = (value >> 24) as u8;
        // 127/8 (loopback), 0.0.0.0 (unspecified), 169.254/16 (link-local).
        octets == 127 || value == 0 || (value >> 16) == 0xa9fe
    } else {
        // fe80::/10 (link-local), ::1 and :: .
        let top = value >> 118;
        top == 0x3fa || value == 0 || value == 1
    }
}

/// One interface address, as configured, into an observation.
fn from_interface(
    device: &DeviceInfo,
    interface: &Interface,
    text: &str,
    collected_at: i64,
    collector: &str,
) -> Option<Observation> {
    let parsed = net::net(text).ok()?;
    let address = net::addr(text).ok()?;
    if is_uninteresting(address.family, address.value) {
        return None;
    }
    // A configured address with no mask tells us the address but not the
    // subnet, and saying `/32` for it would be an invention.
    let has_mask = text.contains('/') || text.split_whitespace().count() > 1;
    Some(Observation {
        ip: address.text.clone(),
        kind: ObservationKind::Interface,
        device_id: device.id.clone(),
        device: device.name.clone(),
        interface: interface.name.clone(),
        prefix_len: has_mask.then_some(parsed.net.len),
        cidr: has_mask.then(|| format!("{}/{}", address.text, parsed.net.len)),
        mac: (!interface.mac.is_empty()).then(|| interface.mac.clone()),
        vendor: None,
        scope: device.scope.clone(),
        first_seen: collected_at,
        last_seen: collected_at,
        collected_at,
        collector: collector.to_string(),
        current: true,
    })
}

fn from_arp(
    device: &DeviceInfo,
    entry: &ArpEntry,
    collected_at: i64,
    collector: &str,
) -> Option<Observation> {
    let address = net::addr(&entry.ip).ok()?;
    if is_uninteresting(address.family, address.value) {
        return None;
    }
    if entry.mac.trim().is_empty() {
        // An entry nothing answered on is not a sighting.
        return None;
    }
    let mac = nocturne_tools::mac::info(&entry.mac)
        .map(|info| info.colon)
        .unwrap_or_else(|_| entry.mac.clone());
    Some(Observation {
        ip: address.text,
        kind: ObservationKind::Arp,
        device_id: device.id.clone(),
        device: device.name.clone(),
        interface: entry.interface.clone(),
        prefix_len: None,
        cidr: None,
        vendor: nocturne_tools::mac::vendor(&mac),
        mac: Some(mac),
        scope: device.scope.clone(),
        first_seen: collected_at,
        last_seen: collected_at,
        collected_at,
        collector: collector.to_string(),
        current: true,
    })
}

fn stored_of<'a>(stored: &'a [Stored], device_id: &str) -> Option<&'a Stored> {
    stored.iter().find(|row| row.device_id == device_id)
}

/// Read the latest collected interfaces and ARP tables of the given devices
/// (all of them when the list is empty).
pub fn harvest(app: &App, device_ids: &[String]) -> AppResult<Harvest> {
    let all = devices(app)?;
    let wanted: Vec<DeviceInfo> = if device_ids.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|device| device_ids.contains(&device.id))
            .collect()
    };
    let interfaces = collect::latest_all(app, Kind::Interfaces)?;
    let arp_tables = collect::latest_all(app, Kind::Arp)?;
    let mut harvest = Harvest::default();
    for device in &wanted {
        let mut coverage = DeviceCoverage {
            device_id: device.id.clone(),
            device: device.name.clone(),
            site: device.site.clone(),
            lab: device.lab,
            interfaces_at: None,
            arp_at: None,
            addresses_configured: 0,
            arp_entries: 0,
            reason: None,
        };
        match stored_of(&interfaces, &device.id) {
            Some(stored) => {
                coverage.interfaces_at = Some(stored.collected_at);
                for interface in stored.typed::<Interface>() {
                    for text in interface.ipv4.iter().chain(interface.ipv6.iter()) {
                        match from_interface(
                            device,
                            &interface,
                            text,
                            stored.collected_at,
                            &stored.source,
                        ) {
                            Some(observation) => {
                                coverage.addresses_configured += 1;
                                harvest.interfaces.push(observation);
                            }
                            None => harvest.skipped_addresses += 1,
                        }
                    }
                }
                if coverage.addresses_configured == 0 {
                    coverage.reason = Some(
                        "its interface table was read and no interface carries an address"
                            .to_string(),
                    );
                }
            }
            None => {
                coverage.reason = Some("its interfaces have never been collected".to_string());
            }
        }
        if let Some(stored) = stored_of(&arp_tables, &device.id) {
            coverage.arp_at = Some(stored.collected_at);
            for entry in stored.typed::<ArpEntry>() {
                if let Some(observation) =
                    from_arp(device, &entry, stored.collected_at, &stored.source)
                {
                    coverage.arp_entries += 1;
                    harvest.arp.push(observation);
                } else {
                    harvest.skipped_addresses += 1;
                }
            }
        }
        harvest.coverage.push(coverage);
    }
    Ok(harvest)
}

/// A VLAN as one device reports it, with the ports it is on.
#[derive(Clone, Debug, PartialEq)]
pub struct VlanSighting {
    pub device: DeviceInfo,
    pub vid: u16,
    pub name: String,
    pub status: String,
    pub ports: u32,
    pub collected_at: i64,
}

/// The collected VLAN tables of every device. A VLAN and a subnet are one
/// decision, so these sit beside the prefixes rather than in a corner.
pub fn vlan_sightings(app: &App) -> AppResult<Vec<VlanSighting>> {
    let devices = devices(app)?;
    let mut sightings = Vec::new();
    for stored in collect::latest_all(app, Kind::Vlans)? {
        let Some(device) = devices.iter().find(|device| device.id == stored.device_id) else {
            continue;
        };
        for vlan in stored.typed::<CollectedVlan>() {
            let Some(vid) = vlan.id.and_then(|id| u16::try_from(id).ok()) else {
                continue;
            };
            sightings.push(VlanSighting {
                device: device.clone(),
                vid,
                name: vlan.name.clone(),
                status: vlan.status.clone(),
                ports: u32::try_from(vlan.interfaces.len()).unwrap_or(u32::MAX),
                collected_at: stored.collected_at,
            });
        }
    }
    sightings.sort_by(|a, b| (a.vid, &a.device.name).cmp(&(b.vid, &b.device.name)));
    Ok(sightings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device() -> DeviceInfo {
        DeviceInfo {
            id: "d1".into(),
            name: "SW1".into(),
            site: "HQ".into(),
            scope: String::new(),
            lab: false,
        }
    }

    #[test]
    fn a_configured_address_keeps_its_mask_and_its_interface() {
        let interface = Interface {
            name: "Vlan10".into(),
            mac: "aa:bb:cc:dd:ee:ff".into(),
            ..Interface::default()
        };
        let observation =
            from_interface(&device(), &interface, "10.1.10.1/24", 5_000, "cli").unwrap();
        assert_eq!(observation.ip, "10.1.10.1");
        assert_eq!(observation.prefix_len, Some(24));
        assert_eq!(observation.cidr.as_deref(), Some("10.1.10.1/24"));
        assert_eq!(observation.interface, "Vlan10");
        assert_eq!(observation.collected_at, 5_000);
        assert_eq!(observation.kind, ObservationKind::Interface);
        // An address with no mask says nothing about the subnet, and does not
        // get a made-up /32.
        let bare = from_interface(&device(), &interface, "10.1.10.1", 5_000, "cli").unwrap();
        assert_eq!((bare.prefix_len, bare.cidr), (None, None));
        // IPv6 alongside IPv4.
        let v6 = from_interface(&device(), &interface, "2001:db8:a::1/64", 1, "cli").unwrap();
        assert_eq!((v6.ip.as_str(), v6.prefix_len), ("2001:db8:a::1", Some(64)));
    }

    #[test]
    fn addresses_every_device_has_are_not_address_management() {
        let interface = Interface {
            name: "Lo0".into(),
            ..Interface::default()
        };
        for text in [
            "127.0.0.1/8",
            "0.0.0.0/0",
            "169.254.1.1/16",
            "fe80::1/64",
            "::1/128",
        ] {
            assert!(
                from_interface(&device(), &interface, text, 1, "cli").is_none(),
                "{text}"
            );
        }
        // A real loopback address is kept.
        assert!(from_interface(&device(), &interface, "10.255.0.1/32", 1, "cli").is_some());
    }

    #[test]
    fn an_arp_entry_without_an_answer_is_not_a_sighting() {
        let incomplete = ArpEntry {
            ip: "10.1.10.9".into(),
            mac: String::new(),
            interface: "Vlan10".into(),
            age: "-".into(),
        };
        assert!(from_arp(&device(), &incomplete, 1, "cli").is_none());
        let answered = ArpEntry {
            mac: "0000.0c12.3456".into(),
            ..incomplete
        };
        let observation = from_arp(&device(), &answered, 7_000, "cli").unwrap();
        assert_eq!(observation.kind, ObservationKind::Arp);
        assert_eq!(observation.mac.as_deref(), Some("00:00:0c:12:34:56"));
        assert_eq!(observation.vendor.as_deref(), Some("Cisco Systems"));
        assert_eq!(
            observation.prefix_len, None,
            "ARP says nothing about the mask"
        );
        assert_eq!(observation.last_seen, 7_000);
    }
}
