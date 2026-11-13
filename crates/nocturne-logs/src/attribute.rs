//! Which device a line came from, and which interface it is about.
//!
//! A syslog datagram carries no identity a receiver can trust: the host name
//! inside the message is whatever the device was told to call itself, and
//! nothing proves it. The one thing the receiver knows for certain is the
//! address the packet came from, so that is what attribution is based on —
//! first the address a device is managed at, then the addresses its
//! interfaces were collected with.
//!
//! When neither matches, the line is kept with no device and the address is
//! listed in `logs.sources` with the reason. It is never guessed from the
//! host name: two switches called `SW1` in different sites are a perfectly
//! normal thing to find in a campus, and a log line filed against the wrong
//! one is worse than one filed against none.

use std::collections::HashMap;
use std::net::IpAddr;

use nocturne_app::model::Device;
use nocturne_app::{App, AppResult, collect};
use nocturne_drivers::ifname;
use nocturne_drivers::model::{Interface, Kind};

/// What a rule's scope and a `logs.top` label need to know about a device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub site: String,
    pub role: String,
    pub tags: Vec<String>,
    pub profile: String,
    /// Interface names as last collected, lowercased, in both the long and
    /// the short spelling, mapped to the canonical name.
    pub interfaces: HashMap<String, String>,
}

/// The outcome of attributing one address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribution {
    pub device_id: Option<String>,
    /// Empty when a device was found; otherwise why not, in a sentence the
    /// user can act on.
    pub reason: String,
}

/// A snapshot of the inventory, indexed by address. Rebuilt when the
/// inventory or the collected data changes, never per line.
#[derive(Debug, Default)]
pub struct Index {
    pub built_at: i64,
    /// Address → the devices that claim it. More than one is an ambiguity
    /// the user has to resolve, not something to pick a winner from.
    managed: HashMap<IpAddr, Vec<String>>,
    interface_ips: HashMap<IpAddr, Vec<String>>,
    /// Device address written as a name rather than an address.
    names: HashMap<String, Vec<String>>,
    devices: HashMap<String, DeviceInfo>,
}

fn host_of(address: &str) -> Option<IpAddr> {
    let address = address.trim();
    address
        .parse::<IpAddr>()
        .ok()
        // "10.0.0.1/24" as an interface address carries its mask.
        .or_else(|| address.split('/').next()?.trim().parse::<IpAddr>().ok())
}

impl Index {
    /// Read the inventory and the collected interfaces into one index.
    pub fn build(app: &App, at: i64) -> AppResult<Index> {
        let mut index = Index {
            built_at: at,
            ..Index::default()
        };
        let devices: Vec<Device> = app
            .store()
            .list(nocturne_app::model::collections::DEVICES)?;
        for device in &devices {
            match host_of(&device.address) {
                Some(address) => index
                    .managed
                    .entry(address)
                    .or_default()
                    .push(device.id.clone()),
                None if !device.address.trim().is_empty() => index
                    .names
                    .entry(device.address.trim().to_ascii_lowercase())
                    .or_default()
                    .push(device.id.clone()),
                None => {}
            }
            index.devices.insert(
                device.id.clone(),
                DeviceInfo {
                    id: device.id.clone(),
                    name: device.name.clone(),
                    site: device.site.clone(),
                    role: device.role.clone(),
                    tags: device.tags.clone(),
                    profile: device.profile.clone(),
                    interfaces: HashMap::new(),
                },
            );
        }
        for stored in collect::latest_all(app, Kind::Interfaces)? {
            let Some(info) = index.devices.get_mut(&stored.device_id) else {
                continue;
            };
            for interface in stored.typed::<Interface>() {
                if interface.name.trim().is_empty() {
                    continue;
                }
                let canonical = interface.name.trim().to_string();
                for spelling in [
                    canonical.to_ascii_lowercase(),
                    ifname::long(&canonical).to_ascii_lowercase(),
                    ifname::short(&canonical).to_ascii_lowercase(),
                ] {
                    info.interfaces.insert(spelling, canonical.clone());
                }
                for address in interface.ipv4.iter().chain(&interface.ipv6) {
                    if let Some(address) = host_of(address) {
                        index
                            .interface_ips
                            .entry(address)
                            .or_default()
                            .push(stored.device_id.clone());
                    }
                }
            }
        }
        Ok(index)
    }

    pub fn device(&self, device_id: &str) -> Option<&DeviceInfo> {
        self.devices.get(device_id)
    }

    pub fn name_of(&self, device_id: &str) -> String {
        self.devices
            .get(device_id)
            .map(|device| device.name.clone())
            .unwrap_or_default()
    }

