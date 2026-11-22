//! Evidence through `App::rpc`.
//!
//! The terminal here is a stand-in that holds a real connection to the lab's
//! fake device (`common::open_session`): what a capture stores is what that
//! device printed. Unlike the real terminal feature the stand-in hides
//! nothing itself, so every secret that is absent from an item below was
//! hidden by this crate.

mod common;

use std::sync::Arc;

use common::{
    Session, With, fixture, hours_ago, lab, open_session, open_session_as, reachability, rpc,
};
use nocturne_app::store::LogQuery;
use nocturne_app::testing::Lab;
use nocturne_app::util::now_ms;
use nocturne_app::{App, Event};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast::Receiver;

/// `rpc`, which also writes the call down when `NOCTURNE_WIRE_SHAPES` names
/// a file: the examples the interface is typed from are taken from a run of
/// these tests rather than written from memory.
async fn call(app: &App, method: &str, params: Value) -> Value {
    let result = rpc(app, method, params.clone()).await;
    if let Ok(path) = std::env::var("NOCTURNE_WIRE_SHAPES") {
        use std::io::Write as _;
        let line = json!({"method": method, "params": params, "result": result});
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("the wire-shape file can be opened");
        writeln!(file, "{line}").expect("the wire-shape file can be written");
    }
    result
}

/// The refusal of a call that must not succeed.
async fn refused(app: &App, method: &str, params: Value) -> nocturne_app::AppError {
    match app.rpc(method, params.clone()).await {
        Ok(value) => panic!("`{method}` should have been refused; it returned {value}"),
        Err(err) => {
            if let Ok(path) = std::env::var("NOCTURNE_WIRE_SHAPES") {
                use std::io::Write as _;
                let line = json!({
                    "method": method,
                    "params": params,
                    "error": {"code": err.code, "message": err.message, "details": err.details},
                });
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .expect("the wire-shape file can be opened");
                writeln!(file, "{line}").expect("the wire-shape file can be written");
            }
            err
        }
    }
}

fn sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Every event published so far, as text.
fn drain(events: &mut Receiver<Event>) -> Vec<Event> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        seen.push(event);
    }
    seen
}

/// The whole audit trail, as text: action, sentence and detail.
fn audit_trail(app: &App) -> String {
    app.store()
        .log_query(&LogQuery {
            stream: Some("audit".into()),
            limit: Some(1_000),
            ..LogQuery::default()
        })
        .expect("the audit trail")
        .iter()
        .map(|entry| {
            format!(
                "{} | {} | {}\n",
                entry.source,
                entry.message,
                entry.body.clone().unwrap_or(Value::Null)
            )
        })
        .collect()
}

/// The newest audit entry with this action.
fn audited(app: &App, action: &str) -> Value {
    let entries = app
        .store()
        .log_query(&LogQuery {
            stream: Some("audit".into()),
            limit: Some(1_000),
            ..LogQuery::default()
        })
        .expect("the audit trail");
    let entry = entries
        .iter()
        .filter(|entry| entry.source == action)
        .max_by_key(|entry| entry.id)
        .unwrap_or_else(|| panic!("no audit entry `{action}`"));
    json!({"message": entry.message, "device_id": entry.device_id, "detail": entry.body})
}

/// How many rows of this feature's own tables contain `needle` anywhere.
fn stored_occurrences(app: &App, needle: &str) -> i64 {
    app.store()
        .with(|conn| {
            let records: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ops_records
                 WHERE instr(head, ?1) > 0 OR instr(body, ?1) > 0 OR instr(search, lower(?1)) > 0",
                [needle],
                |row| row.get(0),
            )?;
            let bodies: i64 = conn.query_row(
                "SELECT COUNT(*) FROM ops_evidence_bodies WHERE instr(body, ?1) > 0",
                [needle],
                |row| row.get(0),
            )?;
            Ok(records + bodies)
        })
        .expect("the tables can be read")
}

fn entry<'a>(incident: &'a Value, id: &str) -> Option<&'a Value> {
    incident["timeline"]
        .as_array()
        .expect("a timeline")
        .iter()
        .find(|entry| entry["id"] == json!(id))
}

async fn incident(app: &App, device_id: &str) -> String {
    let incident = rpc(
        app,
        "incidents.create",
        json!({"title": "SW1 uplink flapping", "device_ids": [device_id], "author": "mo"}),
    )
    .await;
    incident["id"].as_str().expect("an id").to_string()
}

/// A session on SW1 that has run two commands.
async fn session_with_output(lab: &Lab) -> Arc<Session> {
    let session = open_session(lab, "SW1").await;
    session.enter("show clock").await;
    session.enter("show version").await;
    session
}

// ---------------------------------------------------------------- capturing

#[tokio::test]
async fn a_capture_stores_what_the_session_printed_with_its_source_and_the_hash_of_what_is_stored()
{
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let session = session_with_output(&lab).await;
    let mut events = app.subscribe();
    let before = now_ms();

    let item = call(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id, "lines": 50}),
    )
    .await;

    // The text is what the device printed into that session.
    let text = item["body"]["text"].as_str().expect("text");
    assert_eq!(item["body"]["type"], "text");
    assert!(
        text.contains("SW1#show clock\n12:00:00.000 UTC Fri Nov 13 2026\n"),
        "{text}"
    );
    assert!(text.contains("SW1 uptime is 1 week, 2 days, 3 hours, 4 minutes"));
    assert!(text.ends_with("SW1#"), "the prompt it stopped at: {text}");
    assert_eq!(
        text,
        session.lines().join("\n"),
        "the whole session, as read"
    );

    // What is known about it was recorded here, not sent by the caller.
    assert_eq!(item["kind"], "terminal");
    assert_eq!(item["redacted"], true);
    assert_eq!(
        item["source"],
        json!({
            "from": "terminal.snapshot",
            "session_id": session.id,
            "session_title": "SW1",
            "device_id": lab.id("SW1"),
            "device": "SW1",
            "peer": "lab:SW1",
            "lab": true,
            "params": {"lines": 50},
        })
    );
    assert_eq!(
        item["title"],
        format!("SW1 — last {} lines", text.lines().count())
    );
    assert_eq!(item["lines"], text.lines().count());
    assert_eq!(item["bytes"], text.len());
    assert_eq!(item["sha256"], sha256(text), "of the text as it is stored");
    assert_eq!(item["incident_ids"], json!([]));
    let captured_at = item["captured_at"].as_i64().unwrap();
    assert!((before..=now_ms()).contains(&captured_at));

    // The terminal was asked for the session's lines with redaction on.
    let asked = fixture(app).params("terminal.snapshot").unwrap();
    assert_eq!(
        asked,
        json!({"session_id": session.id, "lines": 50, "redact": true})
    );

    // It can be read back whole, and listed without its text.
    let id = item["id"].as_str().unwrap();
    assert_eq!(call(app, "evidence.get", json!({"id": id})).await, item);
    let listed = call(app, "evidence.list", json!({})).await;
    let mut head = item.clone();
    head.as_object_mut().unwrap().remove("body");
    assert_eq!(listed, json!({"items": [head]}));

    // The announcement and the audit entry carry what it is, never its text.
    let seen = drain(&mut events);
    let changed: Vec<&Event> = seen
        .iter()
        .filter(|event| event.topic == "evidence.changed")
        .collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].payload, head);
    assert_eq!(
        audited(app, "evidence.capture_terminal"),
        json!({
            "message": format!(
                "Kept the last {} lines of the terminal session to SW1 as evidence",
                text.lines().count()
            ),
            "device_id": lab.id("SW1"),
            "detail": {
                "evidence_id": id,
                "kind": "terminal",
                "from": "terminal.snapshot",
                "session_id": session.id,
                "lines": text.lines().count(),
                "points": null,
                "bytes": text.len(),
                "sha256": sha256(text),
                "lab": true,
            },
        })
    );
    assert!(!audit_trail(app).contains("12:00:00.000 UTC"));
}

#[tokio::test]
async fn a_secret_in_the_session_is_not_in_the_item_the_audit_trail_or_an_event() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    // A device type with a secret pattern of its own, and a credential whose
    // password no pattern would recognise.
    nocturne_app::profiles::save(
        app,
        &nocturne_app::testing::LAB_IOS
            .replace("id: test_ios", "id: test_widget")
            .replace(
                "config:\n  syntax: ios",
                "config:\n  secrets: ['(?im)\\bwidget-key (?P<secret>\\S+)']\n  syntax: ios",
            ),
    )
    .expect("the second profile loads");
    rpc(
        app,
        "credentials.create",
        json!({"name": "core", "username": "ops", "password": "V4ult-Kept-Passw0rd"}),
    )
    .await;
    lab.fake("SW1").with(|state| {
        state.outputs.insert(
            "show logging".into(),
            "%SEC_LOGIN-4-LOGIN_FAILED: login as V4ult-Kept-Passw0rd rejected\n\
             widget-key Pr0file0nlyKey\n\
             done"
                .into(),
        );
    });
    let secrets = [
        "Sup3rS3cretEnable",
        "C0mmunityStr1ng",
        "Pla1nVtyPassw0rd",
        "V4ult-Kept-Passw0rd",
        "Pr0file0nlyKey",
    ];

    let session = open_session(&lab, "SW1").await;
    let mut events = app.subscribe();
    session.enter("configure terminal").await;
    session.enter("enable secret Sup3rS3cretEnable").await;
    session
        .enter("snmp-server community C0mmunityStr1ng RO")
        .await;
    session.enter("line vty 0 4").await;
    // After a prompt and with no encoding digit: the one shape the engine's
    // rules only catch at the start of a line.
    session.enter("password Pla1nVtyPassw0rd").await;
    session.enter("end").await;
    session.enter("show logging").await;
    session.enter("show running-config").await;
    let printed = session.lines().join("\n");
    for secret in secrets {
        assert!(
            printed.contains(secret),
            "the session itself shows `{secret}`, or this test proves nothing:\n{printed}"
        );
    }

    // `redact` is not a parameter of this method: sending it changes nothing.
    let item = call(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id, "lines": 500, "title": "config with secrets", "redact": false}),
    )
    .await;
    let text = item["body"]["text"].as_str().unwrap();
    assert!(
        text.contains("SW1(config)#enable secret <secret:"),
        "{text}"
    );
    assert!(
        text.contains("SW1(config-line)#password <secret:"),
        "{text}"
    );
    assert!(text.contains("login as <secret:"), "{text}");
    assert!(text.contains("widget-key <secret:"), "{text}");
    assert!(
        text.contains("interface GigabitEthernet0/1"),
        "the rest is kept"
    );
    assert_eq!(
        fixture(app).params("terminal.snapshot").unwrap()["redact"],
        true,
        "and the terminal is still asked with redaction on"
    );
    assert_eq!(item["sha256"], sha256(text));

    let id = item["id"].as_str().unwrap();
    let everything = format!(
        "{item}\n{}\n{}\n{}\n{}",
        call(app, "evidence.get", json!({"id": id})).await,
        call(app, "evidence.list", json!({"query": "config"})).await,
        audit_trail(app),
        drain(&mut events)
            .iter()
            .map(|event| format!("{} {}\n", event.topic, event.payload))
            .collect::<String>(),
    );
    for secret in secrets {
        assert!(
            !everything.contains(secret),
            "`{secret}` got out through a result, the audit trail or an event"
        );
        assert_eq!(
            stored_occurrences(app, secret),
            0,
            "`{secret}` is in the database"
        );
    }
    // What was replaced cannot be had back: nothing stores the table.
    assert_eq!(
        stored_occurrences(app, "<secret:"),
        1,
        "the body, and only it"
    );
}

