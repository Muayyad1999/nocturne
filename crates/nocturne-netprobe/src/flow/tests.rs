//! Decoder tests with export datagrams written out field by field from the specifications:
//! Cisco's NetFlow v5 format, RFC 3954 (v9) and RFC 7011 (IPFIX).

mod captures;
mod traffic;

use std::net::SocketAddr;

use pretty_assertions::assert_eq;

use super::*;

const EXPORTER: &str = "192.0.2.1:2055";

fn exporter() -> SocketAddr {
    EXPORTER.parse().unwrap()
}

/// Builds datagrams from big-endian fields.
#[derive(Default, Clone)]
struct Bytes(Vec<u8>);

impl Bytes {
    fn u8(mut self, value: u8) -> Self {
        self.0.push(value);
        self
    }
    fn u16(mut self, value: u16) -> Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }
    fn u32(mut self, value: u32) -> Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }
    fn u64(mut self, value: u64) -> Self {
        self.0.extend_from_slice(&value.to_be_bytes());
        self
    }
    fn ip4(mut self, address: [u8; 4]) -> Self {
        self.0.extend_from_slice(&address);
        self
    }
    fn ip6(mut self, address: &str) -> Self {
        self.0
            .extend_from_slice(&address.parse::<std::net::Ipv6Addr>().unwrap().octets());
        self
    }
    fn raw(mut self, bytes: &[u8]) -> Self {
        self.0.extend_from_slice(bytes);
        self
    }
    fn len(&self) -> usize {
        self.0.len()
    }
}

/// A v9 flow set / IPFIX set: id, length (header included), content, padded to four bytes.
fn set(id: u16, content: Bytes) -> Bytes {
    let padding = (4 - content.len() % 4) % 4;
    let length = (4 + content.len() + padding) as u16;
    Bytes::default()
        .u16(id)
        .u16(length)
        .raw(&content.0)
        .raw(&vec![0; padding])
}

fn v9_message(source_id: u32, count: u16, sets: &[Bytes]) -> Vec<u8> {
    let mut message = Bytes::default()
        .u16(9)
        .u16(count)
        .u32(86_400_000) // sysUptime: one day
        .u32(1_700_000_000) // export time
        .u32(42) // sequence
        .u32(source_id);
    for set in sets {
        message = message.raw(&set.0);
    }
    message.0
}

fn ipfix_message(domain: u32, sets: &[Bytes]) -> Vec<u8> {
    let body: usize = sets.iter().map(Bytes::len).sum();
    let mut message = Bytes::default()
        .u16(10)
        .u16((16 + body) as u16)
        .u32(1_700_000_000) // export time
        .u32(7) // sequence
        .u32(domain);
    for set in sets {
        message = message.raw(&set.0);
    }
    message.0
}

fn record(version: u8, src: &str, dst: &str) -> FlowRecord {
    FlowRecord {
        exporter: "192.0.2.1".into(),
        version,
        src: src.into(),
        dst: dst.into(),
        src_port: 0,
        dst_port: 0,
        protocol: 0,
        bytes: 0,
        packets: 0,
        tos: 0,
        tcp_flags: 0,
        input_if: 0,
        output_if: 0,
        src_as: 0,
        dst_as: 0,
        next_hop: None,
        start_ms: 0,
        end_ms: 0,
        sampling: 0,
    }
}

// ------------------------------------------------------------------ NetFlow v5

fn v5_datagram() -> Vec<u8> {
    Bytes::default()
        // Header (24 bytes).
        .u16(5) // version
        .u16(2) // count
        .u32(86_400_000) // SysUptime
        .u32(1_700_000_000) // unix_secs
        .u32(0) // unix_nsecs
        .u32(1000) // flow_sequence
        .u8(0) // engine_type
        .u8(0) // engine_id
        .u16(0x4064) // sampling: mode 1, interval 100
        // Record 1 (48 bytes): an HTTPS flow.
        .ip4([10, 0, 0, 1]) // srcaddr
        .ip4([192, 0, 2, 80]) // dstaddr
        .ip4([10, 0, 0, 254]) // nexthop
        .u16(3) // input
        .u16(5) // output
        .u32(10) // dPkts
        .u32(1500) // dOctets
        .u32(80_000) // First
        .u32(85_000) // Last
        .u16(51_234) // srcport
        .u16(443) // dstport
        .u8(0) // pad1
        .u8(0x1b) // tcp_flags: FIN SYN PSH ACK
        .u8(6) // prot
        .u8(0) // tos
        .u16(65_001) // src_as
        .u16(15_169) // dst_as
        .u8(24) // src_mask
        .u8(24) // dst_mask
        .u16(0) // pad2
        // Record 2: a DNS query, expedited forwarding, no next hop.
        .ip4([10, 0, 0, 2])
        .ip4([8, 8, 8, 8])
        .ip4([0, 0, 0, 0])
        .u16(3)
        .u16(5)
        .u32(1)
        .u32(76)
        .u32(84_000)
        .u32(84_000)
        .u16(40_000)
        .u16(53)
        .u8(0)
        .u8(0)
        .u8(17)
        .u8(0xb8)
        .u16(0)
        .u16(0)
        .u8(24)
        .u8(0)
        .u16(0)
        .0
}

#[test]
fn netflow_v5() {
    let datagram = v5_datagram();
    assert_eq!(datagram.len(), 24 + 2 * 48);
    let mut decoder = Decoder::new();
    let records = decoder.decode(&datagram, exporter()).unwrap();
    assert_eq!(
        records,
        vec![
            FlowRecord {
                src_port: 51_234,
                dst_port: 443,
                protocol: 6,
                bytes: 1500,
                packets: 10,
                tcp_flags: 0x1b,
                input_if: 3,
                output_if: 5,
                src_as: 65_001,
                dst_as: 15_169,
                next_hop: Some("10.0.0.254".into()),
                start_ms: 80_000,
                end_ms: 85_000,
                sampling: 100,
                ..record(5, "10.0.0.1", "192.0.2.80")
            },
            FlowRecord {
                src_port: 40_000,
                dst_port: 53,
                protocol: 17,
                bytes: 76,
                packets: 1,
                tos: 0xb8,
                input_if: 3,
                output_if: 5,
                start_ms: 84_000,
                end_ms: 84_000,
                sampling: 100,
                ..record(5, "10.0.0.2", "8.8.8.8")
            },
        ]
    );
    assert_eq!(
        decoder.stats(),
        DecoderStats {
            packets: 1,
            records: 2,
            templates: 0,
            missing_template_sets: 0,
            errors: 0
        }
    );

    // The record count must fit the datagram and the format.
    let mut too_many = datagram.clone();
    too_many[3] = 3;
    let error = decoder.decode(&too_many, exporter()).unwrap_err();
    assert!(
        matches!(&error, ProbeError::Protocol(text) if text.contains("announces 3 records")),
        "{error:?}"
    );
    let mut none = datagram.clone();
    none[3] = 0;
    assert!(decoder.decode(&none, exporter()).is_err());
    let mut absurd = datagram.clone();
    absurd[2] = 0xff;
    assert!(decoder.decode(&absurd, exporter()).is_err());
    assert_eq!(decoder.stats().errors, 3);
    assert_eq!(decoder.stats().records, 2);
}

// ------------------------------------------------------------------ NetFlow v9

/// Template 256: the classic IPv4 flow record, 43 bytes.
fn v9_template() -> Bytes {
    let fields: [(u16, u16); 16] = [
        (8, 4),  // IPV4_SRC_ADDR
        (12, 4), // IPV4_DST_ADDR
        (15, 4), // IPV4_NEXT_HOP
        (10, 2), // INPUT_SNMP
        (14, 2), // OUTPUT_SNMP
        (2, 4),  // IN_PKTS
        (1, 4),  // IN_BYTES
        (22, 4), // FIRST_SWITCHED
        (21, 4), // LAST_SWITCHED
        (7, 2),  // L4_SRC_PORT
        (11, 2), // L4_DST_PORT
        (6, 1),  // TCP_FLAGS
        (4, 1),  // PROTOCOL
        (5, 1),  // SRC_TOS
        (16, 2), // SRC_AS
        (17, 2), // DST_AS
    ];
    let mut template = Bytes::default().u16(256).u16(fields.len() as u16);
    for (kind, length) in fields {
        template = template.u16(kind).u16(length);
    }
    set(0, template)
}

fn v9_data() -> Bytes {
    let records = Bytes::default()
        .ip4([10, 0, 0, 1])
        .ip4([192, 0, 2, 80])
        .ip4([10, 0, 0, 254])
        .u16(3)
        .u16(5)
        .u32(10)
        .u32(1500)
        .u32(80_000)
        .u32(85_000)
        .u16(51_234)
        .u16(443)
        .u8(0x1b)
        .u8(6)
        .u8(0)
        .u16(65_001)
        .u16(15_169)
        // Second record: ICMP echo, no AS information.
        .ip4([10, 0, 0, 2])
        .ip4([10, 0, 0, 1])
        .ip4([0, 0, 0, 0])
        .u16(5)
        .u16(3)
        .u32(4)
        .u32(336)
        .u32(81_000)
        .u32(84_000)
        .u16(0)
        .u16(0x0800)
        .u8(0)
        .u8(1)
        .u8(0)
        .u16(0)
        .u16(0);
    assert_eq!(records.len(), 2 * 43);
    // 86 bytes of records are padded to 88.
    set(256, records)
}

