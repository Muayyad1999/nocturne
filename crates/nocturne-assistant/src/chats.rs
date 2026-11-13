//! Chats: what was said, in two forms.
//!
//! * The **model context** — `nocturne_ai::Message` exactly as it was sent and
//!   received, provider-native blocks included, because Anthropic's thinking
//!   blocks carry signatures that are only valid when replayed unchanged.
//! * The **UI messages** — the readable form the chat window renders.
//!
//! Both are append-only and stored per chat. Compaction does not delete
//! anything: it starts a new context generation and leaves the old one where
//! it is.

use std::collections::HashMap;

use nocturne_ai::{Message, ProviderKind};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, NoParams};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::redact::ChatRedactor;
use crate::shape;
use crate::view::{UiBlock, UiMessage, UiRole};

pub const CHATS: &str = "ai_chats";

fn ui_collection(chat_id: &str) -> String {
    format!("ai_messages.{chat_id}")
}

fn context_collection(chat_id: &str) -> String {
    format!("ai_context.{chat_id}")
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub provider_id: String,
    /// A chat is bound to the provider kind it started with: replayed
    /// provider-native blocks are only valid for the API that produced them.
    pub provider_kind: ProviderKind,
    pub model: String,
    /// Messages in the UI view, for the chat list.
    pub messages: usize,
    /// The last `ai.event` sequence number published for this chat.
    pub seq: u64,
    /// Bumped when the context is compacted; older generations stay stored.
    pub generation: u32,
    /// The turn that is running, if any (also tells a restart what to repair).
    pub pending_turn: Option<String>,
    /// Fingerprint of the conversation prefix (system prompt + tool set) the
    /// stored provider-native blocks were produced under.
    pub prefix: String,
}

impl Default for Chat {
    fn default() -> Self {
        Chat {
            id: String::new(),
            title: String::new(),
            created_at: 0,
            updated_at: 0,
            provider_id: String::new(),
            provider_kind: ProviderKind::Anthropic,
            model: String::new(),
            messages: 0,
            seq: 0,
            generation: 1,
            pending_turn: None,
            prefix: String::new(),
        }
    }
}

/// Event sequence numbers, one counter per chat.
#[derive(Default)]
pub struct ChatState {
    seq: Mutex<HashMap<String, u64>>,
}

/// The next `ai.event` sequence number for a chat.
pub fn next_seq(app: &App, chat_id: &str) -> u64 {
    let state = app.state::<ChatState>();
    let mut counters = state.seq.lock();
    let counter = counters
        .entry(chat_id.to_string())
        .or_insert_with(|| get(app, chat_id).map(|chat| chat.seq).unwrap_or_default());
    *counter += 1;
    *counter
}

/// The last sequence number handed out, without taking another.
pub fn peek_seq(app: &App, chat_id: &str) -> u64 {
    let state = app.state::<ChatState>();
    let counters = state.seq.lock();
    counters.get(chat_id).copied().unwrap_or_default()
}

pub fn get(app: &App, id: &str) -> AppResult<Chat> {
    app.store().require(CHATS, id, "chat")
}

pub fn save(app: &App, chat: &Chat) -> AppResult<()> {
    app.store().put(CHATS, &chat.id, chat)?;
    app.publish(
        "ai.chats.changed",
        &json!({"id": chat.id, "title": chat.title, "updated_at": chat.updated_at}),
    );
    Ok(())
}

pub fn list(app: &App, limit: usize) -> AppResult<Vec<Chat>> {
    let mut chats: Vec<Chat> = app.store().list(CHATS)?;
    chats.sort_by_key(|chat| std::cmp::Reverse(chat.updated_at));
    chats.truncate(limit);
    Ok(chats)
}

// ------------------------------------------------------------ UI messages

pub fn ui_messages(app: &App, chat_id: &str) -> AppResult<Vec<UiMessage>> {
    app.store().list(&ui_collection(chat_id))
}

