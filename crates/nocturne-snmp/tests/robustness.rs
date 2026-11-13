//! Malformed input, seen from outside the crate: random, truncated and corrupted datagrams
//! thrown at the public decoders, at a running agent and — through a relay that mangles the
//! agent's replies — at the client. Nothing may panic or hang; every call ends in a value or
//! an error.
//!
//! (The crate's own unit tests do the same to every internal decoder, down to the BER
//! primitives and the SNMPv3 cryptography.)

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use nocturne_snmp::{
    Agent, Auth, Client, ClientConfig, Oid, SnmpError, Trap, Value, VarBind, decode_trap,
    inform_response, names,
};
use tokio::net::UdpSocket;

/// xorshift64*: deterministic, so a failure can be reproduced.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound.max(1) as u64) as usize
    }

    fn bytes(&mut self, len: usize) -> Vec<u8> {
        (0..len).map(|_| self.next() as u8).collect()
    }

    /// `datagram` with one of several kinds of damage.
    fn mangle(&mut self, datagram: &[u8]) -> Vec<u8> {
        let mut out = datagram.to_vec();
        if out.is_empty() {
            return self.bytes(8);
        }
        let position = self.below(out.len());
        match self.below(7) {
            0 => out.truncate(position),
            1 => out[position] ^= 1 << self.below(8),
            2 => out[position] = self.next() as u8,
            3 => out[position] = 0xff,
            4 => {
                out.remove(position);
            }
            5 => out.insert(position, self.next() as u8),
            _ => {
                let junk = self.bytes(1 + position % 30);
                out.extend(junk);
            }
        }
        out
    }
}

