//! The SNMP trap receiver: real datagrams into a real socket on port 0.
//!
//! The datagrams below were not written by hand. Each was sent by net-snmp
//! 5.9.4's `snmptrap` or `snmpinform` on the machine this was written on
//! and captured off the wire (a UDP socket that printed what it received);
//! the command is above each one. net-snmp sent them — no switch or router
//! did. They are replayed here so that the receiver is tested on machines
//! that have no net-snmp, and the tests at the end run the tools themselves
//! where they are installed and say that they were skipped where not.
//!
//! OIDs are given to net-snmp as numbers: this machine has its tools and
//! not its MIB files, and CI may have neither.
//!
//! Two communities appear: `n0cturne-ro`, which is what the test stores in
//! the vault for the switch, and `gu3ssed-rw`, which is wrong. Both are
//! secrets, and a good part of this file is about neither turning up
//! anywhere.

mod support;

use std::time::Duration;

use nocturne_app::App;
use nocturne_app::testing::Lab;
use nocturne_app::util::now_ms;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;

const COMMUNITY: &str = "n0cturne-ro";
const WRONG: &str = "gu3ssed-rw";

/// `snmptrap -v 2c -c n0cturne-ro <receiver> 12345 1.3.6.1.6.3.1.1.5.3
/// 1.3.6.1.2.1.2.2.1.1.3 i 3 1.3.6.1.2.1.2.2.1.7.3 i 1 1.3.6.1.2.1.2.2.1.8.3 i 2`
/// — linkDown, ifIndex.3 = 3, ifAdminStatus.3 = up, ifOperStatus.3 = down.
const V2C_LINK_DOWN: &str = "
    307c020101040b6e30637475726e652d726fa76a020460ca908d020100020100305c300e06082b060102
    01010300430230393017060a2b06010603010104010006092b0601060301010503300f060a2b06010201
    0202010103020103300f060a2b060102010202010703020101300f060a2b060102010202010803020102";

/// `snmptrap -v 2c -c n0cturne-ro <receiver> 12999 1.3.6.1.6.3.1.1.5.4 …` —
/// linkUp for the same interface, ifOperStatus.3 = up.
const V2C_LINK_UP: &str = "
    307c020101040b6e30637475726e652d726fa76a02043dc0c55a020100020100305c300e06082b060102
    01010300430232c73017060a2b06010603010104010006092b0601060301010504300f060a2b06010201
    0202010103020103300f060a2b060102010202010703020101300f060a2b060102010202010803020101";

/// The linkDown above, sent with `-c gu3ssed-rw`.
const V2C_LINK_DOWN_WRONG_COMMUNITY: &str = "
    307b020101040a677533737365642d7277a76a020457e98011020100020100305c300e06082b06010201
    010300430230393017060a2b06010603010104010006092b0601060301010503300f060a2b0601020102
    02010103020103300f060a2b060102010202010703020101300f060a2b060102010202010803020102";

/// `snmptrap -v 1 -c n0cturne-ro <receiver> 1.3.6.1.4.1.8072.3.2.10 192.0.2.1 2 0 12345
/// 1.3.6.1.2.1.2.2.1.1.3 i 3 1.3.6.1.2.1.2.2.1.7.3 i 1 1.3.6.1.2.1.2.2.1.8.3 i 2`
/// — an SNMPv1 generic trap 2 (linkDown) claiming the agent address 192.0.2.1.
const V1_LINK_DOWN: &str = "
    3063020100040b6e30637475726e652d726fa451060a2b06010401bf0803020a4004c000020102010202
    0100430230393033300f060a2b060102010202010103020103300f060a2b060102010202010703020101
    300f060a2b060102010202010803020102";

/// `snmptrap -v 2c -c n0cturne-ro <receiver> 700 1.3.6.1.6.3.1.1.5.1` — coldStart.
const V2C_COLD_START: &str = "
    3049020101040b6e30637475726e652d726fa73702042bec4b1c0201000201003029300e06082b060102
    01010300430202bc3017060a2b06010603010104010006092b0601060301010501";

/// `snmptrap -v 2c -c n0cturne-ro <receiver> 800 1.3.6.1.6.3.1.1.5.2` — warmStart.
const V2C_WARM_START: &str = "
    3049020101040b6e30637475726e652d726fa737020433e0ac5f0201000201003029300e06082b060102
    01010300430203203017060a2b06010603010104010006092b0601060301010502";

/// `snmptrap -v 2c -c n0cturne-ro <receiver> 90000 1.3.6.1.6.3.1.1.5.5` —
/// authenticationFailure.
const V2C_AUTH_FAILURE: &str = "
    304a020101040b6e30637475726e652d726fa73802042657319a020100020100302a300f06082b060102
    010103004303015f903017060a2b06010603010104010006092b0601060301010505";

/// `snmpinform -v 2c -c n0cturne-ro -r 0 -t 1 <receiver> 999 1.3.6.1.6.3.1.1.5.4
/// 1.3.6.1.2.1.2.2.1.1.5 i 5 1.3.6.1.2.1.2.2.1.7.5 i 1 1.3.6.1.2.1.2.2.1.8.5 i 1`
/// — an InformRequest: linkUp for ifIndex 5.
const V2C_INFORM_LINK_UP: &str = "
    307c020101040b6e30637475726e652d726fa66a02042fd37105020100020100305c300e06082b060102
    01010300430203e73017060a2b06010603010104010006092b0601060301010504300f060a2b06010201
    0202010105020105300f060a2b060102010202010705020101300f060a2b060102010202010805020101";

/// The inform above, sent with `-c gu3ssed-rw`.
const V2C_INFORM_WRONG_COMMUNITY: &str = "
    307b020101040a677533737365642d7277a66a020433fc818f020100020100305c300e06082b06010201
    010300430203e73017060a2b06010603010104010006092b0601060301010504300f060a2b0601020102
    02010105020105300f060a2b060102010202010705020101300f060a2b060102010202010805020101";

/// `snmptrap -v 3 -e 0x80001f8880e5d1a04e2b3c6d5f -u trapuser -a SHA -A authpass-12345
/// -x AES -X privpass-12345 -l authPriv <receiver> 4242 1.3.6.1.6.3.1.1.5.3 …` —
/// the linkDown, authenticated and encrypted.
const V3_AUTH_PRIV: &str = "
    3081da020103301102043ef00b9e020300ffe3040103020103043b3039040d80001f8880e5d1a04e2b3c
    6d5f020101020320012f04087472617075736572040c5b2988f912b4ca05781acfd40408b8e23e9db9bb
    6a760481848f6a77aff1c9295d21e6cab3c4da1e192b303e5cec2476917f87318aa531264a274e60fa55
    b3c166575eed54e87b80e1c7c8a28016612db4d23c81f5b757e42de2b22ce38166f9fc4cc66f74eac387
    deb2f76ef1737430e4aa344272730dce68f92b65559bfde8f1386862aaac941ee8017dc1bb9caef558ee
    da1cff273f3c1dfaafa7f7";

/// The same with `-l authNoPriv` (no `-x`/`-X`): signed, and readable by
/// anyone — which does not make it verified.
const V3_AUTH_NO_PRIV: &str = "
    3081cf0201033011020456cd3d19020300ffe304010102010304333031040d80001f8880e5d1a04e2b3c
    6d5f020101020320013004087472617075736572040ce02942dbf2e126e859c87d780400308181041180
    001f88803e6fd26a8b60036b000000000400a76a020477511184020100020100305c300e06082b060102
    01010300430210923017060a2b06010603010104010006092b0601060301010503300f060a2b06010201
    0202010103020103300f060a2b060102010202010703020101300f060a2b060102010202010803020102";

/// The same with `-l noAuthNoPriv`: a user name and nothing else.
const V3_NO_AUTH: &str = "
    3081c302010330110204342dcb46020300ffe304010002010304273025040d80001f8880e5d1a04e2b3c
    6d5f02010102032001310408747261707573657204000400308181041180001f888052e45b438b60036b
    000000000400a76a02041b252021020100020100305c300e06082b06010201010300430210923017060a
    2b06010603010104010006092b0601060301010503300f060a2b060102010202010103020103300f060a
    2b060102010202010703020101300f060a2b060102010202010803020102";

