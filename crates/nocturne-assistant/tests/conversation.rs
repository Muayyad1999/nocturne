//! A whole conversation, end to end: the events in order, what the model was
//! sent, what was shaped for it, and what is left in the history afterwards.

mod common;

use common::{Harness, Options};
use nocturne_ai::provider::scripted::Step;
use serde_json::json;

#[tokio::test]
async fn lists_devices_runs_commands_reads_a_config_and_proposes_a_change() {
    let harness = Harness::new().await;
    let id = harness.lab.id("SW1");
    harness.script(vec![
        Step::Tools(
            Some("Let me see what is there.".into()),
            vec![("list_devices".into(), json!({"site": "Lab"}))],
        ),
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show vlan brief", "show ip interface brief"]}),
        ),
        Step::tool(
            "get_config",
            json!({"device": "SW1", "section": "interface GigabitEthernet0/2"}),
        ),
        Step::tool(
            "propose_change",
            json!({
                "title": "Move desk 12 to VLAN 40",
                "description": "The user asked for VLAN 40 on the desk port.",
                "targets": [{"device": "SW1", "lines": [
                    "interface GigabitEthernet0/2",
                    " switchport access vlan 40"
                ]}],
            }),
        ),
        Step::text("I drafted the change; it is low risk. Approve it in the Changes screen."),
    ]);

    let turn = harness.send("put desk 12 on vlan 40 on SW1").await;

    // Events: every tool call starts and ends, usage is reported, done last.
    assert_eq!(turn.stop(), "end", "{:#?}", turn.kinds());
    assert_eq!(
        turn.tool_names(),
        vec![
            "list_devices",
            "run_commands",
            "get_config",
            "propose_change"
        ]
    );
    let results = turn.tool_results();
    assert_eq!(results.len(), 4);
    assert!(
        results.iter().all(|(_, status, _)| status == "ok"),
        "{results:#?}"
    );
    assert!(
        results[0].2.contains("device"),
        "a summary for the UI: {:?}",
        results[0].2
    );
    assert!(turn.text().contains("I drafted the change"));
    assert!(!turn.of_kind("usage").is_empty());
    assert_eq!(
        turn.kinds().last().map(String::as_str),
        Some("done"),
        "done is the last event"
    );

    // `seq` increases by one per chat, so a re-attaching UI can spot a gap.
    let seqs = turn.seqs();
    assert_eq!(
        seqs,
        (seqs[0]..seqs[0] + seqs.len() as u64).collect::<Vec<_>>(),
        "contiguous sequence numbers"
    );

    // The change was created against the real device, by the assistant.
    let changes = harness.stored_changes();
    let change = &changes[0];
    assert_eq!(change["created_by"], "assistant");
    assert_eq!(change["origin"], "assistant");
    assert_eq!(change["state"], "planned");
    assert_eq!(change["targets"][0]["device_id"], json!(id));
    assert_eq!(
        change["targets"][0]["plan"]["commands"],
        json!(["interface GigabitEthernet0/2", " switchport access vlan 40"])
    );
    // Nothing was sent to the device itself.
    assert!(
        !harness
            .lab
            .fake("SW1")
            .running_config()
            .contains("switchport access vlan 40"),
        "a proposed change must not reach the device"
    );

    // The link on the tool result points at the change, so the UI can draw a card.
    let proposed = turn.of_kind("tool_end")[3];
    assert_eq!(proposed["link"]["type"], "change");
    assert_eq!(proposed["link"]["id"], change["id"]);

    // History: readable, in order, and reloadable.
    let messages = harness.messages().await;
    assert_eq!(messages.len(), 2, "{messages:#?}");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(
        messages[0]["blocks"][0]["text"], "put desk 12 on vlan 40 on SW1",
        "the briefing is not part of what the user said"
    );
    let blocks = messages[1]["blocks"].as_array().unwrap();
    assert_eq!(messages[1]["role"], "assistant");
    let kinds: Vec<&str> = blocks
        .iter()
        .map(|block| block["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        vec!["text", "tool", "tool", "tool", "tool", "text"],
        "{blocks:#?}"
    );
    assert_eq!(blocks[1]["status"], "ok");
    assert_eq!(blocks[1]["name"], "list_devices");
    assert!(
        blocks[1]["input_summary"]
            .as_str()
            .unwrap()
            .contains("site: Lab")
    );
    assert!(messages[1]["usage"]["input_tokens"].as_u64().unwrap() > 0);

    // The model saw the tool results as delimited data, not as prose.
    let sent = harness.sent_to_model();
    assert!(sent.contains("<nocturne:data source="), "{sent}");
    assert!(sent.contains("show vlan brief"));
    assert!(
        sent.contains("<nocturne:briefing>"),
        "the briefing travels with the user's message"
    );
    assert!(
        harness
            .sent_to_model()
            .contains("never configure a device directly"),
        "the system prompt states the boundary"
    );

    // The chat was titled from the first message, and usage was recorded.
    let chats = harness.app().rpc("ai.chats.list", json!({})).await.unwrap();
    assert_eq!(chats[0]["title"], "put desk 12 on vlan 40 on SW1");
    assert_eq!(chats[0]["messages"], 2);
    let usage = harness.app().rpc("ai.usage", json!({})).await.unwrap();
    assert!(usage["totals"]["turns"].as_u64().unwrap() >= 1);
    assert_eq!(usage["by"][0]["model"], "scripted");
}

