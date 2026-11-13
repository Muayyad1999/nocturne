//! `show` commands for layer 2: VLANs, the MAC table, spanning tree,
//! EtherChannel and port security.

use std::fmt::Write as _;

use crate::api::SimTime;
use crate::device::cli::grammar::Args;
use crate::device::config::{StpMode, Violation, VtpMode};
use crate::ifname::{IfId, IfKind};
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::sim::stp::PortState;

/// Access ports of a VLAN, as `show vlan` lists them.
pub(crate) fn vlan_ports(node: &Node, vlan: u16) -> Vec<IfId> {
    node.cfg
        .interfaces
        .iter()
        .filter(|(id, cfg)| {
            id.is_physical()
                && cfg.channel_group.is_none()
                && !node.is_trunk(**id)
                && cfg.l2.as_ref().is_some_and(|sw| sw.access_vlan == vlan || sw.voice_vlan == Some(vlan))
        })
        .map(|(id, _)| *id)
        .collect()
}

pub(crate) fn vlan(node: &Node, brief: bool, only: Option<u16>) -> String {
    if let Some(id) = only
        && !node.cfg.vlans.contains_key(&id)
    {
        return format!("VLAN id {id} not found in current VLAN database\n");
    }
    let mut out = String::from("\nVLAN Name                             Status    Ports\n");
    out.push_str("---- -------------------------------- --------- -------------------------------\n");
    let wanted = |id: &u16| only.is_none_or(|o| o == *id);
    for (id, vlan) in node.cfg.vlans.iter().filter(|(id, _)| wanted(id)) {
        let status = if vlan.active { "active" } else { "suspended" };
        // Ports wrap so the column stays within 31 characters.
        let mut lines: Vec<String> = vec![String::new()];
        for port in vlan_ports(node, *id) {
            let name = port.short();
            match lines.last_mut() {
                Some(last) if last.is_empty() => last.push_str(&name),
                Some(last) if last.len() + 2 + name.len() <= 31 => {
                    last.push_str(", ");
                    last.push_str(&name);
                }
                _ => lines.push(name),
            }
        }
        let name = crate::util::text::clip(&vlan.name, 32);
        let _ = writeln!(out, "{id:<4} {name:<32} {status:<9} {}", lines[0]);
        for extra in &lines[1..] {
            let _ = writeln!(out, "{:<48}{extra}", "");
        }
    }
    if only.is_none() {
        for (id, name) in [(1002, "fddi-default"), (1003, "token-ring-default"), (1004, "fddinet-default"), (1005, "trnet-default")] {
            let _ = writeln!(out, "{id:<4} {name:<32} act/unsup ");
        }
    }
    if brief {
        return out;
    }
    out.push_str("\nVLAN Type  SAID       MTU   Parent RingNo BridgeNo Stp  BrdgMode Trans1 Trans2\n");
    out.push_str("---- ----- ---------- ----- ------ ------ -------- ---- -------- ------ ------\n");
    for id in node.cfg.vlans.keys().filter(|id| wanted(id)) {
        let _ = writeln!(out, "{id:<4} enet  {:<10} 1500  -      -      -        -    -        0      0   ", 100_000 + u32::from(*id));
    }
    if only.is_none() {
        out.push_str("1002 fddi  101002     1500  -      -      -        -    -        0      0   \n");
        out.push_str("1003 tr    101003     1500  -      -      -        -    -        0      0   \n");
        out.push_str("1004 fdnet 101004     1500  -      -      -        ieee -        0      0   \n");
        out.push_str("1005 trnet 101005     1500  -      -      -        ibm  -        0      0   \n");
    }
    out.push_str("\nRemote SPAN VLANs\n------------------------------------------------------------------------------\n\n\n");
    out.push_str("Primary Secondary Type              Ports\n------- --------- ----------------- ------------------------------------------\n\n");
    out
}

