//! Running one turn: the model, the tools, the events and what is kept.
//!
//! `ai.send` returns as soon as the user's message is stored; the turn itself
//! runs in the background and reports through `ai.event`. Whatever happens —
//! the model refusing, a tool failing, the user cancelling, the step budget
//! running out, the process being closed — the stored history is left in a
//! state the next turn can continue from: every tool call has a result, and
//! the readable history says what happened.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nocturne_ai::agent::{Agent, AgentConfig, AgentEvent, ToolCall, ToolHost, ToolOutput, TurnEnd};
use nocturne_ai::{CancellationToken, Message, StreamEvent, ToolSpec, Usage};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Semaphore, oneshot};

use crate::chats::{self, Chat};
use crate::prompt::{self, Context};
use crate::providers;
use crate::redact::ChatRedactor;
use crate::settings::{Caps, Settings, small_context};
use crate::shape;
use crate::tools::{self, Tool, ToolCtx};
use crate::view::{
    AiEvent, EventSink, Link, Stop, TokenUsage, ToolStatus, UiBlock, UiMessage, UiRole,
    summarise_input,
};

/// What one running turn can be told.
struct Running {
    turn_id: String,
    cancel: CancellationToken,
}

#[derive(Default)]
pub struct TurnState {
    running: Mutex<HashMap<String, Running>>,
    /// Answers to `approval_needed`, keyed by chat and call.
    approvals: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    slots: Mutex<Option<Arc<Semaphore>>>,
}

impl TurnState {
    /// Turns that may run at once, fixed on first use from the settings.
    fn slots(&self, settings: &Settings) -> Arc<Semaphore> {
        let mut slots = self.slots.lock();
        slots
            .get_or_insert_with(|| {
                Arc::new(Semaphore::new(
                    settings.max_concurrent_turns.clamp(1, 16) as usize
                ))
            })
            .clone()
    }
}

fn approval_key(chat_id: &str, call_id: &str) -> String {
    format!("{chat_id}/{call_id}")
}

// ------------------------------------------------------------- the host

/// What a tool call produced, for the event stream and the stored message.
#[derive(Clone)]
struct CallOutcome {
    status: ToolStatus,
    summary: String,
    link: Option<Link>,
}

struct Host {
    app: App,
    chat_id: String,
    sink: Arc<EventSink>,
    tools: Vec<&'static Tool>,
    specs: Vec<ToolSpec>,
    settings: Settings,
    caps: Caps,
    redactor: Arc<ChatRedactor>,
    cancel: CancellationToken,
    /// Tool calls made so far; `max_steps` is a budget of calls.
    used: Mutex<u32>,
    outcomes: Mutex<HashMap<String, CallOutcome>>,
}

impl Host {
    fn outcome(&self, call_id: &str, outcome: CallOutcome) {
        self.outcomes
            .lock()
            .insert(call_id.to_string(), outcome.clone());
    }

    /// Ask the user before touching a device, when they asked to be asked.
    async fn approved(&self, tool: &Tool, call: &ToolCall) -> Result<(), ToolOutput> {
        if !self.settings.confirm_reads || !tool.touches_device {
            return Ok(());
        }
        let (tx, rx) = oneshot::channel();
        let key = approval_key(&self.chat_id, &call.id);
        self.app
            .state::<TurnState>()
            .approvals
            .lock()
            .insert(key.clone(), tx);
        self.sink.send(AiEvent::ApprovalNeeded {
            call_id: call.id.clone(),
            name: tool.name.to_string(),
            title: tool.title.to_string(),
            detail: self.redactor.scrub_values(&summarise_input(&call.input)),
        });
        let wait = std::time::Duration::from_secs(self.settings.approval_timeout_s.max(5) as u64);
        let answer = tokio::select! {
            answer = rx => answer.unwrap_or(false),
            () = tokio::time::sleep(wait) => {
                self.app.state::<TurnState>().approvals.lock().remove(&key);
                self.outcome(&call.id, CallOutcome {
                    status: ToolStatus::Denied,
                    summary: "nobody answered".into(),
                    link: None,
                });
                return Err(ToolOutput::error(
                    "Nobody answered the request to run this, so it was not run. Tell the user what you wanted to do and why, and ask them to approve it.",
                ));
            }
            () = self.cancel.cancelled() => {
                self.app.state::<TurnState>().approvals.lock().remove(&key);
                return Err(ToolOutput::error("The user stopped this turn before approving it."));
            }
        };
        self.app.state::<TurnState>().approvals.lock().remove(&key);
        if answer {
            return Ok(());
        }
        self.outcome(
            &call.id,
            CallOutcome {
                status: ToolStatus::Denied,
                summary: "denied by the user".into(),
                link: None,
            },
        );
        Err(ToolOutput::error(
            "The user did not allow this. Do not try it again in another form; ask them what they would like you to do instead.",
        ))
    }
}

