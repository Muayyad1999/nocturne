//! Registered numbers: IP protocols, EtherTypes, ICMP types.

use serde::{Deserialize, Serialize};

/// A number with the name it is registered under.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct NamedNumber {
    /// The number as it appears on the wire.
    pub number: u32,
    /// The registered keyword or short name.
    pub name: String,
    /// What it is.
    pub description: String,
}

fn named(table: &[(u32, &str, &str)]) -> Vec<NamedNumber> {
    table
        .iter()
        .map(|(number, name, description)| NamedNumber {
            number: *number,
            name: (*name).to_owned(),
            description: (*description).to_owned(),
        })
        .collect()
}

/// From the IANA "Assigned Internet Protocol Numbers" registry.
#[rustfmt::skip]
const IP_PROTOCOLS: &[(u32, &str, &str)] = &[
    (0, "HOPOPT", "IPv6 Hop-by-Hop Options header"),
    (1, "ICMP", "Internet Control Message Protocol"),
    (2, "IGMP", "Internet Group Management Protocol"),
    (4, "IPv4", "IPv4 encapsulated in IP (IP-in-IP tunnels)"),
    (6, "TCP", "Transmission Control Protocol"),
    (8, "EGP", "Exterior Gateway Protocol (historic)"),
    (9, "IGP", "Any private interior gateway protocol; Cisco used it for IGRP"),
    (17, "UDP", "User Datagram Protocol"),
    (27, "RDP", "Reliable Data Protocol"),
    (33, "DCCP", "Datagram Congestion Control Protocol"),
    (41, "IPv6", "IPv6 encapsulated in IP (6in4, 6to4, 6rd, ISATAP)"),
    (43, "IPv6-Route", "IPv6 Routing header (including the Segment Routing header)"),
    (44, "IPv6-Frag", "IPv6 Fragment header"),
    (46, "RSVP", "Resource Reservation Protocol (also RSVP-TE)"),
    (47, "GRE", "Generic Routing Encapsulation"),
    (50, "ESP", "IPsec Encapsulating Security Payload"),
    (51, "AH", "IPsec Authentication Header"),
    (58, "IPv6-ICMP", "ICMP for IPv6 (including Neighbor Discovery and MLD)"),
    (59, "IPv6-NoNxt", "IPv6: no next header"),
    (60, "IPv6-Opts", "IPv6 Destination Options header"),
    (88, "EIGRP", "Enhanced Interior Gateway Routing Protocol"),
    (89, "OSPFIGP", "OSPF, versions 2 and 3"),
    (94, "IPIP", "IP-within-IP encapsulation (Cisco `nos` tunnels)"),
    (97, "ETHERIP", "Ethernet within IP (EoIP; Cisco wireless mobility tunnels)"),
    (98, "ENCAP", "Encapsulation header (RFC 1241)"),
    (103, "PIM", "Protocol Independent Multicast"),
    (108, "IPComp", "IP Payload Compression Protocol"),
    (112, "VRRP", "Virtual Router Redundancy Protocol (also used by CARP)"),
    (113, "PGM", "Pragmatic General Multicast"),
    (115, "L2TP", "Layer 2 Tunnelling Protocol version 3 directly over IP"),
    (124, "ISIS over IPv4", "IS-IS over IPv4 (rare; IS-IS normally runs directly over the data link)"),
    (132, "SCTP", "Stream Control Transmission Protocol"),
    (135, "Mobility Header", "Mobile IPv6"),
    (136, "UDPLite", "Lightweight User Datagram Protocol"),
    (137, "MPLS-in-IP", "MPLS encapsulated in IP"),
    (139, "HIP", "Host Identity Protocol"),
    (140, "Shim6", "Site multihoming by IPv6 intermediation"),
    (143, "Ethernet", "Ethernet frame as the payload of an SRv6 packet"),
    (253, "experimental", "Use for experimentation and testing (RFC 3692)"),
    (254, "experimental", "Use for experimentation and testing (RFC 3692)"),
    (255, "Reserved", "Reserved"),
];

