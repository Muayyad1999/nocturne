//! A lab for the integration tests: the core, this feature, and stubs for
//! the features it gathers from.
//!
//! The stubs answer in the shape the other features' specifications fix, and
//! record what they were called with, so these tests prove the wire contract
//! without depending on another feature crate. Which stubs exist is chosen
//! per test by [`With`], because *the feature is not installed* is a case
//! this crate has to get right — see `tests/absent.rs`.

#![allow(dead_code)] // each test binary uses a different part of this.

use std::collections::HashSet;
use std::time::Duration;

use nocturne_app::testing::{self, Lab};
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use parking_lot::Mutex;
use serde_json::{Value, json};

/// Which other features this build has.
#[derive(Clone, Copy, Debug, Default)]
pub struct With {
    pub alerts: bool,
    /// Whether the alert feature also has `alerts.summary`.
    pub alerts_summary: bool,
    pub changes: bool,
    pub compliance: bool,
    pub insights: bool,
    pub config_devices: bool,
    pub config_search: bool,
    pub ipam: bool,
    pub logs: bool,
    pub locate: bool,
}

impl With {
    /// Everything ops can gather from.
    pub fn all() -> With {
        With {
            alerts: true,
            alerts_summary: true,
            changes: true,
            compliance: true,
            insights: true,
            config_devices: true,
            config_search: true,
            ipam: true,
            logs: true,
            locate: true,
        }
    }

    /// A fresh installation: ops and the core, nothing else.
    pub fn nothing() -> With {
        With::default()
    }
}

/// What the stubs serve and what they were asked.
#[derive(Default)]
pub struct Fixture {
    pub alerts: Mutex<Vec<Value>>,
    /// Whole changes; `changes.list` serves the same objects, which is what
    /// a real `ChangeSummary` is a subset of.
    pub changes: Mutex<Vec<Value>>,
    pub compliance: Mutex<Value>,
    pub compliance_policies: Mutex<Vec<Value>>,
    pub insights: Mutex<Vec<Value>>,
    pub config_devices: Mutex<Vec<Value>>,
    pub config_matches: Mutex<Value>,
    pub ipam: Mutex<Value>,
    pub logs: Mutex<Value>,
    pub locate: Mutex<Value>,
    /// Every call the stubs received, with its parameters.
    pub calls: Mutex<Vec<(String, Value)>>,
    /// Methods that answer with an error.
    pub failing: Mutex<HashSet<String>>,
    /// Methods that take longer than any caller's budget.
    pub slow: Mutex<HashSet<String>>,
    /// Methods that answer with something ops cannot read.
    pub malformed: Mutex<HashSet<String>>,
}

impl Fixture {
    pub fn fail(&self, method: &str) {
        self.failing.lock().insert(method.to_string());
    }

    pub fn delay(&self, method: &str) {
        self.slow.lock().insert(method.to_string());
    }

    pub fn garble(&self, method: &str) {
        self.malformed.lock().insert(method.to_string());
    }

    pub fn called(&self, method: &str) -> usize {
        self.calls
            .lock()
            .iter()
            .filter(|(name, _)| name == method)
            .count()
    }

    pub fn params(&self, method: &str) -> Option<Value> {
        self.calls
            .lock()
            .iter()
            .find(|(name, _)| name == method)
            .map(|(_, params)| params.clone())
    }
}

/// Record the call and honour whatever failure the test asked for.
async fn gate(app: &App, method: &str, params: &Value) -> AppResult<Option<Value>> {
    let fixture = app.state::<Fixture>();
    fixture
        .calls
        .lock()
        .push((method.to_string(), params.clone()));
    if fixture.failing.lock().contains(method) {
        return Err(AppError::unavailable(format!(
            "the stubbed `{method}` is broken in this test"
        )));
    }
    if fixture.slow.lock().contains(method) {
        // Longer than any budget in this crate.
        tokio::time::sleep(Duration::from_secs(30)).await;
    }
    if fixture.malformed.lock().contains(method) {
        return Ok(Some(json!({"something": "else entirely"})));
    }
    Ok(None)
}

macro_rules! stub {
    ($builder:expr, $method:literal, |$app:ident, $params:ident| $body:block) => {
        $builder
            .router
            .add($method, |$app: App, $params: Value| async move {
                if let Some(canned) = gate(&$app, $method, &$params).await? {
                    return Ok(canned);
                }
                $body
            });
    };
}

