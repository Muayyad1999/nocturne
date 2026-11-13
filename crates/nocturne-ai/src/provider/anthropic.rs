//! Anthropic Messages API (raw HTTP — there is no official Rust SDK).
//!
//! Notes on the choices made here, all taken from the current API reference:
//!
//! * API version `2026-10-21`: no sampling parameters are sent.
//! * Thinking is adaptive on every model that supports it, with summarised
//!   display so the UI can show the reasoning; depth is set with
//!   `output_config.effort`.
//! * Assistant turns are replayed **verbatim** (`Message::raw`). Thinking
//!   blocks are signed against the conversation prefix, so history is
//!   append-only: the system prompt and tool list never change mid-conversation
//!   and operator notes are appended as `role: "system"` messages.
//! * Tool inputs stream eagerly and are parsed strictly here; an unparsable
//!   input is reported back to the model instead of being executed.
//! * `stop_reason: "refusal"` is surfaced as [`StopReason::Refusal`]; where the
//!   model supports it the request opts in to server-side fallbacks so a
//!   classifier false positive is retried on a permitted model in the same call.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::http;
use super::{Effort, Provider, ProviderConfig, ProviderKind};
use crate::error::AiError;
use crate::sse::SseParser;
use crate::types::{
    Block, ChatRequest, ChatResponse, Message, ModelInfo, RawContent, Role, StopReason,
    StreamEvent, Usage,
};

const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const DEFAULT_BASE: &str = "https://api.anthropic.com";

/// What a given Claude model accepts, derived from its id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Caps {
    /// `thinking: {type: "adaptive"}` and `output_config.effort`.
    pub adaptive: bool,
    /// `thinking.display` is accepted.
    pub display: bool,
    pub xhigh: bool,
    /// `role: "system"` messages inside `messages`.
    pub mid_system: bool,
    /// Server-side refusal fallbacks.
    pub fallbacks: bool,
    pub context: u32,
    pub max_output: u32,
}

pub(crate) fn caps_for(model: &str) -> Caps {
    let modern = Caps {
        adaptive: true,
        display: true,
        xhigh: true,
        mid_system: true,
        fallbacks: false,
        context: 1_000_000,
        max_output: 64_000,
    };
    let legacy = |major: u32| Caps {
        adaptive: false,
        display: false,
        xhigh: false,
        mid_system: false,
        fallbacks: false,
        context: 200_000,
        max_output: if major >= 4 { 32_000 } else { 4_096 },
    };

    let id = model.to_ascii_lowercase();
    let Some(start) = id.find("claude-") else {
        return modern;
    };
    let mut parts = id[start + "claude-".len()..].split(['-', '@']);
    let family = parts.next().unwrap_or_default();
    let numbers: Vec<u32> = parts
        .map_while(|part| {
            if part.len() <= 2 {
                part.parse::<u32>().ok()
            } else {
                None
            }
        })
        .collect();
    let major = numbers.first().copied().unwrap_or(0);
    let version = major * 10 + numbers.get(1).copied().unwrap_or(0).min(9);

    match family {
        "fable" | "mythos" => Caps {
            fallbacks: true,
            ..modern
        },
        "opus" if version >= 46 => Caps {
            display: version >= 47,
            xhigh: version >= 47,
            mid_system: version >= 48,
            fallbacks: version >= 50,
            ..modern
        },
        "sonnet" if version >= 46 => Caps {
            display: version >= 50,
            xhigh: version >= 50,
            mid_system: version >= 55,
            fallbacks: version >= 55,
            ..modern
        },
        "haiku" if version >= 55 => modern,
        "opus" | "sonnet" | "haiku" => legacy(major),
        // "claude-3-5-sonnet-…" style ids put the version first.
        other if other.parse::<u32>().is_ok() => legacy(other.parse().unwrap_or(3)),
        _ => modern,
    }
}

pub struct Anthropic {
    config: ProviderConfig,
    api_key: String,
    client: reqwest::Client,
    base: String,
    caps: Caps,
    /// Talking to api.anthropic.com itself rather than a gateway.
    first_party: bool,
}

