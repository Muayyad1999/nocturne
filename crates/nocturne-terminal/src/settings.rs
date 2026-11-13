//! The `terminal` settings section.

use nocturne_app::App;
use serde::{Deserialize, Serialize};

pub(crate) const SECTION: &str = "terminal";

/// Bounds of the scrollback a session may keep, in KiB. The lower one keeps
/// the buffer larger than a single `terminal.output` event.
const SCROLLBACK_KIB: std::ops::RangeInclusive<u32> = 64..=16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    /// Raw output kept per session for re-attaching, in KiB. Applies to
    /// sessions opened after it changes.
    pub scrollback_kib: u32,
    /// Log every session to disk unless `terminal.open` says otherwise.
    pub log_sessions: bool,
    /// Answer the login prompts of Telnet, serial and simulated devices from
    /// the credential, once, when the session starts.
    pub auto_login: bool,
    /// Close sessions nobody typed into for this many minutes; `0` never does.
    pub idle_timeout_min: u32,
    /// Send SSH keep-alives (at the interval of the `connections` settings).
    pub keepalive: bool,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        TerminalSettings {
            scrollback_kib: 512,
            log_sessions: false,
            auto_login: true,
            idle_timeout_min: 0,
            keepalive: true,
        }
    }
}

impl TerminalSettings {
    /// The current settings. A section that cannot be read (a value of the
    /// wrong type stored by hand) falls back to the defaults rather than
    /// taking the terminal down.
    pub(crate) fn load(app: &App) -> TerminalSettings {
        nocturne_app::settings::get(app, SECTION).unwrap_or_else(|err| {
            tracing::warn!(%err, "terminal settings are unreadable; using the defaults");
            TerminalSettings::default()
        })
    }

    pub(crate) fn scrollback_bytes(&self) -> usize {
        let kib = self
            .scrollback_kib
            .clamp(*SCROLLBACK_KIB.start(), *SCROLLBACK_KIB.end());
        kib as usize * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_specification() {
        assert_eq!(
            serde_json::to_value(TerminalSettings::default()).unwrap(),
            serde_json::json!({
                "scrollback_kib": 512,
                "log_sessions": false,
                "auto_login": true,
                "idle_timeout_min": 0,
                "keepalive": true,
            })
        );
    }

    #[test]
    fn scrollback_is_clamped_to_a_sane_range() {
        let with = |scrollback_kib| TerminalSettings {
            scrollback_kib,
            ..TerminalSettings::default()
        };
        assert_eq!(with(512).scrollback_bytes(), 512 * 1024);
        assert_eq!(with(0).scrollback_bytes(), 64 * 1024);
        assert_eq!(with(u32::MAX).scrollback_bytes(), 16 * 1024 * 1024);
    }
}
