//! The flow collector, through `App::rpc` and a real UDP socket on port 0.
//!
//! Two kinds of datagram are sent here, and each test says which:
//!
//! * **captured** — what softflowd 1.1.0 and nfdump's nfreplay 1.7.3 sent
//!   when they were run, written down as hex (the engine's
//!   `flow/tests/captures.rs`, which says how). They are replayed, so these
//!   tests run where neither tool is installed. One test runs softflowd
//!   itself where it is, and skips with a reason where it is not.
//! * **written by hand** — built field by field from Cisco's NetFlow v5
//!   format and RFC 3954, for what softflowd cannot be made to send:
//!   interface numbers, data before its template, a chosen sampling rate.
//!
//! None of it is from a router or a switch.

mod support;

// The engine's own record of what real exporters sent, and of the traffic
// they were given: one copy, read by both crates' tests, so the feature is
// tested against exactly the datagrams the decoder is.
#[allow(dead_code)]
#[path = "../../nocturne-netprobe/src/flow/tests/captures.rs"]
mod captures;
#[allow(dead_code)]
#[path = "../../nocturne-netprobe/src/flow/tests/traffic.rs"]
mod traffic;

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::time::Duration;

use nocturne_app::App;
use nocturne_netprobe::flow::{FlowBatch, FlowRecord};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;

// ------------------------------------------------------- datagrams by hand

/// One record of a NetFlow v5 datagram written by hand.
#[derive(Clone, Copy)]
struct V5 {
    src: [u8; 4],
    dst: [u8; 4],
    input: u16,
    output: u16,
    packets: u32,
    bytes: u32,
    src_port: u16,
    dst_port: u16,
    protocol: u8,
    tos: u8,
}

impl V5 {
    /// A TCP flow to port 443 of 192.0.2.80.
    fn https(src: [u8; 4], bytes: u32, packets: u32) -> V5 {
        V5 {
            src,
            dst: [192, 0, 2, 80],
            input: 3,
            output: 5,
            packets,
            bytes,
            src_port: 51_000,
            dst_port: 443,
            protocol: 6,
            tos: 0,
        }
    }
}

/// A NetFlow v5 export datagram, written by hand to Cisco's format: a
/// 24-byte header and 48 bytes per record. `sampling` is the header field
/// (two bits of mode, fourteen of interval).
fn v5(sampling: u16, records: &[V5]) -> Vec<u8> {
    let mut datagram = Vec::new();
    datagram.extend_from_slice(&5u16.to_be_bytes());
    datagram.extend_from_slice(&(records.len() as u16).to_be_bytes());
    datagram.extend_from_slice(&86_400_000u32.to_be_bytes()); // SysUptime
    datagram.extend_from_slice(&1_763_000_000u32.to_be_bytes()); // unix_secs
    datagram.extend_from_slice(&[0; 4]); // unix_nsecs
    datagram.extend_from_slice(&1u32.to_be_bytes()); // flow_sequence
    datagram.extend_from_slice(&[0, 0]); // engine type and id
    datagram.extend_from_slice(&sampling.to_be_bytes());
    for record in records {
        datagram.extend_from_slice(&record.src);
        datagram.extend_from_slice(&record.dst);
        datagram.extend_from_slice(&[0; 4]); // next hop
        datagram.extend_from_slice(&record.input.to_be_bytes());
        datagram.extend_from_slice(&record.output.to_be_bytes());
        datagram.extend_from_slice(&record.packets.to_be_bytes());
        datagram.extend_from_slice(&record.bytes.to_be_bytes());
        datagram.extend_from_slice(&80_000u32.to_be_bytes()); // First
        datagram.extend_from_slice(&81_000u32.to_be_bytes()); // Last
        datagram.extend_from_slice(&record.src_port.to_be_bytes());
        datagram.extend_from_slice(&record.dst_port.to_be_bytes());
        datagram.extend_from_slice(&[0, 0x18, record.protocol, record.tos]); // pad, flags
        datagram.extend_from_slice(&[0; 8]); // AS numbers, masks, pad
    }
    datagram
}

/// A NetFlow v9 datagram of the given flow sets, written by hand to RFC 3954.
fn v9(sets: &[Vec<u8>]) -> Vec<u8> {
    let mut datagram = Vec::new();
    datagram.extend_from_slice(&9u16.to_be_bytes());
    datagram.extend_from_slice(&(sets.len() as u16).to_be_bytes());
    datagram.extend_from_slice(&86_400_000u32.to_be_bytes()); // sysUptime
    datagram.extend_from_slice(&1_763_000_000u32.to_be_bytes()); // export time
    datagram.extend_from_slice(&1u32.to_be_bytes()); // sequence
    datagram.extend_from_slice(&7u32.to_be_bytes()); // source id
    for set in sets {
        datagram.extend_from_slice(set);
    }
    datagram
}

fn v9_set(id: u16, content: &[u8]) -> Vec<u8> {
    let padding = (4 - content.len() % 4) % 4;
    let mut set = Vec::new();
    set.extend_from_slice(&id.to_be_bytes());
    set.extend_from_slice(&((4 + content.len() + padding) as u16).to_be_bytes());
    set.extend_from_slice(content);
    set.extend_from_slice(&vec![0; padding]);
    set
}

/// Template 260: source and destination address, bytes, packets.
fn v9_template() -> Vec<u8> {
    let mut content = Vec::new();
    for field in [260u16, 4, 8, 4, 12, 4, 1, 4, 2, 4] {
        content.extend_from_slice(&field.to_be_bytes());
    }
    v9_set(0, &content)
}

/// Options template 257 (RFC 3954 section 6.1): its scope is the system
/// (type 1, four bytes), its one option `samplingInterval` (34, four bytes).
fn v9_options_template() -> Vec<u8> {
    let mut content = Vec::new();
    for field in [257u16, 4, 4, 1, 4, 34, 4] {
        content.extend_from_slice(&field.to_be_bytes());
    }
    v9_set(1, &content)
}

/// The one record of template 257: this exporter looks at one packet in `rate`.
fn v9_sampling(rate: u32) -> Vec<u8> {
    let mut content = vec![0; 4];
    content.extend_from_slice(&rate.to_be_bytes());
    v9_set(257, &content)
}

/// One record of template 260: 10.0.0.1 → 192.0.2.80, 4,000 bytes in 4 packets.
fn v9_data() -> Vec<u8> {
    let mut content = vec![10, 0, 0, 1, 192, 0, 2, 80];
    content.extend_from_slice(&4_000u32.to_be_bytes());
    content.extend_from_slice(&4u32.to_be_bytes());
    v9_set(260, &content)
}

// ------------------------------------------------------------------ helpers

async fn top(app: &App, params: Value) -> Value {
    app.rpc("flows.top", params)
        .await
        .expect("flows.top answers")
}

