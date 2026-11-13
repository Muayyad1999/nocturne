//! Ollama's native `/api/chat`.
//!
//! Ollama also offers an OpenAI-compatible endpoint, but that one cannot set
//! the context window per request, and Ollama's default (a few thousand
//! tokens) is far too small for an agent that carries tool definitions and
//! device output. The native API takes `options.num_ctx`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::{Provider, ProviderConfig, ProviderKind, ToolMode, http, next_call_id, prompted};
use crate::error::AiError;
use crate::sse::LineBuffer;
use crate::types::{
    Block, ChatRequest, ChatResponse, Message, ModelInfo, RawContent, Role, StopReason,
    StreamEvent, Usage,
};

const DEFAULT_BASE: &str = "http://127.0.0.1:11434";
const DEFAULT_NUM_CTX: u32 = 16_384;

pub struct Ollama {
    config: ProviderConfig,
    client: reqwest::Client,
    base: String,
    prompted: AtomicBool,
}

impl Ollama {
    pub fn new(config: ProviderConfig) -> Result<Ollama, AiError> {
        let base = match http::trim_base(&config.base_url) {
            "" => DEFAULT_BASE,
            other => other.strip_suffix("/v1").unwrap_or(other),
        }
        .to_string();
        Ok(Ollama {
            prompted: AtomicBool::new(config.tool_mode == ToolMode::Prompted),
            client: http::client()?,
            config,
            base,
        })
    }

    fn request_body(&self, request: &ChatRequest<'_>, prompted: bool) -> Value {
        let offer_tools = !request.tools.is_empty() && request.allow_tools;
        let mut system = request.system.to_string();
        if prompted && offer_tools {
            system.push_str(&prompted::instructions(request.tools));
        }
        let mut body = Map::new();
        body.insert("model".into(), json!(self.config.model));
        body.insert("stream".into(), json!(true));
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
        let mut options = Map::new();
        options.insert("num_ctx".into(), json!(self.context_window()));
        if let Some(max_tokens) = self.config.max_tokens {
            options.insert("num_predict".into(), json!(max_tokens));
        }
        body.insert("options".into(), Value::Object(options));
        Value::Object(body)
    }

    fn with_headers(&self, mut builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
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
        let url = format!("{}/api/chat", self.base);
        let mut response = http::send(
            || self.with_headers(self.client.post(&url)).json(&body),
            cancel,
        )
        .await?;
        let idle = http::idle_timeout(&self.config);
        let mut state = StreamState::default();
        let mut lines = LineBuffer::default();
        while let Some(chunk) = http::next_chunk(&mut response, idle, cancel).await? {
            for line in lines.push(&chunk) {
                state.handle(&line, events)?;
            }
        }
        if let Some(line) = lines.finish() {
            state.handle(&line, events)?;
        }
        state.finish(&self.config.model, request)
    }
}

#[async_trait]
impl Provider for Ollama {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Ollama
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    fn context_window(&self) -> u32 {
        self.config.context_window.unwrap_or(DEFAULT_NUM_CTX)
    }

