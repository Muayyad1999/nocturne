//! The RPC surface: everything a person (or the assistant) can do with a
//! change.

use std::collections::HashMap;
use std::time::Duration;

use nocturne_app::connect::{self, CliOptions};
use nocturne_app::error::{AppError, AppResult};
use nocturne_app::jobs::{self, JobSpec};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, inventory};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{
    Approval, Change, ChangeState, ChangeSummary, Intent, Options, Origin, Plan, Target,
};
use crate::settings::Settings;
use crate::{apply, autonomy, db, plan, rollback};

// ------------------------------------------------------------------ helpers

fn check_intent(intent: &Intent, device: &str) -> AppResult<()> {
    match intent {
        Intent::Lines { lines } if lines.iter().all(|line| line.trim().is_empty()) => {
            Err(AppError::invalid(format!(
                "the change for `{device}` has no configuration lines"
            )))
        }
        Intent::Intended { config, .. } if config.trim().is_empty() => Err(AppError::invalid(
            format!("the intended configuration for `{device}` is empty"),
        )),
        _ => Ok(()),
    }
}

/// Resolve the devices of a change, by id or by name, and refuse duplicates.
fn resolve_targets(app: &App, inputs: Vec<TargetInput>) -> AppResult<Vec<Target>> {
    if inputs.is_empty() {
        return Err(AppError::invalid("a change needs at least one device"));
    }
    let mut targets: Vec<Target> = Vec::with_capacity(inputs.len());
    for input in inputs {
        let device = inventory::find(app, &input.device_id)?;
        check_intent(&input.intent, &device.name)?;
        if targets.iter().any(|target| target.device_id == device.id) {
            return Err(AppError::invalid(format!(
                "`{}` appears twice in this change; put everything it should get in one entry",
                device.name
            )));
        }
        targets.push(Target {
            device_id: device.id,
            intent: input.intent,
            plan: None,
            result: None,
        });
    }
    Ok(targets)
}

fn editable(change: &Change) -> AppResult<()> {
    if change.state.is_editable() {
        return Ok(());
    }
    Err(AppError::conflict(format!(
        "`{}` is {} and can no longer be edited; create a new change",
        change.title,
        change.state.as_str().replace('_', " ")
    )))
}

// ------------------------------------------------------------------- params

#[derive(Deserialize)]
pub struct TargetInput {
    /// Device id, name or address.
    pub device_id: String,
    pub intent: Intent,
}

#[derive(Deserialize)]
struct CreateParams {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    origin: Option<Origin>,
    #[serde(default)]
    ticket: Option<String>,
    targets: Vec<TargetInput>,
    #[serde(default)]
    options: Option<Options>,
    /// Start planning straight away.
    #[serde(default)]
    plan: bool,
    #[serde(default)]
    created_by: Option<String>,
}