/// The rows of an answer as (key, label, bytes, packets, flows).
fn rows(answer: &Value) -> Vec<(String, String, u64, u64, u64)> {
    answer["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| {
            (
                row["key"].as_str().expect("key").to_string(),
                row["label"].as_str().expect("label").to_string(),
                row["bytes"].as_u64().expect("bytes"),
                row["packets"].as_u64().expect("packets"),
                row["flows"].as_u64().expect("flows"),
            )
        })
        .collect()
}

fn row(
    key: &str,
    label: &str,
    bytes: u64,
    packets: u64,
    flows: u64,
) -> (String, String, u64, u64, u64) {
    (key.to_string(), label.to_string(), bytes, packets, flows)
}

fn flows_of(status: &Value, key: &str) -> u64 {
    status["flows"][key]
        .as_u64()
        .unwrap_or_else(|| panic!("flows.{key} in {status:#}"))
}

/// One switch at 127.0.0.1 with the collector listening. Returns the app's
/// lab, the switch's id and the port.
async fn exporting_switch() -> (nocturne_app::testing::Lab, String, u16) {
    let lab = lab(&["SW1"]).await;
    let id = lab.id("SW1");
    set_address(&lab.app, &id, "127.0.0.1").await;
    let port = start_flows(&lab.app).await;
    (lab, id, port)
}

// -------------------------------------------------------------------- tests

#[tokio::test]
async fn the_collector_is_off_until_someone_turns_it_on_and_an_empty_answer_says_so() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;

    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(
        status["flows"],
        json!({
            "enabled": false,
            "implemented": true,
            "listening": [],
            "error": null,
            "received": 0,
            "malformed": 0,
            "records": 0,
            "counted": 0,
            "dropped": 0,
            "drops": {"queue_full": 0, "no_room": 0, "unclaimed": 0, "unreadable": 0},
            "queued": 0,
            "queued_bytes": 0,
            "max_queued_bytes": 67_108_864,
            "rate_per_minute": 0,
            "last_at": null,
            "sets_without_template": 0,
            "exporters": 0,
            "unattributed_exporters": 0,
            "memory": {"minutes": 0, "keys": 0, "max_keys": 2_000_000, "evicted_minutes": 0, "oldest_at": null},
            "lab": "A simulated device exports no flows, so nothing counted here came from a lab.",
        })
    );

    // Nothing is not the same as no traffic: the answer says the collector
    // is not collecting and that the window holds nothing at all.
    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(answer["rows"], json!([]));
    assert_eq!(
        answer["total"],
        json!({"bytes": 0, "packets": 0, "flows": 0})
    );
    assert_eq!(answer["coverage"]["collecting"], json!(false));
    assert_eq!(answer["coverage"]["since"], Value::Null);
    assert_eq!(answer["coverage"]["exporters"], json!(0));
    assert_eq!(
        app.rpc("flows.exporters", Value::Null).await.unwrap(),
        json!([])
    );
}

#[tokio::test]
async fn what_softflowd_exported_is_the_top_talkers_by_every_key() {
    // Captured: softflowd's NetFlow v9 export of the five conversations.
    let (lab, id, port) = exporting_switch().await;
    let app = &lab.app;
    send_datagrams("127.0.0.1", port, &[hex(captures::SOFTFLOWD_V9)]).await;
    let status = wait_until_flows_settled(app, 7).await;
    assert_eq!(
        (
            flows_of(&status, "received"),
            flows_of(&status, "records"),
            flows_of(&status, "counted"),
            flows_of(&status, "dropped"),
            flows_of(&status, "exporters"),
            flows_of(&status, "unattributed_exporters"),
        ),
        (1, 7, 7, 0, 1, 0)
    );
    assert_eq!(status["flows"]["memory"]["minutes"], json!(1));

    let by = |key: &'static str| async move { top(app, json!({"by": key})).await };

    assert_eq!(
        rows(&by("source").await),
        vec![
            row("192.0.2.80", "192.0.2.80", 24_880, 22, 1),
            row("10.0.0.7", "10.0.0.7", 9_400, 50, 1),
            // The download's acknowledgements, a DNS question and three pings.
            row("10.0.0.5", "10.0.0.5", 880 + 58 + 180, 22 + 1 + 3, 3),
            row("2001:db8::5", "2001:db8::5", 860, 6, 1),
            row("10.0.0.53", "10.0.0.53", 118, 1, 1),
        ]
    );
    assert_eq!(
        rows(&by("destination").await),
        vec![
            row("10.0.0.5", "10.0.0.5", 24_880 + 118, 23, 2),
            row("10.0.0.9", "10.0.0.9", 9_400, 50, 1),
            row("192.0.2.80", "192.0.2.80", 880, 22, 1),
            row("2001:db8::22", "2001:db8::22", 860, 6, 1),
            row("10.0.0.1", "10.0.0.1", 180, 3, 1),
            row("10.0.0.53", "10.0.0.53", 58, 1, 1),
        ]
    );
    assert_eq!(
        rows(&by("conversation").await),
        vec![
            row(
                "10.0.0.5 <-> 192.0.2.80",
                "10.0.0.5 <-> 192.0.2.80",
                25_760,
                44,
                2
            ),
            row(
                "10.0.0.7 <-> 10.0.0.9",
                "10.0.0.7 <-> 10.0.0.9",
                9_400,
                50,
                1
            ),
            row(
                "2001:db8::5 <-> 2001:db8::22",
                "2001:db8::5 <-> 2001:db8::22",
                860,
                6,
                1
            ),
            row("10.0.0.1 <-> 10.0.0.5", "10.0.0.1 <-> 10.0.0.5", 180, 3, 1),
            row(
                "10.0.0.5 <-> 10.0.0.53",
                "10.0.0.5 <-> 10.0.0.53",
                176,
                2,
                2
            ),
        ]
    );
    assert_eq!(
        rows(&by("application").await),
        vec![
            row("tcp/443", "tcp/443 (https)", 25_760, 44, 2),
            row("udp/16384", "udp/16384 (rtp)", 9_400, 50, 1),
            row("tcp/22", "tcp/22 (ssh)", 860, 6, 1),
            row("ip/1", "ICMP (IP protocol 1)", 180, 3, 1),
            row("udp/53", "udp/53 (domain)", 176, 2, 2),
        ]
    );
    // What is really on the wire: the voice was marked EF, nothing else was.
    assert_eq!(
        rows(&by("dscp").await),
        vec![
            row("0", "DF (0)", 36_376 - 9_400, 105 - 50, 6),
            row("46", "EF (46)", 9_400, 50, 1),
        ]
    );
    // softflowd reads a file and knows no interface: it says 0 for both,
    // and that is what is shown, not a port made up.
    let interfaces = by("interface").await;
    assert_eq!(
        rows(&interfaces),
        vec![
            row("127.0.0.1 0 in", "no interface reported in", 36_376, 105, 7),
            row(
                "127.0.0.1 0 out",
                "no interface reported out",
                36_376,
                105,
                7
            ),
        ]
    );
    assert_eq!(
        interfaces["rows"][0]["interface"],
        json!({"exporter": "127.0.0.1", "device_id": id, "if_index": 0, "name": null, "direction": "in"})
    );

    // The coverage block, the same whichever way it is looked at.
    let answer = by("source").await;
    assert_eq!(
        answer["total"],
        json!({"bytes": 36_376, "packets": 105, "flows": 7})
    );
    assert_eq!(
        answer["other"],
        json!({"bytes": 0, "packets": 0, "flows": 0})
    );
    assert!(answer["rows"][0].get("interface").is_none());
    let coverage = &answer["coverage"];
    let until = coverage["until"].as_i64().unwrap();
    let since = coverage["since"].as_i64().unwrap();
    assert_eq!(since % 60_000, 0, "a bucket starts on a minute");
    assert!(
        since <= until && until - since < 120_000,
        "the data is from the minute it arrived in: {coverage:#}"
    );
    assert_eq!(
        coverage["window_start"].as_i64().unwrap(),
        (until / 60_000 - 15) * 60_000,
        "fifteen whole minutes and the running one, when nothing else is asked for"
    );
    assert_eq!(
        (
            &coverage["collecting"],
            &coverage["records"],
            &coverage["sets_without_template"],
            &coverage["sampling"],
            &coverage["sampled_records"],
            &coverage["exporters"],
        ),
        (
            &json!(true),
            &json!(7),
            &json!(0),
            &json!([]),
            &json!(0),
            &json!(1)
        )
    );

    let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
    let exporter = &exporters[0];
    assert_eq!(exporters.as_array().unwrap().len(), 1);
    assert_eq!(
        (
            &exporter["address"],
            &exporter["device_id"],
            &exporter["device"],
            &exporter["reason"],
            &exporter["version"],
            &exporter["datagrams"],
            &exporter["records"],
            &exporter["dropped"],
            &exporter["sets_without_template"],
            &exporter["sampling"],
        ),
        (
            &json!("127.0.0.1"),
            &json!(id),
            &json!("SW1"),
            &json!(""),
            &json!(9),
            &json!(1),
            &json!(7),
            &json!(0),
            &json!(0),
            &json!([]),
        )
    );
    assert_eq!(
        exporter["templates"],
        json!(5),
        "softflowd's four flow templates and its options template"
    );
    assert_eq!(exporter["last_at"], status["flows"]["last_at"]);
}

