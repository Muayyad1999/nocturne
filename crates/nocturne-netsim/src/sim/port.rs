//! Where frames enter and leave a node: per-port dispatch on ingress, and the
//! layer-3 transmit path that turns "send this out of interface X" into frames
//! on physical ports.

use crate::api::PacketEventKind;
use crate::catalog::Role;
use crate::ifname::{IfId, IfKind};
use crate::sim::frame::{Body, Frame, Meta};
use crate::sim::fx::{Fx, Timer, Why};
use crate::sim::node::Node;
use crate::util::mac::Mac;

impl Node {
    /// A frame arrived on physical port `port`.
    pub(crate) fn rx(&mut self, fx: &mut Fx, port: IfId, frame: Frame) {
        if let Body::Advert(advert) = &frame.body {
            // CDP and LLDP are link-local: consumed here, never forwarded.
            let advert = advert.clone();
            self.advert_rx(fx, port, &advert, &frame);
            return;
        }
        if self.role() == Role::Bridge {
            self.bridge_rx(fx, port, frame);
            return;
        }
        let Some(cfg) = self.cfg.iface(port) else { return };
        if cfg.l2.is_some() || cfg.channel_group.is_some() {
            match self.l2_port(port) {
                Some(l2) => self.switch_rx(fx, l2, port, frame),
                None => fx.dropped(self.id, &frame, Some(port), Why::Fatal, || {
                    format!(
                        "{} is configured for an EtherChannel but is not bundled (suspended), so the switch discards the frame.",
                        port.short()
                    )
                }),
            }
        } else {
            self.routed_rx(fx, port, frame);
        }
    }

    /// Ingress on a routed physical port: pick the (sub-)interface, then layer 3.
    fn routed_rx(&mut self, fx: &mut Fx, port: IfId, frame: Frame) {
        if matches!(frame.body, Body::Bpdu(_)) {
            return;
        }
        let sub = match frame.tag {
            Some(vlan) => {
                self.cfg.subinterfaces(port).find(|(_, cfg)| cfg.dot1q.is_some_and(|(v, _)| v == vlan)).map(|(id, _)| id)
            }
            None => self
                .cfg
                .subinterfaces(port)
                .find(|(_, cfg)| cfg.dot1q.is_some_and(|(_, native)| native))
                .map(|(id, _)| id)
                .or(Some(port)),
        };
        let Some(iface) = sub.filter(|id| self.is_up(*id)) else {
            let noun = self.noun();
            fx.dropped(self.id, &frame, Some(port), Why::Fatal, || match frame.tag {
                Some(vlan) => format!(
                    "The frame is tagged for VLAN {vlan}, but the {noun} has no active sub-interface on {} with encapsulation dot1Q {vlan}: dropped.",
                    port.short()
                ),
                None => format!("{} is down on the {noun}: the frame is dropped.", port.short()),
            });
            return;
        };
        self.l3_rx(fx, iface, frame);
    }

    /// A frame reached a layer-3 interface (routed port, sub-interface or SVI).
    pub(crate) fn l3_rx(&mut self, fx: &mut Fx, iface: IfId, frame: Frame) {
        let mine = frame.dst == self.mac_for(iface) || frame.dst.is_multicast() || self.hsrp_owns_mac(iface, frame.dst);
        if !mine {
            let noun = self.noun();
            fx.dropped(self.id, &frame, Some(iface), Why::Ignored, || {
                format!(
                    "The frame is addressed to {}, not to this {noun}'s MAC address: it is ignored.",
                    frame.dst
                )
            });
            return;
        }
        if !iface.is_physical()
            && let Some(state) = self.ifs.get_mut(&iface)
        {
            state.counters.in_pkts += 1;
            state.counters.in_octets += frame.wire_len();
            state.counters.last_in = Some(fx.now);
        }
        let Frame { src, body, meta, .. } = frame;
        match body {
            Body::Arp(arp) => self.arp_rx(fx, iface, arp, meta),
            Body::Ip(packet) => self.ip_rx(fx, iface, packet, meta, src),
            Body::Bpdu(_) | Body::Advert(_) => {}
        }
    }

