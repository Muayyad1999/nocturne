//! What was run, and what came back.
//!
//! A website forgets. During an incident the useful question is "what did that
//! traceroute look like twenty minutes ago, before we changed the route?", so
//! every probe leaves a record: what was asked for, which address it turned
//! out to be, whose address that is, where it ran from, how it was sent, and
//! either the result or the reason there is none.
//!
//! A record is written **before** the probe runs and finished afterwards, so a
//! running ping is in the list too, and a process that dies mid-probe leaves a
//! record that says so rather than one that waits for ever.

use std::sync::Arc;

use nocturne_app::store::Store;
use nocturne_app::{App, AppError, AppResult, Router, util};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::known::{DeviceHint, Target};

pub const COLLECTION: &str = "probe_runs";

/// One probe, from the moment it was asked for.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Run {
    pub id: String,
    /// `ping`, `traceroute`, `ports`, `sweep`, `dns`, `whois`, `tls`, `http`,
    /// `ntp`, `wol`, `device.ping`, `device.traceroute`, `tftp`, `syslog`.
    pub kind: String,
    /// One line for a list: `Ping 10.0.0.2 (SW1)`.
    pub title: String,
    /// The target as the caller gave it.
    pub target: String,
    /// The address it turned out to be, once that is known.
    pub address: Option<String>,
    /// Whose address that is, when Nocturne knows.
    pub device: Option<DeviceHint>,
    /// `this computer` or `the Nocturne server` — which is the question people
    /// are really asking when they ping from a NOC tool.
    pub from: String,
    /// How it was actually sent, when the engine can say: `icmp_datagram`,
    /// `system_command`, `native-udp`, or the device command that was run.
    pub method: Option<String>,
    /// The job it runs as, for the probes that have one.
    pub job_id: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    /// `None` while it is still running.
    pub ok: Option<bool>,
    /// Why there is no result. Set together with `error_code`, which
    /// distinguishes "this machine may not do that" (`forbidden`) from "the
    /// target said nothing" (`timeout`) — a distinction a result would blur.
    pub error: Option<String>,
    pub error_code: Option<String>,
    /// One line a person can read without opening the result.
    pub summary: String,
    /// The engine's own report.
    pub result: Option<Value>,
}

impl Run {
    /// A record for a probe that is about to start.
    pub fn new(kind: &str, title: impl Into<String>, from: &str, target: &Target) -> Run {
        Run {
            id: util::new_id(),
            kind: kind.to_string(),
            title: title.into(),
            target: target.input.clone(),
            address: (target.host != target.input).then(|| target.host.clone()),
            device: target.device.clone(),
            from: from.to_string(),
            started_at: util::now_ms(),
            ..Run::default()
        }
    }

    /// A record for something that is not aimed at a host (a server, a local read).
    pub fn plain(kind: &str, title: impl Into<String>, from: &str, target: &str) -> Run {
        Run {
            id: util::new_id(),
            kind: kind.to_string(),
            title: title.into(),
            target: target.to_string(),
            from: from.to_string(),
            started_at: util::now_ms(),
            ..Run::default()
        }
    }

    /// The record as the event carries it: everything but the result, which
    /// can be a whole port scan and is one `probe.history.get` away.
    fn summary_only(&self) -> Run {
        Run {
            result: None,
            ..self.clone()
        }
    }
}

/// `this computer` in the desktop app, `the Nocturne server` behind a browser.
pub fn where_from(app: &App) -> &'static str {
    if app.local_access() {
        "this computer"
    } else {
        "the Nocturne server"
    }
}

fn save(app: &App, run: &Run) {
    if let Err(err) = app.store().put(COLLECTION, &run.id, run) {
        tracing::error!(%err, run = %run.id, "a probe run could not be recorded");
    }
    app.publish("probe.ran", &run.summary_only());
}

/// Record a probe that is starting. The record is visible at once.
pub fn begin(app: &App, run: &Run) {
    save(app, run);
}

/// Record what is only known once the job is running: its id, and which
/// method the probe settled on.
///
/// `begin` runs before `jobs::start` has returned, so the record it saves has
/// `job_id: None`. Without this second save that stays true for as long as the
/// probe runs and is corrected only when it ends — so nothing could join a
/// *running* probe to its job, which is exactly when somebody wants to follow
/// it or stop it.
pub fn running(app: &App, run: &Run) {
    save(app, run);
}

/// Record how a probe ended, with a line a person can read.
pub fn succeeded<T: Serialize>(app: &App, run: &mut Run, summary: impl Into<String>, result: &T) {
    run.finished_at = Some(util::now_ms());
    run.ok = Some(true);
    run.summary = summary.into();
    run.result = serde_json::to_value(result).ok();
    save(app, run);
    prune(app);
}

