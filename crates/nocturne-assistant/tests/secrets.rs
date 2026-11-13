//! The secret test.
//!
//! Secrets are put everywhere a real network puts them — the configuration,
//! command output, interface descriptions, log lines, alert details, error
//! messages, the team's notes, the terminal, the vault — and then a
//! conversation reads all of it. Afterwards: every byte that was sent to the
//! provider (system prompt, tool descriptions, tool results, context blocks,
//! titles), everything published on `ai.event` and everything persisted in the
//! chat history is searched for every secret. None may appear anywhere.
//!
//! The last part checks the other direction: that putting a secret back for
//! the change engine cannot be turned into a way of reading it.

mod common;

use common::stubs::{SECRETS, back_up_everything, config_with_secrets};
use common::{Harness, Options};
use nocturne_ai::provider::scripted::Step;
use serde_json::json;

/// The vault's own values, which are not in any configuration rule's way.
const VAULT_SECRETS: &[&str] = &["Str0ngDevicePassword", "En4bleS3cretValue"];

async fn harness_with_secrets() -> Harness {
    let harness = Harness::with(Options {
        devices: vec!["SW1", "SW2"],
        ..Options::default()
    })
    .await;
    // A credential whose values Nocturne knows and the model must not learn,
    // even though no pattern in a configuration would match them.
    harness
        .app()
        .rpc(
            "credentials.update",
            json!({"id": harness.lab.credential_id, "password": VAULT_SECRETS[0],
                   "enable_secret": VAULT_SECRETS[1]}),
        )
        .await
        .expect("credential updated");
    // The device itself prints secrets: its configuration, a show command, an
    // interface description and its banner.
    for name in ["SW1", "SW2"] {
        harness.lab.fake(name).with(|state| {
            state.load(&config_with_secrets(name));
            state.enable_secret = Some(VAULT_SECRETS[1].into());
            state.outputs.insert(
                "show snmp community".into(),
                format!(
                    "Community name: {}\nCommunity Index: {}\n",
                    SECRETS[0], SECRETS[0]
                ),
            );
            state.outputs.insert(
                "show run | include key".into(),
                format!(
                    " ntp authentication-key 1 md5 {}\n radius-server key {}\n",
                    SECRETS[4], SECRETS[1]
                ),
            );
            state.outputs.insert(
                "show tech-support".into(),
                format!("api-key: {}\n", SECRETS[9]),
            );
        });
    }
    // The configurations are backed up, as they are on a network Nocturne has
    // been watching for more than five minutes. That is where the chat's
    // redactor learns this network's secrets.
    back_up_everything(harness.app());
    harness
}

#[tokio::test]
async fn no_secret_reaches_the_model_the_event_stream_or_the_history() {
    let harness = harness_with_secrets().await;
    harness.script(vec![
        Step::tool("get_config", json!({"device": "SW1"})),
        Step::tool("get_config", json!({"device": "SW2"})),
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1", "SW2"],
                   "commands": ["show snmp community", "show run | include key", "show tech-support", "show running-config"]}),
        ),
        Step::tool("search_configs", json!({"query": "snmp-server community"})),
        Step::tool("diff_config", json!({"device": "SW1"})),
        Step::tool("query_logs", json!({"stream": "syslog"})),
        Step::tool("list_alerts", json!({})),
        Step::tool("list_insights", json!({})),
        Step::tool("compliance_results", json!({})),
        Step::tool("interface_status", json!({"device": "SW1"})),
        Step::tool("search_notes", json!({"query": "wan"})),
        Step::tool("explain_terminal", json!({"session_id": "term1"})),
        Step::tool("get_collected", json!({"device": "SW1", "kind": "interfaces"})),
        Step::text("Here is what I found; every secret is hidden behind a placeholder."),
    ]);

    // The user's own selection is full of secrets too, and so is the context.
    let turn = harness
        .send_with(
            "read everything you can about SW1 and SW2",
            json!({
                "device_ids": [harness.lab.id("SW1")],
                "page": "devices",
                "selection": config_with_secrets("SW1"),
                "terminal_session_id": "term1",
            }),
        )
        .await;
    assert_eq!(turn.stop(), "end", "{:#?}", turn.kinds());
    assert_eq!(turn.tool_results().len(), 13);

    // 1. Everything sent to the model.
    let sent = harness.sent_to_model();
    for secret in SECRETS.iter().chain(VAULT_SECRETS) {
        assert!(
            !sent.contains(secret),
            "`{secret}` was sent to the model.\n--- what was sent ---\n{sent}"
        );
    }
    // The structure around the secrets is still there, so the model can work.
    assert!(sent.contains("snmp-server community <secret:"), "{sent}");
    assert!(sent.contains("hostname SW1"));
    assert!(sent.contains("router bgp 65001"));
    assert!(
        sent.contains("ntp authentication-key 1 md5 <secret:"),
        "the algorithm stays readable, the key does not: {sent}"
    );

    // The same value has the same placeholder everywhere, across tools and
    // devices, so the model can still say "both switches use the same key".
    let placeholder = sent
        .split("snmp-server community ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("a placeholder")
        .to_string();
    assert!(placeholder.starts_with("<secret:"), "{placeholder}");
    assert!(
        sent.matches(&placeholder).count() >= 4,
        "one stable placeholder for one secret: {placeholder} appears {} times",
        sent.matches(&placeholder).count()
    );

    // 2. Everything published and everything stored.
    let published = harness.everything_published_and_stored(&[&turn]).await;
    for secret in SECRETS.iter().chain(VAULT_SECRETS) {
        assert!(
            !published.contains(secret),
            "`{secret}` was published or stored.\n{published}"
        );
    }

    // 3. The audit trail, which also goes past a human.
    let audit = harness
        .app()
        .rpc("audit.list", json!({"limit": 500}))
        .await
        .unwrap()
        .to_string();
    for secret in SECRETS.iter().chain(VAULT_SECRETS) {
        assert!(!audit.contains(secret), "`{secret}` is in the audit trail");
    }
}

