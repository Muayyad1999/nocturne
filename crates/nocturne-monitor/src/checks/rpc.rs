//! `checks.*`: the RPC surface of per-host service checks.
//!
//! The shape follows the rest of the feature — `list` / `get` / `save` /
//! `delete` with the same offset cursor, `run_now` beside `monitor.poll_now`,
//! and `history` for the question a service check exists to answer: when did
//! this last go down, and for how long.

use nocturne_app::{App, AppError, AppResult, NoParams, Router, inventory, util};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::JoinSet;

use super::model::{CheckKind, CheckSpec, ServiceCheck, ServiceCheckView};
use super::store::{self, CheckQuery};
use crate::settings::MonitorSettings;
use crate::state::MonitorState;

/// How far back `checks.history` looks when it is not told.
const DEFAULT_WINDOW_MS: i64 = 7 * 86_400_000;
/// How many checks one `checks.run_now` will run.
const RUN_NOW_LIMIT: usize = 50;

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SaveParams {
    /// Left out, a new check is created.
    pub id: Option<String>,
    /// Required for a new check.
    pub device_id: Option<String>,
    /// Required for a new check; unique per device, case-insensitively.
    pub name: Option<String>,
    /// What to check and how. Left out on an update, the stored one is kept.
    pub spec: Option<CheckSpec>,
    /// The host to check; empty means the device's own address.
    pub host: Option<String>,
    pub enabled: Option<bool>,
    pub interval_s: Option<u32>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct RunNowParams {
    pub id: Option<String>,
    pub ids: Vec<String>,
    /// Every enabled check of this device.
    pub device_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct HistoryParams {
    pub id: String,
    /// Default: seven days ago.
    pub since: Option<i64>,
    /// Default: now.
    pub until: Option<i64>,
    pub limit: Option<u32>,
}

// ----------------------------------------------------------------- saving

/// A host a probe can be aimed at. Not a full name check — the engines do
/// that properly — but enough that a pasted URL or a stray space is refused
/// now rather than becoming a check that quietly cannot run.
fn host(text: &str) -> AppResult<String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(String::new());
    }
    if text.contains("://") || text.contains('/') {
        return Err(AppError::invalid(
            "the host is a name or an address, not a URL: put the path in the check's `path` instead",
        ));
    }
    if text.len() > 253
        || text
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(AppError::invalid(format!(
            "`{text}` is not a host name or an address"
        )));
    }
    Ok(text.to_string())
}

fn validate(spec: &CheckSpec) -> AppResult<()> {
    match spec {
        CheckSpec::Icmp { count, .. } => {
            if count.is_some_and(|count| count == 0 || count > 16) {
                return Err(AppError::invalid(
                    "an ICMP check sends between 1 and 16 echo requests",
                ));
            }
        }
        CheckSpec::Tcp { port } => {
            if *port == 0 {
                return Err(AppError::invalid(
                    "0 is not a TCP port; ports are numbers from 1 to 65535",
                ));
            }
        }
        CheckSpec::Dns { name, record, .. } => {
            if name.trim().is_empty() {
                return Err(AppError::invalid("a DNS check needs a name to ask about"));
            }
            host(name)?;
            const KINDS: [&str; 11] = [
                "A", "AAAA", "CNAME", "MX", "NS", "TXT", "SOA", "SRV", "PTR", "CAA", "ANY",
            ];
            let wanted = record.trim().to_uppercase();
            if !wanted.is_empty() && !KINDS.contains(&wanted.as_str()) {
                return Err(AppError::invalid(format!(
                    "`{record}` is not a record type a check can ask for ({})",
                    KINDS.join(", ")
                )));
            }
        }
        CheckSpec::Http(http) | CheckSpec::Https(http) => {
            if http.path.contains("://") {
                return Err(AppError::invalid(
                    "the path is the part after the host (`/health`), not a whole URL",
                ));
            }
            if let Some(bad) = http
                .expect_status
                .iter()
                .copied()
                .find(|status| !(100..=599).contains(status))
            {
                return Err(AppError::invalid(format!(
                    "{bad} is not an HTTP status code"
                )));
            }
        }
        CheckSpec::Tls { warn_days, .. } => {
            if warn_days.is_some_and(|days| days < 0) {
                return Err(AppError::invalid(
                    "a certificate warning is a number of days before expiry, so it cannot be negative",
                ));
            }
        }
        CheckSpec::Ntp { max_offset_ms, .. } => {
            if max_offset_ms.is_some_and(|limit| !limit.is_finite() || limit <= 0.0) {
                return Err(AppError::invalid(
                    "the largest acceptable clock offset has to be a positive number of milliseconds",
                ));
            }
        }
        CheckSpec::Snmp { .. } => {}
    }
    Ok(())
}