fn v9_expected() -> Vec<FlowRecord> {
    vec![
        FlowRecord {
            src_port: 51_234,
            dst_port: 443,
            protocol: 6,
            bytes: 1500,
            packets: 10,
            tcp_flags: 0x1b,
            input_if: 3,
            output_if: 5,
            src_as: 65_001,
            dst_as: 15_169,
            next_hop: Some("10.0.0.254".into()),
            start_ms: 80_000,
            end_ms: 85_000,
            ..record(9, "10.0.0.1", "192.0.2.80")
        },
        FlowRecord {
            dst_port: 0x0800,
            protocol: 1,
            bytes: 336,
            packets: 4,
            input_if: 5,
            output_if: 3,
            start_ms: 81_000,
            end_ms: 84_000,
            ..record(9, "10.0.0.2", "10.0.0.1")
        },
    ]
}

#[test]
fn netflow_v9_template_and_data_in_one_datagram() {
    let mut decoder = Decoder::new();
    let datagram = v9_message(0x101, 3, &[v9_template(), v9_data()]);
    assert_eq!(
        decoder.decode(&datagram, exporter()).unwrap(),
        v9_expected()
    );
    assert_eq!(
        decoder.stats(),
        DecoderStats {
            packets: 1,
            records: 2,
            templates: 1,
            missing_template_sets: 0,
            errors: 0
        }
    );
    // The template stays: later datagrams carry data only.
    let data_only = v9_message(0x101, 2, &[v9_data()]);
    assert_eq!(
        decoder.decode(&data_only, exporter()).unwrap(),
        v9_expected()
    );
    assert_eq!(decoder.stats().records, 4);
}

#[test]
fn netflow_v9_data_before_its_template_is_counted_and_dropped() {
    let mut decoder = Decoder::new();
    let data_only = v9_message(0x101, 2, &[v9_data()]);
    assert_eq!(decoder.decode(&data_only, exporter()).unwrap(), vec![]);
    assert_eq!(decoder.decode(&data_only, exporter()).unwrap(), vec![]);
    assert_eq!(decoder.stats().missing_template_sets, 2);

    // The template alone yields no records, and from then on the data decodes.
    let template_only = v9_message(0x101, 1, &[v9_template()]);
    assert_eq!(decoder.decode(&template_only, exporter()).unwrap(), vec![]);
    assert_eq!(
        decoder.decode(&data_only, exporter()).unwrap(),
        v9_expected()
    );
    assert_eq!(
        decoder.stats(),
        DecoderStats {
            packets: 4,
            records: 2,
            templates: 1,
            missing_template_sets: 2,
            errors: 0
        }
    );
}

#[test]
fn templates_are_kept_per_exporter_and_source_id() {
    let mut decoder = Decoder::new();
    decoder
        .decode(&v9_message(0x101, 1, &[v9_template()]), exporter())
        .unwrap();
    // Another source id on the same exporter, and another exporter with the same ids.
    assert_eq!(
        decoder
            .decode(&v9_message(0x202, 2, &[v9_data()]), exporter())
            .unwrap(),
        vec![]
    );
    let other: SocketAddr = "192.0.2.2:2055".parse().unwrap();
    assert_eq!(
        decoder
            .decode(&v9_message(0x101, 2, &[v9_data()]), other)
            .unwrap(),
        vec![]
    );
    assert_eq!(decoder.stats().missing_template_sets, 2);
    // Each learns its own.
    decoder
        .decode(&v9_message(0x101, 1, &[v9_template()]), other)
        .unwrap();
    let records = decoder
        .decode(&v9_message(0x101, 2, &[v9_data()]), other)
        .unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].exporter, "192.0.2.2");
    assert_eq!(decoder.stats().templates, 2);
}

#[test]
fn netflow_v9_ipv6_template_with_wide_counters() {
    let fields: [(u16, u16); 12] = [
        (27, 16), // IPV6_SRC_ADDR
        (28, 16), // IPV6_DST_ADDR
        (62, 16), // IPV6_NEXT_HOP
        (10, 4),  // INPUT_SNMP, four bytes
        (14, 4),  // OUTPUT_SNMP
        (2, 8),   // IN_PKTS, eight bytes
        (1, 8),   // IN_BYTES
        (7, 2),
        (11, 2),
        (4, 1),
        (16, 4), // four-byte AS numbers
        (17, 4),
    ];
    let mut template = Bytes::default().u16(300).u16(fields.len() as u16);
    for (kind, length) in fields {
        template = template.u16(kind).u16(length);
    }
    let data = Bytes::default()
        .ip6("2001:db8:1::10")
        .ip6("2001:db8:2::20")
        .ip6("fe80::1")
        .u32(70_000)
        .u32(70_001)
        .u64(5_000_000_000) // more packets than 32 bits hold
        .u64(7_000_000_000_000)
        .u16(443)
        .u16(55_555)
        .u8(6)
        .u32(4_200_000_001)
        .u32(64_512);
    let mut decoder = Decoder::new();
    let records = decoder
        .decode(
            &v9_message(1, 2, &[set(0, template), set(300, data)]),
            exporter(),
        )
        .unwrap();
    assert_eq!(
        records,
        vec![FlowRecord {
            src_port: 443,
            dst_port: 55_555,
            protocol: 6,
            bytes: 7_000_000_000_000,
            packets: 5_000_000_000,
            input_if: 70_000,
            output_if: 70_001,
            src_as: 4_200_000_001,
            dst_as: 64_512,
            next_hop: Some("fe80::1".into()),
            ..record(9, "2001:db8:1::10", "2001:db8:2::20")
        }]
    );
}

#[test]
fn netflow_v9_options_announce_the_sampling_rate() {
    // Options template 257: scope "system" (type 1, 4 bytes), then SAMPLING_INTERVAL (34) and
    // SAMPLING_ALGORITHM (35).
    let options_template = set(
        1,
        Bytes::default()
            .u16(257) // template id
            .u16(4) // option scope length: one field specifier
            .u16(8) // option length: two field specifiers
            .u16(1)
            .u16(4) // scope: system
            .u16(34)
            .u16(4) // sampling interval
            .u16(35)
            .u16(1), // sampling algorithm
    );
    let options_data = set(257, Bytes::default().u32(0).u32(64).u8(1));
    let mut decoder = Decoder::new();
    let records = decoder
        .decode(
            &v9_message(
                0x101,
                4,
                &[
                    options_template,
                    options_data.clone(),
                    v9_template(),
                    v9_data(),
                ],
            ),
            exporter(),
        )
        .unwrap();
    assert_eq!(records.len(), 2, "the options record is not a flow");
    assert!(
        records.iter().all(|record| record.sampling == 64),
        "{records:?}"
    );
    assert_eq!(decoder.stats().templates, 2);
    assert_eq!(
        decoder.stats().missing_template_sets,
        0,
        "options data is skipped, not missed"
    );

    // Another source of the same exporter has announced nothing.
    let unsampled = decoder
        .decode(
            &v9_message(0x999, 3, &[v9_template(), v9_data()]),
            exporter(),
        )
        .unwrap();
    assert!(unsampled.iter().all(|record| record.sampling == 0));
    // Options data alone yields nothing.
    assert_eq!(
        decoder
            .decode(&v9_message(0x101, 1, &[options_data]), exporter())
            .unwrap(),
        vec![]
    );
}

#[test]
fn a_redefined_template_replaces_the_old_one() {
    let mut decoder = Decoder::new();
    decoder
        .decode(&v9_message(1, 1, &[v9_template()]), exporter())
        .unwrap();
    // The exporter is reconfigured: template 256 now has just addresses and bytes.
    let slim = set(
        0,
        Bytes::default()
            .u16(256)
            .u16(3)
            .u16(8)
            .u16(4)
            .u16(12)
            .u16(4)
            .u16(1)
            .u16(4),
    );
    let data = set(
        256,
        Bytes::default()
            .ip4([10, 1, 1, 1])
            .ip4([10, 2, 2, 2])
            .u32(999),
    );
    let records = decoder
        .decode(&v9_message(1, 2, &[slim, data]), exporter())
        .unwrap();
    assert_eq!(
        records,
        vec![FlowRecord {
            bytes: 999,
            ..record(9, "10.1.1.1", "10.2.2.2")
        }]
    );
    assert_eq!(decoder.stats().templates, 1);
}

// ------------------------------------------------------------------------ IPFIX

/// Template 400 exercises what is particular to IPFIX: an enterprise-specific element, a
/// variable-length element, and absolute millisecond timestamps.
fn ipfix_template() -> Bytes {
    set(
        2,
        Bytes::default()
            .u16(400) // template id
            .u16(12) // field count
            .u16(8)
            .u16(4) // sourceIPv4Address
            .u16(12)
            .u16(4) // destinationIPv4Address
            .u16(7)
            .u16(2) // sourceTransportPort
            .u16(11)
            .u16(2) // destinationTransportPort
            .u16(4)
            .u16(1) // protocolIdentifier
            .u16(0x8000 | 100)
            .u16(4)
            .u32(9) // enterprise element 100 of PEN 9 (Cisco), four bytes
            .u16(96)
            .u16(0xffff) // applicationName, variable length
            .u16(1)
            .u16(8) // octetDeltaCount
            .u16(2)
            .u16(8) // packetDeltaCount
            .u16(152)
            .u16(8) // flowStartMilliseconds
            .u16(153)
            .u16(8) // flowEndMilliseconds
            .u16(6)
            .u16(2), // tcpControlBits, two bytes in IPFIX
    )
}

