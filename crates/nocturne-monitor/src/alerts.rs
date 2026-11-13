//! Alerts: raising them, counting repeats, resolving them, and the RPC
//! surface around them.
//!
//! An alert is keyed by its `dedup_key` — (rule, device, instance) for a rule,
//! whatever the caller passes for `alerts.raise`. While an alert with that key
//! is open, another sighting counts a repeat; once it resolves, the next
//! sighting is a new alert with a new id, so the history says "down three
//! times this week" rather than "down, still down".

use nocturne_app::store::LogEntry;
use nocturne_app::{App, AppError, AppResult, inventory};
use rusqlite::{Row, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{Alert, AlertState, Severity};
use crate::state::MonitorState;
use crate::{maintenance, notify};

/// What a caller knows when it wants an alert raised.
#[derive(Clone, Debug, Default)]
pub struct AlertSpec {
    pub dedup_key: String,
    pub rule_id: Option<String>,
    pub rule_name: String,
    pub device_id: Option<String>,
    pub instance: Option<String>,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    pub value: Option<f64>,
    /// Channels to notify; empty means every enabled channel.
    pub notify: Vec<String>,
}

// ------------------------------------------------------------------ store

fn row_to_alert(row: &Row<'_>) -> rusqlite::Result<Alert> {
    let body: String = row.get(0)?;
    serde_json::from_str(&body).map_err(|err| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(err))
    })
}

pub fn save(app: &App, alert: &Alert) -> AppResult<()> {
    let body = serde_json::to_string(alert)?;
    app.store().with(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO monitor_alerts
             (id, dedup_key, state, severity, device_id, started_at, last_seen_at, body)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                alert.id,
                alert.dedup_key,
                alert.state.as_str(),
                alert.severity.rank(),
                alert.device_id,
                alert.started_at,
                alert.last_seen_at,
                body,
            ],
        )
    })?;
    Ok(())
}

