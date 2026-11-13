//! OSPFv2.
//!
//! Neighbour discovery is real: hellos travel through the simulated network,
//! so adjacency needs genuine layer-2 reachability, matching areas, timers and
//! MTU, and neighbours move through timed states with a dead interval. The
//! link-state database itself is not flooded: whenever an adjacency or an
//! OSPF-relevant input changes, [`recompute`] derives every router's routes
//! from the adjacencies that are FULL on both sides (see `ospf_spf.rs`).

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::{OspfNeighborRow, SimTime};
use crate::device::config::Area;
use crate::ifname::{IfId, IfKind};
use crate::sim::frame::{L4, Meta, OspfHello, Packet};
use crate::sim::fx::{Fx, Timer};
use crate::sim::node::Node;
use crate::util::net::IfAddr;

pub(crate) use crate::sim::ospf_spf::recompute;

pub(crate) const ALL_SPF_ROUTERS: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum NbrState {
    Init,
    TwoWay,
    ExStart,
    Exchange,
    Loading,
    Full,
}

impl NbrState {
    pub(crate) fn word(self) -> &'static str {
        match self {
            NbrState::Init => "INIT",
            NbrState::TwoWay => "2WAY",
            NbrState::ExStart => "EXSTART",
            NbrState::Exchange => "EXCHANGE",
            NbrState::Loading => "LOADING",
            NbrState::Full => "FULL",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Neighbor {
    pub(crate) addr: Ipv4Addr,
    pub(crate) priority: u8,
    pub(crate) state: NbrState,
    /// When the dead timer runs out unless another hello arrives.
    pub(crate) dead_at: SimTime,
    /// The neighbour's idea of DR and BDR (interface addresses).
    pub(crate) dr: Ipv4Addr,
    pub(crate) bdr: Ipv4Addr,
    pub(crate) mtu: u32,
    pub(crate) since: SimTime,
    pub(crate) changes: u32,
    dead_token: u32,
    adj_token: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IfRole {
    Loopback,
    Waiting,
    Dr,
    Backup,
    DrOther,
}

impl IfRole {
    pub(crate) fn word(self) -> &'static str {
        match self {
            IfRole::Loopback => "LOOP",
            IfRole::Waiting => "WAIT",
            IfRole::Dr => "DR",
            IfRole::Backup => "BDR",
            IfRole::DrOther => "DROTH",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OspfIface {
    pub(crate) area: Area,
    pub(crate) addr: IfAddr,
    pub(crate) cost: u32,
    pub(crate) hello_s: u16,
    pub(crate) dead_s: u16,
    pub(crate) priority: u8,
    pub(crate) mtu: u32,
    pub(crate) passive: bool,
    /// Enabled by `ip ospf N area A` on the interface rather than a network statement.
    pub(crate) by_interface: bool,
    pub(crate) role: IfRole,
    pub(crate) dr: Ipv4Addr,
    pub(crate) bdr: Ipv4Addr,
    pub(crate) neighbors: BTreeMap<Ipv4Addr, Neighbor>,
    /// Hellos sent since the interface came up (the first few go out quickly).
    hellos_sent: u32,
    token: u32,
}

/// One line of `show ip ospf database`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lsa {
    /// `None` for AS-external LSAs.
    pub(crate) area: Option<u32>,
    pub(crate) kind: LsaKind,
    pub(crate) link_id: Ipv4Addr,
    pub(crate) adv_router: Ipv4Addr,
    pub(crate) links: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LsaKind {
    Router,
    Network,
    Summary,
    SummaryAsbr,
    External,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Ospf {
    /// Router ID in effect; latched when the process starts.
    pub(crate) router_id: Option<Ipv4Addr>,
    pub(crate) ifaces: BTreeMap<IfId, OspfIface>,
    pub(crate) lsdb: Vec<Lsa>,
    pub(crate) started: SimTime,
    pub(crate) spf_runs: u32,
    pub(crate) last_spf: SimTime,
    /// What is injected as external routes, to notice when that changes.
    injecting: (bool, bool, Option<bool>),
    /// Other routers' advertisements as this router last received them.
    pub(crate) learned: Vec<std::sync::Arc<crate::sim::ospf_spf::Router>>,
    next_token: u32,
}

impl OspfIface {
    /// Neighbours we are, or should become, fully adjacent with.
    fn wants_adjacency(&self, neighbor: &Neighbor) -> bool {
        let own = self.addr.ip;
        [self.dr, self.bdr]
            .iter()
            .any(|d| *d == own || *d == neighbor.addr)
    }

    /// Role suffix shown after a neighbour's state: `DR`, `BDR` or `DROTHER`.
    pub(crate) fn neighbor_role(&self, neighbor: &Neighbor) -> &'static str {
        if neighbor.addr == self.dr {
            "DR"
        } else if neighbor.addr == self.bdr {
            "BDR"
        } else {
            "DROTHER"
        }
    }

    pub(crate) fn full_neighbors(&self) -> usize {
        self.neighbors
            .values()
            .filter(|n| n.state == NbrState::Full)
            .count()
    }
}

/// Step delay between EXSTART, EXCHANGE, LOADING and FULL.
fn adjacency_step(fx: &Fx) -> u64 {
    if fx.realistic { 1_000_000 } else { 100_000 }
}

impl Node {
    fn ospf_token(&mut self) -> u32 {
        self.ospf.next_token = self.ospf.next_token.wrapping_add(1);
        self.ospf.next_token
    }

    fn ospf_pid(&self) -> u16 {
        self.cfg.ospf.as_ref().map_or(1, |o| o.pid)
    }

    /// The interfaces OSPF should run on, with their parameters.
    fn ospf_wanted(&self) -> BTreeMap<IfId, OspfIface> {
        let mut out = BTreeMap::new();
        let Some(ospf) = self.cfg.ospf.as_ref().filter(|_| self.routes_packets()) else {
            return out;
        };
        for (id, cfg) in &self.cfg.interfaces {
            if !self.is_l3(*id) || !self.is_up(*id) {
                continue;
            }
            let Some(addr) = self.addr(*id) else { continue };
            // An interface-level `ip ospf N area A` wins over network statements,
            // and among network statements the most specific match wins.
            let by_interface = cfg
                .ospf
                .area
                .filter(|(pid, _)| *pid == ospf.pid)
                .map(|(_, area)| area);
            let by_network = ospf
                .networks
                .iter()
                .filter(|n| crate::util::net::wildcard_match(addr.ip, n.addr, n.wildcard))
                .min_by_key(|n| u32::from(n.wildcard))
                .map(|n| n.area);
            let Some(area) = by_interface.or(by_network) else {
                continue;
            };
            let kbps = match (cfg.bandwidth, id.kind) {
                (Some(kbps), _) => u64::from(kbps),
                (None, IfKind::Loopback) => 8_000_000,
                (None, IfKind::Vlan) => 1_000_000,
                (None, _) => {
                    u64::from(self.ifs.get(&id.parent()).map_or(1000, |s| s.speed.max(1))) * 1000
                }
            };
            let cost = cfg.ospf.cost.unwrap_or_else(|| {
                (u64::from(ospf.reference_bandwidth) * 1000 / kbps.max(1)).clamp(1, 65_535) as u32
            });
            let hello_s = cfg.ospf.hello.unwrap_or(10);
            let loopback = id.kind == IfKind::Loopback;
            out.insert(
                *id,
                OspfIface {
                    area,
                    // Loopbacks are advertised as host routes.
                    addr: if loopback {
                        IfAddr::new(addr.ip, 32)
                    } else {
                        addr
                    },
                    cost,
                    hello_s,
                    dead_s: cfg.ospf.dead.unwrap_or(hello_s.saturating_mul(4)),
                    priority: cfg.ospf.priority.unwrap_or(1),
                    mtu: cfg.mtu.unwrap_or(1500),
                    passive: loopback || ospf.is_passive(*id),
                    by_interface: by_interface.is_some(),
                    role: if loopback {
                        IfRole::Loopback
                    } else {
                        IfRole::Waiting
                    },
                    dr: Ipv4Addr::UNSPECIFIED,
                    bdr: Ipv4Addr::UNSPECIFIED,
                    neighbors: BTreeMap::new(),
                    hellos_sent: 0,
                    token: 0,
                },
            );
        }
        out
    }

    /// Bring the running OSPF state in line with configuration and interfaces.
    pub(crate) fn ospf_sync(&mut self, fx: &mut Fx) {
        let wanted = self.ospf_wanted();
        if self.cfg.ospf.is_none() || !self.routes_packets() {
            if self.ospf.router_id.is_some() || !self.ospf.ifaces.is_empty() {
                let ids: Vec<IfId> = self.ospf.ifaces.keys().copied().collect();
                for id in ids {
                    self.ospf_iface_down(fx, id, "Interface down or detached");
                }
                self.ospf = Ospf {
                    next_token: self.ospf.next_token,
                    ..Ospf::default()
                };
                self.rib.ospf.clear();
                fx.spf = true;
            }
            return;
        }
        if self.ospf.router_id.is_none() {
            let configured = self.cfg.ospf.as_ref().and_then(|o| o.router_id);
            let loopback = self
                .rib
                .addrs
                .iter()
                .filter(|(id, _)| id.kind == IfKind::Loopback)
                .map(|(_, a)| a.ip)
                .max();
            let any = self.rib.addrs.iter().map(|(_, a)| a.ip).max();
            // rib.addrs is refreshed just before this runs, so new addresses count.
            let Some(router_id) = configured.or(loopback).or(any) else {
                return;
            };
            self.ospf.router_id = Some(router_id);
            self.ospf.started = fx.now;
            fx.spf = true;
        }
        let injecting = self
            .cfg
            .ospf
            .as_ref()
            .map(|o| {
                (
                    o.redistribute_static,
                    o.redistribute_connected,
                    o.default_originate,
                )
            })
            .unwrap_or_default();
        if self.ospf.injecting != injecting {
            self.ospf.injecting = injecting;
            fx.spf = true;
        }
        let gone: Vec<IfId> = self
            .ospf
            .ifaces
            .keys()
            .filter(|id| !wanted.contains_key(id))
            .copied()
            .collect();
        for id in gone {
            self.ospf_iface_down(fx, id, "Interface down or detached");
            self.ospf.ifaces.remove(&id);
            fx.spf = true;
        }
        for (id, new) in wanted {
            let restart = match self.ospf.ifaces.get(&id) {
                None => true,
                // Timer changes do not reset anything: mismatched hellos are
                // simply ignored until the neighbour's dead timer runs out.
                Some(old) => {
                    (old.area.id, old.addr, old.passive, old.mtu)
                        != (new.area.id, new.addr, new.passive, new.mtu)
                }
            };
            if restart {
                self.ospf_iface_down(fx, id, "Interface down or detached");
                let token = self.ospf_token();
                let (passive, loopback, dead_s) =
                    (new.passive, new.role == IfRole::Loopback, new.dead_s);
                self.ospf.ifaces.insert(id, OspfIface { token, ..new });
                fx.spf = true;
                if !passive {
                    fx.arm(1, Timer::OspfHello { iface: id, token });
                }
                if !loopback {
                    // The wait timer postpones the DR election (a passive
                    // interface elects itself once it runs out).
                    let wait = if fx.realistic {
                        u64::from(dead_s) * 1_000_000
                    } else {
                        2_000_000
                    };
                    fx.arm(wait, Timer::OspfWait { iface: id, token });
                }
            } else if let Some(old) = self.ospf.ifaces.get_mut(&id) {
                if (old.cost, old.priority, old.area.dotted)
                    != (new.cost, new.priority, new.area.dotted)
                {
                    fx.spf |= old.cost != new.cost;
                    old.cost = new.cost;
                    old.priority = new.priority;
                    old.area = new.area;
                }
                old.hello_s = new.hello_s;
                old.dead_s = new.dead_s;
                old.by_interface = new.by_interface;
            }
        }
    }

    /// `clear ip ospf process`: drop every adjacency and start over.
    pub(crate) fn ospf_restart(&mut self, fx: &mut Fx) {
        let ids: Vec<IfId> = self.ospf.ifaces.keys().copied().collect();
        for id in ids {
            self.ospf_iface_down(fx, id, "Interface down or detached");
        }
        self.ospf = Ospf {
            next_token: self.ospf.next_token,
            ..Ospf::default()
        };
        self.rib.ospf.clear();
        fx.spf = true;
        fx.dirty = true;
    }

    /// All neighbours on an interface are gone.
    fn ospf_iface_down(&mut self, fx: &mut Fx, iface: IfId, why: &str) {
        let Some(state) = self.ospf.ifaces.get_mut(&iface) else {
            return;
        };
        let lost: Vec<(Ipv4Addr, NbrState)> = state
            .neighbors
            .iter()
            .map(|(rid, n)| (*rid, n.state))
            .collect();
        state.neighbors.clear();
        for (rid, was) in lost {
            self.ospf_log_change(
                fx,
                rid,
                iface,
                was,
                "DOWN",
                &format!("Neighbor Down: {why}"),
            );
        }
    }

    fn ospf_log_change(
        &mut self,
        fx: &mut Fx,
        rid: Ipv4Addr,
        iface: IfId,
        from: NbrState,
        to: &str,
        why: &str,
    ) {
        fx.cp_change = true;
        fx.changed = true;
        // By default only adjacencies coming up or going away are logged.
        if from == NbrState::Full || to == "FULL" {
            fx.spf = true;
            let text = format!(
                "Process {}, Nbr {rid} on {} from {} to {to}, {why}",
                self.ospf_pid(),
                iface.name(),
                from.word()
            );
            self.log(fx, 5, "OSPF", "ADJCHG", text);
        }
    }

    fn ospf_send_hello(&mut self, fx: &mut Fx, iface: IfId) {
        let Some(router_id) = self.ospf.router_id else {
            return;
        };
        let Some(state) = self.ospf.ifaces.get(&iface) else {
            return;
        };
        let hello = OspfHello {
            router_id,
            area: state.area.id,
            mask_len: state.addr.len,
            hello_s: state.hello_s,
            dead_s: state.dead_s,
            priority: state.priority,
            dr: state.dr,
            bdr: state.bdr,
            neighbors: state.neighbors.keys().copied().collect(),
            mtu: state.mtu,
        };
        let packet = Packet {
            src: state.addr.ip,
            dst: ALL_SPF_ROUTERS,
            ttl: 1,
            size: 64,
            l4: L4::Ospf(Box::new(hello)),
        };
        let noun = self.noun();
        self.ip_send_on(fx, iface, packet, None, || {
            format!(
                "The {noun} multicasts an OSPF hello out of {} to find and keep its neighbours (224.0.0.5 is never routed).",
                iface.short()
            )
        });
    }

    pub(crate) fn ospf_hello_timer(&mut self, fx: &mut Fx, iface: IfId, token: u32) {
        let Some(state) = self
            .ospf
            .ifaces
            .get_mut(&iface)
            .filter(|s| s.token == token && !s.passive)
        else {
            return;
        };
        state.hellos_sent += 1;
        // The first hellos go out a second apart so neighbours behind a port
        // that is still in spanning-tree learning are found quickly.
        let quick = !fx.realistic && state.hellos_sent <= 5;
        let delay = if quick {
            1_000_000
        } else {
            u64::from(state.hello_s.max(1)) * 1_000_000
        };
        fx.arm(delay, Timer::OspfHello { iface, token });
        self.ospf_send_hello(fx, iface);
    }

    pub(crate) fn ospf_rx(&mut self, fx: &mut Fx, iface: IfId, src: Ipv4Addr, hello: &OspfHello) {
        let Some(own_id) = self.ospf.router_id else {
            return;
        };
        let now = fx.now;
        let Some(state) = self.ospf.ifaces.get_mut(&iface) else {
            return;
        };
        // Hellos must agree on area, subnet and timers, or they are ignored.
        let compatible = !state.passive
            && hello.area == state.area.id
            && hello.mask_len == state.addr.len
            && state.addr.on_link(src)
            && hello.hello_s == state.hello_s
            && hello.dead_s == state.dead_s
            && hello.router_id != own_id;
        if !compatible {
            return;
        }
        let sees_us = hello.neighbors.contains(&own_id);
        let dead = u64::from(state.dead_s) * 1_000_000;
        let waiting = state.role == IfRole::Waiting;
        let is_new = !state.neighbors.contains_key(&hello.router_id);
        self.ospf.next_token = self.ospf.next_token.wrapping_add(1);
        let token = self.ospf.next_token;
        let neighbor = state.neighbors.entry(hello.router_id).or_insert(Neighbor {
            addr: src,
            priority: hello.priority,
            state: NbrState::Init,
            dead_at: now + dead,
            dr: hello.dr,
            bdr: hello.bdr,
            mtu: hello.mtu,
            since: now,
            changes: 0,
            dead_token: 0,
            adj_token: 0,
        });
        let role_news =
            (neighbor.dr, neighbor.bdr, neighbor.priority) != (hello.dr, hello.bdr, hello.priority);
        neighbor.addr = src;
        neighbor.priority = hello.priority;
        neighbor.dr = hello.dr;
        neighbor.bdr = hello.bdr;
        neighbor.mtu = hello.mtu;
        neighbor.dead_at = now + dead;
        neighbor.dead_token = token;
        fx.arm(
            dead,
            Timer::OspfNeighbor {
                iface,
                router_id: hello.router_id,
                token,
                dead: true,
            },
        );
        let was = neighbor.state;
        let mut elect = role_news && !waiting;
        match (sees_us, was) {
            (true, NbrState::Init) => {
                neighbor.state = NbrState::TwoWay;
                neighbor.changes += 1;
                elect |= !waiting;
                fx.cp_change = true;
                fx.changed = true;
            }
            (false, s) if s > NbrState::Init => {
                // The neighbour no longer lists us: it restarted or lost our hellos.
                neighbor.state = NbrState::Init;
                neighbor.changes += 1;
                elect |= !waiting;
                self.ospf_log_change(fx, hello.router_id, iface, was, "INIT", "1-Way");
            }
            _ => {}
        }
        // A neighbour that already acts as backup (or as DR without a backup)
        // means the segment has settled: no need to sit out the wait time.
        let backup_seen = hello.bdr == src || (hello.dr == src && hello.bdr.is_unspecified());
        if is_new {
            fx.changed = true;
            self.ospf_send_hello(fx, iface);
        }
        if (waiting && sees_us && backup_seen) || elect {
            self.ospf_elect(fx, iface);
        } else {
            self.ospf_adjacencies(fx, iface);
        }
    }

    pub(crate) fn ospf_wait_timer(&mut self, fx: &mut Fx, iface: IfId, token: u32) {
        let waiting = self
            .ospf
            .ifaces
            .get(&iface)
            .is_some_and(|s| s.token == token && s.role == IfRole::Waiting);
        if waiting {
            self.ospf_elect(fx, iface);
        }
    }

    /// Designated-router election (RFC 2328 section 9.4, without pre-emption).
    fn ospf_elect(&mut self, fx: &mut Fx, iface: IfId) {
        let Some(own_id) = self.ospf.router_id else {
            return;
        };
        let Some(state) = self.ospf.ifaces.get_mut(&iface) else {
            return;
        };
        if state.role == IfRole::Loopback {
            return;
        }
        let own = state.addr.ip;
        let before = (state.dr, state.bdr, state.role);
        // Candidates: (priority, router id, address, declared DR, declared BDR).
        let pass = |dr: Ipv4Addr, bdr: Ipv4Addr| -> (Ipv4Addr, Ipv4Addr) {
            let mut candidates: Vec<(u8, Ipv4Addr, Ipv4Addr, Ipv4Addr, Ipv4Addr)> =
                vec![(state.priority, own_id, own, dr, bdr)];
            candidates.extend(
                state
                    .neighbors
                    .iter()
                    .filter(|(_, n)| n.state >= NbrState::TwoWay)
                    .map(|(rid, n)| (n.priority, *rid, n.addr, n.dr, n.bdr)),
            );
            candidates.retain(|c| c.0 > 0);
            let best = |list: Vec<&(u8, Ipv4Addr, Ipv4Addr, Ipv4Addr, Ipv4Addr)>| {
                list.into_iter().max_by_key(|c| (c.0, c.1)).map(|c| c.2)
            };
            let not_dr: Vec<_> = candidates.iter().filter(|c| c.3 != c.2).collect();
            let declared_bdr: Vec<_> = not_dr.iter().copied().filter(|c| c.4 == c.2).collect();
            let new_bdr = if declared_bdr.is_empty() {
                best(not_dr)
            } else {
                best(declared_bdr)
            };
            let declared_dr: Vec<_> = candidates.iter().filter(|c| c.3 == c.2).collect();
            let new_dr = best(declared_dr).or(new_bdr);
            let new_bdr = new_bdr.filter(|b| Some(*b) != new_dr);
            (
                new_dr.unwrap_or(Ipv4Addr::UNSPECIFIED),
                new_bdr.unwrap_or(Ipv4Addr::UNSPECIFIED),
            )
        };
        let (mut dr, mut bdr) = pass(state.dr, state.bdr);
        // If our own role changed, run it again with the new self-declaration.
        let role_changed = (dr == own) != (state.dr == own) || (bdr == own) != (state.bdr == own);
        if role_changed {
            (dr, bdr) = pass(dr, bdr);
        }
        state.dr = dr;
        state.bdr = bdr;
        state.role = if dr == own {
            IfRole::Dr
        } else if bdr == own {
            IfRole::Backup
        } else {
            IfRole::DrOther
        };
        if before != (state.dr, state.bdr, state.role) {
            fx.cp_change = true;
            fx.changed = true;
            fx.spf = true;
            self.ospf_send_hello(fx, iface);
        }
        self.ospf_adjacencies(fx, iface);
    }

    /// Start adjacencies that should exist; tear down those that should not.
    fn ospf_adjacencies(&mut self, fx: &mut Fx, iface: IfId) {
        let step = adjacency_step(fx);
        let Some(state) = self.ospf.ifaces.get(&iface) else {
            return;
        };
        if state.role == IfRole::Waiting {
            return;
        }
        let mut start = Vec::new();
        let mut stop = Vec::new();
        for (rid, neighbor) in &state.neighbors {
            let wanted = state.wants_adjacency(neighbor);
            match (wanted, neighbor.state) {
                (true, NbrState::TwoWay) => start.push(*rid),
                (false, s) if s > NbrState::TwoWay => stop.push((*rid, s)),
                _ => {}
            }
        }
        for rid in start {
            let token = self.ospf_token();
            let Some(neighbor) = self
                .ospf
                .ifaces
                .get_mut(&iface)
                .and_then(|s| s.neighbors.get_mut(&rid))
            else {
                continue;
            };
            neighbor.state = NbrState::ExStart;
            neighbor.changes += 1;
            neighbor.adj_token = token;
            let addr = neighbor.addr;
            fx.arm(
                step,
                Timer::OspfNeighbor {
                    iface,
                    router_id: rid,
                    token,
                    dead: false,
                },
            );
            fx.cp_change = true;
            fx.changed = true;
            // The database exchange is unicast, so the routers resolve each
            // other's MAC addresses now rather than on the first routed packet.
            self.arp_resolve(fx, iface, addr, &Meta::default());
        }
        for (rid, was) in stop {
            if let Some(neighbor) = self
                .ospf
                .ifaces
                .get_mut(&iface)
                .and_then(|s| s.neighbors.get_mut(&rid))
            {
                neighbor.state = NbrState::TwoWay;
                neighbor.changes += 1;
            }
            self.ospf_log_change(fx, rid, iface, was, "2WAY", "AdjOK?");
        }
    }

    pub(crate) fn ospf_neighbor_timer(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        router_id: Ipv4Addr,
        token: u32,
        dead: bool,
    ) {
        let step = adjacency_step(fx);
        let Some(state) = self.ospf.ifaces.get_mut(&iface) else {
            return;
        };
        let own_mtu = state.mtu;
        let Some(neighbor) = state.neighbors.get_mut(&router_id) else {
            return;
        };
        if dead {
            if neighbor.dead_token != token {
                return;
            }
            let was = neighbor.state;
            state.neighbors.remove(&router_id);
            self.ospf_log_change(
                fx,
                router_id,
                iface,
                was,
                "DOWN",
                "Neighbor Down: Dead timer expired",
            );
            self.ospf_elect(fx, iface);
            return;
        }
        if neighbor.adj_token != token {
            return;
        }
        let was = neighbor.state;
        let next = match was {
            // Database description packets carry the MTU; a mismatch stops here.
            NbrState::ExStart if neighbor.mtu != own_mtu => return,
            NbrState::ExStart => NbrState::Exchange,
            NbrState::Exchange => NbrState::Loading,
            NbrState::Loading => NbrState::Full,
            NbrState::Init | NbrState::TwoWay | NbrState::Full => return,
        };
        neighbor.state = next;
        neighbor.changes += 1;
        if next == NbrState::Full {
            neighbor.since = fx.now;
            self.ospf_log_change(fx, router_id, iface, was, "FULL", "Loading Done");
        } else {
            fx.arm(
                step,
                Timer::OspfNeighbor {
                    iface,
                    router_id,
                    token,
                    dead: false,
                },
            );
            fx.changed = true;
        }
    }

    /// Is an adjacency still on its way up?
    pub(crate) fn ospf_converging(&self, now: SimTime) -> bool {
        self.ospf.ifaces.values().any(|state| {
            // A neighbour that has gone quiet is on its way out.
            let grace = (u64::from(state.dead_s.saturating_sub(state.hello_s)) * 1_000_000)
                .saturating_sub(1_500_000);
            if state
                .neighbors
                .values()
                .any(|n| n.dead_at.saturating_sub(now) < grace)
            {
                return true;
            }
            let forming = state.neighbors.values().any(|n| match n.state {
                NbrState::Exchange | NbrState::Loading => true,
                // One-way hellos are either about to be answered or blocked for good.
                NbrState::Init => false,
                // Stuck in EXSTART on an MTU mismatch is a stable (broken) state.
                NbrState::ExStart => n.mtu == state.mtu,
                NbrState::TwoWay => state.role == IfRole::Waiting || state.wants_adjacency(n),
                NbrState::Full => false,
            });
            forming || (state.role == IfRole::Waiting && !state.neighbors.is_empty())
        })
    }

    pub(crate) fn ospf_neighbor_rows(&self, now: SimTime) -> Vec<OspfNeighborRow> {
        let mut rows = Vec::new();
        for (iface, state) in &self.ospf.ifaces {
            for (rid, n) in &state.neighbors {
                let left = n.dead_at.saturating_sub(now);
                rows.push(OspfNeighborRow {
                    neighbor_id: rid.to_string(),
                    priority: n.priority,
                    state: format!("{}/{}", n.state.word(), state.neighbor_role(n)),
                    dead_time_s: left.div_ceil(1_000_000),
                    address: n.addr.to_string(),
                    interface: iface.name(),
                    area: state.area.to_string(),
                });
            }
        }
        rows
    }
}
