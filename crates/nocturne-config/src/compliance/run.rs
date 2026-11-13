//! Evaluating policies against stored configurations, and reading the result.
//!
//! A run touches no device: it reads the latest stored running configuration
//! of every device in scope and answers each rule from the text. That makes
//! it instant, repeatable and usable with the network unreachable — and it
//! means a device that was never backed up is `not_evaluated`, never `pass`.

use std::collections::BTreeMap;

use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::eval::{self, Subject};
use super::model::{Finding, Policy, Rule, Severity, Status};
use crate::db::{self, FindingFilter, FindingRow};
use crate::redact::Redaction;
use crate::scope::Target;
use crate::versions::{self, Kind};

/// Findings are kept for this many runs, summaries for this many more. A run
/// is one row per device and rule, so the history of scores stays cheap while
/// the detail of what failed is there for the runs anybody still looks at.
const KEEP_FINDINGS_OF_RUNS: u32 = 20;
const KEEP_RUNS: u32 = 500;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeverityCount {
    pub pass: usize,
    pub fail: usize,
}

/// What a run found, in one line for the dashboard.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RunSummary {
    pub run_id: String,
    pub at: i64,
    /// Devices at least one policy covered.
    pub devices: usize,
    /// Devices the request asked about, whether a policy covered them or not.
    pub devices_requested: usize,
    /// Devices in scope with no stored running configuration.
    pub devices_without_backup: usize,
    pub policies: usize,
    /// Rules evaluated, counted once per policy.
    pub rules: usize,
    pub pass: usize,
    pub fail: usize,
    pub not_applicable: usize,
    pub not_evaluated: usize,
    /// Share of the judged findings that passed; `None` when nothing was judged.
    pub score_pct: Option<f64>,
    pub by_severity: BTreeMap<String, SeverityCount>,
    /// Whether the findings of this run are still stored (older runs keep only this summary).
    pub has_findings: bool,
}

impl RunSummary {
    fn count(&mut self, finding: &Finding) {
        let severity = self
            .by_severity
            .entry(finding.severity.as_str().to_string())
            .or_default();
        match finding.status {
            Status::Pass => {
                self.pass += 1;
                severity.pass += 1;
            }
            Status::Fail => {
                self.fail += 1;
                severity.fail += 1;
            }
            Status::NotApplicable => self.not_applicable += 1,
            Status::NotEvaluated => self.not_evaluated += 1,
        }
    }

    fn finish(&mut self) {
        let judged = self.pass + self.fail;
        self.score_pct = (judged > 0).then(|| {
            let share = self.pass as f64 * 100.0 / judged as f64;
            (share * 10.0).round() / 10.0
        });
        for severity in Severity::ALL {
            self.by_severity
                .entry(severity.as_str().to_string())
                .or_default();
        }
    }
}

/// A device as a run sees it: its configuration, parsed, and how to redact it.
struct Subjected {
    target: Target,
    lineage: Vec<String>,
    os: String,
    subject: Option<Subject>,
    version_id: Option<i64>,
    fetched_at: Option<i64>,
    redaction: Redaction,
}

fn prepare(app: &App, target: Target) -> AppResult<Subjected> {
    let lineage = target.profile_lineage(app);
    let os = target
        .platform(app)
        .map(|platform| platform.profile.os.clone())
        .unwrap_or_default();
    let redaction = Redaction::for_profiles(app, [target.profile.as_str()]);
    let latest = versions::latest(app, &target.device.id, Kind::Running)?;
    let (subject, version_id, fetched_at) = match latest {
        Some(version) => {
            let (_, text) = app.store().config_get(version.id)?;
            (
                Some(Subject::parse(&text, target.syntax(app))),
                Some(version.id),
                Some(version.last_seen),
            )
        }
        None => (None, None, None),
    };
    Ok(Subjected {
        target,
        lineage,
        os,
        subject,
        version_id,
        fetched_at,
        redaction,
    })
}

