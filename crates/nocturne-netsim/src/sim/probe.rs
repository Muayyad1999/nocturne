//! Traces and instant probes.
//!
//! A trace injects one PDU and follows it (and everything it causes) through
//! the event queue. An instant probe is the same thing run on a scratch copy of
//! the world, retried while the only obstacle is ARP still resolving, so a
//! probe can never disagree with what the timed simulation would do.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::{LabError, LabEvent, NodeId, PacketEvent, PacketEventKind, PduKind, ProbeHop, ProbeResult, TraceId};
use crate::ifname::IfId;
use crate::sim::World;
use crate::sim::frame::{L4, Packet, UdpApp};
use crate::sim::fx::Why;
use crate::sim::procs::{PingSpec, Style};

/// ARP has to resolve hop by hop, each attempt getting one hop further.
const MAX_ATTEMPTS: usize = 24;
/// Simulated time allowed per attempt.
const ATTEMPT_WINDOW_MICROS: u64 = 500_000;

/// One node on a resolved path.
#[derive(Clone, Debug)]
pub(crate) struct PathHop {
    pub(crate) node: NodeId,
    pub(crate) in_if: Option<IfId>,
    pub(crate) out_if: Option<IfId>,
    pub(crate) note: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProbeOutcome {
    pub(crate) ok: bool,
    pub(crate) rtt_micros: u64,
    pub(crate) path: Vec<PathHop>,
    pub(crate) reason: String,
}

impl ProbeOutcome {
    fn failed(reason: String) -> ProbeOutcome {
        ProbeOutcome { ok: false, rtt_micros: 0, path: Vec::new(), reason }
    }

    pub(crate) fn into_result(self, world: &World) -> ProbeResult {
        ProbeResult {
            ok: self.ok,
            rtt_us: self.rtt_micros,
            hops: self
                .path
                .into_iter()
                .map(|hop| ProbeHop {
                    node: hop.node,
                    name: world.nodes.get(&hop.node).map(|n| n.name.clone()).unwrap_or_default(),
                    in_if: hop.in_if.map(|i| i.name()).unwrap_or_default(),
                    out_if: hop.out_if.map(|i| i.name()).unwrap_or_default(),
                    note: hop.note,
                })
                .collect(),
            reason: self.reason,
        }
    }
}

impl World {
    /// Inject one PDU from `from` towards `dst` and follow it as a trace.
    pub(crate) fn start_trace(&mut self, from: NodeId, dst: Ipv4Addr, kind: PduKind) -> Result<TraceId, LabError> {
        let node = self.nodes.get(&from).ok_or(LabError::UnknownNode(from))?;
        if !node.powered {
            return Err(LabError::PoweredOff(node.name.clone()));
        }
        let what = match kind {
            PduKind::Icmp => format!("Ping from {} to {dst}", node.name),
            PduKind::Tcp { port } => format!("TCP connection from {} to {dst} port {port}", node.name),
            PduKind::Udp { port } => format!("UDP datagram from {} to {dst} port {port}", node.name),
        };
        let trace = self.open_trace(from, what);
        // The first PDU the origin creates from here on is the traced request.
        let request_pdu = self.fx().new_pdu();
        if let Some(state) = self.traces.get_mut(&trace) {
            state.request_pdu = request_pdu;
        }
        self.with_node(from, |node, fx| match kind {
            PduKind::Icmp => {
                let spec = PingSpec {
                    dst,
                    src: None,
                    count: 1,
                    timeout_micros: 2_000_000,
                    size: 100,
                    style: Style::Ios,
                    label: None,
                };
                node.start_ping(fx, spec, Some(trace));
            }
            PduKind::Tcp { port } => {
                node.start_connect(fx, dst, port, Style::Ios, Some(trace));
            }
            PduKind::Udp { port } => {
                let sport = 49152 + node.fresh_ident() % 16000;
                let packet = Packet {
                    src: Ipv4Addr::UNSPECIFIED,
                    dst,
                    ttl: node.default_ttl(),
                    size: 60,
                    l4: L4::Udp { sport, dport: port, app: UdpApp::Data },
                };
                node.ip_send(fx, packet, Some(trace), None);
            }
        });
        self.after_event();
        Ok(trace)
    }

