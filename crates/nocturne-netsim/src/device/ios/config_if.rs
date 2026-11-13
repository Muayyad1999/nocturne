//! Interface configuration mode (single interfaces, ranges and sub-interfaces).

use crate::catalog::Role;
use crate::device::cli::grammar::Args;
use crate::device::config::{
    Area, ChannelMode, Duplex, HsrpGroup, IfConfig, L3Addr, NatSide, SwMode, Switchport, Violation,
    Vlan, VlanSet,
};
use crate::device::ios::config::sync_vlan_dat;
use crate::device::ios::{Cx, Mode, Table, c, n};
use crate::ifname::{IfId, IfKind};
use crate::util::net::{IfAddr, len_from_mask};
use crate::util::text::parse_ranges;

pub(crate) fn table(role: Role) -> Table {
    let mut t: Table = vec![
        n(
            "description ~ <line:text;Up to 240 characters describing this interface>",
            "Interface specific description",
            |cx, a| {
                let text = if a.no {
                    String::new()
                } else {
                    a.text("text").unwrap_or("").to_string()
                };
                each(cx, |cfg, _| cfg.description = text.clone());
            },
        ),
        n("shutdown", "Shutdown the selected interface", shutdown),
        n(
            "ip address ~ <ip:addr;IP address> <ip:mask;IP subnet mask> [secondary]",
            "Set the IP address of an interface",
            ip_address,
        ),
        n(
            "ip address dhcp",
            "IP Address negotiated via DHCP",
            ip_address_dhcp,
        ),
        n(
            "ipv6 address <line:rest>",
            "Configure IPv6 address on interface",
            |cx, a| {
                keep(
                    cx,
                    a,
                    format!("ipv6 address {}", a.text("rest").unwrap_or("")),
                );
            },
        ),
        n("ipv6 enable", "Enable IPv6 on interface", |cx, a| {
            keep(cx, a, "ipv6 enable".to_string())
        }),
        n(
            "speed ~ (10 | 100 | 1000 | 10000 | auto)",
            "Configure speed operation",
            speed,
        ),
        n(
            "duplex ~ (auto | full | half)",
            "Configure duplex operation",
            duplex,
        ),
        n(
            "mtu ~ <64-9216:mtu;MTU size in bytes>",
            "Set the interface Maximum Transmission Unit (MTU)",
            |cx, a| {
                let value = if a.no {
                    None
                } else {
                    a.num("mtu").map(|v| v as u32)
                };
                each(cx, |cfg, _| cfg.mtu = value);
            },
        ),
        n(
            "bandwidth ~ <1-10000000:kbps;Bandwidth in kilobits>",
            "Set bandwidth informational parameter",
            |cx, a| {
                let value = if a.no {
                    None
                } else {
                    a.num("kbps").map(|v| v as u32)
                };
                each(cx, |cfg, _| cfg.bandwidth = value);
            },
        ),
        n(
            "delay ~ <1-16777215:tens;Throughput delay (tens of microseconds)>",
            "Specify interface throughput delay",
            |cx, a| {
                let value = if a.no {
                    None
                } else {
                    a.num("tens").map(|v| v as u32)
                };
                each(cx, |cfg, _| cfg.delay = value);
            },
        ),
        n(
            "ip helper-address ~ <ip:server;IP destination address>",
            "Specify a destination address for UDP broadcasts",
            helper_address,
        ),
        n(
            "ip access-group ~ <word:acl;Access-list name or number> (in | out)",
            "Specify access control for packets",
            access_group,
        ),
        n("cdp enable", "Enable CDP on interface", |cx, a| {
            each(cx, |cfg, _| cfg.cdp_enable = !a.no)
        }),
        n(
            "lldp transmit",
            "Enable LLDP transmission on interface",
            |cx, a| each(cx, |cfg, _| cfg.lldp_transmit = !a.no),
        ),
        n(
            "lldp receive",
            "Enable LLDP reception on interface",
            |cx, a| each(cx, |cfg, _| cfg.lldp_receive = !a.no),
        ),
        n(
            "negotiation auto",
            "Enable link autonegotiation",
            |cx, a| keep(cx, a, "negotiation auto".to_string()),
        ),
        n(
            "power inline <line:rest>",
            "Inline power configuration",
            |cx, a| {
                keep(
                    cx,
                    a,
                    format!("power inline {}", a.text("rest").unwrap_or("")),
                );
            },
        ),
        n(
            "storm-control <line:rest>",
            "storm configuration",
            |cx, a| {
                keep(
                    cx,
                    a,
                    format!("storm-control {}", a.text("rest").unwrap_or("")),
                );
            },
        ),
        c("exit", "Exit from interface configuration mode", |cx, _| {
            cx.sh.mode = Mode::Config
        }),
        c("end", "Exit from configure mode", |cx, _| {
            let reply = cx.sh.leave_config(cx.n, cx.fx);
            cx.out.push_str(&reply.out);
        }),
    ];
    if role != Role::L2Switch {
        t.extend(vec![
            n("ip nat (inside | outside)", "NAT interface commands", |cx, a| {
                let side = if a.no {
                    None
                } else if a.has("inside") {
                    Some(NatSide::Inside)
                } else {
                    Some(NatSide::Outside)
                };
                each_l3(cx, |cfg, _| cfg.nat = side);
            }),
            n("ip ospf cost ~ <1-65535:cost;Cost>", "Interface cost", |cx, a| {
                let value = if a.no { None } else { a.num("cost").map(|v| v as u32) };
                each_l3(cx, |cfg, _| cfg.ospf.cost = value);
            }),
            n("ip ospf priority ~ <0-255:priority;Priority>", "Router priority", |cx, a| {
                let value = if a.no { None } else { a.num("priority").map(|v| v as u8) };
                each_l3(cx, |cfg, _| cfg.ospf.priority = value);
            }),
            n("ip ospf hello-interval ~ <1-65535:seconds;Seconds>", "Time between HELLO packets", |cx, a| {
                let value = if a.no { None } else { a.num("seconds").map(|v| v as u16) };
                each_l3(cx, |cfg, _| cfg.ospf.hello = value);
            }),
            n("ip ospf dead-interval ~ <1-65535:seconds;Seconds>", "Interval after which a neighbor is declared dead", |cx, a| {
                let value = if a.no { None } else { a.num("seconds").map(|v| v as u16) };
                each_l3(cx, |cfg, _| cfg.ospf.dead = value);
            }),
            n("ip ospf <1-65535:pid;Process ID> area ~ <word:area;OSPF area ID as a decimal value or in IP address format>", "Enable OSPF on this interface", ospf_area),
            n("standby version ~ <1-2:version>", "HSRP version", |cx, a| {
                let version = if a.no { 1 } else { a.num("version").unwrap_or(1) as u8 };
                each_l3(cx, |cfg, _| cfg.hsrp_version = version);
            }),
            n("standby [<0-4095:group;group number>] ip ~ <ip:vip;Virtual IP address>", "Enable HSRP and set the virtual IP address", standby),
            n("standby [<0-4095:group>] priority ~ <0-255:priority;Priority value>", "Priority level", standby),
            n("standby [<0-4095:group>] preempt", "Overthrow lower priority Active routers", standby),
            n("encapsulation dot1q ~ <1-4094:vlan;IEEE 802.1Q VLAN ID> [native]", "Set encapsulation type for an interface", encapsulation),
        ]);
    }
    if role != Role::Router {
        t.extend(vec![
            n("switchport", "Set switching mode characteristics", switchport),
            n("switchport mode ~ (access | trunk | dynamic (auto | desirable))", "Set trunking mode of the interface", switchport_mode),
            n("switchport access vlan ~ <1-4094:vlan;VLAN ID of the VLAN when this port is in access mode>", "Set access mode characteristics of the interface", access_vlan),
            n(
                "switchport trunk allowed vlan ~ (all | none | add <word:list;VLAN IDs of the allowed VLANs> | remove <word:list> | except <word:list> | <word:list>)",
                "Set allowed VLAN characteristics when interface is in trunking mode",
                trunk_allowed,
            ),
            n("switchport trunk native vlan ~ <1-4094:vlan;VLAN ID of the native VLAN when this port is in trunking mode>", "Set trunking native characteristics", |cx, a| {
                let vlan = if a.no { 1 } else { a.num("vlan").unwrap_or(1) as u16 };
                each_l2(cx, |sw, _| sw.native_vlan = vlan);
            }),
            n("switchport nonegotiate", "Device will not engage in negotiation protocol on this interface", |cx, a| {
                if !a.no && selection(cx).iter().any(|id| l2_of(cx, *id).is_some_and(|sw| matches!(sw.mode, SwMode::DynamicAuto | SwMode::DynamicDesirable))) {
                    return cx.fail("Command rejected: Conflict between 'nonegotiate' and 'dynamic' status.");
                }
                each_l2(cx, |sw, _| sw.nonegotiate = !a.no);
            }),
            n("switchport voice vlan ~ <1-4094:vlan;Vlan for voice traffic>", "Voice appliance attributes", voice_vlan),
            n("switchport port-security", "Security related command", |cx, a| {
                let dynamic = |cx: &Cx, id: IfId| l2_of(cx, id).is_some_and(|sw| matches!(sw.mode, SwMode::DynamicAuto | SwMode::DynamicDesirable));
                if let (false, Some(id)) = (a.no, selection(cx).into_iter().find(|id| dynamic(cx, *id))) {
                    return cx.fail(format!("Command rejected: {} is a dynamic port.", id.name()));
                }
                each_l2(cx, |sw, _| sw.port_security.enabled = !a.no);
                if a.no {
                    // What the feature learned goes with it.
                    for id in selection(cx) {
                        if let Some(state) = cx.n.ifs.get_mut(&id) {
                            state.secure_macs.clear();
                        }
                        cx.n.macs.flush_port(id);
                    }
                }
            }),
            n("switchport port-security maximum ~ <1-8192:max;Maximum addresses>", "Max secure addresses", |cx, a| {
                let max = if a.no { 1 } else { a.num("max").unwrap_or(1) as u32 };
                each_l2(cx, |sw, _| sw.port_security.maximum = max);
            }),
            n("switchport port-security violation ~ (shutdown | restrict | protect)", "Security violation mode", |cx, a| {
                let mode = if a.has("restrict") && !a.no {
                    Violation::Restrict
                } else if a.has("protect") && !a.no {
                    Violation::Protect
                } else {
                    Violation::Shutdown
                };
                each_l2(cx, |sw, _| sw.port_security.violation = mode);
            }),
            n("switchport port-security mac-address sticky [<mac>]", "Configure dynamic secure addresses as sticky", secure_mac),
            n("switchport port-security mac-address <mac>", "Secure mac address", secure_mac),
            n("channel-group ~ <1-48:group;Channel group number> mode (on | active | passive | desirable | auto)", "Etherchannel/port bundling configuration", channel_group),
            n("spanning-tree portfast ~ [disable | edge | trunk]", "Enable an interface to move directly to forwarding on link up", |cx, a| {
                let value = if a.no { None } else { Some(!a.has("disable")) };
                each(cx, |cfg, _| cfg.stp.portfast = value);
                if value == Some(true) {
                    cx.say("%Warning: portfast should only be enabled on ports connected to a single");
                    cx.say(" host. Connecting hubs, concentrators, switches, bridges, etc... to this");
                    cx.say(" interface  when portfast is enabled, can cause temporary bridging loops.");
                    cx.say(" Use with CAUTION");
                    cx.say("");
                }
            }),
            n("spanning-tree bpduguard ~ (enable | disable)", "Don't accept BPDUs on this interface", |cx, a| {
                let value = if a.no { None } else { Some(a.has("enable")) };
                each(cx, |cfg, _| cfg.stp.bpduguard = value);
            }),
            n("spanning-tree cost ~ <1-200000000:cost;port path cost>", "Change an interface's spanning tree port path cost", |cx, a| {
                let value = if a.no { None } else { a.num("cost").map(|v| v as u32) };
                each(cx, |cfg, _| cfg.stp.cost = value);
            }),
            n("spanning-tree port-priority ~ <0-240:priority;port priority in increments of 16>", "Change an interface's spanning tree port priority", |cx, a| {
                let value = if a.no { None } else { a.num("priority").map(|v| v as u8) };
                if value.is_some_and(|v| v % 16 != 0) {
                    return cx.fail("% Port Priority in increments of 16 is required");
                }
                each(cx, |cfg, _| cfg.stp.port_priority = value);
            }),
        ]);
        if role == Role::L3Switch {
            t.push(n(
                "switchport trunk encapsulation ~ dot1q",
                "Set trunking encapsulation when interface is in trunking mode",
                |cx, a| {
                    each_l2(cx, |sw, _| sw.encap_dot1q = !a.no);
                },
            ));
        }
    }
    t
}

