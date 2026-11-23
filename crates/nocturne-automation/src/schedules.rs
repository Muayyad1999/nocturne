//! Schedules: one background task, an allow-list, and no overlapping runs.
//!
//! The allow-list is the point. A schedule calls an RPC method by name, and
//! the list of names it may call contains nothing that changes a device — not
//! `changes.apply`, not now and not later. A schedule can produce a change
//! (through a playbook or a template); a human still approves it.

use std::time::Duration;

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::jobs;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, NoParams};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{ActionCall, Schedule, When};
use crate::settings::Settings;
use crate::state::AutomationState;
use crate::{cron, db};

/// Methods a schedule or a trigger may call.
///
/// Everything here reads, collects, reports, or produces a change for a human
/// to approve. Nothing here configures a device, and nothing that configures a
/// device is ever added: that is what the change pipeline is for.
pub const ALLOWED: &[&str] = &[
    "config.backup",
    "compliance.run",
    "collect.run",
    "discovery.scan",
    "runner.run",
    "playbooks.run",
    "reports.generate",
    "insights.scan",
    "tasks.run",
];

/// How often the scheduler looks for something due.
const TICK: Duration = Duration::from_secs(1);

/// What a run that was going when the process ended is recorded as, on the
/// next start. Its outcome was never known; it is not a success.
pub const INTERRUPTED: &str = "Nocturne was closed while this was running";

pub fn check_action(app: &App, action: &ActionCall) -> AppResult<()> {
    let method = action.method.trim();
    if method.is_empty() {
        return Err(AppError::invalid("a schedule needs a method to call"));
    }
    if !ALLOWED.contains(&method) {
        return Err(AppError::forbidden(format!(
            "`{method}` cannot be put on a schedule. A schedule may call: {}. Anything that \
             changes a device goes through a change, which a person approves.",
            ALLOWED.join(", ")
        )));
    }
    if !app.methods().contains(&method) {
        return Err(AppError::invalid(format!(
            "`{method}` is not part of this build, so a schedule calling it would never work"
        )));
    }
    if !action.params.is_null() && !action.params.is_object() {
        return Err(AppError::invalid("`params` must be a JSON object"));
    }
    Ok(())
}

fn validated(app: &App, schedule: &Schedule) -> AppResult<()> {
    cron::check(&schedule.when).map_err(AppError::invalid)?;
    check_action(app, &schedule.action)
}

/// When a schedule runs next, from what is stored: `None` while it is off.
fn next_run(schedule: &Schedule, now: i64) -> Option<i64> {
    if !schedule.enabled {
        return None;
    }
    cron::next_after(&schedule.when, now, schedule.last_run_at)
}