#[tokio::test]
async fn every_version_of_the_same_traffic_adds_up_to_the_same_talkers() {
    // Captured: softflowd as v5, v9 and IPFIX, and nfreplay's own v9
    // template, each into a collector of its own.
    for (name, capture, version, with_ipv6) in [
        ("softflowd v5", captures::SOFTFLOWD_V5, 5, false),
        ("softflowd v9", captures::SOFTFLOWD_V9, 9, true),
        ("softflowd IPFIX", captures::SOFTFLOWD_IPFIX, 10, true),
        ("nfreplay v9", captures::NFREPLAY_V9, 9, true),
        ("nfreplay v5", captures::NFREPLAY_V5, 5, false),
    ] {
        let (lab, _id, port) = exporting_switch().await;
        let app = &lab.app;
        send_datagrams("127.0.0.1", port, &[hex(capture)]).await;
        let records = if with_ipv6 { 7 } else { 6 };
        wait_until_flows_settled(app, records).await;

        let mut expected = vec![
            row("tcp/443", "tcp/443 (https)", 25_760, 44, 2),
            row("udp/16384", "udp/16384 (rtp)", 9_400, 50, 1),
            row("tcp/22", "tcp/22 (ssh)", 860, 6, 1),
            row("ip/1", "ICMP (IP protocol 1)", 180, 3, 1),
            row("udp/53", "udp/53 (domain)", 176, 2, 2),
        ];
        if !with_ipv6 {
            // NetFlow v5 cannot carry the SSH conversation: it is IPv6.
            expected.remove(2);
        }
        assert_eq!(
            rows(&top(app, json!({"by": "application"})).await),
            expected,
            "{name}"
        );
        let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
        assert_eq!(exporters[0]["version"], json!(version), "{name}");
    }
}

#[tokio::test]
async fn a_sampling_exporter_has_its_rate_reported_and_its_counts_left_as_sent() {
    // Captured: softflowd -s 100, in each version's own way of saying so
    // (the v5 header, v9 options, IPFIX options the PSAMP way).
    for (name, capture, records) in [
        ("v5", captures::SOFTFLOWD_V5_SAMPLED, 1),
        ("v9", captures::SOFTFLOWD_V9_SAMPLED, 2),
        ("IPFIX", captures::SOFTFLOWD_IPFIX_SAMPLED, 2),
    ] {
        let (lab, _id, port) = exporting_switch().await;
        let app = &lab.app;
        send_datagrams("127.0.0.1", port, &[hex(capture)]).await;
        wait_until_flows_settled(app, records).await;

        let answer = top(app, json!({"by": "source"})).await;
        assert_eq!(
            answer["coverage"]["sampling"],
            json!([100]),
            "softflowd {name}"
        );
        assert_eq!(
            answer["coverage"]["sampled_records"], answer["coverage"]["records"],
            "softflowd {name}"
        );
        // softflowd looked at one packet of the download's forty-four and
        // reports that packet: 40 bytes. Not 4,000.
        assert!(
            rows(&answer).contains(&row("10.0.0.5", "10.0.0.5", 40, 1, 1)),
            "softflowd {name}: what was exported, not what it may stand for: {answer:#}"
        );
        let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
        assert_eq!(exporters[0]["sampling"], json!([100]), "softflowd {name}");
    }
}

#[tokio::test]
async fn sampled_and_unsampled_records_are_told_apart_in_one_answer() {
    // By hand: a v5 header announcing 1 in 512, and one announcing nothing.
    let (lab, _id, port) = exporting_switch().await;
    let app = &lab.app;
    let sampled = v5(0x4000 | 512, &[V5::https([10, 0, 0, 1], 1_000, 1)]);
    let whole = v5(
        0,
        &[
            V5::https([10, 0, 0, 2], 300, 3),
            V5::https([10, 0, 0, 3], 200, 2),
        ],
    );
    send_datagrams("127.0.0.1", port, &[sampled, whole]).await;
    wait_until_flows_settled(app, 3).await;

    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(
        (
            &answer["coverage"]["sampling"],
            &answer["coverage"]["sampled_records"],
            &answer["coverage"]["records"],
        ),
        (&json!([512]), &json!(1), &json!(3))
    );
    assert_eq!(answer["total"]["bytes"], json!(1_500), "added up as sent");
}