fn hex(s: &str) -> Vec<u8> {
    let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn oid(text: &str) -> Oid {
    text.parse().unwrap()
}

/// Notifications and requests as net-snmp 5.9.4 put them on the wire.
fn captured() -> Vec<Vec<u8>> {
    [
        // snmptrap -v 1: linkDown, enterprise-specific
        "305e02010004067075626c6963a451060a2b06010401bf0803020a4004c0000201020102020100430230
         393033300f060a2b060102010202010103020103300f060a2b060102010202010703020102300f060a2b
         060102010202010803020102",
        "303f02010004067075626c6963a43206092b0601040109092b024004c00002070201060201014302109230
         153013060e2b0601040109092b010106010305020101",
        // snmptrap -v 2c: linkUp, coldStart; snmpinform -v 2c
        "30818e02010104067075626c6963a781800204266253580201000201003072300f06082b0601020101030043
         0300d4313017060a2b06010603010104010006092b0601060301010504300f060a2b06010201020201010302
         0103300f060a2b060102010202010703020101300f060a2b0601020102020108030201013013060b2b060102
         011f0101010103040465746833",
        "304502010104067075626c6963a738020404e2d821020100020100302a300f06082b06010201010300430301
         54453017060a2b06010603010104010006092b0601060301010501",
        "305502010104067075626c6963a6480204527cdd17020100020100303a300e06082b06010201010300430203
         e73017060a2b06010603010104010006092b0601060301010504300f060a2b060102010202010105020105",
        // snmptrap -v 3 noAuthNoPriv
        "30819b0201033011020406496271020300ffe3040100020103042230200408800000000102030402
         01010203015445040874726170757365720400040030 5f041180001f888094142f10fd62f66a0000
         00000400a748020458d53c91020100020100303a300e06082b06010201010300430203093017060a
         2b06010603010104010006092b0601060301010503300f060a2b060102010202010102020102",
        // snmpget -v2c (request and response), snmpbulkget -v2c (request)
        "30370201010406 7075626c6963 a02a 02043e288c1d 020100 020100 301c
         300c06082b060102010105000500 300c06082b060102010106000500",
        "30500201010406 7075626c6963 a243 02043e288c1d 020100 020100 3035
         301906082b06010201010500040d6e6f637475726e652d74657374
         301806082b06010201010600040c4e6f637475726e65206c6162",
        "302b02010104067075626c6963a51e020460bb6afa0201000201033010300e060a2b060102011f010101060500",
    ]
    .iter()
    .map(|text| hex(text))
    .collect()
}

fn source() -> SocketAddr {
    "192.0.2.1:162".parse().unwrap()
}

/// A decoded trap must be internally sane, whatever it was decoded from.
fn check_trap(trap: &Trap) {
    assert!(matches!(trap.version, 1..=3));
    assert_eq!(trap.source, source());
    if trap.version != 1 {
        assert!(trap.enterprise.is_none() && trap.generic.is_none() && trap.agent_addr.is_none());
    }
    if trap.generic.is_some() {
        assert!(trap.trap_oid.is_some() && trap.enterprise.is_some() && !trap.is_inform);
    }
    if let Some(trap_oid) = &trap.trap_oid {
        let _ = names::trap_name(trap_oid);
        let _ = names::describe(trap_oid);
    }
    for bind in &trap.varbinds {
        let _ = (
            bind.value.display(),
            bind.value.as_text(),
            bind.value.as_u64(),
        );
        let _ = names::describe(&bind.oid);
    }
    let _ = format!("{trap:?}");
}

fn try_decoders(datagram: &[u8]) -> (bool, bool) {
    let trap = match decode_trap(datagram, source()) {
        Ok(trap) => {
            check_trap(&trap);
            true
        }
        Err(SnmpError::Decode(_) | SnmpError::Unsupported(_)) => false,
        Err(other) => panic!("decode_trap returned {other:?} for {datagram:02x?}"),
    };
    let acknowledged = match inform_response(datagram) {
        Ok(response) => {
            // An acknowledgement is never itself a notification.
            assert!(decode_trap(&response, source()).is_err());
            true
        }
        Err(SnmpError::Decode(_) | SnmpError::Unsupported(_)) => false,
        Err(other) => panic!("inform_response returned {other:?} for {datagram:02x?}"),
    };
    (trap, acknowledged)
}

#[test]
fn trap_decoders_survive_random_truncated_and_corrupted_datagrams() {
    let mut rng = Rng(0x0dd_ba11);
    let corpus = captured();

    // The captures themselves: six notifications, one of them an inform.
    let results: Vec<(bool, bool)> = corpus
        .iter()
        .map(|datagram| try_decoders(datagram))
        .collect();
    assert_eq!(results.iter().filter(|(trap, _)| *trap).count(), 6);
    assert_eq!(
        results
            .iter()
            .filter(|(_, acknowledged)| *acknowledged)
            .count(),
        1
    );

    // Random bytes.
    for len in 0..300 {
        try_decoders(&rng.bytes(len));
    }
    for _ in 0..20_000 {
        let len = rng.below(100);
        try_decoders(&rng.bytes(len));
    }
    // Random bytes behind a plausible start.
    for datagram in &corpus {
        for keep in 0..datagram.len().min(48) {
            let mut bytes = datagram[..keep].to_vec();
            bytes.extend(rng.bytes(40));
            try_decoders(&bytes);
        }
    }

    let mut still_decoded = 0;
    for datagram in &corpus {
        // Every truncation is an error.
        for len in 0..datagram.len() {
            assert_eq!(
                try_decoders(&datagram[..len]),
                (false, false),
                "truncated to {len} octets"
            );
        }
        // Every byte set to every value.
        for position in 0..datagram.len() {
            for value in 0..=255u8 {
                let mut mutated = datagram.clone();
                mutated[position] = value;
                still_decoded += usize::from(try_decoders(&mutated).0);
            }
        }
        // Piled-up damage.
        for _ in 0..2000 {
            let mut mutated = rng.mangle(datagram);
            for _ in 0..rng.below(4) {
                mutated = rng.mangle(&mutated);
            }
            try_decoders(&mutated);
        }
    }
    // Plenty of single-byte changes leave a valid (different) notification behind.
    assert!(still_decoded > 10_000, "{still_decoded}");
}

fn lab_view() -> BTreeMap<Oid, Value> {
    let mut view = BTreeMap::new();
    view.insert(oid("1.3.6.1.2.1.1.1.0"), Value::from("simulated device"));
    view.insert(oid("1.3.6.1.2.1.1.3.0"), Value::TimeTicks(1));
    view.insert(oid("1.3.6.1.2.1.1.5.0"), Value::from("nocturne-test"));
    view.insert(oid("1.3.6.1.2.1.1.6.0"), Value::from("Nocturne lab"));
    for row in 1..=60u32 {
        view.insert(
            oid("1.3.6.1.2.1.2.2.1.2").child(row),
            Value::from(format!("port{row}")),
        );
        view.insert(
            oid("1.3.6.1.2.1.2.2.1.10").child(row),
            Value::Counter32(u32::MAX - row),
        );
        view.insert(
            oid("1.3.6.1.2.1.31.1.1.1.6").child(row),
            Value::Counter64(u64::MAX - u64::from(row)),
        );
    }
    view
}

async fn lab_agent() -> (Agent, tokio::task::JoinHandle<Result<(), SnmpError>>) {
    let agent = Agent::bind("127.0.0.1:0".parse().unwrap(), "public")
        .await
        .unwrap();
    agent.set_view(lab_view());
    let task = tokio::spawn(agent.clone().run());
    (agent, task)
}

async fn client_for(target: SocketAddr, timeout_ms: u64, retries: u32) -> Client {
    let mut config = ClientConfig::new(
        target,
        Auth::V2c {
            community: "public".into(),
        },
    );
    config.timeout = Duration::from_millis(timeout_ms);
    config.retries = retries;
    Client::connect(config).await.unwrap()
}

#[tokio::test]
async fn the_agent_keeps_serving_under_fire() {
    let (agent, task) = lab_agent().await;
    let target = agent.local_addr();
    let mut rng = Rng(0x000a_9e47);
    let attacker = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut client = client_for(target, 2000, 2).await;
    let sys_name = [oid("1.3.6.1.2.1.1.5.0")];

    // Real requests (the captured GET and GETBULK are addressed to community "public" too),
    // damaged in every way, with honest requests in between.
    let corpus = captured();
    let mut sent = 0;
    for round in 0..4000 {
        let datagram = match round % 4 {
            0 => {
                let len = rng.below(200);
                rng.bytes(len)
            }
            _ => {
                let original = &corpus[rng.below(corpus.len())];
                rng.mangle(original)
            }
        };
        attacker.send_to(&datagram, target).await.unwrap();
        sent += 1;
        if round % 250 == 0 {
            let got = client.get(&sys_name).await.unwrap();
            assert_eq!(
                got[0].value,
                Value::from("nocturne-test"),
                "after {sent} hostile datagrams"
            );
        }
    }
    // Empty and maximal datagrams.
    attacker.send_to(&[], target).await.unwrap();
    attacker.send_to(&vec![0x30; 65_000], target).await.unwrap();

    // Still alive, still correct, and the task has not ended.
    let walked = client
        .walk(&oid("1.3.6.1.2.1.2.2.1.2"), usize::MAX)
        .await
        .unwrap();
    assert_eq!(walked.len(), 60);
    assert!(!task.is_finished());
    task.abort();
}

/// Forwards requests to the agent untouched and hands the replies to `damage` on their way
/// back.
async fn mangling_relay(
    agent: SocketAddr,
    mut damage: impl FnMut(Vec<u8>) -> Vec<Vec<u8>> + Send + 'static,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let front = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
    let addr = front.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let back = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let mut buf = vec![0u8; 65_535];
        loop {
            let Ok((len, client)) = front.recv_from(&mut buf).await else {
                return;
            };
            back.send_to(&buf[..len], agent).await.unwrap();
            let reply =
                tokio::time::timeout(Duration::from_millis(500), back.recv_from(&mut buf)).await;
            let Ok(Ok((len, _))) = reply else {
                continue;
            };
            for datagram in damage(buf[..len].to_vec()) {
                let _ = front.send_to(&datagram, client).await;
            }
        }
    });
    (addr, task)
}

