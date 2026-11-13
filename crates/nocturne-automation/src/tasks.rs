//! Tasks: a saved "what to run, and where".
//!
//! A task holds a selector, not a list of devices, so it keeps meaning what it
//! said as the network changes. `tasks.run` is the single name a schedule or a
//! trigger points at, which is why it is the only method on the allow-list
//! that leads to the rest of this crate.

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, NoParams, jobs};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::model::{Action, Selector, Task};
use crate::state::AutomationState;
use crate::{ansible, db, playbooks, runner, select, templates};

fn check(app: &App, task: &Task) -> AppResult<()> {
    if task.select.is_empty() {
        return Err(AppError::invalid(
            "a task needs devices: name them (`device_ids`) or say how to find them (`filter`)",
        ));
    }
    match &task.action {
        Action::Commands { commands, .. } => {
            if commands.iter().all(|command| command.trim().is_empty()) {
                return Err(AppError::invalid("a command task needs commands"));
            }
        }
        Action::Playbook {
            playbook_id, yaml, ..
        } => match (playbook_id, yaml) {
            (Some(id), _) if !id.trim().is_empty() => {
                db::playbook(app, id.trim())?;
            }
            (_, Some(yaml)) if !yaml.trim().is_empty() => {
                let checked = playbooks::validate(app, yaml);
                if !checked.ok {
                    return Err(AppError::invalid(format!(
                        "the playbook does not validate: {}",
                        checked
                            .problems
                            .iter()
                            .map(|problem| problem.message.clone())
                            .collect::<Vec<_>>()
                            .join("; ")
                    )));
                }
            }
            _ => {
                return Err(AppError::invalid(
                    "a playbook task needs a `playbook_id` or a `yaml`",
                ));
            }
        },
        Action::Template { template_id, .. } => {
            templates::get(app, template_id)?;
        }
        Action::Ansible { playbook_path, .. } => {
            if playbook_path.trim().is_empty() {
                return Err(AppError::invalid("an Ansible task needs a playbook path"));
            }
        }
    }
    Ok(())
}

