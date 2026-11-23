//! Shared scaffolding: an in-memory app with the assistant registered, fake
//! switches to point it at, a scripted model in place of a real one, and stub
//! RPC methods for the features that are not part of this build.
//!
//! Nothing here reaches the network or a model endpoint.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use nocturne_ai::provider::scripted::{Scripted, Step};
use nocturne_app::testing::{self, Lab};
use nocturne_app::{App, AppBuilder};
use parking_lot::Mutex;
use serde_json::{Value, json};

pub mod http;
pub mod reach;
pub mod stubs;

/// An app with the assistant, the stub features and a scripted model.
pub struct Harness {
    pub lab: Lab,
    provider: Mutex<Arc<Scripted>>,
    pub provider_id: String,
    pub chat_id: String,
}

pub struct Options {
    pub devices: Vec<&'static str>,
    /// Talk to a real provider at this base URL instead of the scripted one
    /// (a local fixture server), in this tool mode.
    pub endpoint: Option<(String, &'static str)>,
    /// Tokens of context the configured model has; under 16k is "small".
    pub context_window: Option<u32>,
    /// Register the change-pipeline stubs.
    pub changes: bool,
    /// Register the stubs of every other feature.
    pub features: bool,
    /// Register the ISP edition's ledger as well: the paid edition's build.
    pub isp: bool,
    /// Methods of the stand-ins in `reach.rs` to leave out, for a build in
    /// which that part of a feature does not exist.
    pub without: Vec<&'static str>,
    /// More methods to register, for a test that brings stand-ins of its own.
    pub extra: Option<fn(&mut AppBuilder)>,
    pub settings: Value,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            devices: vec!["SW1", "SW2"],
            endpoint: None,
            context_window: None,
            changes: true,
            features: true,
            isp: false,
            without: Vec::new(),
            extra: None,
            settings: json!({}),
        }
    }
}

impl Harness {
    pub async fn new() -> Harness {
        Harness::with(Options::default()).await
    }

    pub async fn with(options: Options) -> Harness {
        let mut builder: AppBuilder = testing::builder();
        nocturne_assistant::init(&mut builder);
        stubs::register(&mut builder, &options);
        let lab = testing::lab(builder, &options.devices).await;

        let provider = Arc::new(Scripted::new(Vec::new()));
        if options.endpoint.is_none() {
            nocturne_assistant::providers::use_provider(&lab.app, provider.clone());
        }
        let provider = Mutex::new(provider);

        if options.settings != json!({}) {
            lab.app
                .rpc(
                    "settings.set",
                    json!({"section": "assistant", "value": options.settings}),
                )
                .await
                .expect("settings");
        }

        let (base_url, tool_mode) = match &options.endpoint {
            Some((url, mode)) => (format!("{url}/v1"), *mode),
            None => ("http://127.0.0.1:1/v1".to_string(), "auto"),
        };
        let mut save = json!({
            "name": "Scripted", "kind": "openai",
            "base_url": base_url, "model": "scripted", "tool_mode": tool_mode,
            "api_key": "sk-test-not-a-real-key", "is_default": true,
        });
        if let Some(window) = options.context_window {
            save["context_window"] = json!(window);
        }
        let entry = lab
            .app
            .rpc("ai.providers.save", save)
            .await
            .expect("provider saved");
        let provider_id = entry["id"].as_str().expect("id").to_string();
        let chat = lab
            .app
            .rpc("ai.chats.create", json!({}))
            .await
            .expect("chat created");
        let chat_id = chat["id"].as_str().expect("id").to_string();
        Harness {
            lab,
            provider,
            provider_id,
            chat_id,
        }
    }

    pub fn app(&self) -> &App {
        &self.lab.app
    }

    /// What the model will answer, in order. Scripting again starts a fresh
    /// model, so everything recorded so far is forgotten: a test that looks at
    /// what was sent scripts once.
    pub fn script(&self, steps: Vec<Step>) {
        let provider = Arc::new(Scripted::new(steps));
        *self.provider.lock() = provider.clone();
        nocturne_assistant::providers::use_provider(self.app(), provider);
    }

    /// Build providers from their records, as the application does.
    pub fn real_providers(&self) {
        nocturne_assistant::providers::use_real_providers(self.app());
    }

    pub fn provider(&self) -> Arc<Scripted> {
        self.provider.lock().clone()
    }