/// From the IEEE Registration Authority's EtherType list and the IANA
/// "IEEE 802 Numbers" registry.
#[rustfmt::skip]
const ETHERTYPES: &[(u32, &str, &str)] = &[
    (0x0800, "IPv4", "Internet Protocol version 4"),
    (0x0806, "ARP", "Address Resolution Protocol"),
    (0x0842, "Wake-on-LAN", "Wake-on-LAN magic packet (when sent directly over Ethernet)"),
    (0x22F3, "TRILL", "Transparent Interconnection of Lots of Links"),
    (0x6002, "DEC MOP RC", "DEC Maintenance Operation Protocol, remote console (still enabled by default on some Cisco interfaces)"),
    (0x6003, "DECnet", "DECnet Phase IV routing"),
    (0x8035, "RARP", "Reverse Address Resolution Protocol"),
    (0x809B, "AppleTalk", "AppleTalk (EtherTalk)"),
    (0x80F3, "AARP", "AppleTalk Address Resolution Protocol"),
    (0x8100, "802.1Q", "VLAN tag (C-Tag), also used for priority-tagged frames"),
    (0x8137, "IPX", "Novell IPX"),
    (0x86DD, "IPv6", "Internet Protocol version 6"),
    (0x8808, "MAC Control", "Ethernet flow control: PAUSE and priority flow control (PFC)"),
    (0x8809, "Slow Protocols", "LACP, the LACP marker protocol, link OAM (802.3ah) and ESMC"),
    (0x8847, "MPLS", "MPLS unicast"),
    (0x8848, "MPLS multicast", "MPLS with an upstream-assigned label"),
    (0x8863, "PPPoE Discovery", "PPP over Ethernet, discovery stage"),
    (0x8864, "PPPoE Session", "PPP over Ethernet, session stage"),
    (0x888E, "EAPOL", "EAP over LAN (802.1X port authentication, MACsec key agreement)"),
    (0x8892, "PROFINET", "PROFINET real-time industrial Ethernet"),
    (0x88A2, "AoE", "ATA over Ethernet"),
    (0x88A4, "EtherCAT", "EtherCAT industrial Ethernet"),
    (0x88A8, "802.1ad", "Service VLAN tag (S-Tag) of provider bridging (QinQ)"),
    (0x88B5, "Local Experimental 1", "IEEE 802 local experimental EtherType"),
    (0x88B6, "Local Experimental 2", "IEEE 802 local experimental EtherType"),
    (0x88B7, "OUI Extended", "IEEE 802 OUI-extended EtherType"),
    (0x88B8, "GOOSE", "IEC 61850 GOOSE (substation automation)"),
    (0x88BA, "SV", "IEC 61850 Sampled Values"),
    (0x88CC, "LLDP", "Link Layer Discovery Protocol"),
    (0x88E1, "HomePlug AV", "HomePlug AV powerline management"),
    (0x88E3, "MRP", "Media Redundancy Protocol (IEC 62439-2)"),
    (0x88E5, "MACsec", "IEEE 802.1AE MAC security"),
    (0x88E7, "PBB", "Provider Backbone Bridging I-Tag (802.1ah)"),
    (0x88F5, "MVRP", "Multiple VLAN Registration Protocol"),
    (0x88F6, "MMRP", "Multiple MAC Registration Protocol"),
    (0x88F7, "PTP", "Precision Time Protocol (IEEE 1588) directly over Ethernet"),
    (0x88F8, "NC-SI", "Network Controller Sideband Interface"),
    (0x88FB, "PRP", "Parallel Redundancy Protocol"),
    (0x8902, "CFM", "Connectivity Fault Management (802.1ag) and Y.1731 OAM"),
    (0x8906, "FCoE", "Fibre Channel over Ethernet"),
    (0x8914, "FIP", "FCoE Initialization Protocol"),
    (0x8915, "RoCE", "RDMA over Converged Ethernet (version 1)"),
    (0x892F, "HSR", "High-availability Seamless Redundancy"),
    (0x893F, "802.1BR", "Bridge port extension E-Tag"),
    (0x894F, "NSH", "Network Service Header"),
    (0x9000, "Loopback", "Ethernet configuration test protocol (Cisco keepalive loopback frames)"),
    (0x9100, "QinQ (legacy)", "Pre-standard outer VLAN tag still configurable on many platforms"),
    (0xF1C1, "802.1CB", "Frame replication and elimination redundancy tag"),
];

