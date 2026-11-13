//! Writing the device configurations.
//!
//! Only commands the simulator implements (`docs/dev/spec-netsim.md`,
//! *CLI: command coverage*) are emitted, and every value comes from the
//! topology or the address plan — nothing a person typed reaches a line
//! without going through [`crate::util::tag`] or [`crate::util::one_line`]
//! first.

use crate::design::DeviceRole;
use crate::model::{Model, SiteModel, VlanModel};
use crate::requirements::{InternetEdge, Purpose, Redundancy, Routing, WanTopology};
use crate::topology::{Dev, PortRole, RoutedKind, Topology, channels, vlan_list};
use crate::util::{Net, banner_text, fmt_ip, one_line, tag};

/// Placeholders, not secrets. The document tells the reader to replace them
/// and nothing here may ever look like a password somebody might keep.
pub(crate) const ENABLE_SECRET: &str = "CHANGEME-enable";
pub(crate) const ADMIN_USER: &str = "netadmin";
pub(crate) const ADMIN_SECRET: &str = "CHANGEME-netadmin";
pub(crate) const SNMP_RO: &str = "CHANGEME-snmp-ro";
/// Every placeholder in a generated configuration starts with this.
pub(crate) const PLACEHOLDER_PREFIX: &str = "CHANGEME-";

const MGMT_ACL: &str = "MGMT-HOSTS";
const NAT_ACL: &str = "NAT-INSIDE";
const GUEST_ACL: &str = "GUEST-IN";

/// Lines accumulate here so indentation and ordering stay in one place.
struct Cfg {
    out: String,
}

impl Cfg {
    fn new() -> Cfg {
        Cfg { out: String::new() }
    }

    fn line(&mut self, text: impl AsRef<str>) -> &mut Cfg {
        self.out.push_str(text.as_ref());
        self.out.push('\n');
        self
    }

    fn sub(&mut self, text: impl AsRef<str>) -> &mut Cfg {
        self.out.push(' ');
        self.line(text)
    }

    fn iface(&mut self, name: &str, description: &str) -> &mut Cfg {
        self.line(format!("interface {name}"));
        if !description.is_empty() {
            self.sub(format!("description {}", one_line(description, 120)));
        }
        self
    }
}

/// Write the configuration of every network device.
pub(crate) fn write_all(model: &Model, t: &mut Topology) {
    for i in 0..t.devs.len() {
        if !t.devs[i].is_network_device() {
            continue;
        }
        let text = match t.devs[i].role {
            DeviceRole::Core => core(model, t, i),
            DeviceRole::Distribution | DeviceRole::Access => switch(model, t, i),
            DeviceRole::Edge => edge(model, t, i),
            _ => String::new(),
        };
        t.devs[i].config = text;
    }
}

fn domain(model: &Model) -> String {
    let name = tag(&model.req.name, 20).to_ascii_lowercase();
    if name.is_empty() {
        "nocturne.lab".to_string()
    } else {
        format!("{name}.lab")
    }
}

/// Hostname, local login, encrypted passwords: the same on every device.
fn head(cfg: &mut Cfg, model: &Model, dev: &Dev) {
    cfg.line(format!("hostname {}", dev.name))
        .line("no ip domain-lookup")
        .line(format!("ip domain-name {}", domain(model)))
        .line(format!("enable secret {ENABLE_SECRET}"))
        .line(format!(
            "username {ADMIN_USER} privilege 15 secret {ADMIN_SECRET}"
        ))
        .line("service password-encryption")
        .line("service timestamps log datetime msec")
        .line("logging buffered 32768")
        .line("crypto key generate rsa modulus 2048")
        .line("ip ssh version 2");
}

