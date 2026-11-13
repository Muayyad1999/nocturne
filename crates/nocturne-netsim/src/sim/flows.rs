//! Background traffic flows.
//!
//! A flow never emits packets. Once a second the world resolves (and caches,
//! until the topology or configuration changes) the path its traffic would take
//! and adds `rate × dt` to every interface counter along it.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::{FlowPattern, FlowSpec, FlowState, LinkId, NodeId, PduKind, SimTime};
use crate::ifname::IfId;
use crate::sim::probe::PathHop;
use crate::sim::{Event, World};

const TICK_MICROS: u64 = 1_000_000;
/// Share of a flow's rate assumed to come back as return traffic.
const RETURN_SHARE: f64 = 0.10;
const AVERAGE_PACKET_BYTES: f64 = 800.0;
/// Time constant of the "5 minute" interface rates.
const RATE_TAU_SECONDS: f64 = 300.0;

#[derive(Clone, Debug)]
pub(crate) struct FlowRuntime {
    pub(crate) spec: FlowSpec,
    pub(crate) src: NodeId,
    pub(crate) dst: Ipv4Addr,
    pub(crate) added: SimTime,
    last_tick: SimTime,
    /// Generation the cached path was resolved in.
    resolved: Option<u64>,
    path: Vec<PathHop>,
    ok: bool,
    reason: String,
    pub(crate) delivered_octets: u64,
    current_bps: u64,
    /// Fractions of octets and packets not yet credited, forward and return.
    carry: [f64; 4],
}

impl FlowRuntime {
    pub(crate) fn new(spec: FlowSpec, src: NodeId, dst: Ipv4Addr, now: SimTime) -> FlowRuntime {
        FlowRuntime {
            spec,
            src,
            dst,
            added: now,
            last_tick: now,
            resolved: None,
            path: Vec::new(),
            ok: false,
            reason: "not resolved yet".to_string(),
            delivered_octets: 0,
            current_bps: 0,
            carry: [0.0; 4],
        }
    }

    pub(crate) fn state(&self, world: &World) -> FlowState {
        FlowState {
            active: self.spec.enabled && self.ok,
            current_bps: if self.spec.enabled { self.current_bps } else { 0 },
            delivered_octets: self.delivered_octets,
            path: self.path.iter().filter_map(|hop| world.nodes.get(&hop.node).map(|n| n.name.clone())).collect(),
            reason: if !self.spec.enabled {
                "disabled".to_string()
            } else if self.ok {
                String::new()
            } else {
                self.reason.clone()
            },
        }
    }
}

/// Sine of a fraction of a full turn, by Bhaskara's approximation. Plain
/// arithmetic keeps results identical on every platform.
fn sin_turns(turns: f64) -> f64 {
    let t = turns - turns.floor();
    let (u, sign) = if t < 0.5 { (t * 2.0, 1.0) } else { ((t - 0.5) * 2.0, -1.0) };
    let p = u * (1.0 - u);
    sign * 16.0 * p / (5.0 - 4.0 * p)
}

/// Offered rate of a flow `elapsed` microseconds after it was added.
fn rate_at(spec: &FlowSpec, elapsed: SimTime) -> f64 {
    let seconds = elapsed as f64 / 1e6;
    let base = spec.bps as f64;
    match spec.pattern {
        FlowPattern::Constant => base,
        FlowPattern::Sine { period_s } => base * (1.0 + sin_turns(seconds / f64::from(period_s.max(1)))),
        FlowPattern::Burst { on_s, off_s } => {
            let (on, off) = (f64::from(on_s.max(1)), f64::from(off_s));
            if seconds % (on + off) < on { base * (on + off) / on } else { 0.0 }
        }
    }
}

/// Credit `amount` to a counter, carrying the fractional part over.
fn credit(carry: &mut f64, amount: f64) -> u64 {
    let total = *carry + amount;
    let whole = total.floor();
    *carry = total - whole;
    whole as u64
}

