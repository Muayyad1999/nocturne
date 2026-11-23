//! A lab for the integration tests: the core, this feature, and stubs for
//! the features it gathers from.
//!
//! The stubs answer in the shape the other features' specifications fix, and
//! record what they were called with, so these tests prove the wire contract
//! without depending on another feature crate. Which stubs exist is chosen
//! per test by [`With`], because *the feature is not installed* is a case
//! this crate has to get right — see `tests/absent.rs`.

#![allow(dead_code)] // each test binary uses a different part of this.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use nocturne_app::testing::{self, Lab};
use nocturne_app::util::{new_id, now_ms};
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
    /// `terminal.list`, `terminal.snapshot`, `terminal.logs.list` and
    /// `terminal.logs.read`, over real sessions on the fake devices.
    pub terminal: bool,
    /// `topology.export`.
    pub topology: bool,
    /// `metrics.query_many`, over the core's sample store.
    pub metrics: bool,
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
            terminal: true,
            topology: true,
            metrics: true,
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
    /// Whole changes. `changes.get` serves them as they are; `changes.list`
    /// projects each one into the shape a real `ChangeSummary` has, which is
    /// *not* a subset of this — it carries `devices: [{device_id, …}]` where
    /// a whole change has `targets`. Serving the whole object here was how
    /// `ChangeSummary::devices()` came to be `None` in every real build
    /// while these tests passed.
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
    /// Methods that do not answer until the test lets them.
    pub held: Mutex<HashMap<String, Arc<tokio::sync::Notify>>>,
    /// The drawing `topology.export` returns.
    pub topology_svg: Mutex<String>,
    /// What `metrics.query_many` answers instead of reading the store.
    pub metrics_answer: Mutex<Option<Value>>,
    /// The session logs `terminal.logs.*` serve: the index row and the text.
    pub session_logs: Mutex<Vec<(Value, String)>>,
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

    /// Make `method` wait, once it has been called, until [`Fixture::release`].
    pub fn hold(&self, method: &str) {
        self.held
            .lock()
            .insert(method.to_string(), Arc::new(tokio::sync::Notify::new()));
    }

    pub fn release(&self, method: &str) {
        if let Some(held) = self.held.lock().remove(method) {
            // A permit, so that a call which has not started waiting yet
            // goes straight through when it does.
            held.notify_one();
        }
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
    let held = fixture.held.lock().get(method).cloned();
    if let Some(held) = held {
        held.notified().await;
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
                    .map(summarise)
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
    builder.state(Terminal::default());
    if with.terminal {
        stub!(builder, "terminal.list", |app, _params| {
            Ok(Value::Array(
                app.state::<Terminal>()
                    .sessions
                    .lock()
                    .iter()
                    .map(|session| session.info())
                    .collect(),
            ))
        });
        stub!(builder, "terminal.snapshot", |app, params| {
            let id = params["session_id"].as_str().unwrap_or_default();
            let session = app
                .state::<Terminal>()
                .get(id)
                .ok_or_else(|| AppError::not_found(format!("terminal session `{id}`")))?;
            let lines = session.lines();
            let wanted = params["lines"].as_u64().unwrap_or(200) as usize;
            let text = lines[lines.len().saturating_sub(wanted)..].join("\n");
            // The real feature hides the device's secrets itself before it
            // answers. This one never does, which is the case ops has to be
            // able to stand: whatever is hidden in what it stores, it hid.
            Ok(json!({
                "lines": if text.is_empty() { 0 } else { text.split('\n').count() },
                "text": text,
                "redacted": false,
            }))
        });
        stub!(builder, "terminal.logs.list", |app, params| {
            let words: Vec<String> = params["query"]
                .as_str()
                .unwrap_or_default()
                .split_whitespace()
                .map(str::to_lowercase)
                .collect();
            Ok(Value::Array(
                app.state::<Fixture>()
                    .session_logs
                    .lock()
                    .iter()
                    .map(|(row, _)| row.clone())
                    .filter(|row| {
                        let haystack = format!("{} {}", row["title"], row["name"]).to_lowercase();
                        words.iter().all(|word| haystack.contains(word))
                    })
                    .collect(),
            ))
        });
        stub!(builder, "terminal.logs.read", |app, params| {
            let name = params["name"].as_str().unwrap_or_default();
            let logs = app.state::<Fixture>().session_logs.lock().clone();
            let (_, text) = logs
                .into_iter()
                .find(|(row, _)| row["name"] == json!(name))
                .ok_or_else(|| AppError::not_found(format!("session log `{name}`")))?;
            let size = text.len();
            let asked = (params["offset"].as_u64().unwrap_or(0) as usize).min(size);
            // Like the real read: the page starts at the beginning of the line
            // the offset falls in, which can be earlier than asked, and says so.
            let offset = text[..asked].rfind('\n').map_or(0, |newline| newline + 1);
            let limit = params["limit"].as_u64().unwrap_or(256 * 1024) as usize;
            let mut end = (offset + limit).min(size);
            // Like the real read: whole lines while more of the file follows.
            if end < size {
                end = text[offset..end]
                    .rfind('\n')
                    .map_or(end, |newline| offset + newline + 1);
            }
            Ok(json!({
                "text": text[offset..end],
                "offset": offset,
                "size": size,
                "eof": end >= size,
                "next_offset": end,
                "redacted": false,
            }))
        });
    }
    if with.topology {
        stub!(builder, "topology.export", |app, params| {
            let format = params["format"].as_str().unwrap_or_default().to_string();
            Ok(json!({
                "format": format,
                "filename": format!("topology-physical.{format}"),
                "text": app.state::<Fixture>().topology_svg.lock().clone(),
            }))
        });
    }
    if with.metrics {
        stub!(builder, "metrics.query_many", |app, params| {
            if let Some(canned) = app.state::<Fixture>().metrics_answer.lock().clone() {
                return Ok(canned);
            }
            // As `nocturne-monitor` answers: the store's own buckets, and
            // only the ones that have something in them.
            let since = params["since"].as_i64().unwrap_or(0);
            let until = params["until"].as_i64().unwrap_or_else(now_ms);
            let step = params["step_s"].as_i64().unwrap_or(60).max(1);
            let max_points = ((until - since) / (step * 1000)).clamp(2, 5_000) as usize;
            let mut answers = Vec::new();
            for wanted in params["series"].as_array().cloned().unwrap_or_default() {
                let device_id = wanted["device_id"].as_str().unwrap_or_default();
                let metric = wanted["metric"].as_str().unwrap_or_default();
                let instance = wanted["instance"].as_str().unwrap_or_default();
                let series = app.store().series_id(device_id, metric, instance)?;
                let points: Vec<Value> = app
                    .store()
                    .samples_query(series, since, until, max_points)?
                    .into_iter()
                    .map(|point| json!([point.ts, point.avg]))
                    .collect();
                answers.push(json!({
                    "device_id": device_id,
                    "metric": metric,
                    "instance": instance,
                    "points": points,
                    "step_s": step,
                    "rollup": "raw",
                }));
            }
            Ok(Value::Array(answers))
        });
    }
    testing::lab(builder, names).await
}

// ------------------------------------------------- a terminal on the fake device

/// The sessions the stand-in for the terminal feature holds. Each is a real
/// connection to one of the lab's fake devices, opened through the core as
/// the terminal feature opens its own: what a snapshot returns is what that
/// device printed.
#[derive(Default)]
pub struct Terminal {
    sessions: Mutex<Vec<Arc<Session>>>,
}

impl Terminal {
    fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .iter()
            .find(|session| session.id == id)
            .cloned()
    }
}

