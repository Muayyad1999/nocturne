//! `show` commands for protocols and services: CDP/LLDP neighbours, DHCP, NAT.

use std::fmt::Write as _;

use crate::api::SimTime;
use crate::ifname::{IfId, IfKind};
use crate::sim::hsrp::{self, HsrpState, Peer};
use crate::sim::neighbors::{CDP_HOLD_S, CDP_INTERVAL_S};
use crate::sim::node::Node;
use crate::sim::ospf::{IfRole, LsaKind, NbrState, Neighbor, OspfIface};
use crate::sim::rip;
use crate::util::clock::{DateTime, hms};
use crate::util::text::clip;

/// `Gig 0/1`: the interface form used in the CDP neighbour table.
fn cdp_port(name: &str) -> String {
    match IfId::parse(name) {
        Some(id) => {
            let kind = match id.kind {
                IfKind::FastEthernet => "Fas",
                IfKind::GigabitEthernet => "Gig",
                IfKind::TenGigabitEthernet => "Ten",
                IfKind::Ethernet => "Eth",
                IfKind::PortChannel => "Por",
                IfKind::Loopback => "Loo",
                IfKind::Vlan => "Vla",
                IfKind::Dot11Radio => "Dot",
            };
            format!(
                "{kind} {}",
                id.short()
                    .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            )
        }
        None => name.to_string(),
    }
}

pub(crate) fn cdp(node: &Node) -> String {
    if !node.cfg.cdp_run {
        return "% CDP is not enabled\n".to_string();
    }
    format!(
        "Global CDP information:\n\tSending CDP packets every {CDP_INTERVAL_S} seconds\n\tSending a holdtime value of {CDP_HOLD_S} seconds\n\tSending CDPv2 advertisements is  enabled\n"
    )
}

pub(crate) fn cdp_neighbors(node: &Node, now: SimTime) -> String {
    if !node.cfg.cdp_run {
        return "% CDP is not enabled\n".to_string();
    }
    let mut out = String::from(
        "Capability Codes: R - Router, T - Trans Bridge, B - Source Route Bridge\n                  S - Switch, H - Host, I - IGMP, r - Repeater, P - Phone, \n                  D - Remote, C - CVTA, M - Two-port Mac Relay \n\n",
    );
    out.push_str("Device ID        Local Intrfce     Holdtme    Capability  Platform  Port ID\n");
    let entries = node.neighbors.entries(false, now);
    for n in &entries {
        let platform = n.advert.platform.trim_start_matches("Nocturne ");
        let rest = format!(
            "{:<18}{:<17}{:<6}{:<10}{}",
            cdp_port(&n.local.name()),
            n.hold_left(now),
            n.advert.capabilities,
            clip(platform, 9),
            cdp_port(&n.advert.port)
        );
        // A long device name gets a line of its own, as on real devices.
        if n.advert.device.len() >= 17 {
            let _ = writeln!(out, "{}\n{:<17}{rest}", n.advert.device, "");
        } else {
            let _ = writeln!(out, "{:<17}{rest}", n.advert.device);
        }
    }
    let _ = writeln!(out, "\nTotal cdp entries displayed : {}", entries.len());
    out
}