    /// Resolve reachability instantly on a scratch copy of the world.
    pub(crate) fn probe(&self, from: NodeId, dst: Ipv4Addr, kind: PduKind, lossless: bool) -> ProbeOutcome {
        let mut scratch = self.clone();
        scratch.scratch = true;
        scratch.capture = false;
        scratch.flows.clear();
        scratch.events.clear();
        scratch.lossless = lossless;
        let mut outcome = ProbeOutcome::failed("the probe could not be sent".to_string());
        for _ in 0..MAX_ATTEMPTS {
            scratch.events.clear();
            let trace = match scratch.start_trace(from, dst, kind) {
                Ok(trace) => trace,
                Err(error) => return ProbeOutcome::failed(error.to_string()),
            };
            let deadline = scratch.now + ATTEMPT_WINDOW_MICROS;
            while scratch.traces.get(&trace).is_some_and(|t| !t.done) {
                match scratch.next_event_at() {
                    Some(at) if at <= deadline => scratch.run_until(at),
                    _ => break,
                }
            }
            let Some(state) = scratch.traces.remove(&trace) else { break };
            let events: BTreeMap<u32, &PacketEvent> = scratch
                .events
                .iter()
                .filter_map(|e| match e {
                    LabEvent::Packet(p) if p.trace == Some(trace) => Some((p.seq, p)),
                    _ => None,
                })
                .collect();
            if state.ok {
                let rtt = state.finished_at.unwrap_or(state.started).saturating_sub(state.started);
                let path = state.delivered.map(|seq| scratch.path_to(&events, seq)).unwrap_or_default();
                return ProbeOutcome { ok: true, rtt_micros: rtt, path, reason: String::new() };
            }
            let retry = matches!(state.worst, Some((Why::ArpPending, ..)));
            let path = match (&state.worst, state.delivered) {
                (Some((_, seq, _, _)), None) => scratch.path_to(&events, *seq),
                (_, Some(seq)) => scratch.path_to(&events, seq),
                _ => Vec::new(),
            };
            outcome = ProbeOutcome { ok: false, rtt_micros: 0, path, reason: super::trace_summary(&state, &scratch.nodes) };
            scratch.end_trace_procs(trace);
            if !retry {
                break;
            }
        }
        outcome
    }

    /// Remove the processes a finished trace left behind on its origin.
    pub(crate) fn end_trace_procs(&mut self, trace: TraceId) {
        for node in self.nodes.values_mut() {
            node.procs.list.retain(|_, proc| proc.trace() != Some(trace));
        }
    }

    /// Rebuild the hop list leading to event `seq` by following parent links.
    fn path_to(&self, events: &BTreeMap<u32, &PacketEvent>, seq: u32) -> Vec<PathHop> {
        let mut chain: Vec<&PacketEvent> = Vec::new();
        let mut cursor = seq;
        while let Some(event) = events.get(&cursor) {
            chain.push(event);
            if event.parent == 0 || chain.len() > 4096 {
                break;
            }
            cursor = event.parent;
        }
        chain.reverse();
        let mut hops: Vec<PathHop> = Vec::new();
        for event in chain {
            if hops.last().is_none_or(|hop| hop.node != event.at) {
                hops.push(PathHop { node: event.at, in_if: None, out_if: None, note: String::new() });
            }
            let index = hops.len() - 1;
            if event.kind == PacketEventKind::Received {
                hops[index].in_if = IfId::parse(&event.interface);
                // The link the frame crossed tells us which port the previous node used.
                if index > 0
                    && let Some(link) = event.link.and_then(|id| self.links.get(&id))
                    && let Some(in_if) = hops[index].in_if
                {
                    hops[index - 1].out_if = link.other_end(event.at, in_if).map(|(_, port)| port);
                }
            } else {
                hops[index].note = event.decision.clone();
            }
        }
        hops
    }
}