pub fn get(app: &App, id: &str) -> AppResult<Alert> {
    app.store()
        .with(|conn| {
            conn.query_row(
                "SELECT body FROM monitor_alerts WHERE id = ?1",
                params![id],
                row_to_alert,
            )
            .map(Some)
            .or_else(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
        })?
        .ok_or_else(|| AppError::not_found(format!("alert `{id}`")))
}

/// The alert with this key that has not resolved yet, if any.
pub fn open_by_key(app: &App, dedup_key: &str) -> AppResult<Option<Alert>> {
    app.store().with(|conn| {
        conn.query_row(
            "SELECT body FROM monitor_alerts WHERE dedup_key = ?1 AND state != 'resolved'
             ORDER BY started_at DESC LIMIT 1",
            params![dedup_key],
            row_to_alert,
        )
        .map(Some)
        .or_else(|err| match err {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
    })
}

/// Every alert that has not resolved yet for a device.
pub fn open_of_device(app: &App, device_id: &str) -> AppResult<Vec<Alert>> {
    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(
            "SELECT body FROM monitor_alerts WHERE device_id = ?1 AND state != 'resolved'
             ORDER BY started_at DESC",
        )?;
        statement
            .query_map(params![device_id], row_to_alert)?
            .collect()
    })
}

pub fn forget_device(app: &App, device_id: &str) -> AppResult<()> {
    app.store().with(|conn| {
        conn.execute(
            "DELETE FROM monitor_alerts WHERE device_id = ?1",
            params![device_id],
        )
    })?;
    Ok(())
}

/// Drop resolved alerts older than `before`.
pub fn prune(app: &App, before: i64) -> AppResult<usize> {
    app.store().with(|conn| {
        conn.execute(
            "DELETE FROM monitor_alerts WHERE state = 'resolved' AND last_seen_at < ?1",
            params![before],
        )
    })
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct AlertQuery {
    /// `firing`, `acknowledged`, `resolved` or `open` (firing + acknowledged).
    pub state: Option<String>,
    /// At least this severe.
    pub severity: Option<Severity>,
    pub device_id: Option<String>,
    /// Only alerts seen since this time.
    pub since: Option<i64>,
    pub limit: Option<u32>,
    /// Rows already received, as a decimal string.
    pub cursor: Option<String>,
}

pub fn list(app: &App, query: &AlertQuery) -> AppResult<Vec<Alert>> {
    let limit = query.limit.unwrap_or(100).clamp(1, 2_000) as i64;
    let offset = match &query.cursor {
        Some(cursor) if !cursor.is_empty() => cursor.parse::<i64>().map_err(|_| {
            AppError::invalid("the cursor is the number of alerts already received")
        })?,
        _ => 0,
    };
    let mut sql = String::from("SELECT body FROM monitor_alerts WHERE 1 = 1");
    match query.state.as_deref() {
        Some("open") => sql.push_str(" AND state != 'resolved'"),
        Some("firing") => sql.push_str(" AND state = 'firing'"),
        Some("acknowledged") => sql.push_str(" AND state = 'acknowledged'"),
        Some("resolved") => sql.push_str(" AND state = 'resolved'"),
        Some(other) => {
            return Err(AppError::invalid(format!(
                "`{other}` is not an alert state (firing, acknowledged, resolved or open)"
            )));
        }
        None => {}
    }
    if query.severity.is_some() {
        sql.push_str(" AND severity >= :severity");
    }
    if query.device_id.is_some() {
        sql.push_str(" AND device_id = :device_id");
    }
    if query.since.is_some() {
        sql.push_str(" AND last_seen_at >= :since");
    }
    sql.push_str(" ORDER BY started_at DESC, id DESC LIMIT :limit OFFSET :offset");

    let severity = query.severity.map(Severity::rank);
    app.store().with(|conn| {
        let mut statement = conn.prepare_cached(&sql)?;
        let mut bound: Vec<(&str, &dyn rusqlite::ToSql)> =
            vec![(":limit", &limit), (":offset", &offset)];
        if let Some(severity) = &severity {
            bound.push((":severity", severity));
        }
        if let Some(device_id) = &query.device_id {
            bound.push((":device_id", device_id));
        }
        if let Some(since) = &query.since {
            bound.push((":since", since));
        }
        statement
            .query_map(bound.as_slice(), row_to_alert)?
            .collect()
    })
}

#[derive(Debug, Serialize)]
pub struct Summary {
    pub open: u64,
    pub by_severity: std::collections::BTreeMap<String, u64>,
    pub newest: Option<Alert>,
}

pub fn summary(app: &App) -> AppResult<Summary> {
    let open = list(
        app,
        &AlertQuery {
            state: Some("open".into()),
            limit: Some(2_000),
            ..AlertQuery::default()
        },
    )?;
    let mut by_severity = std::collections::BTreeMap::new();
    for alert in &open {
        *by_severity
            .entry(alert.severity.as_str().to_string())
            .or_insert(0) += 1;
    }
    Ok(Summary {
        open: open.len() as u64,
        newest: open.first().cloned(),
        by_severity,
    })
}

// -------------------------------------------------------------- lifecycle

/// Raise an alert, or count another sighting of one already open.
///
/// Returns the alert and whether this call is what raised it.
pub fn fire(app: &App, spec: &AlertSpec, now: i64) -> AppResult<(Alert, bool)> {
    let device = spec
        .device_id
        .as_deref()
        .and_then(|id| inventory::get(app, id).ok());
    let muted = maintenance::covers(app, device.as_ref(), now)?;
    if let Some(mut alert) = open_by_key(app, &spec.dedup_key)? {
        alert.count += 1;
        alert.last_seen_at = now;
        alert.value = spec.value;
        alert.detail = spec.detail.clone();
        alert.severity = spec.severity;
        alert.muted_by_maintenance = muted;
        save(app, &alert)?;
        app.publish("alerts.changed", &json!({"alert": alert}));
        return Ok((alert, false));
    }
    let alert = Alert {
        id: nocturne_app::util::new_id(),
        rule_id: spec.rule_id.clone(),
        rule_name: spec.rule_name.clone(),
        device_id: spec.device_id.clone(),
        device: device.as_ref().map(|device| device.name.clone()),
        instance: spec.instance.clone(),
        severity: spec.severity,
        state: AlertState::Firing,
        title: spec.title.clone(),
        detail: spec.detail.clone(),
        value: spec.value,
        started_at: now,
        last_seen_at: now,
        resolved_at: None,
        acked_at: None,
        ack_note: None,
        snoozed_until: None,
        count: 1,
        muted_by_maintenance: muted,
        lab: device.as_ref().is_some_and(|device| device.lab.is_some()),
        dedup_key: spec.dedup_key.clone(),
    };
    save(app, &alert)?;
    record(app, &alert, "raised");
    app.publish("alerts.changed", &json!({"alert": alert}));
    notify::send(app, &alert, notify::Stage::Firing, &spec.notify);
    Ok((alert, true))
}

/// Resolve the open alert with this key, if there is one.
pub fn resolve_key(
    app: &App,
    dedup_key: &str,
    now: i64,
    note: Option<&str>,
    channels: &[String],
) -> AppResult<Option<Alert>> {
    let Some(mut alert) = open_by_key(app, dedup_key)? else {
        return Ok(None);
    };
    alert.state = AlertState::Resolved;
    alert.resolved_at = Some(now);
    alert.last_seen_at = now;
    if let Some(note) = note {
        alert.detail = note.to_string();
    }
    save(app, &alert)?;
    record(app, &alert, "resolved");
    app.publish("alerts.changed", &json!({"alert": alert}));
    notify::send(app, &alert, notify::Stage::Resolved, channels);
    Ok(Some(alert))
}

fn record(app: &App, alert: &Alert, what: &str) {
    let _ = app.store().log_append(&LogEntry {
        id: 0,
        ts: alert.last_seen_at,
        stream: "event".into(),
        device_id: alert.device_id.clone(),
        severity: Some(alert.severity.syslog()),
        source: "alert".into(),
        message: format!("Alert {what}: {}", alert.title),
        body: Some(json!({
            "alert_id": alert.id,
            "rule": alert.rule_name,
            "severity": alert.severity,
            "instance": alert.instance,
            "value": alert.value,
            "muted_by_maintenance": alert.muted_by_maintenance,
        })),
    });
}

// -------------------------------------------------------------------- RPC

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NoteParams {
    pub id: String,
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SnoozeParams {
    pub id: String,
    pub minutes: u32,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct RaiseParams {
    /// The name of whatever is raising this; shown instead of a rule name.
    pub rule: Option<String>,
    pub device_id: Option<String>,
    pub instance: Option<String>,
    pub severity: Severity,
    pub title: String,
    pub detail: Option<String>,
    pub dedup_key: String,
}

impl Default for RaiseParams {
    fn default() -> Self {
        RaiseParams {
            rule: None,
            device_id: None,
            instance: None,
            severity: Severity::Medium,
            title: String::new(),
            detail: None,
            dedup_key: String::new(),
        }
    }
}

pub fn register(router: &mut nocturne_app::Router) {
    router.add("alerts.list", |app: App, params: AlertQuery| async move {
        list(&app, &params)
    });
    router.add("alerts.get", |app: App, params: IdParams| async move {
        get(&app, &params.id)
    });
    router.add(
        "alerts.summary",
        |app: App, _: nocturne_app::NoParams| async move { summary(&app) },
    );
    router.add("alerts.ack", |app: App, params: NoteParams| async move {
        let now = app.state::<MonitorState>().now();
        let mut alert = get(&app, &params.id)?;
        if alert.state == AlertState::Resolved {
            return Err(AppError::conflict(
                "that alert has already resolved; there is nothing to acknowledge",
            ));
        }
        alert.state = AlertState::Acknowledged;
        alert.acked_at = Some(now);
        alert.ack_note = params.note.clone();
        save(&app, &alert)?;
        app.audit(
            "alert.ack",
            alert.device_id.as_deref(),
            format!("Acknowledged the alert `{}`", alert.title),
            json!({"alert_id": alert.id, "note": params.note}),
        );
        app.publish("alerts.changed", &json!({"alert": alert}));
        Ok(alert)
    });
    router.add(
        "alerts.resolve",
        |app: App, params: NoteParams| async move {
            let now = app.state::<MonitorState>().now();
            let alert = get(&app, &params.id)?;
            if alert.state == AlertState::Resolved {
                return Ok(alert);
            }
            // Resolving by hand goes to the channels the rule would have used.
            let channels = alert
                .rule_id
                .as_deref()
                .and_then(|id| {
                    app.store()
                        .get::<crate::model::AlertRule>(crate::db::collections::RULES, id)
                        .ok()
                        .flatten()
                })
                .map(|rule| rule.notify)
                .unwrap_or_default();
            let resolved = resolve_key(
                &app,
                &alert.dedup_key,
                now,
                params.note.as_deref(),
                &channels,
            )?
            .unwrap_or(alert);
            app.audit(
                "alert.resolve",
                resolved.device_id.as_deref(),
                format!("Resolved the alert `{}` by hand", resolved.title),
                json!({"alert_id": resolved.id, "note": params.note}),
            );
            Ok(resolved)
        },
    );
    router.add(
        "alerts.snooze",
        |app: App, params: SnoozeParams| async move {
            let now = app.state::<MonitorState>().now();
            let mut alert = get(&app, &params.id)?;
            if params.minutes == 0 {
                alert.snoozed_until = None;
            } else {
                let minutes = i64::from(params.minutes.min(60 * 24 * 30));
                alert.snoozed_until = Some(now + minutes * 60_000);
            }
            save(&app, &alert)?;
            app.audit(
                "alert.snooze",
                alert.device_id.as_deref(),
                format!(
                    "Snoozed the alert `{}` for {} minutes",
                    alert.title, params.minutes
                ),
                json!({"alert_id": alert.id, "until": alert.snoozed_until}),
            );
            app.publish("alerts.changed", &json!({"alert": alert}));
            Ok(alert)
        },
    );
    router.add("alerts.raise", |app: App, params: RaiseParams| async move {
        if params.title.trim().is_empty() {
            return Err(AppError::invalid("an alert needs a title"));
        }
        if params.dedup_key.trim().is_empty() {
            return Err(AppError::invalid(
                "an alert needs a dedup_key, so that the same problem seen twice is one alert",
            ));
        }
        if let Some(device_id) = &params.device_id {
            inventory::get(&app, device_id)?;
        }
        let now = app.state::<MonitorState>().now();
        let spec = AlertSpec {
            dedup_key: params.dedup_key.trim().to_string(),
            rule_id: None,
            rule_name: params
                .rule
                .clone()
                .unwrap_or_else(|| "raised by hand".into()),
            device_id: params.device_id.clone(),
            instance: params.instance.clone(),
            severity: params.severity,
            title: params.title.trim().to_string(),
            detail: params.detail.clone().unwrap_or_default(),
            value: None,
            notify: Vec::new(),
        };
        let (alert, _) = fire(&app, &spec, now)?;
        Ok::<Value, AppError>(serde_json::to_value(alert)?)
    });
}