pub async fn lab(names: &[&str], with: With) -> Lab {
    let mut builder = testing::builder();
    nocturne_ops::init(&mut builder);
    builder.state(Fixture::default());
    // A compliance run that found nothing wrong, so that a test which does
    // not care about compliance still gets a well-formed answer. The run's
    // time is deliberately absent, which is a case ops has to handle.
    *builder.get::<Fixture>().compliance.lock() = json!({
        "findings": [],
        "summary": {"devices": 0, "rules": 0, "pass": 0, "fail": 0, "score_pct": 100.0},
    });

    if with.alerts {
        stub!(builder, "alerts.list", |app, params| {
            let alerts = app.state::<Fixture>().alerts.lock().clone();
            let since = params.get("since").and_then(Value::as_i64);
            let state = params
                .get("state")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(Value::Array(
                alerts
                    .into_iter()
                    .filter(|alert| {
                        since.is_none_or(|since| {
                            alert["started_at"].as_i64().unwrap_or(0) >= since
                                || alert["resolved_at"].is_null()
                        })
                    })
                    .filter(|alert| match state.as_deref() {
                        Some("open") => alert["resolved_at"].is_null(),
                        Some(wanted) => alert["state"] == json!(wanted),
                        None => true,
                    })
                    .collect(),
            ))
        });
        stub!(builder, "alerts.get", |app, params| {
            let alerts = app.state::<Fixture>().alerts.lock().clone();
            match alerts.into_iter().find(|alert| alert["id"] == params["id"]) {
                Some(alert) => Ok(alert),
                None => Err(AppError::not_found(format!("alert `{}`", params["id"]))),
            }
        });
    }
    if with.alerts_summary {
        stub!(builder, "alerts.summary", |app, _params| {
            let alerts = app.state::<Fixture>().alerts.lock().clone();
            let open: Vec<Value> = alerts
                .iter()
                .filter(|alert| alert["resolved_at"].is_null())
                .cloned()
                .collect();
            let mut by_severity = serde_json::Map::new();
            for alert in &open {
                let severity = alert["severity"].as_str().unwrap_or("info").to_string();
                let count = by_severity
                    .get(&severity)
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                by_severity.insert(severity, json!(count + 1));
            }
            let mut newest = open.clone();
            newest.sort_by_key(|alert| std::cmp::Reverse(alert["started_at"].as_i64()));
            // One alert or null, which is what `nocturne-monitor` really
            // sends. This stub used to answer with a list — the shape this
            // crate's reader expected — so the test agreed with the
            // assumption instead of with the feature, and the mismatch only
            // showed up when a screen noticed every overview was falling
            // back to a capped list.
            Ok(json!({
                "open": open.len(),
                "by_severity": by_severity,
                "newest": newest.first().cloned().unwrap_or(Value::Null),
            }))
        });
    }
    if with.changes {
        stub!(builder, "changes.list", |app, params| {
            let changes = app.state::<Fixture>().changes.lock().clone();
            let state = params
                .get("state")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(Value::Array(
                changes
                    .into_iter()
                    .filter(|change| {
                        state
                            .as_deref()
                            .is_none_or(|want| change["state"] == json!(want))
                    })
                    .collect(),
            ))
        });
        stub!(builder, "changes.get", |app, params| {
            let changes = app.state::<Fixture>().changes.lock().clone();
            match changes
                .into_iter()
                .find(|change| change["id"] == params["id"])
            {
                Some(change) => Ok(change),
                None => Err(AppError::not_found(format!("change `{}`", params["id"]))),
            }
        });
    }
    if with.compliance {
        stub!(builder, "compliance.results", |app, _params| {
            Ok(app.state::<Fixture>().compliance.lock().clone())
        });
        stub!(builder, "compliance.runs", |_app, _params| {
            Ok(Value::Array(Vec::new()))
        });
        stub!(builder, "compliance.policies.list", |app, _params| {
            Ok(Value::Array(
                app.state::<Fixture>().compliance_policies.lock().clone(),
            ))
        });
    }
    if with.insights {
        stub!(builder, "insights.list", |app, _params| {
            let insights = app.state::<Fixture>().insights.lock().clone();
            let mut by_severity = serde_json::Map::new();
            for insight in &insights {
                let severity = insight["severity"].as_str().unwrap_or("info").to_string();
                let count = by_severity
                    .get(&severity)
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                by_severity.insert(severity, json!(count + 1));
            }
            Ok(json!({
                "insights": insights,
                "counts": {"by_severity": by_severity},
            }))
        });
    }
    if with.config_devices {
        stub!(builder, "config.devices", |app, _params| {
            Ok(Value::Array(
                app.state::<Fixture>().config_devices.lock().clone(),
            ))
        });
    }
    if with.config_search {
        stub!(builder, "config.search", |app, _params| {
            Ok(app.state::<Fixture>().config_matches.lock().clone())
        });
    }
    if with.ipam {
        stub!(builder, "ipam.search", |app, _params| {
            Ok(app.state::<Fixture>().ipam.lock().clone())
        });
    }
    if with.logs {
        stub!(builder, "logs.query", |app, _params| {
            Ok(app.state::<Fixture>().logs.lock().clone())
        });
    }
    if with.locate {
        stub!(builder, "locate.host", |app, _params| {
            let located = app.state::<Fixture>().locate.lock().clone();
            if located.is_null() {
                return Err(AppError::not_found("that host"));
            }
            Ok(located)
        });
    }
    testing::lab(builder, names).await
}

pub fn fixture(app: &App) -> std::sync::Arc<Fixture> {
    app.state::<Fixture>()
}

