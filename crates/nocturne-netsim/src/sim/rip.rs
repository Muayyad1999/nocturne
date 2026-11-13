//! RIP: a distance-vector protocol with real periodic and triggered updates.
//!
//! Version 2 carries masks and multicasts to 224.0.0.9; version 1 broadcasts
//! classful networks and the receiver guesses the mask. Split horizon keeps a
//! router from advertising a route back where it learned it, and a network
//! that disappears is advertised once more with metric 16 so neighbours drop
//! it at once instead of waiting for the 180 s timeout.

use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use crate::api::SimTime;
use crate::device::config::RipConfig;
use crate::ifname::{IfId, IfKind};
use crate::sim::frame::{L4, PORT_RIP, Packet, RipMsg, UdpApp};
use crate::sim::fx::{Fx, Timer};
use crate::sim::node::Node;
use crate::sim::rib::{NextHop, Route, Source};
use crate::util::net::{IfAddr, Prefix};

pub(crate) const UPDATE_S: u64 = 30;
pub(crate) const INVALID_S: u64 = 180;
pub(crate) const FLUSH_S: u64 = 240;
const SECOND: u64 = 1_000_000;
/// How long changes are collected before a triggered update goes out.
const TRIGGER_DELAY: u64 = SECOND;
const INFINITY: u8 = 16;
const MAX_PATHS: usize = 4;
const RIP_V2_GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 9);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RipPath {
    pub(crate) via: Ipv4Addr,
    pub(crate) iface: IfId,
    pub(crate) heard: SimTime,
}

#[derive(Clone, Debug)]
pub(crate) struct RipEntry {
    pub(crate) metric: u8,
    pub(crate) paths: Vec<RipPath>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Rip {
    pub(crate) running: bool,
    /// Interfaces covered by a `network` statement, with their addresses.
    pub(crate) ifaces: BTreeMap<IfId, IfAddr>,
    pub(crate) learned: BTreeMap<Prefix, RipEntry>,
    /// What the last update on each interface announced, to poison what vanished.
    sent: BTreeMap<IfId, BTreeSet<Prefix>>,
    /// Neighbours heard from, and when.
    pub(crate) sources: BTreeMap<Ipv4Addr, SimTime>,
    pub(crate) next_update: SimTime,
    /// The configuration the last announcements were based on.
    announced_with: Option<RipConfig>,
    token: u32,
    triggered: bool,
}

impl Node {
    /// Bring the running RIP state in line with configuration and interfaces.
    pub(crate) fn rip_sync(&mut self, fx: &mut Fx) {
        let Some(cfg) = self.cfg.rip.as_ref().filter(|_| self.routes_packets()) else {
            if self.rip.running {
                self.rip = Rip {
                    token: self.rip.token,
                    ..Rip::default()
                };
                self.rip_install(fx);
            }
            return;
        };
        let mut wanted: BTreeMap<IfId, IfAddr> = BTreeMap::new();
        for id in self.cfg.interfaces.keys() {
            if !self.is_l3(*id) || !self.is_up(*id) {
                continue;
            }
            if let Some(addr) = self
                .addr(*id)
                .filter(|a| cfg.networks.contains(&Prefix::classful(a.ip).net))
            {
                wanted.insert(*id, addr);
            }
        }
        if !self.rip.running {
            self.rip.running = true;
            self.rip.token = self.rip.token.wrapping_add(1);
            self.rip.next_update = fx.now + SECOND;
            // The first update goes out soon; after that every 30 seconds.
            fx.arm(
                SECOND,
                Timer::RipUpdate {
                    token: self.rip.token,
                },
            );
        }
        let reconfigured = self.rip.announced_with.as_ref() != Some(cfg);
        if reconfigured {
            // Versions, summarisation and the like change what is announced
            // and what is accepted: tell the neighbours and ask them again.
            self.rip.announced_with = Some(cfg.clone());
            self.rip_trigger(fx);
        }
        if wanted == self.rip.ifaces {
            if reconfigured {
                let ifaces: Vec<IfId> = self.rip.ifaces.keys().copied().collect();
                for iface in ifaces {
                    self.rip_send(fx, iface, true);
                }
            }
            return;
        }
        let fresh: Vec<IfId> = wanted
            .keys()
            .filter(|id| !self.rip.ifaces.contains_key(id))
            .copied()
            .collect();
        self.rip.ifaces = wanted;
        // Routes learned through an interface that left the process go with it.
        let live = &self.rip.ifaces;
        let before = self.rip.learned.len();
        for entry in self.rip.learned.values_mut() {
            entry
                .paths
                .retain(|p| live.get(&p.iface).is_some_and(|a| a.on_link(p.via)));
        }
        self.rip.learned.retain(|_, entry| !entry.paths.is_empty());
        self.rip.sent.retain(|id, _| live.contains_key(id));
        if before != self.rip.learned.len() {
            self.rip_install(fx);
        }
        // Ask the neighbours on new interfaces for their tables right away.
        for iface in fresh {
            self.rip_send(fx, iface, true);
        }
        self.rip_trigger(fx);
    }

