//! The other features ops reads from, each of which may be missing.
//!
//! Everything ops knows about another feature's RPC surface is in this file:
//! the methods it calls, the parameters it sends and the few fields it reads
//! from the answers. Rows are parsed strictly: a row lacking a field ops
//! needs fails the read as `malformed` rather than being filled in. A read
//! that did not work is a [`Gap`]; callers turn it into a `null` section and
//! an entry in their `unavailable` list, so "not installed", "failed" and
//! "nothing yet" stay distinguishable from an honest zero.
//!
//! | Method | Sent | Read |
//! |---|---|---|
//! | `alerts.list` | `{state?: "open", since?, limit}` | `[Alert]` (or `{alerts: [..]}`) |
//! | `alerts.get` | `{id}` | `Alert`; `not_found` or `null` = no such alert |
//! | `changes.list` | `{state?, limit}` | `[ChangeSummary]` (or `{changes: [..]}`), newest first |
//! | `changes.get` | `{id}` | `ChangeDetail` |
//! | `config.devices` | `{filter?}` | `[ConfigDevice]` (or `{devices: [..]}`) |
//! | `compliance.results` | `{status?, limit, cursor?}` | `{findings, summary, run_at? \| at?, next_cursor?}` |
//! | `compliance.runs` | `{limit: 1}` | `[{at}]` (or `{runs: [..]}`), newest first |
//! | `compliance.policies.list` | `{}` | `[{id, name, rules: [{id, title}]}]` |
//! | `insights.list` | `{state: "open", limit}` | `{insights: [Insight], counts?: {by_severity?}}` (or `[Insight]`) |
//! | `config.search` | `{query, limit, context: 0}` | `{matches: [..]}` passed through |
//! | `ipam.search` | `{query}` | `{prefixes?, addresses?, vlans?}` passed through |
//! | `logs.query` | `{text, limit}` | `{entries: [..]}` passed through |
//! | `locate.host` | `{query}` | the answer, passed through; `not_found` or `null` = not located |

use std::collections::HashSet;
use std::time::Duration;

use nocturne_app::{App, AppError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

use crate::OpsState;

/// How long an overview, handover or report waits for another feature.
pub const BUDGET: Duration = Duration::from_secs(5);

/// The RPC methods of this build, read once.
#[derive(Debug, Default)]
pub struct Features {
    methods: HashSet<&'static str>,
}

impl Features {
    pub fn read(app: &App) -> Features {
        Features {
            methods: app.methods().into_iter().collect(),
        }
    }
}

/// Whether `method` exists in this build.
pub fn has(app: &App, method: &str) -> bool {
    app.state::<OpsState>()
        .features
        .get_or_init(|| Features::read(app))
        .methods
        .contains(method)
}

/// Why a section has no data.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The feature is not part of this build.
    NotInstalled,
    /// The feature answered with an error.
    Failed,
    /// The feature did not answer in time.
    Timeout,
    /// The feature answered with something this version of ops cannot read.
    Malformed,
    /// The feature works but has nothing to say yet (no run, no backup, no samples).
    NoData,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::NotInstalled => "not_installed",
            Reason::Failed => "failed",
            Reason::Timeout => "timeout",
            Reason::Malformed => "malformed",
            Reason::NoData => "no_data",
        }
    }
}

/// A section that could not be filled, as results report it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Unavailable {
    pub section: String,
    pub reason: Reason,
    pub message: String,
}

/// A read from another feature that did not produce data.
#[derive(Clone, Debug, PartialEq)]
pub struct Gap {
    pub reason: Reason,
    pub message: String,
    /// The other feature's error code, when it answered with an error.
    pub code: Option<&'static str>,
}

impl Gap {
    pub fn no_data(message: impl Into<String>) -> Gap {
        Gap {
            reason: Reason::NoData,
            message: message.into(),
            code: None,
        }
    }

    fn malformed(method: &str, detail: impl std::fmt::Display) -> Gap {
        Gap {
            reason: Reason::Malformed,
            message: format!(
                "`{method}` answered with something this version cannot read: {detail}"
            ),
            code: None,
        }
    }