#[tokio::test]
async fn an_error_message_from_a_device_is_redacted_too() {
    let harness = harness_with_secrets().await;
    // The device refuses a line and quotes it back, secret and all.
    harness.lab.fake("SW1").with(|state| {
        state.outputs.insert(
            "show logging".into(),
            format!("%AUTH-3-FAIL: login with {} refused\n", SECRETS[1]),
        );
    });
    harness.script(vec![
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show logging"]}),
        ),
        Step::tool("get_config", json!({"device": "no-such-device"})),
        Step::text("done"),
    ]);
    let turn = harness.send("why did authentication fail?").await;
    let everything = format!(
        "{}{}",
        harness.sent_to_model(),
        harness.everything_published_and_stored(&[&turn]).await
    );
    for secret in SECRETS {
        assert!(!everything.contains(secret), "`{secret}` leaked");
    }
}

#[tokio::test]
async fn restoring_a_placeholder_cannot_be_turned_into_a_way_of_reading_it() {
    let harness = harness_with_secrets().await;
    harness.script(vec![
        // Read a configuration, so the chat has a placeholder table.
        Step::tool("get_config", json!({"device": "SW1"})),
        // Move the secret into a line no redaction rule would ever match…
        Step::tool(
            "propose_change",
            json!({"title": "Move the community string",
            "targets": [{"device": "SW1", "lines": [
                "interface GigabitEthernet0/2",
                " description community <secret:1> and <secret:2>"
            ]}]}),
        ),
        // …then read the change back, which is where it would come out.
        Step::Dynamic(Box::new(|request| {
            let change_id = request
                .messages
                .iter()
                .rev()
                .flat_map(|message| message.content.iter())
                .filter_map(|block| match block {
                    nocturne_ai::Block::ToolResult { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .find_map(|content| {
                    content
                        .split("id: ")
                        .nth(1)
                        .and_then(|rest| rest.split_whitespace().next())
                        .map(str::to_string)
                })
                .expect("the change id came back to the model");
            Step::tool("get_change", json!({"id": change_id}))
        })),
        // And through the other door: a configuration search for the value.
        Step::tool("search_configs", json!({"query": "description community"})),
        Step::text("The secret is still hidden."),
    ]);

    let turn = harness
        .send("move the community string into a description")
        .await;
    assert_eq!(turn.stop(), "end", "{:#?}", turn.kinds());

    // The change engine did get the real values: that is the point of
    // restoration, and the change is what a human reviews.
    let changes = harness.app().rpc("changes.list", json!({})).await.unwrap();
    let commands = changes[0]["targets"][0]["plan"]["commands"].to_string();
    assert!(
        commands.contains(SECRETS[0]) || commands.contains(SECRETS[2]),
        "the real value reaches the change engine: {commands}"
    );

    // The model never got it back.
    let everything = format!(
        "{}{}",
        harness.sent_to_model(),
        harness.everything_published_and_stored(&[&turn]).await
    );
    for secret in SECRETS.iter().chain(VAULT_SECRETS) {
        assert!(
            !everything.contains(secret),
            "`{secret}` came back through the restoration path"
        );
    }
    assert!(
        harness
            .sent_to_model()
            .contains("description community <secret:"),
        "the model still sees its own placeholder"
    );
}

#[tokio::test]
async fn a_placeholder_the_model_invented_is_not_quietly_sent_to_a_device() {
    let harness = harness_with_secrets().await;
    harness.script(vec![
        Step::tool(
            "propose_change",
            json!({"title": "Set a community string",
                   "targets": [{"device": "SW1", "lines": ["snmp-server community <secret:4096> RO"]}]}),
        ),
        Step::text("I could not resolve that placeholder."),
    ]);
    let turn = harness.send("set the snmp community back").await;
    assert_eq!(turn.stop(), "end");
    let sent = harness.sent_to_model();
    assert!(
        sent.contains("<secret:4096> did not match any secret from this conversation"),
        "{sent}"
    );
    // The line is still in the draft, as text, where a human will see it — it
    // is not silently dropped and not silently filled in.
    let changes = harness.app().rpc("changes.list", json!({})).await.unwrap();
    assert!(
        changes[0]["targets"][0]["plan"]["commands"]
            .to_string()
            .contains("<secret:4096>")
    );
}

#[tokio::test]
async fn the_placeholder_table_lives_in_the_vault_and_nowhere_else() {
    let harness = harness_with_secrets().await;
    harness.script(vec![
        Step::tool("get_config", json!({"device": "SW1"})),
        Step::text("read"),
    ]);
    harness.send("read the config of SW1").await;

    // The table is in the vault, under this chat's id.
    let stored = harness
        .app()
        .vault()
        .get(&format!("ai:chat:{}:redaction", harness.chat_id))
        .unwrap()
        .expect("the table was stored");
    assert!(
        stored.contains(SECRETS[0]),
        "the table holds the real values"
    );

    // Deleting the chat deletes it.
    harness
        .app()
        .rpc("ai.chats.delete", json!({"id": harness.chat_id}))
        .await
        .unwrap();
    assert!(
        harness
            .app()
            .vault()
            .get(&format!("ai:chat:{}:redaction", harness.chat_id))
            .unwrap()
            .is_none(),
        "the secrets went with the chat"
    );
}