#[tokio::test]
async fn text_a_person_hands_in_is_redacted_on_arrival_and_labelled_operator_supplied() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let session = open_session(&lab, "SW1").await;
    let selection = "SW1(config)#line vty 0 4\n\
                     SW1(config-line)#password Pla1nVtyPassw0rd\n\
                     SW1(config-line)#end";

    let item = call(
        app,
        "evidence.add_text",
        json!({
            "text": selection,
            "title": "what I typed on the vty lines",
            "session_id": session.id,
            "note": "before the reload",
        }),
    )
    .await;
    assert_eq!(item["kind"], "text");
    assert_eq!(
        item["source"],
        json!({
            "from": "operator",
            "session_id": session.id,
            "session_title": "SW1",
            "device_id": lab.id("SW1"),
            "device": "SW1",
            "peer": "lab:SW1",
            "lab": true,
            "note": "text as sent by the client, redacted on arrival; said to be from session `SW1`; the operator's note: before the reload",
        }),
        "it does not claim to be a capture"
    );
    assert_eq!(
        item["body"]["text"],
        "SW1(config)#line vty 0 4\nSW1(config-line)#password <secret:1>\nSW1(config-line)#end"
    );
    assert_eq!(stored_occurrences(app, "Pla1nVtyPassw0rd"), 0);
    assert_eq!(item["lines"], 3);
    assert_eq!(
        audited(app, "evidence.add_text")["detail"]["from"],
        "operator"
    );
    assert!(!audit_trail(app).contains("line vty 0 4"));

    // There is no parameter that stores it as it came.
    let raw = call(
        app,
        "evidence.add_text",
        json!({"text": "enable secret Sup3rS3cretEnable", "title": "raw please", "redact": false, "redacted": false}),
    )
    .await;
    assert_eq!(raw["redacted"], true);
    assert_eq!(raw["body"]["text"], "enable secret <secret:1>");
    assert_eq!(
        raw["source"]["lab"], false,
        "no session and no device: nothing says lab"
    );

    // A title and a note are a person's words, and are redacted like them.
    let titled = call(
        app,
        "evidence.add_text",
        json!({"text": "ok", "title": "snmp-server community C0mmunityStr1ng RO", "note": "enable secret Sup3rS3cretEnable"}),
    )
    .await;
    assert_eq!(titled["title"], "snmp-server community <secret:1> RO");
    assert_eq!(stored_occurrences(app, "C0mmunityStr1ng"), 0);
    assert_eq!(stored_occurrences(app, "Sup3rS3cretEnable"), 0);

    // What it is about has to exist, and to agree with the session named.
    assert_eq!(
        refused(
            app,
            "evidence.add_text",
            json!({"text": "x", "title": "t", "device_id": "nope"})
        )
        .await
        .code,
        "not_found"
    );
    assert_eq!(
        refused(
            app,
            "evidence.add_text",
            json!({"text": "  \n", "title": "t"})
        )
        .await
        .code,
        "invalid"
    );
    assert_eq!(
        refused(app, "evidence.add_text", json!({"text": "x", "title": " "}))
            .await
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn text_can_be_kept_in_a_build_without_a_terminal_and_still_says_who_supplied_it() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    let item = rpc(
        app,
        "evidence.add_text",
        json!({"text": "Gi0/1 is down", "title": "from my notes", "session_id": "s-gone", "device_id": lab.id("SW1")}),
    )
    .await;
    assert_eq!(
        item["source"],
        json!({
            "from": "operator",
            "session_id": "s-gone",
            "device_id": lab.id("SW1"),
            "device": "SW1",
            "lab": true,
            "note": "text as sent by the client, redacted on arrival",
        }),
        "the session is the person's word; nothing about it is made up"
    );
}

#[tokio::test]
async fn without_the_feature_it_reads_from_a_capture_is_unavailable_and_says_why() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    let device = lab.id("SW1");
    for (method, params, missing) in [
        (
            "evidence.capture_terminal",
            json!({"session_id": "s1"}),
            "terminal.list",
        ),
        (
            "evidence.capture_log",
            json!({"name": "2026-11/SW1.log"}),
            "terminal.logs.list",
        ),
        ("evidence.capture_topology", json!({}), "topology.export"),
        (
            "evidence.capture_chart",
            json!({"series": [{"device_id": device, "metric": "up"}], "since": 0, "until": 10}),
            "metrics.query_many",
        ),
    ] {
        let err = refused(app, method, params).await;
        assert_eq!(err.code, "unavailable", "{method}: {}", err.message);
        assert!(
            err.message
                .contains(&format!("`{missing}` is not part of this build")),
            "{method}: {}",
            err.message
        );
    }
    assert_eq!(
        rpc(app, "evidence.list", json!({})).await,
        json!({"items": []}),
        "nothing was invented in its place"
    );
}

#[tokio::test]
async fn a_feature_that_refuses_or_breaks_stores_nothing() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let session = open_session(&lab, "SW1").await;

    // No such session: the terminal's own answer, passed on.
    let err = refused(
        app,
        "evidence.capture_terminal",
        json!({"session_id": "nope"}),
    )
    .await;
    assert_eq!(err.code, "not_found");
    assert!(
        err.message.contains("terminal session `nope`"),
        "{}",
        err.message
    );

    // The terminal breaks between describing the session and reading it.
    fixture(app).fail("terminal.snapshot");
    let err = refused(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id}),
    )
    .await;
    assert_eq!(err.code, "unavailable");
    assert!(
        err.message.contains("`terminal.snapshot` failed"),
        "{}",
        err.message
    );

    // An answer that is not a snapshot is not stored as one.
    fixture(app).failing.lock().clear();
    fixture(app).garble("terminal.snapshot");
    let err = refused(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id}),
    )
    .await;
    assert_eq!(err.code, "unavailable");
    assert!(err.message.contains("cannot read"), "{}", err.message);

    // A drawing that is not a drawing.
    *fixture(app).topology_svg.lock() = "graph LR; A-->B".into();
    let err = refused(app, "evidence.capture_topology", json!({})).await;
    assert_eq!(err.code, "unavailable");

    // A device the inventory does not have cannot be named or labelled.
    let err = refused(
        app,
        "evidence.capture_chart",
        json!({"series": [{"device_id": "gone", "metric": "up"}], "since": 0, "until": 10}),
    )
    .await;
    assert_eq!(err.code, "not_found");

    assert_eq!(
        rpc(app, "evidence.list", json!({})).await["items"],
        json!([])
    );
}

// ------------------------------------------------------------------- limits