    /// Send a message and collect every `ai.event` of the turn, up to `done`.
    pub async fn send(&self, text: &str) -> Turn {
        self.send_with(text, Value::Null).await
    }

    pub async fn send_with(&self, text: &str, context: Value) -> Turn {
        self.send_in(&self.chat_id, text, context).await
    }

    pub async fn send_in(&self, chat_id: &str, text: &str, context: Value) -> Turn {
        let mut params = json!({"chat_id": chat_id, "text": text});
        if !context.is_null() {
            params["context"] = context;
        }
        let mut events = self.app().subscribe();
        let started = self
            .app()
            .rpc("ai.send", params)
            .await
            .expect("ai.send accepted the message");
        let turn_id = started["turn_id"].as_str().expect("turn_id").to_string();
        let mut collected = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let event = tokio::time::timeout_at(deadline, events.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("the turn did not finish; events so far: {collected:#?}")
                })
                .expect("the event bus stayed open");
            if event.topic != "ai.event" || event.payload["chat_id"] != json!(chat_id) {
                continue;
            }
            let finished = event.payload["event"]["type"] == json!("done");
            collected.push(event.payload);
            if finished {
                break;
            }
        }
        Turn {
            turn_id,
            events: collected,
        }
    }

    /// Send a message and answer the questions the turn asks, in order:
    /// `Some(true)` allows, `Some(false)` refuses, `None` — or running out of
    /// answers — leaves the question to time out. The `approval_needed`
    /// events are in the returned turn like every other event.
    pub async fn send_answering(&self, text: &str, answers: &[Option<bool>]) -> Turn {
        self.send_answering_within(text, answers, Duration::from_secs(40))
            .await
    }

    /// As [`Harness::send_answering`], for a turn that is slow on purpose:
    /// one whose features answer with megabytes takes its time on a machine
    /// that is busy with other things.
    pub async fn send_answering_within(
        &self,
        text: &str,
        answers: &[Option<bool>],
        patience: Duration,
    ) -> Turn {
        let mut answers = answers.iter();
        let mut events = self.app().subscribe();
        let started = self
            .app()
            .rpc("ai.send", json!({"chat_id": self.chat_id, "text": text}))
            .await
            .expect("ai.send accepted the message");
        let turn_id = started["turn_id"].as_str().expect("turn_id").to_string();
        let mut collected = Vec::new();
        let deadline = tokio::time::Instant::now() + patience;
        loop {
            let event = tokio::time::timeout_at(deadline, events.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!("the turn did not finish; events so far: {collected:#?}")
                })
                .expect("the event bus stayed open");
            if event.topic != "ai.event" || event.payload["chat_id"] != json!(self.chat_id) {
                continue;
            }
            let kind = event.payload["event"]["type"].as_str().unwrap_or_default();
            if kind == "approval_needed"
                && let Some(Some(allow)) = answers.next()
            {
                self.app()
                    .rpc(
                        "ai.approve",
                        json!({"chat_id": self.chat_id, "allow": allow,
                               "call_id": event.payload["event"]["call_id"]}),
                    )
                    .await
                    .expect("the answer was accepted");
            }
            let finished = kind == "done";
            collected.push(event.payload);
            if finished {
                break;
            }
        }
        Turn {
            turn_id,
            events: collected,
        }
    }

    /// Every tool result the model was sent in this conversation, in order:
    /// what it read, and whether it was told the call had failed.
    pub fn results_sent_to_model(&self) -> Vec<(String, bool)> {
        let requests = self.provider().requests();
        let Some(last) = requests.last() else {
            return Vec::new();
        };
        last.messages
            .iter()
            .flat_map(|message| message.content.iter())
            .filter_map(|block| match block {
                nocturne_ai::Block::ToolResult {
                    content, is_error, ..
                } => Some((content.clone(), *is_error)),
                _ => None,
            })
            .collect()
    }

    /// `ai.send` without waiting, for the tests about turns that overlap.
    pub async fn start(&self, chat_id: &str, text: &str) -> Result<String, nocturne_app::AppError> {
        let started = self
            .app()
            .rpc("ai.send", json!({"chat_id": chat_id, "text": text}))
            .await?;
        Ok(started["turn_id"].as_str().unwrap_or_default().to_string())
    }

    /// The changes the stand-in change engine holds, whole and oldest
    /// first — what `changes.get` says of each. (`changes.list` answers with
    /// summaries, as the real one does.)
    pub fn stored_changes(&self) -> Value {
        Value::Array(self.app().state::<stubs::Changes>().records.lock().clone())
    }

    pub async fn messages(&self) -> Vec<Value> {
        let view = self
            .app()
            .rpc("ai.chats.get", json!({"id": self.chat_id}))
            .await
            .expect("chat read back");
        view["messages"].as_array().cloned().unwrap_or_default()
    }

    /// Every byte this chat sent to the model: system prompts, user messages,
    /// tool results, tool descriptions.
    pub fn sent_to_model(&self) -> String {
        let mut all = String::new();
        for request in self.provider().requests() {
            all.push_str(&request.system);
            all.push('\n');
            for tool in &request.tools {
                all.push_str(&tool.name);
                all.push_str(&tool.description);
                all.push_str(&tool.input_schema.to_string());
                all.push('\n');
            }
            for message in &request.messages {
                all.push_str(&serde_json::to_string(message).expect("a message is serialisable"));
                all.push('\n');
            }
        }
        all
    }

    /// Everything the UI was told and everything that was stored.
    pub async fn everything_published_and_stored(&self, turns: &[&Turn]) -> String {
        let mut all = String::new();
        for turn in turns {
            for event in &turn.events {
                all.push_str(&event.to_string());
                all.push('\n');
            }
        }
        for message in self.messages().await {
            all.push_str(&message.to_string());
            all.push('\n');
        }
        let chats = self
            .app()
            .rpc("ai.chats.list", json!({}))
            .await
            .expect("chats");
        all.push_str(&chats.to_string());
        all
    }
}

