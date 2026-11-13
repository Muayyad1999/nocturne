//! Command runner, configuration templates, playbooks, tasks, schedules,
//! triggers and Ansible interoperability — the repetitive part of the job,
//! made repeatable.
//!
//! # What this crate may do
//!
//! It runs things on many devices and tells the truth about each one. It does
//! **not** configure a device. Reads go through `connect::Cli::read`, which is
//! default-deny: a command is sent only if the platform's policy classifies it
//! as read-only, and a run is refused up front if any of its commands is not a
//! read on any of its targets. Configuration — from a template or a playbook —
//! is handed to `changes.create` as lines, where the change pipeline computes
//! the plan, assesses the risk, asks a human about *that exact plan*, backs
//! the device up, sends, verifies, and rolls back if it has to. There is no
//! flag in this crate that shortcuts that, and there will not be one.
//!
//! The one path that reaches a device outside the pipeline is the Ansible
//! interop ([`ansible`]), because `ansible-playbook` is somebody else's
//! program doing somebody else's work. It says so on every run, in the job log
//! and on the run record, and [`ansible`]'s own documentation lists guarantee
//! by guarantee what it cannot promise.
//!
//! # The shape of it
//!
//! ```text
//! select  ── a filter, resolved at run time, not a saved list of ids
//! runner  ── fan out with bounded concurrency; one result per selected device,
//!            `ok` / `failed` / `unreached` / `skipped`, never a shorter list
//! state   ── one device belongs to one run at a time; a schedule that fires
//!            while its last run is going does not start a second one
//! templates/playbooks ── render and reason; produce changes, send nothing
//! schedules/triggers  ── when to run: a clock, or something that happened
//! ansible ── run what the team already has, and label it honestly
//! ```
//!
//! Specification: `docs/dev/features/automation.md` (conventions:
//! `docs/dev/features/README.md`).

pub mod ansible;
pub mod builtins;
pub mod clock;
pub mod compare;
pub mod cron;
pub mod db;
pub mod model;
pub mod playbooks;
pub mod render;
pub mod runner;
pub mod schedules;
pub mod select;
pub mod settings;
pub mod state;
pub mod tasks;
pub mod templates;
pub mod triggers;

pub use model::{Action, DeviceResult, DeviceState, Run, RunKind, Schedule, Task, Trigger, When};

/// Register this feature's settings, state, RPC methods and background tasks.
pub fn init(builder: &mut nocturne_app::AppBuilder) {
    settings::declare(builder);
    builder.state(state::AutomationState::default());
    runner::register(builder);
    templates::register(builder);
    playbooks::register(builder);
    tasks::register(builder);
    schedules::register(builder);
    triggers::register(builder);
    ansible::register(builder);
    builder.on_start(|app| {
        schedules::spawn(app);
        triggers::spawn(app);
    });
}
