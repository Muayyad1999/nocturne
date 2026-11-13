//! OSPF route computation for every router at once.
//!
//! Instead of flooding link-state advertisements, the world reads every
//! router's interfaces and adjacencies directly and derives what the protocol
//! would converge to: intra-area routes by shortest path first, inter-area
//! routes through area border routers and the backbone, and type-2 external
//! routes through AS boundary routers.
//!
//! Flooding still leaves one visible trace: routers that are fully adjacent
//! (directly or through others) share the current picture, while a router
//! that has silently lost contact with another keeps that router's last
//! advertisement until its own dead timer ends the adjacency. Stale routes
//! therefore linger exactly as long as they would on real equipment.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;
use std::sync::Arc;

use crate::api::{NodeId, SimTime};
use crate::ifname::IfId;
use crate::sim::World;
use crate::sim::ospf::{Lsa, LsaKind, NbrState};
use crate::sim::rib::{NextHop, Route, Source};
use crate::util::net::Prefix;

/// Metric of redistributed routes, and of an originated default route.
const REDISTRIBUTE_METRIC: u32 = 20;
const DEFAULT_METRIC: u32 = 1;
const MAX_PATHS: usize = 4;

/// Something a router can have a route to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Dest {
    Net(Prefix),
    /// An AS boundary router, by router ID.
    Router(Ipv4Addr),
}

type Hops = Vec<NextHop>;

#[derive(Clone, Debug)]
struct IfInfo {
    iface: IfId,
    area: u32,
    subnet: Prefix,
    addr: Ipv4Addr,
    cost: u32,
    dr: Ipv4Addr,
    /// Router IDs of neighbours in FULL state.
    full: Vec<Ipv4Addr>,
}

/// What one router advertises about itself (the content of its own LSAs).
#[derive(Clone, Debug)]
pub(crate) struct Router {
    node: NodeId,
    rid: Ipv4Addr,
    ifaces: Vec<IfInfo>,
    /// Prefixes injected as type-2 externals, with their metric.
    externals: Vec<(Prefix, u32)>,
    /// When this picture of the router was taken.
    stamp: SimTime,
}

impl Router {
    fn areas(&self) -> BTreeSet<u32> {
        self.ifaces.iter().map(|i| i.area).collect()
    }

    fn is_abr(&self) -> bool {
        let areas = self.areas();
        areas.contains(&0) && areas.len() > 1
    }

    /// Do both routers claim a full adjacency with each other on a shared segment?
    fn adjacent(&self, other: &Router) -> bool {
        self.ifaces
            .iter()
            .filter(|i| i.full.contains(&other.rid))
            .any(|i| {
                other
                    .ifaces
                    .iter()
                    .any(|j| (j.area, j.subnet) == (i.area, i.subnet) && j.full.contains(&self.rid))
            })
    }
}

/// Everything reachable from the first `seeds` routers over claimed adjacencies.
fn connected(routers: &[Arc<Router>], seeds: usize) -> BTreeSet<usize> {
    let mut seen: BTreeSet<usize> = (0..seeds.min(routers.len())).collect();
    let mut todo: Vec<usize> = seen.iter().copied().collect();
    while let Some(a) = todo.pop() {
        for b in 0..routers.len() {
            if !seen.contains(&b) && routers[a].adjacent(&routers[b]) {
                seen.insert(b);
                todo.push(b);
            }
        }
    }
    seen
}

struct Edge {
    to: usize,
    cost: u32,
    hop: NextHop,
}

