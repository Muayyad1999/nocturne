//! The simulated world: nodes, links, the discrete event queue, and the glue
//! that applies each node's effects.
//!
//! The world is `Clone` on purpose. Instant probes (`sim::probe`) run the very
//! same event loop on a scratch copy, so there is exactly one implementation of
//! every forwarding decision.

pub(crate) mod acl;
pub(crate) mod arp;
pub(crate) mod dhcp;
pub(crate) mod edit;
pub(crate) mod faults;
pub(crate) mod flows;
pub(crate) mod frame;
pub(crate) mod fx;
pub(crate) mod health;
pub(crate) mod hsrp;
pub(crate) mod ip;
pub(crate) mod nat;
pub(crate) mod neighbors;
pub(crate) mod node;
pub(crate) mod ospf;
pub(crate) mod port;
pub(crate) mod probe;
pub(crate) mod procs;
pub(crate) mod reconcile;
pub(crate) mod rib;
pub(crate) mod rip;
pub(crate) mod stp;
pub(crate) mod switching;
pub(crate) mod syslog;

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap};

use crate::api::{CableKind, Fault, FaultId, FlowId, LabEvent, LinkId, NodeId, PacketEventKind, SimTime, TraceId};
use crate::ifname::IfId;
use crate::util::rng::{self, Rng};
use frame::Frame;
use fx::{Fx, Note, Timer, Why};
use node::Node;

/// Per-device processing delay, microseconds.
const PROCESSING_MICROS: u64 = 1;
/// Upper bound on events handled by one `run_until` call: a runaway simulation
/// must never hang the host.
const EVENT_BUDGET: u64 = 20_000_000;

#[derive(Clone, Debug)]
pub(crate) struct Link {
    pub(crate) a: (NodeId, IfId),
    pub(crate) b: (NodeId, IfId),
    pub(crate) cable: CableKind,
    pub(crate) up: bool,
    /// Held down by a `LinkDown` fault.
    pub(crate) fault_down: bool,
    /// Currently in the "down" half of a `Flapping` fault.
    pub(crate) flap_down: bool,
    pub(crate) loss_percent: u8,
    pub(crate) duplex_fault: bool,
    /// Flow-driven rates in bits per second, per direction.
    pub(crate) bps_ab: u64,
    pub(crate) bps_ba: u64,
}

impl Link {
    pub(crate) fn other_end(&self, node: NodeId, port: IfId) -> Option<(NodeId, IfId)> {
        if self.a == (node, port) {
            Some(self.b)
        } else if self.b == (node, port) {
            Some(self.a)
        } else {
            None
        }
    }

    fn latency_micros(&self) -> u64 {
        match self.cable {
            CableKind::Fiber => 10,
            _ => 20,
        }
    }
}

/// An out-of-band console cable: stored and shown, never carries frames.
#[derive(Clone, Debug)]
pub(crate) struct ConsoleLink {
    pub(crate) a: (NodeId, String),
    pub(crate) b: (NodeId, String),
}

#[derive(Clone, Debug)]
pub(crate) enum Event {
    Arrive { link: LinkId, node: NodeId, port: IfId, frame: Frame },
    Timer { node: NodeId, epoch: u32, timer: Timer },
    FlowTick,
    Flap { fault: FaultId },
}

#[derive(Clone, Debug)]
struct Queued {
    at: SimTime,
    order: u64,
    event: Event,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        (self.at, self.order) == (other.at, other.order)
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    // Reversed: `BinaryHeap` is a max-heap and the earliest event must pop first.
    fn cmp(&self, other: &Self) -> Ordering {
        (other.at, other.order).cmp(&(self.at, self.order))
    }
}

