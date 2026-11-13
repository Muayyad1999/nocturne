//! OpenAI-compatible Chat Completions.
//!
//! The lowest common denominator of the ecosystem: OpenAI itself, OpenRouter,
//! Groq, Gemini's compatibility endpoint, Mistral, DeepSeek, LM Studio, vLLM,
//! llama.cpp, LiteLLM and more. Servers differ in the details, so the request
//! is kept minimal and the response parser is forgiving.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::{
    Effort, Provider, ProviderConfig, ProviderKind, ToolMode, http, next_call_id, prompted,
};
use crate::error::AiError;
use crate::sse::SseParser;
use crate::types::{
    Block, ChatRequest, ChatResponse, Message, ModelInfo, RawContent, Role, StopReason,
    StreamEvent, Usage,
};

pub struct OpenAi {
    config: ProviderConfig,
    api_key: Option<String>,
    client: reqwest::Client,
    base: String,
    /// Tools are described in the prompt instead of the `tools` field.
    prompted: AtomicBool,
    /// The server rejected `stream_options`; stop sending it.
    no_stream_options: AtomicBool,
}

impl OpenAi {
    pub fn new(config: ProviderConfig, api_key: Option<String>) -> Result<OpenAi, AiError> {
        let base = http::trim_base(&config.base_url).to_string();
        if base.is_empty() {
            return Err(AiError::Config(
                "a base URL is required for an OpenAI-compatible provider".into(),
            ));
        }
        let api_key = api_key
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        Ok(OpenAi {
            prompted: AtomicBool::new(config.tool_mode == ToolMode::Prompted),
            no_stream_options: AtomicBool::new(false),
            client: http::client()?,
            config,
            api_key,
            base,
        })
    }

    fn is_openai_proper(&self) -> bool {
        self.base.contains("api.openai.com")
    }

    fn request_body(&self, request: &ChatRequest<'_>, prompted: bool) -> Value {
        let mut body = Map::new();
        body.insert("model".into(), json!(self.config.model));
        body.insert("stream".into(), json!(true));
        if !self.no_stream_options.load(Ordering::Relaxed) {
            body.insert("stream_options".into(), json!({"include_usage": true}));
        }
        let offer_tools = !request.tools.is_empty() && request.allow_tools;
        let mut system = request.system.to_string();
        if prompted && offer_tools {
            system.push_str(&prompted::instructions(request.tools));
        }
        body.insert(
            "messages".into(),
            Value::Array(wire_messages(&system, request.messages, prompted)),
        );
        if offer_tools && !prompted {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {"name": tool.name, "description": tool.description, "parameters": tool.input_schema},
                    })
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
        }
        if let Some(max_tokens) = self.config.max_tokens {
            // OpenAI's own newer models only accept the newer field name.
            let field = if self.is_openai_proper() {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            body.insert(field.into(), json!(max_tokens));
        }
        if let Some(effort) = self.config.effort {
            let level = match effort {
                Effort::Low => "low",
                Effort::Medium => "medium",
                Effort::High | Effort::Xhigh | Effort::Max => "high",
            };
            body.insert("reasoning_effort".into(), json!(level));
        }
        Value::Object(body)
    }

    fn authorised(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(key) = &self.api_key {
            builder = builder.bearer_auth(key);
        }
        for (name, value) in &self.config.extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        builder
    }

    async fn attempt(
        &self,
        request: &ChatRequest<'_>,
        prompted: bool,
        events: &UnboundedSender<StreamEvent>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse, AiError> {
        let body = self.request_body(request, prompted);
        let url = format!("{}/chat/completions", self.base);
        let mut response = http::send(
            || self.authorised(self.client.post(&url)).json(&body),
            cancel,
        )
        .await?;
        let idle = http::idle_timeout(&self.config);
        let mut state = StreamState::default();
        let mut parser = SseParser::default();
        while let Some(chunk) = http::next_chunk(&mut response, idle, cancel).await? {
            for event in parser.push(&chunk) {
                state.handle(&event.data, events)?;
            }
        }
        if let Some(event) = parser.finish() {
            state.handle(&event.data, events)?;
        }
        state.finish(&self.config.model, request)
    }
}

#[async_trait]
impl Provider for OpenAi {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Openai
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    fn context_window(&self) -> u32 {
        self.config
            .context_window
            .unwrap_or(if http::is_local_url(&self.base) {
                16_000
            } else {
                128_000
            })
    }

