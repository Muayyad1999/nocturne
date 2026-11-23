//! The shapes this feature's RPC surface is written against.
//!
//! Mirrored field for field in `ui/src/api/types/automation.ts`. Everything
//! here is stored as a JSON document, so new fields must have a `#[serde(default)]`
//! and never change meaning.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// --------------------------------------------------------------- selection

/// Which devices an automation runs on.
///
/// A filter is resolved **when the run starts**, not when the task is saved,
/// so a task written for "every access switch in Oslo" keeps working as
/// switches are added and removed. Explicit ids and a filter may be combined:
/// the run is the union of both.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Selector {
    /// Device ids (or names, or addresses — resolved like `inventory::find`).
    pub device_ids: Vec<String>,
    pub filter: Option<Filter>,
    /// Devices to leave out of whatever the rest of the selector picked.
    pub exclude_device_ids: Vec<String>,
}

impl Selector {
    pub fn is_empty(&self) -> bool {
        self.device_ids.is_empty() && self.filter.is_none()
    }

    pub fn ids(ids: &[String]) -> Selector {
        Selector {
            device_ids: ids.to_vec(),
            ..Selector::default()
        }
    }
}

/// Device attributes to match. Every field given must match (`and`), except
/// `tags`, where the device must carry all of them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Filter {
    /// Free text over name, address, site, role, tags, profile, model, serial.
    pub query: Option<String>,
    pub site: Option<String>,
    pub role: Option<String>,
    pub tag: Option<String>,
    pub tags: Vec<String>,
    pub profile: Option<String>,
    /// `up`, `down` or `unknown` — as monitoring last saw the device.
    pub status: Option<String>,
    /// `true` only lab devices, `false` only real ones.
    pub lab: Option<bool>,
    /// `true` only protected devices, `false` only unprotected ones.
    pub protected: Option<bool>,
    /// At most this many devices, after sorting by name (a safety net for a
    /// filter that turns out to match the whole estate).
    pub limit: Option<usize>,
}

// -------------------------------------------------------------------- runs

/// What a run was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// Read-only commands over the device CLI.
    #[default]
    Commands,
    /// A Nocturne playbook.
    Playbook,
    /// An `ansible-playbook` process (outside the change pipeline — see `ansible.rs`).
    Ansible,
}

/// How one device fared. `Unreached` is not `Failed`: the device was in the
/// run and Nocturne never got to speak to it, which is the thing an operator
/// most needs to see.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceState {
    #[default]
    Ok,
    /// Reached, and something it was asked to do did not work.
    Failed,
    /// Never reached: no session could be opened (down, refused, timed out).
    Unreached,
    /// Deliberately not started: the device was busy in another run, or the
    /// run was stopped before its turn came.
    Skipped,
}

impl DeviceState {
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceState::Ok => "ok",
            DeviceState::Failed => "failed",
            DeviceState::Unreached => "unreached",
            DeviceState::Skipped => "skipped",
        }
    }
}

/// The header of a run, as history lists it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    pub id: String,
    pub kind: RunKind,
    pub name: String,
    /// The saved task this came from, when it came from one.
    pub task_id: Option<String>,
    /// `user`, `schedule`, `trigger` or `assistant`.
    pub origin: String,
    pub job_id: String,
    /// The commands a `commands` run sent, in order.
    pub commands: Vec<String>,
    /// The playbook a `playbook` run ran, by id when it was a saved one.
    pub playbook_id: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// Devices the run selected — including the ones it could not reach.
    pub devices: usize,
    pub ok: usize,
    pub failed: usize,
    pub unreached: usize,
    pub skipped: usize,
    pub cancelled: bool,
    /// Nothing was sent: this was a dry run.
    pub dry_run: bool,
    /// Changes a playbook or template run handed to the change pipeline.
    /// Nothing here has been applied: a change still needs its own approval.
    pub change_ids: Vec<String>,
    /// Said out loud on every run that is not covered by the change pipeline.
    pub caveat: Option<String>,
}

/// What one device did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceResult {
    pub device_id: String,
    pub name: String,
    pub state: DeviceState,
    /// `state == ok`, kept because the contract names it.
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why a device was skipped, in words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub outputs: Vec<CommandResult>,
    /// Per step, for a playbook run.
    pub steps: Vec<StepResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_id: Option<String>,
    pub elapsed_ms: u64,
}

impl DeviceResult {
    pub fn with_state(mut self, state: DeviceState) -> DeviceResult {
        self.state = state;
        self.ok = state == DeviceState::Ok;
        self
    }
}

/// One command and what the device printed.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandResult {
    pub command: String,
    pub output: String,
    /// The device's own error line, when it rejected the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub elapsed_ms: u64,
    /// The output was longer than the cap and was cut (the marker is in `output`).
    pub truncated: bool,
    /// Rows from the matching ntc-template, when `parse` was asked for and one
    /// exists. Absent is "no template", never "nothing matched".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<Vec<Value>>,
}

/// One playbook step on one device.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StepResult {
    pub name: String,
    /// `ok`, `failed` or `skipped`.
    pub state: String,
    pub detail: String,
    pub outputs: Vec<CommandResult>,
}

// -------------------------------------------------------------- the stored

