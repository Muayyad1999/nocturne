//! Snippets with variables and variants: saving, listing for a session,
//! rendering, sending on the record, and taking a library out and in.
//!
//! Everything goes through `App::rpc`. What a device was sent is read from
//! the device: a `FakeDevice` says which commands it ran, and a scripted
//! console keeps every byte, which is the only way to tell the carriage
//! return a keyboard sends from a line feed.

mod common;

use std::io::Write as _;
use std::sync::Arc;

use common::{Events, Heard, Script, audit_text, lab, open_device, scripted, session_id};
use nocturne_app::store::{LogEntry, LogQuery};
use nocturne_app::testing;
use nocturne_app::{App, AppError, AppResult};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use sha2::Digest as _;

/// `App::rpc`, and when `NOCTURNE_WIRE` names a file the call is appended to
/// it as it went over the wire: the interface's types are written from real
/// answers, and this is where they come from.
async fn call(app: &App, method: &str, params: Value) -> AppResult<Value> {
    let result = app.rpc(method, params.clone()).await;
    if let Ok(path) = std::env::var("NOCTURNE_WIRE")
        && let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        let answer = match &result {
            Ok(value) => json!({"result": value}),
            Err(err) => {
                json!({"error": {"code": err.code, "message": err.message, "details": err.details}})
            }
        };
        let _ = writeln!(
            file,
            "{}",
            json!({"method": method, "params": params, "answer": answer})
        );
    }
    result
}

async fn save(app: &App, params: Value) -> Value {
    call(app, "terminal.snippets.save", params)
        .await
        .expect("save")
}

async fn refused(app: &App, method: &str, params: Value) -> AppError {
    match call(app, method, params.clone()).await {
        Ok(value) => panic!("{method} {params} was accepted: {value}"),
        Err(err) => err,
    }
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().expect("an id").to_string()
}

fn sha256(text: &str) -> String {
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn audit_entries(app: &App, action: &str) -> Vec<LogEntry> {
    app.store()
        .log_query(&LogQuery {
            stream: Some("audit".into()),
            ..LogQuery::default()
        })
        .expect("audit")
        .into_iter()
        .filter(|entry| entry.source == action)
        .collect()
}

/// A second device type, so that a snippet can be for "another" one.
fn add_other_os(app: &App) {
    let yaml = testing::LAB_IOS
        .replace("id: test_ios", "id: other_os")
        .replace("name: Test IOS", "name: Other OS");
    nocturne_app::profiles::save(app, &yaml).expect("the second profile loads");
}

/// A console that says nothing after its prompt and keeps what it is sent.
fn listening(prompt: &str) -> Script {
    Script {
        banner: prompt.to_string(),
        listen_only: true,
        ..Script::default()
    }
}

/// Open a session to a scripted console and wait for its prompt.
async fn open_console(lab: &common::Scripted, node: &str, prompt: &str) -> String {
    let mut events = Events::new(&lab.app);
    let id = session_id(&open_device(&lab.app, &lab.id(node)).await.expect("open"));
    events.wait_for(&id, prompt).await;
    id
}

async fn heard_bytes(heard: &Arc<Heard>, count: usize) -> String {
    common::wait_until(&format!("the console to be sent {count} bytes"), || {
        heard.raw.lock().len() >= count
    })
    .await;
    heard.raw()
}

/// Render and send in one go, as the interface does once the person agreed.
async fn confirm_and_send(
    app: &App,
    id: &str,
    values: Value,
    session_ids: &[&str],
) -> AppResult<Value> {
    let rendered = call(
        app,
        "terminal.snippets.render",
        json!({"id": id, "values": values}),
    )
    .await?;
    call(
        app,
        "terminal.snippets.send",
        json!({
            "id": id,
            "values": values,
            "session_ids": session_ids,
            "expect_sha256": rendered["sha256"],
        }),
    )
    .await
}

// ------------------------------------------------------------ variables

#[tokio::test]
async fn the_variables_of_a_snippet_are_found_on_save_and_listed_with_it() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let saved = save(
        app,
        json!({
            "name": "Access port",
            "text": "interface {{port}}\n description {{descr=}}\n switchport access vlan {{vlan=10}}\n",
            "profile": "test_ios",
            "tags": ["ports"],
        }),
    )
    .await;
    let variables = json!([
        {"name": "port"},
        {"name": "descr", "default": ""},
        {"name": "vlan", "default": "10"},
    ]);
    // No default and an empty default are different things: one must be
    // given a value, the other may be left out.
    assert_eq!(saved["variables"], variables);
    assert!(saved["variables"][0].get("default").is_none());

    save(app, json!({"name": "Clock", "text": "show clock"})).await;
    let listed = call(app, "terminal.snippets.list", json!({}))
        .await
        .expect("list");
    assert_eq!(listed[0]["name"], "Access port");
    assert_eq!(listed[0]["variables"], variables);
    // The text is stored as it was written, placeholders and all.
    assert_eq!(
        listed[0]["text"],
        "interface {{port}}\n description {{descr=}}\n switchport access vlan {{vlan=10}}\n"
    );
    assert_eq!(listed[1]["name"], "Clock");
    assert_eq!(listed[1]["variables"], json!([]));
    // Nothing is wrong with either, and nobody asked which one a session gets.
    for row in listed.as_array().expect("a list") {
        assert!(row.get("problem").is_none(), "{row}");
        assert!(row.get("chosen").is_none(), "{row}");
    }
}

#[tokio::test]
async fn a_malformed_placeholder_is_refused_on_save_with_where_it_is() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    for (text, line, column) in [
        ("interface {{port}}\n description {{descr", 2, 14),
        ("ping {{ host }}", 1, 6),
        ("docker ps --format '{{.Names}}'", 1, 21),
        ("vlan {{id=10}}\nname {{id=20}}", 2, 6),
        ("show clock\u{1b}[201~", 1, 11),
    ] {
        let err = refused(
            app,
            "terminal.snippets.save",
            json!({"name": "Broken", "text": text}),
        )
        .await;
        assert_eq!(err.code, "invalid", "{text:?}");
        assert!(
            err.message
                .contains(&format!("line {line}, column {column}")),
            "{text:?}: {err}"
        );
        assert_eq!(
            err.details,
            Some(json!({"line": line, "column": column})),
            "{text:?}"
        );
    }
    // Refused means not stored.
    assert_eq!(
        call(app, "terminal.snippets.list", json!({}))
            .await
            .expect("list"),
        json!([])
    );
    // The same text is accepted once the braces are marked as text.
    let literal = save(
        app,
        json!({"name": "Containers", "text": "docker ps --format '\\{{.Names}}'"}),
    )
    .await;
    assert_eq!(literal["variables"], json!([]));

    // The other things a snippet may not be.
    for params in [
        json!({"name": "x", "text": "\n\n"}),
        json!({"name": "two\nlines", "text": "y"}),
        json!({"name": "x", "text": "y", "tags": ["a\u{7}b"]}),
        json!({"name": "x", "text": "y", "tags": ["t".repeat(65)]}),
        json!({"name": "x", "text": "y", "tags": (0..33).map(|n| format!("t{n}")).collect::<Vec<_>>()}),
        json!({"name": "n".repeat(201), "text": "y"}),
    ] {
        assert_eq!(
            refused(app, "terminal.snippets.save", params.clone())
                .await
                .code,
            "invalid",
            "{params}"
        );
    }
}

#[tokio::test]
async fn snippets_stored_before_variables_are_read_by_the_rules_of_today() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // As the first version stored them: the same record, written straight
    // into the collection so that no check of today's has seen it.
    let old = |id: &str, name: &str, text: &str| {
        app.store()
            .put(
                "terminal_snippets",
                id,
                &json!({
                    "id": id, "name": name, "text": text, "profile": null,
                    "tags": ["old"], "created_at": 1_700_000_000_000i64,
                    "updated_at": 1_700_000_000_000i64,
                }),
            )
            .expect("an old snippet");
    };
    old("plain", "Plain", "show ip interface brief\n");
    old("names", "Containers", "docker ps --format '{{.Names}}'");
    old("twin1", "Twin", "show clock");
    old("twin2", "Twin", "show version");

    let listed = call(app, "terminal.snippets.list", json!({}))
        .await
        .expect("list");
    let row = |id: &str| {
        listed
            .as_array()
            .expect("a list")
            .iter()
            .find(|row| row["id"] == id)
            .cloned()
            .expect("the snippet is listed")
    };
    // Plain text is what it was: no variables, nothing wrong, and it renders
    // to itself (without the line break at its end, which was never sent).
    assert_eq!(row("plain")["variables"], json!([]));
    assert!(row("plain").get("problem").is_none());
    let rendered = call(app, "terminal.snippets.render", json!({"id": "plain"}))
        .await
        .expect("render");
    assert_eq!(rendered["text"], "show ip interface brief");

    // One whose braces are not a placeholder is still there, with what is
    // wrong with it, and is not typed until somebody has looked at it.
    let problem = row("names")["problem"]
        .as_str()
        .expect("a problem")
        .to_string();
    assert!(problem.starts_with("line 1, column 21: "), "{problem}");
    assert_eq!(row("names")["text"], "docker ps --format '{{.Names}}'");
    let err = refused(app, "terminal.snippets.render", json!({"id": "names"})).await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("line 1, column 21"), "{err}");
    let err = refused(
        app,
        "terminal.snippets.send",
        json!({"id": "names", "session_ids": ["any"], "expect_sha256": "0"}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    // Corrected and saved again, it is a snippet like any other.
    let fixed = save(
        app,
        json!({"id": "names", "name": "Containers", "tags": ["old"],
               "text": "docker ps --format '\\{{.Names}}'"}),
    )
    .await;
    assert!(fixed.get("problem").is_none(), "{fixed}");
    let rendered = call(app, "terminal.snippets.render", json!({"id": "names"}))
        .await
        .expect("render");
    assert_eq!(rendered["text"], "docker ps --format '{{.Names}}'");

    // Two that share a name and a device type, from before that was refused:
    // both are listed, both can be used, and each can be edited where it is.
    assert_eq!(row("twin1")["name"], row("twin2")["name"]);
    let edited = save(
        app,
        json!({"id": "twin1", "name": "Twin", "text": "show clock detail"}),
    )
    .await;
    assert_eq!(edited["text"], "show clock detail");
    // A third is not let in, and neither is anything renamed to join them.
    let err = refused(
        app,
        "terminal.snippets.save",
        json!({"name": "twin", "text": "x"}),
    )
    .await;
    assert_eq!(err.code, "conflict");
    let err = refused(
        app,
        "terminal.snippets.save",
        json!({"id": "plain", "name": "Twin", "text": "x"}),
    )
    .await;
    assert_eq!(err.code, "conflict");
}

// -------------------------------------------------------------- variants

#[tokio::test]
async fn two_snippets_share_a_name_only_when_their_device_types_differ() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    add_other_os(app);
    let generic = save(app, json!({"name": "Port bounce", "text": "generic"})).await;
    save(
        app,
        json!({"name": "Port bounce", "text": "ios", "profile": "test_ios"}),
    )
    .await;
    save(
        app,
        json!({"name": "port BOUNCE", "text": "other", "profile": "other_os"}),
    )
    .await;

    // A second one for the same device type is one too many, whatever its case.
    for params in [
        json!({"name": "PORT bounce", "text": "again"}),
        json!({"name": " Port bounce ", "text": "again", "profile": "test_ios"}),
    ] {
        let err = refused(app, "terminal.snippets.save", params.clone()).await;
        assert_eq!(err.code, "conflict", "{params}");
        assert!(err.message.contains("Port bounce"), "{err}");
    }
    // So is moving one onto another's device type, or renaming one into it.
    let other = save(app, json!({"name": "Other", "text": "x"})).await;
    for params in [
        json!({"id": id_of(&generic), "name": "Port bounce", "text": "generic", "profile": "test_ios"}),
        json!({"id": id_of(&other), "name": "Port Bounce", "text": "x"}),
    ] {
        assert_eq!(
            refused(app, "terminal.snippets.save", params.clone())
                .await
                .code,
            "conflict",
            "{params}"
        );
    }
    // Editing one where it stands is not a conflict with itself.
    let edited = save(
        app,
        json!({"id": id_of(&generic), "name": "Port bounce", "text": "generic, edited"}),
    )
    .await;
    assert_eq!(edited["text"], "generic, edited");
    assert_eq!(
        call(app, "terminal.snippets.list", json!({}))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .len(),
        4
    );
}