#[tokio::test]
async fn past_a_limit_a_call_is_refused_with_what_to_do_and_nothing_is_cut_to_fit() {
    const TEXT: usize = 256 * 1024;
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let session = open_session(&lab, "SW1").await;

    // Exactly the limit is kept whole.
    let line = "interface GigabitEthernet0/1 is up, line protocol is up\n";
    let mut most = line.repeat(TEXT / line.len());
    most.push_str(&"x".repeat(TEXT - most.len()));
    let kept = rpc(
        app,
        "evidence.add_text",
        json!({"text": most, "title": "the most"}),
    )
    .await;
    assert_eq!(kept["bytes"], TEXT);
    assert_eq!(kept["body"]["text"].as_str().unwrap().len(), TEXT);

    // One byte more is refused, and the refusal says what to do.
    let err = refused(
        app,
        "evidence.add_text",
        json!({"text": format!("{most}x"), "title": "too much"}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("at most 256 KiB"), "{}", err.message);
    assert!(
        err.message.contains("keep a smaller selection"),
        "{}",
        err.message
    );
    assert!(
        err.message.contains("Nothing was stored"),
        "{}",
        err.message
    );
    assert_eq!(
        err.details,
        Some(json!({"limit": "text_bytes", "max": TEXT, "actual": TEXT + 1}))
    );

    // More lines than the terminal gives of a session.
    let err = refused(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id, "lines": 10_001}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("evidence.capture_log"),
        "{}",
        err.message
    );
    assert_eq!(
        fixture(app).called("terminal.snapshot"),
        0,
        "it was not even asked"
    );

    // A drawing over 2 MiB.
    *fixture(app).topology_svg.lock() = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\">{}</svg>",
        "<circle r=\"1\"/>".repeat(2 * 1024 * 1024 / 15)
    );
    let err = refused(app, "evidence.capture_topology", json!({})).await;
    assert_eq!(err.code, "invalid");
    assert_eq!(err.details.as_ref().unwrap()["limit"], "svg_bytes");
    assert!(
        err.message.contains("narrow it with a `filter`"),
        "{}",
        err.message
    );

    // More than 20,000 points.
    let points: Vec<Value> = (0..20_001).map(|at| json!([at, 1.0])).collect();
    *fixture(app).metrics_answer.lock() = Some(json!([{"points": points}]));
    let err = refused(
        app,
        "evidence.capture_chart",
        json!({"series": [{"device_id": lab.id("SW1"), "metric": "up"}], "since": 0, "until": 20_001}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert_eq!(
        err.details,
        Some(json!({"limit": "points", "max": 20_000, "actual": 20_001}))
    );
    assert!(err.message.contains("larger `step_s`"), "{}", err.message);

    // A log read larger than an item.
    let err = refused(
        app,
        "evidence.capture_log",
        json!({"name": "2026-11/SW1.log", "limit": TEXT + 1}),
    )
    .await;
    assert_eq!(err.code, "invalid");

    // Titles are titles.
    let err = refused(
        app,
        "evidence.add_text",
        json!({"text": "x", "title": "t".repeat(201)}),
    )
    .await;
    assert_eq!(err.code, "invalid");

    let listed = rpc(app, "evidence.list", json!({})).await;
    assert_eq!(
        listed["items"].as_array().unwrap().len(),
        1,
        "only the one that fitted: {listed}"
    );
}

#[tokio::test]
async fn the_five_thousand_and_first_item_is_refused() {
    use nocturne_ops::records::{self, Kind, Record};
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    // 4,999 placed straight into the store, to get to the edge quickly; the
    // last one that fits and the one that does not go through the method.
    for index in 0..4_999 {
        records::save(
            app.store(),
            Kind::Evidence,
            &Record {
                id: format!("filler-{index}"),
                sort: index,
                facet: "text".into(),
                head: "{}".into(),
                body: "{}".into(),
                ..Record::default()
            },
        )
        .unwrap();
    }
    rpc(
        app,
        "evidence.add_text",
        json!({"text": "x", "title": "the last that fits"}),
    )
    .await;
    let err = refused(
        app,
        "evidence.add_text",
        json!({"text": "x", "title": "one too many"}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert_eq!(
        err.details,
        Some(json!({"limit": "items", "max": 5_000, "actual": 5_000}))
    );
    assert!(err.message.contains("evidence.delete"), "{}", err.message);
    assert_eq!(stored_occurrences(app, "one too many"), 0);
}

// ------------------------------------------------------ drawings and charts

/// Written by hand for this test (synthetic): the shape of what
/// `nocturne-topology`'s SVG export returns, not a recording of it.
const DRAWING: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"400\" height=\"200\" viewBox=\"0 0 400 200\">\n\
    <rect width=\"400\" height=\"200\" fill=\"#fff\"/>\n\
    <g class=\"node lab\"><title>SW1 (lab) &amp; friends</title><circle cx=\"50\" cy=\"50\" r=\"12\"/><text x=\"50\" y=\"80\">SW1</text></g>\n\
    <g class=\"link\"><title>snmp-server community V4ult-Kept-Passw0rd RO</title><line x1=\"50\" y1=\"50\" x2=\"200\" y2=\"50\"/></g>\n\
    </svg>";

#[tokio::test]
async fn a_topology_capture_keeps_the_drawing_with_its_secrets_hidden_and_its_markup_whole() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    rpc(
        app,
        "credentials.create",
        json!({"name": "core", "username": "ops", "snmp_version": "v2c", "snmp_community": "V4ult-Kept-Passw0rd"}),
    )
    .await;
    *fixture(app).topology_svg.lock() = DRAWING.into();

    let item = call(
        app,
        "evidence.capture_topology",
        json!({"view": "default", "filter": {"site": "Lab"}, "include_endpoints": false, "title": "the lab before the change"}),
    )
    .await;
    assert_eq!(item["kind"], "topology");
    assert_eq!(item["body"]["type"], "svg");
    assert!(
        item.get("lines").is_none(),
        "a drawing has no lines: {item}"
    );
    assert_eq!(
        item["source"],
        json!({
            "from": "topology.export",
            "lab": true,
            "params": {"view": "default", "filter": {"site": "Lab"}, "include_endpoints": false},
        })
    );
    assert_eq!(
        fixture(app).params("topology.export").unwrap(),
        json!({"view": "default", "filter": {"site": "Lab"}, "include_endpoints": false, "format": "svg"}),
        "what was not said is left to the topology's own defaults"
    );
    let svg = item["body"]["svg"].as_str().unwrap();
    assert_eq!(
        svg,
        DRAWING.replace("V4ult-Kept-Passw0rd", "&lt;secret:1&gt;"),
        "the drawing, tag for tag, with the one secret replaced by text"
    );
    assert_eq!(item["bytes"], svg.len());
    assert_eq!(item["sha256"], sha256(svg));
    assert_eq!(stored_occurrences(app, "V4ult-Kept-Passw0rd"), 0);
    assert_eq!(
        audited(app, "evidence.capture_topology")["detail"]["bytes"],
        svg.len()
    );
}

#[tokio::test]
async fn a_chart_keeps_the_points_and_the_holes_exactly_as_they_came() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let (one, two) = (lab.id("SW1"), lab.id("SW2"));
    // A device that is not simulated, to tell the lab label apart.
    let hardware = rpc(
        app,
        "devices.create",
        json!({"name": "CORE1", "address": "192.0.2.1", "site": "HQ"}),
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    // On a minute boundary, where the store's one-minute buckets start.
    let start = hours_ago(1) / 60_000 * 60_000;
    // Two polls answered, three missed altogether, two failed, two answered.
    reachability(app, &one, start, "11...0011");
    reachability(app, &hardware, start, "1");
    let asked = json!({
        "series": [{"device_id": one, "metric": "up"}, {"device_id": two, "metric": "up"}],
        "since": start,
        "until": start + 600_000,
        "step_s": 60,
    });
    let source = rpc(app, "metrics.query_many", asked.clone()).await;

    let item = call(app, "evidence.capture_chart", asked.clone()).await;
    assert_eq!(item["kind"], "chart");
    assert_eq!(item["body"]["type"], "chart");
    assert_eq!(
        item["body"]["series"], source,
        "the series are what the monitor answered, nothing added and nothing filled in"
    );
    let points = item["body"]["series"][0]["points"].as_array().unwrap();
    assert_eq!(
        points.len(),
        6,
        "six polls were recorded; the three missed ones are not points"
    );
    assert_eq!(
        item["body"]["coverage"],
        json!({
            "since": start,
            "until": start + 600_000,
            "points": 6,
            "series": [
                {
                    "device_id": one, "device": "SW1", "lab": true, "metric": "up", "instance": "",
                    "points": 6, "first_at": start, "last_at": start + 8 * 60_000,
                    "longest_gap_ms": 4 * 60_000, "step_s": 60, "rollup": "raw",
                },
                // A series nothing was ever recorded for is there, and empty.
                {
                    "device_id": two, "device": "SW2", "lab": true, "metric": "up", "instance": "",
                    "points": 0, "step_s": 60, "rollup": "raw",
                },
            ],
        })
    );
    assert_eq!(item["title"], "up on 2 devices");
    assert_eq!(item["source"]["from"], "metrics.query_many");
    assert_eq!(item["source"]["params"], asked);
    assert_eq!(item["source"]["lab"], true);
    assert!(
        item["source"].get("device_id").is_none(),
        "two devices, so not one"
    );
    assert_eq!(
        audited(app, "evidence.capture_chart")["detail"]["points"],
        6
    );

    // It is found under each device it shows.
    for device in [&one, &two] {
        let found = rpc(
            app,
            "evidence.list",
            json!({"device_id": device, "kind": "chart"}),
        )
        .await;
        assert_eq!(found["items"][0]["id"], item["id"]);
    }

    // One series of a real device: named, and not labelled lab.
    let single = rpc(
        app,
        "evidence.capture_chart",
        json!({"series": [{"device_id": hardware, "metric": "up"}], "since": start, "until": start + 600_000}),
    )
    .await;
    assert_eq!(single["title"], "up on CORE1");
    assert_eq!(single["source"]["device_id"], hardware);
    assert_eq!(single["source"]["device"], "CORE1");
    assert_eq!(single["source"]["lab"], false);

    assert_eq!(
        refused(
            app,
            "evidence.capture_chart",
            json!({"series": [{"device_id": one, "metric": "up"}], "since": 10, "until": 10})
        )
        .await
        .code,
        "invalid"
    );
}

#[tokio::test]
async fn a_piece_of_a_session_log_is_read_through_the_terminal_with_redaction_on() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    // A log as the terminal writes one: a timestamp in front of each line
    // of what the fake device printed in a real session.
    let session = open_session(&lab, "SW1").await;
    session.enter("configure terminal").await;
    session.enter("line vty 0 4").await;
    session.enter("password Pla1nVtyPassw0rd").await;
    session.enter("end").await;
    session.enter("show clock").await;
    let log: String = session
        .lines()
        .iter()
        .enumerate()
        .map(|(index, line)| format!("2026-11-13T05:24:{:02}.000+02:00 {line}\n", index % 60))
        .collect();
    let name = "2026-11/SW1-20261113-052400.log";
    fixture(app).session_logs.lock().push((
        json!({
            "name": name, "device_id": lab.id("SW1"), "title": "SW1", "kind": "sim",
            "started_at": 1, "ended_at": 2, "size": log.len(), "active": false,
        }),
        log.clone(),
    ));

    let item = call(app, "evidence.capture_log", json!({"name": name})).await;
    assert_eq!(item["kind"], "log");
    assert_eq!(
        fixture(app).params("terminal.logs.read").unwrap(),
        json!({"name": name, "offset": 0, "limit": 262_144, "redact": true})
    );
    assert_eq!(
        item["source"],
        json!({
            "from": "terminal.logs.read",
            "session_title": "SW1",
            "device_id": lab.id("SW1"),
            "device": "SW1",
            "lab": true,
            "params": {"name": name, "offset": 0, "limit": 262_144},
            "note": format!("bytes 0 to {0} of {0}, the end of the log", log.len()),
        })
    );
    let text = item["body"]["text"].as_str().unwrap();
    assert!(
        text.contains("+02:00 SW1(config-line)#password <secret:1>\n"),
        "{text}"
    );
    assert!(text.contains("+02:00 12:00:00.000 UTC Fri Nov 13 2026\n"));
    assert_eq!(stored_occurrences(app, "Pla1nVtyPassw0rd"), 0);
    assert_eq!(item["lines"], log.lines().count());

    // A piece of it: from an offset, a limit's worth, ending on a line.
    let offset = log.find("SW1(config)#line").unwrap() - 30;
    let piece = call(
        app,
        "evidence.capture_log",
        json!({"name": name, "offset": offset, "limit": 130, "title": "the vty lines"}),
    )
    .await;
    assert_eq!(piece["lines"], 2);
    assert!(
        piece["source"]["note"]
            .as_str()
            .unwrap()
            .starts_with(&format!("bytes {offset} to ")),
        "{piece}"
    );
    assert!(
        !piece["source"]["note"]
            .as_str()
            .unwrap()
            .contains("end of the log")
    );

    let err = refused(
        app,
        "evidence.capture_log",
        json!({"name": "2026-11/other.log"}),
    )
    .await;
    assert_eq!(err.code, "not_found");
}

// ------------------------------------------------------- list, rename, page

#[tokio::test]
async fn items_are_listed_newest_first_filtered_and_paged_without_their_text() {
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = &lab.app;
    let mut ids = Vec::new();
    for (title, device) in [
        ("first", "SW1"),
        ("second on the uplink", "SW2"),
        ("third", "SW1"),
    ] {
        let item = rpc(
            app,
            "evidence.add_text",
            json!({"text": format!("output of {title}"), "title": title, "device_id": lab.id(device)}),
        )
        .await;
        ids.push(item["id"].as_str().unwrap().to_string());
        // Two items in the same millisecond would still page correctly; this
        // only makes "newest first" something the test can state.
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    }

    let page = call(app, "evidence.list", json!({"limit": 2})).await;
    let titles = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["title"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(titles(&page), ["third", "second on the uplink"]);
    assert!(page["items"][0].get("body").is_none());
    let rest = rpc(
        app,
        "evidence.list",
        json!({"limit": 2, "cursor": page["next_cursor"]}),
    )
    .await;
    assert_eq!(titles(&rest), ["first"]);
    assert!(rest.get("next_cursor").is_none());

    let on_one = rpc(app, "evidence.list", json!({"device_id": lab.id("SW1")})).await;
    assert_eq!(titles(&on_one), ["third", "first"]);
    let found = rpc(app, "evidence.list", json!({"query": "UPLINK sw2"})).await;
    assert_eq!(titles(&found), ["second on the uplink"]);
    // The content is not what a list searches.
    let by_content = rpc(app, "evidence.list", json!({"query": "output"})).await;
    assert_eq!(titles(&by_content), Vec::<String>::new());
    let charts = rpc(app, "evidence.list", json!({"kind": "chart"})).await;
    assert_eq!(charts, json!({"items": []}));

    let renamed = call(
        app,
        "evidence.rename",
        json!({"id": ids[0], "title": "  the first one,\nrenamed "}),
    )
    .await;
    assert_eq!(renamed["title"], "the first one, renamed");
    assert!(renamed.get("body").is_none());
    assert_eq!(
        audited(app, "evidence.rename")["message"],
        "Renamed evidence `first` to `the first one, renamed`"
    );
    assert_eq!(
        rpc(app, "evidence.get", json!({"id": ids[0]})).await["body"]["text"],
        "output of first",
        "a new title changes nothing about the content"
    );
    assert_eq!(
        refused(app, "evidence.rename", json!({"id": "nope", "title": "x"}))
            .await
            .code,
        "not_found"
    );
    assert_eq!(
        refused(app, "evidence.get", json!({"id": "nope"}))
            .await
            .code,
        "not_found"
    );
}

// ---------------------------------------------------------------- incidents

#[tokio::test]
async fn attaching_puts_a_reference_on_the_timeline_at_the_capture_time_and_never_the_text() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let session = session_with_output(&lab).await;
    let incident_id = incident(app, &lab.id("SW1")).await;
    let item = rpc(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id, "title": "clock and version"}),
    )
    .await;
    let id = item["id"].as_str().unwrap();
    let mut events = app.subscribe();

    let attached = call(
        app,
        "evidence.attach",
        json!({"id": id, "incident_id": incident_id, "author": "mo"}),
    )
    .await;
    assert_eq!(attached["incident_ids"], json!([incident_id]));
    assert!(attached.get("body").is_none());

    let incident = call(app, "incidents.get", json!({"id": incident_id})).await;
    let on_timeline = entry(&incident, &format!("evidence:{id}")).expect("the entry");
    assert_eq!(
        on_timeline,
        &json!({
            "id": format!("evidence:{id}"),
            "ts": item["captured_at"],
            "kind": "evidence",
            "text": "clock and version",
            "ref": {"type": "evidence", "id": id},
            "author": "mo",
            "source": {
                "from": "terminal.snapshot",
                "evidence": format!(
                    "evidence `{id}`: terminal session `SW1` (lab), {} lines, redacted, sha256 {}",
                    item["lines"], item["sha256"].as_str().unwrap()
                ),
                "gathered_at": on_timeline["source"]["gathered_at"],
            },
        })
    );
    // The incident holds the reference. The text is in the item and nowhere
    // else: not in the incident's record, not in what its search reads.
    assert!(!incident.to_string().contains("12:00:00.000 UTC"));
    assert_eq!(stored_occurrences(app, "12:00:00.000 UTC"), 1);

    let topics: Vec<String> = drain(&mut events)
        .into_iter()
        .map(|event| event.topic)
        .filter(|topic| topic != "audit.appended")
        .collect();
    assert_eq!(topics, ["evidence.changed", "incidents.changed"]);
    assert_eq!(
        audited(app, "evidence.attach")["detail"],
        json!({
            "evidence_id": id, "incident_id": incident_id, "kind": "terminal",
            "sha256": item["sha256"], "by": "mo",
        })
    );
    let found = rpc(app, "evidence.list", json!({"incident_id": incident_id})).await;
    assert_eq!(found["items"][0]["id"], id);

    // Attaching again is the same state, not a second entry or a second
    // line in the audit trail.
    rpc(
        app,
        "evidence.attach",
        json!({"id": id, "incident_id": incident_id}),
    )
    .await;
    let again = rpc(app, "incidents.get", json!({"id": incident_id})).await;
    assert_eq!(again["timeline"], incident["timeline"]);
    assert_eq!(audit_trail(app).matches("evidence.attach |").count(), 1);

    // The timeline shows the title, so it follows a rename.
    rpc(
        app,
        "evidence.rename",
        json!({"id": id, "title": "what SW1 said"}),
    )
    .await;
    let renamed = rpc(app, "incidents.get", json!({"id": incident_id})).await;
    assert_eq!(
        entry_text(&renamed, &format!("evidence:{id}")),
        Some("what SW1 said")
    );

    // An incident that does not exist changes nothing.
    let err = refused(
        app,
        "evidence.attach",
        json!({"id": id, "incident_id": "nope"}),
    )
    .await;
    assert_eq!(err.code, "not_found");
    assert_eq!(
        rpc(app, "evidence.get", json!({"id": id})).await["incident_ids"],
        json!([incident_id])
    );

    let detached = call(
        app,
        "evidence.detach",
        json!({"id": id, "incident_id": incident_id}),
    )
    .await;
    assert_eq!(detached["incident_ids"], json!([]));
    let after = rpc(app, "incidents.get", json!({"id": incident_id})).await;
    assert!(entry(&after, &format!("evidence:{id}")).is_none());
    assert_eq!(
        rpc(app, "evidence.get", json!({"id": id})).await["body"],
        item["body"],
        "the item itself is untouched"
    );
}

fn entry_text<'a>(incident: &'a Value, id: &str) -> Option<&'a str> {
    entry(incident, id).and_then(|entry| entry["text"].as_str())
}

