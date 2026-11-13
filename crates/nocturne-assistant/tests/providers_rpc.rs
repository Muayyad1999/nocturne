//! Providers: the documents, the key that only ever lives in the vault, and
//! `ai.providers.test` against a local server speaking each API.

mod common;

use common::http::{Reply, Server};
use common::{Harness, Options};
use serde_json::json;

/// An app with the assistant and nothing scripted: these tests drive the
/// provider methods, not a conversation.
async fn harness() -> Harness {
    Harness::with(Options {
        devices: vec!["SW1"],
        ..Options::default()
    })
    .await
}

#[tokio::test]
async fn the_api_key_goes_to_the_vault_and_never_comes_back_out() {
    let harness = harness().await;
    let entry = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Claude", "kind": "anthropic", "model": "claude-opus-5-5",
                   "api_key": "sk-ant-secret-key-value", "effort": "high"}),
        )
        .await
        .unwrap();
    let id = entry["id"].as_str().unwrap().to_string();

    // The entry says a key is set and does not carry it.
    assert_eq!(entry["has_key"], true);
    assert_eq!(entry["effort"], "high");
    assert_eq!(entry["kind"], "anthropic");
    assert!(
        entry.get("api_key").is_none(),
        "the key must not come back: {entry}"
    );
    let serialised = entry.to_string();
    assert!(!serialised.contains("sk-ant-secret-key-value"));

    // Nor in the list, nor in the status, nor in the document store.
    let list = harness
        .app()
        .rpc("ai.providers.list", json!({}))
        .await
        .unwrap()
        .to_string();
    assert!(!list.contains("sk-ant-secret-key-value"));
    let status = harness
        .app()
        .rpc("ai.status", json!({}))
        .await
        .unwrap()
        .to_string();
    assert!(!status.contains("sk-ant-secret-key-value"));
    let stored: serde_json::Value = harness
        .app()
        .store()
        .get("ai_providers", &id)
        .unwrap()
        .unwrap();
    assert!(!stored.to_string().contains("sk-ant-secret-key-value"));

    // It is in the vault, under the id the specification fixes.
    assert_eq!(
        harness
            .app()
            .vault()
            .get(&format!("ai:provider:{id}:key"))
            .unwrap()
            .map(|key| key.to_string()),
        Some("sk-ant-secret-key-value".to_string())
    );

    // Saving again without a key keeps the stored one; an empty one clears it.
    let kept = harness
        .app()
        .rpc("ai.providers.save", json!({"id": id, "name": "Claude 2"}))
        .await
        .unwrap();
    assert_eq!(kept["has_key"], true);
    assert_eq!(kept["name"], "Claude 2");
    let cleared = harness
        .app()
        .rpc("ai.providers.save", json!({"id": id, "api_key": ""}))
        .await
        .unwrap();
    assert_eq!(cleared["has_key"], false);

    // Deleting takes the vault entry with it.
    harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"id": id, "api_key": "sk-again"}),
        )
        .await
        .unwrap();
    harness
        .app()
        .rpc("ai.providers.delete", json!({"id": id}))
        .await
        .unwrap();
    assert!(
        harness
            .app()
            .vault()
            .get(&format!("ai:provider:{id}:key"))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        harness
            .app()
            .rpc("ai.providers.delete", json!({"id": id}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn exactly_one_provider_is_the_default() {
    let harness = harness().await;
    // The harness already saved one, which became the default by being first.
    let first = harness.provider_id.clone();
    let second = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Local", "kind": "ollama", "model": "qwen3:8b"}),
        )
        .await
        .unwrap();
    assert_eq!(second["is_default"], false);
    assert_eq!(
        second["base_url"], "http://127.0.0.1:11434",
        "a sensible base URL when none is given"
    );

    let promoted = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"id": second["id"], "is_default": true}),
        )
        .await
        .unwrap();
    assert_eq!(promoted["is_default"], true);
    let list = harness
        .app()
        .rpc("ai.providers.list", json!({}))
        .await
        .unwrap();
    let defaults: Vec<&serde_json::Value> = list
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["is_default"] == json!(true))
        .collect();
    assert_eq!(defaults.len(), 1);
    assert_eq!(defaults[0]["id"], second["id"]);
    assert_eq!(list[0]["id"], second["id"], "the default is listed first");

    // Deleting the default leaves something else as the default.
    harness
        .app()
        .rpc("ai.providers.delete", json!({"id": second["id"]}))
        .await
        .unwrap();
    let list = harness
        .app()
        .rpc("ai.providers.list", json!({}))
        .await
        .unwrap();
    assert_eq!(list[0]["id"], json!(first));
    assert_eq!(list[0]["is_default"], true);

    // A provider with no model is not a provider.
    let err = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Empty", "kind": "openai", "model": "  "}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("model"), "{}", err.message);
}

