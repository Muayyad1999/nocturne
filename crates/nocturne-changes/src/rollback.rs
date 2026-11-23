//! Undoing a change that has already been applied, on demand.
//!
//! An undo is a change like any other. `changes.rollback` sends nothing: it
//! creates a change whose intent, for each device, is the configuration the
//! device had before the change being undone (the version the apply backed
//! up), as an exact intended configuration. The planner works the commands out
//! from what the device holds *now*, because weeks later it has moved on
//! (other changes, somebody at the console) and the way back is new
//! configuration in everything but name. A person reads that plan, approves
//! that exact plan, and the normal pipeline applies it: backup, dead-man
//! switch, send, verify, and roll back if the undo itself does not verify.
//!
//! What is particular to an undo lives here: which devices can be undone,
//! what its plan says about the device having moved on since, and marking
//! the original rolled back once its undo has put the devices back.

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, inventory, profiles};
use nocturne_drivers::hier::ConfigTree;
use serde_json::json;

use crate::compare;
use crate::db;
use crate::model::{
    Change, ChangeState, Intent, Origin, Plan, Step, Target, TargetResult, TargetState,
};
use crate::redact::Hider;

/// A device the change left changed, or may have: one whose own rollback did
/// not finish, or whose dead-man switch was left to fire.
fn changed_by(result: &TargetResult) -> bool {
    matches!(
        result.state,
        TargetState::Applied | TargetState::RollbackFailed | TargetState::GuardFired
    )
}

/// The undo of `original` that has not finished, if there is one. Two would
/// each be planned from the same configuration, and the second applied over
/// the first.
pub fn open_undo(app: &App, original: &Change) -> AppResult<Option<Change>> {
    let changes: Vec<Change> = app.store().list(db::COLLECTION)?;
    Ok(changes.into_iter().find(|change| {
        change.undoes.as_deref() == Some(original.id.as_str()) && !change.state.is_finished()
    }))
}

/// Whether `change` may be planned again as far as undos go, and, when it is
/// an undo planned again, the change it undoes (whose `undone_by` is then to
/// name it). Asked with `ChangesState::undoing` held.
///
/// While an undo of a change waits, the change is not planned again: its
/// results are what that undo goes back from. Nor is another undo of the same
/// change: planning a finished one again would get round the one-undo-at-a-time
/// rule of `changes.rollback`.
pub fn may_plan_again(app: &App, change: &Change) -> AppResult<Option<Change>> {
    // Only a change that has been undone has an undo that can be waiting,
    // and this saves reading every change for any other.
    if change.undone_by.is_some()
        && let Some(open) = open_undo(app, change)?
    {
        return Err(AppError::conflict(format!(
            "`{}` is waiting to undo `{}`: apply it, or cancel it, before planning `{}` again",
            open.title, change.title, change.title
        ))
        .with_details(json!({"change_id": open.id})));
    }
    let Some(original_id) = &change.undoes else {
        return Ok(None);
    };
    let Ok(original) = db::get(app, original_id) else {
        // Deleted since: the undo still takes the devices back, and there is
        // no record left to link.
        return Ok(None);
    };
    if let Some(open) = open_undo(app, &original)?.filter(|open| open.id != change.id) {
        return Err(AppError::conflict(format!(
            "`{}` already undoes `{}` and is {}: apply it, or cancel it, before planning this undo again",
            open.title,
            original.title,
            open.state.as_str().replace('_', " ")
        ))
        .with_details(json!({"change_id": open.id})));
    }
    Ok((original.undone_by.as_deref() != Some(change.id.as_str())).then_some(original))
}