#[tokio::test]
async fn for_a_session_the_snippet_for_its_device_type_wins_over_the_generic_one() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    add_other_os(app);
    let generic = id_of(
        &save(
            app,
            json!({"name": "Port bounce", "text": "generic bounce"}),
        )
        .await,
    );
    let ios = id_of(
        &save(
            app,
            json!({"name": "Port bounce", "text": "ios bounce", "profile": "test_ios"}),
        )
        .await,
    );
    let other = id_of(
        &save(
            app,
            json!({"name": "Port bounce", "text": "other bounce", "profile": "other_os"}),
        )
        .await,
    );
    let clock = id_of(&save(app, json!({"name": "Clock", "text": "show clock"})).await);
    let only_other = id_of(
        &save(
            app,
            json!({"name": "Other only", "text": "x", "profile": "other_os"}),
        )
        .await,
    );

    let session = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    let choice = |listed: &Value| -> Vec<(String, Value, Value)> {
        listed
            .as_array()
            .expect("a list")
            .iter()
            .map(|row| {
                (
                    id_of(row),
                    row.get("chosen").cloned().unwrap_or(Value::Null),
                    row.get("chosen_instead").cloned().unwrap_or(Value::Null),
                )
            })
            .collect()
    };

    // SW1 is a test_ios device: its own variant is the one, the generic twin
    // steps back and says for whom, and what is for another type is not offered.
    let for_session = call(
        app,
        "terminal.snippets.list",
        json!({"session_id": session}),
    )
    .await
    .expect("list");
    let expected = vec![
        (clock.clone(), json!(true), Value::Null),
        (generic.clone(), json!(false), json!(ios)),
        (ios.clone(), json!(true), Value::Null),
    ];
    assert_eq!(choice(&for_session), expected);
    // Asking by device type is the same question.
    let for_type = call(
        app,
        "terminal.snippets.list",
        json!({"profile": "test_ios"}),
    )
    .await
    .expect("list");
    assert_eq!(choice(&for_type), expected);
    // A search narrows the answer and does not change the choice: the
    // generic one is still second to a variant the words do not match.
    let searched = call(
        app,
        "terminal.snippets.list",
        json!({"session_id": session, "query": "generic"}),
    )
    .await
    .expect("list");
    assert_eq!(
        choice(&searched),
        vec![(generic.clone(), json!(false), json!(ios))]
    );

    // A device nothing is known about: everything is offered, and only what
    // is for any device is marked as right for it.
    let untyped = call(
        app,
        "devices.create",
        json!({"name": "SW1b", "lab": {"lab_id": "testlab", "node": "SW1"}}),
    )
    .await
    .expect("device");
    let unknown = session_id(&open_device(app, &id_of(&untyped)).await.expect("open"));
    let for_unknown = call(
        app,
        "terminal.snippets.list",
        json!({"session_id": unknown}),
    )
    .await
    .expect("list");
    assert_eq!(
        choice(&for_unknown),
        vec![
            (clock, json!(true), Value::Null),
            (only_other, json!(false), Value::Null),
            (generic, json!(true), Value::Null),
            (other, json!(false), Value::Null),
            (ios, json!(false), Value::Null),
        ]
    );

    // Asked for nobody, there is no choice to report.
    let all = call(app, "terminal.snippets.list", json!({}))
        .await
        .expect("list");
    assert_eq!(all.as_array().expect("a list").len(), 5);
    assert!(
        choice(&all)
            .iter()
            .all(|(_, chosen, _)| *chosen == Value::Null)
    );

    let err = refused(
        app,
        "terminal.snippets.list",
        json!({"session_id": session, "profile": "test_ios"}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    let err = refused(app, "terminal.snippets.list", json!({"session_id": "nope"})).await;
    assert_eq!(err.code, "not_found");
}

// ---------------------------------------------------------------- render

#[tokio::test]
async fn rendering_puts_the_values_in_and_pins_the_text_with_its_hash() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = id_of(
        &save(
            app,
            json!({
                "name": "Access port",
                "text": "interface {{port}}\r\n description {{descr=desk}}\r\n switchport access vlan {{vlan}}\r\n",
            }),
        )
        .await,
    );
    let rendered = call(
        app,
        "terminal.snippets.render",
        json!({"id": id, "values": {"port": "Gi0/2", "vlan": "20"}}),
    )
    .await
    .expect("render");
    let text = "interface Gi0/2\n description desk\n switchport access vlan 20";
    assert_eq!(
        rendered,
        json!({
            "text": text,
            "lines": 3,
            "sha256": sha256(text),
            "variables": [
                {"name": "port"},
                {"name": "descr", "default": "desk"},
                {"name": "vlan"},
            ],
        })
    );
    // Another value is another text and another hash.
    let again = call(
        app,
        "terminal.snippets.render",
        json!({"id": id, "values": {"port": "Gi0/3", "vlan": "20", "descr": ""}}),
    )
    .await
    .expect("render");
    assert_eq!(
        again["text"],
        "interface Gi0/3\n description \n switchport access vlan 20"
    );
    assert_ne!(again["sha256"], rendered["sha256"]);
    // Rendering types nothing and records nothing.
    assert!(audit_entries(app, "terminal.snippet.send").is_empty());

    let err = refused(app, "terminal.snippets.render", json!({"id": "ghost"})).await;
    assert_eq!(err.code, "not_found");
}

#[tokio::test]
async fn rendering_is_strict_and_says_which_value_is_wrong_without_repeating_it() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = id_of(
        &save(
            app,
            json!({"name": "User", "text": "username {{user}} secret {{password}}\n"}),
        )
        .await,
    );
    let secret = "Zq7-hunter2-Xk";
    for (values, problems) in [
        (
            json!({"user": "ops"}),
            json!([{"name": "password", "problem": "missing"}]),
        ),
        (
            json!({"user": "ops", "password": secret, "vlan": "10"}),
            json!([{"name": "vlan", "problem": "unknown"}]),
        ),
        (
            json!({"user": "ops", "password": format!("{secret}\nreload")}),
            json!([{"name": "password", "problem": "line_break"}]),
        ),
        (
            json!({"user": "ops", "password": format!("{secret}\rreload")}),
            json!([{"name": "password", "problem": "line_break"}]),
        ),
        (
            json!({"user": "ops", "password": format!("{secret}\u{3}")}),
            json!([{"name": "password", "problem": "control_character"}]),
        ),
        (
            json!({"user": "ops", "password": format!("{secret}\u{1b}[201~")}),
            json!([{"name": "password", "problem": "control_character"}]),
        ),
        (
            json!({"user": "ops\ttab", "password": 12}),
            json!([
                {"name": "user", "problem": "control_character"},
                {"name": "password", "problem": "not_text"},
            ]),
        ),
        (
            json!({"user": "u".repeat(1025), "password": secret}),
            json!([{"name": "user", "problem": "too_long"}]),
        ),
    ] {
        let err = refused(
            app,
            "terminal.snippets.render",
            json!({"id": id, "values": values}),
        )
        .await;
        assert_eq!(err.code, "invalid", "{values}");
        assert_eq!(
            err.details,
            Some(json!({"variables": problems})),
            "{values}"
        );
        for problem in problems.as_array().expect("problems") {
            let name = problem["name"].as_str().expect("a name");
            assert!(err.message.contains(&format!("`{name}`")), "{err}");
        }
        // Errors are logged; a value is not.
        assert!(!err.message.contains(secret), "{err}");
        assert!(!err.message.contains("reload"), "{err}");

        // And `send` renders by the same rules before it looks at anything else.
        let err = refused(
            app,
            "terminal.snippets.send",
            json!({"id": id, "values": values, "session_ids": ["any"], "expect_sha256": "0"}),
        )
        .await;
        assert_eq!(err.code, "invalid", "{values}");
        assert_eq!(err.details, Some(json!({"variables": problems})));
    }
}

// ------------------------------------------------------------------ send

