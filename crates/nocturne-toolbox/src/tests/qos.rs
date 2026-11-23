//! The two QoS calculators through `app.rpc`.
//!
//! The arithmetic and the published tables are tested in `nocturne-tools`.
//! What is tested here is what a caller of the method sees: the parameter
//! names, the whole result as JSON (the interface is typed from these two
//! objects), and that a refusal arrives as `invalid` with the reason in it.
//! The numbers are worked out in the comments, not copied from a run.

use nocturne_tools::qos::SOURCE_PER_CALL;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::support;

/// One app for a whole test: these methods keep no state, and an app per
/// call is a database and a vault opened for one piece of arithmetic.
struct Calculators(nocturne_app::testing::Lab);

impl Calculators {
    async fn new() -> Self {
        Self(support::lab(&[]).await)
    }

    async fn call(&self, method: &str, params: Value) -> Value {
        self.0
            .app
            .rpc(method, params)
            .await
            .unwrap_or_else(|err| panic!("{method} failed: {err}"))
    }

    async fn refuse(&self, method: &str, params: Value) -> nocturne_app::AppError {
        self.0
            .app
            .rpc(method, params)
            .await
            .expect_err(&format!("{method} should have refused"))
    }
}

#[tokio::test]
async fn a_marking_comes_back_in_every_form() {
    let tools = Calculators::new().await;
    // EF is DSCP 46 = 0b101110 (RFC 3246). Two ECN bits to its right make
    // the byte 0b10111000 = 184 = 0xB8; its top three bits, 0b101, are
    // precedence 5, which RFC 791 calls CRITIC/ECP.
    let ef = tools
        .call("tools.qos.marking", json!({"value": "ef", "as": "name"}))
        .await;
    assert_eq!(
        ef,
        json!({
            "dscp": 46,
            "name": "EF",
            "dscp_binary": "101110",
            "dscp_hex": "0x2E",
            "tos": 184,
            "tos_hex": "0xB8",
            "tos_binary": "10111000",
            "precedence": 5,
            "precedence_name": "CRITIC/ECP",
            "ecn": 0,
            "ecn_binary": "00",
            "ecn_name": "Not-ECT",
            "ecn_meaning": "Not ECN-capable: a congested router can only drop this packet (RFC 3168)",
            "phb": "Expedited Forwarding (RFC 3246)",
            "typical_use": "Voice bearer (RTP): low loss, low delay, low jitter, strictly policed",
            "notes": [
                "The two ECN bits are not part of a DSCP or a precedence; the ToS byte is shown with them clear."
            ],
        })
    );

    // The same marking from a JSON number, which is what a script sends.
    let by_number = tools
        .call("tools.qos.marking", json!({"value": 46, "as": "dscp"}))
        .await;
    assert_eq!(by_number, ef);

    // A ToS byte keeps its ECN bits: 0xB9 = 0b10111001 is EF with ECN 01,
    // which RFC 3168 names ECT(1).
    let byte = tools
        .call("tools.qos.marking", json!({"value": "0xb9", "as": "tos"}))
        .await;
    assert_eq!(
        (&byte["dscp"], &byte["name"], &byte["tos"], &byte["ecn"]),
        (&json!(46), &json!("EF"), &json!(185), &json!(1))
    );
    assert_eq!(byte["ecn_name"], "ECT(1)");
    assert_eq!(byte["ecn_binary"], "01");

    // Precedence 5 is 0b101 in the top three bits: DSCP 40, the class
    // selector CS5, byte 160 — and the answer says what it left open.
    let precedence = tools
        .call("tools.qos.marking", json!({"value": 5, "as": "precedence"}))
        .await;
    assert_eq!(
        (&precedence["dscp"], &precedence["name"], &precedence["tos"]),
        (&json!(40), &json!("CS5"), &json!(160))
    );
    assert!(
        precedence["notes"][0]
            .as_str()
            .unwrap()
            .contains("DSCP 40 to 47"),
        "{precedence}"
    );

    // A code point nobody named has no name: null, not a made-up one.
    let unnamed = tools
        .call("tools.qos.marking", json!({"value": 47, "as": "dscp"}))
        .await;
    assert_eq!(unnamed["name"], Value::Null);
    assert_eq!(unnamed["typical_use"], Value::Null);
    assert_eq!(unnamed["tos"], 188);
}