/// The transaction rule, for each method that asks other features what
/// happened before it writes: `alerts.get` is made to wait, and while it
/// waits a note is written and an item attached. With a read-then-write
/// that is not one step, the method would finish by writing back the
/// incident as it was before either.
#[tokio::test]
async fn a_note_and_an_attachment_made_while_a_method_is_gathering_are_still_there_afterwards() {
    for (method, params) in [
        ("incidents.link", json!({"device_ids": ["extra-device"]})),
        ("incidents.sync", json!({})),
        (
            "incidents.update",
            json!({"summary": "the uplink to CORE1"}),
        ),
    ] {
        let lab = lab(&["SW1"], With::all()).await;
        let app = lab.app.clone();
        fixture(&app)
            .alerts
            .lock()
            .push(common::alert("a1", &lab.id("SW1"), "high", hours_ago(1)));
        let incident_id = rpc(
            &app,
            "incidents.create",
            json!({"from_alert": "a1", "author": "mo"}),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_string();
        let item = rpc(
            &app,
            "evidence.add_text",
            json!({"text": "Gi0/1 down", "title": "the log line"}),
        )
        .await;
        let evidence_id = item["id"].as_str().unwrap().to_string();

        let calls_before = fixture(&app).called("alerts.get");
        fixture(&app).hold("alerts.get");
        let mut params = params.clone();
        params["id"] = json!(incident_id);
        let gathering = tokio::spawn({
            let app = app.clone();
            async move { app.rpc(method, params).await }
        });
        // The method has read the incident and is waiting for the alert.
        while fixture(&app).called("alerts.get") == calls_before {
            tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        }

        rpc(
            &app,
            "incidents.note",
            json!({"id": incident_id, "text": "written during the gathering", "author": "zahra"}),
        )
        .await;
        rpc(
            &app,
            "evidence.attach",
            json!({"id": evidence_id, "incident_id": incident_id}),
        )
        .await;

        fixture(&app).release("alerts.get");
        gathering
            .await
            .expect("the call ran")
            .unwrap_or_else(|err| panic!("`{method}` failed: {}", err.message));

        let incident = rpc(&app, "incidents.get", json!({"id": incident_id})).await;
        let texts: Vec<&str> = incident["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["text"].as_str().unwrap())
            .collect();
        assert!(
            texts.contains(&"written during the gathering"),
            "`{method}` wrote a stale copy over the note: {texts:?}"
        );
        assert!(
            entry(&incident, &format!("evidence:{evidence_id}")).is_some(),
            "`{method}` wrote a stale copy over the attachment: {texts:?}"
        );
        // And the method's own change is there too.
        match method {
            "incidents.link" => assert_eq!(incident["device_ids"][1], "extra-device"),
            "incidents.update" => assert_eq!(incident["summary"], "the uplink to CORE1"),
            _ => {}
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notes_and_attachments_from_many_callers_at_once_all_land() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = lab.app.clone();
    let incident_id = incident(&app, &lab.id("SW1")).await;
    let mut evidence_ids = Vec::new();
    for index in 0..12 {
        let item = rpc(
            &app,
            "evidence.add_text",
            json!({"text": format!("line {index}"), "title": format!("item {index}")}),
        )
        .await;
        evidence_ids.push(item["id"].as_str().unwrap().to_string());
    }

    let mut calls = Vec::new();
    for (index, evidence_id) in evidence_ids.iter().enumerate() {
        let (attach, note) = (app.clone(), app.clone());
        let (evidence_id, incident_id) = (evidence_id.clone(), incident_id.clone());
        let for_note = incident_id.clone();
        calls.push(tokio::spawn(async move {
            attach
                .rpc(
                    "evidence.attach",
                    json!({"id": evidence_id, "incident_id": incident_id}),
                )
                .await
                .map(|_| ())
        }));
        calls.push(tokio::spawn(async move {
            note.rpc(
                "incidents.note",
                json!({"id": for_note, "text": format!("note {index}")}),
            )
            .await
            .map(|_| ())
        }));
    }
    for call in calls {
        call.await.expect("the call ran").expect("and succeeded");
    }

    let incident = rpc(&app, "incidents.get", json!({"id": incident_id})).await;
    let kinds = |kind: &str| {
        incident["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["kind"] == json!(kind))
            .count()
    };
    assert_eq!((kinds("evidence"), kinds("note")), (12, 12));
    let listed = rpc(&app, "evidence.list", json!({"incident_id": incident_id})).await;
    assert_eq!(listed["items"].as_array().unwrap().len(), 12);
}

#[tokio::test]
async fn deleting_evidence_an_incident_refers_to_leaves_a_tombstone_not_a_hole() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let session = session_with_output(&lab).await;
    let incident_id = incident(app, &lab.id("SW1")).await;
    let other_incident = incident(app, &lab.id("SW1")).await;
    let item = rpc(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id, "title": "clock and version"}),
    )
    .await;
    let id = item["id"].as_str().unwrap();
    for incident in [&incident_id, &other_incident] {
        rpc(
            app,
            "evidence.attach",
            json!({"id": id, "incident_id": incident}),
        )
        .await;
    }
    // One of the two incidents is deleted first: the item forgets it.
    rpc(app, "incidents.delete", json!({"id": other_incident})).await;
    assert_eq!(
        rpc(app, "evidence.get", json!({"id": id})).await["incident_ids"],
        json!([incident_id])
    );
    let mut events = app.subscribe();

    let deleted = call(app, "evidence.delete", json!({"id": id, "author": "zahra"})).await;
    assert_eq!(
        deleted,
        json!({"deleted": true, "tombstones": [incident_id]})
    );

    let incident = call(app, "incidents.get", json!({"id": incident_id})).await;
    let tombstone = entry(&incident, &format!("evidence:{id}")).expect("the entry is still there");
    let today = chrono::Utc::now().format("%Y-%m-%d");
    assert_eq!(
        tombstone,
        &json!({
            "id": format!("evidence:{id}"),
            "ts": item["captured_at"],
            "kind": "evidence",
            "text": format!("clock and version — evidence deleted on {today} (UTC)"),
            "author": "zahra",
            "source": {
                "from": "evidence.delete",
                "evidence": format!(
                    "evidence `{id}` was deleted; it was terminal session `SW1` (lab), {} lines, redacted, sha256 {}",
                    item["lines"], item["sha256"].as_str().unwrap()
                ),
                "gathered_at": tombstone["source"]["gathered_at"],
            },
        }),
        "in its place, at its time, pointing at nothing"
    );

    // The item and its text are gone.
    assert_eq!(
        refused(app, "evidence.get", json!({"id": id})).await.code,
        "not_found"
    );
    assert_eq!(stored_occurrences(app, "12:00:00.000 UTC"), 0);
    assert_eq!(
        rpc(app, "evidence.list", json!({})).await["items"],
        json!([])
    );
    let seen = drain(&mut events);
    assert!(seen.iter().any(|event| {
        event.topic == "evidence.changed" && event.payload == json!({"id": id, "deleted": true})
    }));
    assert!(seen.iter().any(|event| event.topic == "incidents.changed"));
    assert_eq!(
        audited(app, "evidence.delete")["detail"],
        json!({
            "evidence_id": id, "kind": "terminal", "lines": item["lines"], "bytes": item["bytes"],
            "sha256": item["sha256"], "tombstones": [incident_id], "by": "zahra",
        })
    );

    // The review shows the tombstone in its timeline and has no appendix
    // entry for an item that is not there.
    let review = rpc(
        app,
        "incidents.export",
        json!({"id": incident_id, "format": "markdown", "utc_offset_minutes": 0}),
    )
    .await;
    let content = review["content"].as_str().unwrap();
    assert!(content.contains("evidence deleted on"), "{content}");
    assert!(
        content.trim_end().ends_with("Nothing attached."),
        "{content}"
    );

    assert_eq!(
        refused(app, "evidence.delete", json!({"id": id}))
            .await
            .code,
        "not_found"
    );
}

/// What whoever holds a device's console can make it print.
const HOSTILE: &str = "</pre><script>alert(1)</script><img src=x onerror=alert(2)>\n\
    ```\n\
    # Root cause: the operator\n\
    [click](javascript:alert(3)) | cell | another\n\
    =HYPERLINK(\"http://evil.example\",\"open\")\n\
    end of banner";

#[tokio::test]
async fn the_review_carries_the_evidence_and_hostile_device_output_is_inert_in_every_format() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    lab.fake("SW1").with(|state| {
        state
            .outputs
            .insert("show banner motd".into(), HOSTILE.into())
    });
    let session = open_session(&lab, "SW1").await;
    session.enter("show banner motd").await;
    let incident_id = incident(app, &lab.id("SW1")).await;
    let item = rpc(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id, "lines": 8, "title": "the banner <b>SW1</b> shows"}),
    )
    .await;
    let id = item["id"].as_str().unwrap();
    let stored = item["body"]["text"].as_str().unwrap();
    assert_eq!(stored, format!("SW1#show banner motd\n{HOSTILE}\nSW1#"));
    rpc(
        app,
        "evidence.attach",
        json!({"id": id, "incident_id": incident_id, "author": "mo"}),
    )
    .await;

    let export = |format: &'static str| {
        let incident_id = incident_id.clone();
        async move {
            call(
                app,
                "incidents.export",
                json!({"id": incident_id, "format": format, "utc_offset_minutes": 0}),
            )
            .await["content"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };

    // ---- HTML: one preformatted block, and nothing in it is markup.
    let html = export("html").await;
    assert!(
        !html.contains("<script") && !html.contains("<img src=x"),
        "{html}"
    );
    assert_eq!(
        (
            html.matches("<pre>").count(),
            html.matches("</pre>").count()
        ),
        (1, 1)
    );
    assert!(html.contains("&lt;/pre&gt;&lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(html.contains("<h3>the banner &lt;b&gt;SW1&lt;/b&gt; shows</h3>"));
    for fact in [
        format!("<dt>Evidence</dt><dd>{id}</dd>"),
        "<dt>Kind</dt><dd>terminal</dd>".to_string(),
        "<dt>Source</dt><dd>terminal.snapshot</dd>".to_string(),
        "<dt>Session</dt><dd>SW1</dd>".to_string(),
        "<dt>Device</dt><dd>SW1</dd>".to_string(),
        "<dt>Lab</dt><dd>yes — a simulated device</dd>".to_string(),
        "<dt>Lines</dt><dd>8</dd>".to_string(),
        format!("<dt>SHA-256</dt><dd>{}</dd>", sha256(stored)),
    ] {
        assert!(html.contains(&fact), "missing `{fact}` in {html}");
    }
    let captured = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
        item["captured_at"].as_i64().unwrap(),
    )
    .unwrap()
    .format("%Y-%m-%dT%H:%M:%S+00:00")
    .to_string();
    assert!(
        html.contains(&format!("<dt>Captured</dt><dd>{captured}</dd>")),
        "{html}"
    );

    // ---- Markdown: fenced with more backticks than the output has.
    let markdown = export("markdown").await;
    let lines: Vec<&str> = markdown.lines().collect();
    let open = lines
        .iter()
        .position(|line| *line == "````text")
        .expect("the fence");
    let close = lines
        .iter()
        .rposition(|line| *line == "````")
        .expect("its end");
    assert_eq!(lines[open + 1..close].join("\n"), stored);
    assert!(
        lines[..open].iter().chain(&lines[close + 1..]).all(|line| {
            !line.starts_with("# Root cause")
                && !line.contains("<script>")
                && !line.contains("](javascript")
        }),
        "{markdown}"
    );
    assert!(markdown.contains(r"### the banner \<b\>SW1\</b\> shows"));
    assert!(markdown.contains(&format!("- **SHA-256:** {}", sha256(stored))));
    // The timeline row is the reference, not the text.
    assert!(
        markdown.contains(
            r"| evidence | the banner \<b\>SW1\</b\> shows | mo | terminal.snapshot — evidence"
        ),
        "{markdown}"
    );

    // ---- CSV: a line to a row, in one cell, and no formula.
    let csv_text = export("csv").await;
    let rows: Vec<Vec<String>> = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(csv_text.as_bytes())
        .records()
        .map(|record| record.unwrap().iter().map(str::to_string).collect())
        .collect();
    let first = rows
        .iter()
        .position(|row| row == &["SW1#show banner motd"])
        .expect("the output starts on a row of its own");
    let output: Vec<&Vec<String>> = rows[first..first + 8].iter().collect();
    assert!(output.iter().all(|row| row.len() == 1), "{output:?}");
    assert_eq!(
        output[4],
        &["[click](javascript:alert(3)) | cell | another"]
    );
    assert_eq!(
        output[5],
        &["'=HYPERLINK(\"http://evil.example\",\"open\")"]
    );

    // ---- JSON: the stored text as data.
    let json_text = export("json").await;
    let document: Value = serde_json::from_str(&json_text).unwrap();
    let appendix = document["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["heading"] == "Appendix: evidence")
        .expect("the appendix");
    assert_eq!(appendix["kind"], "evidence");
    assert_eq!(appendix["items"][0]["text"], stored);
    assert_eq!(appendix["items"][0]["facts"]["SHA-256"], sha256(stored));
}

#[tokio::test]
async fn a_review_shows_a_drawing_as_an_image_and_a_chart_as_its_numbers() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let incident_id = incident(app, &lab.id("SW1")).await;
    *fixture(app).topology_svg.lock() =
        "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script><text>SW1</text></svg>"
            .into();
    let drawing = rpc(
        app,
        "evidence.capture_topology",
        json!({"title": "the map"}),
    )
    .await;
    let start = hours_ago(1);
    reachability(app, &lab.id("SW1"), start, "110011");
    let chart = rpc(
        app,
        "evidence.capture_chart",
        json!({"series": [{"device_id": lab.id("SW1"), "metric": "up"}], "since": start, "until": start + 600_000, "step_s": 60}),
    )
    .await;
    for item in [&drawing, &chart] {
        rpc(
            app,
            "evidence.attach",
            json!({"id": item["id"], "incident_id": incident_id}),
        )
        .await;
    }

    let html = rpc(
        app,
        "incidents.export",
        json!({"id": incident_id, "format": "html", "utc_offset_minutes": 0}),
    )
    .await["content"]
        .as_str()
        .unwrap()
        .to_string();
    // Whatever the drawing contains, in the page it is an image.
    assert!(
        !html.contains("<svg") && !html.contains("<script"),
        "{html}"
    );
    assert!(
        html.contains("<img class=\"drawing\" alt=\"the map\" src=\"data:image/svg+xml;base64,")
    );
    assert!(
        html.contains("<td>SW1</td><td>up</td><td></td><td class=\"n\">6</td>"),
        "{html}"
    );
    assert!(
        html.contains("<td>0</td><td>1</td>"),
        "lowest and highest: {html}"
    );
}

// ----------------------------------------------------------------- handover

#[tokio::test]
async fn the_handover_lists_what_was_captured_in_the_period_as_records_with_ids() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let session = session_with_output(&lab).await;
    let since = now_ms() - 1;
    let first = rpc(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id, "title": "before"}),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    let second = rpc(
        app,
        "evidence.add_text",
        json!({"text": "Gi0/1 down", "title": "after", "device_id": lab.id("SW1")}),
    )
    .await;
    // A period ends before its `until`, which is now when it is left out.
    tokio::time::sleep(std::time::Duration::from_millis(3)).await;

    let handover = call(app, "handover.generate", json!({"since": since})).await;
    let mut heads = vec![first.clone(), second.clone()];
    for head in &mut heads {
        head.as_object_mut().unwrap().remove("body");
    }
    assert_eq!(
        handover["evidence"],
        json!({"captured": heads}),
        "oldest first, each the record itself, none with its text"
    );
    assert!(!handover.to_string().contains("12:00:00.000 UTC"));

    // A shift in which nothing was captured has an empty list, not a null:
    // the source is this feature's own table and it was read.
    let earlier = rpc(
        app,
        "handover.generate",
        json!({"since": since - 3_600_000, "until": since}),
    )
    .await;
    assert_eq!(earlier["evidence"], json!({"captured": []}));
    assert!(
        earlier["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["section"] != "evidence")
    );

    let exported = rpc(
        app,
        "handover.export",
        json!({"since": since, "format": "markdown", "utc_offset_minutes": 0}),
    )
    .await;
    let content = exported["content"].as_str().unwrap();
    assert!(content.contains("## Evidence captured"), "{content}");
    for item in [&first, &second] {
        assert!(
            content.contains(&format!(
                "| {} | {} |",
                item["sha256"].as_str().unwrap(),
                item["id"].as_str().unwrap()
            )),
            "{content}"
        );
    }
    assert!(content.contains("| terminal | before | SW1 (lab) | terminal.snapshot | 0 |"));
    assert!(!content.contains("12:00:00.000 UTC"));
}

