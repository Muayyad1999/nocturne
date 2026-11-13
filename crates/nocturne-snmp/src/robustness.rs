//! Hostile input for every decoder in the crate: random bytes, random-but-plausible BER,
//! every truncation and thousands of single-byte corruptions of real datagrams. Nothing may
//! panic, hang or allocate wildly, whatever arrives; a decoder either returns a value or an
//! error.
//!
//! The generators here are shared with the tests of `agent` and `client`, whose decoders are
//! private to those modules.

use crate::ber::{self, Reader, Writer};
use crate::pdu::{
    AnyPdu, CommunityMessage, CommunityVersion, Envelope, Pdu, PduKind, encode_community_message,
    read_value, read_varbinds,
};
use crate::v3::usm::{AuthProtocol, LocalizedKeys, PrivProtocol};
use crate::v3::{FLAG_AUTH, FLAG_PRIV, FLAG_REPORTABLE, OutgoingMessage, ScopedPdu, V3Message};
use crate::{Oid, SnmpError, Value, VarBind, decode_trap, inform_response};

/// A small deterministic generator (xorshift64*): the tests must fail the same way twice.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    pub(crate) fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    pub(crate) fn byte(&mut self) -> u8 {
        self.next() as u8
    }

    pub(crate) fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.byte()).collect()
    }

    /// Random bytes of a random length below `max_len`.
    pub(crate) fn blob(&mut self, max_len: usize) -> Vec<u8> {
        let len = self.below(max_len);
        self.bytes(len)
    }
}