/// Progress of one traced PDU (and everything it caused).
#[derive(Clone, Debug)]
pub(crate) struct TraceState {
    pub(crate) origin: NodeId,
    pub(crate) request_pdu: u64,
    pub(crate) started: SimTime,
    /// Frames belonging to the trace that are still on a wire.
    pub(crate) inflight: u32,
    pub(crate) ok: bool,
    pub(crate) finished_at: Option<SimTime>,
    pub(crate) done: bool,
    /// `seq` of the event that delivered the request to its destination.
    pub(crate) delivered: Option<u32>,
    /// The most telling drop so far: significance, sequence, node, explanation.
    pub(crate) worst: Option<(Why, u32, NodeId, String)>,
    /// A negative answer the origin received (refused, unreachable).
    pub(crate) verdict: Option<String>,
    pub(crate) what: String,
}

#[derive(Clone, Debug)]
pub(crate) struct World {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) seed: u64,
    pub(crate) rng: Rng,
    pub(crate) now: SimTime,
    pub(crate) nodes: BTreeMap<NodeId, Node>,
    pub(crate) links: BTreeMap<LinkId, Link>,
    pub(crate) console_links: BTreeMap<LinkId, ConsoleLink>,
    pub(crate) next_node: NodeId,
    pub(crate) next_link: LinkId,
    queue: BinaryHeap<Queued>,
    next_order: u64,
    next_seq: u32,
    next_pdu: u64,
    pub(crate) realistic: bool,
    pub(crate) capture: bool,
    pub(crate) events: Vec<LabEvent>,
    /// Nodes whose derived state must be recomputed.
    pub(crate) dirty: BTreeSet<NodeId>,
    /// Nodes with a pending `NodeChanged` event.
    pub(crate) changed: BTreeSet<NodeId>,
    /// Bumped on every topology, configuration or control-plane change; caches
    /// of resolved paths are valid for one generation.
    pub(crate) generation: u64,
    pub(crate) last_cp_change: SimTime,
    pub(crate) ospf_dirty: bool,
    pub(crate) traces: BTreeMap<TraceId, TraceState>,
    pub(crate) next_trace: TraceId,
    open_traces: u32,
    pub(crate) faults: BTreeMap<FaultId, Fault>,
    pub(crate) next_fault: FaultId,
    /// When each `InterfaceErrors` fault last credited its counters.
    pub(crate) fault_clock: BTreeMap<FaultId, SimTime>,
    pub(crate) flows: BTreeMap<FlowId, flows::FlowRuntime>,
    pub(crate) next_flow: FlowId,
    pub(crate) flow_tick_armed: bool,
    /// This world is a scratch copy used by an instant probe.
    pub(crate) scratch: bool,
    /// Ignore random frame loss (path resolution for flows).
    pub(crate) lossless: bool,
    /// Source of unique keys for CLI sessions.
    pub(crate) session_keys: u64,
}

impl World {
    pub(crate) fn new(name: &str, seed: u64) -> World {
        World {
            name: name.to_string(),
            description: String::new(),
            seed,
            rng: Rng::new(seed),
            now: 0,
            nodes: BTreeMap::new(),
            links: BTreeMap::new(),
            console_links: BTreeMap::new(),
            next_node: 1,
            next_link: 1,
            queue: BinaryHeap::new(),
            next_order: 0,
            next_seq: 0,
            next_pdu: 0,
            realistic: false,
            capture: false,
            events: Vec::new(),
            dirty: BTreeSet::new(),
            changed: BTreeSet::new(),
            generation: 0,
            last_cp_change: 0,
            ospf_dirty: false,
            traces: BTreeMap::new(),
            next_trace: 1,
            open_traces: 0,
            faults: BTreeMap::new(),
            next_fault: 1,
            fault_clock: BTreeMap::new(),
            flows: BTreeMap::new(),
            next_flow: 1,
            flow_tick_armed: false,
            scratch: false,
            lossless: false,
            session_keys: 0,
        }
    }

    // ------------------------------------------------------------ effects

