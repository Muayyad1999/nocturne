//! One sentence in, one configured device out — with nothing stubbed.
//!
//! This is the claim the whole product rests on, and until now nothing tested
//! it. It could not: the assistant's own tests stub the change pipeline, and
//! they are right to. A feature crate may not depend on another
//! (`CLAUDE.md`), so `nocturne-assistant`'s tests cannot see the real
//! `nocturne-changes` — their `plan_hash` is literally `hash-2`, and no
//! device is ever touched by them. The change engine's own tests, from the
//! other side, apply to a real device but never involve a model.
//!
//! The suite is the one crate where both exist, so this is where the two
//! halves are joined: a prompt reaches the real assistant, which calls the
//! real change engine, which configures a real (simulated) device, and the
//! device is read back afterwards.
//!
//! What it must show, beyond "it worked":
//!
//! * The model never chose a command that was sent. It states an intent; the
//!   deterministic planner turns that into commands.
//! * A refusal is a refusal at the device, not a sentence after the fact.

use std::sync::Arc;
use std::time::Duration;

use nocturne_ai::provider::scripted::{Scripted, Step};
use nocturne_app::testing::{self, Lab};
use nocturne_suite::{AppBuilder, AppConfig};
use serde_json::{Value, json};

struct Stack {
    lab: Lab,
    chat_id: String,
}

/// `nocturne_app::testing` ships `test_ios`, which is `experimental`, and the
/// autonomy policy will not apply to an unverified device type unattended —
/// rightly, and one of the tests below is about exactly that. For the others
/// the same device type is registered again, as a built-in, marked
/// `verified`: only a built-in may call itself that. This is what
/// `nocturne-changes`' own tests do.
const VERIFIED: &str = "test_ios_verified";

fn verified_profile() -> String {
    format!(
        "{}\nmaturity: verified\n",
        testing::LAB_IOS.replace("id: test_ios", &format!("id: {VERIFIED}"))
    )
}

/// The whole backend, assembled as the application assembles it, with one
/// simulated switch in the inventory and a scripted model.
async fn stack(autonomy: &str, steps: Vec<Step>) -> Stack {
    stack_on(autonomy, steps, true).await
}

async fn stack_on(autonomy: &str, steps: Vec<Step>, verified: bool) -> Stack {
    // Not `testing::builder()`: that registers the core, and `init` registers
    // the core and every feature. The point of this test is the real set.
    let mut builder = AppBuilder::new(AppConfig::in_memory()).expect("an in-memory app");
    nocturne_suite::init(&mut builder);
    nocturne_app::profiles::register(&mut builder, &verified_profile());
    let lab = testing::lab(builder, &["SW1"]).await;
    if verified {
        lab.app
            .rpc(
                "devices.update",
                json!({"id": lab.id("SW1"), "profile": VERIFIED}),
            )
            .await
            .expect("the verified device type");
    }

    nocturne_assistant::providers::use_provider(&lab.app, Arc::new(Scripted::new(steps)));
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "assistant", "value": {"autonomy": autonomy}}),
        )
        .await
        .expect("the autonomy mode is set");
    lab.app
        .rpc(
            "ai.providers.save",
            json!({"name": "Scripted", "kind": "openai",
                   // Never reached: `use_provider` replaced it. A record has
                   // to exist for the assistant to consider itself usable.
                   "base_url": "http://127.0.0.1:1/v1", "model": "scripted",
                   "api_key": "sk-test-not-a-real-key", "is_default": true}),
        )
        .await
        .expect("a provider record exists");
    let chat = lab
        .app
        .rpc("ai.chats.create", json!({}))
        .await
        .expect("a chat");
    let chat_id = chat["id"].as_str().expect("id").to_string();
    Stack { lab, chat_id }
}

impl Stack {
    /// Send one message and wait for the turn to finish.
    async fn say(&self, text: &str) -> Vec<Value> {
        let mut events = self.lab.app.subscribe();
        self.lab
            .app
            .rpc("ai.send", json!({"chat_id": self.chat_id, "text": text}))
            .await
            .expect("the message was accepted");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let mut seen = Vec::new();
        loop {
            let event = tokio::time::timeout_at(deadline, events.recv())
                .await
                .unwrap_or_else(|_| panic!("the turn did not finish; so far: {seen:#?}"))
                .expect("the event bus stayed open");
            if event.topic != "ai.event" || event.payload["chat_id"] != json!(self.chat_id) {
                continue;
            }
            let done = event.payload["event"]["type"] == json!("done");
            seen.push(event.payload);
            if done {
                return seen;
            }
        }
    }

    /// The whole record of the one change, not the summary: `changes.list`
    /// returns a `ChangeSummary`, which carries no plan.
    async fn change(&self) -> Value {
        let list = self
            .lab
            .app
            .rpc("changes.list", json!({}))
            .await
            .expect("the change list");
        let id = list[0]["id"].as_str().expect("a change was created");
        self.lab
            .app
            .rpc("changes.get", json!({"id": id}))
            .await
            .expect("the change reads back")
    }
}