fn hex(s: &str) -> Vec<u8> {
    let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

pub(crate) const ENGINE_ID: &[u8] = b"\x80\x00\x1f\x88\x04robustness";

/// Keys the corpus' SNMPv3 messages are protected with.
pub(crate) fn keys(privacy: PrivProtocol) -> LocalizedKeys {
    crate::v3::tests::test_keys(AuthProtocol::Sha256, Some(privacy), ENGINE_ID)
}

const ALL_PRIV: [PrivProtocol; 6] = [
    PrivProtocol::Des,
    PrivProtocol::Aes128,
    PrivProtocol::Aes192,
    PrivProtocol::Aes256,
    PrivProtocol::Aes192Cisco,
    PrivProtocol::Aes256Cisco,
];

fn sample_pdu(kind: PduKind) -> Pdu {
    let oid = |s: &str| s.parse::<Oid>().unwrap();
    Pdu {
        kind,
        request_id: 0x1234_5678,
        error_status: 0,
        error_index: 0,
        varbinds: vec![
            VarBind::new(oid("1.3.6.1.2.1.1.3.0"), Value::TimeTicks(4242)),
            VarBind::new(
                oid("1.3.6.1.6.3.1.1.4.1.0"),
                Value::Oid(oid("1.3.6.1.6.3.1.1.5.3")),
            ),
            VarBind::new(oid("1.3.6.1.2.1.1.5.0"), Value::from("core-rtr-1")),
            VarBind::new(oid("1.3.6.1.2.1.2.2.1.10.1"), Value::Counter32(u32::MAX)),
            VarBind::new(oid("1.3.6.1.2.1.31.1.1.1.6.1"), Value::Counter64(u64::MAX)),
            VarBind::new(
                oid("1.3.6.1.2.1.4.20.1.1.10.0.0.1"),
                Value::IpAddress([10, 0, 0, 1]),
            ),
            VarBind::new(oid("1.3.6.1.2.1.2.2.1.5.1"), Value::Gauge32(1_000_000_000)),
            VarBind::new(oid("1.3.6.1.2.1.2.2.1.7.1"), Value::Integer(-1)),
            VarBind::new(
                oid("1.3.6.1.4.1.2021.10.1.6.1"),
                Value::Opaque(vec![0x9f, 0x78, 4, 0, 0, 0, 0]),
            ),
            VarBind::new(oid("1.3.6.1.2.1.1.9.0"), Value::NoSuchInstance),
            VarBind::new(oid("1.3.6.1.2.1.1.10.0"), Value::Null),
        ],
    }
}

/// Real datagrams of every kind the crate decodes: the net-snmp captures used elsewhere in
/// the unit tests, plus one message of our own for each PDU type and each privacy protocol.
pub(crate) fn corpus() -> Vec<Vec<u8>> {
    let mut corpus = vec![
        // snmpget -v2c: request and response
        hex("30370201010406 7075626c6963 a02a 02043e288c1d 020100 020100 301c
             300c06082b060102010105000500 300c06082b060102010106000500"),
        hex("30500201010406 7075626c6963 a243 02043e288c1d 020100 020100 3035
             301906082b06010201010500040d6e6f637475726e652d74657374
             301806082b06010201010600040c4e6f637475726e65206c6162"),
        // snmptrap -v 1 (linkDown) and (enterprise specific)
        hex("305e02010004067075626c6963a451060a2b06010401bf0803020a4004c0000201020102020100430230
             393033300f060a2b060102010202010103020103300f060a2b060102010202010703020102300f060a2b
             060102010202010803020102"),
        hex("303f02010004067075626c6963a43206092b0601040109092b024004c00002070201060201014302109230
             153013060e2b0601040109092b010106010305020101"),
        // snmptrap -v 2c (coldStart), snmpinform -v 2c
        hex("304502010104067075626c6963a738020404e2d821020100020100302a300f06082b06010201010300430301
             54453017060a2b06010603010104010006092b0601060301010501"),
        hex("305502010104067075626c6963a6480204527cdd17020100020100303a300e06082b06010201010300430203
             e73017060a2b06010603010104010006092b0601060301010504300f060a2b060102010202010105020105"),
        // snmptrap -v 3 noAuthNoPriv, and net-snmp's engine discovery probe
        hex(crate::v3::tests::NETSNMP_V3_TRAP),
        hex("303e020103301102043b64752c020300ffe3040104020103041030 0e0400020100020100040004000400
             30140400 0400 a00e 020435ddbe2a 020100 020100 3000"),
    ];

    for kind in [
        PduKind::Get,
        PduKind::GetNext,
        PduKind::Response,
        PduKind::Set,
        PduKind::GetBulk,
        PduKind::Inform,
        PduKind::TrapV2,
        PduKind::Report,
    ] {
        for version in [CommunityVersion::V1, CommunityVersion::V2c] {
            corpus.push(encode_community_message(
                version,
                b"public",
                &sample_pdu(kind),
            ));
        }
    }

    let scoped = ScopedPdu {
        context_engine_id: ENGINE_ID.to_vec(),
        context_name: b"ctx".to_vec(),
        pdu: sample_pdu(PduKind::Response),
    };
    let message = |flags| OutgoingMessage {
        msg_id: 0x0102_0304,
        flags,
        engine_id: ENGINE_ID,
        engine_boots: 3,
        engine_time: 12_345,
        user: b"operator",
        scoped_pdu: &scoped,
    };
    corpus.push(message(FLAG_REPORTABLE).encode(None, 1).unwrap());
    corpus.push(
        message(FLAG_AUTH)
            .encode(Some(&keys(PrivProtocol::Aes128)), 2)
            .unwrap(),
    );
    for privacy in ALL_PRIV {
        corpus.push(
            message(FLAG_AUTH | FLAG_PRIV)
                .encode(Some(&keys(privacy)), 3)
                .unwrap(),
        );
    }
    corpus
}

/// Random BER that looks enough like SNMP to get past the first checks of a decoder.
fn plausible_ber(rng: &mut Rng, depth: u32) -> Vec<u8> {
    const TAGS: [u8; 22] = [
        0x02, 0x02, 0x04, 0x04, 0x05, 0x06, 0x30, 0x30, 0x30, 0x40, 0x41, 0x42, 0x43, 0x44, 0x46,
        0x80, 0x81, 0x82, 0xa0, 0xa2, 0xa4, 0xa7,
    ];
    let tag = TAGS[rng.below(TAGS.len())];
    let constructed = tag == 0x30 || tag >= 0xa0;
    let mut content = Vec::new();
    if constructed && depth < 5 {
        for _ in 0..rng.below(5) {
            content.extend(plausible_ber(rng, depth + 1));
        }
    } else {
        content = match rng.below(4) {
            0 => Vec::new(),
            1 => vec![rng.byte()],
            2 => rng.blob(6),
            _ => rng.blob(40),
        };
    }
    let mut w = Writer::new();
    w.octets(tag, &content);
    let mut bytes = w.into_bytes();
    // Now and then lie about the length.
    if rng.below(8) == 0 && bytes.len() > 1 {
        bytes[1] = rng.byte();
    }
    bytes
}

/// A message skeleton around random BER: version, community or header, then the noise.
fn plausible_message(rng: &mut Rng) -> Vec<u8> {
    let mut w = Writer::new();
    w.sequence(|w| {
        w.integer([0, 1, 3, 3, 2, -1][rng.below(6)]);
        match rng.below(3) {
            0 => w.octet_string(b"public"),
            1 => w.sequence(|w| {
                w.integer(rng.next() as i32 as i64);
                w.integer(65_507);
                w.octet_string(&[rng.byte() & 0x07]);
                w.integer([3, 3, 3, 0, 4][rng.below(5)]);
            }),
            _ => {}
        }
        for _ in 0..rng.below(4) {
            let noise = plausible_ber(rng, 0);
            if rng.below(3) == 0 {
                w.octet_string(&noise);
            } else {
                w.raw(&noise);
            }
        }
    });
    w.into_bytes()
}

/// Everything hostile, deterministically: about 30 000 datagrams.
pub(crate) fn hostile_inputs() -> Vec<Vec<u8>> {
    let mut rng = Rng::new(0x5eed_cafe_f00d_0001);
    let corpus = corpus();
    let mut inputs: Vec<Vec<u8>> = vec![Vec::new(), vec![0x30], vec![0x30, 0x00], vec![0x30, 0x80]];

    // Pure noise of every small length.
    for len in 0..400 {
        inputs.push(rng.bytes(len));
    }
    for _ in 0..3000 {
        let len = rng.below(120);
        inputs.push(rng.bytes(len));
    }
    // Noise with structure.
    for _ in 0..4000 {
        inputs.push(plausible_ber(&mut rng, 0));
        inputs.push(plausible_message(&mut rng));
    }
    for datagram in &corpus {
        // Every truncation.
        for len in 0..datagram.len() {
            inputs.push(datagram[..len].to_vec());
        }
        // Every byte corrupted, three ways.
        for position in 0..datagram.len() {
            for corruption in 0..3 {
                let mut mutated = datagram.clone();
                mutated[position] = match corruption {
                    0 => mutated[position] ^ (1 << rng.below(8)),
                    1 => 0xff,
                    _ => rng.byte(),
                };
                inputs.push(mutated);
            }
        }
        // Bytes removed, inserted and doubled.
        for _ in 0..40 {
            let mut mutated = datagram.clone();
            let position = rng.below(mutated.len());
            match rng.below(3) {
                0 => {
                    mutated.remove(position);
                }
                1 => mutated.insert(position, rng.byte()),
                _ => {
                    let tail = mutated[position..].to_vec();
                    mutated.extend(tail);
                }
            }
            inputs.push(mutated);
        }
        // Trailing rubbish.
        let mut padded = datagram.clone();
        padded.extend(rng.blob(20));
        padded.push(rng.byte());
        inputs.push(padded);
    }
    // Lengths that promise the moon.
    for claimed in [
        0x7fu8, 0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x88, 0x89, 0xfe, 0xff,
    ] {
        for tag in [0x30u8, 0x04, 0x02, 0x06, 0xa2] {
            let mut bytes = vec![tag, claimed];
            bytes.extend(std::iter::repeat_n(0xff, 12));
            inputs.push(bytes);
        }
    }
    // Nesting as deep as a datagram allows.
    inputs.push([0x30, 0x80].repeat(30_000));
    inputs.push([0x30, 0x82, 0xff, 0xff].repeat(15_000));
    let mut nested = vec![0x05, 0x00];
    for _ in 0..2000 {
        let mut w = Writer::new();
        w.octets(0x30, &nested);
        nested = w.into_bytes();
    }
    inputs.push(nested);
    inputs
}

#[test]
fn the_corpus_is_valid() {
    let source = "192.0.2.1:162".parse().unwrap();
    let corpus = corpus();
    assert!(corpus.len() >= 30);
    let mut traps = 0;
    for datagram in &corpus {
        let envelope = Envelope::open(datagram).expect("corpus datagram");
        match envelope.version {
            0 | 1 => {
                let message = CommunityMessage::decode(datagram).expect("community message");
                // Re-encoding a decoded message reproduces it byte for byte.
                if let AnyPdu::Standard(pdu) = &message.pdu {
                    let again = encode_community_message(message.version, &message.community, pdu);
                    assert_eq!(&again, datagram);
                }
            }
            3 => {
                let message = V3Message::decode(datagram).expect("v3 message");
                assert_eq!(message.message_len, datagram.len());
            }
            other => panic!("unexpected version {other}"),
        }
        traps += usize::from(decode_trap(datagram, source).is_ok());
    }
    // v1 traps, v2 traps and informs in both community versions, and the v3 trap.
    assert_eq!(traps, 9);
}

#[test]
fn no_decoder_panics_on_hostile_input() {
    let source = "192.0.2.1:162".parse().unwrap();
    let key_sets: Vec<LocalizedKeys> = ALL_PRIV.into_iter().map(keys).collect();
    let inputs = hostile_inputs();
    assert!(inputs.len() > 25_000, "{} inputs", inputs.len());

    let (mut community_ok, mut v3_ok, mut trap_ok, mut scoped_ok) = (0, 0, 0, 0);
    for input in &inputs {
        // BER primitives, as far as the input goes.
        let mut reader = Reader::new(input);
        while let Ok((_, mut content)) = reader.read_tlv() {
            let bytes = content.remaining();
            let _ = ber::decode_integer(bytes);
            let _ = ber::decode_integer32(bytes);
            let _ = ber::decode_unsigned32(bytes);
            let _ = ber::decode_unsigned64(bytes);
            let _ = ber::decode_oid(bytes);
            let _ = content.read_tlv();
        }
        let _ = Reader::new(input).integer();
        let _ = Reader::new(input).integer32();
        let _ = Reader::new(input).non_negative("field");
        let _ = Reader::new(input).octet_string();
        let _ = Reader::new(input).oid();
        let _ = Reader::new(input).sequence();
        let _ = read_value(&mut Reader::new(input));
        let _ = read_varbinds(&mut Reader::new(input));
        let _ = AnyPdu::decode(&mut Reader::new(input));

        // Messages.
        let _ = Envelope::open(input);
        if let Ok(message) = CommunityMessage::decode(input) {
            community_ok += 1;
            // What decodes also survives a round trip through our own encoder.
            if let AnyPdu::Standard(pdu) = &message.pdu {
                let again = encode_community_message(message.version, &message.community, pdu);
                let decoded = CommunityMessage::decode(&again).expect("our own encoding");
                assert_eq!(decoded, message);
            }
        }
        if let Ok(message) = V3Message::decode(input) {
            v3_ok += 1;
            assert!(message.message_len <= input.len());
            assert!(message.auth_params.end <= input.len());
            scoped_ok += usize::from(message.scoped_pdu(None).is_ok());
            for keys in &key_sets {
                let _ = message.verify(input, keys);
                let _ = message.scoped_pdu(Some(keys));
            }
        }

        // The public decoders.
        match decode_trap(input, source) {
            Ok(trap) => {
                trap_ok += 1;
                assert!(matches!(trap.version, 1..=3));
                let _ = format!("{trap:?}");
                for bind in &trap.varbinds {
                    let _ = bind.value.display();
                }
            }
            Err(SnmpError::Decode(_) | SnmpError::Unsupported(_)) => {}
            Err(other) => panic!("decode_trap returned {other:?}"),
        }
        match inform_response(input) {
            Ok(response) => {
                // An acknowledgement is itself a well-formed Response.
                let message = CommunityMessage::decode(&response).expect("our own response");
                assert!(matches!(
                    message.pdu,
                    AnyPdu::Standard(Pdu {
                        kind: PduKind::Response,
                        ..
                    })
                ));
            }
            Err(SnmpError::Decode(_) | SnmpError::Unsupported(_)) => {}
            Err(other) => panic!("inform_response returned {other:?}"),
        }
    }
    // The mutations are mild enough that plenty still decodes — the test is not vacuous.
    assert!(community_ok > 1000, "{community_ok}");
    assert!(v3_ok > 500, "{v3_ok}");
    assert!(trap_ok > 300, "{trap_ok}");
    assert!(scoped_ok > 50, "{scoped_ok}");
}

#[test]
fn every_truncation_of_a_valid_datagram_is_an_error() {
    for datagram in corpus() {
        for len in 0..datagram.len() {
            let cut = &datagram[..len];
            assert!(Envelope::open(cut).is_err(), "truncated to {len}");
            assert!(CommunityMessage::decode(cut).is_err());
            assert!(V3Message::decode(cut).is_err());
            assert!(decode_trap(cut, "192.0.2.1:162".parse().unwrap()).is_err());
            assert!(inform_response(cut).is_err());
        }
    }
}

#[test]
fn privacy_primitives_take_any_input() {
    let mut rng = Rng::new(0x00de_c0de);
    for _ in 0..3000 {
        let key = rng.blob(40);
        let salt = rng.blob(12);
        let data = rng.blob(80);
        for protocol in ALL_PRIV {
            let _ = protocol.decrypt(&key, rng.next() as u32, rng.next() as u32, &salt, &data);
            let _ = protocol.encrypt(
                &key,
                rng.next() as u32,
                rng.next() as u32,
                rng.next(),
                &data,
            );
        }
        for auth in [AuthProtocol::Md5, AuthProtocol::Sha1, AuthProtocol::Sha512] {
            let start = rng.below(100);
            let range = start..start + rng.below(60);
            assert!(!auth.verify(&key, &data, range) || data.len() >= auth.mac_len());
        }
    }
}

#[test]
fn text_parsers_take_any_input() {
    let mut rng = Rng::new(0x7e57);
    let alphabet: Vec<char> = "0123456789.:-_ aZifInOctetsMIB\u{e9}\u{2603}\0\n"
        .chars()
        .collect();
    for _ in 0..20_000 {
        let text: String = (0..rng.below(24))
            .map(|_| alphabet[rng.below(alphabet.len())])
            .collect();
        if let Ok(oid) = text.parse::<Oid>() {
            // What parses prints back to something that parses to the same OID.
            assert_eq!(oid.to_string().parse::<Oid>().unwrap(), oid);
            let _ = crate::names::describe(&oid);
            let _ = crate::names::trap_name(&oid);
            let _ = ber::check_oid(&oid);
        }
        if let Some(oid) = crate::names::resolve(&text) {
            assert!(!oid.is_empty());
        }
    }
    // Values of every kind render without complaint, whatever is in them.
    for _ in 0..5000 {
        let bytes = rng.blob(40);
        for value in [
            Value::OctetString(bytes.clone()),
            Value::Opaque(bytes.clone()),
        ] {
            let shown = value.display();
            if let Some(text) = value.as_text() {
                assert_eq!(text, shown);
            }
            let _ = value.as_f64();
        }
        let number = rng.next();
        for value in [
            Value::Integer(number as i64),
            Value::Counter32(number as u32),
            Value::Gauge32(number as u32),
            Value::TimeTicks(number as u32),
            Value::Counter64(number),
        ] {
            assert!(!value.display().is_empty());
            let _ = (value.as_u64(), value.as_i64(), value.as_f64());
        }
    }
}
