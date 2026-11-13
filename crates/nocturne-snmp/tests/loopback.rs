//! The client against the in-crate agent, over real UDP sockets on the loopback interface.
//! Nothing here needs an external daemon.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use nocturne_snmp::{
    Agent, Auth, AuthProtocol, Client, ClientConfig, Oid, SnmpError, Value, VarBind, names, oids,
};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;

type View = BTreeMap<Oid, Value>;

fn oid(text: &str) -> Oid {
    text.parse().expect("valid OID")
}

/// A simulated access switch: the system group and `ports` interfaces.
fn switch_view(ports: u32) -> View {
    let mut view = View::new();
    let mut put = |name: &str, value: Value| {
        view.insert(oid(name), value);
    };
    put(
        oids::SYS_DESCR,
        Value::from("Nocturne simulated switch, software 1.0"),
    );
    put(
        oids::SYS_OBJECT_ID,
        Value::Oid(oid("1.3.6.1.4.1.99999.1.1")),
    );
    put(oids::SYS_UP_TIME, Value::TimeTicks(123_456));
    put(oids::SYS_CONTACT, Value::from("noc@example.net"));
    put(oids::SYS_NAME, Value::from("lab-sw1"));
    put(oids::SYS_LOCATION, Value::from("rack 12"));
    put(oids::IF_NUMBER, Value::Integer(i64::from(ports)));
    for port in 1..=ports {
        let mut cell = |column: &str, value: Value| {
            view.insert(oid(column).child(port), value);
        };
        cell(oids::IF_INDEX, Value::Integer(i64::from(port)));
        cell(
            oids::IF_DESCR,
            Value::from(format!("GigabitEthernet0/{port}")),
        );
        cell(oids::IF_TYPE, Value::Integer(6));
        cell(
            oids::IF_PHYS_ADDRESS,
            Value::OctetString(vec![0x00, 0x1b, 0x21, 0x00, 0x00, port as u8]),
        );
        cell(
            oids::IF_OPER_STATUS,
            Value::Integer(if port.is_multiple_of(3) { 2 } else { 1 }),
        );
        // A 32-bit counter with the top bit set, and a 64-bit one beyond 32 bits.
        cell(oids::IF_IN_OCTETS, Value::Counter32(0xf000_0000 + port));
        cell(
            oids::IF_HC_IN_OCTETS,
            Value::Counter64((1 << 40) + u64::from(port)),
        );
        cell(oids::IF_HIGH_SPEED, Value::Gauge32(1000));
        // Only every other port has a description: a sparse column.
        if port.is_multiple_of(2) {
            cell(
                oids::IF_ALIAS,
                Value::from(format!("uplink to host-{port}")),
            );
        }
    }
    view
}

/// An agent serving `view`, and the task running it.
struct Running {
    agent: Agent,
    task: JoinHandle<Result<(), SnmpError>>,
}

impl Running {
    async fn start(view: View) -> Running {
        let agent = Agent::bind("127.0.0.1:0".parse().unwrap(), "public")
            .await
            .expect("bind agent");
        agent.set_view(view);
        let task = tokio::spawn(agent.clone().run());
        Running { agent, task }
    }

    fn addr(&self) -> SocketAddr {
        self.agent.local_addr()
    }

