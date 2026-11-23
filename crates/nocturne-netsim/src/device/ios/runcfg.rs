//! Rendering the configuration model as an IOS-style running configuration.
//!
//! Every line printed here must be accepted by the configuration grammar, and
//! applying the output to a factory-fresh device must reproduce the same model:
//! that round trip is what `write memory` + reload, lab save/load and the
//! change pipeline all rely on.

use std::fmt::Write as _;

use crate::catalog::Role;
use crate::device::config::{
    AclItem, ClassMatch, DeviceConfig, Duplex, IfConfig, L3Addr, Lease, LineConfig, LoginMode,
    NatSide, NatTarget, PolicyClass, PolicyMap, Secret, Share, Slot, StpMode, SwMode, Switchport,
    Timestamps, Violation, VtpMode,
};
use crate::ifname::{IfId, IfKind};
use crate::sim::acl::rule_text;
use crate::sim::node::Node;
use crate::util::dscp;
use crate::util::text::ranges;

fn secret_text(secret: &Secret) -> String {
    match secret {
        Secret::Plain(text) => text.clone(),
        Secret::Md5(hash) => format!("5 {hash}"),
        Secret::Type7(text) => format!("7 {text}"),
    }
}

/// `secret 5 $1$...` or `password 7 ...` / `password plain`.
pub(crate) fn credential(secret: &Secret) -> String {
    match secret {
        Secret::Md5(_) => format!("secret {}", secret_text(secret)),
        other => format!("password {}", secret_text(other)),
    }
}

fn timestamps(kind: &str, value: Timestamps) -> String {
    match value {
        Timestamps::Off => format!("no service timestamps {kind}"),
        Timestamps::Uptime => format!("service timestamps {kind} uptime"),
        Timestamps::Datetime { msec: true } => format!("service timestamps {kind} datetime msec"),
        Timestamps::Datetime { msec: false } => format!("service timestamps {kind} datetime"),
    }
}

/// A class map's `match` statement, as typed.
pub(crate) fn match_text(statement: &ClassMatch) -> String {
    match statement {
        ClassMatch::Dscp { values, ip } => {
            let names: Vec<String> = values.iter().map(|v| dscp::config_text(*v)).collect();
            format!(
                "match {}dscp {}",
                if *ip { "ip " } else { "" },
                names.join(" ")
            )
        }
        ClassMatch::AccessGroup { acl, named: true } => format!("match access-group name {acl}"),
        ClassMatch::AccessGroup { acl, named: false } => format!("match access-group {acl}"),
        ClassMatch::Any => "match any".to_string(),
    }
}

fn share_text(share: Share) -> String {
    match share {
        Share::Kbps(kbps) => kbps.to_string(),
        Share::Percent(percent) => format!("percent {percent}"),
    }
}

/// The actions of one class of a policy map, as typed.
pub(crate) fn class_lines(class: &PolicyClass) -> Vec<String> {
    let mut out = Vec::new();
    match class.priority {
        Some(Some(share)) => out.push(format!("priority {}", share_text(share))),
        Some(None) => out.push("priority".to_string()),
        None => {}
    }
    if let Some(share) = class.bandwidth {
        out.push(format!("bandwidth {}", share_text(share)));
    }
    if let Some(police) = class.police {
        out.push(format!(
            "police {}{}{}",
            if police.cir_keyword { "cir " } else { "" },
            police.cir,
            if police.actions {
                " conform-action transmit exceed-action drop"
            } else {
                ""
            }
        ));
    }
    if let Some(rate) = class.shape {
        out.push(format!("shape average {rate}"));
    }
    if let Some(value) = class.set_dscp {
        out.push(format!("set dscp {}", dscp::config_text(value)));
    }
    if let Some(child) = &class.child {
        out.push(format!("service-policy {child}"));
    }
    out
}

