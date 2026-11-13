//! IPv4: input, local delivery, forwarding, origination and ICMP errors.
//!
//! The order of operations on a forwarded packet follows real routers:
//! inbound ACL → (outside-to-inside NAT) → local delivery or route lookup →
//! (inside-to-outside NAT) → outbound ACL → TTL → ARP → transmit.

use std::net::Ipv4Addr;

use crate::api::{NodeKind, PacketEventKind, TraceId};
use crate::catalog::Role;
use crate::device::config::NatSide;
use crate::ifname::IfId;
use crate::sim::acl::Verdict;
use crate::sim::frame::{
    Body, Frame, Icmp, L4, Meta, Packet, TcpFlags, UdpApp, Unreach, multicast_mac,
};
use crate::sim::fx::{Fx, Why};
use crate::sim::node::Node;
use crate::sim::rib::{Resolved, Source};
use crate::util::mac::Mac;
use crate::util::net::is_multicast;

/// What became of a packet handed to the forwarding path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sent {
    Ok,
    /// No route (or no usable source address).
    NoRoute,
    /// Dropped while ARP resolves the next hop.
    ArpPending,
    /// Refused by an ACL, NAT or the TTL check.
    Refused,
}

pub(crate) enum IcmpError {
    TimeExceeded,
    Unreachable(Unreach),
}

impl Node {
    pub(crate) fn default_ttl(&self) -> u8 {
        match self.model.kind {
            NodeKind::Pc | NodeKind::Laptop | NodeKind::Nms => 128,
            NodeKind::Server | NodeKind::Cloud | NodeKind::Printer | NodeKind::Phone => 64,
            _ => 255,
        }
    }

    /// Record a packet event for a packet that is not (or no longer) in a frame.
    pub(crate) fn ip_note(
        &self,
        fx: &mut Fx,
        kind: PacketEventKind,
        packet: &Packet,
        meta: &Meta,
        l2: (Mac, Mac),
        iface: Option<IfId>,
        why: Option<Why>,
        decision: impl FnOnce() -> String,
    ) -> u32 {
        if !(fx.capture || meta.trace.is_some()) {
            return meta.via;
        }
        let frame = Frame {
            src: l2.0,
            dst: l2.1,
            tag: None,
            body: Body::Ip(packet.clone()),
            meta: *meta,
        };
        fx.note(self.id, kind, &frame, iface, why, decision)
    }

    /// An IPv4 packet arrived on layer-3 interface `iface`.
    pub(crate) fn ip_rx(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        mut packet: Packet,
        meta: Meta,
        l2_src: Mac,
    ) {
        let l2 = (l2_src, self.mac_for(iface));
        let noun = self.noun();
        let (acl_in, nat_side) = match self.cfg.iface(iface) {
            Some(cfg) => (cfg.acl_in.clone(), cfg.nat),
            None => (None, None),
        };
        if let Some(name) = acl_in
            && let Verdict::Deny(line) = self.acl_eval(fx, &name, &packet)
        {
            self.ip_note(
                fx,
                PacketEventKind::Dropped,
                &packet,
                &meta,
                l2,
                Some(iface),
                Some(Why::Fatal),
                || {
                    format!(
                        "The {noun}'s inbound ACL {name} on {} {line}: the packet is dropped.",
                        iface.short()
                    )
                },
            );
            self.icmp_error(
                fx,
                Some(iface),
                &packet,
                &meta,
                IcmpError::Unreachable(Unreach::AdminProhibited),
            );
            return;
        }
        let mut remark = String::new();
        if nat_side == Some(NatSide::Outside)
            && let Some(text) = self.nat_inbound(fx, &mut packet)
        {
            remark = text;
        }
        if self.ip_is_local(iface, packet.dst) {
            self.ip_local(fx, iface, packet, meta, l2);
            return;
        }
        if !self.routes_packets() {
            self.ip_note(fx, PacketEventKind::Dropped, &packet, &meta, l2, Some(iface), Some(Why::Ignored), || {
                format!("The packet is for {}, which is not this {noun}; it does not route, so the packet is ignored.", packet.dst)
            });
            return;
        }
        self.ip_forward(fx, Some(iface), packet, meta, l2, remark);
    }

    fn ip_is_local(&self, iface: IfId, dst: Ipv4Addr) -> bool {
        dst.is_broadcast()
            || is_multicast(dst)
            || self.rib.addrs.iter().any(|(_, a)| a.ip == dst)
            || self
                .addrs(iface)
                .iter()
                .any(|a| a.len < 31 && a.subnet().broadcast() == dst)
            || self.hsrp_owns_ip(dst)
    }