    pub fn for_section(self, section: &str) -> Unavailable {
        Unavailable {
            section: section.to_string(),
            reason: self.reason,
            message: self.message,
        }
    }

    /// For callers that cannot do without the source.
    pub fn into_error(self) -> AppError {
        AppError::unavailable(self.message)
    }

    pub fn is_not_found(&self) -> bool {
        self.code == Some("not_found")
    }
}

/// Call another feature, waiting at most `budget`.
pub async fn call(app: &App, method: &str, params: Value, budget: Duration) -> Result<Value, Gap> {
    let missing = || Gap {
        reason: Reason::NotInstalled,
        message: format!("`{method}` is not part of this build"),
        code: None,
    };
    if !has(app, method) {
        return Err(missing());
    }
    match tokio::time::timeout(budget, app.rpc(method, params)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) if err.code == "unknown_method" => Err(missing()),
        Ok(Err(err)) => Err(Gap {
            reason: Reason::Failed,
            message: format!("`{method}` failed: {}", err.message),
            code: Some(err.code),
        }),
        Err(_) => Err(Gap {
            reason: Reason::Timeout,
            message: format!("`{method}` did not answer within {} ms", budget.as_millis()),
            code: None,
        }),
    }
}

/// The list in an answer that is either a bare array or an object holding it under `key`.
pub fn list(method: &str, value: Value, key: &str) -> Result<Vec<Value>, Gap> {
    match value {
        Value::Array(items) => Ok(items),
        Value::Object(mut fields) => match fields.remove(key) {
            Some(Value::Array(items)) => Ok(items),
            _ => Err(Gap::malformed(method, format!("no list under `{key}`"))),
        },
        _ => Err(Gap::malformed(method, "not a list")),
    }
}

fn one<T: DeserializeOwned>(method: &str, value: Value) -> Result<T, Gap> {
    serde_json::from_value(value).map_err(|err| Gap::malformed(method, err))
}

fn rows<T: DeserializeOwned>(method: &str, value: Value, key: &str) -> Result<Vec<T>, Gap> {
    list(method, value, key)?
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            serde_json::from_value(row)
                .map_err(|err| Gap::malformed(method, format!("item {index}: {err}")))
        })
        .collect()
}

/// A name that may be absent, empty or (in a future version) not a string.
fn name<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) if !text.trim().is_empty() => Some(text),
        _ => None,
    })
}

// ------------------------------------------------------------------ alerts

/// Alert and insight severities, least severe first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub const MOST_SEVERE_FIRST: [Severity; 5] = [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
        Severity::Info,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

/// What ops reads of an alert (`docs/dev/features/monitor.md`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Alert {
    pub id: String,
    pub severity: Severity,
    pub title: String,
    /// When it fired.
    pub started_at: i64,
    #[serde(default, alias = "acknowledged_at")]
    pub acked_at: Option<i64>,
    #[serde(default)]
    pub resolved_at: Option<i64>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    /// The device's name as the alert recorded it.
    #[serde(default, deserialize_with = "name")]
    pub device: Option<String>,
    #[serde(default)]
    pub rule_id: Option<String>,
    #[serde(default, deserialize_with = "name")]
    pub rule_name: Option<String>,
    #[serde(default)]
    pub lab: Option<bool>,
}

impl Alert {
    pub fn is_open(&self) -> bool {
        self.resolved_at.is_none() && self.state.as_deref() != Some("resolved")
    }

    /// Whether the alert was firing at some moment of `[since, until)`.
    pub fn active_in(&self, since: i64, until: i64) -> bool {
        self.started_at < until && self.resolved_at.is_none_or(|resolved| resolved >= since)
    }
}

/// `alerts.summary`'s answer: the one place that knows how many alerts are
/// open without listing them.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct AlertSummary {
    pub open: u64,
    #[serde(default)]
    pub by_severity: std::collections::BTreeMap<String, u64>,
    /// The newest open alert, or several if a build sends several.
    ///
    /// The monitoring feature sends one (`Alert | null`); this was written
    /// expecting a list, so the whole answer failed to parse and every
    /// overview quietly fell back to counting a capped `alerts.list` —
    /// turning an exact number into "at least 500". Nothing said so, because
    /// a fallback that works is the hardest kind of fault to notice. Both
    /// shapes are read now, which is the right way round for a reader: the
    /// feature it asks may be any version, or may not be the one we expect
    /// at all.
    #[serde(default, deserialize_with = "one_or_many")]
    pub newest: Vec<Alert>,
}