fn collect(world: &World) -> Vec<Router> {
    let mut routers = Vec::new();
    for node in world.nodes.values().filter(|n| n.powered) {
        let (Some(rid), Some(cfg)) = (node.ospf.router_id, node.cfg.ospf.as_ref()) else {
            continue;
        };
        let ifaces: Vec<IfInfo> = node
            .ospf
            .ifaces
            .iter()
            .map(|(id, state)| IfInfo {
                iface: *id,
                area: state.area.id,
                subnet: state.addr.subnet(),
                addr: state.addr.ip,
                cost: state.cost,
                dr: state.dr,
                full: state
                    .neighbors
                    .iter()
                    .filter(|(_, n)| n.state == NbrState::Full)
                    .map(|(rid, _)| *rid)
                    .collect(),
            })
            .collect();
        let covered = |prefix: Prefix| ifaces.iter().any(|i| i.subnet == prefix);
        let mut externals = Vec::new();
        for route in &node.rib.routes {
            let wanted = match route.source {
                Source::Static => cfg.redistribute_static && route.prefix.len > 0,
                Source::Connected => cfg.redistribute_connected && !covered(route.prefix),
                _ => false,
            };
            if wanted {
                externals.push((route.prefix, REDISTRIBUTE_METRIC));
            }
        }
        let has_default = node
            .rib
            .default_route()
            .is_some_and(|r| r.source == Source::Static);
        if cfg
            .default_originate
            .is_some_and(|always| always || has_default)
        {
            externals.push((Prefix::DEFAULT, DEFAULT_METRIC));
        }
        routers.push(Router {
            node: node.id,
            rid,
            ifaces,
            externals,
            stamp: world.now,
        });
    }
    routers
}

/// Transit segments: the designated router plus everyone fully adjacent to it.
/// Returns, per (area, subnet), the DR and the member (router, interface) pairs.
fn segments(routers: &[Arc<Router>]) -> BTreeMap<(u32, Prefix), (usize, Vec<(usize, usize)>)> {
    let mut by_subnet: BTreeMap<(u32, Prefix), Vec<(usize, usize)>> = BTreeMap::new();
    for (r, router) in routers.iter().enumerate() {
        for (i, iface) in router.ifaces.iter().enumerate() {
            by_subnet
                .entry((iface.area, iface.subnet))
                .or_default()
                .push((r, i));
        }
    }
    let mut out = BTreeMap::new();
    for (key, attached) in by_subnet {
        // The DR is whoever names itself; a segment without one carries no transit traffic.
        let Some(&(dr, dr_if)) = attached
            .iter()
            .find(|(r, i)| routers[*r].ifaces[*i].dr == routers[*r].ifaces[*i].addr)
        else {
            continue;
        };
        let mutual = |(r, i): &(usize, usize)| {
            routers[*r].ifaces[*i].full.contains(&routers[dr].rid)
                && routers[dr].ifaces[dr_if].full.contains(&routers[*r].rid)
        };
        let mut members: Vec<(usize, usize)> = attached.iter().copied().filter(mutual).collect();
        if members.is_empty() {
            continue;
        }
        members.push((dr, dr_if));
        out.insert(key, (dr, members));
    }
    out
}

fn merge_hops(into: &mut Hops, more: &Hops) {
    for hop in more {
        if !into.contains(hop) && into.len() < MAX_PATHS {
            into.push(*hop);
        }
    }
}

/// Shortest paths from `source` within one area: cost and first hops per router.
fn dijkstra(edges: &BTreeMap<usize, Vec<Edge>>, source: usize) -> BTreeMap<usize, (u32, Hops)> {
    let mut done: BTreeMap<usize, (u32, Hops)> = BTreeMap::new();
    let mut frontier: BTreeMap<usize, (u32, Hops)> = BTreeMap::from([(source, (0, Vec::new()))]);
    while let Some((&node, _)) = frontier.iter().min_by_key(|(n, (cost, _))| (*cost, **n)) {
        let Some((cost, hops)) = frontier.remove(&node) else {
            break;
        };
        for edge in edges.get(&node).map(Vec::as_slice).unwrap_or(&[]) {
            if done.contains_key(&edge.to) || edge.to == source {
                continue;
            }
            let total = cost.saturating_add(edge.cost);
            let via = if node == source {
                vec![edge.hop]
            } else {
                hops.clone()
            };
            match frontier.get_mut(&edge.to) {
                Some((best, best_hops)) if total == *best => merge_hops(best_hops, &via),
                Some((best, _)) if total > *best => {}
                _ => {
                    frontier.insert(edge.to, (total, via));
                }
            }
        }
        done.insert(node, (cost, hops));
    }
    done
}