#[derive(Deserialize)]
struct UpdateParams {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    ticket: Option<String>,
    #[serde(default)]
    targets: Option<Vec<TargetInput>>,
    #[serde(default)]
    options: Option<Options>,
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct ApproveParams {
    id: String,
    #[serde(default)]
    plan_hashes: Vec<String>,
    /// Device name → the same name typed out, for every device with a
    /// protected hit.
    #[serde(default)]
    confirm: HashMap<String, String>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    by: Option<String>,
}

#[derive(Deserialize)]
struct RollbackParams {
    id: String,
    #[serde(default)]
    device_ids: Vec<String>,
}

#[derive(Deserialize)]
struct PreviewParams {
    device_id: String,
    intent: Intent,
    #[serde(default)]
    options: Option<Options>,
}

#[derive(Serialize)]
struct Started {
    job_id: String,
}

#[derive(Serialize)]
struct CreatedWithJob {
    change: Change,
    job_id: String,
}

// -------------------------------------------------------------------- plan

/// Plan every target of a change, each over its own session.
fn start_planning(app: &App, change: &mut Change) -> AppResult<Planning> {
    let device_ids: Vec<String> = change
        .targets
        .iter()
        .map(|target| target.device_id.clone())
        .collect();
    let settings = Settings::read(app);
    let lanes = (change.options.parallel.max(1) as usize)
        .min(settings.parallel_limit.max(1) as usize)
        .min(device_ids.len().max(1));
    let change_id = change.id.clone();
    let title = change.title.clone();
    let (go, wait) = tokio::sync::oneshot::channel::<()>();
    let job = jobs::start(
        app,
        JobSpec::new("changes.plan", format!("Planning `{title}`"))
            .devices(&device_ids)
            .origin(match change.origin {
                Origin::Assistant => "assistant",
                _ => "user",
            }),
        move |ctx| async move {
            // The change record is written by whoever started us first; a job
            // that owns the record must never race that write.
            wait.await.map_err(|_| {
                AppError::internal("the change was not stored before planning started")
            })?;
            let app = ctx.app().clone();
            let mut change = db::get(&app, &change_id)?;
            let options = change.options.clone();
            let work: Vec<(usize, String, Intent)> = change
                .targets
                .iter()
                .enumerate()
                .map(|(index, target)| (index, target.device_id.clone(), target.intent.clone()))
                .collect();
            let results = ctx
                .for_each(work, lanes, |ctx, (index, device_id, intent)| {
                    let app = ctx.app().clone();
                    let options = options.clone();
                    async move {
                        let settings = Settings::read(&app);
                        let mut cli = connect::cli(
                            &app,
                            &device_id,
                            CliOptions {
                                wait: Duration::from_secs(
                                    settings.connect_wait_s.clamp(1, 600) as u64
                                ),
                                ..CliOptions::default()
                            },
                        )
                        .await?;
                        let planned = plan::plan_on(&app, &mut cli, &intent, &options).await;
                        cli.close().await;
                        let planned = planned?;
                        ctx.device(
                            &device_id,
                            "info",
                            format!(
                                "{} commands, {} risk{}",
                                planned.commands.len(),
                                planned.risk.level.as_str(),
                                if planned.protected_hits.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        ", {} protected resources",
                                        planned.protected_hits.len()
                                    )
                                }
                            ),
                        );
                        Ok((index, planned))
                    }
                })
                .await;

            let mut failures: Vec<Value> = Vec::new();
            for (result, (index, device_id)) in results.into_iter().zip(
                change
                    .targets
                    .iter()
                    .enumerate()
                    .map(|(index, target)| (index, target.device_id.clone()))
                    .collect::<Vec<_>>(),
            ) {
                match result {
                    Ok((at, planned)) => change.targets[at].plan = Some(planned),
                    Err(err) => {
                        change.targets[index].plan = None;
                        ctx.device(&device_id, "error", err.message.clone());
                        failures.push(json!({"device_id": device_id, "error": err.message}));
                    }
                }
            }
            let planned = change.targets.iter().all(|target| target.plan.is_some());
            change.state = if planned {
                ChangeState::Planned
            } else {
                ChangeState::Draft
            };
            // A new plan is a new thing to approve.
            change.approval = None;
            let summary = db::save(&app, &mut change)?;
            app.audit(
                "changes.plan",
                None,
                format!("Planned change `{}`", change.title),
                json!({
                    "change_id": change.id,
                    "risk": summary.risk,
                    "protected_hits": summary.protected_hits,
                    "failed": failures,
                }),
            );
            if !failures.is_empty() {
                return Err(AppError::invalid(format!(
                    "{} of {} devices could not be planned: {}",
                    failures.len(),
                    change.targets.len(),
                    failures
                        .iter()
                        .filter_map(|failure| failure["error"].as_str())
                        .collect::<Vec<_>>()
                        .join(" | ")
                )));
            }
            Ok(json!({"change_id": change.id, "state": change.state}))
        },
    );
    change.job_id = Some(job.id.clone());
    Ok(Planning { job_id: job.id, go })
}

/// A planning job that is waiting for the change record to be stored.
struct Planning {
    job_id: String,
    go: tokio::sync::oneshot::Sender<()>,
}