#[tokio::test]
async fn data_that_arrives_before_its_template_is_counted_and_never_guessed_at() {
    // By hand (RFC 3954): the data flow set first, as after an exporter's
    // restart or a lost template datagram.
    let (lab, _id, port) = exporting_switch().await;
    let app = &lab.app;
    send_datagrams("127.0.0.1", port, &[v9(&[v9_data()])]).await;
    let status = wait_for(app, "the set without a template to be counted", |status| {
        flows_of(status, "sets_without_template") == 1
    })
    .await;
    assert_eq!(
        (flows_of(&status, "records"), flows_of(&status, "malformed")),
        (0, 0),
        "it is a well-formed export that cannot be read yet, not garbage, and not a flow"
    );

    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(
        answer["rows"],
        json!([]),
        "nothing is made of bytes nobody can read"
    );
    assert_eq!(
        answer["total"],
        json!({"bytes": 0, "packets": 0, "flows": 0})
    );
    assert_eq!(
        (
            &answer["coverage"]["sets_without_template"],
            &answer["coverage"]["records"],
            &answer["coverage"]["exporters"],
        ),
        (&json!(1), &json!(0), &json!(1)),
        "the answer says an exporter sent something it does not show"
    );
    let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
    assert_eq!(
        (
            &exporters[0]["sets_without_template"],
            &exporters[0]["templates"],
            &exporters[0]["records"],
        ),
        (&json!(1), &json!(0), &json!(0)),
        "and the exporter is listed, with why nothing of it is there"
    );

    // The template arrives; what comes after it is read.
    send_datagrams("127.0.0.1", port, &[v9(&[v9_template()]), v9(&[v9_data()])]).await;
    wait_until_flows_settled(app, 1).await;
    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(
        rows(&answer),
        vec![row("10.0.0.1", "10.0.0.1", 4_000, 4, 1)]
    );
    assert_eq!(
        answer["coverage"]["sets_without_template"],
        json!(1),
        "the set that came too early stays counted: it was never read"
    );
    let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
    assert_eq!(exporters[0]["templates"], json!(1));
}

#[tokio::test]
async fn a_collector_that_was_moved_or_switched_off_and_on_still_knows_what_its_exporters_said() {
    // By hand (RFC 3954): options saying the exporter samples one packet in
    // a hundred, its template, one record — all from one socket, as an
    // exporter sends from one port. An exporter repeats its templates often
    // and its options seldom, so a collector that forgot both when it was
    // moved would count what came in between as unsampled: a number that
    // looks measured and is not.
    let (lab, _id, port) = exporting_switch().await;
    let app = &lab.app;
    let exporter = send_datagrams(
        "127.0.0.1",
        port,
        &[
            v9(&[v9_options_template(), v9_sampling(100)]),
            v9(&[v9_template()]),
            v9(&[v9_data()]),
        ],
    )
    .await;
    wait_until_flows_settled(app, 1).await;
    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(
        (
            &answer["coverage"]["sampling"],
            &answer["coverage"]["sampled_records"]
        ),
        (&json!([100]), &json!(1))
    );

    // A port of its own choosing would be "port 0" again, which is what the
    // collector already runs with: it is given a number.
    let free = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let moved = free.local_addr().unwrap().port();
    drop(free);
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"flows": {"port": moved}}}),
    )
    .await
    .unwrap();
    wait_for(app, "the collector on its new port", |status| {
        status["flows"]["listening"][0]
            .as_str()
            .is_some_and(|bound| bound.ends_with(&format!(":{moved}")))
    })
    .await;

    // The template again, as the exporter repeats it, and a record; not the
    // options.
    exporter
        .send_to(&v9(&[v9_template(), v9_data()]), ("127.0.0.1", moved))
        .await
        .unwrap();
    wait_until_flows_settled(app, 2).await;
    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(
        (
            &answer["coverage"]["records"],
            &answer["coverage"]["sampled_records"],
            &answer["coverage"]["sampling"],
        ),
        (&json!(2), &json!(2), &json!([100])),
        "the record after the move is a sample like the one before it: {answer:#}"
    );

    // Switched off and on again; then a record without its template.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"flows": {"enabled": false}}}),
    )
    .await
    .unwrap();
    wait_for(app, "the collector to stop", |status| {
        status["flows"]["listening"] == json!([])
    })
    .await;
    let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
    assert_eq!(
        exporters[0]["templates"],
        json!(2),
        "the templates (a flow template and an options one) are still known while it is off"
    );
    let port = start_flows(app).await;
    exporter
        .send_to(&v9(&[v9_data()]), ("127.0.0.1", port))
        .await
        .unwrap();
    let status = wait_until_flows_settled(app, 3).await;
    assert_eq!(
        (
            flows_of(&status, "sets_without_template"),
            flows_of(&status, "counted")
        ),
        (0, 3),
        "a record sent without its template is read with the one sent before: {status:#}"
    );
    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(
        (
            &answer["coverage"]["sampled_records"],
            &answer["total"]["bytes"]
        ),
        (&json!(3), &json!(12_000))
    );
}

#[tokio::test]
async fn garbage_on_the_port_is_counted_and_makes_nobody_an_exporter() {
    let (lab, _id, port) = exporting_switch().await;
    let app = &lab.app;
    let mut cut_short = v5(0, &[V5::https([10, 0, 0, 1], 100, 1)]);
    cut_short.truncate(40);
    send_datagrams(
        "127.0.0.1",
        port,
        &[b"not a flow export".to_vec(), cut_short, vec![0, 7, 0, 0]],
    )
    .await;
    let status = wait_for(app, "the garbage to be counted", |status| {
        flows_of(status, "malformed") == 3
    })
    .await;
    assert_eq!(
        (
            flows_of(&status, "received"),
            flows_of(&status, "records"),
            flows_of(&status, "exporters"),
        ),
        (3, 0, 0)
    );
    assert_eq!(status["flows"]["last_at"], Value::Null);
    assert_eq!(
        app.rpc("flows.exporters", Value::Null).await.unwrap(),
        json!([]),
        "anybody can send bytes: that does not make an exporter to list"
    );
}