    /// Collect changes for a moment, then tell the neighbours.
    fn rip_trigger(&mut self, fx: &mut Fx) {
        if self.rip.running && !self.rip.triggered {
            self.rip.triggered = true;
            fx.arm(TRIGGER_DELAY, Timer::RipTriggered);
        }
    }

    /// The routing table changed: connected networks may have come or gone.
    pub(crate) fn rip_table_changed(&mut self, fx: &mut Fx) {
        self.rip_trigger(fx);
    }

    pub(crate) fn rip_triggered(&mut self, fx: &mut Fx) {
        if std::mem::take(&mut self.rip.triggered) && self.rip.running {
            self.rip_send_all(fx);
        }
    }

    pub(crate) fn rip_update_timer(&mut self, fx: &mut Fx, token: u32) {
        if !self.rip.running || token != self.rip.token {
            return;
        }
        fx.arm(UPDATE_S * SECOND, Timer::RipUpdate { token });
        self.rip.next_update = fx.now + UPDATE_S * SECOND;
        // Routes that were not refreshed for too long are no longer believed.
        let now = fx.now;
        let before: usize = self.rip.learned.values().map(|e| e.paths.len()).sum();
        for entry in self.rip.learned.values_mut() {
            entry
                .paths
                .retain(|p| now.saturating_sub(p.heard) < INVALID_S * SECOND);
        }
        self.rip.learned.retain(|_, entry| !entry.paths.is_empty());
        self.rip
            .sources
            .retain(|_, heard| now.saturating_sub(*heard) < FLUSH_S * SECOND);
        if before
            != self
                .rip
                .learned
                .values()
                .map(|e| e.paths.len())
                .sum::<usize>()
        {
            self.rip_install(fx);
        }
        self.rip_send_all(fx);
    }

    fn rip_send_all(&mut self, fx: &mut Fx) {
        let ifaces: Vec<IfId> = self.rip.ifaces.keys().copied().collect();
        for iface in ifaces {
            self.rip_send(fx, iface, false);
        }
    }

    /// What an update out of `out` announces: prefix and metric as the receiver will see it.
    fn rip_announce(&self, out: IfId, out_addr: IfAddr) -> BTreeMap<Prefix, u8> {
        let Some(cfg) = self.cfg.rip.as_ref() else {
            return BTreeMap::new();
        };
        let v2 = cfg.version == Some(2);
        let summarize = !v2 || cfg.auto_summary;
        let out_major = Prefix::classful(out_addr.ip);
        let mut table: BTreeMap<Prefix, u8> = BTreeMap::new();
        let mut offer = |prefix: Prefix, metric: u8| {
            let major = Prefix::classful(prefix.net);
            let prefix = if prefix.len == 0 {
                prefix
            } else if summarize && major != out_major && prefix.len > major.len {
                // Across a major-network boundary only the classful network is announced.
                major
            } else if !v2 && major == out_major && prefix.len != out_addr.len {
                // Version 1 carries no mask: other subnet sizes cannot be expressed.
                return;
            } else {
                prefix
            };
            let best = table.entry(prefix).or_insert(metric);
            *best = (*best).min(metric);
        };
        // Split horizon: nothing goes back out of the interface it belongs to.
        for (iface, addr) in self.rip.ifaces.iter().filter(|(iface, _)| **iface != out) {
            let prefix = if iface.kind == IfKind::Loopback && addr.len == 32 {
                Prefix::new(addr.ip, 32)
            } else {
                addr.subnet()
            };
            offer(prefix, 1);
        }
        for (prefix, entry) in &self.rip.learned {
            if entry.paths.iter().all(|p| p.iface != out) {
                offer(*prefix, entry.metric.saturating_add(1).min(INFINITY));
            }
        }
        if cfg.default_originate {
            offer(Prefix::DEFAULT, 1);
        }
        table
    }