fn ipfix_data() -> Bytes {
    // 300 bytes of application name need the three-byte length form.
    let long_name = vec![b'n'; 300];
    set(
        400,
        Bytes::default()
            .ip4([10, 0, 0, 1])
            .ip4([192, 0, 2, 80])
            .u16(51_234)
            .u16(443)
            .u8(6)
            .u32(0xdead_beef) // the enterprise element: skipped
            .u8(5)
            .raw(b"https") // short form: one length byte
            .u64(1500)
            .u64(10)
            .u64(1_699_999_990_000)
            .u64(1_699_999_995_500)
            .u16(0x001b)
            // Second record.
            .ip4([10, 0, 0, 2])
            .ip4([8, 8, 8, 8])
            .u16(40_000)
            .u16(53)
            .u8(17)
            .u32(0)
            .u8(255)
            .u16(300)
            .raw(&long_name) // long form: 255, then a two-byte length
            .u64(76)
            .u64(1)
            .u64(1_699_999_999_000)
            .u64(1_699_999_999_000)
            .u16(0),
    )
}

fn ipfix_expected() -> Vec<FlowRecord> {
    vec![
        FlowRecord {
            src_port: 51_234,
            dst_port: 443,
            protocol: 6,
            bytes: 1500,
            packets: 10,
            tcp_flags: 0x1b,
            start_ms: 1_699_999_990_000,
            end_ms: 1_699_999_995_500,
            ..record(10, "10.0.0.1", "192.0.2.80")
        },
        FlowRecord {
            src_port: 40_000,
            dst_port: 53,
            protocol: 17,
            bytes: 76,
            packets: 1,
            start_ms: 1_699_999_999_000,
            end_ms: 1_699_999_999_000,
            ..record(10, "10.0.0.2", "8.8.8.8")
        },
    ]
}

#[test]
fn ipfix_with_enterprise_and_variable_length_fields() {
    let mut decoder = Decoder::new();
    let datagram = ipfix_message(7, &[ipfix_template(), ipfix_data()]);
    assert_eq!(
        decoder.decode(&datagram, exporter()).unwrap(),
        ipfix_expected()
    );
    assert_eq!(
        decoder.stats(),
        DecoderStats {
            packets: 1,
            records: 2,
            templates: 1,
            missing_template_sets: 0,
            errors: 0
        }
    );
    // Data first, template later, another observation domain.
    let mut late = Decoder::new();
    assert_eq!(
        late.decode(&ipfix_message(7, &[ipfix_data()]), exporter())
            .unwrap(),
        vec![]
    );
    late.decode(&ipfix_message(8, &[ipfix_template()]), exporter())
        .unwrap();
    assert_eq!(
        late.decode(&ipfix_message(7, &[ipfix_data()]), exporter())
            .unwrap(),
        vec![]
    );
    assert_eq!(late.stats().missing_template_sets, 2);
    late.decode(&ipfix_message(7, &[ipfix_template()]), exporter())
        .unwrap();
    assert_eq!(
        late.decode(&ipfix_message(7, &[ipfix_data()]), exporter())
            .unwrap(),
        ipfix_expected()
    );
}

#[test]
fn ipfix_time_representations() {
    // (element ids, their lengths, encoded start and end, expected start_ms and end_ms)
    let ntp =
        |unix_secs: u64, fraction: u32| ((unix_secs + 2_208_988_800) << 32) | u64::from(fraction);
    let cases: Vec<(&str, [(u16, u16); 2], Bytes, (u64, u64))> = vec![
        (
            "seconds",
            [(150, 4), (151, 4)],
            Bytes::default().u32(1_699_999_990).u32(1_699_999_995),
            (1_699_999_990_000, 1_699_999_995_000),
        ),
        (
            "microseconds (NTP format): .5 s and .25 s fractions",
            [(154, 8), (155, 8)],
            Bytes::default()
                .u64(ntp(1_699_999_990, 0x8000_0000))
                .u64(ntp(1_699_999_995, 0x4000_0000)),
            (1_699_999_990_500, 1_699_999_995_250),
        ),
        (
            "nanoseconds (NTP format)",
            [(156, 8), (157, 8)],
            Bytes::default()
                .u64(ntp(1_699_999_990, 0))
                .u64(ntp(1_699_999_991, 0xc000_0000)),
            (1_699_999_990_000, 1_699_999_991_750),
        ),
        (
            "microseconds before the export time (1700000000)",
            [(158, 4), (159, 4)],
            Bytes::default().u32(10_000_000).u32(4_500_000),
            (1_699_999_990_000, 1_699_999_995_500),
        ),
        (
            "uptime-relative, with nothing to anchor it",
            [(22, 4), (21, 4)],
            Bytes::default().u32(80_000).u32(85_000),
            (80_000, 85_000),
        ),
    ];
    for (name, [first, second], times, expected) in cases {
        let template = set(
            2,
            Bytes::default()
                .u16(500)
                .u16(4)
                .u16(8)
                .u16(4)
                .u16(12)
                .u16(4)
                .u16(first.0)
                .u16(first.1)
                .u16(second.0)
                .u16(second.1),
        );
        let data = set(
            500,
            Bytes::default()
                .ip4([10, 0, 0, 1])
                .ip4([10, 0, 0, 2])
                .raw(&times.0),
        );
        let records = Decoder::new()
            .decode(&ipfix_message(1, &[template, data]), exporter())
            .unwrap();
        assert_eq!(records.len(), 1, "{name}");
        assert_eq!((records[0].start_ms, records[0].end_ms), expected, "{name}");
    }

    // Uptime-relative times become absolute when the record says when the exporter booted,
    // and a duration stands in for a missing end.
    let template = set(
        2,
        Bytes::default()
            .u16(501)
            .u16(5)
            .u16(8)
            .u16(4)
            .u16(12)
            .u16(4)
            .u16(22)
            .u16(4) // flowStartSysUpTime
            .u16(160)
            .u16(8) // systemInitTimeMilliseconds
            .u16(161)
            .u16(4), // flowDurationMilliseconds
    );
    let data = set(
        501,
        Bytes::default()
            .ip4([10, 0, 0, 1])
            .ip4([10, 0, 0, 2])
            .u32(80_000)
            .u64(1_699_900_000_000)
            .u32(2500),
    );
    let records = Decoder::new()
        .decode(&ipfix_message(1, &[template, data]), exporter())
        .unwrap();
    assert_eq!(
        (records[0].start_ms, records[0].end_ms),
        (1_699_900_080_000, 1_699_900_082_500)
    );
}

#[test]
fn ipfix_options_templates_and_withdrawals() {
    let mut decoder = Decoder::new();
    // Options template 600 (set id 3): one scope field (observationDomainId 149) and
    // samplingInterval (34); its data must not be mistaken for flows or counted as missing.
    let options_template = set(
        3,
        Bytes::default()
            .u16(600)
            .u16(2)
            .u16(1)
            .u16(149)
            .u16(4)
            .u16(34)
            .u16(4),
    );
    let options_data = set(600, Bytes::default().u32(7).u32(128));
    let records = decoder
        .decode(
            &ipfix_message(
                7,
                &[
                    options_template,
                    options_data,
                    ipfix_template(),
                    ipfix_data(),
                ],
            ),
            exporter(),
        )
        .unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.sampling == 128));
    assert_eq!(decoder.stats().templates, 2);
    assert_eq!(decoder.stats().missing_template_sets, 0);

    // Withdrawing template 400: its data is unknown again.
    let withdrawal = set(2, Bytes::default().u16(400).u16(0));
    decoder
        .decode(&ipfix_message(7, &[withdrawal]), exporter())
        .unwrap();
    assert_eq!(decoder.stats().templates, 1);
    assert_eq!(
        decoder
            .decode(&ipfix_message(7, &[ipfix_data()]), exporter())
            .unwrap(),
        vec![]
    );
    assert_eq!(decoder.stats().missing_template_sets, 1);

    // "Withdraw all data templates" (template id 2) leaves options templates alone.
    decoder
        .decode(&ipfix_message(7, &[ipfix_template()]), exporter())
        .unwrap();
    assert_eq!(decoder.stats().templates, 2);
    decoder
        .decode(
            &ipfix_message(7, &[set(2, Bytes::default().u16(2).u16(0))]),
            exporter(),
        )
        .unwrap();
    assert_eq!(decoder.stats().templates, 1);
    decoder
        .decode(
            &ipfix_message(7, &[set(3, Bytes::default().u16(3).u16(0))]),
            exporter(),
        )
        .unwrap();
    assert_eq!(decoder.stats().templates, 0);
}

#[test]
fn records_without_addresses_are_not_flows() {
    // A template with counters only (some exporters describe interfaces this way).
    let template = set(
        2,
        Bytes::default()
            .u16(700)
            .u16(2)
            .u16(10)
            .u16(4)
            .u16(1)
            .u16(8),
    );
    let data = set(700, Bytes::default().u32(3).u64(123_456));
    let mut decoder = Decoder::new();
    assert_eq!(
        decoder
            .decode(&ipfix_message(1, &[template, data]), exporter())
            .unwrap(),
        vec![]
    );
    assert_eq!(decoder.stats().missing_template_sets, 0);
}

// ----------------------------------------------------------------- malformed input

