//! A simulated device: its identity, configuration and runtime state.
//!
//! All protocol logic is written as functions of one node plus an [`Fx`] effect
//! collector (see `sim::fx`); nothing in here reaches into another node.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::{HostConfig, LinkId, NodeId, NodeKind, SimTime};
use crate::catalog::{Model, Role};
use crate::device::config::{DeviceConfig, L3Addr, SwMode, Vlan};
use crate::ifname::{IfId, IfKind};
use crate::sim::arp::ArpCache;
use crate::sim::dhcp::{DhcpClient, DhcpServer};
use crate::sim::hsrp::Hsrp;
use crate::sim::nat::Nat;
use crate::sim::neighbors::Neighbors;
use crate::sim::ospf::Ospf;
use crate::sim::procs::Procs;
use crate::sim::rib::Rib;
use crate::sim::rip::Rip;
use crate::sim::stp::Stp;
use crate::sim::switching::MacTable;
use crate::sim::syslog::LogBuffer;
use crate::util::mac::Mac;
use crate::util::net::IfAddr;
use crate::util::rng;

#[derive(Clone, Debug, Default)]
pub(crate) struct Counters {
    pub(crate) in_pkts: u64,
    pub(crate) in_octets: u64,
    pub(crate) out_pkts: u64,
    pub(crate) out_octets: u64,
    pub(crate) in_broadcasts: u64,
    pub(crate) in_errors: u64,
    pub(crate) crc: u64,
    pub(crate) out_errors: u64,
    pub(crate) in_discards: u64,
    pub(crate) out_discards: u64,
    pub(crate) collisions: u64,
    pub(crate) late_collisions: u64,
    pub(crate) resets: u64,
    pub(crate) last_in: Option<SimTime>,
    pub(crate) last_out: Option<SimTime>,
}

/// What the far end of a cable looks like, as far as negotiation is concerned.
/// Refreshed by the world whenever either end changes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PeerFacts {
    pub(crate) node: NodeId,
    pub(crate) port: IfId,
    /// Administrative switchport mode, if the far end is a switch port.
    pub(crate) sw_mode: Option<SwMode>,
    pub(crate) nonegotiate: bool,
    /// Far end's channel-group number and mode.
    pub(crate) channel: Option<(u32, crate::device::config::ChannelMode)>,
}

/// Runtime state of one interface (physical or logical).
#[derive(Clone, Debug)]
pub(crate) struct IfState {
    pub(crate) mac: Mac,
    /// Physical ports only.
    pub(crate) link: Option<LinkId>,
    pub(crate) peer: Option<PeerFacts>,
    /// Physical: the cable carries signal. Logical: the underlying resource is there.
    pub(crate) carrier: bool,
    /// Line protocol up: usable for traffic.
    pub(crate) up: bool,
    /// The interface was shut down when last reconciled (for syslog transitions).
    pub(crate) was_shutdown: bool,
    /// Err-disable cause, e.g. `"bpduguard"`.
    pub(crate) errdisabled: Option<&'static str>,
    /// Operational speed in Mbit/s and duplex.
    pub(crate) speed: u32,
    pub(crate) full_duplex: bool,
    /// Switch port is operating as a trunk.
    pub(crate) trunking: bool,
    /// Channel-group member is bundled into its port-channel.
    pub(crate) bundled: bool,
    /// Channel-group member whose switch-port settings differ from the port-channel's.
    pub(crate) mismatched: bool,
    /// Channel-group member that negotiates but has found no willing partner.
    pub(crate) standalone: bool,
    pub(crate) counters: Counters,
    pub(crate) last_change: SimTime,
    pub(crate) dhcp: DhcpClient,
    /// Port security: dynamically learned secure addresses.
    pub(crate) secure_macs: Vec<(Mac, u16)>,
    pub(crate) security_violations: u64,
    pub(crate) last_secure_src: Option<(Mac, u16)>,
    /// Smoothed rates in bits per second (driven by flows).
    pub(crate) rate_in: f64,
    pub(crate) rate_out: f64,
    /// Rates offered by flows right now, bits per second.
    pub(crate) load_in: f64,
    pub(crate) load_out: f64,
}

