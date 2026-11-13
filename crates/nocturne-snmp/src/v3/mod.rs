//! SNMPv3 message framing (RFC 3412) with the USM security parameters (RFC 3414).
//!
//! ```text
//! SEQUENCE {
//!   msgVersion            INTEGER (3),
//!   msgGlobalData         SEQUENCE { msgID, msgMaxSize, msgFlags OCTET STRING (1), msgSecurityModel },
//!   msgSecurityParameters OCTET STRING containing
//!                         SEQUENCE { engineID, engineBoots, engineTime, userName, authParams, privParams },
//!   msgData               ScopedPDU  -- or OCTET STRING holding the encrypted ScopedPDU
//! }
//! ScopedPDU ::= SEQUENCE { contextEngineID OCTET STRING, contextName OCTET STRING, PDU }
//! ```

pub(crate) mod usm;

use std::ops::Range;

use crate::SnmpError;
use crate::ber::{Reader, Result, Writer, tag};
use crate::pdu::{AnyPdu, Pdu, VERSION_3};
use usm::LocalizedKeys;

/// msgFlags bits.
pub(crate) const FLAG_AUTH: u8 = 0x01;
pub(crate) const FLAG_PRIV: u8 = 0x02;
pub(crate) const FLAG_REPORTABLE: u8 = 0x04;

/// msgSecurityModel of the User-based Security Model.
const SECURITY_MODEL_USM: i64 = 3;

/// The msgMaxSize we announce: the largest UDP payload over IPv4.
pub(crate) const MAX_MESSAGE_SIZE: u32 = 65_507;

/// The cleartext parts of a received SNMPv3 message.
#[derive(Debug)]
pub(crate) struct V3Message<'a> {
    pub msg_id: i32,
    pub flags: u8,
    pub engine_id: &'a [u8],
    pub engine_boots: u32,
    pub engine_time: u32,
    pub user: &'a [u8],
    /// Where msgAuthenticationParameters sits in the datagram.
    pub auth_params: Range<usize>,
    pub priv_params: &'a [u8],
    /// Length of the message proper; anything after it in the datagram is not part of it.
    pub message_len: usize,
    pub data: ScopedData<'a>,
}