#[test]
fn malformed_datagrams_are_errors_with_a_reason() {
    let mut decoder = Decoder::new();
    let cases: Vec<(Vec<u8>, &str)> = vec![
        (vec![], "shorter than a version field"),
        (vec![0], "shorter than a version field"),
        (vec![0, 7, 0, 0], "version 7"),
        (vec![0, 1, 0, 0, 0, 0, 0, 0], "version 1"),
        (vec![0, 5, 0, 1], "NetFlow v5 header is incomplete"),
        (vec![0, 9, 0, 1, 0, 0], "NetFlow v9 header is incomplete"),
        (vec![0, 10, 0, 16], "IPFIX message header is incomplete"),
        // IPFIX length field larger than the datagram.
        (
            Bytes::default().u16(10).u16(200).u32(0).u32(0).u32(0).0,
            "says it is 200 bytes long",
        ),
        // IPFIX length smaller than its own header.
        (
            Bytes::default().u16(10).u16(8).u32(0).u32(0).u32(0).0,
            "says it is 8 bytes long",
        ),
        // A set shorter than its own header, and one that runs past the datagram.
        (
            v9_message(1, 1, &[Bytes::default().u16(256).u16(2)]),
            "claims 2 bytes",
        ),
        (
            v9_message(1, 1, &[Bytes::default().u16(256).u16(64).u32(0)]),
            "claims 64 bytes",
        ),
        // A template cut off in the middle of its field list.
        (
            v9_message(
                1,
                1,
                &[Bytes::default()
                    .u16(0)
                    .u16(12)
                    .u16(256)
                    .u16(5)
                    .u16(8)
                    .u16(4)],
            ),
            "template 256 is cut off",
        ),
        // A template with an absurd number of fields.
        (
            v9_message(1, 1, &[Bytes::default().u16(0).u16(8).u16(256).u16(5000)]),
            "declares 5000 fields",
        ),
    ];
    let count = cases.len() as u64;
    for (datagram, expected) in cases {
        let error = decoder.decode(&datagram, exporter()).unwrap_err();
        assert!(
            matches!(&error, ProbeError::Protocol(text) if text.contains(expected)),
            "{expected}: {error:?}"
        );
    }
    assert_eq!(
        decoder.stats(),
        DecoderStats {
            packets: 0,
            records: 0,
            templates: 0,
            missing_template_sets: 0,
            errors: count
        }
    );
}

#[test]
fn hostile_templates_cannot_hang_or_bloat_the_decoder() {
    let mut decoder = Decoder::new();
    // A template whose fields are all zero bytes long: its "records" must not loop forever.
    let empty_fields = set(
        0,
        Bytes::default()
            .u16(256)
            .u16(2)
            .u16(8)
            .u16(0)
            .u16(12)
            .u16(0),
    );
    let data = set(256, Bytes::default().raw(&[0xaa; 1000]));
    assert_eq!(
        decoder
            .decode(&v9_message(1, 2, &[empty_fields, data]), exporter())
            .unwrap(),
        vec![]
    );

    // Variable-length fields that claim more than the set holds.
    let variable = set(
        2,
        Bytes::default()
            .u16(300)
            .u16(2)
            .u16(8)
            .u16(4)
            .u16(96)
            .u16(0xffff),
    );
    let overrun = set(
        300,
        Bytes::default()
            .ip4([10, 0, 0, 1])
            .u8(255)
            .u16(60_000)
            .raw(b"short"),
    );
    assert_eq!(
        decoder
            .decode(&ipfix_message(1, &[variable, overrun]), exporter())
            .unwrap(),
        vec![]
    );

    // One-byte records: tens of thousands of them in a datagram, none of them a flow.
    let tiny = set(0, Bytes::default().u16(257).u16(1).u16(4).u16(1));
    let many = set(257, Bytes::default().raw(&vec![6u8; 60_000]));
    assert_eq!(
        decoder
            .decode(&v9_message(1, 2, &[tiny, many]), exporter())
            .unwrap(),
        vec![]
    );

    // A flood of templates from made-up sources: the table stays bounded.
    for source in 0..6000u32 {
        let template = set(
            0,
            Bytes::default()
                .u16(256 + (source % 7) as u16)
                .u16(1)
                .u16(8)
                .u16(4),
        );
        decoder
            .decode(&v9_message(source, 1, &[template]), exporter())
            .unwrap();
    }
    assert_eq!(decoder.stats().templates, 4096);
    // The most recent ones are the ones kept.
    let data = set(256 + (5999 % 7) as u16, Bytes::default().ip4([10, 9, 9, 9]));
    let records = decoder
        .decode(&v9_message(5999, 1, &[data]), exporter())
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].src, "10.9.9.9");
}

/// The fastest of five goes at `work`. A decode that takes a few milliseconds can be held
/// up for much longer by whatever else the machine is doing; it is not held up five times.
fn fastest_of_five(mut work: impl FnMut() -> std::time::Duration) -> std::time::Duration {
    (0..5).map(|_| work()).min().unwrap_or_default()
}

/// What one hostile datagram may cost. Each of the shapes below took between half a second
/// and three in a debug build before the decoder was made to do nothing per record that
/// looks at the whole set or at every template; they take a few milliseconds now.
const A_DATAGRAMS_WORTH: std::time::Duration = std::time::Duration::from_millis(150);

#[test]
fn a_datagram_of_thousands_of_templates_costs_what_its_size_is() {
    // Seven thousand new templates in one datagram, into a table that is already full:
    // every one of them makes the oldest go.
    let flood = |source: u32| {
        let mut content = Bytes::default();
        for n in 0..7_000u16 {
            content = content.u16(256 + n).u16(1).u16(8).u16(4);
        }
        v9_message(source, 1, &[set(0, content)])
    };
    let mut decoder = Decoder::new();
    decoder.decode(&flood(1), exporter()).unwrap();
    assert_eq!(decoder.stats().templates, 4096);
    let mut source = 1;
    let took = fastest_of_five(|| {
        source += 1;
        let datagram = flood(source);
        let started = std::time::Instant::now();
        decoder.decode(&datagram, exporter()).unwrap();
        started.elapsed()
    });
    assert!(took < A_DATAGRAMS_WORTH, "{took:?}");
    assert_eq!(decoder.stats().templates, 4096);
    // And the ones that went were the oldest: of the last datagram's seven thousand, the
    // first 2,904 are gone and the 4,096 after them are there.
    let data = |template: u16| {
        v9_message(
            source,
            1,
            &[set(256 + template, Bytes::default().ip4([10, 9, 9, 9]))],
        )
    };
    let before = decoder.stats().missing_template_sets;
    assert_eq!(decoder.decode(&data(2_903), exporter()).unwrap(), vec![]);
    assert_eq!(decoder.stats().missing_template_sets, before + 1);
    assert_eq!(decoder.decode(&data(2_904), exporter()).unwrap().len(), 1);
    assert_eq!(decoder.decode(&data(6_999), exporter()).unwrap().len(), 1);
}

#[test]
fn a_template_that_is_redefined_is_no_longer_the_oldest() {
    let define = |id: u16| {
        v9_message(
            1,
            1,
            &[set(0, Bytes::default().u16(id).u16(1).u16(8).u16(4))],
        )
    };
    let known = |decoder: &mut Decoder, id: u16| {
        let data = v9_message(1, 1, &[set(id, Bytes::default().ip4([10, 0, 0, 1]))]);
        decoder.decode(&data, exporter()).unwrap().len() == 1
    };
    let mut decoder = Decoder::new();
    // 256 is the first there is, then the table is filled to the brim.
    for id in 256..256 + 4096u16 {
        decoder.decode(&define(id), exporter()).unwrap();
    }
    assert_eq!(decoder.stats().templates, 4096);
    // Said again, it is the newest: the one that makes room for the next is 257.
    decoder.decode(&define(256), exporter()).unwrap();
    decoder.decode(&define(9_000), exporter()).unwrap();
    assert_eq!(decoder.stats().templates, 4096);
    assert!(known(&mut decoder, 256), "the redefined template was kept");
    assert!(!known(&mut decoder, 257), "the oldest one went");
    assert!(known(&mut decoder, 258));
    assert!(known(&mut decoder, 9_000));
}

#[test]
fn a_set_of_zeros_with_something_at_its_end_is_read_once_not_once_per_record() {
    // Sixteen thousand four-byte records of zeros, and one byte after them that is not: no
    // record is padding until the end, and before each of them "is the rest all zeros?" was
    // asked of the whole rest.
    let mut content = vec![0u8; 64_000];
    content.extend([1, 0, 0, 0]);
    let v9 = v9_message(1, 1, &[set(0, Bytes::default().raw(&content))]);
    let ipfix = ipfix_message(1, &[set(2, Bytes::default().raw(&content))]);
    for datagram in [v9, ipfix] {
        let took = fastest_of_five(|| {
            let mut decoder = Decoder::new();
            let started = std::time::Instant::now();
            decoder.decode(&datagram, exporter()).unwrap();
            started.elapsed()
        });
        assert!(took < A_DATAGRAMS_WORTH, "{took:?}");
    }

    // What padding is has not changed: zeros after the last record, however many, are not
    // a template, and a template after zeros that are a (reserved) record still is one.
    let mut decoder = Decoder::new();
    let padded = Bytes::default().u16(300).u16(1).u16(8).u16(4).raw(&[0; 12]);
    decoder
        .decode(&v9_message(1, 1, &[set(0, padded)]), exporter())
        .unwrap();
    assert_eq!(decoder.stats().templates, 1);
    let after_zeros = Bytes::default().raw(&[0; 8]).u16(301).u16(1).u16(8).u16(4);
    decoder
        .decode(&v9_message(1, 1, &[set(0, after_zeros)]), exporter())
        .unwrap();
    assert_eq!(decoder.stats().templates, 2);
}