/// Policy maps in an order that can be typed back in: a policy applied
/// inside another one comes just before it, because a device refuses to
/// reference a policy map it has not seen yet.
pub(crate) fn policy_order(cfg: &DeviceConfig) -> Vec<&PolicyMap> {
    fn place<'c>(
        cfg: &'c DeviceConfig,
        map: &'c PolicyMap,
        out: &mut Vec<&'c PolicyMap>,
        budget: usize,
    ) {
        if out.iter().any(|done| done.name == map.name) {
            return;
        }
        // Policies cannot contain each other, so the budget is never spent;
        // it is there so that a damaged model prints instead of recursing.
        if budget > 0 {
            for child in map.classes.iter().filter_map(|c| c.child.as_deref()) {
                if let Some(child) = cfg.policy_map(child) {
                    place(cfg, child, out, budget - 1);
                }
            }
        }
        if !out.iter().any(|done| done.name == map.name) {
            out.push(map);
        }
    }
    let mut out = Vec::new();
    for map in &cfg.policy_maps {
        place(cfg, map, &mut out, crate::device::config::POLICY_DEPTH + 1);
    }
    out
}

/// The body of the running configuration, from the first `!` to `end`.
pub(crate) fn render(node: &Node) -> String {
    let cfg = &node.cfg;
    let role = node.model.role;
    let mut out = String::new();
    let mut line = |text: &str| {
        out.push_str(text);
        out.push('\n');
    };
    let version: String = node
        .model
        .version
        .split('(')
        .next()
        .unwrap_or("15.0")
        .to_string();
    line("!");
    line(&format!("version {version}"));
    line(&timestamps("debug", cfg.services.timestamps_debug));
    line(&timestamps("log", cfg.services.timestamps_log));
    line(if cfg.services.password_encryption {
        "service password-encryption"
    } else {
        "no service password-encryption"
    });
    line("!");
    line(&format!("hostname {}", cfg.hostname));
    line("!");
    line("boot-start-marker");
    line("boot-end-marker");
    line("!");
    if let Some(size) = cfg.logging.buffered {
        line(&format!("logging buffered {size}"));
    }
    if !cfg.logging.console {
        line("no logging console");
    }
    if let Some(secret) = &cfg.enable_secret {
        line(&format!("enable secret {}", secret_text(secret)));
    }
    if let Some(password) = &cfg.enable_password {
        line(&format!("enable password {}", secret_text(password)));
    }
    line("!");
    for user in &cfg.users {
        let privilege = if user.privilege == 1 {
            String::new()
        } else {
            format!(" privilege {}", user.privilege)
        };
        match &user.secret {
            Some(secret) => line(&format!(
                "username {}{privilege} {}",
                user.name,
                credential(secret)
            )),
            None => line(&format!("username {}{privilege}", user.name)),
        }
    }
    let aaa: Vec<&String> = cfg
        .stored
        .iter()
        .filter(|(slot, _)| *slot == Slot::Aaa)
        .map(|(_, l)| l)
        .collect();
    if aaa.is_empty() {
        line("no aaa new-model");
    }
    for stored in aaa {
        line(stored);
    }
    if let Some(zone) = &cfg.clock_timezone {
        line(&format!(
            "clock timezone {} {} {}",
            zone.name, zone.hours, zone.minutes
        ));
    }
    line("!");
    match (role, cfg.ip_routing) {
        (Role::Router, false) => line("no ip routing"),
        (Role::L3Switch, true) => line("ip routing"),
        _ => {}
    }
    if !cfg.domain_lookup {
        line("no ip domain-lookup");
    }
    if let Some(name) = &cfg.domain_name {
        line(&format!("ip domain-name {name}"));
    }
    if !cfg.name_servers.is_empty() {
        let servers: Vec<String> = cfg.name_servers.iter().map(|s| s.to_string()).collect();
        line(&format!("ip name-server {}", servers.join(" ")));
    }
    for (low, high) in &cfg.dhcp_excluded {
        if low == high {
            line(&format!("ip dhcp excluded-address {low}"));
        } else {
            line(&format!("ip dhcp excluded-address {low} {high}"));
        }
    }
    for (_, stored) in cfg.stored.iter().filter(|(slot, _)| *slot == Slot::IpEarly) {
        line(stored);
    }
    line("!");
    for pool in &cfg.dhcp_pools {
        line(&format!("ip dhcp pool {}", pool.name));
        if let Some(network) = pool.network {
            line(&format!(" network {} {}", network.net, network.mask()));
        }
        let list = |addrs: &[std::net::Ipv4Addr]| {
            addrs
                .iter()
                .map(|a| a.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        };
        if !pool.default_routers.is_empty() {
            line(&format!(" default-router {}", list(&pool.default_routers)));
        }
        if !pool.dns_servers.is_empty() {
            line(&format!(" dns-server {}", list(&pool.dns_servers)));
        }
        if let Some(domain) = &pool.domain {
            line(&format!(" domain-name {domain}"));
        }
        match pool.lease {
            Some(Lease::Infinite) => line(" lease infinite"),
            Some(Lease::Time(d, 0, 0)) => line(&format!(" lease {d}")),
            Some(Lease::Time(d, h, 0)) => line(&format!(" lease {d} {h}")),
            Some(Lease::Time(d, h, m)) => line(&format!(" lease {d} {h} {m}")),
            None => {}
        }
        line("!");
    }
    if node.model.is_switch() {
        if !cfg.vtp.domain.is_empty() {
            line(&format!("vtp domain {}", cfg.vtp.domain));
        }
        match cfg.vtp.mode {
            VtpMode::Server => {}
            VtpMode::Client => line("vtp mode client"),
            VtpMode::Transparent => line("vtp mode transparent"),
        }
    }
    if cfg.lldp_run {
        line("lldp run");
    }
    if !cfg.cdp_run {
        line("no cdp run");
    }
    for (_, stored) in cfg
        .stored
        .iter()
        .filter(|(slot, _)| *slot == Slot::Errdisable)
    {
        line(stored);
    }
    for (_, stored) in cfg.stored.iter().filter(|(slot, _)| *slot == Slot::Archive) {
        line(stored);
    }
    line("!");
    if node.model.is_switch() {
        line(match cfg.stp.mode {
            StpMode::Pvst => "spanning-tree mode pvst",
            StpMode::RapidPvst => "spanning-tree mode rapid-pvst",
        });
        if cfg.stp.portfast_default {
            line("spanning-tree portfast default");
        }
        if cfg.stp.bpduguard_default {
            line("spanning-tree portfast bpduguard default");
        }
        line("spanning-tree extend system-id");
        // VLANs sharing a priority are grouped on one line.
        let mut priorities: Vec<u16> = cfg.stp.priority.values().copied().collect();
        priorities.sort_unstable();
        priorities.dedup();
        for priority in priorities {
            let vlans: Vec<u16> = cfg
                .stp
                .priority
                .iter()
                .filter(|(_, p)| **p == priority)
                .map(|(v, _)| *v)
                .collect();
            line(&format!(
                "spanning-tree vlan {} priority {priority}",
                ranges(&vlans)
            ));
        }
        line("!");
        if cfg.vtp.mode == VtpMode::Transparent {
            for (id, vlan) in cfg
                .vlans
                .iter()
                .filter(|(id, _)| **id != 1 && !(1002..=1005).contains(*id))
            {
                line(&format!("vlan {id}"));
                if vlan.name != crate::device::config::Vlan::default_name(*id) {
                    line(&format!(" name {}", vlan.name));
                }
                if !vlan.active {
                    line(" state suspend");
                }
                line("!");
            }
        }
    }
    for map in &cfg.class_maps {
        line(&format!(
            "class-map {} {}",
            if map.any { "match-any" } else { "match-all" },
            map.name
        ));
        if !map.description.is_empty() {
            line(&format!(" description {}", map.description));
        }
        for statement in &map.matches {
            line(&format!(" {}", match_text(statement)));
        }
    }
    if !cfg.class_maps.is_empty() {
        line("!");
    }
    for map in policy_order(cfg) {
        line(&format!("policy-map {}", map.name));
        if !map.description.is_empty() {
            line(&format!(" description {}", map.description));
        }
        for class in &map.classes {
            line(&format!(" class {}", class.name));
            for text in class_lines(class) {
                line(&format!("  {text}"));
            }
        }
    }
    if !cfg.policy_maps.is_empty() {
        line("!");
    }
    for (id, iface) in &cfg.interfaces {
        line(&format!("interface {}", id.name()));
        for text in interface_lines(node, *id, iface) {
            line(&format!(" {text}"));
        }
        line("!");
    }
    if let Some(ospf) = &cfg.ospf {
        line(&format!("router ospf {}", ospf.pid));
        if let Some(rid) = ospf.router_id {
            line(&format!(" router-id {rid}"));
        }
        if ospf.reference_bandwidth != 100 {
            line(&format!(
                " auto-cost reference-bandwidth {}",
                ospf.reference_bandwidth
            ));
        }
        if ospf.redistribute_connected {
            line(" redistribute connected subnets");
        }
        if ospf.redistribute_static {
            line(" redistribute static subnets");
        }
        if ospf.passive_default {
            line(" passive-interface default");
            for iface in &ospf.active {
                line(&format!(" no passive-interface {}", iface.name()));
            }
        } else {
            for iface in &ospf.passive {
                line(&format!(" passive-interface {}", iface.name()));
            }
        }
        for network in &ospf.networks {
            line(&format!(
                " network {} {} area {}",
                network.addr, network.wildcard, network.area
            ));
        }
        match ospf.default_originate {
            Some(true) => line(" default-information originate always"),
            Some(false) => line(" default-information originate"),
            None => {}
        }
        line("!");
    }
    if let Some(rip) = &cfg.rip {
        line("router rip");
        if let Some(version) = rip.version {
            line(&format!(" version {version}"));
        }
        if rip.passive_default {
            line(" passive-interface default");
            for iface in &rip.active {
                line(&format!(" no passive-interface {}", iface.name()));
            }
        } else {
            for iface in &rip.passive {
                line(&format!(" passive-interface {}", iface.name()));
            }
        }
        for network in &rip.networks {
            line(&format!(" network {network}"));
        }
        if rip.default_originate {
            line(" default-information originate");
        }
        if !rip.auto_summary {
            line(" no auto-summary");
        }
        line("!");
    }
    if let Some(gateway) = cfg.default_gateway {
        line(&format!("ip default-gateway {gateway}"));
    }
    for network in &cfg.default_networks {
        line(&format!("ip default-network {network}"));
    }
    for (_, stored) in cfg.stored.iter().filter(|(slot, _)| *slot == Slot::Http) {
        line(stored);
    }
    if let Some(version) = cfg.ssh_version {
        line(&format!("ip ssh version {version}"));
    }
    for pool in &cfg.nat.pools {
        let tail = if pool.as_prefix {
            format!("prefix-length {}", pool.prefix_len)
        } else {
            format!(
                "netmask {}",
                crate::util::net::mask_from_len(pool.prefix_len)
            )
        };
        line(&format!(
            "ip nat pool {} {} {} {tail}",
            pool.name, pool.start, pool.end
        ));
    }
    for rule in &cfg.nat.rules {
        let target = match &rule.target {
            NatTarget::Interface(iface) => format!("interface {}", iface.name()),
            NatTarget::Pool(name) => format!("pool {name}"),
        };
        line(&format!(
            "ip nat inside source list {} {target}{}",
            rule.acl,
            if rule.overload { " overload" } else { "" }
        ));
    }
    for entry in &cfg.nat.statics {
        match entry.ports {
            Some((proto, local, global)) => line(&format!(
                "ip nat inside source static {} {} {local} {} {global}",
                if proto == crate::device::config::NatProto::Tcp {
                    "tcp"
                } else {
                    "udp"
                },
                entry.local,
                entry.global
            )),
            None => line(&format!(
                "ip nat inside source static {} {}",
                entry.local, entry.global
            )),
        }
    }
    for route in &cfg.static_routes {
        let mut text = format!("ip route {} {}", route.prefix.net, route.prefix.mask());
        if let Some(iface) = route.iface {
            let _ = write!(text, " {}", iface.name());
        }
        if let Some(via) = route.via {
            let _ = write!(text, " {via}");
        }
        if route.distance != 1 {
            let _ = write!(text, " {}", route.distance);
        }
        line(&text);
    }
    line("!");
    for acl in cfg.acls.iter().filter(|acl| acl.number().is_none()) {
        line(&format!(
            "ip access-list {} {}",
            if acl.extended { "extended" } else { "standard" },
            acl.name
        ));
        for item in &acl.items {
            match item {
                AclItem::Rule(rule) => line(&format!(" {}", rule_text(rule, acl.extended))),
                AclItem::Remark(text) => line(&format!(" remark {text}")),
            }
        }
    }
    for acl in cfg.acls.iter().filter(|acl| acl.number().is_some()) {
        for item in &acl.items {
            match item {
                AclItem::Rule(rule) => line(&format!(
                    "access-list {} {}",
                    acl.name,
                    rule_text(rule, acl.extended)
                )),
                AclItem::Remark(text) => line(&format!("access-list {} remark {text}", acl.name)),
            }
        }
    }
    line("!");
    if let Some(level) = &cfg.logging.trap {
        line(&format!("logging trap {level}"));
    }
    for host in &cfg.logging.hosts {
        line(&format!("logging host {host}"));
    }
    for community in &cfg.snmp.communities {
        let acl = community
            .acl
            .as_ref()
            .map(|a| format!(" {a}"))
            .unwrap_or_default();
        line(&format!(
            "snmp-server community {} {}{acl}",
            community.name,
            if community.rw { "RW" } else { "RO" }
        ));
    }
    if let Some(location) = &cfg.snmp.location {
        line(&format!("snmp-server location {location}"));
    }
    if let Some(contact) = &cfg.snmp.contact {
        line(&format!("snmp-server contact {contact}"));
    }
    for stored in &cfg.snmp.stored {
        line(stored);
    }
    line("!");
    for (keyword, banner) in [("login", &cfg.banner_login), ("motd", &cfg.banner_motd)] {
        if let Some(text) = banner {
            line(&format!("banner {keyword} ^C{text}^C"));
        }
    }
    line("!");
    line("line con 0");
    for text in line_lines(&cfg.console, true) {
        line(&format!(" {text}"));
    }
    // Real devices split vty lines at 0-4 / 5-15 and merge equal neighbours.
    let mut start = 0;
    while start < cfg.vty.len() {
        let block_end = if start < 5 {
            5.min(cfg.vty.len())
        } else {
            cfg.vty.len()
        };
        let mut end = start + 1;
        while end < block_end && cfg.vty[end] == cfg.vty[start] {
            end += 1;
        }
        line(&format!("line vty {start} {}", end - 1));
        for text in line_lines(&cfg.vty[start], false) {
            line(&format!(" {text}"));
        }
        start = end;
    }
    line("!");
    for server in &cfg.ntp_servers {
        line(&format!("ntp server {server}"));
    }
    line("end");
    out
}

fn line_lines(cfg: &LineConfig, console: bool) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(acl) = &cfg.access_class_in {
        out.push(format!("access-class {acl} in"));
    }
    if let Some((minutes, seconds)) = cfg.exec_timeout {
        out.push(format!("exec-timeout {minutes} {seconds}"));
    }
    if let Some(level) = cfg.privilege {
        out.push(format!("privilege level {level}"));
    }
    if let Some(password) = &cfg.password {
        out.push(format!("password {}", secret_text(password)));
    }
    if cfg.logging_synchronous {
        out.push("logging synchronous".to_string());
    }
    // The console defaults to no login, vty lines to password login.
    match (cfg.login, console) {
        (LoginMode::None, true) => {}
        (LoginMode::None, false) => out.push("no login".to_string()),
        (LoginMode::Password, _) => out.push("login".to_string()),
        (LoginMode::Local, _) => out.push("login local".to_string()),
    }
    out.extend(cfg.stored.iter().cloned());
    if let Some(transport) = cfg.transport_input {
        out.push(
            match (transport.telnet, transport.ssh) {
                (true, true) => "transport input telnet ssh",
                (true, false) => "transport input telnet",
                (false, true) => "transport input ssh",
                (false, false) => "transport input none",
            }
            .to_string(),
        );
    }
    out
}

