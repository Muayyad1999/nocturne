//! Decoding of traps and informs, and the acknowledgement of informs.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::pdu::{
    AnyPdu, CommunityMessage, CommunityVersion, Envelope, Pdu, PduKind, TrapV1, VERSION_1,
    VERSION_2C, VERSION_3, encode_community_message,
};
use crate::v3::V3Message;
use crate::{Oid, SnmpError, Value, VarBind};

/// sysUpTime.0 — the first binding of every SNMPv2 notification.
const SYS_UP_TIME: [u32; 8] = [1, 3, 6, 1, 2, 1, 1, 3];
/// snmpTrapOID.0 — the second.
const SNMP_TRAP_OID: [u32; 10] = [1, 3, 6, 1, 6, 3, 1, 1, 4, 1];
/// snmpTraps: the SNMPv2 home of the six generic SNMPv1 traps.
const SNMP_TRAPS: [u32; 9] = [1, 3, 6, 1, 6, 3, 1, 1, 5];

/// A notification received from an agent, in a shape common to all protocol versions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trap {
    /// 1, 2 (SNMPv2c) or 3. SNMPv3 notifications are only decoded when they are neither
    /// authenticated nor encrypted.
    pub version: u8,
    /// The community string; for SNMPv3, the user (security) name.
    pub community: String,
    /// Where the datagram came from.
    pub source: SocketAddr,
    /// sysUpTime of the sender when the event occurred, in hundredths of a second.
    pub uptime_ticks: Option<u32>,
    /// snmpTrapOID.0 (v2c) or enterprise + generic/specific mapped to the v2 form (RFC 3584).
    pub trap_oid: Option<Oid>,
    /// SNMPv1 only: the enterprise the trap belongs to.
    pub enterprise: Option<Oid>,
    /// SNMPv1 only: the address the agent claims as its own (`None` if it sent nonsense).
    pub agent_addr: Option<[u8; 4]>,
    /// SNMPv1 only: generic-trap (0 coldStart … 5 egpNeighborLoss, 6 enterpriseSpecific).
    pub generic: Option<u32>,
    /// SNMPv1 only: specific-trap.
    pub specific: Option<u32>,
    /// The payload. For SNMPv2c/v3 the leading sysUpTime.0 and snmpTrapOID.0 bindings are
    /// not repeated here (they are `uptime_ticks` and `trap_oid`), so that a v1 and a v2
    /// notification of the same event carry the same bindings.
    pub varbinds: Vec<VarBind>,
    /// An InformRequest: the sender expects [`inform_response`].
    pub is_inform: bool,
}

/// Decodes a datagram received on the trap port.
///
/// Returns [`SnmpError::Unsupported`] for SNMPv3 notifications that are authenticated or
/// encrypted (verifying them needs the sender's user database) and [`SnmpError::Decode`] for
/// anything that is not a notification.
pub fn decode_trap(datagram: &[u8], source: std::net::SocketAddr) -> Result<Trap, SnmpError> {
    let envelope = Envelope::open(datagram)?;
    match envelope.version {
        VERSION_1 | VERSION_2C => {
            let message = CommunityMessage::decode_body(envelope.version, envelope.body)?;
            let community = String::from_utf8_lossy(&message.community).into_owned();
            match message.pdu {
                AnyPdu::TrapV1(trap) => Ok(from_v1(community, source, trap)),
                AnyPdu::Standard(pdu) => {
                    let version = match message.version {
                        CommunityVersion::V1 => 1,
                        CommunityVersion::V2c => 2,
                    };
                    from_notification(version, community, source, pdu)
                }
            }
        }
        VERSION_3 => {
            let message = V3Message::decode(datagram)?;
            if message.is_authenticated() || message.is_encrypted() {
                return Err(SnmpError::Unsupported(
                    "authenticated or encrypted SNMPv3 notifications cannot be decoded".to_owned(),
                ));
            }
            let user = String::from_utf8_lossy(message.user).into_owned();
            from_notification(3, user, source, message.scoped_pdu(None)?.pdu)
        }
        other => Err(SnmpError::decode(format!("unknown SNMP version {other}"))),
    }
}