    fn rip_send(&mut self, fx: &mut Fx, iface: IfId, request: bool) {
        let Some(cfg) = self.cfg.rip.as_ref() else {
            return;
        };
        let Some(addr) = self.rip.ifaces.get(&iface).copied() else {
            return;
        };
        if cfg.is_passive(iface) || iface.kind == IfKind::Loopback {
            return;
        }
        let version = cfg.version.unwrap_or(1);
        let mut entries: Vec<(Prefix, u8)> = Vec::new();
        if !request {
            let table = self.rip_announce(iface, addr);
            let previous = self.rip.sent.remove(&iface).unwrap_or_default();
            entries.extend(
                previous
                    .iter()
                    .filter(|p| !table.contains_key(p))
                    .map(|p| (*p, INFINITY)),
            );
            entries.extend(table.iter().map(|(p, m)| (*p, *m)));
            self.rip.sent.insert(iface, table.into_keys().collect());
            if entries.is_empty() {
                return;
            }
        }
        let count = entries.len();
        let dst = if version == 2 {
            RIP_V2_GROUP
        } else {
            Ipv4Addr::BROADCAST
        };
        let msg = RipMsg {
            request,
            version,
            entries,
        };
        let packet = Packet {
            src: addr.ip,
            dst,
            ttl: if version == 2 { 1 } else { 2 },
            size: (32 + 20 * count.max(1)).min(532) as u16,
            l4: L4::Udp {
                sport: PORT_RIP,
                dport: PORT_RIP,
                app: UdpApp::Rip(Box::new(msg)),
            },
        };
        let noun = self.noun();
        self.ip_send_on(fx, iface, packet, None, || {
            if request {
                format!("RIP has just started on {}: the {noun} asks its neighbours there for their routing tables.", iface.short())
            } else {
                format!(
                    "The {noun} sends its RIP version {version} update out of {} ({count} route(s)); routes learned on this interface are left out (split horizon).",
                    iface.short()
                )
            }
        });
    }

    pub(crate) fn rip_rx(&mut self, fx: &mut Fx, iface: IfId, src: Ipv4Addr, msg: &RipMsg) {
        let Some(cfg) = self.cfg.rip.as_ref() else {
            return;
        };
        let Some(addr) = self.rip.ifaces.get(&iface).copied() else {
            return;
        };
        // The default is to send version 1 and accept both.
        let accepted = cfg.version.is_none_or(|v| v == msg.version);
        if !self.rip.running || !accepted || !addr.on_link(src) || src == addr.ip {
            return;
        }
        if msg.request {
            self.rip_send(fx, iface, false);
            return;
        }
        let now = fx.now;
        self.rip.sources.insert(src, now);
        let connected: Vec<Prefix> = self
            .rib
            .routes
            .iter()
            .filter(|r| r.source == Source::Connected)
            .map(|r| r.prefix)
            .collect();
        let mut changed = false;
        for (announced, metric) in &msg.entries {
            let prefix = if msg.version == 1 {
                classful_guess(announced.net, addr)
            } else {
                *announced
            };
            if connected.contains(&prefix)
                || self
                    .rib
                    .addrs
                    .iter()
                    .any(|(_, a)| prefix == Prefix::new(a.ip, 32))
            {
                continue;
            }
            let path = RipPath {
                via: src,
                iface,
                heard: now,
            };
            let from_src = |p: &RipPath| p.via == src && p.iface == iface;
            match self.rip.learned.get_mut(&prefix) {
                None if *metric < INFINITY => {
                    self.rip.learned.insert(
                        prefix,
                        RipEntry {
                            metric: *metric,
                            paths: vec![path],
                        },
                    );
                    changed = true;
                }
                None => {}
                Some(entry) if *metric < entry.metric => {
                    *entry = RipEntry {
                        metric: *metric,
                        paths: vec![path],
                    };
                    changed = true;
                }
                Some(entry) if *metric == entry.metric && *metric < INFINITY => {
                    if let Some(known) = entry.paths.iter_mut().find(|p| from_src(p)) {
                        known.heard = now;
                    } else if entry.paths.len() < MAX_PATHS {
                        entry.paths.push(path);
                        changed = true;
                    }
                }
                // Worse news only matters when it comes from the router we use.
                Some(entry) if entry.paths.iter().any(from_src) => {
                    entry.paths.retain(|p| !from_src(p));
                    if entry.paths.is_empty() && *metric < INFINITY {
                        *entry = RipEntry {
                            metric: *metric,
                            paths: vec![path],
                        };
                    }
                    changed = true;
                }
                Some(_) => {}
            }
        }
        self.rip.learned.retain(|_, entry| !entry.paths.is_empty());
        if changed {
            self.rip_install(fx);
            self.rip_trigger(fx);
        } else {
            // `show ip route` shows the time since the last update for each route.
            let ages: BTreeMap<Prefix, SimTime> = self
                .rip
                .learned
                .iter()
                .map(|(p, e)| (*p, newest(e)))
                .collect();
            for route in self
                .rib
                .routes
                .iter_mut()
                .chain(self.rib.rip.iter_mut())
                .filter(|r| r.source == Source::Rip)
            {
                if let Some(heard) = ages.get(&route.prefix) {
                    route.since = *heard;
                }
            }
        }
    }

