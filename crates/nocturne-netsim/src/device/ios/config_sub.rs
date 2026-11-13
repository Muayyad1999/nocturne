//! Configuration sub-modes: VLAN, line, routing processes, DHCP pools and
//! named access lists.

use crate::device::cli::LineId;
use crate::device::cli::grammar::Args;
use crate::device::config::{
    AclItem, AclProto, AclRule, AddrSpec, Area, IcmpClass, Lease, LineConfig, LoginMode,
    OspfNetwork, PortSpec, Transport, Vlan,
};
use crate::device::ios::config::{selected_lines, sync_vlan_dat, typed_secret};
use crate::device::ios::{Cx, Mode, Table, c, n};
use crate::ifname::IfId;
use crate::sim::acl::parse_port;
use crate::util::net::{Prefix, len_from_mask};

fn exit_to_config(cx: &mut Cx, _: &Args) {
    cx.sh.mode = Mode::Config;
}

fn end(cx: &mut Cx, _: &Args) {
    let reply = cx.sh.leave_config(cx.n, cx.fx);
    cx.out.push_str(&reply.out);
}

fn common() -> Table {
    vec![
        c(
            "exit",
            "Exit from the current configuration mode",
            exit_to_config,
        ),
        c("end", "Exit from configure mode", end),
    ]
}

// -------------------------------------------------------------------- VLAN

pub(crate) fn vlan_table() -> Table {
    let mut t = common();
    t.extend(vec![
        n(
            "name ~ <word:name;The ascii name for the VLAN>",
            "Ascii name of the VLAN",
            |cx, a| {
                let Mode::Vlan(ids) = cx.sh.mode.clone() else {
                    return;
                };
                if ids.len() > 1 && !a.no {
                    return cx.fail("%Command is only allowed on a single VLAN.");
                }
                let name = a.text("name").map(str::to_string);
                if let Some(name) = &name
                    && cx
                        .n
                        .cfg
                        .vlans
                        .iter()
                        .any(|(id, v)| v.name == *name && !ids.contains(id))
                {
                    return cx.fail(format!("%VLAN name {name} is already in use."));
                }
                for id in ids {
                    if let Some(vlan) = cx.cfg().vlans.get_mut(&id) {
                        vlan.name = if a.no {
                            Vlan::default_name(id)
                        } else {
                            name.clone().unwrap_or_default()
                        };
                    }
                }
                sync_vlan_dat(cx);
            },
        ),
        n(
            "state ~ (active | suspend)",
            "Operational state of the VLAN",
            |cx, a| {
                let Mode::Vlan(ids) = cx.sh.mode.clone() else {
                    return;
                };
                for id in ids {
                    if let Some(vlan) = cx.cfg().vlans.get_mut(&id) {
                        vlan.active = a.no || !a.has("suspend");
                    }
                }
                sync_vlan_dat(cx);
            },
        ),
    ]);
    t
}

// -------------------------------------------------------------------- lines

/// Apply a change to every line the mode selects.
fn each_line(cx: &mut Cx, mut f: impl FnMut(&mut LineConfig)) {
    for line in selected_lines(cx) {
        let cfg = cx.cfg();
        match line {
            LineId::Console => f(&mut cfg.console),
            LineId::Vty(index) => {
                if let Some(vty) = cfg.vty.get_mut(usize::from(index)) {
                    f(vty);
                }
            }
        }
    }
}