/// Keep the cheaper of two candidates; equal costs share the load.
fn offer<K: Ord>(table: &mut BTreeMap<K, (u32, Hops)>, key: K, cost: u32, hops: &Hops) {
    match table.get_mut(&key) {
        Some((best, best_hops)) if cost == *best => {
            // A locally attached destination (no hops) never becomes a routed one.
            if !best_hops.is_empty() && !hops.is_empty() {
                merge_hops(best_hops, hops);
            } else {
                best_hops.clear();
            }
        }
        Some((best, _)) if cost > *best => {}
        _ => {
            table.insert(key, (cost, hops.clone()));
        }
    }
}

struct Computed {
    routes: Vec<Vec<Route>>,
    lsdbs: Vec<Vec<Lsa>>,
}

fn compute(routers: &[Arc<Router>]) -> Computed {
    let segs = segments(routers);
    // Per area: who can reach whom directly, at what cost, through which port.
    let mut edges: BTreeMap<u32, BTreeMap<usize, Vec<Edge>>> = BTreeMap::new();
    for ((area, _), (_, members)) in &segs {
        for &(r, i) in members {
            for &(s, j) in members.iter().filter(|(s, _)| *s != r) {
                let hop = NextHop {
                    via: Some(routers[s].ifaces[j].addr),
                    iface: Some(routers[r].ifaces[i].iface),
                };
                edges
                    .entry(*area)
                    .or_default()
                    .entry(r)
                    .or_default()
                    .push(Edge {
                        to: s,
                        cost: routers[r].ifaces[i].cost,
                        hop,
                    });
            }
        }
    }
    let no_edges = BTreeMap::new();

    // Shortest paths and reachable destinations for every router in each of its areas.
    let mut paths: Vec<BTreeMap<u32, BTreeMap<usize, (u32, Hops)>>> = Vec::new();
    let mut reach: Vec<BTreeMap<u32, BTreeMap<Dest, (u32, Hops)>>> = Vec::new();
    for (r, router) in routers.iter().enumerate() {
        let mut per_area_paths = BTreeMap::new();
        let mut per_area_reach = BTreeMap::new();
        for area in router.areas() {
            let tree = dijkstra(edges.get(&area).unwrap_or(&no_edges), r);
            let mut table: BTreeMap<Dest, (u32, Hops)> = BTreeMap::new();
            for (s, (cost, hops)) in &tree {
                let other = &routers[*s];
                for iface in other.ifaces.iter().filter(|i| i.area == area) {
                    offer(
                        &mut table,
                        Dest::Net(iface.subnet),
                        cost.saturating_add(iface.cost),
                        hops,
                    );
                }
                if !other.externals.is_empty() {
                    offer(&mut table, Dest::Router(other.rid), *cost, hops);
                }
            }
            per_area_paths.insert(area, tree);
            per_area_reach.insert(area, table);
        }
        paths.push(per_area_paths);
        reach.push(per_area_reach);
    }

    // What each area border router summarises into the backbone...
    let abrs: Vec<usize> = (0..routers.len())
        .filter(|r| routers[*r].is_abr())
        .collect();
    let mut to_backbone: BTreeMap<usize, BTreeMap<Dest, u32>> = BTreeMap::new();
    for &b in &abrs {
        let mut summary: BTreeMap<Dest, u32> = BTreeMap::new();
        for (area, table) in &reach[b] {
            if *area == 0 {
                continue;
            }
            for (dest, (cost, _)) in table {
                let entry = summary.entry(*dest).or_insert(*cost);
                *entry = (*entry).min(*cost);
            }
        }
        to_backbone.insert(b, summary);
    }
    // ...what every backbone router therefore knows: (cost, hops, is inter-area)...
    let mut backbone: BTreeMap<usize, BTreeMap<Dest, (u32, Hops, bool)>> = BTreeMap::new();
    for (r, _) in routers
        .iter()
        .enumerate()
        .filter(|(_, router)| router.areas().contains(&0))
    {
        let mut inter: BTreeMap<Dest, (u32, Hops)> = BTreeMap::new();
        let intra = &reach[r][&0];
        for &b in abrs.iter().filter(|b| **b != r) {
            let Some((to_abr, hops)) = paths[r][&0].get(&b) else {
                continue;
            };
            for (dest, cost) in &to_backbone[&b] {
                if !intra.contains_key(dest) {
                    offer(&mut inter, *dest, to_abr.saturating_add(*cost), hops);
                }
            }
        }
        let mut view: BTreeMap<Dest, (u32, Hops, bool)> = intra
            .iter()
            .map(|(dest, (cost, hops))| (*dest, (*cost, hops.clone(), false)))
            .collect();
        view.extend(
            inter
                .into_iter()
                .map(|(dest, (cost, hops))| (dest, (cost, hops, true))),
        );
        backbone.insert(r, view);
    }
    // ...and what the border routers summarise from the backbone into their other areas.
    let mut into_area: BTreeMap<(usize, u32), BTreeMap<Dest, u32>> = BTreeMap::new();
    for &b in &abrs {
        for area in routers[b].areas().into_iter().filter(|a| *a != 0) {
            let local = &reach[b][&area];
            let summary = backbone[&b]
                .iter()
                .filter(|(dest, _)| !local.contains_key(dest))
                .map(|(d, (cost, _, _))| (*d, *cost))
                .collect();
            into_area.insert((b, area), summary);
        }
    }

    let mut routes = Vec::new();
    let mut lsdbs = Vec::new();
    for (x, router) in routers.iter().enumerate() {
        // Intra-area beats inter-area regardless of cost.
        let mut intra: BTreeMap<Dest, (u32, Hops)> = BTreeMap::new();
        for table in reach[x].values() {
            for (dest, (cost, hops)) in table {
                offer(&mut intra, *dest, *cost, hops);
            }
        }
        let mut inter: BTreeMap<Dest, (u32, Hops)> = BTreeMap::new();
        if let Some(view) = backbone.get(&x) {
            for (dest, (cost, hops, _)) in view.iter().filter(|(_, (_, _, is_inter))| *is_inter) {
                offer(&mut inter, *dest, *cost, hops);
            }
        } else {
            for area in router.areas() {
                for &b in abrs.iter().filter(|b| routers[**b].areas().contains(&area)) {
                    let (Some((to_abr, hops)), Some(summary)) =
                        (paths[x][&area].get(&b), into_area.get(&(b, area)))
                    else {
                        continue;
                    };
                    for (dest, cost) in summary {
                        offer(&mut inter, *dest, to_abr.saturating_add(*cost), hops);
                    }
                }
            }
        }
        inter.retain(|dest, _| !intra.contains_key(dest));

        let mut table: Vec<Route> = Vec::new();
        let route = |prefix: Prefix, source: Source, metric: u32, hops: &Hops| Route {
            prefix,
            source,
            distance: 110,
            metric,
            hops: hops.clone(),
            since: 0,
        };
        for (source, set) in [(Source::OspfIntra, &intra), (Source::OspfInter, &inter)] {
            for (dest, (cost, hops)) in set {
                if let (Dest::Net(prefix), false) = (dest, hops.is_empty()) {
                    table.push(route(*prefix, source, *cost, hops));
                }
            }
        }
        // Type-2 externals: lowest external metric, then the nearest boundary router.
        let mut external: BTreeMap<Prefix, (u32, u32, Hops)> = BTreeMap::new();
        for other in routers
            .iter()
            .filter(|o| o.rid != router.rid && !o.externals.is_empty())
        {
            let Some((forward, hops)) = intra
                .get(&Dest::Router(other.rid))
                .or_else(|| inter.get(&Dest::Router(other.rid)))
            else {
                continue;
            };
            if hops.is_empty() {
                continue;
            }
            for (prefix, metric) in &other.externals {
                let own = router.externals.iter().any(|(p, _)| p == prefix);
                if own
                    || intra.contains_key(&Dest::Net(*prefix))
                    || inter.contains_key(&Dest::Net(*prefix))
                {
                    continue;
                }
                match external.get_mut(prefix) {
                    Some((m, f, h)) if (*metric, *forward) == (*m, *f) => merge_hops(h, hops),
                    Some((m, f, _)) if (*metric, *forward) > (*m, *f) => {}
                    _ => {
                        external.insert(*prefix, (*metric, *forward, hops.clone()));
                    }
                }
            }
        }
        table.extend(external.iter().map(|(prefix, (metric, _, hops))| {
            route(*prefix, Source::OspfExternal, *metric, hops)
        }));
        routes.push(table);

        // The link-state database as this router would hold it.
        let mut lsdb = Vec::new();
        for area in router.areas() {
            let reachable: BTreeSet<usize> = paths[x][&area].keys().copied().collect();
            for &s in &reachable {
                let links = routers[s].ifaces.iter().filter(|i| i.area == area).count() as u32;
                lsdb.push(Lsa {
                    area: Some(area),
                    kind: LsaKind::Router,
                    link_id: routers[s].rid,
                    adv_router: routers[s].rid,
                    links,
                });
            }
            for ((seg_area, _), (dr, members)) in &segs {
                if *seg_area == area && reachable.contains(dr) {
                    let dr_addr = members
                        .iter()
                        .find(|(r, _)| r == dr)
                        .map(|(r, i)| routers[*r].ifaces[*i].addr);
                    if let Some(link_id) = dr_addr {
                        lsdb.push(Lsa {
                            area: Some(area),
                            kind: LsaKind::Network,
                            link_id,
                            adv_router: routers[*dr].rid,
                            links: 0,
                        });
                    }
                }
            }
            for &b in abrs.iter().filter(|b| reachable.contains(b)) {
                let summary: Vec<Dest> = if area == 0 {
                    to_backbone[&b].keys().copied().collect()
                } else {
                    into_area
                        .get(&(b, area))
                        .map(|s| s.keys().copied().collect())
                        .unwrap_or_default()
                };
                for dest in summary {
                    let (kind, link_id) = match dest {
                        Dest::Net(prefix) => (LsaKind::Summary, prefix.net),
                        Dest::Router(rid) => (LsaKind::SummaryAsbr, rid),
                    };
                    if link_id != routers[b].rid || kind == LsaKind::Summary {
                        lsdb.push(Lsa {
                            area: Some(area),
                            kind,
                            link_id,
                            adv_router: routers[b].rid,
                            links: 0,
                        });
                    }
                }
            }
        }
        for other in routers {
            for (prefix, _) in &other.externals {
                lsdb.push(Lsa {
                    area: None,
                    kind: LsaKind::External,
                    link_id: prefix.net,
                    adv_router: other.rid,
                    links: 0,
                });
            }
        }
        lsdb.sort_by_key(|l| (l.area.is_none(), l.area, l.kind, l.link_id, l.adv_router));
        lsdbs.push(lsdb);
    }
    Computed { routes, lsdbs }
}