#[tokio::test]
async fn results_are_capped_with_an_honest_note_about_what_is_missing() {
    let harness = Harness::with(Options {
        devices: vec!["SW1"],
        settings: json!({"result_rows": 2, "result_chars": 400}),
        ..Options::default()
    })
    .await;
    // A device with a great many VLANs, so the table has to be cut.
    harness.lab.fake("SW1").with(|state| {
        let mut config = String::new();
        for vlan in 100..160 {
            config.push_str(&format!("vlan {vlan}\n name tenant{vlan}\n"));
        }
        state.load(&config);
    });
    harness.script(vec![
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show vlan brief"]}),
        ),
        Step::tool("get_collected", json!({"device": "SW1", "kind": "vlans"})),
        Step::text("done"),
    ]);
    // Collected data the tool can read back, stored the way a feeder does.
    let rows: Vec<serde_json::Value> = (100..160)
        .map(|vlan| json!({"id": vlan, "name": format!("tenant{vlan}"), "status": "active"}))
        .collect();
    nocturne_app::collect::store(
        harness.app(),
        &harness.lab.id("SW1"),
        nocturne_drivers::model::Kind::Vlans,
        rows,
        "lab",
    )
    .unwrap();

    let turn = harness.send("how many vlans does SW1 have?").await;
    assert_eq!(turn.stop(), "end");

    let sent = harness.sent_to_model();
    assert!(
        sent.contains("characters left out of the middle"),
        "long command output is capped head and tail: {sent}"
    );
    assert!(
        sent.contains("more row(s) not shown of 60"),
        "a capped table says how many rows are missing: {sent}"
    );
    assert!(sent.contains("filter"), "and what to do about it: {sent}");
}

#[tokio::test]
async fn one_turn_per_chat_but_two_chats_can_run_at_once() {
    let harness = Harness::new().await;
    let second = harness
        .app()
        .rpc("ai.chats.create", json!({}))
        .await
        .unwrap();
    let second_id = second["id"].as_str().unwrap().to_string();

    // A tool call that waits for a device, so the first turn is still running.
    harness.script(vec![
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show version"]}),
        ),
        Step::text("first chat answered"),
        Step::tool(
            "run_commands",
            json!({"devices": ["SW2"], "commands": ["show version"]}),
        ),
        Step::text("second chat answered"),
    ]);

    harness
        .start(&harness.chat_id, "look at SW1")
        .await
        .unwrap();
    // The same chat refuses a second turn.
    let clash = harness
        .app()
        .rpc(
            "ai.send",
            json!({"chat_id": harness.chat_id, "text": "and again"}),
        )
        .await
        .unwrap_err();
    assert_eq!(clash.code, "conflict");
    assert!(
        clash.message.contains("still answering"),
        "{}",
        clash.message
    );

    // A different chat may run at the same time.
    let other = harness
        .send_in(&second_id, "look at SW2", json!(null))
        .await;
    assert_eq!(other.stop(), "end");
    assert!(
        other
            .events
            .iter()
            .all(|event| event["chat_id"] == json!(second_id)),
        "events are per chat"
    );
}

#[tokio::test]
async fn a_tool_error_mid_turn_is_the_models_problem_not_a_failed_turn() {
    let harness = Harness::new().await;
    harness.lab.fake("SW1").with(|state| {
        state.outputs.clear();
    });
    harness.script(vec![
        Step::tool("get_device", json!({"device": "nothing-like-this"})),
        Step::tool("get_device", json!({"device": "SW1"})),
        Step::text("SW1 is a Test IOS switch; there is no device called nothing-like-this."),
    ]);
    let turn = harness.send("tell me about nothing-like-this").await;
    assert_eq!(turn.stop(), "end");
    let results = turn.tool_results();
    assert_eq!(results[0].1, "error");
    assert!(
        results[0].2.contains("no device called"),
        "{:?}",
        results[0].2
    );
    assert_eq!(results[1].1, "ok");
    // The model was told, in words it can relay.
    let sent = harness.sent_to_model();
    assert!(
        sent.contains("use list_devices to see what exists"),
        "{sent}"
    );
    // And the history records the failed call as a failed call.
    let messages = harness.messages().await;
    let blocks = messages[1]["blocks"].as_array().unwrap();
    assert_eq!(blocks[0]["status"], "error");
}