// ------------------------------------------------------------------ helpers

/// The interfaces the current mode applies to.
fn selection(cx: &Cx) -> Vec<IfId> {
    match &cx.sh.mode {
        Mode::If(ids) => ids.clone(),
        _ => Vec::new(),
    }
}

fn l2_of<'c>(cx: &'c Cx, id: IfId) -> Option<&'c Switchport> {
    cx.n.cfg.iface(id).and_then(|cfg| cfg.l2.as_ref())
}

/// Apply a change to every selected interface.
fn each(cx: &mut Cx, mut f: impl FnMut(&mut IfConfig, IfId)) {
    for id in selection(cx) {
        if let Some(cfg) = cx.cfg().interfaces.get_mut(&id) {
            f(cfg, id);
        }
        if id.kind == IfKind::PortChannel {
            sync_members(cx, id);
        }
    }
}

/// What is configured on a port-channel also lands on its member ports.
fn sync_members(cx: &mut Cx, po: IfId) {
    let Some(template) = cx.n.cfg.iface(po).and_then(|cfg| cfg.l2.clone()) else {
        return;
    };
    for member in cx.n.cfg.channel_members(po.number()) {
        if let Some(sw) = cx
            .cfg()
            .interfaces
            .get_mut(&member)
            .and_then(|cfg| cfg.l2.as_mut())
        {
            let port_security = std::mem::take(&mut sw.port_security);
            *sw = Switchport {
                port_security,
                ..template.clone()
            };
        }
    }
}

