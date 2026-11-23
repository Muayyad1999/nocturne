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
    /// Per-class quality-of-service statistics.
    Qos,
}

impl Kind {
    pub const ALL: [Kind; 10] = [
        Kind::Facts,
        Kind::Interfaces,
        Kind::Neighbors,
        Kind::Vlans,
        Kind::MacTable,
        Kind::Arp,
        Kind::Routes,
        Kind::Health,
        Kind::Inventory,
        Kind::Qos,
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
            Kind::Qos => "qos",
        }
    }

    /// Kinds that describe the device as a whole: one row, not a table.
    pub fn is_single_row(self) -> bool {
        matches!(self, Kind::Facts | Kind::Health)
    }

    /// A row of this kind with nothing in it. Its keys are the fields a
    /// profile's `map` may fill; a field that is itself an object lists its
    /// own fields (`counters`), or is empty when it takes any key (`extra`).
    pub fn empty_row(self) -> serde_json::Map<String, serde_json::Value> {
        let row = match self {
            Kind::Facts => serde_json::to_value(Facts::default()),
            Kind::Interfaces => serde_json::to_value(Interface::default()),
            Kind::Neighbors => serde_json::to_value(Neighbor::default()),
            Kind::Vlans => serde_json::to_value(Vlan::default()),
            Kind::MacTable => serde_json::to_value(MacEntry::default()),
            Kind::Arp => serde_json::to_value(ArpEntry::default()),
            Kind::Routes => serde_json::to_value(Route::default()),
            Kind::Health => serde_json::to_value(Health::default()),
            Kind::Inventory => serde_json::to_value(InventoryItem::default()),
            Kind::Qos => serde_json::to_value(QosClass::default()),
        };
        match row {
            Ok(serde_json::Value::Object(row)) => row,
            _ => serde_json::Map::new(),
        }
    }

    /// The fields of this kind that name one of the device's own interfaces.
    /// (A neighbour's `remote_interface` names a port of another device.)
    pub fn interface_fields(self) -> &'static [&'static str] {
        match self {
            Kind::Interfaces => &["name"],
            Kind::Neighbors => &["local_interface"],
            Kind::Vlans => &["interfaces"],
            Kind::MacTable | Kind::Arp | Kind::Routes | Kind::Qos => &["interface"],
            Kind::Facts | Kind::Health | Kind::Inventory => &[],
        }
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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

/// Which way the traffic of a QoS class flows, seen from the device.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Arriving on the interface (an input policy; a simple queue's upload).
    // The default only exists so that `QosClass::default()` can list the
    // row's fields; a row read from a device must state its direction.
    #[default]
    In,
    /// Leaving through the interface (an output policy; a queue tree; a
    /// simple queue's download).
    Out,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
        }
    }
}

