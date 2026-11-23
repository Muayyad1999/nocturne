//! The change record: what is to be done, what was planned, who signed it off
//! and what happened.
//!
//! These types are the contract the UI and the assistant are written against
//! (`docs/dev/features/changes.md`), mirrored field for field in
//! `ui/src/api/types/changes.ts`.
//!
//! They are the record as it is *kept*. The lines of an intent and of a plan
//! are kept exactly as they will be sent, secrets and all, so nothing here
//! is answered to a caller as it is: every method answers a view made by
//! `crate::redact`, which says which fields that touches and how.

use nocturne_drivers::hier::TextDiff;
use nocturne_drivers::session::CommandOutput;
use serde::{Deserialize, Serialize};

/// One intended configuration change, from draft to applied.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Change {
    pub id: String,
    pub title: String,
    pub description: String,
    pub origin: Origin,
    pub state: ChangeState,
    pub targets: Vec<Target>,
    pub options: Options,
    pub approval: Option<Approval>,
    pub created_at: i64,
    pub updated_at: i64,
    /// `user`, `assistant`, or the name of whatever created it.
    pub created_by: String,
    /// Free-text reference (a change-request number).
    pub ticket: Option<String>,
    /// The job that is applying (or last applied) this change.
    pub job_id: Option<String>,
    /// Set by `changes.rollback` on the change it creates: the change this one
    /// undoes. Its devices are taken back to the configuration they had
    /// before that change, by commands worked out from what they hold now.
    pub undoes: Option<String>,
    /// The latest change `changes.rollback` created to undo this one.
    pub undone_by: Option<String>,
}

/// Where the change came from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    User,
    Assistant,
    Template,
    Compliance,
    Restore,
    Drift,
    Playbook,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeState {
    #[default]
    Draft,
    Planned,
    Approved,
    Applying,
    Applied,
    Failed,
    RolledBack,
    PartiallyApplied,
    Cancelled,
}

impl ChangeState {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeState::Draft => "draft",
            ChangeState::Planned => "planned",
            ChangeState::Approved => "approved",
            ChangeState::Applying => "applying",
            ChangeState::Applied => "applied",
            ChangeState::Failed => "failed",
            ChangeState::RolledBack => "rolled_back",
            ChangeState::PartiallyApplied => "partially_applied",
            ChangeState::Cancelled => "cancelled",
        }
    }

    /// States in which the change may still be edited.
    pub fn is_editable(self) -> bool {
        matches!(self, ChangeState::Draft | ChangeState::Planned)
    }

    pub fn is_finished(self) -> bool {
        matches!(
            self,
            ChangeState::Applied
                | ChangeState::Failed
                | ChangeState::RolledBack
                | ChangeState::PartiallyApplied
                | ChangeState::Cancelled
        )
    }
}

/// One device and what it should get.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Target {
    pub device_id: String,
    pub intent: Intent,
    pub plan: Option<Plan>,
    pub result: Option<TargetResult>,
}

/// What the change is, before it is turned into commands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Intent {
    /// Configuration-mode lines exactly as they would be typed (section
    /// headers included).
    Lines { lines: Vec<String> },
    /// Desired configuration (whole file or fragment): the commands are
    /// computed by a hierarchical diff against the running configuration.
    Intended {
        config: String,
        /// `true`: the device must equal `config` (remove what is not in it);
        /// `false`: `config` must be present (additive).
        #[serde(default)]
        exact: bool,
    },
}

impl Default for Intent {
    fn default() -> Self {
        Intent::Lines { lines: Vec::new() }
    }
}

/// Exactly what will be sent to one device, and what it is predicted to do.
///
/// `commands`, `rollback`, `diff` and a hit's `command` are configuration
/// lines, kept whole. `warnings`, `risk.reasons` and a hit's `subject` and
/// `why` are sentences about those lines, and are kept with `<hidden>` where
/// a line they quote holds a secret value.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Plan {
    /// Exactly what will be sent in configuration mode, in order.
    pub commands: Vec<String>,
    /// What undoes it, computed from the same diff (empty = it could not be
    /// computed; see `warnings`).
    pub rollback: Vec<String>,
    /// Predicted running configuration, before → after.
    pub diff: TextDiff,
    pub risk: Risk,
    /// Why this needs a typed confirmation.
    pub protected_hits: Vec<ProtectedHit>,
    pub warnings: Vec<String>,
    /// The running-config backup the plan was computed against.
    pub base_version_id: i64,
    pub base_sha256: String,
    /// sha256 of the predicted configuration (empty when it could not be predicted).
    pub predicted_sha256: String,
    /// The dead-man switch that will be armed, if any.
    pub guard: Option<GuardPlan>,
    /// sha256 over device, commands, rollback, base_sha256 and options.
    pub plan_hash: String,
    pub planned_at: i64,
    /// The device type the plan was computed for.
    pub profile: String,
    /// How far that device type has been checked against real hardware.
    pub maturity: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Risk {
    pub level: RiskLevel,
    pub reasons: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    #[default]
    Low,
    Medium,
    High,
    Critical,
}

impl RiskLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            RiskLevel::Low => "low",
            RiskLevel::Medium => "medium",
            RiskLevel::High => "high",
            RiskLevel::Critical => "critical",
        }
    }

    /// One level worse (`critical` is the top).
    pub fn raised(self) -> RiskLevel {
        match self {
            RiskLevel::Low => RiskLevel::Medium,
            RiskLevel::Medium => RiskLevel::High,
            RiskLevel::High | RiskLevel::Critical => RiskLevel::Critical,
        }
    }
}