/// What `snmpinform -v 3 … -l authPriv` sends first: not a notification
/// but the engine-discovery request that precedes one.
const V3_INFORM_DISCOVERY: &str = "
    304f02010330110204325736ea020300ffe30401040201030410300e0400020100020100040004000400
    3025041180001f8880d4ce96658b60036b000000000400a00e0204426b4f490201000201003000";

/// `snmptrap -v 2c -c n0cturne-ro <receiver> 5000 1.3.6.1.4.1.99999.0.7
/// 1.3.6.1.4.1.99999.1.1.0 s "snmp-server community Str0ngR0 RO"
/// 1.3.6.1.2.1.1.5.0 s edge-sw-7` — a notification nobody has a name for,
/// carrying a configuration line with a secret in it.
const V2C_CONFIG_LINE: &str = "
    308194020101040b6e30637475726e652d726fa78181020443fca4410201000201003073300e06082b06
    010201010300430213883018060a2b060106030101040100060a2b06010401868d1f00073030060b2b06
    010401868d1f0101000421736e6d702d73657276657220636f6d6d756e69747920537472306e67523020
    524f301506082b060102010105000409656467652d73772d37";

/// `snmptrap -v 2c -c n0cturne-ro <receiver> 12345 1.3.6.1.6.3.1.1.5.3
/// 1.3.6.1.2.1.2.2.1.1.3 i 3 1.3.6.1.6.3.18.1.3.0 a 192.0.2.44
/// 1.3.6.1.6.3.18.1.4.0 s 0riginal-c0mm
/// 1.3.6.1.4.1.99999.1.9.0 s "sent with n0cturne-ro by a proxy"` — a linkDown
/// with the two bindings a trap proxy adds (snmpTrapAddress.0 and
/// snmpTrapCommunity.0, RFC 3584), and one that repeats the datagram's own
/// community in a sentence.
const V2C_FORWARDED: &str = "
    3081bc020101040b6e30637475726e652d726fa781a902040fb0c26902010002010030819a300e06082b
    06010201010300430230393017060a2b06010603010104010006092b0601060301010503300f060a2b06
    0102010202010103020103301106092b06010603120103004004c000022c301a06092b06010603120104
    00040d30726967696e616c2d63306d6d302f060b2b06010401868d1f010900042073656e742077697468
    206e30637475726e652d726f20627920612070726f7879";

/// One switch at 127.0.0.1 whose credential holds the community.
async fn switch() -> (Lab, String) {
    let lab = lab(&["SW1"]).await;
    let id = lab.id("SW1");
    set_address(&lab.app, &id, "127.0.0.1").await;
    set_community(&lab.app, &lab.credential_id, COMMUNITY).await;
    (lab, id)
}

/// The same with a stand-in for the monitoring feature's alert methods.
async fn switch_with_monitor() -> (Lab, FakeMonitor, String) {
    let (lab, monitor) = lab_with_monitor(&["SW1"]).await;
    let id = lab.id("SW1");
    set_address(&lab.app, &id, "127.0.0.1").await;
    set_community(&lab.app, &lab.credential_id, COMMUNITY).await;
    (lab, monitor, id)
}

/// The stored traps, newest first.
async fn traps(app: &App) -> Vec<Value> {
    query(app, json!({"stream": "trap", "limit": 200})).await
}

fn count(status: &Value, key: &str) -> u64 {
    status["traps"][key].as_u64().unwrap_or_default()
}

/// What comes back on the socket an inform was sent from, if anything does.
async fn answer_on(socket: &tokio::net::UdpSocket, wait: Duration) -> Option<Vec<u8>> {
    let mut buffer = vec![0u8; 65_535];
    match tokio::time::timeout(wait, socket.recv_from(&mut buffer)).await {
        Ok(Ok((length, _))) => Some(buffer[..length].to_vec()),
        _ => None,
    }
}

/// The acknowledgement RFC 3416 asks for: the inform with its PDU turned
/// into a Response (tag 0xa2 for 0xa6) — same community, same request id,
/// same bindings.
fn response_to(inform: &[u8]) -> Vec<u8> {
    let mut response = inform.to_vec();
    let tag = response
        .iter()
        .position(|byte| *byte == 0xa6)
        .expect("an InformRequest PDU");
    response[tag] = 0xa2;
    response
}

/// Every event published so far, up to and including the live tail's next
/// one: a stored trap goes out on `logs.appended` a moment after it is
/// stored, and that is the event with the most of a trap in it.
async fn events_through_the_tail(
    events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>,
) -> Vec<(String, Value)> {
    let mut seen = Vec::new();
    let tail = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match events.recv().await {
                Ok(event) => {
                    let last = event.topic == "logs.appended";
                    seen.push((event.topic, event.payload));
                    if last {
                        return;
                    }
                }
                Err(err) => panic!("the event bus failed while it was being searched: {err}"),
            }
        }
    })
    .await;
    assert!(
        tail.is_ok(),
        "the stored trap never went out on the live tail; saw {:?}",
        seen.iter().map(|(topic, _)| topic).collect::<Vec<_>>()
    );
    settle().await;
    seen.extend(drain_all(events));
    seen
}

/// Every place a value could have got to, as text, each under its name.
async fn everywhere(
    app: &App,
    events: &[(String, Value)],
    logged: &LogLines,
) -> Vec<(String, String)> {
    let rpc = |method: &'static str, params: Value| async move {
        app.rpc(method, params)
            .await
            .unwrap_or_else(|err| panic!("{method}: {err}"))
            .to_string()
    };
    vec![
        ("the database, every table".into(), everything_stored(app)),
        (
            "the events on the bus".into(),
            events
                .iter()
                .map(|(topic, payload)| format!("{topic} {payload}\n"))
                .collect(),
        ),
        ("logs.status".into(), rpc("logs.status", Value::Null).await),
        (
            "logs.sources".into(),
            rpc("logs.sources", Value::Null).await,
        ),
        (
            "logs.query for traps".into(),
            rpc("logs.query", json!({"stream": "trap"})).await,
        ),
        (
            "logs.query for the audit trail".into(),
            rpc("logs.query", json!({"stream": "audit"})).await,
        ),
        (
            "the settings".into(),
            rpc("settings.get", json!({"section": "logs"})).await,
        ),
        ("the log lines".into(), logged.text()),
    ]
}

// ----------------------------------------------------------------- stored

#[tokio::test]
async fn a_trap_with_the_devices_community_is_stored_by_name_and_attributed() {
    let (lab, id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;

    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "received"), count(&status, "stored")),
        (1, 1),
        "{status:#}"
    );

    let stored = traps(app).await;
    assert_eq!(stored.len(), 1, "{stored:#?}");
    let trap = &stored[0];
    assert_eq!(
        trap["message"],
        json!("linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)"),
        "no monitor is in this build, so the interface is shown by its number"
    );
    assert_eq!(trap["stream"], json!("trap"));
    assert_eq!(trap["device_id"], json!(id));
    assert_eq!(trap["source"], json!("127.0.0.1"));
    assert_eq!(
        trap["severity"],
        Value::Null,
        "a trap has no syslog severity and is given none"
    );
    assert_eq!(
        trap["body"],
        json!({
            "version": "v2c",
            "trap": "linkDown",
            "oid": "1.3.6.1.6.3.1.1.5.3",
            "uptime": 123.45,
            "if_index": 3,
            "varbinds": [
                {"oid": "1.3.6.1.2.1.2.2.1.1.3", "name": "ifIndex.3", "value": "3"},
                {"oid": "1.3.6.1.2.1.2.2.1.7.3", "name": "ifAdminStatus.3", "value": "up(1)"},
                {"oid": "1.3.6.1.2.1.2.2.1.8.3", "name": "ifOperStatus.3", "value": "down(2)"},
            ],
        }),
        "the body is the notification field by field, and has no community in it"
    );

    // It is found the way a syslog line is: by its name, and by its device.
    let top = app
        .rpc(
            "logs.top",
            json!({"stream": "trap", "by": "mnemonic", "since": 0, "until": now_ms() + 1_000}),
        )
        .await
        .unwrap();
    assert_eq!(top[0]["key"], json!("linkDown"), "{top:#}");
    let found = query(
        app,
        json!({"stream": "trap", "text": "linkDown", "device_id": id}),
    )
    .await;
    assert_eq!(found.len(), 1);
    let sources = app.rpc("logs.sources", Value::Null).await.unwrap();
    assert_eq!(sources[0]["address"], json!("127.0.0.1"));
    assert_eq!(sources[0]["device_id"], json!(id));
}