    /// Route and transmit a packet. `in_iface` is `None` for packets this node
    /// originated (no TTL decrement, no ICMP errors to ourselves, no outbound ACL).
    pub(crate) fn ip_forward(
        &mut self,
        fx: &mut Fx,
        in_iface: Option<IfId>,
        mut packet: Packet,
        mut meta: Meta,
        l2: (Mac, Mac),
        mut remark: String,
    ) -> Sent {
        let noun = self.noun();
        let transit = in_iface.is_some();
        if transit && packet.ttl <= 1 {
            self.ip_note(fx, PacketEventKind::Dropped, &packet, &meta, l2, in_iface, Some(Why::Fatal), || {
                format!(
                    "The packet's TTL has run out at this {noun}: it is discarded and an ICMP time-exceeded message goes back to {}.",
                    packet.src
                )
            });
            self.icmp_error(fx, in_iface, &packet, &meta, IcmpError::TimeExceeded);
            return Sent::Refused;
        }
        let Some(resolved) = self.route_lookup(packet.dst) else {
            self.ip_note(fx, PacketEventKind::Dropped, &packet, &meta, l2, in_iface, Some(Why::Fatal), || {
                if transit {
                    format!(
                        "The {noun} has no route to {} and no default route: the packet is dropped and an ICMP destination-unreachable goes back to {}.",
                        packet.dst, packet.src
                    )
                } else if self.role() == Role::Host {
                    format!(
                        "{} is not on the {noun}'s own subnet and no default gateway is configured, so the packet cannot be sent.",
                        packet.dst
                    )
                } else {
                    format!("The {noun} has no route to {}: the packet cannot be sent.", packet.dst)
                }
            });
            if transit {
                self.icmp_error(
                    fx,
                    in_iface,
                    &packet,
                    &meta,
                    IcmpError::Unreachable(Unreach::Host),
                );
            }
            return Sent::NoRoute;
        };
        let out = resolved.iface;
        let out_cfg = self.cfg.iface(out);
        let (acl_out, out_nat) = (
            out_cfg.and_then(|c| c.acl_out.clone()),
            out_cfg.and_then(|c| c.nat),
        );
        let in_nat = in_iface.and_then(|i| self.cfg.iface(i)).and_then(|c| c.nat);
        if in_nat == Some(NatSide::Inside) && out_nat == Some(NatSide::Outside) {
            match self.nat_outbound(fx, &mut packet, out) {
                Ok(Some(text)) => remark.push_str(&text),
                Ok(None) => {}
                Err(text) => {
                    self.ip_note(
                        fx,
                        PacketEventKind::Dropped,
                        &packet,
                        &meta,
                        l2,
                        in_iface,
                        Some(Why::Fatal),
                        || text,
                    );
                    self.icmp_error(
                        fx,
                        in_iface,
                        &packet,
                        &meta,
                        IcmpError::Unreachable(Unreach::Host),
                    );
                    return Sent::Refused;
                }
            }
        }
        if transit
            && let Some(name) = acl_out
            && let Verdict::Deny(line) = self.acl_eval(fx, &name, &packet)
        {
            self.ip_note(
                fx,
                PacketEventKind::Dropped,
                &packet,
                &meta,
                l2,
                Some(out),
                Some(Why::Fatal),
                || {
                    format!(
                        "The {noun}'s outbound ACL {name} on {} {line}: the packet is dropped.",
                        out.short()
                    )
                },
            );
            self.icmp_error(
                fx,
                in_iface,
                &packet,
                &meta,
                IcmpError::Unreachable(Unreach::AdminProhibited),
            );
            return Sent::Refused;
        }
        if transit {
            packet.ttl -= 1;
            self.forwarded += 1;
        }
        let next_hop = resolved.next_hop;
        let Some(mac) = self.arp_resolve(fx, out, next_hop, &meta) else {
            self.ip_note(fx, PacketEventKind::Dropped, &packet, &meta, l2, Some(out), Some(Why::ArpPending), || {
                format!(
                    "The {noun} does not know the MAC address of {} yet. It asks with an ARP request out of {} and drops this packet; once the reply arrives the next packet gets through.",
                    if next_hop == packet.dst { format!("{next_hop}") } else { format!("the next hop {next_hop}") },
                    out.short()
                )
            });
            if let Some(state) = self.ifs.get_mut(&out) {
                state.counters.out_discards += 1;
            }
            return Sent::ArpPending;
        };
        let src_mac = self.mac_for(out);
        meta.via = self.ip_note(
            fx,
            PacketEventKind::Forwarded,
            &packet,
            &meta,
            (src_mac, mac),
            Some(out),
            None,
            || {
                format!(
                    "{remark}{}",
                    self.route_text(&resolved, &packet, mac, transit)
                )
            },
        );
        self.l3_tx(fx, out, src_mac, mac, Body::Ip(packet), meta);
        Sent::Ok
    }