/// A resource the change touches that may not be touched without a human
/// typing the device's name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProtectedHit {
    /// Machine name of the rule that matched, e.g. `management_interface`.
    pub kind: String,
    /// What is protected: an interface, a VLAN, an address, the device.
    pub subject: String,
    /// The command that would touch it.
    pub command: String,
    /// What would happen, in words.
    pub why: String,
}

/// The dead-man switch armed around the change.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardPlan {
    pub name: String,
    pub minutes: u32,
    pub arm: Vec<String>,
    pub disarm: Vec<String>,
    pub effect: String,
}

/// How the change is applied.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    /// Persist to startup after a verified apply.
    pub save: bool,
    /// Arm the platform's dead-man switch for this many minutes; `None` =
    /// decide from the risk level.
    pub guard_minutes: Option<u32>,
    /// Devices at a time.
    pub parallel: u32,
    /// A failed device stops the remaining ones.
    pub stop_on_failure: bool,
    pub verify: Verify,
    /// Reserved for the lab twin.
    pub rehearse: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            save: true,
            guard_minutes: None,
            parallel: 1,
            stop_on_failure: true,
            verify: Verify::default(),
            rehearse: false,
        }
    }
}

/// What must still be true after the change.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Verify {
    /// Re-read the running configuration and check it is what the plan predicted.
    pub config: bool,
    /// No LLDP/CDP neighbour that existed before may be gone afterwards.
    pub neighbors: bool,
    /// No interface that was up before may be down afterwards (except the
    /// ones the change shuts down).
    pub interfaces: bool,
    pub routes: bool,
    /// Addresses the device itself must still be able to ping afterwards.
    pub pings: Vec<String>,
    /// Seconds to wait before verifying.
    pub settle_s: u32,
}

impl Default for Verify {
    fn default() -> Self {
        Verify {
            config: true,
            neighbors: true,
            interfaces: true,
            routes: false,
            pings: Vec::new(),
            settle_s: 5,
        }
    }
}

/// A human's (or the autonomy policy's) signature on specific plans.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Approval {
    /// `user`, or `policy:<mode>` for an assistant-applied change.
    pub by: String,
    pub at: i64,
    /// The `plan_hash` of every target, as it was approved.
    pub plan_hashes: Vec<String>,
    pub note: String,
    /// Devices whose name was typed out to confirm a protected hit.
    pub confirmed_devices: Vec<String>,
}

/// What happened on one device.
///
/// Everything in here is written down while the change is applied and holds
/// no secret value: `<hidden>` stands where one was, in the steps, the
/// checks, the error, the lines that were sent and the transcript alike. It
/// is `<hidden>` and not a numbered placeholder because the record is read
/// long after the table that numbered it is gone.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TargetResult {
    pub state: TargetState,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub steps: Vec<Step>,
    /// Every configuration line that was sent, with the device's answer —
    /// the planned lines and whatever a rollback derived on the spot. The
    /// values in them are hidden; the planned lines are whole in the plan.
    pub sent: Vec<CommandOutput>,
    /// Index into `plan.commands` of the line the device rejected.
    pub failed_line: Option<usize>,
    pub error: Option<String>,
    pub before_version_id: Option<i64>,
    pub after_version_id: Option<i64>,
    pub verification: Vec<Check>,
    /// The session transcript, with the values in it hidden.
    pub transcript: String,
    /// Whether this was written down with its secret values hidden. Every
    /// result recorded since the engine hides them says `true`; one that
    /// does not is a record from before, and is hidden each time it is read
    /// (so what a method answers always says `true`).
    pub redacted: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    #[default]
    Pending,
    Applying,
    Applied,
    Failed,
    RolledBack,
    RollbackFailed,
    Skipped,
    /// The device could not be reached again and its dead-man switch will fire.
    GuardFired,
}

impl TargetState {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetState::Pending => "pending",
            TargetState::Applying => "applying",
            TargetState::Applied => "applied",
            TargetState::Failed => "failed",
            TargetState::RolledBack => "rolled_back",
            TargetState::RollbackFailed => "rollback_failed",
            TargetState::Skipped => "skipped",
            TargetState::GuardFired => "guard_fired",
        }
    }
}

/// One step of the apply pipeline.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Step {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub at: i64,
}

/// One check run after the change.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

// ------------------------------------------------------------------ summary

/// A change as a list shows it: no plans, results or transcripts.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChangeSummary {
    pub id: String,
    pub title: String,
    pub description: String,
    pub origin: Origin,
    pub state: ChangeState,
    pub ticket: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub created_by: String,
    pub job_id: Option<String>,
    /// The worst risk of any planned target, when everything is planned.
    pub risk: Option<RiskLevel>,
    pub protected_hits: usize,
    pub approved_by: Option<String>,
    pub devices: Vec<DeviceSummary>,
}

/// One device inside a [`ChangeSummary`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceSummary {
    pub device_id: String,
    pub name: String,
    pub planned: bool,
    pub risk: Option<RiskLevel>,
    pub protected_hits: usize,
    pub commands: usize,
    pub state: Option<TargetState>,
}