/// SSH-only logins from the management networks, a warning banner, and the
/// monitoring targets when they were given.
fn tail(cfg: &mut Cfg, model: &Model) {
    let services = &model.req.services;
    if let Some(host) = clean_ip(&services.syslog) {
        cfg.line(format!("logging host {host}"));
        cfg.line("logging trap informational");
    }
    if let Some(host) = clean_ip(&services.ntp) {
        cfg.line(format!("ntp server {host}"));
    }
    if let Some(host) = clean_ip(&services.snmp) {
        cfg.line(format!("snmp-server community {SNMP_RO} ro {MGMT_ACL}"));
        cfg.line(format!("snmp-server host {host} {SNMP_RO}"));
        cfg.line("snmp-server enable traps");
        cfg.line(format!(
            "snmp-server location {}",
            one_line(&model.req.name, 60)
        ));
    }
    cfg.line(format!(
        "banner motd #Authorised access only. Activity is logged. {}#",
        banner_text(&model.req.name, 40)
    ));
    cfg.line("line con 0")
        .sub("exec-timeout 15 0")
        .sub("logging synchronous")
        .sub("login local");
    for lines in ["line vty 0 4", "line vty 5 15"] {
        cfg.line(lines)
            .sub("exec-timeout 15 0")
            .sub("login local")
            .sub("transport input ssh")
            .sub(format!("access-class {MGMT_ACL} in"));
    }
}

/// An address a person typed, only if it really is an IPv4 address.
fn clean_ip(value: &Option<String>) -> Option<String> {
    let text = value.as_ref()?.trim();
    text.parse::<std::net::Ipv4Addr>()
        .ok()
        .map(|a| a.to_string())
}

/// The standard access list that says who may log in to a device.
fn mgmt_acl(cfg: &mut Cfg, model: &Model) {
    cfg.line(format!("ip access-list standard {MGMT_ACL}"));
    cfg.sub("remark Device management is reached from the management VLANs only");
    for site in &model.sites {
        if let Some(mgmt) = site.mgmt() {
            cfg.sub(format!(
                "permit {} {}",
                fmt_ip(mgmt.subnet.addr),
                mgmt.subnet.wildcard()
            ));
        }
    }
    cfg.sub(format!(
        "permit {} {}",
        fmt_ip(model.infra.addr),
        model.infra.wildcard()
    ));
    // The monitoring hosts, wherever the requirements put them: a manager
    // that cannot reach the device is a manager that cannot do its job.
    let services = &model.req.services;
    let mut seen: Vec<String> = Vec::new();
    for target in [&services.ntp, &services.syslog, &services.snmp] {
        if let Some(host) = clean_ip(target)
            && !seen.contains(&host)
        {
            seen.push(host.clone());
            cfg.sub(format!("permit host {host}"));
        }
    }
}

/// `vlan` blocks for the VLANs this switch has to know about.
fn vlan_blocks(cfg: &mut Cfg, vlans: &[(u16, String)]) {
    for (id, name) in vlans {
        cfg.line(format!("vlan {id}")).sub(format!("name {name}"));
    }
}

/// Sort key that puts interfaces in the order a real running configuration
/// does: loopbacks, port channels, then the physical ports by speed and slot.
fn if_order(name: &str) -> (u8, Vec<u32>) {
    const KINDS: [&str; 7] = [
        "Loopback",
        "Port-channel",
        "Ethernet",
        "FastEthernet",
        "GigabitEthernet",
        "TenGigabitEthernet",
        "Vlan",
    ];
    let kind = KINDS
        .iter()
        .position(|k| name.starts_with(k))
        .unwrap_or(KINDS.len()) as u8;
    let numbers = name
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect();
    (kind, numbers)
}

