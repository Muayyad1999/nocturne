//! Where this feature's documents live, and how long runs are kept.

use nocturne_app::error::AppResult;
use nocturne_app::{App, AppError};

use crate::model::{DeviceResult, Playbook, Run, Schedule, Task, Template, Trigger};
use crate::settings::Settings;

pub const TASKS: &str = "automation.tasks";
pub const TEMPLATES: &str = "automation.templates";
pub const PLAYBOOKS: &str = "automation.playbooks";
pub const SCHEDULES: &str = "automation.schedules";
pub const TRIGGERS: &str = "automation.triggers";
pub const RUNS: &str = "automation.runs";
/// One document per run: every device's result. Runs are capped, so this
/// collection has at most `keep_runs` documents in it.
pub const RESULTS: &str = "automation.run_results";

pub fn save_run(app: &App, run: &Run) -> AppResult<()> {
    app.store().put(RUNS, &run.id, run)
}

pub fn get_run(app: &App, id: &str) -> AppResult<Run> {
    app.store().require(RUNS, id, "run")
}

pub fn save_results(app: &App, run_id: &str, results: &[DeviceResult]) -> AppResult<()> {
    app.store().put(RESULTS, run_id, &results.to_vec())
}

pub fn results(app: &App, run_id: &str) -> AppResult<Vec<DeviceResult>> {
    Ok(app.store().get(RESULTS, run_id)?.unwrap_or_default())
}

/// Newest first.
pub fn runs(app: &App, limit: usize) -> AppResult<Vec<Run>> {
    let mut runs: Vec<Run> = app.store().list(RUNS)?;
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
    runs.truncate(limit.clamp(1, 1000));
    Ok(runs)
}

pub fn delete_run(app: &App, id: &str) -> AppResult<bool> {
    app.store().delete(RESULTS, id)?;
    app.store().delete(RUNS, id)
}

/// Drop the oldest runs past the retention setting, outputs included.
pub fn prune_runs(app: &App) -> AppResult<usize> {
    let keep = Settings::load(app).keep_runs();
    let mut runs: Vec<Run> = app.store().list(RUNS)?;
    if runs.len() <= keep {
        return Ok(0);
    }
    runs.sort_by(|a, b| b.started_at.cmp(&a.started_at).then(b.id.cmp(&a.id)));
    let mut removed = 0;
    for run in &runs[keep..] {
        delete_run(app, &run.id)?;
        removed += 1;
    }
    Ok(removed)
}

pub fn task(app: &App, id: &str) -> AppResult<Task> {
    app.store().require(TASKS, id, "task")
}

pub fn tasks(app: &App) -> AppResult<Vec<Task>> {
    let mut tasks: Vec<Task> = app.store().list(TASKS)?;
    tasks.sort_by_key(|task| task.name.to_lowercase());
    Ok(tasks)
}

pub fn playbook(app: &App, id: &str) -> AppResult<Playbook> {
    app.store().require(PLAYBOOKS, id, "playbook")
}

pub fn playbooks(app: &App) -> AppResult<Vec<Playbook>> {
    let mut playbooks: Vec<Playbook> = app.store().list(PLAYBOOKS)?;
    playbooks.sort_by_key(|playbook| playbook.name.to_lowercase());
    Ok(playbooks)
}

pub fn stored_template(app: &App, id: &str) -> AppResult<Option<Template>> {
    app.store().get(TEMPLATES, id)
}

pub fn stored_templates(app: &App) -> AppResult<Vec<Template>> {
    app.store().list(TEMPLATES)
}

pub fn schedules(app: &App) -> AppResult<Vec<Schedule>> {
    let mut schedules: Vec<Schedule> = app.store().list(SCHEDULES)?;
    schedules.sort_by_key(|schedule| schedule.name.to_lowercase());
    Ok(schedules)
}

pub fn schedule(app: &App, id: &str) -> AppResult<Schedule> {
    app.store().require(SCHEDULES, id, "schedule")
}

pub fn triggers(app: &App) -> AppResult<Vec<Trigger>> {
    let mut triggers: Vec<Trigger> = app.store().list(TRIGGERS)?;
    triggers.sort_by_key(|trigger| trigger.name.to_lowercase());
    Ok(triggers)
}

pub fn trigger(app: &App, id: &str) -> AppResult<Trigger> {
    app.store().require(TRIGGERS, id, "trigger")
}

/// A name that is not blank, trimmed.
pub fn require_name(name: &str, what: &str) -> AppResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(AppError::invalid(format!("a {what} needs a name")));
    }
    Ok(name.to_string())
}
