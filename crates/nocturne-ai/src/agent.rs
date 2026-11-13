//! The agent loop: let a model work through a task by calling tools until it
//! has an answer.
//!
//! The loop owns only mechanics — streaming, argument validation, result
//! truncation, context compaction, cancellation. *What* a tool does, and
//! whether a human must approve it first, is entirely the [`ToolHost`]'s
//! business: a host that needs approval simply awaits it inside `call`.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use tokio_util::sync::CancellationToken;

use crate::error::AiError;
use crate::provider::Provider;
use crate::schema;
use crate::types::{
    Block, ChatRequest, ChatResponse, Message, Role, StopReason, StreamEvent, ToolSpec, Usage,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> ToolOutput {
        ToolOutput {
            content: content.into(),
            is_error: false,
        }
    }

    pub fn error(content: impl Into<String>) -> ToolOutput {
        ToolOutput {
            content: content.into(),
            is_error: true,
        }
    }
}

/// The application side of the agent: the tools and how to run them.
#[async_trait]
pub trait ToolHost: Send + Sync {
    /// Must return the same set, in the same order, for the whole
    /// conversation: providers bind cached and signed context to it.
    fn tools(&self) -> Vec<ToolSpec>;

    /// Run one call. Arguments have already been validated against the
    /// tool's schema. Approval prompts, redaction and auditing happen here.
    async fn call(&self, call: &ToolCall, cancel: &CancellationToken) -> ToolOutput;
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Incremental model output.
    Stream { event: StreamEvent },
    /// The model finished one message (text and tool calls are final).
    Message { text: String },
    /// A validated tool call is about to run.
    ToolCall { call: ToolCall },
    ToolResult {
        id: String,
        name: String,
        output: ToolOutput,
    },
    Usage {
        last: Usage,
        total: Usage,
        context_tokens: u64,
        context_window: u32,
    },
    /// Something the user should know (fallback model answered, output limit hit, …).
    Notice { text: String },
    /// Older history was replaced by a summary to stay within the context window.
    Compacted,
}

/// Why a turn ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum TurnEnd {
    Completed,
    Refused {
        category: Option<String>,
        explanation: Option<String>,
    },
    /// The model was still calling tools when the step budget ran out.
    StepLimit,
    /// The final reply was cut off by the output-token limit.
    OutputLimit,
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct AgentConfig {
    /// Model calls allowed per user turn.
    pub max_steps: usize,
    /// Characters of one tool result kept for the model; the middle is elided beyond this.
    pub tool_result_chars: usize,
    /// Compact once the conversation holds this many tokens…
    pub compact_at_tokens: u64,
    /// …or this fraction of the model's context window, whichever is smaller.
    pub compact_at_fraction: f64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        AgentConfig {
            max_steps: 60,
            tool_result_chars: 24_000,
            compact_at_tokens: 160_000,
            compact_at_fraction: 0.7,
        }
    }
}

const COMPACTION_PROMPT: &str = "Summarise this conversation so the work can continue from the summary alone — \
every earlier message is about to be discarded. Write notes to yourself, not a message to the user. Include: \
the user's goals, constraints and preferences; every device involved and what was learned about it (exact \
hostnames, interfaces, addresses, VLANs, software versions); changes that were proposed, approved, applied, \
rejected or rolled back, with their outcomes; relevant findings from the most recent tool results; anything \
still unresolved; and the next step you were about to take. Keep exact identifiers and any command output \
that later steps depend on. Do not call tools.";

const SUMMARY_HEADER: &str =
    "[Summary of the conversation so far — earlier messages were compacted]";

pub struct Agent {
    provider: Arc<dyn Provider>,
    config: AgentConfig,
}

impl Agent {
    pub fn new(provider: Arc<dyn Provider>, config: AgentConfig) -> Agent {
        Agent { provider, config }
    }

    pub fn provider(&self) -> &Arc<dyn Provider> {
        &self.provider
    }

