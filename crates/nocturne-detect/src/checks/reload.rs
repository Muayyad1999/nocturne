//! `unplanned_reload`: a device restarted and nothing Nocturne was doing
//! explains it.

use serde_json::{Value, json};

use nocturne_app::jobs::{self, Job, JobFilter};
use nocturne_app::store::LogQuery;
use nocturne_app::{App, inventory};

use super::{UNAUTHENTICATED, compact, device_name, line};
use crate::DetectState;
use crate::catalogue::UNPLANNED_RELOAD;
use crate::model::{Finding, Severity};
use crate::planned::{self, Span, Verdict};
use crate::settings::Settings;
use crate::sources::Gap;

/// The kinds of job that can change a device, and so can be why it
/// restarted: the change pipeline (whose dead-man switch is a timed reload)
/// and an Ansible playbook, which does whatever its author wrote.
///
/// Named one by one rather than "any job on the device", or even "any
/// automation job": polling, backups, collection and the automation
/// feature's own command runs and playbooks only read, and they run against
/// every device all day. If they counted, no restart would ever be
/// unexplained.
const CAN_CHANGE_A_DEVICE: [&str; 3] = ["changes.apply", "changes.rollback", "automation.ansible"];

/// What the lab feature records when it restarts simulated devices itself:
/// a lab started or reset boots every node, and a node given a new startup
/// configuration is reloaded into it. Each entry names its lab (`lab_id`).
///
/// A fault injected into a lab, or a node switched off and on from the lab
/// screen, is not here on purpose: that is somebody breaking a simulated
/// network to see what notices, and this is one of the things that should.
const LAB_RESTARTS: [&str; 3] = ["labs.start", "labs.reset", "labs.node.config_set"];

/// How far either side of a simulated device's restart the lab's own record
/// of having caused it may lie. Both are written by this process within
/// moments of each other, so this is much tighter than the grace: a lab is
/// where restarts are tried out, and a reset must not hide the next one.
const LAB_SLACK_MS: i64 = 60_000;

/// Whether a job was at work on the device around the restart.
fn at_work(job: &Job, at: i64, grace_ms: i64) -> bool {
    if job.started_at > at + grace_ms {
        return false;
    }
    job.finished_at
        .is_none_or(|finished_at| finished_at >= at - grace_ms)
}

/// The jobs that can change a device and have this one among their devices,
/// newest first.
fn jobs_that_can_change(app: &App, device_id: &str) -> Result<Vec<Job>, Gap> {
    let mut found = Vec::new();
    // Asked for kind by kind: a device is polled and backed up so often that
    // "its newest thousand jobs" may not reach back as far as the grace does.
    for kind in CAN_CHANGE_A_DEVICE {
        found.extend(
            jobs::list(
                app,
                &JobFilter {
                    kind: Some(kind.to_string()),
                    device_id: Some(device_id.to_string()),
                    limit: Some(1_000),
                    ..JobFilter::default()
                },
            )
            .map_err(|err| Gap::failed("the job list", err.message))?,
        );
    }
    Ok(found)
}

/// Whether the lab feature restarted this simulated device's lab around
/// `at`, by its own entries in the audit trail.
fn restarted_by_its_lab(app: &App, device_id: &str, at: i64) -> Result<bool, Gap> {
    let Some(lab) = inventory::get(app, device_id)
        .ok()
        .and_then(|device| device.lab)
    else {
        return Ok(false);
    };
    let entries = app
        .store()
        .log_query(&LogQuery {
            stream: Some("audit".into()),
            since: Some(at - LAB_SLACK_MS),
            until: Some(at + LAB_SLACK_MS),
            // The store matches a substring; the exact action is checked below.
            text: Some("labs.".into()),
            limit: Some(5_000),
            ..LogQuery::default()
        })
        .map_err(|err| Gap::failed("the audit trail", err.message))?;
    Ok(entries.iter().any(|entry| {
        LAB_RESTARTS.contains(&entry.source.as_str())
            && entry
                .body
                .as_ref()
                .is_some_and(|body| body["lab_id"] == lab.lab_id.as_str())
    }))
}

/// `logs.signal` with `kind: "reload"`.
pub async fn check(
    app: &App,
    settings: &Settings,
    payload: &Value,
    received_at: i64,
) -> Result<Option<Finding>, Gap> {
    let Some(device_id) = payload["device_id"].as_str() else {
        return Ok(None);
    };
    let at = payload["at"].as_i64().unwrap_or(received_at);
    let grace_ms = settings.grace_ms();
    let verdict = planned::approved_change(app, device_id, Span::moment(at), grace_ms).await?;
    let Verdict::Unexplained { checked, nearest } = verdict else {
        return Ok(None);
    };

    let jobs = jobs_that_can_change(app, device_id)?;
    if jobs.iter().any(|job| at_work(job, at, grace_ms)) {
        return Ok(None);
    }
    if restarted_by_its_lab(app, device_id, at)? {
        return Ok(None);
    }

    let entry_id = payload["entry_id"].as_i64();
    let line = line(app, entry_id);
    // The same line delivered twice is one restart.
    if let Some(fingerprint) = line.fingerprint
        && app
            .state::<DetectState>()
            .same_line_again(UNPLANNED_RELOAD, device_id, fingerprint, at)
    {
        return Ok(None);
    }
    let mut detail = String::from(
        "Decided from a log line in which the device says it restarted, from the changes the pipeline ran on it, and from the jobs that can change a device.",
    );
    if line.forgeable {
        detail.push(' ');
        detail.push_str(UNAUTHENTICATED);
    }
    Ok(Some(Finding {
        detection: UNPLANNED_RELOAD,
        at,
        device_id: Some(device_id.to_string()),
        severity: Severity::Medium,
        title: format!(
            "{} restarted and nothing Nocturne was doing explains it",
            device_name(app, device_id)
        ),
        evidence: compact(json!({
            "decided_from": "log",
            "log_entry_id": entry_id,
            "rule_id": payload["rule_id"],
            "stream": line.stream,
            "via": line.via,
            "changes_checked": checked,
            "nearest_change": nearest,
            "jobs_checked": jobs.len(),
            "grace_minutes": settings.grace_minutes,
            "caution": line.forgeable.then_some(UNAUTHENTICATED),
        })),
        key: "reload".into(),
        detail,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    fn job(started_at: i64, finished_at: Option<i64>) -> Job {
        Job {
            started_at,
            finished_at,
            ..Job::default()
        }
    }

    #[test]
    fn a_job_counts_while_it_runs_and_for_the_grace_either_side() {
        let at = 1_763_000_000_000;
        let grace = 15 * MINUTE;
        assert!(at_work(&job(at - 60 * MINUTE, None), at, grace), "running");
        assert!(at_work(&job(at - 60 * MINUTE, Some(at - grace)), at, grace));
        assert!(!at_work(
            &job(at - 60 * MINUTE, Some(at - grace - 1)),
            at,
            grace
        ));
        assert!(at_work(&job(at + grace, None), at, grace));
        assert!(!at_work(&job(at + grace + 1, None), at, grace));
    }
}