/// Apply a change to the selected interfaces if all of them are layer 3.
fn each_l3(cx: &mut Cx, f: impl FnMut(&mut IfConfig, IfId)) {
    if selection(cx).iter().any(|id| {
        cx.n.cfg
            .iface(*id)
            .is_none_or(|c| c.l2.is_some() || c.channel_group.is_some())
    }) {
        return cx.invalid();
    }
    each(cx, f);
}

/// Apply a change to the switch-port settings of the selected interfaces.
fn each_l2(cx: &mut Cx, mut f: impl FnMut(&mut Switchport, IfId)) {
    if selection(cx).iter().any(|id| l2_of(cx, *id).is_none()) {
        return cx.invalid();
    }
    each(cx, |cfg, id| {
        if let Some(sw) = cfg.l2.as_mut() {
            f(sw, id);
        }
    });
}

/// Add or (with `no`) remove an accepted-and-stored interface line.
fn keep(cx: &mut Cx, a: &Args, line: String) {
    each(cx, |cfg, _| {
        cfg.stored.retain(|l| *l != line);
        if !a.no {
            cfg.stored.push(line.clone());
        }
    });
}

// ----------------------------------------------------------------- commands

fn shutdown(cx: &mut Cx, a: &Args) {
    let mut ids = selection(cx);
    // Shutting a port-channel shuts its member ports with it.
    let members: Vec<IfId> = ids
        .iter()
        .filter(|id| id.kind == IfKind::PortChannel)
        .flat_map(|po| cx.n.cfg.channel_members(po.number()))
        .collect();
    ids.extend(members);
    for id in &ids {
        if let Some(cfg) = cx.cfg().interfaces.get_mut(id) {
            cfg.shutdown = !a.no;
        }
    }
    // `shutdown` / `no shutdown` is how an err-disabled port is recovered.
    for id in ids {
        if let Some(state) = cx.n.ifs.get_mut(&id) {
            state.errdisabled = None;
        }
    }
}