pub(crate) fn mac_table(node: &Node, now: SimTime, a: &Args) -> String {
    let (only_vlan, only_port, only_mac) = (a.num("vlan").map(|v| v as u16), a.iface("if"), a.mac("mac"));
    let mut out = String::from("          Mac Address Table\n-------------------------------------------\n\n");
    out.push_str("Vlan    Mac Address       Type        Ports\n----    -----------       --------    -----\n");
    let mut count = 0;
    for (vlan, mac, entry) in node.macs.rows(now) {
        let keep = only_vlan.is_none_or(|v| v == vlan)
            && only_port.is_none_or(|p| p == entry.port)
            && only_mac.is_none_or(|m| m == mac)
            && !(a.has("dynamic") && entry.is_static)
            && !(a.has("static") && !entry.is_static);
        if keep {
            count += 1;
            let kind = if entry.is_static { "STATIC" } else { "DYNAMIC" };
            let _ = writeln!(out, "{vlan:>4}    {mac}    {kind:<12}{}", entry.port.short());
        }
    }
    let _ = writeln!(out, "Total Mac Addresses for this criterion: {count}");
    out
}

fn vlan_label(vlan: u16) -> String {
    format!("VLAN{vlan:04}")
}

fn port_type(node: &Node, vlan: u16, port: IfId) -> String {
    let half = node.members_of(port).first().and_then(|m| node.ifs.get(m)).is_some_and(|s| !s.full_duplex);
    let edge = node.stp.vlans.get(&vlan).and_then(|v| v.ports.get(&port)).is_some_and(|p| p.edge);
    format!("{}{}", if half { "Shr" } else { "P2p" }, if edge { " Edge" } else { "" })
}

pub(crate) fn spanning_tree(node: &Node, fx: &Fx, only_vlan: Option<u16>, only_port: Option<IfId>) -> String {
    let timers = node.stp_timers(fx);
    let protocol = if node.cfg.stp.mode == StpMode::RapidPvst { "rstp" } else { "ieee" };
    // Rapid mode still advertises the classic forward delay.
    let (hello, max_age, forward) = (
        timers.hello / 1_000_000,
        timers.max_age / 1_000_000,
        if node.cfg.stp.mode == StpMode::RapidPvst { 15 } else { (timers.forward_delay / 1_000_000).max(1) },
    );
    let mut out = String::new();
    if let Some(vlan) = only_vlan
        && !node.stp.vlans.contains_key(&vlan)
    {
        return format!("Spanning tree instance(s) for vlan {vlan} does not exist.\n");
    }
    if let Some(port) = only_port {
        // One line per VLAN the port takes part in.
        out.push_str("\nVlan                Role Sts Cost      Prio.Nbr Type\n");
        out.push_str("------------------- ---- --- --------- -------- --------------------------------\n");
        let mut any = false;
        for (vlan, instance) in &node.stp.vlans {
            if let Some(p) = instance.ports.get(&port) {
                any = true;
                let id = node.stp_port_id(port);
                let _ = writeln!(
                    out,
                    "{:<19} {:<4} {:<3} {:<9} {:<8} {}",
                    vlan_label(*vlan),
                    p.role.code(),
                    p.state.code(),
                    node.stp_path_cost(port),
                    format!("{}.{}", id >> 8, id & 0xff),
                    port_type(node, *vlan, port)
                );
            }
        }
        return if any { out } else { "no spanning tree info available for this interface\n".to_string() };
    }
    for (vlan, instance) in node.stp.vlans.iter().filter(|(v, _)| only_vlan.is_none_or(|o| o == **v)) {
        let own = node.bridge_id(*vlan);
        let _ = writeln!(out, "\n{}", vlan_label(*vlan));
        let _ = writeln!(out, "  Spanning tree enabled protocol {protocol}");
        let _ = writeln!(out, "  Root ID    Priority    {}", instance.root.priority);
        let _ = writeln!(out, "             Address     {}", instance.root.mac);
        match instance.root_port {
            None => out.push_str("             This bridge is the root\n"),
            Some(port) => {
                let _ = writeln!(out, "             Cost        {}", instance.root_cost);
                let _ = writeln!(out, "             Port        {} ({})", node.stp_port_id(port) & 0xff, port.name());
            }
        }
        let _ = writeln!(out, "             Hello Time  {hello:>2} sec  Max Age {max_age} sec  Forward Delay {forward} sec\n");
        let _ = writeln!(
            out,
            "  Bridge ID  Priority    {}  (priority {} sys-id-ext {vlan})",
            own.priority,
            node.cfg.stp.priority_of(*vlan)
        );
        let _ = writeln!(out, "             Address     {}", own.mac);
        let _ = writeln!(out, "             Hello Time  {hello:>2} sec  Max Age {max_age} sec  Forward Delay {forward} sec");
        out.push_str("             Aging Time  300 sec\n\n");
        out.push_str("Interface           Role Sts Cost      Prio.Nbr Type\n");
        out.push_str("------------------- ---- --- --------- -------- --------------------------------\n");
        for (port, p) in &instance.ports {
            let id = node.stp_port_id(*port);
            let _ = writeln!(
                out,
                "{:<19} {:<4} {:<3} {:<9} {:<8} {}",
                port.short(),
                p.role.code(),
                p.state.code(),
                node.stp_path_cost(*port),
                format!("{}.{}", id >> 8, id & 0xff),
                port_type(node, *vlan, *port)
            );
        }
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("No spanning tree instance exists.\n");
    }
    out
}

