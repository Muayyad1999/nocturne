//! The routing table: connected, static and dynamic routes competing by
//! administrative distance, longest-prefix lookup, recursive next hops.

use std::net::Ipv4Addr;

use crate::api::SimTime;
use crate::catalog::Role;
use crate::ifname::IfId;
use crate::sim::fx::Fx;
use crate::sim::node::Node;
use crate::util::net::{IfAddr, Prefix};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Connected,
    /// The device's own address (`L` in `show ip route`).
    Local,
    Static,
    OspfIntra,
    OspfInter,
    OspfExternal,
    Rip,
}

impl Source {
    /// Route code as printed by `show ip route`.
    pub(crate) fn code(self) -> &'static str {
        match self {
            Source::Connected => "C",
            Source::Local => "L",
            Source::Static => "S",
            Source::OspfIntra => "O",
            Source::OspfInter => "O IA",
            Source::OspfExternal => "O E2",
            Source::Rip => "R",
        }
    }

    pub(crate) fn is_ospf(self) -> bool {
        matches!(self, Source::OspfIntra | Source::OspfInter | Source::OspfExternal)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NextHop {
    pub(crate) via: Option<Ipv4Addr>,
    pub(crate) iface: Option<IfId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Route {
    pub(crate) prefix: Prefix,
    pub(crate) source: Source,
    pub(crate) distance: u8,
    pub(crate) metric: u32,
    pub(crate) hops: Vec<NextHop>,
    pub(crate) since: SimTime,
}

/// Where a packet goes next: out of `iface`, to layer-2 neighbour `next_hop`.
#[derive(Clone, Debug)]
pub(crate) struct Resolved {
    pub(crate) iface: IfId,
    pub(crate) next_hop: Ipv4Addr,
    /// The route that matched (for explanations).
    pub(crate) route: Route,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Rib {
    /// Best routes, most specific first.
    pub(crate) routes: Vec<Route>,
    /// Addresses on up layer-3 interfaces (cache, rebuilt with the table).
    pub(crate) addrs: Vec<(IfId, IfAddr)>,
    /// Candidates offered by OSPF and RIP.
    pub(crate) ospf: Vec<Route>,
    pub(crate) rip: Vec<Route>,
    /// Default router learned by DHCP on an interface.
    pub(crate) dhcp_gateway: Option<Ipv4Addr>,
}

impl Rib {
    pub(crate) fn lookup(&self, dst: Ipv4Addr) -> Option<&Route> {
        self.routes.iter().find(|r| r.prefix.contains(dst))
    }

    pub(crate) fn get(&self, prefix: Prefix) -> Option<&Route> {
        self.routes.iter().find(|r| r.prefix == prefix)
    }

    /// The route of last resort, if any.
    pub(crate) fn default_route(&self) -> Option<&Route> {
        self.get(Prefix::DEFAULT)
    }

    pub(crate) fn clear(&mut self) {
        *self = Rib::default();
    }
}

impl Node {
    /// Does this node forward packets between interfaces?
    pub(crate) fn routes_packets(&self) -> bool {
        match self.role() {
            Role::Router | Role::Cloud => self.cfg.ip_routing,
            Role::L3Switch => self.cfg.ip_routing,
            Role::L2Switch | Role::Host | Role::Bridge => false,
        }
    }

    /// Recompute the routing table from every source of routes.
    pub(crate) fn rebuild_rib(&mut self, fx: &mut Fx) {
        let addrs = self.l3_addrs();
        let mut candidates: Vec<Route> = Vec::new();
        let route = |prefix, source, distance, metric, hops| Route { prefix, source, distance, metric, hops, since: fx.now };
        for (iface, addr) in &addrs {
            let hop = vec![NextHop { via: None, iface: Some(*iface) }];
            if addr.len < 32 {
                candidates.push(route(addr.subnet(), Source::Connected, 0, 0, hop.clone()));
            }
            candidates.push(route(Prefix::new(addr.ip, 32), Source::Local, 0, 0, hop));
        }
        let routing = self.routes_packets();
        if routing {
            candidates.extend(self.rib.ospf.iter().cloned());
            candidates.extend(self.rib.rip.iter().cloned());
        }
        // Without routing a device only knows its default gateway.
        let gateway = if routing { None } else { self.cfg.default_gateway };
        for gw in gateway.into_iter().chain(self.rib.dhcp_gateway) {
            let distance = if Some(gw) == gateway { 1 } else { 254 };
            candidates.push(route(Prefix::DEFAULT, Source::Static, distance, 0, vec![NextHop { via: Some(gw), iface: None }]));
        }

        // Static routes are only installed while their next hop resolves, and
        // may resolve through each other, so iterate to a fixed point.
        let statics = if routing { self.cfg.static_routes.clone() } else { Vec::new() };
        let mut installed = vec![false; statics.len()];
        loop {
            let table = select_best(&candidates);
            let mut progressed = false;
            for (i, st) in statics.iter().enumerate() {
                if installed[i] {
                    continue;
                }
                let iface_ok = st.iface.is_none_or(|iface| self.is_up(iface) && self.is_l3(iface));
                let via_ok = match (st.via, st.iface) {
                    (Some(via), None) => table
                        .iter()
                        .any(|r| r.prefix.len > 0 && r.prefix.contains(via) && r.prefix != st.prefix && r.source != Source::Local),
                    _ => true,
                };
                if iface_ok && via_ok {
                    installed[i] = true;
                    progressed = true;
                    candidates.push(route(
                        st.prefix,
                        Source::Static,
                        st.distance,
                        0,
                        vec![NextHop { via: st.via, iface: st.iface }],
                    ));
                }
            }
            if !progressed {
                break;
            }
        }

        let mut best = select_best(&candidates);
        // Keep the age of routes that did not change.
        for r in &mut best {
            if let Some(old) = self.rib.get(r.prefix)
                && old.source == r.source
                && old.hops == r.hops
                && old.metric == r.metric
            {
                r.since = old.since;
            }
        }
        let changed = best.len() != self.rib.routes.len()
            || best.iter().zip(&self.rib.routes).any(|(a, b)| (a.prefix, a.source, &a.hops, a.metric) != (b.prefix, b.source, &b.hops, b.metric));
        self.rib.routes = best;
        self.rib.addrs = addrs;
        if changed {
            fx.changed = true;
            fx.cp_change = true;
            // Other protocols redistribute or advertise what is in the table.
            fx.spf = true;
            self.rip_table_changed(fx);
        }
    }

    /// Resolve the egress interface and layer-2 next hop for `dst`.
    pub(crate) fn route_lookup(&self, dst: Ipv4Addr) -> Option<Resolved> {
        let route = self.rib.lookup(dst)?;
        let hop = pick_hop(route, dst)?;
        let mut target = hop.via.unwrap_or(dst);
        let mut iface = hop.iface;
        // Follow next hops that are themselves reached through another route.
        for _ in 0..8 {
            if let Some(iface) = iface {
                return self.is_up(iface).then(|| Resolved { iface, next_hop: target, route: route.clone() });
            }
            let inner = self.rib.lookup(target)?;
            let inner_hop = pick_hop(inner, target)?;
            iface = inner_hop.iface;
            if let Some(via) = inner_hop.via {
                target = via;
            }
        }
        None
    }

    /// The address this node would use as source when sending to `dst`.
    pub(crate) fn source_for(&self, dst: Ipv4Addr) -> Option<Ipv4Addr> {
        if let Some((_, addr)) = self.rib.addrs.iter().find(|(_, a)| a.ip == dst) {
            return Some(addr.ip);
        }
        let resolved = self.route_lookup(dst)?;
        self.rib
            .addrs
            .iter()
            .filter(|(iface, _)| *iface == resolved.iface)
            .max_by_key(|(_, a)| a.on_link(resolved.next_hop))
            .map(|(_, a)| a.ip)
            .or_else(|| self.rib.addrs.first().map(|(_, a)| a.ip))
    }
}

/// Equal-cost paths share load per destination.
fn pick_hop(route: &Route, dst: Ipv4Addr) -> Option<NextHop> {
    let n = route.hops.len();
    (n > 0).then(|| route.hops[u32::from(dst) as usize % n])
}

/// For every prefix keep the candidate with the lowest distance, then metric;
/// equal candidates from the same source merge into one multi-path route.
fn select_best(candidates: &[Route]) -> Vec<Route> {
    let mut best: Vec<Route> = Vec::new();
    for cand in candidates {
        match best.iter_mut().find(|r| r.prefix == cand.prefix) {
            None => best.push(cand.clone()),
            Some(existing) => {
                let rank = |r: &Route| (r.distance, r.metric);
                if rank(cand) < rank(existing) {
                    *existing = cand.clone();
                } else if rank(cand) == rank(existing) && cand.source == existing.source {
                    for hop in &cand.hops {
                        if !existing.hops.contains(hop) && existing.hops.len() < 4 {
                            existing.hops.push(*hop);
                        }
                    }
                }
            }
        }
    }
    // Most specific first, then by address: the order `lookup` relies on.
    best.sort_by(|a, b| b.prefix.len.cmp(&a.prefix.len).then(a.prefix.net.cmp(&b.prefix.net)));
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(prefix: &str, source: Source, distance: u8, metric: u32, via: [u8; 4]) -> Route {
        let (ip, len) = crate::util::net::parse_cidr(prefix).unwrap();
        Route {
            prefix: Prefix::new(ip, len),
            source,
            distance,
            metric,
            hops: vec![NextHop { via: Some(Ipv4Addr::from(via)), iface: None }],
            since: 0,
        }
    }

    #[test]
    fn best_route_selection() {
        let table = select_best(&[
            r("10.0.0.0/8", Source::Rip, 120, 2, [1, 1, 1, 1]),
            r("10.0.0.0/8", Source::OspfIntra, 110, 20, [2, 2, 2, 2]),
            r("10.1.0.0/16", Source::Static, 1, 0, [3, 3, 3, 3]),
            r("10.1.0.0/16", Source::Static, 1, 0, [4, 4, 4, 4]),
            r("0.0.0.0/0", Source::Static, 200, 0, [5, 5, 5, 5]),
            r("0.0.0.0/0", Source::OspfExternal, 110, 1, [6, 6, 6, 6]),
        ]);
        let rib = Rib { routes: table, ..Rib::default() };
        let hit = rib.lookup(Ipv4Addr::new(10, 1, 2, 3)).unwrap();
        assert_eq!((hit.prefix.len, hit.hops.len()), (16, 2));
        assert_eq!(rib.lookup(Ipv4Addr::new(10, 2, 0, 1)).unwrap().source, Source::OspfIntra);
        // The floating static loses to OSPF until OSPF withdraws its route.
        assert_eq!(rib.default_route().unwrap().source, Source::OspfExternal);
    }
}