fn ip_address(cx: &mut Cx, a: &Args) {
    let ids = selection(cx);
    if ids.iter().any(|id| {
        cx.n.cfg
            .iface(*id)
            .is_none_or(|c| c.l2.is_some() || c.channel_group.is_some())
    }) {
        return cx.invalid();
    }
    let addr = match (a.ip("addr"), a.ip("mask")) {
        (Some(ip), Some(mask)) => {
            let Some(len) = len_from_mask(mask).filter(|len| *len > 0) else {
                return cx.fail(format!("Bad mask 0x{:X} for address {ip}", u32::from(mask)));
            };
            let addr = IfAddr::new(ip, len);
            let host_bits_ok =
                len >= 31 || (ip != addr.subnet().net && ip != addr.subnet().broadcast());
            if !host_bits_ok || ip.is_loopback() || ip.is_multicast() || ip.is_unspecified() {
                return cx.fail(format!("Bad mask /{len} for address {ip}"));
            }
            Some(addr)
        }
        _ => None,
    };
    if a.no {
        each(cx, |cfg, _| match addr {
            // `no ip address` removes everything; with an address only that one.
            None => {
                cfg.addr = None;
                cfg.secondary.clear();
            }
            Some(addr) if a.has("secondary") => cfg.secondary.retain(|s| *s != addr),
            Some(addr) => {
                if cfg.static_addr() == Some(addr) {
                    cfg.addr = None;
                }
            }
        });
        return;
    }
    let Some(addr) = addr else { return };
    // The subnet must not overlap another interface of this device.
    let clash = cx.n.cfg.interfaces.iter().find(|(id, cfg)| {
        !ids.contains(id)
            && cfg
                .static_addr()
                .into_iter()
                .chain(cfg.secondary.iter().copied())
                .any(|other| {
                    other.subnet().covers(addr.subnet()) || addr.subnet().covers(other.subnet())
                })
    });
    if let Some((other, _)) = clash {
        return cx.fail(format!(
            "% {} overlaps with {}",
            addr.subnet().net,
            other.name()
        ));
    }
    if ids.len() > 1 {
        return cx.fail("% IP addresses may not be configured on an interface range");
    }
    each(cx, |cfg, _| {
        if a.has("secondary") {
            if !cfg.secondary.contains(&addr) {
                cfg.secondary.push(addr);
            }
        } else {
            cfg.addr = Some(L3Addr::Static(addr));
            cfg.secondary.retain(|s| *s != addr);
        }
    });
}

