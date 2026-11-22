//! `protected_change_outside_window`: an approved change touched protected
//! resources on a device that no maintenance window covered.
//!
//! Informational by design. The change was approved by a person who was
//! shown the protected resources by name; this only says that nobody had
//! announced the work.

use serde_json::{Value, json};

use nocturne_app::model::Device;
use nocturne_app::{App, inventory};

use crate::DetectState;
use crate::catalogue::PROTECTED_CHANGE_OUTSIDE_WINDOW;
use crate::model::{Finding, Severity};
use crate::settings::Settings;
use crate::sources::{self, Gap, Scope, Window};

const WEEK_MS: i64 = 7 * 86_400_000;

fn any_equal(wanted: &[String], value: &str) -> bool {
    wanted
        .iter()
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(value.trim()))
}

/// Whether a window's scope covers a device.
///
/// The monitor's own rule for maintenance windows, restated because this
/// crate cannot link to it (`nocturne-monitor/src/scope.rs`, with the lab
/// gate off): every facet that names something has to agree, and a facet
/// that names nothing covers everything. A window mutes lab devices like
/// any other.
fn covers(scope: &Scope, device: &Device) -> bool {
    (scope.device_ids.is_empty() || scope.device_ids.iter().any(|id| id == &device.id))
        && (scope.sites.is_empty() || any_equal(&scope.sites, &device.site))
        && (scope.roles.is_empty() || any_equal(&scope.roles, &device.role))
        && (scope.tags.is_empty() || device.tags.iter().any(|tag| any_equal(&scope.tags, tag)))
}

/// Whether a window was open at any moment of `[at - slack_ms, at]`.
///
/// `at` is when the change finished, which is all `changes.changed` says;
/// a change that started inside a window and finished a minute after it
/// closed was still the announced work, hence the slack. A weekly window is
/// the same slot every week for as long as the first one lasted
/// (`nocturne-monitor/src/maintenance.rs`).
fn open_around(window: &Window, at: i64, slack_ms: i64) -> bool {
    let length = window.ends_at - window.starts_at;
    if length <= 0 || at < window.starts_at {
        return false;
    }
    let overlaps = |start: i64| start <= at && start + length > at - slack_ms;
    if window.recurring.is_none() {
        return overlaps(window.starts_at);
    }
    // The occurrence that started most recently. An earlier one ended
    // earlier still, so if this one is out of reach they all are.
    overlaps(window.starts_at + (at - window.starts_at) / WEEK_MS * WEEK_MS)
}

/// The pipeline sent this device something, whatever came of it.
///
/// `failed` is left out on purpose: it is what a target is called both when
/// the pipeline never got as far as sending anything (no connection, a
/// drifted configuration) and when the session was lost half-way, and the
/// summary cannot tell the two apart. Saying "touched" for the first would
/// be saying something that did not happen.
fn touched(state: Option<&str>) -> bool {
    matches!(
        state,
        Some("applied" | "rolled_back" | "rollback_failed" | "guard_fired")
    )
}