pub(crate) fn line_table() -> Table {
    let mut t = common();
    t.extend(vec![
        n("password ~ [0 | 7] <word:password;The password>", "Set a password", |cx, a| {
            let secret = match a.text("password").map(str::to_string) {
                Some(text) if !a.no => match typed_secret(cx, a, &text, false) {
                    Some(secret) => Some(secret),
                    None => return,
                },
                _ => None,
            };
            each_line(cx, |line| line.password = secret.clone());
        }),
        n("login [local]", "Enable password checking", |cx, a| {
            let mode = if a.no {
                LoginMode::None
            } else if a.has("local") {
                LoginMode::Local
            } else {
                LoginMode::Password
            };
            each_line(cx, |line| line.login = mode);
            let lacks_password = mode == LoginMode::Password
                && selected_lines(cx).iter().any(|l| match l {
                    LineId::Console => cx.n.cfg.console.password.is_none(),
                    LineId::Vty(i) => cx.n.cfg.vty.get(usize::from(*i)).is_some_and(|v| v.password.is_none()),
                });
            if lacks_password {
                cx.say("% Login disabled on line, until 'password' is set");
            }
        }),
        n("transport input ~ (all | none | telnet [ssh] | ssh [telnet])", "Define which protocols to use when connecting to the terminal server", |cx, a| {
            let transport = if a.no {
                None
            } else if a.has("all") {
                Some(Transport { telnet: true, ssh: true })
            } else {
                Some(Transport { telnet: a.has("telnet"), ssh: a.has("ssh") })
            };
            each_line(cx, |line| line.transport_input = transport);
        }),
        n("exec-timeout ~ <0-35791:minutes;Timeout in minutes> [<0-2147483:seconds;Timeout in seconds>]", "Set the EXEC timeout", |cx, a| {
            let value = if a.no { None } else { Some((a.num("minutes").unwrap_or(10) as u32, a.num("seconds").unwrap_or(0) as u32)) };
            each_line(cx, |line| line.exec_timeout = value);
        }),
        n("logging synchronous", "Synchronized message output", |cx, a| {
            each_line(cx, |line| line.logging_synchronous = !a.no);
        }),
        n("access-class ~ <word:acl;IP access list> in", "Filter connections based on an IP access list", |cx, a| {
            let acl = if a.no { None } else { a.text("acl").map(str::to_string) };
            each_line(cx, |line| line.access_class_in = acl.clone());
        }),
        n("privilege level ~ <0-15:level;Default privilege level for line>", "Change privilege level for line", |cx, a| {
            let level = if a.no { None } else { a.num("level").map(|l| l as u8) };
            each_line(cx, |line| line.privilege = level);
        }),
        n("history size <0-256:size;Size of history buffer>", "Enable and control the command history function", |cx, a| {
            let text = format!("history size {}", a.num("size").unwrap_or(20));
            each_line(cx, |line| {
                line.stored.retain(|l| !l.starts_with("history size"));
                if !a.no {
                    line.stored.push(text.clone());
                }
            });
        }),
        n("stopbits <1-2:bits>", "Set async line stop bits", |cx, a| {
            let text = format!("stopbits {}", a.num("bits").unwrap_or(1));
            each_line(cx, |line| {
                line.stored.retain(|l| !l.starts_with("stopbits"));
                if !a.no {
                    line.stored.push(text.clone());
                }
            });
        }),
    ]);
    t
}

// --------------------------------------------------------------------- OSPF

fn passive_interface(cx: &mut Cx, a: &Args, ospf: bool) {
    let iface: Option<IfId> = a.iface("if");
    let default = a.has("default");
    if let Some(id) = iface
        && !cx.n.cfg.interfaces.contains_key(&id)
    {
        return cx.invalid();
    }
    let no = a.no;
    let cfg = cx.cfg();
    let (passive_default, passive, active) = if ospf {
        let Some(o) = cfg.ospf.as_mut() else { return };
        (&mut o.passive_default, &mut o.passive, &mut o.active)
    } else {
        let Some(r) = cfg.rip.as_mut() else { return };
        (&mut r.passive_default, &mut r.passive, &mut r.active)
    };
    match (default, iface) {
        (true, _) => {
            *passive_default = !no;
            passive.clear();
            active.clear();
        }
        (false, Some(id)) => {
            // With `passive-interface default`, exceptions are listed as active.
            if *passive_default {
                if no {
                    active.insert(id);
                } else {
                    active.remove(&id);
                }
            } else if no {
                passive.remove(&id);
            } else {
                passive.insert(id);
            }
        }
        (false, None) => {}
    }
}