fn ip_address_dhcp(cx: &mut Cx, a: &Args) {
    each_l3(cx, |cfg, _| {
        if a.no {
            if cfg.addr == Some(L3Addr::Dhcp) {
                cfg.addr = None;
            }
        } else {
            cfg.addr = Some(L3Addr::Dhcp);
        }
    });
}

fn speed(cx: &mut Cx, a: &Args) {
    let value = if a.no || a.has("auto") {
        None
    } else {
        [10, 100, 1000, 10_000]
            .into_iter()
            .find(|s| a.has(&s.to_string()))
    };
    let ids = selection(cx);
    if ids.iter().any(|id| !id.is_physical()) {
        return cx.invalid();
    }
    if let Some(speed) = value
        && ids.iter().any(|id| speed > id.kind.max_mbps())
    {
        return cx.invalid();
    }
    each(cx, |cfg, _| cfg.speed = value);
}

fn duplex(cx: &mut Cx, a: &Args) {
    let value = if a.no || a.has("auto") {
        Duplex::Auto
    } else if a.has("full") {
        Duplex::Full
    } else {
        Duplex::Half
    };
    if selection(cx).iter().any(|id| !id.is_physical()) {
        return cx.invalid();
    }
    each(cx, |cfg, _| cfg.duplex = value);
}

fn helper_address(cx: &mut Cx, a: &Args) {
    let server = a.ip("server");
    each_l3(cx, |cfg, _| match (a.no, server) {
        (true, None) => cfg.helpers.clear(),
        (true, Some(server)) => cfg.helpers.retain(|h| *h != server),
        (false, Some(server)) => {
            if !cfg.helpers.contains(&server) {
                cfg.helpers.push(server);
            }
        }
        (false, None) => {}
    });
}

