//! What comes out: a lab that boots, a plan that reads, and the reasoning.

use nocturne_netsim::LabSpec;
use serde::{Deserialize, Serialize};

use crate::requirements::Purpose;

/// A complete design.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Design {
    /// Every device with its full configuration, cabled up and laid out.
    /// Load it with `nocturne_netsim::Lab::from_spec` and it comes up.
    pub lab: LabSpec,
    pub plan: Plan,
    /// What was chosen and why, in the order the choices were made.
    pub decisions: Vec<Decision>,
    /// Requirements that could not be met exactly, and what was done instead.
    pub warnings: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Plan {
    pub sites: Vec<SitePlan>,
    pub vlans: Vec<VlanPlan>,
    pub links: Vec<LinkPlan>,
    pub devices: Vec<DevicePlan>,
    pub bill_of_materials: Vec<BomLine>,
    /// What a correct build must satisfy.
    pub checks: Vec<Check>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct SitePlan {
    /// The name as it was asked for.
    pub name: String,
    /// The short tag used in device names and descriptions.
    pub tag: String,
    /// The site's whole address block.
    pub block: String,
    pub users: u32,
    /// Device names at this site, in the order they are introduced.
    pub devices: Vec<String>,
    /// VLAN ids at this site.
    pub vlans: Vec<u16>,
    /// `true` when this site holds the Internet connection the others use.
    pub is_hub: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct VlanPlan {
    pub site: String,
    pub id: u16,
    pub name: String,
    pub purpose: Purpose,
    /// `"10.20.0.0/24"`.
    pub subnet: String,
    /// The address hosts use as their default gateway.
    pub gateway: String,
    /// First address handed out by DHCP; empty when the VLAN has no pool.
    pub dhcp_first: String,
    pub dhcp_last: String,
    /// Hosts asked for, growth head-room included.
    pub hosts_needed: u32,
    /// Hosts the subnet can hold.
    pub hosts_available: u32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Access switch to its core or distribution switch.
    #[default]
    AccessUplink,
    /// Distribution switch to the core.
    DistributionUplink,
    /// Between the two cores of a site.
    CoreInterlink,
    /// Core to the site's edge router (routed).
    EdgeUplink,
    /// Site to site (routed).
    Wan,
    /// Edge router to the Internet (routed).
    Internet,
    /// A server, printer, telephone, access point or test host.
    Host,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct LinkPlan {
    pub a: String,
    pub a_if: String,
    pub b: String,
    pub b_if: String,
    pub kind: LinkKind,
    /// The subnet on a routed link; empty on a switched one.
    pub subnet: String,
    /// VLANs carried on a trunk, `"access 10"` on an access port.
    pub carries: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum DeviceRole {
    /// Layer-3 switch holding the gateways for the site.
    #[default]
    Core,
    /// Layer-2 aggregation between the core and a large access layer.
    Distribution,
    /// Layer-2 switch the users plug into.
    Access,
    /// Router facing the Internet and the other sites.
    Edge,
    /// Wireless access point.
    AccessPoint,
    /// An end system: a server, printer, telephone or test host.
    Host,
    /// The Internet, as the lab sees it.
    Internet,
}

impl DeviceRole {
    pub(crate) fn label(self) -> &'static str {
        match self {
            DeviceRole::Core => "core",
            DeviceRole::Distribution => "distribution",
            DeviceRole::Access => "access",
            DeviceRole::Edge => "edge",
            DeviceRole::AccessPoint => "access point",
            DeviceRole::Host => "host",
            DeviceRole::Internet => "internet",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct DevicePlan {
    pub name: String,
    pub site: String,
    pub role: DeviceRole,
    /// A key into `nocturne_netsim::catalog()`.
    pub model: String,
    /// The address the device is managed on; empty for end hosts without one.
    pub mgmt_ip: String,
    /// Routing loopback, empty when the device does not route.
    pub loopback: String,
    pub interfaces: Vec<InterfaceUse>,
}

/// One interface that is in use, and what it is for.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct InterfaceUse {
    pub name: String,
    /// What the far end is; the same text goes into the `description`.
    pub description: String,
    /// `"routed"`, `"trunk"`, `"access 10"`, `"svi"`, `"loopback"`, `"host"`.
    pub mode: String,
    /// `"10.20.0.1/30"`, empty when the interface has no address.
    pub ip: String,
}

/// Generic hardware classes, never invented part numbers or prices.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct BomLine {
    /// `"Layer-3 core switch"`.
    pub class: String,
    /// The simulator model the lab uses to stand for it.
    pub model: String,
    pub quantity: u32,
    /// `"24 × 1G + 4 × 10G"`.
    pub ports: String,
    pub notes: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Expect {
    #[default]
    Reachable,
    Blocked,
}

/// One thing a correct build must satisfy, phrased so it can be probed.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Check {
    /// The node the probe starts from.
    pub from: String,
    /// The IPv4 address probed.
    pub to: String,
    /// What that address is, in words.
    pub to_name: String,
    /// A TCP port to connect to; `None` probes with ICMP.
    pub port: Option<u16>,
    pub expect: Expect,
    pub why: String,
}

/// One choice the design made, and the arithmetic behind it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct Decision {
    /// The site it concerns; empty when it is about the whole project.
    pub site: String,
    pub topic: String,
    pub text: String,
}

impl Decision {
    pub(crate) fn global(topic: &str, text: impl Into<String>) -> Decision {
        Decision {
            site: String::new(),
            topic: topic.to_string(),
            text: text.into(),
        }
    }

    pub(crate) fn site(site: &str, topic: &str, text: impl Into<String>) -> Decision {
        Decision {
            site: site.to_string(),
            topic: topic.to_string(),
            text: text.into(),
        }
    }
}