#[tokio::test]
async fn a_v1_trap_is_stored_with_what_only_version_one_says() {
    let (lab, id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;

    send_datagrams("127.0.0.1", port, &[hex(V1_LINK_DOWN)]).await;
    wait_until_traps_settled(app, 1).await;
    let stored = traps(app).await;
    assert_eq!(stored.len(), 1, "{stored:#?}");
    let trap = &stored[0];
    assert_eq!(
        trap["message"],
        json!("linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)"),
        "generic trap 2 is linkDown, whatever version said so"
    );
    let body = &trap["body"];
    assert_eq!(body["version"], json!("v1"));
    assert_eq!(body["oid"], json!("1.3.6.1.6.3.1.1.5.3"));
    assert_eq!(body["enterprise"], json!("1.3.6.1.4.1.8072.3.2.10"));
    assert_eq!(
        (&body["generic"], &body["specific"]),
        (&json!(2), &json!(0))
    );
    assert_eq!(
        body["agent_addr"],
        json!("192.0.2.1"),
        "what the agent says its address is, kept as what it says"
    );
    assert_eq!(
        trap["device_id"],
        json!(id),
        "and it is filed under the address the datagram came from, not the one it claims"
    );
}

#[tokio::test]
async fn the_trap_rules_ship_with_the_messages_this_receiver_really_writes() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;

    // In the order the rules are checked below; the receiver stores them in
    // the order they arrive on one socket.
    let sent = [
        ("builtin.trap.link_down", V2C_LINK_DOWN),
        ("builtin.trap.cold_start", V2C_COLD_START),
        ("builtin.trap.warm_start", V2C_WARM_START),
        ("builtin.trap.auth_failure_burst", V2C_AUTH_FAILURE),
    ];
    let datagrams: Vec<Vec<u8>> = sent.iter().map(|(_, datagram)| hex(datagram)).collect();
    send_datagrams("127.0.0.1", port, &datagrams).await;
    wait_until_traps_settled(app, 4).await;
    let mut written: Vec<String> = traps(app)
        .await
        .iter()
        .map(|trap| trap["message"].as_str().unwrap().to_string())
        .collect();
    written.reverse();

    let rules = app.rpc("logs.rules.list", Value::Null).await.unwrap();
    let rules = rules.as_array().unwrap();
    let trap_rules: Vec<&Value> = rules
        .iter()
        .filter(|rule| rule["stream"] == json!("trap"))
        .collect();
    assert_eq!(
        trap_rules.len(),
        sent.len(),
        "every trap rule is checked here"
    );
    for ((rule_id, _), message) in sent.iter().zip(&written) {
        let rule = rules
            .iter()
            .find(|rule| rule["id"] == json!(rule_id))
            .unwrap_or_else(|| panic!("{rule_id} is a built-in rule"));
        assert_eq!(
            rule["test_line"],
            json!(message),
            "{rule_id} says its test line is what the receiver writes for net-snmp's datagram"
        );
        assert!(
            rule["test_line_source"]
                .as_str()
                .unwrap()
                .starts_with("Synthetic: "),
            "and does not pass it off as a device's: {rule:#}"
        );
        let result = app
            .rpc("logs.rules.test", json!({"rule": rule, "line": message}))
            .await
            .unwrap();
        assert_eq!(result["matched"], json!(true), "{result:#}");
    }
}

#[tokio::test]
async fn a_secret_in_a_binding_is_replaced_before_the_trap_is_stored() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;

    send_datagrams("127.0.0.1", port, &[hex(V2C_CONFIG_LINE)]).await;
    wait_until_traps_settled(app, 1).await;
    let stored = traps(app).await;
    assert_eq!(stored.len(), 1);
    let text = stored[0].to_string();
    assert!(!text.contains("Str0ngR0"), "{text}");
    assert!(text.contains("<secret:"), "{text}");
    let message = stored[0]["message"].as_str().unwrap();
    assert!(
        message.starts_with(
            "1.3.6.1.4.1.99999.0.7 1.3.6.1.4.1.99999.1.1.0=\"snmp-server community <secret:"
        ),
        "a notification without a name is shown by its OID: {message}"
    );
    assert!(message.ends_with(" sysName=edge-sw-7"), "{message}");
    assert_eq!(stored[0]["body"]["trap"], Value::Null);
}

// -------------------------------------------------------------- community

#[tokio::test]
async fn a_wrong_community_is_rejected_counted_and_appears_nowhere() {
    let (lab, id) = switch().await;
    let app = &lab.app;
    // From here on: the community was just typed into a credential, and
    // what the core does with that is not this test's subject.
    let logged = LogLines::capture();
    tracing::info!("the capture sees this thread");
    let mut events = app.subscribe();
    let port = start_traps(app).await;

    send_datagrams(
        "127.0.0.1",
        port,
        &[
            hex(V2C_LINK_DOWN_WRONG_COMMUNITY),
            hex(V2C_INFORM_WRONG_COMMUNITY),
            hex(V2C_LINK_DOWN),
        ],
    )
    .await;
    let status = wait_until_traps_settled(app, 3).await;
    assert_eq!(
        (
            count(&status, "received"),
            count(&status, "rejected"),
            count(&status, "stored"),
        ),
        (3, 2, 1),
        "{status:#}"
    );
    let rejections = status["traps"]["rejections"].as_array().unwrap();
    assert_eq!(rejections.len(), 1, "{rejections:#?}");
    assert_eq!(rejections[0]["address"], json!("127.0.0.1"));
    assert_eq!(rejections[0]["device_id"], json!(id));
    assert_eq!(rejections[0]["count"], json!(2));
    assert_eq!(
        rejections[0]["reason"],
        json!("the community is not the one stored for `SW1`"),
        "the reason says what is wrong without saying what was tried"
    );
    assert_eq!(
        traps(app).await.len(),
        1,
        "only the one with the right community"
    );

    // A receiver that was started is in the audit trail; so that is looked
    // through as well.
    let audit = query(app, json!({"stream": "audit", "limit": 50})).await;
    assert!(
        audit
            .iter()
            .any(|entry| entry["source"] == json!("logs.receiver.start")
                && entry["body"]["receiver"] == json!("traps")),
        "{audit:#?}"
    );

    let seen = events_through_the_tail(&mut events).await;
    for (place, text) in everywhere(app, &seen, &logged).await {
        for (what, secret) in [("wrong", WRONG), ("right", COMMUNITY)] {
            assert!(
                !text.contains(secret),
                "the {what} community is in {place}:\n{text}"
            );
        }
    }
    assert!(
        logged.text().contains("the capture sees this thread"),
        "the search of the log lines searched something"
    );
}

#[tokio::test]
async fn a_community_does_not_arrive_in_a_binding_either() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let logged = LogLines::capture();
    let mut events = app.subscribe();
    let port = start_traps(app).await;

    send_datagrams("127.0.0.1", port, &[hex(V2C_FORWARDED)]).await;
    wait_until_traps_settled(app, 1).await;
    let stored = traps(app).await;
    assert_eq!(stored.len(), 1, "{stored:#?}");
    assert_eq!(
        stored[0]["message"],
        json!(
            "linkDown ifIndex=3 snmpTrapAddress=192.0.2.44 snmpTrapCommunity=<community> \
             1.3.6.1.4.1.99999.1.9.0=\"sent with <community> by a proxy\""
        ),
        "what a proxy says the original community was is withheld, and so is the \
         datagram's own wherever a binding repeats it"
    );
    assert_eq!(
        stored[0]["body"]["varbinds"][2],
        json!({"oid": "1.3.6.1.6.3.18.1.4.0", "name": "snmpTrapCommunity.0", "value": "<community>"})
    );

    let seen = events_through_the_tail(&mut events).await;
    for (place, text) in everywhere(app, &seen, &logged).await {
        for secret in ["0riginal-c0mm", COMMUNITY] {
            assert!(!text.contains(secret), "`{secret}` is in {place}:\n{text}");
        }
    }
}

