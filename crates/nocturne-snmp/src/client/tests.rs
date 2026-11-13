//! Client tests against scripted responders: real UDP sockets on the loopback interface, with
//! the other end played by a closure that sees every datagram and decides what comes back.
//! This is where the client meets the misbehaviour a well-mannered agent never shows — lost
//! and late datagrams, forged replies, `tooBig`, reports, clocks that jump.
//! (`tests/loopback.rs` covers the in-crate agent, `tests/netsnmp_interop.rs` net-snmp.)

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pretty_assertions::assert_eq;
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

use super::*;
use crate::pdu::CommunityVersion::{V1, V2c};
use crate::v3::usm::LocalizedKeys;
use crate::v3::{FLAG_AUTH, FLAG_PRIV, OutgoingMessage, ScopedPdu, V3Message};

// ---------------------------------------------------------------------------------------------
// Support
// ---------------------------------------------------------------------------------------------

/// A UDP endpoint whose behaviour is a closure: for every datagram received, the datagrams
/// to send back.
struct Responder {
    addr: SocketAddr,
    received: Arc<Mutex<Vec<Vec<u8>>>>,
    task: JoinHandle<()>,
}

impl Responder {
    async fn start(script: impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static) -> Responder {
        Self::start_on(script, false).await
    }

    /// With `reply_from_elsewhere`, answers leave from a second socket — an agent that
    /// replies from another address than the one it was asked on.
    async fn start_on(
        mut script: impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static,
        reply_from_elsewhere: bool,
    ) -> Responder {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let other = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let addr = socket.local_addr().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        let task = tokio::spawn(async move {
            let mut buf = vec![0u8; 65_535];
            loop {
                let Ok((len, peer)) = socket.recv_from(&mut buf).await else {
                    return;
                };
                log.lock().unwrap().push(buf[..len].to_vec());
                for reply in script(&buf[..len]) {
                    let out = if reply_from_elsewhere {
                        &other
                    } else {
                        &socket
                    };
                    out.send_to(&reply, peer).await.unwrap();
                }
            }
        });
        Responder {
            addr,
            received,
            task,
        }
    }

    fn received(&self) -> Vec<Vec<u8>> {
        self.received.lock().unwrap().clone()
    }

    /// The PDUs of the community requests received so far.
    fn requests(&self) -> Vec<Pdu> {
        self.received()
            .iter()
            .map(|datagram| request(datagram))
            .collect()
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn oid(s: &str) -> Oid {
    s.parse().unwrap()
}

fn oids(list: &[&str]) -> Vec<Oid> {
    list.iter().map(|s| oid(s)).collect()
}

fn int(name: &str, value: i64) -> VarBind {
    VarBind::new(oid(name), Value::Integer(value))
}

fn hex(s: &str) -> Vec<u8> {
    let digits: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

/// The PDU of a community request datagram.
fn request(datagram: &[u8]) -> Pdu {
    match CommunityMessage::decode(datagram).unwrap().pdu {
        AnyPdu::Standard(pdu) => pdu,
        AnyPdu::TrapV1(_) => panic!("unexpected trap"),
    }
}

/// A Response to `to` with the given error fields and bindings.
fn reply(to: &Pdu, error_status: u32, error_index: u32, varbinds: Vec<VarBind>) -> Vec<u8> {
    reply_with_id(to.request_id, error_status, error_index, varbinds)
}

fn reply_with_id(
    request_id: i32,
    error_status: u32,
    error_index: u32,
    varbinds: Vec<VarBind>,
) -> Vec<u8> {
    let pdu = Pdu {
        kind: PduKind::Response,
        request_id,
        error_status,
        error_index,
        varbinds,
    };
    encode_community_message(V2c, b"public", &pdu)
}

/// Answers every requested OID with `Integer(position)`.
fn echo(to: &Pdu) -> Vec<u8> {
    let varbinds = to
        .varbinds
        .iter()
        .zip(0..)
        .map(|(bind, i)| VarBind::new(bind.oid.clone(), Value::Integer(i)))
        .collect();
    reply(to, 0, 0, varbinds)
}

async fn client_for(addr: SocketAddr, auth: Auth, timeout_ms: u64, retries: u32) -> Client {
    let mut config = ClientConfig::new(addr, auth);
    config.timeout = Duration::from_millis(timeout_ms);
    config.retries = retries;
    Client::connect(config).await.unwrap()
}

fn public() -> Auth {
    Auth::V2c {
        community: "public".into(),
    }
}

async fn v2c_client(responder: &Responder) -> Client {
    client_for(responder.addr, public(), 500, 0).await
}

const SYS_NAME: &str = "1.3.6.1.2.1.1.5.0";

// ---------------------------------------------------------------------------------------------
// Request matching, retries, strays
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn retries_resend_the_same_datagram_then_time_out() {
    let silent = Responder::start(|_| Vec::new()).await;
    let mut client = client_for(silent.addr, public(), 80, 2).await;

    let started = Instant::now();
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    // One attempt plus two retries, a timeout each.
    assert!(elapsed >= Duration::from_millis(240), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");

    tokio::time::sleep(Duration::from_millis(20)).await;
    let received = silent.received();
    assert_eq!(received.len(), 3);
    // Retransmissions are byte-identical: same request id, same everything.
    assert_eq!(received[0], received[1]);
    assert_eq!(received[0], received[2]);

    // The next request uses another id.
    let _ = client.get(&oids(&[SYS_NAME])).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let requests = silent.requests();
    assert_ne!(requests[0].request_id, requests[3].request_id);
    assert!(requests.iter().all(|pdu| pdu.request_id > 0));
}

#[tokio::test]
async fn a_lost_datagram_is_recovered_by_a_retry() {
    let mut seen = 0;
    let lossy = Responder::start(move |datagram| {
        seen += 1;
        // The first two transmissions vanish.
        if seen <= 2 {
            Vec::new()
        } else {
            vec![echo(&request(datagram))]
        }
    })
    .await;
    let mut client = client_for(lossy.addr, public(), 80, 3).await;
    let got = client.get(&oids(&[SYS_NAME])).await.unwrap();
    assert_eq!(got, vec![int(SYS_NAME, 0)]);
    assert_eq!(lossy.received().len(), 3);
}

#[tokio::test]
async fn strays_duplicates_and_late_replies_are_ignored() {
    let mut previous: Option<Vec<u8>> = None;
    let noisy = Responder::start(move |datagram| {
        let pdu = request(datagram);
        let good = reply(&pdu, 0, 0, vec![int(SYS_NAME, i64::from(pdu.request_id))]);
        let mut out = vec![
            // Not SNMP at all.
            b"\x00\x01garbage".to_vec(),
            Vec::new(),
            // A response to some other request.
            reply_with_id(
                pdu.request_id.wrapping_add(1000),
                0,
                0,
                vec![int(SYS_NAME, -1)],
            ),
            // The right id on something that is not a response.
            encode_community_message(
                V2c,
                b"public",
                &Pdu::request(PduKind::Get, pdu.request_id, &[]),
            ),
            // An SNMPv3 message.
            hex(crate::v3::tests::NETSNMP_V3_TRAP),
        ];
        // The answer to the previous request, arriving late.
        out.extend(previous.take());
        out.push(good.clone());
        // ...and the answer once more.
        out.push(good.clone());
        previous = Some(good);
        out
    })
    .await;
    let mut client = v2c_client(&noisy).await;

    for _ in 0..5 {
        let got = client.get(&oids(&[SYS_NAME])).await.unwrap();
        // Each request gets its own answer, never a neighbour's.
        let sent = noisy.requests().last().unwrap().request_id;
        assert_eq!(got, vec![int(SYS_NAME, i64::from(sent))]);
    }
}

#[tokio::test]
async fn a_reply_from_another_address_is_accepted() {
    let multi_homed = Responder::start_on(|datagram| vec![echo(&request(datagram))], true).await;
    let mut client = v2c_client(&multi_homed).await;
    let got = client.get(&oids(&[SYS_NAME])).await.unwrap();
    assert_eq!(got, vec![int(SYS_NAME, 0)]);
}

#[tokio::test]
async fn a_broken_reply_to_our_request_is_reported_not_timed_out() {
    let broken = Responder::start(|datagram| {
        let mut bytes = reply(&request(datagram), 0, 0, vec![int(SYS_NAME, 1)]);
        // Corrupt the value's tag (the last TLV: 02 01 01) into something unknown.
        let tag = bytes.len() - 3;
        assert_eq!(bytes[tag], 0x02);
        bytes[tag] = 0x5f;
        vec![bytes]
    })
    .await;
    let mut client = v2c_client(&broken).await;
    let started = Instant::now();
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    assert!(matches!(error, SnmpError::Decode(_)), "{error:?}");
    assert!(started.elapsed() < Duration::from_millis(400));

    // A response with the wrong number of bindings is a protocol violation too.
    let short =
        Responder::start(|datagram| vec![reply(&request(datagram), 0, 0, vec![int(SYS_NAME, 1)])])
            .await;
    let mut client = v2c_client(&short).await;
    let error = client
        .get(&oids(&[SYS_NAME, "1.3.6.1.2.1.1.6.0"]))
        .await
        .unwrap_err();
    assert!(matches!(error, SnmpError::Decode(_)), "{error:?}");
}

#[tokio::test]
async fn agent_errors_carry_status_name_and_index() {
    let failing = Responder::start(|datagram| {
        let pdu = request(datagram);
        // The third binding of every PDU is bad.
        if pdu.varbinds.len() >= 3 {
            vec![reply(&pdu, 5, 3, pdu.varbinds.clone())]
        } else {
            vec![echo(&pdu)]
        }
    })
    .await;
    let mut client = v2c_client(&failing).await;
    let error = client.get(&oids(&[SYS_NAME; 4])).await.unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 5,
                name: "genErr",
                index: 3
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        "agent returned genErr (error-status 5, error-index 3)"
    );
    assert!(client.get(&oids(&[SYS_NAME; 2])).await.is_ok());

    // In a long list that goes out as several PDUs the index still refers to the caller's list.
    let failing_later = Responder::start(|datagram| {
        let pdu = request(datagram);
        if pdu.varbinds.len() == 10 {
            vec![reply(&pdu, 2, 4, pdu.varbinds.clone())]
        } else {
            vec![echo(&pdu)]
        }
    })
    .await;
    let mut client = v2c_client(&failing_later).await;
    let error = client.get(&vec![oid(SYS_NAME); 60]).await.unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 2,
                name: "noSuchName",
                index: 54
            }
        ),
        "{error:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// tooBig
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn too_big_getbulk_halves_max_repetitions() {
    // An agent that cannot fit more than 6 repetitions and says tooBig instead of truncating.
    let cramped = Responder::start(|datagram| {
        let pdu = request(datagram);
        assert_eq!(pdu.kind, PduKind::GetBulk);
        let repetitions = pdu.error_index;
        if repetitions > 6 {
            return vec![reply(&pdu, 1, 0, Vec::new())];
        }
        // Serve a column of 20 rows, then the neighbouring column.
        let column = oid("1.3.6.1.2.1.2.2.1.10");
        let after = pdu.varbinds[0]
            .oid
            .suffix(&column)
            .and_then(|suffix| suffix.first().copied())
            .unwrap_or(0);
        let varbinds = (after + 1..=after + repetitions)
            .map(|row| {
                if row <= 20 {
                    VarBind::new(column.child(row), Value::Counter32(row))
                } else {
                    VarBind::new(
                        oid("1.3.6.1.2.1.2.2.1.11").child(row - 20),
                        Value::Counter32(0),
                    )
                }
            })
            .collect();
        vec![reply(&pdu, 0, 0, varbinds)]
    })
    .await;
    let mut client = v2c_client(&cramped).await;

    let walked = client
        .walk(&oid("1.3.6.1.2.1.2.2.1.10"), usize::MAX)
        .await
        .unwrap();
    assert_eq!(walked.len(), 20);
    assert_eq!(
        walked[19],
        VarBind::new(oid("1.3.6.1.2.1.2.2.1.10.20"), Value::Counter32(20))
    );

    // 25 is too big, 12 is too big, 6 fits — and the walk stays at 6 instead of asking for 25
    // again on every step.
    let asked: Vec<u32> = cramped
        .requests()
        .iter()
        .map(|pdu| pdu.error_index)
        .collect();
    assert_eq!(asked, [25, 12, 6, 6, 6, 6]);

    // get_bulk does the same for a single request.
    let bulk = client
        .get_bulk(&oids(&["1.3.6.1.2.1.2.2.1.10"]), 0, 40)
        .await
        .unwrap();
    assert_eq!(bulk.len(), 5);
    let asked: Vec<u32> = cramped.requests()[6..]
        .iter()
        .map(|pdu| pdu.error_index)
        .collect();
    assert_eq!(asked, [40, 20, 10, 5]);
}

#[tokio::test]
async fn too_big_that_halving_cannot_cure_is_an_error() {
    let hopeless =
        Responder::start(|datagram| vec![reply(&request(datagram), 1, 0, Vec::new())]).await;
    let mut client = v2c_client(&hopeless).await;
    let error = client
        .walk(&oid("1.3.6.1.2.1.2"), usize::MAX)
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 1,
                name: "tooBig",
                index: 0
            }
        ),
        "{error:?}"
    );
    // 25, 12, 6, 3, 1 — then it gives up.
    let asked: Vec<u32> = hopeless
        .requests()
        .iter()
        .map(|pdu| pdu.error_index)
        .collect();
    assert_eq!(asked, [25, 12, 6, 3, 1]);

    // A single-OID GET that is tooBig cannot be split any further.
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    assert!(error.is_too_big(), "{error:?}");
}

