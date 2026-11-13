//! Boxes, ports and cables.
//!
//! The topology is built before a single configuration line is written, so the
//! configurations and the cabling can never drift apart: every `description`,
//! every trunk's allowed list and every routed address comes from here.

use nocturne_netsim::HostConfig;

use crate::design::{DeviceRole, LinkKind};
use crate::model::{
    CLOUD_PORTS, CORE_PORTS, DIST_DOWNLINKS, INTERNET_HOST, Model, PUBLIC_BLOCK, SIM_ACCESS_PORTS,
};
use crate::requirements::{Purpose, Redundancy};
use crate::util::{Net, fmt_ip};

/// What a port is used for. Everything the configuration writer needs.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PortRole {
    /// An 802.1Q trunk carrying exactly `allowed`.
    Trunk {
        channel: Option<u32>,
        allowed: Vec<u16>,
    },
    /// An access port. `voice` adds the tagged voice VLAN a desk phone uses.
    Access {
        vlan: u16,
        voice: Option<u16>,
        /// A device is cabled to it in the lab.
        occupied: bool,
    },
    /// A routed port.
    Routed {
        net: Net,
        addr: u32,
        kind: RoutedKind,
    },
    /// An end system's one port.
    Host,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RoutedKind {
    Internet,
    Core,
    Wan,
}

#[derive(Clone, Debug)]
pub(crate) struct Port {
    pub(crate) iface: String,
    pub(crate) description: String,
    pub(crate) role: PortRole,
}

#[derive(Clone, Debug)]
pub(crate) struct Dev {
    pub(crate) name: String,
    /// Index into `Model::sites`; `usize::MAX` for the Internet clouds.
    pub(crate) site: usize,
    pub(crate) role: DeviceRole,
    pub(crate) model_id: &'static str,
    /// 1-based within its role at its site.
    pub(crate) idx: u32,
    pub(crate) loopback: Option<u32>,
    pub(crate) mgmt: Option<u32>,
    pub(crate) ports: Vec<Port>,
    /// Physical ports with nothing on them, to shut and park.
    pub(crate) spare: Vec<String>,
    pub(crate) host: Option<HostConfig>,
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) config: String,
    /// End hosts: the VLAN they sit in, for the plan and the checks.
    pub(crate) vlan: Option<u16>,
}

impl Dev {
    pub(crate) fn is_network_device(&self) -> bool {
        matches!(
            self.role,
            DeviceRole::Core | DeviceRole::Distribution | DeviceRole::Access | DeviceRole::Edge
        )
    }