fn capability_words(codes: &str) -> String {
    codes
        .split_whitespace()
        .map(|code| match code {
            "R" => "Router",
            "S" => "Switch",
            "I" => "IGMP",
            "T" => "Trans-Bridge",
            "H" => "Host",
            other => other,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn cdp_detail(node: &Node, now: SimTime) -> String {
    if !node.cfg.cdp_run {
        return "% CDP is not enabled\n".to_string();
    }
    let mut out = String::new();
    for n in node.neighbors.entries(false, now) {
        let a = &n.advert;
        out.push_str("-------------------------\n");
        let _ = writeln!(out, "Device ID: {}", a.device);
        out.push_str("Entry address(es): \n");
        if let Some(ip) = a.mgmt_ip {
            let _ = writeln!(out, "  IP address: {ip}");
        }
        let _ = writeln!(
            out,
            "Platform: {},  Capabilities: {} ",
            a.platform,
            capability_words(&a.capabilities)
        );
        let _ = writeln!(
            out,
            "Interface: {},  Port ID (outgoing port): {}",
            n.local.name(),
            a.port
        );
        let _ = writeln!(out, "Holdtime : {} sec\n", n.hold_left(now));
        let _ = writeln!(out, "Version :\n{}\n", a.software);
        out.push_str("advertisement version: 2\n");
        if let Some(vlan) = a.native_vlan {
            let _ = writeln!(out, "Native VLAN: {vlan}");
        }
        let _ = writeln!(
            out,
            "Duplex: {}",
            if a.full_duplex { "full" } else { "half" }
        );
        if let Some(ip) = a.mgmt_ip {
            let _ = writeln!(out, "Management address(es): \n  IP address: {ip}");
        }
        out.push('\n');
    }
    if out.is_empty() {
        out.push('\n');
    }
    out
}

pub(crate) fn lldp_neighbors(node: &Node, now: SimTime) -> String {
    if !node.cfg.lldp_run {
        return "% LLDP is not enabled\n".to_string();
    }
    let mut out = String::from(
        "Capability codes:\n    (R) Router, (B) Bridge, (T) Telephone, (C) DOCSIS Cable Device\n    (W) WLAN Access Point, (P) Repeater, (S) Station, (O) Other\n\n",
    );
    out.push_str("Device ID           Local Intf     Hold-time  Capability      Port ID\n");
    let entries = node.neighbors.entries(true, now);
    for n in &entries {
        let _ = writeln!(
            out,
            "{:<20}{:<15}{:<11}{:<16}{}",
            clip(&n.advert.device, 19),
            n.local.short(),
            n.hold_left(now),
            n.advert.capabilities,
            n.advert.port
        );
    }
    let _ = writeln!(out, "\nTotal entries displayed: {}\n", entries.len());
    out
}

pub(crate) fn lldp_detail(node: &Node, now: SimTime) -> String {
    if !node.cfg.lldp_run {
        return "% LLDP is not enabled\n".to_string();
    }
    let entries = node.neighbors.entries(true, now);
    let mut out = String::new();
    for n in &entries {
        let a = &n.advert;
        out.push_str("------------------------------------------------\n");
        let _ = writeln!(out, "Local Intf: {}", n.local.short());
        let _ = writeln!(out, "Chassis id: {}", a.chassis);
        let _ = writeln!(out, "Port id: {}", a.port);
        let _ = writeln!(out, "Port Description: {}", a.port_description);
        let _ = writeln!(out, "System Name: {}\n", a.device);
        let _ = writeln!(out, "System Description: \n{}\n", a.software);
        let _ = writeln!(out, "Time remaining: {} seconds", n.hold_left(now));
        let _ = writeln!(out, "System Capabilities: {}", a.capabilities);
        let _ = writeln!(out, "Enabled Capabilities: {}", a.capabilities);
        out.push_str("Management Addresses:\n");
        match a.mgmt_ip {
            Some(ip) => {
                let _ = writeln!(out, "    IP: {ip}");
            }
            None => out.push_str("    - not advertised\n"),
        }
        out.push_str("Auto Negotiation - supported, enabled\nPhysical media capabilities:\n    1000baseT(FD)\n    100base-TX(FD)\n");
        out.push_str("Media Attachment Unit type: 30\n");
        match a.native_vlan {
            Some(vlan) => {
                let _ = writeln!(out, "Vlan ID: {vlan}");
            }
            None => out.push_str("Vlan ID: - not advertised\n"),
        }
        out.push('\n');
    }
    let _ = writeln!(out, "\nTotal entries displayed: {}\n", entries.len());
    out
}

/// Lease expiry as one word (`Mar-02-1993_12:00AM`): the public TextFSM
/// template for this command only accepts a single token there.
fn expiry(node: &Node, at: Option<SimTime>) -> String {
    match at {
        None => "Infinite".to_string(),
        Some(at) => {
            let t = DateTime::from_unix_ms(node.clock_ms(at));
            let (hour, half) = match t.hour {
                0 => (12, "AM"),
                1..=11 => (t.hour, "AM"),
                12 => (12, "PM"),
                _ => (t.hour - 12, "PM"),
            };
            format!(
                "{}-{:02}-{}_{hour:02}:{:02}{half}",
                t.month_name(),
                t.day,
                t.year,
                t.min
            )
        }
    }
}

/// Client identifier as IOS prints it: `01` followed by the MAC, in dotted groups.
fn client_id(mac: crate::util::mac::Mac) -> String {
    let hex: String = std::iter::once(1u8)
        .chain(mac.0)
        .map(|b| format!("{b:02x}"))
        .collect();
    hex.as_bytes()
        .chunks(4)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join(".")
}

pub(crate) fn dhcp_binding(node: &Node, now: SimTime) -> String {
    let mut out = String::from("Bindings from all pools not associated with VRF:\n");
    out.push_str("IP address          Client-ID/              Lease expiration        Type\n");
    out.push_str("                    Hardware address/\n                    User name\n");
    for binding in node.dhcp_bindings(now) {
        let _ = writeln!(
            out,
            "{:<20}{:<24}{:<24}Automatic",
            binding.ip.to_string(),
            client_id(binding.mac),
            expiry(node, binding.expires)
        );
    }
    out
}

pub(crate) fn dhcp_pool(node: &Node, now: SimTime) -> String {
    let mut out = String::new();
    for pool in &node.cfg.dhcp_pools {
        let leased = node
            .dhcp_bindings(now)
            .iter()
            .filter(|b| b.pool == pool.name)
            .count();
        let _ = writeln!(out, "Pool {} :", pool.name);
        out.push_str(
            " Utilization mark (high/low)    : 100 / 0\n Subnet size (first/next)       : 0 / 0 \n",
        );
        let total = pool
            .network
            .map_or(0, |n| (1u64 << (32 - u32::from(n.len))).saturating_sub(2));
        let _ = writeln!(out, " Total addresses                : {total}");
        let _ = writeln!(out, " Leased addresses               : {leased}");
        out.push_str(" Pending event                  : none\n");
        if let Some(network) = pool.network {
            out.push_str(" 1 subnet is currently in the pool :\n");
            out.push_str(
                " Current index        IP address range                    Leased addresses\n",
            );
            let first = crate::util::net::ip_add(network.net, 1).unwrap_or(network.net);
            let last = std::net::Ipv4Addr::from(u32::from(network.broadcast()).saturating_sub(1));
            let _ = writeln!(
                out,
                " {:<20} {:<16} - {:<16} {leased}",
                first.to_string(),
                first.to_string(),
                last.to_string()
            );
        }
    }
    out
}

pub(crate) fn nat_translations(node: &Node, now: SimTime) -> String {
    let rows = node.nat_table(now);
    if rows.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "Pro Inside global      Inside local       Outside local      Outside global\n",
    );
    for [
        proto,
        inside_global,
        inside_local,
        outside_local,
        outside_global,
    ] in rows
    {
        let _ = writeln!(
            out,
            "{proto} {inside_global:<18} {inside_local:<18} {outside_local:<18} {outside_global}"
        );
    }
    out
}

pub(crate) fn nat_statistics(node: &Node, now: SimTime) -> String {
    use crate::device::config::{NatSide, NatTarget};
    let dynamic = node.nat_table(now).len() - node.cfg.nat.statics.len();
    let statics = node.cfg.nat.statics.len();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Total active translations: {} ({statics} static, {dynamic} dynamic; {dynamic} extended)",
        statics + dynamic
    );
    for (title, side) in [
        ("Outside interfaces:", NatSide::Outside),
        ("Inside interfaces:", NatSide::Inside),
    ] {
        let _ = writeln!(out, "{title}");
        for (id, _) in node
            .cfg
            .interfaces
            .iter()
            .filter(|(_, c)| c.nat == Some(side))
        {
            let _ = writeln!(out, "  {}", id.name());
        }
    }
    let _ = writeln!(out, "Hits: {}  Misses: {}", node.nat.hits, node.nat.misses);
    let _ = writeln!(
        out,
        "CEF Translated packets: {}, CEF Punted packets: 0",
        node.nat.hits
    );
    let _ = writeln!(out, "Expired translations: {}", node.nat.expired);
    out.push_str("Dynamic mappings:\n-- Inside Source\n");
    for (index, rule) in node.cfg.nat.rules.iter().enumerate() {
        let target = match &rule.target {
            NatTarget::Interface(iface) => format!("interface {}", iface.name()),
            NatTarget::Pool(name) => format!("pool {name}"),
        };
        let _ = writeln!(
            out,
            "[Id: {}] access-list {} {target} refcount {dynamic}",
            index + 1,
            rule.acl
        );
    }
    for pool in &node.cfg.nat.pools {
        let _ = writeln!(
            out,
            " pool {}: netmask {}\n\tstart {} end {}",
            pool.name,
            crate::util::net::mask_from_len(pool.prefix_len),
            pool.start,
            pool.end
        );
    }
    out.push_str("\nTotal doors: 0\nAppl doors: 0\nNormal doors: 0\nQueued Packets: 0\n");
    out
}

// -------------------------------------------------------------------- OSPF

fn ospf_off() -> String {
    "%OSPF: No router process is configured\n".to_string()
}

/// `FULL/DR`, `2WAY/DROTHER`, ... as in the neighbour table.
fn neighbor_state(state: &OspfIface, neighbor: &Neighbor) -> String {
    format!(
        "{}/{}",
        neighbor.state.word(),
        state.neighbor_role(neighbor)
    )
}

fn dead_in(neighbor: &Neighbor, now: SimTime) -> String {
    hms(neighbor.dead_at.saturating_sub(now).div_ceil(1_000_000))
}

pub(crate) fn ospf_neighbors(node: &Node, now: SimTime, detail: bool) -> String {
    if node.cfg.ospf.is_none() {
        return ospf_off();
    }
    let mut out = String::new();
    if !detail {
        out.push_str(
            "\nNeighbor ID     Pri   State           Dead Time   Address         Interface\n",
        );
    }
    for (iface, state) in &node.ospf.ifaces {
        for (rid, n) in &state.neighbors {
            if !detail {
                let _ = writeln!(
                    out,
                    "{:<15} {:>3}   {:<15} {:<11} {:<15} {}",
                    rid.to_string(),
                    n.priority,
                    neighbor_state(state, n),
                    dead_in(n, now),
                    n.addr.to_string(),
                    iface.name()
                );
                continue;
            }
            let _ = writeln!(out, " Neighbor {rid}, interface address {}", n.addr);
            let _ = writeln!(
                out,
                "    In the area {} via interface {}",
                state.area,
                iface.name()
            );
            let _ = writeln!(
                out,
                "    Neighbor priority is {}, State is {}, {} state changes",
                n.priority,
                n.state.word(),
                n.changes
            );
            let _ = writeln!(out, "    DR is {} BDR is {}", state.dr, state.bdr);
            out.push_str("    Options is 0x52\n");
            let _ = writeln!(out, "    Dead timer due in {}", dead_in(n, now));
            if n.state == NbrState::Full {
                let _ = writeln!(
                    out,
                    "    Neighbor is up for {}",
                    hms(now.saturating_sub(n.since) / 1_000_000)
                );
            }
            out.push_str(
                "    Index 1/1, retransmission queue length 0, number of retransmission 0\n",
            );
            out.push_str("    First 0x0(0)/0x0(0) Next 0x0(0)/0x0(0)\n");
            out.push_str("    Last retransmission scan length is 0, maximum is 0\n");
            out.push_str("    Last retransmission scan time is 0 msec, maximum is 0 msec\n\n");
        }
    }
    out
}

pub(crate) fn ospf_interfaces(node: &Node, brief: bool, only: Option<IfId>) -> String {
    let Some(cfg) = &node.cfg.ospf else {
        return ospf_off();
    };
    if let Some(wanted) = only.filter(|id| !node.ospf.ifaces.contains_key(id)) {
        return format!("%OSPF: OSPF not enabled on {}\n", wanted.name());
    }
    let mut out = String::new();
    if brief {
        out.push_str(
            "Interface    PID   Area            IP Address/Mask    Cost  State Nbrs F/C\n",
        );
        for (iface, state) in &node.ospf.ifaces {
            let _ = writeln!(
                out,
                "{:<12} {:<5} {:<15} {:<18} {:<5} {:<5} {}/{}",
                iface.short(),
                cfg.pid,
                state.area.to_string(),
                state.addr.to_string(),
                state.cost,
                state.role.word(),
                state.full_neighbors(),
                state.neighbors.len()
            );
        }
        return out;
    }
    let rid = node
        .ospf
        .router_id
        .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED);
    for (iface, state) in node
        .ospf
        .ifaces
        .iter()
        .filter(|(id, _)| only.is_none_or(|wanted| wanted == **id))
    {
        let _ = writeln!(out, "{} is up, line protocol is up ", iface.name());
        let _ = writeln!(
            out,
            "  Internet Address {}, Area {}, Attached via {}",
            state.addr,
            state.area,
            if state.by_interface {
                "Interface Enable"
            } else {
                "Network Statement"
            }
        );
        let kind = if state.role == IfRole::Loopback {
            "LOOPBACK"
        } else {
            "BROADCAST"
        };
        let _ = writeln!(
            out,
            "  Process ID {}, Router ID {rid}, Network Type {kind}, Cost: {}",
            cfg.pid, state.cost
        );
        out.push_str("  Topology-MTID    Cost    Disabled    Shutdown      Topology Name\n");
        let _ = writeln!(
            out,
            "        0           {:<10}no          no            Base",
            state.cost
        );
        if state.role == IfRole::Loopback {
            out.push_str("  Loopback interface is treated as a stub Host\n");
            continue;
        }
        let role = match state.role {
            IfRole::Waiting => "WAITING",
            IfRole::Dr => "DR",
            IfRole::Backup => "BDR",
            _ => "DROTHER",
        };
        let _ = writeln!(
            out,
            "  Transmit Delay is 1 sec, State {role}, Priority {}",
            state.priority
        );
        // Name the routers behind the DR and BDR addresses.
        let owner = |addr: std::net::Ipv4Addr| {
            if addr == state.addr.ip {
                Some(rid)
            } else {
                state
                    .neighbors
                    .iter()
                    .find(|(_, n)| n.addr == addr)
                    .map(|(r, _)| *r)
            }
        };
        if let Some(dr) = owner(state.dr) {
            let _ = writeln!(
                out,
                "  Designated Router (ID) {dr}, Interface address {}",
                state.dr
            );
        }
        if let Some(bdr) = owner(state.bdr) {
            let _ = writeln!(
                out,
                "  Backup Designated router (ID) {bdr}, Interface address {}",
                state.bdr
            );
        }
        let _ = writeln!(
            out,
            "  Timer intervals configured, Hello {}, Dead {}, Wait {}, Retransmit 5",
            state.hello_s, state.dead_s, state.dead_s
        );
        if state.passive {
            out.push_str("    No Hellos (Passive interface) \n");
        } else {
            let _ = writeln!(out, "    oob-resync timeout {}", state.dead_s);
        }
        let _ = writeln!(
            out,
            "  Neighbor Count is {}, Adjacent neighbor count is {} ",
            state.neighbors.len(),
            state.full_neighbors()
        );
        for (nbr, n) in state
            .neighbors
            .iter()
            .filter(|(_, n)| n.state == NbrState::Full)
        {
            let tag = match state.neighbor_role(n) {
                "DR" => "  (Designated Router)",
                "BDR" => "  (Backup Designated Router)",
                _ => "",
            };
            let _ = writeln!(out, "    Adjacent with neighbor {nbr}{tag}");
        }
        out.push_str("  Suppress hello for 0 neighbor(s)\n");
    }
    out
}

