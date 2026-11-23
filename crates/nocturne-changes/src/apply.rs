//! Applying a change to a device, and undoing it when anything goes wrong.
//!
//! The order is the whole point, and it does not vary:
//!
//! connect → drift check → snapshot for verification → backup → arm the
//! dead-man switch → send → settle → verify → (roll back) → disarm → save →
//! final backup.
//!
//! Every step is recorded on the target's result and published as it
//! happens. A device is never left half-configured by Nocturne: a rejected
//! line, a failed check or a lost session all end in a rollback, and the
//! configuration is only ever saved to startup after a verified apply, never
//! while a guard is armed.

use std::time::Duration;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use nocturne_app::connect::{self, Cli, CliOptions};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::jobs::JobCtx;
use nocturne_app::util::now_ms;
use nocturne_app::{App, inventory};
use nocturne_drivers::hier::{self, Syntax};
use nocturne_drivers::session::DriverError;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::compare;
use crate::db;
use crate::model::{
    Change, ChangeState, GuardPlan, Options, Plan, Step, TargetResult, TargetState,
};
use crate::redact::Hider;
use crate::settings::Settings;
use crate::verify;

/// Apply every target of a change, in order, `options.parallel` at a time.
pub async fn run(app: App, ctx: JobCtx, change_id: String) -> AppResult<Value> {
    let mut change = db::get(&app, &change_id)?;
    let options = change.options.clone();
    let settings = Settings::read(&app);
    let parallel = (options.parallel.max(1) as usize).min(settings.parallel_limit.max(1) as usize);

    let planned: Vec<(usize, String, Plan, Option<String>)> = change
        .targets
        .iter()
        .enumerate()
        .filter_map(|(index, target)| {
            target.plan.clone().map(|plan| {
                (
                    index,
                    target.device_id.clone(),
                    plan,
                    verify::exact_configuration(&target.intent),
                )
            })
        })
        .collect();
    if planned.len() != change.targets.len() {
        return Err(AppError::conflict(
            "some devices in this change have no plan; plan the change again before applying it",
        ));
    }
    for target in &mut change.targets {
        target.result = Some(TargetResult {
            state: TargetState::Pending,
            redacted: true,
            ..TargetResult::default()
        });
    }
    db::store(&app, &mut change)?;

    let mut in_flight = FuturesUnordered::new();
    let mut next = 0;
    let mut stop = false;
    let mut cancelled = false;
    loop {
        while !stop && in_flight.len() < parallel && next < planned.len() {
            if ctx.is_cancelled() {
                cancelled = true;
                stop = true;
                break;
            }
            let (index, device_id, plan, exact) = planned[next].clone();
            next += 1;
            let app = app.clone();
            let ctx = ctx.clone();
            let change_id = change_id.clone();
            let options = options.clone();
            let settings = settings.clone();
            in_flight.push(async move {
                let result = apply_device(
                    &app,
                    &ctx,
                    &change_id,
                    &device_id,
                    &plan,
                    exact.as_deref(),
                    &options,
                    &settings,
                )
                .await;
                (index, result)
            });
        }
        let Some((index, result)) = in_flight.next().await else {
            break;
        };
        let failed = result.state != TargetState::Applied;
        let device_id = planned[index].1.clone();
        let plan = &planned[index].2;
        // The audit trail is kept for ever and is read by more people than a
        // change is: it names the lines that were sent without the values in
        // them, and says which steps failed rather than quoting what a
        // device answered. The lines themselves are in the change.
        let hide = Hider::for_profiles(&app, std::slice::from_ref(&plan.profile));
        hide.learn_lines(&plan.commands);
        app.audit(
            "changes.apply",
            Some(&device_id),
            format!(
                "Change `{}` on `{}`: {}",
                change.title,
                inventory::get(&app, &device_id)
                    .map(|device| device.name)
                    .unwrap_or_else(|_| device_id.clone()),
                result.state.as_str().replace('_', " ")
            ),
            json!({
                "change_id": change_id,
                "state": result.state,
                "plan_hash": plan.plan_hash,
                "commands": hide.lines(&plan.commands),
                "failed_steps": failed_steps(&result),
                "before_version_id": result.before_version_id,
                "after_version_id": result.after_version_id,
            }),
        );
        if let Some(target) = change.targets.get_mut(index) {
            target.result = Some(result);
        }
        db::store(&app, &mut change)?;
        app.publish("changes.changed", &db::summary(&app, &change));
        if failed && options.stop_on_failure {
            stop = true;
        }
        if ctx.is_cancelled() {
            cancelled = true;
            stop = true;
        }
    }

    // Whatever was not started is skipped, and says why.
    let reason = if cancelled {
        "the change was cancelled before this device was reached"
    } else {
        "an earlier device failed and the change stops on failure"
    };
    for target in &mut change.targets {
        let pending = target
            .result
            .as_ref()
            .is_none_or(|result| result.state == TargetState::Pending);
        if pending {
            target.result = Some(TargetResult {
                state: TargetState::Skipped,
                started_at: now_ms(),
                finished_at: Some(now_ms()),
                error: Some(reason.to_string()),
                redacted: true,
                ..TargetResult::default()
            });
        }
    }

    change.state = outcome(&change);
    change.job_id = None;
    let summary = db::save(&app, &mut change)?;
    // An undo that put devices back makes the change it undoes read as
    // rolled back on them — cancelled part way or not.
    crate::rollback::undone(&app, &change);
    // The errors were recorded with their values hidden; the job's result
    // carries them, the audit entry says only which steps failed.
    let states: Vec<Value> = change
        .targets
        .iter()
        .map(|target| {
            json!({
                "device_id": target.device_id,
                "state": target.result.as_ref().map(|result| result.state),
                "error": target.result.as_ref().and_then(|result| result.error.clone()),
            })
        })
        .collect();
    let audited: Vec<Value> = change
        .targets
        .iter()
        .map(|target| {
            json!({
                "device_id": target.device_id,
                "state": target.result.as_ref().map(|result| result.state),
                "failed_steps": target.result.as_ref().map(failed_steps).unwrap_or_default(),
            })
        })
        .collect();
    app.audit(
        "changes.apply",
        None,
        format!(
            "Applied change `{}`: {}",
            change.title,
            change.state.as_str().replace('_', " ")
        ),
        json!({"change_id": change.id, "state": change.state, "devices": audited}),
    );
    if cancelled {
        // The record is written; the job itself is reported as cancelled so
        // that nobody reads "succeeded" for something they stopped.
        return Err(nocturne_app::jobs::cancelled());
    }
    Ok(json!({"change_id": change.id, "state": summary.state, "devices": states}))
}