#[async_trait]
impl ToolHost for Host {
    fn tools(&self) -> Vec<ToolSpec> {
        self.specs.clone()
    }

    async fn call(&self, call: &ToolCall, _cancel: &CancellationToken) -> ToolOutput {
        let Some(tool) = self
            .tools
            .iter()
            .copied()
            .find(|tool| tool.name == call.name)
        else {
            return ToolOutput::error(format!("There is no tool called `{}`.", call.name));
        };

        // The step budget counts tool calls, which is what costs time on the
        // network; the model is told to wrap up rather than cut off mid-word.
        {
            let mut used = self.used.lock();
            *used += 1;
            if *used > self.settings.max_steps {
                self.outcome(
                    &call.id,
                    CallOutcome {
                        status: ToolStatus::Error,
                        summary: "step budget used up".into(),
                        link: None,
                    },
                );
                return ToolOutput::error(format!(
                    "This turn has used its budget of {} tool calls, so nothing else was run. \
                     Answer now with what you already know, and say what you did not get to.",
                    self.settings.max_steps
                ));
            }
        }

        if let Err(refusal) = self.approved(tool, call).await {
            return refusal;
        }

        let ctx = ToolCtx::new(
            self.app.clone(),
            tool,
            &self.chat_id,
            &call.id,
            self.redactor.clone(),
            self.caps,
            self.cancel.clone(),
        );
        let outcome = (tool.run)(&ctx, call.input.clone()).await;
        match outcome {
            Ok(ok) => {
                let link = ok.link.or_else(|| ctx.take_link());
                self.outcome(
                    &call.id,
                    CallOutcome {
                        status: ToolStatus::Ok,
                        summary: shape::one_line(&self.redactor.scrub_values(&ok.summary), 120),
                        link: link.clone(),
                    },
                );
                // Belt and braces: the gate redacted every RPC result as it
                // arrived; this catches a value a handler quoted in wording
                // of its own.
                ToolOutput::ok(self.redactor.scrub_values(&ok.content))
            }
            Err(err) => {
                let message = self.redactor.scrub_values(&err.message);
                self.outcome(
                    &call.id,
                    CallOutcome {
                        status: ToolStatus::Error,
                        summary: shape::one_line(&message, 120),
                        link: ctx.take_link(),
                    },
                );
                // A tool that fails is an error result for the model to read
                // and work around, not a failed turn.
                ToolOutput::error(match err.code {
                    "forbidden" => format!(
                        "Refused: {message}\nTell the user exactly this, and what they would have to do themselves."
                    ),
                    "cancelled" => message,
                    _ => format!("That did not work: {message}"),
                })
            }
        }
    }
}

// ------------------------------------------------------------- the runner

#[derive(Deserialize)]
struct SendParams {
    chat_id: String,
    text: String,
    #[serde(default)]
    context: Option<Context>,
}