fn switchport_lines(sw: &Switchport, default_mode: SwMode, out: &mut Vec<String>) {
    if sw.access_vlan != 1 {
        out.push(format!("switchport access vlan {}", sw.access_vlan));
    }
    if sw.encap_dot1q {
        out.push("switchport trunk encapsulation dot1q".to_string());
    }
    if sw.native_vlan != 1 {
        out.push(format!("switchport trunk native vlan {}", sw.native_vlan));
    }
    if !sw.allowed.is_all() {
        let list = if sw.allowed.is_empty() {
            "none".to_string()
        } else {
            sw.allowed.to_ranges()
        };
        out.push(format!("switchport trunk allowed vlan {list}"));
    }
    if sw.mode != default_mode {
        out.push(
            match sw.mode {
                SwMode::Access => "switchport mode access",
                SwMode::Trunk => "switchport mode trunk",
                SwMode::DynamicAuto => "switchport mode dynamic auto",
                SwMode::DynamicDesirable => "switchport mode dynamic desirable",
            }
            .to_string(),
        );
    }
    if sw.nonegotiate {
        out.push("switchport nonegotiate".to_string());
    }
    if let Some(vlan) = sw.voice_vlan {
        out.push(format!("switchport voice vlan {vlan}"));
    }
    let ps = &sw.port_security;
    if ps.maximum != 1 {
        out.push(format!("switchport port-security maximum {}", ps.maximum));
    }
    if ps.enabled {
        out.push("switchport port-security".to_string());
    }
    match ps.violation {
        Violation::Shutdown => {}
        Violation::Restrict => out.push("switchport port-security violation restrict".to_string()),
        Violation::Protect => out.push("switchport port-security violation protect".to_string()),
    }
    if ps.sticky {
        out.push("switchport port-security mac-address sticky".to_string());
    }
    for (mac, sticky) in &ps.macs {
        out.push(format!(
            "switchport port-security mac-address {}{mac}",
            if *sticky { "sticky " } else { "" }
        ));
    }
}