pub(crate) fn ospf(node: &Node, now: SimTime) -> String {
    let (Some(cfg), Some(rid)) = (&node.cfg.ospf, node.ospf.router_id) else {
        return ospf_off();
    };
    let mut areas: Vec<crate::device::config::Area> =
        node.ospf.ifaces.values().map(|s| s.area).collect();
    areas.sort_by_key(|a| a.id);
    areas.dedup_by_key(|a| a.id);
    let externals = node
        .ospf
        .lsdb
        .iter()
        .filter(|l| l.kind == LsaKind::External)
        .count();
    let ago = |micros: SimTime| format!("{}.{:03}", hms(micros / 1_000_000), micros / 1000 % 1000);
    let mut out = String::new();
    let _ = writeln!(out, " Routing Process \"ospf {}\" with ID {rid}", cfg.pid);
    let _ = writeln!(
        out,
        " Start time: {}, Time elapsed: {}",
        ago(node.ospf.started.saturating_sub(node.boot_time)),
        ago(now.saturating_sub(node.ospf.started))
    );
    out.push_str(" Supports only single TOS(TOS0) routes\n Supports opaque LSA\n Supports Link-local Signaling (LLS)\n");
    if areas.len() > 1 && areas.iter().any(|a| a.id == 0) {
        out.push_str(" It is an area border router\n");
    }
    if cfg.redistribute_static || cfg.redistribute_connected || cfg.default_originate.is_some() {
        out.push_str(" It is an autonomous system boundary router\n");
        let mut sources = Vec::new();
        if cfg.redistribute_connected {
            sources.push("connected, includes subnets in redistribution");
        }
        if cfg.redistribute_static {
            sources.push("static, includes subnets in redistribution");
        }
        if !sources.is_empty() {
            let _ = writeln!(
                out,
                " Redistributing External Routes from,\n    {}",
                sources.join("\n    ")
            );
        }
    }
    out.push_str(" Router is not originating router-LSAs with maximum metric\n");
    out.push_str(" Initial SPF schedule delay 5000 msecs\n Minimum hold time between two consecutive SPFs 10000 msecs\n");
    out.push_str(" Maximum wait time between two consecutive SPFs 10000 msecs\n");
    let _ = writeln!(
        out,
        " Number of external LSA {externals}. Checksum Sum 0x000000"
    );
    let _ = writeln!(
        out,
        " Number of areas in this router is {}. {} normal 0 stub 0 nssa",
        areas.len(),
        areas.len()
    );
    let _ = writeln!(
        out,
        " Reference bandwidth unit is {} mbps",
        cfg.reference_bandwidth
    );
    for area in areas {
        let ifaces: Vec<_> = node
            .ospf
            .ifaces
            .values()
            .filter(|s| s.area.id == area.id)
            .collect();
        let loopbacks = ifaces.iter().filter(|s| s.role == IfRole::Loopback).count();
        if area.id == 0 {
            out.push_str("    Area BACKBONE(0)\n");
        } else {
            let _ = writeln!(out, "    Area {area}");
        }
        if loopbacks > 0 {
            let _ = writeln!(
                out,
                "        Number of interfaces in this area is {} ({loopbacks} loopback)",
                ifaces.len()
            );
        } else {
            let _ = writeln!(
                out,
                "        Number of interfaces in this area is {}",
                ifaces.len()
            );
        }
        out.push_str("        Area has no authentication\n");
        let _ = writeln!(
            out,
            "        SPF algorithm last executed {} ago",
            ago(now.saturating_sub(node.ospf.last_spf))
        );
        let _ = writeln!(
            out,
            "        SPF algorithm executed {} times",
            node.ospf.spf_runs
        );
        out.push_str("        Area ranges are\n");
        let lsas = node
            .ospf
            .lsdb
            .iter()
            .filter(|l| l.area == Some(area.id))
            .count();
        let _ = writeln!(out, "        Number of LSA {lsas}. Checksum Sum 0x000000");
    }
    out.push('\n');
    out
}

