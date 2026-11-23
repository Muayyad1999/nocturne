//! Running the checks: once by hand, and automatically after every collection.
//!
//! A scan loads the world once, runs every enabled check over it, folds the
//! results into the ledger, and writes a coverage report. It takes no
//! decisions of its own: what is a finding is each check's business, and what
//! happens to a finding afterwards is the ledger's.
//!
//! Scans are serialised — two at once would race on the ledger and do the
//! same work twice — and the automatic one is debounced, because collecting
//! from forty devices publishes forty events in a few seconds and they want
//! to be one scan.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use nocturne_app::{App, AppError, AppResult, inventory, jobs};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::time::{Duration, Instant};

use crate::catalogue::{self, Check};
use crate::db;
use crate::links;
use crate::model::{Category, Finding, Insight, Severity, State};
use crate::report::CheckReport;
use crate::settings::{self, Problem, Settings};
use crate::world::{self, AppliedChange, LoadRequest, World};

/// How the reason of a check that a setting kept from running begins. A
/// scan limited to other checks keeps a check's last row only if that row
/// is what the check said when it ran, which this is not.
const COULD_NOT_RUN: &str = "it could not run, because a setting it reads cannot be used";

/// Scans in progress, and devices waiting for the next automatic one.
#[derive(Default)]
pub struct ScanState {
    running: tokio::sync::Mutex<()>,
    pub(crate) last_coverage: Mutex<Option<Arc<Coverage>>>,
}

/// What a check could and could not decide, per scan.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckCoverage {
    pub check: String,
    pub title: String,
    pub category: Category,
    pub enabled: bool,
    /// Devices the check reached a verdict for.
    pub evaluated: usize,
    /// Devices it could not decide for.
    pub skipped: usize,
    pub findings: usize,
    /// Why it could not decide, worst first: `(reason, devices)`.
    pub reasons: Vec<Reason>,
    /// When the scan this row is from ran. A scan limited to some checks
    /// keeps every other check's row from the last scan that ran it, so the
    /// rows of one coverage need not all be from the same scan.
    #[serde(default)]
    pub scanned_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reason {
    pub reason: String,
    pub devices: usize,
}

/// The whole last scan: what ran, over how much, and what it could not see.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Coverage {
    pub scanned_at: i64,
    pub devices: usize,
    /// How the links the link-based checks used were worked out.
    pub link_source: String,
    pub links: usize,
    pub checks: Vec<CheckCoverage>,
    /// Features that were not there, and the checks that wanted them.
    pub missing_sources: Vec<String>,
    pub took_ms: u64,
    /// Stored settings this scan could not use: each took its default, and
    /// the checks that read one did not run.
    #[serde(default)]
    pub settings_problems: Vec<Problem>,
}

/// What a scan did, as the job result and the event payload.
#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub devices: usize,
    pub opened: usize,
    pub resolved: usize,
    pub open: usize,
    pub findings: usize,
    pub checks_run: usize,
    pub took_ms: u64,
    /// As in the coverage.
    pub settings_problems: Vec<Problem>,
}

/// Turn a check's [`Finding`] into the insight the ledger keeps.
pub(crate) fn to_insight(
    finding: &Finding,
    category: Category,
    severity: Severity,
    device: &str,
    lab: bool,
    now: i64,
) -> Insight {
    Insight {
        id: finding.id(),
        check: finding.check.to_string(),
        category,
        severity,
        device_id: finding.device_id.clone(),
        device: device.to_string(),
        subject: finding.subject.clone(),
        title: finding.title.clone(),
        detail: finding.detail.clone(),
        evidence: finding.evidence.clone(),
        suggestion: finding.suggestion.clone(),
        related: finding.related.clone(),
        state: State::Open,
        first_seen: now,
        last_seen: now,
        resolved_at: None,
        dismissed_until: None,
        dismissed_at: None,
        dismiss_note: None,
        lab,
        occurrences: 1,
    }
}

/// The row of a check that did not run in this scan, for one reason given
/// for every device.
fn not_run(check: &Check, world: &World, enabled: bool, reasons: Vec<String>) -> CheckCoverage {
    CheckCoverage {
        check: check.id.to_string(),
        title: check.title.to_string(),
        category: check.category,
        enabled,
        evaluated: 0,
        skipped: world.target_count(),
        findings: 0,
        reasons: reasons
            .into_iter()
            .map(|reason| Reason {
                reason,
                devices: world.target_count(),
            })
            .collect(),
        scanned_at: world.now,
    }
}

/// A check whose settings cannot all be used: one reason per bad setting.
/// It is not switched off — whether it is meant to run is the user's — so it
/// counts as on and as having evaluated nothing.
fn could_not_run(check: &Check, world: &World, problems: &[Problem]) -> CheckCoverage {
    let reasons = problems
        .iter()
        .map(|problem| format!("{COULD_NOT_RUN}: {}", problem.message))
        .collect();
    not_run(check, world, true, reasons)
}