/// The steps of a device's result that did not go well, by name and in
/// order, each once.
fn failed_steps(result: &TargetResult) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for step in result.steps.iter().filter(|step| !step.ok) {
        if !names.contains(&step.name) {
            names.push(step.name.clone());
        }
    }
    names
}

/// The state of the change once every device has been dealt with.
fn outcome(change: &Change) -> ChangeState {
    let states: Vec<TargetState> = change
        .targets
        .iter()
        .map(|target| {
            target
                .result
                .as_ref()
                .map(|result| result.state)
                .unwrap_or(TargetState::Skipped)
        })
        .collect();
    let applied = states
        .iter()
        .filter(|state| **state == TargetState::Applied)
        .count();
    let undone = states
        .iter()
        .filter(|state| **state == TargetState::RolledBack)
        .count();
    let cancelled_only = states
        .iter()
        .all(|state| matches!(state, TargetState::Skipped));
    if cancelled_only {
        return ChangeState::Cancelled;
    }
    if applied == states.len() {
        return ChangeState::Applied;
    }
    if applied > 0 {
        return ChangeState::PartiallyApplied;
    }
    if undone
        + states
            .iter()
            .filter(|state| **state == TargetState::Skipped)
            .count()
        == states.len()
    {
        return ChangeState::RolledBack;
    }
    ChangeState::Failed
}

// ----------------------------------------------------------------- one device