/// msgData as found on the wire.
#[derive(Debug)]
pub(crate) enum ScopedData<'a> {
    /// A reader positioned at the plaintext ScopedPDU.
    Plain(Reader<'a>),
    Encrypted(&'a [u8]),
}

impl<'a> V3Message<'a> {
    pub fn decode(datagram: &'a [u8]) -> Result<V3Message<'a>> {
        let mut top = Reader::new(datagram);
        let mut body = top.sequence()?;
        let message_len = top.offset();
        let version = body.integer()?;
        if version != VERSION_3 {
            return Err(SnmpError::decode(format!(
                "not an SNMPv3 message (version {version})"
            )));
        }

        let mut header = body.sequence()?;
        let msg_id = header.integer32()?;
        let _max_size = header.integer()?;
        let flags = match header.octet_string()? {
            [flags] => *flags,
            other => {
                return Err(SnmpError::decode(format!(
                    "msgFlags must be one octet, found {}",
                    other.len()
                )));
            }
        };
        let security_model = header.integer()?;
        header.finish("msgGlobalData")?;
        if security_model != SECURITY_MODEL_USM {
            return Err(SnmpError::Unsupported(format!(
                "SNMPv3 security model {security_model} (only USM is supported)"
            )));
        }

        let mut security = body
            .expect(tag::OCTET_STRING, "msgSecurityParameters")?
            .sequence()?;
        let engine_id = security.octet_string()?;
        let engine_boots = security.non_negative("msgAuthoritativeEngineBoots")?;
        let engine_time = security.non_negative("msgAuthoritativeEngineTime")?;
        let user = security.octet_string()?;
        let auth = security.expect(tag::OCTET_STRING, "msgAuthenticationParameters")?;
        let auth_params = auth.offset()..auth.offset() + auth.remaining().len();
        let priv_params = security.octet_string()?;
        security.finish("the USM security parameters")?;

        let data = if flags & FLAG_PRIV != 0 {
            ScopedData::Encrypted(body.octet_string()?)
        } else {
            ScopedData::Plain(body)
        };

        Ok(V3Message {
            msg_id,
            flags,
            engine_id,
            engine_boots,
            engine_time,
            user,
            auth_params,
            priv_params,
            message_len,
            data,
        })
    }

    pub fn is_authenticated(&self) -> bool {
        self.flags & FLAG_AUTH != 0
    }

    pub fn is_encrypted(&self) -> bool {
        self.flags & FLAG_PRIV != 0
    }

    /// Checks the message digest against `keys`. `datagram` must be the buffer this message
    /// was decoded from.
    pub fn verify(&self, datagram: &[u8], keys: &LocalizedKeys) -> bool {
        datagram.get(..self.message_len).is_some_and(|message| {
            keys.auth_protocol
                .verify(&keys.auth_key, message, self.auth_params.clone())
        })
    }

    /// Decrypts (if necessary) and decodes the scoped PDU.
    pub fn scoped_pdu(&self, keys: Option<&LocalizedKeys>) -> Result<ScopedPdu> {
        match &self.data {
            ScopedData::Plain(reader) => {
                let mut reader = *reader;
                ScopedPdu::decode(&mut reader)
            }
            ScopedData::Encrypted(ciphertext) => {
                let (protocol, key) = keys
                    .and_then(|keys| keys.privacy.as_ref())
                    .ok_or_else(|| SnmpError::decode("encrypted message but no privacy key"))?;
                let plaintext = protocol.decrypt(
                    key,
                    self.engine_boots,
                    self.engine_time,
                    self.priv_params,
                    ciphertext,
                )?;
                // DES pads the plaintext; whatever follows the ScopedPDU is ignored.
                ScopedPdu::decode(&mut Reader::new(&plaintext))
            }
        }
    }
}

/// A PDU with its context.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScopedPdu {
    pub context_engine_id: Vec<u8>,
    pub context_name: Vec<u8>,
    pub pdu: Pdu,
}

impl ScopedPdu {
    pub fn decode(reader: &mut Reader<'_>) -> Result<ScopedPdu> {
        let mut scoped = reader.sequence()?;
        let context_engine_id = scoped.octet_string()?.to_vec();
        let context_name = scoped.octet_string()?.to_vec();
        let pdu = match AnyPdu::decode(&mut scoped)? {
            AnyPdu::Standard(pdu) => pdu,
            AnyPdu::TrapV1(_) => {
                return Err(SnmpError::decode(
                    "SNMPv1 Trap-PDU inside an SNMPv3 message",
                ));
            }
        };
        scoped.finish("the PDU")?;
        Ok(ScopedPdu {
            context_engine_id,
            context_name,
            pdu,
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.sequence(|w| {
            w.octet_string(&self.context_engine_id);
            w.octet_string(&self.context_name);
            self.pdu.encode(w);
        });
        w.into_bytes()
    }
}

/// Everything needed to build an SNMPv3 message.
#[derive(Debug)]
pub(crate) struct OutgoingMessage<'a> {
    pub msg_id: i32,
    /// msgFlags; the auth and priv bits decide what [`encode`](Self::encode) does.
    pub flags: u8,
    pub engine_id: &'a [u8],
    pub engine_boots: u32,
    pub engine_time: u32,
    pub user: &'a [u8],
    pub scoped_pdu: &'a ScopedPdu,
}

impl OutgoingMessage<'_> {
    /// Serialises the message, encrypting and authenticating it as its flags demand.
    ///
    /// `salt` feeds the privacy IV and must be unique per message for a given key.
    pub fn encode(&self, keys: Option<&LocalizedKeys>, salt: u64) -> Result<Vec<u8>> {
        let missing_keys = || {
            SnmpError::Auth("internal error: message needs keys that are not available".to_owned())
        };
        let authenticated = self.flags & FLAG_AUTH != 0;
        let encrypted = self.flags & FLAG_PRIV != 0;

        let scoped = self.scoped_pdu.encode();
        let (data, priv_params) = if encrypted {
            let (protocol, key) = keys
                .and_then(|keys| keys.privacy.as_ref())
                .ok_or_else(missing_keys)?;
            let (ciphertext, salt) =
                protocol.encrypt(key, self.engine_boots, self.engine_time, salt, &scoped)?;
            let mut w = Writer::new();
            w.octet_string(&ciphertext);
            (w.into_bytes(), salt.to_vec())
        } else {
            (scoped, Vec::new())
        };

        let auth_keys = if authenticated {
            Some(keys.ok_or_else(missing_keys)?)
        } else {
            None
        };
        // The digest is computed over the message with this field zeroed, then patched in.
        let placeholder = vec![0u8; auth_keys.map_or(0, |keys| keys.auth_protocol.mac_len())];

        let mut security = Writer::new();
        security.sequence(|w| {
            w.octet_string(self.engine_id);
            w.integer(i64::from(self.engine_boots));
            w.integer(i64::from(self.engine_time));
            w.octet_string(self.user);
            w.octet_string(&placeholder);
            w.octet_string(&priv_params);
        });
        let security = security.into_bytes();

        let mut w = Writer::new();
        w.sequence(|w| {
            w.integer(VERSION_3);
            w.sequence(|w| {
                w.integer(i64::from(self.msg_id));
                w.integer(i64::from(MAX_MESSAGE_SIZE));
                w.octet_string(&[self.flags]);
                w.integer(SECURITY_MODEL_USM);
            });
            w.octet_string(&security);
            w.raw(&data);
        });
        let mut message = w.into_bytes();

        if let Some(keys) = auth_keys {
            // Find the placeholder by parsing what was just written rather than by keeping
            // offsets in step with the encoder.
            let range = V3Message::decode(&message)?.auth_params;
            let mac = keys.auth_protocol.sign(&keys.auth_key, &message);
            message
                .get_mut(range)
                .filter(|field| field.len() == mac.len())
                .ok_or_else(|| SnmpError::decode("authentication field mismatch"))?
                .copy_from_slice(&mac);
        }
        Ok(message)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::usm::{AuthProtocol, PrivProtocol};
    use super::*;
    use crate::pdu::PduKind;
    use crate::{Oid, Value, VarBind};
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

    pub(crate) fn test_keys(
        auth: AuthProtocol,
        privacy: Option<PrivProtocol>,
        engine_id: &[u8],
    ) -> LocalizedKeys {
        let auth_master = auth.master_key(b"authentication-pass").unwrap();
        LocalizedKeys {
            auth_protocol: auth,
            auth_key: auth.localize(&auth_master, engine_id),
            privacy: privacy.map(|protocol| {
                let master = auth.master_key(b"privacy-pass").unwrap();
                (protocol, protocol.localized_key(auth, &master, engine_id))
            }),
        }
    }

    /// The discovery probe net-snmp's `snmpget -v3` sends, captured on the wire.
    const NETSNMP_DISCOVERY: &str = "
        30 3e 02 01 03 30 11 02 04 3b 64 75 2c 02 03 00
        ff e3 04 01 04 02 01 03 04 10 30 0e 04 00 02 01
        00 02 01 00 04 00 04 00 04 00 30 14 04 00 04 00
        a0 0e 02 04 35 dd be 2a 02 01 00 02 01 00 30 00";

    #[test]
    fn discovery_probe_matches_netsnmp_byte_for_byte() {
        let scoped = ScopedPdu {
            context_engine_id: vec![],
            context_name: vec![],
            pdu: Pdu::request(PduKind::Get, 0x35dd_be2a, &[]),
        };
        let message = OutgoingMessage {
            msg_id: 0x3b64_752c,
            flags: FLAG_REPORTABLE,
            engine_id: &[],
            engine_boots: 0,
            engine_time: 0,
            user: &[],
            scoped_pdu: &scoped,
        };
        let encoded = message.encode(None, 0).unwrap();
        assert_eq!(encoded, hex(NETSNMP_DISCOVERY));

        let decoded = V3Message::decode(&encoded).unwrap();
        assert_eq!(decoded.msg_id, 0x3b64_752c);
        assert_eq!(decoded.flags, FLAG_REPORTABLE);
        assert!(decoded.engine_id.is_empty() && decoded.user.is_empty());
        assert!(!decoded.is_authenticated() && !decoded.is_encrypted());
        assert_eq!(decoded.message_len, encoded.len());
        assert_eq!(decoded.scoped_pdu(None).unwrap(), scoped);
    }

    /// An SNMPv3 noAuthNoPriv trap sent by net-snmp's `snmptrap -v3 -e 0x8000000001020304`.
    pub(crate) const NETSNMP_V3_TRAP: &str = "
        30819b0201033011020406496271020300ffe3040100020103042230200408800000000102030402
        01010203015445040874726170757365720400040030 5f041180001f888094142f10fd62f66a0000
        00000400a748020458d53c91020100020100303a300e06082b06010201010300430203093017060a
        2b06010603010104010006092b0601060301010503300f060a2b060102010202010102020102";

    #[test]
    fn captured_v3_trap_decodes() {
        let bytes = hex(NETSNMP_V3_TRAP);
        let message = V3Message::decode(&bytes).unwrap();
        assert_eq!(message.flags, 0);
        assert_eq!(message.engine_id, hex("8000000001020304"));
        assert_eq!(message.engine_boots, 1);
        assert_eq!(message.engine_time, 0x01_5445);
        assert_eq!(message.user, b"trapuser");
        assert!(message.auth_params.is_empty());
        let scoped = message.scoped_pdu(None).unwrap();
        assert_eq!(
            scoped.context_engine_id,
            hex("80001f888094142f10fd62f66a00000000")
        );
        assert_eq!(scoped.pdu.kind, PduKind::TrapV2);
        assert_eq!(scoped.pdu.varbinds.len(), 3);
        assert_eq!(scoped.pdu.varbinds[0].value, Value::TimeTicks(777));
    }

    fn sample_pdu() -> ScopedPdu {
        ScopedPdu {
            context_engine_id: b"engine-under-test".to_vec(),
            context_name: b"ctx".to_vec(),
            pdu: Pdu {
                kind: PduKind::Response,
                request_id: 77,
                error_status: 0,
                error_index: 0,
                varbinds: vec![
                    VarBind::new(oid("1.3.6.1.2.1.1.5.0"), Value::from("core-rtr-1")),
                    VarBind::new(oid("1.3.6.1.2.1.31.1.1.1.6.1"), Value::Counter64(u64::MAX)),
                ],
            },
        }
    }

    const ALL_AUTH: [AuthProtocol; 6] = [
        AuthProtocol::Md5,
        AuthProtocol::Sha1,
        AuthProtocol::Sha224,
        AuthProtocol::Sha256,
        AuthProtocol::Sha384,
        AuthProtocol::Sha512,
    ];
    const ALL_PRIV: [PrivProtocol; 6] = [
        PrivProtocol::Des,
        PrivProtocol::Aes128,
        PrivProtocol::Aes192,
        PrivProtocol::Aes256,
        PrivProtocol::Aes192Cisco,
        PrivProtocol::Aes256Cisco,
    ];

    #[test]
    fn authenticated_and_encrypted_messages_round_trip() {
        let engine_id = b"engine-under-test";
        let scoped = sample_pdu();
        for auth in ALL_AUTH {
            for privacy in std::iter::once(None).chain(ALL_PRIV.into_iter().map(Some)) {
                let keys = test_keys(auth, privacy, engine_id);
                let flags = FLAG_AUTH | if privacy.is_some() { FLAG_PRIV } else { 0 };
                let message = OutgoingMessage {
                    msg_id: 1234,
                    flags,
                    engine_id,
                    engine_boots: 3,
                    engine_time: 4567,
                    user: b"operator",
                    scoped_pdu: &scoped,
                };
                let bytes = message.encode(Some(&keys), 0x1111_2222_3333_4444).unwrap();

                let decoded = V3Message::decode(&bytes).unwrap();
                assert_eq!(decoded.msg_id, 1234);
                assert_eq!(decoded.flags, flags);
                assert_eq!(decoded.engine_id, engine_id);
                assert_eq!((decoded.engine_boots, decoded.engine_time), (3, 4567));
                assert_eq!(decoded.user, b"operator");
                assert_eq!(decoded.auth_params.len(), auth.mac_len());
                assert_eq!(
                    decoded.priv_params.len(),
                    if privacy.is_some() { 8 } else { 0 }
                );
                assert!(decoded.verify(&bytes, &keys), "{auth:?}/{privacy:?}");
                assert_eq!(
                    decoded.scoped_pdu(Some(&keys)).unwrap(),
                    scoped,
                    "{auth:?}/{privacy:?}"
                );

                // Trailing bytes after the message are not part of the digest.
                let mut padded = bytes.clone();
                padded.extend_from_slice(&[0xde, 0xad]);
                assert!(V3Message::decode(&padded).unwrap().verify(&padded, &keys));

                // Keys localised to another engine, or a flipped bit, fail verification.
                let other = test_keys(auth, privacy, b"some-other-engine");
                assert!(!decoded.verify(&bytes, &other));
                let mut tampered = bytes.clone();
                let last = tampered.len() - 1;
                tampered[last] ^= 0x40;
                assert!(
                    !V3Message::decode(&tampered)
                        .unwrap()
                        .verify(&tampered, &keys)
                );

                // The plaintext of an encrypted message is not visible on the wire.
                if privacy.is_some() {
                    assert!(!bytes.windows(10).any(|w| w == b"core-rtr-1"));
                    assert!(decoded.scoped_pdu(None).is_err());
                    assert!(
                        decoded.scoped_pdu(Some(&other)).is_err()
                            || decoded.scoped_pdu(Some(&other)).unwrap() != scoped
                    );
                } else {
                    assert!(bytes.windows(10).any(|w| w == b"core-rtr-1"));
                }
            }
        }
    }

    #[test]
    fn encoding_needs_the_keys_its_flags_call_for() {
        let scoped = sample_pdu();
        let message = |flags| OutgoingMessage {
            msg_id: 1,
            flags,
            engine_id: b"engine",
            engine_boots: 1,
            engine_time: 1,
            user: b"u",
            scoped_pdu: &scoped,
        };
        assert!(message(FLAG_AUTH).encode(None, 0).is_err());
        let auth_only = test_keys(AuthProtocol::Sha1, None, b"engine");
        assert!(message(FLAG_AUTH).encode(Some(&auth_only), 0).is_ok());
        assert!(
            message(FLAG_AUTH | FLAG_PRIV)
                .encode(Some(&auth_only), 0)
                .is_err()
        );
        assert!(message(FLAG_REPORTABLE).encode(None, 0).is_ok());
    }

    #[test]
    fn malformed_headers_are_rejected() {
        // Wrong version.
        assert!(matches!(
            V3Message::decode(&hex("30 03 02 01 01")),
            Err(SnmpError::Decode(_))
        ));
        // msgFlags (`04 01 04` at offset 18) stretched to two octets.
        let mut two_flag_octets = hex(NETSNMP_DISCOVERY);
        assert_eq!(two_flag_octets[18..21], [0x04, 0x01, 0x04]);
        two_flag_octets[19] = 0x02;
        assert!(matches!(
            V3Message::decode(&two_flag_octets),
            Err(SnmpError::Decode(_))
        ));
        // Another security model (4 = the Transport Security Model).
        let mut other_model = hex(NETSNMP_DISCOVERY);
        assert_eq!(other_model[21..24], [0x02, 0x01, 0x03]);
        other_model[23] = 0x04;
        assert!(matches!(
            V3Message::decode(&other_model),
            Err(SnmpError::Unsupported(_))
        ));
        // Every truncation fails cleanly.
        let good = hex(NETSNMP_V3_TRAP);
        for len in 0..good.len() {
            let truncated = &good[..len];
            let result = V3Message::decode(truncated).and_then(|m| m.scoped_pdu(None));
            assert!(result.is_err(), "truncation to {len} bytes");
        }
    }
}
