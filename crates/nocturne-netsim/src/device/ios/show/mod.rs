//! `show` commands. Output formats follow real devices closely enough for the
//! public ntc-templates TextFSM library to parse them (see `tests/ntc_validate.py`).

pub(crate) mod interfaces;
pub(crate) mod protocols;
pub(crate) mod routing;
pub(crate) mod switching;
pub(crate) mod system;

use crate::catalog::Role;
use crate::device::ios::{Cx, Table, c, runcfg};

/// Append a renderer's text to the command output.
macro_rules! emit {
    ($render:expr) => {
        |cx: &mut Cx, _: &crate::device::cli::grammar::Args| {
            let text = $render(cx);
            cx.out.push_str(&text);
        }
    };
}
pub(crate) use emit;

pub(crate) fn table(role: Role, privileged: bool) -> Table {
    let mut t: Table = vec![
        c("show version", "System hardware and software status", emit!(|cx: &mut Cx| system::version(cx.n, cx.fx.now))),
        c("show inventory", "Show the physical inventory", emit!(|cx: &mut Cx| system::inventory(cx.n))),
        c("show clock [detail]", "Display the system clock", emit!(|cx: &mut Cx| system::clock(cx.n, cx.fx.now))),
        c("show users", "Display information about terminal lines", emit!(|cx: &mut Cx| system::users(cx))),
        c("show history", "Display the session command history", emit!(|cx: &mut Cx| system::history(cx))),
        c("show flash:", "display information about flash: file system", emit!(|cx: &mut Cx| system::flash(cx.n, cx.fx.now))),
        c("show flash", "display information about flash: file system", emit!(|cx: &mut Cx| system::flash(cx.n, cx.fx.now))),
        c("show logging", "Show the contents of logging buffers", emit!(|cx: &mut Cx| system::logging(cx.n))),
        c("show privilege", "Show current privilege level", emit!(|cx: &mut Cx| system::privilege(cx))),
        c("show sessions", "Information about Telnet connections", |cx, _| cx.say("% No connections open")),
        c("show protocols", "Active network routing protocols", emit!(|cx: &mut Cx| system::protocols(cx.n))),
        c("show ntp status", "NTP status", emit!(|cx: &mut Cx| system::ntp_status(cx.n))),
        c("show ntp associations", "NTP associations", emit!(|cx: &mut Cx| system::ntp_associations(cx.n))),
        c("show snmp", "snmp statistics", emit!(|cx: &mut Cx| system::snmp(cx.n))),
        c("show snmp community", "snmp communities", emit!(|cx: &mut Cx| system::snmp_community(cx.n))),
        c("show ssh", "Status of SSH server connections", emit!(|cx: &mut Cx| system::ssh(cx.n))),
        c("show ip ssh", "Information on SSH", emit!(|cx: &mut Cx| system::ip_ssh(cx.n))),
        c("show line", "TTY line information", emit!(|cx: &mut Cx| system::line(cx))),
        c("show processes cpu [history | sorted]", "Show CPU use per process", |cx, a| {
            let text = if a.has("history") { system::cpu_history(cx.n, cx.fx) } else { system::processes_cpu(cx.n, cx.fx) };
            cx.out.push_str(&text);
        }),
        c("show memory statistics", "Memory statistics", emit!(|cx: &mut Cx| system::memory(cx.n))),
        c("show environment [all | temperature]", "Environmental monitor statistics", emit!(|cx: &mut Cx| system::environment(cx.n, cx.fx))),
        c("show controllers [<if>]", "Interface controller status", |cx, a| {
            let text = system::controllers(cx.n, a.iface("if"));
            cx.out.push_str(&text);
        }),
        c("show cdp", "CDP information", emit!(|cx: &mut Cx| protocols::cdp(cx.n))),
        c("show cdp neighbors [detail]", "CDP neighbor entries", |cx, a| {
            let text = if a.has("detail") { protocols::cdp_detail(cx.n, cx.fx.now) } else { protocols::cdp_neighbors(cx.n, cx.fx.now) };
            cx.out.push_str(&text);
        }),
        c("show lldp neighbors [detail]", "LLDP neighbor entries", |cx, a| {
            let text = if a.has("detail") { protocols::lldp_detail(cx.n, cx.fx.now) } else { protocols::lldp_neighbors(cx.n, cx.fx.now) };
            cx.out.push_str(&text);
        }),
        c("show ip interface brief", "Brief summary of IP status and configuration", emit!(|cx: &mut Cx| interfaces::ip_brief(cx.n))),
        c("show ip interface [<if>]", "IP interface status and configuration", |cx, a| match interfaces::ip_interface(cx.n, a.iface("if")) {
            Some(text) => cx.out.push_str(&text),
            None => cx.invalid(),
        }),
        c("show interfaces [<if>]", "Interface status and configuration", |cx, a| match interfaces::interfaces(cx.n, cx.fx.now, a.iface("if")) {
            Some(text) => cx.out.push_str(&text),
            None => cx.invalid(),
        }),
        c("show interfaces description", "Show interface description", emit!(|cx: &mut Cx| interfaces::description(cx.n))),
        c("show interfaces counters [errors]", "Show interface counters", |cx, a| {
            let text = interfaces::counters(cx.n, a.has("errors"));
            cx.out.push_str(&text);
        }),
        c("show arp", "ARP table", emit!(|cx: &mut Cx| routing::arp(cx.n, cx.fx.now))),
        c("show ip arp", "IP ARP table", emit!(|cx: &mut Cx| routing::arp(cx.n, cx.fx.now))),
        c("show ip route [<ip:prefix;Network to display information about> | static | connected | ospf | rip | summary]", "IP routing table", |cx, a| {
            let text = routing::ip_route(cx.n, cx.fx.now, a);
            cx.out.push_str(&text);
        }),
        c("show ip protocols", "IP routing protocol process parameters and statistics", emit!(|cx: &mut Cx| routing::ip_protocols(cx.n, cx.fx.now))),
        c("show access-lists [<word:name;ACL name or number>]", "List access lists", |cx, a| {
            let text = routing::access_lists(cx.n, a.text("name"));
            cx.out.push_str(&text);
        }),
        c("show ip access-lists [<word:name;ACL name or number>]", "List IP access lists", |cx, a| {
            let text = routing::access_lists(cx.n, a.text("name"));
            cx.out.push_str(&text);
        }),
        c("show ip dhcp binding", "DHCP address bindings", emit!(|cx: &mut Cx| protocols::dhcp_binding(cx.n, cx.fx.now))),
        c("show ip dhcp pool", "DHCP pools information", emit!(|cx: &mut Cx| protocols::dhcp_pool(cx.n))),
        c("show ip dhcp conflict", "DHCP address conflicts", emit!(|_cx: &mut Cx| "IP address        Detection method   Detection time          VRF\n".to_string())),
    ];
    if privileged {
        t.extend(vec![
            c("show running-config", "Current operating configuration", emit!(|cx: &mut Cx| runcfg::show_running(cx.n))),
            c("show running-config interface <if>", "Show interface configuration", |cx, a| {
                match a.iface("if").and_then(|id| runcfg::show_running_interface(cx.n, id)) {
                    Some(text) => cx.out.push_str(&text),
                    None => cx.invalid(),
                }
            }),
            c("show startup-config", "Contents of startup configuration", emit!(|cx: &mut Cx| runcfg::show_startup(cx.n))),
            c("show tech-support", "Show system information for Tech-Support", emit!(|cx: &mut Cx| system::tech_support(cx))),
        ]);
    }
    if role != Role::L2Switch {
        t.extend(vec![
            c("show ip ospf", "OSPF information", emit!(|cx: &mut Cx| protocols::ospf(cx.n, cx.fx.now))),
            c("show ip ospf neighbor [detail]", "Neighbor list", |cx, a| {
                let text = protocols::ospf_neighbors(cx.n, cx.fx.now, a.has("detail"));
                cx.out.push_str(&text);
            }),
            c("show ip ospf interface [brief]", "Interface information", |cx, a| {
                let text = protocols::ospf_interfaces(cx.n, cx.fx.now, a.has("brief"));
                cx.out.push_str(&text);
            }),
            c("show ip ospf database", "Database summary", emit!(|cx: &mut Cx| protocols::ospf_database(cx.n, cx.fx.now))),
            c("show ip rip database", "IPv4 RIP database", emit!(|cx: &mut Cx| protocols::rip_database(cx.n, cx.fx.now))),
            c("show ip nat translations", "Translation entries", emit!(|cx: &mut Cx| protocols::nat_translations(cx.n, cx.fx.now))),
            c("show ip nat statistics", "Translation statistics", emit!(|cx: &mut Cx| protocols::nat_statistics(cx.n, cx.fx.now))),
            c("show standby [brief]", "Hot Standby Router Protocol (HSRP) information", |cx, a| {
                let text = if a.has("brief") { protocols::standby_brief(cx.n) } else { protocols::standby(cx.n, cx.fx.now) };
                cx.out.push_str(&text);
            }),
        ]);
    }
    if role != Role::Router {
        t.extend(vec![
            c("show interfaces status", "Show interface line status", emit!(|cx: &mut Cx| interfaces::status(cx.n))),
            c("show interfaces trunk", "Show interface trunk information", emit!(|cx: &mut Cx| interfaces::trunk(cx.n))),
            c("show interfaces switchport", "Show interface switchport information", |cx, _| {
                let text = interfaces::switchport(cx.n, None);
                cx.out.push_str(&text);
            }),
            c("show interfaces <if> switchport", "Show interface switchport information", |cx, a| {
                if a.iface("if").is_none_or(|id| !cx.n.cfg.interfaces.contains_key(&id)) {
                    return cx.invalid();
                }
                let text = interfaces::switchport(cx.n, a.iface("if"));
                cx.out.push_str(&text);
            }),
            c("show vlan [brief | id <1-4094:vlan;A VTP VLAN number>]", "VTP VLAN status", |cx, a| {
                let text = switching::vlan(cx.n, a.has("brief"), a.num("vlan").map(|v| v as u16));
                cx.out.push_str(&text);
            }),
            c(
                "show mac address-table [dynamic | static] {vlan <1-4094:vlan;VLAN id> | interface <if> | address <mac>}",
                "MAC forwarding table",
                |cx, a| {
                    let text = switching::mac_table(cx.n, cx.fx.now, a);
                    cx.out.push_str(&text);
                },
            ),
            c(
                "show mac-address-table [dynamic | static] {vlan <1-4094:vlan> | interface <if> | address <mac>}",
                "MAC forwarding table",
                |cx, a| {
                    let text = switching::mac_table(cx.n, cx.fx.now, a);
                    cx.out.push_str(&text);
                },
            ),
            c("show spanning-tree [vlan <1-4094:vlan;VLAN id> | interface <if>]", "Spanning tree topology", |cx, a| {
                let text = switching::spanning_tree(cx.n, cx.fx, a.num("vlan").map(|v| v as u16), a.iface("if"));
                cx.out.push_str(&text);
            }),
            c("show spanning-tree summary", "Summary of port states", emit!(|cx: &mut Cx| switching::spanning_tree_summary(cx.n))),
            c("show spanning-tree root", "Status and configuration of the root bridge", emit!(|cx: &mut Cx| switching::spanning_tree_root(cx.n, cx.fx))),
            c("show etherchannel summary", "One-line summary per channel-group", emit!(|cx: &mut Cx| switching::etherchannel_summary(cx.n))),
            c("show port-security", "Show secure port information", emit!(|cx: &mut Cx| switching::port_security(cx.n))),
            c("show port-security interface <if>", "Show secure interface", |cx, a| match a.iface("if").and_then(|id| switching::port_security_interface(cx.n, id)) {
                Some(text) => cx.out.push_str(&text),
                None => cx.invalid(),
            }),
            c("show port-security address", "Show secure address", emit!(|cx: &mut Cx| switching::port_security_address(cx.n))),
            c("show errdisable recovery", "Error disable recovery", emit!(|cx: &mut Cx| switching::errdisable_recovery(cx.n))),
            c("show vtp status", "VTP domain status", emit!(|cx: &mut Cx| switching::vtp_status(cx.n))),
        ]);
    }
    t
}
