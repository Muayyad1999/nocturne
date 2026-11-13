//! What travels on the simulated wire: Ethernet frames and their payloads,
//! plus the human-readable breakdown shown by the packet inspector.

use std::net::Ipv4Addr;

use crate::api::{LayerInfo, TraceId};
use crate::util::mac::Mac;
use crate::util::net::Prefix;

/// Simulation bookkeeping carried with a frame; not part of the "real" bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Meta {
    /// Identity of the PDU, kept across forwarding, routing and translation.
    pub(crate) pdu: u64,
    pub(crate) trace: Option<TraceId>,
    /// `seq` of the last packet event recorded for this frame (0 = none).
    pub(crate) via: u32,
    /// Bridges crossed; bounds the damage of a layer-2 loop.
    pub(crate) hops: u8,
}

#[derive(Clone, Debug)]
pub(crate) struct Frame {
    pub(crate) src: Mac,
    pub(crate) dst: Mac,
    /// 802.1Q VLAN tag, if any.
    pub(crate) tag: Option<u16>,
    pub(crate) body: Body,
    pub(crate) meta: Meta,
}

#[derive(Clone, Debug)]
pub(crate) enum Body {
    Arp(Arp),
    Ip(Packet),
    Bpdu(Bpdu),
    /// CDP or LLDP advertisement.
    Advert(Box<Advert>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Arp {
    pub(crate) request: bool,
    pub(crate) sender_mac: Mac,
    pub(crate) sender_ip: Ipv4Addr,
    pub(crate) target_mac: Mac,
    pub(crate) target_ip: Ipv4Addr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct BridgeId {
    /// Configured priority plus the VLAN id (extended system id).
    pub(crate) priority: u16,
    pub(crate) mac: Mac,
}

impl std::fmt::Display for BridgeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.priority, self.mac)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Bpdu {
    pub(crate) vlan: u16,
    pub(crate) root: BridgeId,
    pub(crate) cost: u32,
    pub(crate) bridge: BridgeId,
    pub(crate) port: u16,
    pub(crate) topology_change: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Advert {
    pub(crate) lldp: bool,
    pub(crate) device: String,
    pub(crate) port: String,
    pub(crate) port_description: String,
    pub(crate) platform: String,
    pub(crate) capabilities: String,
    pub(crate) software: String,
    pub(crate) mgmt_ip: Option<Ipv4Addr>,
    pub(crate) native_vlan: Option<u16>,
    pub(crate) full_duplex: bool,
    pub(crate) chassis: Mac,
    pub(crate) hold_s: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct Packet {
    pub(crate) src: Ipv4Addr,
    pub(crate) dst: Ipv4Addr,
    pub(crate) ttl: u8,
    /// Total IP length in bytes.
    pub(crate) size: u16,
    pub(crate) l4: L4,
}

#[derive(Clone, Debug)]
pub(crate) enum L4 {
    Icmp(Icmp),
    Udp {
        sport: u16,
        dport: u16,
        app: UdpApp,
    },
    Tcp {
        sport: u16,
        dport: u16,
        flags: TcpFlags,
    },
    Ospf(Box<OspfHello>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TcpFlags {
    Syn,
    SynAck,
    Rst,
    /// A segment of an established connection.
    Ack,
}

#[derive(Clone, Debug)]
pub(crate) enum UdpApp {
    Dhcp(Box<DhcpMsg>),
    Rip(Box<RipMsg>),
    Hsrp(HsrpMsg),
    Dns(DnsMsg),
    Syslog,
    /// Traceroute probe or other opaque datagram.
    Data,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Unreach {
    Net,
    Host,
    Port,
    AdminProhibited,
}

/// The header of the packet an ICMP error refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Orig {
    pub(crate) src: Ipv4Addr,
    pub(crate) dst: Ipv4Addr,
    pub(crate) flow: Flow,
}

/// Transport-level identity of a packet (what NAT and ICMP errors key on).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Flow {
    Icmp { id: u16, seq: u16 },
    Udp { sport: u16, dport: u16 },
    Tcp { sport: u16, dport: u16 },
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Icmp {
    EchoRequest { id: u16, seq: u16 },
    EchoReply { id: u16, seq: u16 },
    Unreachable { code: Unreach, orig: Orig },
    TimeExceeded { orig: Orig },
}

impl Icmp {
    pub(crate) fn is_error(self) -> bool {
        matches!(self, Icmp::Unreachable { .. } | Icmp::TimeExceeded { .. })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DhcpOp {
    Discover,
    Offer,
    Request,
    Ack,
    Nak,
    Release,
}

impl DhcpOp {
    pub(crate) fn name(self) -> &'static str {
        match self {
            DhcpOp::Discover => "DHCPDISCOVER",
            DhcpOp::Offer => "DHCPOFFER",
            DhcpOp::Request => "DHCPREQUEST",
            DhcpOp::Ack => "DHCPACK",
            DhcpOp::Nak => "DHCPNAK",
            DhcpOp::Release => "DHCPRELEASE",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DhcpMsg {
    pub(crate) op: DhcpOp,
    pub(crate) xid: u32,
    pub(crate) client_mac: Mac,
    pub(crate) client_ip: Ipv4Addr,
    pub(crate) your_ip: Ipv4Addr,
    /// Relay agent address (0.0.0.0 when not relayed).
    pub(crate) relay: Ipv4Addr,
    pub(crate) server: Option<Ipv4Addr>,
    pub(crate) requested: Option<Ipv4Addr>,
    pub(crate) mask_len: u8,
    pub(crate) routers: Vec<Ipv4Addr>,
    pub(crate) dns: Vec<Ipv4Addr>,
    pub(crate) domain: Option<String>,
    pub(crate) lease_s: Option<u64>,
    pub(crate) hostname: String,
}

#[derive(Clone, Debug)]
pub(crate) struct OspfHello {
    pub(crate) router_id: Ipv4Addr,
    pub(crate) area: u32,
    pub(crate) mask_len: u8,
    pub(crate) hello_s: u16,
    pub(crate) dead_s: u16,
    pub(crate) priority: u8,
    pub(crate) dr: Ipv4Addr,
    pub(crate) bdr: Ipv4Addr,
    pub(crate) neighbors: Vec<Ipv4Addr>,
    pub(crate) mtu: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct RipMsg {
    pub(crate) request: bool,
    pub(crate) version: u8,
    /// Prefix and metric (16 = unreachable).
    pub(crate) entries: Vec<(Prefix, u8)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HsrpMsg {
    pub(crate) group: u16,
    pub(crate) version: u8,
    /// The sender is (or claims to be) the active router.
    pub(crate) active: bool,
    /// The sender is the standby router.
    pub(crate) standby: bool,
    /// The sender is giving up the active role.
    pub(crate) resign: bool,
    pub(crate) priority: u8,
    pub(crate) vip: Ipv4Addr,
}

#[derive(Clone, Debug)]
pub(crate) struct DnsMsg {
    pub(crate) id: u16,
    pub(crate) name: String,
    /// `None` in a query; `Some(None)` = name not found.
    pub(crate) answer: Option<Option<Ipv4Addr>>,
}

pub(crate) const IP_PROTO_ICMP: u8 = 1;
pub(crate) const IP_PROTO_TCP: u8 = 6;
pub(crate) const IP_PROTO_UDP: u8 = 17;
pub(crate) const IP_PROTO_OSPF: u8 = 89;

pub(crate) const PORT_DNS: u16 = 53;
pub(crate) const PORT_DHCP_SERVER: u16 = 67;
pub(crate) const PORT_DHCP_CLIENT: u16 = 68;
pub(crate) const PORT_SYSLOG: u16 = 514;
pub(crate) const PORT_RIP: u16 = 520;
pub(crate) const PORT_HSRP: u16 = 1985;
pub(crate) const PORT_TRACEROUTE: u16 = 33434;

pub(crate) const MAC_PVST: Mac = Mac([0x01, 0x00, 0x0c, 0xcc, 0xcc, 0xcd]);
pub(crate) const MAC_CDP: Mac = Mac([0x01, 0x00, 0x0c, 0xcc, 0xcc, 0xcc]);
pub(crate) const MAC_LLDP: Mac = Mac([0x01, 0x80, 0xc2, 0x00, 0x00, 0x0e]);

/// The Ethernet group address an IPv4 multicast group maps to.
pub(crate) fn multicast_mac(group: Ipv4Addr) -> Mac {
    let o = group.octets();
    Mac([0x01, 0x00, 0x5e, o[1] & 0x7f, o[2], o[3]])
}

impl Packet {
    pub(crate) fn proto(&self) -> u8 {
        match self.l4 {
            L4::Icmp(_) => IP_PROTO_ICMP,
            L4::Udp { .. } => IP_PROTO_UDP,
            L4::Tcp { .. } => IP_PROTO_TCP,
            L4::Ospf(_) => IP_PROTO_OSPF,
        }
    }

    pub(crate) fn flow(&self) -> Flow {
        match &self.l4 {
            L4::Icmp(Icmp::EchoRequest { id, seq } | Icmp::EchoReply { id, seq }) => {
                Flow::Icmp { id: *id, seq: *seq }
            }
            L4::Udp { sport, dport, .. } => Flow::Udp {
                sport: *sport,
                dport: *dport,
            },
            L4::Tcp { sport, dport, .. } => Flow::Tcp {
                sport: *sport,
                dport: *dport,
            },
            L4::Icmp(_) | L4::Ospf(_) => Flow::Other,
        }
    }

    /// What an ICMP error about this packet would quote.
    pub(crate) fn orig(&self) -> Orig {
        Orig {
            src: self.src,
            dst: self.dst,
            flow: self.flow(),
        }
    }

    pub(crate) fn is_icmp_error(&self) -> bool {
        matches!(self.l4, L4::Icmp(icmp) if icmp.is_error())
    }

    /// One-line description, e.g. `ICMP echo request 10.0.0.2 → 10.0.1.5`.
    pub(crate) fn summary(&self) -> String {
        let ends = format!("{} → {}", self.src, self.dst);
        match &self.l4 {
            L4::Icmp(Icmp::EchoRequest { .. }) => format!("ICMP echo request {ends}"),
            L4::Icmp(Icmp::EchoReply { .. }) => format!("ICMP echo reply {ends}"),
            L4::Icmp(Icmp::Unreachable { code, .. }) => {
                format!("ICMP {} {ends}", unreach_name(*code))
            }
            L4::Icmp(Icmp::TimeExceeded { .. }) => format!("ICMP TTL exceeded {ends}"),
            L4::Udp { sport, dport, app } => match app {
                UdpApp::Dhcp(msg) => format!("{} {ends}", msg.op.name()),
                UdpApp::Rip(msg) => format!(
                    "RIPv2 {} {ends}",
                    if msg.request { "request" } else { "response" }
                ),
                UdpApp::Hsrp(msg) => format!("HSRP hello group {} {ends}", msg.group),
                UdpApp::Dns(msg) => {
                    format!(
                        "DNS {} {} {ends}",
                        if msg.answer.is_some() {
                            "response"
                        } else {
                            "query"
                        },
                        msg.name
                    )
                }
                UdpApp::Syslog => format!("Syslog message {ends}"),
                UdpApp::Data => format!("UDP {sport} → {dport}, {ends}"),
            },
            L4::Tcp {
                sport,
                dport,
                flags,
            } => format!("TCP {} {sport} → {dport}, {ends}", tcp_flag_name(*flags)),
            L4::Ospf(_) => format!("OSPF hello {ends}"),
        }
    }

    /// Protocol label used by the event list.
    pub(crate) fn protocol(&self) -> &'static str {
        match &self.l4 {
            L4::Icmp(_) => "ICMP",
            L4::Udp { app, .. } => match app {
                UdpApp::Dhcp(_) => "DHCP",
                UdpApp::Rip(_) => "RIP",
                UdpApp::Hsrp(_) => "HSRP",
                UdpApp::Dns(_) => "DNS",
                UdpApp::Syslog => "SYSLOG",
                UdpApp::Data => "UDP",
            },
            L4::Tcp { .. } => "TCP",
            L4::Ospf(_) => "OSPF",
        }
    }
}

pub(crate) fn unreach_name(code: Unreach) -> &'static str {
    match code {
        Unreach::Net => "network unreachable",
        Unreach::Host => "host unreachable",
        Unreach::Port => "port unreachable",
        Unreach::AdminProhibited => "administratively prohibited",
    }
}

fn tcp_flag_name(flags: TcpFlags) -> &'static str {
    match flags {
        TcpFlags::Syn => "SYN",
        TcpFlags::SynAck => "SYN-ACK",
        TcpFlags::Rst => "RST",
        TcpFlags::Ack => "ACK",
    }
}

impl Frame {
    /// Bytes on the wire, for interface counters.
    pub(crate) fn wire_len(&self) -> u64 {
        let payload = match &self.body {
            Body::Arp(_) => 46,
            Body::Ip(packet) => u64::from(packet.size).max(46),
            Body::Bpdu(_) => 46,
            Body::Advert(_) => 300,
        };
        payload + 18 + if self.tag.is_some() { 4 } else { 0 }
    }

    pub(crate) fn protocol(&self) -> &'static str {
        match &self.body {
            Body::Arp(_) => "ARP",
            Body::Ip(packet) => packet.protocol(),
            Body::Bpdu(_) => "STP",
            Body::Advert(advert) => {
                if advert.lldp {
                    "LLDP"
                } else {
                    "CDP"
                }
            }
        }
    }

    pub(crate) fn summary(&self) -> String {
        match &self.body {
            Body::Arp(arp) if arp.request => {
                format!(
                    "ARP request: who has {}? Tell {}",
                    arp.target_ip, arp.sender_ip
                )
            }
            Body::Arp(arp) => format!("ARP reply: {} is at {}", arp.sender_ip, arp.sender_mac),
            Body::Ip(packet) => packet.summary(),
            Body::Bpdu(bpdu) => format!(
                "STP BPDU VLAN {}: root {} cost {} from bridge {}",
                bpdu.vlan, bpdu.root, bpdu.cost, bpdu.bridge
            ),
            Body::Advert(advert) => format!(
                "{} advertisement from {} port {}",
                if advert.lldp { "LLDP" } else { "CDP" },
                advert.device,
                advert.port
            ),
        }
    }

    /// OSI-style breakdown for the packet inspector.
    pub(crate) fn layers(&self) -> Vec<LayerInfo> {
        let mut layers = Vec::new();
        let dst_kind = if self.dst.is_broadcast() {
            " (broadcast)"
        } else if self.dst.is_multicast() {
            " (multicast)"
        } else {
            ""
        };
        let ethertype = match &self.body {
            Body::Arp(_) => "0x0806 (ARP)",
            Body::Ip(_) => "0x0800 (IPv4)",
            Body::Bpdu(_) => "802.3 LLC/SNAP (spanning tree)",
            Body::Advert(advert) if advert.lldp => "0x88CC (LLDP)",
            Body::Advert(_) => "802.3 LLC/SNAP (CDP)",
        };
        layers.push(LayerInfo {
            layer: 2,
            title: "Ethernet II".to_string(),
            lines: vec![
                format!("Source MAC: {}", self.src),
                format!("Destination MAC: {}{dst_kind}", self.dst),
                match self.tag {
                    Some(vlan) => format!("802.1Q tag: VLAN {vlan}"),
                    None => "802.1Q tag: none (untagged)".to_string(),
                },
                format!("Type: {ethertype}"),
            ],
        });
        match &self.body {
            Body::Arp(arp) => layers.push(LayerInfo {
                layer: 2,
                title: "ARP".to_string(),
                lines: vec![
                    format!(
                        "Operation: {}",
                        if arp.request {
                            "request (1)"
                        } else {
                            "reply (2)"
                        }
                    ),
                    format!("Sender: {} at {}", arp.sender_ip, arp.sender_mac),
                    format!(
                        "Target: {} at {}",
                        arp.target_ip,
                        if arp.target_mac == Mac::ZERO {
                            "unknown".to_string()
                        } else {
                            arp.target_mac.to_string()
                        }
                    ),
                ],
            }),
            Body::Bpdu(bpdu) => layers.push(LayerInfo {
                layer: 2,
                title: "Spanning tree BPDU".to_string(),
                lines: vec![
                    format!("VLAN: {}", bpdu.vlan),
                    format!("Root bridge: {}", bpdu.root),
                    format!("Root path cost: {}", bpdu.cost),
                    format!("Sender bridge: {}", bpdu.bridge),
                    format!("Sender port: {:#06x}", bpdu.port),
                    format!(
                        "Topology change: {}",
                        if bpdu.topology_change { "yes" } else { "no" }
                    ),
                ],
            }),
            Body::Advert(advert) => layers.push(LayerInfo {
                layer: 2,
                title: if advert.lldp { "LLDP" } else { "CDP" }.to_string(),
                lines: vec![
                    format!("Device: {}", advert.device),
                    format!("Port: {}", advert.port),
                    format!("Platform: {}", advert.platform),
                    format!("Capabilities: {}", advert.capabilities),
                    format!("Hold time: {} s", advert.hold_s),
                ],
            }),
            Body::Ip(packet) => packet.push_layers(&mut layers),
        }
        layers
    }
}

impl Packet {
    fn push_layers(&self, layers: &mut Vec<LayerInfo>) {
        let proto_name = match self.proto() {
            IP_PROTO_ICMP => "ICMP (1)",
            IP_PROTO_TCP => "TCP (6)",
            IP_PROTO_UDP => "UDP (17)",
            _ => "OSPF (89)",
        };
        layers.push(LayerInfo {
            layer: 3,
            title: "IPv4".to_string(),
            lines: vec![
                format!("Source IP: {}", self.src),
                format!("Destination IP: {}", self.dst),
                format!("TTL: {}", self.ttl),
                format!("Protocol: {proto_name}"),
                format!("Total length: {} bytes", self.size),
            ],
        });
        match &self.l4 {
            L4::Icmp(icmp) => {
                let lines = match icmp {
                    Icmp::EchoRequest { id, seq } => {
                        vec![
                            "Type: 8 (echo request)".to_string(),
                            format!("Identifier: {id}"),
                            format!("Sequence: {seq}"),
                        ]
                    }
                    Icmp::EchoReply { id, seq } => {
                        vec![
                            "Type: 0 (echo reply)".to_string(),
                            format!("Identifier: {id}"),
                            format!("Sequence: {seq}"),
                        ]
                    }
                    Icmp::Unreachable { code, orig } => vec![
                        format!("Type: 3 (destination unreachable), {}", unreach_name(*code)),
                        format!("About: {} → {}", orig.src, orig.dst),
                    ],
                    Icmp::TimeExceeded { orig } => vec![
                        "Type: 11 (time exceeded), TTL expired in transit".to_string(),
                        format!("About: {} → {}", orig.src, orig.dst),
                    ],
                };
                layers.push(LayerInfo {
                    layer: 3,
                    title: "ICMP".to_string(),
                    lines,
                });
            }
            L4::Udp { sport, dport, app } => {
                layers.push(LayerInfo {
                    layer: 4,
                    title: "UDP".to_string(),
                    lines: vec![
                        format!("Source port: {sport}"),
                        format!("Destination port: {dport}"),
                    ],
                });
                let app_layer = match app {
                    UdpApp::Dhcp(msg) => Some(("DHCP", {
                        let mut lines = vec![
                            format!("Message: {}", msg.op.name()),
                            format!("Transaction: {:#010x}", msg.xid),
                            format!("Client MAC: {}", msg.client_mac),
                        ];
                        if !msg.your_ip.is_unspecified() {
                            lines.push(format!("Your address: {}/{}", msg.your_ip, msg.mask_len));
                        }
                        if !msg.relay.is_unspecified() {
                            lines.push(format!("Relay agent: {}", msg.relay));
                        }
                        if let Some(router) = msg.routers.first() {
                            lines.push(format!("Default gateway: {router}"));
                        }
                        lines
                    })),
                    UdpApp::Rip(msg) => Some(("RIPv2", {
                        let mut lines = vec![format!(
                            "Command: {}",
                            if msg.request { "request" } else { "response" }
                        )];
                        lines.extend(
                            msg.entries
                                .iter()
                                .take(8)
                                .map(|(p, m)| format!("{p} metric {m}")),
                        );
                        lines
                    })),
                    UdpApp::Hsrp(msg) => Some((
                        "HSRP",
                        vec![
                            format!("Group: {}", msg.group),
                            format!("Priority: {}", msg.priority),
                            format!("Virtual IP: {}", msg.vip),
                            format!(
                                "State: {}",
                                if msg.resign {
                                    "resign"
                                } else if msg.active {
                                    "active"
                                } else if msg.standby {
                                    "standby"
                                } else {
                                    "speak"
                                }
                            ),
                        ],
                    )),
                    UdpApp::Dns(msg) => Some((
                        "DNS",
                        vec![match msg.answer {
                            None => format!("Query: A {}", msg.name),
                            Some(Some(ip)) => format!("Answer: {} is {ip}", msg.name),
                            Some(None) => format!("Answer: {} does not exist", msg.name),
                        }],
                    )),
                    UdpApp::Syslog | UdpApp::Data => None,
                };
                if let Some((title, lines)) = app_layer {
                    layers.push(LayerInfo {
                        layer: 7,
                        title: title.to_string(),
                        lines,
                    });
                }
            }
            L4::Tcp {
                sport,
                dport,
                flags,
            } => layers.push(LayerInfo {
                layer: 4,
                title: "TCP".to_string(),
                lines: vec![
                    format!("Source port: {sport}"),
                    format!("Destination port: {dport}"),
                    format!("Flags: {}", tcp_flag_name(*flags)),
                ],
            }),
            L4::Ospf(hello) => layers.push(LayerInfo {
                layer: 3,
                title: "OSPF hello".to_string(),
                lines: vec![
                    format!("Router ID: {}", hello.router_id),
                    format!("Area: {}", Ipv4Addr::from(hello.area)),
                    format!("Hello/dead interval: {}/{} s", hello.hello_s, hello.dead_s),
                    format!("Designated router: {}", hello.dr),
                    format!("Neighbours seen: {}", hello.neighbors.len()),
                ],
            }),
        }
    }
}
