//! The one screen a shift starts with.
//!
//! What is down, what is firing, what is broken, what changed, what is
//! drifting, what is running. Every part is assembled from somewhere else,
//! and every part that comes from another feature is **independently
//! nullable**: when the feature is not in this build, or did not answer, the
//! part is `null` and the reason is in `unavailable`. That distinction is the
//! whole point of the screen — *no open alerts* and *there is no alerting in
//! this build* must not look the same.

use std::collections::{BTreeMap, BTreeSet};

use nocturne_app::jobs::{self, JobFilter, JobState};
use nocturne_app::store::LogQuery;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppBuilder, AppResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::OpsState;
use crate::configs;
use crate::incidents;
use crate::model::Severity as IncidentSeverity;
use crate::refs::{AlertRef, ChangeRef, Devices, InsightRef, JobRef, Period};
use crate::settings;
use crate::sources::{self, Gap, Reason, Severity, Unavailable};

const DAY_MS: i64 = 86_400_000;
/// How many records a part reads before it stops and says it was cut off.
const SCAN: usize = 500;

/// Collects the parts, and the reasons the missing ones are missing.
#[derive(Default)]
struct Parts {
    object: serde_json::Map<String, Value>,
    unavailable: Vec<Unavailable>,
}

impl Parts {
    fn set(&mut self, name: &str, value: Value) {
        self.object.insert(name.to_string(), value);
    }

    /// A part whose source is not in this build, or did not answer: `null`,
    /// with the reason beside it.
    fn missing(&mut self, name: &str, gap: Gap) {
        self.object.insert(name.to_string(), Value::Null);
        self.unavailable.push(gap.for_section(name));
    }

    /// A part this feature itself could not read from the core store.
    fn failed(&mut self, name: &str, message: String) {
        self.object.insert(name.to_string(), Value::Null);
        self.unavailable.push(Unavailable {
            section: name.to_string(),
            reason: Reason::Failed,
            message,
        });
    }
}