    /// Run one user turn to completion.
    ///
    /// `history` must end with the user's new message. It is extended in
    /// place and stays a valid conversation whatever the outcome, including
    /// cancellation and errors, so the caller can persist it and carry on.
    pub async fn run(
        &self,
        system: &str,
        history: &mut Vec<Message>,
        host: &dyn ToolHost,
        events: &UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<TurnEnd, AiError> {
        let tools = host.tools();
        let mut total = Usage::default();
        let mut context_tokens = 0u64;

        for _ in 0..self.config.max_steps {
            if cancel.is_cancelled() {
                return Ok(TurnEnd::Cancelled);
            }
            match self
                .compact_if_needed(system, history, &tools, context_tokens, events, cancel)
                .await
            {
                Err(AiError::Cancelled) => return Ok(TurnEnd::Cancelled),
                Err(err) => {
                    let _ = events.send(AgentEvent::Notice {
                        text: format!("Could not compact the conversation: {err}"),
                    });
                }
                Ok(()) => {}
            }

            let request = ChatRequest {
                system,
                messages: history,
                tools: &tools,
                allow_tools: true,
            };
            let response = match self.chat(request, events, cancel).await {
                Ok(response) => response,
                Err(AiError::Cancelled) => return Ok(TurnEnd::Cancelled),
                Err(err) => return Err(err),
            };

            total.add(&response.usage);
            context_tokens = response.usage.context_tokens();
            let _ = events.send(AgentEvent::Usage {
                last: response.usage,
                total,
                context_tokens,
                context_window: self.provider.context_window(),
            });
            for notice in &response.notices {
                let _ = events.send(AgentEvent::Notice {
                    text: notice.clone(),
                });
            }

            if let StopReason::Refusal {
                category,
                explanation,
            } = &response.stop
            {
                // Partial output from a declined response is discarded, never treated as an answer.
                history.push(Message::assistant(
                    "[The model declined to continue with this request.]",
                ));
                return Ok(TurnEnd::Refused {
                    category: category.clone(),
                    explanation: explanation.clone(),
                });
            }

            let calls: Vec<ToolCall> = response
                .message
                .tool_uses()
                .map(|(id, name, input)| ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    input: input.clone(),
                })
                .collect();
            let _ = events.send(AgentEvent::Message {
                text: response.message.text(),
            });
            let ChatResponse {
                message,
                stop,
                invalid_tool_inputs,
                ..
            } = response;
            history.push(message);

            if calls.is_empty() {
                match stop {
                    StopReason::Pause => continue,
                    StopReason::MaxTokens => {
                        let _ = events.send(AgentEvent::Notice {
                            text: "The reply was cut off at the model's output limit.".into(),
                        });
                        return Ok(TurnEnd::OutputLimit);
                    }
                    _ => return Ok(TurnEnd::Completed),
                }
            }

            let truncated = matches!(stop, StopReason::MaxTokens);
            let results = self
                .run_tools(
                    &calls,
                    &tools,
                    &invalid_tool_inputs,
                    truncated,
                    host,
                    events,
                    cancel,
                )
                .await;
            history.push(Message {
                role: Role::User,
                content: results,
                raw: None,
            });
        }

        let _ = events.send(AgentEvent::Notice {
            text: format!(
                "Stopped after {} steps without finishing. Say \"continue\" to carry on.",
                self.config.max_steps
            ),
        });
        Ok(TurnEnd::StepLimit)
    }