#[tokio::test]
async fn too_big_get_is_split_and_long_lists_are_chunked() {
    // At most 7 bindings per response.
    let cramped = Responder::start(|datagram| {
        let pdu = request(datagram);
        if pdu.varbinds.len() > 7 {
            return vec![reply(&pdu, 1, 0, Vec::new())];
        }
        let varbinds = pdu
            .varbinds
            .iter()
            .map(|bind| {
                let last = i64::from(*bind.oid.parts().last().unwrap());
                VarBind::new(bind.oid.clone(), Value::Integer(last))
            })
            .collect();
        vec![reply(&pdu, 0, 0, varbinds)]
    })
    .await;
    let mut client = v2c_client(&cramped).await;

    let wanted: Vec<Oid> = (0..120)
        .map(|i| oid("1.3.6.1.4.1.99999.1").child(i))
        .collect();
    let got = client.get(&wanted).await.unwrap();
    // Everything came back, once, in the order asked for.
    assert_eq!(got.len(), 120);
    for (i, bind) in got.iter().enumerate() {
        assert_eq!(
            bind,
            &VarBind::new(wanted[i].clone(), Value::Integer(i as i64))
        );
    }

    let sizes: Vec<usize> = cramped
        .requests()
        .iter()
        .map(|pdu| pdu.varbinds.len())
        .collect();
    // No PDU ever carried more than 50 bindings; the oversized ones were halved until they fit.
    assert_eq!(sizes[0], 50);
    assert!(sizes.iter().all(|&size| size <= 50));
    assert!(sizes.contains(&25) && sizes.contains(&12) && sizes.contains(&6));
    let answered: usize = sizes.iter().filter(|&&size| size <= 7).sum();
    assert_eq!(answered, 120);

    // GETNEXT is treated the same way.
    let got = client.get_next(&wanted[..20]).await.unwrap();
    assert_eq!(got.len(), 20);
}