#[test]
fn withdrawing_everything_over_and_over_looks_at_the_templates_once() {
    // A full table that belongs to somebody else, and a datagram that withdraws every
    // template of its own sender fifteen thousand times — in one set, and in as many sets.
    let mut decoder = Decoder::new();
    let mut content = Bytes::default();
    for n in 0..4_096u16 {
        content = content.u16(256 + n).u16(1).u16(8).u16(4);
    }
    decoder
        .decode(&ipfix_message(9, &[set(2, content)]), exporter())
        .unwrap();
    assert_eq!(decoder.stats().templates, 4096);
    let mut in_one_set = Bytes::default();
    for _ in 0..15_000 {
        in_one_set = in_one_set.u16(2).u16(0);
    }
    let one_set = ipfix_message(7, &[set(2, in_one_set)]);
    let sets: Vec<Bytes> = (0..7_500)
        .map(|_| set(2, Bytes::default().u16(2).u16(0)))
        .collect();
    let many_sets = ipfix_message(7, &sets);
    for datagram in [one_set, many_sets] {
        let took = fastest_of_five(|| {
            let started = std::time::Instant::now();
            decoder.decode(&datagram, exporter()).unwrap();
            started.elapsed()
        });
        assert!(took < A_DATAGRAMS_WORTH, "{took:?}");
    }
    assert_eq!(
        decoder.stats().templates,
        4096,
        "another observation domain's templates are not this one's to withdraw"
    );

    // Withdrawing everything twice still withdraws what was learned in between.
    let template = |id: u16| set(2, Bytes::default().u16(id).u16(1).u16(8).u16(4));
    let withdraw_all = || set(2, Bytes::default().u16(2).u16(0));
    let mut decoder = Decoder::new();
    decoder
        .decode(
            &ipfix_message(7, &[withdraw_all(), template(300), withdraw_all()]),
            exporter(),
        )
        .unwrap();
    assert_eq!(decoder.stats().templates, 0);
    decoder
        .decode(
            &ipfix_message(
                7,
                &[template(300), withdraw_all(), withdraw_all(), template(301)],
            ),
            exporter(),
        )
        .unwrap();
    assert_eq!(decoder.stats().templates, 1);
    // And a withdrawal in the next datagram is a new one.
    decoder
        .decode(&ipfix_message(7, &[withdraw_all()]), exporter())
        .unwrap();
    assert_eq!(decoder.stats().templates, 0);
}

#[test]
fn a_field_of_no_bytes_is_no_field() {
    // A thousand fields of length zero and one byte of address: sixty thousand "records"
    // in a datagram, each of which used to be a thousand steps.
    let mut fields = Bytes::default().u16(300).u16(1_024);
    for _ in 0..1_023 {
        fields = fields.u16(8).u16(0);
    }
    fields = fields.u16(8).u16(1);
    let mut decoder = Decoder::new();
    decoder
        .decode(&v9_message(1, 1, &[set(0, fields)]), exporter())
        .unwrap();
    let data = v9_message(1, 1, &[set(300, Bytes::default().raw(&vec![7u8; 60_000]))]);
    let took = fastest_of_five(|| {
        let started = std::time::Instant::now();
        let records = decoder.decode(&data, exporter()).unwrap();
        let took = started.elapsed();
        assert_eq!(records.len(), 60_000);
        took
    });
    // Sixty thousand records are sixty thousand records; they are not sixty million steps.
    assert!(took < A_DATAGRAMS_WORTH * 4, "{took:?}");

    // An empty field between real ones changes nothing about what the real ones say.
    let with_empty = set(
        0,
        Bytes::default()
            .u16(301)
            .u16(4)
            .u16(8)
            .u16(4)
            .u16(1)
            .u16(0)
            .u16(12)
            .u16(4)
            .u16(1)
            .u16(4),
    );
    let record = set(
        301,
        Bytes::default()
            .ip4([10, 0, 0, 1])
            .ip4([10, 0, 0, 2])
            .u32(1_500),
    );
    let records = decoder
        .decode(&v9_message(1, 2, &[with_empty, record]), exporter())
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        (
            records[0].src.as_str(),
            records[0].dst.as_str(),
            records[0].bytes
        ),
        ("10.0.0.1", "10.0.0.2", 1_500)
    );
}

#[test]
fn truncated_and_random_datagrams_never_panic() {
    let valid: Vec<Vec<u8>> = vec![
        v5_datagram(),
        v9_message(0x101, 3, &[v9_template(), v9_data()]),
        ipfix_message(7, &[ipfix_template(), ipfix_data()]),
    ];
    for datagram in &valid {
        // Every truncation, against a fresh decoder and against one that knows the templates.
        let mut primed = Decoder::new();
        primed.decode(datagram, exporter()).unwrap();
        for cut in 0..datagram.len() {
            let _ = Decoder::new().decode(&datagram[..cut], exporter());
            let _ = primed.decode(&datagram[..cut], exporter());
        }
        // Every single-byte corruption of a valid datagram.
        for index in 0..datagram.len() {
            for value in [0x00, 0x01, 0x7f, 0x80, 0xff] {
                let mut corrupted = datagram.clone();
                corrupted[index] = value;
                let _ = primed.decode(&corrupted, exporter());
            }
        }
    }
    let mut decoder = Decoder::new();
    let mut state = 0x6a09_e667_f3bc_c908u64;
    for round in 0..50_000u32 {
        let mut junk = vec![0u8; (state % 400) as usize];
        for byte in &mut junk {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            *byte = state as u8;
        }
        // Give most of them a real version so the set walkers are reached, and small set ids
        // and lengths so that templates get defined and used.
        if junk.len() >= 24 && round % 5 != 0 {
            junk[0] = 0;
            junk[1] = [5, 9, 10][(round % 3) as usize];
            if junk[1] == 10 {
                let length = junk.len() as u16;
                junk[2..4].copy_from_slice(&length.to_be_bytes());
            }
            let first_set = if junk[1] == 9 { 20 } else { 16 };
            junk[first_set] = (round % 2) as u8;
            junk[first_set + 1] = (round % 4) as u8;
            junk[first_set + 2] = 0;
        }
        if let Ok(records) = decoder.decode(&junk, exporter()) {
            assert!(records.len() <= junk.len());
        }
    }
    assert!(decoder.stats().templates <= 4096);
}

// ------------------------------------------------------------------- top talkers

fn flow(
    src: &str,
    dst: &str,
    protocol: u8,
    src_port: u16,
    dst_port: u16,
    bytes: u64,
    packets: u64,
    input_if: u32,
) -> FlowRecord {
    FlowRecord {
        protocol,
        src_port,
        dst_port,
        bytes,
        packets,
        input_if,
        ..record(9, src, dst)
    }
}

fn sample_flows() -> Vec<FlowRecord> {
    vec![
        flow("10.0.0.1", "192.0.2.80", 6, 51_234, 443, 1000, 10, 3),
        flow("192.0.2.80", "10.0.0.1", 6, 443, 51_234, 9000, 12, 5),
        flow("10.0.0.2", "192.0.2.80", 6, 50_000, 443, 500, 5, 3),
        flow("10.0.0.1", "8.8.8.8", 17, 40_000, 53, 80, 1, 3),
        flow("8.8.8.8", "10.0.0.1", 17, 53, 40_000, 200, 1, 5),
        flow("10.0.0.2", "10.0.0.1", 1, 0, 0x0800, 400, 4, 3),
        flow("10.0.0.9", "224.0.0.5", 89, 0, 0, 300, 3, 4),
    ]
}

fn summary(talkers: &[Talker]) -> Vec<(&str, u64, u64, u64)> {
    talkers
        .iter()
        .map(|t| (t.key.as_str(), t.bytes, t.packets, t.flows))
        .collect()
}

#[test]
fn top_talkers_by_each_key() {
    let flows = sample_flows();
    assert_eq!(
        summary(&top(&flows, "src", 10)),
        [
            ("192.0.2.80", 9000, 12, 1),
            ("10.0.0.1", 1080, 11, 2),
            ("10.0.0.2", 900, 9, 2),
            ("10.0.0.9", 300, 3, 1),
            ("8.8.8.8", 200, 1, 1),
        ]
    );
    assert_eq!(
        summary(&top(&flows, "dst", 3)),
        [
            ("10.0.0.1", 9600, 17, 3),
            ("192.0.2.80", 1500, 15, 2),
            ("224.0.0.5", 300, 3, 1)
        ]
    );
    // Both directions of a conversation count together, under the lower address first.
    assert_eq!(
        summary(&top(&flows, "conversation", 10)),
        [
            ("10.0.0.1 <-> 192.0.2.80", 10_000, 22, 2),
            ("10.0.0.2 <-> 192.0.2.80", 500, 5, 1),
            ("10.0.0.1 <-> 10.0.0.2", 400, 4, 1),
            ("10.0.0.9 <-> 224.0.0.5", 300, 3, 1),
            ("8.8.8.8 <-> 10.0.0.1", 280, 2, 2),
        ]
    );
    // The service port is the lower one, whichever direction the flow goes.
    assert_eq!(
        summary(&top(&flows, "application", 10)),
        [
            ("tcp/443", 10_500, 27, 3),
            ("icmp", 400, 4, 1),
            ("ospf", 300, 3, 1),
            ("udp/53", 280, 2, 2)
        ]
    );
    assert_eq!(
        summary(&top(&flows, "interface", 10)),
        [("5", 9200, 13, 2), ("3", 1980, 20, 4), ("4", 300, 3, 1)]
    );
}

