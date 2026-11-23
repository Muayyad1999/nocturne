//! Maintenance windows: planned work, during which alerts are still recorded
//! but nobody is woken up.

use nocturne_app::model::Device;
use nocturne_app::{App, AppError, AppResult, Router, util};
use serde::Deserialize;
use serde_json::json;

use crate::db::collections::MAINTENANCE;
use crate::model::{MaintenanceWindow, Recurrence, Scope};
use crate::scope;
use crate::state::MonitorState;

const WEEK_MS: i64 = 7 * 86_400_000;

/// Whether a window is open at `now`.
pub fn active(window: &MaintenanceWindow, now: i64) -> bool {
    let length = window.ends_at - window.starts_at;
    if length <= 0 || now < window.starts_at {
        return false;
    }
    match window.recurring {
        None => now < window.ends_at,
        // The same slot every week, for as long as the first one lasted.
        Some(Recurrence::Weekly) => (now - window.starts_at) % WEEK_MS < length,
    }
}

pub fn list(app: &App) -> AppResult<Vec<MaintenanceWindow>> {
    let mut windows: Vec<MaintenanceWindow> = app.store().list(MAINTENANCE)?;
    windows.sort_by_key(|window| (window.starts_at, window.name.to_lowercase()));
    Ok(windows)
}

/// Whether a device (or, with `None`, the installation as a whole) is inside
/// an open maintenance window.
pub fn covers(app: &App, device: Option<&Device>, now: i64) -> AppResult<bool> {
    Ok(covers_among(&list(app)?, device, now))
}

/// The same question against windows already read, for a caller that asks
/// it about many devices in one go.
pub fn covers_among(windows: &[MaintenanceWindow], device: Option<&Device>, now: i64) -> bool {
    windows
        .iter()
        .filter(|window| active(window, now))
        .any(|window| match device {
            Some(device) => scope::matches(&window.scope, device, false),
            // An alert about no device in particular is only muted by a
            // window that mutes everything.
            None => scope::is_everything(&window.scope),
        })
}

/// The windows that are open right now, for `monitor.status`.
pub fn open_now(app: &App, now: i64) -> AppResult<Vec<MaintenanceWindow>> {
    Ok(list(app)?
        .into_iter()
        .filter(|window| active(window, now))
        .collect())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SaveParams {
    pub id: Option<String>,
    pub name: String,
    pub scope: Scope,
    pub starts_at: i64,
    pub ends_at: i64,
    pub recurring: Option<Recurrence>,
    pub note: String,
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

pub fn register(router: &mut Router) {
    router.add(
        "maintenance.list",
        |app: App, _: nocturne_app::NoParams| async move { list(&app) },
    );
    router.add(
        "maintenance.save",
        |app: App, params: SaveParams| async move {
            if params.name.trim().is_empty() {
                return Err(AppError::invalid("give the maintenance window a name"));
            }
            if params.ends_at <= params.starts_at {
                return Err(AppError::invalid(
                    "a maintenance window has to end after it starts",
                ));
            }
            if params.recurring == Some(Recurrence::Weekly)
                && params.ends_at - params.starts_at >= WEEK_MS
            {
                return Err(AppError::invalid(
                    "a weekly maintenance window has to be shorter than a week, or it never closes",
                ));
            }
            let now = app.state::<MonitorState>().now();
            let existing = match &params.id {
                Some(id) if !id.is_empty() => Some(app.store().require::<MaintenanceWindow>(
                    MAINTENANCE,
                    id,
                    "maintenance window",
                )?),
                _ => None,
            };
            let window = MaintenanceWindow {
                id: existing
                    .as_ref()
                    .map(|window| window.id.clone())
                    .unwrap_or_else(util::new_id),
                name: params.name.trim().to_string(),
                scope: params.scope,
                starts_at: params.starts_at,
                ends_at: params.ends_at,
                recurring: params.recurring,
                note: params.note,
                created_at: existing
                    .as_ref()
                    .map(|window| window.created_at)
                    .unwrap_or(now),
                updated_at: now,
            };
            app.store().put(MAINTENANCE, &window.id, &window)?;
            app.audit(
                "maintenance.save",
                None,
                format!("Saved the maintenance window `{}`", window.name),
                json!({"id": window.id, "starts_at": window.starts_at, "ends_at": window.ends_at}),
            );
            app.publish("monitor.status_changed", &json!({"what": "maintenance"}));
            Ok(window)
        },
    );
    router.add(
        "maintenance.delete",
        |app: App, params: IdParams| async move {
            let window: MaintenanceWindow =
                app.store()
                    .require(MAINTENANCE, &params.id, "maintenance window")?;
            app.store().delete(MAINTENANCE, &params.id)?;
            app.audit(
                "maintenance.delete",
                None,
                format!("Deleted the maintenance window `{}`", window.name),
                json!({"id": window.id}),
            );
            app.publish("monitor.status_changed", &json!({"what": "maintenance"}));
            Ok(json!({"deleted": true}))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(starts_at: i64, ends_at: i64, recurring: Option<Recurrence>) -> MaintenanceWindow {
        MaintenanceWindow {
            id: "w1".into(),
            name: "Saturday night".into(),
            starts_at,
            ends_at,
            recurring,
            ..MaintenanceWindow::default()
        }
    }

    #[test]
    fn a_one_off_window_is_open_between_its_ends() {
        let window = window(1_000, 2_000, None);
        assert!(!active(&window, 999));
        assert!(active(&window, 1_000));
        assert!(active(&window, 1_999));
        assert!(!active(&window, 2_000));
    }

    #[test]
    fn a_weekly_window_comes_round_again() {
        let window = window(1_000, 3_600_000 + 1_000, Some(Recurrence::Weekly));
        assert!(active(&window, 1_000));
        assert!(!active(&window, 3_600_000 + 2_000));
        assert!(active(&window, WEEK_MS + 1_000));
        assert!(active(&window, 3 * WEEK_MS + 3_599_999));
        assert!(!active(&window, 3 * WEEK_MS + 3_700_000));
        // Never before the first one.
        assert!(!active(&window, 500));
    }

    #[test]
    fn a_window_with_no_length_is_never_open() {
        assert!(!active(&window(1_000, 1_000, None), 1_000));
    }
}
