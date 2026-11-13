//! Where changes are kept, how they are summarised, and the lock that keeps
//! two applies off one device.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::now_ms;
use nocturne_app::{App, inventory};
use parking_lot::Mutex;
use serde::Deserialize;

use crate::model::{Change, ChangeState, ChangeSummary, DeviceSummary, Origin, RiskLevel};

pub const COLLECTION: &str = "changes";

pub fn get(app: &App, id: &str) -> AppResult<Change> {
    app.store().require(COLLECTION, id, "change")
}

/// Write the change back, stamp it and tell everyone.
pub fn save(app: &App, change: &mut Change) -> AppResult<ChangeSummary> {
    change.updated_at = now_ms();
    app.store().put(COLLECTION, &change.id, change)?;
    let summary = summary(app, change);
    app.publish("changes.changed", &summary);
    Ok(summary)
}

/// Write the change back without announcing it (for intermediate steps of an
/// apply, which announce their own progress).
pub fn store(app: &App, change: &mut Change) -> AppResult<()> {
    change.updated_at = now_ms();
    app.store().put(COLLECTION, &change.id, change)
}

pub fn delete(app: &App, id: &str) -> AppResult<bool> {
    app.store().delete(COLLECTION, id)
}

/// The names of a change's devices, with a placeholder for ones that have
/// since been deleted from the inventory.
pub fn device_names(app: &App, change: &Change) -> HashMap<String, String> {
    change
        .targets
        .iter()
        .map(|target| {
            let name = inventory::get(app, &target.device_id)
                .map(|device| device.name)
                .unwrap_or_else(|_| format!("deleted device {}", target.device_id));
            (target.device_id.clone(), name)
        })
        .collect()
}

pub fn summary(app: &App, change: &Change) -> ChangeSummary {
    let names = device_names(app, change);
    let devices: Vec<DeviceSummary> = change
        .targets
        .iter()
        .map(|target| DeviceSummary {
            device_id: target.device_id.clone(),
            name: names
                .get(&target.device_id)
                .cloned()
                .unwrap_or_else(|| target.device_id.clone()),
            planned: target.plan.is_some(),
            risk: target.plan.as_ref().map(|plan| plan.risk.level),
            protected_hits: target
                .plan
                .as_ref()
                .map(|plan| plan.protected_hits.len())
                .unwrap_or(0),
            commands: target
                .plan
                .as_ref()
                .map(|plan| plan.commands.len())
                .unwrap_or(0),
            state: target.result.as_ref().map(|result| result.state),
        })
        .collect();
    ChangeSummary {
        id: change.id.clone(),
        title: change.title.clone(),
        description: change.description.clone(),
        origin: change.origin,
        state: change.state,
        ticket: change.ticket.clone(),
        created_at: change.created_at,
        updated_at: change.updated_at,
        created_by: change.created_by.clone(),
        job_id: change.job_id.clone(),
        risk: devices.iter().filter_map(|device| device.risk).max(),
        protected_hits: devices.iter().map(|device| device.protected_hits).sum(),
        approved_by: change.approval.as_ref().map(|approval| approval.by.clone()),
        devices,
    }
}

/// Worst risk over the planned targets, if every target is planned.
pub fn overall_risk(change: &Change) -> Option<RiskLevel> {
    change
        .targets
        .iter()
        .map(|target| target.plan.as_ref().map(|plan| plan.risk.level))
        .collect::<Option<Vec<_>>>()
        .and_then(|levels| levels.into_iter().max())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Filter {
    pub state: Option<String>,
    pub device_id: Option<String>,
    pub origin: Option<Origin>,
    pub limit: Option<usize>,
    /// The id of the last change of the previous page.
    pub cursor: Option<String>,
}

/// Newest first, after `cursor`, at most `limit`.
pub fn list(app: &App, filter: &Filter) -> AppResult<Vec<ChangeSummary>> {
    let mut changes: Vec<Change> = app.store().list(COLLECTION)?;
    changes.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    let mut rows: Vec<&Change> = changes
        .iter()
        .filter(|change| match filter.state.as_deref() {
            None | Some("") => true,
            Some("open") => !change.state.is_finished(),
            Some(state) => change.state.as_str() == state,
        })
        .filter(|change| match &filter.device_id {
            None => true,
            Some(id) => change.targets.iter().any(|target| &target.device_id == id),
        })
        .filter(|change| match filter.origin {
            None => true,
            Some(origin) => change.origin == origin,
        })
        .collect();
    if let Some(cursor) = &filter.cursor {
        match rows.iter().position(|change| &change.id == cursor) {
            Some(at) => rows.drain(..=at).for_each(drop),
            None => {
                return Err(AppError::invalid(
                    "the page cursor does not name a change in this list; start from the beginning",
                ));
            }
        }
    }
    rows.truncate(filter.limit.unwrap_or(50).clamp(1, 500));
    Ok(rows
        .into_iter()
        .map(|change| summary(app, change))
        .collect())
}

// -------------------------------------------------------------------- locks

/// Which changes and devices an apply is holding right now. Two applies of
/// one change, or two changes touching one device, must never run at once:
/// the second one is refused with the name of the change that is in the way,
/// rather than queued behind a device session it would wait minutes for.
#[derive(Default)]
pub struct ChangesState {
    busy: Mutex<Busy>,
}

#[derive(Default)]
struct Busy {
    changes: HashSet<String>,
    /// device id → the change applying to it.
    devices: HashMap<String, String>,
}

/// Releases the claim when it is dropped, whatever ended the apply.
pub struct Claim {
    state: Arc<ChangesState>,
    change_id: String,
    devices: Vec<String>,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut busy = self.state.busy.lock();
        busy.changes.remove(&self.change_id);
        for device in &self.devices {
            busy.devices.remove(device);
        }
    }
}

