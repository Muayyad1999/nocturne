//! A provider that replays prepared replies. Used by tests (here and in the
//! application crate) to drive the agent loop deterministically, without a
//! network or an API key.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use super::{Provider, ProviderKind, next_call_id};
use crate::error::AiError;
use crate::types::{
    Block, ChatRequest, ChatResponse, Message, ModelInfo, Role, StopReason, StreamEvent, ToolSpec,
    Usage,
};

/// One scripted model turn.
pub enum Step {
    /// Reply with text and end the turn.
    Text(String),
    /// Optionally say something, then call tools: `(name, arguments)`.
    Tools(Option<String>, Vec<(String, Value)>),
    /// A fully specified response (stop reasons, invalid inputs, usage, …).
    Response(Box<ChatResponse>),
    Fail(AiError),
    /// Decide from the request (e.g. assert on tool results, then answer).
    Dynamic(Box<dyn FnMut(&RecordedRequest) -> Step + Send>),
}

impl Step {
    pub fn text(text: impl Into<String>) -> Step {
        Step::Text(text.into())
    }

    pub fn tool(name: impl Into<String>, arguments: Value) -> Step {
        Step::Tools(None, vec![(name.into(), arguments)])
    }
}

/// What the agent sent on one call, kept for assertions.
#[derive(Clone, Debug)]
pub struct RecordedRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub allow_tools: bool,
}

impl RecordedRequest {
    /// The tool results in the final message, as `(content, is_error)`.
    pub fn last_tool_results(&self) -> Vec<(String, bool)> {
        self.messages
            .last()
            .map(|message| {
                message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        Block::ToolResult {
                            content, is_error, ..
                        } => Some((content.clone(), *is_error)),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub struct Scripted {
    steps: Mutex<VecDeque<Step>>,
    requests: Mutex<Vec<RecordedRequest>>,
    context_window: u32,
    /// Reported as `usage.input_tokens` on every reply.
    pub input_tokens: u64,
}

impl Scripted {
    pub fn new(steps: Vec<Step>) -> Scripted {
        Scripted {
            steps: Mutex::new(steps.into()),
            requests: Mutex::new(Vec::new()),
            context_window: 200_000,
            input_tokens: 100,
        }
    }

    pub fn with_context_window(mut self, tokens: u32) -> Scripted {
        self.context_window = tokens;
        self
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .map(|requests| requests.clone())
            .unwrap_or_default()
    }

    pub fn remaining(&self) -> usize {
        self.steps.lock().map(|steps| steps.len()).unwrap_or(0)
    }

    fn respond(
        &self,
        step: Step,
        recorded: &RecordedRequest,
        events: &UnboundedSender<StreamEvent>,
    ) -> Result<ChatResponse, AiError> {
        let usage = Usage {
            input_tokens: self.input_tokens,
            output_tokens: 20,
            ..Usage::default()
        };
        let reply = |content: Vec<Block>, stop: StopReason| ChatResponse {
            message: Message {
                role: Role::Assistant,
                content,
                raw: None,
            },
            stop,
            usage,
            model: "scripted".into(),
            invalid_tool_inputs: Vec::new(),
            notices: Vec::new(),
        };
        match step {
            Step::Text(text) => {
                let _ = events.send(StreamEvent::Text { text: text.clone() });
                Ok(reply(vec![Block::Text { text }], StopReason::EndTurn))
            }
            Step::Tools(text, calls) => {
                let mut content = Vec::new();
                if let Some(text) = text {
                    let _ = events.send(StreamEvent::Text { text: text.clone() });
                    content.push(Block::Text { text });
                }
                for (name, input) in calls {
                    let id = next_call_id();
                    let _ = events.send(StreamEvent::ToolStart {
                        id: id.clone(),
                        name: name.clone(),
                    });
                    content.push(Block::ToolUse { id, name, input });
                }
                Ok(reply(content, StopReason::ToolUse))
            }
            Step::Response(response) => Ok(*response),
            Step::Fail(error) => Err(error),
            Step::Dynamic(mut decide) => {
                let next = decide(recorded);
                self.respond(next, recorded, events)
            }
        }
    }
}

#[async_trait]
impl Provider for Scripted {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Openai
    }

    fn model(&self) -> &str {
        "scripted"
    }

    fn context_window(&self) -> u32 {
        self.context_window
    }

    async fn chat(
        &self,
        request: ChatRequest<'_>,
        events: &UnboundedSender<StreamEvent>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse, AiError> {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let recorded = RecordedRequest {
            system: request.system.to_string(),
            messages: request.messages.to_vec(),
            tools: request.tools.to_vec(),
            allow_tools: request.allow_tools,
        };
        if let Ok(mut requests) = self.requests.lock() {
            requests.push(recorded.clone());
        }
        let step = self
            .steps
            .lock()
            .ok()
            .and_then(|mut steps| steps.pop_front())
            .ok_or_else(|| AiError::Protocol("the scripted provider ran out of steps".into()))?;
        self.respond(step, &recorded, events)
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, AiError> {
        Ok(vec![ModelInfo {
            id: "scripted".into(),
            name: "Scripted".into(),
            context_window: Some(self.context_window),
            max_output: None,
        }])
    }
}