#[tokio::test]
async fn an_interface_is_named_when_the_monitor_knows_it_and_numbered_when_not() {
    // What stands in for the monitoring feature: `monitor.ifindex` as its
    // specification describes it. It knows index 3 of the switch.
    let mut builder = nocturne_app::testing::builder();
    nocturne_logs::init(&mut builder);
    builder
        .router
        .add("monitor.ifindex", |_app: App, _params: Value| async move {
            Ok(json!({"interfaces": [{"index": 3, "name": "GigabitEthernet0/3"}]}))
        });
    let lab = nocturne_app::testing::lab(builder, &["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    let port = start_flows(app).await;

    // By hand: two flows in through ports 3 and 4, both out through port 5.
    let mut second = V5::https([10, 0, 0, 2], 500, 5);
    second.input = 4;
    let datagram = v5(0, &[V5::https([10, 0, 0, 1], 1_500, 10), second]);
    send_datagrams("127.0.0.1", port, std::slice::from_ref(&datagram)).await;
    wait_until_flows_settled(app, 2).await;

    let answer = top(app, json!({"by": "interface"})).await;
    assert_eq!(
        rows(&answer),
        vec![
            row("127.0.0.1 5 out", "ifIndex 5 out", 2_000, 15, 2),
            row("127.0.0.1 3 in", "GigabitEthernet0/3 in", 1_500, 10, 1),
            row("127.0.0.1 4 in", "ifIndex 4 in", 500, 5, 1),
        ],
        "a port the monitor has no name for is its number, not a guess"
    );
    assert_eq!(
        answer["rows"][1]["interface"],
        json!({"exporter": "127.0.0.1", "device_id": id, "if_index": 3,
               "name": "GigabitEthernet0/3", "direction": "in"})
    );
    assert_eq!(answer["rows"][0]["interface"]["name"], Value::Null);
    assert_eq!(
        answer["total"],
        json!({"bytes": 2_000, "packets": 15, "flows": 2}),
        "a flow is under two interface keys and once in the total"
    );

    // A build without the monitoring feature: the method is not there, and
    // every port is a number.
    let (lab, _id, port) = exporting_switch().await;
    send_datagrams("127.0.0.1", port, &[datagram]).await;
    wait_until_flows_settled(&lab.app, 2).await;
    let labels: Vec<String> = rows(&top(&lab.app, json!({"by": "interface"})).await)
        .into_iter()
        .map(|row| row.1)
        .collect();
    assert_eq!(labels, ["ifIndex 5 out", "ifIndex 3 in", "ifIndex 4 in"]);
}

#[tokio::test]
async fn an_exporter_no_device_claims_is_listed_and_followed_or_listed_and_dropped() {
    // The inventory has a switch, but not at the address the export comes from.
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    let port = start_flows(app).await;
    let datagram = v5(0, &[V5::https([10, 0, 0, 1], 1_500, 10)]);

    send_datagrams("127.0.0.1", port, std::slice::from_ref(&datagram)).await;
    let status = wait_until_flows_settled(app, 1).await;
    assert_eq!(flows_of(&status, "unattributed_exporters"), 1);
    let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
    assert_eq!(
        (
            &exporters[0]["device_id"],
            &exporters[0]["device"],
            &exporters[0]["records"]
        ),
        (&Value::Null, &Value::Null, &json!(1))
    );
    assert_eq!(
        exporters[0]["reason"],
        json!(
            "no device in the inventory has the address 127.0.0.1, and no collected interface carries it"
        )
    );
    // Its traffic is counted, and it can be asked for by its address.
    assert_eq!(rows(&top(app, json!({"by": "source"})).await).len(), 1);
    assert_eq!(
        rows(&top(app, json!({"by": "source", "exporter": "127.0.0.1"})).await),
        vec![row("10.0.0.1", "10.0.0.1", 1_500, 10, 1)]
    );
    // Nothing of it is the switch's.
    let of_switch = top(app, json!({"by": "source", "device_id": id})).await;
    assert_eq!(
        (&of_switch["rows"], &of_switch["coverage"]["exporters"]),
        (&json!([]), &json!(0))
    );

    // `unknown_sources: "drop"`: it stays listed, and what it sends from
    // now on is counted as dropped rather than added up.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"unknown_sources": "drop"}}),
    )
    .await
    .unwrap();
    // The setting reaches the collector with the next `apply`.
    app.rpc("logs.restart", Value::Null).await.unwrap();
    send_datagrams("127.0.0.1", port, std::slice::from_ref(&datagram)).await;
    let status = wait_until_flows_settled(app, 2).await;
    assert_eq!(
        (
            flows_of(&status, "counted"),
            flows_of(&status, "dropped"),
            status["flows"]["drops"]["unclaimed"].as_u64().unwrap(),
        ),
        (1, 1, 1)
    );
    let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
    assert_eq!(
        (
            &exporters[0]["records"],
            &exporters[0]["dropped"],
            &exporters[0]["datagrams"]
        ),
        (&json!(1), &json!(1), &json!(2)),
        "listed with what was not kept, not dropped without trace"
    );
    assert_eq!(
        top(app, json!({"by": "source"})).await["total"]["bytes"],
        json!(1_500)
    );

    // The switch is given that address: the exporter is the switch from
    // then on, by id and by name.
    set_address(app, &id, "127.0.0.1").await;
    send_datagrams("127.0.0.1", port, &[datagram]).await;
    let status = wait_until_flows_settled(app, 3).await;
    assert_eq!(
        (
            flows_of(&status, "counted"),
            flows_of(&status, "unattributed_exporters")
        ),
        (2, 0)
    );
    let exporters = app.rpc("flows.exporters", Value::Null).await.unwrap();
    assert_eq!(
        (
            &exporters[0]["device_id"],
            &exporters[0]["device"],
            &exporters[0]["reason"]
        ),
        (&json!(id), &json!("SW1"), &json!(""))
    );
    for reference in [id.as_str(), "SW1", "sw1"] {
        let answer = top(app, json!({"by": "source", "device_id": reference})).await;
        assert_eq!(
            answer["total"]["bytes"],
            json!(3_000),
            "what the address sent before it was the switch's is the switch's too: {reference}"
        );
    }
}

#[tokio::test]
async fn two_exporters_are_added_up_and_their_interfaces_kept_apart() {
    if !loopback_alias("127.0.0.2").await {
        eprintln!("skipped: this machine has no second loopback address to send from");
        return;
    }
    let lab = lab(&["SW1", "SW2"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    set_address(app, &lab.id("SW2"), "127.0.0.2").await;
    let port = start_flows(app).await;
    // By hand: the same conversation, seen by both switches on port 3.
    let datagram = v5(0, &[V5::https([10, 0, 0, 1], 1_500, 10)]);
    send_datagrams("127.0.0.1", port, std::slice::from_ref(&datagram)).await;
    send_datagrams("127.0.0.2", port, &[datagram]).await;
    wait_until_flows_settled(app, 2).await;

    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(
        rows(&answer),
        vec![row("10.0.0.1", "10.0.0.1", 3_000, 20, 2)],
        "a conversation that crosses two exporters is reported by both, and added up"
    );
    assert_eq!(
        answer["coverage"]["exporters"],
        json!(2),
        "and the answer says it is over two"
    );

    // Every switch has a port 3: the rows say whose.
    assert_eq!(
        rows(&top(app, json!({"by": "interface"})).await),
        vec![
            row("127.0.0.1 3 in", "SW1 ifIndex 3 in", 1_500, 10, 1),
            row("127.0.0.1 5 out", "SW1 ifIndex 5 out", 1_500, 10, 1),
            row("127.0.0.2 3 in", "SW2 ifIndex 3 in", 1_500, 10, 1),
            row("127.0.0.2 5 out", "SW2 ifIndex 5 out", 1_500, 10, 1),
        ]
    );
    // One device's answer is its own.
    let of_second = top(app, json!({"by": "interface", "device_id": "SW2"})).await;
    assert_eq!(
        rows(&of_second),
        vec![
            row("127.0.0.2 3 in", "ifIndex 3 in", 1_500, 10, 1),
            row("127.0.0.2 5 out", "ifIndex 5 out", 1_500, 10, 1),
        ]
    );
    assert_eq!(of_second["coverage"]["exporters"], json!(1));
}

/// What a datagram would have held, to hand to the collector's counting
/// step at a time of the test's choosing. The socket and the decoder are
/// what is skipped; they have their own tests above and in the engine.
fn batch(exporter: &str, records: Vec<FlowRecord>) -> FlowBatch {
    FlowBatch {
        exporter: exporter.parse().expect("an address"),
        version: 9,
        records,
        sets_without_template: 0,
        templates: 1,
    }
}

fn record(src: &str, bytes: u64) -> FlowRecord {
    FlowRecord {
        exporter: "127.0.0.1".into(),
        version: 9,
        src: src.into(),
        dst: "192.0.2.80".into(),
        src_port: 51_000,
        dst_port: 443,
        protocol: 6,
        bytes,
        packets: 1,
        tos: 0,
        tcp_flags: 0,
        input_if: 3,
        output_if: 5,
        src_as: 0,
        dst_as: 0,
        next_hop: None,
        start_ms: 0,
        end_ms: 0,
        sampling: 0,
    }
}

#[tokio::test]
async fn what_arrived_more_than_a_day_ago_is_gone_and_the_answer_says_since_when() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let now = nocturne_app::util::now_ms();
    let minute = |at: i64| at.div_euclid(60_000) * 60_000;
    let yesterday = now - 25 * 3_600_000;
    let ten_minutes_ago = now - 10 * 60_000;
    nocturne_logs::flows::take(
        app,
        &batch("127.0.0.1", vec![record("10.0.0.1", 111)]),
        yesterday,
    );
    nocturne_logs::flows::take(
        app,
        &batch("127.0.0.1", vec![record("10.0.0.2", 222)]),
        ten_minutes_ago,
    );

    // A day is as far back as anything goes.
    let answer = top(app, json!({"by": "source", "minutes": 1440})).await;
    assert_eq!(
        rows(&answer),
        vec![row("10.0.0.2", "10.0.0.2", 222, 1, 1)],
        "what arrived twenty-five hours ago is not in a day's answer"
    );
    assert_eq!(
        answer["coverage"]["since"],
        json!(minute(ten_minutes_ago)),
        "and the answer says where its data begins, which is not where the window does"
    );
    assert!(answer["coverage"]["window_start"].as_i64().unwrap() < minute(ten_minutes_ago));
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(
        (
            &status["flows"]["memory"]["minutes"],
            &status["flows"]["memory"]["oldest_at"]
        ),
        (&json!(1), &json!(minute(ten_minutes_ago))),
        "the old bucket is not kept somewhere out of sight either"
    );

    // A window that ends before the data begins has nothing, and says so
    // rather than showing an empty list that reads as no traffic.
    let answer = top(app, json!({"by": "source", "minutes": 5})).await;
    assert_eq!(
        (
            &answer["rows"],
            &answer["coverage"]["since"],
            &answer["coverage"]["exporters"]
        ),
        (&json!([]), &Value::Null, &json!(0))
    );
    // Twelve minutes reaches it.
    let answer = top(app, json!({"by": "source", "minutes": 12})).await;
    assert_eq!(answer["total"]["bytes"], json!(222));
}

/// Fifty thousand flows from as many made-up sources, a hundred to a
/// datagram, after one from a source that matters.
fn flood(app: &App, at: i64) {
    nocturne_logs::flows::take(
        app,
        &batch("127.0.0.1", vec![record("10.9.9.9", 9_000_000)]),
        at,
    );
    for chunk in 0..500u32 {
        let records = (0..100u32)
            .map(|n| {
                let made_up = chunk * 100 + n;
                record(&format!("172.16.{}.{}", made_up >> 8, made_up & 255), 40)
            })
            .collect();
        nocturne_logs::flows::take(app, &batch("127.0.0.1", records), at);
    }
}

#[tokio::test]
async fn a_flood_of_made_up_addresses_cannot_grow_the_running_minute_without_bound() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    flood(app, nocturne_app::util::now_ms());

    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    // Sources and conversations at their ceiling of 512 each and the five
    // keys everything shares — and, should the clock have passed a minute
    // boundary while the flood ran, the forty-five a finished minute keeps.
    let keys = status["flows"]["memory"]["keys"].as_u64().unwrap();
    assert!(
        keys <= 2 * 512 + 5 + 45,
        "{keys} keys are held for fifty thousand sources"
    );
    assert_eq!(flows_of(&status, "counted"), 50_001);

    let answer = top(app, json!({"by": "source", "limit": 500})).await;
    let listed = rows(&answer);
    assert_eq!(
        listed[0],
        row("10.9.9.9", "10.9.9.9", 9_000_000, 1, 1),
        "the one that mattered is still there, whole"
    );
    assert_eq!(
        answer["total"],
        json!({"bytes": 9_000_000 + 50_000 * 40, "packets": 50_001, "flows": 50_001}),
        "the total is exact whatever was folded"
    );
    let in_rows: u64 = listed.iter().map(|row| row.2).sum();
    assert_eq!(
        in_rows + answer["other"]["bytes"].as_u64().unwrap(),
        answer["total"]["bytes"].as_u64().unwrap(),
        "every byte is in a row or in `other`"
    );
    assert!(
        answer["other"]["flows"].as_u64().unwrap() >= 50_001 - 512,
        "and the answer says how much is in no row: {:#}",
        answer["other"]
    );
}

#[tokio::test]
async fn a_minute_that_is_over_keeps_its_twenty_biggest_talkers_and_says_what_else_there_was() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // In the minute before this one, so that it is over when it is asked about.
    flood(app, nocturne_app::util::now_ms() - 60_000);

    let answer = top(app, json!({"by": "source", "limit": 500})).await;
    let listed = rows(&answer);
    assert_eq!(listed[0], row("10.9.9.9", "10.9.9.9", 9_000_000, 1, 1));
    assert_eq!(listed.len(), 20);
    assert_eq!(
        answer["total"],
        json!({"bytes": 9_000_000 + 50_000 * 40, "packets": 50_001, "flows": 50_001})
    );
    assert_eq!(
        answer["other"],
        json!({"bytes": (50_001 - 20) * 40, "packets": 50_001 - 20, "flows": 50_001 - 20}),
        "what the other 49,981 sources sent is one number, not 49,981 rows"
    );
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(
        status["flows"]["memory"]["keys"],
        json!(2 * 20 + 5),
        "twenty sources, twenty conversations, and the five keys they share"
    );
}