    /// Hand the learned routes to the routing table.
    fn rip_install(&mut self, fx: &mut Fx) {
        self.rib.rip = self
            .rip
            .learned
            .iter()
            .map(|(prefix, entry)| Route {
                prefix: *prefix,
                source: Source::Rip,
                distance: 120,
                metric: u32::from(entry.metric),
                hops: entry
                    .paths
                    .iter()
                    .map(|p| NextHop {
                        via: Some(p.via),
                        iface: Some(p.iface),
                    })
                    .collect(),
                since: newest(entry),
            })
            .collect();
        self.rebuild_rib(fx);
    }

    /// Is a neighbour overdue, so that routes are about to time out?
    pub(crate) fn rip_converging(&self, now: SimTime) -> bool {
        self.rip.triggered
            || self
                .rip
                .learned
                .values()
                .flat_map(|e| &e.paths)
                .any(|p| now.saturating_sub(p.heard) > (UPDATE_S + 5) * SECOND)
    }
}

fn newest(entry: &RipEntry) -> SimTime {
    entry.paths.iter().map(|p| p.heard).max().unwrap_or(0)
}

/// Version 1 updates carry no mask: a subnet of the receiving interface's own
/// major network is assumed to use that interface's mask, anything else its
/// classful mask.
fn classful_guess(net: Ipv4Addr, on: IfAddr) -> Prefix {
    if net.is_unspecified() {
        return Prefix::DEFAULT;
    }
    let major = Prefix::classful(net);
    if major != Prefix::classful(on.ip) || net == major.net {
        return major;
    }
    let subnet = Prefix::new(net, on.len);
    if subnet.net == net {
        subnet
    } else {
        Prefix::new(net, 32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_1_masks_are_guessed_from_the_receiving_interface() {
        let on = IfAddr::new(Ipv4Addr::new(10, 1, 1, 1), 24);
        let guess = |a, b, c, d| classful_guess(Ipv4Addr::new(a, b, c, d), on).to_string();
        assert_eq!(
            guess(10, 2, 3, 0),
            "10.2.3.0/24",
            "same major network: the interface's mask"
        );
        assert_eq!(
            guess(10, 2, 3, 4),
            "10.2.3.4/32",
            "host bits set: a host route"
        );
        assert_eq!(
            guess(172, 16, 0, 0),
            "172.16.0.0/16",
            "another major network: classful"
        );
        assert_eq!(guess(192, 168, 5, 0), "192.168.5.0/24");
        assert_eq!(guess(0, 0, 0, 0), "0.0.0.0/0");
    }
}