#[tokio::test]
async fn a_marking_that_is_not_one_is_refused_and_says_why() {
    let tools = Calculators::new().await;
    // 184 is EF as a ToS byte; as a DSCP it is out of range, and it is not
    // quietly read as the byte.
    let out_of_range = tools
        .refuse("tools.qos.marking", json!({"value": 184, "as": "dscp"}))
        .await;
    assert_eq!(out_of_range.code, "invalid");
    assert_eq!(
        out_of_range.message,
        "invalid DSCP `184`: a DSCP is six bits, 0 to 63; write binary with a prefix, as in 0b101110, \
         and for the whole byte choose `tos`"
    );
    assert_eq!(out_of_range.details.unwrap()["reason"], "invalid");

    // The same number is three different markings, so it is not guessed at.
    let unsaid = tools
        .refuse("tools.qos.marking", json!({"value": 46}))
        .await;
    assert_eq!(unsaid.code, "invalid");
    assert!(
        unsaid
            .message
            .contains("`dscp`, `tos`, `precedence` or `name`"),
        "{}",
        unsaid.message
    );

    let unknown_form = tools
        .refuse("tools.qos.marking", json!({"value": 5, "as": "cos"}))
        .await;
    assert_eq!(unknown_form.code, "invalid");
    assert!(
        unknown_form.message.contains("`cos`"),
        "{}",
        unknown_form.message
    );

    // What was typed comes back in the refusal cut short and with its control
    // characters made visible: not the megabyte that was sent, and not an
    // escape sequence for a terminal or a log viewer to obey.
    let flood = format!("\u{1b}[31m{}", "ef ".repeat(400_000));
    for form in ["dscp", "tos", "precedence", "name"] {
        let refused = tools
            .refuse("tools.qos.marking", json!({"value": flood, "as": form}))
            .await;
        assert_eq!(refused.code, "invalid", "{form}");
        assert!(
            refused.message.chars().count() < 300 && !refused.message.contains('\u{1b}'),
            "{form}: {} characters",
            refused.message.chars().count()
        );
    }

    for params in [
        json!({}),
        json!({"as": "dscp"}),
        json!({"value": "af44", "as": "name"}),
        json!({"value": "46", "as": "name"}),
        json!({"value": 256, "as": "tos"}),
        json!({"value": 8, "as": "precedence"}),
        json!({"value": -1, "as": "dscp"}),
        json!({"value": [46], "as": "dscp"}),
    ] {
        let refused = tools.refuse("tools.qos.marking", params.clone()).await;
        assert_eq!(refused.code, "invalid", "{params}");
    }
}