/// Build the acknowledgement datagram for an InformRequest.
///
/// The Response echoes the inform's request-id and bindings (RFC 3416 §4.2.7); send it back
/// to the address the inform came from. Only SNMPv2c informs can be acknowledged: an SNMPv3
/// inform would have to be answered as an authoritative engine.
pub fn inform_response(datagram: &[u8]) -> Result<Vec<u8>, SnmpError> {
    let envelope = Envelope::open(datagram)?;
    if envelope.version == VERSION_3 {
        return Err(SnmpError::Unsupported(
            "SNMPv3 informs cannot be acknowledged".to_owned(),
        ));
    }
    let message = CommunityMessage::decode_body(envelope.version, envelope.body)?;
    match message.pdu {
        AnyPdu::Standard(inform) if inform.kind == PduKind::Inform => {
            let response = Pdu {
                kind: PduKind::Response,
                request_id: inform.request_id,
                error_status: 0,
                error_index: 0,
                varbinds: inform.varbinds,
            };
            Ok(encode_community_message(
                message.version,
                &message.community,
                &response,
            ))
        }
        _ => Err(SnmpError::decode("not an InformRequest")),
    }
}

fn from_v1(community: String, source: SocketAddr, trap: TrapV1) -> Trap {
    Trap {
        version: 1,
        community,
        source,
        uptime_ticks: Some(trap.timestamp),
        trap_oid: Some(v2_trap_oid(&trap.enterprise, trap.generic, trap.specific)),
        enterprise: Some(trap.enterprise),
        agent_addr: trap.agent_addr,
        generic: Some(trap.generic),
        specific: Some(trap.specific),
        varbinds: trap.varbinds,
        is_inform: false,
    }
}

/// RFC 3584 §3.1: the generic traps become snmpTraps.(generic + 1); an enterprise-specific
/// trap becomes enterprise.0.specific.
fn v2_trap_oid(enterprise: &Oid, generic: u32, specific: u32) -> Oid {
    match generic {
        0..=5 => Oid::new(SNMP_TRAPS).child(generic + 1),
        _ => enterprise.child(0).child(specific),
    }
}

fn from_notification(
    version: u8,
    community: String,
    source: SocketAddr,
    pdu: Pdu,
) -> Result<Trap, SnmpError> {
    let is_inform = match pdu.kind {
        PduKind::TrapV2 => false,
        PduKind::Inform => true,
        other => {
            return Err(SnmpError::decode(format!(
                "not a notification: {other:?} PDU"
            )));
        }
    };

    // RFC 3416 §4.2.6: sysUpTime.0 and snmpTrapOID.0 lead the bindings. Each is taken if it is
    // where it belongs; a sender that leaves one out loses only that field.
    let mut varbinds = pdu.varbinds;
    let mut uptime_ticks = None;
    let mut trap_oid = None;
    if let Some(first) = varbinds.first()
        && is_instance_of(&first.oid, &SYS_UP_TIME)
        && let Some(ticks) = first.value.as_u64().and_then(|t| u32::try_from(t).ok())
    {
        uptime_ticks = Some(ticks);
        varbinds.remove(0);
    }
    if let Some(first) = varbinds.first()
        && is_instance_of(&first.oid, &SNMP_TRAP_OID)
        && let Value::Oid(oid) = &first.value
    {
        trap_oid = Some(oid.clone());
        varbinds.remove(0);
    }

    Ok(Trap {
        version,
        community,
        source,
        uptime_ticks,
        trap_oid,
        enterprise: None,
        agent_addr: None,
        generic: None,
        specific: None,
        varbinds,
        is_inform,
    })
}