#[tokio::test]
async fn a_device_with_no_community_stored_has_its_traps_rejected_until_one_is() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    let port = start_traps(app).await;

    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (1, 0)
    );
    assert_eq!(
        status["traps"]["rejections"][0]["reason"],
        json!("the SNMP credential of `SW1` holds no SNMPv1/v2c community"),
        "the status says what to do about it"
    );

    // The community is stored a moment later: it counts at once, not when
    // a cache next expires.
    set_community(app, &lab.credential_id, COMMUNITY).await;
    settle().await;
    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    let status = wait_until_traps_settled(app, 2).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (1, 1)
    );

    // And taken away again: so does that.
    set_community(app, &lab.credential_id, "").await;
    settle().await;
    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    let status = wait_until_traps_settled(app, 3).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (2, 1)
    );
}

#[tokio::test]
async fn a_trap_from_an_address_no_device_has_is_rejected_while_the_check_is_on() {
    if !loopback_alias("127.0.0.2").await {
        eprintln!("skipped: this machine has no second loopback address (127.0.0.2) to send from");
        return;
    }
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;

    send_datagrams("127.0.0.2", port, &[hex(V2C_LINK_DOWN)]).await;
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (1, 0)
    );
    let rejection = &status["traps"]["rejections"][0];
    assert_eq!(rejection["address"], json!("127.0.0.2"));
    assert_eq!(rejection["device_id"], Value::Null);
    assert!(
        rejection["reason"]
            .as_str()
            .unwrap()
            .contains("no device in the inventory has the address 127.0.0.2"),
        "{rejection:#}"
    );
    assert!(traps(app).await.is_empty());
    assert_eq!(
        app.rpc("logs.sources", Value::Null).await.unwrap(),
        json!([]),
        "a rejected source is not written to the database: anybody can make one up"
    );
}

#[tokio::test]
async fn with_the_check_off_any_community_is_taken_from_a_known_device_and_still_kept_nowhere() {
    let (lab, id) = switch().await;
    let app = &lab.app;
    let logged = LogLines::capture();
    let mut events = app.subscribe();
    let port = start_traps_with(
        app,
        json!({"traps": {"require_known_community": false}, "unknown_sources": "drop"}),
    )
    .await;
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["traps"]["require_known_community"], json!(false));

    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN_WRONG_COMMUNITY)]).await;
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (0, 1)
    );
    let stored = traps(app).await;
    assert_eq!(stored[0]["device_id"], json!(id));

    // An unknown source follows `unknown_sources`, which says drop: not
    // stored, counted, listed — and an inform from it is not acknowledged,
    // because an acknowledgement says "kept".
    if loopback_alias("127.0.0.2").await {
        let stranger = send_datagrams("127.0.0.2", port, &[hex(V2C_INFORM_WRONG_COMMUNITY)]).await;
        let status = wait_until_traps_settled(app, 2).await;
        assert_eq!(
            (count(&status, "dropped"), count(&status, "stored")),
            (1, 1),
            "{status:#}"
        );
        assert_eq!(
            answer_on(&stranger, Duration::from_millis(300)).await,
            None,
            "an inform that was not kept is not acknowledged"
        );
        assert_eq!(count(&status, "acknowledged"), 0);
        let sources = app.rpc("logs.sources", Value::Null).await.unwrap();
        let dropped = sources
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["address"] == json!("127.0.0.2"))
            .expect("the address whose traps are dropped is listed");
        assert_eq!(dropped["dropped"], json!(1));

        // The same source with the setting that keeps: stored with no
        // device, and this time answered.
        app.rpc(
            "settings.set",
            json!({"section": "logs", "value": {
                "traps": {"enabled": true, "port": 0, "bind": "127.0.0.1", "require_known_community": false},
                "unknown_sources": "store",
            }}),
        )
        .await
        .unwrap();
        settle().await;
        assert_eq!(
            trap_port(&app.rpc("logs.status", Value::Null).await.unwrap()),
            port,
            "nothing about the socket changed, so it was not rebound"
        );
        let stranger = send_datagrams("127.0.0.2", port, &[hex(V2C_INFORM_LINK_UP)]).await;
        assert!(
            answer_on(&stranger, Duration::from_secs(2)).await.is_some(),
            "kept, so acknowledged"
        );
        let unattributed = query(app, json!({"stream": "trap", "text": "linkUp"})).await;
        assert_eq!(unattributed.len(), 1);
        assert_eq!(unattributed[0]["device_id"], Value::Null);
    } else {
        eprintln!("skipped in part: no second loopback address (127.0.0.2) to be a stranger from");
    }

    let seen = events_through_the_tail(&mut events).await;
    for (place, text) in everywhere(app, &seen, &logged).await {
        for secret in [WRONG, COMMUNITY] {
            assert!(!text.contains(secret), "`{secret}` is in {place}:\n{text}");
        }
    }
}

// ----------------------------------------------------------------- informs

#[tokio::test]
async fn an_inform_is_acknowledged_once_it_is_stored_and_a_repeat_is_stored_once() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;
    let inform = hex(V2C_INFORM_LINK_UP);

    let sender = send_datagrams("127.0.0.1", port, std::slice::from_ref(&inform)).await;
    let answer = answer_on(&sender, Duration::from_secs(2))
        .await
        .expect("the inform is acknowledged");
    assert_eq!(
        answer,
        response_to(&inform),
        "the Response echoes the request id and the bindings"
    );
    // The answer means "kept": by the time it is here, so is the row.
    let stored = traps(app).await;
    assert_eq!(stored.len(), 1, "{stored:#?}");
    assert_eq!(
        stored[0]["message"],
        json!("linkUp ifIndex=5 ifAdminStatus=up(1) ifOperStatus=up(1)")
    );
    assert_eq!(stored[0]["body"]["inform"], json!(true));
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "stored"), count(&status, "acknowledged")),
        (1, 1)
    );

    // The acknowledgement got lost, as far as the sender can tell: it sends
    // the same inform again. It is answered again and not stored again.
    sender.send_to(&inform, ("127.0.0.1", port)).await.unwrap();
    let again = answer_on(&sender, Duration::from_secs(2))
        .await
        .expect("a repeated inform is acknowledged again");
    assert_eq!(again, response_to(&inform));
    let status = wait_until_traps_settled(app, 2).await;
    assert_eq!(
        (
            count(&status, "stored"),
            count(&status, "duplicates"),
            count(&status, "acknowledged"),
        ),
        (1, 1, 2),
        "{status:#}"
    );
    assert_eq!(traps(app).await.len(), 1, "one event, one row");

    // An inform that is not kept is not acknowledged.
    let stranger = send_datagrams("127.0.0.1", port, &[hex(V2C_INFORM_WRONG_COMMUNITY)]).await;
    let status = wait_until_traps_settled(app, 3).await;
    assert_eq!(count(&status, "rejected"), 1);
    assert_eq!(
        answer_on(&stranger, Duration::from_millis(300)).await,
        None,
        "the sender of a wrong community learns nothing, not even that it was heard"
    );
    assert_eq!(count(&status, "acknowledged"), 2);
}

// ---------------------------------------------------------------------- v3

