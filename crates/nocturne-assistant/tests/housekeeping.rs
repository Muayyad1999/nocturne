//! What happens around a conversation: the audit trail, the turn's own clock,
//! and a turn that the process did not live to finish.

mod common;

use common::{Harness, Options};
use nocturne_ai::provider::scripted::Step;
use serde_json::json;

#[tokio::test]
async fn everything_that_touched_a_device_or_a_change_is_audited_with_the_chat() {
    let harness = Harness::new().await;
    harness.script(vec![
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show vlan brief"]}),
        ),
        Step::tool(
            "collect_data",
            json!({"devices": ["SW1"], "kinds": ["facts"]}),
        ),
        Step::tool(
            "propose_change",
            json!({"title": "Add VLAN 40",
                   "targets": [{"device": "SW1", "lines": ["vlan 40", " name guests"]}]}),
        ),
        // Reads that do not touch a device are not audit-worthy.
        Step::tool("subnet_calc", json!({"input": "10.0.4.0/22"})),
        Step::text("done"),
    ]);
    let turn = harness.send("add vlan 40 to SW1").await;
    assert_eq!(turn.stop(), "end", "{:#?}", turn.kinds());

    let audit = harness
        .app()
        .rpc("audit.list", json!({"limit": 200}))
        .await
        .unwrap();
    let entries = audit["entries"]
        .as_array()
        .cloned()
        .or_else(|| audit.as_array().cloned())
        .expect("audit entries");
    let assistant: Vec<&serde_json::Value> = entries
        .iter()
        .filter(|entry| {
            entry["source"]
                .as_str()
                .unwrap_or_default()
                .starts_with("assistant.")
        })
        .collect();
    let sources: Vec<&str> = assistant
        .iter()
        .map(|entry| entry["source"].as_str().unwrap())
        .collect();
    assert!(sources.contains(&"assistant.exec"), "{sources:?}");
    assert!(sources.contains(&"assistant.collect"), "{sources:?}");
    assert!(sources.contains(&"assistant.change.create"), "{sources:?}");
    assert!(
        !sources.contains(&"assistant.subnet"),
        "arithmetic is not an event in the life of the network"
    );

    let exec = assistant
        .iter()
        .find(|entry| entry["source"] == json!("assistant.exec"))
        .unwrap();
    assert_eq!(
        exec["message"],
        format!("Assistant ran `show vlan brief` on SW1")
    );
    assert_eq!(exec["device_id"], json!(harness.lab.id("SW1")));
    assert_eq!(exec["body"]["chat_id"], json!(harness.chat_id));
    assert_eq!(exec["body"]["tool"], "run_commands");
}