    pub(crate) fn fx(&self) -> Fx {
        Fx::new(self.now, self.realistic, self.seed, self.capture, self.next_seq, self.next_pdu)
    }

    /// Run `f` on a node with a fresh effect collector, then apply the effects.
    pub(crate) fn with_node<R>(&mut self, id: NodeId, f: impl FnOnce(&mut Node, &mut Fx) -> R) -> Option<R> {
        let mut fx = self.fx();
        let result = f(self.nodes.get_mut(&id)?, &mut fx);
        self.apply(id, fx);
        Some(result)
    }

    pub(crate) fn apply(&mut self, node_id: NodeId, fx: Fx) {
        self.next_seq = self.next_seq.wrapping_add(fx.notes.len() as u32);
        self.next_pdu += fx.pdus_used;
        for note in fx.notes {
            self.record(note);
        }
        for trace in fx.trace_ok {
            if let Some(state) = self.traces.get_mut(&trace)
                && !state.ok
            {
                state.ok = true;
                state.finished_at = Some(self.now);
            }
        }
        for (trace, text) in fx.trace_verdict {
            if let Some(state) = self.traces.get_mut(&trace) {
                state.verdict.get_or_insert(text);
            }
        }
        if !self.scratch {
            self.events.extend(fx.events);
        }
        let epoch = self.nodes.get(&node_id).map_or(0, |n| n.epoch);
        for (delay, timer) in fx.timers {
            self.schedule(self.now.saturating_add(delay), Event::Timer { node: node_id, epoch, timer });
        }
        for (port, frame) in fx.tx {
            self.transmit(node_id, port, frame);
        }
        if fx.dirty {
            self.dirty.insert(node_id);
        }
        if fx.cp_change {
            self.touch();
        }
        if fx.changed {
            self.changed.insert(node_id);
        }
        if fx.spf {
            self.ospf_dirty = true;
        }
    }

    /// Note a control-plane change: invalidates cached paths and restarts the
    /// quiescence clock used by `settle`.
    pub(crate) fn touch(&mut self) {
        self.generation += 1;
        self.last_cp_change = self.now;
    }

    fn record(&mut self, note: Note) {
        if let Some(trace) = note.event.trace
            && let Some(state) = self.traces.get_mut(&trace)
        {
            match (note.event.kind, note.why) {
                (PacketEventKind::Dropped, Some(why)) => {
                    let better = state.worst.as_ref().is_none_or(|(old, _, _, _)| why >= *old);
                    if better {
                        state.worst = Some((why, note.event.seq, note.event.at, note.event.decision.clone()));
                    }
                }
                (PacketEventKind::Delivered, _) if note.event.pdu == state.request_pdu => {
                    state.delivered = Some(note.event.seq);
                }
                _ => {}
            }
        }
        self.events.push(LabEvent::Packet(note.event));
    }

    pub(crate) fn schedule(&mut self, at: SimTime, event: Event) {
        self.next_order += 1;
        self.queue.push(Queued { at, order: self.next_order, event });
    }

    // ------------------------------------------------------------- the wire