    async fn chat(
        &self,
        request: ChatRequest<'_>,
        events: &UnboundedSender<StreamEvent>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse, AiError> {
        let mut tries = 0;
        loop {
            tries += 1;
            let prompted = self.prompted.load(Ordering::Relaxed);
            match self.attempt(&request, prompted, events, cancel).await {
                Err(err)
                    if tries < 3
                        && !prompted
                        && self.config.tool_mode == ToolMode::Auto
                        && err.is_tools_unsupported() =>
                {
                    tracing::info!(model = %self.config.model, "model has no native tool calling; switching to prompted tools");
                    self.prompted.store(true, Ordering::Relaxed);
                }
                Err(AiError::Api {
                    status: 400 | 422,
                    ref message,
                    ..
                }) if tries < 3
                    && message.contains("stream_options")
                    && !self.no_stream_options.load(Ordering::Relaxed) =>
                {
                    self.no_stream_options.store(true, Ordering::Relaxed);
                }
                other => return other,
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, AiError> {
        let cancel = CancellationToken::new();
        let url = format!("{}/models", self.base);
        let response = http::send(|| self.authorised(self.client.get(&url)), &cancel).await?;
        let body: Value = response.json().await?;
        let items = body["data"]
            .as_array()
            .or_else(|| body.as_array())
            .cloned()
            .unwrap_or_default();
        let mut models: Vec<ModelInfo> = items
            .iter()
            .filter_map(|item| {
                let id = item["id"].as_str()?.to_string();
                Some(ModelInfo {
                    name: item["name"].as_str().unwrap_or(&id).to_string(),
                    context_window: item["context_length"]
                        .as_u64()
                        .or_else(|| item["context_window"].as_u64())
                        .or_else(|| item["max_context_length"].as_u64())
                        .map(|n| n as u32),
                    max_output: item["top_provider"]["max_completion_tokens"]
                        .as_u64()
                        .map(|n| n as u32),
                    id,
                })
            })
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(models)
    }
}

/// Convert neutral history to Chat Completions messages.
///
/// Consecutive user messages are merged (several chat templates used by local
/// servers insist on strict user/assistant alternation), and operator notes
/// travel as user text because a mid-conversation `system` role breaks many
/// of those templates.
pub(crate) fn wire_messages(system: &str, messages: &[Message], prompted: bool) -> Vec<Value> {
    let mut wire = vec![json!({"role": "system", "content": system})];
    // Tool names by call id, for prompted-mode result labels.
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();

    for message in messages {
        match message.role {
            Role::System => {
                let text = message.text();
                if !text.is_empty() {
                    push_user(&mut wire, format!("[Operator note]\n{text}"));
                }
            }
            Role::User => {
                for block in &message.content {
                    if let Block::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } = block
                    {
                        if prompted {
                            let name = names.get(tool_use_id.as_str()).copied().unwrap_or("tool");
                            push_user(&mut wire, prompted::render_result(name, content, *is_error));
                        } else {
                            let content = if *is_error {
                                format!("ERROR: {content}")
                            } else {
                                content.clone()
                            };
                            wire.push(json!({"role": "tool", "tool_call_id": tool_use_id, "content": content}));
                        }
                    }
                }
                let text = message.text();
                if !text.is_empty() {
                    push_user(&mut wire, text);
                }
            }
            Role::Assistant => {
                let mut text = message.text();
                let mut tool_calls = Vec::new();
                for (id, name, input) in message.tool_uses() {
                    names.insert(id, name);
                    if prompted {
                        if !text.is_empty() {
                            text.push_str("\n\n");
                        }
                        text.push_str(&prompted::render_call(name, input));
                    } else {
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {"name": name, "arguments": input.to_string()},
                        }));
                    }
                }
                let mut entry = Map::new();
                entry.insert("role".into(), json!("assistant"));
                if tool_calls.is_empty() {
                    entry.insert(
                        "content".into(),
                        json!(if text.is_empty() {
                            "(no reply)".to_string()
                        } else {
                            text
                        }),
                    );
                } else {
                    entry.insert(
                        "content".into(),
                        if text.is_empty() {
                            Value::Null
                        } else {
                            json!(text)
                        },
                    );
                    entry.insert("tool_calls".into(), Value::Array(tool_calls));
                }
                wire.push(Value::Object(entry));
            }
        }
    }
    wire
}

fn push_user(wire: &mut Vec<Value>, text: String) {
    if let Some(last) = wire.last_mut()
        && last["role"] == "user"
        && let Some(existing) = last["content"].as_str()
    {
        last["content"] = json!(format!("{existing}\n\n{text}"));
        return;
    }
    wire.push(json!({"role": "user", "content": text}));
}