/// Records what happens on one device: steps, job log and live events.
///
/// Everything that is recorded here is kept in the change, and most of it is
/// published and logged as it happens, so it is recorded with the secret
/// values hidden (`hide`): a detail quotes the line a device rejected or the
/// lines that differ from the prediction, an error quotes what the device
/// answered, and any of those can be a key.
struct Run<'a> {
    app: &'a App,
    ctx: &'a JobCtx,
    change_id: &'a str,
    device_id: String,
    device_name: String,
    result: TargetResult,
    hide: Hider,
}

impl Run<'_> {
    fn step(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = self.hide.text(&detail.into());
        self.result.steps.push(Step {
            name: name.to_string(),
            ok,
            detail: detail.clone(),
            at: now_ms(),
        });
        self.app.publish(
            "changes.progress",
            &json!({
                "change_id": self.change_id,
                "device_id": self.device_id,
                "step": name,
                "ok": ok,
                "detail": detail,
            }),
        );
        self.ctx.device(
            &self.device_id,
            if ok { "info" } else { "error" },
            format!("{}: {name} — {detail}", self.device_name),
        );
    }

    fn fail(&mut self, state: TargetState, error: impl Into<String>) {
        self.result.state = state;
        self.result.error = Some(self.hide.text(&error.into()));
        self.result.finished_at = Some(now_ms());
    }

    /// Keep the lines that were sent, each with the device's answer.
    fn sent(&mut self, lines: &[nocturne_drivers::session::CommandOutput]) {
        let hidden: Vec<_> = lines.iter().map(|line| self.hide.sent(line)).collect();
        self.result.sent.extend(hidden);
    }

    fn done(&mut self, state: TargetState) {
        self.result.state = state;
        self.result.finished_at = Some(now_ms());
    }
}