/// The sub-commands of one interface, in the order a real device prints them.
pub(crate) fn interface_lines(node: &Node, id: IfId, cfg: &IfConfig) -> Vec<String> {
    let role = node.model.role;
    let physical = id.is_physical();
    let mut out = Vec::new();
    if !cfg.description.is_empty() {
        out.push(format!("description {}", cfg.description));
    }
    let bridge_port = node.model.is_switch() && (physical || id.kind == IfKind::PortChannel);
    match &cfg.l2 {
        Some(sw) => switchport_lines(sw, SwMode::DynamicAuto, &mut out),
        None if bridge_port => out.push("no switchport".to_string()),
        None => {}
    }
    if let Some((vlan, native)) = cfg.dot1q {
        out.push(format!(
            "encapsulation dot1Q {vlan}{}",
            if native { " native" } else { "" }
        ));
    }
    if cfg.l2.is_none() && cfg.channel_group.is_none() {
        for addr in &cfg.secondary {
            out.push(format!("ip address {} {} secondary", addr.ip, addr.mask()));
        }
        match cfg.addr {
            Some(L3Addr::Static(addr)) => {
                out.push(format!("ip address {} {}", addr.ip, addr.mask()))
            }
            Some(L3Addr::Dhcp) => out.push("ip address dhcp".to_string()),
            None => out.push("no ip address".to_string()),
        }
    }
    for helper in &cfg.helpers {
        out.push(format!("ip helper-address {helper}"));
    }
    if let Some(acl) = &cfg.acl_in {
        out.push(format!("ip access-group {acl} in"));
    }
    if let Some(acl) = &cfg.acl_out {
        out.push(format!("ip access-group {acl} out"));
    }
    match cfg.nat {
        Some(NatSide::Inside) => out.push("ip nat inside".to_string()),
        Some(NatSide::Outside) => out.push("ip nat outside".to_string()),
        None => {}
    }
    if let Some((pid, area)) = cfg.ospf.area {
        out.push(format!("ip ospf {pid} area {area}"));
    }
    if let Some(cost) = cfg.ospf.cost {
        out.push(format!("ip ospf cost {cost}"));
    }
    if let Some(priority) = cfg.ospf.priority {
        out.push(format!("ip ospf priority {priority}"));
    }
    if let Some(hello) = cfg.ospf.hello {
        out.push(format!("ip ospf hello-interval {hello}"));
    }
    if let Some(dead) = cfg.ospf.dead {
        out.push(format!("ip ospf dead-interval {dead}"));
    }
    if cfg.hsrp_version == 2 {
        out.push("standby version 2".to_string());
    }
    for group in &cfg.hsrp {
        if let Some(ip) = group.ip {
            out.push(format!("standby {} ip {ip}", group.group));
        }
        if group.priority != 100 {
            out.push(format!(
                "standby {} priority {}",
                group.group, group.priority
            ));
        }
        if group.preempt {
            out.push(format!("standby {} preempt", group.group));
        }
    }
    if cfg.shutdown {
        out.push("shutdown".to_string());
    }
    if let Some(mtu) = cfg.mtu {
        out.push(format!("mtu {mtu}"));
    }
    if let Some(bandwidth) = cfg.bandwidth {
        out.push(format!("bandwidth {bandwidth}"));
    }
    if let Some(delay) = cfg.delay {
        out.push(format!("delay {delay}"));
    }
    // Routers always spell out speed and duplex on their Ethernet ports.
    let verbose = role == Role::Router && physical;
    match cfg.duplex {
        Duplex::Auto if verbose => out.push("duplex auto".to_string()),
        Duplex::Auto => {}
        Duplex::Full => out.push("duplex full".to_string()),
        Duplex::Half => out.push("duplex half".to_string()),
    }
    match cfg.speed {
        Some(speed) => out.push(format!("speed {speed}")),
        None if verbose => out.push("speed auto".to_string()),
        None => {}
    }
    if let Some((group, mode)) = cfg.channel_group {
        out.push(format!("channel-group {group} mode {}", mode.keyword()));
    }
    if !cfg.cdp_enable {
        out.push("no cdp enable".to_string());
    }
    if !cfg.lldp_transmit {
        out.push("no lldp transmit".to_string());
    }
    if !cfg.lldp_receive {
        out.push("no lldp receive".to_string());
    }
    match cfg.stp.portfast {
        Some(true) => out.push("spanning-tree portfast".to_string()),
        Some(false) => out.push("spanning-tree portfast disable".to_string()),
        None => {}
    }
    match cfg.stp.bpduguard {
        Some(true) => out.push("spanning-tree bpduguard enable".to_string()),
        Some(false) => out.push("spanning-tree bpduguard disable".to_string()),
        None => {}
    }
    if let Some(cost) = cfg.stp.cost {
        out.push(format!("spanning-tree cost {cost}"));
    }
    if let Some(priority) = cfg.stp.port_priority {
        out.push(format!("spanning-tree port-priority {priority}"));
    }
    if let Some(policy) = &cfg.policy_in {
        out.push(format!("service-policy input {policy}"));
    }
    if let Some(policy) = &cfg.policy_out {
        out.push(format!("service-policy output {policy}"));
    }
    out.extend(cfg.stored.iter().cloned());
    out
}

/// `show running-config`: header plus body.
pub(crate) fn show_running(node: &Node) -> String {
    let body = render(node);
    format!(
        "Building configuration...\n\nCurrent configuration : {} bytes\n{body}",
        body.len()
    )
}

/// `show running-config interface X`.
pub(crate) fn show_running_interface(node: &Node, id: IfId) -> Option<String> {
    let cfg = node.cfg.iface(id)?;
    let mut body = format!("!\ninterface {}\n", id.name());
    for text in interface_lines(node, id, cfg) {
        let _ = writeln!(body, " {text}");
    }
    body.push_str("end\n");
    Some(format!(
        "Building configuration...\n\nCurrent configuration : {} bytes\n{body}\n",
        body.len()
    ))
}

/// `show startup-config`.
pub(crate) fn show_startup(node: &Node) -> String {
    match &node.startup {
        Some(text) => format!("Using {} out of 262144 bytes\n{text}", text.len()),
        None => "startup-config is not present\n".to_string(),
    }
}