/// Accepts `null`, one alert, or a list of them — and anything else is read
/// as none, because the count beside it is what an overview needs and losing
/// the whole summary over this field is how the fallback went unnoticed.
fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<Alert>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect(),
        Value::Object(_) => serde_json::from_value(value)
            .map(|one| vec![one])
            .unwrap_or_default(),
        _ => Vec::new(),
    })
}

#[cfg(test)]
mod alert_summary_tests {
    use super::*;

    fn alert() -> Value {
        json!({
            "id": "a1",
            "severity": "high",
            "title": "SW1 is down",
            "started_at": 1,
            "state": "firing"
        })
    }

    #[test]
    fn the_newest_alert_is_read_whichever_shape_it_arrives_in() {
        // `nocturne-monitor` sends one or null; another build might send a
        // list. A reader asks whatever feature happens to be installed, so it
        // takes all three rather than failing the whole summary over a field
        // it could do without.
        let one: AlertSummary =
            serde_json::from_value(json!({"open": 3, "newest": alert()})).unwrap();
        assert_eq!((one.open, one.newest.len()), (3, 1));

        let many: AlertSummary =
            serde_json::from_value(json!({"open": 3, "newest": [alert(), alert()]})).unwrap();
        assert_eq!(many.newest.len(), 2);

        let none: AlertSummary =
            serde_json::from_value(json!({"open": 0, "newest": Value::Null})).unwrap();
        assert!(none.newest.is_empty());

        let absent: AlertSummary = serde_json::from_value(json!({"open": 0})).unwrap();
        assert!(absent.newest.is_empty());

        // And the count still arrives when the alert itself cannot be read,
        // because the count is the part the overview actually needs.
        let odd: AlertSummary =
            serde_json::from_value(json!({"open": 7, "newest": "tomorrow"})).unwrap();
        assert_eq!((odd.open, odd.newest.len()), (7, 0));
    }
}

/// The alert summary, when this build has one.
pub async fn alert_summary(app: &App) -> Result<AlertSummary, Gap> {
    let value = call(app, "alerts.summary", json!({}), BUDGET).await?;
    one("alerts.summary", value)
}

pub async fn alerts(app: &App, params: Value) -> Result<Vec<Alert>, Gap> {
    let value = call(app, "alerts.list", params, BUDGET).await?;
    rows("alerts.list", value, "alerts")
}

/// One alert; `None` when it does not exist (any more).
pub async fn alert(app: &App, id: &str) -> Result<Option<Alert>, Gap> {
    match call(app, "alerts.get", json!({ "id": id }), BUDGET).await {
        Ok(Value::Null) => Ok(None),
        Ok(value) => one("alerts.get", value).map(Some),
        Err(gap) if gap.is_not_found() => Ok(None),
        Err(gap) => Err(gap),
    }
}

// ----------------------------------------------------------------- changes

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct TargetDevice {
    device_id: String,
}

/// What ops reads of a change in a list (`docs/dev/features/changes.md`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ChangeSummary {
    pub id: String,
    pub title: String,
    /// `draft`, `planned`, `approved`, `applying`, `applied`, `failed`,
    /// `rolled_back`, `partially_applied` or `cancelled`.
    pub state: String,
    pub created_at: i64,
    /// Taken as the time the change reached its current state.
    pub updated_at: i64,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    device_ids: Option<Vec<String>>,
    #[serde(default)]
    targets: Option<Vec<TargetDevice>>,
    /// What `changes.list` actually sends. Features reach each other by
    /// method name and have to cope with the shape they are given, so all
    /// three spellings are read — but this is the one that is there, and
    /// leaving it out meant [`Self::devices`] was always `None` in a real
    /// build. Everything that asks which devices a change touched then
    /// answered "none": no change was ever suggested for an incident, no
    /// applied-change window ever covered a configuration change, and the
    /// shift handover reported every change the pipeline itself had made as
    /// `unplanned` — accusing somebody of working on the console.
    #[serde(default)]
    devices: Option<Vec<TargetDevice>>,
}