#[test]
fn top_talkers_edge_cases() {
    let flows = sample_flows();
    assert_eq!(top(&flows, "src", 0), vec![]);
    assert_eq!(top(&flows, "no-such-key", 10), vec![]);
    assert_eq!(top(&[], "src", 10), vec![]);
    assert_eq!(top(&flows, "src", 1).len(), 1);
    // Equal byte counts are ordered by packets, then by key: the result is deterministic.
    let ties = vec![
        flow("10.0.0.3", "10.9.9.9", 6, 1, 2, 100, 1, 1),
        flow("10.0.0.1", "10.9.9.9", 6, 1, 2, 100, 1, 1),
        flow("10.0.0.2", "10.9.9.9", 6, 1, 2, 100, 5, 1),
    ];
    let keys: Vec<String> = top(&ties, "src", 10).into_iter().map(|t| t.key).collect();
    assert_eq!(keys, ["10.0.0.2", "10.0.0.1", "10.0.0.3"]);
    // Unusual protocols and IPv6 conversations.
    let odd = vec![
        flow("2001:db8::2", "2001:db8::1", 58, 0, 0x8000, 64, 1, 1),
        flow("10.0.0.1", "10.0.0.2", 99, 0, 0, 10, 1, 1),
        flow("10.0.0.1", "10.0.0.2", 6, 0, 8080, 5, 1, 1),
    ];
    assert_eq!(
        summary(&top(&odd, "application", 10)),
        [
            ("icmpv6", 64, 1, 1),
            ("ip/99", 10, 1, 1),
            ("tcp/8080", 5, 1, 1)
        ]
    );
    assert_eq!(
        top(&odd, "conversation", 1)[0].key,
        "2001:db8::1 <-> 2001:db8::2"
    );
    // Counters that would overflow saturate instead.
    let huge = vec![flow("10.0.0.1", "10.0.0.2", 6, 1, 2, u64::MAX, u64::MAX, 1); 2];
    assert_eq!(top(&huge, "src", 1)[0].bytes, u64::MAX);
}

// --------------------------------------------------------------------- collector

/// A queue of `records` flow records, with room enough in bytes that only their number counts.
fn records(records: usize) -> QueueLimits {
    QueueLimits {
        records,
        bytes: 1 << 30,
    }
}

/// The next batch of a collector, which a test never waits for longer than five seconds.
async fn next(queue: &mut FlowQueue) -> FlowBatch {
    tokio::time::timeout(Duration::from_secs(5), queue.recv())
        .await
        .expect("a batch arrives")
        .expect("the collector is running")
}

#[tokio::test]
async fn collector_decodes_what_exporters_send() {
    let (collector, mut batches) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        records(1_000),
        SharedDecoder::new(),
    )
    .await
    .unwrap();
    let address = collector.local_addr();
    let exporter = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    exporter.send_to(&v5_datagram(), address).await.unwrap();
    let batch = next(&mut batches).await;
    assert_eq!(
        (batch.exporter.to_string().as_str(), batch.version),
        ("127.0.0.1", 5)
    );
    assert_eq!((batch.sets_without_template, batch.templates), (0, 0));
    let batch = batch.records;
    assert_eq!(batch.len(), 2);
    assert_eq!(
        (batch[0].exporter.as_str(), batch[0].version, batch[0].bytes),
        ("127.0.0.1", 5, 1500)
    );

    // Garbage, data without a template, then the template and the data again.
    exporter
        .send_to(b"\x00\x09 this is not a flow export", address)
        .await
        .unwrap();
    exporter.send_to(&[], address).await.unwrap();
    exporter
        .send_to(&v9_message(1, 2, &[v9_data()]), address)
        .await
        .unwrap();
    exporter
        .send_to(&v9_message(1, 1, &[v9_template()]), address)
        .await
        .unwrap();
    exporter
        .send_to(&v9_message(1, 2, &[v9_data()]), address)
        .await
        .unwrap();
    // A datagram that held no flow is still reported: it says an exporter is there, that its
    // data could not be read yet, and then that its template has arrived.
    let batch = next(&mut batches).await;
    assert_eq!(
        (
            batch.version,
            batch.records.len(),
            batch.sets_without_template,
            batch.templates
        ),
        (9, 0, 1, 0),
        "data before its template is counted against the exporter, not guessed at"
    );
    let batch = next(&mut batches).await;
    assert_eq!(
        (
            batch.records.len(),
            batch.sets_without_template,
            batch.templates
        ),
        (0, 0, 1)
    );
    let batch = next(&mut batches).await;
    assert_eq!((batch.sets_without_template, batch.templates), (0, 1));
    let batch = batch.records;
    assert_eq!(batch.len(), 2);
    assert_eq!((batch[0].version, batch[0].dst.as_str()), (9, "192.0.2.80"));

    exporter
        .send_to(
            &ipfix_message(7, &[ipfix_template(), ipfix_data()]),
            address,
        )
        .await
        .unwrap();
    let batch = next(&mut batches).await;
    assert_eq!((batch.version, batch.templates), (10, 2));
    let batch = batch.records;
    assert_eq!(batch[1].start_ms, 1_699_999_999_000);
    assert_eq!(collector.waiting(), 0, "everything decoded has been taken");
    assert_eq!(collector.dropped(), Dropped::default());

    assert_eq!(
        collector.stats(),
        DecoderStats {
            packets: 5,
            records: 6,
            templates: 2,
            missing_template_sets: 1,
            errors: 2
        }
    );

    // Stopping releases the port; so does dropping.
    collector.stop().await;
    assert!(UdpSocket::bind(address).await.is_ok());
    let (collector, _batches) = FlowCollector::start(address, records(1_000), SharedDecoder::new())
        .await
        .unwrap();
    let error = FlowCollector::start(address, records(1_000), SharedDecoder::new())
        .await
        .unwrap_err();
    assert!(error.to_string().contains("already using it"), "{error}");
    drop(collector);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(UdpSocket::bind(address).await.is_ok());
}

#[tokio::test]
async fn a_collector_given_the_decoder_of_the_one_before_reads_with_what_was_announced_to_it() {
    let decoder = SharedDecoder::new();
    let (first, mut queue) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        records(1_000),
        decoder.clone(),
    )
    .await
    .unwrap();
    let exporter = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    exporter
        .send_to(&v9_message(1, 1, &[v9_template()]), first.local_addr())
        .await
        .unwrap();
    assert_eq!(next(&mut queue).await.templates, 1);
    first.stop().await;

    // Another port, the same exporter (and so the same source port), only data.
    let (second, mut queue) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        records(1_000),
        decoder.clone(),
    )
    .await
    .unwrap();
    exporter
        .send_to(&v9_message(1, 2, &[v9_data()]), second.local_addr())
        .await
        .unwrap();
    let batch = next(&mut queue).await;
    assert_eq!(
        (
            batch.records.len(),
            batch.sets_without_template,
            batch.templates
        ),
        (2, 0, 1),
        "read with the template the first collector learned"
    );
    // Each collector counts what it decoded; the decoder counts it all.
    assert_eq!((second.stats().packets, second.stats().records), (1, 2));
    assert_eq!((decoder.stats().packets, decoder.stats().records), (2, 2));
    assert_eq!(decoder.templates_of(batch.exporter), 1);

    // A fresh decoder knows nothing of what was said to another.
    let (fresh, mut queue) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        records(1_000),
        SharedDecoder::new(),
    )
    .await
    .unwrap();
    exporter
        .send_to(&v9_message(1, 3, &[v9_data()]), fresh.local_addr())
        .await
        .unwrap();
    let batch = next(&mut queue).await;
    assert_eq!((batch.records.len(), batch.sets_without_template), (0, 1));
}

#[test]
fn records_and_stats_are_json() {
    let records = Decoder::new().decode(&v5_datagram(), exporter()).unwrap();
    let json = serde_json::to_value(&records[0]).unwrap();
    assert_eq!(json["src"], "10.0.0.1");
    assert_eq!(json["next_hop"], "10.0.0.254");
    assert_eq!(json["sampling"], 100);
    let back: FlowRecord = serde_json::from_value(json).unwrap();
    assert_eq!(back, records[0]);
    let stats = serde_json::to_value(DecoderStats::default()).unwrap();
    assert_eq!(stats["missing_template_sets"], 0);
}

// ------------------------------------------------------------------ the queue

