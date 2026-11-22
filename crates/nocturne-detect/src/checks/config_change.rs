//! `unplanned_config_change`: the configuration changed and the pipeline
//! was not the one changing it.
//!
//! One edit can be seen twice — the device says so in its log, and the next
//! backup differs from the one before — and each sighting is judged on its
//! own, with its own evidence. They share a dedup key, so however many times
//! an edit is seen it is one alert with a count.

use serde_json::{Value, json};

use nocturne_app::App;

use super::{UNAUTHENTICATED, compact, device_name, line, word};
use crate::DetectState;
use crate::catalogue::UNPLANNED_CONFIG_CHANGE;
use crate::model::{Finding, Severity};
use crate::planned::{self, Span, Verdict};
use crate::settings::Settings;
use crate::sources::{self, Gap};

const KEY: &str = "config";

/// The device said it was configured: `logs.config_changed {device_id, by?,
/// from?, entry_id, at}`.
pub async fn from_log(
    app: &App,
    settings: &Settings,
    payload: &Value,
    received_at: i64,
) -> Result<Option<Finding>, Gap> {
    // A line from a source that is no known device has no changes to be
    // compared with; the log feature lists such sources itself.
    let Some(device_id) = payload["device_id"].as_str() else {
        return Ok(None);
    };
    let at = payload["at"].as_i64().unwrap_or(received_at);
    let verdict =
        planned::approved_change(app, device_id, Span::moment(at), settings.grace_ms()).await?;
    let Verdict::Unexplained { checked, nearest } = verdict else {
        return Ok(None);
    };

    let entry_id = payload["entry_id"].as_i64();
    let line = line(app, entry_id);
    // The same line delivered twice is one thing the device said, not two
    // edits: the first sighting is the hit.
    if let Some(fingerprint) = line.fingerprint
        && app.state::<DetectState>().same_line_again(
            UNPLANNED_CONFIG_CHANGE,
            device_id,
            fingerprint,
            at,
        )
    {
        return Ok(None);
    }
    let by = word(payload["by"].as_str());
    let from = word(payload["from"].as_str());
    let name = device_name(app, device_id);
    let what = match (&by, &from) {
        (Some(by), Some(from)) => format!("{by} changed {name} from {from}"),
        (Some(by), None) => format!("{by} changed {name}"),
        (None, Some(from)) => format!("{name} was configured from {from}"),
        (None, None) => format!("{name} was configured"),
    };
    let mut detail = String::from(
        "Decided from a log line in which the device says it was configured, and from the changes the pipeline ran on it.",
    );
    if line.forgeable {
        detail.push(' ');
        detail.push_str(UNAUTHENTICATED);
    }
    Ok(Some(Finding {
        detection: UNPLANNED_CONFIG_CHANGE,
        at,
        device_id: Some(device_id.to_string()),
        severity: Severity::High,
        title: format!("{what} and no approved change was being applied"),
        evidence: compact(json!({
            "decided_from": "log",
            "log_entry_id": entry_id,
            "stream": line.stream,
            "via": line.via,
            "by": by,
            "from": from,
            "changes_checked": checked,
            "nearest_change": nearest,
            "grace_minutes": settings.grace_minutes,
            "caution": line.forgeable.then_some(UNAUTHENTICATED),
        })),
        key: KEY.into(),
        detail,
    }))
}

/// A backup differs from the one before: `config.changed {device_id, kind,
/// version_id, previous_id, added, removed}`.
///
/// The event says when the difference was noticed, not when it was made. It
/// was made after the earlier version was last fetched unchanged and before
/// the later one was first fetched, so that stretch is what a change has to
/// fall into — otherwise every approved change would be reported by the
/// scheduled backup that follows it hours later.
///
/// The stretch starts where the earlier version was *last* seen, not where
/// it was first stored. A version can be the device's configuration for
/// months, and very often it was stored by the change that made it: measured
/// from there, that old change would explain whatever is done to the device
/// by hand afterwards.
pub async fn from_backup(
    app: &App,
    settings: &Settings,
    payload: &Value,
) -> Result<Option<Finding>, Gap> {
    let (Some(device_id), Some(version_id), Some(previous_id)) = (
        payload["device_id"].as_str(),
        payload["version_id"].as_i64(),
        payload["previous_id"].as_i64(),
    ) else {
        return Err(Gap::failed(
            "config.changed",
            "the event did not name a device and two versions",
        ));
    };
    // A version was stored and the event itself counts no line that differs
    // (the bytes did, not the configuration): there is nothing to report.
    // Counts that are missing are unknown, which is not the same.
    if payload["added"].as_u64() == Some(0) && payload["removed"].as_u64() == Some(0) {
        return Ok(None);
    }
    let kind = payload["kind"].as_str().unwrap_or("running");
    let previous = sources::version(app, device_id, kind, previous_id).await?;
    let version = sources::version(app, device_id, kind, version_id).await?;
    // Without both the stretch has no start or no end, and judging the
    // moment of the backup alone would accuse somebody of a change the
    // pipeline made.
    let (Some(previous), Some(version)) = (previous, version) else {
        return Err(Gap::failed(
            sources::CONFIG_VERSIONS,
            format!(
                "version {previous_id} or {version_id} of {} is no longer stored, so when the change was made cannot be bounded",
                device_name(app, device_id)
            ),
        ));
    };
    let to = version.first_seen;
    let span = Span {
        from: previous.last_seen.min(to),
        to,
    };
    let verdict = planned::approved_change(app, device_id, span, settings.grace_ms()).await?;
    let Verdict::Unexplained { checked, nearest } = verdict else {
        return Ok(None);
    };
    let name = device_name(app, device_id);
    Ok(Some(Finding {
        detection: UNPLANNED_CONFIG_CHANGE,
        at: to,
        device_id: Some(device_id.to_string()),
        severity: Severity::High,
        title: format!(
            "{name}'s {kind} configuration changed between two backups and no approved change was being applied"
        ),
        evidence: compact(json!({
            "decided_from": "backup",
            "kind": kind,
            "version_id": version_id,
            "previous_id": previous_id,
            "between": {"from": span.from, "to": span.to},
            "lines_added": payload["added"],
            "lines_removed": payload["removed"],
            "changes_checked": checked,
            "nearest_change": nearest,
            "grace_minutes": settings.grace_minutes,
        })),
        key: KEY.into(),
        detail: "Decided from two stored versions of the configuration that differ, and from the changes the pipeline ran on the device between the last time the earlier one was fetched and the first time the later one was. The backup says that it changed, not who changed it.".into(),
    }))
}