#[tokio::test]
async fn voice_bandwidth_comes_back_per_call_and_in_total_with_the_arithmetic() {
    let tools = Calculators::new().await;
    // Ten G.729 calls over Multilink PPP with compressed RTP. By hand, with
    // the figures of Cisco document 7934: 20 ms of G.729 is 20 bytes; the
    // packet is 6 (MP) + 1 (flag) + 2 (cRTP) + 20 = 29 bytes; 1000 / 20 = 50
    // packets a second; 29 * 8 * 50 = 11 600 bps a call, 116 000 for ten.
    // With VAD at the document's 35 %: 65 % of those, 7 540 and 75 400.
    let answer = tools
        .call(
            "tools.qos.voice",
            json!({"codec": "g729", "calls": 10, "link": "mlppp", "crtp": true, "vad": true}),
        )
        .await;
    assert_eq!(
        answer,
        json!({
            "codec": {
                "id": "g729",
                "name": "G.729",
                "bit_rate_bps": 8000,
                "sample_bytes": 10,
                "sample_ms": 10,
                "packet_ms": 20,
                "payload_bytes": 20,
                "mos": 3.92,
            },
            "link": {
                "id": "mlppp",
                "name": "Multilink PPP",
                "overhead_bytes": 7,
                "parts": "MP header 6 + End-of-frame flag 1",
                "crtp": true,
                "source": "Cisco 7934",
                "note": "The Cisco SRND table counts 10 bytes for MLPPP; use the `custom` link with 10 bytes for that figure.",
            },
            "calls": 10,
            "crtp": true,
            "packet_ms": 20,
            "packets_per_second": 50.0,
            "payload_bytes": 20,
            "packet": [
                {"name": "MP header", "bytes": 6},
                {"name": "End-of-frame flag", "bytes": 1},
                {"name": "cRTP-compressed IP/UDP/RTP header", "bytes": 2},
                {"name": "Voice payload", "bytes": 20},
            ],
            "packet_bytes": 29,
            "per_call_bps": 11600,
            "total_bps": 116000,
            "per_call": "11.6 kbps",
            "total": "116 kbps",
            "vad": {
                "saving_pct": 35,
                "per_call_bps": 7540,
                "total_bps": 75400,
                "per_call": "7.54 kbps",
                "total": "75.4 kbps",
                "enough_calls": false,
                "caveat": "Up to 35 % is an average across more than 24 calls; with 10 it must not be counted on. Provision the full figure.",
            },
            "arithmetic": [
                "Voice payload: 20 ms ÷ 10 ms per sample × 10 bytes = 20 bytes",
                "Packet: 6 (MP header) + 1 (End-of-frame flag) + 2 (cRTP-compressed IP/UDP/RTP header) + 20 (Voice payload) = 29 bytes",
                "Packet rate: 1000 ms ÷ 20 ms = 50 packets per second",
                "Per call: 29 bytes × 8 bits × 50 packets per second = 11600 bps",
                "Total: 10 × 11600 bps = 116000 bps",
                "With voice-activity detection, at best 35 % less: 116000 bps × 65 % = 75400 bps",
            ],
            "notes": [
                "One direction of each call; the other direction costs the same again.",
                "Voice packets only: call signalling and RTCP are not included.",
                "The Cisco SRND table counts 10 bytes for MLPPP; use the `custom` link with 10 bytes for that figure.",
                "cRTP is counted at 2 bytes, as in Cisco's table. With UDP checksums it is 4 bytes: 800 bps more per call. Both ends of the link have to run it.",
            ],
            "sources": [SOURCE_PER_CALL],
        })
    );

    // The options are options: without them, G.711 on Ethernet is
    // (18 + 40 + 160) * 8 * 50 = 87 200 bps, 2 616 000 for thirty, and
    // there is no estimate where none was asked for.
    let plain = tools
        .call(
            "tools.qos.voice",
            json!({"codec": "G.711", "calls": 30, "link": "ethernet"}),
        )
        .await;
    assert_eq!(
        (&plain["per_call_bps"], &plain["total_bps"], &plain["vad"]),
        (&json!(87_200), &json!(2_616_000), &Value::Null)
    );
    assert_eq!(plain["crtp"], false);
    assert_eq!(plain["total"], "2.62 Mbps");

    // Voice per packet and a layer-2 size of one's own: 40 ms of G.729 with
    // no layer 2 is (40 + 40) * 8 * 25 = 16 000 bps, and Cisco's sample
    // calculation — a 6-byte MP header, cRTP, 20 bytes — is 28 * 8 * 50.
    let slower = tools
        .call(
            "tools.qos.voice",
            json!({"codec": "g729", "calls": 1, "link": "none", "packet_ms": 40}),
        )
        .await;
    assert_eq!(
        (&slower["per_call_bps"], &slower["packets_per_second"]),
        (&json!(16_000), &json!(25.0))
    );
    let sample = tools
        .call(
            "tools.qos.voice",
            json!({"codec": "g729", "calls": 1, "link": "custom", "l2_bytes": 6, "crtp": true}),
        )
        .await;
    assert_eq!(sample["per_call_bps"], 11_200);
    assert_eq!(sample["link"]["id"], "custom");
    assert_eq!(sample["link"]["overhead_bytes"], 6);
}