// ----------------------------------------------------------------- builders

/// An alert in the shape `docs/dev/features/monitor.md` fixes.
pub fn alert(id: &str, device_id: &str, severity: &str, started_at: i64) -> Value {
    json!({
        "id": id,
        "rule_id": "r-down",
        "rule_name": "Device down",
        "device_id": device_id,
        "device": "recorded-name",
        "instance": null,
        "severity": severity,
        "state": "firing",
        "title": format!("{device_id} is not answering"),
        "detail": "no answer to 3 echoes",
        "value": null,
        "started_at": started_at,
        "last_seen_at": started_at,
        "resolved_at": null,
        "acked_at": null,
        "count": 1,
        "muted_by_maintenance": false,
        "lab": false,
    })
}

pub fn resolved(mut alert: Value, acked_at: i64, resolved_at: i64) -> Value {
    alert["acked_at"] = json!(acked_at);
    alert["resolved_at"] = json!(resolved_at);
    alert["state"] = json!("resolved");
    alert
}

/// A change in the shape `docs/dev/features/changes.md` fixes, with per-device
/// results.
pub fn change(
    id: &str,
    title: &str,
    state: &str,
    device_id: &str,
    created_at: i64,
    finished_at: i64,
) -> Value {
    json!({
        "id": id,
        "title": title,
        "description": "",
        "origin": "user",
        "state": state,
        "ticket": "CHG-42",
        "created_by": "mo",
        "created_at": created_at,
        "updated_at": finished_at,
        "approval": {"by": "zahra", "at": created_at + 60_000, "plan_hashes": [], "note": "",
                     "confirmed_devices": []},
        "targets": [{
            "device_id": device_id,
            "intent": {"type": "lines", "lines": ["interface Gi0/3", " shutdown"]},
            "result": {
                "state": if state == "applied" { "applied" } else { "failed" },
                "started_at": finished_at - 30_000,
                "finished_at": finished_at,
                "steps": [],
                "sent": [],
                "error": if state == "applied" { Value::Null } else { json!("the device rejected the line") },
                "before_version_id": 1,
                "after_version_id": 2,
                "verification": [],
                "transcript": "",
            },
        }],
    })
}

/// A change that has not run yet.
pub fn planned_change(id: &str, title: &str, device_id: &str, created_at: i64) -> Value {
    json!({
        "id": id,
        "title": title,
        "origin": "assistant",
        "state": "planned",
        "created_by": "assistant",
        "created_at": created_at,
        "updated_at": created_at,
        "targets": [{"device_id": device_id, "intent": {"type": "lines", "lines": []}}],
    })
}

pub fn insight(id: &str, device_id: &str, severity: &str, title: &str) -> Value {
    json!({
        "id": id,
        "title": title,
        "severity": severity,
        "state": "open",
        "category": "physical",
        "check": "duplex_mismatch",
        "device_id": device_id,
        "device": "recorded-name",
        "subject": "GigabitEthernet0/1",
        "first_seen": 1_000,
        "last_seen": 2_000,
        "lab": true,
    })
}

/// Reachability samples: one per minute from `start`, `1` up and `0` down.
pub fn reachability(app: &App, device_id: &str, start: i64, pattern: &str) {
    let series = app
        .store()
        .series_id(device_id, "up", "")
        .expect("a series");
    let samples: Vec<(i64, i64, f64)> = pattern
        .chars()
        .enumerate()
        .filter(|(_, ch)| *ch != '.')
        .map(|(index, ch)| {
            (
                series,
                start + index as i64 * 60_000,
                if ch == '1' { 1.0 } else { 0.0 },
            )
        })
        .collect();
    app.store()
        .samples_insert(&samples)
        .expect("samples stored");
}

/// Move a device's `created_at` back.
///
/// `reach::of_device` deliberately refuses to measure a device for time
/// before it was added, so a test that asks about a period before this
/// process started has to say the device was there.
pub fn backdate(app: &App, device_id: &str, created_at: i64) {
    use nocturne_app::model::{Device, collections::DEVICES};
    let mut device: Device = app
        .store()
        .require(DEVICES, device_id, "device")
        .expect("the device is in the inventory");
    device.created_at = created_at;
    app.store()
        .put(DEVICES, device_id, &device)
        .expect("the device is stored");
}

/// A stored running-configuration snapshot, as a backup would be.
pub fn backup(app: &App, device_id: &str, text: &str, source: &str) -> i64 {
    app.store()
        .config_insert(device_id, "running", text, source, "")
        .expect("a stored configuration")
        .0
        .id
}

/// The start of a period that is comfortably in the past.
pub fn hours_ago(hours: i64) -> i64 {
    now_ms() - hours * 3_600_000
}

/// Fail the test with the call's error message rather than a bare panic.
pub async fn rpc(app: &App, method: &str, params: Value) -> Value {
    app.rpc(method, params)
        .await
        .unwrap_or_else(|err| panic!("`{method}` failed: {} ({})", err.message, err.code))
}
