//! Session logs: what is written, where, what the file is called, and how
//! they are listed, read and deleted.

mod common;

use common::{Events, SECRET, Script, builder_at, open_device, scripted_on, session_id};
use serde_json::{Value, json};

/// `YYYY-MM/<name>-<YYYYmmdd-HHMMSS>.log`, and nothing that could climb out
/// of the sessions directory or name a Windows device.
fn well_formed(name: &str, stem: &str) -> bool {
    let Some((month, file)) = name.split_once('/') else {
        return false;
    };
    month.len() == 7
        && month.as_bytes()[4] == b'-'
        && file.starts_with(&format!("{stem}-"))
        && file.ends_with(".log")
        && file.len() == stem.len() + 1 + 15 + 4
        && !name.contains("..")
        && !name.contains('\\')
}

async fn logged_app(dir: &std::path::Path) -> common::Scripted {
    scripted_on(
        builder_at(dir),
        &[(
            "CON1",
            Script {
                banner: "\r\n".into(),
                ..Script::default()
            },
        )],
        true,
    )
    .await
}

#[tokio::test]
async fn a_session_is_written_to_a_file_as_plain_timestamped_text() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = logged_app(dir.path()).await;
    let app = &lab.app;
    let mut events = Events::new(app);

    let info = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("CON1"), "cols": 100, "rows": 30, "log": true}),
        )
        .await
        .expect("open");
    let id = session_id(&info);
    let name = info["logging"].as_str().expect("a log name").to_string();
    assert!(well_formed(&name, "CON1"), "{name}");
    assert_eq!(info["log_error"], json!(null));

    events.wait_for(&id, "SW1>").await;
    // Something with escape sequences and a redraw in it.
    app.rpc(
        "terminal.input",
        json!({"session_id": id, "data": "\u{1b}[1mshow clock\u{1b}[0m\r"}),
    )
    .await
    .expect("input");
    events.wait_for(&id, "show clock").await;
    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");

    let read = app
        .rpc("terminal.logs.read", json!({"name": name}))
        .await
        .expect("read");
    let text = read["text"].as_str().expect("text");
    assert!(read["eof"].as_bool().expect("eof"));
    assert!(
        !text.contains('\u{1b}'),
        "escape sequences were logged: {text:?}"
    );
    assert!(
        text.contains("--- log started: CON1 (sim, lab:CON1) ---"),
        "{text}"
    );
    assert!(text.contains("--- log ended: closed by user ---"), "{text}");
    assert!(text.contains("show clock"), "{text}");
    // Every line starts with an ISO timestamp.
    for line in text.lines() {
        assert!(
            line.len() > 24 && line.as_bytes()[4] == b'-' && line.as_bytes()[10] == b'T',
            "not timestamped: {line:?}"
        );
    }
    // The password auto-login typed is not echoed by the device, so it is
    // not in the log either — and nothing logs input separately.
    assert!(
        !text.contains(SECRET),
        "the password reached the log:\n{text}"
    );

    // And the file really is where the name says.
    let path = dir.path().join("sessions").join(&name);
    assert!(path.is_file(), "{}", path.display());
}