pub fn save(app: &App, params: SaveParams) -> AppResult<ServiceCheckView> {
    let now = app.state::<MonitorState>().now();
    let existing = match &params.id {
        Some(id) if !id.is_empty() => Some(store::require(app, id)?),
        _ => None,
    };
    let device_id = params
        .device_id
        .clone()
        .filter(|id| !id.is_empty())
        .or_else(|| existing.as_ref().map(|(check, _)| check.device_id.clone()))
        .ok_or_else(|| AppError::invalid("say which device the check belongs to"))?;
    let device = inventory::get(app, &device_id)?;

    let name = params
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| existing.as_ref().map(|(check, _)| check.name.clone()))
        .ok_or_else(|| AppError::invalid("give the check a name, such as `web` or `resolver`"))?;
    if name.len() > 64 {
        return Err(AppError::invalid("a check's name is at most 64 characters"));
    }
    let spec = params
        .spec
        .clone()
        .or_else(|| existing.as_ref().map(|(check, _)| check.spec.clone()))
        .ok_or_else(|| {
            AppError::invalid(
                "say what to check: a `spec` with a `kind` of icmp, tcp, dns, http, https, tls, ntp or snmp",
            )
        })?;
    validate(&spec)?;

    let id = existing
        .as_ref()
        .map(|(check, _)| check.id.clone())
        .unwrap_or_else(util::new_id);
    // Two checks called `web` on one device would be two lines a person
    // cannot tell apart, and one alert key for both.
    if let Some(clash) = store::by_name(app, &device_id, &name)?
        && clash.id != id
    {
        return Err(AppError::conflict(format!(
            "`{}` already has a check called `{name}`",
            device.name
        )));
    }

    let interval_s = params
        .interval_s
        .or_else(|| existing.as_ref().map(|(check, _)| check.interval_s))
        .unwrap_or(60)
        .clamp(5, 86_400);
    let timeout_ms = params
        .timeout_ms
        .or_else(|| existing.as_ref().map(|(check, _)| check.timeout_ms))
        .unwrap_or_else(|| spec.kind().default_timeout_ms())
        .clamp(100, 120_000);
    if timeout_ms > u64::from(interval_s) * 1_000 {
        return Err(AppError::invalid(format!(
            "a check cannot be given {timeout_ms} ms to answer when it runs every {interval_s} s: raise the interval or lower the timeout"
        )));
    }

    let check = ServiceCheck {
        id,
        device_id,
        name,
        spec,
        host: match params.host.as_deref() {
            Some(text) => host(text)?,
            None => existing
                .as_ref()
                .map(|(check, _)| check.host.clone())
                .unwrap_or_default(),
        },
        enabled: params
            .enabled
            .or_else(|| existing.as_ref().map(|(check, _)| check.enabled))
            .unwrap_or(true),
        interval_s,
        timeout_ms,
        created_at: existing
            .as_ref()
            .map(|(check, _)| check.created_at)
            .unwrap_or(now),
        updated_at: now,
    };
    let repointed = store::save(app, &check)?;
    // The scheduler hands out a slot inside the interval to a check it has
    // not seen; a changed interval has to get a fresh one, or the check keeps
    // the old cadence until the process restarts.
    app.state::<MonitorState>().forget_check(&check.id);
    app.audit(
        "monitor.check.save",
        Some(&check.device_id),
        format!("Saved the `{}` check on `{}`", check.name, device.name),
        json!({
            "check_id": check.id,
            "kind": check.kind().as_str(),
            "enabled": check.enabled,
            "interval_s": check.interval_s,
            "spec": check.spec,
            "state_reset": repointed,
        }),
    );
    app.publish("monitor.status_changed", &json!({"what": "checks"}));
    let (stored, state) = store::require(app, &check.id)?;
    Ok(store::view(app, stored, state))
}