/// A new change (neither stored nor planned) that takes `device_ids` back to
/// the configuration they had before `original` — every device that can be,
/// when `device_ids` is empty: one `original` left changed (applied, its own
/// rollback failed, or its dead-man switch left to fire) and whose
/// configuration from before can still be read. `original` is not an undo
/// (`changes.rollback` refuses that first).
///
/// A device asked for by name that cannot be undone is refused with the
/// reason; one nobody named is left out, and the undo's description says why.
pub fn undo_of(app: &App, original: &Change, device_ids: &[String]) -> AppResult<Change> {
    let names = db::device_names(app, original);
    let name_of = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    for id in device_ids {
        if !original
            .targets
            .iter()
            .any(|target| &target.device_id == id)
        {
            let name = inventory::get(app, id)
                .map(|device| device.name)
                .unwrap_or_else(|_| id.clone());
            return Err(AppError::invalid(format!(
                "`{name}` is not one of the devices of `{}`",
                original.title
            )));
        }
    }

    let mut targets: Vec<Target> = Vec::new();
    let mut restored: Vec<String> = Vec::new();
    let mut left_out: Vec<String> = Vec::new();
    for target in &original.targets {
        let named = device_ids.contains(&target.device_id);
        if !device_ids.is_empty() && !named {
            continue;
        }
        let device = name_of(&target.device_id);
        let unavailable = |why: String| -> AppResult<()> {
            if named {
                Err(AppError::invalid(format!(
                    "`{device}` cannot be undone from `{}`: {why}",
                    original.title
                )))
            } else {
                Ok(())
            }
        };
        let Some(result) = target.result.as_ref().filter(|result| changed_by(result)) else {
            unavailable(format!(
                "the change did not leave it changed ({}), so there is nothing to undo on it",
                target
                    .result
                    .as_ref()
                    .map(|result| result.state.as_str().replace('_', " "))
                    .unwrap_or_else(|| "it was never applied".into())
            ))?;
            continue;
        };
        let Some(before_id) = result.before_version_id else {
            let why = "no backup of the configuration it had before was kept".to_string();
            unavailable(why.clone())?;
            left_out.push(format!("`{device}`: {why}"));
            continue;
        };
        let before = match app.store().config_get(before_id) {
            Ok((_, text)) if !text.trim().is_empty() => text,
            _ => {
                let why = format!(
                    "the configuration it had before (version {before_id}) can no longer be read"
                );
                unavailable(why.clone())?;
                left_out.push(format!("`{device}`: {why}"));
                continue;
            }
        };
        targets.push(Target {
            device_id: target.device_id.clone(),
            intent: Intent::Intended {
                config: before,
                exact: true,
            },
            plan: None,
            result: None,
        });
        restored.push(format!("`{device}` (version {before_id})"));
    }

    if targets.is_empty() {
        return Err(AppError::invalid(format!(
            "there is nothing to roll back in `{}`: no device of it was applied with a backup of what it had before{}",
            original.title,
            if left_out.is_empty() {
                String::new()
            } else {
                format!(" ({})", left_out.join("; "))
            }
        )));
    }
    let mut description = format!(
        "Takes {} back to the configuration each had before `{}`, by commands worked out from what it holds now. Nothing has been sent: read the plan, then approve and apply it like any other change.",
        restored.join(", "),
        original.title
    );
    if !left_out.is_empty() {
        description.push_str(&format!(" Not included — {}.", left_out.join("; ")));
    }
    let now = now_ms();
    Ok(Change {
        id: new_id(),
        title: format!("Undo: {}", original.title),
        description,
        origin: Origin::Restore,
        state: ChangeState::Draft,
        targets,
        // The checks the change had to pass, the way back has to pass too.
        options: original.options.clone(),
        approval: None,
        created_at: now,
        updated_at: now,
        created_by: "user".into(),
        // A back-out is carried out under the change request it backs out.
        ticket: original.ticket.clone(),
        job_id: None,
        undoes: Some(original.id.clone()),
        undone_by: None,
    })
}

/// Up to four lines, named without their values.
fn named(lines: &[String], hide: &Hider) -> String {
    let shown: Vec<String> = lines
        .iter()
        .take(4)
        .map(|line| format!("`{}`", hide.path(line)))
        .collect();
    match lines.len().saturating_sub(shown.len()) {
        0 => shown.join(", "),
        more => format!("{} and {more} more", shown.join(", ")),
    }
}