#[tokio::test]
async fn logs_are_listed_read_in_pieces_and_deleted() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = logged_app(dir.path()).await;
    let app = &lab.app;
    let mut events = Events::new(app);

    let info = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("CON1"), "cols": 100, "rows": 30, "log": true}),
        )
        .await
        .expect("open");
    let id = session_id(&info);
    let name = info["logging"].as_str().expect("a log name").to_string();
    events.wait_for(&id, "SW1>").await;
    for index in 0..40 {
        app.rpc(
            "terminal.input",
            json!({"session_id": id, "data": format!("line {index:03}\r")}),
        )
        .await
        .expect("input");
    }
    events.wait_for(&id, "line 039").await;

    // While it is open the log is listed as active and cannot be deleted.
    let listed = app
        .rpc("terminal.logs.list", json!({}))
        .await
        .expect("list");
    assert_eq!(listed.as_array().expect("a list").len(), 1);
    assert_eq!(listed[0]["name"], name.as_str());
    assert_eq!(listed[0]["device_id"], lab.id("CON1"));
    assert_eq!(listed[0]["title"], "CON1");
    assert_eq!(listed[0]["kind"], "sim");
    assert_eq!(listed[0]["active"], true);
    assert_eq!(listed[0]["ended_at"], json!(null));
    assert!(listed[0]["size"].as_u64().expect("size") > 0);

    let refused = app
        .rpc("terminal.logs.delete", json!({"names": [name.clone()]}))
        .await
        .expect("delete");
    assert_eq!(refused["deleted"], 0);
    assert_eq!(refused["failed"][0]["code"], "conflict");

    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");

    // Read it in small pieces; each one ends on a line boundary.
    let mut offset = 0u64;
    let mut whole = String::new();
    loop {
        let page = app
            .rpc(
                "terminal.logs.read",
                json!({"name": name, "offset": offset, "limit": 120}),
            )
            .await
            .expect("read");
        let text = page["text"].as_str().expect("text");
        whole.push_str(text);
        let next = page["next_offset"].as_u64().expect("next_offset");
        assert!(next > offset || page["eof"].as_bool().expect("eof"));
        offset = next;
        if page["eof"].as_bool().expect("eof") {
            break;
        }
    }
    assert!(
        whole.contains("line 000") && whole.contains("line 039"),
        "{whole}"
    );

    let after = app
        .rpc("terminal.logs.list", json!({}))
        .await
        .expect("list");
    assert_eq!(after[0]["active"], false);
    assert!(after[0]["ended_at"].as_i64().is_some());

    // Filters.
    assert_eq!(
        app.rpc("terminal.logs.list", json!({"query": "CON1"}))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .len(),
        1
    );
    assert!(
        app.rpc("terminal.logs.list", json!({"query": "no such thing"}))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .is_empty()
    );
    assert!(
        app.rpc("terminal.logs.list", json!({"device_id": "ghost"}))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .is_empty()
    );

    let deleted = app
        .rpc("terminal.logs.delete", json!({"names": [name.clone()]}))
        .await
        .expect("delete");
    assert_eq!(deleted["deleted"], 1);
    assert!(
        app.rpc("terminal.logs.list", json!({}))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .is_empty()
    );
    assert!(!dir.path().join("sessions").join(&name).exists());
    assert_eq!(
        app.rpc("terminal.logs.read", json!({"name": name}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn a_log_name_is_built_from_the_title_and_can_name_nothing_else() {
    let dir = tempfile::tempdir().expect("a directory");
    let app = builder_at(dir.path()).build();

    for (title, stem) in [
        ("../../etc/passwd", "etc_passwd"),
        ("COM3", "COM3"),
        ("admin@10.0.0.1", "admin@10.0.0.1"),
        ("a\\b:c*d?e\"f<g>h|i", "a_b_c_d_e_f_g_h_i"),
        ("...", "session"),
        ("   ", "Local_shell"),
    ] {
        let info = app
            .rpc(
                "terminal.open",
                json!({
                    "adhoc": {
                        "transport": "local",
                        "local": {"program": "sh", "args": ["-c", "echo hi; sleep 0.2"]},
                        "title": title,
                    },
                    "log": true,
                }),
            )
            .await
            .expect("open");
        let name = info["logging"].as_str().expect("a log name").to_string();
        assert!(well_formed(&name, stem), "{title:?} became {name}");
        // The file is inside the sessions directory and nowhere else.
        let path = dir.path().join("sessions").join(&name);
        assert!(path.is_file(), "{}", path.display());
        assert!(
            path.canonicalize()
                .expect("a real path")
                .starts_with(dir.path().canonicalize().expect("a real path")),
            "{}",
            path.display()
        );
        app.rpc("terminal.close", json!({"session_id": session_id(&info)}))
            .await
            .expect("close");
    }

    // And a name made up by the caller cannot leave it either.
    for bad in [
        "../secrets.log",
        "2026-11/../../etc/passwd.log",
        "2026-11/sub/x.log",
        "/etc/passwd",
        "x.log",
    ] {
        let refused = app
            .rpc("terminal.logs.read", json!({"name": bad}))
            .await
            .unwrap_err();
        assert_eq!(refused.code, "invalid", "{bad}");
    }
}

#[tokio::test]
async fn logging_follows_the_setting_and_can_be_turned_on_and_off() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = logged_app(dir.path()).await;
    let app = &lab.app;

    // Off by default.
    let info = open_device(app, &lab.id("CON1")).await.expect("open");
    assert_eq!(info["logging"], json!(null));
    let id = session_id(&info);

    let mut events = Events::new(app);
    let started = app
        .rpc(
            "terminal.logging",
            json!({"session_id": id, "enabled": true}),
        )
        .await
        .expect("logging");
    let name = started["logging"].as_str().expect("a log name").to_string();
    let changed = events.wait_topic("terminal.changed").await;
    assert_eq!(changed.payload["logging"], name.as_str());

    let stopped = app
        .rpc(
            "terminal.logging",
            json!({"session_id": id, "enabled": false}),
        )
        .await
        .expect("logging");
    assert_eq!(stopped["logging"], json!(null));
    // The file stays, completed.
    let listed = app
        .rpc("terminal.logs.list", json!({}))
        .await
        .expect("list");
    assert_eq!(listed[0]["name"], name.as_str());
    assert_eq!(listed[0]["active"], false);
    let text = app
        .rpc("terminal.logs.read", json!({"name": name}))
        .await
        .expect("read")["text"]
        .as_str()
        .expect("text")
        .to_string();
    assert!(
        text.contains("--- log ended: logging turned off ---"),
        "{text}"
    );

    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");

    // With the setting on, every session is logged without being asked.
    app.rpc(
        "settings.set",
        json!({"section": "terminal", "value": {"log_sessions": true}}),
    )
    .await
    .expect("settings");
    let info = open_device(app, &lab.id("CON1")).await.expect("open");
    assert!(info["logging"].as_str().is_some());
    app.rpc("terminal.close", json!({"session_id": session_id(&info)}))
        .await
        .expect("close");
}

#[tokio::test]
async fn an_installation_without_files_keeps_no_logs_and_says_so() {
    let lab = common::scripted(&[("CON1", Script::default())], true).await;
    let app = &lab.app;
    let info = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("CON1"), "log": true}),
        )
        .await
        .expect("open");
    // The session opens anyway: getting onto the device comes first.
    assert_eq!(info["state"], "open");
    assert_eq!(info["logging"], json!(null));
    assert!(
        info["log_error"]
            .as_str()
            .expect("a reason")
            .contains("no files"),
        "{}",
        info["log_error"]
    );
    assert_eq!(
        app.rpc("terminal.logs.list", json!({}))
            .await
            .expect("list"),
        Value::Array(Vec::new())
    );
    let turning_on = app
        .rpc(
            "terminal.logging",
            json!({"session_id": session_id(&info), "enabled": true}),
        )
        .await
        .unwrap_err();
    assert_eq!(turning_on.code, "unavailable");
}

