//! Read-only views of the lab for the host application: the canvas snapshot,
//! per-node tables and health metrics.

use crate::api::{
    ArpRow, CableKind, IfCounters, IfView, LabSnapshot, Light, LinkView, MacRow, NodeMetrics, NodeTables, NodeView,
    NoteSpec, RouteRow, StpRow, VlanRow,
};
use crate::catalog::Role;
use crate::device::ios::show::switching::vlan_ports;
use crate::ifname::IfId;
use crate::sim::World;
use crate::sim::node::Node;
use crate::sim::rib::Source;
use crate::sim::stp::PortRole;

fn light(node: &Node, id: IfId) -> Light {
    let Some(state) = node.ifs.get(&id) else { return Light::Off };
    if !node.powered || state.link.is_none() {
        return Light::Off;
    }
    if !state.up {
        return Light::Red;
    }
    // A bridge port is amber until spanning tree lets it forward everywhere,
    // and while it sits unbundled in a channel group.
    let cfg = node.cfg.iface(id);
    let suspended = cfg.is_some_and(|c| c.channel_group.is_some()) && !state.bundled;
    let port = node.l2_port(id).unwrap_or(id);
    let blocked = node.model.is_switch() && cfg.is_some_and(|c| c.l2.is_some() || c.channel_group.is_some()) && !node.stp.all_forwarding(port);
    if suspended || blocked { Light::Amber } else { Light::Green }
}

fn mode(node: &Node, id: IfId) -> String {
    let Some(cfg) = node.cfg.iface(id) else { return String::new() };
    match (&cfg.l2, node.role()) {
        (Some(_), _) if node.is_trunk(id) => "trunk".to_string(),
        (Some(sw), _) => format!("access {}", sw.access_vlan),
        (None, _) if cfg.channel_group.is_some() => match cfg.channel_group {
            Some((group, _)) => format!("channel {group}"),
            None => String::new(),
        },
        (None, Role::Host) => "host".to_string(),
        (None, Role::Bridge) => String::new(),
        (None, _) => "routed".to_string(),
    }
}

fn node_view(node: &Node) -> NodeView {
    NodeView {
        id: node.id,
        name: node.name.clone(),
        hostname: node.cfg.hostname.clone(),
        model: node.model.id.to_string(),
        kind: node.model.kind,
        x: node.x,
        y: node.y,
        powered: node.powered,
        cli: node.model.cli,
        interfaces: node
            .cfg
            .interfaces
            .iter()
            .map(|(id, cfg)| IfView {
                name: id.name(),
                short: id.short(),
                link: node.ifs.get(id).and_then(|s| s.link),
                admin_up: !cfg.shutdown,
                oper_up: node.is_up(*id),
                light: light(node, *id),
                ip: node.addr(*id).filter(|_| node.is_l3(*id)).map(|a| a.to_string()),
                mode: mode(node, *id),
                description: cfg.description.clone(),
            })
            .collect(),
        mgmt_ip: node.mgmt_ip().map(|ip| ip.to_string()),
    }
}

pub(crate) fn snapshot(world: &World, notes: &[NoteSpec]) -> LabSnapshot {
    let mut links: Vec<LinkView> = world
        .links
        .iter()
        .map(|(id, link)| {
            let end_light = |(node, port): (u32, IfId)| world.nodes.get(&node).map_or(Light::Off, |n| light(n, port));
            LinkView {
                id: *id,
                a: link.a.0,
                a_if: link.a.1.name(),
                b: link.b.0,
                b_if: link.b.1.name(),
                cable: link.cable,
                up: link.up,
                a_light: end_light(link.a),
                b_light: end_light(link.b),
                bps_ab: link.bps_ab,
                bps_ba: link.bps_ba,
            }
        })
        .collect();
    for (id, link) in &world.console_links {
        let powered = |node: u32| world.nodes.get(&node).is_some_and(|n| n.powered);
        let up = powered(link.a.0) && powered(link.b.0);
        let lamp = if up { Light::Green } else { Light::Off };
        links.push(LinkView {
            id: *id,
            a: link.a.0,
            a_if: link.a.1.clone(),
            b: link.b.0,
            b_if: link.b.1.clone(),
            cable: CableKind::Console,
            up,
            a_light: lamp,
            b_light: lamp,
            bps_ab: 0,
            bps_ba: 0,
        });
    }
    links.sort_by_key(|l| l.id);
    LabSnapshot {
        name: world.name.clone(),
        description: world.description.clone(),
        now: world.now,
        nodes: world.nodes.values().map(node_view).collect(),
        links,
        notes: notes.to_vec(),
    }
}

