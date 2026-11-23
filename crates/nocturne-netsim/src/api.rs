//! Public data types: the lab file format, views, events and faults.
//!
//! Everything here is plain serialisable data; the behaviour lives in [`crate::Lab`].

use serde::{Deserialize, Serialize};

/// Stable for the life of the lab, never reused.
pub type NodeId = u32;
pub type LinkId = u32;
pub type ConsoleId = u32;
pub type FlowId = u32;
pub type FaultId = u32;
pub type TraceId = u32;
/// Simulated time in microseconds since the lab was created.
pub type SimTime = u64;

// ------------------------------------------------------------------ errors

/// Why a lab API call was refused.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabError {
    #[error("no node with id {0}")]
    UnknownNode(NodeId),
    #[error("no node named \"{0}\"")]
    UnknownNodeName(String),
    #[error("no link with id {0}")]
    UnknownLink(LinkId),
    #[error("unknown device model \"{0}\"")]
    UnknownModel(String),
    #[error("a node named \"{0}\" already exists")]
    DuplicateName(String),
    #[error("\"{0}\" is not a valid node name")]
    InvalidName(String),
    #[error("{node} has no interface \"{interface}\"")]
    UnknownInterface { node: String, interface: String },
    #[error("{node} {interface} is already cabled")]
    InterfaceInUse { node: String, interface: String },
    #[error("a link needs two different nodes")]
    SelfLink,
    #[error("{0} has no command line")]
    NoCli(String),
    #[error("{0} is powered off")]
    PoweredOff(String),
    #[error("{0} is not an end host")]
    NotAHost(String),
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    #[error("invalid lab: {0}")]
    InvalidSpec(String),
    #[error("not supported: {0}")]
    Unsupported(String),
}

// ---------------------------------------------------------------- lab file

/// A complete, serialisable lab: what [`crate::Lab::from_spec`] loads and
/// [`crate::Lab::to_spec`] saves.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct LabSpec {
    pub name: String,
    pub description: String,
    pub nodes: Vec<NodeSpec>,
    pub links: Vec<LinkSpec>,
    pub notes: Vec<NoteSpec>,
    pub flows: Vec<FlowSpec>,
    pub seed: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct NodeSpec {
    /// Unique; also the initial hostname.
    pub name: String,
    /// Key into [`crate::catalog`].
    pub model: String,
    pub x: f32,
    pub y: f32,
    /// Defaults to `true`.
    pub powered: bool,
    /// Network devices: CLI configuration text applied at boot.
    pub startup_config: Option<String>,
    /// End hosts: addressing and services.
    pub host: Option<HostConfig>,
    /// Addition to the contract: the switch's VLAN database (`flash:vlan.dat`), as
    /// `vlan N` / ` name X` lines. Real switches keep VLANs outside the startup
    /// configuration unless VTP is transparent; this field lets them survive a
    /// save/load. Hand-written labs can simply put `vlan` lines in `startup_config`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vlan_dat: Option<String>,
    /// Addition to the contract: size of the device's RSA host key, if one was
    /// generated (`crypto key generate rsa`). Real devices keep the key outside
    /// the configuration; without it SSH to the device is refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rsa_key_bits: Option<u16>,
}