#[tokio::test]
async fn a_confirmed_snippet_is_typed_as_a_keyboard_would_and_no_enter_is_pressed_for_it() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    lab.fake("SW1").with(|device| {
        device
            .outputs
            .insert("show clock".into(), "12:00:00.000 UTC\n".into());
        device
            .outputs
            .insert("show uptime".into(), "up 3 weeks\n".into());
    });
    let mut events = Events::new(app);
    let session = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    events.wait_for(&session, "SW1>").await;
    lab.fake("SW1").clear_seen();

    // Stored with the line endings of a Windows editor and a last line break.
    let id = id_of(
        &save(
            app,
            json!({"name": "Look", "text": "show {{what=clock}}\r\nshow uptime\r\n"}),
        )
        .await,
    );
    let sent = confirm_and_send(app, &id, json!({}), &[&session])
        .await
        .expect("send");
    assert_eq!(
        sent,
        json!({"sent": [{"session_id": session, "lines": 2}], "failed": []})
    );

    // The first line ran, once: its line break was one Enter, not the two a
    // CRLF would have been. The second line is typed and waits at the prompt.
    events.wait_for(&session, "UTC\r\nSW1>show uptime").await;
    assert_eq!(lab.fake("SW1").seen(), ["show clock"]);
    assert!(!events.text(&session).contains("up 3 weeks"));
    // The Enter is the person's.
    app.rpc(
        "terminal.input",
        json!({"session_id": session, "data": "\r"}),
    )
    .await
    .expect("input");
    events.wait_for(&session, "up 3 weeks").await;
    assert_eq!(lab.fake("SW1").seen(), ["show clock", "show uptime"]);
}

#[tokio::test]
async fn the_bytes_a_send_hands_over_are_the_text_with_one_carriage_return_per_line_break() {
    let lab = scripted(&[("CON1", listening("SW1>"))], false).await;
    let app = &lab.app;
    let session = open_console(&lab, "CON1", "SW1>").await;
    let id = id_of(
        &save(
            app,
            json!({
                "name": "Bounce",
                "text": "interface {{port}}\r\n shutdown\n\n no shutdown\rend\n",
            }),
        )
        .await,
    );
    confirm_and_send(app, &id, json!({"port": "Gi0/2"}), &[&session])
        .await
        .expect("send");
    let typed = "interface Gi0/2\r shutdown\r\r no shutdown\rend";
    assert_eq!(heard_bytes(&lab.heard("CON1"), typed.len()).await, typed);
    // That is all of it: no line feed anywhere, and nothing after the last
    // word — `end` is typed, not entered.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(lab.heard("CON1").raw(), typed);
    // What was typed counts as the session's output like any typing.
    let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
    assert_eq!(listed[0]["bytes_out"], typed.len());
}

#[tokio::test]
async fn a_send_is_refused_when_the_text_is_not_the_one_that_was_confirmed() {
    let lab = scripted(&[("CON1", listening("SW1>"))], false).await;
    let app = &lab.app;
    let session = open_console(&lab, "CON1", "SW1>").await;
    let id = id_of(
        &save(
            app,
            json!({"name": "Vlan", "text": "vlan {{id}}\n name users\n"}),
        )
        .await,
    );
    let confirmed = call(
        app,
        "terminal.snippets.render",
        json!({"id": id, "values": {"id": "10"}}),
    )
    .await
    .expect("render");
    let send = |values: Value, expect: Value| {
        call(
            app,
            "terminal.snippets.send",
            json!({"id": id, "values": values, "session_ids": [session], "expect_sha256": expect}),
        )
    };

    // Another value than the one that was shown.
    let err = send(json!({"id": "20"}), confirmed["sha256"].clone())
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict");
    // A hash of something else, or of nothing.
    for expect in [json!(sha256("vlan 10")), json!(""), json!("0")] {
        let err = send(json!({"id": "10"}), expect).await.unwrap_err();
        assert_eq!(err.code, "conflict");
    }
    // No hash at all is not a confirmation either.
    let err = refused(
        app,
        "terminal.snippets.send",
        json!({"id": id, "values": {"id": "10"}, "session_ids": [session]}),
    )
    .await;
    assert_eq!(err.code, "invalid");

    // Somebody edits the snippet between the confirmation and the send.
    save(
        app,
        json!({"id": id, "name": "Vlan", "text": "vlan {{id}}\n name users\nreload\n"}),
    )
    .await;
    let err = send(json!({"id": "10"}), confirmed["sha256"].clone())
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("Vlan"), "{err}");
    // The refusal does not carry the new text, only what to compare with.
    let now = call(
        app,
        "terminal.snippets.render",
        json!({"id": id, "values": {"id": "10"}}),
    )
    .await
    .expect("render");
    assert_eq!(err.details, Some(json!({"sha256": now["sha256"]})));

    // Through all of that the device was sent nothing and nothing was recorded.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(lab.heard("CON1").raw(), "");
    assert!(audit_entries(app, "terminal.snippet.send").is_empty());

    // Confirmed again as it is now, it goes; the hash may come in either case.
    let upper = now["sha256"].as_str().expect("hash").to_uppercase();
    send(json!({"id": "10"}), json!(upper)).await.expect("send");
    let typed = "vlan 10\r name users\rreload";
    assert_eq!(heard_bytes(&lab.heard("CON1"), typed.len()).await, typed);
}

#[tokio::test]
async fn a_send_reaches_every_session_named_or_none_of_them() {
    let lab = scripted(
        &[
            ("CON1", listening("SW1>")),
            ("CON2", listening("SW2>")),
            // Logs in without being asked and hangs up on `exit`.
            (
                "CON3",
                Script {
                    ask_username: false,
                    prompt: "SW3>".into(),
                    ..Script::default()
                },
            ),
        ],
        false,
    )
    .await;
    let app = &lab.app;
    let one = open_console(&lab, "CON1", "SW1>").await;
    let two = open_console(&lab, "CON2", "SW2>").await;
    let id = id_of(
        &save(
            app,
            json!({"name": "Clock", "text": "show clock\nshow ntp\n"}),
        )
        .await,
    );

    // A session that has ended but is still listed.
    let mut events = Events::new(app);
    let ended = session_id(&open_device(app, &lab.id("CON3")).await.expect("open"));
    events.wait_for(&ended, "Password: ").await;
    for line in ["pw\r", "exit\r"] {
        app.rpc("terminal.input", json!({"session_id": ended, "data": line}))
            .await
            .expect("input");
        events.wait_for(&ended, "SW3>").await;
    }
    events.wait_topic("terminal.closed").await;

    for (sessions, code, culprit) in [
        (
            vec![one.as_str(), ended.as_str(), two.as_str()],
            "closed",
            ended.as_str(),
        ),
        (
            vec![one.as_str(), two.as_str(), "nope"],
            "not_found",
            "nope",
        ),
        (vec![ended.as_str()], "closed", ended.as_str()),
    ] {
        let err = confirm_and_send(app, &id, json!({}), &sessions)
            .await
            .unwrap_err();
        assert_eq!(err.code, code, "{sessions:?}");
        let details = err.details.clone().expect("details");
        assert_eq!(details["refused"].as_array().expect("refused").len(), 1);
        assert_eq!(details["refused"][0]["session_id"], culprit);
        assert_eq!(details["refused"][0]["code"], code);
        if sessions.len() > 1 {
            assert!(err.message.contains("nothing was sent"), "{err}");
        }
    }
    // The same session twice, no session, too many.
    for sessions in [vec![one.as_str(), one.as_str()], vec![]] {
        let err = confirm_and_send(app, &id, json!({}), &sessions)
            .await
            .unwrap_err();
        assert_eq!(err.code, "invalid", "{sessions:?}");
    }
    let crowd: Vec<String> = (0..65).map(|n| format!("s{n}")).collect();
    let crowd: Vec<&str> = crowd.iter().map(String::as_str).collect();
    assert_eq!(
        confirm_and_send(app, &id, json!({}), &crowd)
            .await
            .unwrap_err()
            .code,
        "invalid"
    );

    // Not one byte reached the sessions that were fine, and nothing was recorded.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(lab.heard("CON1").raw(), "");
    assert_eq!(lab.heard("CON2").raw(), "");
    assert!(audit_entries(app, "terminal.snippet.send").is_empty());

    // With every session open, each gets it.
    let sent = confirm_and_send(app, &id, json!({}), &[&two, &one])
        .await
        .expect("send");
    assert_eq!(
        sent,
        json!({
            "sent": [{"session_id": two, "lines": 2}, {"session_id": one, "lines": 2}],
            "failed": [],
        })
    );
    let typed = "show clock\rshow ntp";
    assert_eq!(heard_bytes(&lab.heard("CON1"), typed.len()).await, typed);
    assert_eq!(heard_bytes(&lab.heard("CON2"), typed.len()).await, typed);
}

#[tokio::test]
async fn a_session_that_dies_under_a_send_is_said_and_recorded_as_not_sent() {
    // The second console's line is up but nothing takes what is written to
    // it: open when the send looks, gone when it types.
    let lab = scripted(
        &[
            ("CON1", listening("SW1>")),
            (
                "DEAD",
                Script {
                    banner: "SW2>".into(),
                    deaf: true,
                    ..Script::default()
                },
            ),
        ],
        false,
    )
    .await;
    let app = &lab.app;
    let one = open_console(&lab, "CON1", "SW1>").await;
    let dead = open_console(&lab, "DEAD", "SW2>").await;
    let id = id_of(&save(app, json!({"name": "Clock", "text": "show clock"})).await);

    let sent = confirm_and_send(app, &id, json!({}), &[&dead, &one])
        .await
        .expect("send");
    assert_eq!(sent["sent"], json!([{"session_id": one, "lines": 1}]));
    assert_eq!(sent["failed"].as_array().expect("failed").len(), 1);
    assert_eq!(sent["failed"][0]["session_id"], dead);
    assert_eq!(sent["failed"][0]["code"], "closed");
    assert_eq!(heard_bytes(&lab.heard("CON1"), 10).await, "show clock");

    // The record says where it went and where it did not.
    let entries = audit_entries(app, "terminal.snippet.send");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].message,
        "Sent the terminal snippet `Clock` (1 line) to 2 sessions: DEAD, CON1 (1 of them did not take it)"
    );
    let body = entries[0].body.as_ref().expect("a body");
    assert_eq!(body["sessions"][0]["session_id"], dead);
    assert_eq!(body["sessions"][0]["sent"], false);
    assert_eq!(body["sessions"][1]["sent"], true);

    // When nobody took it the entry does not read as a send at all.
    let sent = confirm_and_send(app, &id, json!({}), &[&dead])
        .await
        .expect("send");
    assert_eq!(sent["sent"], json!([]));
    assert_eq!(sent["failed"][0]["session_id"], dead);
    let entries = audit_entries(app, "terminal.snippet.send");
    assert_eq!(
        entries[0].message,
        "The terminal snippet `Clock` (1 line) was not taken by DEAD"
    );
    assert_eq!(
        entries[0].body.as_ref().expect("a body")["sessions"][0]["sent"],
        false
    );
}