#[tokio::test]
async fn a_handover_takes_what_was_captured_at_its_first_instant_and_not_at_its_last() {
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    let mut items = Vec::new();
    for title in ["before", "first", "during", "last", "after"] {
        items.push(
            rpc(
                app,
                "evidence.add_text",
                json!({"text": "Gi0/1 down", "title": title}),
            )
            .await,
        );
        // Each a different millisecond.
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    }
    let at = |index: usize| items[index]["captured_at"].as_i64().unwrap();
    let titles = |handover: &Value| -> Vec<String> {
        handover["evidence"]["captured"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["title"].as_str().unwrap().to_string())
            .collect()
    };
    // From the instant `first` was captured to the instant `last` was:
    // the period is half open, as every period of a handover is.
    let handover = rpc(
        app,
        "handover.generate",
        json!({"since": at(1), "until": at(3)}),
    )
    .await;
    assert_eq!(titles(&handover), ["first", "during"]);
    let handover = rpc(
        app,
        "handover.generate",
        json!({"since": at(0), "until": at(4) + 1}),
    )
    .await;
    assert_eq!(
        titles(&handover),
        ["before", "first", "during", "last", "after"],
        "oldest first"
    );
    let handover = rpc(
        app,
        "handover.generate",
        json!({"since": at(4) + 1, "until": at(4) + 2}),
    )
    .await;
    assert_eq!(handover["evidence"], json!({"captured": []}));
}

// ------------------------------------------------------- the free-text caps