#[tokio::test]
async fn a_storm_is_dropped_and_counted_rather_than_queued_without_limit() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    const LIMIT: u64 = 100;
    let port = start_flows_with(app, json!({"queue_limit": LIMIT})).await;
    // By hand: thirty records to a datagram, as many as NetFlow v5 allows.
    let thirty: Vec<V5> = (0..30u8)
        .map(|n| V5::https([10, 0, 1, n], 100, 1))
        .collect();
    let datagram = v5(0, &thirty);

    // On a test's single thread the socket is read in bursts: twenty
    // datagrams — six hundred records — are decoded before the counting
    // task gets a turn, and a hundred may wait.
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut most_queued = 0;
    for index in 0..400u32 {
        socket
            .send_to(&datagram, ("127.0.0.1", port))
            .await
            .unwrap();
        if index % 20 == 19 {
            tokio::task::yield_now().await;
        }
        if index % 100 == 99 {
            let status = app.rpc("logs.status", Value::Null).await.unwrap();
            most_queued = most_queued.max(flows_of(&status, "queued"));
        }
    }

    // Settled: nothing more arrives, and every record that did is accounted
    // for. How many datagrams arrived is the kernel's business.
    let mut last = 0;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    let status = loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let status = app.rpc("logs.status", Value::Null).await.unwrap();
        most_queued = most_queued.max(flows_of(&status, "queued"));
        let received = flows_of(&status, "received");
        if received == last
            && flows_of(&status, "queued") == 0
            && flows_of(&status, "counted") + flows_of(&status, "dropped")
                == flows_of(&status, "records")
        {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the records never added up: some are neither counted nor dropped: {status:#}"
        );
        last = received;
    };
    let (received, records, counted, dropped) = (
        flows_of(&status, "received"),
        flows_of(&status, "records"),
        flows_of(&status, "counted"),
        flows_of(&status, "dropped"),
    );
    assert!(
        received > 100,
        "the storm reached the collector: {status:#}"
    );
    assert_eq!(records, received * 30, "all of it was decoded: {status:#}");
    assert!(
        most_queued <= LIMIT,
        "what waited was never more than the limit: {most_queued}"
    );
    assert!(
        dropped > 0,
        "what did not fit was dropped, not kept: {status:#}"
    );
    assert!(counted >= 90, "{status:#}");
    assert_eq!(
        counted + dropped,
        records,
        "every record is in a bucket or counted as dropped: {status:#}"
    );
    assert_eq!(
        status["flows"]["drops"]["queue_full"].as_u64().unwrap(),
        dropped,
        "and the status says why: {status:#}"
    );
    assert_eq!(
        top(app, json!({"by": "destination"})).await["total"]["flows"],
        json!(counted),
        "what the buckets hold is what was counted"
    );

    // It is still listening.
    socket
        .send_to(&datagram, ("127.0.0.1", port))
        .await
        .unwrap();
    let after = wait_for(app, "a datagram after the storm to be counted", |status| {
        flows_of(status, "counted") == counted + 30
    })
    .await;
    assert_eq!(flows_of(&after, "dropped"), dropped);
}

