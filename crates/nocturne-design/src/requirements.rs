//! What the design is asked for.
//!
//! Every type has a `Default`, so a user interface or a language model can
//! send partial JSON and get a sensible network back.

use serde::{Deserialize, Serialize};

/// A whole project: the sites, how they hang together, and the house style.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Requirements {
    /// What the project is called, e.g. `"Acme branch"`.
    pub name: String,
    /// At least one site.
    pub sites: Vec<Site>,
    pub redundancy: Redundancy,
    /// How the sites interconnect and where the Internet is.
    pub wan: Wan,
    pub addressing: Addressing,
    pub routing: Routing,
    /// Head-room left on ports and subnets, in per cent (default 25).
    pub growth_pct: u32,
    /// Host ports per access switch: 24 or 48 (default 24).
    pub access_ports: u32,
    pub services: Services,
    /// Sample end hosts placed per VLAN so the lab can prove the design
    /// (default 1, at most 4).
    pub hosts_per_vlan: u32,
    /// Passed to the simulator, which derives MAC addresses and serial
    /// numbers from it. It changes nothing else in the design.
    pub seed: u64,
}

impl Default for Requirements {
    fn default() -> Self {
        Requirements {
            name: "Network".to_string(),
            sites: Vec::new(),
            redundancy: Redundancy::None,
            wan: Wan::default(),
            addressing: Addressing::default(),
            routing: Routing::Static,
            growth_pct: 25,
            access_ports: 24,
            services: Services::default(),
            hosts_per_vlan: 1,
            seed: 0,
        }
    }
}

/// One building or office.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Site {
    pub name: String,
    /// People working here; one wired port each.
    pub users: u32,
    /// Desk telephones on a separate voice VLAN.
    pub voice: bool,
    /// A wireless network for visitors, fenced off from everything internal.
    pub guest_wifi: bool,
    /// Servers to host on site.
    pub servers: u32,
    pub printers: u32,
    /// Explicit VLANs. Empty means "work them out from the counts above".
    pub vlans: Vec<VlanReq>,
}

/// One VLAN the user asked for by name.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct VlanReq {
    pub name: String,
    pub purpose: Purpose,
    /// Hosts to plan for, before growth head-room.
    pub hosts: u32,
    /// A fixed VLAN id. Left out, the design picks one.
    pub id: Option<u16>,
}

/// What a VLAN is for. It decides the addressing order, whether the VLAN gets
/// a DHCP pool, and whether it is fenced off from the rest of the network.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    #[default]
    Users,
    Voice,
    /// Visitors: Internet only, no reach into the internal networks.
    Guest,
    Servers,
    Printers,
    /// Device management: the only place the network devices accept logins from.
    Management,
    Other,
}

impl Purpose {
    /// The order VLAN ids and addresses are handed out in.
    pub(crate) const ORDER: [Purpose; 6] = [
        Purpose::Users,
        Purpose::Voice,
        Purpose::Guest,
        Purpose::Servers,
        Purpose::Printers,
        Purpose::Management,
    ];

    /// The default VLAN id for this kind of VLAN (management is configurable).
    pub(crate) fn default_id(self) -> u16 {
        match self {
            Purpose::Users => 10,
            Purpose::Voice => 20,
            Purpose::Guest => 30,
            Purpose::Servers => 40,
            Purpose::Printers => 50,
            Purpose::Management => 99,
            Purpose::Other => 60,
        }
    }

    /// The name used when the user did not give one.
    pub(crate) fn default_name(self) -> &'static str {
        match self {
            Purpose::Users => "USERS",
            Purpose::Voice => "VOICE",
            Purpose::Guest => "GUEST",
            Purpose::Servers => "SERVERS",
            Purpose::Printers => "PRINTERS",
            Purpose::Management => "MGMT",
            Purpose::Other => "DATA",
        }
    }

    /// Hosts in this VLAN get their addresses from DHCP.
    pub(crate) fn wants_dhcp(self) -> bool {
        matches!(self, Purpose::Users | Purpose::Voice | Purpose::Guest)
    }

    /// Plain English, for the plan and the document.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            Purpose::Users => "user workstations",
            Purpose::Voice => "IP telephones",
            Purpose::Guest => "visitor wireless, Internet only",
            Purpose::Servers => "on-site servers",
            Purpose::Printers => "printers and other shared devices",
            Purpose::Management => "network device management",
            Purpose::Other => "general purpose",
        }
    }
}

/// How much of the site survives a failure.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Redundancy {
    /// One core, one uplink per access switch. Cheapest, and every box matters.
    #[default]
    None,
    /// One core, two uplinks per access switch bundled into an EtherChannel.
    Uplinks,
    /// Two cores sharing the gateways, access switches homed to both, two
    /// Internet edge routers.
    Full,
}

/// How the sites reach each other and the Internet.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct Wan {
    pub topology: WanTopology,
    /// The site everything else hangs off. Empty means the first site.
    pub hub: Option<String>,
    pub internet: InternetEdge,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WanTopology {
    /// Islands: the sites do not reach each other.
    #[default]
    None,
    /// Every site has one link to the hub.
    HubAndSpoke,
    /// Every site has a link to every other site.
    FullMesh,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InternetEdge {
    /// No Internet access at all.
    None,
    /// One Internet connection, at the hub; the other sites go through it.
    AtHub,
    /// Every site breaks out to the Internet locally.
    #[default]
    PerSite,
}

/// The address space to build in.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Addressing {
    /// The whole project's IPv4 block, e.g. `"10.20.0.0/16"`.
    pub base: String,
    /// Fix the size of each site's block instead of sizing it for the largest site.
    pub per_site_len: Option<u8>,
    /// Prefix length for routed point-to-point links: 30 or 31.
    pub p2p_len: u8,
    /// VLAN id for device management.
    pub mgmt_vlan: u16,
}

impl Default for Addressing {
    fn default() -> Self {
        Addressing {
            base: "10.0.0.0/16".to_string(),
            per_site_len: None,
            p2p_len: 30,
            mgmt_vlan: 99,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Routing {
    /// Default routes outward, summary routes back.
    #[default]
    Static,
    /// A single OSPF area 0.
    Ospf,
}

/// Services the network itself provides, and the servers it talks to.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Services {
    /// Hand out addresses to users, phones and guests.
    pub dhcp: bool,
    /// Translate the internal blocks to the Internet edge's public address.
    pub nat: bool,
    /// Time source, e.g. `"10.0.99.10"`.
    pub ntp: Option<String>,
    /// Syslog collector.
    pub syslog: Option<String>,
    /// SNMP trap receiver / management station.
    pub snmp: Option<String>,
}

impl Default for Services {
    fn default() -> Self {
        Services {
            dhcp: true,
            nat: true,
            ntp: None,
            syslog: None,
            snmp: None,
        }
    }
}
