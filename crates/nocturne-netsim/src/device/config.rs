//! The device configuration model.
//!
//! This is what a device *is configured to do*, independent of the command
//! language used to say so: the IOS-style front end (`device::ios`) parses
//! commands into it and renders it back as a running configuration, and end
//! hosts derive the same model from their `HostConfig`. The dataplane reads
//! only this model, never command text.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use crate::catalog::{Model, Role};
use crate::ifname::{IfId, IfKind};
use crate::util::crypt;
use crate::util::mac::Mac;
use crate::util::net::{IfAddr, Prefix};

// ------------------------------------------------------------- credentials

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Secret {
    Plain(String),
    /// MD5-crypt, `$1$salt$hash`.
    Md5(String),
    /// Reversible "type 7" obfuscation.
    Type7(String),
}

impl Secret {
    pub(crate) fn verify(&self, typed: &str) -> bool {
        match self {
            Secret::Plain(p) => p == typed,
            Secret::Md5(hash) => crypt::md5_crypt_verify(typed, hash),
            Secret::Type7(enc) => crypt::type7_decode(enc).is_some_and(|p| p == typed),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct User {
    pub(crate) name: String,
    pub(crate) privilege: u8,
    pub(crate) secret: Option<Secret>,
}

// ---------------------------------------------------------------- services

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Timestamps {
    Off,
    Uptime,
    Datetime { msec: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Services {
    pub(crate) timestamps_debug: Timestamps,
    pub(crate) timestamps_log: Timestamps,
    pub(crate) password_encryption: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Timezone {
    pub(crate) name: String,
    pub(crate) hours: i8,
    pub(crate) minutes: u8,
}

// ------------------------------------------------------------------- VLANs

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Vlan {
    pub(crate) name: String,
    /// `state active` (true) or `state suspend`.
    pub(crate) active: bool,
}

impl Vlan {
    pub(crate) fn default_name(id: u16) -> String {
        if id == 1 { "default".to_string() } else { format!("VLAN{id:04}") }
    }

    pub(crate) fn new(id: u16) -> Vlan {
        Vlan { name: Vlan::default_name(id), active: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VtpMode {
    Server,
    Client,
    Transparent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Vtp {
    pub(crate) mode: VtpMode,
    pub(crate) domain: String,
}

/// A set of VLAN ids (1..=4094), as used by trunk allowed lists.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct VlanSet(Box<[u64; 64]>);

impl VlanSet {
    pub(crate) fn none() -> VlanSet {
        VlanSet(Box::new([0; 64]))
    }

    pub(crate) fn all() -> VlanSet {
        let mut set = VlanSet::none();
        for id in 1..=4094 {
            set.insert(id);
        }
        set
    }

    pub(crate) fn from_ids(ids: &[u16]) -> VlanSet {
        let mut set = VlanSet::none();
        for id in ids {
            set.insert(*id);
        }
        set
    }

    pub(crate) fn contains(&self, id: u16) -> bool {
        (1..=4094).contains(&id) && self.0[usize::from(id / 64)] >> (id % 64) & 1 == 1
    }

    pub(crate) fn insert(&mut self, id: u16) {
        if (1..=4094).contains(&id) {
            self.0[usize::from(id / 64)] |= 1 << (id % 64);
        }
    }

    pub(crate) fn remove(&mut self, id: u16) {
        if (1..=4094).contains(&id) {
            self.0[usize::from(id / 64)] &= !(1 << (id % 64));
        }
    }

    pub(crate) fn is_all(&self) -> bool {
        self.iter().count() == 4094
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = u16> + '_ {
        (1..=4094u16).filter(|id| self.contains(*id))
    }

    /// `10,20,30-35`.
    pub(crate) fn to_ranges(&self) -> String {
        crate::util::text::ranges(&self.iter().collect::<Vec<_>>())
    }
}

impl std::fmt::Debug for VlanSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_all() { f.write_str("ALL") } else { f.write_str(&self.to_ranges()) }
    }
}

// ----------------------------------------------------------- spanning tree

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StpMode {
    Pvst,
    RapidPvst,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StpConfig {
    pub(crate) mode: StpMode,
    /// Per-VLAN bridge priority (default 32768).
    pub(crate) priority: BTreeMap<u16, u16>,
    pub(crate) portfast_default: bool,
    pub(crate) bpduguard_default: bool,
}

impl StpConfig {
    pub(crate) fn priority_of(&self, vlan: u16) -> u16 {
        self.priority.get(&vlan).copied().unwrap_or(32768)
    }
}

/// Per-interface spanning-tree overrides.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct IfStp {
    /// `None` = follow the global default.
    pub(crate) portfast: Option<bool>,
    pub(crate) bpduguard: Option<bool>,
    pub(crate) cost: Option<u32>,
    pub(crate) port_priority: Option<u8>,
}

// -------------------------------------------------------------- interfaces

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Duplex {
    Auto,
    Full,
    Half,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SwMode {
    DynamicAuto,
    DynamicDesirable,
    Access,
    Trunk,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Violation {
    Shutdown,
    Restrict,
    Protect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PortSecurity {
    pub(crate) enabled: bool,
    pub(crate) maximum: u32,
    pub(crate) violation: Violation,
    pub(crate) sticky: bool,
    /// Configured secure addresses; the flag marks sticky-learned entries.
    pub(crate) macs: Vec<(Mac, bool)>,
}

impl Default for PortSecurity {
    fn default() -> Self {
        PortSecurity { enabled: false, maximum: 1, violation: Violation::Shutdown, sticky: false, macs: Vec::new() }
    }
}

/// Layer-2 personality of a switch port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Switchport {
    pub(crate) mode: SwMode,
    pub(crate) access_vlan: u16,
    pub(crate) voice_vlan: Option<u16>,
    /// `switchport trunk encapsulation dot1q` was entered (the only encapsulation).
    pub(crate) encap_dot1q: bool,
    pub(crate) native_vlan: u16,
    pub(crate) allowed: VlanSet,
    pub(crate) nonegotiate: bool,
    pub(crate) port_security: PortSecurity,
}

impl Switchport {
    pub(crate) fn new(default_mode: SwMode) -> Switchport {
        Switchport {
            mode: default_mode,
            access_vlan: 1,
            voice_vlan: None,
            encap_dot1q: false,
            native_vlan: 1,
            allowed: VlanSet::all(),
            nonegotiate: false,
            port_security: PortSecurity::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChannelMode {
    On,
    Active,
    Passive,
    Desirable,
    Auto,
}

impl ChannelMode {
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            ChannelMode::On => "on",
            ChannelMode::Active => "active",
            ChannelMode::Passive => "passive",
            ChannelMode::Desirable => "desirable",
            ChannelMode::Auto => "auto",
        }
    }

    /// `"LACP"`, `"PAgP"` or `"-"` (static).
    pub(crate) fn protocol(self) -> &'static str {
        match self {
            ChannelMode::On => "-",
            ChannelMode::Active | ChannelMode::Passive => "LACP",
            ChannelMode::Desirable | ChannelMode::Auto => "PAgP",
        }
    }

    /// Would a member in this mode bundle with a far end in mode `other`?
    pub(crate) fn compatible(self, other: ChannelMode) -> bool {
        use ChannelMode::{Active, Auto, Desirable, On, Passive};
        matches!(
            (self, other),
            (On, On) | (Active, Active | Passive) | (Passive, Active) | (Desirable, Desirable | Auto) | (Auto, Desirable)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum L3Addr {
    Static(IfAddr),
    Dhcp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NatSide {
    Inside,
    Outside,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct IfOspf {
    pub(crate) cost: Option<u32>,
    pub(crate) priority: Option<u8>,
    pub(crate) hello: Option<u16>,
    pub(crate) dead: Option<u16>,
    /// `ip ospf <pid> area <a>`.
    pub(crate) area: Option<(u16, Area)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HsrpGroup {
    pub(crate) group: u16,
    pub(crate) ip: Option<Ipv4Addr>,
    pub(crate) priority: u8,
    pub(crate) preempt: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IfConfig {
    pub(crate) description: String,
    pub(crate) shutdown: bool,
    pub(crate) addr: Option<L3Addr>,
    pub(crate) secondary: Vec<IfAddr>,
    /// Mbit/s; `None` = auto.
    pub(crate) speed: Option<u32>,
    pub(crate) duplex: Duplex,
    pub(crate) mtu: Option<u32>,
    /// kbit/s, for routing metrics only.
    pub(crate) bandwidth: Option<u32>,
    pub(crate) delay: Option<u32>,
    /// `Some` = a layer-2 switch port, `None` = routed (or not a bridge port).
    pub(crate) l2: Option<Switchport>,
    pub(crate) channel_group: Option<(u32, ChannelMode)>,
    pub(crate) stp: IfStp,
    /// Sub-interfaces: `encapsulation dot1Q <vlan> [native]`.
    pub(crate) dot1q: Option<(u16, bool)>,
    pub(crate) helpers: Vec<Ipv4Addr>,
    pub(crate) acl_in: Option<String>,
    pub(crate) acl_out: Option<String>,
    pub(crate) nat: Option<NatSide>,
    pub(crate) ospf: IfOspf,
    pub(crate) hsrp_version: u8,
    pub(crate) hsrp: Vec<HsrpGroup>,
    pub(crate) cdp_enable: bool,
    pub(crate) lldp_transmit: bool,
    pub(crate) lldp_receive: bool,
    /// Accepted-and-stored lines (`storm-control ...`, `ipv6 address ...`), verbatim.
    pub(crate) stored: Vec<String>,
}

impl IfConfig {
    pub(crate) fn new() -> IfConfig {
        IfConfig {
            description: String::new(),
            shutdown: false,
            addr: None,
            secondary: Vec::new(),
            speed: None,
            duplex: Duplex::Auto,
            mtu: None,
            bandwidth: None,
            delay: None,
            l2: None,
            channel_group: None,
            stp: IfStp::default(),
            dot1q: None,
            helpers: Vec::new(),
            acl_in: None,
            acl_out: None,
            nat: None,
            ospf: IfOspf::default(),
            hsrp_version: 1,
            hsrp: Vec::new(),
            cdp_enable: true,
            lldp_transmit: true,
            lldp_receive: true,
            stored: Vec::new(),
        }
    }

    pub(crate) fn static_addr(&self) -> Option<IfAddr> {
        match self.addr {
            Some(L3Addr::Static(addr)) => Some(addr),
            _ => None,
        }
    }

    pub(crate) fn is_switchport(&self) -> bool {
        self.l2.is_some()
    }
}

// ----------------------------------------------------------------- routing

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StaticRoute {
    pub(crate) prefix: Prefix,
    pub(crate) via: Option<Ipv4Addr>,
    pub(crate) iface: Option<IfId>,
    pub(crate) distance: u8,
}

/// An OSPF area id, remembering whether it was typed in dotted form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Area {
    pub(crate) id: u32,
    pub(crate) dotted: bool,
}

impl Area {
    pub(crate) const BACKBONE: u32 = 0;

    pub(crate) fn parse(text: &str) -> Option<Area> {
        if let Ok(id) = text.parse::<u32>() {
            return Some(Area { id, dotted: false });
        }
        crate::util::net::parse_ip(text).map(|ip| Area { id: u32::from(ip), dotted: true })
    }
}

impl std::fmt::Display for Area {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.dotted { write!(f, "{}", Ipv4Addr::from(self.id)) } else { write!(f, "{}", self.id) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OspfNetwork {
    pub(crate) addr: Ipv4Addr,
    pub(crate) wildcard: Ipv4Addr,
    pub(crate) area: Area,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OspfConfig {
    pub(crate) pid: u16,
    pub(crate) router_id: Option<Ipv4Addr>,
    pub(crate) networks: Vec<OspfNetwork>,
    pub(crate) passive_default: bool,
    /// Interfaces explicitly made passive (or, with `passive_default`, explicitly active).
    pub(crate) passive: BTreeSet<IfId>,
    pub(crate) active: BTreeSet<IfId>,
    /// `default-information originate [always]`.
    pub(crate) default_originate: Option<bool>,
    pub(crate) redistribute_static: bool,
    pub(crate) redistribute_connected: bool,
    /// Mbit/s; default 100.
    pub(crate) reference_bandwidth: u32,
}

impl OspfConfig {
    pub(crate) fn new(pid: u16) -> OspfConfig {
        OspfConfig {
            pid,
            router_id: None,
            networks: Vec::new(),
            passive_default: false,
            passive: BTreeSet::new(),
            active: BTreeSet::new(),
            default_originate: None,
            redistribute_static: false,
            redistribute_connected: false,
            reference_bandwidth: 100,
        }
    }

    pub(crate) fn is_passive(&self, iface: IfId) -> bool {
        if self.passive_default { !self.active.contains(&iface) } else { self.passive.contains(&iface) }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RipConfig {
    pub(crate) version: Option<u8>,
    /// Classful networks.
    pub(crate) networks: Vec<Ipv4Addr>,
    pub(crate) auto_summary: bool,
    pub(crate) passive_default: bool,
    pub(crate) passive: BTreeSet<IfId>,
    pub(crate) active: BTreeSet<IfId>,
    pub(crate) default_originate: bool,
}

impl RipConfig {
    pub(crate) fn new() -> RipConfig {
        RipConfig {
            version: None,
            networks: Vec::new(),
            auto_summary: true,
            passive_default: false,
            passive: BTreeSet::new(),
            active: BTreeSet::new(),
            default_originate: false,
        }
    }

    pub(crate) fn is_passive(&self, iface: IfId) -> bool {
        if self.passive_default { !self.active.contains(&iface) } else { self.passive.contains(&iface) }
    }
}

// -------------------------------------------------------------------- ACLs

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AclProto {
    Ip,
    Icmp,
    Tcp,
    Udp,
}

impl AclProto {
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            AclProto::Ip => "ip",
            AclProto::Icmp => "icmp",
            AclProto::Tcp => "tcp",
            AclProto::Udp => "udp",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AddrSpec {
    Any,
    Host(Ipv4Addr),
    /// Base address and wildcard mask.
    Net(Ipv4Addr, Ipv4Addr),
}

impl AddrSpec {
    pub(crate) fn matches(self, ip: Ipv4Addr) -> bool {
        match self {
            AddrSpec::Any => true,
            AddrSpec::Host(host) => host == ip,
            AddrSpec::Net(base, wildcard) => crate::util::net::wildcard_match(ip, base, wildcard),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PortSpec {
    Eq(u16),
    Neq(u16),
    Gt(u16),
    Lt(u16),
    Range(u16, u16),
}

impl PortSpec {
    pub(crate) fn matches(self, port: u16) -> bool {
        match self {
            PortSpec::Eq(p) => port == p,
            PortSpec::Neq(p) => port != p,
            PortSpec::Gt(p) => port > p,
            PortSpec::Lt(p) => port < p,
            PortSpec::Range(lo, hi) => (lo..=hi).contains(&port),
        }
    }
}

/// ICMP message classes an extended entry can single out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IcmpClass {
    Echo,
    EchoReply,
    Unreachable,
    TimeExceeded,
}

impl IcmpClass {
    pub(crate) fn keyword(self) -> &'static str {
        match self {
            IcmpClass::Echo => "echo",
            IcmpClass::EchoReply => "echo-reply",
            IcmpClass::Unreachable => "unreachable",
            IcmpClass::TimeExceeded => "time-exceeded",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AclRule {
    pub(crate) seq: u32,
    pub(crate) permit: bool,
    pub(crate) proto: AclProto,
    pub(crate) src: AddrSpec,
    pub(crate) sport: Option<PortSpec>,
    /// Always `Any` in a standard list.
    pub(crate) dst: AddrSpec,
    pub(crate) dport: Option<PortSpec>,
    pub(crate) icmp: Option<IcmpClass>,
    pub(crate) established: bool,
    pub(crate) log: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AclItem {
    Rule(AclRule),
    Remark(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Acl {
    /// The name, or the number as text.
    pub(crate) name: String,
    pub(crate) extended: bool,
    pub(crate) items: Vec<AclItem>,
}

impl Acl {
    /// A numbered list (1-99, 1300-1999 standard; 100-199, 2000-2699 extended).
    pub(crate) fn number(&self) -> Option<u32> {
        self.name.parse().ok()
    }

    pub(crate) fn is_extended_number(n: u32) -> Option<bool> {
        match n {
            1..=99 | 1300..=1999 => Some(false),
            100..=199 | 2000..=2699 => Some(true),
            _ => None,
        }
    }

    pub(crate) fn rules(&self) -> impl Iterator<Item = &AclRule> {
        self.items.iter().filter_map(|item| match item {
            AclItem::Rule(rule) => Some(rule),
            AclItem::Remark(_) => None,
        })
    }

    pub(crate) fn next_seq(&self) -> u32 {
        self.rules().map(|r| r.seq).max().map_or(10, |max| max.saturating_add(10))
    }

    /// Insert keeping rules ordered by sequence number; an existing rule with the
    /// same sequence number is replaced.
    pub(crate) fn insert(&mut self, rule: AclRule) {
        self.items.retain(|item| !matches!(item, AclItem::Rule(r) if r.seq == rule.seq));
        let at = self
            .items
            .iter()
            .position(|item| matches!(item, AclItem::Rule(r) if r.seq > rule.seq))
            .unwrap_or(self.items.len());
        self.items.insert(at, AclItem::Rule(rule));
    }
}

// --------------------------------------------------------------------- NAT

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NatProto {
    Tcp,
    Udp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NatStatic {
    pub(crate) local: Ipv4Addr,
    pub(crate) global: Ipv4Addr,
    /// Port forward: protocol, local port, global port.
    pub(crate) ports: Option<(NatProto, u16, u16)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NatPool {
    pub(crate) name: String,
    pub(crate) start: Ipv4Addr,
    pub(crate) end: Ipv4Addr,
    pub(crate) prefix_len: u8,
    /// Entered as `prefix-length N` rather than `netmask M`.
    pub(crate) as_prefix: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum NatTarget {
    Interface(IfId),
    Pool(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NatRule {
    pub(crate) acl: String,
    pub(crate) target: NatTarget,
    pub(crate) overload: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct NatConfig {
    pub(crate) statics: Vec<NatStatic>,
    pub(crate) pools: Vec<NatPool>,
    pub(crate) rules: Vec<NatRule>,
}

// -------------------------------------------------------------------- DHCP

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Lease {
    /// Days, hours, minutes.
    Time(u32, u32, u32),
    Infinite,
}

impl Lease {
    pub(crate) fn seconds(self) -> Option<u64> {
        match self {
            Lease::Time(d, h, m) => Some(u64::from(d) * 86_400 + u64::from(h) * 3600 + u64::from(m) * 60),
            Lease::Infinite => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DhcpPool {
    pub(crate) name: String,
    pub(crate) network: Option<Prefix>,
    pub(crate) default_routers: Vec<Ipv4Addr>,
    pub(crate) dns_servers: Vec<Ipv4Addr>,
    pub(crate) domain: Option<String>,
    pub(crate) lease: Option<Lease>,
}

impl DhcpPool {
    pub(crate) fn new(name: &str) -> DhcpPool {
        DhcpPool {
            name: name.to_string(),
            network: None,
            default_routers: Vec::new(),
            dns_servers: Vec::new(),
            domain: None,
            lease: None,
        }
    }
}

// ------------------------------------------------------------------- lines

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LoginMode {
    /// `no login`: nobody is asked for anything.
    None,
    /// `login`: the line password.
    Password,
    /// `login local`: the username database.
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Transport {
    pub(crate) telnet: bool,
    pub(crate) ssh: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineConfig {
    pub(crate) password: Option<Secret>,
    pub(crate) login: LoginMode,
    /// `None` = platform default (both protocols).
    pub(crate) transport_input: Option<Transport>,
    /// Minutes, seconds; `None` = default (10 minutes).
    pub(crate) exec_timeout: Option<(u32, u32)>,
    pub(crate) logging_synchronous: bool,
    pub(crate) access_class_in: Option<String>,
    pub(crate) privilege: Option<u8>,
    pub(crate) stored: Vec<String>,
}

impl LineConfig {
    pub(crate) fn console() -> LineConfig {
        LineConfig {
            password: None,
            login: LoginMode::None,
            transport_input: None,
            exec_timeout: None,
            logging_synchronous: false,
            access_class_in: None,
            privilege: None,
            stored: Vec::new(),
        }
    }

    pub(crate) fn vty() -> LineConfig {
        LineConfig { login: LoginMode::Password, ..LineConfig::console() }
    }

    /// Idle time after which the session is logged out, `None` = never.
    pub(crate) fn timeout_micros(&self) -> Option<u64> {
        let (minutes, seconds) = self.exec_timeout.unwrap_or((10, 0));
        let total = u64::from(minutes) * 60 + u64::from(seconds);
        (total > 0).then_some(total * 1_000_000)
    }
}

// ------------------------------------------------------- management plane

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Logging {
    /// `no logging console` clears it.
    pub(crate) console: bool,
    pub(crate) buffered: Option<u32>,
    pub(crate) hosts: Vec<Ipv4Addr>,
    pub(crate) trap: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SnmpCommunity {
    pub(crate) name: String,
    pub(crate) rw: bool,
    pub(crate) acl: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Snmp {
    pub(crate) communities: Vec<SnmpCommunity>,
    pub(crate) location: Option<String>,
    pub(crate) contact: Option<String>,
    /// `snmp-server host ...` and `snmp-server enable traps ...` lines, verbatim.
    pub(crate) stored: Vec<String>,
}

/// Where an accepted-and-stored global line is printed in the running configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Slot {
    Aaa,
    IpEarly,
    Errdisable,
    Archive,
    Http,
}

// ------------------------------------------------------------- the device

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DeviceConfig {
    pub(crate) hostname: String,
    pub(crate) services: Services,
    pub(crate) enable_secret: Option<Secret>,
    pub(crate) enable_password: Option<Secret>,
    pub(crate) users: Vec<User>,
    pub(crate) clock_timezone: Option<Timezone>,
    pub(crate) ip_routing: bool,
    pub(crate) domain_lookup: bool,
    pub(crate) domain_name: Option<String>,
    pub(crate) name_servers: Vec<Ipv4Addr>,
    pub(crate) default_gateway: Option<Ipv4Addr>,
    pub(crate) default_networks: Vec<Ipv4Addr>,
    pub(crate) dhcp_excluded: Vec<(Ipv4Addr, Ipv4Addr)>,
    pub(crate) dhcp_pools: Vec<DhcpPool>,
    pub(crate) vtp: Vtp,
    pub(crate) vlans: BTreeMap<u16, Vlan>,
    pub(crate) stp: StpConfig,
    pub(crate) interfaces: BTreeMap<IfId, IfConfig>,
    pub(crate) ospf: Option<OspfConfig>,
    pub(crate) rip: Option<RipConfig>,
    pub(crate) static_routes: Vec<StaticRoute>,
    pub(crate) acls: Vec<Acl>,
    pub(crate) nat: NatConfig,
    pub(crate) logging: Logging,
    pub(crate) snmp: Snmp,
    pub(crate) ntp_servers: Vec<Ipv4Addr>,
    pub(crate) banner_motd: Option<String>,
    pub(crate) banner_login: Option<String>,
    pub(crate) console: LineConfig,
    /// One entry per vty line (5 on routers, 16 on switches).
    pub(crate) vty: Vec<LineConfig>,
    pub(crate) cdp_run: bool,
    pub(crate) lldp_run: bool,
    pub(crate) ssh_version: Option<u8>,
    /// Accepted-and-stored global lines, verbatim, by position in the output.
    pub(crate) stored: Vec<(Slot, String)>,
}

impl DeviceConfig {
    /// The factory configuration of a model.
    pub(crate) fn factory(model: &Model, hostname: &str) -> DeviceConfig {
        let mut interfaces = BTreeMap::new();
        for port in &model.ports {
            let mut cfg = IfConfig::new();
            match model.role {
                // Router ports are administratively down until configured.
                Role::Router => cfg.shutdown = true,
                Role::L2Switch | Role::L3Switch => cfg.l2 = Some(Switchport::new(SwMode::DynamicAuto)),
                Role::Host | Role::Cloud | Role::Bridge => {}
            }
            interfaces.insert(*port, cfg);
        }
        let mut vlans = BTreeMap::new();
        if model.is_switch() {
            vlans.insert(1, Vlan::new(1));
            let mut svi = IfConfig::new();
            svi.shutdown = true;
            interfaces.insert(IfId::vlan(1), svi);
        }
        let vty_lines = if model.is_switch() { 16 } else { 5 };
        DeviceConfig {
            hostname: hostname.to_string(),
            services: Services {
                timestamps_debug: Timestamps::Datetime { msec: true },
                timestamps_log: Timestamps::Datetime { msec: true },
                password_encryption: false,
            },
            enable_secret: None,
            enable_password: None,
            users: Vec::new(),
            clock_timezone: None,
            ip_routing: matches!(model.role, Role::Router | Role::Cloud),
            domain_lookup: true,
            domain_name: None,
            name_servers: Vec::new(),
            default_gateway: None,
            default_networks: Vec::new(),
            dhcp_excluded: Vec::new(),
            dhcp_pools: Vec::new(),
            vtp: Vtp { mode: VtpMode::Server, domain: String::new() },
            vlans,
            stp: StpConfig {
                mode: StpMode::Pvst,
                priority: BTreeMap::new(),
                portfast_default: false,
                bpduguard_default: false,
            },
            interfaces,
            ospf: None,
            rip: None,
            static_routes: Vec::new(),
            acls: Vec::new(),
            nat: NatConfig::default(),
            logging: Logging { console: true, buffered: None, hosts: Vec::new(), trap: None },
            snmp: Snmp::default(),
            ntp_servers: Vec::new(),
            banner_motd: None,
            banner_login: None,
            console: LineConfig::console(),
            vty: vec![LineConfig::vty(); vty_lines],
            cdp_run: model.is_network_os(),
            lldp_run: false,
            ssh_version: None,
            stored: Vec::new(),
        }
    }

    pub(crate) fn acl(&self, name: &str) -> Option<&Acl> {
        self.acls.iter().find(|a| a.name == name)
    }

    pub(crate) fn acl_mut(&mut self, name: &str) -> Option<&mut Acl> {
        self.acls.iter_mut().find(|a| a.name == name)
    }

    pub(crate) fn user(&self, name: &str) -> Option<&User> {
        self.users.iter().find(|u| u.name == name)
    }

    pub(crate) fn iface(&self, id: IfId) -> Option<&IfConfig> {
        self.interfaces.get(&id)
    }

    /// Is the VLAN present and active in the VLAN database?
    pub(crate) fn vlan_active(&self, id: u16) -> bool {
        self.vlans.get(&id).is_some_and(|v| v.active)
    }

    /// Members of a port-channel, in interface order.
    pub(crate) fn channel_members(&self, group: u32) -> Vec<IfId> {
        self.interfaces
            .iter()
            .filter(|(_, cfg)| cfg.channel_group.is_some_and(|(g, _)| g == group))
            .map(|(id, _)| *id)
            .collect()
    }

    /// Sub-interfaces of a physical port.
    pub(crate) fn subinterfaces(&self, parent: IfId) -> impl Iterator<Item = (IfId, &IfConfig)> {
        self.interfaces.iter().filter(move |(id, _)| id.is_sub() && id.parent() == parent).map(|(id, cfg)| (*id, cfg))
    }
}

/// Can a logical interface of this kind be created on demand on this model?
pub(crate) fn can_create(model: &Model, id: IfId) -> bool {
    match (id.kind, id.is_sub()) {
        (IfKind::Loopback, false) => model.is_network_os(),
        (IfKind::Vlan, false) => model.is_switch() && (1..=4094).contains(&id.number()),
        (IfKind::PortChannel, false) => model.is_switch() && (1..=48).contains(&id.number()),
        (_, true) => {
            id.kind.is_physical() && model.ports.contains(&id.parent()) && matches!(model.role, Role::Router | Role::L3Switch)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::model;

    #[test]
    fn vlan_sets() {
        let mut set = VlanSet::all();
        assert!(set.is_all() && set.contains(1) && set.contains(4094) && !set.contains(0) && !set.contains(4095));
        set.remove(10);
        assert!(!set.is_all() && !set.contains(10));
        let set = VlanSet::from_ids(&[10, 20, 30, 31, 32]);
        assert_eq!(set.to_ranges(), "10,20,30-32");
        assert!(VlanSet::none().is_empty());
    }

    #[test]
    fn factory_defaults() {
        let router = DeviceConfig::factory(model("router").unwrap(), "R1");
        assert!(router.ip_routing && router.interfaces.values().all(|i| i.shutdown && i.l2.is_none()));
        assert_eq!(router.vty.len(), 5);
        let switch = DeviceConfig::factory(model("switch").unwrap(), "SW1");
        assert!(!switch.ip_routing && switch.vlans.contains_key(&1));
        assert!(switch.interfaces[&IfId::parse("fa0/1").unwrap()].is_switchport());
        assert!(switch.interfaces[&IfId::vlan(1)].shutdown);
        assert_eq!(switch.vty.len(), 16);
    }

    #[test]
    fn secrets_and_channel_modes() {
        assert!(Secret::Plain("x".into()).verify("x"));
        assert!(Secret::Md5("$1$xxxxxxxx$UYCIxa628.9qXjpQCjM4a.".into()).verify("password"));
        assert!(Secret::Type7("0822455D0A16".into()).verify("cisco"));
        assert!(!Secret::Type7("0822455D0A16".into()).verify("Cisco"));
        assert!(ChannelMode::Active.compatible(ChannelMode::Passive));
        assert!(!ChannelMode::Passive.compatible(ChannelMode::Passive));
        assert!(!ChannelMode::On.compatible(ChannelMode::Active));
        assert!(ChannelMode::Auto.compatible(ChannelMode::Desirable));
    }

    #[test]
    fn acl_insert_keeps_sequence_order() {
        let rule = |seq| AclRule {
            seq,
            permit: true,
            proto: AclProto::Ip,
            src: AddrSpec::Any,
            sport: None,
            dst: AddrSpec::Any,
            dport: None,
            icmp: None,
            established: false,
            log: false,
        };
        let mut acl = Acl { name: "T".into(), extended: false, items: Vec::new() };
        assert_eq!(acl.next_seq(), 10);
        acl.insert(rule(10));
        acl.insert(rule(30));
        acl.insert(rule(20));
        acl.insert(rule(20));
        assert_eq!(acl.rules().map(|r| r.seq).collect::<Vec<_>>(), [10, 20, 30]);
        assert_eq!(acl.next_seq(), 40);
    }
}