#[tokio::test]
async fn free_text_fields_refuse_pasted_output_and_point_at_evidence() {
    const PROSE: usize = 64 * 1024;
    let lab = lab(&["SW1"], With::nothing()).await;
    let app = &lab.app;
    let incident_id = incident(app, &lab.id("SW1")).await;
    let fits = "x".repeat(PROSE);
    let too_long = "x".repeat(PROSE + 1);

    // An incident's note.
    rpc(
        app,
        "incidents.note",
        json!({"id": incident_id, "text": fits}),
    )
    .await;
    let err = refused(
        app,
        "incidents.note",
        json!({"id": incident_id, "text": too_long}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert!(
        err.message
            .starts_with("a note on an incident can be at most 64 KiB"),
        "{}",
        err.message
    );
    assert!(
        err.message.contains("`evidence.capture_terminal`")
            && err.message.contains("`evidence.add_text`"),
        "{}",
        err.message
    );
    assert_eq!(
        err.details,
        Some(json!({
            "limit": "prose_bytes", "max": PROSE, "actual": PROSE + 1,
            "instead": ["evidence.capture_terminal", "evidence.add_text"],
        }))
    );
    let incident = rpc(app, "incidents.get", json!({"id": incident_id})).await;
    assert_eq!(
        incident["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|entry| entry["kind"] == "note")
            .count(),
        1,
        "the refused one is not on the timeline"
    );

    // An incident's own prose.
    for field in ["summary", "impact", "root_cause"] {
        let err = refused(
            app,
            "incidents.update",
            json!({"id": incident_id, field: too_long}),
        )
        .await;
        assert_eq!(err.code, "invalid", "{field}");
        assert!(err.message.contains("evidence"), "{field}: {}", err.message);
    }
    let err = refused(
        app,
        "incidents.create",
        json!({"title": "t", "summary": too_long}),
    )
    .await;
    assert_eq!(err.code, "invalid");

    // A note's body.
    rpc(app, "notes.save", json!({"title": "runbook", "body": fits})).await;
    let err = refused(
        app,
        "notes.save",
        json!({"title": "runbook", "body": too_long}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert!(
        err.message
            .starts_with("a note's body can be at most 64 KiB"),
        "{}",
        err.message
    );
    assert_eq!(
        rpc(app, "notes.list", json!({})).await["notes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // A handover's notes, saved or only exported.
    let since = hours_ago(1);
    rpc(app, "handover.save", json!({"since": since, "notes": fits})).await;
    for method in ["handover.save", "handover.export"] {
        let err = refused(app, method, json!({"since": since, "notes": too_long})).await;
        assert_eq!(err.code, "invalid", "{method}");
        assert!(
            err.message
                .starts_with("a handover's notes can be at most 64 KiB"),
            "{method}: {}",
            err.message
        );
    }
    assert_eq!(
        rpc(app, "handover.list", json!({})).await["handovers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

// ------------------------------------------------- nothing gets out anywhere

/// Not a key: lines of the base64 alphabet, as long as a key's (synthetic,
/// written for these tests).
const KEY_LINES: [&str; 3] = [
    "MIIEowIBAAKCAQEAx1Yd0Zc3kq7v9JmM2bT4n8sQe5uWfR6hLpA3cXyZ0aBdEfGh",
    "IjKlMnOpQrStUvWxYz0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJ",
    "KLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKL",
];
const ENABLE_PLAIN: &str = "Sup3rS3cretEnable";
/// In the shape of an MD5-crypt hash (synthetic).
const ENABLE_HASH: &str = "$1$n0ct$NocturneTestHashValue0";
const COMMUNITY: &str = "C0mmunityStr1ng";
/// In the shape of type-7 passwords (synthetic).
const TYPE_7: &str = "0822455D0A16544541";
const VTY_TYPE_7: &str = "094F471A1A0A464058";

fn every_secret() -> [&'static str; 8] {
    [
        ENABLE_PLAIN,
        ENABLE_HASH,
        COMMUNITY,
        TYPE_7,
        VTY_TYPE_7,
        KEY_LINES[0],
        KEY_LINES[1],
        KEY_LINES[2],
    ]
}

fn key_block() -> String {
    format!(
        "-----BEGIN RSA PRIVATE KEY-----\n{}\n{}\n{}\n-----END RSA PRIVATE KEY-----",
        KEY_LINES[0], KEY_LINES[1], KEY_LINES[2]
    )
}

/// A configuration with one of each kind of secret in it (synthetic).
fn secret_config() -> String {
    format!(
        "enable secret 5 {ENABLE_HASH}\n\
         enable secret {ENABLE_PLAIN}\n\
         username admin privilege 15 password 7 {TYPE_7}\n\
         snmp-server community {COMMUNITY} RO\n\
         line vty 0 4\n password 7 {VTY_TYPE_7}\n{}",
        key_block()
    )
}

/// Every value of every table of the database, as text: what a copy of the
/// file would give away.
fn whole_database(app: &App) -> String {
    use rusqlite::types::ValueRef;
    app.store()
        .with(|conn| {
            let tables: Vec<String> = conn
                .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?;
            let mut out = String::new();
            for table in tables {
                let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\""))?;
                let columns = statement.column_count();
                let mut rows = statement.query([])?;
                while let Some(row) = rows.next()? {
                    out.push_str(&table);
                    for index in 0..columns {
                        out.push_str(" | ");
                        match row.get_ref(index)? {
                            ValueRef::Null => {}
                            ValueRef::Integer(value) => out.push_str(&value.to_string()),
                            ValueRef::Real(value) => out.push_str(&value.to_string()),
                            ValueRef::Text(bytes) | ValueRef::Blob(bytes) => {
                                out.push_str(&String::from_utf8_lossy(bytes));
                            }
                        }
                    }
                    out.push('\n');
                }
            }
            Ok(out)
        })
        .expect("the database can be read")
}

/// The drawings a page embeds, decoded: an `<img>` carries its SVG as
/// base64, where a search of the page for a secret would not find it.
fn embedded_drawings(html: &str) -> Vec<String> {
    use base64::Engine as _;
    const OPEN: &str = "src=\"data:image/svg+xml;base64,";
    html.match_indices(OPEN)
        .map(|(at, _)| {
            let encoded = &html[at + OPEN.len()..];
            let encoded = &encoded[..encoded.find('"').expect("the attribute is closed")];
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .expect("base64"),
            )
            .expect("a drawing is text")
        })
        .collect()
}

/// The check the specification asks for with one secret and one method,
/// made with every kind of secret through every way an item is made, and
/// then looked for everywhere an item can go: every result, every event,
/// the audit trail, the review of an incident and a handover in every
/// format, and every row of every table.
#[tokio::test]
async fn an_enable_secret_a_community_a_type_7_password_and_a_key_get_out_through_nothing() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let device = lab.id("SW1");
    let config = secret_config();
    lab.fake("SW1").with(|state| {
        state.outputs.insert("show secrets".into(), config.clone());
    });
    let mut events = app.subscribe();
    let since = now_ms() - 1;
    // Every result of every call made below.
    let mut seen: Vec<Value> = Vec::new();

    // ---- a terminal session, opened by hand under a title somebody typed
    let session_title = format!("SW1 password 7 {TYPE_7}");
    let session = open_session_as(&lab, "SW1", &session_title).await;
    session.enter("show secrets").await;
    let printed = session.lines().join("\n");
    for secret in every_secret() {
        assert!(
            printed.contains(secret),
            "the session itself shows `{secret}`, or this test proves nothing:\n{printed}"
        );
    }
    let whole = call(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id}),
    )
    .await;
    assert!(
        whole["body"]["text"]
            .as_str()
            .unwrap()
            .contains("enable secret 5 <secret:"),
        "{whole}"
    );
    // The last three lines: the end of the key, without its beginning.
    let cut = call(
        app,
        "evidence.capture_terminal",
        json!({"session_id": session.id, "lines": 3, "title": format!("the end of it, enable secret {ENABLE_PLAIN}")}),
    )
    .await;
    assert_eq!(cut["body"]["text"], "<hidden>\n<hidden>\nSW1#");

    // ---- the session's log, as the terminal writes one: every line under
    // its timestamp, the lines of the key included
    let log: String = session
        .lines()
        .iter()
        .enumerate()
        .map(|(index, line)| format!("2026-11-13T05:24:{:02}.000+02:00 {line}\n", index % 60))
        .collect();
    let stamp = "2026-11-13T05:24:00.000+02:00 ".len();
    let name = "2026-11/SW1-20261113-052400.log";
    fixture(app).session_logs.lock().push((
        json!({
            "name": name, "device_id": device, "title": session_title, "kind": "sim",
            "started_at": 1, "ended_at": 2, "size": log.len(), "active": false,
        }),
        log.clone(),
    ));
    let logged = call(app, "evidence.capture_log", json!({"name": name})).await;
    // Two pieces that meet in the middle of the key: one ends in it, one
    // begins in it.
    let middle = log.find(KEY_LINES[1]).unwrap() - stamp;
    let through = middle + stamp + KEY_LINES[1].len() + 1;
    let head = call(
        app,
        "evidence.capture_log",
        json!({"name": name, "limit": through}),
    )
    .await;
    let tail = call(
        app,
        "evidence.capture_log",
        json!({"name": name, "offset": middle}),
    )
    .await;
    for piece in [&head, &tail] {
        assert!(
            piece["body"]["text"].as_str().unwrap().contains("<hidden>"),
            "a piece that holds part of the key: {piece}"
        );
    }

    // ---- a drawing (synthetic: written for this test, not an export)
    *fixture(app).topology_svg.lock() = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"400\" height=\"200\">\n\
         <title>snmp-server community {COMMUNITY} RO</title>\n\
         <desc>{}</desc>\n\
         <g class=\"node\" data-description=\"username admin privilege 15 password 7 {TYPE_7}\">\
         <text x=\"1\" y=\"2\">enable secret 5 {ENABLE_HASH}</text>\
         <text x=\"1\" y=\"3\">enable secret {ENABLE_PLAIN}</text>\
         <text x=\"1\" y=\"4\">line vty 0 4: password 7 {VTY_TYPE_7}</text></g>\n\
         </svg>",
        key_block()
    );
    let drawn = call(
        app,
        "evidence.capture_topology",
        json!({
            "view": format!("enable secret {ENABLE_PLAIN}"),
            "filter": {"site": format!("snmp-server community {COMMUNITY} RO")},
        }),
    )
    .await;
    assert!(
        drawn["body"]["svg"]
            .as_str()
            .unwrap()
            .contains("enable secret 5 &lt;secret:"),
        "{drawn}"
    );

    // ---- a chart whose names and labels are what somebody made them
    let asked_series = json!({
        "device_id": device,
        "metric": format!("enable secret {ENABLE_PLAIN}"),
        "instance": format!("password 7 {TYPE_7}"),
    });
    let mut answered = asked_series.clone();
    answered["points"] = json!([[1, 1.0], [5, 0.0]]);
    answered["step_s"] = json!(60);
    answered["rollup"] = json!(format!("snmp-server community {COMMUNITY} RO"));
    answered["unit"] = json!(format!(
        "enable secret 5 {ENABLE_HASH}\n password 7 {VTY_TYPE_7}"
    ));
    answered["label"] = json!(key_block());
    *fixture(app).metrics_answer.lock() = Some(json!([answered]));
    let charted = call(
        app,
        "evidence.capture_chart",
        json!({"series": [asked_series], "since": 0, "until": 10}),
    )
    .await;
    assert_eq!(charted["body"]["coverage"]["points"], 2);

    // ---- text a person hands in, with a title and a note
    let added = call(
        app,
        "evidence.add_text",
        json!({
            "text": config,
            "title": format!("snmp-server community {COMMUNITY} RO"),
            "note": format!("enable secret 5 {ENABLE_HASH}"),
            "session_id": session.id,
        }),
    )
    .await;
    // A selection out of the log, timestamps and all, that ends in the key.
    let selected = call(
        app,
        "evidence.add_text",
        json!({"text": &log[..through - 1], "title": "a selection from the log", "device_id": device}),
    )
    .await;
    seen.push(
        call(
            app,
            "evidence.rename",
            json!({"id": selected["id"], "title": format!("username admin password 7 {TYPE_7}")}),
        )
        .await,
    );

    let items = [
        whole, cut, logged, head, tail, drawn, charted, added, selected,
    ];
    for item in &items {
        assert_eq!(item["redacted"], true);
    }

    // ---- on an incident, in its review, in a handover
    let incident_id = incident(app, &device).await;
    for item in &items {
        seen.push(
            call(
                app,
                "evidence.attach",
                json!({"id": item["id"], "incident_id": incident_id, "author": "mo"}),
            )
            .await,
        );
        seen.push(call(app, "evidence.get", json!({"id": item["id"]})).await);
    }
    seen.push(call(app, "incidents.get", json!({"id": incident_id})).await);
    let mut drawings: Vec<String> = Vec::new();
    for format in ["markdown", "csv", "html", "json"] {
        let review = call(
            app,
            "incidents.export",
            json!({"id": incident_id, "format": format, "utc_offset_minutes": 0}),
        )
        .await;
        let content = review["content"].as_str().unwrap();
        if format == "html" {
            drawings = embedded_drawings(content);
            assert_eq!(
                content.matches("<article class=\"exhibit\">").count(),
                items.len(),
                "every item is in the appendix"
            );
        }
        seen.push(review);
    }
    assert_eq!(drawings.len(), 1, "the one drawing, as an image");
    for filter in [
        json!({}),
        json!({"incident_id": incident_id}),
        json!({"device_id": device}),
        json!({"query": "secret"}),
        json!({"kind": "terminal"}),
        json!({"kind": "log"}),
        json!({"kind": "topology"}),
        json!({"kind": "chart"}),
        json!({"kind": "text"}),
    ] {
        seen.push(call(app, "evidence.list", filter).await);
    }
    let generated = call(app, "handover.generate", json!({"since": since})).await;
    assert_eq!(
        generated["evidence"]["captured"].as_array().unwrap().len(),
        items.len()
    );
    seen.push(generated);
    let saved = call(
        app,
        "handover.save",
        json!({"since": since, "notes": "", "author": "mo"}),
    )
    .await;
    seen.push(call(app, "handover.get", json!({"id": saved["id"]})).await);
    for format in ["markdown", "csv", "html", "json"] {
        seen.push(
            call(
                app,
                "handover.export",
                json!({"id": saved["id"], "format": format, "utc_offset_minutes": 0}),
            )
            .await,
        );
    }
    seen.push(saved);
    seen.push(call(app, "search.query", json!({"q": "secret"})).await);

    // ---- and when one is taken off, and one deleted
    seen.push(
        call(
            app,
            "evidence.detach",
            json!({"id": items[0]["id"], "incident_id": incident_id, "author": "mo"}),
        )
        .await,
    );
    seen.push(
        call(
            app,
            "evidence.delete",
            json!({"id": items[1]["id"], "author": "zahra"}),
        )
        .await,
    );
    seen.push(call(app, "incidents.get", json!({"id": incident_id})).await);
    seen.push(call(app, "audit.search", json!({})).await);
    seen.extend(items);

    let results = seen
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let events: String = drain(&mut events)
        .iter()
        .map(|event| format!("{} {}\n", event.topic, event.payload))
        .collect();
    let audit = audit_trail(app);
    let database = whole_database(app);
    // That each of these is the thing it is meant to be, and has the items
    // in it: a search of an empty string finds no secret either.
    assert!(results.contains("Appendix: evidence") && results.contains("Evidence captured"));
    assert!(events.contains("evidence.changed") && events.contains("incidents.changed"));
    for action in [
        "evidence.capture_terminal",
        "evidence.capture_log",
        "evidence.capture_topology",
        "evidence.capture_chart",
        "evidence.add_text",
        "evidence.rename",
        "evidence.attach",
        "evidence.detach",
        "evidence.delete",
    ] {
        assert!(
            audit.contains(&format!("{action} |")),
            "no `{action}` in the trail"
        );
    }
    assert!(database.contains("ops_evidence_bodies | ") && database.contains("<secret:"));
    assert!(drawings[0].contains("&lt;secret:"), "{}", drawings[0]);

    for (place, text) in [
        ("a result", &results),
        ("an event", &events),
        ("the audit trail", &audit),
        ("the database", &database),
        ("a drawing in a document", &drawings[0]),
    ] {
        for secret in every_secret() {
            assert!(!text.contains(secret), "`{secret}` got out through {place}");
        }
    }
}

// --------------------------------------------------------- many at once

/// One incident, and everything that can change it going on at the same
/// time: notes, attachments, links, syncs, updates, renames. Each of them
/// reads the incident and writes it back; nothing any of them wrote may be
/// missing at the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn nothing_written_to_an_incident_is_lost_while_everything_else_changes_it() {
    const NOTES: usize = 60;
    const ITEMS: usize = 12;
    const ALERTS: usize = 6;
    let lab = lab(&["SW1", "SW2"], With::all()).await;
    let app = lab.app.clone();
    for index in 0..ALERTS {
        fixture(&app).alerts.lock().push(common::alert(
            &format!("a{index}"),
            &lab.id("SW1"),
            "high",
            hours_ago(1) + index as i64,
        ));
    }
    let incident_id = incident(&app, &lab.id("SW1")).await;
    let mut evidence_ids = Vec::new();
    for index in 0..ITEMS {
        let item = rpc(
            &app,
            "evidence.add_text",
            json!({"text": format!("line {index}"), "title": format!("item {index}")}),
        )
        .await;
        evidence_ids.push(item["id"].as_str().unwrap().to_string());
    }

    let mut calls = Vec::new();
    let mut run = |method: &'static str, params: Value| {
        let app = app.clone();
        calls.push(tokio::spawn(async move {
            app.rpc(method, params)
                .await
                .map(|_| ())
                .map_err(|err| format!("`{method}` failed: {}", err.message))
        }));
    };
    for index in 0..NOTES {
        run(
            "incidents.note",
            json!({"id": incident_id, "text": format!("note {index}"), "author": "mo"}),
        );
        // One item for each of the first notes: fewer items than notes.
        if let Some(evidence_id) = evidence_ids.get(index) {
            run(
                "evidence.attach",
                json!({"id": evidence_id, "incident_id": incident_id, "author": "zahra"}),
            );
            run(
                "evidence.rename",
                json!({"id": evidence_id, "title": format!("renamed {index}")}),
            );
        }
        if index < ALERTS {
            run(
                "incidents.link",
                json!({"id": incident_id, "alert_ids": [format!("a{index}")]}),
            );
        }
        match index % 10 {
            3 => run("incidents.sync", json!({"id": incident_id})),
            5 => run(
                "incidents.link",
                json!({"id": incident_id, "device_ids": [lab.id("SW2")]}),
            ),
            7 => run(
                "incidents.update",
                json!({"id": incident_id, "impact": "the whole second floor"}),
            ),
            _ => {}
        }
    }
    for call in calls {
        call.await.expect("the call ran").expect("and succeeded");
    }

    let incident = rpc(&app, "incidents.get", json!({"id": incident_id})).await;
    let timeline = incident["timeline"].as_array().unwrap();
    for index in 0..NOTES {
        assert_eq!(
            timeline
                .iter()
                .filter(|entry| entry["kind"] == "note" && entry["text"] == format!("note {index}"))
                .count(),
            1,
            "note {index} is on the timeline once"
        );
    }
    for (index, id) in evidence_ids.iter().enumerate() {
        let on_timeline = entry(&incident, &format!("evidence:{id}"))
            .unwrap_or_else(|| panic!("the attachment of item {index} was lost"));
        // Whichever of the two came first, the entry shows the title the
        // item has now.
        assert_eq!(on_timeline["text"], format!("renamed {index}"));
        assert_eq!(on_timeline["ref"], json!({"type": "evidence", "id": id}));
        let item = rpc(&app, "evidence.get", json!({"id": id})).await;
        assert_eq!(item["incident_ids"], json!([incident_id]));
        assert_eq!(item["title"], format!("renamed {index}"));
    }
    let mut ids: Vec<&str> = timeline
        .iter()
        .map(|entry| entry["id"].as_str().unwrap())
        .collect();
    let entries = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), entries, "no entry is on the timeline twice");
    let alert_ids: Vec<&str> = incident["alert_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap())
        .collect();
    for index in 0..ALERTS {
        assert!(
            alert_ids.contains(&format!("a{index}").as_str()),
            "the link to alert a{index} was lost: {alert_ids:?}"
        );
    }
    assert!(
        incident["device_ids"]
            .as_array()
            .unwrap()
            .contains(&json!(lab.id("SW2")))
    );
    assert_eq!(incident["impact"], "the whole second floor");
    let listed = rpc(
        &app,
        "evidence.list",
        json!({"incident_id": incident_id, "limit": 200}),
    )
    .await;
    assert_eq!(listed["items"].as_array().unwrap().len(), ITEMS);
}