fn access_group(cx: &mut Cx, a: &Args) {
    let acl = if a.no {
        None
    } else {
        a.text("acl").map(str::to_string)
    };
    let inbound = a.has("in");
    let outbound = a.has("out");
    each_l3(cx, |cfg, _| {
        // `no ip access-group` without a direction clears both.
        if inbound || !outbound {
            cfg.acl_in = acl.clone();
        }
        if outbound || !inbound {
            cfg.acl_out = acl.clone();
        }
    });
}

fn ospf_area(cx: &mut Cx, a: &Args) {
    let pid = a.num("pid").unwrap_or(1) as u16;
    let value = match (a.no, a.text("area").map(Area::parse)) {
        (true, _) => None,
        (false, Some(Some(area))) => Some((pid, area)),
        _ => return cx.invalid(),
    };
    each_l3(cx, |cfg, _| cfg.ospf.area = value);
}

fn standby(cx: &mut Cx, a: &Args) {
    let group = a.num("group").unwrap_or(0) as u16;
    let vip = a.ip("vip");
    if let (false, Some(vip)) = (a.no, vip) {
        let addrs: Vec<IfAddr> = selection(cx)
            .iter()
            .filter_map(|id| cx.n.addr(*id))
            .collect();
        if addrs.iter().any(|addr| addr.ip == vip) {
            return cx.fail("% Address cannot equal interface IP address");
        }
        if addrs.iter().any(|addr| !addr.on_link(vip)) {
            cx.say("% Warning: address is not within a subnet on this interface");
        }
    }
    let version = selection(cx)
        .first()
        .and_then(|id| cx.n.cfg.iface(*id))
        .map_or(1, |cfg| cfg.hsrp_version);
    if version == 1 && group > 255 {
        return cx.fail("% Group number out of range for HSRP version 1 (0-255)");
    }
    let priority = a.num("priority").map(|p| p as u8);
    let (is_ip, is_priority, is_preempt) = (a.has("ip"), a.has("priority"), a.has("preempt"));
    let no = a.no;
    each_l3(cx, |cfg, _| {
        if no && is_ip {
            cfg.hsrp.retain(|g| g.group != group);
            return;
        }
        let index = match cfg.hsrp.iter().position(|g| g.group == group) {
            Some(index) => index,
            None if no => return,
            None => {
                cfg.hsrp.push(HsrpGroup {
                    group,
                    ip: None,
                    priority: 100,
                    preempt: false,
                });
                cfg.hsrp.sort_by_key(|g| g.group);
                cfg.hsrp.iter().position(|g| g.group == group).unwrap_or(0)
            }
        };
        let entry = &mut cfg.hsrp[index];
        if is_ip {
            entry.ip = vip;
        }
        if is_priority {
            entry.priority = if no { 100 } else { priority.unwrap_or(100) };
        }
        if is_preempt {
            entry.preempt = !no;
        }
    });
}

fn encapsulation(cx: &mut Cx, a: &Args) {
    let ids = selection(cx);
    if ids.iter().any(|id| !id.is_sub()) {
        return cx.invalid();
    }
    let value = if a.no {
        None
    } else {
        a.num("vlan").map(|v| (v as u16, a.has("native")))
    };
    if let Some((vlan, _)) = value {
        // One VLAN per sub-interface of a physical port.
        let taken = cx.n.cfg.interfaces.iter().any(|(id, cfg)| {
            !ids.contains(id)
                && ids.iter().any(|mine| mine.parent() == id.parent())
                && cfg.dot1q.is_some_and(|(v, _)| v == vlan)
        });
        if taken {
            return cx.fail(format!("%Configuration of multiple subinterfaces of the same main\ninterface with the same VID ({vlan}) is not permitted."));
        }
    }
    each(cx, |cfg, _| cfg.dot1q = value);
}