#[tokio::test]
async fn a_collector_nobody_empties_drops_and_counts_instead_of_growing() {
    // Room for five records, and every datagram holds two.
    let (collector, mut queue) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        records(5),
        SharedDecoder::new(),
    )
    .await
    .unwrap();
    let address = collector.local_addr();
    let exporter = UdpSocket::bind("127.0.0.1:0").await.unwrap();

    // Nobody takes anything. Two datagrams of two records fit under five; the rest do not.
    for _ in 0..40 {
        exporter.send_to(&v5_datagram(), address).await.unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while collector.stats().packets < 40 {
        assert!(
            std::time::Instant::now() < deadline,
            "the collector decoded {:?}",
            collector.stats()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(collector.waiting(), 4, "never more than the limit");
    assert_eq!(
        collector.dropped(),
        Dropped {
            datagrams: 38,
            records: 76
        },
        "what did not fit is counted, record for record"
    );
    assert_eq!(collector.stats().records, 80, "all of it was decoded");

    // Taking what waits makes room again, and the collector is still listening.
    assert_eq!(next(&mut queue).await.records.len(), 2);
    assert_eq!(next(&mut queue).await.records.len(), 2);
    assert_eq!(collector.waiting(), 0);
    exporter.send_to(&v5_datagram(), address).await.unwrap();
    assert_eq!(next(&mut queue).await.records.len(), 2);
    assert_eq!(collector.dropped().datagrams, 38);
}

#[tokio::test]
async fn a_collector_nobody_empties_holds_no_more_bytes_than_it_may_however_many_records_it_may() {
    // What one datagram of two records is once decoded, from this exporter's address.
    let one = FlowBatch {
        exporter: "127.0.0.1".parse().unwrap(),
        version: 5,
        records: Decoder::new()
            .decode(&v5_datagram(), "127.0.0.1:9".parse().unwrap())
            .unwrap(),
        sets_without_template: 0,
        templates: 0,
    }
    .weight();
    // Room for a million records, and in bytes for two datagrams and a half.
    let limit = one * 5 / 2;
    let (collector, mut queue) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        QueueLimits {
            records: 1_000_000,
            bytes: limit,
        },
        SharedDecoder::new(),
    )
    .await
    .unwrap();
    let address = collector.local_addr();
    let exporter = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    for _ in 0..40 {
        exporter.send_to(&v5_datagram(), address).await.unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while collector.stats().packets < 40 {
        assert!(
            std::time::Instant::now() < deadline,
            "the collector decoded {:?}",
            collector.stats()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        (collector.waiting(), collector.waiting_bytes()),
        (4, 2 * one),
        "two datagrams' worth waits, and no more"
    );
    assert_eq!(
        collector.dropped(),
        Dropped {
            datagrams: 38,
            records: 76
        },
        "what did not fit in bytes is dropped and counted like what does not fit in number"
    );

    // Taken, they make room again.
    assert_eq!(next(&mut queue).await.records.len(), 2);
    assert_eq!(next(&mut queue).await.records.len(), 2);
    assert_eq!(collector.waiting_bytes(), 0);
    exporter.send_to(&v5_datagram(), address).await.unwrap();
    assert_eq!(next(&mut queue).await.records.len(), 2);
}

#[tokio::test]
async fn a_datagram_bigger_than_the_queue_is_let_through_when_nothing_waits() {
    let (collector, mut queue) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        records(1),
        SharedDecoder::new(),
    )
    .await
    .unwrap();
    let exporter = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    exporter
        .send_to(&v5_datagram(), collector.local_addr())
        .await
        .unwrap();
    assert_eq!(
        next(&mut queue).await.records.len(),
        2,
        "a limit smaller than one datagram must not mean that nothing ever arrives"
    );
    assert_eq!(collector.dropped(), Dropped::default());
}

/// Wait until the collector has decoded `packets` datagrams; five seconds at most.
async fn until_decoded(collector: &FlowCollector, packets: u64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while collector.stats().packets < packets {
        assert!(
            std::time::Instant::now() < deadline,
            "the collector decoded {:?}",
            collector.stats()
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A NetFlow v5 datagram of `count` copies of [`v5_datagram`]'s first record.
fn v5_of(count: u16) -> Vec<u8> {
    let two = v5_datagram();
    let mut datagram = two[..24].to_vec();
    datagram[2..4].copy_from_slice(&count.to_be_bytes());
    for _ in 0..count {
        datagram.extend_from_slice(&two[24..72]);
    }
    datagram
}

#[tokio::test]
async fn a_datagram_larger_than_the_queue_does_not_turn_another_exporter_away_while_it_waits() {
    // Room for five records. Whoever can reach the port can send nothing but datagrams larger
    // than that (sixty thousand one-byte records is one); each is let onto an empty queue, so
    // one of them nearly always waits there.
    let (collector, mut queue) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        records(5),
        SharedDecoder::new(),
    )
    .await
    .unwrap();
    let address = collector.local_addr();
    let flood = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let quiet = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    flood.send_to(&v5_of(30), address).await.unwrap();
    until_decoded(&collector, 1).await;
    // Another exporter's records get in beside it, as many as the limit has room for; another
    // datagram larger than the queue does not, and neither does what would go past the limit.
    for datagram in [v5_datagram(), v5_datagram(), v5_of(30), v5_datagram()] {
        let from = if datagram.len() > 200 { &flood } else { &quiet };
        from.send_to(&datagram, address).await.unwrap();
    }
    until_decoded(&collector, 5).await;
    assert_eq!(collector.waiting(), 30 + 2 + 2);
    assert_eq!(
        collector.dropped(),
        Dropped {
            datagrams: 2,
            records: 32
        }
    );
    // In the order they came.
    for records in [30, 2, 2] {
        assert_eq!(next(&mut queue).await.records.len(), records);
    }
    assert_eq!((collector.waiting(), collector.waiting_bytes()), (0, 0));
    // Empty again, so one larger than the queue is let in again, and the others beside it.
    flood.send_to(&v5_of(30), address).await.unwrap();
    quiet.send_to(&v5_datagram(), address).await.unwrap();
    assert_eq!(next(&mut queue).await.records.len(), 30);
    assert_eq!(next(&mut queue).await.records.len(), 2);
    assert_eq!(collector.dropped().datagrams, 2);
}

#[test]
fn an_ipv4_sender_reported_the_ipv6_way_is_still_its_ipv4_address() {
    assert_eq!(
        canonical("[::ffff:192.0.2.1]:2055".parse().unwrap()),
        "192.0.2.1:2055".parse().unwrap()
    );
    // What is not an IPv4 address in disguise stays what it is.
    for peer in ["192.0.2.1:2055", "[2001:db8::1]:2055", "[::1]:9"] {
        let peer: SocketAddr = peer.parse().unwrap();
        assert_eq!(canonical(peer), peer);
    }
}

#[tokio::test]
async fn an_ipv4_exporter_is_an_ipv4_address_on_a_socket_bound_to_every_family() {
    let Ok((collector, mut queue)) = FlowCollector::start(
        "[::]:0".parse().unwrap(),
        records(100),
        SharedDecoder::new(),
    )
    .await
    else {
        eprintln!("skipped: this machine has no IPv6 socket to bind");
        return;
    };
    let exporter = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = collector.local_addr().port();
    if exporter
        .send_to(&v5_datagram(), ("127.0.0.1", port))
        .await
        .is_err()
    {
        eprintln!("skipped: the IPv6 socket does not take IPv4 here");
        return;
    }
    let Ok(Some(batch)) = tokio::time::timeout(Duration::from_secs(2), queue.recv()).await else {
        eprintln!("skipped: the IPv6 socket does not take IPv4 here (IPV6_V6ONLY)");
        return;
    };
    // The system reports the sender as ::ffff:127.0.0.1; an inventory knows it as 127.0.0.1.
    assert_eq!(batch.exporter.to_string(), "127.0.0.1");
    assert_eq!(batch.records[0].exporter, "127.0.0.1");
}

#[tokio::test]
async fn datagrams_without_flows_cannot_pile_up_either() {
    let (collector, _queue) = FlowCollector::start(
        "127.0.0.1:0".parse().unwrap(),
        records(5),
        SharedDecoder::new(),
    )
    .await
    .unwrap();
    let exporter = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    // Templates only: no record, so the record count never moves. The queue still holds
    // at most as many datagrams as its limit.
    for _ in 0..30 {
        exporter
            .send_to(&v9_message(1, 1, &[v9_template()]), collector.local_addr())
            .await
            .unwrap();
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while collector.stats().packets < 30 {
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        collector.dropped(),
        Dropped {
            datagrams: 25,
            records: 0
        }
    );
}

// ------------------------------------------------------------------ decoder memory

#[test]
fn templates_are_counted_per_exporter_address() {
    let mut decoder = Decoder::new();
    let first: SocketAddr = "192.0.2.1:2055".parse().unwrap();
    let again: SocketAddr = "192.0.2.1:40000".parse().unwrap();
    let other: SocketAddr = "192.0.2.2:2055".parse().unwrap();
    decoder
        .decode(&v9_message(1, 1, &[v9_template()]), first)
        .unwrap();
    decoder
        .decode(&v9_message(2, 1, &[v9_template()]), first)
        .unwrap();
    // The same exporter after a restart, from another source port.
    decoder
        .decode(&v9_message(1, 1, &[v9_template()]), again)
        .unwrap();
    decoder
        .decode(&ipfix_message(7, &[ipfix_template()]), other)
        .unwrap();
    assert_eq!(decoder.templates_of(first.ip()), 3);
    assert_eq!(decoder.templates_of(other.ip()), 1);
    assert_eq!(decoder.templates_of("192.0.2.3".parse().unwrap()), 0);

    // Redefining a template is not one more, and a withdrawal is one less.
    decoder
        .decode(&v9_message(1, 1, &[v9_template()]), first)
        .unwrap();
    assert_eq!(decoder.templates_of(first.ip()), 3);
    let withdraw_all = set(2, Bytes::default().u16(2).u16(0));
    decoder
        .decode(&ipfix_message(7, &[withdraw_all]), other)
        .unwrap();
    assert_eq!(decoder.templates_of(other.ip()), 0);
    assert_eq!(decoder.stats().templates, 3);
}

/// A v9 options template (id 300) that announces a sampling interval, and its data.
fn v9_sampling_options(rate: u32) -> [Bytes; 2] {
    [
        set(
            1,
            Bytes::default()
                .u16(300)
                .u16(4) // scope length
                .u16(4) // option length
                .u16(1) // scope: system
                .u16(4)
                .u16(34) // SAMPLING_INTERVAL
                .u16(4),
        ),
        set(300, Bytes::default().u32(0).u32(rate)),
    ]
}

#[test]
fn sampling_rates_announced_from_made_up_sources_do_not_pile_up() {
    let mut decoder = Decoder::new();
    // Five times as many (exporter, source id) pairs as templates are kept (4,096), each
    // announcing a rate. The templates are bounded; the rates have to be as well.
    const SOURCES: u32 = 5 * 4096;
    for source_id in 0..SOURCES {
        let [template, data] = v9_sampling_options(100);
        decoder
            .decode(&v9_message(source_id, 2, &[template, data]), exporter())
            .unwrap();
        assert!(
            decoder.rates_remembered() <= 2 * 4096,
            "{} rates are remembered after {source_id} sources",
            decoder.rates_remembered()
        );
    }
    // The newest source is among those still known: its flows carry its rate.
    let newest = SOURCES - 1;
    let records = decoder
        .decode(
            &v9_message(newest, 3, &[v9_template(), v9_data()]),
            exporter(),
        )
        .unwrap();
    assert_eq!(records[0].sampling, 100);
}

#[test]
fn ipfix_says_a_sampling_rate_as_size_out_of_population_too() {
    // RFC 5477: samplingSize (309) packets are taken out of every samplingPopulation (310).
    // Written from the RFC; no exporter at hand announces its rate this way.
    let options = set(
        3,
        Bytes::default()
            .u16(400)
            .u16(3) // fields
            .u16(1) // of which scope
            .u16(149) // observationDomainId
            .u16(4)
            .u16(309)
            .u16(4)
            .u16(310)
            .u16(4),
    );
    let data = set(400, Bytes::default().u32(7).u32(2).u32(1000));
    let mut decoder = Decoder::new();
    let records = decoder
        .decode(
            &ipfix_message(7, &[options, data, ipfix_template(), ipfix_data()]),
            exporter(),
        )
        .unwrap();
    assert_eq!(records[0].sampling, 500, "2 out of every 1000 is 1 in 500");
}

// ------------------------------------------------------------------ real exporters

/// Bytes from the hex a capture was written down in (whitespace ignored).
fn hex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

/// What an exporter reported, in the terms the traffic is described in.
fn reported(records: &[FlowRecord]) -> Vec<traffic::Flow> {
    let mut flows: Vec<traffic::Flow> = records
        .iter()
        .map(|record| traffic::Flow {
            src: record.src.clone(),
            dst: record.dst.clone(),
            src_port: record.src_port,
            dst_port: record.dst_port,
            protocol: record.protocol,
            packets: record.packets,
            bytes: record.bytes,
            tos: record.tos,
        })
        .collect();
    flows.sort();
    flows
}

/// The traffic without its IPv6 conversation, which NetFlow v5 cannot carry.
fn ipv4_only() -> Vec<traffic::Flow> {
    traffic::flows()
        .into_iter()
        .filter(|flow| !flow.src.contains(':'))
        .collect()
}

#[test]
fn the_traffic_is_what_its_table_says() {
    let flows = traffic::flows();
    assert_eq!(flows.len(), 7);
    assert_eq!(flows.iter().map(|flow| flow.packets).sum::<u64>(), 105);
    assert_eq!(
        flows.iter().map(|flow| flow.bytes).sum::<u64>(),
        36_376,
        "what nfdump added the same export up to"
    );
}

#[test]
fn what_softflowd_sent_is_decoded_in_every_version() {
    for (name, capture, version, expected) in [
        ("v5", captures::SOFTFLOWD_V5, 5, ipv4_only()),
        ("v9", captures::SOFTFLOWD_V9, 9, traffic::flows()),
        ("IPFIX", captures::SOFTFLOWD_IPFIX, 10, traffic::flows()),
    ] {
        let mut decoder = Decoder::new();
        let records = decoder.decode(&hex(capture), exporter()).unwrap();
        assert_eq!(reported(&records), expected, "softflowd {name}");
        assert!(
            records.iter().all(|record| record.version == version),
            "{name}"
        );
        assert!(
            records.iter().all(|record| record.sampling <= 1),
            "softflowd {name} did not sample, and says 1 or nothing: {records:?}"
        );
        assert_eq!(decoder.stats().missing_template_sets, 0, "{name}");
        assert_eq!(decoder.stats().errors, 0, "{name}");
    }
}

#[test]
fn what_softflowd_sent_carries_the_tcp_flags_and_the_marking_it_saw() {
    let records = Decoder::new()
        .decode(&hex(captures::SOFTFLOWD_V9), exporter())
        .unwrap();
    let voice = records
        .iter()
        .find(|record| record.src == "10.0.0.7")
        .unwrap();
    assert_eq!(voice.tos, 0xb8, "EF, as the packets were marked");
    let download = records
        .iter()
        .find(|record| record.src == "192.0.2.80")
        .unwrap();
    assert_eq!(download.tcp_flags, 0x1b, "SYN, ACK, PSH and FIN were seen");
    let ping = records.iter().find(|record| record.protocol == 1).unwrap();
    assert_eq!(
        (ping.src_port, ping.dst_port),
        (0, 0x0800),
        "an echo request: type 8, code 0, where NetFlow v5 puts it"
    );
}

#[test]
fn a_sampling_exporter_is_heard_saying_so_in_every_version() {
    for (name, capture) in [
        ("v5", captures::SOFTFLOWD_V5_SAMPLED),
        ("v9", captures::SOFTFLOWD_V9_SAMPLED),
        ("IPFIX", captures::SOFTFLOWD_IPFIX_SAMPLED),
    ] {
        let records = Decoder::new().decode(&hex(capture), exporter()).unwrap();
        assert!(!records.is_empty(), "softflowd {name} -s 100");
        for record in &records {
            assert_eq!(record.sampling, 100, "softflowd {name} -s 100: {record:?}");
            // What it counted is what it reports: one packet of the hundred, not a hundred.
            assert_eq!(record.packets, 1, "{name}");
        }
    }
}

#[test]
fn what_nfreplay_sent_is_the_same_traffic_under_another_template() {
    let mut decoder = Decoder::new();
    let records = decoder
        .decode(&hex(captures::NFREPLAY_V9), exporter())
        .unwrap();
    assert_eq!(reported(&records), traffic::flows());
    assert_eq!(decoder.stats().missing_template_sets, 0);
    // nfreplay sends absolute times, in milliseconds.
    assert!(
        records
            .iter()
            .all(|record| record.start_ms > 1_700_000_000_000 && record.end_ms >= record.start_ms),
        "{records:?}"
    );

    let records = Decoder::new()
        .decode(&hex(captures::NFREPLAY_V5), exporter())
        .unwrap();
    assert_eq!(reported(&records), ipv4_only());
}

/// An exporter's program, when it is installed.
fn installed(tool: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(["/usr/sbin", "/usr/local/sbin", "/usr/bin"].map(std::path::PathBuf::from))
        .map(|directory| directory.join(tool))
        .find(|candidate| candidate.is_file())
}

#[tokio::test]
async fn softflowd_itself_is_collected_from_in_every_version() {
    let Some(softflowd) = installed("softflowd") else {
        eprintln!(
            "skipped: softflowd is not installed (apt-get install softflowd), so no exporter \
             was run; the datagrams one sent are still decoded by the tests above"
        );
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(scratch.path().join("traffic.pcap"), traffic::pcap()).unwrap();

    for (version, expected) in [
        ("5", ipv4_only()),
        ("9", traffic::flows()),
        ("10", traffic::flows()),
    ] {
        let (collector, mut queue) = FlowCollector::start(
            "127.0.0.1:0".parse().unwrap(),
            records(10_000),
            SharedDecoder::new(),
        )
        .await
        .unwrap();
        let target = format!("127.0.0.1:{}", collector.local_addr().port());
        // It reads the capture file, exports every flow in it and exits. The pid file and
        // the control socket go where the capture is, not into /var/run.
        let output = tokio::process::Command::new(&softflowd)
            .current_dir(scratch.path())
            .args([
                "-r",
                "traffic.pcap",
                "-n",
                &target,
                "-v",
                version,
                "-6",
                "-d",
            ])
            .args(["-p", "softflowd.pid", "-c", "softflowd.ctl"])
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .expect("softflowd can be run");
        assert!(
            output.status.success(),
            "softflowd -v {version} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let mut records = Vec::new();
        while records.len() < expected.len() {
            let batch = next(&mut queue).await;
            assert_eq!(batch.exporter.to_string(), "127.0.0.1");
            assert_eq!(batch.sets_without_template, 0, "softflowd -v {version}");
            records.extend(batch.records);
        }
        assert_eq!(reported(&records), expected, "softflowd -v {version}");
        assert_eq!(collector.stats().errors, 0, "softflowd -v {version}");
        collector.stop().await;
    }
}
