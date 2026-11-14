//! The types the screens and the assistant are written against.
//!
//! The one distinction this feature exists for runs through every type here:
//!
//! * A **record** is what a person wrote down: a [`Prefix`], an
//!   [`AddressRecord`], a [`VlanRecord`]. It is only ever changed by somebody
//!   asking for it to be changed.
//! * An **observation** is what the network said: an [`Observation`], built
//!   from collected interface addresses and ARP tables. It always names the
//!   device, the interface and the moment it was collected, and nothing in
//!   this crate ever turns one into a record on its own.
//!
//! [`Address`] is the two put side by side for display — `documented` and
//! `observed` say which halves are present, and `observations` is the
//! evidence, so no screen has to guess whether a row is a plan or a fact.

use serde::{Deserialize, Serialize};

// ----------------------------------------------------------------- settings

/// Settings section `ipam`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// An ARP sighting older than this stops counting as occupancy and is
    /// forgotten at the next sync: the address becomes "not seen" again.
    pub arp_stale_days: u32,
    /// Re-read collected data when `collect.updated` arrives (debounced).
    pub sync_on_collect: bool,
    /// How long after the last collection the automatic sync waits for the
    /// dust to settle.
    pub sync_delay_ms: u32,
    /// The largest block `ipam.prefix.get` enumerates address by address (the
    /// classic grid). A larger prefix lists only the addresses it knows of.
    pub grid_max_addresses: u32,
    /// Recognise the well-known HSRP/VRRP virtual MAC ranges, so a shared
    /// virtual address is not reported as a duplicate.
    pub recognise_virtual_macs: bool,
    /// Two different MACs for one address within this window are a conflict;
    /// further apart, it is a lease that moved on.
    pub mac_change_window_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            arp_stale_days: 30,
            sync_on_collect: true,
            sync_delay_ms: 1_500,
            grid_max_addresses: 4096,
            recognise_virtual_macs: true,
            mac_change_window_minutes: 60,
        }
    }
}

// -------------------------------------------------------------------- enums

/// What a prefix is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Lan,
    Wan,
    Loopback,
    Management,
    P2p,
    Dmz,
    Guest,
    Voice,
    Other,
}

/// Where a *record* came from. `discovered` means nobody has confirmed it: it
/// exists because an address was seen inside it, which is not the same as
/// somebody having planned it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    #[default]
    Manual,
    Discovered,
}

/// What filled in an [`Address`] row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressSource {
    /// A record a person wrote.
    Manual,
    /// Configured on a device's interface.
    Interface,
    /// Answered an ARP request.
    Arp,
    /// Reserved by the contract for a name-service feeder; nothing in
    /// Nocturne produces one yet.
    Dns,
}

/// The state of one address.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// In use: documented as in use, or seen on the network.
    #[default]
    Active,
    /// Held back on purpose.
    Reserved,
    /// Inside the prefix's DHCP range.
    Dhcp,
    /// **Not documented and not seen.** It is not a promise that the address
    /// is available — only that Nocturne has no record of it and nothing has
    /// ever answered on it. What was collected, and when, is in the
    /// reconciliation report's coverage.
    Free,
    /// Two devices, or a record and the network, disagree about it.
    Conflict,
}

/// Which table an observation came out of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    /// An address configured on an interface of the device.
    Interface,
    /// An address that answered on the device's network.
    Arp,
}

impl ObservationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ObservationKind::Interface => "interface",
            ObservationKind::Arp => "arp",
        }
    }

    pub fn parse(text: &str) -> Option<ObservationKind> {
        match text {
            "interface" => Some(ObservationKind::Interface),
            "arp" => Some(ObservationKind::Arp),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ records

/// A DHCP range inside a prefix.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DhcpRange {
    pub start: String,
    pub end: String,
}

/// A block of addresses somebody is keeping track of.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefix {
    pub id: String,
    /// Canonical `network/length`, IPv4 or IPv6.
    pub cidr: String,
    pub name: String,
    pub description: String,
    /// The site this block belongs to. A lab's blocks live in their own
    /// namespace, `lab:<lab id>`, and never mix with production utilisation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlan_id: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dhcp: Option<DhcpRange>,
    pub tags: Vec<String>,
    pub source: Source,
    /// For a `discovered` prefix: the interfaces it was inferred from. This is
    /// evidence, not a record — a person editing the prefix makes it `manual`
    /// and this stays as the note of where it came from.
    pub discovered_from: Vec<Sighting>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A device and interface that showed something, with the time it was read.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sighting {
    pub device_id: String,
    pub device: String,
    pub interface: String,
    /// The address, as configured (`10.1.10.1/24`).
    pub cidr: String,
    pub collected_at: i64,
}

/// An address a person wrote down. Nothing collected ever writes one of
/// these: that is what keeps a plan a plan.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AddressRecord {
    /// Canonical address text.
    pub ip: String,
    /// Production (`""`) or a lab (`lab:<lab id>`).
    pub scope: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_id: Option<String>,
    /// `active`, `reserved` or `dhcp` — a record cannot be `free` (there
    /// would be nothing to record) and cannot be `conflict` (that is a
    /// finding, not something anybody documents).
    pub status: Status,
    pub hostname: String,
    pub description: String,
    /// Whom the person says the address belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A VLAN a person wrote down.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VlanRecord {
    pub id: String,
    pub vid: u16,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    pub description: String,
    pub prefix_ids: Vec<String>,
    pub source: Source,
    pub created_at: i64,
    pub updated_at: i64,
}

