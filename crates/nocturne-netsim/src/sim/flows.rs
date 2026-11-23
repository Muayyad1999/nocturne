//! Background traffic flows.
//!
//! A flow never emits packets. Once a second the world resolves (and caches,
//! until the topology or configuration changes) the path its traffic would take
//! and adds `rate × dt` to every interface counter along it. On the way the
//! traffic meets what would hold real traffic back: the speed of each
//! interface it leaves through, and the service policies attached to the
//! interfaces (`sim::qos`). What one of them drops is not offered further on.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::{FlowId, FlowPattern, FlowSpec, FlowState, LinkId, NodeId, PduKind, SimTime};
use crate::ifname::IfId;
use crate::sim::probe::PathHop;
use crate::sim::{Event, World, qos};

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
            current_bps: if self.spec.enabled {
                self.current_bps
            } else {
                0
            },
            delivered_octets: self.delivered_octets,
            path: self
                .path
                .iter()
                .filter_map(|hop| world.nodes.get(&hop.node).map(|n| n.name.clone()))
                .collect(),
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
    let (u, sign) = if t < 0.5 {
        (t * 2.0, 1.0)
    } else {
        ((t - 0.5) * 2.0, -1.0)
    };
    let p = u * (1.0 - u);
    sign * 16.0 * p / (5.0 - 4.0 * p)
}