/// One line of the activity feed.
fn activity(
    ts: i64,
    kind: &str,
    text: String,
    device_id: Option<&str>,
    reference: Option<Value>,
) -> Value {
    let mut line = json!({"ts": ts, "kind": kind, "text": text});
    if let Some(device_id) = device_id {
        line["device_id"] = json!(device_id);
    }
    if let Some(reference) = reference {
        line["ref"] = reference;
    }
    line
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct GetParams {
    /// Ignore the cached copy and read everything again.
    pub refresh: bool,
}

pub async fn get(app: &App, params: &GetParams) -> AppResult<Value> {
    let config = settings::get(app)?;
    let now = now_ms();
    if !params.refresh {
        let cached = app.state::<OpsState>().overview.lock().clone();
        if let Some((made_at, value)) = cached
            && now.saturating_sub(made_at) < config.overview_cache_ms as i64
        {
            return Ok(value);
        }
    }
    let value = build(app, now).await?;
    *app.state::<OpsState>().overview.lock() = Some((now, value.clone()));
    Ok(value)
}

async fn build(app: &App, now: i64) -> AppResult<Value> {
    let mut parts = Parts::default();
    let devices = Devices::load(app)?;
    let mut feed: Vec<Value> = Vec::new();

    parts.set("devices", devices_part(&devices));

    // The four features are asked all at once: a slow one should not delay
    // the others.
    let (alerts, changes, compliance, insights) = tokio::join!(
        alerts_part(app),
        sources::changes(app, json!({"limit": SCAN})),
        sources::compliance(app, Some("fail")),
        sources::open_insights(app, SCAN),
    );

    match alerts {
        Ok(summary) => {
            let newest: Vec<AlertRef> = summary
                .newest
                .iter()
                .take(5)
                .map(|alert| AlertRef::new(alert, &devices))
                .collect();
            for alert in summary.newest.iter().take(5) {
                feed.push(activity(
                    alert.started_at,
                    "alert",
                    format!("{} alert: {}", alert.severity.as_str(), alert.title),
                    alert.device_id.as_deref(),
                    Some(json!({"type": "alert", "id": alert.id})),
                ));
            }
            let mut by_severity = serde_json::Map::new();
            for severity in Severity::MOST_SEVERE_FIRST {
                by_severity.insert(
                    severity.as_str().to_string(),
                    json!(
                        summary
                            .by_severity
                            .get(severity.as_str())
                            .copied()
                            .unwrap_or(0)
                    ),
                );
            }
            parts.set(
                "alerts",
                json!({
                    "open": summary.open,
                    "by_severity": Value::Object(by_severity),
                    "newest": newest,
                    "truncated": summary.truncated,
                    "from": summary.from,
                }),
            );
        }
        Err(gap) => parts.missing("alerts", gap),
    }

    match changes {
        Ok(changes) => {
            let recent: Vec<ChangeRef> = changes.iter().take(5).map(ChangeRef::from).collect();
            for change in changes.iter().take(5) {
                feed.push(activity(
                    change.updated_at,
                    "change",
                    format!("Change {}: {}", change.state, change.title),
                    change
                        .devices()
                        .and_then(|ids| ids.first().cloned())
                        .as_deref(),
                    Some(json!({"type": "change", "id": change.id})),
                ));
            }
            let since = now - DAY_MS;
            parts.set(
                "changes",
                json!({
                    "awaiting_approval": changes.iter().filter(|change| change.is_awaiting_approval()).count(),
                    "applying": changes.iter().filter(|change| change.state == "applying").count(),
                    "applied_24h": changes
                        .iter()
                        .filter(|change| change.is_applied() && change.updated_at >= since)
                        .count(),
                    "failed_24h": changes
                        .iter()
                        .filter(|change| change.is_failure() && change.updated_at >= since)
                        .count(),
                    "recent": recent,
                    // The counts are over the newest `SCAN` changes; beyond
                    // that they are a floor, and say so.
                    "truncated": changes.len() >= SCAN,
                }),
            );
        }
        Err(gap) => parts.missing("changes", gap),
    }

    match backups_part(app, &devices, now) {
        Ok(value) => parts.set("backups", value),
        Err(err) => parts.failed("backups", err.message),
    }

    match compliance {
        Ok(compliance) => {
            let failing: BTreeSet<&str> = compliance
                .findings
                .iter()
                .filter(|finding| finding.status == "fail")
                .map(|finding| finding.device_id.as_str())
                .collect();
            parts.set(
                "compliance",
                json!({
                    "score_pct": compliance.summary.score_pct,
                    "failing_devices": failing.len(),
                    "failing_findings": compliance.findings.len(),
                    "run_at": compliance.run_at,
                    "truncated": compliance.truncated,
                }),
            );
        }
        Err(gap) => parts.missing("compliance", gap),
    }

    match insights {
        Ok(insights) => {
            let mut open = insights.open.clone();
            open.sort_by_key(|insight| std::cmp::Reverse(insight.severity));
            let top: Vec<InsightRef> = open
                .iter()
                .take(5)
                .map(|insight| InsightRef::new(insight, &devices))
                .collect();
            parts.set(
                "insights",
                json!({
                    "open": insights.total.unwrap_or(insights.open.len() as u64),
                    "top": top,
                    "truncated": insights.total.is_none() && insights.open.len() >= SCAN,
                }),
            );
        }
        Err(gap) => parts.missing("insights", gap),
    }

    match jobs_part(app, now) {
        Ok(value) => parts.set("jobs", value),
        Err(err) => parts.failed("jobs", err.message),
    }

    // This feature's own records are always there.
    parts.set("incidents", incidents_part(app)?);

    match audit_feed(app, now, &devices) {
        Ok(lines) => feed.extend(lines),
        Err(err) => parts.unavailable.push(Unavailable {
            section: "activity".into(),
            reason: Reason::Failed,
            message: err.message,
        }),
    }
    feed.sort_by_key(|line| std::cmp::Reverse(line["ts"].as_i64().unwrap_or(0)));
    feed.truncate(20);
    parts.set("activity", Value::Array(feed));

    parts.set("unavailable", serde_json::to_value(&parts.unavailable)?);
    parts.set("generated_at", json!(now));
    Ok(Value::Object(parts.object))
}

// ------------------------------------------------------------------ devices

#[derive(Default)]
struct SiteCounts {
    total: usize,
    down: usize,
    unknown: usize,
}

fn devices_part(devices: &Devices) -> Value {
    let mut sites: BTreeMap<&str, SiteCounts> = BTreeMap::new();
    let (mut up, mut down, mut unknown, mut lab) = (0, 0, 0, 0);
    for row in devices.rows() {
        let site = sites.entry(row.device.site.as_str()).or_default();
        site.total += 1;
        match row.state.reachable {
            Some(true) => up += 1,
            Some(false) => {
                down += 1;
                site.down += 1;
            }
            None => {
                unknown += 1;
                site.unknown += 1;
            }
        }
        if row.device.lab.is_some() {
            lab += 1;
        }
    }
    json!({
        "total": devices.rows().len(),
        "up": up,
        "down": down,
        // Never polled, or monitoring switched off. Not "up".
        "unknown": unknown,
        "lab": lab,
        "by_site": sites
            .into_iter()
            .map(|(site, counts)| json!({
                "site": site,
                "total": counts.total,
                "down": counts.down,
                "unknown": counts.unknown,
            }))
            .collect::<Vec<_>>(),
    })
}

// ------------------------------------------------------------------- alerts

/// What the overview needs about alerts, from whichever method this build has.
struct AlertsPart {
    open: u64,
    by_severity: BTreeMap<String, u64>,
    newest: Vec<sources::Alert>,
    truncated: bool,
    /// Which method the numbers came from, so a reader can tell an exact
    /// count from one counted out of a capped list.
    from: &'static str,
}

/// `alerts.summary` knows how many alerts are open without listing them, so
/// it is asked first. A build whose monitor has no summary — or whose summary
/// this version cannot read — is counted from `alerts.list` instead.
async fn alerts_part(app: &App) -> Result<AlertsPart, Gap> {
    match sources::alert_summary(app).await {
        Ok(summary) => {
            return Ok(AlertsPart {
                open: summary.open,
                by_severity: summary.by_severity,
                newest: summary.newest,
                truncated: false,
                from: "alerts.summary",
            });
        }
        Err(gap) if gap.reason == Reason::NotInstalled || gap.reason == Reason::Malformed => {}
        Err(gap) => return Err(gap),
    }
    let alerts = sources::alerts(app, json!({"state": "open", "limit": SCAN})).await?;
    let open: Vec<sources::Alert> = alerts
        .iter()
        .filter(|alert| alert.is_open())
        .cloned()
        .collect();
    let mut by_severity: BTreeMap<String, u64> = BTreeMap::new();
    for alert in &open {
        *by_severity
            .entry(alert.severity.as_str().to_string())
            .or_default() += 1;
    }
    let mut newest = open.clone();
    newest.sort_by_key(|alert| std::cmp::Reverse(alert.started_at));
    newest.truncate(5);
    Ok(AlertsPart {
        open: open.len() as u64,
        by_severity,
        newest,
        truncated: alerts.len() >= SCAN,
        from: "alerts.list",
    })
}

// ------------------------------------------------------------------ backups

/// Backup coverage, counted from the core's own snapshot table rather than
/// asked of the configuration feature: every backup is stored there by
/// whoever took it, so the count is as true in a build without that feature
/// as in one with it.
fn backups_part(app: &App, devices: &Devices, now: i64) -> AppResult<Value> {
    let since = now - DAY_MS;
    let (mut with, mut without, mut changed) = (0, 0, 0);
    let mut oldest: Option<i64> = None;
    for row in devices.rows() {
        match configs::newest_backup(app.store(), &row.device.id)? {
            Some((at, _lines)) => {
                with += 1;
                oldest = Some(oldest.map_or(at, |old: i64| old.min(at)));
                if at >= since {
                    // A version only exists when its content differed from
                    // the one before it, so a device with a recent version
                    // that is not its first really changed recently.
                    let (recent, _) = configs::changes_of(
                        app.store(),
                        &row.device.id,
                        Period {
                            since,
                            until: now + 1,
                        },
                    )?;
                    if recent.iter().any(|change| !change.first) {
                        changed += 1;
                    }
                }
            }
            None => without += 1,
        }
    }
    Ok(json!({
        "devices_with_backup": with,
        "devices_without": without,
        "changed_24h": changed,
        "oldest_backup_at": oldest,
        "from": "store.configs",
    }))
}

// --------------------------------------------------------------------- jobs

fn jobs_part(app: &App, now: i64) -> AppResult<Value> {
    let running = jobs::list(
        app,
        &JobFilter {
            state: Some("running".into()),
            limit: Some(50),
            ..JobFilter::default()
        },
    )?;
    let finished = jobs::list(
        app,
        &JobFilter {
            state: Some("finished".into()),
            limit: Some(200),
            ..JobFilter::default()
        },
    )?;
    let since = now - DAY_MS;
    Ok(json!({
        "running": running.iter().map(JobRef::from).collect::<Vec<_>>(),
        "failed_24h": finished
            .iter()
            .filter(|job| job.state == JobState::Failed)
            .filter(|job| job.finished_at.is_some_and(|at| at >= since))
            .count(),
    }))
}

// ---------------------------------------------------------------- incidents

fn incidents_part(app: &App) -> AppResult<Value> {
    let listed = incidents::list(
        app,
        &incidents::ListParams {
            state: Some("unresolved".into()),
            limit: Some(5),
            ..incidents::ListParams::default()
        },
    )?;
    let mut by_severity = serde_json::Map::new();
    for severity in IncidentSeverity::ALL {
        by_severity.insert(
            severity.as_str().to_string(),
            listed
                .counts
                .by_severity
                .get(severity.as_str())
                .cloned()
                .unwrap_or(json!(0)),
        );
    }
    let count = |state: &str| {
        listed
            .counts
            .by_state
            .get(state)
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Ok(json!({
        "open": count("open"),
        "mitigated": count("mitigated"),
        "unresolved": count("open") + count("mitigated"),
        "by_severity": Value::Object(by_severity),
        "newest": listed.incidents,
    }))
}

// ----------------------------------------------------------------- activity

/// The audit trail's newest entries as activity lines.
fn audit_feed(app: &App, now: i64, devices: &Devices) -> AppResult<Vec<Value>> {
    let entries = app.store().log_query(&LogQuery {
        stream: Some("audit".into()),
        since: Some(now - 7 * DAY_MS),
        limit: Some(40),
        ..LogQuery::default()
    })?;
    Ok(entries
        .iter()
        .map(|entry| {
            let on = entry
                .device_id
                .as_deref()
                .filter(|id| devices.contains(id))
                .map(|id| format!(" ({})", devices.name(id)))
                .unwrap_or_default();
            activity(
                entry.ts,
                "audit",
                format!("{}{on}", entry.message),
                entry.device_id.as_deref(),
                Some(json!({"type": "audit", "id": entry.id.to_string()})),
            )
        })
        .collect())
}

pub fn register(builder: &mut AppBuilder) {
    builder
        .router
        .add("overview.get", |app: App, params: GetParams| async move {
            get(&app, &params).await
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::model::{Device, DeviceRow, DeviceState, LabRef};
    use pretty_assertions::assert_eq;

    #[test]
    fn a_missing_part_is_null_with_a_reason_beside_it() {
        let mut parts = Parts::default();
        parts.set("devices", json!({"total": 0}));
        parts.missing(
            "alerts",
            Gap {
                reason: Reason::NotInstalled,
                message: "`alerts.list` is not part of this build".into(),
                code: None,
            },
        );
        parts.failed("backups", "database error".into());
        assert_eq!(parts.object["alerts"], Value::Null, "never a zero");
        assert_eq!(parts.object["backups"], Value::Null);
        assert_eq!(parts.unavailable.len(), 2);
        assert_eq!(parts.unavailable[0].section, "alerts");
        assert_eq!(parts.unavailable[0].reason, Reason::NotInstalled);
        assert_eq!(parts.unavailable[1].reason, Reason::Failed);
    }

    #[test]
    fn device_counts_separate_unknown_from_up() {
        let row = |name: &str, site: &str, reachable: Option<bool>, lab: bool| DeviceRow {
            device: Device {
                id: name.into(),
                name: name.into(),
                site: site.into(),
                lab: lab.then(LabRef::default),
                ..Device::default()
            },
            state: DeviceState {
                reachable,
                ..DeviceState::default()
            },
        };
        let devices = Devices::from_rows(vec![
            row("SW1", "HQ", Some(true), false),
            row("SW2", "HQ", Some(false), false),
            row("SW3", "HQ", None, false),
            row("LAB1", "Lab", Some(true), true),
        ]);
        let part = devices_part(&devices);
        assert_eq!(part["total"], 4);
        assert_eq!(part["up"], 2);
        assert_eq!(part["down"], 1);
        assert_eq!(part["unknown"], 1, "never polled is not up");
        assert_eq!(part["lab"], 1);
        assert_eq!(part["by_site"][0]["site"], "HQ");
        assert_eq!(part["by_site"][0]["total"], 3);
        assert_eq!(part["by_site"][0]["down"], 1);
        assert_eq!(part["by_site"][0]["unknown"], 1);
        assert_eq!(part["by_site"][1]["site"], "Lab");
    }
}
