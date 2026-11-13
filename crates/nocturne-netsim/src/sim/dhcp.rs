//! DHCP: the server (pools, bindings), the relay agent (`ip helper-address`)
//! and the client (hosts, and router interfaces with `ip address dhcp`).

use std::net::Ipv4Addr;

use crate::api::{DhcpRow, PacketEventKind, SimTime};
use crate::device::config::{DhcpPool, L3Addr, Lease as LeaseTime};
use crate::ifname::IfId;
use crate::sim::frame::{
    Body, DhcpMsg, DhcpOp, L4, Meta, PORT_DHCP_CLIENT, PORT_DHCP_SERVER, Packet, UdpApp,
};
use crate::sim::fx::{Fx, Timer};
use crate::sim::node::Node;
use crate::util::mac::Mac;
use crate::util::net::{IfAddr, Prefix, ip_add};
use crate::util::rng;

const DEFAULT_LEASE_S: u64 = 86_400;
/// How often, and how many times, a new client looks for its default gateway.
const GATEWAY_PROBE_MICROS: u64 = 3_000_000;
const GATEWAY_PROBES: u8 = 5;
/// A client that got no answer after this many tries is no longer "converging".
const EAGER_ATTEMPTS: u32 = 4;

// ------------------------------------------------------------------ client