#[tokio::test]
async fn a_device_secret_in_a_log_is_hidden_unless_it_is_asked_for() {
    let dir = tempfile::tempdir().expect("a directory");
    let lab = scripted_on(
        builder_at(dir.path()),
        &[(
            "CON1",
            Script {
                banner: "\r\nSW1#show run\r\nenable secret Sup3rS3cret\r\nSW1#".into(),
                listen_only: true,
                ..Script::default()
            },
        )],
        false,
    )
    .await;
    let app = &lab.app;
    let mut events = Events::new(app);
    let info = app
        .rpc(
            "terminal.open",
            json!({"device_id": lab.id("CON1"), "log": true}),
        )
        .await
        .expect("open");
    let id = session_id(&info);
    let name = info["logging"].as_str().expect("a log name").to_string();
    events.wait_for(&id, "Sup3rS3cret").await;
    app.rpc("terminal.close", json!({"session_id": id}))
        .await
        .expect("close");

    let hidden = app
        .rpc("terminal.logs.read", json!({"name": name}))
        .await
        .expect("read");
    assert_eq!(hidden["redacted"], true);
    assert!(
        !hidden["text"]
            .as_str()
            .expect("text")
            .contains("Sup3rS3cret")
    );

    let verbatim = app
        .rpc("terminal.logs.read", json!({"name": name, "redact": false}))
        .await
        .expect("read");
    assert_eq!(verbatim["redacted"], false);
    assert!(
        verbatim["text"]
            .as_str()
            .expect("text")
            .contains("Sup3rS3cret")
    );
}
