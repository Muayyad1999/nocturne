//! Effects: what a node asks the world to do on its behalf.
//!
//! Node logic never touches the event queue, the links or other nodes. It
//! records frames to transmit, timers to arm, log lines and packet-trace notes in
//! an `Fx`, and the world applies them once the node is done. That keeps every
//! protocol function a plain `fn(&mut Node, &mut Fx, ..)`.

use std::net::Ipv4Addr;

use crate::api::{LabEvent, NodeId, PacketEvent, PacketEventKind, SimTime, TraceId};
use crate::ifname::IfId;
use crate::sim::frame::Frame;

/// Timers a node can arm for itself. `token` fields let stale timers be ignored.
#[derive(Clone, Debug)]
pub(crate) enum Timer {
    StpHello,
    StpForward {
        vlan: u16,
        port: IfId,
        token: u32,
    },
    /// Periodic CDP/LLDP advertisement.
    Advert,
    Proc {
        id: u32,
        token: u32,
    },
    DhcpClient {
        iface: IfId,
        token: u32,
    },
    /// A DHCP client is still looking for its default gateway.
    GatewayProbe {
        iface: IfId,
        token: u32,
        left: u8,
    },
    OspfHello {
        iface: IfId,
        token: u32,
    },
    /// Adjacency progress or dead-interval check for one neighbour.
    OspfNeighbor {
        iface: IfId,
        router_id: Ipv4Addr,
        token: u32,
        dead: bool,
    },
    OspfWait {
        iface: IfId,
        token: u32,
    },
    RipUpdate {
        token: u32,
    },
    RipTriggered,
    Hsrp {
        iface: IfId,
        group: u16,
        token: u32,
    },
}

/// How much a dropped frame says about why a traced PDU failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Why {
    /// A flooded copy reached a device that was not the destination.
    Ignored,
    /// A flooded copy hit a blocked port.
    Blocked,
    /// Dropped only because ARP had not finished; retrying will get further.
    ArpPending,
    /// The real reason the PDU died.
    Fatal,
}

#[derive(Clone, Debug)]
pub(crate) struct Note {
    pub(crate) event: PacketEvent,
    pub(crate) why: Option<Why>,
}

#[derive(Debug)]
pub(crate) struct Fx {
    pub(crate) now: SimTime,
    pub(crate) realistic: bool,
    pub(crate) seed: u64,
    /// Record packet events for every frame (otherwise only traced PDUs).
    pub(crate) capture: bool,
    seq_base: u32,
    pdu_base: u64,
    pub(crate) pdus_used: u64,
    /// Frames to transmit: physical port and frame.
    pub(crate) tx: Vec<(IfId, Frame)>,
    /// Timers to arm: delay in microseconds.
    pub(crate) timers: Vec<(u64, Timer)>,
    pub(crate) events: Vec<LabEvent>,
    pub(crate) notes: Vec<Note>,
    /// Traces whose goal was reached (reply received / datagram delivered).
    pub(crate) trace_ok: Vec<TraceId>,
    /// Negative answers received by the origin of a trace ("connection refused").
    pub(crate) trace_verdict: Vec<(TraceId, String)>,
    /// Derived state must be recomputed (interface state, tables).
    pub(crate) dirty: bool,
    /// The control plane changed: restarts the quiescence clock.
    pub(crate) cp_change: bool,
    /// Tables changed: the host should refresh its view of the node.
    pub(crate) changed: bool,
    /// OSPF inputs changed: rerun the shortest-path computation.
    pub(crate) spf: bool,
}

impl Fx {
    pub(crate) fn new(
        now: SimTime,
        realistic: bool,
        seed: u64,
        capture: bool,
        seq_base: u32,
        pdu_base: u64,
    ) -> Fx {
        Fx {
            now,
            realistic,
            seed,
            capture,
            seq_base,
            pdu_base,
            pdus_used: 0,
            tx: Vec::new(),
            timers: Vec::new(),
            events: Vec::new(),
            notes: Vec::new(),
            trace_ok: Vec::new(),
            trace_verdict: Vec::new(),
            dirty: false,
            cp_change: false,
            changed: false,
            spf: false,
        }
    }

    /// Identity for a newly originated PDU.
    pub(crate) fn new_pdu(&mut self) -> u64 {
        self.pdus_used += 1;
        self.pdu_base + self.pdus_used
    }

    pub(crate) fn arm(&mut self, delay_micros: u64, timer: Timer) {
        self.timers.push((delay_micros, timer));
    }

    /// Is anyone watching this frame?
    pub(crate) fn watching(&self, frame: &Frame) -> bool {
        self.capture || frame.meta.trace.is_some()
    }

    /// Record a packet event for `frame` at `node` and return its sequence number,
    /// which callers store in `frame.meta.via` to chain the next event. Costs
    /// nothing (the closure is not run) when nobody is watching the frame.
    pub(crate) fn note(
        &mut self,
        node: NodeId,
        kind: PacketEventKind,
        frame: &Frame,
        iface: Option<IfId>,
        why: Option<Why>,
        decision: impl FnOnce() -> String,
    ) -> u32 {
        if !self.watching(frame) {
            return frame.meta.via;
        }
        let seq = self
            .seq_base
            .wrapping_add(self.notes.len() as u32)
            .wrapping_add(1);
        self.notes.push(Note {
            event: PacketEvent {
                trace: frame.meta.trace,
                pdu: frame.meta.pdu,
                seq,
                time: self.now,
                kind,
                at: node,
                from: None,
                link: None,
                protocol: frame.protocol().to_string(),
                summary: frame.summary(),
                layers: frame.layers(),
                decision: decision(),
                interface: iface.map(|i| i.name()).unwrap_or_default(),
                parent: frame.meta.via,
            },
            why,
        });
        seq
    }

    /// Shorthand for a `Dropped` event.
    pub(crate) fn dropped(
        &mut self,
        node: NodeId,
        frame: &Frame,
        iface: Option<IfId>,
        why: Why,
        decision: impl FnOnce() -> String,
    ) {
        self.note(
            node,
            PacketEventKind::Dropped,
            frame,
            iface,
            Some(why),
            decision,
        );
    }
}