impl ChangeSummary {
    /// The devices the change touched, from whichever of the three spellings
    /// the change engine sent; `None` when the summary carries none of them.
    pub fn devices(&self) -> Option<Vec<String>> {
        let from = |targets: &Vec<TargetDevice>| -> Vec<String> {
            targets
                .iter()
                .map(|target| target.device_id.clone())
                .collect()
        };
        self.device_ids
            .clone()
            .or_else(|| self.devices.as_ref().map(from))
            .or_else(|| self.targets.as_ref().map(from))
    }

    pub fn is_awaiting_approval(&self) -> bool {
        self.state == "planned"
    }

    pub fn is_applied(&self) -> bool {
        self.state == "applied"
    }

    /// Ended without doing (all of) what it set out to do.
    pub fn is_failure(&self) -> bool {
        matches!(
            self.state.as_str(),
            "failed" | "rolled_back" | "partially_applied"
        )
    }

    /// Reached a device, whatever the outcome.
    pub fn was_executed(&self) -> bool {
        self.is_applied() || self.is_failure() || self.state == "applying"
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct TargetResult {
    /// `pending`, `applying`, `applied`, `failed`, `rolled_back`,
    /// `rollback_failed`, `skipped` or `guard_fired`.
    pub state: String,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub finished_at: Option<i64>,
    #[serde(default)]
    pub before_version_id: Option<i64>,
    #[serde(default)]
    pub after_version_id: Option<i64>,
    #[serde(default)]
    pub error: Option<String>,
}

impl TargetResult {
    /// The device was (or is being) configured: not merely queued or skipped.
    pub fn touched_device(&self) -> bool {
        !matches!(self.state.as_str(), "pending" | "skipped")
    }

    /// A time is only real when the change engine stamped it.
    pub fn started(&self) -> Option<i64> {
        self.started_at.filter(|at| *at > 0)
    }

    pub fn finished(&self) -> Option<i64> {
        self.finished_at.filter(|at| *at > 0)
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Target {
    pub device_id: String,
    #[serde(default)]
    pub result: Option<TargetResult>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Approval {
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub at: Option<i64>,
}

/// What ops reads of a whole change.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ChangeDetail {
    pub id: String,
    pub title: String,
    pub state: String,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub ticket: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    pub targets: Vec<Target>,
    #[serde(default)]
    pub approval: Option<Approval>,
}

impl ChangeDetail {
    /// The change as a list would show it.
    pub fn summary(&self) -> ChangeSummary {
        ChangeSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            state: self.state.clone(),
            created_at: self.created_at,
            updated_at: self.updated_at,
            origin: self.origin.clone(),
            ticket: self.ticket.clone(),
            created_by: self.created_by.clone(),
            device_ids: Some(
                self.targets
                    .iter()
                    .map(|target| target.device_id.clone())
                    .collect(),
            ),
            // Built here from `device_ids`, which is already filled in above.
            devices: None,
            targets: None,
        }
    }
}

pub async fn changes(app: &App, params: Value) -> Result<Vec<ChangeSummary>, Gap> {
    change_rows(call(app, "changes.list", params, BUDGET).await?)
}

/// `changes.list`'s answer, for callers that made the call themselves.
pub fn change_rows(value: Value) -> Result<Vec<ChangeSummary>, Gap> {
    rows("changes.list", value, "changes")
}

/// One change with its targets; `None` when it does not exist (any more).
pub async fn change(app: &App, id: &str) -> Result<Option<ChangeDetail>, Gap> {
    match call(app, "changes.get", json!({ "id": id }), BUDGET).await {
        Ok(Value::Null) => Ok(None),
        Ok(value) => one("changes.get", value).map(Some),
        Err(gap) if gap.is_not_found() => Ok(None),
        Err(gap) => Err(gap),
    }
}

// ------------------------------------------------------------------ config

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigHead {
    pub fetched_at: i64,
}

/// A row of the backup coverage table (`config.devices`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ConfigDevice {
    pub device_id: String,
    #[serde(default, deserialize_with = "name")]
    pub name: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    /// The newest running-config backup; `None` = never backed up.
    #[serde(default)]
    pub running: Option<ConfigHead>,
    #[serde(default)]
    pub versions: Option<u64>,
    #[serde(default)]
    pub last_change_at: Option<i64>,
    #[serde(default)]
    pub last_error: Option<String>,
}

pub async fn config_devices(app: &App, filter: Option<Value>) -> Result<Vec<ConfigDevice>, Gap> {
    let params = match filter {
        Some(filter) => json!({ "filter": filter }),
        None => json!({}),
    };
    let value = call(app, "config.devices", params, BUDGET).await?;
    rows("config.devices", value, "devices")
}

// -------------------------------------------------------------- compliance

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Finding {
    pub policy_id: String,
    pub rule_id: String,
    pub device_id: String,
    /// `pass`, `fail`, `not_applicable` or `not_evaluated`.
    pub status: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct ComplianceSummary {
    #[serde(default)]
    pub score_pct: Option<f64>,
    #[serde(default)]
    pub devices: Option<u64>,
    #[serde(default)]
    pub rules: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct CompliancePage {
    findings: Vec<Finding>,
    summary: ComplianceSummary,
    #[serde(default, alias = "at")]
    run_at: Option<i64>,
    #[serde(default)]
    next_cursor: Option<Value>,
}

/// The latest compliance run.
#[derive(Clone, Debug, PartialEq)]
pub struct Compliance {
    pub findings: Vec<Finding>,
    pub summary: ComplianceSummary,
    /// When the run happened, if the compliance feature says.
    pub run_at: Option<i64>,
    /// The findings were cut off: the feature kept returning pages past the limit.
    pub truncated: bool,
}

const COMPLIANCE_PAGE: usize = 5_000;
const COMPLIANCE_MAX_PAGES: usize = 20;

/// The findings of the latest run, optionally only those with `status`.
/// A build without any run yet answers `no_data`.
pub async fn compliance(app: &App, status: Option<&str>) -> Result<Compliance, Gap> {
    let mut findings = Vec::new();
    let mut cursor: Option<Value> = None;
    let mut first: Option<(ComplianceSummary, Option<i64>)> = None;
    let mut truncated = false;
    for page in 0..COMPLIANCE_MAX_PAGES {
        let mut params = json!({ "limit": COMPLIANCE_PAGE });
        if let Some(status) = status {
            params["status"] = json!(status);
        }
        if let Some(cursor) = &cursor {
            params["cursor"] = cursor.clone();
        }
        let value = match call(app, "compliance.results", params, BUDGET).await {
            Ok(value) => value,
            Err(gap) if gap.is_not_found() => {
                return Err(Gap::no_data("no compliance run has been made yet"));
            }
            Err(gap) => return Err(gap),
        };
        let page_data: CompliancePage = one("compliance.results", value)?;
        let full = page_data.findings.len() >= COMPLIANCE_PAGE;
        findings.extend(page_data.findings);
        first.get_or_insert((page_data.summary, page_data.run_at));
        cursor = page_data.next_cursor.filter(|cursor| !cursor.is_null());
        if cursor.is_none() {
            // A full page without a way to continue may hide more findings.
            truncated = full;
            break;
        }
        truncated = page + 1 == COMPLIANCE_MAX_PAGES;
    }
    let (summary, run_at) = first.unwrap_or_default();
    let run_at = match run_at {
        Some(at) => Some(at),
        None => latest_compliance_run(app).await,
    };
    if summary.score_pct.is_none() && findings.is_empty() {
        // A run that judged nothing is not the same as no run at all, and
        // saying the second when the first happened is simply untrue — the
        // screen quotes this sentence. Ask what has been run before deciding
        // which it is, and do not guess at why a run judged nothing: that
        // depends on what was stored, which this does not know.
        return Err(match run_at {
            Some(_) => Gap::no_data(
                "a compliance run has been made, but it judged nothing and reached no score",
            ),
            None => Gap::no_data("no compliance run has been made yet"),
        });
    }
    Ok(Compliance {
        findings,
        summary,
        run_at,
        truncated,
    })
}

#[derive(Deserialize)]
struct ComplianceRun {
    at: i64,
}

/// The time of the newest run from `compliance.runs`, when that method can tell.
async fn latest_compliance_run(app: &App) -> Option<i64> {
    let value = call(app, "compliance.runs", json!({ "limit": 1 }), BUDGET)
        .await
        .ok()?;
    let runs: Vec<ComplianceRun> = rows("compliance.runs", value, "runs").ok()?;
    runs.iter().map(|run| run.at).max()
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct PolicyRule {
    pub id: String,
    #[serde(default, deserialize_with = "name")]
    pub title: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Policy {
    pub id: String,
    #[serde(default, deserialize_with = "name")]
    pub name: Option<String>,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

/// Policy and rule names, to label findings with.
pub async fn compliance_policies(app: &App) -> Result<Vec<Policy>, Gap> {
    let value = call(app, "compliance.policies.list", json!({}), BUDGET).await?;
    rows("compliance.policies.list", value, "policies")
}

// ---------------------------------------------------------------- insights

/// What ops reads of an insight (`docs/dev/features/insights.md`).
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Insight {
    pub id: String,
    pub title: String,
    pub severity: Severity,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default, deserialize_with = "name")]
    pub device: Option<String>,
    #[serde(default, deserialize_with = "name")]
    pub subject: Option<String>,
    #[serde(default)]
    pub check: Option<String>,
    #[serde(default)]
    pub first_seen: Option<i64>,
    #[serde(default)]
    pub last_seen: Option<i64>,
    #[serde(default)]
    pub lab: Option<bool>,
}

/// The open insights.
#[derive(Clone, Debug, PartialEq)]
pub struct Insights {
    pub open: Vec<Insight>,
    /// How many are open in total, when the feature counts them (`counts.by_severity`).
    pub total: Option<u64>,
}

pub async fn open_insights(app: &App, limit: usize) -> Result<Insights, Gap> {
    let params = json!({ "state": "open", "limit": limit });
    let value = call(app, "insights.list", params, BUDGET).await?;
    let total = value
        .pointer("/counts/by_severity")
        .and_then(Value::as_object)
        .and_then(|counts| counts.values().map(Value::as_u64).sum::<Option<u64>>());
    let open: Vec<Insight> = rows("insights.list", value, "insights")?;
    Ok(Insights {
        open: open
            .into_iter()
            .filter(|insight| insight.state.as_deref().is_none_or(|state| state == "open"))
            .collect(),
        total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn lists_are_read_bare_or_under_their_key() {
        assert_eq!(
            list("m", json!([1, 2]), "items").unwrap(),
            [json!(1), json!(2)]
        );
        assert_eq!(
            list("m", json!({"items": [1], "total": 9}), "items").unwrap(),
            [json!(1)]
        );
        let gap = list("alerts.list", json!({"rows": []}), "alerts").unwrap_err();
        assert_eq!(gap.reason, Reason::Malformed);
        assert!(gap.message.contains("`alerts.list`") && gap.message.contains("`alerts`"));
        assert_eq!(
            list("m", json!("text"), "items").unwrap_err().reason,
            Reason::Malformed
        );
    }

    #[test]
    fn alerts_are_parsed_strictly_but_tolerate_extra_fields() {
        // The monitor's full shape: everything ops does not read is ignored.
        let full = json!({
            "id": "a1", "rule_id": "r1", "rule_name": "Device down", "device_id": "d1", "device": "SW1",
            "instance": null, "severity": "critical", "state": "acknowledged", "title": "SW1 is down",
            "detail": "no answer to 3 echoes", "value": null, "started_at": 1_000, "last_seen_at": 5_000,
            "resolved_at": null, "acked_at": 2_000, "ack_note": "on it", "snoozed_until": null, "count": 4,
            "muted_by_maintenance": false, "lab": false
        });
        let alerts: Vec<Alert> = rows("alerts.list", json!([full]), "alerts").unwrap();
        assert_eq!(alerts[0].severity, Severity::Critical);
        assert_eq!(alerts[0].acked_at, Some(2_000));
        assert_eq!(alerts[0].device.as_deref(), Some("SW1"));
        assert!(alerts[0].is_open());

        // A missing required field names the row and the field.
        let gap = rows::<Alert>(
            "alerts.list",
            json!([{"id": "a1", "severity": "high", "title": "x", "started_at": 1}, {"id": "a2", "severity": "high", "title": "y"}]),
            "alerts",
        )
        .unwrap_err();
        assert_eq!(gap.reason, Reason::Malformed);
        assert!(
            gap.message.contains("item 1") && gap.message.contains("started_at"),
            "{}",
            gap.message
        );

        // A severity ops does not know is not mapped to a neighbour.
        let gap = rows::<Alert>(
            "alerts.list",
            json!([{"id": "a", "severity": "warning", "title": "x", "started_at": 1}]),
            "alerts",
        )
        .unwrap_err();
        assert!(gap.message.contains("warning"), "{}", gap.message);

        // The other spelling of the acknowledgement time is understood.
        let alert: Alert = one(
            "alerts.get",
            json!({"id": "a", "severity": "low", "title": "x", "started_at": 1, "acknowledged_at": 7, "resolved_at": 9, "device": 12}),
        )
        .unwrap();
        assert_eq!((alert.acked_at, alert.device.clone()), (Some(7), None));
        assert!(!alert.is_open());
    }

    #[test]
    fn alert_activity_windows_are_half_open() {
        let alert = |started_at, resolved_at| Alert {
            id: "a".into(),
            severity: Severity::Low,
            title: "t".into(),
            started_at,
            acked_at: None,
            resolved_at,
            state: None,
            device_id: None,
            device: None,
            rule_id: None,
            rule_name: None,
            lab: None,
        };
        assert!(alert(100, None).active_in(100, 200));
        assert!(alert(50, None).active_in(100, 200), "still firing");
        assert!(alert(199, Some(500)).active_in(100, 200));
        assert!(
            !alert(200, None).active_in(100, 200),
            "fired at the end of the window"
        );
        assert!(
            alert(10, Some(100)).active_in(100, 200),
            "resolved at the start"
        );
        assert!(
            !alert(10, Some(99)).active_in(100, 200),
            "resolved before it"
        );
    }

    #[test]
    fn change_devices_come_from_either_shape() {
        let with_ids: ChangeSummary = one(
            "changes.list",
            json!({"id": "c", "title": "t", "state": "applied", "created_at": 1, "updated_at": 2, "device_ids": ["d1", "d2"]}),
        )
        .unwrap();
        assert_eq!(
            with_ids.devices(),
            Some(vec!["d1".to_string(), "d2".to_string()])
        );
        let with_targets: ChangeSummary = one(
            "changes.list",
            json!({"id": "c", "title": "t", "state": "failed", "created_at": 1, "updated_at": 2, "targets": [{"device_id": "d3", "intent": {"type": "lines", "lines": []}}]}),
        )
        .unwrap();
        assert_eq!(with_targets.devices(), Some(vec!["d3".to_string()]));
        assert!(with_targets.is_failure() && with_targets.was_executed());
        let bare: ChangeSummary = one(
            "changes.list",
            json!({"id": "c", "title": "t", "state": "planned", "created_at": 1, "updated_at": 2}),
        )
        .unwrap();
        assert_eq!(bare.devices(), None);
        assert!(bare.is_awaiting_approval() && !bare.was_executed());

        let detail: ChangeDetail = one(
            "changes.get",
            json!({"id": "c", "title": "t", "state": "applied", "created_at": 1, "updated_at": 2,
                   "targets": [{"device_id": "d1", "result": {"state": "applied", "started_at": 10, "finished_at": 0, "after_version_id": 7}}, {"device_id": "d2"}]}),
        )
        .unwrap();
        assert_eq!(
            detail.summary().devices(),
            Some(vec!["d1".to_string(), "d2".to_string()])
        );
        let result = detail.targets[0].result.as_ref().unwrap();
        assert_eq!(
            (result.started(), result.finished()),
            (Some(10), None),
            "0 is not a time"
        );
        assert!(result.touched_device());
    }
}