#[tokio::test]
async fn a_send_is_recorded_by_name_place_size_and_hash_and_never_by_what_it_said() {
    let lab = lab(&["SW1", "SW2"]).await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let one = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    let two = session_id(&open_device(app, &lab.id("SW2")).await.expect("open"));
    events.wait_for(&one, "SW1>").await;
    events.wait_for(&two, "SW2>").await;

    let secret = "Zq7-hunter2-Xk";
    let id = id_of(
        &save(
            app,
            json!({
                "name": "Local user",
                "text": "username {{user}} privilege 15 secret {{password}}\n",
                "profile": "test_ios",
            }),
        )
        .await,
    );
    let values = json!({"user": "ops", "password": secret});
    let rendered = call(
        app,
        "terminal.snippets.render",
        json!({"id": id, "values": values}),
    )
    .await
    .expect("render");
    confirm_and_send(app, &id, values.clone(), &[&one])
        .await
        .expect("send");
    events.wait_for(&one, secret).await;

    let entries = audit_entries(app, "terminal.snippet.send");
    assert_eq!(entries.len(), 1);
    let entry = &entries[0];
    assert_eq!(entry.device_id, Some(lab.id("SW1")));
    assert_eq!(
        entry.message,
        "Sent the terminal snippet `Local user` (1 line) to SW1"
    );
    assert_eq!(
        entry.body,
        Some(json!({
            "snippet_id": id,
            "name": "Local user",
            "profile": "test_ios",
            "lines": 1,
            "sha256": rendered["sha256"],
            "sessions": [{
                "session_id": one,
                "title": "SW1",
                "device_id": lab.id("SW1"),
                "lab": true,
                "sent": true,
            }],
            "device_ids": [lab.id("SW1")],
        }))
    );

    // To two devices it is still one entry, about neither of them alone.
    confirm_and_send(app, &id, values, &[&one, &two])
        .await
        .expect("send");
    events.wait_for(&two, secret).await;
    let entries = audit_entries(app, "terminal.snippet.send");
    assert_eq!(entries.len(), 2);
    // Newest first.
    let entry = &entries[0];
    assert_eq!(entry.device_id, None);
    assert_eq!(
        entry.message,
        "Sent the terminal snippet `Local user` (1 line) to 2 sessions: SW1, SW2"
    );
    let body = entry.body.as_ref().expect("a body");
    let mut devices = vec![lab.id("SW1"), lab.id("SW2")];
    devices.sort();
    assert_eq!(body["device_ids"], json!(devices));
    assert_eq!(body["sessions"][1]["session_id"], two);

    // Neither the value, nor the line it was put into, nor the template is
    // anywhere in the audit trail or in an event — the terminal's own output
    // aside, which is the device echoing what it was sent.
    let audit = audit_text(app);
    events.drain();
    let published: String = events
        .seen
        .iter()
        .filter(|event| event.topic != "terminal.output")
        .map(|event| event.payload.to_string())
        .collect();
    for forbidden in [secret, "privilege 15", "username ops", "{{password}}"] {
        assert!(
            !audit.contains(forbidden),
            "{forbidden} is in the audit trail"
        );
        assert!(!published.contains(forbidden), "{forbidden} is in an event");
    }
}

#[tokio::test]
async fn a_snippet_for_one_device_type_is_not_typed_into_a_device_of_another() {
    let lab = scripted(&[("CON1", listening("SW1>"))], false).await;
    let app = &lab.app;
    add_other_os(app);
    let session = open_console(&lab, "CON1", "SW1>").await;
    let other = id_of(
        &save(
            app,
            json!({"name": "Reset", "text": "/system reset", "profile": "other_os"}),
        )
        .await,
    );
    let err = confirm_and_send(app, &other, json!({}), &[&session])
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("other_os") && err.message.contains("test_ios"),
        "{err}"
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(lab.heard("CON1").raw(), "");

    // Its own type, and one for any device, are fine.
    let own = id_of(
        &save(
            app,
            json!({"name": "Reset", "text": "reload", "profile": "test_ios"}),
        )
        .await,
    );
    confirm_and_send(app, &own, json!({}), &[&session])
        .await
        .expect("send");
    assert_eq!(heard_bytes(&lab.heard("CON1"), 6).await, "reload");
}

#[tokio::test]
async fn a_session_that_asked_for_bracketed_paste_gets_a_snippet_between_the_markers() {
    // A shell says so when it starts; a network device never does.
    let lab = scripted(
        &[
            ("SHELL", listening("\u{1b}[?2004h$ ")),
            ("CON1", listening("SW1>")),
        ],
        false,
    )
    .await;
    let app = &lab.app;
    let shell = open_console(&lab, "SHELL", "$ ").await;
    let console = open_console(&lab, "CON1", "SW1>").await;
    let id = id_of(&save(app, json!({"name": "Two", "text": "date\nuptime\n"})).await);

    // Alone, the shell gets the lines held together, and runs none of them.
    confirm_and_send(app, &id, json!({}), &[&shell])
        .await
        .expect("send");
    let held = "\u{1b}[200~date\ruptime\u{1b}[201~";
    assert_eq!(heard_bytes(&lab.heard("SHELL"), held.len()).await, held);
    // The device that did not ask gets the text and nothing around it.
    confirm_and_send(app, &id, json!({}), &[&console])
        .await
        .expect("send");
    assert_eq!(heard_bytes(&lab.heard("CON1"), 11).await, "date\ruptime");

    // Sent to both at once, neither is bracketed: to the one that did not
    // ask, the markers would be six characters typed at its prompt.
    confirm_and_send(app, &id, json!({}), &[&shell, &console])
        .await
        .expect("send");
    assert_eq!(
        heard_bytes(&lab.heard("SHELL"), held.len() + 11).await,
        format!("{held}date\ruptime")
    );
    assert_eq!(
        heard_bytes(&lab.heard("CON1"), 22).await,
        "date\ruptimedate\ruptime"
    );
}

// --------------------------------------------------------- export, import

/// What a list says of each snippet that a person would compare.
fn library(listed: &Value) -> Vec<Value> {
    listed
        .as_array()
        .expect("a list")
        .iter()
        .map(|row| {
            json!({
                "name": row["name"], "text": row["text"],
                "profile": row["profile"], "tags": row["tags"],
            })
        })
        .collect()
}

async fn listed(app: &App) -> Value {
    call(app, "terminal.snippets.list", json!({}))
        .await
        .expect("list")
}

async fn import(app: &App, document: Value, on_conflict: &str, dry_run: bool) -> Value {
    call(
        app,
        "terminal.snippets.import",
        json!({"document": document, "on_conflict": on_conflict, "dry_run": dry_run}),
    )
    .await
    .expect("import")
}

fn document(snippets: Value) -> Value {
    json!({"format": "nocturne.snippets/1", "snippets": snippets})
}

#[tokio::test]
async fn an_export_holds_what_a_person_wrote_and_comes_back_in_whole() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    save(
        app,
        json!({"name": "Port bounce", "text": "interface {{port}}\n shutdown\n no shutdown\n",
               "profile": "test_ios", "tags": ["ports", "fix"]}),
    )
    .await;
    save(app, json!({"name": "Port bounce", "text": "generic"})).await;
    save(app, json!({"name": "Clock", "text": "show clock\r\n"})).await;

    let exported = call(app, "terminal.snippets.export", json!(null))
        .await
        .expect("export");
    assert_eq!(
        exported,
        json!({
            "format": "nocturne.snippets/1",
            "snippets": [
                {"name": "Clock", "text": "show clock\r\n", "profile": null, "tags": []},
                {"name": "Port bounce", "text": "generic", "profile": null, "tags": []},
                {"name": "Port bounce", "text": "interface {{port}}\n shutdown\n no shutdown\n",
                 "profile": "test_ios", "tags": ["fix", "ports"]},
            ],
        })
    );

    // Into an installation that has none of them, as an object or as the
    // text of the file it was saved to.
    for as_text in [false, true] {
        let other = common::lab(&["SW9"]).await;
        let given = if as_text {
            json!(serde_json::to_string_pretty(&exported).expect("json"))
        } else {
            exported.clone()
        };
        let result = import(&other.app, given, "skip", false).await;
        assert_eq!(
            result,
            json!({
                "created": 3, "replaced": 0, "skipped": 0, "renamed": 0,
                "problems": [], "applied": true, "dry_run": false,
            })
        );
        assert_eq!(
            library(&listed(&other.app).await),
            library(&listed(app).await)
        );
        // A second time adds nothing, in any mode: they are all here already.
        for mode in ["skip", "replace", "rename"] {
            let again = import(&other.app, exported.clone(), mode, false).await;
            assert_eq!(
                (&again["created"], &again["replaced"], &again["skipped"]),
                (&json!(0), &json!(0), &json!(3)),
                "{mode}"
            );
        }
        assert_eq!(
            listed(&other.app).await.as_array().expect("a list").len(),
            3
        );
    }
}