fn sha256(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

/// Was the session lost, rather than the command refused?
fn session_lost(err: &DriverError) -> bool {
    matches!(
        err,
        DriverError::Closed { .. } | DriverError::Channel(_) | DriverError::Timeout { .. }
    )
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
async fn apply_device(
    app: &App,
    ctx: &JobCtx,
    change_id: &str,
    device_id: &str,
    plan: &Plan,
    exact: Option<&str>,
    options: &Options,
    settings: &Settings,
) -> TargetResult {
    let device = match inventory::get(app, device_id) {
        Ok(device) => device,
        Err(err) => {
            return TargetResult {
                state: TargetState::Failed,
                started_at: now_ms(),
                finished_at: Some(now_ms()),
                error: Some(Hider::for_patterns(&[]).text(&err.message)),
                redacted: true,
                ..TargetResult::default()
            };
        }
    };
    let mut run = Run {
        app,
        ctx,
        change_id,
        device_id: device_id.to_string(),
        device_name: device.name.clone(),
        result: TargetResult {
            state: TargetState::Applying,
            started_at: now_ms(),
            // Everything `Run` adds to it goes through `hide`.
            redacted: true,
            ..TargetResult::default()
        },
        // Until the session says what the device really is.
        hide: Hider::for_profiles(app, std::slice::from_ref(&device.profile)),
    };

    // 1. Connect.
    let mut cli = match connect::cli(
        app,
        device_id,
        CliOptions {
            wait: Duration::from_secs(settings.connect_wait_s.clamp(1, 600) as u64),
            ..CliOptions::default()
        },
    )
    .await
    {
        Ok(cli) => {
            run.step(
                "connect",
                true,
                format!("logged in to `{}`", cli.hostname()),
            );
            cli
        }
        Err(err) => {
            run.step("connect", false, err.message.clone());
            run.fail(TargetState::Failed, err.message);
            return run.result;
        }
    };
    let syntax = cli.platform.profile.config.syntax;
    run.hide = Hider::for_platform(&cli.platform);
    run.hide.learn_lines(&plan.commands);
    run.hide.learn_lines(&plan.rollback);

    // 2. Drift check: the plan is only valid for the configuration it was made against.
    let before = match cli.session().fetch_config(false).await {
        Ok(text) => text,
        Err(err) => {
            run.step("drift check", false, err.to_string());
            run.fail(TargetState::Failed, err.to_string());
            finish(&mut run, cli, settings).await;
            return run.result;
        }
    };
    run.hide.learn(&before);
    if sha256(&before) != plan.base_sha256 {
        let message = format!(
            "the configuration of `{}` changed since this was planned — plan again, look at what is different, and approve the new plan",
            device.name
        );
        run.step("drift check", false, message.clone());
        // Keep what the device looks like now: that is what the operator has to look at.
        if let Ok((version, _)) = app.store().config_insert(
            device_id,
            "running",
            &before,
            "pre-change",
            "found by a drift check before a change",
        ) {
            run.result.before_version_id = Some(version.id);
        }
        run.fail(TargetState::Failed, message);
        finish(&mut run, cli, settings).await;
        return run.result;
    }
    run.step(
        "drift check",
        true,
        "the running configuration is still the one the plan was computed against",
    );

    // 3. Snapshot what verification will compare against.
    let baseline = verify::snapshot(app, &mut cli, &options.verify).await;
    let wanted = options.verify.neighbors || options.verify.interfaces || options.verify.routes;
    run.step(
        "snapshot",
        baseline.notes.is_empty(),
        match (wanted, baseline.notes.is_empty()) {
            (false, _) => "nothing to read: this change checks neither neighbours, interfaces nor routes afterwards".to_string(),
            (true, true) => format!(
                "read what this change will be checked against ({})",
                [
                    options.verify.neighbors.then_some("neighbours"),
                    options.verify.interfaces.then_some("interfaces"),
                    options.verify.routes.then_some("routes"),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(", ")
            ),
            (true, false) => baseline.notes.join("; "),
        },
    );

    // 4. Backup, pinned so retention cannot remove what a rollback needs.
    match app.store().config_insert(
        device_id,
        "running",
        &before,
        "pre-change",
        &format!("before change {change_id}"),
    ) {
        Ok((version, _)) => {
            run.result.before_version_id = Some(version.id);
            pin(app, version.id, change_id).await;
            run.step(
                "backup",
                true,
                format!("kept the running configuration as version {}", version.id),
            );
        }
        Err(err) => {
            run.step("backup", false, err.message.clone());
            run.fail(
                TargetState::Failed,
                format!(
                    "the change was not sent: the backup failed ({})",
                    err.message
                ),
            );
            finish(&mut run, cli, settings).await;
            return run.result;
        }
    }

    // 5. Arm the dead-man switch before anything is sent.
    let mut armed = false;
    if let Some(guard) = &plan.guard {
        match arm(&mut cli, guard).await {
            Ok(detail) => {
                armed = true;
                run.step("arm guard", true, detail);
            }
            Err(message) => {
                run.step("arm guard", false, message.clone());
                run.fail(
                    TargetState::Failed,
                    format!(
                        "nothing was sent: the {} could not be armed ({message}), and this change needs one",
                        guard.name
                    ),
                );
                finish(&mut run, cli, settings).await;
                return run.result;
            }
        }
    }

    // 6. Send.
    let sent = cli.session().configure(&plan.commands).await;
    let rejected = match sent {
        Ok(result) => {
            run.sent(&result.lines);
            match (result.failed_at, result.commit_error.clone()) {
                (None, None) => {
                    run.step(
                        "apply",
                        true,
                        format!("{} commands accepted", result.lines.len()),
                    );
                    None
                }
                (failed_at, commit_error) => {
                    run.result.failed_line = failed_at;
                    let message = match (failed_at, commit_error) {
                        (Some(at), _) => format!(
                            "the device rejected `{}`: {}",
                            plan.commands.get(at).map(String::as_str).unwrap_or("?"),
                            result
                                .lines
                                .get(at)
                                .and_then(|line| line.error.clone())
                                .unwrap_or_else(|| "no reason given".into())
                        ),
                        (None, Some(error)) => format!("the device refused the change: {error}"),
                        _ => "the device refused the change".to_string(),
                    };
                    run.step("apply", false, message.clone());
                    Some(message)
                }
            }
        }
        Err(err) if session_lost(&err) => {
            run.step("apply", false, err.to_string());
            return lost(
                run,
                cli,
                app,
                &device.name,
                plan,
                &before,
                settings,
                syntax,
                armed,
                err,
            )
            .await;
        }
        Err(err) => {
            run.step("apply", false, err.to_string());
            Some(err.to_string())
        }
    };

    // 7. Settle and verify — unless the device already refused the change.
    let mut failure = rejected;
    if failure.is_none() {
        let settle = if device.lab.is_some() {
            0
        } else {
            options.verify.settle_s
        };
        if settle > 0 {
            tokio::time::sleep(Duration::from_secs(settle as u64)).await;
        }
        match cli.session().fetch_config(false).await {
            Ok(after) => {
                // What the device holds now may be somebody else's doing,
                // and is as secret as what it held before.
                run.hide.learn(&after);
                let checks = verify::run_checks(
                    app,
                    &mut cli,
                    plan,
                    exact,
                    &options.verify,
                    &baseline,
                    &before,
                    &after,
                    syntax,
                    &run.hide,
                )
                .await;
                let bad: Vec<String> = checks
                    .iter()
                    .filter(|check| !check.ok)
                    .map(|check| format!("{}: {}", check.name, check.detail))
                    .collect();
                run.result.verification = checks;
                if bad.is_empty() {
                    run.step(
                        "verify",
                        true,
                        format!("{} checks passed", run.result.verification.len()),
                    );
                    run.result.after_version_id = store_after(app, device_id, &after, change_id);
                } else {
                    run.step("verify", false, bad.join(" | "));
                    failure = Some(bad.join(" | "));
                }
            }
            Err(err) if session_lost(&err) => {
                run.step("verify", false, err.to_string());
                return lost(
                    run,
                    cli,
                    app,
                    &device.name,
                    plan,
                    &before,
                    settings,
                    syntax,
                    armed,
                    err,
                )
                .await;
            }
            Err(err) => {
                run.step("verify", false, err.to_string());
                failure = Some(err.to_string());
            }
        }
    }

    // 8. Anything wrong: undo it.
    if let Some(message) = failure {
        let restored = roll_back(&mut run, &mut cli, plan, &before, syntax).await;
        if armed {
            disarm(&mut run, &mut cli, plan).await;
        }
        run.fail(
            if restored {
                TargetState::RolledBack
            } else {
                TargetState::RollbackFailed
            },
            message,
        );
        finish(&mut run, cli, settings).await;
        return run.result;
    }

    // 9. Disarm, then save — never the other way round.
    let disarmed = if armed {
        disarm(&mut run, &mut cli, plan).await
    } else {
        true
    };
    if options.save {
        if !disarmed {
            run.step(
                "save",
                false,
                "not saved: the dead-man switch is still armed, and saving now would make the change survive the reload it will cause",
            );
        } else {
            match cli.session().save().await {
                Ok(outputs) => {
                    let error = outputs.iter().find_map(|output| output.error.clone());
                    match error {
                        None => run.step("save", true, "written to the startup configuration"),
                        Some(error) => run.step("save", false, error),
                    }
                }
                Err(err) => run.step("save", false, err.to_string()),
            }
        }
    }
    match run.result.after_version_id {
        Some(id) => run.step(
            "final backup",
            true,
            format!("kept the configuration after the change as version {id}"),
        ),
        None => run.step(
            "final backup",
            false,
            "the configuration after the change could not be stored",
        ),
    }
    run.done(TargetState::Applied);
    finish(&mut run, cli, settings).await;
    run.result
}

/// Keep the configuration as it is after a verified change.
fn store_after(app: &App, device_id: &str, after: &str, change_id: &str) -> Option<i64> {
    match app.store().config_insert(
        device_id,
        "running",
        after,
        "post-change",
        &format!("after change {change_id}"),
    ) {
        Ok((version, _)) => Some(version.id),
        Err(err) => {
            tracing::warn!(%err, device_id, "the post-change backup could not be stored");
            None
        }
    }
}

/// Ask the configuration feature to keep this version, if it is in the build.
async fn pin(app: &App, version_id: i64, change_id: &str) {
    let _ = app
        .rpc(
            "config.pin",
            json!({"version_id": version_id, "reason": format!("needed to roll back change {change_id}")}),
        )
        .await;
}

async fn arm(cli: &mut Cli, guard: &GuardPlan) -> Result<String, String> {
    if guard.arm.is_empty() {
        return Err("the device type defines no command to arm it".into());
    }
    for command in &guard.arm {
        let output = cli
            .session()
            .exec(command)
            .await
            .map_err(|err| err.to_string())?;
        if let Some(error) = output.error {
            return Err(format!("`{command}`: {error}"));
        }
    }
    Ok(format!(
        "{} armed for {} minutes — if this change cuts the session, {}",
        guard.name, guard.minutes, guard.effect
    ))
}

/// Cancel the dead-man switch. Returns whether it is really disarmed.
async fn disarm(run: &mut Run<'_>, cli: &mut Cli, plan: &Plan) -> bool {
    let Some(guard) = &plan.guard else {
        return true;
    };
    for command in &guard.disarm {
        match cli.session().exec(command).await {
            Ok(output) => {
                if let Some(error) = output.error {
                    run.step(
                        "disarm guard",
                        false,
                        format!("`{command}`: {error} — {} is still armed", guard.name),
                    );
                    return false;
                }
            }
            Err(err) => {
                run.step(
                    "disarm guard",
                    false,
                    format!("{err} — {} is still armed", guard.name),
                );
                return false;
            }
        }
    }
    run.step("disarm guard", true, format!("{} cancelled", guard.name));
    true
}

/// Send the rollback and check that the device really is back where it was.
async fn roll_back(
    run: &mut Run<'_>,
    cli: &mut Cli,
    plan: &Plan,
    before: &str,
    syntax: Syntax,
) -> bool {
    if plan.rollback.is_empty() {
        // Nothing was derived at planning time; derive it from where we are now.
        run.step(
            "rollback",
            false,
            "the plan carries no rollback commands; working out what to undo from the device",
        );
    } else {
        match cli.session().configure(&plan.rollback).await {
            Ok(result) => {
                run.sent(&result.lines);
                if let Some(at) = result.failed_at {
                    run.step(
                        "rollback",
                        false,
                        format!(
                            "the device rejected `{}` while undoing the change",
                            plan.rollback.get(at).map(String::as_str).unwrap_or("?")
                        ),
                    );
                }
            }
            Err(err) => {
                run.step("rollback", false, err.to_string());
                return false;
            }
        }
    }

    // Did it actually come back?
    for attempt in 0..2 {
        let now = match cli.session().fetch_config(false).await {
            Ok(text) => text,
            Err(err) => {
                run.step(
                    "rollback",
                    false,
                    format!("the configuration could not be read back: {err}"),
                );
                return false;
            }
        };
        run.hide.learn(&now);
        let differences = compare::differences_of(before, &now, syntax);
        if differences.is_empty() {
            run.step(
                "rollback",
                true,
                "the running configuration is back to what it was before the change",
            );
            return true;
        }
        if attempt == 1 {
            let remaining = differences.describe(8, &run.hide);
            run.step(
                "rollback",
                false,
                format!("the device is still not what it was before the change — {remaining}"),
            );
            return false;
        }
        // Second try: ask the engine for the difference from here.
        let second = hier::remediate(&now, before, syntax);
        if !second.supported || second.commands.is_empty() {
            let remaining = differences.describe(8, &run.hide);
            run.step(
                "rollback",
                false,
                format!(
                    "the device is not what it was before the change and no further commands could be derived — {remaining}"
                ),
            );
            return false;
        }
        run.hide.learn_lines(&second.commands);
        run.step(
            "rollback",
            true,
            format!(
                "{} more commands derived from the device to finish undoing the change",
                second.commands.len()
            ),
        );
        match cli.session().configure(&second.commands).await {
            Ok(result) => run.sent(&result.lines),
            Err(err) => {
                run.step("rollback", false, err.to_string());
                return false;
            }
        }
    }
    false
}

/// The session died in the middle of the change: reconnect and undo it, or
/// report what the device is about to do by itself.
#[allow(clippy::too_many_arguments)]
async fn lost(
    mut run: Run<'_>,
    mut cli: Cli,
    app: &App,
    device_name: &str,
    plan: &Plan,
    before: &str,
    settings: &Settings,
    syntax: Syntax,
    armed: bool,
    err: DriverError,
) -> TargetResult {
    // Let go of the session (and the device's lock) before trying again.
    take_transcript(&mut run, &mut cli, settings);
    drop(cli);
    let device_id = run.device_id.clone();
    let attempts = settings.reconnect_attempts.clamp(1, 10);
    let interval = Duration::from_secs(settings.reconnect_interval_s.clamp(1, 120) as u64);
    let mut reconnected = None;
    for attempt in 1..=attempts {
        tokio::time::sleep(interval).await;
        match connect::cli(
            app,
            &device_id,
            CliOptions {
                wait: Duration::from_secs(settings.connect_wait_s.clamp(1, 600) as u64),
                ..CliOptions::default()
            },
        )
        .await
        {
            Ok(cli) => {
                run.step(
                    "reconnect",
                    true,
                    format!("reached `{device_name}` again on attempt {attempt}"),
                );
                reconnected = Some(cli);
                break;
            }
            Err(error) => run.step(
                "reconnect",
                false,
                format!("attempt {attempt} of {attempts}: {}", error.message),
            ),
        }
    }

    let Some(mut cli) = reconnected else {
        if armed {
            let guard = plan.guard.as_ref();
            let message = format!(
                "`{device_name}` could not be reached again after the session was lost ({err}). Its {} will fire in up to {} minutes: {}. Nothing has been saved, so the device comes back as it was.",
                guard
                    .map(|guard| guard.name.as_str())
                    .unwrap_or("dead-man switch"),
                guard.map(|guard| guard.minutes).unwrap_or_default(),
                guard
                    .map(|guard| guard.effect.as_str())
                    .unwrap_or("it undoes the change by itself"),
            );
            run.step("guard", true, message.clone());
            run.fail(TargetState::GuardFired, message);
        } else {
            let message = format!(
                "the session to `{device_name}` was lost while the change was being made ({err}), the device is unreachable and no dead-man switch was armed. Check it: its configuration may be half-changed."
            );
            run.fail(TargetState::Failed, message);
        }
        return run.result;
    };

    let restored = roll_back(&mut run, &mut cli, plan, before, syntax).await;
    if armed {
        disarm(&mut run, &mut cli, plan).await;
    }
    run.fail(
        if restored {
            TargetState::RolledBack
        } else {
            TargetState::RollbackFailed
        },
        format!(
            "the session was lost while the change was being made ({err}); `{device_name}` was reached again and {}",
            if restored {
                "put back as it was"
            } else {
                "could not be put back completely"
            }
        ),
    );
    finish(&mut run, cli, settings).await;
    run.result
}

/// Take the (redacted) transcript off a session.
fn take_transcript(run: &mut Run<'_>, cli: &mut Cli, settings: &Settings) {
    if !settings.keep_transcripts {
        return;
    }
    let text = cli.take_transcript();
    let mut redacted = run.hide.transcript(&text);
    const LIMIT: usize = 200_000;
    if redacted.len() > LIMIT {
        let cut = redacted
            .char_indices()
            .nth(LIMIT)
            .map(|(at, _)| at)
            .unwrap_or(redacted.len());
        redacted.truncate(cut);
        redacted.push_str("\n… (transcript truncated)\n");
    }
    run.result.transcript.push_str(&redacted);
}

async fn finish(run: &mut Run<'_>, mut cli: Cli, settings: &Settings) {
    take_transcript(run, &mut cli, settings);
    cli.close().await;
}