fn switchport(cx: &mut Cx, a: &Args) {
    let ids = selection(cx);
    let bridge_capable = |id: &IfId| id.is_physical() || id.kind == IfKind::PortChannel;
    if !ids.iter().all(bridge_capable) {
        return cx.invalid();
    }
    if a.no {
        if cx.role() != Role::L3Switch || ids.iter().any(|id| id.kind == IfKind::PortChannel) {
            return cx.invalid();
        }
        if ids.iter().any(|id| {
            cx.n.cfg
                .iface(*id)
                .is_some_and(|c| c.channel_group.is_some())
        }) {
            return cx.fail(
                "% Interface is a member of a port-channel; remove it from the channel first",
            );
        }
        each(cx, |cfg, _| cfg.l2 = None);
    } else {
        each(cx, |cfg, _| {
            if cfg.l2.is_none() {
                // Becoming a switch port drops the layer-3 configuration.
                cfg.l2 = Some(Switchport::new(SwMode::DynamicAuto));
                cfg.addr = None;
                cfg.secondary.clear();
            }
        });
    }
}

fn switchport_mode(cx: &mut Cx, a: &Args) {
    let mode = if a.no {
        SwMode::DynamicAuto
    } else if a.has("access") {
        SwMode::Access
    } else if a.has("trunk") {
        SwMode::Trunk
    } else if a.has("desirable") {
        SwMode::DynamicDesirable
    } else {
        SwMode::DynamicAuto
    };
    let dynamic = matches!(mode, SwMode::DynamicAuto | SwMode::DynamicDesirable);
    let conflict = selection(cx).iter().any(|id| {
        l2_of(cx, *id).is_some_and(|sw| dynamic && (sw.nonegotiate || sw.port_security.enabled))
    });
    if conflict {
        return cx.fail("Command rejected: Conflict between 'nonegotiate' or port security and 'dynamic' status.");
    }
    each_l2(cx, |sw, _| sw.mode = mode);
}

/// A VLAN referenced by an access port is created on the fly, as on real switches.
fn ensure_vlan(cx: &mut Cx, vlan: u16, what: &str) {
    if !cx.n.cfg.vlans.contains_key(&vlan) {
        cx.say(format!(
            "% {what} VLAN does not exist. Creating vlan {vlan}"
        ));
        cx.cfg().vlans.insert(vlan, Vlan::new(vlan));
        sync_vlan_dat(cx);
    }
}

fn access_vlan(cx: &mut Cx, a: &Args) {
    let vlan = if a.no {
        1
    } else {
        a.num("vlan").unwrap_or(1) as u16
    };
    if selection(cx).iter().any(|id| l2_of(cx, *id).is_none()) {
        return cx.invalid();
    }
    ensure_vlan(cx, vlan, "Access");
    each_l2(cx, |sw, _| sw.access_vlan = vlan);
}

fn voice_vlan(cx: &mut Cx, a: &Args) {
    let vlan = if a.no {
        None
    } else {
        a.num("vlan").map(|v| v as u16)
    };
    if selection(cx).iter().any(|id| l2_of(cx, *id).is_none()) {
        return cx.invalid();
    }
    if let Some(vlan) = vlan {
        ensure_vlan(cx, vlan, "Voice");
    }
    each_l2(cx, |sw, _| sw.voice_vlan = vlan);
}

fn trunk_allowed(cx: &mut Cx, a: &Args) {
    let list = match a.text("list") {
        Some(text) => match parse_ranges(text, 1, 4094) {
            Some(ids) => Some(ids),
            None => return cx.invalid(),
        },
        None => None,
    };
    let (all, none, add, remove, except) = (
        a.no || a.has("all"),
        a.has("none"),
        a.has("add"),
        a.has("remove"),
        a.has("except"),
    );
    each_l2(cx, |sw, _| match (&list, all, none) {
        (_, true, _) => sw.allowed = VlanSet::all(),
        (_, _, true) => sw.allowed = VlanSet::none(),
        (Some(ids), _, _) if add => ids.iter().for_each(|id| sw.allowed.insert(*id)),
        (Some(ids), _, _) if remove => ids.iter().for_each(|id| sw.allowed.remove(*id)),
        (Some(ids), _, _) if except => {
            sw.allowed = VlanSet::all();
            ids.iter().for_each(|id| sw.allowed.remove(*id));
        }
        (Some(ids), _, _) => sw.allowed = VlanSet::from_ids(ids),
        (None, _, _) => {}
    });
}