/// Recompute and install OSPF routes on every router.
pub(crate) fn recompute(world: &mut World) {
    let current: Vec<Arc<Router>> = collect(world).into_iter().map(Arc::new).collect();
    let mut wanted: BTreeMap<NodeId, (Vec<Route>, Vec<Lsa>, Vec<Arc<Router>>)> = BTreeMap::new();
    let mut placed: BTreeSet<usize> = BTreeSet::new();
    for first in 0..current.len() {
        if placed.contains(&first) {
            continue;
        }
        // Routers joined by full adjacencies have synchronised databases.
        let mut view: Vec<Arc<Router>> = vec![current[first].clone()];
        let mut index = vec![first];
        let mut next = 0;
        while next < view.len() {
            for (other, router) in current.iter().enumerate() {
                if !index.contains(&other) && view[next].adjacent(router) {
                    index.push(other);
                    view.push(router.clone());
                }
            }
            next += 1;
        }
        placed.extend(index.iter().copied());
        let members = view.len();
        // For routers they lost touch with, they keep the newest copy any of them has.
        let mut stale: BTreeMap<Ipv4Addr, Arc<Router>> = BTreeMap::new();
        for member in &view {
            let learned = world
                .nodes
                .get(&member.node)
                .map(|n| n.ospf.learned.as_slice())
                .unwrap_or_default();
            for old in learned
                .iter()
                .filter(|old| view.iter().all(|m| m.rid != old.rid))
            {
                if stale
                    .get(&old.rid)
                    .is_none_or(|have| have.stamp < old.stamp)
                {
                    stale.insert(old.rid, old.clone());
                }
            }
        }
        view.extend(stale.into_values());
        let keep = connected(&view, members);
        let view: Vec<Arc<Router>> = view
            .into_iter()
            .enumerate()
            .filter(|(i, _)| keep.contains(i))
            .map(|(_, r)| r)
            .collect();
        let computed = compute(&view);
        for (i, (routes, lsdb)) in computed
            .routes
            .into_iter()
            .zip(computed.lsdbs)
            .enumerate()
            .take(members)
        {
            let learned = view
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, r)| r.clone())
                .collect();
            wanted.insert(view[i].node, (routes, lsdb, learned));
        }
    }
    let now = world.now;
    let ids: Vec<NodeId> = world.nodes.keys().copied().collect();
    for id in ids {
        let (routes, lsdb, learned) = wanted.remove(&id).unwrap_or_default();
        let Some(node) = world.nodes.get_mut(&id) else {
            continue;
        };
        let same = node.rib.ospf.len() == routes.len()
            && node.rib.ospf.iter().zip(&routes).all(|(a, b)| {
                (a.prefix, a.source, a.metric, &a.hops) == (b.prefix, b.source, b.metric, &b.hops)
            });
        let stale = routes.is_empty() && node.rib.routes.iter().any(|r| r.source.is_ospf());
        node.ospf.learned = learned;
        // SPF runs when the link-state database changes, as on a real router.
        if node.ospf.router_id.is_some() && node.ospf.lsdb != lsdb {
            node.ospf.lsdb = lsdb;
            node.ospf.spf_runs += 1;
            node.ospf.last_spf = now;
        }
        if !same || stale {
            node.rib.ospf = routes;
            world.with_node(id, |node, fx| node.rebuild_rib(fx));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ifname::IfKind;

    fn iface(
        n: u32,
        area: u32,
        net: [u8; 4],
        host: u8,
        cost: u32,
        dr: [u8; 4],
        full: &[[u8; 4]],
    ) -> IfInfo {
        let subnet = Prefix::new(Ipv4Addr::from(net), 24);
        IfInfo {
            iface: IfId::new(IfKind::GigabitEthernet, &[0, n]),
            area,
            subnet,
            addr: Ipv4Addr::new(net[0], net[1], net[2], host),
            cost,
            dr: Ipv4Addr::from(dr),
            full: full.iter().map(|r| Ipv4Addr::from(*r)).collect(),
        }
    }

    /// R1 --area 1-- R2 --area 0-- R3, with a stub LAN on each end and a static
    /// route redistributed by R3.
    fn three_routers() -> Vec<Arc<Router>> {
        let rid = |n: u8| Ipv4Addr::new(n, n, n, n);
        let routers = vec![
            Router {
                node: 1,
                rid: rid(1),
                ifaces: vec![
                    iface(0, 1, [10, 1, 0, 0], 1, 10, [10, 1, 0, 1], &[]),
                    iface(1, 1, [10, 0, 12, 0], 1, 1, [10, 0, 12, 2], &[[2, 2, 2, 2]]),
                ],
                externals: vec![],
                stamp: 0,
            },
            Router {
                node: 2,
                rid: rid(2),
                ifaces: vec![
                    iface(0, 1, [10, 0, 12, 0], 2, 1, [10, 0, 12, 2], &[[1, 1, 1, 1]]),
                    iface(1, 0, [10, 0, 23, 0], 2, 5, [10, 0, 23, 3], &[[3, 3, 3, 3]]),
                ],
                externals: vec![],
                stamp: 0,
            },
            Router {
                node: 3,
                rid: rid(3),
                ifaces: vec![
                    iface(0, 0, [10, 0, 23, 0], 3, 5, [10, 0, 23, 3], &[[2, 2, 2, 2]]),
                    iface(1, 0, [10, 3, 0, 0], 1, 10, [10, 3, 0, 1], &[]),
                ],
                externals: vec![(
                    Prefix::new(Ipv4Addr::new(172, 16, 0, 0), 16),
                    REDISTRIBUTE_METRIC,
                )],
                stamp: 0,
            },
        ];
        routers.into_iter().map(Arc::new).collect()
    }

    fn find(routes: &[Route], prefix: &str) -> Option<(Source, u32, String)> {
        let (ip, len) = crate::util::net::parse_cidr(prefix).unwrap();
        routes
            .iter()
            .find(|r| r.prefix == Prefix::new(ip, len))
            .map(|r| {
                (
                    r.source,
                    r.metric,
                    r.hops
                        .iter()
                        .map(|h| h.via.unwrap().to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                )
            })
    }

    #[test]
    fn intra_inter_and_external_routes() {
        let routers = three_routers();
        let computed = compute(&routers);
        let (r1, r2, r3) = (
            &computed.routes[0],
            &computed.routes[1],
            &computed.routes[2],
        );
        // R1 (internal to area 1) sees area 0 through the border router R2.
        assert_eq!(
            find(r1, "10.0.23.0/24"),
            Some((Source::OspfInter, 6, "10.0.12.2".into()))
        );
        assert_eq!(
            find(r1, "10.3.0.0/24"),
            Some((Source::OspfInter, 16, "10.0.12.2".into()))
        );
        assert_eq!(
            find(r1, "172.16.0.0/16"),
            Some((Source::OspfExternal, 20, "10.0.12.2".into()))
        );
        assert_eq!(find(r1, "10.1.0.0/24"), None, "own connected network");
        // The border router has intra-area routes into both areas.
        assert_eq!(
            find(r2, "10.1.0.0/24"),
            Some((Source::OspfIntra, 11, "10.0.12.1".into()))
        );
        assert_eq!(
            find(r2, "10.3.0.0/24"),
            Some((Source::OspfIntra, 15, "10.0.23.3".into()))
        );
        // R3 (backbone) learns area 1 as inter-area and does not route to its own static.
        assert_eq!(
            find(r3, "10.1.0.0/24"),
            Some((Source::OspfInter, 16, "10.0.23.2".into()))
        );
        assert_eq!(
            find(r3, "10.0.12.0/24"),
            Some((Source::OspfInter, 6, "10.0.23.2".into()))
        );
        assert_eq!(find(r3, "172.16.0.0/16"), None);
        // Database: R1 holds router LSAs for area 1 only, plus summaries and the external.
        let lsdb = &computed.lsdbs[0];
        assert_eq!(lsdb.iter().filter(|l| l.kind == LsaKind::Router).count(), 2);
        assert_eq!(
            lsdb.iter().filter(|l| l.kind == LsaKind::Network).count(),
            1
        );
        assert!(
            lsdb.iter()
                .any(|l| l.kind == LsaKind::Summary && l.link_id == Ipv4Addr::new(10, 3, 0, 0))
        );
        assert!(
            lsdb.iter()
                .any(|l| l.kind == LsaKind::SummaryAsbr && l.link_id == Ipv4Addr::new(3, 3, 3, 3))
        );
        assert!(
            lsdb.iter()
                .any(|l| l.kind == LsaKind::External && l.link_id == Ipv4Addr::new(172, 16, 0, 0))
        );
    }

    #[test]
    fn adjacency_must_be_full_on_both_sides() {
        let mut routers = three_routers();
        // R3 no longer considers R2 fully adjacent.
        Arc::make_mut(&mut routers[2]).ifaces[0].full.clear();
        let computed = compute(&routers);
        assert_eq!(
            connected(&routers, 1),
            BTreeSet::from([0, 1]),
            "R3 is cut off from R1's point of view"
        );
        assert_eq!(find(&computed.routes[0], "10.3.0.0/24"), None);
        assert_eq!(find(&computed.routes[1], "10.3.0.0/24"), None);
        assert_eq!(
            find(&computed.routes[1], "10.1.0.0/24").map(|r| r.0),
            Some(Source::OspfIntra)
        );
    }
}