pub(crate) fn ospf_table() -> Table {
    let mut t = common();
    t.extend(vec![
        n("router-id ~ <ip:id;OSPF router-id in IP address format>", "router-id for this OSPF process", |cx, a| {
            let id = if a.no { None } else { a.ip("id") };
            if let Some(ospf) = cx.cfg().ospf.as_mut() {
                ospf.router_id = id;
            }
            // A process that already has neighbours keeps its identity until it is reset.
            if cx.n.ospf.ifaces.values().all(|state| state.neighbors.is_empty()) {
                cx.n.ospf.router_id = None;
            } else if !a.no {
                cx.say("% OSPF: Reload or use \"clear ip ospf process\" command, for this to take effect");
            }
        }),
        n("network <ip:addr;Network number> <ip:wildcard;OSPF wild card bits> area <word:area;OSPF area ID as a decimal value or in IP address format>", "Enable routing on an IP network", |cx, a| {
            let (Some(addr), Some(wildcard), Some(area)) = (a.ip("addr"), a.ip("wildcard"), a.text("area").and_then(Area::parse)) else {
                return cx.invalid();
            };
            // Like IOS, store the address with the don't-care bits cleared.
            let addr = std::net::Ipv4Addr::from(u32::from(addr) & !u32::from(wildcard));
            let entry = OspfNetwork { addr, wildcard, area };
            let Some(ospf) = cx.cfg().ospf.as_mut() else { return };
            ospf.networks.retain(|n| (n.addr, n.wildcard) != (addr, wildcard));
            if !a.no {
                ospf.networks.push(entry);
            }
        }),
        n("passive-interface (default | <if>)", "Suppress routing updates on an interface", |cx, a| passive_interface(cx, a, true)),
        n("default-information originate [always]", "Distribute a default route", |cx, a| {
            let value = if a.no { None } else { Some(a.has("always")) };
            if let Some(ospf) = cx.cfg().ospf.as_mut() {
                ospf.default_originate = value;
            }
        }),
        n("redistribute (static | connected) [subnets]", "Redistribute information from another routing protocol", |cx, a| {
            let Some(ospf) = cx.cfg().ospf.as_mut() else { return };
            if a.has("static") {
                ospf.redistribute_static = !a.no;
            } else {
                ospf.redistribute_connected = !a.no;
            }
        }),
        n("auto-cost reference-bandwidth ~ <1-4294967:mbps;The reference bandwidth in terms of Mbits per second>", "Calculate OSPF interface cost according to bandwidth", |cx, a| {
            let value = if a.no { 100 } else { a.num("mbps").unwrap_or(100) as u32 };
            if let Some(ospf) = cx.cfg().ospf.as_mut() {
                ospf.reference_bandwidth = value;
            }
            if !a.no {
                cx.say("% OSPF: Reference bandwidth is changed.");
                cx.say("        Please ensure reference bandwidth is consistent across all routers.");
            }
        }),
        c("log-adjacency-changes [detail]", "Log changes in adjacency state", |_, _| {}),
    ]);
    t
}

// ---------------------------------------------------------------------- RIP

pub(crate) fn rip_table() -> Table {
    let mut t = common();
    t.extend(vec![
        n(
            "version ~ <1-2:version>",
            "Set routing protocol version",
            |cx, a| {
                let version = if a.no {
                    None
                } else {
                    a.num("version").map(|v| v as u8)
                };
                if let Some(rip) = cx.cfg().rip.as_mut() {
                    rip.version = version;
                }
            },
        ),
        n(
            "network <ip:network;Network number>",
            "Enable routing on an IP network",
            |cx, a| {
                let Some(network) = a.ip("network") else {
                    return;
                };
                // RIP networks are classful.
                let network = Prefix::classful(network).net;
                let Some(rip) = cx.cfg().rip.as_mut() else {
                    return;
                };
                rip.networks.retain(|n| *n != network);
                if !a.no {
                    rip.networks.push(network);
                }
            },
        ),
        n(
            "auto-summary",
            "Enable automatic network number summarization",
            |cx, a| {
                if let Some(rip) = cx.cfg().rip.as_mut() {
                    rip.auto_summary = !a.no;
                }
            },
        ),
        n(
            "passive-interface (default | <if>)",
            "Suppress routing updates on an interface",
            |cx, a| passive_interface(cx, a, false),
        ),
        n(
            "default-information originate",
            "Control distribution of default information",
            |cx, a| {
                if let Some(rip) = cx.cfg().rip.as_mut() {
                    rip.default_originate = !a.no;
                }
            },
        ),
    ]);
    t
}

// --------------------------------------------------------------------- DHCP