#[tokio::test]
async fn a_v3_notification_that_needs_authentication_is_counted_and_not_stored() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;

    send_datagrams(
        "127.0.0.1",
        port,
        &[
            hex(V3_AUTH_PRIV),
            hex(V3_AUTH_NO_PRIV),
            hex(V3_INFORM_DISCOVERY),
        ],
    )
    .await;
    let status = wait_until_traps_settled(app, 3).await;
    assert_eq!(
        (
            count(&status, "unauthenticated"),
            count(&status, "malformed"),
            count(&status, "stored"),
            count(&status, "rejected"),
        ),
        (2, 1, 0, 0),
        "the two notifications are counted as what they are, and the discovery \
         request as not a notification: {status:#}"
    );
    assert!(
        traps(app).await.is_empty(),
        "one of them can be read by anyone and says linkDown; reading it is not verifying it"
    );
    assert!(
        query(app, json!({"stream": "event", "limit": 10}))
            .await
            .is_empty(),
        "and no rule heard of it"
    );
}

#[tokio::test]
async fn a_v3_notification_without_authentication_is_kept_only_from_a_device_stored_that_way() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;

    // The switch is stored with an SNMPv2c community.
    send_datagrams("127.0.0.1", port, &[hex(V3_NO_AUTH)]).await;
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (1, 0)
    );
    assert_eq!(
        status["traps"]["rejections"][0]["reason"],
        json!(
            "`SW1` is stored with an SNMPv1/v2c community and this was SNMPv3 without authentication"
        )
    );

    // Stored as an SNMPv3 user without authentication, under another name.
    app.rpc(
        "credentials.update",
        json!({"id": lab.credential_id, "snmp_version": "v3", "snmp_user": "someone-else"}),
    )
    .await
    .unwrap();
    settle().await;
    send_datagrams("127.0.0.1", port, &[hex(V3_NO_AUTH)]).await;
    let status = wait_until_traps_settled(app, 2).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (2, 0)
    );

    // Under the name the notification carries.
    app.rpc(
        "credentials.update",
        json!({"id": lab.credential_id, "snmp_user": "trapuser"}),
    )
    .await
    .unwrap();
    settle().await;
    send_datagrams("127.0.0.1", port, &[hex(V3_NO_AUTH), hex(V2C_LINK_DOWN)]).await;
    let status = wait_until_traps_settled(app, 4).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (3, 1),
        "the v3 one is kept; the v2c one is now the one that does not belong: {status:#}"
    );
    let stored = traps(app).await;
    assert_eq!(stored[0]["body"]["version"], json!("v3"));
    assert_eq!(
        stored[0]["message"],
        json!("linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)")
    );

    // A user set up with authentication never sends one without.
    app.rpc(
        "credentials.update",
        json!({"id": lab.credential_id, "snmp_auth_protocol": "sha1", "snmp_auth_password": "authpass-12345"}),
    )
    .await
    .unwrap();
    settle().await;
    send_datagrams("127.0.0.1", port, &[hex(V3_NO_AUTH)]).await;
    let status = wait_until_traps_settled(app, 5).await;
    assert_eq!(
        (count(&status, "rejected"), count(&status, "stored")),
        (4, 1)
    );
}

// ------------------------------------------------------------------- names

#[tokio::test]
async fn an_interface_is_shown_by_name_when_the_monitor_has_a_map_of_the_device() {
    // What stands in for the monitoring feature: `monitor.ifindex` as its
    // specification describes it, for the one device it has polled.
    let polled = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let mut builder = nocturne_app::testing::builder();
    nocturne_logs::init(&mut builder);
    let known = polled.clone();
    builder
        .router
        .add("monitor.ifindex", move |_app: App, params: Value| {
            let known = known.clone();
            async move {
                let interfaces = if params["device_id"] == json!(*known.lock().unwrap()) {
                    json!([
                        {"index": 3, "name": "GigabitEthernet0/3"},
                        {"index": 4, "name": "GigabitEthernet0/4"},
                    ])
                } else {
                    json!([])
                };
                Ok(json!({"at": 1_763_000_000_000i64, "interfaces": interfaces}))
            }
        });
    let (lab, monitor) = {
        // The alert methods as well, to see the name reach an alert.
        let monitor = FakeMonitor::default();
        let raised = monitor.clone();
        builder
            .router
            .add("alerts.raise", move |app: App, params: Value| {
                let raised = raised.clone();
                async move {
                    raised
                        .raises
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    app.publish("test.alert_raised", &params);
                    Ok(json!({"id": "alert1"}))
                }
            });
        (
            nocturne_app::testing::lab(builder, &["SW1", "SW2"]).await,
            monitor,
        )
    };
    let app = &lab.app;
    let id = lab.id("SW1");
    *polled.lock().unwrap() = id.clone();
    set_address(app, &id, "127.0.0.1").await;
    set_community(app, &lab.credential_id, COMMUNITY).await;
    let mut events = app.subscribe();
    let port = start_traps(app).await;

    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    wait_until_traps_settled(app, 1).await;
    let stored = traps(app).await;
    assert_eq!(
        stored[0]["message"],
        json!("linkDown ifIndex=3 (GigabitEthernet0/3) ifAdminStatus=up(1) ifOperStatus=down(2)")
    );
    assert_eq!(stored[0]["body"]["interface"], json!("GigabitEthernet0/3"));
    assert_eq!(
        stored[0]["body"]["varbinds"][0],
        json!({"oid": "1.3.6.1.2.1.2.2.1.1.3", "name": "ifIndex.3", "value": "3",
               "interface": "GigabitEthernet0/3"})
    );

    // The alert says the name too, and is still keyed by the number: a map
    // that arrives between a down and an up must not make them two ports.
    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(
        raised["title"],
        json!("Link down: ifIndex 3 (GigabitEthernet0/3)")
    );
    assert_eq!(raised["instance"], json!("3"));
    assert_eq!(
        raised["dedup_key"],
        json!(format!("logs:builtin.trap.link_down:{id}:3"))
    );
    assert_eq!(monitor.raise_calls(), 1);

    // An interface the map does not have stays a number.
    send_datagrams("127.0.0.1", port, &[hex(V2C_INFORM_LINK_UP)]).await;
    wait_until_traps_settled(app, 2).await;
    let stored = traps(app).await;
    assert_eq!(
        stored[0]["message"],
        json!("linkUp ifIndex=5 ifAdminStatus=up(1) ifOperStatus=up(1)")
    );
    assert_eq!(stored[0]["body"].get("interface"), None);
}

// ------------------------------------------------------------------- rules

#[tokio::test]
async fn a_link_going_down_raises_an_alert_that_its_coming_up_clears() {
    let (lab, monitor, id) = switch_with_monitor().await;
    let app = &lab.app;
    let mut calls = app.subscribe();
    let mut signals = app.subscribe();
    let port = start_traps(app).await;

    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    let raised = next_event(&mut calls, "test.alert_raised").await;
    let key = format!("logs:builtin.trap.link_down:{id}:3");
    assert_eq!(raised["dedup_key"], json!(key));
    assert_eq!(raised["title"], json!("Link down: ifIndex 3"));
    assert_eq!(raised["severity"], json!("low"));
    assert_eq!(raised["instance"], json!("3"));
    assert_eq!(raised["device_id"], json!(id));

    // Up again a moment later. Inside the minute the alert was raised in it
    // is held, so that a link that bounces is one alert and not six.
    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_UP)]).await;
    wait_until_traps_settled(app, 2).await;
    settle().await;
    assert_eq!(monitor.open_keys(), vec![key.clone()]);

    // The minute passes (the wall clock is moved, not waited for).
    nocturne_logs::rules::sweep(app, now_ms() + 61_000);
    let cleared = next_event(&mut calls, "test.alert_cleared").await;
    assert_eq!(
        cleared["dedup_key"],
        json!(key),
        "cleared with the key it was raised with"
    );
    assert_eq!(cleared["cleared"], json!(true));
    assert!(monitor.open_keys().is_empty());

    let kinds: Vec<(Value, Value)> = of_rule(
        &drained(&mut signals, "logs.signal"),
        "builtin.trap.link_down",
    )
    .iter()
    .map(|signal| (signal["kind"].clone(), signal["reason"].clone()))
    .collect();
    assert_eq!(
        kinds,
        vec![
            (json!("alert"), Value::Null),
            (json!("cleared"), json!("opposite")),
        ]
    );
}