    pub(crate) fn port(&self, iface: &str) -> Option<&Port> {
        self.ports.iter().find(|p| p.iface == iface)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Cable {
    pub(crate) a: usize,
    pub(crate) a_if: String,
    pub(crate) b: usize,
    pub(crate) b_if: String,
    pub(crate) kind: LinkKind,
    pub(crate) subnet: Option<Net>,
    pub(crate) carries: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Topology {
    pub(crate) devs: Vec<Dev>,
    pub(crate) cables: Vec<Cable>,
    /// Device indexes by site and role, in creation order.
    pub(crate) cores: Vec<Vec<usize>>,
    pub(crate) edges: Vec<Vec<usize>>,
    pub(crate) access: Vec<Vec<usize>>,
    pub(crate) distribution: Vec<Vec<usize>>,
    pub(crate) host_switch: Vec<Option<usize>>,
    pub(crate) nms: Option<usize>,
    /// Horizontal span `(x, width)` of each site's column, for the note boxes.
    pub(crate) site_spans: Vec<(f32, f32)>,
}

// --------------------------------------------------------- interface names

pub(crate) fn fa(n: u32) -> String {
    format!("FastEthernet0/{n}")
}
pub(crate) fn gi_access(n: u32) -> String {
    format!("GigabitEthernet0/{n}")
}
pub(crate) fn gi_l3(n: u32) -> String {
    format!("GigabitEthernet1/0/{n}")
}
pub(crate) fn te_l3(n: u32) -> String {
    format!("TenGigabitEthernet1/1/{n}")
}
pub(crate) fn gi_router(n: u32) -> String {
    format!("GigabitEthernet0/{n}")
}
pub(crate) fn eth(n: u32) -> String {
    format!("Ethernet{n}")
}

const ROW_CLOUD: f32 = 40.0;
const ROW_EDGE: f32 = 200.0;
const ROW_CORE: f32 = 340.0;
const ROW_DIST: f32 = 480.0;
const ROW_ACCESS: f32 = 620.0;
const ROW_HOST: f32 = 760.0;
const COLUMN: f32 = 170.0;
const SITE_GAP: f32 = 140.0;

/// One workstation / printer / access-point port the design has to provide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Demand {
    vlan: u16,
    class: PortClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortClass {
    Workstation,
    Printer,
    AccessPoint,
}

/// Build every device and every cable.
pub(crate) fn build(model: &mut Model) -> Result<Topology, String> {
    let mut t = Topology::default();
    let sites = model.sites.len();
    t.cores = vec![Vec::new(); sites];
    t.edges = vec![Vec::new(); sites];
    t.access = vec![Vec::new(); sites];
    t.distribution = vec![Vec::new(); sites];
    t.host_switch = vec![None; sites];

    // The Internet, one cloud per four edge connections.
    let internet_links: usize = (0..sites)
        .map(|i| {
            if model.sites[i].internet {
                model.sites[i].counts.edges as usize
            } else {
                0
            }
        })
        .sum();
    let public = Net::parse(PUBLIC_BLOCK).ok_or("the public block is not a block")?;
    let clouds = internet_links.div_ceil(CLOUD_PORTS);
    if internet_links * 4 > public.size() as usize {
        return Err(format!(
            "{internet_links} Internet connections need more than {PUBLIC_BLOCK} can hold"
        ));
    }
    let mut cloud_ids = Vec::new();
    for c in 0..clouds {
        let name = if c == 0 {
            "INTERNET".to_string()
        } else {
            format!("INTERNET{}", c + 1)
        };
        cloud_ids.push(t.devs.len());
        t.devs.push(Dev {
            name,
            site: usize::MAX,
            role: DeviceRole::Internet,
            model_id: "cloud",
            idx: c as u32 + 1,
            loopback: None,
            mgmt: None,
            ports: Vec::new(),
            spare: Vec::new(),
            host: Some(HostConfig {
                extra_ips: vec![format!("{INTERNET_HOST}/32")],
                services: vec!["dns".to_string(), "http".to_string()],
                ..HostConfig::default()
            }),
            x: 0.0,
            y: ROW_CLOUD,
            config: String::new(),
            vlan: None,
        });
    }
    let mut next_public = 0u32;
    let mut cloud_port = vec![0u32; clouds];

    // Devices, site by site, in the order the addresses were handed out.
    for s in 0..sites {
        build_site(model, &mut t, s)?;
    }

    // Cable the access layer, the cores and the edges.
    for s in 0..sites {
        cable_site(model, &mut t, s)?;
    }

    // Internet connections.
    for s in 0..sites {
        if !model.sites[s].internet {
            continue;
        }
        for e in 0..t.edges[s].len() {
            let dev = t.edges[s][e];
            let Some(iface) = t.devs[dev]
                .ports
                .iter()
                .find(|p| {
                    matches!(
                        p.role,
                        PortRole::Routed {
                            kind: RoutedKind::Internet,
                            ..
                        }
                    )
                })
                .map(|p| p.iface.clone())
            else {
                continue;
            };
            let link = Net::new(public.addr + next_public * 4, 30);
            next_public += 1;
            let cloud_index = (next_public as usize - 1) / CLOUD_PORTS;
            let cloud = cloud_ids[cloud_index];
            let port = cloud_port[cloud_index];
            cloud_port[cloud_index] += 1;
            let cloud_addr = link.first_host();
            let edge_addr = link.first_host() + 1;
            let cloud_name = t.devs[cloud].name.clone();
            let edge_name = t.devs[dev].name.clone();
            if let Some(p) = t.devs[dev].ports.iter_mut().find(|p| p.iface == iface) {
                p.role = PortRole::Routed {
                    net: link,
                    addr: edge_addr,
                    kind: RoutedKind::Internet,
                };
                p.description = format!("Internet uplink to {cloud_name}");
            }
            let cloud_if = eth(port);
            let host = t.devs[cloud].host.get_or_insert_with(HostConfig::default);
            if port == 0 {
                host.ip = Some(fmt_ip(cloud_addr));
                host.mask = Some(link.netmask());
            } else {
                host.extra_ips
                    .push(format!("{}/30@{cloud_if}", fmt_ip(cloud_addr)));
            }
            t.devs[cloud].ports.push(Port {
                iface: cloud_if.clone(),
                description: format!("To {edge_name}"),
                role: PortRole::Host,
            });
            t.cables.push(Cable {
                a: dev,
                a_if: iface,
                b: cloud,
                b_if: cloud_if,
                kind: LinkKind::Internet,
                subnet: Some(link),
                carries: String::new(),
            });
        }
    }

    // Site-to-site links, on the first edge router of each site.
    for i in 0..model.wan_links.len() {
        let link = model.wan_links[i];
        let (Some(&a), Some(&b)) = (t.edges[link.a].first(), t.edges[link.b].first()) else {
            return Err(format!(
                "a WAN link between {} and {} has no edge router to land on",
                model.sites[link.a].name, model.sites[link.b].name
            ));
        };
        let a_if = claim_wan_port(&mut t.devs[a], &model.sites[link.b].tag)?;
        let b_if = claim_wan_port(&mut t.devs[b], &model.sites[link.a].tag)?;
        set_routed(
            &mut t.devs[a],
            &a_if,
            link.subnet,
            link.subnet.first_host(),
            RoutedKind::Wan,
        );
        set_routed(
            &mut t.devs[b],
            &b_if,
            link.subnet,
            link.subnet.first_host() + 1,
            RoutedKind::Wan,
        );
        t.cables.push(Cable {
            a,
            a_if,
            b,
            b_if,
            kind: LinkKind::Wan,
            subnet: Some(link.subnet),
            carries: String::new(),
        });
    }

    for s in 0..sites {
        prune_trunks(model, &mut t, s);
    }
    lay_out(model, &mut t);
    Ok(t)
}

/// Trunks carry exactly the VLANs the far end needs, and the far end is only
/// fully known once the end systems have been placed — so the allowed lists
/// are worked out here, bottom up, rather than while cabling.
fn prune_trunks(model: &Model, t: &mut Topology, s: usize) {
    let site = &model.sites[s];
    let mgmt_id = site
        .mgmt()
        .map(|v| v.id)
        .unwrap_or(model.req.addressing.mgmt_vlan);

    let leaves: Vec<usize> = t.access[s]
        .iter()
        .copied()
        .chain(t.host_switch[s])
        .collect();
    for dev in leaves {
        let mut allowed = vec![mgmt_id];
        for port in &t.devs[dev].ports {
            if let PortRole::Access { vlan, voice, .. } = &port.role {
                allowed.push(*vlan);
                allowed.extend(voice.iter().copied());
            }
        }
        allowed.sort_unstable();
        allowed.dedup();
        set_trunks(t, dev, &allowed);
        propagate(t, dev, &allowed, LinkKind::AccessUplink);
    }
    for d in 0..t.distribution[s].len() {
        let dev = t.distribution[s][d];
        let mut allowed = vec![mgmt_id];
        for port in &t.devs[dev].ports {
            if let PortRole::Trunk { allowed: list, .. } = &port.role
                && port.description.starts_with("Downlink")
            {
                allowed.extend(list.iter().copied());
            }
        }
        allowed.sort_unstable();
        allowed.dedup();
        for port in &mut t.devs[dev].ports {
            if let PortRole::Trunk { allowed: list, .. } = &mut port.role
                && port.description.starts_with("Uplink")
            {
                *list = allowed.clone();
            }
        }
        propagate(t, dev, &allowed, LinkKind::DistributionUplink);
    }
    // The core peer link carries everything, transit VLAN included.
    let mut everything: Vec<u16> = site.vlans.iter().map(|v| v.id).collect();
    everything.push(model.transit_vlan);
    everything.sort_unstable();
    for &core in &t.cores[s] {
        for port in &mut t.devs[core].ports {
            if let PortRole::Trunk { allowed, .. } = &mut port.role
                && port.description.starts_with("Peer link")
            {
                *allowed = everything.clone();
            }
        }
    }
    for cable in &mut t.cables {
        if let Some(port) = t.devs[cable.a].port(&cable.a_if)
            && let PortRole::Trunk { allowed, .. } = &port.role
        {
            cable.carries = vlan_list(allowed);
        }
    }
}

/// Every trunk on a leaf switch carries the same list: its own VLANs.
fn set_trunks(t: &mut Topology, dev: usize, allowed: &[u16]) {
    for port in &mut t.devs[dev].ports {
        if let PortRole::Trunk { allowed: list, .. } = &mut port.role {
            *list = allowed.to_vec();
        }
    }
}

/// Give the parent's downlink the same list as the child's uplink.
fn propagate(t: &mut Topology, child: usize, allowed: &[u16], kind: LinkKind) {
    let ends: Vec<(usize, String)> = t
        .cables
        .iter()
        .filter(|c| c.a == child && c.kind == kind)
        .map(|c| (c.b, c.b_if.clone()))
        .collect();
    for (parent, iface) in ends {
        if let Some(port) = t.devs[parent].ports.iter_mut().find(|p| p.iface == iface)
            && let PortRole::Trunk { allowed: list, .. } = &mut port.role
        {
            *list = allowed.to_vec();
        }
    }
}

fn claim_wan_port(dev: &mut Dev, peer_tag: &str) -> Result<String, String> {
    let port = dev
        .ports
        .iter_mut()
        .find(|p| {
            matches!(
                p.role,
                PortRole::Routed {
                    kind: RoutedKind::Wan,
                    ..
                }
            ) && p.description.is_empty()
        })
        .ok_or_else(|| format!("{} has no free WAN port", dev.name))?;
    port.description = format!("WAN link to {peer_tag}");
    Ok(port.iface.clone())
}

fn set_routed(dev: &mut Dev, iface: &str, net: Net, addr: u32, kind: RoutedKind) {
    if let Some(port) = dev.ports.iter_mut().find(|p| p.iface == iface) {
        port.role = PortRole::Routed { net, addr, kind };
    }
}

// ------------------------------------------------------------------ a site

fn build_site(model: &mut Model, t: &mut Topology, s: usize) -> Result<(), String> {
    let counts = model.sites[s].counts;
    let tag = model.sites[s].tag.clone();
    let mgmt_net = model.sites[s].mgmt().map(|v| v.subnet);
    // Device management addresses start above the static reserve, so the
    // gateway, the cores and the test hosts keep the bottom of the subnet.
    let mut next_mgmt = crate::model::STATIC_RESERVE;
    let mut mgmt_addr = |what: &str| -> Result<Option<u32>, String> {
        let Some(net) = mgmt_net else { return Ok(None) };
        next_mgmt += 1;
        match net.nth(next_mgmt) {
            Some(addr) if addr <= net.last_host() => Ok(Some(addr)),
            _ => Err(format!(
                "the management subnet {net} has run out of addresses at {what}; \
                 it needs to be bigger"
            )),
        }
    };

    // Cores, then edges: the order the loopbacks were allocated in.
    for c in 0..counts.cores {
        let loopback = model.sites[s].take_loopback();
        // With two cores the gateway address is virtual and each core keeps its
        // own, .2 and .3; a lone core owns the gateway address itself.
        let mgmt = if counts.cores == 2 {
            mgmt_net.and_then(|n| n.nth(2 + c))
        } else {
            mgmt_net.map(|n| n.first_host())
        };
        t.cores[s].push(t.devs.len());
        t.devs.push(Dev {
            name: format!("{tag}-CORE{}", c + 1),
            site: s,
            role: DeviceRole::Core,
            model_id: "switch-l3",
            idx: c + 1,
            loopback,
            mgmt,
            ports: Vec::new(),
            spare: Vec::new(),
            host: None,
            x: 0.0,
            y: ROW_CORE,
            config: String::new(),
            vlan: None,
        });
    }
    for e in 0..counts.edges {
        let loopback = model.sites[s].take_loopback();
        t.edges[s].push(t.devs.len());
        t.devs.push(Dev {
            name: format!("{tag}-EDGE{}", e + 1),
            site: s,
            role: DeviceRole::Edge,
            model_id: "router",
            idx: e + 1,
            loopback,
            mgmt: loopback,
            ports: Vec::new(),
            spare: Vec::new(),
            host: None,
            x: 0.0,
            y: ROW_EDGE,
            config: String::new(),
            vlan: None,
        });
    }
    for d in 0..counts.distribution {
        let mgmt = mgmt_addr("a distribution switch")?;
        t.distribution[s].push(t.devs.len());
        t.devs.push(Dev {
            name: format!("{tag}-DIST{}", d + 1),
            site: s,
            role: DeviceRole::Distribution,
            model_id: "switch-l3",
            idx: d + 1,
            loopback: None,
            mgmt,
            ports: Vec::new(),
            spare: Vec::new(),
            host: None,
            x: 0.0,
            y: ROW_DIST,
            config: String::new(),
            vlan: None,
        });
    }
    for a in 0..counts.access {
        let mgmt = mgmt_addr("an access switch")?;
        t.access[s].push(t.devs.len());
        t.devs.push(Dev {
            name: format!("{tag}-ACC{}", a + 1),
            site: s,
            role: DeviceRole::Access,
            model_id: "switch",
            idx: a + 1,
            loopback: None,
            mgmt,
            ports: Vec::new(),
            spare: Vec::new(),
            host: None,
            x: 0.0,
            y: ROW_ACCESS,
            config: String::new(),
            vlan: None,
        });
    }
    if counts.host_switch {
        let mgmt = mgmt_addr("the server switch")?;
        t.host_switch[s] = Some(t.devs.len());
        t.devs.push(Dev {
            name: format!("{tag}-SRVSW"),
            site: s,
            role: DeviceRole::Access,
            model_id: "switch",
            idx: counts.access + 1,
            loopback: None,
            mgmt,
            ports: Vec::new(),
            spare: Vec::new(),
            host: None,
            x: 0.0,
            y: ROW_ACCESS,
            config: String::new(),
            vlan: None,
        });
    }
    Ok(())
}

/// Hand the host ports out and cable everything inside one site.
fn cable_site(model: &mut Model, t: &mut Topology, s: usize) -> Result<(), String> {
    let redundancy = model.req.redundancy;
    let site = model.sites[s].clone();
    let counts = site.counts;
    let mgmt_id = site
        .mgmt()
        .map(|v| v.id)
        .unwrap_or(model.req.addressing.mgmt_vlan);
    let voice_id = site.vlan(Purpose::Voice).map(|v| v.id);
    let all_vlans: Vec<u16> = site.vlans.iter().map(|v| v.id).collect();

    let mut po_next: Vec<u32> = vec![1; t.devs.len()];
    if counts.cores == 2 {
        let net = model.sites[s]
            .take_p2p()
            .ok_or_else(|| format!("no point-to-point block left for {}", site.name))?;
        model.sites[s].transit = Some(net);
    }

    // ---- the core peer link and the transit VLAN -------------------------
    if counts.cores == 2 {
        let (c0, c1) = (t.cores[s][0], t.cores[s][1]);
        let (n0, n1) = (t.devs[c0].name.clone(), t.devs[c1].name.clone());
        let (g0, g1) = (take_group(&mut po_next, c0), take_group(&mut po_next, c1));
        for p in 1..=2u32 {
            let iface = te_l3(p);
            push_port(
                &mut t.devs[c0],
                &iface,
                &format!("Peer link to {n1}"),
                PortRole::Trunk {
                    channel: Some(g0),
                    allowed: all_vlans.clone(),
                },
            );
            push_port(
                &mut t.devs[c1],
                &iface,
                &format!("Peer link to {n0}"),
                PortRole::Trunk {
                    channel: Some(g1),
                    allowed: all_vlans.clone(),
                },
            );
            t.cables.push(Cable {
                a: c0,
                a_if: iface.clone(),
                b: c1,
                b_if: iface,
                kind: LinkKind::CoreInterlink,
                subnet: None,
                carries: vlan_list(&all_vlans),
            });
        }
    }

    // ---- core to edge ----------------------------------------------------
    for (e, &edge) in t.edges[s].iter().enumerate() {
        let core = t.cores[s][e.min(t.cores[s].len() - 1)];
        let net = model.sites[s]
            .take_p2p()
            .ok_or_else(|| format!("no point-to-point block left for {}", site.name))?;
        let core_name = t.devs[core].name.clone();
        let edge_name = t.devs[edge].name.clone();
        // The core's first gigabit port always faces the edge.
        let core_if = gi_l3(1);
        push_port(
            &mut t.devs[core],
            &core_if,
            &format!("Routed link to {edge_name}"),
            PortRole::Routed {
                net,
                addr: net.first_host(),
                kind: RoutedKind::Core,
            },
        );
        // The edge's ports: Internet first, then the core, then the WAN.
        let mut edge_ports: Vec<(String, String, PortRole)> = Vec::new();
        let mut next = 0u32;
        if site.internet {
            edge_ports.push((
                gi_router(next),
                String::new(),
                PortRole::Routed {
                    net: Net::new(0, 30),
                    addr: 0,
                    kind: RoutedKind::Internet,
                },
            ));
            next += 1;
        }
        let edge_if = gi_router(next);
        next += 1;
        edge_ports.push((
            edge_if.clone(),
            format!("Routed link to {core_name}"),
            PortRole::Routed {
                net,
                addr: net.first_host() + 1,
                kind: RoutedKind::Core,
            },
        ));
        if e == 0 {
            let wan_here = model
                .wan_links
                .iter()
                .filter(|l| l.a == s || l.b == s)
                .count() as u32;
            for _ in 0..wan_here {
                edge_ports.push((
                    gi_router(next),
                    String::new(),
                    PortRole::Routed {
                        net: Net::new(0, 30),
                        addr: 0,
                        kind: RoutedKind::Wan,
                    },
                ));
                next += 1;
            }
        }
        if next > 8 {
            return Err(format!(
                "{edge_name} needs {next} routed ports and the largest router model has 8; \
                 reduce the number of WAN links at {}",
                site.name
            ));
        }
        if next > 4 {
            t.devs[edge].model_id = "router-8";
        }
        let total = if t.devs[edge].model_id == "router-8" {
            8
        } else {
            4
        };
        for (iface, description, role) in edge_ports {
            push_port(&mut t.devs[edge], &iface, &description, role);
        }
        for p in next..total {
            t.devs[edge].spare.push(gi_router(p));
        }
        t.cables.push(Cable {
            a: core,
            a_if: core_if,
            b: edge,
            b_if: edge_if,
            kind: LinkKind::EdgeUplink,
            subnet: Some(net),
            carries: String::new(),
        });
    }
    // ---- the host ports --------------------------------------------------
    let demand = host_port_demand(&site);
    let mut cursor = 0usize;
    for (k, &share) in site.ports_per_access.iter().enumerate() {
        let dev = t.access[s][k];
        for n in 1..=SIM_ACCESS_PORTS.min(share) {
            let Some(item) = demand.get(cursor).copied() else {
                break;
            };
            cursor += 1;
            let (description, voice) = match item.class {
                PortClass::Workstation => ("User workstation".to_string(), voice_id),
                PortClass::Printer => ("Printer".to_string(), None),
                PortClass::AccessPoint => ("Wireless access point".to_string(), None),
            };
            push_port(
                &mut t.devs[dev],
                &fa(n),
                &description,
                PortRole::Access {
                    vlan: item.vlan,
                    voice,
                    occupied: false,
                },
            );
        }
    }

    // ---- uplinks ---------------------------------------------------------
    let parents: Vec<usize> = if counts.distribution > 0 {
        t.distribution[s].clone()
    } else {
        t.cores[s].clone()
    };
    let group_size = if counts.distribution > 0 {
        let per = if redundancy == Redundancy::Uplinks {
            DIST_DOWNLINKS / 2
        } else {
            DIST_DOWNLINKS
        };
        per.max(1) as usize
    } else {
        usize::MAX
    };
    let mut parent_next: Vec<u32> = vec![1; t.devs.len()];
    let mut children: Vec<usize> = t.access[s].clone();
    if let Some(hs) = t.host_switch[s] {
        children.push(hs);
    }
    for (k, &child) in children.iter().enumerate() {
        let group = if group_size == usize::MAX {
            0
        } else {
            k / group_size
        };
        let targets: Vec<usize> = match redundancy {
            Redundancy::Full if counts.distribution > 0 => {
                let a = parents.get(group * 2).copied();
                let b = parents.get(group * 2 + 1).copied();
                a.into_iter().chain(b).collect()
            }
            Redundancy::Full => parents.clone(),
            _ => parents.get(group).copied().into_iter().collect(),
        };
        if targets.is_empty() {
            return Err(format!(
                "{} has no switch to home {} to",
                site.name, t.devs[child].name
            ));
        }
        let carried = carried_vlans(&t.devs[child], mgmt_id);
        let bundled = redundancy == Redundancy::Uplinks;
        let child_group = bundled.then(|| take_group(&mut po_next, child));
        let parent_group = bundled.then(|| take_group(&mut po_next, targets[0]));
        let links: Vec<(u32, usize)> = match redundancy {
            Redundancy::None => vec![(1, targets[0])],
            Redundancy::Uplinks => vec![(1, targets[0]), (2, targets[0])],
            Redundancy::Full => targets
                .iter()
                .enumerate()
                .map(|(i, &p)| (i as u32 + 1, p))
                .collect(),
        };
        for (port, parent) in links {
            let parent_name = t.devs[parent].name.clone();
            let child_name = t.devs[child].name.clone();
            let child_if = gi_access(port);
            let parent_if = if t.devs[parent].role == DeviceRole::Core {
                let n = parent_next[parent].max(2);
                parent_next[parent] = n + 1;
                if n > CORE_PORTS {
                    return Err(format!("{parent_name} has run out of downlink ports"));
                }
                gi_l3(n)
            } else {
                let n = parent_next[parent];
                parent_next[parent] = n + 1;
                if n > DIST_DOWNLINKS {
                    return Err(format!("{parent_name} has run out of downlink ports"));
                }
                gi_l3(n)
            };
            push_port(
                &mut t.devs[child],
                &child_if,
                &format!("Uplink to {parent_name}"),
                PortRole::Trunk {
                    channel: child_group,
                    allowed: carried.clone(),
                },
            );
            push_port(
                &mut t.devs[parent],
                &parent_if,
                &format!("Downlink to {child_name}"),
                PortRole::Trunk {
                    channel: parent_group,
                    allowed: carried.clone(),
                },
            );
            t.cables.push(Cable {
                a: child,
                a_if: child_if,
                b: parent,
                b_if: parent_if,
                kind: LinkKind::AccessUplink,
                subnet: None,
                carries: vlan_list(&carried),
            });
        }
    }

    // Distribution switches hang off the cores the same way.
    if counts.distribution > 0 {
        let carried = all_vlans.clone();
        for (d, &dist) in t.distribution[s].clone().iter().enumerate() {
            let links: Vec<(u32, usize)> = match redundancy {
                Redundancy::None => vec![(23, t.cores[s][0])],
                Redundancy::Uplinks => vec![(23, t.cores[s][0]), (24, t.cores[s][0])],
                Redundancy::Full => vec![(23, t.cores[s][d % 2]), (24, t.cores[s][(d + 1) % 2])],
            };
            let bundled = redundancy == Redundancy::Uplinks;
            let dist_group = bundled.then(|| take_group(&mut po_next, dist));
            let core_group = bundled.then(|| take_group(&mut po_next, t.cores[s][0]));
            for (port, core) in links {
                let core_name = t.devs[core].name.clone();
                let dist_name = t.devs[dist].name.clone();
                let dist_if = gi_l3(port);
                let n = parent_next[core].max(2);
                parent_next[core] = n + 1;
                if n > CORE_PORTS {
                    return Err(format!("{core_name} has run out of downlink ports"));
                }
                let core_if = gi_l3(n);
                push_port(
                    &mut t.devs[dist],
                    &dist_if,
                    &format!("Uplink to {core_name}"),
                    PortRole::Trunk {
                        channel: dist_group,
                        allowed: carried.clone(),
                    },
                );
                push_port(
                    &mut t.devs[core],
                    &core_if,
                    &format!("Downlink to {dist_name}"),
                    PortRole::Trunk {
                        channel: core_group,
                        allowed: carried.clone(),
                    },
                );
                t.cables.push(Cable {
                    a: dist,
                    a_if: dist_if,
                    b: core,
                    b_if: core_if,
                    kind: LinkKind::DistributionUplink,
                    subnet: None,
                    carries: vlan_list(&carried),
                });
            }
        }
    }

    // ---- end systems -----------------------------------------------------
    place_hosts(model, t, s, &mut parent_next)?;

    // Whatever is left on a core or a distribution switch is parked.
    for &dev in t.cores[s].iter().chain(t.distribution[s].iter()) {
        let used: Vec<String> = t.devs[dev].ports.iter().map(|p| p.iface.clone()).collect();
        let mut spare = Vec::new();
        for n in 1..=CORE_PORTS {
            if !used.contains(&gi_l3(n)) {
                spare.push(gi_l3(n));
            }
        }
        for n in 1..=4 {
            if !used.contains(&te_l3(n)) {
                spare.push(te_l3(n));
            }
        }
        t.devs[dev].spare = spare;
    }
    for &dev in t.access[s].iter().chain(t.host_switch[s].iter()) {
        let used: Vec<String> = t.devs[dev].ports.iter().map(|p| p.iface.clone()).collect();
        let mut spare = Vec::new();
        for n in 1..=SIM_ACCESS_PORTS {
            if !used.contains(&fa(n)) {
                spare.push(fa(n));
            }
        }
        for n in 1..=2 {
            if !used.contains(&gi_access(n)) {
                spare.push(gi_access(n));
            }
        }
        t.devs[dev].spare = spare;
    }
    Ok(())
}

/// The workstation, printer and access-point ports the site needs, in order.
fn host_port_demand(site: &crate::model::SiteModel) -> Vec<Demand> {
    let mut out = Vec::new();
    let mut push = |vlan: u16, class: PortClass, n: u32| {
        for _ in 0..n.min(100_000) {
            out.push(Demand { vlan, class });
        }
    };
    for vlan in &site.vlans {
        match vlan.purpose {
            Purpose::Users | Purpose::Other => {
                push(vlan.id, PortClass::Workstation, vlan.hosts_raw)
            }
            Purpose::Printers => push(vlan.id, PortClass::Printer, vlan.hosts_raw),
            _ => {}
        }
    }
    for vlan in &site.vlans {
        if vlan.purpose == Purpose::Guest {
            push(
                vlan.id,
                PortClass::AccessPoint,
                vlan.hosts_raw.div_ceil(crate::model::USERS_PER_AP).max(1),
            );
        }
    }
    out
}

/// The VLANs a switch's uplink has to carry: the ones on its own ports, and
/// management. Nothing else.
fn carried_vlans(dev: &Dev, mgmt: u16) -> Vec<u16> {
    let mut out = vec![mgmt];
    for port in &dev.ports {
        match &port.role {
            PortRole::Access { vlan, voice, .. } => {
                out.push(*vlan);
                out.extend(voice.iter().copied());
            }
            PortRole::Trunk { allowed, .. } => out.extend(allowed.iter().copied()),
            _ => {}
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

pub(crate) fn vlan_list(vlans: &[u16]) -> String {
    vlans
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// The next free EtherChannel group number on a device.
fn take_group(po_next: &mut [u32], dev: usize) -> u32 {
    let slot = po_next.get_mut(dev);
    match slot {
        Some(next) => {
            let group = (*next).clamp(1, 48);
            *next = group + 1;
            group
        }
        None => 1,
    }
}

/// Every EtherChannel on a device: group number, what it faces, and what it
/// carries. The members of a group always share their switchport settings.
pub(crate) fn channels(dev: &Dev) -> Vec<(u32, String, Vec<u16>)> {
    let mut out: Vec<(u32, String, Vec<u16>)> = Vec::new();
    for port in &dev.ports {
        if let PortRole::Trunk {
            channel: Some(group),
            allowed,
        } = &port.role
            && !out.iter().any(|(g, ..)| g == group)
        {
            out.push((*group, port.description.clone(), allowed.clone()));
        }
    }
    out.sort_by_key(|(g, ..)| *g);
    out
}

fn push_port(dev: &mut Dev, iface: &str, description: &str, role: PortRole) {
    if let Some(existing) = dev.ports.iter_mut().find(|p| p.iface == iface) {
        existing.description = description.to_string();
        existing.role = role;
        return;
    }
    dev.ports.push(Port {
        iface: iface.to_string(),
        description: description.to_string(),
        role,
    });
}

// ------------------------------------------------------------- end systems

/// Put the sample hosts the lab proves the design with on real ports.
fn place_hosts(
    model: &mut Model,
    t: &mut Topology,
    s: usize,
    parent_next: &mut [u32],
) -> Result<(), String> {
    let site = model.sites[s].clone();
    let per_vlan = model.req.hosts_per_vlan;
    let tag = site.tag.clone();
    let mut warnings: Vec<String> = Vec::new();
    // Names are numbered per kind across the whole site: two VLANs of the
    // same kind must not both produce a `PC1`.
    let mut counters: Vec<(&'static str, u32)> = Vec::new();
    let mut number = |prefix: &'static str| -> u32 {
        match counters.iter_mut().find(|(p, _)| *p == prefix) {
            Some((_, n)) => {
                *n += 1;
                *n
            }
            None => {
                counters.push((prefix, 1));
                1
            }
        }
    };

    // Workstations and printers go on ports the design already provides.
    for vlan in &site.vlans {
        let (model_id, prefix) = match vlan.purpose {
            Purpose::Users | Purpose::Other => ("pc", "PC"),
            Purpose::Printers => ("printer", "PRT"),
            _ => continue,
        };
        let wanted = per_vlan.min(vlan.hosts_raw);
        let mut placed = 0;
        let switches = t.access[s].clone();
        'switches: for dev in switches {
            let ifaces: Vec<String> = t.devs[dev]
                .ports
                .iter()
                .filter(|p| {
                    matches!(&p.role, PortRole::Access { vlan: v, occupied: false, .. } if *v == vlan.id)
                })
                .map(|p| p.iface.clone())
                .collect();
            for iface in ifaces {
                if placed >= wanted {
                    break 'switches;
                }
                let addr = vlan.test_host(placed).ok_or_else(|| {
                    format!(
                        "VLAN {} of {} is too small for a test host",
                        vlan.id, site.name
                    )
                })?;
                placed += 1;
                let name = format!("{tag}-{prefix}{}", number(prefix));
                occupy(&mut t.devs[dev], &iface, &name);
                add_host(t, s, &name, model_id, addr, vlan, dev, iface);
            }
        }
    }

    // Telephones, guests and a DHCP client need a port of their own in the lab.
    let mut extras: Vec<(&'static str, &'static str, u16, u32, bool)> = Vec::new();
    for vlan in &site.vlans {
        let wanted = per_vlan.min(vlan.hosts_raw);
        match vlan.purpose {
            Purpose::Voice => {
                for n in 0..wanted {
                    extras.push(("phone", "PH", vlan.id, n, false));
                }
            }
            Purpose::Guest => {
                for n in 0..wanted {
                    extras.push(("laptop", "GUEST", vlan.id, n, false));
                }
            }
            _ => {}
        }
    }
    if model.req.services.dhcp
        && let Some(users) = site.vlan(Purpose::Users)
        && users.pool.is_some()
    {
        extras.push(("laptop", "DHCP", users.id, 0, true));
    }
    for (model_id, prefix, vlan_id, n, dhcp) in extras {
        let Some(vlan) = site.vlans.iter().find(|v| v.id == vlan_id) else {
            continue;
        };
        let Some((dev, iface)) = free_access_port(&*t, s) else {
            warnings.push(format!(
                "{} has no spare access port for a test {} in VLAN {vlan_id}; that VLAN is not \
                 probed in the lab",
                site.name,
                prefix.to_ascii_lowercase()
            ));
            continue;
        };
        let name = format!("{tag}-{prefix}{}", number(prefix));
        let description = match prefix {
            "PH" => "IP telephone",
            "GUEST" => "Guest wireless client",
            _ => "Workstation (DHCP)",
        };
        push_port(
            &mut t.devs[dev],
            &iface,
            description,
            PortRole::Access {
                vlan: vlan_id,
                voice: None,
                occupied: true,
            },
        );
        if dhcp {
            let host = HostConfig {
                dhcp: true,
                ..HostConfig::default()
            };
            add_host_cfg(t, s, &name, model_id, host, Some(vlan_id), dev, iface, 0);
        } else {
            let addr = vlan.test_host(n).ok_or_else(|| {
                format!(
                    "VLAN {vlan_id} of {} is too small for a test host",
                    site.name
                )
            })?;
            add_host(t, s, &name, model_id, addr, vlan, dev, iface);
        }
    }

    // Servers and the management station: on their own switch, or on the core.
    let mut server_hosts: Vec<(String, &'static str, u32, u16)> = Vec::new();
    if let Some(vlan) = site.vlan(Purpose::Servers) {
        for n in 0..per_vlan.min(site.servers) {
            let addr = vlan.test_host(n).ok_or_else(|| {
                format!(
                    "VLAN {} of {} is too small for a server",
                    vlan.id, site.name
                )
            })?;
            server_hosts.push((
                format!("{tag}-SRV{}", number("SRV")),
                "server",
                addr,
                vlan.id,
            ));
        }
    }
    let nms_host = if site.is_hub {
        site.mgmt().and_then(|v| v.test_host(0)).map(|addr| {
            (
                "NMS".to_string(),
                "nms",
                addr,
                site.mgmt().map(|v| v.id).unwrap_or(0),
            )
        })
    } else {
        None
    };
    for (name, model_id, addr, vlan_id) in server_hosts.into_iter().chain(nms_host) {
        let Some(vlan) = site.vlans.iter().find(|v| v.id == vlan_id) else {
            continue;
        };
        let (dev, iface) = match t.host_switch[s] {
            Some(hs) => match first_free_host_port(&t.devs[hs]) {
                Some(iface) => (hs, iface),
                None => continue,
            },
            None => {
                let core = t.cores[s][0];
                // Count down from the top so the downlinks keep the low ports.
                let found = (2..=CORE_PORTS).rev().map(gi_l3).find(|candidate| {
                    t.devs[core].port(candidate).is_none()
                        && candidate != &gi_l3(parent_next[core].saturating_sub(1))
                });
                let Some(iface) = found else { continue };
                (core, iface)
            }
        };
        let description = if model_id == "nms" {
            "Management station"
        } else {
            "Server"
        };
        push_port(
            &mut t.devs[dev],
            &iface,
            description,
            PortRole::Access {
                vlan: vlan_id,
                voice: None,
                occupied: true,
            },
        );
        add_host(t, s, &name, model_id, addr, vlan, dev, iface);
        if model_id == "nms" {
            t.nms = Some(t.devs.len() - 1);
        }
    }

    // Access points are devices too: they take the ports planned for them.
    for k in 0..t.access[s].len() {
        let dev = t.access[s][k];
        let ifaces: Vec<String> = t.devs[dev]
            .ports
            .iter()
            .filter(|p| p.description == "Wireless access point")
            .map(|p| p.iface.clone())
            .collect();
        for iface in ifaces {
            let ap = number("AP");
            let name = format!("{tag}-AP{ap}");
            let vlan_id = match &t.devs[dev].port(&iface).map(|p| p.role.clone()) {
                Some(PortRole::Access { vlan, .. }) => *vlan,
                _ => continue,
            };
            occupy(&mut t.devs[dev], &iface, &name);
            let node = t.devs.len();
            t.devs.push(Dev {
                name: name.clone(),
                site: s,
                role: DeviceRole::AccessPoint,
                model_id: "ap",
                idx: ap,
                loopback: None,
                mgmt: None,
                ports: vec![Port {
                    iface: "GigabitEthernet0".to_string(),
                    description: format!("To {}", t.devs[dev].name),
                    role: PortRole::Host,
                }],
                spare: Vec::new(),
                host: None,
                x: 0.0,
                y: ROW_HOST,
                config: String::new(),
                vlan: Some(vlan_id),
            });
            t.cables.push(Cable {
                a: node,
                a_if: "GigabitEthernet0".to_string(),
                b: dev,
                b_if: iface,
                kind: LinkKind::Host,
                subnet: None,
                carries: format!("access {vlan_id}"),
            });
        }
    }

    model.warnings.extend(warnings);
    Ok(())
}

fn occupy(dev: &mut Dev, iface: &str, who: &str) {
    if let Some(port) = dev.ports.iter_mut().find(|p| p.iface == iface) {
        port.description = format!("{} ({who})", port.description);
        if let PortRole::Access {
            occupied, voice, ..
        } = &mut port.role
        {
            *occupied = true;
            // The simulated workstation cannot take the tagged voice VLAN a
            // real desk phone passes through, so this one port is data only.
            *voice = None;
        }
    }
}

/// The lowest host port on the site's access switches that nothing uses yet.
fn free_access_port(t: &Topology, s: usize) -> Option<(usize, String)> {
    for &dev in &t.access[s] {
        if let Some(iface) = first_free_host_port(&t.devs[dev]) {
            return Some((dev, iface));
        }
    }
    None
}

fn first_free_host_port(dev: &Dev) -> Option<String> {
    (1..=SIM_ACCESS_PORTS)
        .map(fa)
        .find(|iface| dev.port(iface).is_none())
}

#[allow(clippy::too_many_arguments)]
fn add_host(
    t: &mut Topology,
    s: usize,
    name: &str,
    model_id: &'static str,
    addr: u32,
    vlan: &crate::model::VlanModel,
    switch: usize,
    iface: String,
) {
    let host = HostConfig {
        ip: Some(fmt_ip(addr)),
        mask: Some(vlan.subnet.netmask()),
        gateway: Some(fmt_ip(vlan.gateway)),
        dns: Some(INTERNET_HOST.to_string()),
        services: if model_id == "server" {
            ["http", "ssh"].map(String::from).to_vec()
        } else {
            Vec::new()
        },
        ..HostConfig::default()
    };
    add_host_cfg(
        t,
        s,
        name,
        model_id,
        host,
        Some(vlan.id),
        switch,
        iface,
        addr,
    );
}

#[allow(clippy::too_many_arguments)]
fn add_host_cfg(
    t: &mut Topology,
    s: usize,
    name: &str,
    model_id: &'static str,
    host: HostConfig,
    vlan: Option<u16>,
    switch: usize,
    iface: String,
    addr: u32,
) {
    let node = t.devs.len();
    t.devs.push(Dev {
        name: name.to_string(),
        site: s,
        role: DeviceRole::Host,
        model_id,
        idx: 0,
        loopback: None,
        mgmt: (addr != 0).then_some(addr),
        ports: vec![Port {
            iface: "Ethernet0".to_string(),
            description: format!("To {}", t.devs[switch].name),
            role: PortRole::Host,
        }],
        spare: Vec::new(),
        host: Some(host),
        x: 0.0,
        y: ROW_HOST,
        config: String::new(),
        vlan,
    });
    t.cables.push(Cable {
        a: node,
        a_if: "Ethernet0".to_string(),
        b: switch,
        b_if: iface,
        kind: LinkKind::Host,
        subnet: None,
        carries: vlan.map(|v| format!("access {v}")).unwrap_or_default(),
    });
}

// ----------------------------------------------------------------- picture

/// Sites side by side, each reading top-down: Internet, edge, core,
/// distribution, access, hosts.
fn lay_out(model: &Model, t: &mut Topology) {
    let mut x = 0.0f32;
    let mut site_spans: Vec<(f32, f32)> = Vec::new();
    for s in 0..model.sites.len() {
        let rows = [ROW_EDGE, ROW_CORE, ROW_DIST, ROW_ACCESS, ROW_HOST];
        let widest = rows
            .iter()
            .map(|row| t.devs.iter().filter(|d| d.site == s && d.y == *row).count())
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let width = widest * COLUMN;
        for row in rows {
            let items: Vec<usize> = (0..t.devs.len())
                .filter(|&i| t.devs[i].site == s && t.devs[i].y == row)
                .collect();
            let span = items.len() as f32 * COLUMN;
            let start = x + (width - span) / 2.0 + COLUMN / 2.0;
            for (i, &dev) in items.iter().enumerate() {
                t.devs[dev].x = start + i as f32 * COLUMN;
            }
        }
        site_spans.push((x, width));
        x += width + SITE_GAP;
    }
    let total = (x - SITE_GAP).max(COLUMN);
    let clouds: Vec<usize> = (0..t.devs.len())
        .filter(|&i| t.devs[i].role == DeviceRole::Internet)
        .collect();
    for (i, &dev) in clouds.iter().enumerate() {
        t.devs[dev].x = total * (i as f32 + 0.5) / clouds.len().max(1) as f32;
    }
    t.site_spans = site_spans;
}
