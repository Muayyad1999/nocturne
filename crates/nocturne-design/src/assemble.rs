//! Turning the model and the topology into the published [`Design`].

use nocturne_netsim::{
    CableKind, Endpoint, FlowPattern, FlowSpec, LabSpec, LinkSpec, NodeSpec, NoteSpec,
};

use crate::checks;
use crate::config;
use crate::design::{
    BomLine, Decision, Design, DevicePlan, DeviceRole, InterfaceUse, LinkPlan, Plan, SitePlan,
    VlanPlan,
};
use crate::model::{INTERNET_HOST, Model};
use crate::requirements::{InternetEdge, Purpose, Redundancy, Routing, WanTopology};
use crate::topology::{PortRole, RoutedKind, Topology};
use crate::util::{fmt_ip, one_line};

/// Every network device comes with a host key, so SSH to it works.
const RSA_BITS: u16 = 2048;

pub(crate) fn finish(mut model: Model, mut t: Topology) -> Design {
    config::write_all(&model, &mut t);
    late_decisions(&mut model, &t);
    let checks = checks::derive(&model, &t);
    let lab = lab_spec(&model, &t);
    let plan = Plan {
        sites: site_plans(&model, &t),
        vlans: vlan_plans(&model),
        links: link_plans(&model, &t),
        devices: device_plans(&model, &t),
        bill_of_materials: bom(&model, &t),
        checks,
    };
    Design {
        lab,
        plan,
        decisions: model.decisions,
        warnings: model.warnings,
    }
}