/// From the IANA "ICMP Type Numbers" registry (ICMP for IPv4).
#[rustfmt::skip]
const ICMP_TYPES: &[(u32, &str, &str)] = &[
    (0, "Echo Reply", "Answer to an echo request (ping)"),
    (3, "Destination Unreachable", "Codes: 0 network, 1 host, 2 protocol, 3 port unreachable; 4 fragmentation needed and DF set (path MTU discovery); 5 source route failed; 9, 10, 13 administratively prohibited"),
    (4, "Source Quench", "Deprecated congestion signal (RFC 6633)"),
    (5, "Redirect", "Use another gateway. Codes: 0 network, 1 host, 2 ToS and network, 3 ToS and host"),
    (8, "Echo Request", "Ping"),
    (9, "Router Advertisement", "ICMP router discovery (RFC 1256)"),
    (10, "Router Solicitation", "ICMP router discovery (RFC 1256)"),
    (11, "Time Exceeded", "Codes: 0 TTL exceeded in transit (what traceroute relies on), 1 fragment reassembly time exceeded"),
    (12, "Parameter Problem", "Bad IP header. Codes: 0 pointer indicates the error, 1 missing a required option, 2 bad length"),
    (13, "Timestamp", "Timestamp request"),
    (14, "Timestamp Reply", "Timestamp reply"),
    (15, "Information Request", "Deprecated (RFC 6918)"),
    (16, "Information Reply", "Deprecated (RFC 6918)"),
    (17, "Address Mask Request", "Deprecated (RFC 6918)"),
    (18, "Address Mask Reply", "Deprecated (RFC 6918)"),
    (30, "Traceroute", "Deprecated (RFC 6918); traceroute uses types 11 and 3 instead"),
    (40, "Photuris", "Security failures (RFC 2521)"),
    (42, "Extended Echo Request", "Probe of an interface on another node (RFC 8335)"),
    (43, "Extended Echo Reply", "Answer to an extended echo request (RFC 8335)"),
];

/// From the IANA "ICMPv6 type Numbers" registry.
#[rustfmt::skip]
const ICMPV6_TYPES: &[(u32, &str, &str)] = &[
    (1, "Destination Unreachable", "Codes: 0 no route, 1 administratively prohibited, 3 address unreachable, 4 port unreachable"),
    (2, "Packet Too Big", "Path MTU discovery; must never be filtered"),
    (3, "Time Exceeded", "Codes: 0 hop limit exceeded in transit, 1 fragment reassembly time exceeded"),
    (4, "Parameter Problem", "Codes: 0 erroneous header field, 1 unrecognised next header, 2 unrecognised option"),
    (128, "Echo Request", "Ping"),
    (129, "Echo Reply", "Answer to an echo request"),
    (130, "Multicast Listener Query", "MLD query"),
    (131, "Multicast Listener Report", "MLD version 1 report"),
    (132, "Multicast Listener Done", "MLD version 1 done"),
    (133, "Router Solicitation", "Neighbor Discovery: a host asks for router advertisements"),
    (134, "Router Advertisement", "Neighbor Discovery: prefixes, default router, SLAAC and DHCPv6 flags"),
    (135, "Neighbor Solicitation", "Neighbor Discovery: address resolution and duplicate address detection"),
    (136, "Neighbor Advertisement", "Neighbor Discovery: answer to a solicitation"),
    (137, "Redirect", "Neighbor Discovery: a better first hop exists"),
    (138, "Router Renumbering", "Router renumbering (RFC 2894)"),
    (141, "Inverse Neighbor Discovery Solicitation", "Inverse Neighbor Discovery (RFC 3122)"),
    (142, "Inverse Neighbor Discovery Advertisement", "Inverse Neighbor Discovery (RFC 3122)"),
    (143, "Multicast Listener Report v2", "MLD version 2 report"),
    (151, "Multicast Router Advertisement", "Multicast router discovery (RFC 4286)"),
    (152, "Multicast Router Solicitation", "Multicast router discovery (RFC 4286)"),
    (153, "Multicast Router Termination", "Multicast router discovery (RFC 4286)"),
    (155, "RPL Control", "Routing protocol for low-power and lossy networks"),
    (160, "Extended Echo Request", "Probe of an interface on another node (RFC 8335)"),
    (161, "Extended Echo Reply", "Answer to an extended echo request (RFC 8335)"),
];