#[derive(Clone, Debug)]
pub(crate) struct Lease {
    pub(crate) addr: IfAddr,
    pub(crate) server: Ipv4Addr,
    pub(crate) gateway: Option<Ipv4Addr>,
    pub(crate) dns: Vec<Ipv4Addr>,
    pub(crate) obtained: SimTime,
    pub(crate) expires: Option<SimTime>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ClientState {
    #[default]
    Idle,
    Selecting,
    Requesting,
    Bound,
    Renewing,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DhcpClient {
    pub(crate) lease: Option<Lease>,
    state: ClientState,
    xid: u32,
    token: u32,
    attempts: u32,
}

impl DhcpClient {
    /// Give up the lease and stop (the interface went down or was reconfigured).
    pub(crate) fn release(&mut self) {
        self.lease = None;
        self.state = ClientState::Idle;
        self.token = self.token.wrapping_add(1);
        self.attempts = 0;
    }
}

// ------------------------------------------------------------------ server

#[derive(Clone, Debug)]
pub(crate) struct Binding {
    pub(crate) ip: Ipv4Addr,
    pub(crate) mac: Mac,
    pub(crate) pool: String,
    pub(crate) expires: Option<SimTime>,
    /// Offered but not yet requested.
    tentative: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DhcpServer {
    pub(crate) bindings: Vec<Binding>,
}

/// The pool a `dhcp` service on a server serves: its own subnet, from .100 up
/// when the subnet is big enough.
fn service_pool(node: &Node) -> Option<(DhcpPool, u32)> {
    if !node.has_service("dhcp") {
        return None;
    }
    let (_, addr) = node.rib.addrs.first().copied()?;
    let subnet = addr.subnet();
    let first = if subnet.len <= 24 { 100 } else { 1 };
    let mut pool = DhcpPool::new("service");
    pool.network = Some(subnet);
    pool.default_routers = node.cfg.default_gateway.into_iter().collect();
    pool.dns_servers = if node.has_service("dns") {
        vec![addr.ip]
    } else {
        node.cfg.name_servers.clone()
    };
    Some((pool, first))
}

impl Node {
    fn dhcp_pools(&self) -> Vec<(DhcpPool, u32)> {
        let mut pools: Vec<(DhcpPool, u32)> = self
            .cfg
            .dhcp_pools
            .iter()
            .cloned()
            .map(|p| (p, 1))
            .collect();
        pools.extend(service_pool(self));
        pools
    }

    fn dhcp_excluded(&self, ip: Ipv4Addr) -> bool {
        self.cfg
            .dhcp_excluded
            .iter()
            .any(|(low, high)| (u32::from(*low)..=u32::from(*high)).contains(&u32::from(ip)))
            || self.rib.addrs.iter().any(|(_, a)| a.ip == ip)
            || self.cfg.default_gateway == Some(ip)
    }

    /// Pick an address for a client: its current binding, else the lowest free one.
    fn dhcp_allocate(
        &mut self,
        pool: &DhcpPool,
        first: u32,
        mac: Mac,
        now: SimTime,
    ) -> Option<Ipv4Addr> {
        let network = pool.network?;
        self.dhcpd
            .bindings
            .retain(|b| b.expires.is_none_or(|t| t > now));
        if let Some(existing) = self
            .dhcpd
            .bindings
            .iter()
            .find(|b| b.mac == mac && b.pool == pool.name)
        {
            return Some(existing.ip);
        }
        let hosts = (1u32 << (32 - u32::from(network.len))).saturating_sub(2);
        (first..=hosts)
            .filter_map(|offset| ip_add(network.net, offset))
            .find(|ip| {
                !self.dhcp_excluded(*ip)
                    && !pool.default_routers.contains(ip)
                    && !self.dhcpd.bindings.iter().any(|b| b.ip == *ip)
                    && self.arp.lookup(*ip, now).is_none()
            })
    }

    fn dhcp_reply(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        request: &DhcpMsg,
        reply: DhcpMsg,
        cause: &Meta,
    ) {
        let server_ip = reply.server.unwrap_or(Ipv4Addr::UNSPECIFIED);
        let to_relay = !request.relay.is_unspecified();
        let packet = Packet {
            src: server_ip,
            dst: if to_relay {
                request.relay
            } else {
                Ipv4Addr::BROADCAST
            },
            ttl: self.default_ttl(),
            size: 328,
            l4: L4::Udp {
                sport: PORT_DHCP_SERVER,
                dport: if to_relay {
                    PORT_DHCP_SERVER
                } else {
                    PORT_DHCP_CLIENT
                },
                app: UdpApp::Dhcp(Box::new(reply)),
            },
        };
        if to_relay {
            self.ip_send(fx, packet, None, Some(cause));
        } else {
            self.dhcp_to_client(fx, iface, request.client_mac, packet, cause);
        }
    }

    /// Hand a server message to a client that has no address yet: IP broadcast
    /// in a frame addressed to the client's MAC.
    fn dhcp_to_client(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        client: Mac,
        packet: Packet,
        cause: &Meta,
    ) {
        if !self.is_up(iface) {
            return;
        }
        let mut meta = self.caused(fx, cause);
        let src_mac = self.mac_for(iface);
        let noun = self.noun();
        meta.via = self.ip_note(fx, PacketEventKind::Created, &packet, &meta, (src_mac, client), Some(iface), None, || {
            format!(
                "The {noun} sends the DHCP message out of {} to the client's MAC address {client}; the client has no IP address yet, so the IP destination is the broadcast address.",
                iface.short()
            )
        });
        self.l3_tx(fx, iface, src_mac, client, Body::Ip(packet), meta);
    }

    fn dhcp_server_rx(&mut self, fx: &mut Fx, iface: IfId, msg: &DhcpMsg, meta: &Meta) -> bool {
        // The pool is chosen by where the client sits: the relay's address, or
        // else the interface the request arrived on.
        let Some(anchor) = (if msg.relay.is_unspecified() {
            self.addr(iface).map(|a| a.ip)
        } else {
            Some(msg.relay)
        }) else {
            return false;
        };
        let Some((pool, first)) = self
            .dhcp_pools()
            .into_iter()
            .find(|(p, _)| p.network.is_some_and(|n| n.contains(anchor)))
        else {
            return false;
        };
        let Some(network) = pool.network else {
            return false;
        };
        let server_ip = self
            .addr(iface)
            .map(|a| a.ip)
            .unwrap_or(Ipv4Addr::UNSPECIFIED);
        let lease_s = match pool.lease {
            Some(LeaseTime::Infinite) => None,
            Some(time) => time.seconds(),
            None => Some(DEFAULT_LEASE_S),
        };
        let now = fx.now;
        let answer = |op: DhcpOp, ip: Ipv4Addr| DhcpMsg {
            op,
            xid: msg.xid,
            client_mac: msg.client_mac,
            client_ip: Ipv4Addr::UNSPECIFIED,
            your_ip: ip,
            relay: msg.relay,
            server: Some(server_ip),
            requested: None,
            mask_len: network.len,
            routers: pool.default_routers.clone(),
            dns: pool.dns_servers.clone(),
            domain: pool.domain.clone(),
            lease_s,
            hostname: String::new(),
        };
        match msg.op {
            DhcpOp::Discover => {
                let Some(ip) = self.dhcp_allocate(&pool, first, msg.client_mac, now) else {
                    return true;
                };
                if !self.dhcpd.bindings.iter().any(|b| b.ip == ip) {
                    // Hold the offer briefly so two clients are not offered the same address.
                    self.dhcpd.bindings.push(Binding {
                        ip,
                        mac: msg.client_mac,
                        pool: pool.name.clone(),
                        expires: Some(now + 60_000_000),
                        tentative: true,
                    });
                }
                self.dhcp_reply(fx, iface, msg, answer(DhcpOp::Offer, ip), meta);
            }
            DhcpOp::Request => {
                // A request naming another server is that server's business.
                if msg
                    .server
                    .is_some_and(|s| s != server_ip && !self.owns_ip(s))
                {
                    self.dhcpd
                        .bindings
                        .retain(|b| !(b.mac == msg.client_mac && b.tentative));
                    return true;
                }
                let wanted = msg
                    .requested
                    .or(Some(msg.client_ip).filter(|ip| !ip.is_unspecified()));
                let ours = self.dhcp_allocate(&pool, first, msg.client_mac, now);
                match (wanted, ours) {
                    (Some(wanted), Some(ours)) if wanted == ours => {
                        self.dhcpd.bindings.retain(|b| b.ip != ours);
                        self.dhcpd.bindings.push(Binding {
                            ip: ours,
                            mac: msg.client_mac,
                            pool: pool.name.clone(),
                            expires: lease_s.map(|s| now + s * 1_000_000),
                            tentative: false,
                        });
                        self.dhcpd.bindings.sort_by_key(|b| b.ip);
                        fx.changed = true;
                        fx.cp_change = true;
                        self.dhcp_reply(fx, iface, msg, answer(DhcpOp::Ack, ours), meta);
                    }
                    _ => self.dhcp_reply(
                        fx,
                        iface,
                        msg,
                        answer(DhcpOp::Nak, Ipv4Addr::UNSPECIFIED),
                        meta,
                    ),
                }
            }
            DhcpOp::Release => {
                self.dhcpd.bindings.retain(|b| b.mac != msg.client_mac);
                fx.changed = true;
            }
            DhcpOp::Offer | DhcpOp::Ack | DhcpOp::Nak => {}
        }
        true
    }

    /// DHCP traffic reaching this node: client, server or relay role.
    pub(crate) fn dhcp_rx(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        packet: &Packet,
        msg: &DhcpMsg,
        meta: &Meta,
        l2: (Mac, Mac),
    ) {
        let L4::Udp { dport, .. } = packet.l4 else {
            return;
        };
        let noun = self.noun();
        let from_client = matches!(msg.op, DhcpOp::Discover | DhcpOp::Request | DhcpOp::Release);
        if dport == PORT_DHCP_CLIENT {
            self.dhcp_client_rx(fx, iface, packet, msg, meta, l2);
            return;
        }
        if from_client {
            let via = self.ip_note(
                fx,
                PacketEventKind::Delivered,
                packet,
                meta,
                l2,
                Some(iface),
                None,
                || {
                    format!(
                        "The {} from {} reaches this {noun}'s DHCP process.",
                        msg.op.name(),
                        msg.client_mac
                    )
                },
            );
            let meta = Meta { via, ..*meta };
            if self.dhcp_server_rx(fx, iface, msg, &meta) {
                return;
            }
            // Not ours to answer: relay it if the interface has helper addresses.
            let helpers = self
                .cfg
                .iface(iface)
                .map(|c| c.helpers.clone())
                .unwrap_or_default();
            let Some(gateway) = self.addr(iface).map(|a| a.ip) else {
                return;
            };
            if !msg.relay.is_unspecified() || !packet.dst.is_broadcast() {
                return;
            }
            for helper in helpers {
                let relayed = Packet {
                    src: gateway,
                    dst: helper,
                    ttl: self.default_ttl(),
                    size: packet.size,
                    l4: L4::Udp {
                        sport: PORT_DHCP_SERVER,
                        dport: PORT_DHCP_SERVER,
                        app: UdpApp::Dhcp(Box::new(DhcpMsg {
                            relay: gateway,
                            ..msg.clone()
                        })),
                    },
                };
                self.ip_send(fx, relayed, None, Some(&meta));
            }
        } else if let Some((client_iface, _)) = self
            .rib
            .addrs
            .iter()
            .copied()
            .find(|(_, a)| a.ip == msg.relay)
        {
            // A server's answer to something we relayed: pass it on to the client.
            let via = self.ip_note(fx, PacketEventKind::Delivered, packet, meta, l2, Some(iface), None, || {
                format!(
                    "The {noun} relayed this client's request (ip helper-address), so it passes the server's {} on to the client on {}.",
                    msg.op.name(),
                    client_iface.short()
                )
            });
            let out = Packet {
                src: msg.relay,
                dst: Ipv4Addr::BROADCAST,
                ttl: self.default_ttl(),
                size: packet.size,
                l4: L4::Udp {
                    sport: PORT_DHCP_SERVER,
                    dport: PORT_DHCP_CLIENT,
                    app: UdpApp::Dhcp(Box::new(msg.clone())),
                },
            };
            self.dhcp_to_client(
                fx,
                client_iface,
                msg.client_mac,
                out,
                &Meta { via, ..*meta },
            );
        }
    }

    // ---------------------------------------------------------------- client

    fn dhcp_client_send(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        op: DhcpOp,
        requested: Option<Ipv4Addr>,
        server: Option<Ipv4Addr>,
    ) {
        let Some(state) = self.ifs.get(&iface) else {
            return;
        };
        let (xid, client_ip) = (state.dhcp.xid, state.dhcp.lease.as_ref().map(|l| l.addr.ip));
        let msg = DhcpMsg {
            op,
            xid,
            client_mac: self.mac_for(iface),
            client_ip: client_ip.unwrap_or(Ipv4Addr::UNSPECIFIED),
            your_ip: Ipv4Addr::UNSPECIFIED,
            relay: Ipv4Addr::UNSPECIFIED,
            server,
            requested,
            mask_len: 0,
            routers: Vec::new(),
            dns: Vec::new(),
            domain: None,
            lease_s: None,
            hostname: self.hostname().to_string(),
        };
        let packet = Packet {
            src: Ipv4Addr::UNSPECIFIED,
            dst: Ipv4Addr::BROADCAST,
            ttl: self.default_ttl(),
            size: 328,
            l4: L4::Udp {
                sport: PORT_DHCP_CLIENT,
                dport: PORT_DHCP_SERVER,
                app: UdpApp::Dhcp(Box::new(msg)),
            },
        };
        let noun = self.noun();
        self.ip_send_on(fx, iface, packet, None, || match op {
            DhcpOp::Discover => format!("The {noun} has no address on {}: it broadcasts a DHCPDISCOVER to find a server.", iface.short()),
            DhcpOp::Request => format!("The {noun} broadcasts a DHCPREQUEST for the address it was offered, so every server hears which offer it took."),
            _ => format!("The {noun} sends a {}.", op.name()),
        });
    }

    /// Start, keep or stop the DHCP client on every interface.
    pub(crate) fn dhcp_client_sync(&mut self, fx: &mut Fx) {
        let ids: Vec<IfId> = self.cfg.interfaces.keys().copied().collect();
        for id in ids {
            let wanted = self.is_l3(id)
                && self
                    .cfg
                    .iface(id)
                    .is_some_and(|c| c.addr == Some(L3Addr::Dhcp))
                && self.is_up(id);
            let Some(state) = self.ifs.get_mut(&id) else {
                continue;
            };
            match (wanted, state.dhcp.state) {
                (true, ClientState::Idle) => {
                    state.dhcp.token = state.dhcp.token.wrapping_add(1);
                    state.dhcp.attempts = 0;
                    state.dhcp.state = ClientState::Selecting;
                    let token = state.dhcp.token;
                    // A moment for the link (and spanning tree) to come up.
                    fx.arm(100_000, Timer::DhcpClient { iface: id, token });
                }
                (false, s) if s != ClientState::Idle => state.dhcp.release(),
                _ => {}
            }
        }
    }

    /// Keep asking for the default gateway's MAC address until it answers.
    pub(crate) fn dhcp_gateway_probe(&mut self, fx: &mut Fx, iface: IfId, token: u32, left: u8) {
        let Some(client) = self
            .ifs
            .get(&iface)
            .map(|state| &state.dhcp)
            .filter(|client| client.token == token)
        else {
            return;
        };
        let Some(gateway) = client.lease.as_ref().and_then(|lease| lease.gateway) else {
            return;
        };
        if left == 0 || self.arp.contains(gateway) {
            return;
        }
        self.arp_resolve(fx, iface, gateway, &Meta::default());
        fx.arm(
            GATEWAY_PROBE_MICROS,
            Timer::GatewayProbe {
                iface,
                token,
                left: left - 1,
            },
        );
    }

    pub(crate) fn dhcp_client_timer(&mut self, fx: &mut Fx, iface: IfId, token: u32) {
        let seed = fx.seed;
        let Some(state) = self.ifs.get_mut(&iface) else {
            return;
        };
        if state.dhcp.token != token {
            return;
        }
        let client = &mut state.dhcp;
        match client.state {
            ClientState::Idle => {}
            ClientState::Selecting | ClientState::Requesting => {
                // (Re)start discovery, backing off 1, 2, 4 ... seconds, at most 16.
                client.state = ClientState::Selecting;
                client.attempts += 1;
                client.xid =
                    rng::hash(&[seed, u64::from(self.id), u64::from(client.attempts), fx.now])
                        as u32;
                let delay = 1_000_000u64 << client.attempts.clamp(1, 5).saturating_sub(1);
                fx.arm(delay, Timer::DhcpClient { iface, token });
                self.dhcp_client_send(fx, iface, DhcpOp::Discover, None, None);
            }
            ClientState::Bound => {
                // Half the lease is gone: ask the server to extend it.
                let Some(lease) = client.lease.clone() else {
                    return;
                };
                client.state = ClientState::Renewing;
                client.xid = client.xid.wrapping_add(1);
                let remaining = lease
                    .expires
                    .map_or(60_000_000, |t| t.saturating_sub(fx.now));
                fx.arm(remaining.max(1_000_000), Timer::DhcpClient { iface, token });
                self.dhcp_client_send(
                    fx,
                    iface,
                    DhcpOp::Request,
                    Some(lease.addr.ip),
                    Some(lease.server),
                );
            }
            ClientState::Renewing => {
                // The lease ran out without an answer.
                client.release();
                fx.dirty = true;
            }
        }
    }

    fn dhcp_client_rx(
        &mut self,
        fx: &mut Fx,
        iface: IfId,
        packet: &Packet,
        msg: &DhcpMsg,
        meta: &Meta,
        l2: (Mac, Mac),
    ) {
        let noun = self.noun();
        let mine = msg.client_mac == self.mac_for(iface);
        let Some(state) = self
            .ifs
            .get_mut(&iface)
            .filter(|s| mine && s.dhcp.xid == msg.xid)
        else {
            return;
        };
        let client = &mut state.dhcp;
        let token = client.token;
        match (msg.op, client.state) {
            (DhcpOp::Offer, ClientState::Selecting) => {
                client.state = ClientState::Requesting;
                self.ip_note(
                    fx,
                    PacketEventKind::Delivered,
                    packet,
                    meta,
                    l2,
                    Some(iface),
                    None,
                    || {
                        format!(
                            "The {noun} accepts the first offer it hears: {}/{}.",
                            msg.your_ip, msg.mask_len
                        )
                    },
                );
                self.dhcp_client_send(fx, iface, DhcpOp::Request, Some(msg.your_ip), msg.server);
            }
            (DhcpOp::Ack, ClientState::Requesting | ClientState::Renewing) => {
                let fresh = client.lease.is_none();
                client.state = ClientState::Bound;
                client.attempts = 0;
                client.lease = Some(Lease {
                    addr: IfAddr::new(msg.your_ip, msg.mask_len),
                    server: msg.server.unwrap_or(packet.src),
                    gateway: msg.routers.first().copied(),
                    dns: msg.dns.clone(),
                    obtained: fx.now,
                    expires: msg.lease_s.map(|s| fx.now + s * 1_000_000),
                });
                if let Some(seconds) = msg.lease_s {
                    fx.arm(seconds * 500_000, Timer::DhcpClient { iface, token });
                }
                fx.dirty = true;
                fx.cp_change = true;
                self.ip_note(
                    fx,
                    PacketEventKind::Delivered,
                    packet,
                    meta,
                    l2,
                    Some(iface),
                    None,
                    || {
                        format!(
                            "The server confirms the lease: the {noun} configures {}/{} on {}{}.",
                            msg.your_ip,
                            msg.mask_len,
                            iface.short(),
                            msg.routers
                                .first()
                                .map(|g| format!(" with default gateway {g}"))
                                .unwrap_or_default()
                        )
                    },
                );
                if fresh {
                    let mask = crate::util::net::mask_from_len(msg.mask_len);
                    let text = format!(
                        "Interface {} assigned DHCP address {}, mask {mask}, hostname {}",
                        iface.name(),
                        msg.your_ip,
                        self.hostname()
                    );
                    self.log(fx, 6, "DHCP", "ADDRESS_ASSIGN", text);
                }
                // A freshly configured host looks for its default gateway at once,
                // and again a few times if nobody answers yet.
                if let (true, Some(gateway)) = (fresh, msg.routers.first()) {
                    self.arp_resolve(fx, iface, *gateway, meta);
                    fx.arm(
                        GATEWAY_PROBE_MICROS,
                        Timer::GatewayProbe {
                            iface,
                            token,
                            left: GATEWAY_PROBES,
                        },
                    );
                }
            }
            (DhcpOp::Nak, ClientState::Requesting | ClientState::Renewing) => {
                client.release();
                client.state = ClientState::Selecting;
                fx.arm(
                    1_000_000,
                    Timer::DhcpClient {
                        iface,
                        token: client.token,
                    },
                );
                fx.dirty = true;
            }
            _ => {}
        }
    }

    /// Drop every lease and start over (`ipconfig /release`, `/renew`).
    pub(crate) fn dhcp_release_all(&mut self, fx: &mut Fx) {
        let ids: Vec<IfId> = self
            .ifs
            .iter()
            .filter(|(_, s)| s.dhcp.lease.is_some())
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            let server = self
                .ifs
                .get(&id)
                .and_then(|s| s.dhcp.lease.as_ref())
                .map(|l| l.server);
            self.dhcp_client_send(fx, id, DhcpOp::Release, None, server);
            if let Some(state) = self.ifs.get_mut(&id) {
                state.dhcp.release();
            }
        }
        fx.dirty = true;
    }

    pub(crate) fn dhcp_server_of(&self, iface: IfId) -> Option<Ipv4Addr> {
        self.ifs.get(&iface)?.dhcp.lease.as_ref().map(|l| l.server)
    }

    /// Still trying eagerly to get a first address?
    pub(crate) fn dhcp_converging(&self) -> bool {
        self.ifs.values().any(|s| {
            matches!(
                s.dhcp.state,
                ClientState::Selecting | ClientState::Requesting
            ) && s.dhcp.attempts < EAGER_ATTEMPTS
        })
    }

    /// Forget bindings for pools that no longer exist.
    pub(crate) fn dhcp_server_sync(&mut self) {
        if self.dhcpd.bindings.is_empty() {
            return;
        }
        let pools: Vec<(String, Option<Prefix>)> = self
            .dhcp_pools()
            .into_iter()
            .map(|(p, _)| (p.name, p.network))
            .collect();
        self.dhcpd.bindings.retain(|b| {
            pools
                .iter()
                .any(|(name, net)| *name == b.pool && net.is_some_and(|n| n.contains(b.ip)))
        });
    }

    pub(crate) fn dhcp_clear_bindings(&mut self, only: Option<Ipv4Addr>) {
        self.dhcpd
            .bindings
            .retain(|b| only.is_some_and(|ip| ip != b.ip));
    }

    /// Committed bindings that have not expired.
    pub(crate) fn dhcp_bindings(&self, now: SimTime) -> Vec<&Binding> {
        self.dhcpd
            .bindings
            .iter()
            .filter(|b| !b.tentative && b.expires.is_none_or(|t| t > now))
            .collect()
    }

    pub(crate) fn dhcp_rows(&self, now: SimTime) -> Vec<DhcpRow> {
        self.dhcp_bindings(now)
            .into_iter()
            .map(|b| DhcpRow {
                ip: b.ip.to_string(),
                mac: b.mac.to_string(),
                pool: b.pool.clone(),
                expires_s: b
                    .expires
                    .map_or(u64::MAX, |t| t.saturating_sub(now) / 1_000_000),
                kind: "Automatic".to_string(),
            })
            .collect()
    }
}
