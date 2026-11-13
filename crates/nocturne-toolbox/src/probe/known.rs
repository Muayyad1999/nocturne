//! What the inventory knows about an address, and what a probe is aimed at.
//!
//! This is the half of the toolbox a website cannot have. A ping can be aimed
//! at `SW1` instead of at an address, and an address that comes back from a
//! sweep, a DNS answer or a traceroute can be labelled "that is SW1, VLAN 10"
//! — which during an incident is the difference between a list of numbers and
//! a picture of the network.
//!
//! The map is built from the device records plus the interface addresses the
//! collectors have stored, and only **exact** address matches are reported: an
//! address inside a device's subnet does not belong to that device, and
//! guessing would be fabrication.

use std::collections::HashMap;
use std::net::IpAddr;

use nocturne_app::model::{Device, Transport};
use nocturne_app::{App, AppError, AppResult, collect, inventory};
use nocturne_drivers::model::{Interface, Kind};
use serde::{Deserialize, Serialize};

/// A device of the inventory that an address belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceHint {
    pub id: String,
    pub name: String,
    /// The address that matched.
    pub address: String,
    pub role: String,
    pub site: String,
    /// `true` for a simulated device: lab data is always labelled as lab data.
    pub lab: bool,
    /// Where the match came from: `the device's address in the inventory` or
    /// `interface Vlan10`.
    pub matched: String,
}

/// Every address Nocturne knows about, and whose it is.
///
/// Cloneable: a probe hands a copy to the task that labels its findings as
/// they arrive, which at campus scale is a map of a few hundred entries.
#[derive(Clone, Debug, Default)]
pub struct Known {
    by_address: HashMap<IpAddr, DeviceHint>,
}

impl Known {
    /// Build the map: the devices' own addresses first, then the addresses
    /// their interfaces were last seen with (the device record wins, because
    /// that is the one a person typed).
    ///
    /// This walks the device list and the stored interface tables, which is
    /// cheap at campus scale and wrong at ten thousand devices; the core has
    /// no indexed "who owns this address" lookup and three features now want
    /// one. It is in the report.
    pub fn load(app: &App) -> Known {
        let mut known = Known::default();
        let devices = inventory::list(app, &Default::default())
            .map(|rows| rows.into_iter().map(|row| row.device).collect::<Vec<_>>())
            .unwrap_or_default();
        let by_id: HashMap<String, Device> = devices
            .iter()
            .map(|device| (device.id.clone(), device.clone()))
            .collect();

        if let Ok(stored) = collect::latest_all(app, Kind::Interfaces) {
            for table in stored {
                let Some(device) = by_id.get(&table.device_id) else {
                    continue;
                };
                for interface in table.typed::<Interface>() {
                    for cidr in interface.ipv4.iter().chain(&interface.ipv6) {
                        let text = cidr.split('/').next().unwrap_or(cidr);
                        if let Ok(address) = text.parse::<IpAddr>() {
                            known.by_address.insert(
                                address,
                                hint(device, address, format!("interface {}", interface.name)),
                            );
                        }
                    }
                }
            }
        }
        for device in &devices {
            if let Ok(address) = device.address.parse::<IpAddr>() {
                known.by_address.insert(
                    address,
                    hint(
                        device,
                        address,
                        "the device's address in the inventory".to_string(),
                    ),
                );
            }
        }
        known
    }

    /// Whose address this is, if anybody's.
    pub fn of(&self, address: IpAddr) -> Option<DeviceHint> {
        self.by_address.get(&address).cloned()
    }

    /// Whose address this text is, if it is an address at all.
    pub fn of_text(&self, address: &str) -> Option<DeviceHint> {
        address
            .trim()
            .parse::<IpAddr>()
            .ok()
            .and_then(|address| self.of(address))
    }
}

fn hint(device: &Device, address: IpAddr, matched: String) -> DeviceHint {
    DeviceHint {
        id: device.id.clone(),
        name: device.name.clone(),
        address: address.to_string(),
        role: device.role.clone(),
        site: device.site.clone(),
        lab: device.lab.is_some(),
        matched,
    }
}

/// What a probe was aimed at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    /// What the caller asked for, as they wrote it.
    pub input: String,
    /// What is handed to the engine: an address, or a name for it to resolve.
    pub host: String,
    /// The device the request named, when it named one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<DeviceHint>,
}

