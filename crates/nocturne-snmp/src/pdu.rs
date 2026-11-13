//! PDUs, variable bindings and the community-based (v1/v2c) message wrapper.

use crate::ber::{self, Reader, Result, Writer, tag};
use crate::{Oid, SnmpError, Value, VarBind};

/// The PDU types of RFC 1157 and RFC 3416, valued by their BER tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PduKind {
    Get = 0xa0,
    GetNext = 0xa1,
    Response = 0xa2,
    Set = 0xa3,
    /// The SNMPv1 Trap-PDU, which has a layout of its own ([`TrapV1`]).
    TrapV1 = 0xa4,
    GetBulk = 0xa5,
    Inform = 0xa6,
    TrapV2 = 0xa7,
    Report = 0xa8,
}

impl PduKind {
    pub fn from_tag(tag: u8) -> Option<PduKind> {
        Some(match tag {
            0xa0 => PduKind::Get,
            0xa1 => PduKind::GetNext,
            0xa2 => PduKind::Response,
            0xa3 => PduKind::Set,
            0xa4 => PduKind::TrapV1,
            0xa5 => PduKind::GetBulk,
            0xa6 => PduKind::Inform,
            0xa7 => PduKind::TrapV2,
            0xa8 => PduKind::Report,
            _ => return None,
        })
    }

    pub fn tag(self) -> u8 {
        self as u8
    }
}

/// Every PDU except the SNMPv1 trap.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Pdu {
    pub kind: PduKind,
    pub request_id: i32,
    /// error-status — or non-repeaters in a GetBulk request.
    pub error_status: u32,
    /// error-index — or max-repetitions in a GetBulk request.
    pub error_index: u32,
    pub varbinds: Vec<VarBind>,
}

impl Pdu {
    /// A request carrying NULL placeholders for `oids`.
    pub fn request(kind: PduKind, request_id: i32, oids: &[Oid]) -> Pdu {
        Pdu {
            kind,
            request_id,
            error_status: 0,
            error_index: 0,
            varbinds: oids
                .iter()
                .map(|oid| VarBind::new(oid.clone(), Value::Null))
                .collect(),
        }
    }

    pub fn encode(&self, w: &mut Writer) {
        w.constructed(self.kind.tag(), |w| {
            w.integer(i64::from(self.request_id));
            w.integer(i64::from(self.error_status));
            w.integer(i64::from(self.error_index));
            write_varbinds(w, &self.varbinds);
        });
    }

    fn decode(kind: PduKind, mut body: Reader<'_>) -> Result<Pdu> {
        let request_id = body.integer32()?;
        let error_status = body.non_negative("error-status")?;
        let error_index = body.non_negative("error-index")?;
        let varbinds = read_varbinds(&mut body)?;
        body.finish("the variable bindings")?;
        Ok(Pdu {
            kind,
            request_id,
            error_status,
            error_index,
            varbinds,
        })
    }
}

/// The SNMPv1 Trap-PDU (RFC 1157 §4.1.6).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TrapV1 {
    pub enterprise: Oid,
    /// `None` when the agent sent something other than four octets.
    pub agent_addr: Option<[u8; 4]>,
    pub generic: u32,
    pub specific: u32,
    pub timestamp: u32,
    pub varbinds: Vec<VarBind>,
}

impl TrapV1 {
    fn decode(mut body: Reader<'_>) -> Result<TrapV1> {
        let enterprise = body.oid()?;
        let agent_addr = body
            .expect(tag::IP_ADDRESS, "agent-addr")?
            .remaining()
            .try_into()
            .ok();
        // Trap numbers are small non-negative integers; devices that put 32-bit patterns in
        // specific-trap get them reinterpreted as unsigned rather than rejected.
        let generic = body.integer32()? as u32;
        let specific = body.integer32()? as u32;
        let (stamp_tag, stamp) = body.read_tlv()?;
        if stamp_tag != tag::TIME_TICKS && stamp_tag != tag::INTEGER {
            return Err(SnmpError::decode(format!(
                "expected time-stamp, found tag 0x{stamp_tag:02x}"
            )));
        }
        let timestamp = ber::decode_unsigned32(stamp.remaining())?;
        let varbinds = read_varbinds(&mut body)?;
        body.finish("the variable bindings")?;
        Ok(TrapV1 {
            enterprise,
            agent_addr,
            generic,
            specific,
            timestamp,
            varbinds,
        })
    }
}

/// Any PDU that can appear in a message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AnyPdu {
    Standard(Pdu),
    TrapV1(TrapV1),
}