/// What a check said the last time a scan ran it, for a scan that was asked
/// to run others. Only a row of the check having run: one that said it was
/// switched off, or that a setting kept it from running, is about the
/// settings of then, and they may have changed since.
fn last_said(check: &Check, previous: Option<&Coverage>) -> Option<CheckCoverage> {
    let previous = previous?;
    let row = previous.checks.iter().find(|row| row.check == check.id)?;
    if !row.enabled
        || row
            .reasons
            .iter()
            .any(|reason| reason.reason.starts_with(COULD_NOT_RUN))
    {
        return None;
    }
    let mut row = row.clone();
    if row.scanned_at == 0 {
        // Stored before rows carried their own time: all of them were from
        // the scan the coverage was.
        row.scanned_at = previous.scanned_at;
    }
    Some(row)
}

/// Run the checks over a world that is already loaded. Pure apart from the
/// metric and log queries the checks make themselves, and the one thing a
/// scan writes: the ledger.
///
/// `only` names the checks to run, switched off or not: the operator is
/// explicit. The others are not run, and their rows say why — switched off
/// if they are, and otherwise what they said the last time they ran
/// (`previous`), never that they are switched off when they are not.
fn run_checks(
    world: &World,
    settings: &Settings,
    only: &[String],
    previous: Option<&Coverage>,
) -> (Vec<db::Outcome>, Coverage) {
    let mut outcomes = Vec::new();
    let mut coverage = Vec::new();

    for check in catalogue::CHECKS {
        let problems = settings.problems_for(check);
        let switched_off = settings.switch_readable(check.id) && !settings.enabled(check.id);
        let runs = if only.is_empty() {
            !switched_off
        } else {
            only.iter().any(|id| id == check.id)
        };
        if !runs {
            coverage.push(if switched_off {
                not_run(
                    check,
                    world,
                    false,
                    vec!["the check is switched off".into()],
                )
            } else if !problems.is_empty() {
                could_not_run(check, world, &problems)
            } else {
                last_said(check, previous).unwrap_or_else(|| {
                    not_run(
                        check,
                        world,
                        true,
                        vec!["not part of this scan, which was limited to other checks".into()],
                    )
                })
            });
            continue;
        }
        // Run on a default nobody chose, it would answer the wrong question,
        // and every finding it did not see would count towards resolving.
        // Left alone, its findings stay as they were.
        if !problems.is_empty() {
            coverage.push(could_not_run(check, world, &problems));
            continue;
        }
        let mut report = CheckReport::new(check.id);
        (check.run)(world, settings, &mut report);

        let mut reasons: BTreeMap<&'static str, usize> = BTreeMap::new();
        for (_, reason) in report.skips() {
            *reasons.entry(reason).or_insert(0) += 1;
        }
        let mut reasons: Vec<Reason> = reasons
            .into_iter()
            .map(|(reason, devices)| Reason {
                reason: reason.to_string(),
                devices,
            })
            .collect();
        reasons.sort_by(|a, b| b.devices.cmp(&a.devices).then(a.reason.cmp(&b.reason)));

        let found: Vec<Insight> = report
            .findings
            .iter()
            .filter_map(|finding| {
                let data = world.device(&finding.device_id)?;
                Some(to_insight(
                    finding,
                    check.category,
                    finding.severity.unwrap_or(check.severity),
                    data.name(),
                    data.lab(),
                    world.now,
                ))
            })
            .collect();

        coverage.push(CheckCoverage {
            check: check.id.to_string(),
            title: check.title.to_string(),
            category: check.category,
            enabled: true,
            evaluated: report.evaluated().len(),
            skipped: report.skips().count(),
            findings: found.len(),
            reasons,
            scanned_at: world.now,
        });
        outcomes.push(db::Outcome {
            check: check.id,
            found,
            evaluated: report.evaluated().clone(),
            undecided: report.undecided_ids().clone(),
        });
    }

    // From every row, kept ones too: what was missing when a check last ran
    // is still the reason its row says what it says.
    let missing: BTreeSet<String> = coverage
        .iter()
        .flat_map(|row| {
            row.reasons
                .iter()
                .filter(|reason| reason.reason.contains("feature is not installed"))
                .map(move |reason| format!("{}: {}", row.check, reason.reason))
        })
        .collect();
    let coverage = Coverage {
        scanned_at: world.now,
        devices: world.target_count(),
        link_source: world.link_source.to_string(),
        links: world.links.len(),
        checks: coverage,
        missing_sources: missing.into_iter().collect(),
        // Set by the caller, which knows when the scan started.
        took_ms: 0,
        settings_problems: settings.problems.clone(),
    };
    (outcomes, coverage)
}