    /// Every address this index knows, with the device that claims it: used
    /// to re-point the stored source list after the inventory changes.
    pub fn resolve_all(&self, addresses: &[String]) -> Vec<(String, Option<String>, String)> {
        addresses
            .iter()
            .map(|address| {
                let found = self.attribute(address);
                (address.clone(), found.device_id, found.reason)
            })
            .collect()
    }

    /// The device a line from `source` belongs to.
    pub fn attribute(&self, source: &str) -> Attribution {
        let refused = |reason: String| Attribution {
            device_id: None,
            reason,
        };
        let named = |ids: &[String]| -> String {
            ids.iter()
                .map(|id| self.name_of(id))
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let host = host_of(source);
        let by_address = host.and_then(|address| self.managed.get(&address));
        if let Some(ids) = by_address {
            return match ids.as_slice() {
                [only] => Attribution {
                    device_id: Some(only.clone()),
                    reason: String::new(),
                },
                many => refused(format!(
                    "{source} is the address of {} devices ({}); give them distinct addresses",
                    many.len(),
                    named(many)
                )),
            };
        }
        if let Some(ids) = host.and_then(|address| self.interface_ips.get(&address)) {
            let mut unique: Vec<&String> = ids.iter().collect();
            unique.sort();
            unique.dedup();
            return match unique.as_slice() {
                [only] => Attribution {
                    device_id: Some((*only).clone()),
                    reason: String::new(),
                },
                many => {
                    let many: Vec<String> = many.iter().map(|id| (*id).clone()).collect();
                    refused(format!(
                        "{source} is a collected interface address of {} devices ({})",
                        many.len(),
                        named(&many)
                    ))
                }
            };
        }
        // A device whose address is a host name cannot be matched against a
        // packet's source address without resolving it, which would make
        // attribution depend on DNS at the moment the line arrives.
        if let Some(ids) = self.names.get(&source.trim().to_ascii_lowercase())
            && let [only] = ids.as_slice()
        {
            return Attribution {
                device_id: Some(only.clone()),
                reason: String::new(),
            };
        }
        refused(format!(
            "no device in the inventory has the address {source}, and no collected interface carries it"
        ))
    }

    /// The interface of `device_id` that `message` is about, if it names one.
    ///
    /// Returns the canonical name as the device's own collected interface
    /// list spells it, together with the word the line used. A line that
    /// names something that is not an interface of that device resolves to
    /// nothing: half a match is not a match.
    pub fn interface_in(&self, device_id: Option<&str>, message: &str) -> Option<(String, String)> {
        let info = self.devices.get(device_id?)?;
        if info.interfaces.is_empty() {
            return None;
        }
        for token in candidates(message) {
            let lowered = token.to_ascii_lowercase();
            if let Some(canonical) = info.interfaces.get(&lowered).or_else(|| {
                info.interfaces
                    .get(&ifname::long(&lowered).to_ascii_lowercase())
            }) {
                return Some((canonical.clone(), token.to_string()));
            }
        }
        None
    }
}

/// The words of a message that could be an interface name. Only the first
/// few dozen: an interface is named at the front of a log line, and a line
/// can be sixty-four kilobytes long.
fn candidates(message: &str) -> impl Iterator<Item = &str> {
    message
        .split(|c: char| !(c.is_ascii_alphanumeric() || "/._-@:".contains(c)))
        .filter(|token| {
            // Something with a digit in it, which every interface name has,
            // and not a bare number, a time or an address.
            token.len() >= 3
                && token
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic())
                && token.chars().any(|c| c.is_ascii_digit())
        })
        .map(|token| token.trim_end_matches(['.', ',', ':', '-']))
        .take(48)
}

/// Build the index if the one held is older than `max_age_ms` or missing.
pub fn refresh(app: &App, now: i64, max_age_ms: i64) -> AppResult<std::sync::Arc<Index>> {
    let state = app.state::<crate::state::LogsState>();
    if let Some(index) = state.index.read().clone()
        && now - index.built_at < max_age_ms
    {
        return Ok(index);
    }
    let index = std::sync::Arc::new(Index::build(app, now)?);
    *state.index.write() = Some(index.clone());
    Ok(index)
}

/// Throw the index away: the inventory or the collected data changed.
pub fn invalidate(app: &App) {
    *app.state::<crate::state::LogsState>().index.write() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interface_candidates_skip_what_cannot_be_an_interface() {
        let found: Vec<&str> = candidates(
            "%LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down at 10.0.0.1",
        )
        .collect();
        assert!(found.contains(&"GigabitEthernet1/0/3"), "{found:?}");
        assert!(!found.contains(&"down"));
        assert!(!found.contains(&"3"));
    }

    #[test]
    fn an_address_with_a_mask_is_still_an_address() {
        assert_eq!(
            host_of("10.0.0.1/24"),
            Some("10.0.0.1".parse::<IpAddr>().unwrap())
        );
        assert_eq!(host_of("switch.example.com"), None);
    }
}