#[tokio::test]
async fn nothing_is_sent_for_empty_or_impossible_requests() {
    let idle = Responder::start(|datagram| vec![echo(&request(datagram))]).await;
    let mut client = v2c_client(&idle).await;

    assert!(client.get(&[]).await.unwrap().is_empty());
    assert!(client.get_next(&[]).await.unwrap().is_empty());
    assert!(client.get_bulk(&[], 0, 10).await.unwrap().is_empty());
    assert!(client.set(&[]).await.unwrap().is_empty());
    assert!(client.walk(&oid("1.3.6.1"), 0).await.unwrap().is_empty());
    let column = oids(&["1.3.6.1.2.1.2.2.1.2"]);
    assert!(client.table(&column, 0).await.unwrap().is_empty());
    assert!(client.table(&[], 10).await.unwrap().is_empty());

    // OIDs that BER cannot express, values SNMP cannot carry.
    for bad in ["3.1", "1.40.1", "9"] {
        let error = client.get(&oids(&[bad])).await.unwrap_err();
        assert!(
            matches!(error, SnmpError::Unsupported(_)),
            "{bad}: {error:?}"
        );
        assert!(matches!(
            client.walk(&oid(bad), 10).await,
            Err(SnmpError::Unsupported(_))
        ));
        assert!(matches!(
            client.table(&oids(&[bad]), 10).await,
            Err(SnmpError::Unsupported(_))
        ));
        assert!(matches!(
            client.get_bulk(&oids(&[bad]), 0, 1).await,
            Err(SnmpError::Unsupported(_))
        ));
    }
    let unsettable = [
        VarBind::new(oid(SYS_NAME), Value::Integer(i64::from(i32::MAX) + 1)),
        VarBind::new(oid(SYS_NAME), Value::NoSuchInstance),
        VarBind::new(oid(SYS_NAME), Value::Oid(oid("5.5"))),
        VarBind::new(oid("4.4"), Value::Null),
    ];
    for bind in unsettable {
        let error = client.set(std::slice::from_ref(&bind)).await.unwrap_err();
        assert!(
            matches!(error, SnmpError::Unsupported(_)),
            "{bind:?}: {error:?}"
        );
    }

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        idle.received().is_empty(),
        "nothing should have reached the agent"
    );
}

// ---------------------------------------------------------------------------------------------
// Walks
// ---------------------------------------------------------------------------------------------

/// A GETBULK/GETNEXT responder over a fixed, possibly ill-ordered, list of bindings: each
/// requested OID is answered with what follows it in `list` (by position, not by value).
fn sequence_agent(list: Vec<VarBind>) -> impl FnMut(&[u8]) -> Vec<Vec<u8>> + Send + 'static {
    move |datagram| {
        let pdu = request(datagram);
        let repetitions = if pdu.kind == PduKind::GetBulk {
            pdu.error_index as usize
        } else {
            1
        };
        let mut varbinds = Vec::new();
        let mut cursors: Vec<Oid> = pdu.varbinds.iter().map(|bind| bind.oid.clone()).collect();
        for _ in 0..repetitions {
            for cursor in &mut cursors {
                let next = match list.iter().position(|bind| bind.oid == *cursor) {
                    Some(i) => list.get(i + 1),
                    // Not in the list: the first entry that sorts after it.
                    None => list.iter().find(|bind| bind.oid > *cursor),
                };
                let next = next
                    .cloned()
                    .unwrap_or_else(|| VarBind::new(cursor.clone(), Value::EndOfMibView));
                cursor.clone_from(&next.oid);
                varbinds.push(next);
            }
        }
        vec![reply(&pdu, 0, 0, varbinds)]
    }
}