/// One class of a QoS policy, in one direction, on one interface.
///
/// A policy is a tree: a class may hold a child policy, whose classes name
/// it as their `parent`. A parent's counters include its children's, so add
/// up the rows of one level, never the whole table.
///
/// Every number is optional, and absent means the platform does not print
/// it: an absent drop counter is not "no drops".
// No struct-level `#[serde(default)]`, unlike the other rows: `direction`
// has no sensible default, and a row without one must be refused, not filed
// under `in`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QosClass {
    /// The interface the policy is attached to. Empty when the platform
    /// attaches it to something else (a RouterOS queue on an address).
    #[serde(default, deserialize_with = "lenient::string")]
    pub interface: String,
    #[serde(deserialize_with = "lenient::direction")]
    pub direction: Direction,
    /// The policy the class belongs to: an IOS policy-map; on RouterOS the
    /// queueing structure, `queue simple` or `queue tree`.
    #[serde(default, deserialize_with = "lenient::string")]
    pub policy: String,
    /// The class; on RouterOS the queue. `class-default` is a class like any other.
    #[serde(default, deserialize_with = "lenient::string")]
    pub class: String,
    /// The class, or queue, this one sits under.
    #[serde(default, deserialize_with = "lenient::opt_string")]
    pub parent: Option<String>,
    /// The configured rate: a shaper's, a policer's CIR, a priority rate, a
    /// bandwidth guarantee or a queue's max-limit.
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub rate_bps: Option<u64>,
    /// Classified into this class.
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub packets: Option<u64>,
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub bytes: Option<u64>,
    /// Every drop, whatever the reason.
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub dropped_packets: Option<u64>,
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub dropped_bytes: Option<u64>,
    /// Dropped from the queue (tail drop, WRED): congestion.
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub queue_dropped_packets: Option<u64>,
    /// Dropped by a policer: the policy doing what it was told to.
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub police_dropped_packets: Option<u64>,
    /// Queue depth at the moment of the read.
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub queued_packets: Option<u64>,
    /// Rates as the device averages them itself (IOS: over the load
    /// interval), kept apart from anything computed from the counters.
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub offered_rate_bps: Option<u64>,
    #[serde(default, deserialize_with = "lenient::opt_u64")]
    pub drop_rate_bps: Option<u64>,
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

    /// Text, or `None` when there is none.
    pub fn opt_string<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<String>, D::Error> {
        Ok(Some(string(deserializer)?).filter(|text| !text.is_empty()))
    }

    /// `in` or `out`, also in the words devices use for them. Anything else
    /// is an error: a counter filed under the wrong direction is worse than
    /// a row that is refused.
    pub fn direction<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<super::Direction, D::Error> {
        use serde::de::Error;
        let text = string(deserializer)?.to_ascii_lowercase();
        match text.as_str() {
            "in" | "input" | "ingress" => Ok(super::Direction::In),
            "out" | "output" | "egress" => Ok(super::Direction::Out),
            other => Err(D::Error::custom(format!(
                "direction must be `in` or `out`, not `{other}`"
            ))),
        }
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
            Value::Number(number) => number.as_u64().or_else(|| number.as_f64().and_then(count)),
            Value::String(text) => {
                let digits: String = text
                    .trim()
                    .chars()
                    .filter(|c| *c != ',' && *c != '_')
                    .collect();
                digits
                    .parse::<u64>()
                    .ok()
                    .or_else(|| digits.parse::<f64>().ok().and_then(count))
            }
            _ => None,
        })
    }

    /// A float as a count, where it is one: not negative, and not beyond
    /// what 64 bits hold. A plain cast would turn anything larger into
    /// `u64::MAX`, which is a number nobody printed.
    fn count(number: f64) -> Option<u64> {
        const BEYOND: f64 = 18_446_744_073_709_551_616.0; // 2^64
        (0.0..BEYOND).contains(&number).then_some(number as u64)
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
    fn kinds_describe_their_rows() {
        assert!(Kind::Facts.is_single_row() && Kind::Health.is_single_row());
        assert!(!Kind::Interfaces.is_single_row());
        let interface = Kind::Interfaces.empty_row();
        assert!(interface.contains_key("name") && interface.contains_key("speed_bps"));
        assert!(interface["counters"].get("in_octets").is_some());
        let facts = Kind::Facts.empty_row();
        assert_eq!(facts["extra"], json!({}), "extra takes any key");
        for kind in Kind::ALL {
            let row = kind.empty_row();
            assert!(!row.is_empty(), "{kind}");
            for field in kind.interface_fields() {
                assert!(row.contains_key(*field), "{kind}.{field}");
            }
        }
    }

    #[test]
    fn a_qos_number_the_device_did_not_print_is_absent_not_zero() {
        let class: QosClass = serde_json::from_value(json!({
            "interface": "GigabitEthernet0/1",
            "direction": "out",
            "policy": "WAN-EDGE",
            "class": "VOICE",
            "parent": "class-default",
            "packets": "1,250",
            "dropped_packets": 0,
            "queued_packets": "",
        }))
        .unwrap();
        assert_eq!(class.direction, Direction::Out);
        assert_eq!(class.parent.as_deref(), Some("class-default"));
        assert_eq!(class.packets, Some(1250));
        assert_eq!(class.dropped_packets, Some(0), "a printed zero is a zero");
        assert_eq!(class.queued_packets, None, "an empty value is not a zero");
        assert_eq!(class.bytes, None);
        assert_eq!(class.queue_dropped_packets, None);
        assert_eq!(class.police_dropped_packets, None);
        assert_eq!(class.rate_bps, None);

        // Written out, the absent numbers are null and the direction is a word.
        let written = serde_json::to_value(&class).unwrap();
        assert_eq!(written["direction"], "out");
        assert_eq!(written["bytes"], json!(null));
        assert_eq!(written["dropped_packets"], 0);

        // A top-level class has no parent, however the profile spells "none".
        let top: QosClass =
            serde_json::from_value(json!({"direction": "input", "class": "c", "parent": " "}))
                .unwrap();
        assert_eq!((top.direction, top.parent), (Direction::In, None));
    }

    #[test]
    fn a_qos_row_without_a_direction_is_refused() {
        let missing = serde_json::from_value::<QosClass>(json!({"class": "VOICE"})).unwrap_err();
        assert!(missing.to_string().contains("direction"), "{missing}");
        let unknown =
            serde_json::from_value::<QosClass>(json!({"class": "VOICE", "direction": "both"}))
                .unwrap_err();
        assert!(unknown.to_string().contains("`both`"), "{unknown}");
    }

    #[test]
    fn a_number_beyond_64_bits_is_absent_not_the_largest_number_there_is() {
        let read = |packets: serde_json::Value| {
            serde_json::from_value::<QosClass>(json!({"direction": "out", "packets": packets}))
                .unwrap()
                .packets
        };
        assert_eq!(read(json!("18446744073709551615")), Some(u64::MAX));
        // One more than fits, as text and as the float an expression makes of it.
        assert_eq!(read(json!("18446744073709551616")), None);
        assert_eq!(read(json!(1.8446744073709552e19)), None);
        assert_eq!(read(json!("99999999999999999999999")), None);
        assert_eq!(read(json!(-1)), None);
        // What a float can hold exactly is still read.
        assert_eq!(read(json!(4294967296.0)), Some(4_294_967_296));
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