impl IfState {
    pub(crate) fn new(mac: Mac, speed: u32) -> IfState {
        IfState {
            mac,
            link: None,
            peer: None,
            carrier: false,
            up: false,
            was_shutdown: false,
            errdisabled: None,
            mismatched: false,
            standalone: false,
            speed,
            full_duplex: true,
            trunking: false,
            bundled: false,
            counters: Counters::default(),
            last_change: 0,
            dhcp: DhcpClient::default(),
            secure_macs: Vec::new(),
            security_violations: 0,
            last_secure_src: None,
            rate_in: 0.0,
            rate_out: 0.0,
            load_in: 0.0,
            load_out: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub(crate) id: NodeId,
    /// Canvas name (unique in the lab); the hostname lives in the configuration.
    pub(crate) name: String,
    pub(crate) model: &'static Model,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) powered: bool,
    /// Incremented on every power change so stale timers can be recognised.
    pub(crate) epoch: u32,
    pub(crate) base_mac: Mac,
    pub(crate) serial: String,
    pub(crate) boot_time: SimTime,
    /// Milliseconds added to the 1993 epoch + uptime by `clock set`.
    pub(crate) clock_offset_ms: Option<i64>,
    pub(crate) cfg: DeviceConfig,
    /// The saved configuration (`None` = never saved).
    pub(crate) startup: Option<String>,
    /// `flash:vlan.dat`.
    pub(crate) vlan_dat: Option<BTreeMap<u16, Vlan>>,
    /// End hosts: the persisted addressing the configuration is derived from.
    pub(crate) host: Option<HostConfig>,
    pub(crate) ifs: BTreeMap<IfId, IfState>,
    pub(crate) macs: MacTable,
    pub(crate) arp: ArpCache,
    pub(crate) rib: Rib,
    pub(crate) stp: Stp,
    pub(crate) neighbors: Neighbors,
    pub(crate) ospf: Ospf,
    pub(crate) rip: Rip,
    pub(crate) dhcpd: DhcpServer,
    pub(crate) nat: Nat,
    pub(crate) hsrp: Hsrp,
    /// ACL hit counters by (list name, sequence number).
    pub(crate) acl_hits: BTreeMap<(String, u32), u64>,
    pub(crate) log: LogBuffer,
    pub(crate) procs: Procs,
    /// Sim time of the last configuration change (for `show running-config` headers).
    pub(crate) last_config_change: Option<SimTime>,
    /// Packets routed or switched since boot (drives the simulated CPU load).
    pub(crate) forwarded: u64,
    /// Source of ephemeral ports and identifiers.
    pub(crate) next_ident: u16,
    /// Name → address records served by the `dns` service (kept by the world).
    pub(crate) dns_zone: Vec<(String, Ipv4Addr)>,
    /// Sessions currently open on the device's lines (for `show users`).
    pub(crate) sessions: Vec<crate::device::cli::SessionInfo>,
    /// The running configuration differs from what was last saved.
    pub(crate) unsaved: bool,
    /// Startup-configuration lines the device rejected at boot, with its reply.
    pub(crate) boot_errors: Vec<(String, String)>,
    /// RSA host key size, once generated (kept in private NVRAM across reloads).
    pub(crate) rsa_bits: Option<u16>,
    /// Extra CPU load from `HighCpu` faults, in percent.
    pub(crate) cpu_fault: u8,
}

impl Node {
    pub(crate) fn new(id: NodeId, name: &str, model: &'static Model, seed: u64) -> Node {
        let tag = (rng::mix(seed) as u16).wrapping_add(id as u16);
        let base_mac = Mac([0x02, 0x4e, 0x43, (tag >> 8) as u8, tag as u8, 0x00]);
        let serial = format!("NV{:05}{:03}", rng::mix(seed) % 100_000, 100 + id % 900);
        let mut node = Node {
            id,
            name: name.to_string(),
            model,
            x: 0.0,
            y: 0.0,
            powered: true,
            epoch: 0,
            base_mac,
            serial,
            boot_time: 0,
            clock_offset_ms: None,
            cfg: DeviceConfig::factory(model, name),
            startup: None,
            vlan_dat: None,
            host: None,
            ifs: BTreeMap::new(),
            macs: MacTable::default(),
            arp: ArpCache::default(),
            rib: Rib::default(),
            stp: Stp::default(),
            neighbors: Neighbors::default(),
            ospf: Ospf::default(),
            rip: Rip::default(),
            dhcpd: DhcpServer::default(),
            nat: Nat::default(),
            hsrp: Hsrp::default(),
            acl_hits: BTreeMap::new(),
            log: LogBuffer::default(),
            procs: Procs::default(),
            last_config_change: None,
            forwarded: 0,
            next_ident: 0,
            dns_zone: Vec::new(),
            sessions: Vec::new(),
            unsaved: false,
            boot_errors: Vec::new(),
            rsa_bits: None,
            cpu_fault: 0,
        };
        for (index, port) in model.ports.iter().enumerate() {
            let mac = base_mac.offset(index as u32 + 1);
            node.ifs
                .insert(*port, IfState::new(mac, port.kind.max_mbps()));
        }
        node
    }

    pub(crate) fn role(&self) -> Role {
        self.model.role
    }

    /// "router", "switch", "host", ... for plain-English explanations.
    pub(crate) fn noun(&self) -> &'static str {
        match self.model.kind {
            NodeKind::Router => "router",
            NodeKind::Switch | NodeKind::L3Switch => "switch",
            NodeKind::Firewall => "firewall",
            NodeKind::AccessPoint => "access point",
            NodeKind::Server => "server",
            NodeKind::Cloud => "cloud",
            NodeKind::Pc
            | NodeKind::Laptop
            | NodeKind::Printer
            | NodeKind::Phone
            | NodeKind::Nms => "host",
        }
    }

    pub(crate) fn hostname(&self) -> &str {
        &self.cfg.hostname
    }

    /// MAC address a logical or physical interface uses as its source address.
    pub(crate) fn mac_for(&self, id: IfId) -> Mac {
        match id.kind {
            IfKind::Vlan => self.base_mac.offset(0x40),
            IfKind::Loopback => Mac::ZERO,
            IfKind::PortChannel => self
                .cfg
                .channel_members(id.number())
                .first()
                .and_then(|member| self.ifs.get(member))
                .map_or(self.base_mac.offset(0x60 + id.number()), |state| state.mac),
            _ => self
                .ifs
                .get(&id.parent())
                .map_or(self.base_mac, |state| state.mac),
        }
    }

    pub(crate) fn is_up(&self, id: IfId) -> bool {
        self.ifs.get(&id).is_some_and(|state| state.up)
    }

    /// Does this interface take part in IP (routed port, SVI, loopback, sub-interface)?
    pub(crate) fn is_l3(&self, id: IfId) -> bool {
        self.role() != Role::Bridge
            && self
                .cfg
                .iface(id)
                .is_some_and(|cfg| cfg.l2.is_none() && cfg.channel_group.is_none())
    }

    /// Primary address in effect on an interface (static, or learned by DHCP).
    pub(crate) fn addr(&self, id: IfId) -> Option<IfAddr> {
        match self.cfg.iface(id)?.addr? {
            L3Addr::Static(addr) => Some(addr),
            L3Addr::Dhcp => self
                .ifs
                .get(&id)?
                .dhcp
                .lease
                .as_ref()
                .map(|lease| lease.addr),
        }
    }

    /// Primary and secondary addresses of an interface.
    pub(crate) fn addrs(&self, id: IfId) -> Vec<IfAddr> {
        let mut out: Vec<IfAddr> = self.addr(id).into_iter().collect();
        if let Some(cfg) = self.cfg.iface(id) {
            out.extend(cfg.secondary.iter().copied());
        }
        out
    }

    /// Up layer-3 interfaces with every address on them.
    pub(crate) fn l3_addrs(&self) -> Vec<(IfId, IfAddr)> {
        let mut out = Vec::new();
        for id in self.cfg.interfaces.keys() {
            if self.is_l3(*id) && self.is_up(*id) {
                out.extend(self.addrs(*id).into_iter().map(|addr| (*id, addr)));
            }
        }
        out
    }

    /// Is `ip` one of this node's own (up) interface addresses?
    pub(crate) fn owns_ip(&self, ip: Ipv4Addr) -> bool {
        self.l3_addrs().iter().any(|(_, addr)| addr.ip == ip)
    }

    /// Operational VLAN-carrying mode of a switch port.
    pub(crate) fn is_trunk(&self, id: IfId) -> bool {
        self.ifs.get(&id).is_some_and(|state| state.trunking)
    }

    /// The layer-2 port a physical switch port forwards as: itself, or the
    /// port-channel it is bundled into. `None` when it is not a (usable) bridge port.
    pub(crate) fn l2_port(&self, phys: IfId) -> Option<IfId> {
        let cfg = self.cfg.iface(phys)?;
        match cfg.channel_group {
            Some((group, _)) => {
                let po = IfId::port_channel(group);
                let bundled = self.ifs.get(&phys).is_some_and(|state| state.bundled);
                (bundled && self.cfg.iface(po).is_some_and(|c| c.l2.is_some())).then_some(po)
            }
            None => cfg.l2.is_some().then_some(phys),
        }
    }

    /// Physical members currently carrying traffic for a layer-2 port.
    pub(crate) fn members_of(&self, port: IfId) -> Vec<IfId> {
        if port.kind == IfKind::PortChannel {
            self.cfg
                .channel_members(port.number())
                .into_iter()
                .filter(|m| self.ifs.get(m).is_some_and(|s| s.bundled && s.up))
                .collect()
        } else {
            vec![port]
        }
    }

    /// Does a layer-2 port carry `vlan` (access VLAN, or allowed on the trunk)?
    pub(crate) fn port_carries(&self, port: IfId, vlan: u16) -> bool {
        let Some(sw) = self.cfg.iface(port).and_then(|cfg| cfg.l2.as_ref()) else {
            return false;
        };
        if self.is_trunk(port) {
            sw.allowed.contains(vlan)
        } else {
            sw.access_vlan == vlan || sw.voice_vlan == Some(vlan)
        }
    }

    /// Up layer-2 ports (physical switch ports outside channels, and port-channels).
    pub(crate) fn l2_ports(&self) -> Vec<IfId> {
        self.cfg
            .interfaces
            .iter()
            .filter(|(id, cfg)| {
                cfg.l2.is_some()
                    && cfg.channel_group.is_none()
                    && (id.is_physical() || id.kind == IfKind::PortChannel)
                    && self.is_up(**id)
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// The DNS server this node asks: configured, or learned by DHCP.
    pub(crate) fn dns_server(&self) -> Option<Ipv4Addr> {
        self.cfg
            .name_servers
            .first()
            .or(self.rib.dhcp_dns.first())
            .copied()
    }

    /// A fresh identifier for ICMP echo ids, ephemeral ports and transaction ids.
    pub(crate) fn fresh_ident(&mut self) -> u16 {
        self.next_ident = self.next_ident.wrapping_add(1);
        if self.next_ident == 0 {
            self.next_ident = 1;
        }
        self.next_ident
    }

    /// Device clock in milliseconds since the Unix epoch.
    pub(crate) fn clock_ms(&self, now: SimTime) -> i64 {
        let base = crate::util::clock::EPOCH_1993 * 1000
            + (now.saturating_sub(self.boot_time) / 1000) as i64;
        base.saturating_add(self.clock_offset_ms.unwrap_or(0))
    }

    pub(crate) fn uptime(&self, now: SimTime) -> SimTime {
        now.saturating_sub(self.boot_time)
    }
}