#[tokio::test]
async fn the_client_survives_mangled_replies() {
    let (agent, agent_task) = lab_agent().await;
    let expected = lab_view();

    // 1. Every reply is preceded by a damaged copy of itself. The copy is either rejected
    //    (and the genuine reply used) or — when the damage left a well-formed response —
    //    taken at face value; either way the call returns.
    let mut rng = Rng(0xc4a05);
    let (addr, relay) = mangling_relay(agent.local_addr(), move |reply| {
        vec![rng.mangle(&reply), reply]
    })
    .await;
    let mut client = client_for(addr, 200, 1).await;
    let (mut ok, mut failed) = (0, 0);
    for round in 0..300u32 {
        let row = round % 60 + 1;
        let wanted = [
            oid("1.3.6.1.2.1.2.2.1.2").child(row),
            oid("1.3.6.1.2.1.31.1.1.1.6").child(row),
        ];
        let outcome = match round % 5 {
            0 => client.get(&wanted).await.map(|binds| binds.len()),
            1 => client.get_next(&wanted).await.map(|binds| binds.len()),
            2 => client
                .get_bulk(&wanted, 0, 5)
                .await
                .map(|binds| binds.len()),
            3 => client
                .walk(&oid("1.3.6.1.2.1.1"), 50)
                .await
                .map(|binds| binds.len()),
            _ => client
                .table(&[oid("1.3.6.1.2.1.2.2.1.2")], 10)
                .await
                .map(|rows| rows.len()),
        };
        match outcome {
            Ok(_) => ok += 1,
            Err(SnmpError::Decode(_) | SnmpError::Agent { .. } | SnmpError::Timeout) => failed += 1,
            Err(other) => panic!("unexpected {other:?}"),
        }
    }
    assert!(ok > 100, "{ok} succeeded, {failed} failed");
    relay.abort();

    // 2. Only damage, never the genuine reply: calls end with an error (or with whatever a
    //    still-well-formed reply says), never with a hang.
    let mut rng = Rng(0xfa11);
    let (addr, relay) =
        mangling_relay(agent.local_addr(), move |reply| vec![rng.mangle(&reply)]).await;
    let mut client = client_for(addr, 30, 0).await;
    let sys_name = [oid("1.3.6.1.2.1.1.5.0")];
    for _ in 0..200 {
        let call = client.get(&sys_name);
        let _ = tokio::time::timeout(Duration::from_secs(5), call)
            .await
            .expect("the call must return");
    }
    relay.abort();

    // 3. Replies with their last octet cut off. The outer length no longer fits, so such a
    //    datagram cannot even be attributed to the request: it is ignored like any other
    //    stray, and the request times out.
    let (addr, relay) = mangling_relay(agent.local_addr(), |mut reply| {
        reply.truncate(reply.len() - 1);
        vec![reply]
    })
    .await;
    let mut client = client_for(addr, 100, 0).await;
    let error = client.get(&[oid("1.3.6.1.2.1.1.5.0")]).await.unwrap_err();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    relay.abort();

    // 4. A clean relay, for comparison: everything matches the agent's view.
    let (addr, relay) = mangling_relay(agent.local_addr(), |reply| vec![reply]).await;
    let mut client = client_for(addr, 1000, 1).await;
    let walked = client.walk(&oid("1.3.6.1"), usize::MAX).await.unwrap();
    let expected: Vec<VarBind> = expected
        .into_iter()
        .map(|(oid, value)| VarBind::new(oid, value))
        .collect();
    assert_eq!(walked, expected);
    relay.abort();
    agent_task.abort();
}

