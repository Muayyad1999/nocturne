//! Sessions against the fake devices: opening, typing, the output protocol,
//! attaching mid-stream, resizing, closing from either side, listing.

mod common;

use common::{Events, PATIENCE, lab, open_device, session_id};
use serde_json::json;

/// 20 000 numbered lines with a non-ASCII character in each, so that a split
/// between two output events falls inside a character sooner or later.
fn numbered(lines: usize) -> String {
    (0..lines)
        .map(|index| format!("line {index:05} ☃ padding-to-make-it-wider\n"))
        .collect()
}

/// The numbers of the `line NNNNN` lines in `text`, in the order they appear.
fn numbers(text: &str) -> Vec<usize> {
    text.match_indices("line ")
        .filter_map(|(at, _)| text.get(at + 5..at + 10)?.parse().ok())
        .collect()
}

#[tokio::test]
async fn a_session_opens_reports_itself_and_closes() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let info = open_device(app, &lab.id("SW1")).await.expect("open");
    let id = session_id(&info);

    assert_eq!(info["title"], "SW1");
    assert_eq!(info["kind"], "sim");
    assert_eq!(info["state"], "open");
    assert_eq!(info["device_id"], lab.id("SW1"));
    assert_eq!(info["lab"], true);
    assert_eq!((&info["cols"], &info["rows"]), (&json!(100), &json!(30)));
    assert_eq!(info["logging"], json!(null));
    assert_eq!(info["info"]["peer"], "lab:SW1");

    let opened = events.wait_topic("terminal.opened").await;
    assert_eq!(opened.payload["id"], id.as_str());

    let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
    assert_eq!(listed.as_array().expect("a list").len(), 1);
    assert_eq!(listed[0]["id"], id.as_str());

    // The device's greeting arrives as output.
    events.wait_for(&id, "SW1>").await;

    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
    events.drain();
    assert_eq!(
        events.of("terminal.closed")[0].payload["session_id"],
        id.as_str()
    );
    assert_eq!(
        events.of("terminal.removed")[0].payload["session_id"],
        id.as_str()
    );
    assert!(
        app.rpc("terminal.list", json!(null))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .is_empty()
    );
    // Typing into it now says so rather than failing silently.
    let gone = app
        .rpc("terminal.input", json!({"session_id": id, "data": "x"}))
        .await
        .unwrap_err();
    assert_eq!(gone.code, "not_found");

    assert!(
        common::audit_text(app).contains("Opened a terminal session to SW1"),
        "{}",
        common::audit_text(app)
    );
}

#[tokio::test]
async fn typing_reaches_the_device_and_its_answer_comes_back_in_order() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    events.wait_for(&id, "SW1>").await;

    app.rpc(
        "terminal.input",
        json!({"session_id": id, "data": "show clock\r"}),
    )
    .await
    .expect("input");
    events.wait_for(&id, "12:00:00").await;

    let text = events.text(&id);
    // The echo comes before the answer, and the prompt after it.
    let echo = text.find("show clock").expect("the echo");
    let answer = text.find("12:00:00").expect("the answer");
    assert!(echo < answer, "{text}");
    assert_eq!(lab.fake("SW1").seen(), ["show clock"]);

    // Every output event was numbered without a gap (checked while reading),
    // and the counters followed.
    let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
    assert_eq!(listed[0]["bytes_out"], 11);
    assert!(listed[0]["bytes_in"].as_u64().expect("bytes_in") >= text.len() as u64 - 4);
}

#[tokio::test]
async fn attaching_mid_stream_gets_the_scrollback_and_every_later_event() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let spam = numbered(20_000);
    lab.fake("SW1")
        .with(|device| device.outputs.insert("spam".into(), spam.clone()));

    let mut first = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    first.wait_for(&id, "SW1>").await;

    // A second client turns up while the burst is in flight. It subscribes,
    // then attaches: everything with a higher `seq` must still reach it.
    let mut late = Events::new(app);
    app.rpc(
        "terminal.input",
        json!({"session_id": id, "data": "spam\r"}),
    )
    .await
    .expect("input");
    let attached = app
        .rpc("terminal.attach", json!({"session_id": id}))
        .await
        .expect("attach");
    let scrollback = base64_decode(attached["scrollback"].as_str().expect("scrollback"));
    let seq = attached["seq"].as_u64().expect("seq");
    assert_eq!(attached["info"]["id"], id.as_str());
    late.attached(&id, &scrollback, seq);

    // `Events` asserts there is no gap as it reads; the numbers prove there
    // is no duplicate and nothing was lost in the hand-over.
    late.wait_for(&id, "line 19999").await;
    first.wait_for(&id, "line 19999").await;

    for (who, text) in [("late", late.text(&id)), ("first", first.text(&id))] {
        let seen = numbers(&text);
        assert_eq!(seen.last().copied(), Some(19_999), "{who} lost the end");
        assert_eq!(seen.len(), 20_000, "{who} saw {} lines", seen.len());
        for (expected, actual) in seen.iter().enumerate() {
            assert_eq!(expected, *actual, "{who} is out of order or duplicated");
        }
        // Multi-byte characters survived being split across events.
        assert_eq!(text.matches('☃').count(), 20_000, "{who} corrupted UTF-8");
    }
    assert_eq!(late.text(&id), first.text(&id));

    // Attaching when there is nothing new hands over a consistent pair again.
    let again = app
        .rpc("terminal.attach", json!({"session_id": id}))
        .await
        .expect("attach");
    assert!(again["seq"].as_u64().expect("seq") >= seq);
    assert!(
        String::from_utf8_lossy(&base64_decode(
            again["scrollback"].as_str().expect("scrollback")
        ))
        .contains("line 19999")
    );
}