#[tokio::test]
async fn walk_stops_at_the_subtree_end_and_at_end_of_mib() {
    let list = vec![
        int("1.3.6.1.2.1.1.1.0", 1),
        int("1.3.6.1.2.1.1.2.0", 2),
        int("1.3.6.1.2.1.2.1.0", 3),
        int("1.3.6.1.2.1.2.2.1.1.1", 4),
    ];
    let agent = Responder::start(sequence_agent(list.clone())).await;
    let mut client = v2c_client(&agent).await;
    let everything = usize::MAX;

    // Leaves the subtree: stop, without the foreign binding.
    assert_eq!(
        client
            .walk(&oid("1.3.6.1.2.1.1"), everything)
            .await
            .unwrap(),
        list[..2]
    );
    // Runs into endOfMibView.
    assert_eq!(
        client
            .walk(&oid("1.3.6.1.2.1.2"), everything)
            .await
            .unwrap(),
        list[2..]
    );
    assert_eq!(client.walk(&oid("1.3"), everything).await.unwrap(), list);
    // The limit is exact.
    assert_eq!(client.walk(&oid("1.3"), 3).await.unwrap(), list[..3]);
    assert_eq!(client.walk(&oid("1.3"), 1).await.unwrap(), list[..1]);
    // Nothing below, and nothing at the OID itself.
    assert!(
        client
            .walk(&oid("1.3.6.1.2.1.3"), everything)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn walk_asks_for_no_more_than_the_limit_leaves_room_for() {
    let list: Vec<VarBind> = (1..=100)
        .map(|i| int(&format!("1.3.6.1.2.1.2.2.1.10.{i}"), i))
        .collect();
    let agent = Responder::start(sequence_agent(list.clone())).await;
    let mut client = v2c_client(&agent).await;

    let walked = client.walk(&oid("1.3.6.1.2.1.2.2.1.10"), 30).await.unwrap();
    assert_eq!(walked, list[..30]);
    let asked: Vec<u32> = agent.requests().iter().map(|pdu| pdu.error_index).collect();
    assert_eq!(asked, [25, 5]);
}

#[tokio::test]
async fn walk_guards_against_agents_that_loop() {
    async fn bounded_walk(client: &mut Client) -> Vec<VarBind> {
        let system = oid("1.3.6.1.2.1.1");
        let walk = client.walk(&system, usize::MAX);
        tokio::time::timeout(Duration::from_secs(5), walk)
            .await
            .expect("the walk must terminate")
            .unwrap()
    }

    // After .3 this agent goes back to .2 — forever, if followed.
    let looping = vec![
        int("1.3.6.1.2.1.1.1.0", 1),
        int("1.3.6.1.2.1.1.2.0", 2),
        int("1.3.6.1.2.1.1.3.0", 3),
        int("1.3.6.1.2.1.1.2.0", 2),
    ];
    let agent = Responder::start(sequence_agent(looping.clone())).await;
    assert_eq!(
        bounded_walk(&mut v2c_client(&agent).await).await,
        looping[..3]
    );

    // An agent that answers GETNEXT with the very OID it was asked about.
    let stuck = Responder::start(|datagram| {
        let pdu = request(datagram);
        vec![reply(&pdu, 0, 0, vec![int("1.3.6.1.2.1.1.1.0", 1)])]
    })
    .await;
    assert_eq!(
        bounded_walk(&mut v2c_client(&stuck).await).await,
        vec![int("1.3.6.1.2.1.1.1.0", 1)]
    );

    // An agent that answers GETBULK with nothing at all.
    let mute = Responder::start(|datagram| vec![reply(&request(datagram), 0, 0, Vec::new())]).await;
    assert!(bounded_walk(&mut v2c_client(&mute).await).await.is_empty());
}

#[tokio::test]
async fn v1_walks_with_getnext_and_ends_on_no_such_name() {
    let list = vec![int("1.3.6.1.2.1.1.1.0", 1), int("1.3.6.1.2.1.1.2.0", 2)];
    let served = list.clone();
    let agent = Responder::start(move |datagram| {
        let message = CommunityMessage::decode(datagram).unwrap();
        assert_eq!(message.version, V1);
        let pdu = request(datagram);
        let mut response = Pdu {
            kind: PduKind::Response,
            request_id: pdu.request_id,
            error_status: 0,
            error_index: 0,
            varbinds: Vec::new(),
        };
        for (position, bind) in pdu.varbinds.iter().enumerate() {
            match served.iter().find(|candidate| candidate.oid > bind.oid) {
                Some(next) => response.varbinds.push(next.clone()),
                // SNMPv1's end of MIB: noSuchName, the request's bindings echoed.
                None => {
                    response.error_status = 2;
                    response.error_index = position as u32 + 1;
                    response.varbinds = pdu.varbinds.clone();
                    break;
                }
            }
        }
        vec![encode_community_message(V1, b"public", &response)]
    })
    .await;
    let auth = Auth::V1 {
        community: "public".into(),
    };
    let mut client = client_for(agent.addr, auth, 500, 0).await;

    assert_eq!(
        client
            .walk(&oid("1.3.6.1.2.1.1"), usize::MAX)
            .await
            .unwrap(),
        list
    );
    assert!(
        agent
            .requests()
            .iter()
            .all(|pdu| pdu.kind == PduKind::GetNext)
    );

    // Two columns in one GETNEXT per row; they end at different times.
    let columns = oids(&["1.3.6.1.2.1.1.1", "1.3.6.1.2.1.1.2"]);
    let table = client.table(&columns, usize::MAX).await.unwrap();
    assert_eq!(table.len(), 1);
    assert_eq!(table[&vec![0u32]].len(), 2);

    // SNMPv1 has no GETBULK and no Counter64.
    assert!(matches!(
        client.get_bulk(&oids(&[SYS_NAME]), 0, 5).await,
        Err(SnmpError::Unsupported(_))
    ));
    let counter = VarBind::new(oid(SYS_NAME), Value::Counter64(1));
    assert!(matches!(
        client.set(&[counter]).await,
        Err(SnmpError::Unsupported(_))
    ));
}

#[tokio::test]
async fn table_follows_each_column_through_ragged_responses() {
    // Three columns of different lengths; the agent answers at most 4 bindings per GETBULK,
    // so repetitions are cut in the middle of a row.
    let column = |c: u32| oid("1.3.6.1.4.1.99999.1.1").child(c);
    let mut list = Vec::new();
    for (c, rows) in [(1u32, 7u32), (2, 3), (3, 5)] {
        for row in 1..=rows {
            let value = Value::Integer(i64::from(c * 100 + row));
            list.push(VarBind::new(column(c).child(row * 10), value));
        }
    }
    let mut full = sequence_agent(list);
    let agent = Responder::start(move |datagram| {
        let mut replies = full(datagram);
        let AnyPdu::Standard(mut pdu) = CommunityMessage::decode(&replies[0]).unwrap().pdu else {
            unreachable!()
        };
        pdu.varbinds.truncate(4);
        replies[0] = encode_community_message(V2c, b"public", &pdu);
        replies
    })
    .await;
    let mut client = v2c_client(&agent).await;

    let columns = [column(1), column(2), column(3)];
    let table = client.table(&columns, usize::MAX).await.unwrap();
    let indexes: Vec<u32> = table.keys().map(|index| index[0]).collect();
    assert_eq!(indexes, [10, 20, 30, 40, 50, 60, 70]);
    for (index, row) in &table {
        let row_number = index[0] / 10;
        assert_eq!(
            row[&columns[0]],
            Value::Integer(i64::from(100 + row_number))
        );
        assert_eq!(
            row.contains_key(&columns[1]),
            row_number <= 3,
            "row {row_number}"
        );
        assert_eq!(
            row.contains_key(&columns[2]),
            row_number <= 5,
            "row {row_number}"
        );
        if row_number <= 3 {
            assert_eq!(
                row[&columns[1]],
                Value::Integer(i64::from(200 + row_number))
            );
        }
    }

    // A row limit returns the first rows, whole.
    let limited = client.table(&columns, 2).await.unwrap();
    assert_eq!(
        limited.keys().map(|index| index[0]).collect::<Vec<_>>(),
        [10, 20]
    );
    assert!(limited.values().all(|row| row.len() == 3));
}

// ---------------------------------------------------------------------------------------------
// SNMPv3: a scripted authoritative engine
// ---------------------------------------------------------------------------------------------

const USER: &str = "operator";
const AUTH_PASS: &str = "authentication-pass";
const PRIV_PASS: &str = "privacy-pass";

const UNSUPPORTED_SEC_LEVELS: &str = "1.3.6.1.6.3.15.1.1.1.0";
const NOT_IN_TIME_WINDOWS: &str = "1.3.6.1.6.3.15.1.1.2.0";
const UNKNOWN_USER_NAMES: &str = "1.3.6.1.6.3.15.1.1.3.0";
const UNKNOWN_ENGINE_IDS: &str = "1.3.6.1.6.3.15.1.1.4.0";
const WRONG_DIGESTS: &str = "1.3.6.1.6.3.15.1.1.5.0";
const DECRYPTION_ERRORS: &str = "1.3.6.1.6.3.15.1.1.6.0";

/// How the fake engine deviates from a well-behaved agent.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Quirk {
    None,
    /// Answer every request (after discovery) with this report.
    AlwaysReport(&'static str),
    /// Say nothing at all to encrypted requests.
    IgnoreEncrypted,
    /// Like net-snmp: say nothing when the decrypted content makes no sense.
    SilentOnDecryptionError,
    /// Say nothing to anything but discovery.
    DeadAfterDiscovery,
    /// Say nothing to requests that ask for something — a request that is lost or takes
    /// longer than the manager is willing to wait.
    IgnoreRealRequests,
    /// Send the notInTimeWindows report without authenticating it.
    UnsignedTimeReport,
    /// Before the real response, send forged and downgraded ones.
    ForgeFirst,
}

/// What the fake engine saw.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Seen {
    Discovery,
    Request { flags: u8, boots: u32, time: u32 },
}

struct FakeEngine {
    id: Vec<u8>,
    boots: u32,
    time: u32,
    auth: AuthProtocol,
    privacy: Option<PrivProtocol>,
    /// Whether the discovery report reveals boots and time (some agents send zeros).
    discloses_clock: bool,
    quirk: Quirk,
    seen: Vec<Seen>,
    /// The keys last derived, and the engine ID and privacy protocol they are for.
    derived: Mutex<Option<(Vec<u8>, Option<PrivProtocol>, LocalizedKeys)>>,
}

impl FakeEngine {
    fn new(auth: AuthProtocol, privacy: Option<PrivProtocol>) -> FakeEngine {
        FakeEngine {
            id: b"\x80\x00\x1f\x88\x04fake-engine-1".to_vec(),
            boots: 7,
            time: 1000,
            auth,
            privacy,
            discloses_clock: true,
            quirk: Quirk::None,
            seen: Vec::new(),
            derived: Mutex::new(None),
        }
    }

    fn keys(&self) -> LocalizedKeys {
        let mut derived = self.derived.lock().unwrap();
        if let Some((id, privacy, keys)) = &*derived
            && *id == self.id
            && *privacy == self.privacy
        {
            return keys.clone();
        }
        let keys = self.derive_keys();
        *derived = Some((self.id.clone(), self.privacy, keys.clone()));
        keys
    }

    fn derive_keys(&self) -> LocalizedKeys {
        let master = self.auth.master_key(AUTH_PASS.as_bytes()).unwrap();
        LocalizedKeys {
            auth_protocol: self.auth,
            auth_key: self.auth.localize(&master, &self.id),
            privacy: self.privacy.map(|protocol| {
                let master = self.auth.master_key(PRIV_PASS.as_bytes()).unwrap();
                (
                    protocol,
                    protocol.localized_key(self.auth, &master, &self.id),
                )
            }),
        }
    }

    fn send(&self, msg_id: i32, flags: u8, pdu: Pdu, salt: u64) -> Vec<u8> {
        let scoped = ScopedPdu {
            context_engine_id: self.id.clone(),
            context_name: Vec::new(),
            pdu,
        };
        OutgoingMessage {
            msg_id,
            flags,
            engine_id: &self.id,
            engine_boots: self.boots,
            engine_time: self.time,
            user: USER.as_bytes(),
            scoped_pdu: &scoped,
        }
        .encode(Some(&self.keys()), salt)
        .unwrap()
    }

    fn report(&self, msg_id: i32, flags: u8, counter: &str) -> Vec<u8> {
        let pdu = Pdu {
            kind: PduKind::Report,
            request_id: 0,
            error_status: 0,
            error_index: 0,
            varbinds: vec![VarBind::new(oid(counter), Value::Counter32(1))],
        };
        self.send(msg_id, flags, pdu, 1)
    }

    fn handle(&mut self, datagram: &[u8]) -> Vec<Vec<u8>> {
        let message = V3Message::decode(datagram).unwrap();
        let flags = message.flags & (FLAG_AUTH | FLAG_PRIV);

        // RFC 3414 §3.2 step 3: an engine ID that is not ours gets the discovery report.
        if message.engine_id != self.id {
            self.seen.push(Seen::Discovery);
            let clock = (self.boots, self.time);
            if !self.discloses_clock {
                (self.boots, self.time) = (0, 0);
            }
            let report = self.report(message.msg_id, 0, UNKNOWN_ENGINE_IDS);
            (self.boots, self.time) = clock;
            return vec![report];
        }
        self.seen.push(Seen::Request {
            flags,
            boots: message.engine_boots,
            time: message.engine_time,
        });
        match self.quirk {
            Quirk::AlwaysReport(counter) => {
                // Only notInTimeWindows is sent authenticated.
                let level = if counter == NOT_IN_TIME_WINDOWS {
                    FLAG_AUTH
                } else {
                    0
                };
                return vec![self.report(message.msg_id, level, counter)];
            }
            Quirk::DeadAfterDiscovery => return Vec::new(),
            Quirk::IgnoreEncrypted if message.is_encrypted() => return Vec::new(),
            _ => {}
        }

        if message.user != USER.as_bytes() {
            return vec![self.report(message.msg_id, 0, UNKNOWN_USER_NAMES)];
        }
        let keys = self.keys();
        if message.is_authenticated() && !message.verify(datagram, &keys) {
            return vec![self.report(message.msg_id, 0, WRONG_DIGESTS)];
        }
        if message.is_authenticated()
            && (message.engine_boots != self.boots || message.engine_time.abs_diff(self.time) > 150)
        {
            let level = if self.quirk == Quirk::UnsignedTimeReport {
                0
            } else {
                FLAG_AUTH
            };
            return vec![self.report(message.msg_id, level, NOT_IN_TIME_WINDOWS)];
        }
        let Ok(scoped) = message.scoped_pdu(Some(&keys)) else {
            if self.quirk == Quirk::SilentOnDecryptionError {
                return Vec::new();
            }
            return vec![self.report(message.msg_id, 0, DECRYPTION_ERRORS)];
        };
        if self.quirk == Quirk::IgnoreRealRequests && !scoped.pdu.varbinds.is_empty() {
            return Vec::new();
        }

        // The answer: every OID with the security level it was asked at.
        let response = Pdu {
            kind: PduKind::Response,
            request_id: scoped.pdu.request_id,
            error_status: 0,
            error_index: 0,
            varbinds: scoped
                .pdu
                .varbinds
                .iter()
                .map(|bind| VarBind::new(bind.oid.clone(), Value::Integer(i64::from(flags))))
                .collect(),
        };
        let good = self.send(message.msg_id, flags, response.clone(), 77);
        if self.quirk != Quirk::ForgeFirst {
            return vec![good];
        }
        let lie = Pdu {
            varbinds: vec![VarBind::new(oid(SYS_NAME), Value::from("forged"))],
            ..response
        };
        // A response with a broken digest...
        let mut tampered = self.send(message.msg_id, flags, lie.clone(), 78);
        let last = tampered.len() - 1;
        tampered[last] ^= 0x01;
        // ...one without any security at all (a downgrade)...
        let downgraded = self.send(message.msg_id, 0, lie.clone(), 79);
        // ...one signed by somebody else...
        let mut stranger = FakeEngine::new(self.auth, self.privacy);
        stranger.id = b"\x80\x00\x1f\x88\x04another-engine".to_vec();
        let foreign = stranger.send(message.msg_id, flags, lie.clone(), 80);
        // ...and a properly signed answer to a different message.
        let misdirected = self.send(message.msg_id.wrapping_add(1), flags, lie, 81);
        vec![tampered, downgraded, foreign, misdirected, good]
    }
}

/// A fake engine on a socket, shared with the test for inspection.
async fn fake_engine(engine: FakeEngine) -> (Responder, Arc<Mutex<FakeEngine>>) {
    let engine = Arc::new(Mutex::new(engine));
    let handle = Arc::clone(&engine);
    let responder = Responder::start(move |datagram| handle.lock().unwrap().handle(datagram)).await;
    (responder, engine)
}

fn v3_auth(auth: Option<AuthProtocol>, privacy: Option<PrivProtocol>) -> Auth {
    Auth::V3 {
        user: USER.into(),
        auth: auth.map(|protocol| (protocol, AUTH_PASS.into())),
        privacy: privacy.map(|protocol| (protocol, PRIV_PASS.into())),
        context_name: String::new(),
    }
}

async fn v3_client(
    responder: &Responder,
    auth: Option<AuthProtocol>,
    privacy: Option<PrivProtocol>,
) -> Client {
    client_for(responder.addr, v3_auth(auth, privacy), V3_TIMEOUT_MS, 0).await
}

/// Long enough that a busy machine does not turn an answer into a timeout, short enough
/// that the tests which wait for one stay quick.
const V3_TIMEOUT_MS: u64 = 250;

fn seen(engine: &Arc<Mutex<FakeEngine>>) -> Vec<Seen> {
    engine.lock().unwrap().seen.clone()
}

fn auth_message(result: Result<Vec<VarBind>, SnmpError>) -> String {
    match result {
        Err(SnmpError::Auth(message)) => message,
        other => panic!("expected SnmpError::Auth, got {other:?}"),
    }
}

const SHA256: AuthProtocol = AuthProtocol::Sha256;
const AES: PrivProtocol = PrivProtocol::Aes128;
const AUTH_PRIV: u8 = FLAG_AUTH | FLAG_PRIV;

#[tokio::test]
async fn v3_discovers_once_and_then_just_asks() {
    for (auth, privacy, flags) in [
        (None, None, 0),
        (Some(SHA256), None, FLAG_AUTH),
        (Some(SHA256), Some(AES), AUTH_PRIV),
        (Some(AuthProtocol::Md5), Some(PrivProtocol::Des), AUTH_PRIV),
    ] {
        let (responder, engine) =
            fake_engine(FakeEngine::new(auth.unwrap_or(SHA256), privacy)).await;
        let mut client = v3_client(&responder, auth, privacy).await;
        assert!(
            seen(&engine).is_empty(),
            "connect() must not talk to the agent"
        );

        for _ in 0..3 {
            let got = client.get(&oids(&[SYS_NAME])).await.unwrap();
            // The fake engine answers with the security level it saw.
            assert_eq!(got, vec![int(SYS_NAME, i64::from(flags))]);
        }
        let seen = seen(&engine);
        assert_eq!(seen[0], Seen::Discovery);
        assert_eq!(
            seen.len(),
            4,
            "one discovery, then one message per request: {seen:?}"
        );
        for request in &seen[1..] {
            // The clock learned from discovery is used as is.
            assert!(
                matches!(request, Seen::Request { flags: f, boots: 7, time: 1000..=1002 } if *f == flags),
                "{request:?}"
            );
        }
        assert_eq!(client.cached_key_sets(), usize::from(auth.is_some()));
    }
}

#[tokio::test]
async fn v3_resynchronises_once_on_not_in_time_window() {
    for quirk in [Quirk::None, Quirk::UnsignedTimeReport] {
        // An agent whose discovery report does not reveal its clock.
        let mut engine = FakeEngine::new(SHA256, Some(AES));
        engine.discloses_clock = false;
        engine.quirk = quirk;
        let (responder, engine) = fake_engine(engine).await;
        let mut client = v3_client(&responder, Some(SHA256), Some(AES)).await;

        assert!(client.get(&oids(&[SYS_NAME])).await.is_ok(), "{quirk:?}");
        assert_eq!(
            seen(&engine),
            [
                Seen::Discovery,
                // Sent with the zeros from discovery, rejected as outside the time window...
                Seen::Request {
                    flags: AUTH_PRIV,
                    boots: 0,
                    time: 0
                },
                // ...and repeated with the clock from the report.
                Seen::Request {
                    flags: AUTH_PRIV,
                    boots: 7,
                    time: 1000
                },
            ],
            "{quirk:?}"
        );

        // The agent reboots: boots go up, time starts over. Same dance, once.
        {
            let mut engine = engine.lock().unwrap();
            engine.boots = 8;
            engine.time = 3;
            engine.seen.clear();
        }
        assert!(client.get(&oids(&[SYS_NAME])).await.is_ok(), "{quirk:?}");
        let after_reboot = seen(&engine);
        assert_eq!(after_reboot.len(), 2, "{after_reboot:?}");
        assert!(matches!(after_reboot[0], Seen::Request { boots: 7, .. }));
        assert_eq!(
            after_reboot[1],
            Seen::Request {
                flags: AUTH_PRIV,
                boots: 8,
                time: 3
            }
        );

        // From here on the client is in step: no more reports.
        engine.lock().unwrap().seen.clear();
        assert!(client.get(&oids(&[SYS_NAME])).await.is_ok());
        assert_eq!(seen(&engine).len(), 1);
    }
}

#[tokio::test]
async fn v3_gives_up_when_resynchronising_does_not_help() {
    // An agent that rejects every clock, even its own.
    let mut engine = FakeEngine::new(SHA256, None);
    engine.quirk = Quirk::AlwaysReport(NOT_IN_TIME_WINDOWS);
    let (responder, engine) = fake_engine(engine).await;
    let mut client = v3_client(&responder, Some(SHA256), None).await;
    let message = auth_message(client.get(&oids(&[SYS_NAME])).await);
    assert!(message.contains("not in time window"), "{message}");
    // Discovery, the request, one resynchronised repeat — and no more.
    assert_eq!(seen(&engine).len(), 3);

    // An agent that keeps disowning the engine ID it just announced.
    let mut engine = FakeEngine::new(SHA256, None);
    engine.quirk = Quirk::AlwaysReport(UNKNOWN_ENGINE_IDS);
    let (responder, engine) = fake_engine(engine).await;
    let mut client = v3_client(&responder, Some(SHA256), None).await;
    let message = auth_message(client.get(&oids(&[SYS_NAME])).await);
    assert!(message.contains("unknown engine ID"), "{message}");
    // Discovery, request, rediscovery, request.
    assert_eq!(
        seen(&engine)
            .iter()
            .filter(|s| **s == Seen::Discovery)
            .count(),
        2
    );
    assert_eq!(seen(&engine).len(), 4);
}

#[tokio::test]
async fn v3_rediscovers_when_the_engine_id_changes() {
    let (responder, engine) = fake_engine(FakeEngine::new(SHA256, Some(AES))).await;
    let mut client = v3_client(&responder, Some(SHA256), Some(AES)).await;
    assert!(client.get(&oids(&[SYS_NAME])).await.is_ok());
    assert_eq!(client.cached_key_sets(), 1);

    // The device is replaced: new engine ID, hence new localised keys.
    {
        let mut engine = engine.lock().unwrap();
        engine.id = b"\x80\x00\x1f\x88\x04fake-engine-2".to_vec();
        engine.boots = 1;
        engine.time = 50;
        engine.seen.clear();
    }
    let got = client.get(&oids(&[SYS_NAME])).await.unwrap();
    assert_eq!(got, vec![int(SYS_NAME, i64::from(AUTH_PRIV))]);
    assert_eq!(
        seen(&engine),
        [
            // The request addressed to the old engine draws the unknownEngineID report...
            Seen::Discovery,
            // ...the client asks who is there...
            Seen::Discovery,
            // ...and repeats the request for the new engine, with its clock.
            Seen::Request {
                flags: AUTH_PRIV,
                boots: 1,
                time: 50
            },
        ]
    );
    // Keys are kept per engine ID: both engines' keys are in the cache now.
    assert_eq!(client.cached_key_sets(), 2);

    // Back to the first engine: its keys are still there.
    {
        let mut engine = engine.lock().unwrap();
        engine.id = b"\x80\x00\x1f\x88\x04fake-engine-1".to_vec();
        engine.seen.clear();
    }
    assert!(client.get(&oids(&[SYS_NAME])).await.is_ok());
    assert_eq!(client.cached_key_sets(), 2);
}

#[tokio::test]
async fn v3_reports_become_auth_errors_that_name_the_cause() {
    let cases = [
        (UNSUPPORTED_SEC_LEVELS, "unsupported security level"),
        (UNKNOWN_USER_NAMES, "unknown user name"),
        (
            WRONG_DIGESTS,
            "wrong digest (authentication passphrase or protocol mismatch)",
        ),
        (
            DECRYPTION_ERRORS,
            "decryption error (privacy passphrase or protocol mismatch)",
        ),
        ("1.3.6.1.6.3.11.2.1.1.0", "unknown security model"),
        ("1.3.6.1.6.3.11.2.1.2.0", "invalid"),
        ("1.3.6.1.6.3.11.2.1.3.0", "unknown PDU handler"),
        ("1.3.6.1.6.3.12.1.4.0", "unavailable context"),
        ("1.3.6.1.6.3.12.1.5.0", "unknown context"),
        ("1.3.6.1.4.1.99999.7.0", "1.3.6.1.4.1.99999.7.0"),
    ];
    for (counter, expected) in cases {
        let mut engine = FakeEngine::new(SHA256, Some(AES));
        engine.quirk = Quirk::AlwaysReport(counter);
        let (responder, engine) = fake_engine(engine).await;
        let mut client = v3_client(&responder, Some(SHA256), Some(AES)).await;
        let message = auth_message(client.get(&oids(&[SYS_NAME])).await);
        assert!(message.contains(expected), "{counter}: {message}");
        // A terminal report ends the request at once: discovery plus one message.
        assert_eq!(seen(&engine).len(), 2, "{counter}");
    }
}

#[tokio::test]
async fn v3_wrong_credentials_are_told_apart() {
    let (responder, _engine) = fake_engine(FakeEngine::new(SHA256, Some(AES))).await;
    let target = responder.addr;
    let attempt = |user: &'static str, auth_pass: &'static str, priv_pass: &'static str| async move {
        let auth = Auth::V3 {
            user: user.into(),
            auth: Some((SHA256, auth_pass.into())),
            privacy: Some((AES, priv_pass.into())),
            context_name: String::new(),
        };
        client_for(target, auth, V3_TIMEOUT_MS, 0)
            .await
            .get(&oids(&[SYS_NAME]))
            .await
    };

    let message = auth_message(attempt(USER, "wrong-passphrase", PRIV_PASS).await);
    assert!(message.contains("wrong digest"), "{message}");
    let message = auth_message(attempt("somebody", AUTH_PASS, PRIV_PASS).await);
    assert!(message.contains("unknown user name"), "{message}");
    let message = auth_message(attempt(USER, AUTH_PASS, "wrong-passphrase").await);
    assert!(message.contains("decryption error"), "{message}");
    assert!(attempt(USER, AUTH_PASS, PRIV_PASS).await.is_ok());
}