    /// Send a layer-2 payload out of a layer-3 interface.
    pub(crate) fn l3_tx(&mut self, fx: &mut Fx, iface: IfId, src: Mac, dst: Mac, body: Body, meta: Meta) {
        let mut frame = Frame { src, dst, tag: None, body, meta };
        if !iface.is_physical()
            && let Some(state) = self.ifs.get_mut(&iface)
        {
            state.counters.out_pkts += 1;
            state.counters.out_octets += frame.wire_len();
            state.counters.last_out = Some(fx.now);
        }
        match iface.kind {
            IfKind::Vlan => self.switch_tx_from_cpu(fx, iface.number() as u16, frame),
            IfKind::Loopback => {}
            _ if iface.is_sub() => {
                if let Some((vlan, native)) = self.cfg.iface(iface).and_then(|cfg| cfg.dot1q) {
                    frame.tag = (!native).then_some(vlan);
                    fx.tx.push((iface.parent(), frame));
                }
            }
            _ => fx.tx.push((iface, frame)),
        }
    }

    /// Transparent bridging (access point): learn, forward or flood, tags untouched.
    fn bridge_rx(&mut self, fx: &mut Fx, port: IfId, mut frame: Frame) {
        frame.meta.hops = frame.meta.hops.saturating_add(1);
        if frame.meta.hops > crate::sim::switching::MAX_BRIDGE_HOPS {
            return;
        }
        let vlan = frame.tag.unwrap_or(0);
        if !frame.src.is_multicast() {
            self.macs.learn(vlan, frame.src, port, fx.now);
        }
        let known = (!frame.dst.is_multicast()).then(|| self.macs.lookup(vlan, frame.dst, fx.now)).flatten();
        let out: Vec<IfId> = match known {
            Some(p) if p == port => Vec::new(),
            Some(p) => vec![p],
            None => self.ifs.iter().filter(|(id, s)| **id != port && s.up).map(|(id, _)| *id).collect(),
        };
        frame.meta.via = fx.note(self.id, PacketEventKind::Forwarded, &frame, None, None, || {
            format!(
                "The access point bridges the frame transparently between its wired and radio sides ({}).",
                if out.is_empty() { "no other port needs it".to_string() } else { short_list(&out) }
            )
        });
        for p in out {
            fx.tx.push((p, frame.clone()));
        }
    }

    /// A timer this node armed has fired.
    pub(crate) fn on_timer(&mut self, fx: &mut Fx, timer: Timer) {
        match timer {
            Timer::StpHello => self.stp_hello(fx),
            Timer::StpForward { vlan, port, token } => self.stp_forward_timer(fx, vlan, port, token),
            Timer::Advert => self.advert_timer(fx),
            Timer::Proc { id, token } => self.proc_timer(fx, id, token),
            Timer::DhcpClient { iface, token } => self.dhcp_client_timer(fx, iface, token),
            Timer::OspfHello { iface, token } => self.ospf_hello_timer(fx, iface, token),
            Timer::OspfNeighbor { iface, router_id, token } => self.ospf_neighbor_timer(fx, iface, router_id, token),
            Timer::OspfWait { iface, token } => self.ospf_wait_timer(fx, iface, token),
            Timer::RipUpdate { token } => self.rip_update_timer(fx, token),
            Timer::RipTriggered => self.rip_triggered(fx),
            Timer::Hsrp { iface, group, token } => self.hsrp_timer(fx, iface, group, token),
        }
    }
}

/// `Fa0/1, Gi0/2` for explanations.
pub(crate) fn short_list(ports: &[IfId]) -> String {
    ports.iter().map(|p| p.short()).collect::<Vec<_>>().join(", ")
}