#[tokio::test]
async fn an_import_treats_a_snippet_that_is_already_here_as_it_was_asked_to() {
    let incoming = document(json!([
        {"name": "clock", "text": "show clock detail", "tags": ["time"]},
        {"name": "Port bounce", "text": "interface {{port}}\n shutdown", "profile": "test_ios"},
        {"name": "New one", "text": "show version"},
    ]));
    let at_start = |app: &App| {
        let app = app.clone();
        async move {
            let clock = save(
                &app,
                json!({"name": "Clock", "text": "show clock", "tags": ["old"]}),
            )
            .await;
            // The same name for another audience is not the same snippet.
            save(&app, json!({"name": "Port bounce", "text": "generic"})).await;
            clock
        }
    };

    // skip: what is here stays as it is.
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let clock = at_start(app).await;
    let mut events = Events::new(app);
    let result = import(app, incoming.clone(), "skip", false).await;
    assert_eq!(
        result,
        json!({
            "created": 2, "replaced": 0, "skipped": 1, "renamed": 0,
            "problems": [], "applied": true, "dry_run": false,
        })
    );
    assert_eq!(
        library(&listed(app).await),
        [
            json!({"name": "Clock", "text": "show clock", "profile": null, "tags": ["old"]}),
            json!({"name": "New one", "text": "show version", "profile": null, "tags": []}),
            json!({"name": "Port bounce", "text": "generic", "profile": null, "tags": []}),
            json!({"name": "Port bounce", "text": "interface {{port}}\n shutdown", "profile": "test_ios", "tags": []}),
        ]
    );
    // One event for the lot, and one audit entry that counts and does not quote.
    let changed = events.wait_topic("terminal.snippets.changed").await;
    assert_eq!(changed.payload["saved"].as_array().expect("ids").len(), 2);
    let entries = audit_entries(app, "terminal.snippet.import");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].body,
        Some(
            json!({"created": 2, "replaced": 0, "skipped": 1, "renamed": 0, "on_conflict": "skip"})
        )
    );
    assert!(!audit_text(app).contains("show version"));

    // replace: the entry takes the place of the one here, which keeps its id.
    let lab = common::lab(&["SW1"]).await;
    let app = &lab.app;
    let clock_before = at_start(app).await;
    let result = import(app, incoming.clone(), "replace", false).await;
    assert_eq!(
        (&result["created"], &result["replaced"], &result["skipped"]),
        (&json!(2), &json!(1), &json!(0))
    );
    let after = listed(app).await;
    assert_eq!(after[0]["id"], clock_before["id"]);
    assert_eq!(after[0]["created_at"], clock_before["created_at"]);
    assert_eq!(
        library(&after)[0],
        json!({"name": "clock", "text": "show clock detail", "profile": null, "tags": ["time"]})
    );
    assert_eq!(after.as_array().expect("a list").len(), 4);

    // rename: both are kept, the entry under the first name that is free.
    let lab = common::lab(&["SW1"]).await;
    let app = &lab.app;
    at_start(app).await;
    save(app, json!({"name": "Clock (2)", "text": "taken"})).await;
    let result = import(app, incoming.clone(), "rename", false).await;
    assert_eq!(
        result,
        json!({
            "created": 3, "replaced": 0, "skipped": 0, "renamed": 1,
            "problems": [], "applied": true, "dry_run": false,
        })
    );
    let names: Vec<Value> = listed(app)
        .await
        .as_array()
        .expect("a list")
        .iter()
        .map(|row| json!([row["name"], row["text"]]))
        .collect();
    assert_eq!(
        names,
        [
            json!(["Clock", "show clock"]),
            json!(["Clock (2)", "taken"]),
            json!(["clock (3)", "show clock detail"]),
            json!(["New one", "show version"]),
            json!(["Port bounce", "generic"]),
            json!(["Port bounce", "interface {{port}}\n shutdown"]),
        ]
    );

    // No mode named is the one that loses nothing.
    let lab = common::lab(&["SW1"]).await;
    let app = &lab.app;
    at_start(app).await;
    let result = call(
        app,
        "terminal.snippets.import",
        json!({"document": incoming}),
    )
    .await
    .expect("import");
    assert_eq!(
        (&result["created"], &result["skipped"]),
        (&json!(2), &json!(1))
    );
    assert_eq!(listed(app).await[0]["text"], "show clock");
    let _ = clock;
}

#[tokio::test]
async fn a_dry_run_says_what_would_happen_and_changes_nothing() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    save(app, json!({"name": "Clock", "text": "show clock"})).await;
    let before = listed(app).await;
    let audit_before = audit_text(app);
    let incoming = document(json!([
        {"name": "Clock", "text": "show clock detail"},
        {"name": "New one", "text": "show version"},
    ]));
    let mut events = Events::new(app);

    for (mode, counts) in [
        ("skip", json!([1, 0, 1, 0])),
        ("replace", json!([1, 1, 0, 0])),
        ("rename", json!([2, 0, 0, 1])),
    ] {
        let would = import(app, incoming.clone(), mode, true).await;
        assert_eq!(
            json!([
                would["created"],
                would["replaced"],
                would["skipped"],
                would["renamed"]
            ]),
            counts,
            "{mode}"
        );
        assert_eq!(would["applied"], false);
        assert_eq!(would["dry_run"], true);
        assert_eq!(would["problems"], json!([]));
    }
    // Not a snippet, not an audit entry, not an event.
    assert_eq!(listed(app).await, before);
    assert_eq!(audit_text(app), audit_before);
    events.drain();
    assert!(events.of("terminal.snippets.changed").is_empty());

    // And the real thing then does exactly what the dry run said.
    let done = import(app, incoming, "replace", false).await;
    assert_eq!(
        json!([
            done["created"],
            done["replaced"],
            done["skipped"],
            done["renamed"]
        ]),
        json!([1, 1, 0, 0])
    );
    assert_eq!(done["applied"], true);
    assert_eq!(listed(app).await[0]["text"], "show clock detail");
}

#[tokio::test]
async fn a_document_that_is_not_a_snippets_document_is_refused() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let entry = json!({"name": "Clock", "text": "show clock"});
    let padding = "x".repeat(4 * 1024 * 1024);
    let crowd: Vec<Value> = (0..2001)
        .map(|n| json!({"name": format!("s{n}"), "text": "x"}))
        .collect();
    for (document, says) in [
        (
            json!({"format": "nocturne.snippets/2", "snippets": [entry]}),
            "format",
        ),
        (
            json!({"format": "nocturne.evidence/1", "snippets": [entry]}),
            "format",
        ),
        (json!({"snippets": [entry]}), "format"),
        (json!({"format": "nocturne.snippets/1"}), "snippets"),
        (
            json!({"format": "nocturne.snippets/1", "snippets": {"0": entry}}),
            "snippets",
        ),
        (json!([entry]), "document"),
        (json!(42), "document"),
        (
            json!("{\"format\": \"nocturne.snippets/1\", \"snippets\": ["),
            "not JSON",
        ),
        (json!("show clock"), "not JSON"),
        // Too large, as the text of a file and as an object.
        (
            json!(format!(
                "{{\"format\":\"nocturne.snippets/1\",\"snippets\":[],\"x\":\"{padding}\"}}"
            )),
            "at most",
        ),
        (
            json!({"format": "nocturne.snippets/1", "snippets": [], "x": padding}),
            "at most",
        ),
        (
            json!({"format": "nocturne.snippets/1", "snippets": crowd}),
            "at most",
        ),
    ] {
        let dry = refused(
            app,
            "terminal.snippets.import",
            json!({"document": document, "dry_run": true}),
        )
        .await;
        assert_eq!(dry.code, "invalid");
        assert!(dry.message.contains(says), "expected `{says}`: {dry}");
        let err = app
            .rpc("terminal.snippets.import", json!({"document": document}))
            .await
            .unwrap_err();
        assert_eq!(err.code, "invalid");
        // A refusal never repeats the document.
        assert!(err.message.len() < 400, "{}", err.message.len());
    }
    let err = refused(
        app,
        "terminal.snippets.import",
        json!({"document": document(json!([entry])), "on_conflict": "overwrite"}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert_eq!(listed(app).await, json!([]));
}

#[tokio::test]
async fn an_import_with_one_entry_that_fails_takes_none_of_them_and_says_which() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    save(app, json!({"name": "Clock", "text": "show clock"})).await;
    let before = listed(app).await;
    let audit_before = audit_text(app);
    let good = json!({"name": "Good", "text": "show version"});

    for (bad, name, says) in [
        // The rules of the form.
        (
            json!({"name": "Braces", "text": "ping {{ host }}"}),
            Some("Braces"),
            "line 1, column 6",
        ),
        (
            json!({"name": "Escape", "text": "show clock\u{1b}[201~reload"}),
            Some("Escape"),
            "control character (U+001B)",
        ),
        (
            json!({"name": "Back\u{8}space", "text": "x"}),
            Some("Back\u{fffd}space"),
            "control character (U+0008)",
        ),
        (
            json!({"name": "Two\nlines", "text": "x"}),
            Some("Two\u{fffd}lines"),
            "line break",
        ),
        (json!({"name": "", "text": "x"}), Some(""), "needs a name"),
        (
            json!({"name": "Empty", "text": ""}),
            Some("Empty"),
            "no text",
        ),
        (
            json!({"name": "Tagged", "text": "x", "tags": ["a\u{202e}b"]}),
            Some("Tagged"),
            "text-direction mark",
        ),
        (
            json!({"name": "Elsewhere", "text": "x", "profile": "no_such_os"}),
            Some("Elsewhere"),
            "no_such_os",
        ),
        // The shape of an entry.
        (json!({"text": "x"}), None, "has no `name`"),
        (json!({"name": "No text"}), Some("No text"), "has no `text`"),
        (json!({"name": 7, "text": "x"}), None, "`name` is not text"),
        (
            json!({"name": "Numbers", "text": ["x"]}),
            Some("Numbers"),
            "`text` is not text",
        ),
        (
            json!({"name": "Tags", "text": "x", "tags": "a,b"}),
            Some("Tags"),
            "`tags`",
        ),
        (
            json!({"name": "Tags", "text": "x", "tags": ["a", 1]}),
            Some("Tags"),
            "`tags`",
        ),
        (
            json!({"name": "Type", "text": "x", "profile": 3}),
            Some("Type"),
            "`profile`",
        ),
        (json!("show clock"), None, "not an object"),
        (json!(null), None, "not an object"),
        // The same snippet twice in one document.
        (
            json!({"name": "GOOD", "text": "another"}),
            Some("GOOD"),
            "earlier entry",
        ),
    ] {
        let incoming = document(json!([good, bad, {"name": "Also good", "text": "show users"}]));
        for (mode, dry_run) in [("skip", false), ("replace", false), ("rename", true)] {
            let result = import(app, incoming.clone(), mode, dry_run).await;
            assert_eq!(
                json!([
                    result["created"],
                    result["replaced"],
                    result["skipped"],
                    result["renamed"]
                ]),
                json!([0, 0, 0, 0]),
                "{bad}"
            );
            assert_eq!(result["applied"], false, "{bad}");
            let problems = result["problems"].as_array().expect("problems");
            assert_eq!(problems.len(), 1, "{bad}: {result}");
            assert_eq!(problems[0]["index"], 1, "{bad}");
            assert_eq!(
                problems[0].get("name").and_then(Value::as_str),
                name,
                "{bad}"
            );
            assert!(
                problems[0]["problem"]
                    .as_str()
                    .expect("text")
                    .contains(says),
                "{bad}: {}",
                problems[0]["problem"]
            );
        }
    }
    // Every problem of a document is said, in the order of its entries.
    let result = import(
        app,
        document(json!([{"name": "A"}, good, {"name": "B", "text": "{{"}, 5])),
        "skip",
        false,
    )
    .await;
    let indexes: Vec<&Value> = result["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .map(|problem| &problem["index"])
        .collect();
    assert_eq!(indexes, [&json!(0), &json!(2), &json!(3)]);

    // The two entries that were fine each time are not here: none of an
    // import that fails is taken. Nothing was recorded either.
    assert_eq!(listed(app).await, before);
    assert_eq!(audit_text(app), audit_before);

    // A name too long to repeat is cut short in the answer.
    let result = import(
        app,
        document(json!([{"name": "n".repeat(5000), "text": "x"}])),
        "skip",
        true,
    )
    .await;
    assert_eq!(
        result["problems"][0]["name"]
            .as_str()
            .expect("a name")
            .chars()
            .count(),
        80
    );
}

#[tokio::test]
async fn replacing_is_refused_when_it_is_not_clear_which_snippet_is_meant() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // Two of one name, from before that was refused.
    for id in ["twin1", "twin2"] {
        app.store()
            .put(
                "terminal_snippets",
                id,
                &json!({"id": id, "name": "Twin", "text": id, "profile": null, "tags": [],
                        "created_at": 1, "updated_at": 1}),
            )
            .expect("an old snippet");
    }
    let incoming =
        document(json!([{"name": "Twin", "text": "new"}, {"name": "Fresh", "text": "x"}]));
    let result = import(app, incoming.clone(), "replace", false).await;
    assert_eq!(result["applied"], false);
    assert_eq!(result["problems"][0]["index"], 0);
    assert!(
        result["problems"][0]["problem"]
            .as_str()
            .expect("text")
            .contains("2 snippets"),
        "{result}"
    );
    assert_eq!(listed(app).await.as_array().expect("a list").len(), 2);
    // Keeping both, or keeping what is here, is clear enough.
    let kept = import(app, incoming.clone(), "skip", true).await;
    assert_eq!((&kept["created"], &kept["skipped"]), (&json!(1), &json!(1)));
    let both = import(app, incoming, "rename", false).await;
    assert_eq!((&both["created"], &both["renamed"]), (&json!(2), &json!(1)));
    assert_eq!(listed(app).await.as_array().expect("a list").len(), 4);
}