// ------------------------------------------------------- hostile content

fn unhtml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn csv_rows(text: &str) -> Vec<Vec<String>> {
    csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes())
        .records()
        .map(|record| record.unwrap().iter().map(str::to_string).collect())
        .collect()
}

fn would_run(cell: &str) -> bool {
    matches!(
        cell.trim_start().chars().next(),
        Some('=' | '+' | '-' | '@')
    )
}

/// What a drawing can carry when whoever drew it wants a page to do
/// something (synthetic).
const HOSTILE_DRAWING: &str = "<svg xmlns=\"http://www.w3.org/2000/svg\" onload=\"alert(1)\">\
    <script>alert(2)</script>\
    <foreignObject><iframe xmlns=\"http://www.w3.org/1999/xhtml\" src=\"javascript:alert(3)\"></iframe></foreignObject>\
    <image href=\"http://tracker.example/beacon.png\"/>\
    <a href=\"javascript:alert(4)\"><text>SW1</text></a></svg>";

/// Terminal escape sequences, control characters that rewrite a line, a
/// line of a hundred thousand characters, fences, markup, formulas and a
/// direction override, as text a person selected; and a drawing with a
/// script in it. In every format of the review they are text and a picture.
#[tokio::test]
async fn escape_sequences_long_lines_markup_and_a_scripted_drawing_are_inert_in_every_format() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let long = "A".repeat(100_000);
    let hostile = format!(
        "\u{1b}]0;owned\u{7}\u{1b}[2J\u{1b}[31mred\u{1b}[0m\n\
         safe\u{8}\u{8}\u{8}\u{8}evil\rover\n\
         {long}\n\
         nul\u{0}byte\n\
         ~~~\n\
         ``````\n\
         <!-- --><svg onload=alert(1)><script>alert(2)</script>\n\
         </pre></article></section><iframe src=//evil.example>\n\
         \t=cmd|' /C calc'!A0\n  -2+3+cmd|' /C calc'!A0\n\
         @SUM(1+1)*cmd|' /C calc'!A0\n\
         +1\n\
         \"quoted\",comma\n\
         \u{202e}reversed\u{202c}\n\
         [x](javascript:alert(3))\n\
         | a | b |\n\
         # heading\n\
         <img src=x onerror=alert(4)>"
    );
    // The same text as a reader is to see it: every character that would do
    // something shown as a picture of itself.
    let shown = hostile
        .replace('\u{1b}', "\u{241b}")
        .replace('\u{7}', "\u{2407}")
        .replace('\u{8}', "\u{2408}")
        .replace('\r', "\u{240d}")
        .replace('\u{0}', "\u{2400}")
        .replace(['\u{202e}', '\u{202c}'], "\u{fffd}");
    let shown_lines: Vec<&str> = shown.split('\n').collect();
    let title = "=HYPERLINK(\"http://evil.example\") <script>alert(5)</script>";

    let item = rpc(
        app,
        "evidence.add_text",
        json!({"text": hostile, "title": title}),
    )
    .await;
    assert_eq!(
        item["body"]["text"], hostile,
        "it is stored as it came: there is no secret in it"
    );
    assert_eq!(item["sha256"], sha256(&hostile));
    assert_eq!(
        rpc(app, "evidence.get", json!({"id": item["id"]})).await,
        item,
        "and comes back the same, the NUL included"
    );
    *fixture(app).topology_svg.lock() = HOSTILE_DRAWING.into();
    let drawing = rpc(
        app,
        "evidence.capture_topology",
        json!({"title": "the map"}),
    )
    .await;
    assert_eq!(
        drawing["body"],
        json!({"type": "svg", "svg": HOSTILE_DRAWING}),
        "a string in a JSON value, which runs nowhere until somebody makes markup of it"
    );
    let incident_id = incident(app, &lab.id("SW1")).await;
    for id in [&item["id"], &drawing["id"]] {
        rpc(
            app,
            "evidence.attach",
            json!({"id": id, "incident_id": incident_id}),
        )
        .await;
    }
    let export = |format: &'static str| {
        let incident_id = incident_id.clone();
        async move {
            rpc(
                app,
                "incidents.export",
                json!({"id": incident_id, "format": format, "utc_offset_minutes": 0}),
            )
            .await["content"]
                .as_str()
                .unwrap()
                .to_string()
        }
    };
    let quiet = |document: &str| {
        document
            .chars()
            .all(|ch| !ch.is_control() || matches!(ch, '\n' | '\t'))
            && !document.contains(['\u{202e}', '\u{202c}'])
    };

    // ---- HTML
    let html = export("html").await;
    assert!(
        quiet(&html),
        "a character that does something is in the page"
    );
    for markup in [
        "<script",
        "<svg",
        "<iframe",
        "<!--",
        "<img src=x",
        "<foreignObject",
    ] {
        assert!(!html.contains(markup), "`{markup}` is markup in the page");
    }
    assert!(!html.contains("beacon.png") && !html.contains("javascript:alert(4)"));
    for (open, close, count) in [
        ("<pre>", "</pre>", 1),
        ("<article", "</article>", 2),
        ("<img ", "\">", 1),
    ] {
        assert_eq!(html.matches(open).count(), count, "{open}");
        assert!(html.matches(close).count() >= count, "{close}");
    }
    assert_eq!(
        html.matches("<section>").count(),
        html.matches("</section>").count()
    );
    let pre = html.split_once("<pre>").unwrap().1;
    let pre = pre.split_once("</pre>").unwrap().0;
    assert_eq!(unhtml(pre), shown);
    assert!(html.contains(
        "<h3>=HYPERLINK(&quot;http://evil.example&quot;) &lt;script&gt;alert(5)&lt;/script&gt;</h3>"
    ));
    // The drawing is an image, whole, and nothing of it is in the page.
    assert_eq!(embedded_drawings(&html), [HOSTILE_DRAWING]);
    assert!(
        html.contains("<img class=\"drawing\" alt=\"the map\" src=\"data:image/svg+xml;base64,")
    );

    // ---- Markdown
    let markdown = export("markdown").await;
    assert!(quiet(&markdown));
    let lines: Vec<&str> = markdown.lines().collect();
    let open = lines
        .iter()
        .position(|line| *line == "```````text")
        .expect("a fence of seven backticks, one more than the six in the text");
    let close = lines
        .iter()
        .rposition(|line| *line == "```````")
        .expect("its end");
    assert_eq!(lines[open + 1..close], shown_lines[..]);
    for line in lines[..open].iter().chain(&lines[close + 1..]) {
        assert!(
            !line.replace("\\<", "").contains('<') && !line.starts_with("# heading"),
            "markup outside the fence: {line}"
        );
    }
    assert!(!markdown.contains("foreignObject") && !markdown.contains("beacon.png"));
    assert!(markdown.contains(&format!("*A drawing of {} bytes.", HOSTILE_DRAWING.len())));

    // ---- CSV
    let csv_text = export("csv").await;
    assert!(quiet(&csv_text));
    let rows = csv_rows(&csv_text);
    let first = rows
        .iter()
        .position(|row| row.len() == 1 && row[0] == shown_lines[0])
        .expect("the text starts on a row of its own");
    for (row, line) in rows[first..].iter().zip(&shown_lines) {
        let expected = if would_run(line) {
            format!("'{line}")
        } else {
            line.to_string()
        };
        assert_eq!(row, &[expected], "one line, one row, one cell");
    }
    for cell in rows.iter().flatten() {
        assert!(!would_run(cell), "a spreadsheet would run `{cell}`");
    }
    assert!(rows.contains(&vec![format!("'{title}")]));
    assert!(!csv_text.contains("foreignObject") && !csv_text.contains("beacon.png"));

    // ---- JSON
    let document: Value = serde_json::from_str(&export("json").await).expect("JSON");
    let appendix = document["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["heading"] == "Appendix: evidence")
        .expect("the appendix");
    let exhibits = appendix["items"].as_array().unwrap();
    assert_eq!(exhibits.len(), 2);
    let text = exhibits
        .iter()
        .find(|exhibit| exhibit["content"] == "text")
        .unwrap();
    assert_eq!(text["text"], hostile);
    assert_eq!(text["title"], title);
    let svg = exhibits
        .iter()
        .find(|exhibit| exhibit["content"] == "svg")
        .unwrap();
    assert_eq!(svg["svg"], HOSTILE_DRAWING);
}

// ------------------------------------------------------ exactly at the edge

