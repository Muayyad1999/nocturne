//! What the operator can tune about the pipeline, and the autonomy setting
//! the assistant is held to.

use nocturne_app::settings;
use nocturne_app::{App, AppBuilder};
use serde::{Deserialize, Serialize};

/// Settings section `changes`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Minutes the dead-man switch is armed for when the plan does not say.
    pub guard_minutes: u32,
    /// The most devices a change may be applied to at once.
    pub parallel_limit: u32,
    /// Attempts to reach a device again after a session is lost mid-apply.
    pub reconnect_attempts: u32,
    /// Seconds between those attempts.
    pub reconnect_interval_s: u32,
    /// Seconds to wait for a device that another operation is holding.
    pub connect_wait_s: u32,
    /// Keep the (redacted) session transcript in the change record.
    pub keep_transcripts: bool,
    /// Refuse to approve a change that names no ticket.
    pub require_ticket: bool,
    /// Allow a change to be applied to a device whose *device type* has never
    /// been checked against real hardware.
    ///
    /// Off, and the default is the point. A `community` or `experimental`
    /// profile is a set of regular expressions and command lists written from
    /// documentation; what it believes is read-only, how it enters
    /// configuration mode, and whether its dead-man switch exists have been
    /// checked against recorded output at best. Sending configuration through
    /// one is a reasonable thing to *decide* to do, and an unreasonable thing
    /// to do by accident on a device somebody depends on.
    ///
    /// `docs/profiles.md` and `docs/STATUS.md` have both promised this since
    /// the profile pack grew past Cisco and MikroTik; until now neither the
    /// apply path nor anything else enforced it, and only the *assistant* was
    /// held back (by `autonomy::decide`). A human could approve and apply.
    pub allow_unverified_writes: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            guard_minutes: 10,
            parallel_limit: 8,
            reconnect_attempts: 3,
            reconnect_interval_s: 20,
            connect_wait_s: 45,
            keep_transcripts: true,
            require_ticket: false,
            allow_unverified_writes: false,
        }
    }
}

impl Settings {
    pub fn read(app: &App) -> Settings {
        settings::get(app, SECTION).unwrap_or_default()
    }
}

pub const SECTION: &str = "changes";
pub const ASSISTANT_SECTION: &str = "assistant";

/// How much the AI assistant may do without a human. Shared with the
/// assistant feature, which declares the rest of its section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantSettings {
    pub autonomy: Autonomy,
}

impl Default for AssistantSettings {
    fn default() -> Self {
        AssistantSettings {
            autonomy: Autonomy::Draft,
        }
    }
}

/// The autonomy mode in force. An unreadable or absent section means
/// `draft`: the safe end.
pub fn autonomy(app: &App) -> Autonomy {
    settings::get_value(app, ASSISTANT_SECTION)
        .ok()
        .and_then(|value| value.get("autonomy").cloned())
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
}

pub fn declare(builder: &mut AppBuilder) {
    settings::declare(builder, SECTION, &Settings::default());
    // The assistant feature declares the rest of its section; the change
    // engine owns `autonomy` because it is the one that enforces it.
    settings::declare(builder, ASSISTANT_SECTION, &AssistantSettings::default());
}