#[tokio::test]
async fn an_import_that_would_pass_the_most_snippets_kept_is_refused_whole() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // Written straight into the collection: two thousand saves would test
    // the patience of whoever runs this, not the limit.
    for n in 0..1999 {
        let id = format!("s{n:04}");
        app.store()
            .put(
                "terminal_snippets",
                &id,
                &json!({"id": id, "name": format!("Snippet {n:04}"), "text": "x", "profile": null,
                        "tags": [], "created_at": 1, "updated_at": 1}),
            )
            .expect("a snippet");
    }
    let two =
        document(json!([{"name": "One more", "text": "x"}, {"name": "Two more", "text": "x"}]));
    for dry_run in [true, false] {
        let err = refused(
            app,
            "terminal.snippets.import",
            json!({"document": two, "dry_run": dry_run}),
        )
        .await;
        assert_eq!(err.code, "conflict");
    }
    assert_eq!(listed(app).await.as_array().expect("a list").len(), 1999);
    // One still fits, and one that is already here does not count.
    let fits =
        document(json!([{"name": "One more", "text": "x"}, {"name": "Snippet 0007", "text": "x"}]));
    let result = import(app, fits, "skip", false).await;
    assert_eq!(
        (&result["created"], &result["skipped"]),
        (&json!(1), &json!(1))
    );
    assert_eq!(listed(app).await.as_array().expect("a list").len(), 2000);
}

// ------------------------------------------------------ unredacted reads

#[tokio::test]
async fn a_snapshot_with_its_secrets_showing_is_recorded_and_a_redacted_one_is_not() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    lab.fake("SW1").with(|device| {
        device
            .outputs
            .insert("show secret".into(), "enable secret Sup3rS3cret\n".into());
    });
    let mut events = Events::new(app);
    let session = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));
    events.wait_for(&session, "SW1>").await;
    app.rpc(
        "terminal.input",
        json!({"session_id": session, "data": "show secret\r"}),
    )
    .await
    .expect("input");
    events.wait_for(&session, "Sup3rS3cret\r\nSW1>").await;

    // Redacted, by default and when asked for: nothing to record.
    for params in [
        json!({"session_id": session}),
        json!({"session_id": session, "redact": true}),
    ] {
        let snapshot = call(app, "terminal.snapshot", params)
            .await
            .expect("snapshot");
        assert!(
            !snapshot["text"]
                .as_str()
                .expect("text")
                .contains("Sup3rS3cret")
        );
    }
    assert!(audit_entries(app, "terminal.view_unredacted").is_empty());

    let verbatim = call(
        app,
        "terminal.snapshot",
        json!({"session_id": session, "lines": 3, "redact": false}),
    )
    .await
    .expect("snapshot");
    assert_eq!(
        verbatim,
        json!({
            "text": "SW1>show secret\nenable secret Sup3rS3cret\nSW1>",
            "lines": 3,
            "redacted": false,
        })
    );
    let entries = audit_entries(app, "terminal.view_unredacted");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].device_id, Some(lab.id("SW1")));
    assert_eq!(
        entries[0].message,
        "Viewed unredacted terminal output of `SW1`"
    );
    assert_eq!(
        entries[0].body,
        Some(json!({"source": "snapshot", "session_id": session, "title": "SW1", "lines": 3}))
    );
    // Every such view is one entry, and none of them says what was seen.
    call(
        app,
        "terminal.snapshot",
        json!({"session_id": session, "redact": false}),
    )
    .await
    .expect("snapshot");
    assert_eq!(audit_entries(app, "terminal.view_unredacted").len(), 2);
    assert!(!audit_text(app).contains("Sup3rS3cret"));
}

#[tokio::test]
async fn a_log_read_with_its_secrets_showing_is_recorded_page_by_page() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = testing::lab(common::builder_at(dir.path()), &["SW1"]).await;
    let app = &lab.app;
    lab.fake("SW1").with(|device| {
        device
            .outputs
            .insert("show secret".into(), "enable secret Sup3rS3cret\n".into());
    });
    let mut events = Events::new(app);
    let info = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("SW1"), "cols": 100, "rows": 30, "log": true}),
        )
        .await
        .expect("open");
    let session = session_id(&info);
    let name = info["logging"].as_str().expect("a log name").to_string();
    events.wait_for(&session, "SW1>").await;
    app.rpc(
        "terminal.input",
        json!({"session_id": session, "data": "show secret\r"}),
    )
    .await
    .expect("input");
    events.wait_for(&session, "Sup3rS3cret\r\nSW1>").await;
    app.rpc("terminal.close", json!({"session_id": session}))
        .await
        .expect("close");

    let redacted = call(app, "terminal.logs.read", json!({"name": name}))
        .await
        .expect("read");
    assert_eq!(redacted["redacted"], true);
    assert_eq!(redacted["offset"], 0);
    assert!(
        !redacted["text"]
            .as_str()
            .expect("text")
            .contains("Sup3rS3cret")
    );
    assert!(audit_entries(app, "terminal.view_unredacted").is_empty());

    let verbatim = call(
        app,
        "terminal.logs.read",
        json!({"name": name, "redact": false}),
    )
    .await
    .expect("read");
    assert!(
        verbatim["text"]
            .as_str()
            .expect("text")
            .contains("enable secret Sup3rS3cret")
    );
    assert_eq!(verbatim["redacted"], false);
    let entries = audit_entries(app, "terminal.view_unredacted");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].device_id, Some(lab.id("SW1")));
    assert_eq!(entries[0].message, "Viewed unredacted session log of `SW1`");
    assert_eq!(
        entries[0].body,
        Some(json!({"source": "log", "name": name, "offset": 0, "bytes": verbatim["size"]}))
    );

    // A page that starts in the middle of a line is recorded as the page it
    // was: from the start of that line.
    let size = verbatim["size"].as_u64().expect("size");
    let second_line = verbatim["text"]
        .as_str()
        .expect("text")
        .find('\n')
        .expect("more than one line") as u64
        + 1;
    let page = call(
        app,
        "terminal.logs.read",
        json!({"name": name, "offset": second_line + 12, "limit": 10, "redact": false}),
    )
    .await
    .expect("read");
    let start = page["offset"].as_u64().expect("offset");
    assert_eq!(start, second_line, "{page}");
    let entries = audit_entries(app, "terminal.view_unredacted");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].body.as_ref().expect("a body")["offset"], start);
    assert_eq!(
        entries[0].body.as_ref().expect("a body")["bytes"],
        page["next_offset"].as_u64().expect("next") - start
    );

    // Reading past the end shows nothing, so it records nothing.
    let nothing = call(
        app,
        "terminal.logs.read",
        json!({"name": name, "offset": size, "redact": false}),
    )
    .await
    .expect("read");
    assert_eq!(nothing["text"], "");
    assert_eq!(audit_entries(app, "terminal.view_unredacted").len(), 2);
    // A log that is not there is not a view of anything.
    let err = refused(
        app,
        "terminal.logs.read",
        json!({"name": "2026-01/ghost-20260101-000000.log", "redact": false}),
    )
    .await;
    assert_eq!(err.code, "not_found");
    assert_eq!(audit_entries(app, "terminal.view_unredacted").len(), 2);
    assert!(!audit_text(app).contains("Sup3rS3cret"));
}

