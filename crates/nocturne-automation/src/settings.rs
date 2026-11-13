//! The `automation` settings section.

use nocturne_app::{App, AppBuilder};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Devices worked on at once in one run.
    pub concurrency: usize,
    /// Runs kept in history; the oldest are deleted with their outputs.
    pub keep_runs: usize,
    /// Cap on one command's output on one device, in bytes.
    pub output_cap_bytes: usize,
    /// Lines trimmed before `runner.compare` groups outputs: the parts that
    /// differ between two devices for no interesting reason.
    pub volatile_patterns: Vec<String>,
    /// The scheduler runs at all.
    pub schedules_enabled: bool,
    /// Triggers listen at all.
    pub triggers_enabled: bool,
    /// A schedule that was due while Nocturne was not running catches up once
    /// if it was missed by less than this.
    pub catch_up_within_s: u64,
    /// `ansible-playbook` to use; empty = look it up on `PATH`.
    pub ansible_path: String,
    /// How long an `ansible-playbook` process may run before it is stopped.
    pub ansible_timeout_s: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            concurrency: 8,
            keep_runs: 50,
            // The contract's cap. A single `show tech-support` is bigger than
            // this; the marker in the stored output says so.
            output_cap_bytes: 2 * 1024 * 1024,
            volatile_patterns: default_volatile(),
            schedules_enabled: true,
            triggers_enabled: true,
            catch_up_within_s: 7 * 24 * 3600,
            ansible_path: String::new(),
            ansible_timeout_s: 3600,
        }
    }
}

/// Lines whose content is a clock, a counter or a session id: two switches
/// with identical configuration still print different ones.
fn default_volatile() -> Vec<String> {
    [
        r"(?i)uptime is",
        r"(?i)^\s*(current )?(configuration|time)\s*:",
        r"(?i)last (input|output|clearing)",
        r"(?i)^Building configuration",
        r"\d{2}:\d{2}:\d{2}",
        r"(?i)^\s*ntp clock-period",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

impl Settings {
    pub fn load(app: &App) -> Settings {
        nocturne_app::settings::get(app, "automation").unwrap_or_default()
    }

    /// Clamped to something a desktop machine and a few hundred devices survive.
    pub fn concurrency(&self) -> usize {
        self.concurrency.clamp(1, 64)
    }

    pub fn keep_runs(&self) -> usize {
        self.keep_runs.clamp(1, 1000)
    }

    pub fn output_cap(&self) -> usize {
        self.output_cap_bytes.clamp(4 * 1024, 32 * 1024 * 1024)
    }
}

pub fn declare(builder: &mut AppBuilder) {
    nocturne_app::settings::declare(builder, "automation", &Settings::default());
}