/// The notes that can only be written once the whole design exists.
fn late_decisions(model: &mut Model, t: &Topology) {
    let req = model.req.clone();
    model.decisions.push(Decision::global(
        "Redundancy",
        format!(
            "Each site is built as a collapsed core: {}.",
            config::redundancy_note(req.redundancy)
        ),
    ));
    model.decisions.push(Decision::global(
        "Routing",
        match req.routing {
            Routing::Ospf => format!(
                "OSPF process 1, area 0 across the core-to-edge links, the core peer links and \
                 the WAN. The user VLANs are passive, so no neighbour can appear on a port a \
                 user is plugged into, and every router-id is its loopback inside {}.",
                model.infra
            ),
            Routing::Static => format!(
                "Static routing: each core sends everything it does not know to its edge router, \
                 each edge router holds a summary route for its own site ({} per site) and one \
                 route per remote site. Nothing is redistributed, so there is no protocol to \
                 go wrong — and nothing reconverges on its own either.",
                model
                    .sites
                    .first()
                    .map(|s| s.block.to_string())
                    .unwrap_or_default()
            ),
        },
    ));
    if req.redundancy == Redundancy::Full {
        model.decisions.push(Decision::global(
            "Gateways",
            "The two cores of a site share every gateway address with HSRP version 2, and they \
             take it in turns to be the active one VLAN by VLAN. Spanning-tree root priority \
             follows the same split, so the traffic of a VLAN leaves through the core that \
             owns its gateway instead of crossing the peer link."
                .to_string(),
        ));
    }
    model.decisions.push(Decision::global(
        "Hardening",
        format!(
            "Every device: SSH-only VTYs with `access-class {}`, `login local` against one \
             privilege-15 account, `service password-encryption`, a warning banner, unused \
             ports shut and parked in VLAN {}, and `spanning-tree portfast` with BPDU guard on \
             every port a user can plug into. Every password in these configurations is a \
             placeholder beginning `{}`.",
            "MGMT-HOSTS",
            model.parking_vlan,
            config::PLACEHOLDER_PREFIX
        ),
    ));

    // Things that are true of the design but not quite what was asked for.
    let voice_sites: Vec<&str> = model
        .sites
        .iter()
        .filter(|s| s.vlan(Purpose::Voice).is_some())
        .map(|s| s.name.as_str())
        .collect();
    if !voice_sites.is_empty() {
        model.warnings.push(format!(
            "Every user port carries `switchport voice vlan` so a desk telephone can pass the \
             workstation's traffic through untagged — except the handful of ports the lab cables \
             a test workstation to, which are data only: the simulated workstation drops the \
             tagged voice VLAN a real telephone would absorb. Affected sites: {}.",
            voice_sites.join(", ")
        ));
    }
    if model.sites.iter().any(|s| s.counts.aps > 0) {
        model.warnings.push(
            "The access points are cabled and their ports are configured, but the lab's guest \
             test clients are cabled straight to an access port in the guest VLAN: the \
             simulator's access point has a radio port it will not let anything connect to, so \
             a client cannot be put behind one."
                .to_string(),
        );
    }
    if req.redundancy == Redundancy::Full && !model.wan_links.is_empty() {
        model.warnings.push(format!(
            "Full redundancy was asked for and the LAN has none: two cores, two edge routers, \
             every access switch homed to both. The WAN does not: each of the {} site-to-site \
             links lands on the first edge router of each site, so a cut circuit isolates that \
             adjacency until it is repaired. A second circuit per adjacency is a commercial \
             decision, not a design one — ask for one and terminate it on the second edge router.",
            model.wan_links.len()
        ));
    }
    if req.redundancy != Redundancy::Full {
        let servers: u32 = model.sites.iter().map(|s| s.servers).sum();
        if servers > 0 {
            model.warnings.push(
                "The servers hang off the single core switch. Nothing is wrong with that in a \
                 branch, but it does mean a core maintenance window is a server outage."
                    .to_string(),
            );
        }
    }
    for (what, target) in [
        ("time source", &req.services.ntp),
        ("syslog collector", &req.services.syslog),
        ("SNMP manager", &req.services.snmp),
    ] {
        let Some(addr) = target
            .as_ref()
            .and_then(|t| t.trim().parse::<std::net::Ipv4Addr>().ok())
        else {
            if target.as_ref().is_some_and(|t| !t.trim().is_empty()) {
                model.warnings.push(format!(
                    "the {what} \"{}\" is not an IPv4 address, so it was left out of the \
                     configurations",
                    one_line(target.as_deref().unwrap_or(""), 40)
                ));
            }
            continue;
        };
        if !model.base.contains(u32::from(addr)) {
            model.warnings.push(format!(
                "the {what} {addr} is outside {}, so the devices will try to reach it through \
                 the Internet edge; put it inside the internal address space or expect it to \
                 be unreachable from a site with no Internet breakout",
                model.base
            ));
        }
    }
    if req.wan.internet == InternetEdge::AtHub && model.sites.len() > 1 {
        model.warnings.push(format!(
            "All Internet traffic leaves through {}. Every other site depends on that site and \
             on its WAN link being up to reach anything outside.",
            model
                .sites
                .get(model.hub)
                .map(|s| s.name.as_str())
                .unwrap_or("the hub")
        ));
    }
    let samples: usize = t
        .devs
        .iter()
        .filter(|d| d.role == DeviceRole::Host && d.model_id != "nms")
        .count();
    model.decisions.push(Decision::global(
        "What the lab contains",
        format!(
            "{} devices and {} cables, with {samples} sample end systems — one per VLAN per \
             site by default — so every VLAN, every gateway and every access list can be \
             probed. The plan sizes the VLANs for the full user counts; the lab does not place \
             thousands of workstations to prove the same thing.",
            t.devs.len(),
            t.cables.len()
        ),
    ));
}

// ------------------------------------------------------------------ the lab