pub struct Session {
    pub id: String,
    pub title: String,
    pub device_id: String,
    pub peer: String,
    opened_at: i64,
    writer: nocturne_transport::ConnectionWriter,
    received: Arc<Mutex<Vec<u8>>>,
}

impl Session {
    /// What `terminal.list` says about it, field for field what
    /// `nocturne-terminal`'s `SessionInfo` serialises to.
    fn info(&self) -> Value {
        json!({
            "id": self.id,
            "title": self.title,
            "kind": "sim",
            "device_id": self.device_id,
            "lab": true,
            "opened_at": self.opened_at,
            "state": "open",
            "close_reason": null,
            "closed_at": null,
            "cols": 200,
            "rows": 50,
            "info": {"kind": "sim", "peer": self.peer, "server_banner": null, "kex": null,
                     "cipher": null, "mac": null, "host_key_algorithm": null,
                     "host_key_fingerprint": null, "legacy_crypto": false, "via": []},
            "bytes_in": self.received.lock().len(),
            "bytes_out": 0,
            "logging": null,
            "log_error": null,
        })
    }

    /// Everything the device has printed, as the lines a person read.
    pub fn lines(&self) -> Vec<String> {
        let mut screen = nocturne_drivers::term::Screen::new();
        screen.feed(&self.received.lock());
        screen.finish()
    }