#[tokio::test]
async fn a_voice_request_that_makes_no_sense_is_refused_and_says_why() {
    let tools = Calculators::new().await;
    // Cisco: "cRTP is not available over Ethernet".
    let on_ethernet = tools
        .refuse(
            "tools.qos.voice",
            json!({"codec": "g729", "calls": 10, "link": "ethernet", "crtp": true}),
        )
        .await;
    assert_eq!(on_ethernet.code, "invalid");
    assert!(
        on_ethernet
            .message
            .contains("compressed RTP is not counted on Ethernet"),
        "{}",
        on_ethernet.message
    );

    let no_calls = tools
        .refuse("tools.qos.voice", json!({"codec": "g729", "link": "ppp"}))
        .await;
    assert_eq!(no_calls.code, "invalid");
    assert!(no_calls.message.contains("`calls`"), "{}", no_calls.message);

    let too_many = tools
        .refuse(
            "tools.qos.voice",
            json!({"codec": "g729", "calls": 1_000_001, "link": "ppp"}),
        )
        .await;
    assert_eq!(too_many.code, "invalid");
    assert_eq!(too_many.details.unwrap()["reason"], "too_large");

    let unknown = tools
        .refuse(
            "tools.qos.voice",
            json!({"codec": "opus", "calls": 1, "link": "ppp"}),
        )
        .await;
    assert!(
        unknown.message.contains("`opus`") && unknown.message.contains("g729"),
        "{}",
        unknown.message
    );

    // A codec or a link name is quoted back cut short, however long it was.
    let flood = "g729 ".repeat(200_000);
    for params in [
        json!({"codec": flood, "calls": 1, "link": "ppp"}),
        json!({"codec": "g729", "calls": 1, "link": flood}),
    ] {
        let refused = tools.refuse("tools.qos.voice", params).await;
        assert_eq!(refused.code, "invalid");
        assert!(
            refused.message.chars().count() < 300,
            "{} characters",
            refused.message.chars().count()
        );
    }

    // Numbers a form field cannot hold but a script can send.
    for params in [
        json!({"codec": "g729", "calls": 4_294_967_296_u64, "link": "ppp"}),
        json!({"codec": "g729", "calls": 1, "link": "ppp", "packet_ms": -20}),
        json!({"codec": "g729", "calls": 1, "link": "ppp", "packet_ms": 0}),
        json!({"codec": "g729", "calls": 1, "link": "ppp", "packet_ms": 250}),
        json!({"codec": "g729", "calls": 1, "link": "custom", "l2_bytes": 257}),
        json!({"codec": "g729", "calls": 1, "link": "custom", "l2_bytes": 1.5}),
        json!({"codec": "g729", "calls": 1, "link": "ppp", "crtp": "yes"}),
    ] {
        let refused = tools.refuse("tools.qos.voice", params.clone()).await;
        assert_eq!(refused.code, "invalid", "{params}");
    }

    for params in [
        json!({}),
        json!({"codec": "g729", "calls": 0, "link": "ppp"}),
        json!({"codec": "g729", "calls": -3, "link": "ppp"}),
        json!({"codec": "g729", "calls": "ten", "link": "ppp"}),
        json!({"codec": "g729", "calls": 2.5, "link": "ppp"}),
        json!({"codec": "g729", "calls": 1}),
        json!({"codec": "g729", "calls": 1, "link": "carrier pigeon"}),
        json!({"codec": "g723", "calls": 1, "link": "ppp"}),
        json!({"codec": "g729", "calls": 1, "link": "ppp", "packet_ms": 25}),
        json!({"codec": "g729", "calls": 1, "link": "ppp", "l2_bytes": 6}),
        json!({"codec": "g729", "calls": 1, "link": "custom"}),
    ] {
        let refused = tools.refuse("tools.qos.voice", params.clone()).await;
        assert_eq!(refused.code, "invalid", "{params}");
    }
}