pub(crate) fn ospf_database(node: &Node, now: SimTime) -> String {
    let (Some(cfg), Some(rid)) = (&node.cfg.ospf, node.ospf.router_id) else {
        return ospf_off();
    };
    let mut out = format!(
        "\n            OSPF Router with ID ({rid}) (Process ID {})\n",
        cfg.pid
    );
    let age = (now.saturating_sub(node.ospf.last_spf) / 1_000_000).min(1800);
    let area_name = |id: u32| {
        node.ospf
            .ifaces
            .values()
            .find(|s| s.area.id == id)
            .map_or(id.to_string(), |s| s.area.to_string())
    };
    let mut section: Option<(Option<u32>, LsaKind)> = None;
    for lsa in &node.ospf.lsdb {
        if section != Some((lsa.area, lsa.kind)) {
            section = Some((lsa.area, lsa.kind));
            let title = match lsa.kind {
                LsaKind::Router => "Router Link States",
                LsaKind::Network => "Net Link States",
                LsaKind::Summary => "Summary Net Link States",
                LsaKind::SummaryAsbr => "Summary ASB Link States",
                LsaKind::External => "Type-5 AS External Link States",
            };
            match lsa.area {
                Some(area) => {
                    let _ = writeln!(out, "\n\t\t{title} (Area {})\n", area_name(area));
                }
                None => {
                    let _ = writeln!(out, "\n\t\t{title}\n");
                }
            }
            out.push_str(match lsa.kind {
                LsaKind::Router => {
                    "Link ID         ADV Router      Age         Seq#       Checksum Link count\n"
                }
                LsaKind::External => {
                    "Link ID         ADV Router      Age         Seq#       Checksum Tag\n"
                }
                _ => "Link ID         ADV Router      Age         Seq#       Checksum\n",
            });
        }
        // Checksums are synthetic but stable for a given LSA.
        let checksum = crate::util::rng::hash(&[
            u64::from(u32::from(lsa.link_id)),
            u64::from(u32::from(lsa.adv_router)),
            lsa.kind as u64,
        ]) & 0xffff;
        let tail = match lsa.kind {
            LsaKind::Router => format!(" {}", lsa.links),
            LsaKind::External => " 0".to_string(),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "{:<15} {:<15} {:<11} 0x{:08X} 0x{checksum:06X}{tail}",
            lsa.link_id.to_string(),
            lsa.adv_router.to_string(),
            age,
            0x8000_0000u32 + node.ospf.spf_runs.clamp(1, 0xffff)
        );
    }
    out.push('\n');
    out
}