/// Record that a probe produced no result, and why.
///
/// This is the honest half of the feature: `forbidden` means this machine may
/// not send that, `timeout` means nothing answered, `not_found` means the name
/// does not resolve. None of them is a result, and none of them is written as
/// one.
pub fn failed(app: &App, run: &mut Run, error: &AppError) {
    run.finished_at = Some(util::now_ms());
    run.ok = Some(false);
    run.error = Some(error.message.clone());
    run.error_code = Some(error.code.to_string());
    run.summary = match error.code {
        "forbidden" | "unsupported" => format!("could not run: {}", error.message),
        _ => error.message.clone(),
    };
    save(app, run);
    prune(app);
}

/// Keep only the newest `keep_runs` finished records.
fn prune(app: &App) {
    let keep = crate::settings::ToolboxSettings::load(app).keep_runs as usize;
    let Ok(mut runs) = app.store().list::<Run>(COLLECTION) else {
        return;
    };
    runs.retain(|run| run.ok.is_some());
    if runs.len() <= keep {
        return;
    }
    runs.sort_by_key(|run| run.finished_at.unwrap_or(run.started_at));
    for run in &runs[..runs.len() - keep] {
        let _ = app.store().delete(COLLECTION, &run.id);
    }
}

/// Runs left unfinished by a process that ended are closed at start-up.
pub fn close_interrupted(store: &Arc<Store>) {
    let Ok(runs) = store.list::<Run>(COLLECTION) else {
        return;
    };
    for mut run in runs.into_iter().filter(|run| run.ok.is_none()) {
        run.ok = Some(false);
        run.error = Some("Nocturne was closed while this was running".into());
        run.error_code = Some("cancelled".into());
        run.summary = "interrupted".into();
        run.finished_at = Some(util::now_ms());
        let _ = store.put(COLLECTION, &run.id, &run);
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ListParams {
    /// One kind, or several separated by commas.
    pub kind: Option<String>,
    /// Only runs aimed at this device (its id).
    pub device_id: Option<String>,
    /// Free text over the title, target and summary.
    pub query: Option<String>,
    pub limit: Option<usize>,
    /// Rows already received, as a decimal string.
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct DeleteParams {
    pub ids: Vec<String>,
    /// Delete every finished run instead of named ones.
    pub all: bool,
}

#[derive(Debug, Serialize)]
pub struct History {
    pub runs: Vec<Run>,
    /// Runs matching the filters, before paging.
    pub total: usize,
    /// The cursor for the next page, or `None` at the end.
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Deleted {
    pub removed: usize,
}

fn list(app: &App, params: &ListParams) -> AppResult<History> {
    let kinds: Vec<String> = params
        .kind
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(|kind| kind.trim().to_ascii_lowercase())
        .filter(|kind| !kind.is_empty())
        .collect();
    let mut runs: Vec<Run> = app
        .store()
        .list::<Run>(COLLECTION)?
        .into_iter()
        .filter(|run| kinds.is_empty() || kinds.contains(&run.kind))
        .filter(|run| match params.device_id.as_deref() {
            None | Some("") => true,
            Some(id) => run.device.as_ref().is_some_and(|device| device.id == id),
        })
        .filter(|run| match params.query.as_deref() {
            None | Some("") => true,
            Some(query) => util::matches_query(
                &format!("{} {} {}", run.title, run.target, run.summary),
                query,
            ),
        })
        .collect();
    // Newest first; the id breaks ties so paging is stable within a millisecond.
    runs.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    let total = runs.len();
    let skip: usize = params
        .cursor
        .as_deref()
        .unwrap_or("0")
        .trim()
        .parse()
        .unwrap_or(0);
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let page: Vec<Run> = runs
        .into_iter()
        .skip(skip)
        .take(limit)
        // A list does not need every echo of every ping.
        .map(|run| Run {
            result: None,
            ..run
        })
        .collect();
    let next = skip + page.len();
    Ok(History {
        runs: page,
        total,
        cursor: (next < total).then(|| next.to_string()),
    })
}

pub fn register(router: &mut Router) {
    router.add("probe.history", |app: App, params: ListParams| async move {
        list(&app, &params)
    });
    router.add(
        "probe.history.get",
        |app: App, params: IdParams| async move {
            app.store()
                .require::<Run>(COLLECTION, &params.id, "probe run")
        },
    );
    router.add(
        "probe.history.delete",
        |app: App, params: DeleteParams| async move {
            let ids = if params.all {
                app.store()
                    .list::<Run>(COLLECTION)?
                    .into_iter()
                    .filter(|run| run.ok.is_some())
                    .map(|run| run.id)
                    .collect()
            } else {
                params.ids
            };
            let mut removed = 0;
            for id in &ids {
                if app.store().delete(COLLECTION, id)? {
                    removed += 1;
                }
            }
            // Deleting records is the one mutation in this module, so it is
            // the one thing here that goes into the trail.
            if removed > 0 {
                app.audit(
                    "probe.history.delete",
                    None,
                    format!(
                        "Deleted {removed} probe run{} from the toolbox history",
                        if removed == 1 { "" } else { "s" }
                    ),
                    serde_json::json!({"all": params.all, "removed": removed}),
                );
            }
            app.publish("probe.ran", &serde_json::json!({"removed": removed}));
            Ok(Deleted { removed })
        },
    );
}