    /// Plain-English account of a forwarding decision.
    fn route_text(&self, resolved: &Resolved, packet: &Packet, mac: Mac, transit: bool) -> String {
        let noun = self.noun();
        let route = &resolved.route;
        let out = resolved.iface.short();
        let dst = packet.dst;
        let how = match (route.source, resolved.next_hop == dst) {
            (Source::Connected | Source::Local, _) => format!(
                "{dst} is on the directly connected network {} ({out}), so the frame is addressed straight to its MAC address {mac}",
                route.prefix
            ),
            (_, _) if route.prefix.len == 0 && self.role() == Role::Host => format!(
                "{dst} is not on the {noun}'s own subnet, so the packet goes to the default gateway {} (MAC {mac}) out of {out}",
                resolved.next_hop
            ),
            (source, _) => format!(
                "the longest match for {dst} is {} {}{} via {}, out of {out}; the frame is addressed to the next hop's MAC {mac}",
                source.code(),
                route.prefix,
                if route.prefix.len == 0 {
                    " (the default route)"
                } else {
                    ""
                },
                resolved.next_hop
            ),
        };
        if transit {
            format!(
                "The {noun} looks in its routing table: {how}. It lowers the TTL to {} and rewrites the Ethernet header.",
                packet.ttl
            )
        } else {
            format!("The {noun} decides where to send its packet: {how}.")
        }
    }

    /// Originate a packet. An unspecified source address is filled in from the
    /// egress interface. Returns the PDU identity and what happened to it.
    pub(crate) fn ip_send(
        &mut self,
        fx: &mut Fx,
        mut packet: Packet,
        trace: Option<TraceId>,
        cause: Option<&Meta>,
    ) -> (u64, Sent) {
        let mut meta = match cause {
            Some(cause) => self.caused(fx, cause),
            None => Meta {
                pdu: fx.new_pdu(),
                trace,
                via: 0,
                hops: 0,
            },
        };
        let noun = self.noun();
        let l2 = (Mac::ZERO, Mac::ZERO);
        if packet.src.is_unspecified() {
            match self.source_for(packet.dst) {
                Some(src) => packet.src = src,
                None => {
                    self.ip_note(fx, PacketEventKind::Dropped, &packet, &meta, l2, None, Some(Why::Fatal), || {
                        format!(
                            "The {noun} has no route towards {} (no matching network and no default gateway), so nothing is sent.",
                            packet.dst
                        )
                    });
                    return (meta.pdu, Sent::NoRoute);
                }
            }
        }
        meta.via = self.ip_note(
            fx,
            PacketEventKind::Created,
            &packet,
            &meta,
            l2,
            None,
            None,
            || format!("The {noun} builds the packet: {}.", packet.summary()),
        );
        let pdu = meta.pdu;
        if let Some((iface, _)) = self
            .rib
            .addrs
            .iter()
            .find(|(_, a)| a.ip == packet.dst)
            .copied()
        {
            self.ip_local(fx, iface, packet, meta, l2);
            return (pdu, Sent::Ok);
        }
        (
            pdu,
            self.ip_forward(fx, None, packet, meta, l2, String::new()),
        )
    }

    /// Originate a link-local packet (broadcast or multicast) on one interface.
    pub(crate) fn ip_send_on(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        packet: Packet,
        cause: Option<&Meta>,
        why: impl FnOnce() -> String,
    ) {
        if !self.is_up(iface) {
            return;
        }
        let mut meta = match cause {
            Some(cause) => self.caused(fx, cause),
            None => Meta {
                pdu: fx.new_pdu(),
                ..Meta::default()
            },
        };
        let dst_mac = if is_multicast(packet.dst) {
            multicast_mac(packet.dst)
        } else {
            Mac::BROADCAST
        };
        let src_mac = self.mac_for(iface);
        meta.via = self.ip_note(
            fx,
            PacketEventKind::Created,
            &packet,
            &meta,
            (src_mac, dst_mac),
            Some(iface),
            None,
            why,
        );
        self.l3_tx(fx, iface, src_mac, dst_mac, Body::Ip(packet), meta);
    }