/// The OSPF part of `show ip protocols`.
pub(crate) fn ospf_protocols(node: &Node, now: SimTime) -> String {
    let (Some(cfg), Some(rid)) = (&node.cfg.ospf, node.ospf.router_id) else {
        return String::new();
    };
    let mut out = format!("Routing Protocol is \"ospf {}\"\n", cfg.pid);
    out.push_str("  Outgoing update filter list for all interfaces is not set\n");
    out.push_str("  Incoming update filter list for all interfaces is not set\n");
    let _ = writeln!(out, "  Router ID {rid}");
    let areas: std::collections::BTreeSet<u32> =
        node.ospf.ifaces.values().map(|s| s.area.id).collect();
    if areas.len() > 1 && areas.contains(&0) {
        out.push_str("  It is an area border router\n");
    }
    let _ = writeln!(
        out,
        "  Number of areas in this router is {}. {} normal 0 stub 0 nssa",
        areas.len(),
        areas.len()
    );
    out.push_str("  Maximum path: 4\n  Routing for Networks:\n");
    for network in &cfg.networks {
        let _ = writeln!(
            out,
            "    {} {} area {}",
            network.addr, network.wildcard, network.area
        );
    }
    let by_interface: Vec<_> = node
        .ospf
        .ifaces
        .iter()
        .filter(|(_, s)| s.by_interface)
        .collect();
    if !by_interface.is_empty() {
        out.push_str("  Routing on Interfaces Configured Explicitly (Area ");
        let _ = writeln!(out, "{}):", by_interface[0].1.area);
        for (iface, _) in by_interface {
            let _ = writeln!(out, "    {}", iface.name());
        }
    }
    let passive: Vec<_> = node
        .ospf
        .ifaces
        .iter()
        .filter(|(_, s)| s.passive && s.role != IfRole::Loopback)
        .collect();
    if !passive.is_empty() {
        out.push_str("  Passive Interface(s):\n");
        for (iface, _) in passive {
            let _ = writeln!(out, "    {}", iface.name());
        }
    }
    out.push_str("  Routing Information Sources:\n    Gateway         Distance      Last Update\n");
    let mut sources: Vec<std::net::Ipv4Addr> = node
        .ospf
        .lsdb
        .iter()
        .map(|l| l.adv_router)
        .filter(|r| *r != rid)
        .collect();
    sources.sort();
    sources.dedup();
    for source in sources {
        let _ = writeln!(
            out,
            "    {:<20} 110      {}",
            source.to_string(),
            hms(now.saturating_sub(node.ospf.last_spf) / 1_000_000)
        );
    }
    out.push_str("  Distance: (default is 110)\n\n");
    out
}