    fn transmit(&mut self, node_id: NodeId, port: IfId, frame: Frame) {
        let now = self.now;
        let Some(node) = self.nodes.get_mut(&node_id) else { return };
        let Some(state) = node.ifs.get_mut(&port) else { return };
        let Some(link_id) = state.link.filter(|_| state.carrier) else {
            state.counters.out_discards += 1;
            return;
        };
        state.counters.out_pkts += 1;
        state.counters.out_octets += frame.wire_len();
        state.counters.last_out = Some(now);
        let sender_full = state.full_duplex;
        let Some(link) = self.links.get(&link_id) else { return };
        let Some((peer, peer_port)) = link.other_end(node_id, port) else { return };
        if !link.up {
            return;
        }
        let dice = rng::hash(&[self.seed, u64::from(link_id), frame.meta.pdu, now]);
        let lost = !self.lossless && link.loss_percent > 0 && dice % 100 < u64::from(link.loss_percent);
        let latency = link.latency_micros();
        let loaded = link.bps_ab + link.bps_ba > 0;
        let receiver_full = self.nodes.get(&peer).and_then(|n| n.ifs.get(&peer_port)).is_some_and(|s| s.full_duplex);
        // A duplex mismatch only hurts under load: while flows keep the link
        // busy, a share of the frames dies in late collisions.
        let collided = !self.lossless && loaded && sender_full != receiver_full && dice >> 8 & 7 == 0;
        if lost || collided {
            if collided {
                self.count_collision(node_id, port, peer, peer_port, sender_full);
            }
            let mut fx = self.fx();
            fx.dropped(node_id, &frame, Some(port), Why::Fatal, || {
                if lost {
                    format!("The frame is lost on the cable (packet-loss fault on the link out of {}).", port.short())
                } else {
                    format!(
                        "The frame is destroyed by a late collision: the two ends of the link out of {} disagree about duplex.",
                        port.short()
                    )
                }
            });
            self.apply(node_id, fx);
            return;
        }
        if let Some(trace) = frame.meta.trace
            && let Some(state) = self.traces.get_mut(&trace)
        {
            state.inflight += 1;
        }
        self.schedule(now + latency + PROCESSING_MICROS, Event::Arrive { link: link_id, node: peer, port: peer_port, frame });
    }

    fn count_collision(&mut self, tx_node: NodeId, tx_port: IfId, rx_node: NodeId, rx_port: IfId, sender_full: bool) {
        // The half-duplex end sees late collisions; the full-duplex end sees
        // damaged frames (CRC errors and runts).
        let (half, full) = if sender_full { ((rx_node, rx_port), (tx_node, tx_port)) } else { ((tx_node, tx_port), (rx_node, rx_port)) };
        if let Some(state) = self.nodes.get_mut(&half.0).and_then(|n| n.ifs.get_mut(&half.1)) {
            state.counters.collisions += 1;
            state.counters.late_collisions += 1;
            state.counters.out_errors += 1;
        }
        if let Some(state) = self.nodes.get_mut(&full.0).and_then(|n| n.ifs.get_mut(&full.1)) {
            state.counters.in_errors += 1;
            state.counters.crc += 1;
        }
    }

    fn arrive(&mut self, link_id: LinkId, node_id: NodeId, port: IfId, mut frame: Frame) {
        if let Some(trace) = frame.meta.trace
            && let Some(state) = self.traces.get_mut(&trace)
        {
            state.inflight = state.inflight.saturating_sub(1);
        }
        let Some(link) = self.links.get(&link_id) else { return };
        if !link.up {
            return;
        }
        let Some((from, _)) = link.other_end(node_id, port) else { return };
        let mut fx = self.fx();
        if fx.watching(&frame) {
            let from_name = self.nodes.get(&from).map(|n| n.name.clone()).unwrap_or_default();
            frame.meta.via = fx.note(node_id, PacketEventKind::Received, &frame, Some(port), None, || {
                format!("The frame arrives on {} from {from_name}.", port.short())
            });
            if let Some(note) = fx.notes.last_mut() {
                note.event.from = Some(from);
                note.event.link = Some(link_id);
            }
        }
        let now = self.now;
        let Some(node) = self.nodes.get_mut(&node_id) else { return };
        if !node.powered {
            return;
        }
        let Some(state) = node.ifs.get_mut(&port) else { return };
        if !state.carrier {
            return;
        }
        state.counters.in_pkts += 1;
        state.counters.in_octets += frame.wire_len();
        state.counters.last_in = Some(now);
        if frame.dst.is_multicast() {
            state.counters.in_broadcasts += 1;
        }
        node.rx(&mut fx, port, frame);
        self.apply(node_id, fx);
    }

    // ------------------------------------------------------------ the clock