/// The events of one turn.
#[derive(Debug)]
pub struct Turn {
    pub turn_id: String,
    pub events: Vec<Value>,
}

impl Turn {
    pub fn kinds(&self) -> Vec<String> {
        self.events
            .iter()
            .map(|event| event["event"]["type"].as_str().unwrap_or("?").to_string())
            .collect()
    }

    pub fn of_kind(&self, kind: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter(|event| event["event"]["type"] == json!(kind))
            .map(|event| &event["event"])
            .collect()
    }

    pub fn seqs(&self) -> Vec<u64> {
        self.events
            .iter()
            .filter_map(|event| event["seq"].as_u64())
            .collect()
    }

    pub fn text(&self) -> String {
        self.of_kind("text")
            .iter()
            .filter_map(|event| event["delta"].as_str())
            .collect::<Vec<_>>()
            .join("")
    }

    pub fn stop(&self) -> String {
        self.of_kind("done")
            .first()
            .and_then(|event| event["stop"].as_str())
            .unwrap_or_default()
            .to_string()
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.of_kind("tool_start")
            .iter()
            .filter_map(|event| event["name"].as_str())
            .map(str::to_string)
            .collect()
    }

    /// `(call_id, status, result_summary)` of every finished tool call.
    pub fn tool_results(&self) -> Vec<(String, String, String)> {
        self.of_kind("tool_end")
            .iter()
            .map(|event| {
                (
                    event["call_id"].as_str().unwrap_or_default().to_string(),
                    event["status"].as_str().unwrap_or_default().to_string(),
                    event["result_summary"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect()
    }
}

/// Every method that must never be reachable: the never-reachable list of
/// `boundary.rs`, which with the allow-list is every method the assembled
/// application registers. The stubs put a tripwire on every one of them this
/// build does not really have.
pub fn forbidden() -> Vec<&'static str> {
    nocturne_assistant::tools::boundary::never_names().collect()
}

/// Append one real value to the file named by `NOCTURNE_WIRE_SHAPES`, as a
/// line of JSON under a label. Off unless a run asks for it: it is how the
/// examples in a build report are taken from a run instead of from memory.
pub fn wire(label: &str, value: &Value) {
    let Ok(path) = std::env::var("NOCTURNE_WIRE_SHAPES") else {
        return;
    };
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{}", json!({"label": label, "value": value}));
    }
}

/// Records every call to a method the assistant must never be able to reach.
#[derive(Default)]
pub struct Tripwire {
    pub tripped: Mutex<Vec<String>>,
}

/// The one `Step` helper the scripted provider does not have: answer with
/// text after asserting something about the request.
pub fn step_text(text: &str) -> Step {
    Step::text(text)
}

pub fn tool(name: &str, arguments: Value) -> Step {
    Step::tool(name, arguments)
}