    async fn chat(
        &self,
        request: ChatRequest<'_>,
        events: &UnboundedSender<StreamEvent>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse, AiError> {
        let prompted = self.prompted.load(Ordering::Relaxed);
        match self.attempt(&request, prompted, events, cancel).await {
            Err(err)
                if !prompted
                    && self.config.tool_mode == ToolMode::Auto
                    && err.is_tools_unsupported() =>
            {
                tracing::info!(model = %self.config.model, "model has no native tool calling; switching to prompted tools");
                self.prompted.store(true, Ordering::Relaxed);
                self.attempt(&request, true, events, cancel).await
            }
            other => other,
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, AiError> {
        let cancel = CancellationToken::new();
        let url = format!("{}/api/tags", self.base);
        let response = http::send(|| self.with_headers(self.client.get(&url)), &cancel).await?;
        let body: Value = response.json().await?;
        let models = body["models"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item["name"]
                            .as_str()
                            .or_else(|| item["model"].as_str())?
                            .to_string();
                        let size = item["details"]["parameter_size"]
                            .as_str()
                            .unwrap_or_default();
                        let quant = item["details"]["quantization_level"]
                            .as_str()
                            .unwrap_or_default();
                        let detail = [size, quant]
                            .iter()
                            .filter(|part| !part.is_empty())
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" ");
                        Some(ModelInfo {
                            name: if detail.is_empty() {
                                id.clone()
                            } else {
                                format!("{id} ({detail})")
                            },
                            id,
                            context_window: None,
                            max_output: None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }
}

pub(crate) fn wire_messages(system: &str, messages: &[Message], prompted: bool) -> Vec<Value> {
    let mut wire = vec![json!({"role": "system", "content": system})];
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
                        let name = names.get(tool_use_id.as_str()).copied().unwrap_or("tool");
                        if prompted {
                            push_user(&mut wire, prompted::render_result(name, content, *is_error));
                        } else {
                            let content = if *is_error {
                                format!("ERROR: {content}")
                            } else {
                                content.clone()
                            };
                            wire.push(
                                json!({"role": "tool", "tool_name": name, "content": content}),
                            );
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
                        tool_calls.push(json!({"function": {"name": name, "arguments": input}}));
                    }
                }
                let mut entry = Map::new();
                entry.insert("role".into(), json!("assistant"));
                entry.insert("content".into(), json!(text));
                if !tool_calls.is_empty() {
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
pub(crate) struct StreamState {
    text: String,
    thinking: String,
    calls: Vec<(String, String, Value)>,
    done_reason: Option<String>,
    done: bool,
    usage: Usage,
    model: String,
}

impl StreamState {
    pub(crate) fn handle(
        &mut self,
        line: &str,
        events: &UnboundedSender<StreamEvent>,
    ) -> Result<(), AiError> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(());
        }
        let chunk: Value = serde_json::from_str(line)
            .map_err(|err| AiError::Protocol(format!("unparsable stream line: {err}")))?;
        if let Some(error) = chunk.get("error").filter(|error| !error.is_null()) {
            return Err(http::stream_error(error));
        }
        if let Some(model) = chunk["model"].as_str() {
            self.model = model.to_string();
        }
        let message = &chunk["message"];
        if let Some(text) = message["thinking"].as_str().filter(|text| !text.is_empty()) {
            self.thinking.push_str(text);
            let _ = events.send(StreamEvent::Thinking {
                text: text.to_string(),
            });
        }
        if let Some(text) = message["content"].as_str().filter(|text| !text.is_empty()) {
            self.text.push_str(text);
            let _ = events.send(StreamEvent::Text {
                text: text.to_string(),
            });
        }
        if let Some(tool_calls) = message["tool_calls"].as_array() {
            for call in tool_calls {
                let function = &call["function"];
                let Some(name) = function["name"].as_str().filter(|name| !name.is_empty()) else {
                    continue;
                };
                let arguments = match &function["arguments"] {
                    Value::String(encoded) => {
                        serde_json::from_str(encoded).unwrap_or_else(|_| json!({}))
                    }
                    Value::Object(_) => function["arguments"].clone(),
                    _ => json!({}),
                };
                let id = next_call_id();
                let _ = events.send(StreamEvent::ToolStart {
                    id: id.clone(),
                    name: name.to_string(),
                });
                self.calls.push((id, name.to_string(), arguments));
            }
        }
        if chunk["done"].as_bool() == Some(true) {
            self.done = true;
            self.done_reason = chunk["done_reason"].as_str().map(str::to_string);
            self.usage = Usage {
                input_tokens: chunk["prompt_eval_count"].as_u64().unwrap_or(0),
                output_tokens: chunk["eval_count"].as_u64().unwrap_or(0),
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            };
        }
        Ok(())
    }

    pub(crate) fn finish(
        self,
        requested_model: &str,
        request: &ChatRequest<'_>,
    ) -> Result<ChatResponse, AiError> {
        if !self.done {
            return Err(AiError::Network(
                "the response stream ended before the message was complete".into(),
            ));
        }
        let mut content = Vec::new();
        if !self.thinking.trim().is_empty() {
            content.push(Block::Thinking {
                text: self.thinking,
            });
        }
        let mut text = self.text;
        let mut tool_blocks: Vec<Block> = self
            .calls
            .into_iter()
            .map(|(id, name, input)| Block::ToolUse { id, name, input })
            .collect();
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
        let stop = if !tool_blocks.is_empty() {
            StopReason::ToolUse
        } else if self.done_reason.as_deref() == Some("length") {
            StopReason::MaxTokens
        } else {
            StopReason::EndTurn
        };
        content.extend(tool_blocks);
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
                    provider: ProviderKind::Ollama,
                    model: model.clone(),
                    content: Value::Null,
                }),
            },
            stop,
            usage: self.usage,
            model,
            invalid_tool_inputs: Vec::new(),
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
            description: "Run".into(),
            input_schema: json!({"type": "object"}),
            parallel_safe: false,
        }]
    }

    fn provider() -> Ollama {
        Ollama::new(ProviderConfig {
            kind: ProviderKind::Ollama,
            model: "qwen3:8b".into(),
            ..ProviderConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn request_sets_context_window_and_native_tools() {
        let provider = provider();
        let tools = tools();
        let messages = vec![
            Message::user("hi"),
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "c1".into(),
                    name: "run_command".into(),
                    input: json!({"command": "show clock"}),
                }],
                raw: None,
            },
            Message {
                role: Role::User,
                content: vec![Block::ToolResult {
                    tool_use_id: "c1".into(),
                    content: "12:00".into(),
                    is_error: false,
                }],
                raw: None,
            },
        ];
        let body = provider.request_body(
            &ChatRequest {
                system: "sys",
                messages: &messages,
                tools: &tools,
                allow_tools: true,
            },
            false,
        );
        assert_eq!(body["options"]["num_ctx"], 16_384);
        assert_eq!(body["tools"][0]["function"]["name"], "run_command");
        let wire = body["messages"].as_array().unwrap();
        assert_eq!(
            wire[2]["tool_calls"][0]["function"]["arguments"],
            json!({"command": "show clock"})
        );
        assert_eq!(
            wire[3],
            json!({"role": "tool", "tool_name": "run_command", "content": "12:00"})
        );
        assert_eq!(provider.base, "http://127.0.0.1:11434");
    }

    #[test]
    fn parses_ndjson_stream_with_thinking_and_tool_calls() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = StreamState::default();
        for line in [
            json!({"model": "qwen3:8b", "message": {"role": "assistant", "content": "", "thinking": "plan"}, "done": false}),
            json!({"message": {"role": "assistant", "content": "Checking."}, "done": false}),
            json!({"message": {"role": "assistant", "content": "", "tool_calls": [{"function": {"name": "run_command", "arguments": {"command": "show vlan"}}}]}, "done": false}),
            json!({"message": {"role": "assistant", "content": ""}, "done": true, "done_reason": "stop", "prompt_eval_count": 321, "eval_count": 45}),
        ] {
            state.handle(&line.to_string(), &tx).unwrap();
        }
        drop(tx);
        let tools = tools();
        let response = state
            .finish(
                "qwen3:8b",
                &ChatRequest {
                    system: "",
                    messages: &[],
                    tools: &tools,
                    allow_tools: true,
                },
            )
            .unwrap();
        assert_eq!(response.stop, StopReason::ToolUse);
        assert_eq!(response.usage.input_tokens, 321);
        assert_eq!(response.message.text(), "Checking.");
        let (_, name, input) = response.message.tool_uses().next().unwrap();
        assert_eq!(
            (name, input),
            ("run_command", &json!({"command": "show vlan"}))
        );
        assert!(matches!(rx.try_recv(), Ok(StreamEvent::Thinking { .. })));
    }

    #[test]
    fn stream_errors_and_truncation_are_reported() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = StreamState::default();
        let err = state
            .handle(r#"{"error":"model requires more system memory"}"#, &tx)
            .unwrap_err();
        assert!(err.to_string().contains("more system memory"));
        let tools = tools();
        let cut = StreamState::default().finish(
            "m",
            &ChatRequest {
                system: "",
                messages: &[],
                tools: &tools,
                allow_tools: true,
            },
        );
        assert!(cut.is_err());
    }
}