#[tokio::test]
async fn v3_forged_and_downgraded_replies_are_ignored() {
    for privacy in [None, Some(AES), Some(PrivProtocol::Des)] {
        let mut engine = FakeEngine::new(SHA256, privacy);
        engine.quirk = Quirk::ForgeFirst;
        let (responder, _engine) = fake_engine(engine).await;
        let mut client = v3_client(&responder, Some(SHA256), privacy).await;
        let flags = FLAG_AUTH | if privacy.is_some() { FLAG_PRIV } else { 0 };
        for _ in 0..3 {
            let got = client.get(&oids(&[SYS_NAME])).await.unwrap();
            // Only the genuine answer gets through.
            assert_eq!(got, vec![int(SYS_NAME, i64::from(flags))], "{privacy:?}");
        }
    }
}

#[tokio::test]
async fn v3_silence_after_discovery_is_explained_when_it_can_be() {
    fn flags_seen(engine: &Arc<Mutex<FakeEngine>>) -> Vec<u8> {
        seen(engine)
            .iter()
            .filter_map(|s| match s {
                Seen::Request { flags, .. } => Some(*flags),
                Seen::Discovery => None,
            })
            .collect()
    }

    // An agent that authenticates the user but stays silent on encrypted requests (what
    // net-snmp does when the privacy key is wrong). The client probes: unencrypted — an
    // answer; encrypted — silence again. Conclusion: the privacy settings are wrong.
    let mut engine = FakeEngine::new(SHA256, Some(AES));
    engine.quirk = Quirk::IgnoreEncrypted;
    let (responder, engine) = fake_engine(engine).await;
    let mut client = v3_client(&responder, Some(SHA256), Some(AES)).await;
    let message = auth_message(client.get(&oids(&[SYS_NAME])).await);
    assert!(
        message.contains("decryption error (privacy passphrase or protocol mismatch)")
            && message.contains("does not answer encrypted requests"),
        "{message}"
    );
    assert_eq!(flags_seen(&engine), [AUTH_PRIV, FLAG_AUTH, AUTH_PRIV]);

    // An agent that decrypts perfectly well but does not answer this particular request in
    // time (a slow table, a lost datagram): the encrypted probe is answered, so this is a
    // timeout and must not be blamed on the passphrase.
    let mut engine = FakeEngine::new(SHA256, Some(AES));
    engine.quirk = Quirk::IgnoreRealRequests;
    let (responder, engine) = fake_engine(engine).await;
    let mut client = v3_client(&responder, Some(SHA256), Some(AES)).await;
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    assert_eq!(flags_seen(&engine), [AUTH_PRIV, FLAG_AUTH, AUTH_PRIV]);
    // Privacy is known to work now: further timeouts are reported without probing.
    engine.lock().unwrap().seen.clear();
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    assert_eq!(flags_seen(&engine), [AUTH_PRIV]);

    // An agent that falls silent altogether: that is a timeout, and nothing else.
    let mut engine = FakeEngine::new(SHA256, Some(AES));
    engine.quirk = Quirk::DeadAfterDiscovery;
    let (responder, engine) = fake_engine(engine).await;
    let mut client = v3_client(&responder, Some(SHA256), Some(AES)).await;
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    assert_eq!(flags_seen(&engine), [AUTH_PRIV, FLAG_AUTH]);

    // No agent at all: the timeout comes from discovery, without any probing.
    let nobody = Responder::start(|_| Vec::new()).await;
    let mut client = v3_client(&nobody, Some(SHA256), Some(AES)).await;
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(nobody.received().len(), 1);

    // Once encryption has worked, a later silence is just a timeout: no probe, no verdict.
    let (responder, engine) = fake_engine(FakeEngine::new(SHA256, Some(AES))).await;
    let mut client = v3_client(&responder, Some(SHA256), Some(AES)).await;
    assert!(client.get(&oids(&[SYS_NAME])).await.is_ok());
    {
        let mut engine = engine.lock().unwrap();
        engine.quirk = Quirk::IgnoreEncrypted;
        engine.seen.clear();
    }
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    assert_eq!(flags_seen(&engine), [AUTH_PRIV]);

    // Authentication only: there is no privacy to doubt, a timeout is a timeout.
    let mut engine = FakeEngine::new(SHA256, None);
    engine.quirk = Quirk::DeadAfterDiscovery;
    let (responder, engine) = fake_engine(engine).await;
    let mut client = v3_client(&responder, Some(SHA256), None).await;
    let error = client.get(&oids(&[SYS_NAME])).await.unwrap_err();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    assert_eq!(flags_seen(&engine), [FLAG_AUTH]);
}