    /// Process every event due up to `deadline`, then set the clock to it.
    pub(crate) fn run_until(&mut self, deadline: SimTime) {
        let mut budget = EVENT_BUDGET;
        self.after_event();
        while self.queue.peek().is_some_and(|q| q.at <= deadline) && budget > 0 {
            let Some(queued) = self.queue.pop() else { break };
            budget -= 1;
            self.now = self.now.max(queued.at);
            self.dispatch(queued.event);
            self.after_event();
        }
        self.now = self.now.max(deadline);
        self.account_error_faults();
    }

    /// Time of the next scheduled event, if any.
    pub(crate) fn next_event_at(&self) -> Option<SimTime> {
        self.queue.peek().map(|q| q.at)
    }

    fn dispatch(&mut self, event: Event) {
        match event {
            Event::Arrive { link, node, port, frame } => self.arrive(link, node, port, frame),
            Event::Timer { node, epoch, timer } => {
                let live = self.nodes.get(&node).is_some_and(|n| n.powered && n.epoch == epoch);
                if live {
                    self.with_node(node, |n, fx| n.on_timer(fx, timer));
                }
            }
            Event::FlowTick => flows::tick(self),
            Event::Flap { fault } => faults::flap(self, fault),
        }
    }

    /// Bring derived state up to date and close finished traces.
    pub(crate) fn after_event(&mut self) {
        if !self.dirty.is_empty() || self.ospf_dirty {
            self.reconcile();
        }
        if self.open_traces > 0 {
            self.close_finished_traces();
        }
    }

    // ---------------------------------------------------------------- traces

    pub(crate) fn open_trace(&mut self, origin: NodeId, what: String) -> TraceId {
        let id = self.next_trace;
        self.next_trace += 1;
        self.open_traces += 1;
        self.traces.insert(
            id,
            TraceState {
                origin,
                request_pdu: 0,
                started: self.now,
                inflight: 0,
                ok: false,
                finished_at: None,
                done: false,
                delivered: None,
                worst: None,
                verdict: None,
                what,
            },
        );
        id
    }

    fn close_finished_traces(&mut self) {
        let finished: Vec<TraceId> =
            self.traces.iter().filter(|(_, s)| !s.done && s.inflight == 0).map(|(id, _)| *id).collect();
        for id in finished {
            let Some(state) = self.traces.get_mut(&id) else { continue };
            state.done = true;
            self.open_traces = self.open_traces.saturating_sub(1);
            let summary = trace_summary(state, &self.nodes);
            let ok = state.ok;
            self.events.push(LabEvent::TraceDone { trace: id, ok, summary });
            // Finished traces are only kept around for the probe that asked.
            if !self.scratch {
                self.traces.remove(&id);
            }
        }
    }
}

/// One sentence describing how a trace ended.
pub(crate) fn trace_summary(state: &TraceState, nodes: &BTreeMap<NodeId, Node>) -> String {
    if state.ok {
        let micros = state.finished_at.unwrap_or(state.started).saturating_sub(state.started);
        return format!("{} succeeded in {}.{:03} ms.", state.what, micros / 1000, micros % 1000);
    }
    match (&state.worst, &state.verdict) {
        (Some((why, _, node, text)), _) if *why >= Why::ArpPending => {
            let name = nodes.get(node).map_or("?", |n| n.name.as_str());
            format!("{} failed at {name}: {text}", state.what)
        }
        (_, Some(verdict)) => format!("{} failed: {verdict}", state.what),
        (Some((_, _, node, text)), None) if state.delivered.is_none() => {
            let name = nodes.get(node).map_or("?", |n| n.name.as_str());
            format!("{} failed at {name}: {text}", state.what)
        }
        _ if state.delivered.is_some() => {
            format!("{} failed: the PDU reached its destination but no answer came back.", state.what)
        }
        _ => format!("{} failed: the PDU never reached its destination.", state.what),
    }
}