// --------------------------------------------------------------------- RIP

/// The RIP part of `show ip protocols`.
pub(crate) fn rip_protocols(node: &Node, now: SimTime) -> String {
    let Some(cfg) = node.cfg.rip.as_ref().filter(|_| node.rip.running) else {
        return String::new();
    };
    let mut out = String::from("Routing Protocol is \"rip\"\n");
    out.push_str("  Outgoing update filter list for all interfaces is not set\n");
    out.push_str("  Incoming update filter list for all interfaces is not set\n");
    let due = node.rip.next_update.saturating_sub(now).div_ceil(1_000_000);
    let _ = writeln!(
        out,
        "  Sending updates every {} seconds, next due in {due} seconds",
        rip::UPDATE_S
    );
    let _ = writeln!(
        out,
        "  Invalid after {} seconds, hold down 180, flushed after {}",
        rip::INVALID_S,
        rip::FLUSH_S
    );
    out.push_str("  Redistributing: rip\n");
    let (send, receive, row) = match cfg.version {
        Some(2) => ("2", "version 2", "2     2"),
        Some(_) => ("1", "version 1", "1     1"),
        None => ("1", "any version", "1     1 2"),
    };
    let _ = writeln!(
        out,
        "  Default version control: send version {send}, receive {receive}"
    );
    out.push_str("    Interface             Send  Recv  Triggered RIP  Key-chain\n");
    for iface in node
        .rip
        .ifaces
        .keys()
        .filter(|id| id.kind != IfKind::Loopback && !cfg.is_passive(**id))
    {
        let _ = writeln!(out, "    {:<22}{row}", iface.name());
    }
    let summary = cfg.version != Some(2) || cfg.auto_summary;
    let _ = writeln!(
        out,
        "  Automatic network summarization is {}in effect",
        if summary { "" } else { "not " }
    );
    out.push_str("  Maximum path: 4\n  Routing for Networks:\n");
    let mut networks = cfg.networks.clone();
    networks.sort();
    for network in networks {
        let _ = writeln!(out, "    {network}");
    }
    let passive: Vec<&IfId> = node
        .rip
        .ifaces
        .keys()
        .filter(|id| cfg.is_passive(**id))
        .collect();
    if !passive.is_empty() {
        out.push_str("  Passive Interface(s):\n");
        for iface in passive {
            let _ = writeln!(out, "    {}", iface.name());
        }
    }
    out.push_str("  Routing Information Sources:\n    Gateway         Distance      Last Update\n");
    for (gateway, heard) in &node.rip.sources {
        let _ = writeln!(
            out,
            "    {:<20} 120      {}",
            gateway.to_string(),
            hms(now.saturating_sub(*heard) / 1_000_000)
        );
    }
    out.push_str("  Distance: (default is 120)\n\n");
    out
}