/// Run a task now. `override_select`, when given, replaces the task's own
/// selector — which is how a trigger passes the device the event was about.
pub async fn run(
    app: &App,
    task: &Task,
    origin: &str,
    dry_run: bool,
    override_select: Option<Selector>,
) -> AppResult<Value> {
    let select = override_select.unwrap_or_else(|| task.select.clone());
    // Resolve here as well as inside, so a task whose filter now matches
    // nothing fails with that reason rather than starting an empty job.
    select::resolve(app, &select)?;

    let started = match &task.action {
        Action::Commands { commands, parse } => runner::start_command_run(
            app,
            runner::RunParams {
                select,
                commands: commands.clone(),
                name: Some(task.name.clone()),
                parse: *parse,
                dry_run,
                origin: Some(origin.to_string()),
                task_id: Some(task.id.clone()),
            },
        )?,
        Action::Playbook {
            playbook_id,
            yaml,
            vars,
        } => playbooks::start(
            app,
            playbooks::RunParams {
                playbook_id: playbook_id.clone(),
                yaml: yaml.clone(),
                select,
                vars: vars.clone(),
                dry_run,
                origin: Some(origin.to_string()),
                task_id: Some(task.id.clone()),
            },
        )?,
        Action::Template { template_id, vars } => {
            templates::apply(
                app,
                templates::ApplyParams {
                    template_id: template_id.clone(),
                    select,
                    vars: vars.clone(),
                    title: Some(task.name.clone()),
                    dry_run,
                    origin: Some(origin.to_string()),
                    task_id: Some(task.id.clone()),
                },
            )
            .await?
        }
        Action::Ansible {
            playbook_path,
            check,
            extra_args,
        } => {
            ansible::run(
                app,
                ansible::RunParams {
                    playbook_path: playbook_path.clone(),
                    select,
                    limit: None,
                    check: *check,
                    extra_args: extra_args.clone(),
                    origin: Some(origin.to_string()),
                    task_id: Some(task.id.clone()),
                    dry_run,
                },
            )
            .await?
        }
    };

    // A template task returns a change id, not a job: nothing to follow.
    let job_id = started
        .get("job_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    let run_id = started
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    if !dry_run {
        let mut task = task.clone();
        let state = app.state::<AutomationState>();
        task.last_run_at = Some(state.now_ms());
        task.last_run_id = run_id.clone();
        task.last_ok = None;
        app.store().put(db::TASKS, &task.id, &task)?;
        app.publish("tasks.changed", &task);
        if let Some(job_id) = job_id.clone() {
            // Record the outcome when it is known, without holding the caller.
            let app = app.clone();
            tokio::spawn(async move {
                let job = jobs::wait(&app, &job_id).await;
                let ok = job
                    .as_ref()
                    .map(|job| job.state == jobs::JobState::Succeeded)
                    .unwrap_or(false);
                let ok = ok
                    && run_id
                        .as_ref()
                        .and_then(|id| db::get_run(&app, id).ok())
                        .map(|run| run.failed == 0 && run.unreached == 0)
                        .unwrap_or(true);
                if let Ok(mut stored) = db::task(&app, &task.id) {
                    stored.last_ok = Some(ok);
                    let _ = app.store().put(db::TASKS, &stored.id, &stored);
                    app.publish("tasks.changed", &stored);
                }
            });
        }
    }
    Ok(started)
}

// --------------------------------------------------------------------- rpc

#[derive(Debug, Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Debug, Deserialize)]
struct SaveParams {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    select: Selector,
    action: Action,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RunNowParams {
    id: String,
    #[serde(default)]
    dry_run: bool,
    /// Narrow this one run — a trigger uses it to pass the device the event
    /// was about.
    #[serde(default)]
    device_ids: Option<Vec<String>>,
    #[serde(default)]
    origin: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PreviewParams {
    #[serde(default)]
    id: Option<String>,
    #[serde(flatten)]
    select: Selector,
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("tasks.list", |app: App, _: NoParams| async move {
        db::tasks(&app)
    });

    router.add("tasks.get", |app: App, params: IdParams| async move {
        db::task(&app, &params.id)
    });

    router.add("tasks.save", |app: App, params: SaveParams| async move {
        let name = db::require_name(&params.name, "task")?;
        let existing = params
            .id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
            .and_then(|id| db::task(&app, id.trim()).ok());
        let task = Task {
            id: existing
                .as_ref()
                .map(|task| task.id.clone())
                .unwrap_or_else(new_id),
            name,
            description: params.description,
            select: params.select,
            action: params.action,
            tags: params.tags,
            created_at: existing
                .as_ref()
                .map(|task| task.created_at)
                .unwrap_or_else(now_ms),
            updated_at: now_ms(),
            last_run_at: existing.as_ref().and_then(|task| task.last_run_at),
            last_run_id: existing.as_ref().and_then(|task| task.last_run_id.clone()),
            last_ok: existing.as_ref().and_then(|task| task.last_ok),
        };
        check(&app, &task)?;
        app.store().put(db::TASKS, &task.id, &task)?;
        app.audit(
            "tasks.save",
            None,
            format!("Saved the automation task `{}`", task.name),
            json!({"task_id": task.id}),
        );
        app.publish("tasks.changed", &task);
        Ok(task)
    });

    router.add("tasks.delete", |app: App, params: IdParams| async move {
        let task = db::task(&app, &params.id)?;
        app.store().delete(db::TASKS, &params.id)?;
        app.audit(
            "tasks.delete",
            None,
            format!("Deleted the automation task `{}`", task.name),
            json!({"task_id": params.id}),
        );
        app.publish("tasks.changed", &json!({"id": params.id, "deleted": true}));
        Ok(json!({"deleted": true}))
    });

    router.add("tasks.run", |app: App, params: RunNowParams| async move {
        let task = db::task(&app, &params.id)?;
        check(&app, &task)?;
        let override_select = params.device_ids.filter(|ids| !ids.is_empty()).map(|ids| {
            // Keep the task's filter out of it: the caller named devices.
            Selector::ids(&ids)
        });
        let origin = params.origin.unwrap_or_else(|| "user".into());
        app.audit(
            "tasks.run",
            None,
            format!(
                "Ran the automation task `{}`{}",
                task.name,
                if params.dry_run { " as a dry run" } else { "" }
            ),
            json!({"task_id": task.id, "origin": origin, "dry_run": params.dry_run}),
        );
        run(&app, &task, &origin, params.dry_run, override_select).await
    });

    // Which devices a task would run on, right now.
    router.add(
        "tasks.devices",
        |app: App, params: PreviewParams| async move {
            let select = match &params.id {
                Some(id) if !id.trim().is_empty() => db::task(&app, id.trim())?.select,
                _ => params.select,
            };
            let devices = select::resolve(&app, &select)?;
            Ok(json!({
                "count": devices.len(),
                "devices": devices
                    .iter()
                    .map(|device| json!({
                        "device_id": device.id,
                        "name": device.name,
                        "address": device.address,
                        "profile": device.profile,
                        "site": device.site,
                        "role": device.role,
                        "protected": device.protected,
                    }))
                    .collect::<Vec<_>>(),
            }))
        },
    );
}
