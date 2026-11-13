//! The vendor-neutral data model every collector produces.
//!
//! Profiles map each platform's parsed output into these shapes, so the rest
//! of Nocturne (topology, monitoring, the AI's tools, the UI) never looks at
//! vendor-specific fields. Deserialisation is deliberately lenient — numbers
//! may arrive as strings, booleans as "up"/"yes" — because the values come
//! out of text parsers.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// The kinds of structured data a profile can collect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Facts,
    Interfaces,
    Neighbors,
    Vlans,
    MacTable,
    Arp,
    Routes,
    Health,
    Inventory,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Facts,
        Kind::Interfaces,
        Kind::Neighbors,
        Kind::Vlans,
        Kind::MacTable,
        Kind::Arp,
        Kind::Routes,
        Kind::Health,
        Kind::Inventory,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Facts => "facts",
            Kind::Interfaces => "interfaces",
            Kind::Neighbors => "neighbors",
            Kind::Vlans => "vlans",
            Kind::MacTable => "mac_table",
            Kind::Arp => "arp",
            Kind::Routes => "routes",
            Kind::Health => "health",
            Kind::Inventory => "inventory",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Facts {
    #[serde(deserialize_with = "lenient::string")]
    pub hostname: String,
    #[serde(deserialize_with = "lenient::string")]
    pub vendor: String,
    #[serde(deserialize_with = "lenient::string")]
    pub model: String,
    /// Operating system family, e.g. "IOS-XE", "RouterOS", "Junos".
    #[serde(deserialize_with = "lenient::string")]
    pub os: String,
    #[serde(deserialize_with = "lenient::string")]
    pub os_version: String,
    #[serde(deserialize_with = "lenient::string")]
    pub serial: String,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub uptime_s: Option<u64>,
    /// Anything else worth keeping (image file, config register, stack members…).
    pub extra: IndexMap<String, String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Interface {
    #[serde(deserialize_with = "lenient::string")]
    pub name: String,
    #[serde(deserialize_with = "lenient::string")]
    pub description: String,
    #[serde(deserialize_with = "lenient::opt_bool")]
    pub admin_up: Option<bool>,
    #[serde(deserialize_with = "lenient::opt_bool")]
    pub oper_up: Option<bool>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub speed_bps: Option<u64>,
    #[serde(deserialize_with = "lenient::string")]
    pub duplex: String,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub mtu: Option<u64>,
    #[serde(deserialize_with = "lenient::string")]
    pub mac: String,
    /// Addresses in CIDR form, e.g. "10.0.0.1/24".
    #[serde(deserialize_with = "lenient::strings")]
    pub ipv4: Vec<String>,
    #[serde(deserialize_with = "lenient::strings")]
    pub ipv6: Vec<String>,
    /// "access", "trunk", "routed" or empty when unknown.
    #[serde(deserialize_with = "lenient::string")]
    pub mode: String,
    /// Access VLAN (or native VLAN on a trunk).
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub vlan: Option<u64>,
    /// Hardware or media type as the device reports it.
    #[serde(deserialize_with = "lenient::string")]
    pub kind: String,
    pub counters: Counters,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Counters {
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub in_octets: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub out_octets: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub in_packets: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub out_packets: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub in_errors: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub out_errors: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub in_discards: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub out_discards: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub crc: Option<u64>,
    /// Rates as the device computes them (often a 5-minute average).
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub in_rate_bps: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub out_rate_bps: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Neighbor {
    #[serde(deserialize_with = "lenient::string")]
    pub local_interface: String,
    #[serde(deserialize_with = "lenient::string")]
    pub device: String,
    #[serde(deserialize_with = "lenient::string")]
    pub remote_interface: String,
    #[serde(deserialize_with = "lenient::string")]
    pub platform: String,
    #[serde(deserialize_with = "lenient::string")]
    pub address: String,
    /// "cdp", "lldp", "mndp", …
    #[serde(deserialize_with = "lenient::string")]
    pub protocol: String,
    #[serde(deserialize_with = "lenient::string")]
    pub capabilities: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vlan {
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub id: Option<u64>,
    #[serde(deserialize_with = "lenient::string")]
    pub name: String,
    #[serde(deserialize_with = "lenient::string")]
    pub status: String,
    #[serde(deserialize_with = "lenient::strings")]
    pub interfaces: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MacEntry {
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub vlan: Option<u64>,
    #[serde(deserialize_with = "lenient::string")]
    pub mac: String,
    #[serde(deserialize_with = "lenient::string")]
    pub interface: String,
    /// "dynamic", "static", …
    #[serde(deserialize_with = "lenient::string")]
    pub kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArpEntry {
    #[serde(deserialize_with = "lenient::string")]
    pub ip: String,
    #[serde(deserialize_with = "lenient::string")]
    pub mac: String,
    #[serde(deserialize_with = "lenient::string")]
    pub interface: String,
    #[serde(deserialize_with = "lenient::string")]
    pub age: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Route {
    /// Destination in CIDR form.
    #[serde(deserialize_with = "lenient::string")]
    pub prefix: String,
    /// "connected", "static", "ospf", "bgp", … as the device labels it.
    #[serde(deserialize_with = "lenient::string")]
    pub protocol: String,
    #[serde(deserialize_with = "lenient::string")]
    pub next_hop: String,
    #[serde(deserialize_with = "lenient::string")]
    pub interface: String,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub distance: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub metric: Option<u64>,
    #[serde(deserialize_with = "lenient::string")]
    pub vrf: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Health {
    #[serde(deserialize_with = "lenient::opt_f64")]
    pub cpu_pct: Option<f64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub memory_used: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub memory_total: Option<u64>,
    #[serde(deserialize_with = "lenient::opt_f64")]
    pub temperature_c: Option<f64>,
    #[serde(deserialize_with = "lenient::opt_u64")]
    pub uptime_s: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct InventoryItem {
    #[serde(deserialize_with = "lenient::string")]
    pub name: String,
    #[serde(deserialize_with = "lenient::string")]
    pub description: String,
    #[serde(deserialize_with = "lenient::string")]
    pub part_id: String,
    #[serde(deserialize_with = "lenient::string")]
    pub serial: String,
}

/// Forgiving deserialisers for values that came out of text.
pub(crate) mod lenient {
    use serde::{Deserialize, Deserializer};
    use serde_json::Value;

    pub fn string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
        Ok(match Value::deserialize(deserializer)? {
            Value::String(text) => text.trim().to_string(),
            Value::Null => String::new(),
            Value::Array(items) => items
                .iter()
                .map(plain)
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
                .join(", "),
            other => plain(&other),
        })
    }

    pub fn strings<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
        Ok(match Value::deserialize(deserializer)? {
            Value::Array(items) => items
                .iter()
                .map(plain)
                .filter(|item| !item.is_empty())
                .collect(),
            Value::Null => Vec::new(),
            other => {
                let text = plain(&other);
                if text.is_empty() {
                    Vec::new()
                } else {
                    vec![text]
                }
            }
        })
    }

    pub fn opt_u64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
        Ok(match Value::deserialize(deserializer)? {
            Value::Number(number) => number
                .as_u64()
                .or_else(|| number.as_f64().filter(|n| *n >= 0.0).map(|n| n as u64)),
            Value::String(text) => {
                let digits: String = text
                    .trim()
                    .chars()
                    .filter(|c| *c != ',' && *c != '_')
                    .collect();
                digits.parse::<u64>().ok().or_else(|| {
                    digits
                        .parse::<f64>()
                        .ok()
                        .filter(|n| *n >= 0.0)
                        .map(|n| n as u64)
                })
            }
            _ => None,
        })
    }

    pub fn opt_f64<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<f64>, D::Error> {
        Ok(match Value::deserialize(deserializer)? {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.trim().trim_end_matches('%').trim().parse::<f64>().ok(),
            _ => None,
        })
    }

    pub fn opt_bool<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<bool>, D::Error> {
        Ok(match Value::deserialize(deserializer)? {
            Value::Bool(flag) => Some(flag),
            Value::Number(number) => number.as_i64().map(|n| n != 0),
            Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "up" | "on" | "enabled" | "1" | "connected" | "running" => {
                    Some(true)
                }
                "false" | "no" | "down" | "off" | "disabled" | "0" | "notconnect" => Some(false),
                _ => None,
            },
            _ => None,
        })
    }

    fn plain(value: &Value) -> String {
        match value {
            Value::String(text) => text.trim().to_string(),
            Value::Null => String::new(),
            other => other.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn interface_accepts_values_as_text() {
        let interface: Interface = serde_json::from_value(json!({
            "name": " GigabitEthernet0/1 ",
            "admin_up": "up",
            "oper_up": false,
            "speed_bps": "1,000,000,000",
            "mtu": 1500,
            "ipv4": "10.0.0.1/24",
            "ipv6": [],
            "vlan": "30",
            "counters": {"in_errors": "12", "crc": null, "in_octets": 1.0e9}
        }))
        .unwrap();
        assert_eq!(interface.name, "GigabitEthernet0/1");
        assert_eq!(
            (interface.admin_up, interface.oper_up),
            (Some(true), Some(false))
        );
        assert_eq!(interface.speed_bps, Some(1_000_000_000));
        assert_eq!(interface.ipv4, vec!["10.0.0.1/24"]);
        assert_eq!(interface.vlan, Some(30));
        assert_eq!(interface.counters.in_errors, Some(12));
        assert_eq!(interface.counters.crc, None);
        assert_eq!(interface.counters.in_octets, Some(1_000_000_000));
    }

    #[test]
    fn unknown_text_becomes_none_not_an_error() {
        let health: Health = serde_json::from_value(
            json!({"cpu_pct": "17%", "temperature_c": "n/a", "uptime_s": ""}),
        )
        .unwrap();
        assert_eq!(health.cpu_pct, Some(17.0));
        assert_eq!(health.temperature_c, None);
        assert_eq!(health.uptime_s, None);
        let facts: Facts = serde_json::from_value(
            json!({"model": ["WS-C3750X-48P", "WS-C3750X-24"], "serial": null}),
        )
        .unwrap();
        assert_eq!(facts.model, "WS-C3750X-48P, WS-C3750X-24");
        assert_eq!(facts.serial, "");
    }
}