    /// One model call, forwarding its stream to the agent's event channel.
    async fn chat(
        &self,
        request: ChatRequest<'_>,
        events: &UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<ChatResponse, AiError> {
        let (tx, mut rx) = unbounded_channel::<StreamEvent>();
        let call = async {
            let result = self.provider.chat(request, &tx, cancel).await;
            drop(tx);
            result
        };
        let forward = async {
            while let Some(event) = rx.recv().await {
                let _ = events.send(AgentEvent::Stream { event });
            }
        };
        let (result, ()) = tokio::join!(call, forward);
        result
    }

    /// Execute the tool calls of one assistant message and build the
    /// `tool_result` blocks — exactly one per call, in call order.
    async fn run_tools(
        &self,
        calls: &[ToolCall],
        tools: &[ToolSpec],
        invalid: &[(String, String)],
        truncated: bool,
        host: &dyn ToolHost,
        events: &UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Vec<Block> {
        // Decide what can run before running anything.
        let prepared: Vec<Result<ToolCall, ToolOutput>> = calls
            .iter()
            .map(|call| {
                if truncated {
                    return Err(ToolOutput::error(
                        "The reply hit the output limit before this tool call was complete, so nothing was run. \
                         Call the tool again; if the input is large, split the work into smaller calls.",
                    ));
                }
                if let Some((_, raw)) = invalid.iter().find(|(id, _)| *id == call.id) {
                    return Err(ToolOutput::error(json!({"INVALID_JSON": raw}).to_string()));
                }
                let Some(spec) = tools.iter().find(|tool| tool.name == call.name) else {
                    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
                    return Err(ToolOutput::error(format!(
                        "There is no tool named \"{}\". Available tools: {}.",
                        call.name,
                        names.join(", ")
                    )));
                };
                let mut input = call.input.clone();
                schema::coerce(&spec.input_schema, &mut input);
                match schema::validate(&spec.input_schema, &input) {
                    Ok(()) => Ok(ToolCall { id: call.id.clone(), name: call.name.clone(), input }),
                    Err(problem) => Err(ToolOutput::error(format!("Invalid arguments: {problem}."))),
                }
            })
            .collect();

        let all_parallel = prepared.len() > 1
            && prepared.iter().all(|entry| match entry {
                Ok(call) => tools
                    .iter()
                    .any(|tool| tool.name == call.name && tool.parallel_safe),
                Err(_) => true,
            });

        let run_one = |entry: Result<ToolCall, ToolOutput>, original: &ToolCall| {
            let name = original.name.clone();
            let id = original.id.clone();
            async move {
                let output = match entry {
                    Err(rejection) => rejection,
                    Ok(call) => {
                        let _ = events.send(AgentEvent::ToolCall { call: call.clone() });
                        if cancel.is_cancelled() {
                            ToolOutput::error("Cancelled by the user before this ran.")
                        } else {
                            host.call(&call, cancel).await
                        }
                    }
                };
                let output = ToolOutput {
                    content: elide(&output.content, self.config.tool_result_chars),
                    ..output
                };
                let _ = events.send(AgentEvent::ToolResult {
                    id: id.clone(),
                    name,
                    output: output.clone(),
                });
                Block::ToolResult {
                    tool_use_id: id,
                    content: output.content,
                    is_error: output.is_error,
                }
            }
        };

        if all_parallel {
            futures::future::join_all(
                prepared
                    .into_iter()
                    .zip(calls)
                    .map(|(entry, call)| run_one(entry, call)),
            )
            .await
        } else {
            let mut results = Vec::with_capacity(calls.len());
            for (entry, call) in prepared.into_iter().zip(calls) {
                results.push(run_one(entry, call).await);
            }
            results
        }
    }

    /// Replace the history with a summary when it has grown too large.
    async fn compact_if_needed(
        &self,
        system: &str,
        history: &mut Vec<Message>,
        tools: &[ToolSpec],
        reported_tokens: u64,
        events: &UnboundedSender<AgentEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), AiError> {
        let window = u64::from(self.provider.context_window());
        let limit = self
            .config
            .compact_at_tokens
            .min((window as f64 * self.config.compact_at_fraction) as u64);
        let size = reported_tokens.max(estimate_tokens(system, history, tools));
        if size < limit || history.len() < 3 {
            return Ok(());
        }

        // A plain trailing user message is the request still to be answered:
        // keep it verbatim and summarise only what came before.
        let pending = match history.last() {
            Some(last)
                if last.role == Role::User
                    && last
                        .content
                        .iter()
                        .all(|block| matches!(block, Block::Text { .. })) =>
            {
                history.pop()
            }
            _ => None,
        };
        let mut to_summarise = history.clone();
        match to_summarise.last_mut() {
            Some(last) if last.role == Role::User => {
                last.content.push(Block::text(COMPACTION_PROMPT))
            }
            _ => to_summarise.push(Message::user(COMPACTION_PROMPT)),
        }

        let (tx, _rx) = unbounded_channel::<StreamEvent>();
        let request = ChatRequest {
            system,
            messages: &to_summarise,
            tools,
            allow_tools: false,
        };
        let outcome = self.provider.chat(request, &tx, cancel).await;
        let summary = match outcome {
            Ok(response)
                if !response.message.text().trim().is_empty()
                    && !matches!(response.stop, StopReason::Refusal { .. }) =>
            {
                response.message.text()
            }
            other => {
                // Leave the history exactly as it was.
                if let Some(pending) = pending {
                    history.push(pending);
                }
                return match other {
                    Err(err) => Err(err),
                    Ok(_) => Err(AiError::Protocol("the model returned no summary".into())),
                };
            }
        };

        let mut text = format!("{SUMMARY_HEADER}\n{}\n[End of summary]", summary.trim());
        match pending {
            Some(pending) => {
                text.push_str("\n\n");
                text.push_str(&pending.text());
            }
            None => text.push_str("\n\nContinue the task from where the summary leaves off."),
        }
        history.clear();
        history.push(Message::user(text));
        let _ = events.send(AgentEvent::Compacted);
        Ok(())
    }
}

/// A deliberately pessimistic token estimate (about three characters per token).
pub fn estimate_tokens(system: &str, history: &[Message], tools: &[ToolSpec]) -> u64 {
    let mut chars = system.len();
    for tool in tools {
        chars += tool.name.len() + tool.description.len() + tool.input_schema.to_string().len();
    }
    for message in history {
        for block in &message.content {
            chars += match block {
                Block::Text { text } | Block::Thinking { text } => text.len(),
                Block::ToolUse { name, input, .. } => name.len() + input.to_string().len(),
                Block::ToolResult { content, .. } => content.len(),
            };
        }
    }
    (chars / 3) as u64
}

/// Keep the head and tail of an over-long tool result and say what was dropped.
fn elide(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let head = max_chars * 7 / 10;
    let tail = max_chars - head;
    let head_text: String = text.chars().take(head).collect();
    let tail_text: String = text.chars().skip(total - tail).collect();
    format!(
        "{head_text}\n\n[… {} characters omitted. Narrow the request (a filter such as `| include` or `| section`, \
         a specific interface, a shorter time range) to see the rest …]\n\n{tail_text}",
        total - max_chars
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::scripted::{Scripted, Step};
    use std::sync::Mutex;

    struct Host {
        log: Mutex<Vec<ToolCall>>,
    }

    impl Host {
        fn new() -> Host {
            Host {
                log: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl ToolHost for Host {
        fn tools(&self) -> Vec<ToolSpec> {
            vec![
                ToolSpec {
                    name: "run_command".into(),
                    description: "Run a show command".into(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"device": {"type": "string"}, "command": {"type": "string"}, "timeout": {"type": "integer"}},
                        "required": ["device", "command"],
                        "additionalProperties": false
                    }),
                    parallel_safe: true,
                },
                ToolSpec {
                    name: "apply".into(),
                    description: "Apply config".into(),
                    input_schema: json!({"type": "object", "properties": {"lines": {"type": "array", "items": {"type": "string"}}}, "required": ["lines"]}),
                    parallel_safe: false,
                },
            ]
        }

        async fn call(&self, call: &ToolCall, _cancel: &CancellationToken) -> ToolOutput {
            self.log.lock().unwrap().push(call.clone());
            match call.name.as_str() {
                "run_command" => ToolOutput::ok(format!(
                    "output of `{}` on {}",
                    call.input["command"].as_str().unwrap(),
                    call.input["device"].as_str().unwrap()
                )),
                "apply" if call.input["lines"].as_array().unwrap().is_empty() => {
                    ToolOutput::error("nothing to apply")
                }
                _ => ToolOutput::ok("x".repeat(100_000)),
            }
        }
    }

    async fn run(
        steps: Vec<Step>,
        history: &mut Vec<Message>,
        config: AgentConfig,
    ) -> (
        Result<TurnEnd, AiError>,
        Vec<AgentEvent>,
        Arc<Scripted>,
        Host,
    ) {
        let provider = Arc::new(Scripted::new(steps));
        let agent = Agent::new(provider.clone(), config);
        let host = Host::new();
        let (tx, mut rx) = unbounded_channel();
        let end = agent
            .run(
                "system prompt",
                history,
                &host,
                &tx,
                &CancellationToken::new(),
            )
            .await;
        drop(tx);
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        (end, events, provider, host)
    }

    #[tokio::test]
    async fn runs_tools_until_the_model_answers() {
        let mut history = vec![Message::user("what vlans does sw1 have?")];
        let (end, events, provider, host) = run(
            vec![
                Step::Tools(
                    Some("Checking.".into()),
                    vec![(
                        "run_command".into(),
                        json!({"device": "sw1", "command": "show vlan brief"}),
                    )],
                ),
                Step::text("sw1 has VLANs 10 and 20."),
            ],
            &mut history,
            AgentConfig::default(),
        )
        .await;

        assert_eq!(end.unwrap(), TurnEnd::Completed);
        assert_eq!(host.log.lock().unwrap().len(), 1);
        assert_eq!(history.len(), 4);
        assert_eq!(history[3].text(), "sw1 has VLANs 10 and 20.");
        // The second request carried the tool result back to the model.
        let second = &provider.requests()[1];
        assert_eq!(
            second.last_tool_results(),
            vec![("output of `show vlan brief` on sw1".to_string(), false)]
        );
        assert!(events.iter().any(
            |event| matches!(event, AgentEvent::ToolResult { name, .. } if name == "run_command")
        ));
        assert!(
            events
                .iter()
                .any(|event| matches!(event, AgentEvent::Usage { .. }))
        );
    }

    #[tokio::test]
    async fn bad_arguments_and_unknown_tools_go_back_as_errors_without_running() {
        let mut history = vec![Message::user("go")];
        let (end, _, provider, host) = run(
            vec![
                Step::Tools(
                    None,
                    vec![
                        ("run_command".into(), json!({"device": "sw1"})),
                        ("reboot_everything".into(), json!({})),
                        (
                            "run_command".into(),
                            json!({"device": "sw1", "command": "show clock", "timeout": "30"}),
                        ),
                    ],
                ),
                Step::text("done"),
            ],
            &mut history,
            AgentConfig::default(),
        )
        .await;

        assert_eq!(end.unwrap(), TurnEnd::Completed);
        let results = provider.requests()[1].last_tool_results();
        assert_eq!(results.len(), 3, "every call gets exactly one result");
        assert!(results[0].1 && results[0].0.contains("missing required field \"command\""));
        assert!(results[1].1 && results[1].0.contains("no tool named \"reboot_everything\""));
        assert!(!results[2].1);
        // Only the valid call ran, with its "30" coerced to an integer.
        let log = host.log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].input["timeout"], json!(30));
    }

    #[tokio::test]
    async fn invalid_json_and_truncated_calls_are_not_executed() {
        let bad_call = Message {
            role: Role::Assistant,
            content: vec![Block::ToolUse {
                id: "t1".into(),
                name: "apply".into(),
                input: json!({}),
            }],
            raw: None,
        };
        let response = |stop| ChatResponse {
            message: bad_call.clone(),
            stop,
            usage: Usage::default(),
            model: "m".into(),
            invalid_tool_inputs: vec![("t1".into(), "{\"lines\": [\"vlan 3".into())],
            notices: vec![],
        };
        let mut history = vec![Message::user("go")];
        let (end, _, provider, host) = run(
            vec![
                Step::Response(Box::new(response(StopReason::ToolUse))),
                Step::Response(Box::new(response(StopReason::MaxTokens))),
                Step::text("ok"),
            ],
            &mut history,
            AgentConfig::default(),
        )
        .await;
        assert_eq!(end.unwrap(), TurnEnd::Completed);
        assert!(host.log.lock().unwrap().is_empty());
        let requests = provider.requests();
        assert!(
            requests[1].last_tool_results()[0]
                .0
                .contains("INVALID_JSON")
        );
        assert!(
            requests[2].last_tool_results()[0]
                .0
                .contains("output limit")
        );
    }

    #[tokio::test]
    async fn long_results_are_elided_and_tool_errors_are_flagged() {
        let mut history = vec![Message::user("go")];
        let (_, _, provider, _) = run(
            vec![
                Step::Tools(
                    None,
                    vec![
                        ("apply".into(), json!({"lines": ["vlan 30"]})),
                        ("apply".into(), json!({"lines": []})),
                    ],
                ),
                Step::text("ok"),
            ],
            &mut history,
            AgentConfig {
                tool_result_chars: 1_000,
                ..AgentConfig::default()
            },
        )
        .await;
        let results = provider.requests()[1].last_tool_results();
        assert!(results[0].0.contains("99000 characters omitted") && results[0].0.len() < 1_400);
        assert_eq!(results[1], ("nothing to apply".to_string(), true));
    }

    #[tokio::test]
    async fn refusal_ends_the_turn_with_a_valid_history() {
        let refusal = ChatResponse {
            message: Message {
                role: Role::Assistant,
                content: vec![Block::text("partial")],
                raw: None,
            },
            stop: StopReason::Refusal {
                category: Some("cyber".into()),
                explanation: None,
            },
            usage: Usage::default(),
            model: "m".into(),
            invalid_tool_inputs: vec![],
            notices: vec![],
        };
        let mut history = vec![Message::user("go")];
        let (end, ..) = run(
            vec![Step::Response(Box::new(refusal))],
            &mut history,
            AgentConfig::default(),
        )
        .await;
        assert_eq!(
            end.unwrap(),
            TurnEnd::Refused {
                category: Some("cyber".into()),
                explanation: None
            }
        );
        assert_eq!(history.len(), 2);
        assert!(!history[1].text().contains("partial"));
    }

    #[tokio::test]
    async fn step_limit_stops_a_runaway_loop() {
        let steps = (0..5)
            .map(|_| {
                Step::tool(
                    "run_command",
                    json!({"device": "a", "command": "show clock"}),
                )
            })
            .collect();
        let mut history = vec![Message::user("go")];
        let (end, events, _, host) = run(
            steps,
            &mut history,
            AgentConfig {
                max_steps: 3,
                ..AgentConfig::default()
            },
        )
        .await;
        assert_eq!(end.unwrap(), TurnEnd::StepLimit);
        assert_eq!(host.log.lock().unwrap().len(), 3);
        assert_eq!(
            history.last().unwrap().role,
            Role::User,
            "history ends with tool results, ready to continue"
        );
        assert!(
            events.iter().any(
                |event| matches!(event, AgentEvent::Notice { text } if text.contains("3 steps"))
            )
        );
    }

    #[tokio::test]
    async fn errors_and_cancellation_leave_history_usable() {
        let mut history = vec![Message::user("go")];
        let (end, ..) = run(
            vec![Step::Fail(AiError::Timeout)],
            &mut history,
            AgentConfig::default(),
        )
        .await;
        assert!(matches!(end, Err(AiError::Timeout)));
        assert_eq!(history.len(), 1);

        let provider = Arc::new(Scripted::new(vec![Step::text("never sent")]));
        let agent = Agent::new(provider, AgentConfig::default());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (tx, _rx) = unbounded_channel();
        let end = agent
            .run("s", &mut history, &Host::new(), &tx, &cancel)
            .await;
        assert_eq!(end.unwrap(), TurnEnd::Cancelled);
    }

    #[tokio::test]
    async fn compacts_when_the_context_fills_up() {
        let mut history = vec![
            Message::user("first question ".repeat(200)),
            Message::assistant("first answer ".repeat(200)),
            Message::user("second question ".repeat(200)),
            Message::assistant("second answer ".repeat(200)),
            Message::user("and now the real question"),
        ];
        let provider = Arc::new(
            Scripted::new(vec![
                Step::text("- user asked two things\n- sw1 is fine"),
                Step::text("final answer"),
            ])
            .with_context_window(2_000),
        );
        let agent = Agent::new(provider.clone(), AgentConfig::default());
        let (tx, mut rx) = unbounded_channel();
        let end = agent
            .run(
                "system",
                &mut history,
                &Host::new(),
                &tx,
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(end.unwrap(), TurnEnd::Completed);

        let requests = provider.requests();
        assert!(
            !requests[0].allow_tools,
            "the summary request must not offer tools"
        );
        assert!(
            requests[0]
                .messages
                .last()
                .unwrap()
                .text()
                .contains("Summarise this conversation")
        );
        // The real request is the summary plus the pending question, nothing else.
        assert_eq!(requests[1].messages.len(), 1);
        let carried = requests[1].messages[0].text();
        assert!(carried.contains("sw1 is fine") && carried.ends_with("and now the real question"));
        assert_eq!(history.len(), 2);
        let mut compacted = false;
        while let Ok(event) = rx.try_recv() {
            compacted |= matches!(event, AgentEvent::Compacted);
        }
        assert!(compacted);
    }

    #[test]
    fn elide_keeps_head_and_tail() {
        let text: String = (0..1000)
            .map(|i| char::from(b'a' + (i % 26) as u8))
            .collect();
        let short = elide(&text, 100);
        assert!(short.starts_with(&text[..70]) && short.ends_with(&text[970..]));
        assert!(short.contains("900 characters omitted"));
        assert_eq!(elide("short", 100), "short");
    }
}