fn lab_spec(model: &Model, t: &Topology) -> LabSpec {
    let nodes: Vec<NodeSpec> = t
        .devs
        .iter()
        .map(|dev| NodeSpec {
            name: dev.name.clone(),
            model: dev.model_id.to_string(),
            x: dev.x,
            y: dev.y,
            powered: true,
            startup_config: dev.is_network_device().then(|| dev.config.clone()),
            host: dev.host.clone(),
            vlan_dat: None,
            rsa_key_bits: dev.is_network_device().then_some(RSA_BITS),
        })
        .collect();
    let links: Vec<LinkSpec> = t
        .cables
        .iter()
        .map(|c| LinkSpec {
            a: Endpoint {
                node: t.devs[c.a].name.clone(),
                interface: c.a_if.clone(),
            },
            b: Endpoint {
                node: t.devs[c.b].name.clone(),
                interface: c.b_if.clone(),
            },
            cable: CableKind::Auto,
        })
        .collect();
    let notes: Vec<NoteSpec> = model
        .sites
        .iter()
        .enumerate()
        .filter_map(|(s, site)| {
            let (x, width) = t.site_spans.get(s).copied()?;
            let devices = t.devs.iter().filter(|d| d.site == s).count();
            Some(NoteSpec {
                x: x + 16.0,
                y: 96.0,
                w: (width - 32.0).max(220.0),
                h: 72.0,
                text: format!(
                    "{} — {} · {} VLANs · {devices} devices{}",
                    site.name,
                    site.block,
                    site.vlans.len(),
                    if site.internet {
                        ", Internet breakout"
                    } else if site.is_hub {
                        ", hub"
                    } else {
                        ""
                    }
                ),
                color: if site.is_hub { "#f6e3b4" } else { "#cfe3f7" }.to_string(),
            })
        })
        .collect();
    let flows: Vec<FlowSpec> = model
        .sites
        .iter()
        .enumerate()
        .filter(|(_, site)| site.internet || model.req.wan.internet != InternetEdge::None)
        .filter_map(|(s, site)| {
            let users = site.vlan(Purpose::Users)?.id;
            let src = t
                .devs
                .iter()
                .find(|d| d.site == s && d.role == DeviceRole::Host && d.vlan == Some(users))?;
            Some(FlowSpec {
                name: format!("{} web browsing", site.name),
                src: src.name.clone(),
                dst_ip: INTERNET_HOST.to_string(),
                bps: 2_000_000,
                pattern: FlowPattern::Sine { period_s: 120 },
                enabled: true,
                dscp: None,
            })
        })
        .collect();
    LabSpec {
        name: one_line(&model.req.name, 60),
        description: description(model),
        nodes,
        links,
        notes,
        flows,
        seed: model.req.seed,
    }
}

fn description(model: &Model) -> String {
    let users: u32 = model.sites.iter().map(|s| s.users).sum();
    let wan = match model.req.wan.topology {
        WanTopology::None => "no WAN".to_string(),
        WanTopology::HubAndSpoke => format!(
            "hub and spoke on {}",
            model
                .sites
                .get(model.hub)
                .map(|s| s.name.as_str())
                .unwrap_or("the hub")
        ),
        WanTopology::FullMesh => "a full mesh between the sites".to_string(),
    };
    format!(
        "{}: {} site{} for {users} user{}, {wan}, {}. {}. Log in to any device over SSH as {} \
         (every password in this lab is a placeholder).",
        one_line(&model.req.name, 60),
        model.sites.len(),
        if model.sites.len() == 1 { "" } else { "s" },
        if users == 1 { "" } else { "s" },
        match model.req.wan.internet {
            InternetEdge::None => "no Internet access".to_string(),
            InternetEdge::AtHub => "one Internet breakout at the hub".to_string(),
            InternetEdge::PerSite => "an Internet breakout at every site".to_string(),
        },
        config::redundancy_note(model.req.redundancy),
        config::ADMIN_USER,
    )
}

// ----------------------------------------------------------------- the plan