#[tokio::test]
async fn the_step_budget_ends_the_turn_and_tells_the_user() {
    let harness = Harness::with(Options {
        settings: json!({"max_steps": 2}),
        ..Options::default()
    })
    .await;
    harness.script(vec![
        Step::tool("list_devices", json!({})),
        Step::tool("list_devices", json!({})),
        Step::tool("list_devices", json!({})),
        Step::text("I ran out of room."),
    ]);
    let turn = harness.send("keep looking").await;
    assert_eq!(turn.stop(), "max_steps");
    let results = turn.tool_results();
    assert_eq!(results.len(), 3);
    assert_eq!(results[2].1, "error");
    assert!(results[2].2.contains("budget"), "{:?}", results[2].2);
    assert!(
        harness.sent_to_model().contains("budget of 2 tool calls"),
        "the model is told to wrap up"
    );
}

#[tokio::test]
async fn cancelling_mid_tool_leaves_a_consistent_history() {
    let harness = Harness::new().await;
    harness.script(vec![
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1", "SW2"], "commands": ["show running-config"]}),
        ),
        Step::text("never reached"),
    ]);
    let mut events = harness.app().subscribe();
    harness
        .start(&harness.chat_id, "read both configs")
        .await
        .unwrap();

    // Cancel as soon as the tool has started.
    loop {
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
            .await
            .expect("an event arrived")
            .unwrap();
        if event.topic == "ai.event" && event.payload["event"]["type"] == json!("tool_start") {
            break;
        }
    }
    let cancelled = harness
        .app()
        .rpc("ai.cancel", json!({"chat_id": harness.chat_id}))
        .await
        .unwrap();
    assert_eq!(cancelled["cancelled"], true);

    // The turn ends as cancelled, and the stored history still holds a result
    // for the call that was in flight.
    let mut stop = String::new();
    while stop.is_empty() {
        let event = tokio::time::timeout(std::time::Duration::from_secs(10), events.recv())
            .await
            .expect("an event arrived")
            .unwrap();
        if event.topic == "ai.event" && event.payload["event"]["type"] == json!("done") {
            stop = event.payload["event"]["stop"].as_str().unwrap().to_string();
        }
    }
    assert_eq!(stop, "cancelled");
    let messages = harness.messages().await;
    let last = messages.last().unwrap();
    let blocks = last["blocks"].as_array().unwrap();
    assert!(
        blocks.iter().any(|block| block["type"] == json!("notice")),
        "the history says what happened: {blocks:#?}"
    );
    // Cancelling again is not an error, it simply reports nothing was running.
    let again = harness
        .app()
        .rpc("ai.cancel", json!({"chat_id": harness.chat_id}))
        .await
        .unwrap();
    assert_eq!(again["cancelled"], false);
}

#[tokio::test]
async fn history_survives_a_restart_and_is_replayed_to_the_model() {
    let harness = Harness::new().await;
    harness.script(vec![
        Step::tool("list_devices", json!({})),
        Step::text("two switches."),
        Step::Dynamic(Box::new(|request| {
            // The second turn must carry the first one with it.
            let all = request
                .messages
                .iter()
                .map(|message| message.text())
                .collect::<Vec<_>>()
                .join("|");
            assert!(all.contains("how many switches"), "{all}");
            assert!(all.contains("two switches."), "{all}");
            assert!(
                request.messages.len() >= 4,
                "the tool call and its result are replayed: {:#?}",
                request.messages
            );
            Step::text("still two.")
        })),
    ]);
    harness.send("how many switches are there?").await;
    let second = harness.send("and now?").await;
    assert_eq!(second.text(), "still two.");
    assert_eq!(harness.messages().await.len(), 4);

    // The sequence numbers keep rising across turns of one chat.
    let view = harness
        .app()
        .rpc("ai.chats.get", json!({"id": harness.chat_id}))
        .await
        .unwrap();
    assert!(view["seq"].as_u64().unwrap() > 1);
    assert_eq!(view["chat"]["messages"], 4);
}

#[tokio::test]
async fn with_no_provider_configured_sending_says_where_to_add_one() {
    let harness = Harness::new().await;
    harness
        .app()
        .rpc("ai.providers.delete", json!({"id": harness.provider_id}))
        .await
        .unwrap();
    let err = harness
        .app()
        .rpc(
            "ai.send",
            json!({"chat_id": harness.chat_id, "text": "anything"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "unavailable");
    assert!(err.message.contains("Settings → AI"), "{}", err.message);
    assert!(err.message.contains("Ollama"), "{}", err.message);

    let status = harness.app().rpc("ai.status", json!({})).await.unwrap();
    assert_eq!(status["configured"], false);
    assert_eq!(status["redaction"], true);
    assert!(status["default"].is_null());
    // Creating a chat says the same thing rather than failing silently.
    let err = harness
        .app()
        .rpc("ai.chats.create", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "unavailable");
}
