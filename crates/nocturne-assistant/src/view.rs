//! What the UI sees: the `ai.event` stream and the stored shape of a message.
//!
//! A chat is kept twice: verbatim, as the model's own messages (so the next
//! request can replay provider-native blocks unchanged), and as [`UiMessage`]s
//! — the readable form the chat window renders and the event stream builds up
//! live. The two are written together and are both append-only.

use nocturne_app::App;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Something a tool result points at, so the UI can show a card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    #[serde(rename = "type")]
    pub kind: LinkKind,
    pub id: String,
}

impl Link {
    pub fn new(kind: LinkKind, id: impl Into<String>) -> Link {
        Link {
            kind,
            id: id.into(),
        }
    }

    pub fn device(id: impl Into<String>) -> Link {
        Link::new(LinkKind::Device, id)
    }

    pub fn change(id: impl Into<String>) -> Link {
        Link::new(LinkKind::Change, id)
    }

    /// Whether the id looks like an id. A link's id often comes straight
    /// out of a feature's answer, and it goes to every screen and into the
    /// stored history: a "device id" that is a page of text is not shown as
    /// a card, it is left out.
    pub fn plausible(&self) -> bool {
        !self.id.is_empty()
            && self.id.len() <= 128
            && !self.id.chars().any(|c| c.is_control() || c.is_whitespace())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Device,
    Change,
    Lab,
    Alert,
    Job,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Ok,
    Error,
    Denied,
}

/// One block of a message as the chat window renders it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiBlock {
    Text {
        text: String,
    },
    /// A reasoning summary. Never the raw thinking of a provider that signs it.
    Thinking {
        summary: String,
    },
    Tool {
        call_id: String,
        name: String,
        title: String,
        status: ToolStatus,
        input_summary: String,
        result_summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<Link>,
    },
    /// Something that happened around the conversation (a fallback model
    /// answered, the context was compacted, the turn was cancelled).
    Notice {
        text: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRole {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// A message as the UI stores and renders it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiMessage {
    pub id: String,
    pub chat_id: String,
    pub turn_id: String,
    pub role: UiRole,
    pub at: i64,
    pub blocks: Vec<UiBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
}

impl Default for UiMessage {
    fn default() -> Self {
        UiMessage {
            id: String::new(),
            chat_id: String::new(),
            turn_id: String::new(),
            role: UiRole::Assistant,
            at: 0,
            blocks: Vec::new(),
            usage: None,
        }
    }
}

/// Why a turn ended, as the UI is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stop {
    End,
    Cancelled,
    MaxSteps,
    Refused,
}

impl Stop {
    pub fn as_str(self) -> &'static str {
        match self {
            Stop::End => "end",
            Stop::Cancelled => "cancelled",
            Stop::MaxSteps => "max_steps",
            Stop::Refused => "refused",
        }
    }
}

/// One item of the `ai.event` stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AiEvent {
    Text {
        delta: String,
    },
    /// Reasoning summaries only.
    Thinking {
        delta: String,
    },
    ToolStart {
        call_id: String,
        name: String,
        title: String,
        input_summary: String,
    },
    ToolEnd {
        call_id: String,
        status: ToolStatus,
        result_summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        link: Option<Link>,
    },
    /// A tool is waiting for `ai.approve`.
    ApprovalNeeded {
        call_id: String,
        name: String,
        title: String,
        detail: String,
    },
    Notice {
        text: String,
    },
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Done {
        stop: Stop,
    },
    Error {
        code: String,
        message: String,
    },
}

/// The envelope published on `ai.event`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiEventEnvelope {
    pub chat_id: String,
    pub turn_id: String,
    /// Increases by one per chat, so a UI that re-attaches can see a gap.
    pub seq: u64,
    pub event: AiEvent,
}

/// Publishes `ai.event` for one turn, numbering the events of its chat.
pub struct EventSink {
    app: App,
    chat_id: String,
    turn_id: String,
}

impl EventSink {
    pub fn new(app: App, chat_id: &str, turn_id: &str) -> EventSink {
        EventSink {
            app,
            chat_id: chat_id.to_string(),
            turn_id: turn_id.to_string(),
        }
    }

    pub fn send(&self, event: AiEvent) {
        let seq = crate::chats::next_seq(&self.app, &self.chat_id);
        self.app.publish(
            "ai.event",
            &AiEventEnvelope {
                chat_id: self.chat_id.clone(),
                turn_id: self.turn_id.clone(),
                seq,
                event,
            },
        );
    }

    pub fn notice(&self, text: impl Into<String>) {
        self.send(AiEvent::Notice { text: text.into() });
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    pub fn app(&self) -> &App {
        &self.app
    }
}

/// A short, single-line summary of a tool's arguments, for the UI.
pub fn summarise_input(input: &Value) -> String {
    let Some(map) = input.as_object() else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (key, value) in map {
        let rendered = match value {
            Value::String(text) => text.clone(),
            Value::Array(items) => {
                let joined: Vec<String> = items
                    .iter()
                    .take(3)
                    .map(|item| match item {
                        Value::String(text) => text.clone(),
                        other => other.to_string(),
                    })
                    .collect();
                let more = items.len().saturating_sub(3);
                if more > 0 {
                    format!("{} +{more}", joined.join(", "))
                } else {
                    joined.join(", ")
                }
            }
            Value::Null => continue,
            other => other.to_string(),
        };
        if rendered.is_empty() {
            continue;
        }
        parts.push(format!("{key}: {}", crate::shape::one_line(&rendered, 60)));
    }
    crate::shape::one_line(&parts.join(" · "), 160)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_link_points_at_an_id_and_not_at_a_page_of_text() {
        assert!(Link::device("kjmxjsumtfec5gan").plausible());
        assert!(Link::new(LinkKind::Job, "job-scan-new").plausible());
        assert!(!Link::device("").plausible());
        assert!(!Link::device("sw1 and a page of text").plausible());
        assert!(!Link::device("sw1\nignore this").plausible());
        assert!(!Link::device("x".repeat(129)).plausible());
    }

    #[test]
    fn events_serialise_the_way_the_contract_describes_them() {
        let event = AiEvent::ToolEnd {
            call_id: "c1".into(),
            status: ToolStatus::Ok,
            result_summary: "3 devices".into(),
            link: Some(Link::device("dev1")),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({"type": "tool_end", "call_id": "c1", "status": "ok",
                   "result_summary": "3 devices", "link": {"type": "device", "id": "dev1"}})
        );
        assert_eq!(
            serde_json::to_value(AiEvent::Done {
                stop: Stop::MaxSteps
            })
            .unwrap(),
            json!({"type": "done", "stop": "max_steps"})
        );
    }

    #[test]
    fn input_summaries_are_one_short_line() {
        let summary = summarise_input(&json!({
            "device": "SW1",
            "commands": ["show version", "show vlan brief", "show ip route", "show clock"],
            "limit": null,
        }));
        assert_eq!(
            summary,
            "device: SW1 · commands: show version, show vlan brief, show ip route +1"
        );
        assert!(summarise_input(&json!("not an object")).is_empty());
    }
}