/// What the plan of an undo says beyond what any plan says: that the device
/// has been changed since the change being undone left it (the way back
/// takes that too), and whether the commands really reach the configuration
/// from before. Both are about the configuration, so the lines they name are
/// named without their values.
pub fn review(app: &App, original_id: &str, device_id: &str, intent: &Intent, plan: &mut Plan) {
    let Intent::Intended { config: before, .. } = intent else {
        return;
    };
    let Ok(original) = db::get(app, original_id) else {
        return;
    };
    let Ok(platform) = profiles::platform(app, &plan.profile) else {
        return;
    };
    let Ok((_, now)) = app.store().config_get(plan.base_version_id) else {
        return;
    };
    let syntax = platform.profile.config.syntax;
    let device = inventory::get(app, device_id)
        .map(|device| device.name)
        .unwrap_or_else(|_| device_id.to_string());
    let hide = Hider::for_platform(&platform);
    hide.learn(&now);
    hide.learn(before);

    let left_by = original
        .targets
        .iter()
        .find(|target| target.device_id == device_id)
        .and_then(|target| target.result.as_ref())
        .and_then(|result| result.after_version_id)
        .and_then(|id| app.store().config_get(id).ok());
    match left_by {
        Some((_, after)) => {
            hide.learn(&after);
            let since = compare::differences_of(&after, &now, syntax);
            if !since.is_empty() {
                let mut moved: Vec<String> = Vec::new();
                if !since.unexpected.is_empty() {
                    moved.push(format!("added since: {}", named(&since.unexpected, &hide)));
                }
                if !since.missing.is_empty() {
                    moved.push(format!("removed since: {}", named(&since.missing, &hide)));
                }
                plan.warnings.push(format!(
                    "`{device}` has changed since `{}` left it ({}). Undoing the change takes that back as well: what this puts back is the configuration from before that change.",
                    original.title,
                    moved.join("; ")
                ));
            }
        }
        None => plan.warnings.push(format!(
            "Nocturne kept no copy of `{device}` as `{}` left it, so it cannot tell what has changed on it since. These commands are the way from what it holds now back to before that change.",
            original.title
        )),
    }

    // Only where the planner could predict the result: elsewhere the apply
    // checks the device against the configuration from before itself.
    if plan.predicted_sha256.is_empty() {
        return;
    }
    let (predicted, _) = crate::plan::predict(&now, &plan.commands, syntax);
    let short = compare::differences(&ConfigTree::parse(before, syntax), &predicted);
    if !short.is_empty() {
        let mut parts: Vec<String> = Vec::new();
        if !short.missing.is_empty() {
            parts.push(format!("still missing: {}", named(&short.missing, &hide)));
        }
        if !short.unexpected.is_empty() {
            parts.push(format!("still there: {}", named(&short.unexpected, &hide)));
        }
        plan.warnings.push(format!(
            "These commands do not put `{device}` back exactly as it was before `{}` ({}). The apply checks it against what they are predicted to do.",
            original.title,
            parts.join("; ")
        ));
    }
}

/// The state of a change once some of its devices have been put back.
fn state_after_undo(change: &Change) -> ChangeState {
    let states: Vec<TargetState> = change
        .targets
        .iter()
        .filter_map(|target| target.result.as_ref().map(|result| result.state))
        .collect();
    if states.iter().all(|state| {
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
    }
}

/// After an undo has been applied: each device it put back reads, in the
/// change it undoes, as rolled back. A device the undo did not put back is
/// left as the original recorded it; the undo's own record says what
/// happened there.
pub fn undone(app: &App, undo: &Change) {
    let Some(original_id) = &undo.undoes else {
        return;
    };
    let state = app.state::<db::ChangesState>();
    let _one_at_a_time = state.undoing.lock();
    let mut original = match db::get(app, original_id) {
        Ok(original) => original,
        // Deleted since: the undo's record is all there is.
        Err(_) => return,
    };
    let mut devices = Vec::new();
    for target in &undo.targets {
        if target.result.as_ref().map(|result| result.state) != Some(TargetState::Applied) {
            continue;
        }
        let Some(kept) = original
            .targets
            .iter_mut()
            .find(|kept| kept.device_id == target.device_id)
            .and_then(|kept| kept.result.as_mut())
            .filter(|kept| changed_by(kept))
        else {
            continue;
        };
        kept.state = TargetState::RolledBack;
        kept.finished_at = Some(now_ms());
        kept.steps.push(Step {
            name: "undone".into(),
            ok: true,
            detail: format!(
                "undone by change `{}` ({}), which put the device back to the configuration it had before this change",
                undo.title, undo.id
            ),
            at: now_ms(),
        });
        devices.push(json!({"device_id": target.device_id, "state": TargetState::RolledBack}));
    }
    if devices.is_empty() {
        return;
    }
    original.state = state_after_undo(&original);
    if let Err(err) = db::save(app, &mut original) {
        tracing::error!(%err, change_id = %original.id, "the change an undo put back could not be marked rolled back");
        return;
    }
    app.audit(
        "changes.rollback",
        None,
        format!(
            "Rolled back change `{}` by applying `{}`",
            original.title, undo.title
        ),
        json!({"change_id": original.id, "undo_id": undo.id, "devices": devices}),
    );
}

/// An undo that is deleted is no longer the original's undo.
pub fn forget(app: &App, undo: &Change) {
    let Some(original_id) = &undo.undoes else {
        return;
    };
    let state = app.state::<db::ChangesState>();
    let _one_at_a_time = state.undoing.lock();
    let Ok(mut original) = db::get(app, original_id) else {
        return;
    };
    if original.undone_by.as_deref() == Some(undo.id.as_str()) {
        original.undone_by = None;
        if let Err(err) = db::save(app, &mut original) {
            tracing::warn!(%err, change_id = %original.id, "the link to a deleted undo could not be removed");
        }
    }
}