// ------------------------------------------------------------------ methods

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;

    router.add("changes.list", |app: App, filter: db::Filter| async move {
        db::list(&app, &filter)
    });

    router.add("changes.get", |app: App, params: IdParams| async move {
        db::get(&app, &params.id)
    });

    router.add(
        "changes.create",
        |app: App, params: CreateParams| async move {
            let title = params.title.trim().to_string();
            if title.is_empty() {
                return Err(AppError::invalid("a change needs a title"));
            }
            let targets = resolve_targets(&app, params.targets)?;
            let origin = params.origin.unwrap_or(Origin::User);
            let now = now_ms();
            let mut change = Change {
                id: new_id(),
                title,
                description: params.description,
                origin,
                state: ChangeState::Draft,
                targets,
                options: params.options.unwrap_or_default(),
                approval: None,
                created_at: now,
                updated_at: now,
                created_by: params.created_by.unwrap_or_else(|| match origin {
                    Origin::Assistant => "assistant".into(),
                    _ => "user".into(),
                }),
                ticket: params.ticket,
                job_id: None,
            };
            let planning = if params.plan {
                Some(start_planning(&app, &mut change)?)
            } else {
                None
            };
            db::save(&app, &mut change)?;
            let job_id = planning.map(|planning| {
                let _ = planning.go.send(());
                planning.job_id
            });
            app.audit(
                "changes.create",
                None,
                format!(
                    "Created change `{}` for {} device(s)",
                    change.title,
                    change.targets.len()
                ),
                json!({"change_id": change.id, "origin": change.origin, "ticket": change.ticket}),
            );
            match job_id {
                Some(job_id) => Ok(serde_json::to_value(CreatedWithJob { change, job_id })?),
                None => Ok(serde_json::to_value(change)?),
            }
        },
    );

    router.add(
        "changes.update",
        |app: App, params: UpdateParams| async move {
            let mut change = db::get(&app, &params.id)?;
            editable(&change)?;
            if let Some(title) = params.title {
                let title = title.trim().to_string();
                if title.is_empty() {
                    return Err(AppError::invalid("a change needs a title"));
                }
                change.title = title;
            }
            if let Some(description) = params.description {
                change.description = description;
            }
            if let Some(ticket) = params.ticket {
                change.ticket = Some(ticket).filter(|ticket| !ticket.trim().is_empty());
            }
            let mut invalidated = false;
            if let Some(targets) = params.targets {
                change.targets = resolve_targets(&app, targets)?;
                invalidated = true;
            }
            if let Some(options) = params.options {
                if options != change.options {
                    invalidated = true;
                }
                change.options = options;
            }
            if invalidated {
                // The plans were computed for the old targets and options, and
                // the approval was given for those plans.
                for target in &mut change.targets {
                    target.plan = None;
                }
                change.approval = None;
                change.state = ChangeState::Draft;
            }
            db::save(&app, &mut change)?;
            app.audit(
                "changes.update",
                None,
                format!("Edited change `{}`", change.title),
                json!({"change_id": change.id, "plans_discarded": invalidated}),
            );
            Ok(change)
        },
    );

    router.add("changes.plan", |app: App, params: IdParams| async move {
        let mut change = db::get(&app, &params.id)?;
        if matches!(change.state, ChangeState::Applying) {
            return Err(AppError::conflict(format!(
                "`{}` is being applied right now",
                change.title
            )));
        }
        // A change that failed or was undone may be planned again — the drift
        // check tells people to do exactly that. One whose devices were
        // changed may not: its results are the record of what happened, and
        // the rollback still needs them.
        if matches!(
            change.state,
            ChangeState::Applied | ChangeState::PartiallyApplied | ChangeState::Cancelled
        ) {
            return Err(AppError::conflict(format!(
                "`{}` is {} and cannot be planned again; create a new change",
                change.title,
                change.state.as_str().replace('_', " ")
            )));
        }
        if change.state.is_finished() {
            for target in &mut change.targets {
                target.result = None;
            }
        }
        let planning = start_planning(&app, &mut change)?;
        change.approval = None;
        db::save(&app, &mut change)?;
        let _ = planning.go.send(());
        Ok(Started {
            job_id: planning.job_id,
        })
    });

    router.add(
        "changes.approve",
        |app: App, params: ApproveParams| async move {
            let mut change = db::get(&app, &params.id)?;
            let settings = Settings::read(&app);
            if change.state != ChangeState::Planned {
                return Err(AppError::conflict(format!(
                    "`{}` is {}; only a planned change can be approved",
                    change.title,
                    change.state.as_str().replace('_', " ")
                )));
            }
            if settings.require_ticket
                && change
                    .ticket
                    .as_ref()
                    .is_none_or(|ticket| ticket.trim().is_empty())
            {
                return Err(AppError::invalid(
                    "this installation requires a change reference (ticket) before a change is approved",
                ));
            }
            let names = db::device_names(&app, &change);
            let mut wanted: Vec<String> = Vec::new();
            let mut confirmed: Vec<String> = Vec::new();
            for target in &change.targets {
                let name = names
                    .get(&target.device_id)
                    .cloned()
                    .unwrap_or_else(|| target.device_id.clone());
                let Some(plan) = &target.plan else {
                    return Err(AppError::conflict(format!(
                        "`{name}` has no plan; plan the change before approving it"
                    )));
                };
                wanted.push(plan.plan_hash.clone());
                if !params.plan_hashes.contains(&plan.plan_hash) {
                    return Err(AppError::conflict(format!(
                        "the plan for `{name}` is not the one being approved — it was planned again after you looked at it. Open the change, read the commands and approve it again."
                    ))
                    .with_details(json!({"device_id": target.device_id, "plan_hash": plan.plan_hash})));
                }
                if !plan.protected_hits.is_empty() {
                    let typed = params
                        .confirm
                        .get(&name)
                        .or_else(|| params.confirm.get(&target.device_id));
                    let ok = typed.is_some_and(|typed| typed.trim().eq_ignore_ascii_case(&name));
                    if !ok {
                        return Err(AppError::forbidden(format!(
                            "this change touches protected resources on `{name}` ({}). To approve it, type the device's name exactly: `{name}`.",
                            plan.protected_hits
                                .iter()
                                .map(|hit| hit.subject.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .with_details(json!({
                            "device_id": target.device_id,
                            "device_name": name,
                            "needs": "typed_confirmation",
                            "protected_hits": plan.protected_hits,
                        })));
                    }
                    confirmed.push(name.clone());
                }
            }
            if let Some(extra) = params
                .plan_hashes
                .iter()
                .find(|hash| !wanted.contains(hash))
            {
                return Err(AppError::invalid(format!(
                    "the approval names a plan that is not part of this change ({extra})"
                )));
            }

            change.approval = Some(Approval {
                by: params.by.unwrap_or_else(|| "user".into()),
                at: now_ms(),
                plan_hashes: wanted,
                note: params.note,
                confirmed_devices: confirmed,
            });
            change.state = ChangeState::Approved;
            db::save(&app, &mut change)?;
            let approval = change.approval.clone().unwrap_or_default();
            app.audit(
                "changes.approve",
                None,
                format!(
                    "Approved change `{}` ({} device(s))",
                    change.title,
                    change.targets.len()
                ),
                json!({
                    "change_id": change.id,
                    "plan_hashes": approval.plan_hashes,
                    "confirmed_devices": approval.confirmed_devices,
                    "note": approval.note,
                }),
            );
            Ok(change)
        },
    );

    router.add(
        "changes.unapprove",
        |app: App, params: IdParams| async move {
            let mut change = db::get(&app, &params.id)?;
            if change.state != ChangeState::Approved {
                return Err(AppError::conflict(format!(
                    "`{}` is not approved",
                    change.title
                )));
            }
            change.approval = None;
            change.state = ChangeState::Planned;
            db::save(&app, &mut change)?;
            app.audit(
                "changes.unapprove",
                None,
                format!("Withdrew the approval of change `{}`", change.title),
                json!({"change_id": change.id}),
            );
            Ok(change)
        },
    );

    router.add("changes.apply", |app: App, params: IdParams| async move {
        let change = db::get(&app, &params.id)?;
        require_approved(&change)?;
        start_apply(&app, change, "user").await
    });

    router.add(
        "changes.assistant_apply",
        |app: App, params: IdParams| async move {
            let mut change = db::get(&app, &params.id)?;
            if change.state == ChangeState::Approved {
                // A human has already signed it off; the assistant may start it.
                return start_apply(&app, change, "assistant").await;
            }
            if change.state != ChangeState::Planned {
                return Err(AppError::conflict(format!(
                    "`{}` is {}; plan it before applying it",
                    change.title,
                    change.state.as_str().replace('_', " ")
                )));
            }
            let by = autonomy::decide(&app, &change)?;
            change.approval = Some(Approval {
                by: by.clone(),
                at: now_ms(),
                plan_hashes: change
                    .targets
                    .iter()
                    .filter_map(|target| target.plan.as_ref().map(|plan| plan.plan_hash.clone()))
                    .collect(),
                note: "applied by the assistant under the autonomy policy".into(),
                confirmed_devices: Vec::new(),
            });
            change.state = ChangeState::Approved;
            db::store(&app, &mut change)?;
            app.audit(
                "changes.approve",
                None,
                format!(
                    "Change `{}` was applied by the assistant under the autonomy policy ({by})",
                    change.title
                ),
                json!({"change_id": change.id, "by": by}),
            );
            start_apply(&app, change, "assistant").await
        },
    );

    router.add(
        "changes.rollback",
        |app: App, params: RollbackParams| async move {
            let mut change = db::get(&app, &params.id)?;
            if change.state == ChangeState::Applying {
                return Err(AppError::conflict(format!(
                    "`{}` is being applied right now; cancel it first",
                    change.title
                )));
            }
            let mut device_ids = Vec::new();
            for reference in &params.device_ids {
                device_ids.push(inventory::find(&app, reference)?.id);
            }
            let candidates = rollback::candidates(&change, &device_ids);
            if candidates.is_empty() {
                return Err(AppError::invalid(format!(
                    "there is nothing to roll back in `{}`: no device of it was applied with a backup of what it had before",
                    change.title
                )));
            }
            let claim = db::claim(&app, &change, "rolled back")?;
            let change_id = change.id.clone();
            let title = change.title.clone();
            let (go, wait) = tokio::sync::oneshot::channel::<()>();
            let job = jobs::start(
                &app,
                JobSpec::new("changes.rollback", format!("Rolling back `{title}`"))
                    .devices(&candidates),
                move |ctx| async move {
                    let claim = claim;
                    wait.await.map_err(|_| {
                        AppError::internal("the change was not stored before the rollback started")
                    })?;
                    let app = ctx.app().clone();
                    let outcome = rollback::run(app, ctx, change_id, device_ids).await;
                    drop(claim);
                    outcome
                },
            );
            change.job_id = Some(job.id.clone());
            db::save(&app, &mut change)?;
            let _ = go.send(());
            Ok(Started { job_id: job.id })
        },
    );

    router.add("changes.cancel", |app: App, params: IdParams| async move {
        let mut change = db::get(&app, &params.id)?;
        match change.state {
            ChangeState::Applying => {
                let job_id = change.job_id.clone().ok_or_else(|| {
                    AppError::internal("the change is being applied but has no job")
                })?;
                jobs::cancel(&app, &job_id)?;
                app.audit(
                    "changes.cancel",
                    None,
                    format!(
                        "Cancelled change `{}` while it was being applied; the device in flight finishes or is rolled back",
                        change.title
                    ),
                    json!({"change_id": change.id}),
                );
                Ok(json!({"cancelled": true, "state": ChangeState::Applying}))
            }
            state if state.is_finished() => Err(AppError::conflict(format!(
                "`{}` is {} and cannot be cancelled",
                change.title,
                state.as_str().replace('_', " ")
            ))),
            _ => {
                change.state = ChangeState::Cancelled;
                change.approval = None;
                db::save(&app, &mut change)?;
                app.audit(
                    "changes.cancel",
                    None,
                    format!("Cancelled change `{}`", change.title),
                    json!({"change_id": change.id}),
                );
                Ok(json!({"cancelled": true, "state": ChangeState::Cancelled}))
            }
        }
    });

    router.add("changes.delete", |app: App, params: IdParams| async move {
        let change = db::get(&app, &params.id)?;
        if change.state == ChangeState::Applying {
            return Err(AppError::conflict(format!(
                "`{}` is being applied; cancel it and wait for it to finish",
                change.title
            )));
        }
        let removed = db::delete(&app, &params.id)?;
        app.audit(
            "changes.delete",
            None,
            format!(
                "Deleted change `{}` ({}, {} device(s))",
                change.title,
                change.state.as_str().replace('_', " "),
                change.targets.len()
            ),
            json!({
                "change_id": change.id,
                "state": change.state,
                "commands": change
                    .targets
                    .iter()
                    .map(|target| json!({
                        "device_id": target.device_id,
                        "commands": target.plan.as_ref().map(|plan| plan.commands.clone()).unwrap_or_default(),
                    }))
                    .collect::<Vec<_>>(),
            }),
        );
        app.publish(
            "changes.changed",
            &json!({"id": change.id, "state": "deleted"}),
        );
        Ok(json!({"deleted": removed}))
    });

    router.add(
        "changes.preview",
        |app: App, params: PreviewParams| async move {
            let device = inventory::find(&app, &params.device_id)?;
            check_intent(&params.intent, &device.name)?;
            let options = params.options.unwrap_or_default();
            let settings = Settings::read(&app);
            let mut cli = connect::cli(
                &app,
                &device.id,
                CliOptions {
                    wait: Duration::from_secs(settings.connect_wait_s.clamp(1, 600) as u64),
                    ..CliOptions::default()
                },
            )
            .await?;
            let planned = plan::plan_on(&app, &mut cli, &params.intent, &options).await;
            cli.close().await;
            planned
        },
    );
}

fn require_approved(change: &Change) -> AppResult<()> {
    if change.state != ChangeState::Approved {
        return Err(AppError::conflict(format!(
            "`{}` is {}; it has to be approved before it can be applied",
            change.title,
            change.state.as_str().replace('_', " ")
        )));
    }
    let approval = change
        .approval
        .as_ref()
        .ok_or_else(|| AppError::conflict("the change is approved but carries no approval"))?;
    for target in &change.targets {
        let plan: &Plan = target.plan.as_ref().ok_or_else(|| {
            AppError::conflict("a device in this change has no plan; plan it again")
        })?;
        if !approval.plan_hashes.contains(&plan.plan_hash) {
            return Err(AppError::conflict(
                "the approval is for a different plan than the one stored now — plan the change again and approve what you see",
            ));
        }
    }
    Ok(())
}

/// Refuse a change to a device type nobody has checked against real hardware.
///
/// Both `docs/profiles.md` and `docs/STATUS.md` have long said a `community`
/// or `experimental` profile "never gets write access by default". Nothing
/// enforced it: `risk.rs` raised the risk level and said why, and
/// `autonomy::decide` held the *assistant* back, but a human could approve
/// and apply. Seven of the nine profiles that ship are `community`, so this
/// was the difference between a documented safety control and a wish.
///
/// Checked here because `start_apply` is the one place both the human and the
/// assistant paths pass through. Deliberately **not** applied to a rollback:
/// undoing a change makes a device safer, and a gate that can stop you
/// putting a device back as you found it is worse than the risk it guards.
fn require_verified_or_allowed(app: &App, change: &Change) -> AppResult<()> {
    if Settings::read(app).allow_unverified_writes {
        return Ok(());
    }
    for target in &change.targets {
        let Some(plan) = target.plan.as_ref() else {
            continue;
        };
        if plan.maturity == "verified" {
            continue;
        }
        // The name, not the id: this is a sentence somebody reads at speed.
        let name = inventory::get(app, &target.device_id)
            .map(|device| device.name)
            .unwrap_or_else(|_| target.device_id.clone());
        return Err(AppError::forbidden(format!(
            "`{}` is a `{}` device type, which has never been checked against real hardware, so Nocturne will not send configuration to `{}`. The risk is that what the profile believes about this platform — which commands only read, how configuration mode is entered, whether the dead-man switch exists — has only ever been checked against recorded output. Turn on `changes.allow_unverified_writes` to apply anyway, or verify the device type first (`docs/profiles.md`).",
            plan.profile, plan.maturity, name
        )));
    }
    Ok(())
}

async fn start_apply(app: &App, mut change: Change, origin: &str) -> AppResult<Started> {
    require_verified_or_allowed(app, &change)?;
    let claim = db::claim(app, &change, "applied")?;
    let device_ids: Vec<String> = change
        .targets
        .iter()
        .map(|target| target.device_id.clone())
        .collect();
    let change_id = change.id.clone();
    let title = change.title.clone();
    let (go, wait) = tokio::sync::oneshot::channel::<()>();
    let job = jobs::start(
        app,
        JobSpec::new("changes.apply", format!("Applying `{title}`"))
            .devices(&device_ids)
            .origin(origin),
        move |ctx| async move {
            let claim = claim;
            // Not a step before the change record says it is being applied:
            // the apply owns that record from here on.
            wait.await.map_err(|_| {
                AppError::internal("the change was not stored before the apply started")
            })?;
            let app = ctx.app().clone();
            let outcome = apply::run(app, ctx, change_id).await;
            drop(claim);
            outcome
        },
    );
    change.state = ChangeState::Applying;
    change.job_id = Some(job.id.clone());
    db::save(app, &mut change)?;
    let _ = go.send(());
    app.audit(
        "changes.apply",
        None,
        format!("Started applying change `{title}`"),
        json!({"change_id": change.id, "job_id": job.id, "origin": origin}),
    );
    Ok(Started { job_id: job.id })
}

/// Kept so the UI's summary type and the backend's cannot drift apart.
pub type Summary = ChangeSummary;