/// Group `GigabitEthernet1/0/5, …/6, …/9` into `interface range` commands.
fn parked(cfg: &mut Cfg, ifaces: &[String], parking: u16, switchport: bool) {
    let mut runs: Vec<(String, u32, u32)> = Vec::new();
    for iface in ifaces {
        let Some(cut) = iface.rfind('/').map(|i| i + 1).or(Some(0)) else {
            continue;
        };
        let (head, tail) = iface.split_at(cut);
        let Ok(n) = tail.parse::<u32>() else { continue };
        match runs.last_mut() {
            Some((h, _, last)) if h == head && *last + 1 == n => *last = n,
            _ => runs.push((head.to_string(), n, n)),
        }
    }
    for (head, first, last) in runs {
        if first == last {
            cfg.line(format!("interface {head}{first}"));
        } else {
            cfg.line(format!("interface range {head}{first} - {last}"));
        }
        cfg.sub("description Unused");
        if switchport {
            cfg.sub("switchport mode access")
                .sub(format!("switchport access vlan {parking}"));
        }
        cfg.sub("shutdown");
    }
}

/// `switchport` settings shared by every trunk.
fn trunk(cfg: &mut Cfg, allowed: &[u16], parking: u16, dot1q: bool) {
    if dot1q {
        cfg.sub("switchport trunk encapsulation dot1q");
    }
    cfg.sub("switchport mode trunk")
        .sub("switchport nonegotiate")
        .sub(format!("switchport trunk native vlan {parking}"))
        .sub(format!(
            "switchport trunk allowed vlan {}",
            vlan_list(allowed)
        ));
}

// -------------------------------------------------------------------- core

/// Which core is the active gateway for a VLAN: they alternate, and spanning
/// tree follows so the traffic does not cross the peer link to get out.
pub(crate) fn hsrp_active(site: &SiteModel, vlan: &VlanModel) -> u32 {
    let index = site.vlans.iter().position(|v| v.id == vlan.id).unwrap_or(0);
    (index % 2) as u32
}