#[tokio::test]
async fn attaching_to_a_quiet_new_session_starts_from_nothing() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    events.wait_for(&id, "SW1>").await;

    let attached = app
        .rpc("terminal.attach", json!({"session_id": id}))
        .await
        .expect("attach");
    let scrollback = base64_decode(attached["scrollback"].as_str().expect("scrollback"));
    assert_eq!(String::from_utf8_lossy(&scrollback), "\r\nSW1>");
    assert!(attached["seq"].as_u64().expect("seq") >= 1);
}

#[tokio::test]
async fn resizing_is_remembered_and_announced() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));

    app.rpc(
        "terminal.resize",
        json!({"session_id": id, "cols": 200, "rows": 60}),
    )
    .await
    .expect("resize");
    let changed = events.wait_topic("terminal.changed").await;
    assert_eq!(
        (&changed.payload["cols"], &changed.payload["rows"]),
        (&json!(200), &json!(60))
    );

    let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
    assert_eq!(listed[0]["cols"], 200);

    // The same size again is not news.
    let before = events.of("terminal.changed").len();
    app.rpc(
        "terminal.resize",
        json!({"session_id": id, "cols": 200, "rows": 60}),
    )
    .await
    .expect("resize");
    events.drain();
    assert_eq!(events.of("terminal.changed").len(), before);
}

#[tokio::test]
async fn the_far_end_hanging_up_ends_the_session_but_keeps_it_listed() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    events.wait_for(&id, "SW1>").await;

    app.rpc(
        "terminal.input",
        json!({"session_id": id, "data": "exit\r"}),
    )
    .await
    .expect("input");
    let closed = events.wait_topic("terminal.closed").await;
    assert_eq!(closed.payload["session_id"], id.as_str());
    assert_eq!(closed.payload["reason"], "closed by the remote end");

    // Still listed, so the user can read what is on the screen.
    let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
    assert_eq!(listed[0]["state"], "closed");
    assert_eq!(listed[0]["close_reason"], "closed by the remote end");
    assert!(listed[0]["closed_at"].as_i64().is_some());
    // And it can still be attached to and read.
    let attached = app
        .rpc("terminal.attach", json!({"session_id": id}))
        .await
        .expect("attach");
    assert!(
        String::from_utf8_lossy(&base64_decode(
            attached["scrollback"].as_str().expect("scrollback")
        ))
        .contains("SW1>")
    );

    let typing = app
        .rpc("terminal.input", json!({"session_id": id, "data": "x"}))
        .await
        .unwrap_err();
    assert_eq!(typing.code, "closed");
    assert!(typing.message.contains("open a new one"), "{typing}");

    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");
}

#[tokio::test]
async fn typing_into_several_sessions_at_once_reports_each_one() {
    let lab = lab(&["SW1", "SW2"]).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let first = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    let second = session_id(&open_device(app, &lab.id("SW2")).await.expect("open"));
    events.wait_for(&first, "SW1>").await;
    events.wait_for(&second, "SW2>").await;

    let sent = app
        .rpc(
            "terminal.input_many",
            json!({"session_ids": [first, second, "nosuchsession"], "data": "show clock\r"}),
        )
        .await
        .expect("input_many");
    assert_eq!(sent["sent"], 2);
    assert_eq!(sent["failed"][0]["session_id"], "nosuchsession");
    assert_eq!(sent["failed"][0]["code"], "not_found");

    events.wait_for(&first, "12:00:00").await;
    events.wait_for(&second, "12:00:00").await;
    assert_eq!(lab.fake("SW1").seen(), ["show clock"]);
    assert_eq!(lab.fake("SW2").seen(), ["show clock"]);

    let empty = app
        .rpc(
            "terminal.input_many",
            json!({"session_ids": [], "data": "x"}),
        )
        .await
        .unwrap_err();
    assert_eq!(empty.code, "invalid");
}