pub(crate) fn spanning_tree_summary(node: &Node) -> String {
    let mode = if node.cfg.stp.mode == StpMode::RapidPvst { "rapid-pvst" } else { "pvst" };
    let roots: Vec<String> =
        node.stp.vlans.iter().filter(|(_, v)| v.root_port.is_none()).map(|(id, _)| vlan_label(*id)).collect();
    let word = |on: bool| if on { "enabled" } else { "disabled" };
    let mut out = format!("Switch is in {mode} mode\n");
    let _ = writeln!(out, "Root bridge for: {}", if roots.is_empty() { "none".to_string() } else { roots.join(", ") });
    out.push_str("Extended system ID           is enabled\n");
    let _ = writeln!(out, "Portfast Default             is {}", word(node.cfg.stp.portfast_default));
    let _ = writeln!(out, "PortFast BPDU Guard Default  is {}", word(node.cfg.stp.bpduguard_default));
    out.push_str("Portfast BPDU Filter Default is disabled\nLoopguard Default            is disabled\n");
    out.push_str("EtherChannel misconfig guard is enabled\nUplinkFast                   is disabled\n");
    out.push_str("BackboneFast                 is disabled\nConfigured Pathcost method used is short\n\n");
    out.push_str("Name                   Blocking Listening Learning Forwarding STP Active\n");
    out.push_str("---------------------- -------- --------- -------- ---------- ----------\n");
    let mut totals = [0usize; 4];
    for (vlan, instance) in &node.stp.vlans {
        let count = |state: PortState| instance.ports.values().filter(|p| p.state == state).count();
        let row = [
            count(PortState::Blocking),
            count(PortState::Listening),
            count(PortState::Learning),
            count(PortState::Forwarding),
        ];
        for (total, value) in totals.iter_mut().zip(row) {
            *total += value;
        }
        let _ = writeln!(
            out,
            "{:<22} {:>8} {:>9} {:>8} {:>10} {:>10}",
            vlan_label(*vlan),
            row[0],
            row[1],
            row[2],
            row[3],
            row.iter().sum::<usize>()
        );
    }
    out.push_str("---------------------- -------- --------- -------- ---------- ----------\n");
    let _ = writeln!(
        out,
        "{:<22} {:>8} {:>9} {:>8} {:>10} {:>10}",
        format!("{} vlans", node.stp.vlans.len()),
        totals[0],
        totals[1],
        totals[2],
        totals[3],
        totals.iter().sum::<usize>()
    );
    out
}