/// The longest host name or address a probe will accept.
const MAX_HOST: usize = 253;

/// `host` if it is a host name or an address, and the reason if it is not.
///
/// Every probe validates its target here, not only the ones that put it on a
/// device's command line — but that is where it matters most: `8.8.8.8;
/// reload` must never reach a router, and the only way to be sure is to
/// refuse everything that is not a name or an address, rather than to look
/// for the characters that are known to be dangerous.
pub fn host_name(host: &str) -> AppResult<String> {
    let text = host.trim();
    if text.is_empty() {
        return Err(AppError::invalid(
            "give something to probe: an address, a host name, or `device` for one in the inventory",
        ));
    }
    if text.chars().count() > MAX_HOST {
        return Err(AppError::invalid(format!(
            "`{}…` is longer than any host name can be ({MAX_HOST} characters)",
            text.chars().take(32).collect::<String>()
        )));
    }
    // An address, including IPv6, or a DNS name. No spaces, no shell or CLI
    // punctuation, no control characters, no zone index.
    let allowed = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'));
    if !allowed || text.starts_with('-') {
        let bad: String = text
            .chars()
            .filter(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')))
            .take(4)
            .collect();
        return Err(AppError::invalid(format!(
            "`{text}` is not a host name or an address{}",
            if bad.is_empty() {
                String::new()
            } else {
                format!(" (`{bad}` cannot be part of one)")
            }
        )));
    }
    Ok(text.to_string())
}

/// Work out what to probe from a `{host, device}` pair.
///
/// Exactly one of them is given. `device` is any reference the inventory
/// understands — an id, a name or an address — which is what makes "ping SW1"
/// work; the address probed is the one Nocturne manages the device at.
pub fn resolve(
    app: &App,
    host: Option<&str>,
    device: Option<&str>,
    known: &Known,
) -> AppResult<Target> {
    match (
        host.map(str::trim).filter(|text| !text.is_empty()),
        device.map(str::trim).filter(|text| !text.is_empty()),
    ) {
        (Some(_), Some(_)) => Err(AppError::invalid(
            "give either `host` or `device`, not both",
        )),
        (None, None) => Err(AppError::invalid(
            "give a `host` to probe, or a `device` from the inventory to probe",
        )),
        (Some(host), None) => {
            let host = host_name(host)?;
            Ok(Target {
                device: known.of_text(&host),
                input: host.clone(),
                host,
            })
        }
        (None, Some(reference)) => {
            let found = inventory::find(app, reference)?;
            if found.transport == Transport::Serial {
                return Err(AppError::invalid(format!(
                    "{} is reached over a serial console, so there is no address to probe from here",
                    found.name
                )));
            }
            if found.address.trim().is_empty() {
                return Err(AppError::invalid(format!(
                    "{} has no address in the inventory, so there is nothing to probe",
                    found.name
                )));
            }
            let host = host_name(&found.address)?;
            let hint = known.of_text(&host).unwrap_or_else(|| DeviceHint {
                id: found.id.clone(),
                name: found.name.clone(),
                address: found.address.trim().to_string(),
                role: found.role.clone(),
                site: found.site.clone(),
                lab: found.lab.is_some(),
                matched: "the device's address in the inventory".to_string(),
            });
            Ok(Target {
                input: reference.to_string(),
                host,
                device: Some(hint),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_is_a_host_name_or_an_address_and_nothing_else() {
        assert_eq!(host_name(" 10.0.0.1 ").unwrap(), "10.0.0.1");
        assert_eq!(host_name("sw1.lab.example").unwrap(), "sw1.lab.example");
        assert_eq!(host_name("2001:db8::1").unwrap(), "2001:db8::1");

        // The injection attempts that matter, because this text can end up on
        // a device's command line.
        for attempt in [
            "8.8.8.8; reload",
            "8.8.8.8 && reload",
            "8.8.8.8\nreload",
            "`reload`",
            "$(reload)",
            "8.8.8.8 | show run",
            "8.8.8.8 repeat 99999",
            "-I eth0",
            "10.0.0.1%eth0",
            "",
        ] {
            let refused = host_name(attempt).unwrap_err();
            assert_eq!(refused.code, "invalid", "accepted `{attempt}`");
        }
        assert_eq!(host_name(&"a".repeat(300)).unwrap_err().code, "invalid");
    }
}