fn core(model: &Model, t: &Topology, i: usize) -> String {
    let dev = &t.devs[i];
    let site = &model.sites[dev.site];
    let two = site.counts.cores == 2;
    let me = dev.idx - 1;
    let peer = t.cores[dev.site]
        .iter()
        .find(|&&x| x != i)
        .map(|&x| &t.devs[x]);
    let mut cfg = Cfg::new();
    head(&mut cfg, model, dev);
    cfg.line("ip routing");

    let mut vlans: Vec<(u16, String)> = site.vlans.iter().map(|v| (v.id, v.name.clone())).collect();
    if two {
        vlans.push((model.transit_vlan, "TRANSIT".to_string()));
    }
    vlans.push((model.parking_vlan, "PARKED".to_string()));
    vlan_blocks(&mut cfg, &vlans);

    cfg.line("spanning-tree mode rapid-pvst");
    let mine: Vec<u16> = site
        .vlans
        .iter()
        .filter(|v| !two || hsrp_active(site, v) == me)
        .map(|v| v.id)
        .collect();
    let theirs: Vec<u16> = site
        .vlans
        .iter()
        .filter(|v| two && hsrp_active(site, v) != me)
        .map(|v| v.id)
        .collect();
    if !mine.is_empty() {
        cfg.line(format!(
            "spanning-tree vlan {} root primary",
            vlan_list(&mine)
        ));
    }
    if !theirs.is_empty() {
        cfg.line(format!(
            "spanning-tree vlan {} root secondary",
            vlan_list(&theirs)
        ));
    }

    // DHCP lives on the first core; the second relays to it.
    let pools: Vec<&VlanModel> = site.vlans.iter().filter(|v| v.pool.is_some()).collect();
    if me == 0 && !pools.is_empty() {
        for vlan in &pools {
            let Some((first, _)) = vlan.pool else {
                continue;
            };
            cfg.line(format!(
                "ip dhcp excluded-address {} {}",
                fmt_ip(vlan.subnet.addr),
                fmt_ip(first - 1)
            ));
        }
        for vlan in &pools {
            cfg.line(format!("ip dhcp pool {}-{}", site.tag, vlan.name))
                .sub(format!(
                    "network {} {}",
                    fmt_ip(vlan.subnet.addr),
                    vlan.subnet.netmask()
                ))
                .sub(format!("default-router {}", fmt_ip(vlan.gateway)))
                .sub(format!("dns-server {}", crate::model::INTERNET_HOST))
                .sub(format!("domain-name {}", domain(model)));
            if vlan.purpose == Purpose::Guest {
                cfg.sub("lease 0 8");
            }
        }
    }

    mgmt_acl(&mut cfg, model);
    if let Some(guest) = site.vlan(Purpose::Guest) {
        guest_acl(&mut cfg, model, guest);
    }

    if let Some(loopback) = dev.loopback {
        cfg.iface("Loopback0", "Router id and management")
            .sub(format!("ip address {} 255.255.255.255", fmt_ip(loopback)));
    }
    for (group, description, allowed) in channels(dev) {
        cfg.iface(&format!("Port-channel{group}"), &description);
        trunk(&mut cfg, &allowed, model.parking_vlan, true);
    }

    ports(&mut cfg, model, t, i, true);

    // The gateways.
    for vlan in &site.vlans {
        let addr = if two {
            vlan.subnet.nth(2 + me)
        } else {
            Some(vlan.gateway)
        };
        let Some(addr) = addr else { continue };
        cfg.iface(
            &format!("Vlan{}", vlan.id),
            &format!("{} gateway", vlan.name),
        );
        cfg.sub(format!(
            "ip address {} {}",
            fmt_ip(addr),
            vlan.subnet.netmask()
        ));
        if two {
            let active = hsrp_active(site, vlan) == me;
            cfg.sub("standby version 2").sub(format!(
                "standby {} ip {}",
                vlan.id,
                fmt_ip(vlan.gateway)
            ));
            if active {
                cfg.sub(format!("standby {} priority 110", vlan.id))
                    .sub(format!("standby {} preempt", vlan.id));
            }
            // Only the first core runs DHCP, so the second has to relay.
            if me == 1
                && vlan.pool.is_some()
                && let Some(server) = t.cores[dev.site].first().and_then(|&c| t.devs[c].loopback)
            {
                cfg.sub(format!("ip helper-address {}", fmt_ip(server)));
            }
        }
        if vlan.purpose == Purpose::Guest {
            cfg.sub(format!("ip access-group {GUEST_ACL} in"));
        }
        cfg.sub("no shutdown");
    }
    if two && let Some(transit) = site.transit {
        let peer_name = peer.map(|p| p.name.as_str()).unwrap_or("the peer core");
        cfg.iface(
            &format!("Vlan{}", model.transit_vlan),
            &format!("Transit to {peer_name}"),
        );
        cfg.sub(format!(
            "ip address {} {}",
            fmt_ip(transit.first_host() + me),
            transit.netmask()
        ))
        .sub("no shutdown");
    }

    routing_core(&mut cfg, model, t, i);
    tail(&mut cfg, model);
    cfg.out
}

/// Visitors reach the Internet and their own gateway, and nothing else.
fn guest_acl(cfg: &mut Cfg, model: &Model, guest: &VlanModel) {
    let (g, gw) = (fmt_ip(guest.subnet.addr), guest.subnet.wildcard());
    let base = model.internal();
    cfg.line(format!("ip access-list extended {GUEST_ACL}"))
        .sub("remark Visitors reach the Internet, never the internal networks")
        .sub(format!("permit ip {g} {gw} {g} {gw}"))
        .sub(format!(
            "deny ip {g} {gw} {} {}",
            fmt_ip(base.addr),
            base.wildcard()
        ))
        .sub(format!("permit ip {g} {gw} any"));
}