#[tokio::test]
async fn testing_a_provider_reports_whether_the_model_can_use_tools() {
    // A model with native tool calling.
    let native = Server::start(vec![Reply::sse_data(&[json!({"choices": [{"delta": {
        "tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                        "function": {"name": "report_ok", "arguments": "{\"ok\": true}"}}]},
        "finish_reason": "tool_calls"}]})])])
    .await;
    let harness = harness().await;
    harness.real_providers();
    let entry = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Native", "kind": "openai", "model": "gpt-test",
                   "base_url": format!("{}/v1", native.url), "api_key": "sk-test"}),
        )
        .await
        .unwrap();
    let result = harness
        .app()
        .rpc("ai.providers.test", json!({"id": entry["id"]}))
        .await
        .unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["tool_calling"], "native");
    assert_eq!(result["model"], "gpt-test");
    assert!(result["latency_ms"].as_u64().is_some());
    let seen = native.seen();
    assert_eq!(seen.len(), 1, "one tiny request, no more");
    assert_eq!(seen[0].body["tools"][0]["function"]["name"], "report_ok");

    // A model that answers but has no tool API: the second attempt describes
    // the tools in the prompt, and that works.
    let prompted = Server::start(vec![
        Reply::sse_data(&[
            json!({"choices": [{"delta": {"content": "I cannot call tools."},
                                              "finish_reason": "stop"}]}),
        ]),
        Reply::sse_data(&[json!({"choices": [{"delta": {"content":
            "```tool\n{\"name\": \"report_ok\", \"arguments\": {\"ok\": true}}\n```"},
            "finish_reason": "stop"}]})]),
    ])
    .await;
    let entry = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Local", "kind": "openai", "model": "tinymodel",
                   "base_url": format!("{}/v1", prompted.url)}),
        )
        .await
        .unwrap();
    let result = harness
        .app()
        .rpc("ai.providers.test", json!({"id": entry["id"]}))
        .await
        .unwrap();
    assert_eq!(result["ok"], true, "{result}");
    assert_eq!(result["tool_calling"], "prompted");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("no native tool calling"),
        "{result}"
    );
    let seen = prompted.seen();
    assert_eq!(seen.len(), 2);
    assert!(seen[1].body.get("tools").is_none());

    // A model that cannot use tools at all, either way.
    let useless = Server::start(vec![
        Reply::sse_data(&[
            json!({"choices": [{"delta": {"content": "Hello!"}, "finish_reason": "stop"}]}),
        ]),
        Reply::sse_data(&[
            json!({"choices": [{"delta": {"content": "Hello again!"}, "finish_reason": "stop"}]}),
        ]),
    ])
    .await;
    let entry = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Chatty", "kind": "openai", "model": "chatty",
                   "base_url": format!("{}/v1", useless.url)}),
        )
        .await
        .unwrap();
    let result = harness
        .app()
        .rpc("ai.providers.test", json!({"id": entry["id"]}))
        .await
        .unwrap();
    assert_eq!(result["tool_calling"], "none");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("cannot use tools"),
        "{result}"
    );
}