impl Default for NodeSpec {
    fn default() -> Self {
        NodeSpec {
            name: String::new(),
            model: String::new(),
            x: 0.0,
            y: 0.0,
            powered: true,
            startup_config: None,
            host: None,
            vlan_dat: None,
            rsa_key_bits: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct HostConfig {
    pub dhcp: bool,
    /// `"192.168.1.10"`.
    pub ip: Option<String>,
    /// `"255.255.255.0"`.
    pub mask: Option<String>,
    pub gateway: Option<String>,
    pub dns: Option<String>,
    /// Extra addresses the node answers for, `"8.8.8.8/32"` style. On a multi-port
    /// node (`cloud`) an entry may be bound to a port: `"203.0.113.1/30@Ethernet1"`.
    pub extra_ips: Vec<String>,
    /// `"http"`, `"dns"`, `"dhcp"`, `"tftp"`, `"syslog"`, `"ntp"`, `"ssh"`, `"telnet"`.
    pub services: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LinkSpec {
    pub a: Endpoint,
    pub b: Endpoint,
    #[serde(default)]
    pub cable: CableKind,
}

/// One end of a link, by node *name*.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Endpoint {
    pub node: String,
    pub interface: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CableKind {
    #[default]
    Auto,
    Copper,
    Fiber,
    /// Out-of-band console cable between the well-known ports `Console` (network
    /// device) and `RS232` (host). Never carries frames.
    Console,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct NoteSpec {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub text: String,
    pub color: String,
}

/// Background traffic that moves interface counters without emitting packets.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct FlowSpec {
    pub name: String,
    /// Source node name.
    pub src: String,
    pub dst_ip: String,
    /// Average rate in bits per second.
    pub bps: u64,
    pub pattern: FlowPattern,
    pub enabled: bool,
    /// Addition to the contract: the DSCP marking the traffic carries, 0 to 63
    /// (46 = EF). `None`, the default, states no marking: the flow matches no
    /// `match dscp` and falls into `class-default` unless an access list
    /// catches it. Left out of the saved lab when unset, so labs saved before
    /// the field existed load and save unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dscp: Option<u8>,
}

impl Default for FlowSpec {
    fn default() -> Self {
        FlowSpec {
            name: String::new(),
            src: String::new(),
            dst_ip: String::new(),
            bps: 0,
            pattern: FlowPattern::Constant,
            enabled: true,
            dscp: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlowPattern {
    #[default]
    Constant,
    /// Rate swings between 0 and 2 × `bps` over `period_s` seconds.
    Sine { period_s: u32 },
    /// `on_s` seconds at a rate that averages to `bps`, then `off_s` seconds idle.
    Burst { on_s: u32, off_s: u32 },
}

/// Live state of a flow, as returned by [`crate::Lab::flows`].
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct FlowState {
    /// The forwarding path currently delivers the traffic.
    pub active: bool,
    /// Rate being offered right now (pattern applied), bits per second.
    pub current_bps: u64,
    /// Octets delivered end to end since the flow was added.
    pub delivered_octets: u64,
    /// Node names along the resolved path, source first.
    pub path: Vec<String>,
    /// Why the flow is not delivering, empty when `active`.
    pub reason: String,
}

// ---------------------------------------------------------------- catalog

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ModelInfo {
    /// `"router"`, `"switch"`, `"switch-l3"`, `"pc"`, ...
    pub id: String,
    pub kind: NodeKind,
    /// `"Router (4 × GE)"`.
    pub label: String,
    pub description: String,
    /// Full interface names in display order.
    pub interfaces: Vec<String>,
    pub cli: CliKind,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    Router,
    Switch,
    L3Switch,
    Firewall,
    AccessPoint,
    Pc,
    Laptop,
    Server,
    Printer,
    Phone,
    Cloud,
    Nms,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CliKind {
    Ios,
    Host,
    None,
}

// ------------------------------------------------------------ automation

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ConfigOutcome {
    pub lines: Vec<ConfigLineResult>,
    /// No line was rejected.
    pub ok: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ConfigLineResult {
    pub line: String,
    /// What the device printed in response (often empty).
    pub output: String,
    /// The device rejected the line.
    pub error: bool,
}

// ------------------------------------------------------------- inspection

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct LabSnapshot {
    pub name: String,
    pub description: String,
    pub now: SimTime,
    pub nodes: Vec<NodeView>,
    pub links: Vec<LinkView>,
    pub notes: Vec<NoteSpec>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NodeView {
    pub id: NodeId,
    pub name: String,
    pub hostname: String,
    pub model: String,
    pub kind: NodeKind,
    pub x: f32,
    pub y: f32,
    pub powered: bool,
    pub cli: CliKind,
    pub interfaces: Vec<IfView>,
    /// Best address to manage the device on.
    pub mgmt_ip: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IfView {
    /// `"GigabitEthernet0/1"`.
    pub name: String,
    /// `"Gi0/1"`.
    pub short: String,
    pub link: Option<LinkId>,
    pub admin_up: bool,
    pub oper_up: bool,
    pub light: Light,
    /// `"10.0.0.1/24"`.
    pub ip: Option<String>,
    /// `"access 10"` | `"trunk"` | `"routed"` | `"host"` | `""`.
    pub mode: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Light {
    /// Up and forwarding.
    Green,
    /// Up but not (yet) forwarding: spanning tree listening/learning/blocking.
    Amber,
    /// Cabled but down (shutdown, err-disabled, far end down).
    Red,
    /// No cable, or the device is powered off.
    Off,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LinkView {
    pub id: LinkId,
    pub a: NodeId,
    pub a_if: String,
    pub b: NodeId,
    pub b_if: String,
    pub cable: CableKind,
    pub up: bool,
    pub a_light: Light,
    pub b_light: Light,
    /// Current simulated rate from `a` to `b`, bits per second.
    pub bps_ab: u64,
    pub bps_ba: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NodeTables {
    pub routes: Vec<RouteRow>,
    pub arp: Vec<ArpRow>,
    pub mac: Vec<MacRow>,
    pub vlans: Vec<VlanRow>,
    pub stp: Vec<StpRow>,
    pub neighbors: Vec<NeighborRow>,
    pub ospf_neighbors: Vec<OspfNeighborRow>,
    pub dhcp_bindings: Vec<DhcpRow>,
    pub nat: Vec<NatRow>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct RouteRow {
    /// `"C"`, `"S"`, `"S*"`, `"O"`, `"O IA"`, `"O E2"`, `"R"`.
    pub code: String,
    /// `"10.0.0.0/24"`.
    pub prefix: String,
    pub ad: u32,
    pub metric: u32,
    /// Empty for directly connected routes.
    pub next_hop: String,
    pub interface: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ArpRow {
    pub ip: String,
    /// `"0050.7966.6801"`.
    pub mac: String,
    pub interface: String,
    /// Zero for the device's own addresses.
    pub age_s: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct MacRow {
    pub vlan: u16,
    pub mac: String,
    /// `"DYNAMIC"` or `"STATIC"`.
    pub kind: String,
    pub port: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct VlanRow {
    pub id: u16,
    pub name: String,
    /// `"active"` or `"suspended"`.
    pub status: String,
    /// Short names of the access ports in the VLAN.
    pub ports: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct StpRow {
    pub vlan: u16,
    pub interface: String,
    /// `"Root"`, `"Desg"`, `"Altn"`, `"Back"`.
    pub role: String,
    /// `"FWD"`, `"BLK"`, `"LIS"`, `"LRN"`.
    pub state: String,
    pub cost: u32,
    /// `"32778 0050.7966.6801"` (priority and address).
    pub root_id: String,
    pub bridge_id: String,
    /// This bridge is the root of the VLAN.
    pub is_root: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NeighborRow {
    /// `"CDP"` or `"LLDP"`.
    pub protocol: String,
    pub local_if: String,
    pub device: String,
    pub remote_if: String,
    pub platform: String,
    pub mgmt_ip: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct OspfNeighborRow {
    pub neighbor_id: String,
    pub priority: u8,
    /// `"FULL/DR"`, `"2WAY/DROTHER"`, `"INIT/DROTHER"`, ...
    pub state: String,
    pub dead_time_s: u64,
    pub address: String,
    pub interface: String,
    pub area: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct DhcpRow {
    pub ip: String,
    pub mac: String,
    pub pool: String,
    /// Seconds until the lease expires.
    pub expires_s: u64,
    /// `"Automatic"`.
    pub kind: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NatRow {
    /// `"icmp"`, `"tcp"`, `"udp"` or `"---"` for a static entry.
    pub protocol: String,
    pub inside_global: String,
    pub inside_local: String,
    pub outside_local: String,
    pub outside_global: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct NodeMetrics {
    pub hostname: String,
    pub uptime_s: u64,
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub temperature_c: f32,
    pub interfaces: Vec<IfCounters>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct IfCounters {
    pub name: String,
    pub description: String,
    pub admin_up: bool,
    pub oper_up: bool,
    pub speed_bps: u64,
    pub mtu: u32,
    pub in_octets: u64,
    pub out_octets: u64,
    pub in_pkts: u64,
    pub out_pkts: u64,
    pub in_errors: u64,
    pub out_errors: u64,
    pub in_discards: u64,
    pub out_discards: u64,
    pub crc: u64,
    pub last_change: SimTime,
}

// ------------------------------------------------------------------ probes

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ProbeResult {
    pub ok: bool,
    /// Round-trip time of the successful exchange, microseconds.
    pub rtt_us: u64,
    /// The forward path, source first. On failure: as far as the probe got.
    pub hops: Vec<ProbeHop>,
    /// Why the probe failed; empty on success.
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ProbeHop {
    pub node: NodeId,
    pub name: String,
    /// Empty on the source.
    pub in_if: String,
    /// Empty on the destination.
    pub out_if: String,
    /// What the device did with the packet.
    pub note: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PduKind {
    Icmp,
    Tcp { port: u16 },
    Udp { port: u16 },
}

// ------------------------------------------------------------------ events

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LabEvent {
    LinkState {
        link: LinkId,
        up: bool,
    },
    Syslog {
        node: NodeId,
        time: SimTime,
        severity: u8,
        facility: String,
        mnemonic: String,
        message: String,
        /// The fully formatted line, timestamp included.
        line: String,
    },
    Packet(PacketEvent),
    /// Configuration or tables changed: the host should refresh its views.
    NodeChanged {
        node: NodeId,
    },
    TraceDone {
        trace: TraceId,
        ok: bool,
        summary: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PacketEvent {
    pub trace: Option<TraceId>,
    /// Identity of the PDU; stays the same while it is forwarded, routed or translated.
    pub pdu: u64,
    /// Lab-wide, strictly increasing event number.
    pub seq: u32,
    pub time: SimTime,
    pub kind: PacketEventKind,
    pub at: NodeId,
    /// `Received` only: the node the frame came from.
    pub from: Option<NodeId>,
    /// `Received` only: the link the frame crossed.
    pub link: Option<LinkId>,
    /// `"ARP"`, `"ICMP"`, `"DHCP"`, `"OSPF"`, `"STP"`, `"TCP"`, `"UDP"`, `"CDP"`, ...
    pub protocol: String,
    /// `"ICMP echo request 10.0.0.2 → 10.0.1.5"`.
    pub summary: String,
    /// OSI-style breakdown for the inspector.
    pub layers: Vec<LayerInfo>,
    /// Plain English: what this device did and why.
    pub decision: String,
    /// Addition to the contract: the interface involved at `at` (ingress for
    /// `Received`/`Dropped` on input, egress for `Forwarded` out of one port), or empty.
    #[serde(default)]
    pub interface: String,
    /// Addition to the contract: `seq` of the event this one follows from (0 = none),
    /// so a flooded frame's branches can be told apart.
    #[serde(default)]
    pub parent: u32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PacketEventKind {
    Created,
    Forwarded,
    Received,
    Delivered,
    Dropped,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LayerInfo {
    /// OSI layer number: 2, 3, 4 or 7.
    pub layer: u8,
    pub title: String,
    pub lines: Vec<String>,
}

// ------------------------------------------------------------------ faults

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    LinkDown {
        link: LinkId,
    },
    InterfaceErrors {
        node: NodeId,
        interface: String,
        crc_per_s: u32,
    },
    PacketLoss {
        link: LinkId,
        percent: u8,
    },
    Flapping {
        link: LinkId,
        period_s: u32,
    },
    HighCpu {
        node: NodeId,
        percent: u8,
    },
    PowerOff {
        node: NodeId,
    },
    DuplexMismatch {
        link: LinkId,
    },
}