/// Append a message to the readable history. Ids are ordinals, so the store's
/// own ordering is the conversation's.
pub fn append_ui(app: &App, chat: &mut Chat, message: UiMessage) -> AppResult<UiMessage> {
    let collection = ui_collection(&chat.id);
    let index = app.store().count(&collection)?;
    let message = UiMessage {
        id: format!("{index:08}"),
        chat_id: chat.id.clone(),
        ..message
    };
    app.store().put(&collection, &message.id, &message)?;
    chat.messages = (index + 1) as usize;
    chat.updated_at = now_ms();
    save(app, chat)?;
    Ok(message)
}

// --------------------------------------------------------- model context

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ContextEntry {
    generation: u32,
    index: u32,
    message: Message,
}

fn context_id(generation: u32, index: u32) -> String {
    format!("{generation:04}-{index:06}")
}

/// The conversation as the model must see it again: the current generation,
/// in order.
pub fn load_context(app: &App, chat: &Chat) -> AppResult<Vec<Message>> {
    let mut entries: Vec<ContextEntry> = app.store().list(&context_collection(&chat.id))?;
    entries.retain(|entry| entry.generation == chat.generation);
    entries.sort_by_key(|entry| entry.index);
    Ok(entries.into_iter().map(|entry| entry.message).collect())
}

/// Store the conversation after a turn. A compacted history starts a new
/// generation; what was there before is kept, not overwritten.
pub fn save_context(
    app: &App,
    chat: &mut Chat,
    history: &[Message],
    compacted: bool,
) -> AppResult<()> {
    let collection = context_collection(&chat.id);
    if compacted {
        chat.generation += 1;
    }
    let existing: Vec<ContextEntry> = app.store().list(&collection)?;
    for (index, message) in history.iter().enumerate() {
        let entry = ContextEntry {
            generation: chat.generation,
            index: index as u32,
            message: message.clone(),
        };
        app.store().put(
            &collection,
            &context_id(chat.generation, entry.index),
            &entry,
        )?;
    }
    // A generation never shrinks except through compaction, but a turn that
    // failed before its message was added can leave one stale tail entry.
    for entry in existing {
        if entry.generation == chat.generation && entry.index as usize >= history.len() {
            app.store()
                .delete(&collection, &context_id(entry.generation, entry.index))?;
        }
    }
    save(app, chat)
}

// ---------------------------------------------------------------- create

pub fn create(
    app: &App,
    title: Option<String>,
    provider_id: Option<String>,
    from_chat_id: Option<String>,
) -> AppResult<Chat> {
    let record = match provider_id {
        Some(id) if !id.is_empty() => crate::providers::get(app, &id)?,
        _ => crate::providers::default_provider(app)?,
    };
    let now = now_ms();
    let mut chat = Chat {
        id: new_id(),
        title: title.unwrap_or_default(),
        created_at: now,
        updated_at: now,
        provider_id: record.id.clone(),
        provider_kind: record.kind,
        model: record.model.clone(),
        ..Chat::default()
    };
    // Switching provider starts a new chat; the old one is carried over as a
    // summary rather than as messages another API would reject.
    if let Some(previous) = from_chat_id.filter(|id| !id.is_empty()) {
        let summary = carry_over(app, &previous)?;
        if !summary.is_empty() {
            app.store().put(
                &context_collection(&chat.id),
                &context_id(chat.generation, 0),
                &ContextEntry {
                    generation: chat.generation,
                    index: 0,
                    message: Message::user(summary.clone()),
                },
            )?;
            chat.title = format!("{} (continued)", get(app, &previous)?.title);
            append_ui(
                app,
                &mut chat,
                UiMessage {
                    role: UiRole::Assistant,
                    at: now,
                    blocks: vec![UiBlock::Notice {
                        text: "Carried over from the previous chat, because it was started with a \
                               different kind of model."
                            .into(),
                    }],
                    ..UiMessage::default()
                },
            )?;
        }
    }
    save(app, &chat)?;
    Ok(chat)
}