impl Anthropic {
    pub fn new(config: ProviderConfig, api_key: Option<String>) -> Result<Anthropic, AiError> {
        let api_key = api_key
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty())
            .ok_or_else(|| AiError::Config("an Anthropic API key is required".into()))?;
        let base = match http::trim_base(&config.base_url) {
            "" => DEFAULT_BASE,
            other => other.strip_suffix("/v1").unwrap_or(other),
        }
        .to_string();
        let first_party = base.contains("api.anthropic.com");
        let caps = caps_for(&config.model);
        Ok(Anthropic {
            client: http::client()?,
            config,
            api_key,
            base,
            caps,
            first_party,
        })
    }

    fn effort(&self) -> &'static str {
        match self.config.effort.unwrap_or(Effort::High) {
            Effort::Xhigh if !self.caps.xhigh => "high",
            effort => effort.as_str(),
        }
    }

    fn uses_fallbacks(&self) -> bool {
        self.config.refusal_fallback && self.caps.fallbacks && self.first_party
    }

    fn request_body(&self, request: &ChatRequest<'_>) -> Value {
        let mut body = Map::new();
        body.insert("model".into(), json!(self.config.model));
        body.insert(
            "max_tokens".into(),
            json!(self.config.max_tokens.unwrap_or(self.caps.max_output)),
        );
        body.insert("stream".into(), json!(true));
        body.insert(
            "system".into(),
            json!([{"type": "text", "text": request.system, "cache_control": {"type": "ephemeral"}}]),
        );
        if !request.tools.is_empty() {
            let tools: Vec<Value> = request
                .tools
                .iter()
                .map(|tool| {
                    let mut entry = json!({
                        "name": tool.name,
                        "description": tool.description,
                        "input_schema": tool.input_schema,
                    });
                    // Gateways in front of the API may reject the field; only the real API gets it.
                    if self.first_party {
                        entry["eager_input_streaming"] = json!(true);
                    }
                    entry
                })
                .collect();
            body.insert("tools".into(), Value::Array(tools));
            if !request.allow_tools {
                body.insert("tool_choice".into(), json!({"type": "none"}));
            }
        }
        body.insert(
            "messages".into(),
            Value::Array(wire_messages(request.messages, self.caps.mid_system)),
        );
        if self.caps.adaptive {
            let mut thinking = json!({"type": "adaptive"});
            if self.caps.display {
                thinking["display"] = json!("summarized");
            }
            body.insert("thinking".into(), thinking);
            body.insert("output_config".into(), json!({"effort": self.effort()}));
        }
        if self.first_party {
            // Automatic breakpoint on the last cacheable block, so each turn re-reads the previous prefix.
            body.insert("cache_control".into(), json!({"type": "ephemeral"}));
        }
        if self.uses_fallbacks() {
            body.insert("fallbacks".into(), json!("default"));
        }
        Value::Object(body)
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut builder = self
            .client
            .post(format!("{}/v1/messages", self.base))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("accept", "text/event-stream");
        if self.uses_fallbacks() {
            builder = builder.header("anthropic-beta", FALLBACK_BETA);
        }
        for (name, value) in &self.config.extra_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        builder.json(body)
    }
}