#[tokio::test]
async fn a_turn_that_runs_past_its_time_limit_is_stopped_and_says_so() {
    let harness = Harness::with(Options {
        devices: vec!["SW1"],
        // The tool will wait for an approval that never comes, for longer
        // than the whole turn is allowed to take.
        settings: json!({"turn_timeout_s": 1, "confirm_reads": true, "approval_timeout_s": 120}),
        ..Options::default()
    })
    .await;
    harness.script(vec![
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show version"]}),
        ),
        Step::text("never reached"),
    ]);
    let turn = harness.send("look at SW1").await;
    assert_eq!(turn.stop(), "cancelled");
    assert!(
        turn.of_kind("notice").iter().any(|notice| notice["text"]
            .as_str()
            .unwrap_or_default()
            .contains("time limit")),
        "{:#?}",
        turn.events
    );
    // Nothing was sent to the device, and the chat can be used again.
    assert!(harness.lab.fake("SW1").seen().is_empty());
    let messages = harness.messages().await;
    assert_eq!(messages.len(), 2);
    assert!(
        harness
            .app()
            .rpc(
                "ai.send",
                json!({"chat_id": harness.chat_id, "text": "again"})
            )
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn a_turn_interrupted_by_a_restart_is_repaired_on_the_next_start() {
    let harness = Harness::new().await;
    harness.script(vec![Step::text("hello")]);
    harness.send("hello").await;

    // Pretend the process stopped mid-turn: the chat is left holding a turn.
    let mut chat = nocturne_assistant::chats::get(harness.app(), &harness.chat_id).unwrap();
    chat.pending_turn = Some("interrupted-turn".into());
    nocturne_assistant::chats::save(harness.app(), &chat).unwrap();

    // What `init`'s start-up hook does.
    nocturne_assistant::chats::repair_interrupted(harness.app()).unwrap();

    let messages = harness.messages().await;
    let last = messages.last().unwrap();
    assert_eq!(last["role"], "assistant");
    assert_eq!(last["turn_id"], "interrupted-turn");
    assert_eq!(last["blocks"][0]["type"], "notice");
    assert!(
        last["blocks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Nocturne was closed"),
        "{last}"
    );
    let chat = nocturne_assistant::chats::get(harness.app(), &harness.chat_id).unwrap();
    assert!(chat.pending_turn.is_none(), "and it is not repaired twice");
    nocturne_assistant::chats::repair_interrupted(harness.app()).unwrap();
    assert_eq!(harness.messages().await.len(), messages.len());
}

#[tokio::test]
async fn the_team_s_own_instructions_travel_with_every_turn() {
    let harness = Harness::with(Options {
        devices: vec!["SW1"],
        settings: json!({
            "system_prompt_extra":
                "Change window is 22:00-02:00. Access ports are named `desk-<number>`.",
        }),
        ..Options::default()
    })
    .await;
    harness.script(vec![Step::text("understood")]);
    harness
        .send_with(
            "what is our change window?",
            json!({"device_ids": [harness.lab.id("SW1")], "page": "devices"}),
        )
        .await;

    let request = &harness.provider().requests()[0];
    let briefing = request.messages[0].text();
    assert!(
        briefing.contains("Change window is 22:00-02:00"),
        "the team's instructions: {briefing}"
    );
    assert!(
        briefing.contains("autonomy: draft"),
        "the mode in plain words: {briefing}"
    );
    assert!(
        briefing.contains("device: SW1"),
        "what the user is looking at: {briefing}"
    );
    assert!(
        briefing.contains("2 device(s)") || briefing.contains("1 device(s)"),
        "a snapshot of the network: {briefing}"
    );
    assert!(briefing.contains("date: "), "{briefing}");
    assert!(
        briefing.contains("what is our change window?"),
        "and the question itself"
    );
    // The system prompt itself is the frozen part: nothing volatile in it.
    assert!(!request.system.contains("Change window is 22:00-02:00"));
    assert!(
        !request.system.contains("autonomy: draft"),
        "the mode itself is not in the frozen prefix"
    );
    assert!(!request.system.contains("date: "));
}

#[tokio::test]
async fn compaction_starts_a_new_generation_and_keeps_the_old_one() {
    let harness = Harness::new().await;
    let app = harness.app();
    let mut chat = nocturne_assistant::chats::get(app, &harness.chat_id).unwrap();
    assert_eq!(chat.generation, 1);

    let history = vec![
        nocturne_ai::Message::user("first"),
        nocturne_ai::Message::assistant("answer"),
        nocturne_ai::Message::user("second"),
    ];
    nocturne_assistant::chats::save_context(app, &mut chat, &history, false).unwrap();
    assert_eq!(
        nocturne_assistant::chats::load_context(app, &chat).unwrap(),
        history
    );

    // The agent compacted: one summary message replaces the three.
    let compacted = vec![nocturne_ai::Message::user("[Summary] first and second")];
    nocturne_assistant::chats::save_context(app, &mut chat, &compacted, true).unwrap();
    assert_eq!(chat.generation, 2);
    assert_eq!(
        nocturne_assistant::chats::load_context(app, &chat).unwrap(),
        compacted,
        "the model sees only the new generation"
    );
    assert_eq!(
        app.store()
            .count(&format!("ai_context.{}", harness.chat_id))
            .unwrap(),
        4,
        "and nothing was deleted"
    );

    // A turn that shortened the history without compacting (an error before
    // the assistant message) leaves no stale tail behind.
    let shorter = vec![nocturne_ai::Message::user("only this")];
    nocturne_assistant::chats::save_context(app, &mut chat, &shorter, false).unwrap();
    assert_eq!(
        nocturne_assistant::chats::load_context(app, &chat).unwrap(),
        shorter
    );
}

#[tokio::test]
async fn the_registered_surface_is_the_contract() {
    let mut builder = nocturne_app::testing::builder();
    nocturne_assistant::init(&mut builder);
    let app = builder.build();
    let ours: Vec<&str> = app
        .methods()
        .into_iter()
        .filter(|method| method.starts_with("ai."))
        .collect();
    assert_eq!(
        ours,
        vec![
            "ai.approve",
            "ai.cancel",
            "ai.chats.create",
            "ai.chats.delete",
            "ai.chats.get",
            "ai.chats.list",
            "ai.chats.rename",
            "ai.presets",
            "ai.providers.delete",
            "ai.providers.detect_local",
            "ai.providers.list",
            "ai.providers.models",
            "ai.providers.presets",
            "ai.providers.save",
            "ai.providers.test",
            "ai.send",
            "ai.status",
            // Extra: what the assistant can reach in this build.
            "ai.tools",
            "ai.usage",
        ]
    );
    // The settings section exists with every key the UI expects.
    let settings = nocturne_app::settings::get_value(&app, "assistant").unwrap();
    let mut keys: Vec<&str> = settings
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "approval_timeout_s",
            "autonomy",
            "confirm_reads",
            "isp_access",
            "max_concurrent_turns",
            "max_steps",
            "result_chars",
            "result_rows",
            "system_prompt_extra",
            "turn_timeout_s",
        ]
    );
    assert_eq!(settings["autonomy"], "draft");
    assert_eq!(settings["max_steps"], 40);
    assert_eq!(settings["confirm_reads"], false);
    // The customer ledger is closed to the model until somebody opens it.
    assert_eq!(settings["isp_access"], false);
}