    fn at_a_prompt(&self) -> bool {
        self.lines().last().is_some_and(|line| {
            let line = line.trim_end();
            line.ends_with('#') || line.ends_with('>') || line.ends_with(':')
        })
    }

    /// Wait until the device has printed something more than the `had`
    /// bytes it had, has stopped printing, and is waiting for input. Without
    /// "something more" a busy machine could find the old prompt unchanged
    /// three times before the device had even read the line.
    async fn settle(&self, had: usize) {
        let mut quiet = 0;
        let mut seen = usize::MAX;
        for _ in 0..5_000 {
            tokio::time::sleep(Duration::from_millis(2)).await;
            let now = self.received.lock().len();
            quiet = if now > had && now == seen && self.at_a_prompt() {
                quiet + 1
            } else {
                0
            };
            seen = now;
            if quiet >= 3 {
                return;
            }
        }
        panic!(
            "the fake device never came back to a prompt; it printed:\n{}",
            self.lines().join("\n")
        );
    }

    /// Type a line and wait for the device to answer it.
    pub async fn enter(&self, line: &str) {
        let had = self.received.lock().len();
        self.writer
            .send(format!("{line}\n").into_bytes())
            .await
            .expect("the session is open");
        self.settle(had).await;
    }
}

/// Open a terminal session to the lab device called `name`, in enable mode.
pub async fn open_session(lab: &Lab, name: &str) -> Arc<Session> {
    open_session_as(lab, name, name).await
}

/// [`open_session`] under a title of the caller's choosing, as a session
/// opened by hand has: the terminal calls it what the person typed.
pub async fn open_session_as(lab: &Lab, name: &str, title: &str) -> Arc<Session> {
    let app = &lab.app;
    let device = nocturne_app::inventory::get(app, &lab.id(name)).expect("the device");
    let connection =
        nocturne_app::connect::open(app, &device, &nocturne_app::connect::OpenOptions::default())
            .await
            .expect("a connection to the fake device");
    let peer = connection.info().peer.clone();
    let (writer, mut reader) = connection.split();
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = received.clone();
    tokio::spawn(async move {
        while let Some(output) = reader.recv().await {
            match output {
                nocturne_transport::Output::Data(bytes) => sink.lock().extend_from_slice(&bytes),
                nocturne_transport::Output::Closed { .. } => break,
            }
        }
    });
    let session = Arc::new(Session {
        id: new_id(),
        title: title.to_string(),
        device_id: device.id.clone(),
        peer,
        opened_at: now_ms(),
        writer,
        received,
    });
    app.state::<Terminal>()
        .sessions
        .lock()
        .push(session.clone());
    session.settle(0).await;
    session.enter("enable").await;
    session.enter("ena").await;
    session
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

/// A whole change as `changes.list` really returns it.
///
/// `nocturne-changes` builds a `ChangeSummary` with a `devices` array of
/// `DeviceSummary` — `{device_id, name, risk, protected_hits, state}` — and
/// no `targets` and no `device_ids`. Serving the whole change object instead
/// is what let `ChangeSummary::devices()` return `None` in every real build
/// without a single test noticing.
pub fn summarise(change: Value) -> Value {
    let devices: Vec<Value> = change["targets"]
        .as_array()
        .map(|targets| {
            targets
                .iter()
                .map(|target| {
                    json!({
                        "device_id": target["device_id"].clone(),
                        "name": target["device_name"].clone(),
                        "risk": target["plan"]["risk"]["level"].clone(),
                        "protected_hits": target["plan"]["protected_hits"]
                            .as_array()
                            .map_or(0, Vec::len),
                        "state": target["result"]["state"].clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut summary = json!({
        "id": change["id"].clone(),
        "title": change["title"].clone(),
        "description": change["description"].clone(),
        "state": change["state"].clone(),
        "origin": change["origin"].clone(),
        "ticket": change["ticket"].clone(),
        "created_at": change["created_at"].clone(),
        "updated_at": change["updated_at"].clone(),
        "created_by": change["created_by"].clone(),
        "job_id": change["job_id"].clone(),
        "approved_by": change["approval"]["by"].clone(),
        "devices": devices,
    });
    if let Some(object) = summary.as_object_mut() {
        // A null field and an absent one are different things on the wire,
        // and the real summary leaves these out rather than sending null.
        object.retain(|_, value| !value.is_null());
    }
    summary
}