/// A deterministic digest of a chat — no model call, so switching provider
/// costs nothing and cannot fail.
fn carry_over(app: &App, chat_id: &str) -> AppResult<String> {
    let chat = get(app, chat_id)?;
    let messages = ui_messages(app, chat_id)?;
    if messages.is_empty() {
        return Ok(String::new());
    }
    let mut out = format!(
        "[Carried over from the earlier chat \"{}\". It was with a different model, so only this \
         summary came across.]\n",
        chat.title
    );
    let mut tools: Vec<String> = Vec::new();
    for message in &messages {
        for block in &message.blocks {
            match block {
                UiBlock::Text { text } if !text.trim().is_empty() => out.push_str(&format!(
                    "\n{}: {}",
                    match message.role {
                        UiRole::User => "The user said",
                        UiRole::Assistant => "You answered",
                    },
                    shape::one_line(text, 400)
                )),
                UiBlock::Tool {
                    name,
                    input_summary,
                    result_summary,
                    ..
                } => tools.push(format!("{name}({input_summary}) → {result_summary}")),
                _ => {}
            }
        }
    }
    if !tools.is_empty() {
        let kept: Vec<String> = tools.iter().rev().take(20).rev().cloned().collect();
        out.push_str(&format!("\n\nWhat you looked at:\n- {}", kept.join("\n- ")));
    }
    Ok(shape::cap_text(&out, 6_000))
}

/// Title a chat from the user's first message — no extra model call for
/// something nobody reads twice.
pub fn title_from(text: &str) -> String {
    let title = shape::one_line(text, 60);
    if title.is_empty() {
        "New chat".to_string()
    } else {
        title
    }
}

// ----------------------------------------------------------- housekeeping

/// A turn that was running when the process stopped is nobody's any more.
pub fn repair_interrupted(app: &App) -> AppResult<()> {
    for mut chat in app.store().list::<Chat>(CHATS)? {
        let Some(turn_id) = chat.pending_turn.take() else {
            continue;
        };
        append_ui(
            app,
            &mut chat,
            UiMessage {
                turn_id,
                role: UiRole::Assistant,
                at: now_ms(),
                blocks: vec![UiBlock::Notice {
                    text: "Nocturne was closed while this answer was being written, so it stopped \
                           here. Ask again to carry on."
                        .into(),
                }],
                ..UiMessage::default()
            },
        )?;
        save(app, &chat)?;
    }
    Ok(())
}

// ----------------------------------------------------------------- presets

/// The quick actions the UI offers. `needs` says what has to be in context
/// before the action makes sense.
fn presets() -> Value {
    json!([
        {"id": "explain_output", "title": "Explain this output", "needs": ["selection"],
         "description": "What the selected output means, line by line where it matters.",
         "prompt": "Explain the output I have selected: what it says, what is normal in it and what is not, and what I should look at next."},
        {"id": "why_down", "title": "Why is this interface down?", "needs": ["device"],
         "description": "Look at the port, its neighbour and the logs, and say what is wrong.",
         "prompt": "This interface is down. Look at the device's own view of the port, what it is connected to, recent log lines and anything monitoring saw, and tell me the most likely cause with the evidence for it. Do not guess without saying you are guessing."},
        {"id": "health_check", "title": "Health check this device", "needs": ["device"],
         "description": "Facts, interfaces, errors, alerts and anything that looks wrong.",
         "prompt": "Give this device a health check: what it is, how long it has been up, interfaces that are down or erroring, anything monitoring or the logs are complaining about, and the three things most worth fixing."},
        {"id": "open_alerts", "title": "Summarise open alerts", "needs": ["none"],
         "description": "What is firing, grouped by what is probably the same cause.",
         "prompt": "Summarise the alerts that are open: group the ones that are probably the same underlying problem, say which device to look at first and why."},
        {"id": "draft_change", "title": "Draft a change…", "needs": ["device"],
         "description": "Say what should be true; get a planned change back.",
         "prompt": "I want to change something on this device. Ask me what, gather what you need to know from the device first, then draft the change with propose_change and show me the plan before anything is applied."},
        {"id": "handover", "title": "Write the handover note", "needs": ["none"],
         "description": "What happened on the network today, for the next shift.",
         "prompt": "Write the handover note for the next shift: what changed today, what is still broken, what is being watched, and what the next person should pick up first. Use the changes, alerts and logs, and say where each point comes from."},
        {"id": "device_type", "title": "Create a device type from this output", "needs": ["selection"],
         "description": "Draft a device-type profile from a device's own output.",
         "prompt": "From the output I selected, draft a Nocturne device-type profile (YAML) for this platform: prompts, how to enable, how to enter configuration mode, which commands are read-only, and the collectors you can write confidently. Check it with validate_profile and tell me what you were unsure about. I will save it myself."}
    ])
}

