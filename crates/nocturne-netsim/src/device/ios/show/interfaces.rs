//! `show interfaces ...` and `show ip interface ...`.

use std::fmt::Write as _;

use crate::api::SimTime;
use crate::device::config::{Duplex, IfConfig, L3Addr, SwMode};
use crate::ifname::{IfId, IfKind};
use crate::sim::node::Node;
use crate::util::clock::age_compact;
use crate::util::text::clip;

/// Hardware name as printed by `show interfaces`.
pub(crate) fn hardware(id: IfId) -> &'static str {
    match id.kind {
        IfKind::Loopback => "Loopback",
        IfKind::PortChannel => "EtherChannel",
        IfKind::Ethernet => "Ethernet",
        IfKind::FastEthernet => "Fast Ethernet",
        IfKind::GigabitEthernet => "Gigabit Ethernet",
        IfKind::TenGigabitEthernet => "Ten Gigabit Ethernet",
        IfKind::Dot11Radio => "802.11 Radio",
        IfKind::Vlan => "EtherSVI",
    }
}

fn media_type(id: IfId) -> &'static str {
    match id.kind {
        IfKind::FastEthernet => "10/100BaseTX",
        IfKind::GigabitEthernet | IfKind::Ethernet => "10/100/1000BaseTX",
        IfKind::TenGigabitEthernet => "SFP-10GBase-SR",
        _ => "",
    }
}

/// `("up" | "down" | "administratively down", "up" | "down")`.
pub(crate) fn status_words(node: &Node, id: IfId) -> (&'static str, &'static str) {
    let shutdown = node.cfg.iface(id).is_some_and(|c| c.shutdown);
    let up = node.is_up(id);
    match (shutdown, up) {
        (true, _) => ("administratively down", "down"),
        (false, true) => ("up", "up"),
        (false, false) => ("down", "down"),
    }
}

/// Switch-style connection state: connected, notconnect, disabled, err-disabled.
fn connect_state(node: &Node, id: IfId) -> &'static str {
    let Some(state) = node.ifs.get(&id) else { return "notconnect" };
    let cfg = node.cfg.iface(id);
    if state.errdisabled.is_some() {
        "err-disabled"
    } else if cfg.is_some_and(|c| c.shutdown) {
        "disabled"
    } else if state.up && cfg.is_some_and(|c| c.channel_group.is_some()) && !state.bundled {
        "suspended"
    } else if state.up {
        "connected"
    } else {
        "notconnect"
    }
}

pub(crate) fn ip_brief(node: &Node) -> String {
    let mut out = String::from("Interface                  IP-Address      OK? Method Status                Protocol\n");
    for (id, cfg) in &node.cfg.interfaces {
        let l3 = node.is_l3(*id);
        let (address, method) = match (l3, node.addr(*id), cfg.addr) {
            (true, Some(addr), Some(L3Addr::Dhcp)) => (addr.ip.to_string(), "DHCP"),
            (true, Some(addr), _) => (addr.ip.to_string(), "manual"),
            _ => ("unassigned".to_string(), "unset"),
        };
        let (status, proto) = status_words(node, *id);
        let _ = writeln!(out, "{:<27}{address:<16}YES {method:<7}{status:<22}{proto}", id.name());
    }
    out
}

fn duplex_speed_line(node: &Node, id: IfId, cfg: &IfConfig) -> String {
    let Some(state) = node.ifs.get(&id) else { return String::new() };
    let duplex = match (state.up, cfg.duplex) {
        (true, _) => if state.full_duplex { "Full-duplex" } else { "Half-duplex" },
        (false, Duplex::Full) => "Full-duplex",
        (false, Duplex::Half) => "Half-duplex",
        (false, Duplex::Auto) => "Auto-duplex",
    };
    let speed = match (state.up, cfg.speed) {
        (true, _) => format!("{}Mb/s", state.speed),
        (false, Some(speed)) => format!("{speed}Mb/s"),
        (false, None) => "Auto-speed".to_string(),
    };
    format!("{duplex}, {speed}, media type is {}", media_type(id))
}

