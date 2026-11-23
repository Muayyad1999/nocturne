//! The traffic the real-exporter tests are about: five conversations, written out packet by
//! packet as a capture file an exporter can be given to account for.
//!
//! Nothing here was captured from a network. It is written by hand so that the right answer
//! is known: what an exporter reports about it can be compared with [`flows`], which is
//! counted from the very packets [`pcap`] writes.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One direction of one conversation, as an exporter should report it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Flow {
    pub src: String,
    pub dst: String,
    pub src_port: u16,
    /// For ICMP: type * 256 + code.
    pub dst_port: u16,
    pub protocol: u8,
    pub packets: u64,
    /// IP bytes: header and payload, without the Ethernet frame.
    pub bytes: u64,
    pub tos: u8,
}

struct Packet {
    src: IpAddr,
    dst: IpAddr,
    protocol: u8,
    tos: u8,
    /// The transport header and its payload.
    payload: Vec<u8>,
    src_port: u16,
    dst_port: u16,
}

fn tcp(src: &str, sport: u16, dst: &str, dport: u16, flags: u8, data: usize, tos: u8) -> Packet {
    let mut payload = Vec::with_capacity(20 + data);
    payload.extend_from_slice(&sport.to_be_bytes());
    payload.extend_from_slice(&dport.to_be_bytes());
    payload.extend_from_slice(&1000u32.to_be_bytes()); // sequence
    payload.extend_from_slice(&2000u32.to_be_bytes()); // acknowledgement
    payload.extend_from_slice(&[5 << 4, flags]); // header length, flags
    payload.extend_from_slice(&65_535u16.to_be_bytes()); // window
    payload.extend_from_slice(&[0; 4]); // checksum, urgent pointer
    payload.resize(20 + data, b'd');
    Packet {
        src: src.parse().unwrap(),
        dst: dst.parse().unwrap(),
        protocol: 6,
        tos,
        payload,
        src_port: sport,
        dst_port: dport,
    }
}

fn udp(src: &str, sport: u16, dst: &str, dport: u16, data: usize, tos: u8) -> Packet {
    let mut payload = Vec::with_capacity(8 + data);
    payload.extend_from_slice(&sport.to_be_bytes());
    payload.extend_from_slice(&dport.to_be_bytes());
    payload.extend_from_slice(&((8 + data) as u16).to_be_bytes());
    payload.extend_from_slice(&[0; 2]); // no checksum
    payload.resize(8 + data, b'u');
    Packet {
        src: src.parse().unwrap(),
        dst: dst.parse().unwrap(),
        protocol: 17,
        tos,
        payload,
        src_port: sport,
        dst_port: dport,
    }
}

fn echo_request(src: &str, dst: &str) -> Packet {
    let mut payload = vec![8, 0, 0, 0, 0, 1, 0, 1]; // type 8, code 0, id 1, sequence 1
    payload.resize(8 + 32, b'x');
    let checksum = internet_checksum(&payload);
    payload[2..4].copy_from_slice(&checksum.to_be_bytes());
    Packet {
        src: src.parse().unwrap(),
        dst: dst.parse().unwrap(),
        protocol: 1,
        tos: 0,
        payload,
        src_port: 0,
        dst_port: 8 * 256,
    }
}

fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for pair in data.chunks(2) {
        sum += u32::from(pair[0]) << 8 | u32::from(*pair.get(1).unwrap_or(&0));
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn packets() -> Vec<Packet> {
    let mut packets = Vec::new();
    // A download over HTTPS: the server sends twenty full segments, the client acknowledges.
    packets.push(tcp("10.0.0.5", 51_000, "192.0.2.80", 443, 0x02, 0, 0));
    packets.push(tcp("192.0.2.80", 443, "10.0.0.5", 51_000, 0x12, 0, 0));
    for _ in 0..20 {
        packets.push(tcp("192.0.2.80", 443, "10.0.0.5", 51_000, 0x18, 1200, 0));
        packets.push(tcp("10.0.0.5", 51_000, "192.0.2.80", 443, 0x10, 0, 0));
    }
    packets.push(tcp("10.0.0.5", 51_000, "192.0.2.80", 443, 0x11, 0, 0));
    packets.push(tcp("192.0.2.80", 443, "10.0.0.5", 51_000, 0x11, 0, 0));
    // A DNS question and its answer.
    packets.push(udp("10.0.0.5", 40_000, "10.0.0.53", 53, 30, 0));
    packets.push(udp("10.0.0.53", 53, "10.0.0.5", 40_000, 90, 0));
    // Voice, marked EF (ToS byte 0xb8).
    for _ in 0..50 {
        packets.push(udp("10.0.0.7", 16_384, "10.0.0.9", 16_384, 160, 0xb8));
    }
    // A ping nobody answers.
    for _ in 0..3 {
        packets.push(echo_request("10.0.0.5", "10.0.0.1"));
    }
    // SSH over IPv6.
    packets.push(tcp("2001:db8::5", 52_000, "2001:db8::22", 22, 0x02, 0, 0));
    for _ in 0..5 {
        packets.push(tcp("2001:db8::5", 52_000, "2001:db8::22", 22, 0x18, 100, 0));
    }
    packets
}

fn ip_length(packet: &Packet) -> usize {
    match packet.src {
        IpAddr::V4(_) => 20 + packet.payload.len(),
        IpAddr::V6(_) => 40 + packet.payload.len(),
    }
}

/// What the packets add up to, one entry per direction, sorted.
pub fn flows() -> Vec<Flow> {
    let mut flows: Vec<Flow> = Vec::new();
    for packet in packets() {
        let (src, dst) = (packet.src.to_string(), packet.dst.to_string());
        let found = flows.iter_mut().find(|flow| {
            flow.src == src
                && flow.dst == dst
                && flow.src_port == packet.src_port
                && flow.dst_port == packet.dst_port
                && flow.protocol == packet.protocol
        });
        match found {
            Some(flow) => {
                flow.packets += 1;
                flow.bytes += ip_length(&packet) as u64;
            }
            None => flows.push(Flow {
                src,
                dst,
                src_port: packet.src_port,
                dst_port: packet.dst_port,
                protocol: packet.protocol,
                packets: 1,
                bytes: ip_length(&packet) as u64,
                tos: packet.tos,
            }),
        }
    }
    flows.sort();
    flows
}

/// The packets as a capture file (libpcap format, Ethernet link type).
pub fn pcap() -> Vec<u8> {
    let mut file = Vec::new();
    // Magic, version 2.4, no time zone offset, no accuracy, snap length, Ethernet.
    file.extend_from_slice(&0xa1b2_c3d4u32.to_le_bytes());
    file.extend_from_slice(&2u16.to_le_bytes());
    file.extend_from_slice(&4u16.to_le_bytes());
    file.extend_from_slice(&[0; 8]);
    file.extend_from_slice(&65_535u32.to_le_bytes());
    file.extend_from_slice(&1u32.to_le_bytes());
    for (index, packet) in packets().iter().enumerate() {
        let frame = frame(packet);
        // Ten milliseconds apart, starting at a fixed second.
        let micros = index as u64 * 10_000;
        file.extend_from_slice(&(1_763_000_000u32 + (micros / 1_000_000) as u32).to_le_bytes());
        file.extend_from_slice(&((micros % 1_000_000) as u32).to_le_bytes());
        file.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        file.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        file.extend_from_slice(&frame);
    }
    file
}

fn frame(packet: &Packet) -> Vec<u8> {
    let mut frame = vec![2, 0, 0, 0, 0, 2, 2, 0, 0, 0, 0, 1]; // destination and source MAC
    match (packet.src, packet.dst) {
        (IpAddr::V4(src), IpAddr::V4(dst)) => {
            frame.extend_from_slice(&0x0800u16.to_be_bytes());
            frame.extend_from_slice(&ipv4_header(packet, src, dst));
        }
        (IpAddr::V6(src), IpAddr::V6(dst)) => {
            frame.extend_from_slice(&0x86ddu16.to_be_bytes());
            frame.extend_from_slice(&ipv6_header(packet, src, dst));
        }
        _ => unreachable!("a packet has one address family"),
    }
    frame.extend_from_slice(&packet.payload);
    frame
}

fn ipv4_header(packet: &Packet, src: Ipv4Addr, dst: Ipv4Addr) -> Vec<u8> {
    let mut header = vec![0x45, packet.tos];
    header.extend_from_slice(&((20 + packet.payload.len()) as u16).to_be_bytes());
    header.extend_from_slice(&[0, 1, 0, 0, 64, packet.protocol, 0, 0]); // id, no fragment, TTL
    header.extend_from_slice(&src.octets());
    header.extend_from_slice(&dst.octets());
    let checksum = internet_checksum(&header);
    header[10..12].copy_from_slice(&checksum.to_be_bytes());
    header
}

fn ipv6_header(packet: &Packet, src: Ipv6Addr, dst: Ipv6Addr) -> Vec<u8> {
    let mut header = (6u32 << 28 | u32::from(packet.tos) << 20)
        .to_be_bytes()
        .to_vec();
    header.extend_from_slice(&(packet.payload.len() as u16).to_be_bytes());
    header.extend_from_slice(&[packet.protocol, 64]); // next header, hop limit
    header.extend_from_slice(&src.octets());
    header.extend_from_slice(&dst.octets());
    header
}