pub(crate) fn spanning_tree_root(node: &Node, fx: &Fx) -> String {
    let timers = node.stp_timers(fx);
    let mut out = String::from("\n                                        Root    Hello Max Fwd\n");
    out.push_str("Vlan                   Root ID          Cost    Time  Age Dly  Root Port\n");
    out.push_str("---------------- -------------------- --------- ----- --- ---  ------------\n");
    for (vlan, instance) in &node.stp.vlans {
        let row = format!(
            "{:<16} {:>5} {} {:>9} {:>4} {:>4} {:>3}  {}",
            vlan_label(*vlan),
            instance.root.priority,
            instance.root.mac,
            instance.root_cost,
            timers.hello / 1_000_000,
            timers.max_age / 1_000_000,
            (timers.forward_delay / 1_000_000).max(1),
            instance.root_port.map(|p| p.short()).unwrap_or_default()
        );
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

pub(crate) fn etherchannel_summary(node: &Node) -> String {
    let mut out = String::from(
        "Flags:  D - down        P - bundled in port-channel\n        I - stand-alone s - suspended\n        H - Hot-standby (LACP only)\n        R - Layer3      S - Layer2\n        U - in use      f - failed to allocate aggregator\n\n        M - not in use, minimum links not met\n        u - unsuitable for bundling\n        w - waiting to be aggregated\n        d - default port\n\n\n",
    );
    let groups: Vec<u32> =
        node.cfg.interfaces.keys().filter(|id| id.kind == IfKind::PortChannel).map(|id| id.number()).collect();
    let _ = writeln!(out, "Number of channel-groups in use: {}", groups.len());
    let _ = writeln!(out, "Number of aggregators:           {}\n", groups.len());
    out.push_str("Group  Port-channel  Protocol    Ports\n");
    out.push_str("------+-------------+-----------+-----------------------------------------------\n");
    for group in groups {
        let po = IfId::port_channel(group);
        let members = node.cfg.channel_members(group);
        let protocol = members
            .first()
            .and_then(|m| node.cfg.iface(*m))
            .and_then(|c| c.channel_group)
            .map_or("-", |(_, mode)| mode.protocol());
        let flags = if node.is_up(po) { "SU" } else { "SD" };
        let mut row = format!("{group:<7}{:<16}{protocol:^9} ", format!("{}({flags})", po.short()));
        for member in members {
            let state = node.ifs.get(&member);
            let flag = match state {
                Some(s) if s.bundled => "P",
                Some(s) if s.carrier => "s",
                _ => "D",
            };
            let _ = write!(row, "{:<12}", format!("{}({flag})", member.short()));
        }
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out.push('\n');
    out
}

fn secure_ports(node: &Node) -> Vec<IfId> {
    node.cfg
        .interfaces
        .iter()
        .filter(|(_, cfg)| cfg.l2.as_ref().is_some_and(|sw| sw.port_security.enabled))
        .map(|(id, _)| *id)
        .collect()
}

fn violation_word(violation: Violation) -> &'static str {
    match violation {
        Violation::Shutdown => "Shutdown",
        Violation::Restrict => "Restrict",
        Violation::Protect => "Protect",
    }
}

pub(crate) fn port_security(node: &Node) -> String {
    let mut out = String::from("Secure Port  MaxSecureAddr  CurrentAddr  SecurityViolation  Security Action\n");
    out.push_str("                (Count)       (Count)          (Count)\n");
    out.push_str("---------------------------------------------------------------------------\n");
    let mut total = 0usize;
    for port in secure_ports(node) {
        let (Some(sw), Some(state)) = (node.cfg.iface(port).and_then(|c| c.l2.as_ref()), node.ifs.get(&port)) else { continue };
        let ps = &sw.port_security;
        let current = ps.macs.len() + state.secure_macs.len();
        total += current.saturating_sub(1);
        let _ = writeln!(
            out,
            "{:>11} {:>14} {:>12} {:>18} {:>16}",
            port.short(),
            ps.maximum,
            current,
            state.security_violations,
            violation_word(ps.violation)
        );
    }
    out.push_str("---------------------------------------------------------------------------\n");
    let _ = writeln!(out, "Total Addresses in System (excluding one mac per port)     : {total}");
    out.push_str("Max Addresses limit in System (excluding one mac per port) : 4096\n");
    out
}

pub(crate) fn port_security_interface(node: &Node, port: IfId) -> Option<String> {
    let sw = node.cfg.iface(port)?.l2.as_ref()?;
    let state = node.ifs.get(&port)?;
    let ps = &sw.port_security;
    let status = match (ps.enabled, state.errdisabled.is_some(), state.up) {
        (false, _, _) => "Secure-down",
        (true, true, _) => "Secure-shutdown",
        (true, false, true) => "Secure-up",
        (true, false, false) => "Secure-down",
    };
    let sticky = ps.macs.iter().filter(|(_, sticky)| *sticky).count();
    let last = state.last_secure_src.map_or("0000.0000.0000:0".to_string(), |(mac, vlan)| format!("{mac}:{vlan}"));
    let mut out = String::new();
    let _ = writeln!(out, "Port Security              : {}", if ps.enabled { "Enabled" } else { "Disabled" });
    let _ = writeln!(out, "Port Status                : {status}");
    let _ = writeln!(out, "Violation Mode             : {}", violation_word(ps.violation));
    out.push_str("Aging Time                 : 0 mins\nAging Type                 : Absolute\n");
    out.push_str("SecureStatic Address Aging : Disabled\n");
    let _ = writeln!(out, "Maximum MAC Addresses      : {}", ps.maximum);
    let _ = writeln!(out, "Total MAC Addresses        : {}", ps.macs.len() + state.secure_macs.len());
    let _ = writeln!(out, "Configured MAC Addresses   : {}", ps.macs.len() - sticky);
    let _ = writeln!(out, "Sticky MAC Addresses       : {sticky}");
    let _ = writeln!(out, "Last Source Address:Vlan   : {last}");
    let _ = writeln!(out, "Security Violation Count   : {}", state.security_violations);
    Some(out)
}

pub(crate) fn port_security_address(node: &Node) -> String {
    let mut out = String::from("               Secure Mac Address Table\n");
    out.push_str("-----------------------------------------------------------------------------\n");
    out.push_str("Vlan    Mac Address       Type                          Ports   Remaining Age\n");
    out.push_str("                                                                   (mins)    \n");
    out.push_str("----    -----------       ----                          -----   -------------\n");
    let mut total = 0usize;
    for port in secure_ports(node) {
        let (Some(sw), Some(state)) = (node.cfg.iface(port).and_then(|c| c.l2.as_ref()), node.ifs.get(&port)) else { continue };
        let mut rows: Vec<(u16, String, &str)> = Vec::new();
        for (mac, sticky) in &sw.port_security.macs {
            rows.push((sw.access_vlan, mac.to_string(), if *sticky { "SecureSticky" } else { "SecureConfigured" }));
        }
        for (mac, vlan) in &state.secure_macs {
            rows.push((*vlan, mac.to_string(), "SecureDynamic"));
        }
        total += rows.len().saturating_sub(1);
        for (vlan, mac, kind) in rows {
            let _ = writeln!(out, "{vlan:>4}    {mac}    {kind:<30}{:<8}     -", port.short());
        }
    }
    out.push_str("-----------------------------------------------------------------------------\n");
    let _ = writeln!(out, "Total Addresses in System (excluding one mac per port)     : {total}");
    out.push_str("Max Addresses limit in System (excluding one mac per port) : 4096\n");
    out
}

pub(crate) fn errdisable_recovery(node: &Node) -> String {
    let mut out = String::from("ErrDisable Reason            Timer Status\n-----------------            --------------\n");
    for reason in ["arp-inspection", "bpduguard", "channel-misconfig (STP)", "dhcp-rate-limit", "link-flap", "psecure-violation", "security-violation", "udld"] {
        // Recovery timers are stored but not run: ports recover by shutdown / no shutdown.
        let _ = writeln!(out, "{reason:<29}Disabled");
    }
    out.push_str("\nTimer interval: 300 seconds\n\nInterfaces that will be enabled at the next timeout:\n\n");
    let disabled: Vec<String> =
        node.ifs.iter().filter_map(|(id, s)| s.errdisabled.map(|cause| format!("{} ({cause})", id.short()))).collect();
    if !disabled.is_empty() {
        let _ = writeln!(out, "Err-disabled now (manual recovery needed): {}", disabled.join(", "));
    }
    out
}

pub(crate) fn vtp_status(node: &Node) -> String {
    let mode = match node.cfg.vtp.mode {
        VtpMode::Server => "Server",
        VtpMode::Client => "Client",
        VtpMode::Transparent => "Transparent",
    };
    let mut out = String::new();
    out.push_str("VTP Version capable             : 1 to 3\nVTP version running             : 1\n");
    let _ = writeln!(out, "VTP Domain Name                 : {}", node.cfg.vtp.domain);
    out.push_str("VTP Pruning Mode                : Disabled\nVTP Traps Generation            : Disabled\n");
    let _ = writeln!(out, "Device ID                       : {}", node.base_mac);
    out.push_str("Configuration last modified by 0.0.0.0 at 0-0-00 00:00:00\n");
    out.push_str("Local updater ID is 0.0.0.0 (no valid interface found)\n\nFeature VLAN:\n--------------\n");
    let _ = writeln!(out, "VTP Operating Mode                : {mode}");
    out.push_str("Maximum VLANs supported locally   : 1005\n");
    let _ = writeln!(out, "Number of existing VLANs          : {}", node.cfg.vlans.len() + 4);
    out.push_str("Configuration Revision            : 0\n");
    out.push_str("MD5 digest                        : 0x57 0xCD 0x40 0x65 0x63 0x59 0x47 0xBD \n");
    out.push_str("                                    0x56 0x9D 0x4A 0x3E 0xA5 0x69 0x35 0xBC \n");
    out
}