/// `oid` is the scalar `object` — with its `.0`, or (from sloppy senders) without.
fn is_instance_of(oid: &Oid, object: &[u32]) -> bool {
    matches!(oid.parts().strip_prefix(object), Some([] | [0]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names;
    use pretty_assertions::assert_eq;

    fn oid(s: &str) -> Oid {
        s.parse().unwrap()
    }

    fn hex(s: &str) -> Vec<u8> {
        let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    fn source() -> SocketAddr {
        "192.0.2.77:50123".parse().unwrap()
    }

    // The datagrams below were produced by net-snmp 5.9.4's snmptrap/snmpinform and captured
    // on the wire.

    /// `snmptrap -v 1 -c public … 1.3.6.1.4.1.8072.3.2.10 192.0.2.1 2 0 12345 ifIndex.3 i 3
    /// ifAdminStatus.3 i 2 ifOperStatus.3 i 2`
    const V1_LINK_DOWN: &str = "
        305e02010004067075626c6963a451060a2b06010401bf0803020a4004c0000201020102020100430230
        393033300f060a2b060102010202010103020103300f060a2b060102010202010703020102300f060a2b
        060102010202010803020102";

    /// `snmptrap -v 1 -c public … 1.3.6.1.4.1.9.9.43.2 192.0.2.7 6 1 4242 ccmHistoryEventTerminalType.5 i 1`
    const V1_ENTERPRISE_SPECIFIC: &str = "
        303f02010004067075626c6963a43206092b0601040109092b024004c00002070201060201014302109230
        153013060e2b0601040109092b010106010305020101";

    /// `snmptrap -v 2c -c public … 54321 linkUp ifIndex.3 i 3 ifAdminStatus.3 i 1
    /// ifOperStatus.3 i 1 ifName.3 s eth3`
    const V2C_LINK_UP: &str = "
        30818e02010104067075626c6963a781800204266253580201000201003072300f06082b0601020101030043
        0300d4313017060a2b06010603010104010006092b0601060301010504300f060a2b06010201020201010302
        0103300f060a2b060102010202010703020101300f060a2b0601020102020108030201013013060b2b060102
        011f0101010103040465746833";

    /// `snmptrap -v 2c -c public … '' coldStart` (the uptime is the sender's own).
    const V2C_COLD_START: &str = "
        304502010104067075626c6963a738020404e2d821020100020100302a300f06082b06010201010300430301
        54453017060a2b06010603010104010006092b0601060301010501";

    /// `snmpinform -v 2c -c public … 999 linkUp ifIndex.5 i 5`
    const V2C_INFORM: &str = "
        305502010104067075626c6963a6480204527cdd17020100020100303a300e06082b06010201010300430203
        e73017060a2b06010603010104010006092b0601060301010504300f060a2b060102010202010105020105";

    #[test]
    fn v1_link_down() {
        let trap = decode_trap(&hex(V1_LINK_DOWN), source()).unwrap();
        assert_eq!(
            trap,
            Trap {
                version: 1,
                community: "public".to_owned(),
                source: source(),
                uptime_ticks: Some(12_345),
                trap_oid: Some(oid("1.3.6.1.6.3.1.1.5.3")),
                enterprise: Some(oid("1.3.6.1.4.1.8072.3.2.10")),
                agent_addr: Some([192, 0, 2, 1]),
                generic: Some(2),
                specific: Some(0),
                varbinds: vec![
                    VarBind::new(oid("1.3.6.1.2.1.2.2.1.1.3"), Value::Integer(3)),
                    VarBind::new(oid("1.3.6.1.2.1.2.2.1.7.3"), Value::Integer(2)),
                    VarBind::new(oid("1.3.6.1.2.1.2.2.1.8.3"), Value::Integer(2)),
                ],
                is_inform: false,
            }
        );
        assert_eq!(
            names::trap_name(trap.trap_oid.as_ref().unwrap()),
            Some("linkDown")
        );
    }

    #[test]
    fn v1_enterprise_specific_maps_to_the_v2_notification() {
        let trap = decode_trap(&hex(V1_ENTERPRISE_SPECIFIC), source()).unwrap();
        assert_eq!(trap.version, 1);
        assert_eq!(trap.enterprise, Some(oid("1.3.6.1.4.1.9.9.43.2")));
        assert_eq!(trap.agent_addr, Some([192, 0, 2, 7]));
        assert_eq!((trap.generic, trap.specific), (Some(6), Some(1)));
        assert_eq!(trap.uptime_ticks, Some(4242));
        // enterprise.0.specific — which is exactly how CISCO-CONFIG-MAN-MIB registers it.
        assert_eq!(trap.trap_oid, Some(oid("1.3.6.1.4.1.9.9.43.2.0.1")));
        assert_eq!(
            names::trap_name(trap.trap_oid.as_ref().unwrap()),
            Some("ciscoConfigManEvent")
        );
        assert_eq!(trap.varbinds.len(), 1);
    }

    #[test]
    fn every_generic_v1_trap_maps_to_snmp_traps() {
        let enterprise = oid("1.3.6.1.4.1.99999");
        let expected = [
            "coldStart",
            "warmStart",
            "linkDown",
            "linkUp",
            "authenticationFailure",
            "egpNeighborLoss",
        ];
        for (generic, name) in (0u32..).zip(expected) {
            let mapped = v2_trap_oid(&enterprise, generic, 42);
            assert_eq!(mapped, oid("1.3.6.1.6.3.1.1.5").child(generic + 1));
            assert_eq!(names::trap_name(&mapped), Some(name));
        }
        assert_eq!(
            v2_trap_oid(&enterprise, 6, 42),
            oid("1.3.6.1.4.1.99999.0.42")
        );
    }

    #[test]
    fn v2c_link_up() {
        let trap = decode_trap(&hex(V2C_LINK_UP), source()).unwrap();
        assert_eq!(
            trap,
            Trap {
                version: 2,
                community: "public".to_owned(),
                source: source(),
                uptime_ticks: Some(54_321),
                trap_oid: Some(oid("1.3.6.1.6.3.1.1.5.4")),
                enterprise: None,
                agent_addr: None,
                generic: None,
                specific: None,
                varbinds: vec![
                    VarBind::new(oid("1.3.6.1.2.1.2.2.1.1.3"), Value::Integer(3)),
                    VarBind::new(oid("1.3.6.1.2.1.2.2.1.7.3"), Value::Integer(1)),
                    VarBind::new(oid("1.3.6.1.2.1.2.2.1.8.3"), Value::Integer(1)),
                    VarBind::new(oid("1.3.6.1.2.1.31.1.1.1.1.3"), Value::from("eth3")),
                ],
                is_inform: false,
            }
        );
        assert_eq!(
            names::trap_name(trap.trap_oid.as_ref().unwrap()),
            Some("linkUp")
        );
    }

    #[test]
    fn v2c_cold_start() {
        let trap = decode_trap(&hex(V2C_COLD_START), source()).unwrap();
        assert_eq!(trap.version, 2);
        assert_eq!(trap.uptime_ticks, Some(87_109));
        assert_eq!(trap.trap_oid, Some(oid("1.3.6.1.6.3.1.1.5.1")));
        assert!(trap.varbinds.is_empty());
        assert!(!trap.is_inform);
        assert_eq!(
            names::trap_name(trap.trap_oid.as_ref().unwrap()),
            Some("coldStart")
        );
    }

    #[test]
    fn v2c_inform_and_its_acknowledgement() {
        let datagram = hex(V2C_INFORM);
        let trap = decode_trap(&datagram, source()).unwrap();
        assert!(trap.is_inform);
        assert_eq!(trap.version, 2);
        assert_eq!(trap.uptime_ticks, Some(999));
        assert_eq!(trap.trap_oid, Some(oid("1.3.6.1.6.3.1.1.5.4")));
        assert_eq!(
            trap.varbinds,
            vec![VarBind::new(
                oid("1.3.6.1.2.1.2.2.1.1.5"),
                Value::Integer(5)
            )]
        );

        // The acknowledgement is the same message with the PDU tag changed to Response.
        let mut expected = datagram.clone();
        assert_eq!(expected[13], 0xa6);
        expected[13] = 0xa2;
        assert_eq!(inform_response(&datagram).unwrap(), expected);
    }

    #[test]
    fn only_informs_are_acknowledged() {
        for not_an_inform in [V2C_LINK_UP, V1_LINK_DOWN, V2C_COLD_START] {
            assert!(matches!(
                inform_response(&hex(not_an_inform)),
                Err(SnmpError::Decode(_))
            ));
        }
        assert!(matches!(
            inform_response(&hex(crate::v3::tests::NETSNMP_V3_TRAP)),
            Err(SnmpError::Unsupported(_))
        ));
        assert!(matches!(inform_response(&[]), Err(SnmpError::Decode(_))));
    }

    #[test]
    fn v3_unauthenticated_trap() {
        let trap = decode_trap(&hex(crate::v3::tests::NETSNMP_V3_TRAP), source()).unwrap();
        assert_eq!(trap.version, 3);
        assert_eq!(trap.community, "trapuser");
        assert_eq!(trap.uptime_ticks, Some(777));
        assert_eq!(trap.trap_oid, Some(oid("1.3.6.1.6.3.1.1.5.3")));
        assert_eq!(
            trap.varbinds,
            vec![VarBind::new(
                oid("1.3.6.1.2.1.2.2.1.1.2"),
                Value::Integer(2)
            )]
        );
        assert!(!trap.is_inform);
    }

    #[test]
    fn v3_protected_traps_are_unsupported() {
        use crate::pdu::PduKind;
        use crate::v3::usm::{AuthProtocol, PrivProtocol};
        use crate::v3::{FLAG_AUTH, FLAG_PRIV, OutgoingMessage, ScopedPdu};

        let engine_id = b"\x80\x00\x1f\x88\x04trap-sender";
        let scoped = ScopedPdu {
            context_engine_id: engine_id.to_vec(),
            context_name: vec![],
            pdu: Pdu {
                kind: PduKind::TrapV2,
                request_id: 1,
                error_status: 0,
                error_index: 0,
                varbinds: vec![],
            },
        };
        let keys =
            crate::v3::tests::test_keys(AuthProtocol::Sha1, Some(PrivProtocol::Aes128), engine_id);
        for flags in [FLAG_AUTH, FLAG_AUTH | FLAG_PRIV] {
            let datagram = OutgoingMessage {
                msg_id: 5,
                flags,
                engine_id,
                engine_boots: 1,
                engine_time: 2,
                user: b"trapper",
                scoped_pdu: &scoped,
            }
            .encode(Some(&keys), 9)
            .unwrap();
            assert!(matches!(
                decode_trap(&datagram, source()),
                Err(SnmpError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn non_notifications_are_rejected() {
        // A GET request and a Response are not traps.
        let get = hex(
            "30290201010406 7075626c6963 a01c 0204 4205f751 020100 020100 300e 300c 0608 2b06010201010500 0500",
        );
        assert!(matches!(
            decode_trap(&get, source()),
            Err(SnmpError::Decode(_))
        ));
        let response = inform_response(&hex(V2C_INFORM)).unwrap();
        assert!(matches!(
            decode_trap(&response, source()),
            Err(SnmpError::Decode(_))
        ));
        // Unknown version, garbage, nothing.
        assert!(decode_trap(&hex("3003020102"), source()).is_err());
        assert!(decode_trap(&[0x42; 40], source()).is_err());
        assert!(decode_trap(&[], source()).is_err());
    }

    #[test]
    fn sloppy_notifications_keep_what_they_have() {
        let encode = |varbinds: Vec<VarBind>| {
            encode_community_message(
                CommunityVersion::V2c,
                b"c",
                &Pdu {
                    kind: PduKind::TrapV2,
                    request_id: 1,
                    error_status: 0,
                    error_index: 0,
                    varbinds,
                },
            )
        };
        let payload = VarBind::new(oid("1.3.6.1.2.1.2.2.1.1.9"), Value::Integer(9));
        let trap_oid = Value::Oid(oid("1.3.6.1.6.3.1.1.5.3"));

        // No sysUpTime at all: the trap OID is still recognised.
        let trap = decode_trap(
            &encode(vec![
                VarBind::new(oid("1.3.6.1.6.3.1.1.4.1.0"), trap_oid.clone()),
                payload.clone(),
            ]),
            source(),
        )
        .unwrap();
        assert_eq!(trap.uptime_ticks, None);
        assert_eq!(trap.trap_oid, Some(oid("1.3.6.1.6.3.1.1.5.3")));
        assert_eq!(trap.varbinds, vec![payload.clone()]);

        // Instance sub-identifiers forgotten.
        let trap = decode_trap(
            &encode(vec![
                VarBind::new(oid("1.3.6.1.2.1.1.3"), Value::TimeTicks(5)),
                VarBind::new(oid("1.3.6.1.6.3.1.1.4.1"), trap_oid.clone()),
            ]),
            source(),
        )
        .unwrap();
        assert_eq!(trap.uptime_ticks, Some(5));
        assert_eq!(trap.trap_oid, Some(oid("1.3.6.1.6.3.1.1.5.3")));
        assert!(trap.varbinds.is_empty());

        // Nothing recognisable: everything stays in the bindings.
        let odd = vec![
            payload.clone(),
            VarBind::new(oid("1.3.6.1.2.1.1.3.0"), Value::TimeTicks(5)),
        ];
        let trap = decode_trap(&encode(odd.clone()), source()).unwrap();
        assert_eq!((trap.uptime_ticks, trap.trap_oid.clone()), (None, None));
        assert_eq!(trap.varbinds, odd);

        // An empty notification is still a notification.
        let trap = decode_trap(&encode(vec![]), source()).unwrap();
        assert_eq!((trap.uptime_ticks, trap.trap_oid), (None, None));
    }

    #[test]
    fn traps_serialize() {
        let trap = decode_trap(&hex(V1_LINK_DOWN), source()).unwrap();
        let json = serde_json::to_string(&trap).unwrap();
        assert!(
            json.contains(r#""trap_oid":"1.3.6.1.6.3.1.1.5.3""#),
            "{json}"
        );
        assert!(json.contains(r#""source":"192.0.2.77:50123""#), "{json}");
        assert_eq!(serde_json::from_str::<Trap>(&json).unwrap(), trap);
    }
}