fn site_plans(model: &Model, t: &Topology) -> Vec<SitePlan> {
    model
        .sites
        .iter()
        .enumerate()
        .map(|(s, site)| SitePlan {
            name: site.name.clone(),
            tag: site.tag.clone(),
            block: site.block.to_string(),
            users: site.users,
            devices: t
                .devs
                .iter()
                .filter(|d| d.site == s && d.role != DeviceRole::Host)
                .map(|d| d.name.clone())
                .collect(),
            vlans: site.vlans.iter().map(|v| v.id).collect(),
            is_hub: site.is_hub,
        })
        .collect()
}

fn vlan_plans(model: &Model) -> Vec<VlanPlan> {
    let mut out = Vec::new();
    for site in &model.sites {
        for vlan in &site.vlans {
            let (first, last) = vlan
                .pool
                .map(|(a, b)| (fmt_ip(a), fmt_ip(b)))
                .unwrap_or_default();
            out.push(VlanPlan {
                site: site.name.clone(),
                id: vlan.id,
                name: vlan.name.clone(),
                purpose: vlan.purpose,
                subnet: vlan.subnet.to_string(),
                gateway: fmt_ip(vlan.gateway),
                dhcp_first: first,
                dhcp_last: last,
                hosts_needed: vlan.hosts_needed,
                hosts_available: vlan.subnet.usable().min(u64::from(u32::MAX)) as u32,
            });
        }
    }
    out
}

fn link_plans(model: &Model, t: &Topology) -> Vec<LinkPlan> {
    let _ = model;
    t.cables
        .iter()
        .map(|c| LinkPlan {
            a: t.devs[c.a].name.clone(),
            a_if: c.a_if.clone(),
            b: t.devs[c.b].name.clone(),
            b_if: c.b_if.clone(),
            kind: c.kind,
            subnet: c.subnet.map(|n| n.to_string()).unwrap_or_default(),
            carries: c.carries.clone(),
        })
        .collect()
}

fn device_plans(model: &Model, t: &Topology) -> Vec<DevicePlan> {
    t.devs
        .iter()
        .map(|dev| {
            let site = model
                .sites
                .get(dev.site)
                .map(|s| s.name.clone())
                .unwrap_or_default();
            let mut interfaces: Vec<InterfaceUse> = Vec::new();
            if let Some(loopback) = dev.loopback {
                interfaces.push(InterfaceUse {
                    name: "Loopback0".to_string(),
                    description: "Router id and management".to_string(),
                    mode: "loopback".to_string(),
                    ip: format!("{}/32", fmt_ip(loopback)),
                });
            }
            for port in &dev.ports {
                let (mode, ip) = match &port.role {
                    PortRole::Trunk { channel, .. } => (
                        match channel {
                            Some(n) => format!("trunk (Port-channel{n})"),
                            None => "trunk".to_string(),
                        },
                        String::new(),
                    ),
                    PortRole::Access { vlan, voice, .. } => (
                        match voice {
                            Some(v) => format!("access {vlan} + voice {v}"),
                            None => format!("access {vlan}"),
                        },
                        String::new(),
                    ),
                    PortRole::Routed { net, addr, kind } => (
                        match kind {
                            RoutedKind::Internet => "routed (outside)".to_string(),
                            _ => "routed".to_string(),
                        },
                        format!("{}/{}", fmt_ip(*addr), net.len),
                    ),
                    PortRole::Host => {
                        ("host".to_string(), dev.mgmt.map(fmt_ip).unwrap_or_default())
                    }
                };
                interfaces.push(InterfaceUse {
                    name: port.iface.clone(),
                    description: port.description.clone(),
                    mode,
                    ip,
                });
            }
            if dev.role == DeviceRole::Core {
                if let Some(site) = model.sites.get(dev.site) {
                    for vlan in &site.vlans {
                        let addr = if site.counts.cores == 2 {
                            vlan.subnet.nth(1 + dev.idx)
                        } else {
                            Some(vlan.gateway)
                        };
                        let Some(addr) = addr else { continue };
                        interfaces.push(InterfaceUse {
                            name: format!("Vlan{}", vlan.id),
                            description: format!("{} gateway", vlan.name),
                            mode: if site.counts.cores == 2 {
                                format!("svi (HSRP {} → {})", vlan.id, fmt_ip(vlan.gateway))
                            } else {
                                "svi".to_string()
                            },
                            ip: format!("{}/{}", fmt_ip(addr), vlan.subnet.len),
                        });
                    }
                }
            } else if let Some(mgmt) = dev.mgmt
                && matches!(dev.role, DeviceRole::Access | DeviceRole::Distribution)
                && let Some(vlan) = model.sites.get(dev.site).and_then(|s| s.mgmt())
            {
                interfaces.push(InterfaceUse {
                    name: format!("Vlan{}", vlan.id),
                    description: "Management".to_string(),
                    mode: "svi".to_string(),
                    ip: format!("{}/{}", fmt_ip(mgmt), vlan.subnet.len),
                });
            }
            DevicePlan {
                name: dev.name.clone(),
                site,
                role: dev.role,
                model: dev.model_id.to_string(),
                mgmt_ip: dev.mgmt.map(fmt_ip).unwrap_or_default(),
                loopback: dev.loopback.map(fmt_ip).unwrap_or_default(),
                interfaces,
            }
        })
        .collect()
}