// ---------------------------------------------------------------- register

#[derive(Deserialize)]
struct ListParams {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct CreateParams {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    provider_id: Option<String>,
    /// Carry a summary of this chat over into the new one.
    #[serde(default)]
    from_chat_id: Option<String>,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct RenameParams {
    id: String,
    title: String,
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("ai.chats.list", |app: App, params: ListParams| async move {
        list(&app, params.limit.unwrap_or(50).clamp(1, 500))
    });

    router.add(
        "ai.chats.create",
        |app: App, params: CreateParams| async move {
            create(&app, params.title, params.provider_id, params.from_chat_id)
        },
    );

    router.add("ai.chats.get", |app: App, params: IdParams| async move {
        let chat = get(&app, &params.id)?;
        let messages = ui_messages(&app, &params.id)?;
        let seq = peek_seq(&app, &params.id).max(chat.seq);
        Ok(json!({"chat": chat, "messages": messages, "seq": seq}))
    });

    router.add(
        "ai.chats.rename",
        |app: App, params: RenameParams| async move {
            let mut chat = get(&app, &params.id)?;
            let title = params.title.trim();
            if title.is_empty() {
                return Err(AppError::invalid("a chat needs a title"));
            }
            chat.title = shape::one_line(title, 120);
            chat.updated_at = now_ms();
            save(&app, &chat)?;
            Ok(chat)
        },
    );

    router.add("ai.chats.delete", |app: App, params: IdParams| async move {
        let chat = get(&app, &params.id)?;
        let ui = ui_collection(&chat.id);
        for message in ui_messages(&app, &chat.id)? {
            app.store().delete(&ui, &message.id)?;
        }
        let context = context_collection(&chat.id);
        for entry in app.store().list::<ContextEntry>(&context)? {
            app.store()
                .delete(&context, &context_id(entry.generation, entry.index))?;
        }
        app.store().delete(CHATS, &chat.id)?;
        ChatRedactor::forget(&app, &chat.id);
        app.state::<ChatState>().seq.lock().remove(&chat.id);
        app.publish("ai.chats.changed", &json!({"id": chat.id, "deleted": true}));
        Ok(json!({"deleted": true}))
    });

    router.add("ai.presets", |_app: App, _: NoParams| async move {
        Ok(presets())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_the_first_thing_the_user_said() {
        assert_eq!(
            title_from("  why is   Gi0/3 on SW1 down?\nit was fine yesterday  "),
            "why is Gi0/3 on SW1 down? it was fine yesterday"
        );
        assert_eq!(title_from("   "), "New chat");
        assert!(title_from(&"x".repeat(200)).chars().count() <= 60);
    }

    #[test]
    fn presets_name_what_they_need() {
        let presets = presets();
        for preset in presets.as_array().unwrap() {
            assert!(preset["id"].is_string());
            assert!(!preset["prompt"].as_str().unwrap().is_empty());
            for need in preset["needs"].as_array().unwrap() {
                assert!(
                    ["device", "selection", "alert", "change", "none"]
                        .contains(&need.as_str().unwrap()),
                    "{need}"
                );
            }
        }
    }
}