// ------------------------------------------------------------- observations

/// One sighting of one address: which device, which interface, when it was
/// collected. Never merged with a record, never promoted to one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Observation {
    pub ip: String,
    pub kind: ObservationKind,
    pub device_id: String,
    /// The device's name at the time of the sync, for display.
    pub device: String,
    pub interface: String,
    /// The mask the interface carried; `None` for an ARP sighting, which says
    /// nothing about the size of the subnet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_len: Option<u8>,
    /// The address exactly as configured, `10.1.10.1/24`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cidr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<String>,
    /// The site of the device that saw it (`lab:<id>` for a lab device).
    pub scope: String,
    pub first_seen: i64,
    pub last_seen: i64,
    /// When the data behind this sighting was read from the device.
    pub collected_at: i64,
    /// `cli`, `lab`, `snmp`: how the collection was made.
    pub collector: String,
    /// The sighting is still in the device's latest table.
    pub current: bool,
}

impl Observation {
    /// An ARP sighting that is no longer in the device's table but has not
    /// yet aged out. An interface address is never stale: it is configured or
    /// it is gone.
    pub fn is_stale(&self) -> bool {
        !self.current && self.kind == ObservationKind::Arp
    }
}

// ------------------------------------------------------------------ results

/// A prefix with everything computed from what is inside it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrefixRow {
    #[serde(flatten)]
    pub prefix: Prefix,
    pub parent_id: Option<String>,
    pub depth: u32,
    /// Direct children.
    pub children: u32,
    /// Addresses a host can have (IPv4 minus network and broadcast above a
    /// `/31`; IPv6 minus the Subnet-Router anycast address).
    pub usable: u64,
    /// The same number, exactly, for IPv6 blocks that outgrow 64 bits.
    pub usable_text: String,
    /// Addresses in use: documented as in use, or seen on the network.
    pub used: u32,
    pub reserved: u32,
    /// `usable - used - reserved`, saturated like `usable`.
    pub free: u64,
    /// Percentage used, `0.0` when `util_known` is false.
    pub util_pct: f32,
    /// False when the block is too large for a percentage to mean anything
    /// (IPv6 `/64` and larger): the number is then not unknown-as-zero, it is
    /// simply not a thing that can be said.
    pub util_known: bool,
    /// How many of `used` have a record.
    pub documented: u32,
    /// How many of `used` were seen on the network.
    pub observed: u32,
    /// Share of the block covered by child prefixes.
    pub child_coverage_pct: f32,
    /// Interfaces configured inside this block, newest collection first.
    pub devices: Vec<PrefixDevice>,
}

/// An interface configured inside a prefix — an observation, so it carries
/// when it was collected.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PrefixDevice {
    pub device_id: String,
    pub device: String,
    pub interface: String,
    pub ip: String,
    /// The mask the interface carried, which is not always the prefix's own.
    pub prefix_len: Option<u8>,
    pub collected_at: i64,
    pub lab: bool,
}

/// One address, with the record and the evidence kept apart.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Address {
    pub ip: String,
    pub prefix_id: Option<String>,
    pub status: Status,
    pub hostname: Option<String>,
    pub description: Option<String>,
    /// From the record when there is one, otherwise from the strongest
    /// observation; `documented` says which.
    pub device_id: Option<String>,
    pub interface: Option<String>,
    pub mac: Option<String>,
    pub vendor: Option<String>,
    pub source: AddressSource,
    /// When the address was first seen on the network, or the record was
    /// created. `None` when it has never been seen and never recorded.
    pub first_seen: Option<i64>,
    pub last_seen: Option<i64>,
    /// When the record was last written, if there is one. Never a sighting.
    pub documented_at: Option<i64>,
    pub tags: Vec<String>,
    /// A person wrote a record for this address.
    pub documented: bool,
    /// The network has shown this address at least once.
    pub observed: bool,
    /// Every sighting, with device, interface and collection time.
    pub observations: Vec<Observation>,
    /// Where the record and the network say different things. Reported, never
    /// resolved on its own.
    pub disagreements: Vec<Disagreement>,
}