fn bom(model: &Model, t: &Topology) -> Vec<BomLine> {
    let count = |role: DeviceRole, model_id: &str| {
        t.devs
            .iter()
            .filter(|d| d.role == role && d.model_id == model_id)
            .count() as u32
    };
    let mut out = Vec::new();
    let mut push = |class: &str, model_id: &str, quantity: u32, ports: &str, notes: &str| {
        if quantity > 0 {
            out.push(BomLine {
                class: class.to_string(),
                model: model_id.to_string(),
                quantity,
                ports: ports.to_string(),
                notes: notes.to_string(),
            });
        }
    };
    push(
        "Layer-3 core switch",
        "switch-l3",
        count(DeviceRole::Core, "switch-l3"),
        "24 × 1G + 4 × 10G",
        "Holds the gateways, routes between the VLANs, runs DHCP",
    );
    push(
        "Layer-2 aggregation switch",
        "switch-l3",
        count(DeviceRole::Distribution, "switch-l3"),
        "24 × 1G + 4 × 10G",
        "Aggregates the access layer; no gateways of its own",
    );
    push(
        "Access switch",
        "switch",
        count(DeviceRole::Access, "switch"),
        "24 × 100M + 2 × 1G uplinks",
        "User, printer and access-point ports",
    );
    push(
        "Edge router",
        "router",
        count(DeviceRole::Edge, "router"),
        "4 × 1G",
        "Internet breakout, translation, site-to-site links",
    );
    push(
        "Edge router (high port count)",
        "router-8",
        count(DeviceRole::Edge, "router-8"),
        "8 × 1G",
        "Needed where several WAN links land",
    );
    push(
        "Wireless access point",
        "ap",
        count(DeviceRole::AccessPoint, "ap"),
        "1 × 1G uplink",
        "One per 40 planned wireless users",
    );
    let servers: u32 = model.sites.iter().map(|s| s.servers).sum();
    let printers: u32 = model.sites.iter().map(|s| s.printers).sum();
    let phones: u32 = model
        .sites
        .iter()
        .filter(|s| s.vlan(Purpose::Voice).is_some())
        .map(|s| s.users)
        .sum();
    push(
        "Server",
        "server",
        servers,
        "1 × 1G",
        "Asked for in the requirements; the lab places a sample of them",
    );
    push(
        "Printer",
        "printer",
        printers,
        "1 × 100M",
        "Asked for in the requirements; the lab places a sample of them",
    );
    push(
        "IP telephone",
        "phone",
        phones,
        "1 × 100M + pass-through",
        "One per user; they share the user's port, so they add no switch ports",
    );
    out
}
