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
use crate::redact::{self, ChangeView, Hider};
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
        _ => refuse_placeholders(intent, device),
    }
}

/// A change is read with placeholders where its secret values are, and a
/// screen (or a model) that sends those lines back would have the
/// placeholder itself configured as the community or the key. What is sent
/// to a device is exactly what is given here, so this is where it stops.
///
/// Hand-written lines are held to both forms. An intended configuration is
/// held to the numbered form only, which no device prints: one that was read
/// from a device that masks its own secrets may say `<hidden>` on lines that
/// are not going to change, and the planner refuses a `<hidden>` in the lines
/// it would actually send.
fn refuse_placeholders(intent: &Intent, device: &str) -> AppResult<()> {
    let copied = match intent {
        Intent::Lines { lines } => lines.iter().any(|line| redact::has_placeholder(line)),
        Intent::Intended { config, .. } => redact::has_numbered_placeholder(config),
    };
    if !copied {
        return Ok(());
    }
    Err(AppError::invalid(format!(
        "the configuration for `{device}` contains a redaction placeholder (`<secret:…>` or `<hidden>`): it was copied from a view that hides secret values. \
         Read the change with `redact: false` and edit that, so that the real values are sent."
    )))
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
struct GetParams {
    id: String,
    /// `false`: the lines exactly as they will be sent. Audited.
    #[serde(default)]
    redact: Option<bool>,
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
    /// `false`: the lines exactly as they would be sent. Audited.
    #[serde(default)]
    redact: Option<bool>,
}

#[derive(Serialize)]
struct Started {
    job_id: String,
}

#[derive(Serialize)]
struct CreatedWithJob {
    change: ChangeView,
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
            let undoes = change.undoes.clone();
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
                    let undoes = undoes.clone();
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
                        let mut planned = planned?;
                        if let Some(original) = &undoes {
                            rollback::review(&app, original, &device_id, &intent, &mut planned);
                        }
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
            // For the audit trail: which device and what kind of failure, not
            // the words of it.
            let mut failed: Vec<Value> = Vec::new();
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
                        // A refusal quotes the line it refuses, and a failed
                        // read can quote what the device printed last.
                        let message = Hider::for_device(&app, &device_id).text(&err.message);
                        ctx.device(&device_id, "error", message.clone());
                        failures.push(json!({"device_id": device_id, "error": message}));
                        failed.push(json!({"device_id": device_id, "code": err.code}));
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
                    "failed": failed,
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

    router.add("changes.get", |app: App, params: GetParams| async move {
        let change = db::get(&app, &params.id)?;
        Ok(if params.redact.unwrap_or(true) {
            redact::change(&app, &change)
        } else {
            redact::change_unredacted(&app, &change)
        })
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
                // Only `changes.rollback` makes an undo: its lines are the
                // engine's, never the caller's.
                undoes: None,
                undone_by: None,
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
            let change = redact::change(&app, &change);
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
            if params.targets.is_some() && change.undoes.is_some() {
                // What makes it an undo is that its lines are the engine's,
                // worked out from the configuration stored before the change
                // it undoes; edited by hand it would be some other change.
                return Err(AppError::invalid(format!(
                    "`{}` is an undo: what it puts on each device is the configuration that device had before the change it undoes, and that is not replaced by hand. Create a new change for anything else.",
                    change.title
                )));
            }
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
            Ok(redact::change(&app, &change))
        },
    );

    router.add("changes.plan", |app: App, params: IdParams| async move {
        // Whether a change may be planned again depends on the undos there
        // are, which are created under this lock.
        let state = app.state::<db::ChangesState>();
        let _undoing = state.undoing.lock();
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
        let relink = rollback::may_plan_again(&app, &change)?;
        if change.state.is_finished() {
            for target in &mut change.targets {
                target.result = None;
            }
            // Its undo undid what those results recorded. Planned again, the
            // change starts over, and a screen must not say it was undone.
            change.undone_by = None;
            // With its results gone it is a draft until its plan is in — and
            // an undo planned again counts as waiting from now, not from
            // when the planning finishes.
            change.state = ChangeState::Draft;
        }
        let planning = start_planning(&app, &mut change)?;
        change.approval = None;
        db::save(&app, &mut change)?;
        if let Some(mut original) = relink {
            original.undone_by = Some(change.id.clone());
            db::save(&app, &mut original)?;
        }
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
                        // The hits name the commands that touch what is
                        // protected, and a command that touches a login is
                        // a command with a secret in it.
                        let hide = Hider::for_profiles(&app, std::slice::from_ref(&plan.profile));
                        let hits = hide.hits(&plan.protected_hits);
                        return Err(AppError::forbidden(format!(
                            "this change touches protected resources on `{name}` ({}). To approve it, type the device's name exactly: `{name}`.",
                            hits.iter()
                                .map(|hit| hit.subject.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .with_details(json!({
                            "device_id": target.device_id,
                            "device_name": name,
                            "needs": "typed_confirmation",
                            "protected_hits": hits,
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
            Ok(redact::change(&app, &change))
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
            Ok(redact::change(&app, &change))
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
            let approved = match change.state {
                ChangeState::Approved => {
                    require_approved(&change)?;
                    true
                }
                ChangeState::Planned => false,
                state => {
                    return Err(AppError::conflict(format!(
                        "`{}` is {}; plan it before applying it",
                        change.title,
                        state.as_str().replace('_', " ")
                    )));
                }
            };
            // Asked of an approved change too: a person's approval is consent
            // to the plan, not leave for the assistant to start it, and it
            // grants no more than the autonomy setting does. The refusal says
            // why in the plan's own words.
            let by = autonomy::decide(&app, &change)
                .map_err(|err| redact::hider_for(&app, &change).error(err))?;
            if approved {
                // The approval stays the person's; `start_apply` records that
                // the assistant is the one who started it.
                return start_apply(&app, change, "assistant").await;
            }
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
        // Sends nothing. The way back is a new change, planned from what the
        // devices hold now and applied only once a person approves that plan.
        |app: App, params: RollbackParams| async move {
            let mut device_ids = Vec::new();
            for reference in &params.device_ids {
                device_ids.push(inventory::find(&app, reference)?.id);
            }
            let (original, undo, job_id) = {
                let state = app.state::<db::ChangesState>();
                let _one_at_a_time = state.undoing.lock();
                // Read with the lock held: an undo of it that finishes in the
                // meantime marks this record rolled back, and the link written
                // below must not write an older copy over that.
                let mut original = db::get(&app, &params.id)?;
                if original.state == ChangeState::Applying {
                    return Err(AppError::conflict(format!(
                        "`{}` is being applied right now; cancel it first",
                        original.title
                    )));
                }
                if let Some(undone) = &original.undoes {
                    // Undoing an undo would make the change it undid again,
                    // under an undo's exemption from `allow_unverified_writes`
                    // and with that change's record still reading rolled back.
                    let undone = db::get(&app, undone).map_or_else(
                        |_| "the change it undid".to_string(),
                        |change| format!("`{}`", change.title),
                    );
                    return Err(AppError::invalid(format!(
                        "`{}` is itself an undo, and an undo is not undone in turn. To have {undone} back, plan it again (or create a change for what the devices should hold): it is planned from what they hold now and approved like any other change.",
                        original.title
                    )));
                }
                if let Some(open) = rollback::open_undo(&app, &original)? {
                    return Err(AppError::conflict(format!(
                        "`{}` already undoes `{}` and is {}: apply it, or cancel it before asking for another",
                        open.title,
                        original.title,
                        open.state.as_str().replace('_', " ")
                    ))
                    .with_details(json!({"change_id": open.id})));
                }
                let mut undo = rollback::undo_of(&app, &original, &device_ids)?;
                let planning = start_planning(&app, &mut undo)?;
                db::save(&app, &mut undo)?;
                original.undone_by = Some(undo.id.clone());
                db::save(&app, &mut original)?;
                let _ = planning.go.send(());
                (original, undo, planning.job_id)
            };
            let names = db::device_names(&app, &undo);
            let devices: Vec<Value> = undo
                .targets
                .iter()
                .map(|target| {
                    json!({
                        "device_id": target.device_id,
                        "before_version_id": original
                            .targets
                            .iter()
                            .find(|kept| kept.device_id == target.device_id)
                            .and_then(|kept| kept.result.as_ref())
                            .and_then(|result| result.before_version_id),
                    })
                })
                .collect();
            app.audit(
                "changes.create",
                None,
                format!(
                    "Created change `{}` for {} device(s)",
                    undo.title,
                    undo.targets.len()
                ),
                json!({"change_id": undo.id, "origin": undo.origin, "ticket": undo.ticket, "undoes": original.id}),
            );
            app.audit(
                "changes.rollback",
                None,
                format!(
                    "Asked to undo change `{}` on {}: created change `{}` to do it, from which nothing has been sent",
                    original.title,
                    undo.targets
                        .iter()
                        .map(|target| format!(
                            "`{}`",
                            names.get(&target.device_id).cloned().unwrap_or_default()
                        ))
                        .collect::<Vec<_>>()
                        .join(", "),
                    undo.title
                ),
                json!({"change_id": original.id, "undo_id": undo.id, "devices": devices}),
            );
            Ok(serde_json::to_value(CreatedWithJob {
                change: redact::change(&app, &undo),
                job_id,
            })?)
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
        rollback::forget(&app, &change);
        app.audit(
            "changes.delete",
            None,
            format!(
                "Deleted change `{}` ({}, {} device(s))",
                change.title,
                change.state.as_str().replace('_', " "),
                change.targets.len()
            ),
            // The record that the change existed and what it sent outlives
            // the change, in a trail that is never edited: the lines are
            // named there without the values in them.
            json!({
                "change_id": change.id,
                "state": change.state,
                "undoes": change.undoes,
                "commands": change
                    .targets
                    .iter()
                    .map(|target| {
                        let hide = redact::hider_for_target(&app, target);
                        json!({
                            "device_id": target.device_id,
                            "plan_hash": target.plan.as_ref().map(|plan| plan.plan_hash.clone()),
                            "commands": target
                                .plan
                                .as_ref()
                                .map(|plan| hide.lines(&plan.commands))
                                .unwrap_or_default(),
                        })
                    })
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
            .await
            // As in a planning job: a failed login can quote what the device
            // printed last.
            .map_err(|err| Hider::for_device(&app, &device.id).error(err))?;
            let planned = plan::plan_on(&app, &mut cli, &params.intent, &options).await;
            cli.close().await;
            Ok(redact::plan(
                &app,
                &device.id,
                &device.name,
                planned?,
                params.redact.unwrap_or(true),
            ))
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
/// assistant paths pass through. Deliberately **not** applied to an undo
/// (`changes.rollback`): undoing a change makes a device safer, and a gate
/// that can stop you putting a device back as you found it is worse than the
/// risk it guards. An undo's lines are the engine's, worked out from the
/// configuration stored before the change, and cannot be edited, so the
/// exemption covers nothing else. The assistant is still held to the device
/// type by `autonomy::decide`.
fn require_verified_or_allowed(app: &App, change: &Change) -> AppResult<()> {
    if change.undoes.is_some() || Settings::read(app).allow_unverified_writes {
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
    // Who pressed start and who signed the plan are two facts: the assistant
    // may start a change a person approved, when the policy allows it.
    let approved_by = change
        .approval
        .as_ref()
        .map(|approval| approval.by.clone())
        .unwrap_or_default();
    app.audit(
        "changes.apply",
        None,
        if origin == "assistant" {
            format!("The assistant started applying change `{title}` (approved by {approved_by})")
        } else {
            format!("Started applying change `{title}` (approved by {approved_by})")
        },
        json!({"change_id": change.id, "job_id": job.id, "origin": origin, "approved_by": approved_by}),
    );
    Ok(Started { job_id: job.id })
}

/// Kept so the UI's summary type and the backend's cannot drift apart.
pub type Summary = ChangeSummary;