/// Everything a scan needs that only another feature can answer.
struct FromOtherFeatures {
    topology: Option<Value>,
    applied_changes: Option<HashMap<String, Vec<AppliedChange>>>,
}

async fn ask_other_features(app: &App) -> FromOtherFeatures {
    let methods = app.methods();
    let topology = if methods.contains(&"topology.get") {
        app.rpc("topology.get", json!({"layer": "physical"}))
            .await
            .ok()
    } else {
        None
    };
    let applied_changes = if methods.contains(&"changes.list") {
        match app
            .rpc("changes.list", json!({"state": "applied", "limit": 200}))
            .await
        {
            Ok(changes) => {
                let mut per_device: HashMap<String, Vec<AppliedChange>> = HashMap::new();
                for change in changes.as_array().map(Vec::as_slice).unwrap_or_default() {
                    let applied = AppliedChange {
                        id: change["id"].as_str().unwrap_or_default().to_string(),
                        title: change["title"].as_str().unwrap_or_default().to_string(),
                        at: change["updated_at"].as_i64().unwrap_or_default(),
                    };
                    for device in change["devices"]
                        .as_array()
                        .map(Vec::as_slice)
                        .unwrap_or_default()
                    {
                        if let Some(device_id) = device["device_id"].as_str() {
                            per_device
                                .entry(device_id.to_string())
                                .or_default()
                                .push(applied.clone());
                        }
                    }
                }
                Some(per_device)
            }
            // The feature is there but could not answer: treat its data as
            // absent rather than as "no change was applied".
            Err(_) => None,
        }
    } else {
        None
    };
    FromOtherFeatures {
        topology,
        applied_changes,
    }
}

/// Run a scan. `device_ids` empty = every device; `checks` empty = every
/// enabled check.
pub async fn run(app: &App, device_ids: Vec<String>, checks: Vec<String>) -> AppResult<Summary> {
    for check in &checks {
        if catalogue::get(check).is_none() {
            return Err(AppError::invalid(format!(
                "there is no check called `{check}`; `insights.checks` lists them"
            )));
        }
    }
    let state = app.state::<ScanState>();
    let _serialised = state.running.lock().await;
    let settings = settings::load(app)?;
    let external = ask_other_features(app).await;
    // A scan of some checks keeps what the others said last. Read under the
    // lock, so it is the coverage of the scan before this one. One that
    // cannot be read leaves those rows saying they were not part of the scan.
    let previous = if checks.is_empty() {
        None
    } else {
        coverage(app).ok().flatten()
    };

    let app_for_scan = app.clone();
    let for_scan = settings.clone();
    let worker = tokio::task::spawn_blocking(move || {
        let settings = for_scan;
        let started = std::time::Instant::now();
        let now = nocturne_app::util::now_ms();
        let mut world = world::load(
            &app_for_scan,
            LoadRequest {
                device_ids: &device_ids,
                include_lab: settings.include_lab,
                links: Vec::new(),
                link_source: "none",
                applied_changes: external.applied_changes,
                now,
            },
        )?;
        match external.topology.as_ref() {
            Some(graph) => {
                world.links = links::from_topology(graph, &world.devices);
                world.link_source = "topology";
            }
            None => {
                world.links = links::from_neighbors(&world.devices);
                world.link_source = "neighbors";
            }
        }
        let (outcomes, coverage) = run_checks(&world, &settings, &checks, previous.as_ref());
        // Timed before the findings are stored rather than after, because the
        // coverage goes into the same transaction as them: the two must agree,
        // and `took_ms` missing the cost of its own write is a far smaller
        // inaccuracy than a reader seeing findings from a scan that, according
        // to the coverage, never happened.
        let coverage = Coverage {
            took_ms: started.elapsed().as_millis() as u64,
            ..coverage
        };
        let changed = db::reconcile(app_for_scan.store(), now, &outcomes, &coverage)?;
        let findings: usize = outcomes.iter().map(|outcome| outcome.found.len()).sum();
        let checks_run = outcomes.len();
        Ok::<_, AppError>((changed, coverage, findings, checks_run, device_ids))
    });
    let (changed, coverage, findings, checks_run, device_ids) = worker
        .await
        .map_err(|err| AppError::internal(format!("the scan did not finish: {err}")))??;

    // Already stored, with the findings, in one transaction. This only puts
    // it where `coverage()` looks first.
    let coverage = Arc::new(coverage);
    *state.last_coverage.lock() = Some(coverage.clone());

    let summary = Summary {
        devices: coverage.devices,
        opened: changed.opened.len(),
        resolved: changed.resolved.len(),
        open: changed.open.len(),
        findings,
        checks_run,
        took_ms: coverage.took_ms,
        settings_problems: coverage.settings_problems.clone(),
    };
    app.publish(
        "insights.changed",
        &json!({
            "device_id": (device_ids.len() == 1).then(|| device_ids[0].clone()),
            "opened": changed.opened.len(),
            "resolved": changed.resolved.len(),
        }),
    );
    raise_alerts(app, &settings, &changed.opened).await;
    Ok(summary)
}