/// A saved piece of repetitive work: what to run, and where.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub description: String,
    pub select: Selector,
    pub action: Action,
    pub tags: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_run_at: Option<i64>,
    pub last_run_id: Option<String>,
    /// `true` when the last run reached every device it selected without a failure.
    pub last_ok: Option<bool>,
}

/// What a task does. Nothing here sends configuration: `template` produces a
/// change for the change pipeline to decide on, and `ansible` is a separate,
/// clearly-labelled path (see the crate documentation).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Read-only commands, refused up front if any is not read-only.
    Commands {
        commands: Vec<String>,
        #[serde(default)]
        parse: bool,
    },
    /// A saved playbook, or a playbook written inline.
    Playbook {
        #[serde(default)]
        playbook_id: Option<String>,
        #[serde(default)]
        yaml: Option<String>,
        #[serde(default)]
        vars: Value,
    },
    /// Render a template per device and hand the lines to `changes.create`.
    Template {
        template_id: String,
        #[serde(default)]
        vars: Value,
    },
    /// Run an existing `ansible-playbook` against these devices.
    Ansible {
        playbook_path: String,
        #[serde(default = "default_true")]
        check: bool,
        #[serde(default)]
        extra_args: Vec<String>,
    },
}

fn default_true() -> bool {
    true
}

impl Default for Action {
    fn default() -> Self {
        Action::Commands {
            commands: Vec::new(),
            parse: false,
        }
    }
}

// --------------------------------------------------------------- templates

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateKind {
    /// Renders to configuration lines, which become a change.
    #[default]
    Config,
    /// Renders to read-only commands, which become a runner run.
    Commands,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: TemplateKind,
    pub body: String,
    /// Shipped with Nocturne: readable, not editable.
    pub builtin: bool,
    /// Device types it is written for; empty = any.
    pub profiles: Vec<String>,
    pub variables: Vec<Variable>,
    pub tags: Vec<String>,
    pub updated_at: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VariableKind {
    #[default]
    String,
    Int,
    Bool,
    Ip,
    Cidr,
    Vlan,
    Interface,
    List,
    Choice,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Variable {
    pub name: String,
    pub label: String,
    pub kind: VariableKind,
    pub required: bool,
    pub default: Option<Value>,
    pub choices: Vec<String>,
    pub help: String,
    /// A `string` that is text over several lines, such as a banner. Every
    /// other value is one line: a line break in it would be a second line of
    /// configuration.
    pub multiline: bool,
}

impl Variable {
    /// The label if there is one, else the name — what an error says.
    pub fn title(&self) -> &str {
        if self.label.trim().is_empty() {
            &self.name
        } else {
            &self.label
        }
    }
}

// --------------------------------------------------------------- playbooks

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Playbook {
    pub id: String,
    pub name: String,
    pub description: String,
    pub yaml: String,
    pub updated_at: i64,
}

// --------------------------------------------------------------- schedules

/// When a schedule fires. `daily` and `cron` are in the machine's local time
/// zone, so "02:30" stays 02:30 across a daylight-saving change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum When {
    Interval {
        every_s: u64,
    },
    Daily {
        /// `HH:MM`, 24-hour.
        at: String,
        /// `mon`…`sun`; empty = every day.
        #[serde(default)]
        days: Vec<String>,
    },
    /// Five fields: minute hour day-of-month month day-of-week.
    Cron {
        expr: String,
    },
}

impl Default for When {
    fn default() -> Self {
        When::Interval { every_s: 3600 }
    }
}

/// An RPC call a schedule or a trigger makes. The method must be on the
/// allow-list in `schedules.rs`; nothing that changes a device is on it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ActionCall {
    pub method: String,
    pub params: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Schedule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub when: When,
    pub action: ActionCall,
    pub last_run_at: Option<i64>,
    pub last_job_id: Option<String>,
    pub last_ok: Option<bool>,
    pub last_error: Option<String>,
    pub next_run_at: Option<i64>,
    pub created_at: i64,
}

// ---------------------------------------------------------------- triggers

/// Run something when the rest of the application says something happened.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Trigger {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// An event topic, exactly (`alerts.raised`) or as a prefix (`alerts.`).
    pub topic: String,
    /// Fire only when every one of these JSON pointers into the event payload
    /// equals the given value (`{"/severity": "critical"}`).
    pub match_payload: Value,
    /// Collect events for this long before firing, and fire once. `0` fires at
    /// once, but never more than once while an earlier run is still going.
    pub debounce_s: u64,
    pub action: ActionCall,
    /// Hand the device the event was about to the action as `device_ids`.
    pub pass_device: bool,
    pub last_fired_at: Option<i64>,
    pub last_job_id: Option<String>,
    pub fired_count: u64,
    pub created_at: i64,
}

// ----------------------------------------------------------------- ansible

/// What this machine has, and what the Ansible path does not promise.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnsibleStatus {
    pub installed: bool,
    pub version: Option<String>,
    pub path: Option<String>,
    pub collections: Option<Vec<String>>,
    /// Why it is not usable here, when it is not.
    pub unsupported: Option<String>,
    /// Said on every run; see the crate documentation.
    pub caveats: Vec<String>,
}
