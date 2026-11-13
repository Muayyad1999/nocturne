//! Undoing a change that has already been applied, on demand.
//!
//! This is not "send the rollback commands and hope": the configuration the
//! device had before the change is still in the version store, so the
//! commands are derived from where the device is *now* back to that version,
//! and the result is checked against it line by line.

use std::time::Duration;

use nocturne_app::connect::{self, CliOptions};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::jobs::JobCtx;
use nocturne_app::util::now_ms;
use nocturne_app::{App, inventory};
use nocturne_drivers::hier;
use serde_json::{Value, json};

use crate::compare;
use crate::db;
use crate::model::{Change, ChangeState, Step, TargetState};
use crate::settings::Settings;

/// Which devices of a change can be rolled back on demand.
pub fn candidates(change: &Change, device_ids: &[String]) -> Vec<String> {
    change
        .targets
        .iter()
        .filter(|target| device_ids.is_empty() || device_ids.contains(&target.device_id))
        .filter(|target| {
            target.result.as_ref().is_some_and(|result| {
                matches!(
                    result.state,
                    TargetState::Applied | TargetState::RollbackFailed | TargetState::GuardFired
                ) && result.before_version_id.is_some()
            })
        })
        .map(|target| target.device_id.clone())
        .collect()
}

pub async fn run(
    app: App,
    ctx: JobCtx,
    change_id: String,
    device_ids: Vec<String>,
) -> AppResult<Value> {
    let settings = Settings::read(&app);
    let mut change = db::get(&app, &change_id)?;
    let wanted = candidates(&change, &device_ids);
    let mut done: Vec<Value> = Vec::new();

    for device_id in wanted {
        ctx.check()?;
        let device = inventory::get(&app, &device_id)?;
        let index = change
            .targets
            .iter()
            .position(|target| target.device_id == device_id);
        let before_id = index
            .and_then(|index| change.targets[index].result.as_ref())
            .and_then(|result| result.before_version_id);
        let Some(before_id) = before_id else {
            continue;
        };
        let (_, before) = app.store().config_get(before_id)?;
        let mut steps: Vec<Step> = Vec::new();
        let mut step = |name: &str, ok: bool, detail: String| {
            app.publish(
                "changes.progress",
                &json!({"change_id": change_id, "device_id": device_id, "step": name, "ok": ok, "detail": detail}),
            );
            ctx.device(
                &device_id,
                if ok { "info" } else { "error" },
                format!("{}: {name} — {detail}", device.name),
            );
            steps.push(Step {
                name: name.to_string(),
                ok,
                detail,
                at: now_ms(),
            });
        };

        let outcome = async {
            let mut cli = connect::cli(
                &app,
                &device_id,
                CliOptions {
                    wait: Duration::from_secs(settings.connect_wait_s.clamp(1, 600) as u64),
                    ..CliOptions::default()
                },
            )
            .await?;
            step(
                "connect",
                true,
                format!("logged in to `{}`", cli.hostname()),
            );
            let syntax = cli.platform.profile.config.syntax;
            let now = cli.session().fetch_config(false).await?;
            let differences = compare::differences_of(&before, &now, syntax);
            if differences.is_empty() {
                step(
                    "rollback",
                    true,
                    "the device is already as it was before the change".into(),
                );
                cli.close().await;
                return Ok::<bool, AppError>(true);
            }
            let back = hier::remediate(&now, &before, syntax);
            if !back.supported || back.commands.is_empty() {
                step(
                    "rollback",
                    false,
                    format!(
                        "no commands could be derived to put `{}` back ({})",
                        device.name,
                        differences.describe(6)
                    ),
                );
                cli.close().await;
                return Ok(false);
            }
            step(
                "rollback",
                true,
                format!(
                    "sending {} commands derived from the device",
                    back.commands.len()
                ),
            );
            let sent = cli.session().configure(&back.commands).await?;
            if let Some(at) = sent.failed_at {
                step(
                    "rollback",
                    false,
                    format!(
                        "the device rejected `{}`",
                        back.commands.get(at).map(String::as_str).unwrap_or("?")
                    ),
                );
            }
            let after = cli.session().fetch_config(false).await?;
            let left = compare::differences_of(&before, &after, syntax);
            let ok = left.is_empty();
            step(
                "verify",
                ok,
                if ok {
                    "the running configuration is back to what it was".into()
                } else {
                    format!("the device is still different — {}", left.describe(8))
                },
            );
            if ok && change.options.save {
                match cli.session().save().await {
                    Ok(_) => step("save", true, "written to the startup configuration".into()),
                    Err(err) => step("save", false, err.to_string()),
                }
            }
            let _ = app.store().config_insert(
                &device_id,
                "running",
                &after,
                "post-change",
                &format!("after rolling back change {change_id}"),
            );
            cli.close().await;
            Ok(ok)
        }
        .await;

        let (state, error) = match outcome {
            Ok(true) => (TargetState::RolledBack, None),
            Ok(false) => (
                TargetState::RollbackFailed,
                Some("the device could not be put back as it was".to_string()),
            ),
            Err(err) => {
                step("rollback", false, err.message.clone());
                (TargetState::RollbackFailed, Some(err.message))
            }
        };
        if let Some(index) = index
            && let Some(result) = change.targets[index].result.as_mut()
        {
            result.state = state;
            result.error = error.clone();
            result.finished_at = Some(now_ms());
            result.steps.extend(steps);
        }
        done.push(json!({"device_id": device_id, "state": state, "error": error}));
        db::store(&app, &mut change)?;
    }

    let states: Vec<TargetState> = change
        .targets
        .iter()
        .filter_map(|target| target.result.as_ref().map(|result| result.state))
        .collect();
    change.state = if states.iter().all(|state| {
        matches!(
            state,
            TargetState::RolledBack | TargetState::Skipped | TargetState::Pending
        )
    }) {
        ChangeState::RolledBack
    } else if states.contains(&TargetState::Applied) {
        ChangeState::PartiallyApplied
    } else {
        ChangeState::Failed
    };
    change.job_id = None;
    db::save(&app, &mut change)?;
    app.audit(
        "changes.rollback",
        None,
        format!("Rolled back change `{}`", change.title),
        json!({"change_id": change.id, "devices": done}),
    );
    Ok(json!({"change_id": change.id, "devices": done}))
}
