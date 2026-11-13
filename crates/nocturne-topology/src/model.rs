//! The graph as the UI and the assistant see it, and this feature's settings.

use serde::{Deserialize, Serialize};

/// Which picture of the network is wanted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    /// Cables: what the neighbour protocols and the lab report.
    #[default]
    Physical,
    /// Addressing: which interfaces share a subnet.
    L3,
}

impl Layer {
    pub fn as_str(self) -> &'static str {
        match self {
            Layer::Physical => "physical",
            Layer::L3 => "l3",
        }
    }
}

/// What a node is. Inferred from the inventory role, then from the
/// capabilities a neighbour protocol reported, then from the platform string.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Router,
    Switch,
    L3Switch,
    Firewall,
    AccessPoint,
    Server,
    Host,
    /// A WAN circuit, the internet, or anything else outside the network.
    Cloud,
    #[default]
    Unknown,
}

impl NodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            NodeKind::Router => "router",
            NodeKind::Switch => "switch",
            NodeKind::L3Switch => "l3_switch",
            NodeKind::Firewall => "firewall",
            NodeKind::AccessPoint => "access_point",
            NodeKind::Server => "server",
            NodeKind::Host => "host",
            NodeKind::Cloud => "cloud",
            NodeKind::Unknown => "unknown",
        }
    }
}

/// How a link is known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Lldp,
    Cdp,
    Mndp,
    /// Two interfaces in one subnet.
    Subnet,
    /// The lab feature says so.
    Lab,
    /// A person drew it.
    Manual,
    /// A MAC-table sighting on an access port.
    Mac,
    #[default]
    Unknown,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Lldp => "lldp",
            LinkKind::Cdp => "cdp",
            LinkKind::Mndp => "mndp",
            LinkKind::Subnet => "subnet",
            LinkKind::Lab => "lab",
            LinkKind::Manual => "manual",
            LinkKind::Mac => "mac",
            LinkKind::Unknown => "unknown",
        }
    }

    /// The neighbour protocol a collected row names, if it is one.
    pub fn from_protocol(protocol: &str) -> Option<LinkKind> {
        match protocol.trim().to_ascii_lowercase().as_str() {
            "lldp" => Some(LinkKind::Lldp),
            "cdp" => Some(LinkKind::Cdp),
            "mndp" | "mikrotik" | "ndp" => Some(LinkKind::Mndp),
            _ => None,
        }
    }
}

/// How we came to believe a link exists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    /// Both ends reported it (or the lab, which owns its own wiring).
    Confirmed,
    /// Only one end did: the other is unmanaged, silent, or was never collected.
    OneSided,
    /// Derived from addresses or MAC tables, not reported as a neighbour.
    #[default]
    Inferred,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::Confirmed => "confirmed",
            Confidence::OneSided => "one_sided",
            Confidence::Inferred => "inferred",
        }
    }
}

/// Up, down, or nobody has looked.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Up,
    Down,
    #[default]
    Unknown,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Up => "up",
            Status::Down => "down",
            Status::Unknown => "unknown",
        }
    }

    pub fn from_flag(flag: Option<bool>) -> Status {
        match flag {
            Some(true) => Status::Up,
            Some(false) => Status::Down,
            None => Status::Unknown,
        }
    }
}

/// One end of a link.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct End {
    pub node: String,
    /// Canonical interface name (`GigabitEthernet1/0/1`), when it is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
}

impl End {
    pub fn new(node: impl Into<String>) -> End {
        End {
            node: node.into(),
            interface: None,
            ip: None,
        }
    }

    pub fn with_interface(node: impl Into<String>, interface: Option<String>) -> End {
        End {
            node: node.into(),
            interface: interface.filter(|name| !name.is_empty()),
            ip: None,
        }
    }
}