/// Dress a verdict up as a finding, hiding the secrets in its evidence.
///
/// A configuration is judged exactly as it is stored — a rule about a
/// community called `public` could never fire against text in which the
/// community has already become `<secret:1:weak>` — so redaction happens
/// here, on the way out, and nowhere earlier.
pub fn dress(verdict: eval::Verdict, redaction: &Redaction, base: Finding) -> Finding {
    Finding {
        status: verdict.status,
        evidence: verdict
            .evidence
            .into_iter()
            .map(|mut line| {
                line.line = redaction.text(&line.line);
                line
            })
            .collect(),
        missing: verdict.missing,
        detail: verdict.detail,
        sections: verdict.sections,
        ..base
    }
}

/// Evaluate one rule against one prepared device.
fn judge(device: &Subjected, policy: &Policy, rule: &Rule) -> Finding {
    let base = Finding {
        policy_id: policy.id.clone(),
        policy_name: policy.name.clone(),
        rule_id: rule.id.clone(),
        title: rule.title.clone(),
        severity: rule.severity,
        device_id: device.target.device.id.clone(),
        device_name: device.target.device.name.clone(),
        version_id: device.version_id,
        fetched_at: device.fetched_at,
        remediable: rule
            .remediation
            .as_ref()
            .is_some_and(|lines| !lines.is_empty()),
        ..Finding::default()
    };
    let Some(subject) = &device.subject else {
        return Finding {
            status: Status::NotEvaluated,
            detail: format!(
                "`{}` has no stored running configuration, so nothing is known about this rule. Back the device up first.",
                device.target.device.name
            ),
            ..base
        };
    };
    match eval::evaluate(&rule.check, subject) {
        Ok(verdict) => dress(verdict, &device.redaction, base),
        Err(err) => Finding {
            status: Status::NotEvaluated,
            detail: format!("the rule cannot be evaluated: {}", err.message),
            ..base
        },
    }
}

/// Evaluate `policies` against `targets` and store the run.
pub(crate) fn execute(
    app: &App,
    targets: Vec<Target>,
    policies: &[Policy],
) -> AppResult<Vec<Finding>> {
    let mut findings: Vec<Finding> = Vec::new();
    for target in targets {
        let device = prepare(app, target)?;
        for policy in policies {
            if !policy
                .scope
                .covers(&device.target.device, &device.lineage, &device.os)
            {
                continue;
            }
            for rule in &policy.rules {
                findings.push(judge(&device, policy, rule));
            }
        }
    }
    Ok(findings)
}

/// Count a set of findings.
pub fn summarize(findings: &[Finding], devices_requested: usize) -> RunSummary {
    let mut summary = RunSummary {
        devices_requested,
        has_findings: true,
        ..RunSummary::default()
    };
    let mut devices: Vec<&str> = Vec::new();
    let mut without_backup: Vec<&str> = Vec::new();
    let mut policies: Vec<&str> = Vec::new();
    let mut rules: Vec<(&str, &str)> = Vec::new();
    for finding in findings {
        summary.count(finding);
        for (seen, value) in [
            (&mut devices, finding.device_id.as_str()),
            (&mut policies, finding.policy_id.as_str()),
        ] {
            if !seen.contains(&value) {
                seen.push(value);
            }
        }
        let rule = (finding.policy_id.as_str(), finding.rule_id.as_str());
        if !rules.contains(&rule) {
            rules.push(rule);
        }
        if finding.version_id.is_none() && !without_backup.contains(&finding.device_id.as_str()) {
            without_backup.push(&finding.device_id);
        }
    }
    summary.devices = devices.len();
    summary.devices_without_backup = without_backup.len();
    summary.policies = policies.len();
    summary.rules = rules.len();
    summary.finish();
    summary
}