/// Hand the serious new findings to the monitor, if there is one. The insight
/// id is the dedup key, so one problem is one alert however often it is seen.
async fn raise_alerts(app: &App, settings: &Settings, opened: &[Insight]) {
    if !app.methods().contains(&"alerts.raise") {
        return;
    }
    for insight in opened {
        if insight.severity < settings.raise_alerts_from || insight.state != State::Open {
            continue;
        }
        let raised = app
            .rpc(
                "alerts.raise",
                json!({
                    "rule": format!("insight: {}", insight.check),
                    "device_id": insight.device_id,
                    "instance": insight.subject,
                    "severity": insight.severity,
                    "title": insight.title,
                    "detail": insight.detail,
                    "dedup_key": insight.id,
                }),
            )
            .await;
        if let Err(err) = raised {
            tracing::warn!(insight = %insight.id, error = %err.message, "the alert could not be raised");
        }
    }
}

/// `insights.scan` runs as a job: a full scan reads every device's collected
/// data and the UI should be able to watch it.
pub fn start_job(app: &App, device_ids: Vec<String>, checks: Vec<String>) -> AppResult<Value> {
    let title = match device_ids.len() {
        0 => "Scan every device for issues".to_string(),
        1 => format!(
            "Scan {} for issues",
            inventory::get(app, &device_ids[0])
                .map(|device| device.name)
                .unwrap_or_else(|_| device_ids[0].clone())
        ),
        count => format!("Scan {count} devices for issues"),
    };
    let spec = jobs::JobSpec::new("insights.scan", title)
        .devices(&device_ids)
        .origin("user");
    let job = jobs::start(app, spec, move |ctx| async move {
        let summary = run(ctx.app(), device_ids, checks).await?;
        for problem in &summary.settings_problems {
            ctx.warn(format!(
                "a setting could not be used, so the checks that read it did not run: {}",
                problem.message
            ));
        }
        ctx.info(format!(
            "{} finding(s) over {} device(s): {} new, {} resolved",
            summary.findings, summary.devices, summary.opened, summary.resolved
        ));
        Ok(serde_json::to_value(summary)?)
    });
    Ok(jobs::started(&job))
}

/// Scan automatically after collections and configuration changes, debounced.
pub fn spawn_watcher(app: &App) {
    app.spawn("insights.watcher", watcher(app.clone()));
}

async fn watcher(app: App) {
    let mut events = app.subscribe();
    let shutdown = app.shutdown_token();
    let mut pending: BTreeSet<String> = BTreeSet::new();
    // Set when something happened that cannot be attributed to one device:
    // the next automatic scan then covers everything.
    let mut scan_everything = false;
    let mut deadline: Option<Instant> = None;

    loop {
        let delay = settings::load(&app)
            .map(|settings| settings.auto_scan_delay_ms)
            .unwrap_or(5_000);
        let wait = async {
            match deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = wait => {
                deadline = None;
                let devices: Vec<String> = if scan_everything {
                    Vec::new()
                } else {
                    std::mem::take(&mut pending).into_iter().collect()
                };
                scan_everything = false;
                pending.clear();
                if let Err(err) = run(&app, devices, Vec::new()).await {
                    tracing::warn!(error = %err.message, "the automatic scan failed");
                }
            }
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    // Lagged: events were missed, so the next scan should be
                    // over everything rather than over nothing.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        scan_everything = true;
                        deadline = Some(Instant::now() + Duration::from_millis(delay));
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                match event.topic.as_str() {
                    "collect.updated" | "config.changed" | "logs.signal" => {
                        if !settings::load(&app).map(|settings| settings.auto_scan).unwrap_or(false) {
                            continue;
                        }
                        match event.payload["device_id"].as_str() {
                            Some(device_id) => {
                                pending.insert(device_id.to_string());
                            }
                            // Something happened that names no device: scan all.
                            None => scan_everything = true,
                        }
                        deadline = Some(Instant::now() + Duration::from_millis(delay));
                    }
                    "devices.changed" if event.payload["change"] == "deleted" => {
                        for id in event.payload["ids"].as_array().map(Vec::as_slice).unwrap_or_default() {
                            if let Some(id) = id.as_str() {
                                let _ = db::forget_device(app.store(), id);
                                pending.remove(id);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The coverage of the last scan, from memory or from the store.
pub fn coverage(app: &App) -> AppResult<Option<Coverage>> {
    if let Some(coverage) = app.state::<ScanState>().last_coverage.lock().clone() {
        return Ok(Some((*coverage).clone()));
    }
    app.store().kv_get("insights.coverage")
}
