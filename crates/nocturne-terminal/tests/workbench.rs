//! Snippets, classifying a paste, generating a key, the settings section and
//! the idle timeout.

mod common;

use std::time::Duration;

use common::{lab, open_device, session_id};
use serde_json::json;

#[tokio::test]
async fn the_settings_section_is_the_one_the_specification_names() {
    let lab = lab(&["SW1"]).await;
    let section = lab
        .app
        .rpc("settings.get", json!({"section": "terminal"}))
        .await
        .expect("settings");
    assert_eq!(
        section,
        json!({
            "scrollback_kib": 512,
            "log_sessions": false,
            "auto_login": true,
            "idle_timeout_min": 0,
            "keepalive": true,
        })
    );
}

#[tokio::test]
async fn snippets_are_saved_listed_and_deleted() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;

    assert_eq!(
        app.rpc("terminal.snippets.list", json!({}))
            .await
            .expect("list"),
        json!([])
    );

    let saved = app
        .rpc(
            "terminal.snippets.save",
            json!({
                "name": "Port bounce",
                "text": "interface Gi0/1\n shutdown\n no shutdown\n",
                "profile": "test_ios",
                "tags": ["access", " access ", "", "ports"],
            }),
        )
        .await
        .expect("save");
    let id = saved["id"].as_str().expect("id").to_string();
    assert_eq!(saved["name"], "Port bounce");
    assert_eq!(saved["profile"], "test_ios");
    // Tags are tidied, not taken as typed.
    assert_eq!(saved["tags"], json!(["access", "ports"]));
    assert!(saved["created_at"].as_i64().expect("created_at") > 0);

    let other = app
        .rpc(
            "terminal.snippets.save",
            json!({"name": "Anywhere", "text": "show version\n"}),
        )
        .await
        .expect("save");
    assert_eq!(other["profile"], json!(null));

    // Listed by name.
    let listed = app
        .rpc("terminal.snippets.list", json!({}))
        .await
        .expect("list");
    assert_eq!(listed.as_array().expect("a list").len(), 2);
    assert_eq!(listed[0]["name"], "Anywhere");
    assert_eq!(listed[1]["name"], "Port bounce");

    // Filtered by profile (a snippet for any profile always applies) and by word.
    let generic = app
        .rpc("terminal.snippets.list", json!({"profile": "generic"}))
        .await
        .expect("list");
    assert_eq!(generic.as_array().expect("a list").len(), 1);
    assert_eq!(generic[0]["name"], "Anywhere");
    let found = app
        .rpc("terminal.snippets.list", json!({"query": "shutdown"}))
        .await
        .expect("list");
    assert_eq!(found.as_array().expect("a list").len(), 1);
    assert_eq!(found[0]["name"], "Port bounce");

    // Saving with an id replaces, keeping the id and the creation time.
    let updated = app
        .rpc(
            "terminal.snippets.save",
            json!({"id": id, "name": "Port bounce", "text": "interface Gi0/2\n shutdown\n"}),
        )
        .await
        .expect("save");
    assert_eq!(updated["id"], id.as_str());
    assert_eq!(updated["created_at"], saved["created_at"]);
    assert_eq!(updated["profile"], json!(null));
    assert_eq!(
        app.rpc("terminal.snippets.list", json!({}))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .len(),
        2
    );

    // Refusals say what is wrong.
    for (params, code) in [
        (json!({"name": "", "text": "x"}), "invalid"),
        (json!({"name": "x", "text": ""}), "invalid"),
        (
            json!({"name": "x", "text": "y", "profile": "no_such_profile"}),
            "not_found",
        ),
        (
            json!({"id": "ghost", "name": "x", "text": "y"}),
            "not_found",
        ),
    ] {
        assert_eq!(
            app.rpc("terminal.snippets.save", params.clone())
                .await
                .unwrap_err()
                .code,
            code,
            "{params}"
        );
    }

    let deleted = app
        .rpc("terminal.snippets.delete", json!({"id": id}))
        .await
        .expect("delete");
    assert_eq!(deleted["deleted"], 1);
    assert_eq!(
        app.rpc("terminal.snippets.delete", json!({"id": id}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        app.rpc("terminal.snippets.list", json!({}))
            .await
            .expect("list")
            .as_array()
            .expect("a list")
            .len(),
        1
    );
    assert!(common::audit_text(app).contains("Deleted the terminal snippet `Port bounce`"));
}

#[tokio::test]
async fn a_paste_is_classified_line_by_line() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let text = "show version\n\ninterface GigabitEthernet0/1\n shutdown\nreload in 5\n";

    let verdicts = app
        .rpc(
            "terminal.classify",
            json!({"device_id": lab.id("SW1"), "text": text}),
        )
        .await
        .expect("classify");
    let verdicts = verdicts.as_array().expect("a list");
    // One entry per line, blank lines included, so the UI can line them up.
    assert_eq!(verdicts.len(), text.split('\n').count());
    assert_eq!(verdicts[0]["line"], "show version");
    assert_eq!(verdicts[0]["class"], "read_only");
    assert_eq!(verdicts[1]["line"], "");
    assert_eq!(verdicts[2]["class"], "change");
    assert_eq!(verdicts[3]["line"], " shutdown");
    assert_eq!(verdicts[3]["class"], "change");
    assert_eq!(verdicts[4]["class"], "dangerous");
    assert!(
        !verdicts[4]["reason"].as_str().expect("a reason").is_empty(),
        "a verdict must say why"
    );

    // Without a device the generic policy answers, so an ad-hoc session can
    // warn too.
    let generic = app
        .rpc(
            "terminal.classify",
            json!({"text": "reload\nshow ip route"}),
        )
        .await
        .expect("classify");
    assert_eq!(generic[0]["class"], "dangerous");
    assert_eq!(generic[1]["class"], "read_only");

    assert_eq!(
        app.rpc(
            "terminal.classify",
            json!({"device_id": "ghost", "text": "show version"})
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    // Carriage returns from a pasted Windows file are not part of the command.
    let crlf = app
        .rpc(
            "terminal.classify",
            json!({"text": "show version\r\nreload\r\n"}),
        )
        .await
        .expect("classify");
    assert_eq!(crlf[0]["line"], "show version");
    assert_eq!(crlf[1]["class"], "dangerous");
}

#[tokio::test]
async fn a_key_pair_is_generated_and_kept_nowhere() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;

    let generated = app
        .rpc(
            "keys.generate",
            json!({"kind": "ed25519", "comment": "ops@nocturne"}),
        )
        .await
        .expect("keys.generate");
    let private = generated["private_key"].as_str().expect("a private key");
    let public = generated["public_key"].as_str().expect("a public key");
    assert!(
        private.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"),
        "{private}"
    );
    assert!(
        private.ends_with("-----END OPENSSH PRIVATE KEY-----\n"),
        "{private}"
    );
    assert!(
        public.starts_with("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5"),
        "{public}"
    );
    assert!(public.ends_with(" ops@nocturne"), "{public}");
    assert_eq!(generated["kind"], "ed25519");
    assert!(
        generated["fingerprint"]
            .as_str()
            .expect("a fingerprint")
            .starts_with("SHA256:")
    );

    // Two calls give two keys, and neither is written down.
    let again = app
        .rpc("keys.generate", json!({"comment": "second"}))
        .await
        .expect("keys.generate");
    assert_ne!(again["public_key"], generated["public_key"]);
    let audit = common::audit_text(app);
    assert!(audit.contains("Generated an SSH key pair (ed25519)"));
    assert!(
        !audit.contains("PRIVATE KEY"),
        "the key reached the audit trail"
    );
    assert!(!audit.contains(public));

    for params in [json!({"kind": "dsa"}), json!({"comment": "two\nlines"})] {
        assert_eq!(
            app.rpc("keys.generate", params.clone())
                .await
                .unwrap_err()
                .code,
            "invalid",
            "{params}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_session_nobody_touches_closes_after_the_timeout() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    app.rpc(
        "settings.set",
        json!({"section": "terminal", "value": {"idle_timeout_min": 1}}),
    )
    .await
    .expect("settings");
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));

    // Virtual time: the reaper looks every 15 s and closes what has been
    // quiet for a minute.
    let mut closed = None;
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
        let session = listed
            .as_array()
            .expect("a list")
            .iter()
            .find(|session| session["id"] == id.as_str())
            .cloned()
            .expect("the session");
        if session["state"] == "closed" {
            closed = Some(session);
            break;
        }
    }
    let closed = closed.expect("the idle session should have been closed");
    assert!(
        closed["close_reason"]
            .as_str()
            .expect("a reason")
            .contains("inactivity"),
        "{}",
        closed["close_reason"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_session_being_typed_into_is_not_idle() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    app.rpc(
        "settings.set",
        json!({"section": "terminal", "value": {"idle_timeout_min": 1}}),
    )
    .await
    .expect("settings");
    let id = session_id(&open_device(app, &lab.id("SW1")).await.expect("open"));

    for _ in 0..6 {
        tokio::time::sleep(Duration::from_secs(30)).await;
        app.rpc(
            "terminal.input",
            json!({"session_id": id, "data": "show clock\r"}),
        )
        .await
        .expect("input");
    }
    let listed = app.rpc("terminal.list", json!(null)).await.expect("list");
    assert_eq!(listed[0]["state"], "open");
}