/// Run a schedule's action and wait for the job it started, if it started one.
///
/// Waiting matters: `runner.run` returns a job id at once, and without the
/// wait the next tick would happily start the same schedule again on the same
/// devices.
///
/// The slot is spent before anything is called: the stored schedule moves on
/// to its next run and records this one as started, with no outcome yet. A
/// process that dies while the action runs (out of memory, a power cut) then
/// comes back to a schedule that is not due, instead of running the same thing
/// again on every start. When the run ends its outcome is written onto the
/// schedule as it is stored *then*: one deleted while it ran stays deleted, and
/// one edited or switched off keeps the edit.
///
/// `by_hand` is `schedules.run_now`. Otherwise the scheduler decided from what
/// it read at its tick, so the stored record has to agree that it is still on
/// and still due: switched off a moment ago, it does not run.
async fn fire(app: App, id: String, origin: &'static str, by_hand: bool) -> AppResult<Value> {
    let state = app.state::<AutomationState>();
    let started = state.now_ms();
    let schedule = {
        let _records = state.records();
        let mut schedule = db::schedule(&app, &id)?;
        let due = schedule.next_run_at.is_some_and(|due| due <= started);
        if !by_hand && !(schedule.enabled && due) {
            return Ok(Value::Null);
        }
        schedule.last_run_at = Some(started);
        schedule.last_job_id = None;
        schedule.last_ok = None;
        schedule.last_error = None;
        schedule.next_run_at = next_run(&schedule, started);
        app.store().put(db::SCHEDULES, &schedule.id, &schedule)?;
        schedule
    };
    app.publish("schedules.changed", &schedule);

    let mut params = match schedule.action.params.clone() {
        Value::Object(object) => object,
        _ => serde_json::Map::new(),
    };
    params
        .entry("origin".to_string())
        .or_insert_with(|| json!(origin));
    let result = app
        .rpc(&schedule.action.method, Value::Object(params))
        .await;

    let job_id = result
        .as_ref()
        .ok()
        .and_then(|value| value.get("job_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    app.publish(
        "schedules.fired",
        &json!({"id": schedule.id, "job_id": job_id, "at": started}),
    );

    let outcome = match (&result, &job_id) {
        (Ok(_), Some(job_id)) => match jobs::wait(&app, job_id).await {
            Ok(job) => match job.state {
                jobs::JobState::Succeeded => (true, None),
                jobs::JobState::Cancelled => (false, Some("the run was cancelled".to_string())),
                _ => (
                    false,
                    Some(job.error.unwrap_or_else(|| "the run failed".into())),
                ),
            },
            Err(err) => (false, Some(err.message)),
        },
        (Ok(_), None) => (true, None),
        (Err(err), _) => (false, Some(err.message.clone())),
    };

    let stored = {
        let _records = state.records();
        match app.store().get::<Schedule>(db::SCHEDULES, &id)? {
            Some(mut stored) => {
                stored.last_run_at = Some(started);
                stored.last_job_id = job_id;
                stored.last_ok = Some(outcome.0);
                stored.last_error = outcome.1;
                // Counted from now, not from the slot that was missed: a laptop
                // that was shut for a week does not owe seven backups.
                stored.next_run_at = next_run(&stored, state.now_ms());
                app.store().put(db::SCHEDULES, &stored.id, &stored)?;
                Some(stored)
            }
            None => None,
        }
    };
    if let Some(stored) = stored {
        app.publish("schedules.changed", &stored);
    }
    result
}

/// The background task.
async fn scheduler(app: App) {
    let state = app.state::<AutomationState>();
    loop {
        tokio::time::sleep(TICK).await;
        let settings = Settings::load(&app);
        if !settings.schedules_enabled {
            continue;
        }
        let now = state.now_ms();
        let Ok(schedules) = db::schedules(&app) else {
            continue;
        };
        for schedule in schedules {
            if !schedule.enabled {
                continue;
            }
            let Some(due) = schedule.next_run_at else {
                continue;
            };
            if due > now {
                continue;
            }
            // Too long ago to be worth catching up on: move on to the next slot.
            let missed_by = (now - due) / 1000;
            if missed_by > settings.catch_up_within_s as i64 {
                let moved = {
                    let _records = state.records();
                    // Onto what is stored now: an edit since this tick's read
                    // has already worked out a slot of its own.
                    match db::schedule(&app, &schedule.id) {
                        Ok(mut stored) if stored.next_run_at == schedule.next_run_at => {
                            stored.next_run_at = if stored.enabled {
                                cron::next_after(&stored.when, now, None)
                            } else {
                                None
                            };
                            app.store()
                                .put(db::SCHEDULES, &stored.id, &stored)
                                .ok()
                                .map(|_| stored)
                        }
                        _ => None,
                    }
                };
                if let Some(moved) = moved {
                    app.publish("schedules.changed", &moved);
                }
                continue;
            }
            // The previous run has not finished: this tick does nothing, and
            // the schedule stays due, so it starts as soon as that one ends.
            let Some(guard) = state.start_firing(&schedule.id) else {
                continue;
            };
            let app = app.clone();
            tokio::spawn(async move {
                let _guard = guard;
                let name = schedule.name.clone();
                if let Err(err) = fire(app, schedule.id, "schedule", false).await {
                    tracing::warn!(schedule = %name, %err, "a scheduled action failed");
                }
            });
        }
    }
}

// --------------------------------------------------------------------- rpc

#[derive(Debug, Deserialize)]
struct SaveParams {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    when: When,
    action: ActionCall,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
struct PreviewParams {
    when: When,
    #[serde(default)]
    count: Option<usize>,
}

#[derive(Serialize)]
struct Preview {
    next: Vec<i64>,
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("schedules.list", |app: App, _: NoParams| async move {
        db::schedules(&app)
    });

    router.add(
        "schedules.save",
        |app: App, params: SaveParams| async move {
            let name = db::require_name(&params.name, "schedule")?;
            let state = app.state::<AutomationState>();
            let records = state.records();
            let existing = params
                .id
                .as_ref()
                .filter(|id| !id.trim().is_empty())
                .and_then(|id| db::schedule(&app, id.trim()).ok());
            let mut schedule = Schedule {
                id: existing
                    .as_ref()
                    .map(|schedule| schedule.id.clone())
                    .unwrap_or_else(new_id),
                name,
                enabled: params.enabled,
                when: params.when,
                action: params.action,
                created_at: existing
                    .as_ref()
                    .map(|schedule| schedule.created_at)
                    .unwrap_or_else(now_ms),
                last_run_at: existing.as_ref().and_then(|schedule| schedule.last_run_at),
                last_job_id: existing
                    .as_ref()
                    .and_then(|schedule| schedule.last_job_id.clone()),
                last_ok: existing.as_ref().and_then(|schedule| schedule.last_ok),
                last_error: existing
                    .as_ref()
                    .and_then(|schedule| schedule.last_error.clone()),
                next_run_at: None,
            };
            validated(&app, &schedule)?;
            schedule.next_run_at = next_run(&schedule, state.now_ms());
            app.store().put(db::SCHEDULES, &schedule.id, &schedule)?;
            drop(records);
            app.audit(
                "schedules.save",
                None,
                format!(
                    "Saved the schedule `{}` ({})",
                    schedule.name, schedule.action.method
                ),
                json!({"schedule_id": schedule.id, "enabled": schedule.enabled}),
            );
            app.publish("schedules.changed", &schedule);
            Ok(schedule)
        },
    );

    router.add(
        "schedules.delete",
        |app: App, params: IdParams| async move {
            let schedule = {
                let state = app.state::<AutomationState>();
                let _records = state.records();
                let schedule = db::schedule(&app, &params.id)?;
                app.store().delete(db::SCHEDULES, &params.id)?;
                schedule
            };
            app.audit(
                "schedules.delete",
                None,
                format!("Deleted the schedule `{}`", schedule.name),
                json!({"schedule_id": params.id}),
            );
            app.publish(
                "schedules.changed",
                &json!({"id": params.id, "deleted": true}),
            );
            Ok(json!({"deleted": true}))
        },
    );

    router.add(
        "schedules.run_now",
        |app: App, params: IdParams| async move {
            let schedule = db::schedule(&app, &params.id)?;
            validated(&app, &schedule)?;
            let state = app.state::<AutomationState>();
            let guard = state.start_firing(&schedule.id).ok_or_else(|| {
                AppError::conflict(format!(
                    "`{}` is still running from its last start",
                    schedule.name
                ))
            })?;
            let _guard = guard;
            app.audit(
                "schedules.run_now",
                None,
                format!("Ran the schedule `{}` by hand", schedule.name),
                json!({"schedule_id": schedule.id}),
            );
            fire(app.clone(), schedule.id, "schedule", true).await
        },
    );

    router.add(
        "schedules.preview",
        |_app: App, params: PreviewParams| async move {
            cron::check(&params.when).map_err(AppError::invalid)?;
            Ok(Preview {
                next: cron::preview(&params.when, now_ms(), params.count.unwrap_or(5)),
            })
        },
    );
}

pub fn spawn(app: &App) {
    // Schedules that were due while the process was not running are left due:
    // the first tick either runs them once or, if they were missed by too
    // much, moves them on. Either way it happens once. A run that was going
    // when the process ended has no outcome to wait for: nothing is running
    // yet, so one recorded as started and never finished was cut short.
    if let Ok(schedules) = db::schedules(app) {
        let state = app.state::<AutomationState>();
        for mut schedule in schedules {
            let mut changed = false;
            if schedule.last_run_at.is_some() && schedule.last_ok.is_none() {
                schedule.last_ok = Some(false);
                schedule.last_error = Some(INTERRUPTED.to_string());
                changed = true;
            }
            if schedule.enabled && schedule.next_run_at.is_none() {
                schedule.next_run_at =
                    cron::next_after(&schedule.when, state.now_ms(), schedule.last_run_at);
                changed = true;
            }
            if changed {
                let _ = app.store().put(db::SCHEDULES, &schedule.id, &schedule);
            }
        }
    }
    app.spawn("automation.scheduler", scheduler(app.clone()));
}

/// Shared with the trigger runner: both honour the same allow-list.
pub async fn call_action(
    app: &App,
    id: &str,
    name: &str,
    action: &ActionCall,
    origin: &'static str,
    extra: Option<Value>,
) -> AppResult<(Option<String>, AppResult<Value>)> {
    check_action(app, action)?;
    let mut params = match action.params.clone() {
        Value::Object(object) => object,
        _ => serde_json::Map::new(),
    };
    if let Some(Value::Object(extra)) = extra {
        for (key, value) in extra {
            params.insert(key, value);
        }
    }
    params
        .entry("origin".to_string())
        .or_insert_with(|| json!(origin));
    let result = app.rpc(&action.method, Value::Object(params)).await;
    let job_id = result
        .as_ref()
        .ok()
        .and_then(|value| value.get("job_id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    tracing::debug!(id, name, origin, "automation action called");
    Ok((job_id, result))
}

/// Wait for the job an action started, so "still running" means something.
pub async fn wait_for(app: &App, job_id: &Option<String>) {
    if let Some(job_id) = job_id {
        let _ = jobs::wait(app, job_id).await;
    }
}