    async fn client(&self, auth: Auth) -> Client {
        client(self.addr(), auth, 1000, 0).await
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn client(target: SocketAddr, auth: Auth, timeout_ms: u64, retries: u32) -> Client {
    let mut config = ClientConfig::new(target, auth);
    config.timeout = Duration::from_millis(timeout_ms);
    config.retries = retries;
    Client::connect(config).await.expect("create client")
}

fn v1() -> Auth {
    Auth::V1 {
        community: "public".into(),
    }
}

fn v2c() -> Auth {
    Auth::V2c {
        community: "public".into(),
    }
}

/// What a walk of the whole view must return for a protocol version.
fn expected_walk(view: &View, with_counter64: bool) -> Vec<VarBind> {
    view.iter()
        .filter(|(_, value)| with_counter64 || !matches!(value, Value::Counter64(_)))
        .map(|(oid, value)| VarBind::new(oid.clone(), value.clone()))
        .collect()
}

// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn get_returns_values_exceptions_and_v1_errors() {
    let view = switch_view(4);
    let running = Running::start(view.clone()).await;
    let wanted = [
        oid(oids::SYS_NAME),
        oid(oids::SYS_UP_TIME),
        oid(oids::IF_IN_OCTETS).child(2),
        oid(oids::IF_PHYS_ADDRESS).child(3),
        oid(oids::SYS_OBJECT_ID),
    ];

    for auth in [v1(), v2c()] {
        let mut client = running.client(auth).await;
        let got = client.get(&wanted).await.unwrap();
        assert_eq!(got.len(), wanted.len());
        for (bind, requested) in got.iter().zip(&wanted) {
            assert_eq!(&bind.oid, requested);
            assert_eq!(&bind.value, &view[requested]);
        }
        assert_eq!(got[0].value.as_text().as_deref(), Some("lab-sw1"));
        assert_eq!(got[1].value.display(), "20m 34s");
        assert_eq!(got[2].value.as_u64(), Some(0xf000_0002));
        assert_eq!(got[3].value.display(), "00:1b:21:00:00:03");
        assert_eq!(
            names::describe(&got[2].oid).as_deref(),
            Some("ifInOctets.2")
        );
    }

    // SNMPv2c: what is missing comes back as an exception, in place.
    let mut client = running.client(v2c()).await;
    let got = client
        .get(&[
            oid(oids::SYS_NAME),
            oid(oids::IF_DESCR).child(99),
            oid("1.3.6.1.2.1.1.99.0"),
            oid(oids::IF_HC_IN_OCTETS).child(1),
        ])
        .await
        .unwrap();
    assert_eq!(got[0].value, Value::from("lab-sw1"));
    assert_eq!(got[1].value, Value::NoSuchInstance);
    assert_eq!(got[2].value, Value::NoSuchObject);
    assert_eq!(got[3].value, Value::Counter64((1 << 40) + 1));
    assert!(got[1].value.is_exception() && !got[3].value.is_exception());

    // SNMPv1: the request fails, and the index says which binding was at fault.
    let mut client = running.client(v1()).await;
    let error = client
        .get(&[
            oid(oids::SYS_NAME),
            oid(oids::SYS_CONTACT),
            oid("1.3.6.1.2.1.1.99.0"),
        ])
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 2,
                name: "noSuchName",
                index: 3
            }
        ),
        "{error:?}"
    );
    // Counter64 does not exist in SNMPv1.
    let error = client
        .get(&[oid(oids::IF_HC_IN_OCTETS).child(1)])
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 2,
                index: 1,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn get_next_steps_through_the_view() {
    let view = switch_view(2);
    let running = Running::start(view.clone()).await;
    let all: Vec<&Oid> = view.keys().collect();

    let mut client = running.client(v2c()).await;
    // From the root to the first object; from each object to the one after it.
    let first = client.get_next(&[oid("1.3")]).await.unwrap();
    assert_eq!(&first[0].oid, all[0]);
    for pair in all.windows(2) {
        let next = client
            .get_next(std::slice::from_ref(pair[0]))
            .await
            .unwrap();
        assert_eq!(&next[0].oid, pair[1]);
        assert_eq!(next[0].value, view[pair[1]]);
    }
    // Several at once, and past the end.
    let last = (*all.last().unwrap()).clone();
    let got = client
        .get_next(&[oid(oids::SYS_DESCR), oid("1.3.6.1.2.1.2"), last.clone()])
        .await
        .unwrap();
    assert_eq!(got[0].oid, oid(oids::SYS_OBJECT_ID));
    assert_eq!(got[1].oid, oid(oids::IF_NUMBER));
    assert_eq!(got[2], VarBind::new(last.clone(), Value::EndOfMibView));

    // SNMPv1 reports the end of the MIB as noSuchName.
    let mut client = running.client(v1()).await;
    let error = client
        .get_next(&[oid(oids::SYS_DESCR), last])
        .await
        .unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 2,
                index: 2,
                ..
            }
        ),
        "{error:?}"
    );
}