/// IP protocol numbers worth knowing: the value of the IPv4 Protocol field
/// and of the IPv6 Next Header field.
pub fn ip_protocols() -> Vec<NamedNumber> {
    named(IP_PROTOCOLS)
}

/// EtherTypes worth knowing.
pub fn ethertypes() -> Vec<NamedNumber> {
    named(ETHERTYPES)
}

/// ICMP message types for IPv4, with the codes that matter in their description.
pub fn icmp_types() -> Vec<NamedNumber> {
    named(ICMP_TYPES)
}

/// ICMPv6 message types.
pub fn icmpv6_types() -> Vec<NamedNumber> {
    named(ICMPV6_TYPES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup(table: &[NamedNumber], number: u32) -> &str {
        table
            .iter()
            .find(|entry| entry.number == number)
            .map_or("", |entry| entry.name.as_str())
    }

    fn assert_tidy(table: &[NamedNumber], max: u32) {
        assert!(
            table.windows(2).all(|pair| pair[0].number < pair[1].number),
            "sorted, no duplicates"
        );
        for entry in table {
            assert!(entry.number <= max, "{}", entry.number);
            assert!(
                !entry.name.is_empty()
                    && !entry.description.is_empty()
                    && !entry.description.ends_with('.')
            );
        }
    }

    #[test]
    fn ip_protocol_numbers() {
        let table = ip_protocols();
        assert_tidy(&table, 255);
        for (number, name) in [
            (1, "ICMP"),
            (2, "IGMP"),
            (4, "IPv4"),
            (6, "TCP"),
            (17, "UDP"),
            (41, "IPv6"),
            (46, "RSVP"),
            (47, "GRE"),
            (50, "ESP"),
            (51, "AH"),
            (58, "IPv6-ICMP"),
            (88, "EIGRP"),
            (89, "OSPFIGP"),
            (103, "PIM"),
            (112, "VRRP"),
            (115, "L2TP"),
            (132, "SCTP"),
        ] {
            assert_eq!(lookup(&table, number), name, "{number}");
        }
        assert!(table.len() >= 35);
    }

    #[test]
    fn ethertype_numbers() {
        let table = ethertypes();
        assert_tidy(&table, 0xFFFF);
        // Values below 0x0600 are lengths, not EtherTypes.
        assert!(table.iter().all(|entry| entry.number >= 0x0600));
        for (number, name) in [
            (0x0800, "IPv4"),
            (0x0806, "ARP"),
            (0x8100, "802.1Q"),
            (0x86DD, "IPv6"),
            (0x8809, "Slow Protocols"),
            (0x8847, "MPLS"),
            (0x8863, "PPPoE Discovery"),
            (0x8864, "PPPoE Session"),
            (0x888E, "EAPOL"),
            (0x88A8, "802.1ad"),
            (0x88CC, "LLDP"),
            (0x88E5, "MACsec"),
            (0x88F7, "PTP"),
            (0x8902, "CFM"),
            (0x8906, "FCoE"),
        ] {
            assert_eq!(lookup(&table, number), name, "{number:#06x}");
        }
        assert!(table.len() >= 40);
    }

    #[test]
    fn icmp_type_numbers() {
        let v4 = icmp_types();
        assert_tidy(&v4, 255);
        for (number, name) in [
            (0, "Echo Reply"),
            (3, "Destination Unreachable"),
            (5, "Redirect"),
            (8, "Echo Request"),
            (11, "Time Exceeded"),
            (12, "Parameter Problem"),
        ] {
            assert_eq!(lookup(&v4, number), name, "{number}");
        }
        assert!(
            v4[1]
                .description
                .contains("4 fragmentation needed and DF set")
        );

        let v6 = icmpv6_types();
        assert_tidy(&v6, 255);
        for (number, name) in [
            (1, "Destination Unreachable"),
            (2, "Packet Too Big"),
            (3, "Time Exceeded"),
            (128, "Echo Request"),
            (129, "Echo Reply"),
            (133, "Router Solicitation"),
            (134, "Router Advertisement"),
            (135, "Neighbor Solicitation"),
            (136, "Neighbor Advertisement"),
            (137, "Redirect"),
        ] {
            assert_eq!(lookup(&v6, number), name, "{number}");
        }
    }
}