/// Take the change and its devices, or say who has them.
pub fn claim(app: &App, change: &Change, what: &str) -> AppResult<Claim> {
    let state = app.state::<ChangesState>();
    let devices: Vec<String> = change
        .targets
        .iter()
        .map(|target| target.device_id.clone())
        .collect();
    let mut busy = state.busy.lock();
    if busy.changes.contains(&change.id) {
        return Err(AppError::conflict(format!(
            "`{}` is already being {what}; wait for it to finish",
            change.title
        )));
    }
    if let Some((device, holder)) = devices
        .iter()
        .find_map(|id| busy.devices.get(id).map(|holder| (id, holder.clone())))
    {
        let name = inventory::get(app, device)
            .map(|device| device.name)
            .unwrap_or_else(|_| device.clone());
        let title = app
            .store()
            .get::<Change>(COLLECTION, &holder)
            .ok()
            .flatten()
            .map(|change| change.title)
            .unwrap_or_else(|| holder.clone());
        return Err(AppError::conflict(format!(
            "another change (`{title}`) is being applied to `{name}` right now; only one change may touch a device at a time"
        )));
    }
    busy.changes.insert(change.id.clone());
    for device in &devices {
        busy.devices.insert(device.clone(), change.id.clone());
    }
    drop(busy);
    Ok(Claim {
        state,
        change_id: change.id.clone(),
        devices,
    })
}

/// Changes left `applying` by a process that stopped: nobody is driving them
/// any more, so they are failed with a message naming the devices to check.
pub fn recover_interrupted(app: &App) -> AppResult<usize> {
    let changes: Vec<Change> = app.store().list(COLLECTION)?;
    let mut recovered = 0;
    for mut change in changes
        .into_iter()
        .filter(|change| change.state == ChangeState::Applying)
    {
        let names = device_names(app, &change);
        let unfinished: Vec<String> = change
            .targets
            .iter()
            .filter(|target| {
                target.result.as_ref().is_none_or(|result| {
                    matches!(
                        result.state,
                        crate::model::TargetState::Applying | crate::model::TargetState::Pending
                    )
                })
            })
            .filter_map(|target| names.get(&target.device_id).cloned())
            .collect();
        let message = if unfinished.is_empty() {
            "Nocturne was closed during the apply — check these devices".to_string()
        } else {
            format!(
                "Nocturne was closed during the apply — check these devices: {}",
                unfinished.join(", ")
            )
        };
        for target in &mut change.targets {
            let result = target.result.get_or_insert_with(Default::default);
            if matches!(
                result.state,
                crate::model::TargetState::Applying | crate::model::TargetState::Pending
            ) {
                result.state = crate::model::TargetState::Failed;
                result.finished_at = Some(now_ms());
                result.error = Some(message.clone());
            }
        }
        change.state = ChangeState::Failed;
        change.job_id = None;
        save(app, &mut change)?;
        app.audit(
            "changes.interrupted",
            None,
            format!("Change `{}` was interrupted: {message}", change.title),
            serde_json::json!({"change_id": change.id, "devices": unfinished}),
        );
        recovered += 1;
    }
    Ok(recovered)
}
