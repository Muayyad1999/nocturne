//! The two kinds of model that are easy to get wrong: one with very little
//! context, and one with no tool calling at all.

mod common;

use common::http::{Reply, Server};
use common::{Harness, Options};
use nocturne_ai::provider::scripted::Step;
use serde_json::json;

#[tokio::test]
async fn a_small_context_model_gets_the_core_tools_and_tighter_caps() {
    let harness = Harness::with(Options {
        devices: vec!["SW1"],
        context_window: Some(8_000),
        ..Options::default()
    })
    .await;
    // A configuration far longer than a small model's share of context.
    harness.lab.fake("SW1").with(|state| {
        let mut config = String::from("hostname SW1\n");
        for vlan in 100..200 {
            config.push_str(&format!("vlan {vlan}\n name tenant-number-{vlan}\n"));
        }
        state.load(&config);
    });
    harness.script(vec![
        Step::tool(
            "run_commands",
            json!({"devices": ["SW1"], "commands": ["show running-config"]}),
        ),
        Step::text("read"),
    ]);
    let turn = harness.send("read the config of SW1").await;
    assert_eq!(turn.stop(), "end");

    let request = &harness.provider().requests()[0];
    let mut offered: Vec<&str> = request
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    offered.sort_unstable();
    assert_eq!(
        offered,
        vec![
            "get_change",
            "get_config",
            "get_device",
            "list_alerts",
            "list_devices",
            "ping",
            "propose_change",
            "run_commands",
            "search_configs",
            "subnet_calc",
        ],
        "a model with little room gets the core set only"
    );
    // The descriptions are the short ones.
    let run = request
        .tools
        .iter()
        .find(|tool| tool.name == "run_commands")
        .unwrap();
    assert!(
        run.description.len() < 200,
        "the short description, {} characters: {}",
        run.description.len(),
        run.description
    );
    assert!(run.description.contains("read-only"));

    // And the results are capped harder: the whole configuration of the fake
    // switch does not fit in 2 500 characters.
    let sent = harness.sent_to_model();
    assert!(sent.contains("characters left out of the middle"), "{sent}");
    let status = harness.app().rpc("ai.status", json!({})).await.unwrap();
    assert_eq!(status["tools"], 10);
}

#[tokio::test]
async fn a_model_with_no_tool_calling_works_through_the_prompted_path() {
    // The model never uses a tool-call API: it prints a fenced `tool` block,
    // which is the path `nocturne_ai` implements for local models.
    let server = Server::start(vec![
        Reply::sse_data(&[json!({"choices": [{"delta": {"content":
            "I will look at the inventory.\n```tool\n{\"name\": \"list_devices\", \"arguments\": {\"site\": \"Lab\"}}\n```"},
            "finish_reason": "stop"}]})]),
        Reply::sse_data(&[json!({"choices": [{"delta": {"content":
            "There are two switches at the Lab site: SW1 and SW2."},
            "finish_reason": "stop"}]})]),
    ])
    .await;

    let harness = Harness::with(Options {
        devices: vec!["SW1", "SW2"],
        endpoint: Some((server.url.clone(), "prompted")),
        ..Options::default()
    })
    .await;

    let turn = harness.send("what switches do we have at the lab?").await;
    assert_eq!(turn.stop(), "end", "{:#?}", turn.events);
    assert_eq!(turn.tool_names(), vec!["list_devices"]);
    assert_eq!(turn.tool_results()[0].1, "ok");
    assert!(
        turn.text().contains("two switches"),
        "the final answer: {:?}",
        turn.text()
    );

    let seen = server.seen();
    assert_eq!(seen.len(), 2, "one request per step");
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].header("authorization"),
        Some("Bearer sk-test-not-a-real-key")
    );
    assert!(
        seen[0].body.get("tools").is_none(),
        "prompted mode sends no native tools: {}",
        seen[0].body
    );
    let system = seen[0].body["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(
        system.contains("# Tools") && system.contains("list_devices"),
        "the tools are described in the prompt instead"
    );
    assert!(
        system.contains("never configure a device directly"),
        "and the boundary is still stated"
    );
    // The tool result came back as a user message, the way prompted mode does.
    let replayed = seen[1].body["messages"].to_string();
    assert!(
        replayed.contains("Tool result from list_devices"),
        "{replayed}"
    );
    assert!(replayed.contains("SW1") && replayed.contains("SW2"));

    // History is stored exactly as for a native model.
    let messages = harness.messages().await;
    assert_eq!(messages.len(), 2);
    let kinds: Vec<&str> = messages[1]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["text", "tool", "text"]);
}

#[tokio::test]
async fn a_model_endpoint_that_fails_mid_turn_leaves_the_chat_usable() {
    // Enough replies for the provider's own retries; every one the same.
    let server = Server::start(
        (0..5)
            .map(|_| {
                Reply::json(
                    500,
                    json!({"error": {"message": "the GPU fell over", "type": "server_error"}}),
                )
            })
            .collect(),
    )
    .await;
    let harness = Harness::with(Options {
        devices: vec!["SW1"],
        endpoint: Some((server.url.clone(), "auto")),
        ..Options::default()
    })
    .await;
    let turn = harness.send("hello?").await;

    let errors = turn.of_kind("error");
    assert_eq!(errors.len(), 1, "{:#?}", turn.events);
    assert!(
        errors[0]["message"]
            .as_str()
            .unwrap()
            .contains("the GPU fell over"),
        "the endpoint's own words reach the user: {}",
        errors[0]
    );
    assert_eq!(turn.stop(), "end");

    // The history says what happened, and the chat is not stuck.
    let messages = harness.messages().await;
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[1]["blocks"][0]["type"], "notice");
    let again = harness
        .app()
        .rpc(
            "ai.send",
            json!({"chat_id": harness.chat_id, "text": "still there?"}),
        )
        .await;
    assert!(again.is_ok(), "a new turn can start: {again:?}");
}