#[derive(Default)]
struct PendingCall {
    id: String,
    name: String,
    arguments: String,
    announced: bool,
}

#[derive(Default)]
pub(crate) struct StreamState {
    text: String,
    thinking: String,
    calls: BTreeMap<u64, PendingCall>,
    finish_reason: Option<String>,
    usage: Usage,
    model: String,
    saw_any: bool,
}

impl StreamState {
    pub(crate) fn handle(
        &mut self,
        data: &str,
        events: &UnboundedSender<StreamEvent>,
    ) -> Result<(), AiError> {
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            return Ok(());
        }
        let chunk: Value = serde_json::from_str(data)
            .map_err(|err| AiError::Protocol(format!("unparsable stream chunk: {err}")))?;
        if let Some(error) = chunk.get("error").filter(|error| !error.is_null()) {
            return Err(http::stream_error(error));
        }
        self.saw_any = true;
        if let Some(model) = chunk["model"].as_str() {
            self.model = model.to_string();
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            let cached = usage["prompt_tokens_details"]["cached_tokens"]
                .as_u64()
                .unwrap_or(0);
            self.usage = Usage {
                input_tokens: usage["prompt_tokens"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(cached),
                output_tokens: usage["completion_tokens"].as_u64().unwrap_or(0),
                cache_read_tokens: cached,
                cache_write_tokens: 0,
            };
        }
        let Some(choice) = chunk["choices"].get(0) else {
            return Ok(());
        };
        if let Some(reason) = choice["finish_reason"].as_str() {
            self.finish_reason = Some(reason.to_string());
        }
        // Non-streaming servers put the whole message under "message".
        let delta = if choice["delta"].is_object() {
            &choice["delta"]
        } else {
            &choice["message"]
        };

        for key in ["reasoning_content", "reasoning"] {
            if let Some(text) = delta[key].as_str().filter(|text| !text.is_empty()) {
                self.thinking.push_str(text);
                let _ = events.send(StreamEvent::Thinking {
                    text: text.to_string(),
                });
                break;
            }
        }
        if let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) {
            self.text.push_str(text);
            let _ = events.send(StreamEvent::Text {
                text: text.to_string(),
            });
        }
        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for (position, call) in tool_calls.iter().enumerate() {
                let index = call["index"].as_u64().unwrap_or(position as u64);
                let pending = self.calls.entry(index).or_default();
                if let Some(id) = call["id"].as_str().filter(|id| !id.is_empty()) {
                    pending.id = id.to_string();
                }
                let function = &call["function"];
                if let Some(name) = function["name"].as_str().filter(|name| !name.is_empty()) {
                    pending.name.push_str(name);
                }
                let fragment = match &function["arguments"] {
                    Value::String(text) => text.clone(),
                    // Some servers send the arguments as an object rather than a JSON string.
                    Value::Object(_) => function["arguments"].to_string(),
                    _ => String::new(),
                };
                if pending.id.is_empty() {
                    pending.id = next_call_id();
                }
                if !pending.announced && !pending.name.is_empty() {
                    pending.announced = true;
                    let _ = events.send(StreamEvent::ToolStart {
                        id: pending.id.clone(),
                        name: pending.name.clone(),
                    });
                }
                if !fragment.is_empty() {
                    pending.arguments.push_str(&fragment);
                    let _ = events.send(StreamEvent::ToolInput {
                        id: pending.id.clone(),
                        partial: fragment,
                    });
                }
            }
        }
        Ok(())
    }

    pub(crate) fn finish(
        self,
        requested_model: &str,
        request: &ChatRequest<'_>,
    ) -> Result<ChatResponse, AiError> {
        if !self.saw_any {
            return Err(AiError::Network(
                "the model endpoint returned an empty response".into(),
            ));
        }
        let mut content = Vec::new();
        let mut invalid = Vec::new();
        if !self.thinking.trim().is_empty() {
            content.push(Block::Thinking {
                text: self.thinking,
            });
        }

        let mut text = self.text;
        let mut tool_blocks = Vec::new();
        for call in self.calls.into_values() {
            if call.name.is_empty() {
                continue;
            }
            let id = if call.id.is_empty() {
                next_call_id()
            } else {
                call.id
            };
            let input = if call.arguments.trim().is_empty() {
                json!({})
            } else {
                match serde_json::from_str::<Value>(&call.arguments) {
                    Ok(parsed @ Value::Object(_)) => parsed,
                    _ => {
                        invalid.push((id.clone(), call.arguments));
                        json!({})
                    }
                }
            };
            tool_blocks.push(Block::ToolUse {
                id,
                name: call.name,
                input,
            });
        }
        // Calls printed as text: the only channel in prompted mode, and a
        // common failure of local models in native mode.
        if tool_blocks.is_empty() && request.allow_tools && !request.tools.is_empty() {
            let (rest, parsed) = prompted::extract_calls(&text, request.tools);
            if !parsed.is_empty() {
                text = rest;
                tool_blocks.extend(parsed.into_iter().map(|call| Block::ToolUse {
                    id: next_call_id(),
                    name: call.name,
                    input: call.arguments,
                }));
            }
        }
        if !text.trim().is_empty() {
            content.push(Block::Text { text });
        }
        let has_tools = !tool_blocks.is_empty();
        content.extend(tool_blocks);

        let stop = match self.finish_reason.as_deref() {
            _ if has_tools => StopReason::ToolUse,
            Some("length") => StopReason::MaxTokens,
            Some("content_filter") => StopReason::Refusal {
                category: Some("content_filter".into()),
                explanation: None,
            },
            Some("stop") | Some("tool_calls") | Some("function_call") | Some("eos") | None => {
                StopReason::EndTurn
            }
            Some(other) => StopReason::Other {
                detail: other.to_string(),
            },
        };
        let model = if self.model.is_empty() {
            requested_model.to_string()
        } else {
            self.model
        };
        Ok(ChatResponse {
            message: Message {
                role: Role::Assistant,
                content,
                raw: Some(RawContent {
                    provider: ProviderKind::Openai,
                    model: model.clone(),
                    content: Value::Null,
                }),
            },
            stop,
            usage: self.usage,
            model,
            invalid_tool_inputs: invalid,
            notices: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolSpec;
    use pretty_assertions::assert_eq;

    fn tools() -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "run_command".into(),
            description: "Run a command".into(),
            input_schema: json!({"type": "object", "properties": {"command": {"type": "string"}}}),
            parallel_safe: false,
        }]
    }

    fn provider(base: &str, mode: ToolMode) -> OpenAi {
        let config = ProviderConfig {
            kind: ProviderKind::Openai,
            base_url: base.into(),
            model: "test-model".into(),
            tool_mode: mode,
            ..ProviderConfig::default()
        };
        OpenAi::new(config, Some("key".into())).unwrap()
    }

    fn history() -> Vec<Message> {
        vec![
            Message::user("check sw1"),
            Message {
                role: Role::Assistant,
                content: vec![
                    Block::text("Looking."),
                    Block::ToolUse {
                        id: "call_a".into(),
                        name: "run_command".into(),
                        input: json!({"command": "show clock"}),
                    },
                ],
                raw: None,
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: "call_a".into(),
                    content: "12:00".into(),
                    is_error: false,
                }],
                raw: None,
            },
            Message::system("Read-only mode is on."),
            Message::user("and sw2?"),
        ]
    }

    #[test]
    fn native_request_and_message_mapping() {
        let provider = provider("https://example.test/v1/", ToolMode::Auto);
        let tools = tools();
        let messages = history();
        let body = provider.request_body(
            &ChatRequest {
                system: "sys",
                messages: &messages,
                tools: &tools,
                allow_tools: true,
            },
            false,
        );
        assert_eq!(body["tools"][0]["function"]["name"], "run_command");
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert!(body.get("max_tokens").is_none() && body.get("temperature").is_none());

        let wire = body["messages"].as_array().unwrap();
        assert_eq!(wire[0], json!({"role": "system", "content": "sys"}));
        assert_eq!(
            wire[2]["tool_calls"][0]["function"]["arguments"],
            "{\"command\":\"show clock\"}"
        );
        assert_eq!(
            wire[3],
            json!({"role": "tool", "tool_call_id": "call_a", "content": "12:00"})
        );
        // The operator note and the next user message merge into one user turn.
        assert_eq!(
            wire[4],
            json!({"role": "user", "content": "[Operator note]\nRead-only mode is on.\n\nand sw2?"})
        );
        assert_eq!(wire.len(), 5);
    }

    #[test]
    fn prompted_request_describes_tools_in_the_system_prompt() {
        let provider = provider("http://127.0.0.1:1234/v1", ToolMode::Prompted);
        let tools = tools();
        let messages = history();
        let body = provider.request_body(
            &ChatRequest {
                system: "sys",
                messages: &messages,
                tools: &tools,
                allow_tools: true,
            },
            true,
        );
        assert!(body.get("tools").is_none());
        let wire = body["messages"].as_array().unwrap();
        assert!(
            wire[0]["content"]
                .as_str()
                .unwrap()
                .contains("## run_command")
        );
        assert!(wire[2]["content"].as_str().unwrap().contains("```tool"));
        assert!(
            wire[3]["content"]
                .as_str()
                .unwrap()
                .starts_with("[Tool result from run_command]\n12:00")
        );
        assert_eq!(provider.context_window(), 16_000);
    }

    fn run(chunks: &[Value]) -> (ChatResponse, Vec<StreamEvent>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = StreamState::default();
        for chunk in chunks {
            state.handle(&chunk.to_string(), &tx).unwrap();
        }
        state.handle("[DONE]", &tx).unwrap();
        drop(tx);
        let mut seen = Vec::new();
        while let Ok(event) = rx.try_recv() {
            seen.push(event);
        }
        let tools = tools();
        let response = state
            .finish(
                "test-model",
                &ChatRequest {
                    system: "",
                    messages: &[],
                    tools: &tools,
                    allow_tools: true,
                },
            )
            .unwrap();
        (response, seen)
    }

    #[test]
    fn accumulates_streamed_tool_calls() {
        let (response, seen) = run(&[
            json!({"model": "gpt-x", "choices": [{"delta": {"role": "assistant", "content": "On it."}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "run_command", "arguments": ""}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{\"command\":"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "function": {"arguments": "\"show vlan\"}"}}]}}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 100, "completion_tokens": 20, "prompt_tokens_details": {"cached_tokens": 60}}}),
        ]);
        assert_eq!(response.stop, StopReason::ToolUse);
        assert_eq!(response.model, "gpt-x");
        assert_eq!(
            response.usage,
            Usage {
                input_tokens: 40,
                output_tokens: 20,
                cache_read_tokens: 60,
                cache_write_tokens: 0
            }
        );
        assert_eq!(
            response.message.content,
            vec![
                Block::text("On it."),
                Block::ToolUse {
                    id: "call_1".into(),
                    name: "run_command".into(),
                    input: json!({"command": "show vlan"})
                },
            ]
        );
        assert!(seen.contains(&StreamEvent::ToolStart {
            id: "call_1".into(),
            name: "run_command".into()
        }));
    }

    #[test]
    fn recovers_tool_calls_printed_as_text() {
        let (response, _) = run(&[
            json!({"choices": [{"delta": {"content": "Checking.\n```tool\n{\"name\": \"run_command\", "}}]}),
            json!({"choices": [{"delta": {"content": "\"arguments\": {\"command\": \"show clock\"}}\n```"}, "finish_reason": "stop"}]}),
        ]);
        assert_eq!(response.stop, StopReason::ToolUse);
        assert_eq!(response.message.text(), "Checking.");
        let (_, name, input) = response.message.tool_uses().next().unwrap();
        assert_eq!(
            (name, input),
            ("run_command", &json!({"command": "show clock"}))
        );
    }

    #[test]
    fn handles_object_arguments_reasoning_and_bad_json() {
        let (response, seen) = run(&[
            json!({"choices": [{"delta": {"reasoning_content": "think"}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"function": {"name": "run_command", "arguments": {"command": "show ip route"}}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 1, "id": "c2", "function": {"name": "run_command", "arguments": "{broken"}}]}, "finish_reason": "tool_calls"}]}),
        ]);
        assert!(seen.contains(&StreamEvent::Thinking {
            text: "think".into()
        }));
        let uses: Vec<_> = response.message.tool_uses().collect();
        assert!(uses[0].0.starts_with("call_"));
        assert_eq!(uses[0].2, &json!({"command": "show ip route"}));
        assert_eq!(
            response.invalid_tool_inputs,
            vec![("c2".to_string(), "{broken".to_string())]
        );
    }

    #[test]
    fn maps_finish_reasons_and_stream_errors() {
        let (length, _) =
            run(&[json!({"choices": [{"delta": {"content": "cut"}, "finish_reason": "length"}]})]);
        assert_eq!(length.stop, StopReason::MaxTokens);
        let (filtered, _) = run(&[
            json!({"choices": [{"delta": {"content": "x"}, "finish_reason": "content_filter"}]}),
        ]);
        assert!(matches!(filtered.stop, StopReason::Refusal { .. }));

        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = StreamState::default();
        let err = state
            .handle(
                &json!({"error": {"message": "boom", "type": "server_error"}}).to_string(),
                &tx,
            )
            .unwrap_err();
        assert!(err.to_string().contains("boom"));
    }
}