#[tokio::test]
async fn the_codecs_and_links_of_the_voice_calculator_are_reference_tables() {
    let tools = Calculators::new().await;
    // A picker is built from these, so the ids are the ones the calculator takes.
    let codecs = tools
        .call("tools.reference", json!({"table": "voice_codecs"}))
        .await;
    assert_eq!(codecs["total"], 10);
    assert_eq!(
        codecs["columns"],
        json!([
            "id",
            "name",
            "bit_rate_bps",
            "sample_bytes",
            "sample_ms",
            "packet_ms",
            "payload_bytes",
            "mos"
        ])
    );
    // Cisco 7934: G.729 is 10 bytes every 10 ms, sent 20 ms (20 bytes) at a time.
    assert_eq!(
        codecs["rows"][1],
        json!({
            "id": "g729",
            "name": "G.729",
            "bit_rate_bps": 8000,
            "sample_bytes": 10,
            "sample_ms": 10,
            "packet_ms": 20,
            "payload_bytes": 20,
            "mos": 3.92,
        })
    );
    // The document gives no MOS for iLBC: null, not zero.
    assert_eq!(codecs["rows"][9]["id"], "ilbc-30");
    assert_eq!(codecs["rows"][9]["mos"], Value::Null);

    let links = tools
        .call("tools.reference", json!({"table": "voice_links"}))
        .await;
    assert_eq!(links["total"], 9);
    // Cisco 7934: "18 bytes for Ethernet L2 headers, which include 4 bytes of FCS".
    assert_eq!(
        links["rows"][0],
        json!({
            "id": "ethernet",
            "name": "Ethernet",
            "overhead_bytes": 18,
            "parts": "Ethernet header 14 + Ethernet FCS 4",
            "crtp": false,
            "source": "Cisco 7934",
            "note": "The frame as Cisco counts it: without the preamble and the inter-frame gap. The Cisco SRND table counts the 14-byte header alone.",
        })
    );
    // Every id in the tables is accepted by the calculator.
    for codec in codecs["rows"].as_array().unwrap() {
        for link in links["rows"].as_array().unwrap() {
            let answer = tools
                .call(
                    "tools.qos.voice",
                    json!({"codec": codec["id"], "calls": 1, "link": link["id"]}),
                )
                .await;
            assert_eq!(answer["codec"]["id"], codec["id"]);
            assert_eq!(answer["link"], *link);
        }
    }

    let tables = tools.call("tools.reference.tables", Value::Null).await;
    let listed = |id: &str| {
        tables
            .as_array()
            .unwrap()
            .iter()
            .find(|table| table["table"] == id)
            .unwrap_or_else(|| panic!("{id} is not listed"))
            .clone()
    };
    assert_eq!(
        listed("voice_codecs"),
        json!({
            "table": "voice_codecs",
            "title": "Voice codecs",
            "description": "Bit rate, sample size, default packet and payload of each codec; the input of tools.qos.voice.",
            "rows": 10,
        })
    );
    assert_eq!(
        listed("voice_links"),
        json!({
            "table": "voice_links",
            "title": "Layer-2 overhead of a voice packet",
            "description": "What each link type adds to a voice packet and whether compressed RTP is counted on it; the input of tools.qos.voice.",
            "rows": 9,
        })
    );
    // The aliases an engineer would type.
    assert_eq!(
        tools
            .call("tools.reference", json!({"table": "codecs"}))
            .await["table"],
        "voice_codecs"
    );

    // And both calculators are in the catalogue, as arithmetic.
    let catalogue = tools.call("tools.list", Value::Null).await;
    for id in ["tools.qos.marking", "tools.qos.voice"] {
        let entry = catalogue
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap_or_else(|| panic!("{id} is not in the catalogue"));
        assert_eq!(
            (&entry["group"], &entry["offline"], &entry["job"]),
            (&json!("qos"), &json!(true), &json!(false))
        );
    }
}