#[tokio::test]
async fn the_collector_follows_its_settings_and_says_why_it_cannot_bind() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;

    // Hold a port, then ask the collector for it.
    let taken = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    let wanted = json!({"section": "logs", "value": {"flows": {"enabled": true, "port": port, "bind": "127.0.0.1"}}});
    app.rpc("settings.set", wanted).await.unwrap();
    let status = wait_for(app, "the collector to report the clash", |status| {
        !status["flows"]["error"].is_null()
    })
    .await;
    let error = status["flows"]["error"].as_str().unwrap();
    assert!(
        error.contains(&format!("127.0.0.1:{port}")) && error.contains("already using it"),
        "the error names the address and what is wrong: {error}"
    );
    assert_eq!(
        (&status["flows"]["enabled"], &status["flows"]["listening"]),
        (&json!(true), &json!([]))
    );
    assert_eq!(
        top(app, json!({"by": "source"})).await["coverage"]["collecting"],
        json!(false),
        "switched on and not listening is not collecting"
    );

    // Whatever held the port goes away; `logs.restart` is how a person says
    // "try again", with nothing in the settings changed.
    drop(taken);
    let status = app.rpc("logs.restart", Value::Null).await.unwrap();
    assert_eq!(status["flows"]["error"], Value::Null);
    assert_eq!(
        status["flows"]["listening"],
        json!([format!("udp 127.0.0.1:{port}")])
    );
    // A second restart leaves a running collector alone.
    let again = app.rpc("logs.restart", Value::Null).await.unwrap();
    assert_eq!(again["flows"]["listening"], status["flows"]["listening"]);

    let datagram = v5(0, &[V5::https([10, 0, 0, 1], 1_500, 10)]);
    send_datagrams("127.0.0.1", port, std::slice::from_ref(&datagram)).await;
    wait_until_flows_settled(app, 1).await;

    // Another port: the old one is closed, the new one answers, and the
    // numbers go on from where they were rather than from zero.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"flows": {"port": 0}}}),
    )
    .await
    .unwrap();
    let was = format!("udp 127.0.0.1:{port}");
    let status = wait_for(app, "the collector to move to another port", |status| {
        status["flows"]["listening"][0]
            .as_str()
            .is_some_and(|listening| listening != was)
    })
    .await;
    let moved = flow_port(&status);
    assert!(
        tokio::net::UdpSocket::bind(("127.0.0.1", port))
            .await
            .is_ok(),
        "the port it was moved off is free again"
    );
    send_datagrams("127.0.0.1", moved, &[datagram]).await;
    let status = wait_until_flows_settled(app, 2).await;
    assert_eq!(
        (flows_of(&status, "received"), flows_of(&status, "counted")),
        (2, 2),
        "a collector that is rebound does not forget what it had counted"
    );

    // Switched off: the port closes, and what was counted can still be
    // asked for — with the answer saying nothing new is coming in.
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"flows": {"enabled": false}}}),
    )
    .await
    .unwrap();
    let status = wait_for(app, "the collector to stop", |status| {
        status["flows"]["listening"] == json!([])
    })
    .await;
    assert_eq!(status["flows"]["enabled"], json!(false));
    assert!(
        tokio::net::UdpSocket::bind(("127.0.0.1", moved))
            .await
            .is_ok()
    );
    let answer = top(app, json!({"by": "source"})).await;
    assert_eq!(
        (&answer["total"]["bytes"], &answer["coverage"]["collecting"]),
        (&json!(3_000), &json!(false))
    );

    // Each start is in the audit trail, with the address and nothing else.
    let audit = query(app, json!({"stream": "audit", "limit": 50})).await;
    let starts: Vec<&Value> = audit
        .iter()
        .filter(|entry| {
            entry["source"] == json!("logs.receiver.start")
                && entry["body"]["receiver"] == json!("flows")
        })
        .collect();
    assert_eq!(starts.len(), 2, "{audit:#?}");
    assert_eq!(
        starts[1]["body"],
        json!({"receiver": "flows", "listening": [format!("udp 127.0.0.1:{port}")]})
    );
}