/// Store a run and its findings; returns the stored summary.
pub fn store(app: &App, findings: &[Finding], mut summary: RunSummary) -> AppResult<RunSummary> {
    summary.run_id = new_id();
    summary.at = now_ms();
    let rows: Vec<FindingRow> = findings
        .iter()
        .map(|finding| {
            Ok(FindingRow {
                device_id: finding.device_id.clone(),
                policy_id: finding.policy_id.clone(),
                rule_id: finding.rule_id.clone(),
                status: finding.status.as_str(),
                severity: finding.severity.as_str(),
                body: serde_json::to_string(finding)?,
            })
        })
        .collect::<AppResult<_>>()?;
    db::insert_run(
        app.store(),
        &summary.run_id,
        summary.at,
        &serde_json::to_string(&summary)?,
        &rows,
    )?;
    db::trim_runs(app.store(), KEEP_FINDINGS_OF_RUNS, KEEP_RUNS)?;
    Ok(summary)
}

fn summary_of(row: &db::RunRow) -> RunSummary {
    let mut summary: RunSummary = serde_json::from_str(&row.summary).unwrap_or_default();
    summary.run_id = row.id.clone();
    summary.at = row.at;
    summary.has_findings = row.has_findings;
    summary.finish();
    summary
}

/// Runs, newest first.
pub fn history(app: &App, before_id: Option<&str>, limit: usize) -> AppResult<Vec<RunSummary>> {
    let before_seq = match before_id {
        Some(id) => Some(
            db::run(app.store(), Some(id))?
                .ok_or_else(|| AppError::not_found(format!("compliance run `{id}`")))?
                .seq,
        ),
        None => None,
    };
    Ok(db::runs(app.store(), before_seq, limit)?
        .iter()
        .map(summary_of)
        .collect())
}

/// One page of a run's findings.
pub struct Page {
    pub summary: RunSummary,
    pub findings: Vec<Finding>,
    pub next_cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ResultsFilter {
    pub status: Option<String>,
    pub severity: Option<String>,
    pub device_id: Option<String>,
    pub policy_id: Option<String>,
    pub rule_id: Option<String>,
}

/// Read a run's findings. `run_id` of `None` means the most recent run.
pub fn results(
    app: &App,
    run_id: Option<&str>,
    filter: &ResultsFilter,
    cursor: Option<&str>,
    limit: usize,
) -> AppResult<Page> {
    let row = db::run(app.store(), run_id)?.ok_or_else(|| match run_id {
        Some(id) => AppError::not_found(format!("compliance run `{id}`")),
        None => AppError::new(
            "not_found",
            "no compliance run has been made yet; start one with `compliance.run`",
        ),
    })?;
    if let Some(status) = &filter.status
        && Status::parse(status).is_none()
    {
        return Err(AppError::invalid(format!(
            "`{status}` is not a status: pass, fail, not_applicable or not_evaluated"
        )));
    }
    if let Some(severity) = &filter.severity
        && Severity::parse(severity).is_none()
    {
        return Err(AppError::invalid(format!(
            "`{severity}` is not a severity: info, low, medium, high or critical"
        )));
    }
    let summary = summary_of(&row);
    let after_seq: i64 = match cursor {
        Some(cursor) => cursor.parse().map_err(|_| {
            AppError::invalid("`cursor` is not one this method returned".to_string())
        })?,
        None => 0,
    };
    let stored = db::findings(
        app.store(),
        row.seq,
        &FindingFilter {
            status: filter.status.clone(),
            severity: filter.severity.clone(),
            device_id: filter.device_id.clone(),
            policy_id: filter.policy_id.clone(),
            rule_id: filter.rule_id.clone(),
        },
        after_seq,
        limit + 1,
    )?;
    let more = stored.len() > limit;
    let mut next_cursor = None;
    let mut findings = Vec::with_capacity(stored.len().min(limit));
    for (seq, body) in stored.into_iter().take(limit) {
        findings.push(serde_json::from_str(&body)?);
        if more {
            next_cursor = Some(seq.to_string());
        }
    }
    Ok(Page {
        summary,
        findings,
        next_cursor,
    })
}

/// Announce a finished run.
pub fn announce(app: &App, summary: &RunSummary) {
    app.publish(
        "compliance.finished",
        &json!({"run_id": summary.run_id, "summary": summary}),
    );
}