#[tokio::test]
async fn a_cold_start_and_a_warm_start_are_each_a_restart() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();
    let port = start_traps(app).await;

    send_datagrams(
        "127.0.0.1",
        port,
        &[hex(V2C_COLD_START), hex(V2C_WARM_START)],
    )
    .await;
    wait_until_traps_settled(app, 2).await;
    settle().await;
    let signals = drained(&mut events, "logs.signal");
    let said: Vec<(&str, &str)> = signals
        .iter()
        .map(|signal| {
            (
                signal["rule_id"].as_str().unwrap(),
                signal["kind"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        said,
        [
            ("builtin.trap.cold_start", "reload"),
            ("builtin.trap.warm_start", "reload"),
        ],
        "{signals:#?}"
    );
}

#[tokio::test]
async fn seven_authentication_failures_raise_nothing_and_the_eighth_raises_one_alert() {
    let (lab, monitor, id) = switch_with_monitor().await;
    let app = &lab.app;
    let mut events = app.subscribe();
    let port = start_traps(app).await;
    let failure = hex(V2C_AUTH_FAILURE);

    send_datagrams("127.0.0.1", port, &vec![failure.clone(); 7]).await;
    wait_until_traps_settled(app, 7).await;
    settle().await;
    assert_eq!(monitor.raise_calls(), 0, "seven in a minute is not a burst");
    assert_eq!(
        traps(app).await.len(),
        7,
        "every one of them is stored all the same"
    );

    send_datagrams("127.0.0.1", port, std::slice::from_ref(&failure)).await;
    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(
        raised["title"],
        json!("Repeated SNMP authentication failures")
    );
    assert_eq!(raised["severity"], json!("high"));
    assert_eq!(
        raised["dedup_key"],
        json!(format!("logs:builtin.trap.auth_failure_burst:{id}:"))
    );

    // It goes on: the alert is already open, and is not raised again for
    // every further eight inside the window.
    send_datagrams("127.0.0.1", port, &vec![failure; 16]).await;
    wait_until_traps_settled(app, 24).await;
    settle().await;
    assert_eq!(monitor.raise_calls(), 1);
}

// ---------------------------------------------------------------- receiver

#[tokio::test]
async fn the_trap_receiver_follows_its_settings_and_says_why_it_cannot_bind() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let logged = LogLines::capture();

    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["traps"]["enabled"], json!(false));
    assert_eq!(status["traps"]["listening"], json!([]));
    assert_eq!(
        status["traps"]["require_known_community"],
        json!(true),
        "the careful setting is the default"
    );
    assert!(
        status["traps"]["lab"]
            .as_str()
            .unwrap()
            .contains("no traps"),
        "the status says a lab contributes nothing here: {status:#}"
    );

    // Hold a port, then ask the receiver for it.
    let taken = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    let wanted = json!({"section": "logs", "value": {"traps": {"enabled": true, "port": port, "bind": "127.0.0.1"}}});
    app.rpc("settings.set", wanted.clone()).await.unwrap();
    let status = wait_for(app, "the receiver to report the clash", |status| {
        !status["traps"]["error"].is_null()
    })
    .await;
    let error = status["traps"]["error"].as_str().unwrap();
    assert!(
        error.contains(&format!("127.0.0.1:{port}")) && error.contains("already listening there"),
        "the error names the address and what is wrong: {error}"
    );
    assert_eq!(status["traps"]["enabled"], json!(true));
    assert_eq!(status["traps"]["listening"], json!([]));
    assert!(
        logged.text().contains("the trap receiver could not start"),
        "what the receiver's own tasks log is seen by the tests that search the log \
         lines for a community: {}",
        logged.text()
    );

    // Whatever held the port goes away; `logs.restart` is how a person says
    // "try again", with nothing in the settings changed.
    drop(taken);
    let status = app.rpc("logs.restart", Value::Null).await.unwrap();
    assert_eq!(status["traps"]["error"], Value::Null, "{status:#}");
    assert_eq!(
        status["traps"]["listening"],
        json!([format!("udp 127.0.0.1:{port}")])
    );

    // Switching the community check needs no new socket.
    let mut relaxed = wanted.clone();
    relaxed["value"]["traps"]["require_known_community"] = json!(false);
    app.rpc("settings.set", relaxed).await.unwrap();
    let status = wait_for(app, "the setting to reach the receiver", |status| {
        status["traps"]["require_known_community"] == json!(false)
    })
    .await;
    assert_eq!(
        status["traps"]["listening"],
        json!([format!("udp 127.0.0.1:{port}")])
    );

    // Switched off, the port is closed: something else can have it.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"traps": {"enabled": false, "port": port, "bind": "127.0.0.1"}}}),
    )
    .await
    .unwrap();
    wait_for(app, "the receiver to stop", |status| {
        status["traps"]["enabled"] == json!(false) && status["traps"]["listening"] == json!([])
    })
    .await;
    tokio::net::UdpSocket::bind(("127.0.0.1", port))
        .await
        .expect("the port is free again once the receiver is off");

    // An address this machine does not have is refused with advice.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"traps": {"enabled": true, "bind": "not-an-address"}}}),
    )
    .await
    .unwrap();
    let status = wait_for(app, "the receiver to refuse the address", |status| {
        status["traps"]["error"]
            .as_str()
            .is_some_and(|error| error.contains("not-an-address"))
    })
    .await;
    assert!(
        status["traps"]["error"]
            .as_str()
            .unwrap()
            .contains("0.0.0.0"),
        "{status:#}"
    );
}

#[tokio::test]
async fn a_storm_of_traps_is_dropped_and_counted_rather_than_queued_without_limit() {
    // The writer is held up on its first batch, as a slow disk would hold
    // it: asking for the interface names of the device takes a while.
    let mut builder = nocturne_app::testing::builder();
    nocturne_logs::init(&mut builder);
    builder
        .router
        .add("monitor.ifindex", |_app: App, _params: Value| async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            Ok(json!({"interfaces": []}))
        });
    let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    set_community(app, &lab.credential_id, COMMUNITY).await;
    const LIMIT: u64 = 100;
    let port = start_traps_with(app, json!({"queue_limit": LIMIT})).await;

    let datagram = hex(V2C_LINK_DOWN);
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut most_queued = 0;
    for index in 0..4_000u32 {
        socket
            .send_to(&datagram, ("127.0.0.1", port))
            .await
            .unwrap();
        // Let the receiver run: on a test's single thread it otherwise
        // only gets a turn when the kernel's buffer has overflowed.
        if index % 20 == 19 {
            tokio::task::yield_now().await;
        }
        if index % 500 == 499 {
            let status = app.rpc("logs.status", Value::Null).await.unwrap();
            most_queued = most_queued.max(count(&status, "queued"));
        }
    }

    // Settled: nothing more arrives, and everything that did is accounted
    // for. How many arrived is the kernel's business (it drops what does
    // not fit its buffer, and nobody is told).
    let mut last = 0;
    let status = loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = app.rpc("logs.status", Value::Null).await.unwrap();
        most_queued = most_queued.max(count(&status, "queued"));
        let received = count(&status, "received");
        if received == last && traps_accounted(&status) == received && count(&status, "queued") == 0
        {
            break status;
        }
        last = received;
    };
    let (received, stored, dropped) = (
        count(&status, "received"),
        count(&status, "stored"),
        count(&status, "dropped"),
    );
    assert!(
        received > 1_000,
        "the storm reached the receiver: {status:#}"
    );
    assert!(
        most_queued <= LIMIT,
        "the queue never held more than its limit: {most_queued}"
    );
    assert!(
        dropped > 0,
        "with the writer held up, what did not fit was dropped: {status:#}"
    );
    assert!(stored > 0, "{status:#}");
    assert_eq!(
        stored + dropped,
        received,
        "every datagram is either stored or counted as dropped: {status:#}"
    );
    let rows: i64 = app
        .store()
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM logs WHERE stream = 'trap'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(rows as u64, stored, "and `stored` is what is in the store");

    // Four thousand link-downs of one port are one alert's worth of signal,
    // not four thousand.
    let signals = query(app, json!({"stream": "event", "limit": 200})).await;
    assert_eq!(signals.len(), 1, "{signals:#?}");

    // It is still listening.
    send_datagrams("127.0.0.1", port, &[hex(V2C_COLD_START)]).await;
    let after = wait_for(app, "a trap after the storm to be stored", |status| {
        count(status, "stored") == stored + 1
    })
    .await;
    assert_eq!(count(&after, "dropped"), dropped);
}