fn routing_core(cfg: &mut Cfg, model: &Model, t: &Topology, i: usize) {
    let dev = &t.devs[i];
    let site = &model.sites[dev.site];
    let base = model.internal();
    // The far end of this core's link to its edge router, if it has one.
    let edge_hop = dev.ports.iter().find_map(|p| match &p.role {
        PortRole::Routed {
            net,
            addr,
            kind: RoutedKind::Core,
        } => Some(if *addr == net.first_host() {
            net.first_host() + 1
        } else {
            net.first_host()
        }),
        _ => None,
    });
    let peer_hop = site.transit.map(|transit| {
        let me = dev.idx - 1;
        transit.first_host() + 1 - me
    });

    match model.req.routing {
        Routing::Ospf => {
            let Some(id) = dev.loopback else { return };
            cfg.line("router ospf 1")
                .sub(format!("router-id {}", fmt_ip(id)))
                .sub("passive-interface default");
            for port in &dev.ports {
                if matches!(
                    port.role,
                    PortRole::Routed {
                        kind: RoutedKind::Core,
                        ..
                    }
                ) {
                    cfg.sub(format!("no passive-interface {}", port.iface));
                }
            }
            if site.transit.is_some() {
                cfg.sub(format!("no passive-interface Vlan{}", model.transit_vlan));
            }
            cfg.sub(format!(
                "network {} {} area 0",
                fmt_ip(base.addr),
                base.wildcard()
            ));
        }
        Routing::Static => {
            if let Some(hop) = edge_hop {
                cfg.line(format!("ip route 0.0.0.0 0.0.0.0 {}", fmt_ip(hop)));
                // The loopback of the edge router on the far end of that link,
                // named rather than left to the default route: the peer core
                // sends this core traffic for it, and a default route would
                // hand it straight back.
                let me = (dev.idx - 1) as usize;
                if let Some(loopback) = t.edges[dev.site].get(me).and_then(|&e| t.devs[e].loopback)
                {
                    cfg.line(format!(
                        "ip route {} 255.255.255.255 {}",
                        fmt_ip(loopback),
                        fmt_ip(hop)
                    ));
                }
            }
            if let Some(hop) = peer_hop {
                // Through the peer core when this core's own way out is gone.
                let distance = if edge_hop.is_some() { " 20" } else { "" };
                cfg.line(format!(
                    "ip route 0.0.0.0 0.0.0.0 {}{distance}",
                    fmt_ip(hop)
                ));
                // Everything in this site's infrastructure slice that sits
                // behind the peer core, named one by one: a summary route
                // towards the edge would send these back and forth, because
                // the edge router summarises the slice towards the cores.
                for &c in &t.cores[dev.site] {
                    if c != i
                        && let Some(loopback) = t.devs[c].loopback
                    {
                        cfg.line(format!(
                            "ip route {} 255.255.255.255 {}",
                            fmt_ip(loopback),
                            fmt_ip(hop)
                        ));
                    }
                }
                let me = (dev.idx - 1) as usize;
                for (e, &edge) in t.edges[dev.site].iter().enumerate() {
                    if e == me {
                        continue;
                    }
                    if let Some(loopback) = t.devs[edge].loopback {
                        cfg.line(format!(
                            "ip route {} 255.255.255.255 {}",
                            fmt_ip(loopback),
                            fmt_ip(hop)
                        ));
                    }
                    if let Some(net) = t.devs[edge].ports.iter().find_map(|p| match &p.role {
                        PortRole::Routed {
                            net,
                            kind: RoutedKind::Core,
                            ..
                        } => Some(*net),
                        _ => None,
                    }) {
                        cfg.line(format!(
                            "ip route {} {} {}",
                            fmt_ip(net.addr),
                            net.netmask(),
                            fmt_ip(hop)
                        ));
                    }
                }
            }
            // The WAN lands on the site's first edge router, so a core that
            // does not face it has to be told which way the other sites are:
            // its own default points at its own edge, which only knows the
            // Internet.
            if model.sites.len() > 1 && !model.wan_links.is_empty() {
                let me = dev.idx - 1;
                let towards = if me == 0 { edge_hop } else { peer_hop };
                if let Some(hop) = towards {
                    cfg.line(format!(
                        "ip route {} {} {}",
                        fmt_ip(base.addr),
                        base.netmask(),
                        fmt_ip(hop)
                    ));
                }
            }
        }
    }
}