/// Ask the model to propose a change and then to apply it, the way the
/// assistant's own tests drive it: the second step has to read the change id
/// out of the first step's result, because the model is the only thing that
/// knows it.
fn propose_then_apply(device: &str, lines: Value) -> Vec<Step> {
    vec![
        Step::tool(
            "propose_change",
            json!({"title": format!("guest VLAN on {device}"),
                   "targets": [{"device": device, "lines": lines}]}),
        ),
        Step::Dynamic(Box::new(|request| {
            let id = request
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
            Step::tool("apply_change", json!({"id": id}))
        })),
        Step::text("done"),
    ]
}

#[tokio::test]
async fn a_sentence_becomes_a_line_in_a_devices_running_configuration() {
    let stack = stack(
        "lab",
        propose_then_apply(
            "SW1",
            json!(["interface GigabitEthernet0/2", " switchport access vlan 20"]),
        ),
    )
    .await;

    let before = stack.lab.fake("SW1").running_config();
    assert!(
        before.contains(" switchport access vlan 10"),
        "the port starts on VLAN 10:\n{before}"
    );

    stack.say("put the desk on Gi0/2 onto the voice VLAN").await;

    // The record first: it must show the real pipeline, not a shortcut.
    let change = stack.change().await;
    assert_eq!(change["state"], "applied", "{change:#?}");
    assert_eq!(
        change["approval"]["by"], "policy:lab",
        "approved by the autonomy policy, not by the model"
    );
    let target = &change["targets"][0];
    let plan = &target["plan"];
    assert_eq!(plan["profile"], VERIFIED);
    assert!(
        plan["plan_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() >= 16),
        "a real plan hash, not a stub's: {:?}",
        plan["plan_hash"]
    );
    // The rollback is the planner's, and a real one: removing the VLAN
    // assignment, not deleting the interface.
    let rollback = plan["rollback"].as_array().expect("a rollback");
    assert!(
        rollback
            .iter()
            .any(|line| line.as_str().is_some_and(|line| line.contains("vlan 10"))),
        "rollback should restore the VLAN the port was on: {rollback:?}"
    );

    // Then the device, which is the only witness that matters.
    let after = stack.lab.fake("SW1").running_config();
    assert!(
        after.contains(" switchport access vlan 20"),
        "the device should be on the voice VLAN now:\n{after}"
    );
    assert!(
        !after.contains(" switchport access vlan 10"),
        "and not still on the old one:\n{after}"
    );

    // Verification ran against the device, and the engine saved afterwards.
    let checks = target["result"]["verification"]
        .as_array()
        .expect("verification ran");
    assert!(
        checks.iter().any(|check| check["ok"] == json!(true)),
        "{checks:#?}"
    );
    assert!(
        stack.lab.fake("SW1").with(|state| state.saved()),
        "a verified change is saved"
    );

    // And the thing that separates this from a model with a shell: every
    // command the device saw came from the planner. The model's own words
    // were an intent, which is all `propose_change` accepts.
    assert_eq!(
        target["intent"]["type"], "lines",
        "the model stated an intent, not a session"
    );
}

#[tokio::test]
async fn a_refused_apply_leaves_the_device_exactly_as_it_was() {
    let stack = stack(
        "draft",
        propose_then_apply(
            "SW1",
            json!(["interface GigabitEthernet0/2", " switchport access vlan 20"]),
        ),
    )
    .await;

    let before = stack.lab.fake("SW1").running_config();
    stack.lab.fake("SW1").clear_seen();

    stack.say("put the desk on Gi0/2 onto the voice VLAN").await;

    let change = stack.change().await;
    assert_eq!(
        change["state"], "planned",
        "in draft mode it waits for a human: {change:#?}"
    );

    // This is the assertion that cannot be made in the assistant's own tests,
    // because there the change engine is a stub that never touches anything.
    // Here the engine is real, so a refusal that let a command through would
    // show up — and nothing may have been sent at all.
    assert_eq!(
        stack.lab.fake("SW1").running_config(),
        before,
        "the device must be untouched"
    );
    let sent = stack.lab.fake("SW1").seen();
    assert!(
        !sent
            .iter()
            .any(|command| { command.starts_with("configure") || command.contains("switchport") }),
        "nothing that configures anything may have been sent: {sent:?}"
    );
}

/// The rule from `CLAUDE.md`, proved where it matters: a device type nobody
/// has checked against real hardware is never applied to unattended, even in
/// the most permissive mode and even on a simulated device. `test_ios` is
/// `experimental`, so leaving it in place is the whole test.
#[tokio::test]
async fn an_unverified_device_type_is_never_applied_to_unattended() {
    let stack = stack_on(
        "lab",
        propose_then_apply(
            "SW1",
            json!(["interface GigabitEthernet0/2", " switchport access vlan 20"]),
        ),
        false,
    )
    .await;

    let before = stack.lab.fake("SW1").running_config();
    stack.say("put the desk on Gi0/2 onto the voice VLAN").await;

    let change = stack.change().await;
    assert_eq!(change["state"], "planned", "{change:#?}");
    assert_eq!(
        change["targets"][0]["plan"]["maturity"], "experimental",
        "the plan records which device type it was made for"
    );
    assert_eq!(
        stack.lab.fake("SW1").running_config(),
        before,
        "and the device is untouched"
    );
}