async fn send(app: &App, params: SendParams) -> AppResult<Value> {
    let text = params.text.trim().to_string();
    if text.is_empty() {
        return Err(AppError::invalid("there is no message to send"));
    }
    let mut chat = chats::get(app, &params.chat_id)?;
    let record = match providers::get(app, &chat.provider_id) {
        Ok(record) if record.enabled => record,
        // The chat's provider was deleted or switched off: fall back to the
        // default rather than leaving the chat unusable.
        _ => providers::default_provider(app)?,
    };
    if record.kind != chat.provider_kind && chat.messages > 0 {
        return Err(AppError::conflict(format!(
            "This chat was started with a different kind of model ({:?}), and the two cannot share a conversation. Start a new chat; Nocturne will carry a summary over.",
            chat.provider_kind
        ))
        .with_details(json!({"needs": "new_chat", "chat_id": chat.id})));
    }

    let state = app.state::<TurnState>();
    if let Some(running) = state.running.lock().get(&chat.id) {
        return Err(AppError::conflict(format!(
            "This chat is still answering (turn {}). Wait for it, or stop it with ai.cancel.",
            running.turn_id
        )));
    }

    let turn_id = new_id();
    let cancel = CancellationToken::new();
    state.running.lock().insert(
        chat.id.clone(),
        Running {
            turn_id: turn_id.clone(),
            cancel: cancel.clone(),
        },
    );

    if chat.title.trim().is_empty() {
        chat.title = chats::title_from(&text);
    }
    chat.pending_turn = Some(turn_id.clone());
    chats::append_ui(
        app,
        &mut chat,
        UiMessage {
            turn_id: turn_id.clone(),
            role: UiRole::User,
            at: now_ms(),
            blocks: vec![UiBlock::Text { text: text.clone() }],
            ..UiMessage::default()
        },
    )?;

    let app_for_turn = app.clone();
    let turn = turn_id.clone();
    app.spawn("ai.turn", async move {
        let chat_id = chat.id.clone();
        let outcome = run_turn(
            &app_for_turn,
            chat,
            turn.clone(),
            text,
            params.context.unwrap_or_default(),
            record,
            cancel,
        )
        .await;
        if let Err(err) = outcome {
            let sink = EventSink::new(app_for_turn.clone(), &chat_id, &turn);
            sink.send(AiEvent::Error {
                code: err.code.to_string(),
                message: err.message.clone(),
            });
            sink.send(AiEvent::Done { stop: Stop::End });
            if let Ok(mut chat) = chats::get(&app_for_turn, &chat_id) {
                chat.pending_turn = None;
                let _ = chats::append_ui(
                    &app_for_turn,
                    &mut chat,
                    UiMessage {
                        turn_id: turn.clone(),
                        role: UiRole::Assistant,
                        at: now_ms(),
                        blocks: vec![UiBlock::Notice {
                            text: err.message.clone(),
                        }],
                        ..UiMessage::default()
                    },
                );
            }
        }
        app_for_turn
            .state::<TurnState>()
            .running
            .lock()
            .remove(&chat_id);
    });

    Ok(json!({"turn_id": turn_id}))
}