// ---------------------------------------------------- access / distribution

fn switch(model: &Model, t: &Topology, i: usize) -> String {
    let dev = &t.devs[i];
    let site = &model.sites[dev.site];
    let l3 = dev.model_id == "switch-l3";
    let mut cfg = Cfg::new();
    head(&mut cfg, model, dev);

    let mut ids: Vec<u16> = Vec::new();
    for port in &dev.ports {
        match &port.role {
            PortRole::Access { vlan, voice, .. } => {
                ids.push(*vlan);
                ids.extend(voice.iter().copied());
            }
            PortRole::Trunk { allowed, .. } => ids.extend(allowed.iter().copied()),
            _ => {}
        }
    }
    ids.sort_unstable();
    ids.dedup();
    let vlans: Vec<(u16, String)> = ids
        .iter()
        .map(|id| {
            let name = site
                .vlans
                .iter()
                .find(|v| v.id == *id)
                .map(|v| v.name.clone())
                .unwrap_or_else(|| format!("VLAN{id}"));
            (*id, name)
        })
        .chain(std::iter::once((model.parking_vlan, "PARKED".to_string())))
        .collect();
    vlan_blocks(&mut cfg, &vlans);
    cfg.line("spanning-tree mode rapid-pvst");

    mgmt_acl(&mut cfg, model);

    for (group, description, allowed) in channels(dev) {
        cfg.iface(&format!("Port-channel{group}"), &description);
        trunk(&mut cfg, &allowed, model.parking_vlan, l3);
    }

    ports(&mut cfg, model, t, i, l3);

    if let (Some(mgmt), Some(vlan)) = (dev.mgmt, site.mgmt()) {
        cfg.iface(&format!("Vlan{}", vlan.id), "Management")
            .sub(format!(
                "ip address {} {}",
                fmt_ip(mgmt),
                vlan.subnet.netmask()
            ))
            .sub("no shutdown");
        cfg.line(format!("ip default-gateway {}", fmt_ip(vlan.gateway)));
    }
    tail(&mut cfg, model);
    cfg.out
}

// -------------------------------------------------------------------- edge

fn edge(model: &Model, t: &Topology, i: usize) -> String {
    let dev = &t.devs[i];
    let site = &model.sites[dev.site];
    let base = model.internal();
    let nat = model.req.services.nat
        && dev.ports.iter().any(|p| {
            matches!(
                p.role,
                PortRole::Routed {
                    kind: RoutedKind::Internet,
                    ..
                }
            )
        });
    let mut cfg = Cfg::new();
    head(&mut cfg, model, dev);
    mgmt_acl(&mut cfg, model);
    if nat {
        cfg.line(format!("ip access-list standard {NAT_ACL}"))
            .sub("remark The internal address space, translated on the way out")
            .sub(format!("permit {} {}", fmt_ip(base.addr), base.wildcard()));
    }
    if let Some(loopback) = dev.loopback {
        cfg.iface("Loopback0", "Router id and management")
            .sub(format!("ip address {} 255.255.255.255", fmt_ip(loopback)));
    }
    ports(&mut cfg, model, t, i, false);
    routing_edge(&mut cfg, model, t, i);
    if nat
        && let Some(out) = dev.ports.iter().find(|p| {
            matches!(
                p.role,
                PortRole::Routed {
                    kind: RoutedKind::Internet,
                    ..
                }
            )
        })
    {
        cfg.line(format!(
            "ip nat inside source list {NAT_ACL} interface {} overload",
            out.iface
        ));
    }
    let _ = site;
    tail(&mut cfg, model);
    cfg.out
}