/// A device, a neighbour we have never logged in to, or a host we found in a
/// MAC table.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    /// Absent for unmanaged neighbours, endpoints and manual-only nodes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub lab: bool,
    pub status: Status,
    /// Seen by a neighbour protocol (or named in a manual link) but not in the inventory.
    pub unmanaged: bool,
    pub x: f64,
    pub y: f64,
    pub pinned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alerts: Option<u32>,
    /// When the data this node was built from was collected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collected_at: Option<i64>,
    /// The device's data is older than the staleness setting.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

/// A connection between two nodes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Link {
    pub id: String,
    pub a: End,
    pub b: End,
    pub kind: LinkKind,
    pub confidence: Confidence,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed_bps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub util_pct: Option<f64>,
    /// VLANs carried, as a compressed list (`1,10-12,99`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlans: Option<String>,
    /// Port-channel this link stands for, with its member count.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Every protocol that reported it (`lldp`, `cdp`), in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<String>,
    /// Member interface pairs when this link is a folded port-channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
}

impl Link {
    /// A stable identity for the link, independent of which end came first.
    pub fn signature(&self) -> String {
        let one = format!(
            "{}|{}",
            self.a.node,
            self.a.interface.as_deref().unwrap_or("")
        );
        let two = format!(
            "{}|{}",
            self.b.node,
            self.b.interface.as_deref().unwrap_or("")
        );
        let (first, second) = if one <= two { (one, two) } else { (two, one) };
        format!("{}::{first}::{second}", self.kind.as_str())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroupKind {
    #[default]
    Site,
    Subnet,
    Lab,
}

impl GroupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GroupKind::Site => "site",
            GroupKind::Subnet => "subnet",
            GroupKind::Lab => "lab",
        }
    }
}

/// Nodes that belong together: a site, a subnet, a lab.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub kind: GroupKind,
    pub label: String,
    pub nodes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub devices: usize,
    pub links: usize,
    pub unmanaged: usize,
    pub stale_devices: usize,
    pub endpoints: usize,
}

/// How old the data the graph was drawn from is.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DataAge {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oldest_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub newest_at: Option<i64>,
}

/// The network, drawn.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    pub groups: Vec<Group>,
    /// What could not be worked out, in sentences meant for a person.
    pub warnings: Vec<String>,
    pub stats: Stats,
    pub generated_at: i64,
    pub data_age: DataAge,
    pub layer: Layer,
    pub view: String,
}

impl Graph {
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn name_of(&self, id: &str) -> String {
        self.node(id)
            .map(|node| node.name.clone())
            .unwrap_or_else(|| id.to_string())
    }
}

/// Which devices a view or an export covers.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Filter {
    pub site: Option<String>,
    pub tag: Option<String>,
    pub role: Option<String>,
    pub lab_id: Option<String>,
    pub device_ids: Vec<String>,
}

impl Filter {
    pub fn is_empty(&self) -> bool {
        self == &Filter::default()
    }

    pub fn matches(&self, device: &nocturne_app::model::Device) -> bool {
        if let Some(site) = &self.site
            && !device.site.eq_ignore_ascii_case(site)
        {
            return false;
        }
        if let Some(role) = &self.role
            && !device.role.eq_ignore_ascii_case(role)
        {
            return false;
        }
        if let Some(tag) = &self.tag
            && !device
                .tags
                .iter()
                .any(|known| known.eq_ignore_ascii_case(tag))
        {
            return false;
        }
        if let Some(lab_id) = &self.lab_id
            && device.lab.as_ref().map(|lab| lab.lab_id.as_str()) != Some(lab_id.as_str())
        {
            return false;
        }
        if !self.device_ids.is_empty() && !self.device_ids.contains(&device.id) {
            return false;
        }
        true
    }
}