/// A record and the network disagreeing about one field of one address.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Disagreement {
    /// `device`, `interface` or `mac`.
    pub field: String,
    /// What the record says, and what the network showed — in the terms of
    /// `field`.
    ///
    /// For `interface` and `mac` these are the values themselves. **For
    /// `device` they are inventory ids**, not names: they are produced by
    /// `addresses::compose`, which is a pure function of a record and its
    /// sightings and has no inventory to look a name up in. That is worth
    /// keeping — it is why composing is cheap and testable — but it does mean
    /// a caller that shows a `device` disagreement must resolve both sides
    /// itself. [`Self::device`] below is already a name, which makes the
    /// difference easy to miss.
    pub documented: String,
    pub observed: String,
    pub device_id: String,
    pub device: String,
    pub interface: String,
    pub collected_at: i64,
}

/// A run of addresses inside a prefix with neither a record nor a sighting.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FreeRange {
    pub start: String,
    pub end: String,
    /// Addresses in the run, saturated at `u64::MAX` for IPv6.
    pub count: u64,
    /// Exact count, as decimal text.
    pub count_text: String,
}

/// A VLAN record, the collected VLANs that match it, or both.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VlanRow {
    #[serde(flatten)]
    pub vlan: VlanRecord,
    /// Where it exists on the network, with the name each device gives it.
    pub devices: Vec<VlanDevice>,
    /// The prefixes tied to it: `prefix_ids` plus any prefix whose
    /// `vlan_id` is this VLAN.
    pub prefixes: Vec<VlanPrefix>,
    pub documented: bool,
    pub observed: bool,
}

/// A VLAN as one device reports it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VlanDevice {
    pub device_id: String,
    pub device: String,
    /// The name on that device, which is not always the documented one.
    pub name: String,
    pub status: String,
    pub ports: u32,
    pub collected_at: i64,
    pub lab: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VlanPrefix {
    pub id: String,
    pub cidr: String,
}

// ---------------------------------------------------------------- conflicts

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    /// One address on two devices, or two MACs on one address at once.
    DuplicateIp,
    /// Two prefixes that overlap without one containing the other.
    Overlap,
    /// A documented address that is not inside the prefix it claims.
    OutsideAnyPrefix,
    /// The gateway a prefix documents is not the one configured in it.
    GatewayMismatch,
}

/// Something that cannot be true at the same time. A conflict is a finding:
/// it is reported with everything it was decided from and never fixed by
/// guessing which side is right.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conflict {
    pub kind: ConflictKind,
    /// The address, the prefix or the VLAN the conflict is about.
    pub subject: String,
    /// One sentence a person can act on.
    pub detail: String,
    pub involved: Vec<Involved>,
}

/// One side of a conflict, and whether it is a record or a sighting.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Involved {
    /// `record` or `observation`.
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cidr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collected_at: Option<i64>,
}

impl Involved {
    pub fn record() -> Involved {
        Involved {
            kind: "record".to_string(),
            ..Involved::default()
        }
    }

    pub fn observation(observation: &Observation) -> Involved {
        Involved {
            kind: "observation".to_string(),
            device_id: Some(observation.device_id.clone()),
            device: Some(observation.device.clone()),
            interface: Some(observation.interface.clone()),
            ip: Some(observation.ip.clone()),
            cidr: observation.cidr.clone(),
            mac: observation.mac.clone(),
            collected_at: Some(observation.collected_at),
            ..Involved::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_record_and_an_observation_never_share_a_shape() {
        // The wire forms are different types with different fields, which is
        // what stops a screen from showing one as the other.
        let observation = Observation {
            ip: "10.1.10.5".into(),
            kind: ObservationKind::Arp,
            device_id: "d1".into(),
            device: "SW1".into(),
            interface: "Vlan10".into(),
            prefix_len: None,
            cidr: None,
            mac: Some("00:11:22:33:44:55".into()),
            vendor: None,
            scope: String::new(),
            first_seen: 10,
            last_seen: 20,
            collected_at: 20,
            collector: "cli".into(),
            current: false,
        };
        let value = serde_json::to_value(&observation).unwrap();
        assert_eq!(value["kind"], "arp");
        assert!(
            value.get("status").is_none(),
            "an observation has no status to state"
        );
        assert!(
            observation.is_stale(),
            "an ARP sighting that left the table is stale"
        );
        assert!(
            !Observation {
                kind: ObservationKind::Interface,
                ..observation.clone()
            }
            .is_stale(),
            "an interface address is configured or gone, never stale"
        );

        let record = AddressRecord {
            ip: "10.1.10.5".into(),
            status: Status::Reserved,
            ..AddressRecord::default()
        };
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["status"], "reserved");
        assert!(
            value.get("collected_at").is_none(),
            "a record was not collected"
        );
        assert_eq!(serde_json::to_value(Status::Free).unwrap(), json!("free"));
    }

    #[test]
    fn settings_round_trip_with_their_defaults() {
        let value = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(value["arp_stale_days"], 30);
        let patched: Settings =
            serde_json::from_value(json!({"arp_stale_days": 7})).expect("partial settings");
        assert_eq!(patched.arp_stale_days, 7);
        assert!(patched.sync_on_collect, "the rest keep their defaults");
    }
}