fn routing_edge(cfg: &mut Cfg, model: &Model, t: &Topology, i: usize) {
    let dev = &t.devs[i];
    let base = model.internal();
    let site = &model.sites[dev.site];
    let internet = dev.ports.iter().find_map(|p| match &p.role {
        PortRole::Routed {
            net,
            addr,
            kind: RoutedKind::Internet,
        } => Some((*net, *addr)),
        _ => None,
    });
    if let Some((net, addr)) = internet {
        let upstream = if addr == net.first_host() {
            net.first_host() + 1
        } else {
            net.first_host()
        };
        cfg.line(format!("ip route 0.0.0.0 0.0.0.0 {}", fmt_ip(upstream)));
    }

    match model.req.routing {
        Routing::Ospf => {
            let Some(id) = dev.loopback else { return };
            cfg.line("router ospf 1")
                .sub(format!("router-id {}", fmt_ip(id)));
            for port in &dev.ports {
                if matches!(
                    port.role,
                    PortRole::Routed {
                        kind: RoutedKind::Internet,
                        ..
                    }
                ) {
                    cfg.sub(format!("passive-interface {}", port.iface));
                }
            }
            cfg.sub(format!(
                "network {} {} area 0",
                fmt_ip(base.addr),
                base.wildcard()
            ));
            if internet.is_some() {
                cfg.sub("default-information originate");
            }
        }
        Routing::Static => {
            // The site behind this router.
            if let Some(hop) = dev.ports.iter().find_map(|p| match &p.role {
                PortRole::Routed {
                    net,
                    addr,
                    kind: RoutedKind::Core,
                } => Some(far_end(*net, *addr)),
                _ => None,
            }) {
                cfg.line(format!(
                    "ip route {} {} {}",
                    fmt_ip(site.block.addr),
                    site.block.netmask(),
                    fmt_ip(hop)
                ));
                // And this site's loopbacks and transit links, which live in
                // its own slice of the infrastructure block.
                cfg.line(format!(
                    "ip route {} {} {}",
                    fmt_ip(site.infra.addr),
                    site.infra.netmask(),
                    fmt_ip(hop)
                ));
            }
            // The other sites — but only from the edge router the WAN
            // actually lands on. The second edge router of a site has no WAN
            // port, so for it everything internal goes back to its core.
            let has_wan = dev.ports.iter().any(|p| {
                matches!(
                    p.role,
                    PortRole::Routed {
                        kind: RoutedKind::Wan,
                        ..
                    }
                )
            });
            let hub_hop = model.wan_links.iter().filter(|_| has_wan).find_map(|link| {
                let (near, far) = if link.a == dev.site {
                    (link.subnet.first_host(), link.subnet.first_host() + 1)
                } else if link.b == dev.site {
                    (link.subnet.first_host() + 1, link.subnet.first_host())
                } else {
                    return None;
                };
                let peer = if link.a == dev.site { link.b } else { link.a };
                let _ = near;
                Some((peer, far))
            });
            for link in model.wan_links.iter().filter(|_| has_wan) {
                let (peer, hop) = if link.a == dev.site {
                    (link.b, link.subnet.first_host() + 1)
                } else if link.b == dev.site {
                    (link.a, link.subnet.first_host())
                } else {
                    continue;
                };
                for block in [model.sites[peer].block, model.sites[peer].infra] {
                    cfg.line(format!(
                        "ip route {} {} {}",
                        fmt_ip(block.addr),
                        block.netmask(),
                        fmt_ip(hop)
                    ));
                }
            }
            // A spoke that has no Internet of its own sends the rest to the hub.
            if internet.is_none()
                && model.req.wan.internet != InternetEdge::None
                && let Some((_, hop)) = hub_hop
            {
                cfg.line(format!("ip route 0.0.0.0 0.0.0.0 {}", fmt_ip(hop)));
            }
            // Hub and spoke: a spoke reaches the other spokes through the hub.
            if model.req.wan.topology == WanTopology::HubAndSpoke
                && dev.site != model.hub
                && let Some((_, hop)) = hub_hop
            {
                cfg.line(format!(
                    "ip route {} {} {}",
                    fmt_ip(base.addr),
                    base.netmask(),
                    fmt_ip(hop)
                ));
            }
            // A second edge router carries no WAN link, so everything
            // internal it does not have a better route for goes back to its
            // core, which knows the way to the other sites.
            if !has_wan
                && model.sites.len() > 1
                && !model.wan_links.is_empty()
                && let Some(hop) = dev.ports.iter().find_map(|p| match &p.role {
                    PortRole::Routed {
                        net,
                        addr,
                        kind: RoutedKind::Core,
                    } => Some(far_end(*net, *addr)),
                    _ => None,
                })
            {
                cfg.line(format!(
                    "ip route {} {} {}",
                    fmt_ip(base.addr),
                    base.netmask(),
                    fmt_ip(hop)
                ));
            }
        }
    }
}