#[tokio::test]
async fn testing_a_provider_reports_a_rejected_key_as_a_failure_not_a_crash() {
    let server = Server::start(vec![Reply::json(
        401,
        json!({"error": {"message": "invalid x-api-key", "type": "authentication_error"}}),
    )])
    .await;
    let harness = harness().await;
    harness.real_providers();
    let entry = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Claude", "kind": "anthropic", "model": "claude-opus-5-5",
                   "base_url": server.url, "api_key": "sk-ant-wrong"}),
        )
        .await
        .unwrap();
    let result = harness
        .app()
        .rpc("ai.providers.test", json!({"id": entry["id"]}))
        .await
        .unwrap();
    assert_eq!(result["ok"], false, "{result}");
    assert_eq!(result["tool_calling"], "none");
    let message = result["error"].as_str().unwrap_or_default();
    assert!(message.contains("invalid x-api-key"), "{message}");
    assert!(
        message.contains("Settings → AI"),
        "and what to do about it: {message}"
    );
    // One attempt only: a rejected key is not retried with another tool mode.
    assert_eq!(server.seen().len(), 1);
    assert_eq!(server.seen()[0].path, "/v1/messages");
    assert_eq!(server.seen()[0].header("x-api-key"), Some("sk-ant-wrong"));
}

#[tokio::test]
async fn models_can_be_listed_for_a_saved_provider_or_a_typed_in_endpoint() {
    let server = Server::start(vec![
        Reply::json(
            200,
            json!({"data": [{"id": "gpt-test-1"}, {"id": "gpt-test-2"}]}),
        ),
        Reply::json(200, json!({"data": [{"id": "gpt-test-1"}]})),
    ])
    .await;
    let harness = harness().await;

    // Before anything is saved: the "add provider" dialog asks like this.
    let models = harness
        .app()
        .rpc(
            "ai.providers.models",
            json!({"kind": "openai", "base_url": format!("{}/v1", server.url), "api_key": "sk-x"}),
        )
        .await
        .unwrap();
    let ids: Vec<&str> = models
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["gpt-test-1", "gpt-test-2"]);
    assert_eq!(
        server.seen()[0].header("authorization"),
        Some("Bearer sk-x")
    );

    // And afterwards, by id, with the key from the vault.
    let entry = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Fixture", "kind": "openai", "model": "gpt-test-1",
                   "base_url": format!("{}/v1", server.url), "api_key": "sk-stored"}),
        )
        .await
        .unwrap();
    let models = harness
        .app()
        .rpc("ai.providers.models", json!({"id": entry["id"]}))
        .await
        .unwrap();
    assert_eq!(models[0]["id"], "gpt-test-1");
    assert_eq!(
        server.seen()[1].header("authorization"),
        Some("Bearer sk-stored"),
        "the key came from the vault, not from the caller"
    );

    // Neither an id nor a kind is not a question anybody can answer.
    let err = harness
        .app()
        .rpc("ai.providers.models", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "invalid");
}

#[tokio::test]
async fn the_add_provider_dialog_has_presets_and_can_look_for_local_runtimes() {
    let harness = harness().await;
    let presets = harness
        .app()
        .rpc("ai.providers.presets", json!({}))
        .await
        .unwrap();
    let ids: Vec<&str> = presets
        .as_array()
        .unwrap()
        .iter()
        .map(|preset| preset["id"].as_str().unwrap())
        .collect();
    for expected in ["anthropic", "ollama", "lmstudio", "openai", "custom"] {
        assert!(
            ids.contains(&expected),
            "{expected} is missing from {ids:?}"
        );
    }
    let claude = presets
        .as_array()
        .unwrap()
        .iter()
        .find(|preset| preset["id"] == json!("anthropic"))
        .unwrap();
    assert_eq!(claude["needs_key"], true);
    assert_eq!(claude["local"], false);
    let ollama = presets
        .as_array()
        .unwrap()
        .iter()
        .find(|preset| preset["id"] == json!("ollama"))
        .unwrap();
    assert_eq!(ollama["local"], true);
    assert_eq!(ollama["needs_key"], false);

    // Nothing is listening on this machine, so nothing is found — and that is
    // an empty list, not an error.
    let found = harness
        .app()
        .rpc("ai.providers.detect_local", json!({}))
        .await
        .unwrap();
    assert!(found.is_array(), "{found}");

    // The quick actions are there for the UI too.
    let actions = harness.app().rpc("ai.presets", json!({})).await.unwrap();
    assert!(actions.as_array().unwrap().len() >= 7);
    assert!(
        actions
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["id"] == json!("why_down"))
    );
}