impl World {
    /// Make sure the once-a-second flow tick is scheduled.
    pub(crate) fn arm_flow_tick(&mut self) {
        if !self.flow_tick_armed && !self.scratch && !self.flows.is_empty() {
            self.flow_tick_armed = true;
            self.schedule(self.now + TICK_MICROS, Event::FlowTick);
        }
    }
}

pub(crate) fn tick(world: &mut World) {
    world.flow_tick_armed = false;
    if world.scratch || world.flows.is_empty() {
        return;
    }
    let now = world.now;
    let generation = world.generation;

    // Re-resolve paths that predate the last topology or configuration change.
    let stale: Vec<(u32, NodeId, Ipv4Addr)> = world
        .flows
        .iter()
        .filter(|(_, f)| f.spec.enabled && f.resolved != Some(generation))
        .map(|(id, f)| (*id, f.src, f.dst))
        .collect();
    for (id, src, dst) in stale {
        let outcome = world.probe(src, dst, PduKind::Icmp, true);
        if let Some(flow) = world.flows.get_mut(&id) {
            flow.resolved = Some(generation);
            flow.ok = outcome.ok;
            flow.reason = outcome.reason;
            flow.path = outcome.path;
        }
    }

    // Credit counters; collect instantaneous per-interface and per-link rates.
    let mut if_rates: BTreeMap<(NodeId, IfId), (f64, f64)> = BTreeMap::new();
    let mut link_rates: BTreeMap<LinkId, (f64, f64)> = BTreeMap::new();
    let ids: Vec<u32> = world.flows.keys().copied().collect();
    for id in ids {
        let Some(flow) = world.flows.get_mut(&id) else { continue };
        let dt = now.saturating_sub(flow.last_tick) as f64 / 1e6;
        flow.last_tick = now;
        if !flow.spec.enabled {
            flow.current_bps = 0;
            continue;
        }
        let bps = rate_at(&flow.spec, now.saturating_sub(flow.added));
        flow.current_bps = bps as u64;
        let (ok, path) = (flow.ok, flow.path.clone());
        let mut carry = flow.carry;
        // Forward traffic thins out after each lossy link; return traffic is a
        // fixed share of what was delivered and only exists if the path works.
        let mut forward = bps;
        let back = if ok { bps * RETURN_SHARE } else { 0.0 };
        let last = path.len().saturating_sub(1);
        for (index, hop) in path.iter().enumerate() {
            let Some(node) = world.nodes.get_mut(&hop.node) else { break };
            let transit = index > 0 && index < last;
            if transit {
                node.forwarded += (forward * dt / 8.0 / AVERAGE_PACKET_BYTES) as u64;
            }
            if let Some(port) = hop.in_if {
                let rates = if_rates.entry((hop.node, port)).or_default();
                rates.0 += forward;
                rates.1 += back;
                if let Some(state) = node.ifs.get_mut(&port) {
                    let octets = forward * dt / 8.0;
                    state.counters.in_octets += credit(&mut carry[0], octets);
                    state.counters.in_pkts += credit(&mut carry[1], octets / AVERAGE_PACKET_BYTES);
                    state.counters.out_octets += (back * dt / 8.0) as u64;
                    state.counters.out_pkts += (back * dt / 8.0 / AVERAGE_PACKET_BYTES) as u64;
                    // A broken path stops here: the last interface reached discards.
                    if !ok && index == last {
                        state.counters.in_discards += (octets / AVERAGE_PACKET_BYTES).ceil() as u64;
                    }
                }
            }
            let Some(port) = hop.out_if else { continue };
            let Some(state) = node.ifs.get_mut(&port) else { continue };
            let octets = forward * dt / 8.0;
            state.counters.out_octets += credit(&mut carry[2], octets);
            state.counters.out_pkts += credit(&mut carry[3], octets / AVERAGE_PACKET_BYTES);
            state.counters.in_octets += (back * dt / 8.0) as u64;
            state.counters.in_pkts += (back * dt / 8.0 / AVERAGE_PACKET_BYTES) as u64;
            let rates = if_rates.entry((hop.node, port)).or_default();
            rates.0 += back;
            rates.1 += forward;
            let full = state.full_duplex;
            let link_id = state.link;
            if let Some(link_id) = link_id
                && let Some(link) = world.links.get(&link_id)
            {
                let a_to_b = link.a == (hop.node, port);
                let entry = link_rates.entry(link_id).or_default();
                if a_to_b {
                    entry.0 += forward;
                    entry.1 += back;
                } else {
                    entry.1 += forward;
                    entry.0 += back;
                }
                let peer = link.other_end(hop.node, port);
                let loss = f64::from(link.loss_percent) / 100.0;
                // Duplex mismatch: late collisions on the half-duplex end,
                // CRC errors on the full-duplex end, roughly 1 % of the load.
                let peer_full = peer.and_then(|(n, p)| world.nodes.get(&n).and_then(|n| n.ifs.get(&p))).map(|s| s.full_duplex);
                if let (Some((peer_node, peer_port)), Some(peer_full)) = (peer, peer_full)
                    && peer_full != full
                {
                    let damaged = (forward * dt / 8.0 / AVERAGE_PACKET_BYTES / 100.0).ceil() as u64;
                    let (half_end, full_end) =
                        if full { ((peer_node, peer_port), (hop.node, port)) } else { ((hop.node, port), (peer_node, peer_port)) };
                    if let Some(s) = world.nodes.get_mut(&half_end.0).and_then(|n| n.ifs.get_mut(&half_end.1)) {
                        s.counters.late_collisions += damaged;
                        s.counters.collisions += damaged;
                        s.counters.out_errors += damaged;
                    }
                    if let Some(s) = world.nodes.get_mut(&full_end.0).and_then(|n| n.ifs.get_mut(&full_end.1)) {
                        s.counters.crc += damaged;
                        s.counters.in_errors += damaged;
                    }
                }
                forward *= 1.0 - loss;
            }
        }
        if let Some(flow) = world.flows.get_mut(&id) {
            flow.carry = carry;
            if ok {
                flow.delivered_octets += (forward * dt / 8.0) as u64;
            }
        }
    }

    for (id, link) in &mut world.links {
        let (ab, ba) = link_rates.get(id).copied().unwrap_or_default();
        link.bps_ab = ab as u64;
        link.bps_ba = ba as u64;
    }
    // Exponentially smoothed interface rates, like the "5 minute" averages.
    let alpha = (TICK_MICROS as f64 / 1e6) / RATE_TAU_SECONDS;
    for (node_id, node) in &mut world.nodes {
        for (port, state) in &mut node.ifs {
            let (rate_in, rate_out) = if_rates.get(&(*node_id, *port)).copied().unwrap_or_default();
            state.rate_in += (rate_in - state.rate_in) * alpha;
            state.rate_out += (rate_out - state.rate_out) * alpha;
            state.load_in = rate_in;
            state.load_out = rate_out;
        }
    }
    world.arm_flow_tick();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_average_to_the_configured_rate() {
        let mut spec = FlowSpec { bps: 1_000_000, ..FlowSpec::default() };
        assert_eq!(rate_at(&spec, 5_000_000), 1e6);
        spec.pattern = FlowPattern::Sine { period_s: 60 };
        let mean: f64 = (0..60).map(|s| rate_at(&spec, s * 1_000_000)).sum::<f64>() / 60.0;
        assert!((mean - 1e6).abs() < 2e4, "{mean}");
        assert!(rate_at(&spec, 15_000_000) > 1.9e6 && rate_at(&spec, 45_000_000) < 0.1e6);
        spec.pattern = FlowPattern::Burst { on_s: 2, off_s: 8 };
        assert_eq!(rate_at(&spec, 1_000_000), 5e6);
        assert_eq!(rate_at(&spec, 5_000_000), 0.0);
        let mut carry = 0.0;
        let total: u64 = (0..10).map(|_| credit(&mut carry, 0.25)).sum();
        assert_eq!(total, 2);
    }
}