    /// Send an ICMP error about `original` back to its source.
    pub(crate) fn icmp_error(
        &mut self,
        fx: &mut Fx,
        in_iface: Option<IfId>,
        original: &Packet,
        meta: &Meta,
        error: IcmpError,
    ) {
        if original.is_icmp_error()
            || original.dst.is_broadcast()
            || is_multicast(original.dst)
            || original.src.is_unspecified()
        {
            return;
        }
        let src = in_iface
            .and_then(|i| self.addr(i))
            .map(|a| a.ip)
            .unwrap_or(Ipv4Addr::UNSPECIFIED);
        let icmp = match error {
            IcmpError::TimeExceeded => Icmp::TimeExceeded {
                orig: original.orig(),
            },
            IcmpError::Unreachable(code) => Icmp::Unreachable {
                code,
                orig: original.orig(),
            },
        };
        let packet = Packet {
            src,
            dst: original.src,
            ttl: self.default_ttl(),
            size: 56,
            l4: L4::Icmp(icmp),
        };
        self.ip_send(fx, packet, None, Some(meta));
    }

    /// A packet addressed to this node.
    fn ip_local(&mut self, fx: &mut Fx, iface: IfId, packet: Packet, meta: Meta, l2: (Mac, Mac)) {
        let noun = self.noun();
        let owned =
            self.rib.addrs.iter().any(|(_, a)| a.ip == packet.dst) || self.hsrp_owns_ip(packet.dst);
        match &packet.l4 {
            L4::Icmp(Icmp::EchoRequest { id, seq }) => {
                // Answer unicast and broadcast pings, not multicast ones.
                let reply_src = if owned {
                    Some(packet.dst)
                } else if is_multicast(packet.dst) {
                    None
                } else {
                    self.addr(iface).map(|a| a.ip)
                };
                let Some(reply_src) = reply_src else { return };
                let via = self.ip_note(fx, PacketEventKind::Delivered, &packet, &meta, l2, Some(iface), None, || {
                    format!(
                        "The packet is addressed to this {noun} ({}). ICMP answers the echo request with an echo reply to {}.",
                        packet.dst, packet.src
                    )
                });
                let reply = Packet {
                    src: reply_src,
                    dst: packet.src,
                    ttl: self.default_ttl(),
                    size: packet.size,
                    l4: L4::Icmp(Icmp::EchoReply { id: *id, seq: *seq }),
                };
                self.ip_send(fx, reply, None, Some(&Meta { via, ..meta }));
            }
            L4::Icmp(Icmp::EchoReply { id, seq }) => {
                self.ip_note(
                    fx,
                    PacketEventKind::Delivered,
                    &packet,
                    &meta,
                    l2,
                    Some(iface),
                    None,
                    || {
                        format!(
                            "The echo reply from {} reaches the {noun} that sent the request.",
                            packet.src
                        )
                    },
                );
                self.proc_echo_reply(fx, &packet, *id, *seq);
            }
            L4::Icmp(icmp @ (Icmp::Unreachable { .. } | Icmp::TimeExceeded { .. })) => {
                self.ip_note(
                    fx,
                    PacketEventKind::Delivered,
                    &packet,
                    &meta,
                    l2,
                    Some(iface),
                    None,
                    || {
                        format!(
                            "{} reports a problem with a packet this {noun} sent: {}.",
                            packet.src,
                            packet.summary()
                        )
                    },
                );
                self.proc_icmp_error(fx, packet.src, *icmp);
            }
            L4::Udp { .. } => self.udp_rx(fx, iface, packet, meta, l2, owned),
            L4::Tcp { .. } => self.tcp_rx(fx, iface, packet, meta, l2, owned),
            L4::Ospf(hello) => {
                let hello = hello.clone();
                self.ospf_rx(fx, iface, packet.src, &hello);
            }
        }
    }