#[tokio::test]
async fn a_snapshot_is_the_plain_text_of_the_last_lines() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    lab.fake("SW1").with(|device| {
        device.outputs.insert(
            "show secret".into(),
            "\u{1b}[1mSW1 configuration\u{1b}[0m\nenable secret Sup3rS3cret\n".into(),
        );
    });
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    events.wait_for(&id, "SW1>").await;
    app.rpc(
        "terminal.input",
        json!({"session_id": id, "data": "show secret\r"}),
    )
    .await
    .expect("input");
    events.wait_for(&id, "Sup3rS3cret").await;

    let snapshot = app
        .rpc("terminal.snapshot", json!({"session_id": id}))
        .await
        .expect("snapshot");
    let text = snapshot["text"].as_str().expect("text");
    assert!(
        !text.contains('\u{1b}'),
        "escape sequences survived: {text:?}"
    );
    assert!(text.contains("SW1 configuration"), "{text}");
    // The device's own secret is hidden unless it is asked for.
    assert_eq!(snapshot["redacted"], true);
    assert!(!text.contains("Sup3rS3cret"), "{text}");

    let verbatim = app
        .rpc(
            "terminal.snapshot",
            json!({"session_id": id, "redact": false}),
        )
        .await
        .expect("snapshot");
    assert!(
        verbatim["text"]
            .as_str()
            .expect("text")
            .contains("Sup3rS3cret")
    );
    assert_eq!(verbatim["redacted"], false);

    // The tail only.
    let two = app
        .rpc("terminal.snapshot", json!({"session_id": id, "lines": 2}))
        .await
        .expect("snapshot");
    assert_eq!(two["lines"], 2);
}

#[tokio::test]
async fn a_break_reaches_a_transport_that_can_carry_one() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    // A simulated console can: the device simply ignores it. (The refusal on
    // a transport that cannot is in `adhoc.rs`, over a local shell.)
    app.rpc("terminal.send_break", json!({"session_id": id}))
        .await
        .expect("break");
}

#[tokio::test]
async fn unknown_sessions_and_devices_are_reported_plainly() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    for (method, params) in [
        ("terminal.attach", json!({"session_id": "nope"})),
        ("terminal.snapshot", json!({"session_id": "nope"})),
        (
            "terminal.resize",
            json!({"session_id": "nope", "cols": 80, "rows": 24}),
        ),
        ("terminal.close", json!({"session_id": "nope"})),
        ("terminal.send_break", json!({"session_id": "nope"})),
    ] {
        let err = app.rpc(method, params).await.unwrap_err();
        assert_eq!(err.code, "not_found", "{method}");
        assert!(err.message.contains("nope"), "{method}: {err}");
    }
    let no_target = app.rpc("terminal.open", json!({})).await.unwrap_err();
    assert_eq!(no_target.code, "invalid");
    assert!(no_target.message.contains("device_id"), "{no_target}");

    let both = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("SW1"), "adhoc": {"host": "x"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(both.code, "invalid");

    let missing = app
        .rpc("terminal.open", json!({"device_id": "ghost"}))
        .await
        .unwrap_err();
    assert_eq!(missing.code, "not_found");
}

#[tokio::test]
async fn the_number_of_sessions_is_capped() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let mut ids = Vec::new();
    for _ in 0..64 {
        ids.push(session_id(
            &open_device(app, &lab.id("SW1")).await.expect("open"),
        ));
    }
    let refused = open_device(app, &lab.id("SW1")).await.unwrap_err();
    assert_eq!(refused.code, "limit");
    assert!(refused.message.contains("64"), "{refused}");

    // A closed session makes room; a live one never does.
    let mut events = Events::new(app);
    app.rpc(
        "terminal.input",
        json!({"session_id": ids[0], "data": "exit\r"}),
    )
    .await
    .expect("input");
    let closed = events.wait_topic("terminal.closed").await;
    assert_eq!(closed.payload["session_id"], ids[0].as_str());
    let replacement = open_device(app, &lab.id("SW1")).await.expect("open");
    assert_ne!(session_id(&replacement), ids[0]);

    for id in ids.iter().skip(1) {
        let _ = app.rpc("terminal.close", json!({"session_id": id})).await;
    }
}

#[tokio::test]
async fn closing_the_app_closes_every_session() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    events.wait_for(&id, "SW1>").await;

    app.shutdown();
    let closed = tokio::time::timeout(PATIENCE, events.wait_topic("terminal.closed"))
        .await
        .expect("the session should close at shutdown");
    assert_eq!(closed.payload["session_id"], id.as_str());
    assert!(
        closed.payload["reason"]
            .as_str()
            .expect("reason")
            .contains("shutting down"),
        "{}",
        closed.payload
    );
}

fn base64_decode(text: &str) -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .expect("base64")
}