#[allow(clippy::too_many_lines)]
async fn run_turn(
    app: &App,
    mut chat: Chat,
    turn_id: String,
    text: String,
    context: Context,
    record: providers::ProviderRecord,
    cancel: CancellationToken,
) -> AppResult<()> {
    let settings = Settings::read(app);
    let sink = Arc::new(EventSink::new(app.clone(), &chat.id, &turn_id));
    let redactor = Arc::new(ChatRedactor::open(app, &chat.id));

    let provider = providers::build(app, &record)?;
    let window = record
        .context_window
        .unwrap_or_else(|| provider.context_window());
    let small = small_context(window);
    let caps = settings.caps_for(window);
    let available = tools::available(app, small);
    let specs: Vec<ToolSpec> = available.iter().map(|tool| tool.spec(small)).collect();

    // The model's own history. A changed prefix (new version, different tool
    // set) invalidates provider-native blocks, so they are dropped and the
    // turns replay as plain text, which every API accepts.
    let mut history = chats::load_context(app, &chat)?;
    let prefix = prefix_hash(&specs);
    if chat.model != record.model || chat.prefix != prefix {
        for message in &mut history {
            message.raw = None;
        }
        chat.model = record.model.clone();
        chat.prefix = prefix;
    }

    let briefing = prompt::briefing(app, &settings, &available, &context, &redactor);
    history.push(Message::user(prompt::user_message(&briefing, &text)));

    let host = Host {
        app: app.clone(),
        chat_id: chat.id.clone(),
        sink: sink.clone(),
        tools: available.clone(),
        specs,
        settings: settings.clone(),
        caps,
        redactor: redactor.clone(),
        cancel: cancel.clone(),
        used: Mutex::new(0),
        outcomes: Mutex::new(HashMap::new()),
    };

    let agent = Agent::new(
        provider.clone(),
        AgentConfig {
            // One model call per batch of tool calls, plus the final answer.
            max_steps: settings.max_steps as usize + 2,
            tool_result_chars: caps.chars.max(1_000) * 3,
            ..AgentConfig::default()
        },
    );

    // Wait for a slot first: queueing behind other chats is not time this
    // turn has spent, so the clock starts afterwards.
    let slots = app.state::<TurnState>().slots(&settings);
    let _slot = tokio::select! {
        slot = slots.acquire_owned() => slot.ok(),
        () = cancel.cancelled() => {
            sink.send(AiEvent::Done { stop: Stop::Cancelled });
            chat.pending_turn = None;
            chats::save(app, &chat)?;
            return Ok(());
        }
    };

    // The turn's own clock. Cancelling is how a timeout ends a turn, so the
    // history stays consistent: in-flight reads finish on their own.
    let timeout_fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watchdog = {
        let cancel = cancel.clone();
        let fired = timeout_fired.clone();
        let seconds = u64::from(settings.turn_timeout_s.max(1));
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
            fired.store(true, std::sync::atomic::Ordering::Relaxed);
            cancel.cancel();
        })
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AgentEvent>();
    let collector = Collector::new(sink.clone(), &available);
    let run = {
        let host = &host;
        let history = &mut history;
        let cancel = cancel.clone();
        async move {
            let end = agent.run(prompt::SYSTEM, history, host, &tx, &cancel).await;
            drop(tx);
            end
        }
    };
    let translate = async {
        let mut collector = collector;
        while let Some(event) = rx.recv().await {
            collector.handle(event, &host.outcomes);
        }
        collector
    };
    let (end, collector) = tokio::join!(run, translate);
    watchdog.abort();

    let timed_out = timeout_fired.load(std::sync::atomic::Ordering::Relaxed);
    let (stop, closing) = match &end {
        Ok(TurnEnd::Completed) => (Stop::End, None),
        Ok(TurnEnd::OutputLimit) => (Stop::End, None),
        Ok(TurnEnd::StepLimit) => (
            Stop::MaxSteps,
            Some(format!(
                "Stopped after {} tool calls without finishing.",
                settings.max_steps
            )),
        ),
        Ok(TurnEnd::Refused { explanation, .. }) => (
            Stop::Refused,
            Some(
                explanation
                    .clone()
                    .unwrap_or_else(|| "The model declined to answer this.".into()),
            ),
        ),
        Ok(TurnEnd::Cancelled) if timed_out => (
            Stop::Cancelled,
            Some(format!(
                "This turn ran past its time limit of {} seconds and was stopped. Anything already \
                 sent to a device finished on its own.",
                settings.turn_timeout_s
            )),
        ),
        Ok(TurnEnd::Cancelled) => (Stop::Cancelled, Some("Stopped by the user.".into())),
        // The model endpoint itself failed. The turn ends, the chat stays
        // usable, and the user is told in the endpoint's own words.
        Err(_) => (Stop::End, None),
    };
    let failure = match &end {
        Err(err) => {
            let err = AppError::from(err.clone());
            Some(AppError {
                code: err.code,
                message: redactor.scrub_values(&err.message),
                details: None,
            })
        }
        _ => None,
    };
    let used_steps = *host.used.lock();
    let stop = if used_steps > settings.max_steps && stop == Stop::End {
        Stop::MaxSteps
    } else {
        stop
    };

    let mut blocks = collector.blocks;
    if let Some(err) = &failure {
        sink.send(AiEvent::Error {
            code: err.code.to_string(),
            message: err.message.clone(),
        });
        blocks.push(UiBlock::Notice {
            text: err.message.clone(),
        });
    }
    if let Some(text) = &closing {
        sink.send(AiEvent::Notice { text: text.clone() });
        blocks.push(UiBlock::Notice { text: text.clone() });
    }
    if blocks.is_empty() {
        blocks.push(UiBlock::Notice {
            text: "The model answered with nothing at all.".into(),
        });
    }

    redactor.save(app, &chat.id);
    chat.pending_turn = None;
    chats::save_context(app, &mut chat, &history, collector.compacted)?;
    chats::append_ui(
        app,
        &mut chat,
        UiMessage {
            turn_id: turn_id.clone(),
            role: UiRole::Assistant,
            at: now_ms(),
            blocks,
            usage: Some(TokenUsage {
                input_tokens: collector.usage.input_tokens,
                output_tokens: collector.usage.output_tokens,
            }),
            ..UiMessage::default()
        },
    )?;
    if collector.usage != Usage::default() {
        let _ = crate::usage::record(app, &record.id, &record.model, &collector.usage);
    }
    sink.send(AiEvent::Done { stop });
    // Last, so the stored number covers every event of the turn: a UI that
    // comes back after a restart can tell whether it missed anything.
    chat.seq = chats::peek_seq(app, &chat.id);
    chats::save(app, &chat)?;
    Ok(())
}

/// A stable fingerprint of the conversation prefix the provider binds to.
fn prefix_hash(specs: &[ToolSpec]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(prompt::SYSTEM.as_bytes());
    for spec in specs {
        hasher.update(spec.name.as_bytes());
        hasher.update(spec.description.as_bytes());
    }
    hex::encode(&hasher.finalize()[..8])
}

/// Turns the agent's events into `ai.event` and into the message that is kept.
struct Collector {
    sink: Arc<EventSink>,
    titles: HashMap<&'static str, &'static str>,
    blocks: Vec<UiBlock>,
    thinking: String,
    usage: Usage,
    compacted: bool,
}