// ------------------------------------------- what a second reader looked for

/// Everything a value could carry that is not a character on its line, and
/// what the refusal calls it. Each is put between two words, so that an
/// accepted one would show as a second command.
const SMUGGLED: [(&str, &str); 17] = [
    ("\n", "line_break"),
    ("\r", "line_break"),
    ("\r\n", "line_break"),
    // NEL, the line break of the C1 controls.
    ("\u{85}", "control_character"),
    ("\u{2028}", "line_break"),
    ("\u{2029}", "line_break"),
    // An escape sequence: cursor up, and the end of a bracketed paste.
    ("\u{1b}[A", "control_character"),
    ("\u{1b}[201~", "control_character"),
    // The same two written with the one-character CSI.
    ("\u{9b}A", "control_character"),
    ("\u{9b}201~", "control_character"),
    ("\u{0}", "control_character"),
    ("\t", "control_character"),
    ("\u{b}", "control_character"),
    ("\u{c}", "control_character"),
    // Ctrl+C, a backspace, a delete.
    ("\u{3}", "control_character"),
    ("\u{8}", "control_character"),
    ("\u{7f}", "control_character"),
];

/// Every table of the store a snippet method could write to, row by row.
fn store_dump(app: &App) -> Vec<String> {
    app.store()
        .with(|conn| {
            let mut rows = Vec::new();
            for query in [
                "SELECT 'docs', collection || '/' || id, body, created_at, updated_at FROM docs ORDER BY collection, id",
                "SELECT 'kv', key, value, 0, 0 FROM kv ORDER BY key",
                "SELECT 'logs', stream || '/' || source, message || ' ' || coalesce(body, ''), id, ts FROM logs ORDER BY id",
            ] {
                let mut statement = conn.prepare(query)?;
                let found = statement.query_map([], |row| {
                    Ok(format!(
                        "{} {} {} {} {}",
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })?;
                for row in found {
                    rows.push(row?);
                }
            }
            Ok(rows)
        })
        .expect("the store can be read")
}

#[tokio::test]
async fn nothing_a_value_or_a_default_holds_becomes_a_second_command() {
    let lab = scripted(&[("CON1", listening("SW1>"))], false).await;
    let app = &lab.app;
    let session = open_console(&lab, "CON1", "SW1>").await;
    let id = id_of(
        &save(
            app,
            json!({"name": "Port", "text": "show interface {{port}}\n"}),
        )
        .await,
    );

    for (smuggled, problem) in SMUGGLED {
        let value = format!("Gi0/1{smuggled}reload");
        let problems = json!({"variables": [{"name": "port", "problem": problem}]});
        let err = refused(
            app,
            "terminal.snippets.render",
            json!({"id": id, "values": {"port": value}}),
        )
        .await;
        assert_eq!(err.code, "invalid", "{smuggled:?}");
        assert_eq!(err.details, Some(problems.clone()), "{smuggled:?}");
        assert!(!err.message.contains("reload"), "{err}");
        // Sent with the hash of the text a careless substitution would have
        // made: knowing what to expect does not make it acceptable.
        let err = refused(
            app,
            "terminal.snippets.send",
            json!({
                "id": id, "values": {"port": value}, "session_ids": [session],
                "expect_sha256": sha256(&format!("show interface {value}")),
            }),
        )
        .await;
        assert_eq!(err.code, "invalid", "{smuggled:?}");
        assert_eq!(err.details, Some(problems), "{smuggled:?}");

        // The same in a default is not a snippet: not by the form, and not
        // by a document either.
        let text = format!("show interface {{{{port=Gi0/1{smuggled}reload}}}}");
        let err = refused(
            app,
            "terminal.snippets.save",
            json!({"name": "Default", "text": text}),
        )
        .await;
        assert_eq!(err.code, "invalid", "{smuggled:?}");
        let brought = import(
            app,
            document(json!([{"name": "Default", "text": text}])),
            "skip",
            false,
        )
        .await;
        assert_eq!(brought["applied"], false, "{smuggled:?}");
        assert_eq!(brought["problems"][0]["index"], 0, "{smuggled:?}");
    }

    // One that was stored before defaults were checked is not typed either.
    app.store()
        .put(
            "terminal_snippets",
            "old",
            &json!({
                "id": "old", "name": "Old", "profile": null, "tags": [],
                "text": "show interface {{port=Gi0/1\u{1b}[201~}}",
                "created_at": 1, "updated_at": 1,
            }),
        )
        .expect("an old snippet");
    for method in ["terminal.snippets.render", "terminal.snippets.send"] {
        let err = refused(
            app,
            method,
            json!({"id": "old", "session_ids": [session], "expect_sha256": "0"}),
        )
        .await;
        assert_eq!(err.code, "invalid", "{method}");
    }

    // Through all of it the console was sent nothing, only `Port` exists and
    // nothing was recorded as sent.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(lab.heard("CON1").raw(), "");
    assert!(audit_entries(app, "terminal.snippet.send").is_empty());
    assert_eq!(listed(app).await.as_array().expect("a list").len(), 2);

    // A value that is written like a placeholder — of this snippet, of
    // another name, with a default, escaped — is a value: it goes in once
    // and is typed as it stands.
    let id = id_of(
        &save(
            app,
            json!({"name": "Ping", "text": "ping {{host}} source {{src=lo0}}\n"}),
        )
        .await,
    );
    let mut typed = String::new();
    for value in ["{{src}}", "{{host}}", "{{src=lo9}}", "\\{{src}}", "{{"] {
        confirm_and_send(app, &id, json!({"host": value}), &[&session])
            .await
            .expect("send");
        typed.push_str(&format!("ping {value} source lo0"));
        assert_eq!(heard_bytes(&lab.heard("CON1"), typed.len()).await, typed);
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(lab.heard("CON1").raw(), typed);
}

#[tokio::test]
async fn a_snippet_changed_after_it_was_confirmed_reaches_none_of_the_sessions_named() {
    let lab = scripted(
        &[
            ("CON1", listening("SW1>")),
            ("CON2", listening("SW2>")),
            ("CON3", listening("SW3>")),
        ],
        false,
    )
    .await;
    let app = &lab.app;
    let one = open_console(&lab, "CON1", "SW1>").await;
    let two = open_console(&lab, "CON2", "SW2>").await;
    let three = open_console(&lab, "CON3", "SW3>").await;
    let id = id_of(
        &save(
            app,
            json!({"name": "Vlan", "text": "vlan {{id}}\n name users\n"}),
        )
        .await,
    );
    let confirmed = call(
        app,
        "terminal.snippets.render",
        json!({"id": id, "values": {"id": "10"}}),
    )
    .await
    .expect("render");
    let send = || {
        call(
            app,
            "terminal.snippets.send",
            json!({
                "id": id, "values": {"id": "10"}, "session_ids": [one, two, three],
                "expect_sha256": confirmed["sha256"],
            }),
        )
    };

    // Edited: one character, the name left alone.
    save(
        app,
        json!({"id": id, "name": "Vlan", "text": "vlan {{id}}\n name users \n"}),
    )
    .await;
    assert_eq!(send().await.unwrap_err().code, "conflict");
    // Deleted altogether.
    call(app, "terminal.snippets.delete", json!({"id": id}))
        .await
        .expect("delete");
    assert_eq!(send().await.unwrap_err().code, "not_found");

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    for node in ["CON1", "CON2", "CON3"] {
        assert_eq!(lab.heard(node).raw(), "", "{node}");
    }
    assert!(audit_entries(app, "terminal.snippet.send").is_empty());
}

#[tokio::test]
async fn values_that_are_not_an_object_are_refused_without_being_repeated() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = id_of(
        &save(
            app,
            json!({"name": "User", "text": "username ops secret {{password}}"}),
        )
        .await,
    );
    let secret = "Zq7-hunter2-Xk";
    for values in [json!(secret), json!([secret]), json!(7), json!(true)] {
        for method in ["terminal.snippets.render", "terminal.snippets.send"] {
            let err = refused(
                app,
                method,
                json!({"id": id, "values": values, "session_ids": ["any"], "expect_sha256": "0"}),
            )
            .await;
            assert_eq!(err.code, "invalid", "{values}");
            assert!(err.message.contains("`values`"), "{err}");
            // A refusal is logged, and what was sent as a value may be one.
            assert!(!err.message.contains(secret), "{err}");
        }
    }
    // A flood of names is refused as one thing, not answered name by name.
    let flood: serde_json::Map<String, Value> = (0..5000)
        .map(|n| (format!("{secret}{n}"), json!(secret)))
        .collect();
    let err = refused(
        app,
        "terminal.snippets.render",
        json!({"id": id, "values": flood}),
    )
    .await;
    assert_eq!(err.code, "invalid");
    assert!(err.message.len() < 200, "{}", err.message.len());
    assert!(!err.message.contains(secret), "{err}");

    // Leaving the values out, or saying `null`, is giving none.
    let plain = id_of(&save(app, json!({"name": "Clock", "text": "show clock"})).await);
    for params in [json!({"id": plain}), json!({"id": plain, "values": null})] {
        let rendered = call(app, "terminal.snippets.render", params)
            .await
            .expect("render");
        assert_eq!(rendered["text"], "show clock");
    }
}

#[tokio::test]
async fn a_dry_run_leaves_every_row_of_the_store_as_it_was() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let clock = save(app, json!({"name": "Clock", "text": "show clock"})).await;
    save(
        app,
        json!({"name": "Port bounce", "text": "interface {{port}}\n shutdown\n", "profile": "test_ios"}),
    )
    .await;
    let before = store_dump(app);
    assert!(before.iter().any(|row| row.contains("show clock")));
    let mut events = Events::new(app);

    let fine = document(json!([
        {"name": "Clock", "text": "show clock detail", "tags": ["time"]},
        {"name": "Port bounce", "text": "interface {{port}}\n shutdown\n", "profile": "test_ios"},
        {"name": "New one", "text": "show version"},
        // An id of a snippet that is here is not a way to reach it.
        {"id": clock["id"], "name": "Borrowed id", "text": "write erase", "created_at": 1},
    ]));
    let broken = document(json!([
        {"name": "Good", "text": "show users"},
        {"name": "Bad", "text": "show {{"},
    ]));
    for mode in ["skip", "replace", "rename"] {
        for incoming in [&fine, &broken] {
            for as_text in [false, true] {
                let given = if as_text {
                    json!(incoming.to_string())
                } else {
                    incoming.clone()
                };
                let would = import(app, given, mode, true).await;
                assert_eq!(would["applied"], false, "{mode}");
                assert_eq!(would["dry_run"], true, "{mode}");
                assert_eq!(store_dump(app), before, "{mode} wrote something");
            }
        }
    }
    events.drain();
    assert!(events.of("terminal.snippets.changed").is_empty());
    assert!(events.of("audit.appended").is_empty());

    // And when it is done for real, the entry that named the id of a
    // snippet here came in as one of its own; `Clock` is as it was.
    let done = import(app, fine, "skip", false).await;
    assert_eq!((&done["created"], &done["skipped"]), (&json!(2), &json!(2)));
    let after = listed(app).await;
    let row = |name: &str| {
        after
            .as_array()
            .expect("a list")
            .iter()
            .find(|row| row["name"] == name)
            .cloned()
            .expect("the snippet is listed")
    };
    assert_eq!(row("Clock"), clock);
    assert_ne!(row("Borrowed id")["id"], clock["id"]);
    assert_ne!(row("Borrowed id")["created_at"], 1);
}

#[tokio::test]
async fn a_document_brought_twice_is_answered_the_same_and_adds_nothing_the_second_time() {
    let big = "x".repeat(256 * 1024 + 1);
    let many_tags: Vec<String> = (0..5000).map(|n| format!("t{n}")).collect();
    let nested = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
    let entry = json!({"name": "Clock", "text": "show clock detail"});

    // What is refused outright, and what is answered with its problems.
    let hostile: Vec<Value> = vec![
        json!(nested),
        json!(format!(
            "{{\"format\":\"nocturne.snippets/1\",\"snippets\":{nested}}}"
        )),
        json!("{\"format\":\"nocturne.snippets/1\",\"snippets\":[]} and then some"),
        json!("null"),
        json!("[]"),
        json!("\"nocturne.snippets/1\""),
        json!({"format": ["nocturne.snippets/1"], "snippets": [entry]}),
        json!({"format": "nocturne.snippets/1 ", "snippets": [entry]}),
        json!({"format": "NOCTURNE.SNIPPETS/1", "snippets": [entry]}),
        json!({"format": "nocturne.snippets/1", "snippets": "show clock"}),
        json!({"format": "nocturne.snippets/1", "snippets": null}),
        document(json!([{"name": "Big", "text": big}])),
        document(json!([{"name": "Tagged", "text": "x", "tags": many_tags}])),
        document(json!([{"name": "Nul", "text": "show\u{0}clock"}])),
        document(json!([{"name": "Separator", "text": "show clock\u{2028}reload"}])),
        document(json!([{"name": "Default", "text": "ping {{host=10.0.0.1\treload}}"}])),
        document(json!([{"name": "Twice", "text": "{{a=1}} {{a=2}}"}])),
        document(json!([{"name": "x".repeat(201), "text": "x"}])),
        document(json!([entry, {"name": " clock ", "text": "again"}])),
        document(json!([entry, [entry], {"name": {"$ne": null}, "text": "x"}, 1e308, true])),
        document(json!([{"name": "<img src=x onerror=alert(1)>", "text": "{{"}])),
    ];

    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    save(app, json!({"name": "Clock", "text": "show clock"})).await;
    let before = store_dump(app);
    for document in &hostile {
        let mut answers = Vec::new();
        for _ in 0..2 {
            let answer = call(
                app,
                "terminal.snippets.import",
                json!({"document": document, "on_conflict": "replace"}),
            )
            .await;
            answers.push(match answer {
                Ok(result) => {
                    assert_eq!(result["applied"], false, "{result}");
                    assert_ne!(result["problems"], json!([]), "{result}");
                    for count in ["created", "replaced", "skipped", "renamed"] {
                        assert_eq!(result[count], 0, "{result}");
                    }
                    result.to_string()
                }
                Err(err) => {
                    assert_eq!(err.code, "invalid", "{err}");
                    // Never the document back, in whole or in part.
                    assert!(err.message.len() < 400, "{}", err.message.len());
                    err.message
                }
            });
        }
        assert_eq!(answers[0], answers[1]);
        assert_eq!(
            store_dump(app),
            before,
            "a refused document wrote something"
        );
    }
    // Markup in a name comes back as the text it is: nothing here renders it,
    // and nothing in the answer is more than that text.
    let result = import(app, hostile[hostile.len() - 1].clone(), "skip", true).await;
    assert_eq!(
        result["problems"][0]["name"],
        "<img src=x onerror=alert(1)>"
    );

    // A document that is fine, brought twice in each mode: the second time
    // every entry is already here, and not a row of the store changes.
    let fine = document(json!([
        {"name": "clock", "text": "show clock detail", "tags": ["time"]},
        {"name": "Port bounce", "text": "interface {{port}}\n shutdown", "profile": "test_ios"},
        {"name": "__proto__", "text": "show version"},
    ]));
    for (mode, first) in [
        ("skip", json!([2, 0, 1, 0])),
        ("replace", json!([2, 1, 0, 0])),
        ("rename", json!([3, 0, 0, 1])),
    ] {
        let lab = common::lab(&["SW1"]).await;
        let app = &lab.app;
        save(app, json!({"name": "Clock", "text": "show clock"})).await;
        // With the byte-order mark an editor on Windows puts in front.
        let as_file = json!(format!("\u{feff}{fine}"));
        let counts = |result: &Value| {
            json!([
                result["created"],
                result["replaced"],
                result["skipped"],
                result["renamed"]
            ])
        };
        let result = import(app, as_file.clone(), mode, false).await;
        assert_eq!(counts(&result), first, "{mode}");
        assert_eq!(result["applied"], true, "{mode}");
        let after = store_dump(app);

        for again in [as_file, fine.clone()] {
            let would = import(app, again.clone(), mode, true).await;
            let result = import(app, again, mode, false).await;
            assert_eq!(counts(&result), counts(&would), "{mode}");
            assert_eq!(
                (&result["created"], &result["replaced"]),
                (&json!(0), &json!(0)),
                "{mode}: {result}"
            );
            assert_eq!(result["skipped"], 3, "{mode}: {result}");
            assert_eq!(store_dump(app), after, "{mode}: the second import wrote");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn saves_and_imports_that_race_for_one_name_leave_one_snippet_of_it() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    // A library of some size, written straight into the collection. It is
    // what makes the race one that is lost when nothing guards it: a save
    // checks its name against every snippet between reading them and
    // writing its own, and the longer that takes the more of the other
    // saves have read the library by then, before the name was in it.
    const ALREADY: usize = 800;
    for n in 0..ALREADY {
        let id = format!("s{n:04}");
        app.store()
            .put(
                "terminal_snippets",
                &id,
                &json!({"id": id, "name": format!("Snippet {n:04}"), "text": "show clock",
                        "profile": null, "tags": [], "created_at": 1, "updated_at": 1}),
            )
            .expect("a snippet");
    }
    const ROUNDS: usize = 6;
    for round in 0..ROUNDS {
        let name = format!("Raced {round}");
        let gate = Arc::new(tokio::sync::Barrier::new(16));
        let mut racers = Vec::new();
        for racer in 0..16 {
            let (app, gate, name) = (app.clone(), gate.clone(), name.clone());
            racers.push(tokio::spawn(async move {
                gate.wait().await;
                let text = format!("show clock {racer}");
                // Half of them through the form, half with a document.
                if racer % 2 == 0 {
                    app.rpc(
                        "terminal.snippets.save",
                        json!({"name": name, "text": text}),
                    )
                    .await
                    .map(|_| 1)
                } else {
                    app.rpc(
                        "terminal.snippets.import",
                        json!({"document": document(json!([{"name": name, "text": text}]))}),
                    )
                    .await
                    .map(|result| {
                        assert_eq!(result["applied"], true, "{result}");
                        result["created"].as_u64().expect("a count")
                    })
                }
            }));
        }
        let mut made = 0;
        for racer in racers {
            match racer.await.expect("the call ran") {
                Ok(created) => made += created,
                Err(err) => assert_eq!(err.code, "conflict", "{err}"),
            }
        }
        assert_eq!(made, 1, "{name}");
    }
    assert_eq!(
        listed(app).await.as_array().expect("a list").len(),
        ALREADY + ROUNDS
    );
}

#[tokio::test]
async fn a_value_that_was_typed_is_kept_nowhere_once_it_has_been_sent() {
    let lab = scripted(&[("CON1", listening("SW1>"))], false).await;
    let app = &lab.app;
    let session = open_console(&lab, "CON1", "SW1>").await;
    let id = id_of(
        &save(
            app,
            json!({"name": "Local user", "text": "username {{user}} secret {{password}}\n"}),
        )
        .await,
    );
    let secret = "Zq7-hunter2-Xk";
    let values = json!({"user": "ops-Qx9", "password": secret});
    confirm_and_send(app, &id, values.clone(), &[&session])
        .await
        .expect("send");
    let typed = format!("username ops-Qx9 secret {secret}");
    assert_eq!(heard_bytes(&lab.heard("CON1"), typed.len()).await, typed);
    // Refusals on the way are not a place for it either: the wrong hash, a
    // session that is not there.
    for (sessions, expect) in [(json!([session]), "0"), (json!(["nope"]), "0")] {
        refused(
            app,
            "terminal.snippets.send",
            json!({"id": id, "values": values, "session_ids": sessions, "expect_sha256": expect}),
        )
        .await;
    }

    // Every document, every setting and every line of every log stream —
    // the audit trail among them — and neither value is in any of it.
    let kept = store_dump(app);
    assert!(
        kept.iter()
            .any(|row| row.contains("audit/terminal.snippet.send")),
        "{kept:#?}"
    );
    for row in &kept {
        assert!(!row.contains(secret), "the password was kept: {row}");
        assert!(!row.contains("ops-Qx9"), "a value was kept: {row}");
    }
}