#[tokio::test]
async fn an_address_that_is_not_one_is_a_reason_not_a_crash() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    app.rpc(
        "settings.set",
        json!({"section": "logs", "value": {"flows": {"enabled": true, "port": 0, "bind": "everywhere"}}}),
    )
    .await
    .unwrap();
    let status = wait_for(app, "the collector to refuse the address", |status| {
        !status["flows"]["error"].is_null()
    })
    .await;
    assert!(
        status["flows"]["error"]
            .as_str()
            .unwrap()
            .contains("`everywhere` is not an address this machine can listen on"),
        "{status:#}"
    );
}

#[tokio::test]
async fn an_answer_that_cannot_be_given_is_refused_with_the_reason() {
    let (lab, _id, _port) = exporting_switch().await;
    let app = &lab.app;
    let refused = |params: Value| async move {
        let err = app
            .rpc("flows.top", params.clone())
            .await
            .expect_err(&format!("{params} is refused"));
        (err.code, err.message)
    };

    let (code, message) = refused(json!({})).await;
    assert_eq!(code, "invalid");
    assert_eq!(
        message,
        "`flows.top` needs `by`: one of source, destination, conversation, application, interface, dscp"
    );
    let (code, message) = refused(json!({"by": "port"})).await;
    assert_eq!(code, "invalid");
    assert!(
        message.starts_with("`port` is not something flows are added up by"),
        "{message}"
    );
    for minutes in [0, -5, 1441] {
        let (code, message) = refused(json!({"by": "source", "minutes": minutes})).await;
        assert_eq!(code, "invalid", "{minutes}");
        assert_eq!(
            message,
            "`minutes` is 1 to 1440: flows are kept for a day, in memory"
        );
    }
    let (code, _) = refused(json!({"by": "source", "device_id": "no-such-switch"})).await;
    assert_eq!(
        code, "not_found",
        "a device that is not there is not a device without traffic"
    );
    let (code, message) = refused(json!({"by": "source", "exporter": "the router"})).await;
    assert_eq!(code, "invalid");
    assert!(message.contains("`exporter` is the address"), "{message}");

    // A limit is brought into range rather than refused.
    let answer = top(app, json!({"by": "source", "limit": 0, "minutes": 1440})).await;
    assert_eq!(answer["rows"], json!([]));
}

#[tokio::test]
async fn nothing_of_a_flow_is_stored_published_or_logged() {
    let (lab, _id, port) = exporting_switch().await;
    let app = &lab.app;
    let logged = LogLines::capture();
    let mut events = app.subscribe();
    send_datagrams(
        "127.0.0.1",
        port,
        &[hex(captures::SOFTFLOWD_V9), b"garbage".to_vec()],
    )
    .await;
    wait_until_flows_settled(app, 7).await;
    top(app, json!({"by": "conversation"})).await;
    settle().await;

    // The addresses softflowd reported, searched for everywhere a flow
    // could have been written down.
    let stored = everything_stored(app);
    let published: String = drain_all(&mut events)
        .into_iter()
        .map(|(topic, payload)| format!("{topic} {payload}\n"))
        .collect();
    for address in ["192.0.2.80", "10.0.0.7", "2001:db8::22"] {
        assert!(!stored.contains(address), "{address} is in the database");
        assert!(
            !published.contains(address),
            "{address} was published: {published}"
        );
        assert!(!logged.text().contains(address), "{address} was logged");
    }
    assert!(
        query(app, json!({"limit": 50}))
            .await
            .iter()
            .all(|entry| entry["stream"] == json!("audit")),
        "the only rows are the audit trail's"
    );
}

/// softflowd, when it is installed.
fn softflowd() -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(["/usr/sbin", "/usr/local/sbin", "/usr/bin"].map(std::path::PathBuf::from))
        .map(|directory| directory.join("softflowd"))
        .find(|candidate| candidate.is_file())
}

#[tokio::test]
async fn softflowd_itself_fills_the_buckets_in_every_version() {
    let Some(softflowd) = softflowd() else {
        eprintln!(
            "skipped: softflowd is not installed (apt-get install softflowd), so no exporter \
             was run; the datagrams one sent are replayed by the tests above"
        );
        return;
    };
    let scratch = tempfile::tempdir().unwrap();
    std::fs::write(scratch.path().join("traffic.pcap"), traffic::pcap()).unwrap();

    for version in ["5", "9", "10"] {
        let (lab, _id, port) = exporting_switch().await;
        let app = &lab.app;
        // It reads the capture file, exports every flow in it and exits.
        let output = tokio::process::Command::new(&softflowd)
            .current_dir(scratch.path())
            .args(["-r", "traffic.pcap", "-n", &format!("127.0.0.1:{port}")])
            .args(["-v", version, "-6", "-d"])
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

        // What the packets add up to, per pair of addresses — counted from
        // the packets the capture file was written from, not from what
        // softflowd said.
        let mut expected: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
        let sent: Vec<traffic::Flow> = traffic::flows()
            .into_iter()
            .filter(|flow| version != "5" || !flow.src.contains(':'))
            .collect();
        for flow in &sent {
            let (a, b): (IpAddr, IpAddr) = (flow.src.parse().unwrap(), flow.dst.parse().unwrap());
            let (low, high) = if b < a { (b, a) } else { (a, b) };
            let entry = expected.entry(format!("{low} <-> {high}")).or_default();
            entry.0 += flow.bytes;
            entry.1 += flow.packets;
            entry.2 += 1;
        }
        wait_until_flows_settled(app, sent.len() as u64).await;
        let answer = top(app, json!({"by": "conversation"})).await;
        let found: BTreeMap<String, (u64, u64, u64)> = rows(&answer)
            .into_iter()
            .map(|row| (row.0, (row.2, row.3, row.4)))
            .collect();
        assert_eq!(found, expected, "softflowd -v {version}");
        assert_eq!(
            answer["coverage"]["sets_without_template"],
            json!(0),
            "softflowd -v {version}"
        );
        let status = app.rpc("logs.status", Value::Null).await.unwrap();
        assert_eq!(flows_of(&status, "malformed"), 0, "softflowd -v {version}");
        assert_eq!(
            rows(&top(app, json!({"by": "dscp"})).await)
                .iter()
                .map(|row| row.1.clone())
                .collect::<Vec<_>>(),
            ["DF (0)", "EF (46)"],
            "softflowd -v {version}: the marking the voice packets carried"
        );
    }
}