#[tokio::test]
async fn v3_tries_the_other_aes_key_extension() {
    use PrivProtocol::{Aes192, Aes192Cisco, Aes256, Aes256Cisco};
    let sha1 = AuthProtocol::Sha1;

    // The agent stretches short keys one way, the client is configured for the other.
    for (agents, clients) in [
        (Aes256Cisco, Aes256),
        (Aes256, Aes256Cisco),
        (Aes192Cisco, Aes192),
        (Aes192, Aes192Cisco),
    ] {
        let mut engine = FakeEngine::new(sha1, Some(agents));
        engine.quirk = Quirk::SilentOnDecryptionError;
        let (responder, engine) = fake_engine(engine).await;
        let mut client = v3_client(&responder, Some(sha1), Some(clients)).await;

        let got = client.get(&oids(&[SYS_NAME])).await.unwrap();
        assert_eq!(
            got,
            vec![int(SYS_NAME, i64::from(AUTH_PRIV))],
            "{clients:?}"
        );
        // After discovery: the request that met silence, the unencrypted probe (answered),
        // the encrypted probe (silence), the encrypted probe with the other key extension
        // (answered), and the request again.
        let flags: Vec<u8> = seen(&engine)[1..]
            .iter()
            .map(|s| match s {
                Seen::Request { flags, .. } => *flags,
                Seen::Discovery => panic!("unexpected rediscovery"),
            })
            .collect();
        assert_eq!(
            flags,
            [AUTH_PRIV, FLAG_AUTH, AUTH_PRIV, AUTH_PRIV, AUTH_PRIV],
            "{clients:?}"
        );

        // The flavour that worked is remembered: the next request goes straight through.
        engine.lock().unwrap().seen.clear();
        assert!(client.get(&oids(&[SYS_NAME])).await.is_ok());
        assert_eq!(seen(&engine).len(), 1, "{clients:?}");
    }

    // With a hash long enough for both flavours to coincide there is nothing else to try,
    // and a wrong passphrase is still a wrong passphrase.
    let mut engine = FakeEngine::new(AuthProtocol::Sha512, Some(Aes256));
    engine.quirk = Quirk::IgnoreEncrypted;
    let (responder, engine) = fake_engine(engine).await;
    let mut client = v3_client(&responder, Some(AuthProtocol::Sha512), Some(Aes256)).await;
    let message = auth_message(client.get(&oids(&[SYS_NAME])).await);
    assert!(message.contains("decryption error"), "{message}");
    assert_eq!(
        seen(&engine).len(),
        4,
        "discovery, request, two probes — and no second flavour to try"
    );
}