impl Collector {
    fn new(sink: Arc<EventSink>, tools: &[&'static Tool]) -> Collector {
        Collector {
            sink,
            titles: tools.iter().map(|tool| (tool.name, tool.title)).collect(),
            blocks: Vec::new(),
            thinking: String::new(),
            usage: Usage::default(),
            compacted: false,
        }
    }

    fn flush_thinking(&mut self) {
        let summary = std::mem::take(&mut self.thinking);
        if !summary.trim().is_empty() {
            self.blocks.push(UiBlock::Thinking { summary });
        }
    }

    fn handle(&mut self, event: AgentEvent, outcomes: &Mutex<HashMap<String, CallOutcome>>) {
        match event {
            AgentEvent::Stream { event } => match event {
                StreamEvent::Text { text } => self.sink.send(AiEvent::Text { delta: text }),
                StreamEvent::Thinking { text } => {
                    self.thinking.push_str(&text);
                    self.sink.send(AiEvent::Thinking { delta: text });
                }
                StreamEvent::ToolStart { .. } | StreamEvent::ToolInput { .. } => {}
            },
            AgentEvent::Message { text } => {
                self.flush_thinking();
                if !text.trim().is_empty() {
                    self.blocks.push(UiBlock::Text { text });
                }
            }
            AgentEvent::ToolCall { call } => {
                let title = self
                    .titles
                    .get(call.name.as_str())
                    .copied()
                    .unwrap_or(call.name.as_str())
                    .to_string();
                let input_summary = summarise_input(&call.input);
                self.sink.send(AiEvent::ToolStart {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    title: title.clone(),
                    input_summary: input_summary.clone(),
                });
                self.blocks.push(UiBlock::Tool {
                    call_id: call.id,
                    name: call.name,
                    title,
                    status: ToolStatus::Running,
                    input_summary,
                    result_summary: String::new(),
                    link: None,
                });
            }
            AgentEvent::ToolResult { id, output, .. } => {
                let recorded = outcomes.lock().get(&id).cloned();
                let (status, summary, link) = match recorded {
                    Some(outcome) => (outcome.status, outcome.summary, outcome.link),
                    // The agent itself rejected the call (bad arguments, no
                    // such tool): there is no outcome of ours to show.
                    None => (
                        if output.is_error {
                            ToolStatus::Error
                        } else {
                            ToolStatus::Ok
                        },
                        shape::one_line(&output.content, 120),
                        None,
                    ),
                };
                for block in self.blocks.iter_mut().rev() {
                    if let UiBlock::Tool {
                        call_id,
                        status: block_status,
                        result_summary,
                        link: block_link,
                        ..
                    } = block
                        && *call_id == id
                    {
                        *block_status = status;
                        *result_summary = summary.clone();
                        *block_link = link.clone();
                        break;
                    }
                }
                self.sink.send(AiEvent::ToolEnd {
                    call_id: id,
                    status,
                    result_summary: summary,
                    link,
                });
            }
            AgentEvent::Usage { last, .. } => {
                self.usage.add(&last);
                self.sink.send(AiEvent::Usage {
                    input_tokens: last.input_tokens,
                    output_tokens: last.output_tokens,
                });
            }
            AgentEvent::Notice { text } => {
                self.blocks.push(UiBlock::Notice { text: text.clone() });
                self.sink.send(AiEvent::Notice { text });
            }
            AgentEvent::Compacted => {
                self.compacted = true;
                let text = "The earlier part of this conversation was summarised to make room."
                    .to_string();
                self.blocks.push(UiBlock::Notice { text: text.clone() });
                self.sink.send(AiEvent::Notice { text });
            }
        }
    }
}

// ---------------------------------------------------------------- register

#[derive(Deserialize)]
struct ChatParams {
    chat_id: String,
}

#[derive(Deserialize)]
struct ApproveParams {
    chat_id: String,
    call_id: String,
    allow: bool,
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("ai.send", |app: App, params: SendParams| async move {
        send(&app, params).await
    });

    router.add("ai.cancel", |app: App, params: ChatParams| async move {
        let state = app.state::<TurnState>();
        let running = state.running.lock();
        match running.get(&params.chat_id) {
            Some(running) => {
                running.cancel.cancel();
                Ok(json!({"cancelled": true, "turn_id": running.turn_id}))
            }
            None => Ok(json!({"cancelled": false})),
        }
    });

    router.add("ai.approve", |app: App, params: ApproveParams| async move {
        let key = approval_key(&params.chat_id, &params.call_id);
        let answer = app.state::<TurnState>().approvals.lock().remove(&key);
        match answer {
            Some(tx) => {
                let _ = tx.send(params.allow);
                Ok(json!({"answered": true}))
            }
            None => Err(AppError::not_found(format!(
                "nothing is waiting for approval under `{}` (it may have timed out)",
                params.call_id
            ))),
        }
    });
}
