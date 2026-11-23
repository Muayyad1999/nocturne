//! The `assistant` settings section.
//!
//! `autonomy` is shared with the change engine, which is the crate that
//! enforces it (`changes.assistant_apply`); it is declared here as well so
//! that the section is complete whichever of the two is registered last.

use nocturne_app::{App, AppBuilder, settings};
use serde::de::Deserializer;
use serde::{Deserialize, Serialize};

pub const SECTION: &str = "assistant";

/// How much the assistant may do without a human.
///
/// Parsed leniently: anything unknown is `draft`, the safe end — a settings
/// file written by a newer version must never widen what the model may do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Autonomy {
    /// Never apply by itself.
    #[default]
    Draft,
    /// Only lab devices.
    Lab,
    /// Lab devices, and low-risk changes to real ones.
    LowRisk,
}

impl Autonomy {
    pub fn as_str(self) -> &'static str {
        match self {
            Autonomy::Draft => "draft",
            Autonomy::Lab => "lab",
            Autonomy::LowRisk => "low_risk",
        }
    }

    /// How the mode is explained to the model, in plain words.
    pub fn in_words(self) -> &'static str {
        match self {
            Autonomy::Draft => {
                "You may never apply a change yourself. Prepare it with `propose_change`, then \
                 tell the user what you prepared, what it would do and why, and ask them to \
                 review and approve it in the Changes screen."
            }
            Autonomy::Lab => {
                "You may apply a change yourself with `apply_change` when every device in it is \
                 a lab device. For anything on the real network, prepare the change and ask the \
                 user to approve it."
            }
            Autonomy::LowRisk => {
                "You may apply a change yourself with `apply_change` on lab devices, and on real \
                 devices when the plan came back low risk with no protected resources. Anything \
                 else goes to the user for approval. The change engine decides; if it refuses, \
                 relay the reason."
            }
        }
    }
}

impl<'de> Deserialize<'de> for Autonomy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Autonomy, D::Error> {
        let raw = serde_json::Value::deserialize(deserializer)?;
        Ok(match raw.as_str().unwrap_or("draft") {
            "lab" => Autonomy::Lab,
            "low_risk" => Autonomy::LowRisk,
            _ => Autonomy::Draft,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Shared with `nocturne-changes`, which enforces it.
    pub autonomy: Autonomy,
    /// Ask the user before every tool that touches a device.
    pub confirm_reads: bool,
    /// Tool calls allowed in one turn.
    pub max_steps: u32,
    /// Seconds a whole turn may take.
    pub turn_timeout_s: u32,
    /// Rows of a table kept in a tool result.
    pub result_rows: u32,
    /// Characters of one tool result kept (head and tail).
    pub result_chars: u32,
    /// ISP edition: whether the model may see the customer ledger at all.
    /// Off unless somebody decides otherwise, because switching it on sends
    /// customers' names, plans and balances to whichever model is configured.
    /// Contact details are never sent, whatever this says.
    pub isp_access: bool,
    /// The team's own standing instructions, added to the system prompt.
    pub system_prompt_extra: String,
    /// Seconds to wait for an answer to an approval request.
    pub approval_timeout_s: u32,
    /// Turns that may run at the same time, across all chats.
    pub max_concurrent_turns: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            autonomy: Autonomy::Draft,
            confirm_reads: false,
            max_steps: 40,
            turn_timeout_s: 900,
            result_rows: 50,
            result_chars: 8000,
            isp_access: false,
            system_prompt_extra: String::new(),
            approval_timeout_s: 300,
            max_concurrent_turns: 3,
        }
    }
}

impl Settings {
    /// The section as stored, falling back to the defaults rather than failing
    /// a turn over a settings file nobody can read.
    pub fn read(app: &App) -> Settings {
        settings::get(app, SECTION).unwrap_or_default()
    }

    /// Result caps for a model with `context_window` tokens of room.
    pub fn caps_for(&self, context_window: u32) -> Caps {
        if small_context(context_window) {
            Caps {
                rows: (self.result_rows / 2).clamp(5, 20) as usize,
                chars: (self.result_chars / 3).clamp(500, 2_500) as usize,
            }
        } else {
            Caps {
                rows: self.result_rows.clamp(1, 1_000) as usize,
                chars: self.result_chars.clamp(200, 200_000) as usize,
            }
        }
    }
}

/// A model with this many tokens of context or fewer gets the core tool set
/// and tighter result caps. It is the window the engine assumes for a model
/// on the local network whose window nobody configured, and that is the
/// model the line is drawn for: every tool, even in the short form, would
/// take nearly all of what its conversation may grow to before the agent
/// compacts it.
pub const SMALL_CONTEXT: u32 = 16_000;

pub fn small_context(context_window: u32) -> bool {
    context_window > 0 && context_window <= SMALL_CONTEXT
}

/// How much of a result is kept for the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    pub rows: usize,
    pub chars: usize,
}

pub fn declare(builder: &mut AppBuilder) {
    settings::declare(builder, SECTION, &Settings::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn autonomy_falls_back_to_draft_for_anything_unknown() {
        let parse = |value: serde_json::Value| serde_json::from_value::<Autonomy>(value).unwrap();
        assert_eq!(parse(json!("low_risk")), Autonomy::LowRisk);
        assert_eq!(parse(json!("lab")), Autonomy::Lab);
        assert_eq!(parse(json!("full_self_driving")), Autonomy::Draft);
        assert_eq!(parse(json!(null)), Autonomy::Draft);
        assert_eq!(parse(json!(7)), Autonomy::Draft);
    }

    #[test]
    fn small_models_get_tighter_caps() {
        let settings = Settings::default();
        assert_eq!(
            settings.caps_for(200_000),
            Caps {
                rows: 50,
                chars: 8_000
            }
        );
        assert_eq!(
            settings.caps_for(8_192),
            Caps {
                rows: 20,
                chars: 2_500
            }
        );
        // The window a local model gets when nobody configured one is small,
        // and one token more is not.
        assert_eq!(settings.caps_for(16_000), settings.caps_for(8_192));
        assert_eq!(settings.caps_for(16_001), settings.caps_for(200_000));
        // An unknown window is not treated as a small one.
        assert!(!small_context(0));
    }

    #[test]
    fn the_customer_ledger_is_closed_until_somebody_opens_it() {
        assert!(!Settings::default().isp_access);
        // A settings file from before the key existed, and one that holds
        // something that is not a yes, both read as closed.
        let old: Settings = serde_json::from_value(json!({"autonomy": "lab"})).unwrap();
        assert!(!old.isp_access);
        assert!(serde_json::from_value::<Settings>(json!({"isp_access": "yes"})).is_err());
        let open: Settings = serde_json::from_value(json!({"isp_access": true})).unwrap();
        assert!(open.isp_access);
    }
}