#[tokio::test]
async fn v3_tries_the_other_aes_key_extension_on_a_decryption_report() {
    use PrivProtocol::{Aes256, Aes256Cisco};
    let sha1 = AuthProtocol::Sha1;

    // An agent that reports what it cannot decrypt, as the RFC asks.
    for (agents, clients) in [(Aes256Cisco, Aes256), (Aes256, Aes256Cisco)] {
        let (responder, engine) = fake_engine(FakeEngine::new(sha1, Some(agents))).await;
        let mut client = v3_client(&responder, Some(sha1), Some(clients)).await;
        let got = client.get(&oids(&[SYS_NAME])).await.unwrap();
        assert_eq!(
            got,
            vec![int(SYS_NAME, i64::from(AUTH_PRIV))],
            "{clients:?}"
        );
        // No timeout had to be sat out: discovery, the request that drew the report, a
        // probe with the other flavour, the request again.
        assert_eq!(seen(&engine).len(), 4, "{clients:?}");
    }

    // A wrong passphrase is reported as such, with or without a second flavour to try.
    for (auth, privacy) in [(sha1, Aes256), (sha1, AES), (AuthProtocol::Sha512, Aes256)] {
        let (responder, engine) = fake_engine(FakeEngine::new(auth, Some(privacy))).await;
        let wrong = Auth::V3 {
            user: USER.into(),
            auth: Some((auth, AUTH_PASS.into())),
            privacy: Some((privacy, "not-the-privacy-passphrase".into())),
            context_name: String::new(),
        };
        let mut client = client_for(responder.addr, wrong, V3_TIMEOUT_MS, 0).await;
        let message = auth_message(client.get(&oids(&[SYS_NAME])).await);
        assert_eq!(
            message,
            "decryption error (privacy passphrase or protocol mismatch)"
        );
        let expected = if auth == sha1 && privacy == Aes256 {
            3
        } else {
            2
        };
        assert_eq!(seen(&engine).len(), expected, "{auth:?}/{privacy:?}");
    }
}

#[tokio::test]
async fn v3_keeps_one_session_across_many_requests() {
    // Requests of growing size, so that DES pads to every length.
    let des = PrivProtocol::Des;
    let (responder, engine) = fake_engine(FakeEngine::new(AuthProtocol::Sha1, Some(des))).await;
    let mut client = v3_client(&responder, Some(AuthProtocol::Sha1), Some(des)).await;
    for round in 0..20 {
        let wanted: Vec<Oid> = (0..=round).map(|i| oid("1.3.6.1.2.1.1").child(i)).collect();
        let got = client.get(&wanted).await.unwrap();
        assert_eq!(got.len(), wanted.len());
    }
    // One discovery for the whole session, and no message was repeated.
    let seen = seen(&engine);
    assert_eq!(seen.iter().filter(|s| **s == Seen::Discovery).count(), 1);
    assert_eq!(seen.len(), 21);

    // Every message carries a fresh msgID and a fresh salt.
    let datagrams = responder.received();
    let mut msg_ids = std::collections::HashSet::new();
    let mut salts = std::collections::HashSet::new();
    for datagram in &datagrams[1..] {
        let message = V3Message::decode(datagram).unwrap();
        assert!(msg_ids.insert(message.msg_id));
        assert!(salts.insert(message.priv_params.to_vec()));
    }
}