/// `changes.changed`, whose payload is the change as `changes.list` shows it.
pub async fn check(
    app: &App,
    settings: &Settings,
    payload: &Value,
    received_at: i64,
) -> Result<Option<Finding>, Gap> {
    // A deleted change is announced as `{id, state: "deleted"}`.
    if payload["state"] == "deleted" {
        return Ok(None);
    }
    let change = sources::parse_change(payload.clone())?;
    // The event is published at every step of an apply. Only a change that
    // has finished, with no job still on it, is judged — once.
    let finished = matches!(
        change.state.as_str(),
        "applied" | "partially_applied" | "rolled_back" | "failed"
    );
    if !finished || change.job_id.is_some() {
        return Ok(None);
    }
    let protected: Vec<_> = change
        .devices
        .iter()
        .filter(|device| device.protected_hits > 0 && touched(device.state.as_deref()))
        .collect();
    if protected.is_empty() {
        return Ok(None);
    }
    let at = if change.updated_at > 0 {
        change.updated_at
    } else {
        received_at
    };
    let windows = sources::windows(app).await?;
    // Marked as judged only now: a change whose windows could not be read
    // has not been judged, and may be if it is announced again.
    if !app
        .state::<DetectState>()
        .first_judgement(&change.id, change.updated_at)
    {
        return Ok(None);
    }
    let mut outside: Vec<(Device, u64)> = Vec::new();
    let mut gone: Vec<&str> = Vec::new();
    for target in protected {
        // A device deleted since has no site, role or tags left to match a
        // window's scope against; it is named, and not judged.
        let Ok(device) = inventory::get(app, &target.device_id) else {
            gone.push(&target.device_id);
            continue;
        };
        let covered = windows.iter().any(|window| {
            covers(&window.scope, &device) && open_around(window, at, settings.grace_ms())
        });
        if !covered {
            outside.push((device, target.protected_hits));
        }
    }
    if outside.is_empty() {
        return Ok(None);
    }
    let title = if change.title.trim().is_empty() {
        format!("change {}", change.id)
    } else {
        format!("`{}`", change.title.trim())
    };
    let whose = match outside.as_slice() {
        [(only, _)] => format!("on {}", only.name),
        many => format!("on {} devices", many.len()),
    };
    Ok(Some(Finding {
        detection: PROTECTED_CHANGE_OUTSIDE_WINDOW,
        at,
        device_id: match outside.as_slice() {
            [(only, _)] => Some(only.id.clone()),
            _ => None,
        },
        severity: Severity::Info,
        title: format!(
            "{title} touched protected resources {whose} outside any maintenance window"
        ),
        evidence: json!({
            "decided_from": "changes.changed",
            "change_id": change.id,
            "state": change.state,
            "finished_at": at,
            "devices": outside
                .iter()
                .map(|(device, hits)| json!({"device_id": device.id, "protected_hits": hits}))
                .collect::<Vec<_>>(),
            "devices_no_longer_in_inventory": gone,
            "windows_checked": windows.len(),
            "grace_minutes": settings.grace_minutes,
        }),
        key: change.id.clone(),
        detail: "Decided from the change itself and from the maintenance windows. The change was approved with its protected resources named; this says only that no window announced the work.".into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600_000;
    const MINUTE: i64 = 60_000;

    fn window(starts_at: i64, ends_at: i64, weekly: bool) -> Window {
        Window {
            starts_at,
            ends_at,
            recurring: weekly.then(|| "weekly".to_string()),
            ..Window::default()
        }
    }

    #[test]
    fn a_one_off_window_counts_while_it_is_open_and_for_the_slack_after_it_closes() {
        let window = window(10 * HOUR, 11 * HOUR, false);
        let slack = 15 * MINUTE;
        assert!(!open_around(&window, 10 * HOUR - 1, slack), "before it");
        assert!(open_around(&window, 10 * HOUR, slack));
        assert!(open_around(&window, 11 * HOUR - 1, slack));
        assert!(open_around(&window, 11 * HOUR + slack - 1, slack));
        assert!(!open_around(&window, 11 * HOUR + slack, slack));
    }

    #[test]
    fn a_weekly_window_comes_round_again_and_never_before_the_first_one() {
        let window = window(10 * HOUR, 11 * HOUR, true);
        let slack = 15 * MINUTE;
        assert!(open_around(
            &window,
            3 * WEEK_MS + 10 * HOUR + 30 * MINUTE,
            slack
        ));
        assert!(!open_around(&window, 3 * WEEK_MS + 12 * HOUR, slack));
        assert!(!open_around(&window, 3 * WEEK_MS + 9 * HOUR, slack));
        assert!(!open_around(&window, 9 * HOUR, slack));
    }

    #[test]
    fn the_slack_reaches_into_the_gap_after_a_weekly_occurrence() {
        let window = window(10 * HOUR, 11 * HOUR, true);
        let slack = 15 * MINUTE;
        let closed = 2 * WEEK_MS + 11 * HOUR;
        assert!(open_around(&window, closed + slack - 1, slack));
        assert!(!open_around(&window, closed + slack, slack));
        // And with no slack, closed is closed.
        assert!(!open_around(&window, closed, 0));
    }

    #[test]
    fn a_window_with_no_length_is_never_open() {
        assert!(!open_around(&window(HOUR, HOUR, false), HOUR, 15 * MINUTE));
    }

    #[test]
    fn every_facet_a_scope_names_has_to_agree() {
        let device = Device {
            id: "d1".into(),
            site: "HQ".into(),
            role: "core".into(),
            tags: vec!["floor-2".into()],
            ..Device::default()
        };
        assert!(
            covers(&Scope::default(), &device),
            "nothing named: everything"
        );
        let mut scope = Scope {
            sites: vec!["hq".into()],
            ..Scope::default()
        };
        assert!(covers(&scope, &device), "sites ignore case");
        scope.roles = vec!["access".into()];
        assert!(!covers(&scope, &device));
        scope.roles = vec!["core".into()];
        scope.tags = vec!["FLOOR-2".into()];
        assert!(covers(&scope, &device));
        scope.device_ids = vec!["other".into()];
        assert!(!covers(&scope, &device));
    }

    #[test]
    fn only_a_device_the_pipeline_sent_something_to_was_touched() {
        for state in ["applied", "rolled_back", "rollback_failed", "guard_fired"] {
            assert!(touched(Some(state)), "{state}");
        }
        for state in ["pending", "skipped", "applying", "failed"] {
            assert!(!touched(Some(state)), "{state}");
        }
        assert!(!touched(None));
    }
}