fn one_interface(node: &Node, now: SimTime, id: IfId, cfg: &IfConfig) -> String {
    let Some(state) = node.ifs.get(&id) else { return String::new() };
    let mut out = String::new();
    let (status, proto) = status_words(node, id);
    // Switch ports add the connection state in parentheses.
    let bridge_port = node.model.is_switch() && (id.is_physical() || id.kind == IfKind::PortChannel);
    let suffix = if bridge_port { format!(" ({})", connect_state(node, id)) } else { String::new() };
    let _ = writeln!(out, "{} is {status}, line protocol is {proto}{suffix}", id.name());
    let mac = node.mac_for(id);
    if id.kind == IfKind::Loopback {
        out.push_str("  Hardware is Loopback\n");
    } else {
        let _ = writeln!(out, "  Hardware is {}, address is {mac} (bia {mac})", hardware(id));
    }
    if !cfg.description.is_empty() {
        let _ = writeln!(out, "  Description: {}", cfg.description);
    }
    if node.is_l3(id)
        && let Some(addr) = node.addr(id)
    {
        let _ = writeln!(out, "  Internet address is {addr}");
        for secondary in &cfg.secondary {
            let _ = writeln!(out, "  Secondary address {secondary}");
        }
    }
    let speed = match id.kind {
        IfKind::Loopback => 8_000_000,
        IfKind::Vlan => 1_000_000,
        _ if state.speed > 0 => u64::from(state.speed) * 1000,
        _ => u64::from(id.kind.max_mbps().max(1000)) * 1000,
    };
    let bandwidth = cfg.bandwidth.map_or(speed, u64::from);
    let (mtu, delay) = match id.kind {
        IfKind::Loopback => (1514, 5000),
        _ => (cfg.mtu.unwrap_or(1500), cfg.delay.map_or(if speed >= 1_000_000 { 10 } else if speed >= 100_000 { 100 } else { 1000 }, |d| d * 10)),
    };
    let _ = writeln!(out, "  MTU {mtu} bytes, BW {bandwidth} Kbit/sec, DLY {delay} usec,");
    let load = |rate: f64| ((rate / (bandwidth as f64 * 1000.0) * 255.0) as u32).clamp(1, 255);
    let _ = writeln!(
        out,
        "     reliability 255/255, txload {}/255, rxload {}/255",
        load(state.rate_out),
        load(state.rate_in)
    );
    match (id.kind, cfg.dot1q) {
        (IfKind::Loopback, _) => out.push_str("  Encapsulation LOOPBACK, loopback not set\n"),
        (_, Some((vlan, _))) => {
            let _ = writeln!(out, "  Encapsulation 802.1Q Virtual LAN, Vlan ID  {vlan}.");
        }
        _ => out.push_str("  Encapsulation ARPA, loopback not set\n"),
    }
    if id.kind == IfKind::Vlan {
        out.push_str("  Keepalive not supported\n");
    } else {
        out.push_str("  Keepalive set (10 sec)\n");
    }
    if id.is_physical() {
        let _ = writeln!(out, "  {}", duplex_speed_line(node, id, cfg));
        out.push_str("  input flow-control is off, output flow-control is unsupported\n");
    }
    if id.kind == IfKind::PortChannel {
        let members: Vec<String> = node.members_of(id).iter().map(|m| m.short()).collect();
        let _ = writeln!(out, "  Members in this channel: {}", members.join(" "));
    }
    if id.kind != IfKind::Loopback {
        out.push_str("  ARP type: ARPA, ARP Timeout 04:00:00\n");
    }
    let ago = |at: Option<SimTime>| at.map_or("never".to_string(), |t| age_compact(now.saturating_sub(t)));
    let c = &state.counters;
    let _ = writeln!(out, "  Last input {}, output {}, output hang never", ago(c.last_in), ago(c.last_out));
    out.push_str("  Last clearing of \"show interface\" counters never\n");
    let _ = writeln!(
        out,
        "  Input queue: 0/75/{}/0 (size/max/drops/flushes); Total output drops: {}",
        c.in_discards, c.out_discards
    );
    out.push_str("  Queueing strategy: fifo\n  Output queue: 0/40 (size/max)\n");
    let _ = writeln!(
        out,
        "  5 minute input rate {} bits/sec, {} packets/sec",
        state.rate_in as u64,
        (state.rate_in / 6400.0) as u64
    );
    let _ = writeln!(
        out,
        "  5 minute output rate {} bits/sec, {} packets/sec",
        state.rate_out as u64,
        (state.rate_out / 6400.0) as u64
    );
    let _ = writeln!(out, "     {} packets input, {} bytes, 0 no buffer", c.in_pkts, c.in_octets);
    let _ = writeln!(out, "     Received {} broadcasts (0 IP multicasts)", c.in_broadcasts);
    out.push_str("     0 runts, 0 giants, 0 throttles\n");
    let _ = writeln!(out, "     {} input errors, {} CRC, 0 frame, 0 overrun, 0 ignored", c.in_errors, c.crc);
    if id.is_physical() {
        let _ = writeln!(out, "     0 watchdog, {} multicast, 0 pause input", c.in_broadcasts);
        out.push_str("     0 input packets with dribble condition detected\n");
    }
    let _ = writeln!(out, "     {} packets output, {} bytes, 0 underruns", c.out_pkts, c.out_octets);
    let _ = writeln!(
        out,
        "     {} output errors, {} collisions, {} interface resets",
        c.out_errors, c.collisions, c.resets
    );
    if id.is_physical() {
        out.push_str("     0 unknown protocol drops\n");
        let _ = writeln!(out, "     0 babbles, {} late collision, 0 deferred", c.late_collisions);
        out.push_str("     0 lost carrier, 0 no carrier, 0 pause output\n");
    }
    out.push_str("     0 output buffer failures, 0 output buffers swapped out\n");
    out
}