    fn udp_rx(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        packet: Packet,
        meta: Meta,
        l2: (Mac, Mac),
        owned: bool,
    ) {
        let noun = self.noun();
        let L4::Udp { sport, dport, app } = &packet.l4 else {
            return;
        };
        match app {
            UdpApp::Dhcp(msg) => {
                let msg = msg.clone();
                self.dhcp_rx(fx, iface, &packet, &msg, &meta, l2);
            }
            UdpApp::Rip(msg) => {
                let msg = msg.clone();
                self.rip_rx(fx, iface, packet.src, &msg);
            }
            UdpApp::Hsrp(msg) => self.hsrp_rx(fx, iface, packet.src, *msg),
            UdpApp::Dns(msg) => {
                let msg = msg.clone();
                self.dns_rx(fx, &packet, &msg, &meta, l2, iface);
            }
            UdpApp::Syslog | UdpApp::Data if !owned => {}
            UdpApp::Syslog if self.has_service("syslog") => {
                self.ip_note(
                    fx,
                    PacketEventKind::Delivered,
                    &packet,
                    &meta,
                    l2,
                    Some(iface),
                    None,
                    || format!("The syslog service on this {noun} stores the message."),
                );
            }
            UdpApp::Syslog | UdpApp::Data => {
                // Nothing listens: the datagram arrived, which is what a UDP
                // trace or a traceroute probe wanted to know.
                if let Some(trace) = meta.trace {
                    fx.trace_ok.push(trace);
                }
                let via = self.ip_note(fx, PacketEventKind::Delivered, &packet, &meta, l2, Some(iface), None, || {
                    format!(
                        "The datagram reaches {} but nothing listens on UDP port {dport}: the {noun} answers with ICMP port unreachable.",
                        packet.dst
                    )
                });
                let _ = sport;
                self.icmp_error(
                    fx,
                    Some(iface),
                    &packet,
                    &Meta { via, ..meta },
                    IcmpError::Unreachable(Unreach::Port),
                );
            }
        }
    }

    fn tcp_rx(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        packet: Packet,
        meta: Meta,
        l2: (Mac, Mac),
        owned: bool,
    ) {
        let noun = self.noun();
        let L4::Tcp {
            sport,
            dport,
            flags,
        } = packet.l4
        else {
            return;
        };
        if !owned {
            return;
        }
        match flags {
            TcpFlags::Syn => {
                let open = self.tcp_listens(dport, packet.src);
                let via = self.ip_note(fx, PacketEventKind::Delivered, &packet, &meta, l2, Some(iface), None, || {
                    if open {
                        format!("TCP port {dport} is open on this {noun}: it accepts the connection with a SYN-ACK.")
                    } else {
                        format!("Nothing accepts connections on TCP port {dport} of this {noun}: it refuses with a RST.")
                    }
                });
                let reply = Packet {
                    src: packet.dst,
                    dst: packet.src,
                    ttl: self.default_ttl(),
                    size: 44,
                    l4: L4::Tcp {
                        sport: dport,
                        dport: sport,
                        flags: if open {
                            TcpFlags::SynAck
                        } else {
                            TcpFlags::Rst
                        },
                    },
                };
                self.ip_send(fx, reply, None, Some(&Meta { via, ..meta }));
            }
            TcpFlags::SynAck | TcpFlags::Rst => {
                self.ip_note(fx, PacketEventKind::Delivered, &packet, &meta, l2, Some(iface), None, || {
                    if flags == TcpFlags::SynAck {
                        format!("The SYN-ACK from {} completes the handshake: the connection is open.", packet.src)
                    } else {
                        format!("{} refused the connection (RST).", packet.src)
                    }
                });
                self.proc_tcp_reply(fx, packet.src, sport, dport, flags == TcpFlags::SynAck);
            }
            TcpFlags::Ack => {}
        }
    }

    /// Does this node accept TCP connections on `port` from `from`?
    pub(crate) fn tcp_listens(&mut self, port: u16, from: Ipv4Addr) -> bool {
        if self.model.is_network_os() {
            return match port {
                23 => self.vty_accepts(false, from),
                22 => self.vty_accepts(true, from),
                80 => self
                    .cfg
                    .stored
                    .iter()
                    .any(|(_, line)| line == "ip http server"),
                443 => self
                    .cfg
                    .stored
                    .iter()
                    .any(|(_, line)| line == "ip http secure-server"),
                _ => false,
            };
        }
        let service = match port {
            80 | 443 => "http",
            53 => "dns",
            22 => "ssh",
            23 => "telnet",
            _ => return false,
        };
        self.has_service(service)
    }

    /// Is a vty line reachable with the given protocol from `from`?
    pub(crate) fn vty_accepts(&mut self, ssh: bool, from: Ipv4Addr) -> bool {
        let Some(line) = self.cfg.vty.first().cloned() else {
            return false;
        };
        let transport = line
            .transport_input
            .is_none_or(|t| if ssh { t.ssh } else { t.telnet });
        // SSH additionally needs host keys.
        let keys = !ssh || self.rsa_bits.is_some();
        let allowed = match &line.access_class_in {
            Some(acl) => self.acl_permits_source(acl, from),
            None => true,
        };
        transport && keys && allowed
    }

    pub(crate) fn has_service(&self, name: &str) -> bool {
        self.host
            .as_ref()
            .is_some_and(|h| h.services.iter().any(|s| s.eq_ignore_ascii_case(name)))
    }
}