pub(crate) fn rip_database(node: &Node, now: SimTime) -> String {
    let Some(cfg) = node.cfg.rip.as_ref().filter(|_| node.rip.running) else {
        return String::new();
    };
    // Everything RIP knows, connected or learned, grouped under its major network.
    let mut lines: std::collections::BTreeMap<crate::util::net::Prefix, Vec<String>> =
        Default::default();
    for (iface, addr) in &node.rip.ifaces {
        lines
            .entry(addr.subnet())
            .or_default()
            .push(format!("    directly connected, {}", iface.name()));
    }
    for (prefix, entry) in &node.rip.learned {
        let rows = lines.entry(*prefix).or_default();
        rows.push(String::new());
        for path in &entry.paths {
            let age = hms(now.saturating_sub(path.heard) / 1_000_000);
            rows.push(format!(
                "    [{}] via {}, {age}, {}",
                entry.metric,
                path.via,
                path.iface.name()
            ));
        }
    }
    let summary = cfg.version != Some(2) || cfg.auto_summary;
    let mut out = String::new();
    let mut major = None;
    for (prefix, rows) in lines {
        let classful = crate::util::net::Prefix::classful(prefix.net);
        if summary && prefix.len > 0 && major != Some(classful) {
            major = Some(classful);
            let _ = writeln!(out, "{classful}    auto-summary");
        }
        match rows.split_first() {
            Some((first, rest)) if !first.is_empty() => {
                let _ = writeln!(out, "{prefix}{first}");
                for row in rest {
                    let _ = writeln!(out, "{row}");
                }
            }
            _ => {
                let _ = writeln!(out, "{prefix}");
                for row in rows.iter().filter(|r| !r.is_empty()) {
                    let _ = writeln!(out, "{row}");
                }
            }
        }
    }
    out
}

// -------------------------------------------------------------------- HSRP

/// `local`, an address, or `unknown`, as the brief table words a role holder.
fn hsrp_holder(own: bool, peer: Option<Peer>) -> String {
    match (own, peer) {
        (true, _) => "local".to_string(),
        (false, Some(peer)) => peer.ip.to_string(),
        (false, None) => "unknown".to_string(),
    }
}

/// Every configured group with its running state (`None` while it is not running).
fn hsrp_groups(
    node: &Node,
) -> Vec<(
    IfId,
    &crate::device::config::HsrpGroup,
    u8,
    Option<&hsrp::Group>,
)> {
    let mut out = Vec::new();
    for (iface, cfg) in &node.cfg.interfaces {
        for entry in &cfg.hsrp {
            out.push((
                *iface,
                entry,
                cfg.hsrp_version,
                node.hsrp.groups.get(&(*iface, entry.group)),
            ));
        }
    }
    out
}

