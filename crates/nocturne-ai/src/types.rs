//! Provider-neutral conversation types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::ProviderKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// An operator instruction added mid-conversation (never the first message).
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Block {
    Text {
        text: String,
    },
    /// A reasoning summary, kept for display. Providers replay their own
    /// native form from [`Message::raw`]; this block is never sent back.
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(default)]
        is_error: bool,
    },
}

impl Block {
    pub fn text(text: impl Into<String>) -> Block {
        Block::Text { text: text.into() }
    }
}

/// Provider-native message content that must be replayed verbatim.
///
/// Anthropic thinking blocks carry signatures that are bound to the exact
/// conversation that produced them, so the assistant turn is stored as it
/// arrived and echoed unchanged on later requests to the same provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RawContent {
    pub provider: ProviderKind,
    pub model: String,
    pub content: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Block>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<RawContent>,
}

impl Message {
    pub fn user(text: impl Into<String>) -> Message {
        Message {
            role: Role::User,
            content: vec![Block::text(text)],
            raw: None,
        }
    }

    pub fn system(text: impl Into<String>) -> Message {
        Message {
            role: Role::System,
            content: vec![Block::text(text)],
            raw: None,
        }
    }

    pub fn assistant(text: impl Into<String>) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![Block::text(text)],
            raw: None,
        }
    }

    /// All text blocks joined with blank lines.
    pub fn text(&self) -> String {
        let parts: Vec<&str> = self
            .content
            .iter()
            .filter_map(|block| match block {
                Block::Text { text } if !text.is_empty() => Some(text.as_str()),
                _ => None,
            })
            .collect();
        parts.join("\n\n")
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|block| match block {
            Block::ToolUse { id, name, input } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

/// A tool the model may call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema (draft-07 subset) for the arguments object.
    pub input_schema: Value,
    /// Whether calls to this tool may run concurrently with other calls.
    #[serde(default)]
    pub parallel_safe: bool,
}

pub struct ChatRequest<'a> {
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    /// When false the model must answer in text only (used for summaries).
    pub allow_tools: bool,
}

/// Incremental output while a response is being generated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolStart {
        id: String,
        name: String,
    },
    /// A fragment of the tool-call arguments JSON, as generated.
    ToolInput {
        id: String,
        partial: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    /// The provider declined to answer (safety classifier or model refusal).
    Refusal {
        category: Option<String>,
        explanation: Option<String>,
    },
    /// A server-side loop paused; re-send the conversation to resume.
    Pause,
    Other {
        detail: String,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
}

impl Usage {
    /// Tokens the request occupied in the model's context window.
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens + self.cache_read_tokens + self.cache_write_tokens + self.output_tokens
    }

    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_read_tokens += other.cache_read_tokens;
        self.cache_write_tokens += other.cache_write_tokens;
    }
}

#[derive(Clone, Debug)]
pub struct ChatResponse {
    pub message: Message,
    pub stop: StopReason,
    pub usage: Usage,
    /// The model that actually produced the message (may differ after a fallback).
    pub model: String,
    /// Tool-use ids whose arguments were not valid JSON, with the raw text received.
    pub invalid_tool_inputs: Vec<(String, String)>,
    /// Things worth telling the user (a fallback model answered, …).
    pub notices: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub max_output: Option<u32>,
}