// ---------------------------------------------------------------- running

async fn run_now(app: &App, params: RunNowParams) -> AppResult<Value> {
    let mut checks: Vec<ServiceCheck> = Vec::new();
    if let Some(device_id) = params.device_id.as_deref().filter(|id| !id.is_empty()) {
        inventory::get(app, device_id)?;
        checks.extend(
            store::of_device(app, device_id)?
                .into_iter()
                .filter(|(check, _)| check.enabled)
                .map(|(check, _)| check),
        );
    }
    for id in params.ids.iter().chain(params.id.as_ref()) {
        let (check, _) = store::require(app, id)?;
        if !checks.iter().any(|already| already.id == check.id) {
            checks.push(check);
        }
    }
    if checks.is_empty() {
        return Err(AppError::invalid(
            "name a check to run, or a device whose checks should all run",
        ));
    }
    if checks.len() > RUN_NOW_LIMIT {
        return Err(AppError::invalid(format!(
            "that is {} checks; run at most {RUN_NOW_LIMIT} at a time",
            checks.len()
        )));
    }

    // Bounded the same way the poller is, so "check everything now" on a
    // busy device cannot open more sockets than a polling cycle would.
    let concurrency = MonitorSettings::load(app).concurrency.clamp(1, 64) as usize;
    let mut running: JoinSet<(String, AppResult<super::store::Recorded>)> = JoinSet::new();
    let mut queue = checks.into_iter();
    let mut results: Vec<(String, AppResult<super::store::Recorded>)> = Vec::new();
    for check in queue.by_ref().take(concurrency) {
        let app = app.clone();
        running.spawn(async move {
            let id = check.id.clone();
            (id, super::run_and_record(&app, &check).await)
        });
    }
    while let Some(finished) = running.join_next().await {
        match finished {
            Ok(result) => results.push(result),
            Err(error) => return Err(AppError::internal(format!("a check panicked: {error}"))),
        }
        if let Some(check) = queue.next() {
            let app = app.clone();
            running.spawn(async move {
                let id = check.id.clone();
                (id, super::run_and_record(&app, &check).await)
            });
        }
    }

    let mut answered = Vec::new();
    for (id, result) in results {
        match result {
            Ok(recorded) => {
                let (check, state) = store::require(app, &id)?;
                answered.push(json!({
                    "check_id": id,
                    "name": check.name,
                    "kind": check.kind().as_str(),
                    "status": state.status.as_str(),
                    "previous": recorded.previous.as_str(),
                    "changed": recorded.changed,
                    "code": state.code,
                    "message": state.message,
                    "rtt_ms": state.rtt_ms,
                    "facts": state.facts,
                }));
            }
            // A check that could not even be recorded is reported as itself,
            // not as a result.
            Err(error) => answered.push(json!({
                "check_id": id,
                "error": error.message,
            })),
        }
    }
    answered.sort_by(|left, right| {
        left["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["name"].as_str().unwrap_or_default())
    });
    Ok(json!({"ran": answered.len(), "checks": answered}))
}

// ---------------------------------------------------------------- history

fn history(app: &App, params: HistoryParams) -> AppResult<Value> {
    let (check, state) = store::require(app, &params.id)?;
    let now = app.state::<MonitorState>().now();
    let until = params.until.unwrap_or(now);
    let since = params.since.unwrap_or(until - DEFAULT_WINDOW_MS);
    if until <= since {
        return Err(AppError::invalid("the window has to end after it starts"));
    }
    let events = store::events(app, &check.id, since, until, params.limit.unwrap_or(500))?;
    let summary = store::summarise(app, &check.id, since, until, &events)?;
    Ok(json!({
        "check_id": check.id,
        "device_id": check.device_id,
        "name": check.name,
        "kind": check.kind().as_str(),
        "since": since,
        "until": until,
        "current": state,
        "events": events,
        "summary": summary,
        // Where the response times are: the same series API as every other
        // metric, so a check charts like anything else.
        "series": {
            "device_id": check.device_id,
            "instance": check.id,
            "metrics": [
                super::run::METRIC_UP,
                super::run::METRIC_RTT,
                super::run::METRIC_CERT_DAYS,
            ],
        },
    }))
}

// --------------------------------------------------------------- the router

pub fn register(router: &mut Router) {
    router.add("checks.list", |app: App, params: CheckQuery| async move {
        if let Some(device_id) = &params.device_id {
            inventory::get(&app, device_id)?;
        }
        let rows = store::list(&app, &params)?;
        Ok::<Vec<ServiceCheckView>, AppError>(store::views(&app, rows))
    });
    router.add("checks.get", |app: App, params: IdParams| async move {
        let (check, state) = store::require(&app, &params.id)?;
        Ok::<ServiceCheckView, AppError>(store::view(&app, check, state))
    });
    router.add("checks.save", |app: App, params: SaveParams| async move {
        save(&app, params)
    });
    router.add("checks.delete", |app: App, params: IdParams| async move {
        let (check, _) = store::require(&app, &params.id)?;
        let device = inventory::get(&app, &check.device_id)
            .map(|device| device.name)
            .unwrap_or_default();
        store::delete(&app, &params.id)?;
        app.state::<MonitorState>().forget_check(&check.id);
        // Any alert it raised is now about something that does not exist.
        // The rules engine resolves those itself, once it can see that the
        // check is gone — the same sweep that closes an alert about an
        // interface that has been removed.
        crate::rules::evaluate_checks(&app, &check.device_id).await;
        app.audit(
            "monitor.check.delete",
            Some(&check.device_id),
            format!("Deleted the `{}` check on `{device}`", check.name),
            json!({"check_id": check.id, "kind": check.kind().as_str()}),
        );
        app.publish("monitor.status_changed", &json!({"what": "checks"}));
        Ok(json!({"deleted": true}))
    });
    router.add(
        "checks.run_now",
        |app: App, params: RunNowParams| async move { run_now(&app, params).await },
    );
    router.add(
        "checks.history",
        |app: App, params: HistoryParams| async move { history(&app, params) },
    );
    router.add("checks.kinds", |_: App, _: NoParams| async move {
        // What a form can offer, with the defaults it should prefill.
        Ok::<Value, AppError>(Value::Array(
            [
                (CheckKind::Icmp, "Echo requests to the host."),
                (CheckKind::Tcp, "A connection to one port."),
                (CheckKind::Dns, "One question, asked of this server."),
                (
                    CheckKind::Http,
                    "A request, with the status and body checked.",
                ),
                (
                    CheckKind::Https,
                    "The same over TLS, with the certificate inspected.",
                ),
                (CheckKind::Tls, "A handshake, for the certificate alone."),
                (CheckKind::Ntp, "The time, and how far off ours is."),
                (
                    CheckKind::Snmp,
                    "One object, over the device's own SNMP credential.",
                ),
            ]
            .into_iter()
            .map(|(kind, what)| {
                json!({
                    "kind": kind.as_str(),
                    "what": what,
                    "default_timeout_ms": kind.default_timeout_ms(),
                    "sees_certificates": kind.sees_certificates(),
                })
            })
            .collect(),
        ))
    });
}