#[tokio::test]
async fn bulk_walk_across_several_hundred_oids() {
    let view = switch_view(48);
    assert!(view.len() > 400, "{} objects", view.len());
    let running = Running::start(view.clone()).await;
    let everything = oid("1.3.6.1");

    // SNMPv2c with GETBULK, at several bulk sizes.
    let expected = expected_walk(&view, true);
    for max_repetitions in [1, 2, 7, 25, 100, 1000] {
        let mut config = ClientConfig::new(running.addr(), v2c());
        config.max_repetitions = max_repetitions;
        let mut client = Client::connect(config).await.unwrap();
        let walked = client.walk(&everything, usize::MAX).await.unwrap();
        assert_eq!(
            walked.len(),
            expected.len(),
            "max-repetitions {max_repetitions}"
        );
        assert_eq!(walked, expected, "max-repetitions {max_repetitions}");
    }

    // SNMPv1 with GETNEXT: the same, minus the Counter64 objects.
    let mut client = running.client(v1()).await;
    let walked = client.walk(&everything, usize::MAX).await.unwrap();
    assert_eq!(walked, expected_walk(&view, false));
    assert!(walked.len() < expected.len());

    // Subtrees and limits.
    let mut client = running.client(v2c()).await;
    let descriptions = client.walk(&oid(oids::IF_DESCR), usize::MAX).await.unwrap();
    assert_eq!(descriptions.len(), 48);
    assert_eq!(
        descriptions[9].value.as_text().as_deref(),
        Some("GigabitEthernet0/10")
    );
    assert_eq!(descriptions[9].oid, oid(oids::IF_DESCR).child(10));
    let if_table = client.walk(&oid(oids::IF_TABLE), usize::MAX).await.unwrap();
    assert_eq!(if_table.len(), 48 * 6);
    for limit in [1, 24, 25, 26, 47, 48, 49, 5000] {
        let limited = client.walk(&oid(oids::IF_DESCR), limit).await.unwrap();
        assert_eq!(limited, descriptions[..limit.min(48)], "limit {limit}");
    }
    assert!(client.walk(&everything, 0).await.unwrap().is_empty());

    // Walking an instance yields the instance; walking a hole yields nothing.
    let single = client.walk(&oid(oids::SYS_NAME), usize::MAX).await.unwrap();
    assert_eq!(
        single,
        vec![VarBind::new(oid(oids::SYS_NAME), Value::from("lab-sw1"))]
    );
    assert!(
        client
            .walk(&oid("1.3.6.1.2.1.77"), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        client
            .walk(&oid("1.3.6.1.2.1.1.5.0.1"), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn table_pivots_columns_into_rows() {
    let view = switch_view(48);
    let running = Running::start(view).await;
    let columns = [
        oid(oids::IF_DESCR),
        oid(oids::IF_OPER_STATUS),
        oid(oids::IF_HC_IN_OCTETS),
        oid(oids::IF_ALIAS),
        // A column the agent does not have at all.
        oid(oids::IF_OUT_OCTETS),
    ];

    let mut client = running.client(v2c()).await;
    let table = client.table(&columns, usize::MAX).await.unwrap();
    assert_eq!(table.len(), 48);
    for (position, (index, row)) in table.iter().enumerate() {
        let port = position as u32 + 1;
        // Rows come in index order.
        assert_eq!(index, &vec![port]);
        assert_eq!(
            row[&columns[0]].as_text().as_deref(),
            Some(format!("GigabitEthernet0/{port}").as_str())
        );
        assert_eq!(
            row[&columns[1]].as_i64(),
            Some(if port.is_multiple_of(3) { 2 } else { 1 })
        );
        assert_eq!(row[&columns[2]].as_u64(), Some((1 << 40) + u64::from(port)));
        // The sparse column is present on even ports only; cells keep the column order.
        let present: Vec<&Oid> = row.keys().collect();
        if port.is_multiple_of(2) {
            assert_eq!(
                present,
                [&columns[0], &columns[1], &columns[2], &columns[3]]
            );
            assert_eq!(
                row[&columns[3]].as_text().as_deref(),
                Some(format!("uplink to host-{port}").as_str())
            );
        } else {
            assert_eq!(present, [&columns[0], &columns[1], &columns[2]]);
        }
    }

    // A row limit returns the first rows, complete.
    let limited = client.table(&columns, 5).await.unwrap();
    assert_eq!(limited.len(), 5);
    assert_eq!(
        limited.keys().collect::<Vec<_>>(),
        table.keys().take(5).collect::<Vec<_>>()
    );
    assert_eq!(limited[&vec![4u32]], table[&vec![4u32]]);
    assert!(client.table(&columns, 0).await.unwrap().is_empty());

    // SNMPv1 pivots too, without the Counter64 column.
    let mut client = running.client(v1()).await;
    let v1_table = client.table(&columns, usize::MAX).await.unwrap();
    assert_eq!(v1_table.len(), 48);
    assert!(v1_table.values().all(|row| !row.contains_key(&columns[2])));
    assert_eq!(v1_table[&vec![2u32]].len(), 3);
    assert_eq!(
        v1_table[&vec![2u32]][&columns[0]],
        table[&vec![2u32]][&columns[0]]
    );

    // A table that does not exist is empty, not an error.
    assert!(
        client
            .table(&[oid("1.3.6.1.2.1.99.1.1.1.4")], usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn too_big_responses_are_handled() {
    let view = switch_view(48);
    let running = Running::start(view.clone()).await;
    // The smallest message size every SNMP entity must accept (RFC 3417).
    running.agent.set_max_message_size(484);
    let mut client = running.client(v2c()).await;

    // 48 descriptions do not fit in 484 octets: the agent says tooBig, the client splits the
    // request until the pieces fit and still returns one answer, in order.
    let wanted: Vec<Oid> = (1..=48)
        .map(|port| oid(oids::IF_DESCR).child(port))
        .collect();
    let got = client.get(&wanted).await.unwrap();
    assert_eq!(got.len(), 48);
    for (bind, requested) in got.iter().zip(&wanted) {
        assert_eq!(&bind.oid, requested);
        assert_eq!(bind.value, view[requested]);
    }
    let next = client.get_next(&wanted).await.unwrap();
    assert_eq!(next.len(), 48);
    assert_eq!(next[0].oid, wanted[1]);

    // GETBULK responses are cut short by the agent; walks and tables continue regardless.
    let walked = client.walk(&oid("1.3.6.1"), usize::MAX).await.unwrap();
    assert_eq!(walked, expected_walk(&view, true));
    let columns = [
        oid(oids::IF_DESCR),
        oid(oids::IF_ALIAS),
        oid(oids::IF_HC_IN_OCTETS),
    ];
    let table = client.table(&columns, usize::MAX).await.unwrap();
    assert_eq!(table.len(), 48);
    assert_eq!(table.values().filter(|row| row.len() == 3).count(), 24);
    let bulk = client
        .get_bulk(&[oid(oids::IF_DESCR)], 0, 40)
        .await
        .unwrap();
    assert!(
        !bulk.is_empty() && bulk.len() < 40,
        "{} bindings",
        bulk.len()
    );

    // SNMPv1 at the same size limit.
    let mut v1_client = running.client(v1()).await;
    assert_eq!(v1_client.get(&wanted).await.unwrap(), got);
    assert_eq!(
        v1_client.walk(&oid("1.3.6.1"), usize::MAX).await.unwrap(),
        expected_walk(&view, false)
    );

    // One value that can never fit is a tooBig the client cannot cure.
    let mut big = view.clone();
    big.insert(
        oid("1.3.6.1.4.1.99999.9.1.0"),
        Value::OctetString(vec![0x55; 600]),
    );
    running.agent.set_view(big);
    let error = client
        .get(&[oid("1.3.6.1.4.1.99999.9.1.0")])
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
    // ...and with a larger limit it is served.
    running.agent.set_max_message_size(65_507);
    let got = client.get(&[oid("1.3.6.1.4.1.99999.9.1.0")]).await.unwrap();
    assert_eq!(got[0].value, Value::OctetString(vec![0x55; 600]));
}

#[tokio::test]
async fn timeouts_and_wrong_credentials() {
    let running = Running::start(switch_view(2)).await;
    let sys_name = [oid(oids::SYS_NAME)];

    // The wrong community gets no answer: a timeout after every attempt has run its course.
    let wrong = Auth::V2c {
        community: "private".into(),
    };
    let mut stranger = client(running.addr(), wrong, 100, 2).await;
    let started = Instant::now();
    let error = stranger.get(&sys_name).await.unwrap_err();
    let elapsed = started.elapsed();
    assert!(matches!(error, SnmpError::Timeout), "{error:?}");
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
    assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
    assert!(matches!(
        stranger.walk(&oid("1.3"), 10).await,
        Err(SnmpError::Timeout)
    ));
    assert!(matches!(
        stranger.table(&sys_name, 10).await,
        Err(SnmpError::Timeout)
    ));

    // This agent speaks v1 and v2c only: SNMPv3 discovery goes unanswered.
    let v3 = Auth::V3 {
        user: "operator".into(),
        auth: Some((AuthProtocol::Sha256, "a-passphrase".into())),
        privacy: None,
        context_name: String::new(),
    };
    let mut v3_client = client(running.addr(), v3, 100, 0).await;
    assert!(matches!(
        v3_client.get(&sys_name).await,
        Err(SnmpError::Timeout)
    ));

    // Nobody listening at all.
    let unused = UdpSocket::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let mut nobody = client(unused, v2c(), 100, 1).await;
    let started = Instant::now();
    assert!(matches!(
        nobody.get(&sys_name).await,
        Err(SnmpError::Timeout)
    ));
    assert!(started.elapsed() >= Duration::from_millis(200));

    // The right community still works, and so does the agent after all of the above.
    let mut friend = running.client(v2c()).await;
    assert_eq!(
        friend.get(&sys_name).await.unwrap()[0].value,
        Value::from("lab-sw1")
    );

    // SET is refused: this agent is read-only.
    let bind = VarBind::new(oid(oids::SYS_NAME), Value::from("renamed"));
    let error = friend.set(std::slice::from_ref(&bind)).await.unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 17,
                name: "notWritable",
                index: 1
            }
        ),
        "{error:?}"
    );
    let error = running.client(v1()).await.set(&[bind]).await.unwrap_err();
    assert!(
        matches!(
            error,
            SnmpError::Agent {
                status: 2,
                name: "noSuchName",
                index: 1
            }
        ),
        "{error:?}"
    );
    assert_eq!(
        friend.get(&sys_name).await.unwrap()[0].value,
        Value::from("lab-sw1")
    );
}

/// A UDP relay between a client and the agent that loses requests and holds back replies.
struct Relay {
    addr: SocketAddr,
    /// Requests that arrived from the client.
    seen: Arc<AtomicUsize>,
    task: JoinHandle<()>,
}

impl Relay {
    /// Requests are numbered from 0 as they arrive. Those in `lost` vanish; the replies to
    /// those in `delayed` are held back for `delay`.
    async fn start(agent: SocketAddr, lost: &[usize], delayed: &[usize], delay: Duration) -> Relay {
        let (lost, delayed) = (lost.to_vec(), delayed.to_vec());
        let front = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = front.local_addr().unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&seen);
        let task = tokio::spawn(async move {
            let mut buf = vec![0u8; 65_535];
            loop {
                let Ok((len, client)) = front.recv_from(&mut buf).await else {
                    return;
                };
                let number = counter.fetch_add(1, Ordering::SeqCst);
                if lost.contains(&number) {
                    continue;
                }
                let request = buf[..len].to_vec();
                let front = Arc::clone(&front);
                let hold = delayed.contains(&number).then_some(delay);
                // Each request gets its own socket towards the agent, so its reply finds the
                // way back without any bookkeeping.
                tokio::spawn(async move {
                    let back = UdpSocket::bind("127.0.0.1:0").await.unwrap();
                    back.send_to(&request, agent).await.unwrap();
                    let mut reply = vec![0u8; 65_535];
                    let wait =
                        tokio::time::timeout(Duration::from_secs(2), back.recv_from(&mut reply));
                    if let Ok(Ok((len, _))) = wait.await {
                        if let Some(hold) = hold {
                            tokio::time::sleep(hold).await;
                        }
                        let _ = front.send_to(&reply[..len], client).await;
                    }
                });
            }
        });
        Relay { addr, seen, task }
    }

    fn seen(&self) -> usize {
        self.seen.load(Ordering::SeqCst)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn retries_recover_lost_datagrams() {
    let running = Running::start(switch_view(8)).await;
    let sys_name = [oid(oids::SYS_NAME)];
    let lab_sw1 = Value::from("lab-sw1");

    // Two transmissions lost, two retries configured: the third one gets through.
    let relay = Relay::start(running.addr(), &[0, 1], &[], Duration::ZERO).await;
    let mut patient = client(relay.addr, v2c(), 200, 2).await;
    let started = Instant::now();
    assert_eq!(patient.get(&sys_name).await.unwrap()[0].value, lab_sw1);
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "two timeouts were sat out"
    );
    assert_eq!(relay.seen(), 3);
    // The link is clean now: a whole walk goes through without further retransmissions.
    let walked = patient
        .walk(&oid(oids::IF_DESCR), usize::MAX)
        .await
        .unwrap();
    assert_eq!(walked.len(), 8);
    assert_eq!(relay.seen(), 4);

    // The same loss with one retry fewer is a timeout.
    let relay = Relay::start(running.addr(), &[0, 1], &[], Duration::ZERO).await;
    let mut impatient = client(relay.addr, v2c(), 200, 1).await;
    assert!(matches!(
        impatient.get(&sys_name).await,
        Err(SnmpError::Timeout)
    ));
    assert_eq!(relay.seen(), 2);
    // The next request has a retry to spare again.
    assert_eq!(impatient.get(&sys_name).await.unwrap()[0].value, lab_sw1);

    // Losses in the middle of a walk are ridden out as well.
    let relay = Relay::start(running.addr(), &[1, 3, 4, 7], &[], Duration::ZERO).await;
    let mut config = ClientConfig::new(relay.addr, v2c());
    config.timeout = Duration::from_millis(200);
    config.retries = 2;
    config.max_repetitions = 3;
    let mut walker = Client::connect(config).await.unwrap();
    let walked = walker.walk(&oid(oids::IF_DESCR), usize::MAX).await.unwrap();
    assert_eq!(walked.len(), 8);
    assert!(walked.windows(2).all(|pair| pair[0].oid < pair[1].oid));
}

#[tokio::test]
async fn late_replies_are_matched_or_ignored() {
    let running = Running::start(switch_view(8)).await;
    let late = Duration::from_millis(450);

    // The reply to the first transmission is held back past the timeout and the retry is
    // lost: the late reply arrives while the client waits for the retry's answer. It carries
    // the request id of this very request, and is taken.
    let relay = Relay::start(running.addr(), &[1], &[0], late).await;
    let mut client_a = client(relay.addr, v2c(), 300, 1).await;
    let got = client_a.get(&[oid(oids::SYS_NAME)]).await.unwrap();
    assert_eq!(got[0].value, Value::from("lab-sw1"));
    assert_eq!(relay.seen(), 2);

    // Here the retry is answered at once, and the held-back first reply turns up later, in
    // the middle of the requests that follow. It must not be taken for any of their answers.
    let relay = Relay::start(running.addr(), &[], &[0], late).await;
    let mut client_b = client(relay.addr, v2c(), 300, 1).await;
    let got = client_b.get(&[oid(oids::SYS_NAME)]).await.unwrap();
    assert_eq!(got[0].value, Value::from("lab-sw1"));
    let started = Instant::now();
    let mut round = 0u32;
    while started.elapsed() < Duration::from_millis(400) {
        let port = round % 8 + 1;
        round += 1;
        let wanted = oid(oids::IF_DESCR).child(port);
        let got = client_b.get(std::slice::from_ref(&wanted)).await.unwrap();
        assert_eq!(got[0].oid, wanted);
        assert_eq!(
            got[0].value.as_text().as_deref(),
            Some(format!("GigabitEthernet0/{port}").as_str())
        );
    }
    assert!(round > 8, "only {round} requests in 400 ms");
}

#[tokio::test]
async fn view_updates_are_atomic() {
    // Two objects that always change together.
    const FIRST: &str = "1.3.6.1.4.1.99999.2.1.0";
    const SECOND: &str = "1.3.6.1.4.1.99999.2.2.0";
    const GENERATIONS: u32 = 300;
    fn view_for(generation: u32) -> View {
        let mut view = switch_view(4);
        view.insert(oid(FIRST), Value::Counter32(generation));
        view.insert(oid(SECOND), Value::Counter32(generation));
        view
    }
    let pair = [oid(FIRST), oid(SECOND)];
    let running = Running::start(view_for(0)).await;

    let agent = running.agent.clone();
    let updater = tokio::spawn(async move {
        for generation in 1..=GENERATIONS {
            agent.set_view(view_for(generation));
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    });

    let mut client = running.client(v2c()).await;
    let mut generations = Vec::new();
    while !updater.is_finished() {
        let got = client.get(&pair).await.unwrap();
        // Never half of one view and half of the next.
        assert_eq!(got[0].value, got[1].value);
        generations.push(got[0].value.as_u64().unwrap());
        // The same holds across the bindings of a bulk response.
        let bulk = client
            .get_bulk(&[oid("1.3.6.1.4.1.99999.2")], 0, 2)
            .await
            .unwrap();
        assert_eq!(bulk[0].value, bulk[1].value);
    }
    updater.await.unwrap();
    assert!(
        generations.windows(2).all(|pair| pair[0] <= pair[1]),
        "updates are seen in order"
    );
    assert!(
        generations.len() > 20,
        "only {} polls during the updates",
        generations.len()
    );
    assert_eq!(
        client.get(&pair).await.unwrap()[0].value,
        Value::Counter32(GENERATIONS)
    );

    // An empty view is a valid view.
    running.agent.set_view(View::new());
    let got = client.get(&[oid(oids::SYS_NAME)]).await.unwrap();
    assert_eq!(got[0].value, Value::NoSuchObject);
    assert!(
        client
            .walk(&oid("1.3"), usize::MAX)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn many_clients_share_one_agent() {
    let view = switch_view(24);
    let running = Running::start(view.clone()).await;
    let expected = expected_walk(&view, true);

    let mut tasks = Vec::new();
    for worker in 0..32u32 {
        let addr = running.addr();
        let expected = expected.clone();
        tasks.push(tokio::spawn(async move {
            let mut client = client(addr, v2c(), 2000, 1).await;
            for _ in 0..3 {
                let walked = client.walk(&oid("1.3.6.1"), usize::MAX).await.unwrap();
                assert_eq!(walked, expected, "worker {worker}");
            }
            let port = worker % 24 + 1;
            let got = client
                .get(&[oid(oids::IF_INDEX).child(port)])
                .await
                .unwrap();
            assert_eq!(got[0].value, Value::Integer(i64::from(port)));
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }
}

#[tokio::test]
async fn the_agent_serves_only_while_run_is_polled() {
    let agent = Agent::bind("127.0.0.1:0".parse().unwrap(), "public")
        .await
        .unwrap();
    agent.set_view(switch_view(2));
    let sys_name = [oid(oids::SYS_NAME)];
    let mut client = client(agent.local_addr(), v2c(), 100, 0).await;

    // Bound but not running: requests wait in the socket, unanswered.
    assert!(matches!(
        client.get(&sys_name).await,
        Err(SnmpError::Timeout)
    ));

    // Running: served — including nothing stale from the request that timed out.
    let server = tokio::spawn(agent.clone().run());
    assert_eq!(
        client.get(&sys_name).await.unwrap()[0].value,
        Value::from("lab-sw1")
    );

    // Aborted: silence again. The handle is still good for another run.
    server.abort();
    let _ = server.await;
    assert!(matches!(
        client.get(&sys_name).await,
        Err(SnmpError::Timeout)
    ));
    let server = tokio::spawn(agent.clone().run());
    assert_eq!(
        client.get(&sys_name).await.unwrap()[0].value,
        Value::from("lab-sw1")
    );
    server.abort();
}

#[tokio::test]
async fn ipv6_loopback() {
    // Not every build host has IPv6; skip quietly where it does not.
    let Ok(agent) = Agent::bind("[::1]:0".parse().unwrap(), "public").await else {
        eprintln!("no IPv6 loopback here: skipped");
        return;
    };
    agent.set_view(switch_view(3));
    let addr = agent.local_addr();
    assert!(addr.is_ipv6());
    let task = tokio::spawn(agent.run());
    let mut client = client(addr, v2c(), 1000, 0).await;
    let walked = client.walk(&oid(oids::IF_DESCR), usize::MAX).await.unwrap();
    assert_eq!(walked.len(), 3);
    task.abort();
}

/// Not a test of correctness but a yardstick: how long walks and tables of a large MIB take
/// against the in-crate agent on the loopback interface.
/// `cargo test -p nocturne-snmp --test loopback -- --ignored --nocapture`
#[tokio::test]
#[ignore = "timing report, run on demand"]
async fn large_table_timings() {
    const ROWS: u32 = 10_000;
    let columns: Vec<Oid> = [
        oids::IF_DESCR,
        oids::IF_OPER_STATUS,
        oids::IF_IN_OCTETS,
        oids::IF_HC_IN_OCTETS,
        oids::IF_ALIAS,
    ]
    .iter()
    .map(|column| oid(column))
    .collect();
    let mut view = View::new();
    for row in 1..=ROWS {
        view.insert(
            columns[0].child(row),
            Value::from(format!("TenGigabitEthernet1/0/{row}")),
        );
        view.insert(columns[1].child(row), Value::Integer(1));
        view.insert(columns[2].child(row), Value::Counter32(row));
        view.insert(
            columns[3].child(row),
            Value::Counter64(u64::from(row) << 20),
        );
        view.insert(
            columns[4].child(row),
            Value::from(format!("customer circuit {row}")),
        );
    }
    let objects = view.len();
    let running = Running::start(view).await;

    for (label, auth, max_repetitions, limit) in [
        ("v2c walk, max-repetitions 25 (default)", v2c(), 25, 65_507),
        ("v2c walk, max-repetitions 100", v2c(), 100, 65_507),
        (
            "v2c walk, max-repetitions 25, agent limited to 1472 octets",
            v2c(),
            25,
            1472,
        ),
        ("v1 walk (GETNEXT)", v1(), 25, 65_507),
    ] {
        running.agent.set_max_message_size(limit);
        let mut config = ClientConfig::new(running.addr(), auth);
        config.max_repetitions = max_repetitions;
        let mut client = Client::connect(config).await.unwrap();
        let started = Instant::now();
        let walked = client.walk(&oid("1.3.6.1"), usize::MAX).await.unwrap();
        let elapsed = started.elapsed();
        println!(
            "{label}: {} objects in {elapsed:.2?} ({:.0} objects/s)",
            walked.len(),
            walked.len() as f64 / elapsed.as_secs_f64()
        );
        assert!(walked.len() == objects || walked.len() == objects - ROWS as usize);
    }

    for (label, max_repetitions, limit) in [
        (
            "v2c table, 5 columns, max-repetitions 25 (default)",
            25,
            65_507,
        ),
        ("v2c table, 5 columns, max-repetitions 100", 100, 65_507),
        (
            "v2c table, 5 columns, agent limited to 1472 octets",
            25,
            1472,
        ),
    ] {
        running.agent.set_max_message_size(limit);
        let mut config = ClientConfig::new(running.addr(), v2c());
        config.max_repetitions = max_repetitions;
        let mut client = Client::connect(config).await.unwrap();
        let started = Instant::now();
        let table = client.table(&columns, usize::MAX).await.unwrap();
        let elapsed = started.elapsed();
        println!(
            "{label}: {} rows in {elapsed:.2?} ({:.0} rows/s)",
            table.len(),
            table.len() as f64 / elapsed.as_secs_f64()
        );
        assert_eq!(table.len(), ROWS as usize);
        assert!(table.values().all(|row| row.len() == 5));
    }
}