pub(crate) fn dhcp_table() -> Table {
    fn pool<'c>(cx: &'c mut Cx) -> Option<&'c mut crate::device::config::DhcpPool> {
        let Mode::DhcpPool(name) = cx.sh.mode.clone() else {
            return None;
        };
        cx.cfg().dhcp_pools.iter_mut().find(|p| p.name == name)
    }
    let mut t = common();
    t.extend(vec![
        n("network ~ <ip:net;Network number in dotted-decimal notation> [<ip:mask;Network mask> | <word:prefix;Network prefix length, e.g. /24>]", "Network number and mask", |cx, a| {
            if a.no {
                if let Some(pool) = pool(cx) {
                    pool.network = None;
                }
                return;
            }
            let Some(net) = a.ip("net") else { return };
            let len = match (a.ip("mask"), a.text("prefix")) {
                (Some(mask), _) => len_from_mask(mask),
                (None, Some(prefix)) => prefix.trim_start_matches('/').parse::<u8>().ok().filter(|l| *l <= 32),
                (None, None) => Some(Prefix::classful(net).len),
            };
            let Some(len) = len.filter(|l| (1..=30).contains(l)) else { return cx.fail("% Invalid network mask") };
            if let Some(pool) = pool(cx) {
                pool.network = Some(Prefix::new(net, len));
            }
        }),
        n("default-router ~ <ip:router;Router's IP address> {<ip:router>}", "Default routers", |cx, a| {
            let routers = if a.no { Vec::new() } else { a.ips("router") };
            if let Some(pool) = pool(cx) {
                pool.default_routers = routers;
            }
        }),
        n("dns-server ~ <ip:server;Server's IP address> {<ip:server>}", "DNS servers", |cx, a| {
            let servers = if a.no { Vec::new() } else { a.ips("server") };
            if let Some(pool) = pool(cx) {
                pool.dns_servers = servers;
            }
        }),
        n("domain-name ~ <word:domain;Domain name>", "Domain name", |cx, a| {
            let domain = if a.no { None } else { a.text("domain").map(str::to_string) };
            if let Some(pool) = pool(cx) {
                pool.domain = domain;
            }
        }),
        n("lease ~ (infinite | <0-365:days;Days> [<0-23:hours;Hours> [<0-59:minutes;Minutes>]])", "Address lease time", |cx, a| {
            let lease = if a.no {
                None
            } else if a.has("infinite") {
                Some(Lease::Infinite)
            } else {
                Some(Lease::Time(
                    a.num("days").unwrap_or(1) as u32,
                    a.num("hours").unwrap_or(0) as u32,
                    a.num("minutes").unwrap_or(0) as u32,
                ))
            };
            if lease == Some(Lease::Time(0, 0, 0)) {
                return cx.fail("% Lease time must be greater than zero");
            }
            if let Some(pool) = pool(cx) {
                pool.lease = lease;
            }
        }),
    ]);
    t
}

// --------------------------------------------------------------------- ACLs

const STANDARD_ENTRY: &str = "[<1-2147483647:seq;Sequence Number>] (permit | deny) (any | host <ip:src;Source address> | <ip:src;Source address> [<ip:src_wildcard;Wildcard bits>]) [log]";
const OPERATORS: [&str; 5] = ["eq", "neq", "gt", "lt", "range"];

pub(crate) fn acl_table(extended: bool) -> Table {
    let mut t = common();
    if extended {
        let src = "(any | host <ip:src;Source address> | <ip:src;Source address> <ip:src_wildcard;Source wildcard bits>)";
        let dst = "(any | host <ip:dst;Destination address> | <ip:dst;Destination address> <ip:dst_wildcard;Destination wildcard bits>)";
        let sport = "[(eq | neq | gt | lt) <word:sport;Port number or name> | range <word:sport> <word:sport_end>]";
        let dport = "[(eq | neq | gt | lt) <word:dport;Port number or name> | range <word:dport> <word:dport_end>]";
        // Patterns must be 'static; this one is assembled once per process.
        let entry: &'static str = Box::leak(
            format!(
                "[<1-2147483647:seq;Sequence Number>] (permit | deny) (ip | icmp | tcp | udp) {src} {sport} {dst} {dport} {{echo | echo-reply | unreachable | time-exceeded | established | log}}"
            )
            .into_boxed_str(),
        );
        t.push(n(entry, "Specify packets to forward or reject", acl_entry));
    } else {
        t.push(n(
            STANDARD_ENTRY,
            "Specify packets to forward or reject",
            acl_entry,
        ));
    }
    t.push(n(
        "remark <line:text;Comment up to 100 characters>",
        "Access list entry comment",
        |cx, a| {
            let Some(name) = acl_name(cx) else { return };
            let text = a.text("text").unwrap_or("").to_string();
            let Some(acl) = cx.cfg().acl_mut(&name) else {
                return;
            };
            acl.items
                .retain(|item| !matches!(item, AclItem::Remark(t) if *t == text));
            if !a.no {
                acl.items.push(AclItem::Remark(text));
            }
        },
    ));
    t.push(n(
        "<1-2147483647:seq;Sequence number of the entry to delete>",
        "",
        |cx, a| {
            // Only meaningful as `no 20`: delete entry 20.
            if !a.no {
                return cx.invalid();
            }
            let Some(name) = acl_name(cx) else { return };
            let seq = a.num("seq").unwrap_or(0) as u32;
            if let Some(acl) = cx.cfg().acl_mut(&name) {
                acl.items
                    .retain(|item| !matches!(item, AclItem::Rule(r) if r.seq == seq));
            }
            cx.n.acl_hits.remove(&(name, seq));
        },
    ));
    t
}