#[async_trait]
impl Provider for Anthropic {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
    }

    fn model(&self) -> &str {
        &self.config.model
    }

    fn context_window(&self) -> u32 {
        self.config.context_window.unwrap_or(self.caps.context)
    }

    async fn chat(
        &self,
        request: ChatRequest<'_>,
        events: &UnboundedSender<StreamEvent>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse, AiError> {
        let body = self.request_body(&request);
        let idle = http::idle_timeout(&self.config);
        let mut restarted = false;
        loop {
            let mut response = http::send(|| self.post(&body), cancel).await?;
            let mut state = StreamState::default();
            let mut parser = SseParser::default();
            let outcome = async {
                while let Some(chunk) = http::next_chunk(&mut response, idle, cancel).await? {
                    for event in parser.push(&chunk) {
                        state.handle(&event.data, events)?;
                    }
                }
                if let Some(event) = parser.finish() {
                    state.handle(&event.data, events)?;
                }
                Ok::<(), AiError>(())
            }
            .await;
            match outcome {
                Ok(()) => return state.finish(&self.config.model),
                // Nothing reached the user yet, so starting over is invisible.
                Err(err) if !state.started && err.is_retryable() && !restarted => {
                    tracing::debug!(%err, "restarting model stream before first content");
                    restarted = true;
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, AiError> {
        let cancel = CancellationToken::new();
        let response = http::send(
            || {
                self.client
                    .get(format!("{}/v1/models?limit=1000", self.base))
                    .header("x-api-key", &self.api_key)
                    .header("anthropic-version", API_VERSION)
            },
            &cancel,
        )
        .await?;
        let body: Value = response.json().await?;
        let models = body["data"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let id = item["id"].as_str()?.to_string();
                        Some(ModelInfo {
                            name: item["display_name"].as_str().unwrap_or(&id).to_string(),
                            context_window: item["max_input_tokens"].as_u64().map(|n| n as u32),
                            max_output: item["max_tokens"].as_u64().map(|n| n as u32),
                            id,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(models)
    }
}

/// Convert neutral history into the `messages` array.
///
/// The conversion is a pure function of the history, so the same history
/// always produces byte-identical output — which keeps both the prompt cache
/// and signed thinking blocks valid from one request to the next.
pub(crate) fn wire_messages(messages: &[Message], mid_system: bool) -> Vec<Value> {
    let mut wire: Vec<Value> = Vec::with_capacity(messages.len());
    for message in messages {
        match message.role {
            Role::System => {
                let text = message.text();
                if text.is_empty() {
                    continue;
                }
                if mid_system && wire.last().is_some_and(|last| last["role"] == "user") {
                    wire.push(json!({"role": "system", "content": text}));
                } else {
                    // Models without mid-conversation system messages get the note as user text.
                    push_content(
                        &mut wire,
                        "user",
                        vec![json!({"type": "text", "text": format!("[Operator note]\n{text}")})],
                    );
                }
            }
            Role::User => {
                // Tool results must lead the turn that answers a tool call.
                let mut content: Vec<Value> = Vec::new();
                for block in &message.content {
                    if let Block::ToolResult {
                        tool_use_id,
                        content: text,
                        is_error,
                    } = block
                    {
                        let mut entry = json!({
                            "type": "tool_result",
                            "tool_use_id": tool_use_id,
                            "content": if text.is_empty() { "(no output)" } else { text.as_str() },
                        });
                        if *is_error {
                            entry["is_error"] = json!(true);
                        }
                        content.push(entry);
                    }
                }
                for block in &message.content {
                    if let Block::Text { text } = block
                        && !text.trim().is_empty()
                    {
                        content.push(json!({"type": "text", "text": text}));
                    }
                }
                if !content.is_empty() {
                    push_content(&mut wire, "user", content);
                }
            }
            Role::Assistant => {
                let content = match &message.raw {
                    Some(RawContent {
                        provider: ProviderKind::Anthropic,
                        content: Value::Array(blocks),
                        ..
                    }) if !blocks.is_empty() => blocks.clone(),
                    _ => neutral_assistant_content(message),
                };
                push_content(&mut wire, "assistant", content);
            }
        }
    }
    wire
}

fn neutral_assistant_content(message: &Message) -> Vec<Value> {
    let mut content: Vec<Value> = Vec::new();
    for block in &message.content {
        match block {
            Block::Text { text } if !text.trim().is_empty() => {
                content.push(json!({"type": "text", "text": text}))
            }
            Block::ToolUse { id, name, input } => {
                let input = if input.is_object() {
                    input.clone()
                } else {
                    json!({})
                };
                content.push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
            }
            _ => {}
        }
    }
    if content.is_empty() {
        content.push(json!({"type": "text", "text": "(no reply)"}));
    }
    content
}

/// Append content, merging into the previous message when the role repeats.
fn push_content(wire: &mut Vec<Value>, role: &str, content: Vec<Value>) {
    if let Some(last) = wire.last_mut()
        && last["role"] == role
        && let Some(existing) = last["content"].as_array_mut()
    {
        existing.extend(content);
        return;
    }
    wire.push(json!({"role": role, "content": content}));
}

/// Accumulates one streamed response.
#[derive(Default)]
pub(crate) struct StreamState {
    blocks: Vec<Value>,
    partial_inputs: HashMap<usize, String>,
    invalid: Vec<(String, String)>,
    usage: Usage,
    model: String,
    stop_reason: Option<String>,
    stop_details: Option<Value>,
    /// A content block has begun: restarting now would duplicate output.
    pub(crate) started: bool,
}

impl StreamState {
    pub(crate) fn handle(
        &mut self,
        data: &str,
        events: &UnboundedSender<StreamEvent>,
    ) -> Result<(), AiError> {
        let data: Value = serde_json::from_str(data)
            .map_err(|err| AiError::Protocol(format!("unparsable stream event: {err}")))?;
        match data["type"].as_str().unwrap_or_default() {
            "message_start" => {
                let message = &data["message"];
                if let Some(model) = message["model"].as_str() {
                    self.model = model.to_string();
                }
                merge_usage(&mut self.usage, &message["usage"]);
            }
            "content_block_start" => {
                self.started = true;
                let index = data["index"].as_u64().unwrap_or(0) as usize;
                while self.blocks.len() <= index {
                    self.blocks.push(Value::Null);
                }
                let block = data["content_block"].clone();
                match block["type"].as_str() {
                    Some("tool_use") => {
                        let _ = events.send(StreamEvent::ToolStart {
                            id: block["id"].as_str().unwrap_or_default().to_string(),
                            name: block["name"].as_str().unwrap_or_default().to_string(),
                        });
                    }
                    Some("text") => {
                        if let Some(text) = block["text"].as_str().filter(|text| !text.is_empty()) {
                            let _ = events.send(StreamEvent::Text {
                                text: text.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
                self.blocks[index] = block;
            }
            "content_block_delta" => {
                let index = data["index"].as_u64().unwrap_or(0) as usize;
                let delta = &data["delta"];
                let Some(block) = self.blocks.get_mut(index).and_then(Value::as_object_mut) else {
                    return Ok(());
                };
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        let text = delta["text"].as_str().unwrap_or_default();
                        append(block, "text", text);
                        let _ = events.send(StreamEvent::Text {
                            text: text.to_string(),
                        });
                    }
                    Some("thinking_delta") => {
                        let text = delta["thinking"].as_str().unwrap_or_default();
                        append(block, "thinking", text);
                        let _ = events.send(StreamEvent::Thinking {
                            text: text.to_string(),
                        });
                    }
                    Some("signature_delta") => append(
                        block,
                        "signature",
                        delta["signature"].as_str().unwrap_or_default(),
                    ),
                    Some("input_json_delta") => {
                        let partial = delta["partial_json"].as_str().unwrap_or_default();
                        self.partial_inputs
                            .entry(index)
                            .or_default()
                            .push_str(partial);
                        let _ = events.send(StreamEvent::ToolInput {
                            id: block
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            partial: partial.to_string(),
                        });
                    }
                    Some("citations_delta") => {
                        let citations = block.entry("citations").or_insert_with(|| json!([]));
                        if let Some(list) = citations.as_array_mut() {
                            list.push(delta["citation"].clone());
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = data["index"].as_u64().unwrap_or(0) as usize;
                self.finish_tool_input(index);
            }
            "message_delta" => {
                if let Some(reason) = data["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(reason.to_string());
                }
                if let Some(details) = data["delta"]
                    .get("stop_details")
                    .filter(|details| !details.is_null())
                {
                    self.stop_details = Some(details.clone());
                }
                merge_usage(&mut self.usage, &data["usage"]);
            }
            "error" => return Err(http::stream_error(&data["error"])),
            // "ping", "message_stop" and event types added later need no action.
            _ => {}
        }
        Ok(())
    }

    /// Parse the accumulated argument JSON strictly. Eager streaming means the
    /// API has not validated it, so a bad input is recorded rather than run.
    fn finish_tool_input(&mut self, index: usize) {
        let Some(block) = self.blocks.get_mut(index).and_then(Value::as_object_mut) else {
            return;
        };
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            return;
        }
        let raw = self.partial_inputs.remove(&index).unwrap_or_default();
        if raw.trim().is_empty() {
            if !block.get("input").is_some_and(Value::is_object) {
                block.insert("input".into(), json!({}));
            }
            return;
        }
        match serde_json::from_str::<Value>(&raw) {
            Ok(parsed @ Value::Object(_)) => {
                block.insert("input".into(), parsed);
            }
            _ => {
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                block.insert("input".into(), json!({}));
                self.invalid.push((id, raw));
            }
        }
    }

    pub(crate) fn finish(mut self, requested_model: &str) -> Result<ChatResponse, AiError> {
        // A stream cut off mid-block leaves partial tool input behind.
        let open: Vec<usize> = self.partial_inputs.keys().copied().collect();
        for index in open {
            self.finish_tool_input(index);
        }
        let Some(stop_reason) = self.stop_reason.take() else {
            return Err(AiError::Network(
                "the response stream ended before the message was complete".into(),
            ));
        };

        let boundary = self
            .blocks
            .iter()
            .rposition(|block| block["type"] == "fallback");
        let mut notices = Vec::new();
        let mut echo: Vec<Value> = Vec::new();
        let mut content: Vec<Block> = Vec::new();

        for (index, block) in self.blocks.into_iter().enumerate() {
            let kind = block["type"].as_str().unwrap_or_default().to_string();
            // After a mid-output fallback, only text and the fallback markers
            // from the declined attempt may be echoed back.
            let declined_attempt = boundary.is_some_and(|boundary| index < boundary);
            match kind.as_str() {
                "text" => {
                    let text = block["text"].as_str().unwrap_or_default().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    content.push(Block::Text { text });
                    echo.push(block);
                }
                "thinking" => {
                    if declined_attempt {
                        continue;
                    }
                    if let Some(text) = block["thinking"]
                        .as_str()
                        .filter(|text| !text.trim().is_empty())
                    {
                        content.push(Block::Thinking {
                            text: text.to_string(),
                        });
                    }
                    echo.push(block);
                }
                "tool_use" => {
                    if declined_attempt {
                        continue;
                    }
                    content.push(Block::ToolUse {
                        id: block["id"].as_str().unwrap_or_default().to_string(),
                        name: block["name"].as_str().unwrap_or_default().to_string(),
                        input: block["input"].clone(),
                    });
                    echo.push(block);
                }
                "fallback" => {
                    let from = block["from"]["model"]
                        .as_str()
                        .unwrap_or("the requested model");
                    let to = block["to"]["model"].as_str().unwrap_or("a fallback model");
                    notices.push(format!(
                        "{from} declined this request; {to} answered instead."
                    ));
                    echo.push(block);
                }
                "" => {}
                _ => {
                    if !declined_attempt {
                        echo.push(block);
                    }
                }
            }
        }

        let stop = match stop_reason.as_str() {
            "end_turn" | "stop_sequence" => StopReason::EndTurn,
            "tool_use" => StopReason::ToolUse,
            "max_tokens" | "model_context_window_exceeded" => StopReason::MaxTokens,
            "pause_turn" => StopReason::Pause,
            "refusal" => StopReason::Refusal {
                category: self
                    .stop_details
                    .as_ref()
                    .and_then(|details| details["category"].as_str())
                    .map(str::to_string),
                explanation: self
                    .stop_details
                    .as_ref()
                    .and_then(|details| details["explanation"].as_str())
                    .map(str::to_string),
            },
            other => StopReason::Other {
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
                    provider: ProviderKind::Anthropic,
                    model: model.clone(),
                    content: Value::Array(echo),
                }),
            },
            stop,
            usage: self.usage,
            model,
            invalid_tool_inputs: self.invalid,
            notices,
        })
    }
}

fn append(block: &mut Map<String, Value>, key: &str, extra: &str) {
    match block.get_mut(key) {
        Some(Value::String(existing)) => existing.push_str(extra),
        _ => {
            block.insert(key.to_string(), Value::String(extra.to_string()));
        }
    }
}

fn merge_usage(usage: &mut Usage, value: &Value) {
    if let Some(n) = value["input_tokens"].as_u64() {
        usage.input_tokens = n;
    }
    if let Some(n) = value["output_tokens"].as_u64() {
        usage.output_tokens = n;
    }
    if let Some(n) = value["cache_read_input_tokens"].as_u64() {
        usage.cache_read_tokens = n;
    }
    if let Some(n) = value["cache_creation_input_tokens"].as_u64() {
        usage.cache_write_tokens = n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolSpec;
    use pretty_assertions::assert_eq;

    fn provider(model: &str) -> Anthropic {
        let config = ProviderConfig {
            model: model.into(),
            ..ProviderConfig::default()
        };
        Anthropic::new(config, Some("sk-test".into())).unwrap()
    }

    fn tool() -> ToolSpec {
        ToolSpec {
            name: "run_command".into(),
            description: "Run a command".into(),
            input_schema: json!({"type": "object", "properties": {"command": {"type": "string"}}, "required": ["command"]}),
            parallel_safe: false,
        }
    }

    #[test]
    fn capabilities_follow_the_model_family() {
        let opus55 = caps_for("claude-opus-5-5");
        assert!(opus55.adaptive && opus55.xhigh && opus55.mid_system && opus55.fallbacks);
        let fable = caps_for("claude-fable-5-2");
        assert!(fable.adaptive && fable.fallbacks && fable.mid_system);
        let sonnet5 = caps_for("claude-sonnet-5");
        assert!(sonnet5.adaptive && sonnet5.xhigh && !sonnet5.mid_system && !sonnet5.fallbacks);
        let sonnet46 = caps_for("claude-sonnet-4-6");
        assert!(sonnet46.adaptive && !sonnet46.xhigh && !sonnet46.display);
        let haiku55 = caps_for("claude-haiku-5-5");
        assert!(haiku55.adaptive && haiku55.mid_system && !haiku55.fallbacks);
        let haiku45 = caps_for("claude-haiku-4-5");
        assert!(!haiku45.adaptive && haiku45.context == 200_000);
        let dated = caps_for("claude-opus-4-5-20251101");
        assert!(!dated.adaptive);
        let old_style = caps_for("claude-3-5-sonnet-20241022");
        assert!(!old_style.adaptive && old_style.max_output == 4_096);
        let bedrock = caps_for("anthropic.claude-opus-5-5");
        assert_eq!(bedrock, opus55);
        assert!(caps_for("claude-opus-6").adaptive);
    }

    #[test]
    fn request_has_current_api_shape() {
        let provider = provider("claude-opus-5-5");
        let messages = vec![Message::user("hello")];
        let tools = vec![tool()];
        let body = provider.request_body(&ChatRequest {
            system: "be brief",
            messages: &messages,
            tools: &tools,
            allow_tools: true,
        });

        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["stream"], true);
        assert_eq!(
            body["thinking"],
            json!({"type": "adaptive", "display": "summarized"})
        );
        assert_eq!(body["output_config"], json!({"effort": "high"}));
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(
            body["system"][0]["cache_control"],
            json!({"type": "ephemeral"})
        );
        assert_eq!(body["tools"][0]["eager_input_streaming"], true);
        assert!(body.get("tool_choice").is_none());
        for removed in ["temperature", "top_p", "top_k"] {
            assert!(
                body.get(removed).is_none(),
                "{removed} must not be sent on API version {API_VERSION}"
            );
        }
    }

    #[test]
    fn older_models_and_gateways_get_a_conservative_request() {
        let haiku = provider("claude-haiku-4-5");
        let messages = vec![Message::user("hello")];
        let body = haiku.request_body(&ChatRequest {
            system: "s",
            messages: &messages,
            tools: &[],
            allow_tools: true,
        });
        assert!(
            body.get("thinking").is_none()
                && body.get("output_config").is_none()
                && body.get("fallbacks").is_none()
        );

        let config = ProviderConfig {
            model: "claude-opus-5-5".into(),
            base_url: "https://llm-gateway.example.net/v1/".into(),
            effort: Some(Effort::Low),
            ..ProviderConfig::default()
        };
        let gateway = Anthropic::new(config, Some("k".into())).unwrap();
        assert_eq!(gateway.base, "https://llm-gateway.example.net");
        let tools = vec![tool()];
        let body = gateway.request_body(&ChatRequest {
            system: "s",
            messages: &messages,
            tools: &tools,
            allow_tools: false,
        });
        assert!(body["tools"][0].get("eager_input_streaming").is_none());
        assert!(body.get("fallbacks").is_none() && body.get("cache_control").is_none());
        assert_eq!(body["tool_choice"], json!({"type": "none"}));
        assert_eq!(body["output_config"]["effort"], "low");
    }

    #[test]
    fn xhigh_effort_degrades_where_unsupported() {
        let config = ProviderConfig {
            model: "claude-sonnet-4-6".into(),
            effort: Some(Effort::Xhigh),
            ..ProviderConfig::default()
        };
        assert_eq!(
            Anthropic::new(config, Some("k".into())).unwrap().effort(),
            "high"
        );
    }

    #[test]
    fn wire_messages_replay_raw_content_and_lead_with_tool_results() {
        let raw = json!([
            {"type": "thinking", "thinking": "", "signature": "sig-abc"},
            {"type": "tool_use", "id": "toolu_1", "name": "run_command", "input": {"command": "show clock"}}
        ]);
        let history = vec![
            Message::user("what time is it on sw1?"),
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "toolu_1".into(),
                    name: "run_command".into(),
                    input: json!({"command": "show clock"}),
                }],
                raw: Some(RawContent {
                    provider: ProviderKind::Anthropic,
                    model: "claude-opus-5-5".into(),
                    content: raw.clone(),
                }),
            },
            Message {
                role: Role::User,
                content: vec![
                    Block::text("also check sw2"),
                    Block::ToolResult {
                        tool_use_id: "toolu_1".into(),
                        content: "12:00:00".into(),
                        is_error: false,
                    },
                ],
                raw: None,
            },
            Message::system("The operator switched to read-only mode."),
        ];

        let wire = wire_messages(&history, true);
        assert_eq!(wire.len(), 4);
        assert_eq!(
            wire[1]["content"], raw,
            "assistant turn must be echoed verbatim, empty thinking included"
        );
        assert_eq!(wire[2]["content"][0]["type"], "tool_result");
        assert_eq!(wire[2]["content"][1]["text"], "also check sw2");
        assert_eq!(
            wire[3],
            json!({"role": "system", "content": "The operator switched to read-only mode."})
        );

        let folded = wire_messages(&history, false);
        assert_eq!(folded.len(), 3);
        assert_eq!(
            folded[2]["content"][2]["text"],
            "[Operator note]\nThe operator switched to read-only mode."
        );
    }

    #[test]
    fn foreign_assistant_turns_are_rebuilt_without_thinking() {
        let history = vec![
            Message::user("hi"),
            Message {
                role: Role::Assistant,
                content: vec![Block::Thinking { text: "hmm".into() }, Block::text("hello")],
                raw: Some(RawContent {
                    provider: ProviderKind::Openai,
                    model: "x".into(),
                    content: json!({"anything": 1}),
                }),
            },
        ];
        let wire = wire_messages(&history, true);
        assert_eq!(
            wire[1]["content"],
            json!([{"type": "text", "text": "hello"}])
        );
    }

    fn feed(events: &[Value]) -> (StreamState, Vec<StreamEvent>) {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = StreamState::default();
        for event in events {
            state.handle(&event.to_string(), &tx).unwrap();
        }
        drop(tx);
        let mut seen = Vec::new();
        while let Ok(event) = rx.try_recv() {
            seen.push(event);
        }
        (state, seen)
    }

    #[test]
    fn accumulates_a_streamed_tool_call_turn() {
        let (state, seen) = feed(&[
            json!({"type": "message_start", "message": {"id": "msg_1", "model": "claude-opus-5-5", "usage": {"input_tokens": 120, "cache_read_input_tokens": 900, "output_tokens": 1}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "Check the VLAN table."}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "c2lnbmF0dXJl"}}),
            json!({"type": "content_block_stop", "index": 0}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Let me look."}}),
            json!({"type": "content_block_stop", "index": 1}),
            json!({"type": "ping"}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_9", "name": "run_command", "input": {}}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"command\": \"show "}}),
            json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "vlan brief\"}"}}),
            json!({"type": "content_block_stop", "index": 2}),
            json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 57}}),
            json!({"type": "message_stop"}),
        ]);
        assert!(seen.contains(&StreamEvent::Thinking {
            text: "Check the VLAN table.".into()
        }));
        assert!(seen.contains(&StreamEvent::ToolStart {
            id: "toolu_9".into(),
            name: "run_command".into()
        }));

        let response = state.finish("claude-opus-5-5").unwrap();
        assert_eq!(response.stop, StopReason::ToolUse);
        assert_eq!(
            response.usage,
            Usage {
                input_tokens: 120,
                output_tokens: 57,
                cache_read_tokens: 900,
                cache_write_tokens: 0
            }
        );
        assert_eq!(
            response.message.content,
            vec![
                Block::Thinking {
                    text: "Check the VLAN table.".into()
                },
                Block::text("Let me look."),
                Block::ToolUse {
                    id: "toolu_9".into(),
                    name: "run_command".into(),
                    input: json!({"command": "show vlan brief"})
                },
            ]
        );
        let raw = response.message.raw.unwrap().content;
        assert_eq!(
            raw[0],
            json!({"type": "thinking", "thinking": "Check the VLAN table.", "signature": "c2lnbmF0dXJl"})
        );
        assert_eq!(raw[2]["input"], json!({"command": "show vlan brief"}));
        assert!(response.invalid_tool_inputs.is_empty());
    }

    #[test]
    fn unparsable_tool_input_is_reported_not_run() {
        let (state, _) = feed(&[
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "toolu_x", "name": "run_command", "input": {}}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"command\": \"show run"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "max_tokens"}, "usage": {"output_tokens": 9}}),
        ]);
        let response = state.finish("m").unwrap();
        assert_eq!(response.stop, StopReason::MaxTokens);
        assert_eq!(
            response.invalid_tool_inputs,
            vec![(
                "toolu_x".to_string(),
                "{\"command\": \"show run".to_string()
            )]
        );
        assert_eq!(response.message.raw.unwrap().content[0]["input"], json!({}));
    }

    #[test]
    fn refusal_carries_category_and_never_reads_content() {
        let (state, _) = feed(&[
            json!({"type": "message_start", "message": {"model": "claude-fable-5-2", "usage": {"input_tokens": 10}}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "refusal", "stop_details": {"type": "refusal", "category": "cyber", "explanation": "declined"}}, "usage": {"output_tokens": 0}}),
        ]);
        let response = state.finish("claude-fable-5-2").unwrap();
        assert_eq!(
            response.stop,
            StopReason::Refusal {
                category: Some("cyber".into()),
                explanation: Some("declined".into())
            }
        );
        assert!(response.message.content.is_empty());
    }

    #[test]
    fn mid_output_fallback_drops_the_declined_attempts_thinking_and_tools() {
        let (state, _) = feed(&[
            json!({"type": "message_start", "message": {"model": "claude-opus-5", "usage": {}}}),
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "a", "signature": "s1"}}),
            json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": "Partial answer"}}),
            json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_dead", "name": "run_command", "input": {}}}),
            json!({"type": "content_block_start", "index": 3, "content_block": {"type": "fallback", "from": {"model": "claude-fable-5-2"}, "to": {"model": "claude-opus-5"}}}),
            json!({"type": "content_block_start", "index": 4, "content_block": {"type": "thinking", "thinking": "b", "signature": "s2"}}),
            json!({"type": "content_block_start", "index": 5, "content_block": {"type": "text", "text": "Final answer"}}),
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 5}}),
        ]);
        let response = state.finish("claude-fable-5-2").unwrap();
        assert_eq!(response.model, "claude-opus-5");
        assert_eq!(
            response.notices,
            vec![
                "claude-fable-5-2 declined this request; claude-opus-5 answered instead."
                    .to_string()
            ]
        );
        let kinds: Vec<String> = response
            .message
            .raw
            .as_ref()
            .unwrap()
            .content
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds, ["text", "fallback", "thinking", "text"]);
        assert!(
            response.message.tool_uses().next().is_none(),
            "tool calls from the declined attempt must not run"
        );
    }

    #[test]
    fn stream_errors_and_truncated_streams_are_errors() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut state = StreamState::default();
        let err = state
            .handle(&json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}).to_string(), &tx)
            .unwrap_err();
        assert!(err.is_retryable());
        assert!(StreamState::default().finish("m").is_err());
    }
}