fn far_end(net: Net, addr: u32) -> u32 {
    if addr == net.first_host() {
        net.first_host() + 1
    } else {
        net.first_host()
    }
}

// ------------------------------------------------------------------- ports

/// Every physical port of a device, in running-configuration order: the ones
/// that are in use as themselves, the ones that are not as parked ranges.
fn ports(cfg: &mut Cfg, model: &Model, t: &Topology, i: usize, dot1q: bool) {
    let dev = &t.devs[i];
    let switchport = dev.model_id.starts_with("switch");
    let mut items: Vec<(&str, Option<&crate::topology::Port>)> = dev
        .ports
        .iter()
        .map(|p| (p.iface.as_str(), Some(p)))
        .chain(dev.spare.iter().map(|s| (s.as_str(), None)))
        .collect();
    items.sort_by_key(|(name, _)| if_order(name));
    let mut run: Vec<String> = Vec::new();
    for (name, port) in items {
        let Some(port) = port else {
            run.push(name.to_string());
            continue;
        };
        if !run.is_empty() {
            parked(cfg, &run, model.parking_vlan, switchport);
            run.clear();
        }
        match &port.role {
            PortRole::Trunk { channel, allowed } => {
                cfg.iface(&port.iface, &port.description);
                trunk(cfg, allowed, model.parking_vlan, dot1q);
                if let Some(group) = channel {
                    cfg.sub(format!("channel-group {group} mode active"));
                }
            }
            PortRole::Access {
                vlan,
                voice,
                occupied,
            } => {
                cfg.iface(&port.iface, &port.description);
                cfg.sub("switchport mode access")
                    .sub(format!("switchport access vlan {vlan}"));
                if let Some(voice) = voice {
                    cfg.sub(format!("switchport voice vlan {voice}"));
                }
                cfg.sub("spanning-tree portfast")
                    .sub("spanning-tree bpduguard enable");
                let _ = occupied;
            }
            PortRole::Routed { net, addr, kind } => {
                cfg.iface(&port.iface, &port.description);
                if switchport {
                    cfg.sub("no switchport");
                }
                cfg.sub(format!("ip address {} {}", fmt_ip(*addr), net.netmask()));
                if model.req.services.nat && dev.role == DeviceRole::Edge {
                    match kind {
                        RoutedKind::Internet => cfg.sub("ip nat outside"),
                        _ => cfg.sub("ip nat inside"),
                    };
                }
                cfg.sub("no shutdown");
            }
            PortRole::Host => {}
        }
    }
    if !run.is_empty() {
        parked(cfg, &run, model.parking_vlan, switchport);
    }
}

/// `Redundancy` only reaches the configurations through the topology, but the
/// document explains it, so keep the mapping in one place.
pub(crate) fn redundancy_note(redundancy: Redundancy) -> &'static str {
    match redundancy {
        Redundancy::None => {
            "one core switch and one uplink per access switch: cheapest to build, and every \
             box is a single point of failure"
        }
        Redundancy::Uplinks => {
            "one core switch, every access switch dual-homed to it with an EtherChannel, so a \
             cut uplink costs bandwidth and not service"
        }
        Redundancy::Full => {
            "two core switches sharing every gateway with HSRP, access switches homed to both, \
             and two Internet edge routers"
        }
    }
}