/// The list the current mode (or a numbered `access-list` line) is editing.
fn acl_name(cx: &Cx) -> Option<String> {
    match &cx.sh.mode {
        Mode::AclStd(name) | Mode::AclExt(name) => Some(name.clone()),
        _ => None,
    }
}

/// Build a port match from the operator keyword and the typed port(s).
fn port_spec(
    a: &Args,
    proto: AclProto,
    which: &str,
    operator: Option<&str>,
) -> Result<Option<PortSpec>, ()> {
    let Some(first) = a.text(which) else {
        return Ok(None);
    };
    if !matches!(proto, AclProto::Tcp | AclProto::Udp) {
        return Err(());
    }
    let port = parse_port(proto, first).ok_or(())?;
    Ok(Some(match operator {
        Some("range") => {
            let end = a
                .text(&format!("{which}_end"))
                .and_then(|t| parse_port(proto, t))
                .ok_or(())?;
            PortSpec::Range(port.min(end), port.max(end))
        }
        Some("neq") => PortSpec::Neq(port),
        Some("gt") => PortSpec::Gt(port),
        Some("lt") => PortSpec::Lt(port),
        _ => PortSpec::Eq(port),
    }))
}

fn addr_spec(a: &Args, addr: &str, wildcard: &str) -> AddrSpec {
    match (a.ip(addr), a.ip(wildcard)) {
        (Some(ip), Some(wildcard)) if u32::from(wildcard) == 0 => AddrSpec::Host(ip),
        (Some(ip), Some(wildcard)) => AddrSpec::Net(
            std::net::Ipv4Addr::from(u32::from(ip) & !u32::from(wildcard)),
            wildcard,
        ),
        // `host A`, or a bare address in a standard list: one host.
        (Some(ip), None) => AddrSpec::Host(ip),
        (None, _) => AddrSpec::Any,
    }
}

fn acl_entry(cx: &mut Cx, a: &Args) {
    let Some(name) = acl_name(cx) else { return };
    let proto = if a.has("tcp") {
        AclProto::Tcp
    } else if a.has("udp") {
        AclProto::Udp
    } else if a.has("icmp") {
        AclProto::Icmp
    } else {
        AclProto::Ip
    };
    // Operator keywords come in typing order: the source port's (if a source
    // port was given) before the destination port's.
    let operators: Vec<&str> = a
        .keywords()
        .iter()
        .copied()
        .filter(|k| OPERATORS.contains(k))
        .collect();
    let has_sport = a.text("sport").is_some();
    let sport_op = if has_sport {
        operators.first().copied()
    } else {
        None
    };
    let dport_op = operators.get(usize::from(has_sport)).copied();
    let (Ok(sport), Ok(dport)) = (
        port_spec(a, proto, "sport", sport_op),
        port_spec(a, proto, "dport", dport_op),
    ) else {
        return cx.invalid();
    };
    let icmp = [
        ("echo", IcmpClass::Echo),
        ("echo-reply", IcmpClass::EchoReply),
        ("unreachable", IcmpClass::Unreachable),
        ("time-exceeded", IcmpClass::TimeExceeded),
    ]
    .into_iter()
    .find(|(keyword, _)| a.has(keyword))
    .map(|(_, class)| class);
    if icmp.is_some() && proto != AclProto::Icmp || a.has("established") && proto != AclProto::Tcp {
        return cx.invalid();
    }
    let mut rule = AclRule {
        seq: a.num("seq").unwrap_or(0) as u32,
        permit: a.has("permit"),
        proto,
        src: addr_spec(a, "src", "src_wildcard"),
        sport,
        dst: addr_spec(a, "dst", "dst_wildcard"),
        dport,
        icmp,
        established: a.has("established"),
        log: a.has("log"),
    };
    let Some(acl) = cx.cfg().acl_mut(&name) else {
        return;
    };
    let same = |r: &AclRule, wanted: &AclRule| {
        AclRule {
            seq: 0,
            ..r.clone()
        } == AclRule {
            seq: 0,
            ..wanted.clone()
        }
    };
    if a.no {
        // `no permit ...` removes the entry with the same definition.
        acl.items.retain(|item| !matches!(item, AclItem::Rule(r) if same(r, &rule) || (rule.seq != 0 && r.seq == rule.seq)));
        return;
    }
    if rule.seq == 0 {
        // Re-entering an identical entry is a no-op, as on real devices.
        if acl.rules().any(|r| same(r, &rule)) {
            return;
        }
        rule.seq = acl.next_seq();
    }
    acl.insert(rule);
}