pub(crate) fn standby_brief(node: &Node) -> String {
    let mut out = String::from(
        "                     P indicates configured to preempt.\n                     |\n",
    );
    out.push_str("Interface   Grp  Pri P State   Active          Standby         Virtual IP\n");
    for (iface, entry, _, group) in hsrp_groups(node) {
        let state = group.map_or("Init", |g| g.state.word());
        let _ = writeln!(
            out,
            "{:<11} {:<4} {:<3} {} {:<7} {:<15} {:<15} {}",
            iface.short(),
            entry.group,
            entry.priority,
            if entry.preempt { "P" } else { " " },
            state,
            hsrp_holder(
                group.is_some_and(|g| g.state == HsrpState::Active),
                group.and_then(|g| g.active)
            ),
            hsrp_holder(
                group.is_some_and(|g| g.state == HsrpState::Standby),
                group.and_then(|g| g.standby)
            ),
            entry
                .ip
                .map(|ip| ip.to_string())
                .unwrap_or_else(|| "unknown".to_string())
        );
    }
    out
}

pub(crate) fn standby(node: &Node, now: SimTime) -> String {
    let mut out = String::new();
    let secs = |at: SimTime| {
        let left = at.saturating_sub(now);
        format!("{}.{:03}", left / 1_000_000, left / 1000 % 1000)
    };
    for (iface, entry, version, group) in hsrp_groups(node) {
        let number = entry.group;
        let _ = writeln!(
            out,
            "{} - Group {number}{}",
            iface.name(),
            if version == 2 { " (version 2)" } else { "" }
        );
        let mac = hsrp::virtual_mac(version, number);
        let vip = entry
            .ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let Some(group) = group else {
            // Not running: say why, as far as the configuration tells.
            let why = if !node.is_up(iface) {
                "interface down"
            } else if entry.ip.is_none() {
                "virtual IP address not configured"
            } else if node.addr(iface).is_none() {
                "interface has no IP address"
            } else {
                "virtual IP in wrong subnet"
            };
            let _ = writeln!(out, "  State is Init ({why})");
            let _ = writeln!(out, "  Virtual IP address is {vip}");
            out.push_str("  Active virtual MAC address is unknown\n");
            let _ = writeln!(
                out,
                "    Local virtual MAC address is {mac} (v{version} default)"
            );
            let _ = writeln!(
                out,
                "  Hello time {} sec, hold time {} sec",
                hsrp::HELLO_S,
                hsrp::HOLD_S
            );
            let _ = writeln!(
                out,
                "  Preemption {}",
                if entry.preempt { "enabled" } else { "disabled" }
            );
            out.push_str("  Active router is unknown\n  Standby router is unknown\n");
            let _ = writeln!(
                out,
                "  Priority {} (configured {})",
                entry.priority, entry.priority
            );
            let _ = writeln!(
                out,
                "  Group name is \"hsrp-{}-{number}\" (default)",
                iface.short()
            );
            continue;
        };
        let _ = writeln!(out, "  State is {}", group.state.word());
        let _ = writeln!(
            out,
            "    {} state change{}, last state change {}",
            group.changes,
            if group.changes == 1 { "" } else { "s" },
            hms(now.saturating_sub(group.last_change) / 1_000_000)
        );
        let _ = writeln!(out, "  Virtual IP address is {vip}");
        if group.state == HsrpState::Active || group.active.is_some() {
            let _ = writeln!(out, "  Active virtual MAC address is {mac}");
        } else {
            out.push_str("  Active virtual MAC address is unknown\n");
        }
        let _ = writeln!(
            out,
            "    Local virtual MAC address is {mac} (v{version} default)"
        );
        let _ = writeln!(
            out,
            "  Hello time {} sec, hold time {} sec",
            hsrp::HELLO_S,
            hsrp::HOLD_S
        );
        if group.state >= HsrpState::Speak {
            let _ = writeln!(
                out,
                "    Next hello sent in {} secs",
                secs(group.next_hello)
            );
        }
        let _ = writeln!(
            out,
            "  Preemption {}",
            if entry.preempt { "enabled" } else { "disabled" }
        );
        let holder = |own: bool, peer: Option<Peer>, role: &str| match (own, peer) {
            (true, _) => format!("  {role} router is local\n"),
            (false, Some(peer)) => {
                format!(
                    "  {role} router is {}, priority {} (expires in {} sec)\n",
                    peer.ip,
                    peer.priority,
                    secs(peer.expires)
                )
            }
            (false, None) => format!("  {role} router is unknown\n"),
        };
        out.push_str(&holder(
            group.state == HsrpState::Active,
            group.active,
            "Active",
        ));
        out.push_str(&holder(
            group.state == HsrpState::Standby,
            group.standby,
            "Standby",
        ));
        let _ = writeln!(
            out,
            "  Priority {} (configured {})",
            entry.priority, entry.priority
        );
        let _ = writeln!(
            out,
            "  Group name is \"hsrp-{}-{number}\" (default)",
            iface.short()
        );
    }
    out
}