#[tokio::test]
async fn a_chat_is_bound_to_the_kind_of_model_it_started_with() {
    let harness = harness().await;
    harness.script(vec![nocturne_ai::provider::scripted::Step::text("hello")]);
    harness.send("hello").await;

    // Switching the chat's provider to another API is refused, with what to
    // do about it.
    let anthropic = harness
        .app()
        .rpc(
            "ai.providers.save",
            json!({"name": "Claude", "kind": "anthropic", "model": "claude-opus-5-5",
                   "api_key": "sk-ant", "is_default": true}),
        )
        .await
        .unwrap();
    harness
        .app()
        .rpc("ai.providers.delete", json!({"id": harness.provider_id}))
        .await
        .unwrap();
    let err = harness
        .app()
        .rpc(
            "ai.send",
            json!({"chat_id": harness.chat_id, "text": "and now?"}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "conflict");
    assert!(err.message.contains("Start a new chat"), "{}", err.message);

    // A new chat carries a summary of the old one over, without a model call.
    let carried = harness
        .app()
        .rpc(
            "ai.chats.create",
            json!({"provider_id": anthropic["id"], "from_chat_id": harness.chat_id}),
        )
        .await
        .unwrap();
    assert_eq!(carried["provider_kind"], "anthropic");
    let view = harness
        .app()
        .rpc("ai.chats.get", json!({"id": carried["id"]}))
        .await
        .unwrap();
    assert_eq!(view["messages"][0]["blocks"][0]["type"], "notice");
    assert!(
        view["messages"][0]["blocks"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Carried over")
    );
    assert!(
        view["chat"]["title"]
            .as_str()
            .unwrap()
            .contains("(continued)")
    );
}

#[tokio::test]
async fn chats_can_be_listed_renamed_and_deleted() {
    let harness = harness().await;
    harness.script(vec![nocturne_ai::provider::scripted::Step::text("hi")]);
    harness.send("first question").await;

    let renamed = harness
        .app()
        .rpc(
            "ai.chats.rename",
            json!({"id": harness.chat_id, "title": "  VLAN work  "}),
        )
        .await
        .unwrap();
    assert_eq!(renamed["title"], "VLAN work");
    assert_eq!(
        harness
            .app()
            .rpc(
                "ai.chats.rename",
                json!({"id": harness.chat_id, "title": " "})
            )
            .await
            .unwrap_err()
            .code,
        "invalid"
    );

    let list = harness.app().rpc("ai.chats.list", json!({})).await.unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["title"], "VLAN work");
    assert_eq!(list[0]["messages"], 2);

    harness
        .app()
        .rpc("ai.chats.delete", json!({"id": harness.chat_id}))
        .await
        .unwrap();
    assert!(
        harness
            .app()
            .rpc("ai.chats.list", json!({}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        harness
            .app()
            .rpc("ai.chats.get", json!({"id": harness.chat_id}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn usage_is_recorded_per_provider_and_model() {
    let harness = harness().await;
    harness.script(vec![
        nocturne_ai::provider::scripted::Step::text("one"),
        nocturne_ai::provider::scripted::Step::text("two"),
    ]);
    harness.send("first").await;
    harness.send("second").await;

    let usage = harness.app().rpc("ai.usage", json!({})).await.unwrap();
    assert_eq!(usage["totals"]["turns"], 2);
    assert!(usage["totals"]["input_tokens"].as_u64().unwrap() >= 200);
    assert_eq!(usage["by"].as_array().unwrap().len(), 1);
    assert_eq!(usage["by"][0]["provider_id"], json!(harness.provider_id));
    assert_eq!(usage["by"][0]["model"], "scripted");
    assert_eq!(usage["by"][0]["turns"], 2);

    // Asking from the future gives nothing, not an error.
    let later = harness
        .app()
        .rpc(
            "ai.usage",
            json!({"since": nocturne_app::util::now_ms() + 86_400_000}),
        )
        .await
        .unwrap();
    assert_eq!(later["totals"]["turns"], 0);
    assert!(later["by"].as_array().unwrap().is_empty());
}