pub(crate) fn tables(node: &Node, now: u64) -> NodeTables {
    let routes = node
        .rib
        .routes
        .iter()
        .filter(|r| r.source != Source::Local)
        .flat_map(|r| {
            r.hops.iter().map(move |hop| RouteRow {
                code: if r.prefix.len == 0 && r.source == Source::Static { "S*".to_string() } else { r.source.code().to_string() },
                prefix: r.prefix.to_string(),
                ad: u32::from(r.distance),
                metric: r.metric,
                next_hop: hop.via.map(|v| v.to_string()).unwrap_or_default(),
                interface: hop.iface.map(|i| i.name()).unwrap_or_default(),
            })
        })
        .collect();
    let mut arp: Vec<ArpRow> = node
        .rib
        .addrs
        .iter()
        .filter(|(iface, _)| iface.kind != crate::ifname::IfKind::Loopback)
        .map(|(iface, addr)| ArpRow { ip: addr.ip.to_string(), mac: node.mac_for(*iface).to_string(), interface: iface.name(), age_s: 0 })
        .collect();
    arp.extend(node.arp.rows(now).into_iter().map(|(ip, entry)| ArpRow {
        ip: ip.to_string(),
        mac: entry.mac.to_string(),
        interface: entry.iface.name(),
        age_s: now.saturating_sub(entry.learned) / 1_000_000,
    }));
    let mac = node
        .macs
        .rows(now)
        .into_iter()
        .map(|(vlan, mac, entry)| MacRow {
            vlan,
            mac: mac.to_string(),
            kind: if entry.is_static { "STATIC" } else { "DYNAMIC" }.to_string(),
            port: entry.port.short(),
        })
        .collect();
    let vlans = if node.model.is_switch() {
        node.cfg
            .vlans
            .iter()
            .map(|(id, vlan)| VlanRow {
                id: *id,
                name: vlan.name.clone(),
                status: if vlan.active { "active" } else { "suspended" }.to_string(),
                ports: vlan_ports(node, *id).iter().map(|p| p.short()).collect(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut stp = Vec::new();
    for (vlan, instance) in &node.stp.vlans {
        let own = node.bridge_id(*vlan);
        for (port, p) in &instance.ports {
            stp.push(StpRow {
                vlan: *vlan,
                interface: port.short(),
                role: p.role.code().to_string(),
                state: p.state.code().to_string(),
                cost: node.stp_path_cost(*port),
                root_id: instance.root.to_string(),
                bridge_id: own.to_string(),
                is_root: instance.root_port.is_none() && p.role != PortRole::Alternate,
            });
        }
    }
    NodeTables {
        routes,
        arp,
        mac,
        vlans,
        stp,
        neighbors: node.neighbor_rows(now),
        ospf_neighbors: node.ospf_neighbor_rows(now),
        dhcp_bindings: node.dhcp_rows(now),
        nat: node.nat_rows(now),
    }
}

pub(crate) fn metrics(node: &Node, now: u64, seed: u64) -> NodeMetrics {
    let (mem_total, mem_used) = node.memory();
    NodeMetrics {
        hostname: node.cfg.hostname.clone(),
        uptime_s: if node.powered { node.uptime(now) / 1_000_000 } else { 0 },
        cpu_pct: if node.powered { node.cpu_percent(now, seed) } else { 0.0 },
        mem_used: if node.powered { mem_used } else { 0 },
        mem_total,
        temperature_c: if node.powered { node.temperature(now, seed) } else { 20.0 },
        interfaces: node
            .cfg
            .interfaces
            .iter()
            .filter_map(|(id, cfg)| {
                let state = node.ifs.get(id)?;
                let c = &state.counters;
                let speed = if state.speed > 0 { state.speed } else { id.kind.max_mbps().max(1000) };
                Some(IfCounters {
                    name: id.name(),
                    description: cfg.description.clone(),
                    admin_up: !cfg.shutdown,
                    oper_up: state.up,
                    speed_bps: u64::from(speed) * 1_000_000,
                    mtu: cfg.mtu.unwrap_or(1500),
                    in_octets: c.in_octets,
                    out_octets: c.out_octets,
                    in_pkts: c.in_pkts,
                    out_pkts: c.out_pkts,
                    in_errors: c.in_errors,
                    out_errors: c.out_errors,
                    in_discards: c.in_discards,
                    out_discards: c.out_discards,
                    crc: c.crc,
                    last_change: state.last_change,
                })
            })
            .collect(),
    }
}