#[test]
fn text_and_serde_inputs_are_handled() {
    let mut rng = Rng(0x7e47);
    let alphabet: Vec<char> = "0123456789.:-_ ifInOctetsIFMB\u{e9}\u{2603}\0\n\"{}[],"
        .chars()
        .collect();
    let mut parsed = 0;
    for _ in 0..50_000 {
        let text: String = (0..rng.below(20))
            .map(|_| alphabet[rng.below(alphabet.len())])
            .collect();
        if let Ok(oid) = text.parse::<Oid>() {
            parsed += 1;
            assert_eq!(oid.to_string().parse::<Oid>().unwrap(), oid);
            let _ = names::describe(&oid);
        }
        let _ = names::resolve(&text);
        let _ = serde_json::from_str::<Oid>(&text);
        let _ = serde_json::from_str::<Value>(&text);
        let _ = serde_json::from_str::<VarBind>(&text);
        let _ = serde_json::from_str::<Auth>(&text);
        let _ = serde_json::from_str::<Trap>(&text);
    }
    assert!(parsed > 100, "{parsed}");

    // What we serialise, we read back.
    for datagram in captured() {
        if let Ok(trap) = decode_trap(&datagram, source()) {
            let json = serde_json::to_string(&trap).unwrap();
            assert_eq!(serde_json::from_str::<Trap>(&json).unwrap(), trap);
        }
    }
}