/// Offered rate of a flow `elapsed` microseconds after it was added.
fn rate_at(spec: &FlowSpec, elapsed: SimTime) -> f64 {
    let seconds = elapsed as f64 / 1e6;
    let base = spec.bps as f64;
    match spec.pattern {
        FlowPattern::Constant => base,
        FlowPattern::Sine { period_s } => {
            base * (1.0 + sin_turns(seconds / f64::from(period_s.max(1))))
        }
        FlowPattern::Burst { on_s, off_s } => {
            let (on, off) = (f64::from(on_s.max(1)), f64::from(off_s));
            if seconds % (on + off) < on {
                base * (on + off) / on
            } else {
                0.0
            }
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
    /// Make sure the once-a-second flow tick is scheduled. It runs while
    /// there are flows, and after the last one is gone until nothing shows
    /// their traffic any more: a link must not go on reporting a rate, nor a
    /// queue stand full, for traffic that has stopped.
    pub(crate) fn arm_flow_tick(&mut self) {
        let wanted = !self.flows.is_empty() || self.traffic_lingers;
        if !self.flow_tick_armed && !self.scratch && wanted {
            self.flow_tick_armed = true;
            self.schedule(self.now + TICK_MICROS, Event::FlowTick);
        }
    }
}

/// One side of a hop as a flow's traffic meets it: arriving on an interface
/// or leaving through one.
struct Stage {
    node: NodeId,
    port: IfId,
    /// Leaving through the interface (`false`: arriving on it).
    output: bool,
    /// Index of the hop in the flow's path.
    hop: usize,
    /// The class the traffic falls into at each level of the policy attached
    /// here; empty when there is none.
    class: Vec<usize>,
    /// Bits per second arriving at the stage.
    rate: f64,
    /// Share of it that gets through the policy and onto the wire.
    pass: f64,
    /// Share lost on the cable behind an output stage (a `PacketLoss` fault).
    loss: f64,
}

/// A flow's traffic for one tick: out along the path, then the return
/// traffic back along it.
struct Chain {
    flow: FlowId,
    /// Offered rate right now, bits per second.
    bps: f64,
    /// Seconds since the flow was last credited.
    dt: f64,
    ok: bool,
    last_hop: usize,
    stages: Vec<Stage>,
    /// Index of the first stage of the return traffic.
    turn: usize,
}

/// A place where traffic can be held back: an interface and a direction.
type Point = (NodeId, IfId, bool);

/// Upper bound on the rounds spent agreeing on rates. Policies on a loop-free
/// path settle in as many rounds as the path has stages; the bound is for
/// paths that cross each other in a circle.
const MAX_ROUNDS: usize = 64;

/// Carry each chain's rate from stage to stage.
fn propagate(chains: &mut [Chain]) {
    for chain in chains {
        let mut rate = chain.bps;
        // Share of the offered traffic the policies on the way out let through.
        let mut kept = 1.0;
        let turn = chain.turn;
        for (index, stage) in chain.stages.iter_mut().enumerate() {
            if index == turn {
                // Return traffic answers what arrived; random loss on a
                // cable does not thin it (it never did).
                rate = chain.bps * RETURN_SHARE * kept;
            }
            stage.rate = rate;
            rate *= stage.pass;
            if index < turn {
                kept *= stage.pass;
                rate *= 1.0 - stage.loss;
            }
        }
    }
}

/// What the policy (or, without one, the single output queue) at a point lets through.
fn hold_back(
    world: &World,
    plans: &BTreeMap<Point, Option<qos::Level>>,
    point: Point,
    visits: &[(usize, usize)],
    chains: &[Chain],
) -> qos::Outcome {
    let (node_id, port, output) = point;
    let members: Vec<qos::Member> = visits
        .iter()
        .map(|(chain, stage)| {
            let chain = &chains[*chain];
            let stage = &chain.stages[*stage];
            qos::Member {
                rate: stage.rate,
                octets: stage.rate * chain.dt / 8.0,
                path: stage.class.clone(),
            }
        })
        .collect();
    let node = world.nodes.get(&node_id);
    let speed = node
        .and_then(|n| n.ifs.get(&port))
        .map_or(0.0, |state| f64::from(state.speed) * 1e6);
    let capacity = (output && speed > 0.0).then_some(speed);
    match plans.get(&point).and_then(Option::as_ref) {
        Some(level) => {
            // Percentages are of the interface's `bandwidth`, as on a real
            // device; what it can carry is its speed.
            let reference = node
                .and_then(|n| n.cfg.iface(port))
                .and_then(|cfg| cfg.bandwidth)
                .map_or(speed, |kbps| f64::from(kbps) * 1000.0);
            qos::evaluate(level, &members, output, capacity, reference)
        }
        None => match capacity {
            Some(capacity) => qos::fifo(&members, capacity),
            None => qos::Outcome {
                pass: vec![1.0; members.len()],
                ..qos::Outcome::default()
            },
        },
    }
}

pub(crate) fn tick(world: &mut World) {
    world.flow_tick_armed = false;
    if world.scratch || (world.flows.is_empty() && !world.traffic_lingers) {
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

    // What each flow offers in this tick.
    struct Offer {
        flow: FlowId,
        bps: f64,
        dt: f64,
        ok: bool,
        path: Vec<PathHop>,
        src: NodeId,
        dst: Ipv4Addr,
        dscp: Option<u8>,
    }
    let mut offers: Vec<Offer> = Vec::new();
    for (id, flow) in &mut world.flows {
        let dt = now.saturating_sub(flow.last_tick) as f64 / 1e6;
        flow.last_tick = now;
        if !flow.spec.enabled {
            flow.current_bps = 0;
            continue;
        }
        let bps = rate_at(&flow.spec, now.saturating_sub(flow.added));
        flow.current_bps = bps as u64;
        offers.push(Offer {
            flow: *id,
            bps,
            dt,
            ok: flow.ok,
            path: flow.path.clone(),
            src: flow.src,
            dst: flow.dst,
            dscp: flow.spec.dscp,
        });
    }

    // Lay each flow out as a chain of stages, and sort it into the classes of
    // the policies it meets. Marking by `set dscp` travels with the traffic.
    let mut plans: BTreeMap<Point, Option<qos::Level>> = BTreeMap::new();
    let mut chains: Vec<Chain> = Vec::new();
    for offer in offers {
        // The address the traffic leaves its source with (translation on the
        // way is not followed).
        let src_ip = offer
            .path
            .first()
            .and_then(|hop| Some((world.nodes.get(&offer.src)?, hop.out_if?)))
            .and_then(|(node, port)| node.addr(port))
            .map(|addr| addr.ip);
        let mut stages: Vec<Stage> = Vec::new();
        let mut stage =
            |traffic: &mut qos::Traffic, node_id: NodeId, port: IfId, output: bool, hop: usize| {
                let point = (node_id, port, output);
                let node = world.nodes.get(&node_id);
                let level = plans.entry(point).or_insert_with(|| {
                    let node = node?;
                    let policy = node.cfg.service_policy(port, output)?;
                    qos::plan(&node.cfg, &policy.name)
                });
                let class = match (level, node) {
                    (Some(level), Some(node)) => {
                        let (path, dscp) = qos::classify(&node.cfg, level, traffic);
                        traffic.dscp = dscp;
                        path
                    }
                    _ => Vec::new(),
                };
                let loss = if output {
                    node.and_then(|n| n.ifs.get(&port))
                        .and_then(|state| state.link)
                        .and_then(|link| world.links.get(&link))
                        .map_or(0.0, |link| f64::from(link.loss_percent) / 100.0)
                } else {
                    0.0
                };
                Stage {
                    node: node_id,
                    port,
                    output,
                    hop,
                    class,
                    rate: 0.0,
                    pass: 1.0,
                    loss,
                }
            };
        let mut traffic = qos::Traffic {
            dscp: offer.dscp,
            src: src_ip,
            dst: Some(offer.dst),
        };
        for (hop, at) in offer.path.iter().enumerate() {
            if let Some(port) = at.in_if {
                stages.push(stage(&mut traffic, at.node, port, false, hop));
            }
            if let Some(port) = at.out_if {
                stages.push(stage(&mut traffic, at.node, port, true, hop));
            }
        }
        let turn = stages.len();
        if offer.ok {
            // The answers carry the flow's own marking back.
            let mut traffic = qos::Traffic {
                dscp: offer.dscp,
                src: Some(offer.dst),
                dst: src_ip,
            };
            for (hop, at) in offer.path.iter().enumerate().rev() {
                if let Some(port) = at.out_if {
                    stages.push(stage(&mut traffic, at.node, port, false, hop));
                }
                if let Some(port) = at.in_if {
                    stages.push(stage(&mut traffic, at.node, port, true, hop));
                }
            }
        }
        chains.push(Chain {
            flow: offer.flow,
            bps: offer.bps,
            dt: offer.dt,
            ok: offer.ok,
            last_hop: offer.path.len().saturating_sub(1),
            stages,
            turn,
        });
    }

    // Traffic can be held back where it leaves an interface (the interface
    // has a speed) and where a policy is attached.
    let mut points: BTreeMap<Point, Vec<(usize, usize)>> = BTreeMap::new();
    for (c, chain) in chains.iter().enumerate() {
        for (s, stage) in chain.stages.iter().enumerate() {
            if stage.output || !stage.class.is_empty() {
                points
                    .entry((stage.node, stage.port, stage.output))
                    .or_default()
                    .push((c, s));
            }
        }
    }
    // What one point drops, the points behind it are no longer offered: go
    // round until the rates stop moving.
    let rounds = chains.iter().map(|c| c.stages.len()).max().unwrap_or(0) + 1;
    for _ in 0..rounds.min(MAX_ROUNDS) {
        propagate(&mut chains);
        let mut moved = false;
        for (point, visits) in &points {
            let outcome = hold_back(world, &plans, *point, visits, &chains);
            for ((c, s), pass) in visits.iter().zip(outcome.pass) {
                let stage = &mut chains[*c].stages[*s];
                if stage.pass != pass {
                    stage.pass = pass;
                    moved = true;
                }
            }
        }
        if !moved {
            break;
        }
    }
    propagate(&mut chains);
    // The settled rates are what gets counted.
    let mut class_ticks: BTreeMap<
        NodeId,
        BTreeMap<(IfId, bool), BTreeMap<Vec<String>, qos::ClassTick>>,
    > = BTreeMap::new();
    let mut queue_drops: Vec<(NodeId, IfId, f64)> = Vec::new();
    for (point, visits) in &points {
        let outcome = hold_back(world, &plans, *point, visits, &chains);
        for ((c, s), pass) in visits.iter().zip(&outcome.pass) {
            chains[*c].stages[*s].pass = *pass;
        }
        let (node, port, output) = *point;
        if outcome.queue_drop_octets > 0.0 {
            queue_drops.push((node, port, outcome.queue_drop_octets));
        }
        if plans.get(point).is_some_and(Option::is_some) {
            class_ticks
                .entry(node)
                .or_default()
                .insert((port, output), outcome.classes);
        }
    }

    // Credit counters; collect instantaneous per-interface and per-link rates.
    let mut if_rates: BTreeMap<(NodeId, IfId), (f64, f64)> = BTreeMap::new();
    let mut link_rates: BTreeMap<LinkId, (f64, f64)> = BTreeMap::new();
    for chain in &chains {
        let dt = chain.dt;
        let mut carry = world.flows.get(&chain.flow).map_or([0.0; 4], |f| f.carry);
        let mut delivered = if chain.stages.is_empty() {
            chain.bps
        } else {
            0.0
        };
        for (index, stage) in chain.stages.iter().enumerate() {
            let back = index >= chain.turn;
            let arriving = stage.rate;
            let sent = stage.rate * stage.pass;
            let Some(node) = world.nodes.get_mut(&stage.node) else {
                break;
            };
            if !back {
                delivered = sent * (1.0 - stage.loss);
            }
            match (back, stage.output) {
                // Forward traffic arriving.
                (false, false) => {
                    if stage.hop > 0 && stage.hop < chain.last_hop {
                        node.forwarded += (arriving * dt / 8.0 / AVERAGE_PACKET_BYTES) as u64;
                    }
                    if_rates.entry((stage.node, stage.port)).or_default().0 += arriving;
                    if let Some(state) = node.ifs.get_mut(&stage.port) {
                        let octets = arriving * dt / 8.0;
                        state.counters.in_octets += credit(&mut carry[0], octets);
                        state.counters.in_pkts +=
                            credit(&mut carry[1], octets / AVERAGE_PACKET_BYTES);
                        // A broken path stops here: the last interface reached discards.
                        if !chain.ok && stage.hop == chain.last_hop {
                            state.counters.in_discards +=
                                (octets / AVERAGE_PACKET_BYTES).ceil() as u64;
                        }
                    }
                }
                // Forward traffic leaving.
                (false, true) => {
                    let Some(state) = node.ifs.get_mut(&stage.port) else {
                        continue;
                    };
                    let octets = sent * dt / 8.0;
                    state.counters.out_octets += credit(&mut carry[2], octets);
                    state.counters.out_pkts += credit(&mut carry[3], octets / AVERAGE_PACKET_BYTES);
                    if_rates.entry((stage.node, stage.port)).or_default().1 += sent;
                    let full = state.full_duplex;
                    let link_id = state.link;
                    if let Some(link_id) = link_id
                        && let Some(link) = world.links.get(&link_id)
                    {
                        let entry = link_rates.entry(link_id).or_default();
                        if link.a == (stage.node, stage.port) {
                            entry.0 += sent;
                        } else {
                            entry.1 += sent;
                        }
                        let peer = link.other_end(stage.node, stage.port);
                        // Duplex mismatch: late collisions on the half-duplex end,
                        // CRC errors on the full-duplex end, roughly 1 % of the load.
                        let peer_full = peer
                            .and_then(|(n, p)| world.nodes.get(&n).and_then(|n| n.ifs.get(&p)))
                            .map(|s| s.full_duplex);
                        if let (Some((peer_node, peer_port)), Some(peer_full)) = (peer, peer_full)
                            && peer_full != full
                        {
                            let damaged =
                                (sent * dt / 8.0 / AVERAGE_PACKET_BYTES / 100.0).ceil() as u64;
                            let (half_end, full_end) = if full {
                                ((peer_node, peer_port), (stage.node, stage.port))
                            } else {
                                ((stage.node, stage.port), (peer_node, peer_port))
                            };
                            if let Some(s) = world
                                .nodes
                                .get_mut(&half_end.0)
                                .and_then(|n| n.ifs.get_mut(&half_end.1))
                            {
                                s.counters.late_collisions += damaged;
                                s.counters.collisions += damaged;
                                s.counters.out_errors += damaged;
                            }
                            if let Some(s) = world
                                .nodes
                                .get_mut(&full_end.0)
                                .and_then(|n| n.ifs.get_mut(&full_end.1))
                            {
                                s.counters.crc += damaged;
                                s.counters.in_errors += damaged;
                            }
                        }
                    }
                }
                // Return traffic arriving where the forward traffic left.
                (true, false) => {
                    if_rates.entry((stage.node, stage.port)).or_default().0 += arriving;
                    let Some(state) = node.ifs.get_mut(&stage.port) else {
                        continue;
                    };
                    state.counters.in_octets += (arriving * dt / 8.0) as u64;
                    state.counters.in_pkts += (arriving * dt / 8.0 / AVERAGE_PACKET_BYTES) as u64;
                    if let Some(link_id) = state.link
                        && let Some(link) = world.links.get(&link_id)
                    {
                        let entry = link_rates.entry(link_id).or_default();
                        if link.a == (stage.node, stage.port) {
                            entry.1 += arriving;
                        } else {
                            entry.0 += arriving;
                        }
                    }
                }
                // Return traffic leaving where the forward traffic arrived.
                (true, true) => {
                    if_rates.entry((stage.node, stage.port)).or_default().1 += sent;
                    if let Some(state) = node.ifs.get_mut(&stage.port) {
                        state.counters.out_octets += (sent * dt / 8.0) as u64;
                        state.counters.out_pkts += (sent * dt / 8.0 / AVERAGE_PACKET_BYTES) as u64;
                    }
                }
            }
        }
        if let Some(flow) = world.flows.get_mut(&chain.flow) {
            flow.carry = carry;
            if chain.ok {
                // A flow to the node's own address crosses no port, so no
                // port's speed bounds it: whatever rate was asked for must
                // not wrap the counter.
                flow.delivered_octets = flow
                    .delivered_octets
                    .saturating_add((delivered * dt / 8.0 + 1e-6) as u64);
            }
        }
    }
    // What a queue dropped shows as output drops on its interface; what a
    // policer dropped does not, as on real devices.
    for (node, port, octets) in queue_drops {
        if let Some(state) = world
            .nodes
            .get_mut(&node)
            .and_then(|n| n.ifs.get_mut(&port))
        {
            let mut packets = credit(&mut state.out_drop_carry, octets / AVERAGE_PACKET_BYTES);
            // A share of a rate is a product, and a whole packet on paper
            // can come out a hair short: do not let it wait a tick for that.
            if state.out_drop_carry > 1.0 - 1e-6 {
                packets += 1;
                state.out_drop_carry -= 1.0;
            }
            state.counters.out_discards += packets;
        }
    }

    for (id, link) in &mut world.links {
        let (ab, ba) = link_rates.get(id).copied().unwrap_or_default();
        // Rounded, not cut: a thinned rate is a product and may sit a hair
        // under the whole number it stands for.
        link.bps_ab = ab.round() as u64;
        link.bps_ba = ba.round() as u64;
    }
    // Exponentially smoothed interface rates, like the "5 minute" averages.
    let alpha = (TICK_MICROS as f64 / 1e6) / RATE_TAU_SECONDS;
    let idle = BTreeMap::new();
    for (node_id, node) in &mut world.nodes {
        for (port, state) in &mut node.ifs {
            let (rate_in, rate_out) = if_rates
                .get(&(*node_id, *port))
                .copied()
                .unwrap_or_default();
            state.rate_in += (rate_in - state.rate_in) * alpha;
            state.rate_out += (rate_out - state.rate_out) * alpha;
            state.load_in = rate_in;
            state.load_out = rate_out;
        }
        node.qos_account(class_ticks.get(node_id).unwrap_or(&idle), alpha);
    }
    // Below one bit per second an average prints as nothing.
    world.traffic_lingers = world
        .links
        .values()
        .any(|link| link.bps_ab > 0 || link.bps_ba > 0)
        || world.nodes.values().any(|node| {
            node.ifs
                .values()
                .any(|state| state.rate_in >= 1.0 || state.rate_out >= 1.0)
                || node.qos_lingers()
        });
    world.arm_flow_tick();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_average_to_the_configured_rate() {
        let mut spec = FlowSpec {
            bps: 1_000_000,
            ..FlowSpec::default()
        };
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