#[tokio::test]
async fn a_datagram_of_thousands_of_bindings_is_one_bounded_row_and_says_how_many_there_were() {
    // Written by hand (BER, RFC 3416): six thousand bindings of a null
    // each, which is what fits in a datagram and nothing a tool will send.
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;
    let binds: Vec<Vec<u8>> = (0..6_000u32)
        .map(|index| ber::varbind(&[1, 3, index], ber::null()))
        .collect();
    let stuffed = ber::v2c_trap(COMMUNITY, 77, &[1, 3, 6, 1, 4, 1, 99999, 0, 7], &binds);
    assert!(stuffed.len() < 65_000, "{}", stuffed.len());
    // What the receiver would hold for it, had it kept them all.
    let decoded = nocturne_snmp::decode_trap(&stuffed, "127.0.0.1:1".parse().unwrap()).unwrap();
    assert_eq!(decoded.varbinds.len(), 6_000);

    send_datagrams("127.0.0.1", port, &[stuffed, hex(V2C_LINK_DOWN)]).await;
    let status = wait_until_traps_settled(app, 2).await;
    assert_eq!(count(&status, "stored"), 2, "{status:#}");
    let stored = traps(app).await;
    assert_eq!(
        stored[0]["message"],
        json!("linkDown ifIndex=3 ifAdminStatus=up(1) ifOperStatus=down(2)"),
        "the receiver is as it was for the trap behind it"
    );
    let row = &stored[1];
    let message = row["message"].as_str().unwrap();
    assert!(
        message.starts_with("1.3.6.1.4.1.99999.0.7 1.3.0=null 1.3.1=null "),
        "{message}"
    );
    assert!(message.ends_with(" (+5988 more)"), "{message}");
    let kept = row["body"]["varbinds"].as_array().unwrap();
    assert_eq!(kept.len(), 64);
    assert_eq!(kept[63]["oid"], json!("1.3.63"));
    assert_eq!(
        row["body"]["varbinds_omitted"],
        json!(6_000 - 64),
        "what is not in the row is counted against what the datagram held"
    );
    assert!(row["body"].to_string().len() < 8_000);
}

#[tokio::test]
async fn a_trap_the_database_would_not_take_is_counted_as_dropped_and_the_next_is_stored() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;
    // The index table is gone, as on a database that is damaged or full:
    // the write of the row fails.
    let rename = |from: &'static str, to: &'static str| {
        app.store()
            .with(|conn| conn.execute_batch(&format!("ALTER TABLE {from} RENAME TO {to}")))
            .expect("the table is renamed")
    };
    rename("logs_syslog", "logs_syslog_away");
    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "stored"), count(&status, "dropped")),
        (0, 1),
        "received, not kept, and counted as that: {status:#}"
    );
    assert!(traps(app).await.is_empty());

    rename("logs_syslog_away", "logs_syslog");
    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_UP)]).await;
    let status = wait_until_traps_settled(app, 2).await;
    assert_eq!(
        (
            count(&status, "received"),
            count(&status, "stored"),
            count(&status, "dropped")
        ),
        (2, 1, 1),
        "{status:#}"
    );
}

#[tokio::test]
async fn switching_the_receiver_off_does_not_throw_away_what_it_had_received() {
    // The writer is slow over its first trap, so the ones behind it are
    // still waiting when the receiver is switched off.
    let mut builder = nocturne_app::testing::builder();
    nocturne_logs::init(&mut builder);
    builder
        .router
        .add("monitor.ifindex", |_app: App, _params: Value| async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            Ok(json!({"interfaces": []}))
        });
    let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    set_community(app, &lab.credential_id, COMMUNITY).await;
    let port = start_traps(app).await;

    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    wait_for(app, "the first trap to reach the writer", |status| {
        count(status, "received") == 1 && count(status, "queued") == 0
    })
    .await;
    send_datagrams(
        "127.0.0.1",
        port,
        &[
            hex(V2C_COLD_START),
            hex(V2C_WARM_START),
            hex(V2C_AUTH_FAILURE),
        ],
    )
    .await;
    wait_for(app, "the rest to be taken off the socket", |status| {
        count(status, "received") == 4
    })
    .await;
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"traps": {"enabled": false}}}),
    )
    .await
    .unwrap();

    let status = wait_for(app, "everything received to be accounted for", |status| {
        status["traps"]["enabled"] == json!(false) && traps_accounted(status) == 4
    })
    .await;
    assert_eq!(
        (count(&status, "stored"), count(&status, "dropped")),
        (4, 0),
        "received before it was switched off, so written: {status:#}"
    );
    assert_eq!(traps(app).await.len(), 4);
}

#[tokio::test]
async fn a_flood_of_forged_traps_is_turned_away_before_the_queue() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    const LIMIT: u64 = 100;
    let port = start_traps_with(app, json!({"queue_limit": LIMIT})).await;

    let forged = hex(V2C_LINK_DOWN_WRONG_COMMUNITY);
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    for index in 0..3_000u32 {
        socket.send_to(&forged, ("127.0.0.1", port)).await.unwrap();
        if index % 20 == 19 {
            tokio::task::yield_now().await;
        }
    }
    // The real one arrives in the middle of it and is not crowded out.
    send_datagrams("127.0.0.1", port, &[hex(V2C_LINK_DOWN)]).await;
    let status = wait_for(app, "the real trap to be stored", |status| {
        count(status, "stored") == 1 && traps_accounted(status) == count(status, "received")
    })
    .await;
    assert!(count(&status, "rejected") > 1_000, "{status:#}");
    assert_eq!(
        count(&status, "dropped"),
        0,
        "a forged trap never takes a place in the queue: {status:#}"
    );
    assert_eq!(count(&status, "rejected") + 1, count(&status, "received"));
    let rejections = status["traps"]["rejections"].as_array().unwrap();
    assert_eq!(
        rejections.len(),
        1,
        "one address, one row, however many it sent"
    );
    assert_eq!(rejections[0]["count"], json!(count(&status, "rejected")));
    assert_eq!(traps(app).await.len(), 1);
}

// ------------------------------------------------------------ device setup