impl AnyPdu {
    /// Decodes the PDU at the reader's position.
    pub fn decode(reader: &mut Reader<'_>) -> Result<AnyPdu> {
        let (tag, body) = reader.read_tlv()?;
        match PduKind::from_tag(tag) {
            Some(PduKind::TrapV1) => TrapV1::decode(body).map(AnyPdu::TrapV1),
            Some(kind) => Pdu::decode(kind, body).map(AnyPdu::Standard),
            None => Err(SnmpError::decode(format!("unknown PDU tag 0x{tag:02x}"))),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Values and variable bindings
// ---------------------------------------------------------------------------------------------

pub(crate) fn write_value(w: &mut Writer, value: &Value) {
    match value {
        Value::Integer(v) => w.integer(*v),
        Value::OctetString(bytes) => w.octet_string(bytes),
        Value::Null => w.null(tag::NULL),
        Value::Oid(oid) => w.oid(oid),
        Value::IpAddress(octets) => w.octets(tag::IP_ADDRESS, octets),
        Value::Counter32(v) => w.unsigned(tag::COUNTER32, u64::from(*v)),
        Value::Gauge32(v) => w.unsigned(tag::GAUGE32, u64::from(*v)),
        Value::TimeTicks(v) => w.unsigned(tag::TIME_TICKS, u64::from(*v)),
        Value::Opaque(bytes) => w.octets(tag::OPAQUE, bytes),
        Value::Counter64(v) => w.unsigned(tag::COUNTER64, *v),
        Value::NoSuchObject => w.null(tag::NO_SUCH_OBJECT),
        Value::NoSuchInstance => w.null(tag::NO_SUCH_INSTANCE),
        Value::EndOfMibView => w.null(tag::END_OF_MIB_VIEW),
    }
}

pub(crate) fn read_value(reader: &mut Reader<'_>) -> Result<Value> {
    let (value_tag, content) = reader.read_tlv()?;
    let bytes = content.remaining();
    Ok(match value_tag {
        tag::INTEGER => Value::Integer(ber::decode_integer(bytes)?),
        tag::OCTET_STRING => Value::OctetString(bytes.to_vec()),
        tag::NULL => Value::Null,
        tag::OBJECT_ID => Value::Oid(ber::decode_oid(bytes)?),
        tag::IP_ADDRESS => match <[u8; 4]>::try_from(bytes) {
            Ok(octets) => Value::IpAddress(octets),
            // Some agents put IPv6 or empty addresses in an IpAddress. Losing the whole
            // response over one such cell helps nobody, so hand the bytes over as they are.
            Err(_) => Value::OctetString(bytes.to_vec()),
        },
        tag::COUNTER32 => Value::Counter32(ber::decode_unsigned32(bytes)?),
        tag::GAUGE32 | tag::UINTEGER32 => Value::Gauge32(ber::decode_unsigned32(bytes)?),
        tag::TIME_TICKS => Value::TimeTicks(ber::decode_unsigned32(bytes)?),
        tag::OPAQUE | tag::NSAP_ADDRESS => Value::Opaque(bytes.to_vec()),
        tag::COUNTER64 => Value::Counter64(ber::decode_unsigned64(bytes)?),
        tag::NO_SUCH_OBJECT => Value::NoSuchObject,
        tag::NO_SUCH_INSTANCE => Value::NoSuchInstance,
        tag::END_OF_MIB_VIEW => Value::EndOfMibView,
        other => {
            return Err(SnmpError::decode(format!(
                "unsupported value tag 0x{other:02x}"
            )));
        }
    })
}

pub(crate) fn write_varbind(w: &mut Writer, bind: &VarBind) {
    w.sequence(|w| {
        w.oid(&bind.oid);
        write_value(w, &bind.value);
    });
}

pub(crate) fn write_varbinds(w: &mut Writer, binds: &[VarBind]) {
    w.sequence(|w| {
        for bind in binds {
            write_varbind(w, bind);
        }
    });
}

pub(crate) fn read_varbinds(reader: &mut Reader<'_>) -> Result<Vec<VarBind>> {
    let mut list = reader.sequence()?;
    let mut binds = Vec::new();
    while !list.is_empty() {
        let mut bind = list.sequence()?;
        let oid = bind.oid()?;
        let value = read_value(&mut bind)?;
        bind.finish("a variable binding")?;
        binds.push(VarBind::new(oid, value));
    }
    Ok(binds)
}

/// Checks that every OID in `binds` — names and OID values alike — can be put on the wire.
pub(crate) fn check_varbinds(binds: &[VarBind]) -> Result<()> {
    for bind in binds {
        ber::check_oid(&bind.oid)?;
        if let Value::Oid(oid) = &bind.value {
            ber::check_oid(oid)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------------------------

/// msgVersion values.
pub(crate) const VERSION_1: i64 = 0;
pub(crate) const VERSION_2C: i64 = 1;
pub(crate) const VERSION_3: i64 = 3;

/// The community-based protocol versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommunityVersion {
    V1,
    V2c,
}

impl CommunityVersion {
    fn number(self) -> i64 {
        match self {
            CommunityVersion::V1 => VERSION_1,
            CommunityVersion::V2c => VERSION_2C,
        }
    }
}

/// An SNMPv1 or SNMPv2c message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CommunityMessage {
    pub version: CommunityVersion,
    pub community: Vec<u8>,
    pub pdu: AnyPdu,
}

/// Encodes `SEQUENCE { version, community, pdu }`.
pub(crate) fn encode_community_message(
    version: CommunityVersion,
    community: &[u8],
    pdu: &Pdu,
) -> Vec<u8> {
    let mut w = Writer::new();
    w.sequence(|w| {
        w.integer(version.number());
        w.octet_string(community);
        pdu.encode(w);
    });
    w.into_bytes()
}

/// The outer layer of any SNMP datagram: the version and the rest of the message.
pub(crate) struct Envelope<'a> {
    pub version: i64,
    /// Everything after msgVersion inside the outer SEQUENCE.
    pub body: Reader<'a>,
}

impl<'a> Envelope<'a> {
    /// Opens the outer SEQUENCE. Bytes after it are ignored, as net-snmp does.
    pub fn open(datagram: &'a [u8]) -> Result<Envelope<'a>> {
        let mut body = Reader::new(datagram).sequence()?;
        let version = body.integer()?;
        Ok(Envelope { version, body })
    }
}

impl CommunityMessage {
    /// Decodes a v1/v2c datagram; anything else (SNMPv3 included) is an error.
    pub fn decode(datagram: &[u8]) -> Result<CommunityMessage> {
        let envelope = Envelope::open(datagram)?;
        Self::decode_body(envelope.version, envelope.body)
    }

    /// Decodes what follows msgVersion in a v1/v2c message.
    pub fn decode_body(version: i64, mut body: Reader<'_>) -> Result<CommunityMessage> {
        let version = match version {
            VERSION_1 => CommunityVersion::V1,
            VERSION_2C => CommunityVersion::V2c,
            other => {
                return Err(SnmpError::decode(format!(
                    "not a community-based SNMP version: {other}"
                )));
            }
        };
        let community = body.octet_string()?.to_vec();
        let pdu = AnyPdu::decode(&mut body)?;
        body.finish("the PDU")?;
        Ok(CommunityMessage {
            version,
            community,
            pdu,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    pub(crate) fn hex(s: &str) -> Vec<u8> {
        let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        assert!(digits.len() % 2 == 0, "odd number of hex digits");
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn value_bytes(value: &Value) -> Vec<u8> {
        let mut w = Writer::new();
        write_value(&mut w, value);
        w.into_bytes()
    }

    #[test]
    fn value_vectors() {
        let vectors: Vec<(Value, Vec<u8>)> = vec![
            (Value::Integer(3), hex("02 01 03")),
            (Value::Integer(-129), hex("02 02 ff 7f")),
            (Value::OctetString(b"eth3".to_vec()), hex("04 04 65 74 68 33")),
            (Value::OctetString(vec![]), hex("04 00")),
            (Value::Null, hex("05 00")),
            (
                Value::Oid(oid("1.3.6.1.6.3.1.1.5.4")),
                hex("06 09 2b 06 01 06 03 01 01 05 04"),
            ),
            (Value::IpAddress([192, 0, 2, 1]), hex("40 04 c0 00 02 01")),
            (Value::Counter32(4_294_967_295), hex("41 05 00 ff ff ff ff")),
            (Value::Gauge32(1_000_000_000), hex("42 04 3b 9a ca 00")),
            (Value::TimeTicks(54_321), hex("43 03 00 d4 31")),
            (Value::Opaque(vec![0x9f, 0x78, 0x04, 0x3f, 0, 0, 0]), hex("44 07 9f 78 04 3f 00 00 00")),
            (Value::Counter64(8_519_442), hex("46 04 00 81 ff 12")),
            (
                Value::Counter64(u64::MAX),
                hex("46 09 00 ff ff ff ff ff ff ff ff"),
            ),
            (Value::NoSuchObject, hex("80 00")),
            (Value::NoSuchInstance, hex("81 00")),
            (Value::EndOfMibView, hex("82 00")),
        ];
        for (value, bytes) in vectors {
            assert_eq!(value_bytes(&value), bytes, "encoding {value:?}");
            let mut reader = Reader::new(&bytes);
            assert_eq!(read_value(&mut reader).unwrap(), value, "decoding {value:?}");
            assert!(reader.is_empty());
        }
    }

    #[test]
    fn value_decoding_tolerates_field_quirks() {
        let decode = |s: &str| read_value(&mut Reader::new(&hex(s)));
        // IpAddress of the wrong size degrades to an octet string instead of failing.
        assert_eq!(decode("40 00").unwrap(), Value::OctetString(vec![]));
        assert_eq!(
            decode("40 06 01 02 03 04 05 06").unwrap(),
            Value::OctetString(vec![1, 2, 3, 4, 5, 6])
        );
        // Obsolete application types.
        assert_eq!(decode("47 01 2a").unwrap(), Value::Gauge32(42));
        assert_eq!(decode("45 02 01 02").unwrap(), Value::Opaque(vec![1, 2]));
        // Counter32 without the mandatory leading zero, zero-length integers.
        assert_eq!(decode("41 04 ff ff ff ff").unwrap(), Value::Counter32(u32::MAX));
        assert_eq!(decode("02 00").unwrap(), Value::Integer(0));
        assert_eq!(decode("43 00").unwrap(), Value::TimeTicks(0));
        // Unknown tags and oversized numbers are errors, never panics.
        assert!(matches!(decode("48 01 00"), Err(SnmpError::Decode(_))));
        assert!(matches!(decode("24 00"), Err(SnmpError::Decode(_))));
        assert!(matches!(
            decode("41 05 01 00 00 00 00"),
            Err(SnmpError::Decode(_))
        ));
    }

    /// `snmpget -d -v2c -c public … sysName.0 sysLocation.0` against net-snmp 5.9.4,
    /// captured on the wire (request, then the agent's response).
    const CAPTURED_GET: &str = "
        30 37 02 01 01 04 06 70 75 62 6c 69 63 a0 2a 02
        04 3e 28 8c 1d 02 01 00 02 01 00 30 1c 30 0c 06
        08 2b 06 01 02 01 01 05 00 05 00 30 0c 06 08 2b
        06 01 02 01 01 06 00 05 00";
    const CAPTURED_RESPONSE: &str = "
        30 50 02 01 01 04 06 70 75 62 6c 69 63 a2 43 02
        04 3e 28 8c 1d 02 01 00 02 01 00 30 35 30 19 06
        08 2b 06 01 02 01 01 05 00 04 0d 6e 6f 63 74 75
        72 6e 65 2d 74 65 73 74 30 18 06 08 2b 06 01 02
        01 01 06 00 04 0c 4e 6f 63 74 75 72 6e 65 20 6c
        61 62";

    #[test]
    fn captured_v2c_get_is_reproduced_byte_for_byte() {
        let request = Pdu::request(
            PduKind::Get,
            0x3e28_8c1d,
            &[oid("1.3.6.1.2.1.1.5.0"), oid("1.3.6.1.2.1.1.6.0")],
        );
        let encoded = encode_community_message(CommunityVersion::V2c, b"public", &request);
        assert_eq!(encoded, hex(CAPTURED_GET));

        let decoded = CommunityMessage::decode(&encoded).unwrap();
        assert_eq!(
            decoded,
            CommunityMessage {
                version: CommunityVersion::V2c,
                community: b"public".to_vec(),
                pdu: AnyPdu::Standard(request),
            }
        );
    }

    #[test]
    fn captured_v2c_response_decodes() {
        let message = CommunityMessage::decode(&hex(CAPTURED_RESPONSE)).unwrap();
        assert_eq!(message.version, CommunityVersion::V2c);
        assert_eq!(message.community, b"public");
        let AnyPdu::Standard(pdu) = message.pdu else {
            panic!("expected a standard PDU");
        };
        assert_eq!(pdu.kind, PduKind::Response);
        assert_eq!(pdu.request_id, 1_042_844_701);
        assert_eq!(pdu.error_status, 0);
        assert_eq!(pdu.error_index, 0);
        assert_eq!(
            pdu.varbinds,
            vec![
                VarBind::new(oid("1.3.6.1.2.1.1.5.0"), Value::from("nocturne-test")),
                VarBind::new(oid("1.3.6.1.2.1.1.6.0"), Value::from("Nocturne lab")),
            ]
        );

        // Re-encoding the decoded response gives the captured bytes back.
        let reencoded = encode_community_message(CommunityVersion::V2c, b"public", &pdu);
        assert_eq!(reencoded, hex(CAPTURED_RESPONSE));
    }

    /// `snmpbulkget -Cn0 -Cr3 … ifHCInOctets` and the v1 noSuchName response, also captured.
    #[test]
    fn captured_getbulk_and_v1_error_decode() {
        let bulk = hex("30 2b 02 01 01 04 06 70 75 62 6c 69 63 a5 1e 02
                        04 60 bb 6a fa 02 01 00 02 01 03 30 10 30 0e 06
                        0a 2b 06 01 02 01 1f 01 01 01 06 05 00");
        let mut request = Pdu::request(PduKind::GetBulk, 0x60bb_6afa, &[oid("1.3.6.1.2.1.31.1.1.1.6")]);
        request.error_index = 3; // max-repetitions
        assert_eq!(encode_community_message(CommunityVersion::V2c, b"public", &request), bulk);

        let response = hex("30 54 02 01 01 04 06 70 75 62 6c 69 63 a2 47 02
                            04 60 bb 6a fa 02 01 00 02 01 00 30 39 30 13 06
                            0b 2b 06 01 02 01 1f 01 01 01 06 01 46 04 00 81
                            ff 12 30 10 06 0b 2b 06 01 02 01 1f 01 01 01 06
                            02 46 01 00 30 10 06 0b 2b 06 01 02 01 1f 01 01
                            01 06 03 46 01 00");
        let AnyPdu::Standard(pdu) = CommunityMessage::decode(&response).unwrap().pdu else {
            panic!("expected a standard PDU");
        };
        assert_eq!(
            pdu.varbinds,
            vec![
                VarBind::new(oid("1.3.6.1.2.1.31.1.1.1.6.1"), Value::Counter64(8_519_442)),
                VarBind::new(oid("1.3.6.1.2.1.31.1.1.1.6.2"), Value::Counter64(0)),
                VarBind::new(oid("1.3.6.1.2.1.31.1.1.1.6.3"), Value::Counter64(0)),
            ]
        );

        let v1_error = hex("30 37 02 01 00 04 06 70 75 62 6c 69 63 a2 2a 02
                            04 42 05 f7 50 02 01 02 02 01 02 30 1c 30 0c 06
                            08 2b 06 01 02 01 01 05 00 05 00 30 0c 06 08 2b
                            06 01 02 01 01 63 00 05 00");
        let message = CommunityMessage::decode(&v1_error).unwrap();
        assert_eq!(message.version, CommunityVersion::V1);
        let AnyPdu::Standard(pdu) = message.pdu else {
            panic!("expected a standard PDU");
        };
        assert_eq!((pdu.error_status, pdu.error_index), (2, 2));
        assert_eq!(pdu.varbinds[1].oid, oid("1.3.6.1.2.1.1.99.0"));
    }

    #[test]
    fn message_decoding_is_strict_about_structure() {
        let good = hex(CAPTURED_RESPONSE);
        // Trailing bytes after the message are ignored...
        let mut padded = good.clone();
        padded.extend_from_slice(&[0, 0, 0]);
        assert!(CommunityMessage::decode(&padded).is_ok());
        // ...but every truncation is an error.
        for len in 0..good.len() {
            assert!(
                matches!(CommunityMessage::decode(&good[..len]), Err(SnmpError::Decode(_))),
                "truncation to {len} bytes"
            );
        }
        // SNMPv3 and unknown versions are not community messages.
        assert!(CommunityMessage::decode(&hex("30 03 02 01 03")).is_err());
        assert!(CommunityMessage::decode(&hex("30 03 02 01 02")).is_err());
        // Unknown PDU tag.
        assert!(CommunityMessage::decode(&hex("30 0b 02 01 01 04 00 a9 04 02 00 02 00")).is_err());
        // A negative error-status is nonsense.
        assert!(
            CommunityMessage::decode(&hex(
                "30 12 02 01 01 04 00 a2 0b 02 01 01 02 01 ff 02 01 00 30 00"
            ))
            .is_err()
        );
    }

    #[test]
    fn unencodable_varbinds_are_caught_before_encoding() {
        let fine = [VarBind::new(oid("1.3.6.1.2.1.1.5.0"), Value::Oid(oid("0.0")))];
        assert!(check_varbinds(&fine).is_ok());
        let bad_name = [VarBind::new(oid("9.9"), Value::Null)];
        assert!(matches!(check_varbinds(&bad_name), Err(SnmpError::Unsupported(_))));
        let bad_value = [VarBind::new(oid("1.3"), Value::Oid(oid("1.99")))];
        assert!(matches!(check_varbinds(&bad_value), Err(SnmpError::Unsupported(_))));
    }
}