fn secure_mac(cx: &mut Cx, a: &Args) {
    let sticky = a.has("sticky");
    let mac = a.mac("mac");
    let no = a.no;
    let mut too_many = false;
    if mac.is_none() {
        // `mac-address sticky` alone toggles sticky learning; addresses already
        // learned change kind with it.
        for id in selection(cx) {
            let Some(vlan) = l2_of(cx, id).map(|sw| sw.access_vlan) else {
                return cx.invalid();
            };
            let learned: Vec<_> =
                cx.n.ifs
                    .get_mut(&id)
                    .map(|state| std::mem::take(&mut state.secure_macs))
                    .unwrap_or_default();
            let Some(ps) = cx
                .cfg()
                .interfaces
                .get_mut(&id)
                .and_then(|c| c.l2.as_mut())
                .map(|sw| &mut sw.port_security)
            else {
                continue;
            };
            ps.sticky = !no;
            let mut dynamic = learned;
            if no {
                dynamic.extend(
                    ps.macs
                        .iter()
                        .filter(|(_, sticky)| *sticky)
                        .map(|(mac, _)| (*mac, vlan)),
                );
                ps.macs.retain(|(_, sticky)| !sticky);
            } else {
                ps.macs
                    .extend(dynamic.drain(..).map(|(mac, _)| (mac, true)));
            }
            if let Some(state) = cx.n.ifs.get_mut(&id) {
                state.secure_macs = dynamic;
            }
        }
        return;
    }
    each_l2(cx, |sw, _| {
        let ps = &mut sw.port_security;
        match mac {
            None => {}
            Some(mac) => {
                ps.macs.retain(|(m, _)| *m != mac);
                if !no {
                    if ps.macs.len() >= ps.maximum as usize {
                        too_many = true;
                    } else {
                        ps.macs.push((mac, sticky));
                    }
                }
            }
        }
    });
    if too_many {
        cx.fail("Total secure mac-addresses on interface has reached maximum limit.");
    }
}

fn channel_group(cx: &mut Cx, a: &Args) {
    let ids = selection(cx);
    if ids.iter().any(|id| !id.is_physical()) {
        return cx.invalid();
    }
    if a.no {
        each(cx, |cfg, _| cfg.channel_group = None);
        return;
    }
    let group = a.num("group").unwrap_or(1) as u32;
    let mode = if a.has("on") {
        ChannelMode::On
    } else if a.has("active") {
        ChannelMode::Active
    } else if a.has("passive") {
        ChannelMode::Passive
    } else if a.has("desirable") {
        ChannelMode::Desirable
    } else {
        ChannelMode::Auto
    };
    if ids.iter().any(|id| l2_of(cx, *id).is_none()) {
        return cx
            .fail("% Layer-3 EtherChannels are not supported; the port must be a switch port");
    }
    // All members of a channel must use the same protocol.
    let mismatch = cx.n.cfg.interfaces.iter().any(|(id, cfg)| {
        !ids.contains(id)
            && cfg
                .channel_group
                .is_some_and(|(g, m)| g == group && m.protocol() != mode.protocol())
    });
    if mismatch {
        return cx.fail("Command rejected: the interface can not be added to the channel group (protocol mismatch)");
    }
    let po = IfId::port_channel(group);
    if !cx.n.cfg.interfaces.contains_key(&po) {
        // The port-channel is created with the first member's switch-port settings.
        let template = ids.first().and_then(|id| l2_of(cx, *id)).cloned();
        let mut cfg = IfConfig::new();
        cfg.l2 = template.or_else(|| Some(Switchport::new(SwMode::DynamicAuto)));
        if let Some(sw) = cfg.l2.as_mut() {
            sw.port_security = Default::default();
        }
        cx.cfg().interfaces.insert(po, cfg);
        cx.say(format!(
            "Creating a port-channel interface Port-channel {group}"
        ));
        cx.say("");
    }
    each(cx, |cfg, _| cfg.channel_group = Some((group, mode)));
}