#[tokio::test]
async fn device_setup_gives_the_trap_lines_and_leaves_the_community_out() {
    let (lab, id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;

    let setup = app
        .rpc("logs.device_setup", json!({"device_id": id}))
        .await
        .unwrap();
    assert_eq!(
        setup["trap_lines"],
        json!([
            "snmp-server enable traps snmp authentication linkdown linkup coldstart warmstart",
            format!("snmp-server host 127.0.0.1 version 2c <snmp-community> udp-port {port}"),
        ]),
        "the port the receiver is really listening on, and a placeholder for the secret"
    );
    assert_eq!(setup["trap_port"], json!(port));
    assert!(
        !setup.to_string().contains(COMMUNITY),
        "the community is in the vault and is not returned: {setup:#}"
    );
    let note = setup["trap_note"].as_str().unwrap();
    for said in [
        "<snmp-community>",
        "changes.create",
        "exactly the community stored",
        "forge",
        "below 1024",
    ] {
        assert!(
            note.contains(said),
            "the note does not say `{said}`: {note}"
        );
    }
    assert!(
        lab.fake("SW1")
            .with(|device| device.seen.clone())
            .is_empty(),
        "device_setup does not touch the device"
    );

    // RouterOS cannot be told a port, and this router has no credential.
    let router = app
        .rpc(
            "devices.create",
            json!({"name": "RB1", "address": "127.0.0.3", "profile": "mikrotik_routeros"}),
        )
        .await
        .unwrap();
    let setup = app
        .rpc(
            "logs.device_setup",
            json!({"device_id": router["id"].as_str().unwrap()}),
        )
        .await
        .unwrap();
    assert_eq!(
        setup["trap_lines"],
        json!([format!(
            "/snmp set trap-version=2 trap-community=<snmp-community> trap-target={} \
             trap-generators=interfaces,start-trap trap-interfaces=all",
            setup["address"].as_str().unwrap()
        )])
    );
    let note = setup["trap_note"].as_str().unwrap();
    assert!(
        note.contains("UDP 162") && note.contains(&format!("listens on {port}")),
        "a platform that only sends to 162 is said to: {note}"
    );
    assert!(
        note.contains("would be rejected") && note.contains("`RB1` has no credential"),
        "and so is a device whose traps would not be kept: {note}"
    );
}

// --------------------------------------------------------------- retention

#[tokio::test]
async fn traps_age_out_and_are_cleared_like_the_lines_beside_them() {
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let port = start_traps(app).await;
    send_datagrams(
        "127.0.0.1",
        port,
        &[hex(V2C_LINK_DOWN), hex(V2C_COLD_START)],
    )
    .await;
    wait_until_traps_settled(app, 2).await;
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(
        status["store"]["rows"],
        json!(2),
        "what devices sent is counted together, traps included"
    );

    // One of them is from last year.
    app.store()
        .with(|conn| {
            conn.execute(
                "UPDATE logs SET ts = ts - 400 * 86400000 WHERE stream = 'trap' AND message LIKE 'coldStart%'",
                [],
            )
        })
        .unwrap();
    let pruned = nocturne_logs::ingest::prune_once(app).unwrap();
    assert_eq!((pruned.by_age, pruned.by_rows), (1, 0));
    let left = traps(app).await;
    assert_eq!(left.len(), 1);
    assert!(left[0]["message"].as_str().unwrap().starts_with("linkDown"));

    let cleared = app
        .rpc("logs.clear", json!({"stream": "trap"}))
        .await
        .unwrap();
    assert_eq!(cleared, json!({"removed": 1, "stream": "trap"}));
    assert!(traps(app).await.is_empty());
    let orphans: i64 = app
        .store()
        .with(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM logs_syslog WHERE log_id NOT IN (SELECT id FROM logs)",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(orphans, 0, "the index rows went with them");
}

// ----------------------------------------------------- net-snmp, when there

const LINK_DOWN: &str = "1.3.6.1.6.3.1.1.5.3";
const LINK_UP: &str = "1.3.6.1.6.3.1.1.5.4";

/// The bindings of a link trap for ifIndex 7, as `snmptrap` arguments.
fn link_bindings(oper_status: &str) -> Vec<String> {
    [
        "1.3.6.1.2.1.2.2.1.1.7",
        "i",
        "7",
        "1.3.6.1.2.1.2.2.1.7.7",
        "i",
        "1",
        "1.3.6.1.2.1.2.2.1.8.7",
        "i",
        oper_status,
    ]
    .iter()
    .map(|word| word.to_string())
    .collect()
}

fn arguments(words: &[&str], rest: Vec<String>) -> Vec<String> {
    words
        .iter()
        .map(|word| word.to_string())
        .chain(rest)
        .collect()
}

#[tokio::test]
async fn net_snmp_traps_of_both_versions_are_kept_as_sent() {
    let Some(snmptrap) = net_snmp("snmptrap") else {
        eprintln!("skipped: net-snmp's `snmptrap` is not installed on this machine");
        return;
    };
    let (lab, id) = switch().await;
    let app = &lab.app;
    let target = format!("127.0.0.1:{}", start_traps(app).await);

    assert!(
        run_net_snmp(
            &snmptrap,
            &arguments(
                &["-v", "2c", "-c", COMMUNITY, &target, "4711", LINK_DOWN],
                link_bindings("2"),
            ),
        )
        .await
    );
    assert!(
        run_net_snmp(
            &snmptrap,
            &arguments(
                &[
                    "-v",
                    "1",
                    "-c",
                    COMMUNITY,
                    &target,
                    "1.3.6.1.4.1.8072.3.2.10",
                    "192.0.2.9",
                    "3",
                    "0",
                    "4712"
                ],
                link_bindings("1"),
            ),
        )
        .await
    );
    assert!(
        run_net_snmp(
            &snmptrap,
            &arguments(
                &["-v", "2c", "-c", WRONG, &target, "4713", LINK_DOWN],
                link_bindings("2"),
            ),
        )
        .await,
        "a trap gets no answer, so its sender cannot tell that it was rejected"
    );
    let status = wait_until_traps_settled(app, 3).await;
    assert_eq!(
        (count(&status, "stored"), count(&status, "rejected")),
        (2, 1),
        "{status:#}"
    );
    let stored = traps(app).await;
    assert_eq!(
        stored[1]["message"],
        json!("linkDown ifIndex=7 ifAdminStatus=up(1) ifOperStatus=down(2)")
    );
    assert_eq!(stored[1]["body"]["uptime"], json!(47.11));
    assert_eq!(
        stored[0]["message"],
        json!("linkUp ifIndex=7 ifAdminStatus=up(1) ifOperStatus=up(1)"),
        "SNMPv1's generic trap 3 is linkUp"
    );
    assert_eq!(stored[0]["body"]["version"], json!("v1"));
    assert_eq!(stored[0]["body"]["agent_addr"], json!("192.0.2.9"));
    assert!(stored.iter().all(|trap| trap["device_id"] == json!(id)));
}

#[tokio::test]
async fn net_snmp_has_its_inform_acknowledged_and_gives_up_on_one_with_the_wrong_community() {
    let Some(snmpinform) = net_snmp("snmpinform") else {
        eprintln!("skipped: net-snmp's `snmpinform` is not installed on this machine");
        return;
    };
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let target = format!("127.0.0.1:{}", start_traps(app).await);

    // `snmpinform` exits successfully only when the Response arrives and is
    // the answer to what it sent.
    assert!(
        run_net_snmp(
            &snmpinform,
            &arguments(
                &[
                    "-v", "2c", "-c", COMMUNITY, "-r", "0", "-t", "5", &target, "999", LINK_UP
                ],
                link_bindings("1"),
            ),
        )
        .await,
        "net-snmp did not accept the acknowledgement"
    );
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "stored"), count(&status, "acknowledged")),
        (1, 1)
    );
    assert_eq!(traps(app).await[0]["body"]["inform"], json!(true));

    // With the wrong community nothing comes back, and it times out.
    assert!(
        !run_net_snmp(
            &snmpinform,
            &arguments(
                &[
                    "-v", "2c", "-c", WRONG, "-r", "0", "-t", "1", &target, "999", LINK_UP
                ],
                link_bindings("1"),
            ),
        )
        .await,
        "an inform with the wrong community was acknowledged"
    );
    let status = wait_until_traps_settled(app, 2).await;
    assert_eq!(
        (
            count(&status, "stored"),
            count(&status, "rejected"),
            count(&status, "acknowledged"),
        ),
        (1, 1, 1)
    );
}

#[tokio::test]
async fn net_snmp_v3_authpriv_notification_is_counted_and_not_stored() {
    let Some(snmptrap) = net_snmp("snmptrap") else {
        eprintln!("skipped: net-snmp's `snmptrap` is not installed on this machine");
        return;
    };
    let (lab, _id) = switch().await;
    let app = &lab.app;
    let target = format!("127.0.0.1:{}", start_traps(app).await);

    assert!(
        run_net_snmp(
            &snmptrap,
            &arguments(
                &[
                    "-v",
                    "3",
                    "-e",
                    "0x80001f8880e5d1a04e2b3c6d5f",
                    "-u",
                    "trapuser",
                    "-a",
                    "SHA",
                    "-A",
                    "authpass-12345",
                    "-x",
                    "AES",
                    "-X",
                    "privpass-12345",
                    "-l",
                    "authPriv",
                    &target,
                    "4242",
                    LINK_DOWN,
                ],
                link_bindings("2"),
            ),
        )
        .await
    );
    let status = wait_until_traps_settled(app, 1).await;
    assert_eq!(
        (count(&status, "unauthenticated"), count(&status, "stored")),
        (1, 0),
        "{status:#}"
    );
    assert!(traps(app).await.is_empty());
}