// ---------------------------------------------------------------------------------------------
// Hostile replies
// ---------------------------------------------------------------------------------------------

#[test]
fn reply_matching_takes_any_datagram() {
    let inputs = crate::robustness::hostile_inputs();
    let (mut matched, mut broken) = (0, 0);
    // The request ids of the corpus' messages, and one that matches nothing.
    for request_id in [0x1234_5678, 0x3e28_8c1d, 7] {
        for input in &inputs {
            match community_reply(input, request_id) {
                Ok(Some(pdu)) => {
                    matched += 1;
                    assert_eq!((pdu.kind, pdu.request_id), (PduKind::Response, request_id));
                }
                Ok(None) => {}
                Err(SnmpError::Decode(_)) => broken += 1,
                Err(other) => panic!("unexpected {other:?}"),
            }
            let _ = response_id(input);
        }
    }
    // Corrupted responses to "our" request are either still usable or reported as broken;
    // everything else is ignored.
    assert!(matched > 100, "{matched}");
    assert!(broken > 100, "{broken}");
}

#[tokio::test]
async fn mangled_replies_never_wedge_the_client() {
    for (auth, privacy) in [
        (None, None),
        (Some(SHA256), None),
        (Some(SHA256), Some(AES)),
        (Some(AuthProtocol::Md5), Some(PrivProtocol::Des)),
    ] {
        let engine = Arc::new(Mutex::new(FakeEngine::new(auth.unwrap_or(SHA256), privacy)));
        let handle = Arc::clone(&engine);
        let mut rng = crate::robustness::Rng::new(0xbad_5eed);
        // Every reply is preceded by a corrupted copy of itself — and sometimes replaced by it.
        let responder = Responder::start(move |datagram| {
            let mut out = Vec::new();
            for reply in handle.lock().unwrap().handle(datagram) {
                let mut mangled = reply.clone();
                match rng.below(4) {
                    0 => mangled.truncate(rng.below(reply.len())),
                    1 => mangled.extend(rng.bytes(5)),
                    _ => {
                        let position = rng.below(mangled.len());
                        mangled[position] = rng.byte();
                    }
                }
                out.push(mangled);
                if rng.below(10) != 0 {
                    out.push(reply);
                }
            }
            out
        })
        .await;
        let mut client = client_for(responder.addr, v3_auth(auth, privacy), 100, 0).await;

        let (mut ok, mut failed) = (0, 0);
        for round in 0..60 {
            let wanted: Vec<Oid> = (0..=round % 5)
                .map(|i| oid("1.3.6.1.2.1.1").child(i))
                .collect();
            let request = tokio::time::timeout(Duration::from_secs(10), client.get(&wanted));
            match request
                .await
                .expect("a request must end, one way or the other")
            {
                Ok(got) => {
                    ok += 1;
                    assert_eq!(got.len(), wanted.len());
                }
                Err(_) => failed += 1,
            }
        }
        // With authentication a corrupted reply is recognised and the genuine one used.
        if auth.is_some() {
            assert!(ok >= 30, "{auth:?}/{privacy:?}: {ok} ok, {failed} failed");
        }
        assert!(ok > 0, "{auth:?}/{privacy:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------------------------

#[test]
fn credentials_are_redacted_in_debug_output() {
    let configs = [
        Auth::V1 {
            community: "s3cret-community".into(),
        },
        Auth::V2c {
            community: "s3cret-community".into(),
        },
        Auth::V3 {
            user: "operator".into(),
            auth: Some((AuthProtocol::Sha256, "s3cret-auth".into())),
            privacy: Some((PrivProtocol::Aes256, "s3cret-priv".into())),
            context_name: "ctx".into(),
        },
    ];
    for auth in configs {
        let config = ClientConfig::new("192.0.2.1:161".parse().unwrap(), auth);
        let printed = format!("{config:?}");
        assert!(!printed.contains("s3cret"), "{printed}");
        assert!(printed.contains("192.0.2.1:161"), "{printed}");
    }
    let v3 = v3_auth(Some(AuthProtocol::Sha256), Some(PrivProtocol::Aes256));
    assert_eq!(
        format!("{v3:?}"),
        r#"V3 { user: "operator", auth: Some((Sha256, "***")), privacy: Some((Aes256, "***")), context_name: "" }"#
    );
    assert_eq!(format!("{:?}", public()), r#"V2c { community: "***" }"#);
}

#[test]
fn auth_serde_matches_the_contract() {
    let cases = [
        (
            Auth::V1 {
                community: "public".into(),
            },
            r#"{"version":"v1","community":"public"}"#,
        ),
        (public(), r#"{"version":"v2c","community":"public"}"#),
        (
            Auth::V3 {
                user: "operator".into(),
                auth: Some((AuthProtocol::Sha256, "auth-pass".into())),
                privacy: Some((PrivProtocol::Aes128, "priv-pass".into())),
                context_name: "ctx".into(),
            },
            r#"{"version":"v3","user":"operator","auth":["sha256","auth-pass"],"privacy":["aes128","priv-pass"],"context_name":"ctx"}"#,
        ),
    ];
    for (auth, json) in cases {
        assert_eq!(serde_json::to_string(&auth).unwrap(), json);
        let back: Auth = serde_json::from_str(json).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
    }
    // The optional parts default.
    let minimal: Auth = serde_json::from_str(r#"{"version":"v3","user":"monitor"}"#).unwrap();
    assert!(matches!(
        minimal,
        Auth::V3 { ref user, auth: None, privacy: None, ref context_name }
            if user == "monitor" && context_name.is_empty()
    ));
}

#[tokio::test]
async fn config_defaults_and_validation() {
    let target: SocketAddr = "127.0.0.1:161".parse().unwrap();
    let config = ClientConfig::new(target, public());
    assert_eq!(config.timeout, Duration::from_secs(2));
    assert_eq!(config.retries, 1);
    assert_eq!(config.max_repetitions, 25);

    let client = Client::connect(config).await.unwrap();
    assert_eq!(client.target(), target);
    assert!(format!("{client:?}").contains("v2c"));

    // IPv6 targets get an IPv6 socket (where the host has IPv6 at all).
    let v6: SocketAddr = "[::1]:161".parse().unwrap();
    if let Ok(client) = Client::connect(ClientConfig::new(v6, public())).await {
        assert_eq!(client.target(), v6);
    }

    // Privacy without authentication does not exist; passphrases must not be empty.
    let no_auth_priv = Auth::V3 {
        user: "u".into(),
        auth: None,
        privacy: Some((PrivProtocol::Aes128, "priv-pass".into())),
        context_name: String::new(),
    };
    assert!(matches!(
        Client::connect(ClientConfig::new(target, no_auth_priv)).await,
        Err(SnmpError::Unsupported(_))
    ));
    for (auth_pass, priv_pass) in [("", "priv-pass"), ("auth-pass", "")] {
        let empty = Auth::V3 {
            user: "u".into(),
            auth: Some((AuthProtocol::Sha1, auth_pass.into())),
            privacy: Some((PrivProtocol::Aes128, priv_pass.into())),
            context_name: String::new(),
        };
        assert!(matches!(
            Client::connect(ClientConfig::new(target, empty)).await,
            Err(SnmpError::Auth(_))
        ));
    }
}

#[test]
fn id_sequences_stay_positive_and_wrap() {
    let mut ids = IdSequence(i32::MAX - 1);
    assert_eq!(ids.next(), i32::MAX);
    assert_eq!(ids.next(), 1);
    assert_eq!(ids.next(), 2);
    for _ in 0..100 {
        let mut ids = IdSequence::random();
        let first = ids.next();
        assert!(first >= 1);
        assert_eq!(ids.next(), if first == i32::MAX { 1 } else { first + 1 });
    }
    // Two sequences do not start at the same place (with overwhelming probability).
    let starts: std::collections::HashSet<i32> =
        (0..50).map(|_| IdSequence::random().next()).collect();
    assert!(starts.len() > 45);
}