#[tokio::test]
async fn each_limit_holds_exactly_what_it_says_and_not_one_more() {
    const TEXT: usize = 256 * 1024;
    const SVG: usize = 2 * 1024 * 1024;
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let device = lab.id("SW1");
    let mut kept = 0;

    // ---- a drawing of exactly 2 MiB, and one byte more
    let drawing = |bytes: usize| {
        let (open, close) = ("<svg xmlns=\"http://www.w3.org/2000/svg\">", "</svg>");
        let shapes = "<g/>".repeat((bytes - open.len() - close.len()) / 4);
        let pad = " ".repeat(bytes - open.len() - close.len() - shapes.len());
        format!("{open}{shapes}{pad}{close}")
    };
    *fixture(app).topology_svg.lock() = drawing(SVG);
    let item = rpc(app, "evidence.capture_topology", json!({})).await;
    assert_eq!(item["bytes"], SVG);
    assert_eq!(item["body"]["svg"].as_str().unwrap().len(), SVG);
    kept += 1;
    *fixture(app).topology_svg.lock() = drawing(SVG + 1);
    let err = refused(app, "evidence.capture_topology", json!({})).await;
    assert_eq!(err.code, "invalid");
    assert_eq!(
        err.details,
        Some(json!({"limit": "svg_bytes", "max": SVG, "actual": SVG + 1}))
    );

    // ---- 20,000 points over two series, and one more
    let series = |points: usize| {
        let points: Vec<Value> = (0..points).map(|at| json!([at, 1.0])).collect();
        json!({"points": points})
    };
    let two = json!({
        "series": [
            {"device_id": device, "metric": "up"},
            {"device_id": device, "metric": "up", "instance": "b"},
        ],
        "since": 0,
        "until": 20_001,
    });
    *fixture(app).metrics_answer.lock() = Some(json!([series(10_000), series(10_000)]));
    let item = rpc(app, "evidence.capture_chart", two.clone()).await;
    assert_eq!(item["body"]["coverage"]["points"], 20_000);
    assert_eq!(
        item["body"]["series"][1]["points"]
            .as_array()
            .unwrap()
            .len(),
        10_000
    );
    kept += 1;
    *fixture(app).metrics_answer.lock() = Some(json!([series(10_000), series(10_001)]));
    let err = refused(app, "evidence.capture_chart", two).await;
    assert_eq!(
        err.details,
        Some(json!({"limit": "points", "max": 20_000, "actual": 20_001}))
    );

    // ---- text that fits as it comes and not once its secret is a placeholder
    let line = "interface GigabitEthernet0/1 is up, line protocol is up\n";
    let secret = "password abcd";
    let mut grows = line.repeat((TEXT - secret.len()) / line.len());
    grows.push_str(&"y".repeat(TEXT - secret.len() - grows.len() - 1));
    grows.push('\n');
    grows.push_str(secret);
    assert_eq!(grows.len(), TEXT);
    let err = refused(
        app,
        "evidence.add_text",
        json!({"text": grows, "title": "grows"}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert_eq!(
        err.details,
        Some(json!({
            "limit": "text_bytes", "max": TEXT,
            "actual": TEXT - "abcd".len() + "<secret:1>".len(),
        })),
        "what counts is what would be stored, and none of it is cut"
    );

    // ---- lines of a session and bytes of a log
    let session = open_session(&lab, "SW1").await;
    session.enter("show clock").await;
    for lines in [1, 10_000] {
        rpc(
            app,
            "evidence.capture_terminal",
            json!({"session_id": session.id, "lines": lines}),
        )
        .await;
        kept += 1;
    }
    for lines in [0, 10_001] {
        let err = refused(
            app,
            "evidence.capture_terminal",
            json!({"session_id": session.id, "lines": lines}),
        )
        .await;
        assert_eq!(err.code, "invalid", "lines: {lines}");
    }
    let name = "2026-11/SW1-20261113-052400.log";
    fixture(app).session_logs.lock().push((
        json!({"name": name, "device_id": device, "title": "SW1", "kind": "sim",
               "started_at": 1, "ended_at": 2, "size": 9, "active": false}),
        "SW1#exit\n".into(),
    ));
    for limit in [1, TEXT] {
        // One byte of a log is no whole line; what matters is that the
        // request is taken.
        match app
            .rpc(
                "evidence.capture_log",
                json!({"name": name, "limit": limit}),
            )
            .await
        {
            Ok(_) => kept += 1,
            Err(err) => assert_ne!(err.code, "invalid", "limit {limit}: {}", err.message),
        }
    }
    let reads = fixture(app).called("terminal.logs.read");
    for limit in [0, TEXT + 1] {
        let err = refused(
            app,
            "evidence.capture_log",
            json!({"name": name, "limit": limit}),
        )
        .await;
        assert_eq!(err.code, "invalid", "limit: {limit}");
        assert!(
            err.message
                .contains("`limit` is between 1 and 262144 bytes"),
            "limit {limit}: {}",
            err.message
        );
    }
    assert_eq!(
        fixture(app).called("terminal.logs.read"),
        reads,
        "a read that cannot be kept is not made"
    );

    // ---- a title of 200 characters and a note of 1,000, counted as
    // characters; and one more of each
    let item = rpc(
        app,
        "evidence.add_text",
        json!({"text": "x", "title": "é".repeat(200), "note": "ö".repeat(1_000)}),
    )
    .await;
    assert_eq!(item["title"].as_str().unwrap().chars().count(), 200);
    kept += 1;
    for (title, note) in [
        ("é".repeat(201), "n".into()),
        ("t".into(), "ö".repeat(1_001)),
    ] {
        let err = refused(
            app,
            "evidence.add_text",
            json!({"text": "x", "title": title, "note": note}),
        )
        .await;
        assert_eq!(err.code, "invalid");
    }

    // ---- what is named as a session is an id or it is refused: it is
    // stored with the item whether or not the session is still there
    let item = rpc(
        app,
        "evidence.add_text",
        json!({"text": "x", "title": "t", "session_id": "s".repeat(64)}),
    )
    .await;
    assert_eq!(item["source"]["session_id"], "s".repeat(64));
    kept += 1;
    for not_an_id in [
        "s".repeat(65),
        "enable secret Sup3rS3cretEnable".to_string(),
        "<script>".to_string(),
    ] {
        let err = refused(
            app,
            "evidence.add_text",
            json!({"text": "x", "title": "t", "session_id": not_an_id}),
        )
        .await;
        assert_eq!(err.code, "invalid");
        assert!(err.message.contains("`session_id`"), "{}", err.message);
    }

    // ---- what was asked for is recorded with the item, so it has a size
    let fits = json!({
        "series": [{"device_id": device, "metric": "up", "instance": "i".repeat(65_000)}],
        "since": 0,
        "until": 10,
        "title": "a long name",
    });
    assert!(fits.to_string().len() <= 64 * 1024 + 30);
    *fixture(app).metrics_answer.lock() = Some(json!([series(1)]));
    rpc(app, "evidence.capture_chart", fits).await;
    kept += 1;
    let calls = fixture(app).called("metrics.query_many");
    let err = refused(
        app,
        "evidence.capture_chart",
        json!({
            "series": [{"device_id": device, "metric": "up", "instance": "i".repeat(66_000)}],
            "since": 0,
            "until": 10,
            "title": "a longer one",
        }),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert_eq!(err.details.as_ref().unwrap()["limit"], "params_bytes");
    assert_eq!(err.details.as_ref().unwrap()["max"], 64 * 1024);
    let err = refused(
        app,
        "evidence.capture_topology",
        json!({"filter": {"device_ids": vec!["d"; 30_000]}}),
    )
    .await;
    assert_eq!(err.details.as_ref().unwrap()["limit"], "params_bytes");
    assert_eq!(
        fixture(app).called("metrics.query_many"),
        calls,
        "the other feature is not even asked"
    );

    // ---- a page is at most 200 items and at least one
    for (limit, most) in [(0, 1), (1, 1), (1_000_000, 200)] {
        let page = rpc(app, "evidence.list", json!({"limit": limit})).await;
        assert!(page["items"].as_array().unwrap().len() <= most);
    }

    // Everything that fitted is there, whole, and nothing that did not.
    let stored = rpc(app, "evidence.list", json!({"limit": 200})).await;
    assert_eq!(stored["items"].as_array().unwrap().len(), kept);
}

/// A review carries at most 4 MiB of evidence. What does not fit is still
/// listed, with everything known about it and the statement that its
/// content is not there — never left out without a word, never cut.
#[tokio::test]
async fn a_review_holds_four_mebibytes_of_evidence_and_says_what_it_left_out() {
    const SVG: usize = 2 * 1024 * 1024;
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    let incident_id = incident(app, &lab.id("SW1")).await;
    let (open, close) = ("<svg xmlns=\"http://www.w3.org/2000/svg\">", "</svg>");
    let shapes = "<g/>".repeat((SVG - open.len() - close.len()) / 4);
    let pad = " ".repeat(SVG - open.len() - close.len() - shapes.len());
    *fixture(app).topology_svg.lock() = format!("{open}{shapes}{pad}{close}");
    let mut ids = Vec::new();
    for title in ["first", "second", "third"] {
        let item = rpc(app, "evidence.capture_topology", json!({"title": title})).await;
        assert_eq!(item["bytes"], SVG);
        rpc(
            app,
            "evidence.attach",
            json!({"id": item["id"], "incident_id": incident_id}),
        )
        .await;
        ids.push(item);
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
    }

    let review = rpc(
        app,
        "incidents.export",
        json!({"id": incident_id, "format": "json", "utc_offset_minutes": 0}),
    )
    .await;
    let document: Value = serde_json::from_str(review["content"].as_str().unwrap()).unwrap();
    let appendix = document["sections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|section| section["heading"] == "Appendix: evidence")
        .expect("the appendix");
    let exhibits = appendix["items"].as_array().unwrap();
    assert_eq!(exhibits.len(), 3, "all three are listed");
    // Two of 2 MiB are exactly the 4 MiB a document carries.
    for exhibit in &exhibits[..2] {
        assert_eq!(exhibit["content"], "svg");
        assert_eq!(exhibit["svg"].as_str().unwrap().len(), SVG);
    }
    let left_out = &exhibits[2];
    assert_eq!(left_out["title"], "third");
    assert_eq!(left_out["content"], "omitted");
    assert!(left_out.get("svg").is_none());
    assert!(
        left_out["why"]
            .as_str()
            .unwrap()
            .contains("at most 4.0 MiB of evidence and this item is 2.0 MiB"),
        "{left_out}"
    );
    assert_eq!(left_out["facts"]["Evidence"], ids[2]["id"]);
    assert_eq!(left_out["facts"]["SHA-256"], ids[2]["sha256"]);

    let html = rpc(
        app,
        "incidents.export",
        json!({"id": incident_id, "format": "html", "utc_offset_minutes": 0}),
    )
    .await["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(html.matches("<img class=\"drawing\"").count(), 2);
    assert!(html.contains("<p class=\"missing\">Not included — this document carries at most"));
}

/// Times a source has no business sending. They are its times, so they are
/// kept; nothing here may fall over them.
#[tokio::test]
async fn a_chart_with_times_at_the_ends_of_the_number_line_is_kept_and_exported() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    *fixture(app).metrics_answer.lock() = Some(json!([
        {"points": [[i64::MIN, 1.0], [i64::MAX, 2.0], [0, 3.0]]}
    ]));
    let item = rpc(
        app,
        "evidence.capture_chart",
        json!({"series": [{"device_id": lab.id("SW1"), "metric": "up"}], "since": 0, "until": 10}),
    )
    .await;
    let covered = &item["body"]["coverage"]["series"][0];
    assert_eq!(covered["first_at"], i64::MIN);
    assert_eq!(covered["last_at"], i64::MAX);
    assert_eq!(covered["longest_gap_ms"], i64::MAX);
    let incident_id = incident(app, &lab.id("SW1")).await;
    rpc(
        app,
        "evidence.attach",
        json!({"id": item["id"], "incident_id": incident_id}),
    )
    .await;
    for format in ["markdown", "csv", "html", "json"] {
        let review = rpc(
            app,
            "incidents.export",
            json!({"id": incident_id, "format": format, "utc_offset_minutes": 0}),
        )
        .await;
        assert!(
            review["content"]
                .as_str()
                .unwrap()
                .contains("Appendix: evidence"),
            "{format}"
        );
    }
}

// ------------------------------------------------- silence, and a restart

/// The third way a feature can fail a capture, after "not in the build" and
/// "answered with something else": it does not answer. The clock is stopped
/// so that twenty seconds take none.
#[tokio::test]
async fn a_feature_that_never_answers_makes_a_capture_unavailable_after_twenty_seconds() {
    let lab = lab(&["SW1"], With::all()).await;
    let app = &lab.app;
    fixture(app).delay("topology.export");
    tokio::time::pause();
    let asked = tokio::time::Instant::now();
    let err = refused(app, "evidence.capture_topology", json!({})).await;
    let waited = asked.elapsed();
    tokio::time::resume();
    assert_eq!(err.code, "unavailable");
    assert!(
        err.message
            .contains("`topology.export` did not answer within 20000 ms"),
        "{}",
        err.message
    );
    assert!((20..30).contains(&waited.as_secs()), "it waited {waited:?}");
    assert_eq!(
        rpc(app, "evidence.list", json!({})).await,
        json!({"items": []}),
        "and nothing was put in its place"
    );
}

#[tokio::test]
async fn an_item_is_the_same_item_after_a_restart_and_its_file_never_held_the_secret() {
    use nocturne_app::{AppBuilder, AppConfig};
    let dir = tempfile::tempdir().expect("a directory");
    let start = || -> Option<App> {
        let mut builder = match AppBuilder::new(AppConfig::at(dir.path())) {
            Ok(builder) => builder,
            // The instance before this one is still letting go of the
            // directory.
            Err(err) if err.code == "conflict" => return None,
            Err(err) => panic!("the app cannot start: {}", err.message),
        };
        nocturne_app::init(&mut builder);
        nocturne_ops::init(&mut builder);
        Some(builder.build())
    };

    let app = start().expect("the first start");
    let item = rpc(
        &app,
        "evidence.add_text",
        json!({
            "text": format!("{}\nGi0/1 is down", secret_config()),
            "title": format!("before the restart, enable secret {ENABLE_PLAIN}"),
        }),
    )
    .await;
    let incident = rpc(
        &app,
        "incidents.create",
        json!({"title": "SW1 uplink flapping", "author": "mo"}),
    )
    .await;
    let attached = rpc(
        &app,
        "evidence.attach",
        json!({"id": item["id"], "incident_id": incident["id"], "author": "mo"}),
    )
    .await;
    app.shutdown();
    drop(app);

    let mut app = None;
    for _ in 0..500 {
        app = start();
        if app.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let app = app.expect("the directory was let go of");
    let again = rpc(&app, "evidence.get", json!({"id": item["id"]})).await;
    assert_eq!(again["body"], item["body"]);
    assert_eq!(again["sha256"], item["sha256"]);
    assert_eq!(
        again["sha256"],
        sha256(again["body"]["text"].as_str().unwrap()),
        "the hash is still that of what is stored"
    );
    assert_eq!(again["incident_ids"], attached["incident_ids"]);
    let listed = rpc(
        &app,
        "evidence.list",
        json!({"incident_id": incident["id"]}),
    )
    .await;
    assert_eq!(listed["items"][0]["id"], item["id"]);
    let timeline = rpc(&app, "incidents.get", json!({"id": incident["id"]})).await;
    assert!(
        entry(
            &timeline,
            &format!("evidence:{}", item["id"].as_str().unwrap())
        )
        .is_some()
    );

    // The files themselves, byte for byte: the database and whatever it
    // keeps beside it.
    app.shutdown();
    drop(app);
    let mut searched = 0;
    for file in std::fs::read_dir(dir.path()).expect("the directory") {
        let path = file.expect("an entry").path();
        if !path.is_file() {
            continue;
        }
        let bytes = std::fs::read(&path).expect("a file");
        searched += bytes.len();
        for secret in every_secret() {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes()),
                "`{secret}` is in {}",
                path.display()
            );
        }
    }
    assert!(
        searched > 4_096,
        "a database was searched: {searched} bytes"
    );
}