/// `show interfaces [X]`; `None` if the named interface does not exist.
pub(crate) fn interfaces(node: &Node, now: SimTime, only: Option<IfId>) -> Option<String> {
    if let Some(id) = only {
        return node.cfg.iface(id).map(|cfg| one_interface(node, now, id, cfg));
    }
    Some(node.cfg.interfaces.iter().map(|(id, cfg)| one_interface(node, now, *id, cfg)).collect())
}

pub(crate) fn description(node: &Node) -> String {
    let mut out = String::from("Interface                      Status         Protocol Description\n");
    for (id, cfg) in &node.cfg.interfaces {
        let (status, proto) = status_words(node, *id);
        let status = if status == "administratively down" { "admin down" } else { status };
        let row = format!("{:<31}{status:<15}{proto:<9}{}", id.short(), cfg.description);
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

fn bridge_ports(node: &Node) -> Vec<(IfId, &IfConfig)> {
    node.cfg
        .interfaces
        .iter()
        .filter(|(id, _)| id.is_physical() || id.kind == IfKind::PortChannel)
        .map(|(id, cfg)| (*id, cfg))
        .collect()
}

pub(crate) fn status(node: &Node) -> String {
    let mut out = String::from("\nPort      Name               Status       Vlan       Duplex  Speed Type\n");
    for (id, cfg) in bridge_ports(node) {
        let Some(state) = node.ifs.get(&id) else { continue };
        let vlan = match &cfg.l2 {
            Some(_) if node.is_trunk(id) => "trunk".to_string(),
            Some(sw) => sw.access_vlan.to_string(),
            None if cfg.channel_group.is_some() => {
                // A channel member shows what its port-channel carries.
                let po = cfg.channel_group.map(|(g, _)| IfId::port_channel(g));
                match po.and_then(|po| node.cfg.iface(po)).and_then(|c| c.l2.as_ref()) {
                    Some(_) if po.is_some_and(|po| node.is_trunk(po)) => "trunk".to_string(),
                    Some(sw) => sw.access_vlan.to_string(),
                    None => "routed".to_string(),
                }
            }
            None => "routed".to_string(),
        };
        let duplex = match (state.up, cfg.duplex) {
            (true, Duplex::Auto) => if state.full_duplex { "a-full" } else { "a-half" },
            (false, Duplex::Auto) => "auto",
            (_, Duplex::Full) => "full",
            (_, Duplex::Half) => "half",
        };
        let speed = match (state.up, cfg.speed) {
            (true, None) => format!("a-{}", state.speed),
            (false, None) => "auto".to_string(),
            (_, Some(speed)) => speed.to_string(),
        };
        let row = format!(
            "{:<10}{:<19}{:<13}{vlan:<11}{duplex:>6} {speed:>6} {}",
            id.short(),
            clip(&cfg.description, 18),
            connect_state(node, id),
            media_type(id)
        );
        out.push_str(row.trim_end());
        out.push('\n');
    }
    out
}

pub(crate) fn trunk(node: &Node) -> String {
    let trunks: Vec<(IfId, &IfConfig)> = bridge_ports(node)
        .into_iter()
        .filter(|(id, cfg)| cfg.l2.is_some() && cfg.channel_group.is_none() && node.is_trunk(*id) && node.is_up(*id))
        .collect();
    if trunks.is_empty() {
        return "\n".to_string();
    }
    let mut out = String::from("\nPort        Mode             Encapsulation  Status        Native vlan\n");
    for (id, cfg) in &trunks {
        let Some(sw) = &cfg.l2 else { continue };
        let mode = match sw.mode {
            SwMode::Trunk => "on",
            SwMode::DynamicDesirable => "desirable",
            SwMode::DynamicAuto => "auto",
            SwMode::Access => "off",
        };
        let _ = writeln!(out, "{:<12}{mode:<17}{:<15}{:<14}{}", id.short(), "802.1q", "trunking", sw.native_vlan);
    }
    let active = |vlan: &u16| node.cfg.vlan_active(*vlan);
    let sections: [(&str, &dyn Fn(IfId, &crate::device::config::Switchport) -> String); 3] = [
        ("Vlans allowed on trunk", &|_, sw| if sw.allowed.is_all() { "1-4094".to_string() } else { sw.allowed.to_ranges() }),
        ("Vlans allowed and active in management domain", &|_, sw| {
            crate::util::text::ranges(&sw.allowed.iter().filter(active).collect::<Vec<_>>())
        }),
        ("Vlans in spanning tree forwarding state and not pruned", &|id, sw| {
            let forwarding: Vec<u16> = sw
                .allowed
                .iter()
                .filter(active)
                .filter(|v| node.stp.state(*v, id) == crate::sim::stp::PortState::Forwarding)
                .collect();
            crate::util::text::ranges(&forwarding)
        }),
    ];
    for (title, render) in sections {
        let _ = writeln!(out, "\nPort        {title}");
        for (id, cfg) in &trunks {
            let Some(sw) = &cfg.l2 else { continue };
            let list = render(*id, sw);
            let _ = writeln!(out, "{:<12}{}", id.short(), if list.is_empty() { "none".to_string() } else { list });
        }
    }
    out
}

pub(crate) fn switchport(node: &Node, only: Option<IfId>) -> String {
    let mut out = String::new();
    for (id, cfg) in bridge_ports(node).into_iter().filter(|(id, _)| only.is_none_or(|o| o == *id)) {
        let _ = writeln!(out, "Name: {}", id.short());
        let Some(sw) = &cfg.l2 else {
            out.push_str("Switchport: Disabled\n\n");
            continue;
        };
        let up = node.is_up(id);
        let trunking = node.is_trunk(id);
        let admin = match sw.mode {
            SwMode::Access => "static access",
            SwMode::Trunk => "trunk",
            SwMode::DynamicAuto => "dynamic auto",
            SwMode::DynamicDesirable => "dynamic desirable",
        };
        let oper = match (up, trunking) {
            (false, _) => "down",
            (true, true) => "trunk",
            (true, false) => "static access",
        };
        let vlan_name = |vlan: u16| match node.cfg.vlans.get(&vlan) {
            Some(v) if v.active => v.name.clone(),
            _ => "Inactive".to_string(),
        };
        out.push_str("Switchport: Enabled\n");
        let _ = writeln!(out, "Administrative Mode: {admin}");
        let _ = writeln!(out, "Operational Mode: {oper}");
        out.push_str("Administrative Trunking Encapsulation: dot1q\n");
        let _ = writeln!(out, "Operational Trunking Encapsulation: {}", if up && trunking { "dot1q" } else { "native" });
        let negotiation = !sw.nonegotiate && sw.mode != SwMode::Access;
        let _ = writeln!(out, "Negotiation of Trunking: {}", if negotiation { "On" } else { "Off" });
        let _ = writeln!(out, "Access Mode VLAN: {} ({})", sw.access_vlan, vlan_name(sw.access_vlan));
        let _ = writeln!(out, "Trunking Native Mode VLAN: {} ({})", sw.native_vlan, vlan_name(sw.native_vlan));
        out.push_str("Administrative Native VLAN tagging: enabled\n");
        match sw.voice_vlan {
            Some(vlan) => {
                let _ = writeln!(out, "Voice VLAN: {vlan} ({})", vlan_name(vlan));
            }
            None => out.push_str("Voice VLAN: none\n"),
        }
        out.push_str("Administrative private-vlan host-association: none\n");
        out.push_str("Administrative private-vlan mapping: none\n");
        out.push_str("Administrative private-vlan trunk native VLAN: none\n");
        out.push_str("Administrative private-vlan trunk Native VLAN tagging: enabled\n");
        out.push_str("Administrative private-vlan trunk encapsulation: dot1q\n");
        out.push_str("Administrative private-vlan trunk normal VLANs: none\n");
        out.push_str("Administrative private-vlan trunk associations: none\n");
        out.push_str("Administrative private-vlan trunk mappings: none\n");
        out.push_str("Operational private-vlan: none\n");
        let allowed = if sw.allowed.is_all() {
            "ALL".to_string()
        } else if sw.allowed.is_empty() {
            "NONE".to_string()
        } else {
            sw.allowed.to_ranges()
        };
        let _ = writeln!(out, "Trunking VLANs Enabled: {allowed}");
        out.push_str("Pruning VLANs Enabled: 2-1001\nCapture Mode Disabled\nCapture VLANs Allowed: ALL\n\n");
        out.push_str("Protected: false\nUnknown unicast blocked: disabled\nUnknown multicast blocked: disabled\n");
        out.push_str("Appliance trust: none\n\n");
    }
    out
}

pub(crate) fn counters(node: &Node, errors: bool) -> String {
    let ports = bridge_ports(node);
    let mut out = String::new();
    if errors {
        out.push_str("\nPort        Align-Err     FCS-Err    Xmit-Err     Rcv-Err  UnderSize  OutDiscards\n");
        for (id, _) in &ports {
            let Some(c) = node.ifs.get(id).map(|s| &s.counters) else { continue };
            let _ = writeln!(out, "{:<10}{:>11}{:>12}{:>12}{:>12}{:>11}{:>13}", id.short(), 0, c.crc, c.out_errors, c.in_errors, 0, c.out_discards);
        }
        out.push_str("\nPort      Single-Col  Multi-Col   Late-Col  Excess-Col  Carri-Sen      Runts     Giants\n");
        for (id, _) in &ports {
            let Some(c) = node.ifs.get(id).map(|s| &s.counters) else { continue };
            let _ = writeln!(out, "{:<10}{:>10}{:>11}{:>11}{:>12}{:>11}{:>11}{:>11}", id.short(), 0, 0, c.late_collisions, 0, 0, 0, 0);
        }
        return out;
    }
    out.push_str("\nPort            InOctets    InUcastPkts    InMcastPkts    InBcastPkts\n");
    for (id, _) in &ports {
        let Some(c) = node.ifs.get(id).map(|s| &s.counters) else { continue };
        let unicast = c.in_pkts.saturating_sub(c.in_broadcasts);
        let _ = writeln!(out, "{:<10}{:>14}{:>15}{:>15}{:>15}", id.short(), c.in_octets, unicast, 0, c.in_broadcasts);
    }
    out.push_str("\nPort           OutOctets   OutUcastPkts   OutMcastPkts   OutBcastPkts\n");
    for (id, _) in &ports {
        let Some(c) = node.ifs.get(id).map(|s| &s.counters) else { continue };
        let _ = writeln!(out, "{:<10}{:>14}{:>15}{:>15}{:>15}", id.short(), c.out_octets, c.out_pkts, 0, 0);
    }
    out
}

fn one_ip_interface(node: &Node, id: IfId, cfg: &IfConfig) -> String {
    let (status, proto) = status_words(node, id);
    let mut out = format!("{} is {status}, line protocol is {proto}\n", id.name());
    let Some(addr) = node.addr(id).filter(|_| node.is_l3(id)) else {
        out.push_str("  Internet protocol processing disabled\n");
        return out;
    };
    let _ = writeln!(out, "  Internet address is {addr}");
    let _ = writeln!(out, "  Broadcast address is 255.255.255.255");
    if cfg.addr == Some(L3Addr::Dhcp) {
        out.push_str("  Address determined by DHCP\n");
    } else {
        out.push_str("  Address determined by setup command\n");
    }
    let _ = writeln!(out, "  MTU is {} bytes", cfg.mtu.unwrap_or(1500));
    match cfg.helpers.as_slice() {
        [] => out.push_str("  Helper address is not set\n"),
        [one] => {
            let _ = writeln!(out, "  Helper address is {one}");
        }
        many => {
            let list: Vec<String> = many.iter().map(|h| h.to_string()).collect();
            let _ = writeln!(out, "  Helper addresses are {}", list.join("\n                       "));
        }
    }
    out.push_str("  Directed broadcast forwarding is disabled\n");
    for secondary in &cfg.secondary {
        let _ = writeln!(out, "  Secondary address {secondary}");
    }
    let acl = |name: &Option<String>| name.clone().unwrap_or_else(|| "not set".to_string());
    let _ = writeln!(out, "  Outgoing access list is {}", acl(&cfg.acl_out));
    let _ = writeln!(out, "  Inbound  access list is {}", acl(&cfg.acl_in));
    out.push_str("  Proxy ARP is disabled\n  Local Proxy ARP is disabled\n  Security level is default\n");
    out.push_str("  Split horizon is enabled\n  ICMP redirects are always sent\n  ICMP unreachables are always sent\n");
    out.push_str("  ICMP mask replies are never sent\n  IP fast switching is enabled\n  IP CEF switching is enabled\n");
    match cfg.nat {
        Some(crate::device::config::NatSide::Inside) => out.push_str("  Network address translation is enabled, interface in domain inside\n"),
        Some(crate::device::config::NatSide::Outside) => out.push_str("  Network address translation is enabled, interface in domain outside\n"),
        None => out.push_str("  Network address translation is disabled\n"),
    }
    out
}

/// `show ip interface [X]`; `None` if the named interface does not exist.
pub(crate) fn ip_interface(node: &Node, only: Option<IfId>) -> Option<String> {
    if let Some(id) = only {
        return node.cfg.iface(id).map(|cfg| one_ip_interface(node, id, cfg));
    }
    Some(node.cfg.interfaces.iter().map(|(id, cfg)| one_ip_interface(node, *id, cfg)).collect())
}