/// A saved picture: which layer, which devices, and whether hosts are drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    pub id: String,
    pub name: String,
    pub layer: Layer,
    pub filter: Filter,
    pub include_endpoints: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for View {
    fn default() -> View {
        View {
            id: DEFAULT_VIEW.to_string(),
            name: "Default".to_string(),
            layer: Layer::Physical,
            filter: Filter::default(),
            include_endpoints: false,
            created_at: 0,
            updated_at: 0,
        }
    }
}

/// The view every client gets when it asks for none.
pub const DEFAULT_VIEW: &str = "default";

/// A node's place in a view.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Position {
    pub x: f64,
    pub y: f64,
    /// A person put it here: no automatic layout may move it.
    pub pinned: bool,
}

/// Which way the nodes are arranged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    /// Ranks by role (core at the top), barycentre ordering, sites as columns.
    #[default]
    Layered,
    /// Spring embedder, for meshes with no hierarchy.
    Force,
    /// Rings around a root.
    Radial,
}

impl Algorithm {
    pub fn as_str(self) -> &'static str {
        match self {
            Algorithm::Layered => "layered",
            Algorithm::Force => "force",
            Algorithm::Radial => "radial",
        }
    }
}

/// `topology` settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Neighbour data older than this contributes links, but the device is
    /// counted in `stale_devices` and named in `warnings`.
    pub stale_after_hours: u32,
    /// A port with more MAC addresses than this is not an access port.
    pub endpoint_max_macs: u32,
    /// Fold the members of a port-channel into one link when both ends agree.
    pub fold_port_channels: bool,
    /// How many physical-graph snapshots `topology.changes` keeps.
    pub max_snapshots: u32,
    /// Default arrangement for `topology.autolayout`.
    pub default_algorithm: Algorithm,
    /// Ask the lab feature for the wiring of lab devices (`labs.links`).
    pub use_lab_links: bool,
    /// Ask the monitor feature for link utilisation (`monitor.interfaces`).
    pub use_monitor_utilisation: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            stale_after_hours: 24,
            endpoint_max_macs: 3,
            fold_port_channels: true,
            max_snapshots: 200,
            default_algorithm: Algorithm::Layered,
            use_lab_links: true,
            use_monitor_utilisation: true,
        }
    }
}

pub const SETTINGS_SECTION: &str = "topology";

/// Compress `[1, 2, 3, 7, 8, 20]` to `1-3,7-8,20`.
pub fn compress_ranges(values: &[u64]) -> String {
    let mut sorted: Vec<u64> = values.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mut parts: Vec<String> = Vec::new();
    let mut index = 0;
    while index < sorted.len() {
        let start = sorted[index];
        let mut end = start;
        while index + 1 < sorted.len() && sorted[index + 1] == end + 1 {
            index += 1;
            end = sorted[index];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        index += 1;
    }
    parts.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_compress() {
        assert_eq!(compress_ranges(&[1, 2, 3, 7, 8, 20]), "1-3,7-8,20");
        assert_eq!(compress_ranges(&[5]), "5");
        assert_eq!(compress_ranges(&[]), "");
        assert_eq!(compress_ranges(&[3, 1, 2, 2]), "1-3");
    }

    #[test]
    fn link_signature_does_not_depend_on_end_order() {
        let one = Link {
            a: End::with_interface("a", Some("Gi0/1".into())),
            b: End::with_interface("b", Some("Gi0/2".into())),
            kind: LinkKind::Lldp,
            ..Link::default()
        };
        let other = Link {
            a: one.b.clone(),
            b: one.a.clone(),
            ..one.clone()
        };
        assert_eq!(one.signature(), other.signature());
    }

    #[test]
    fn protocols_map_to_link_kinds() {
        assert_eq!(LinkKind::from_protocol(" LLDP "), Some(LinkKind::Lldp));
        assert_eq!(LinkKind::from_protocol("cdp"), Some(LinkKind::Cdp));
        assert_eq!(LinkKind::from_protocol("mndp"), Some(LinkKind::Mndp));
        assert_eq!(LinkKind::from_protocol("foaf"), None);
    }
}
