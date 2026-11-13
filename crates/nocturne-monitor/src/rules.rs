//! Alert rules: the built-in set, the store behind `alerts.rules.*`, and the
//! evaluation that runs after every poll.
//!
//! Evaluation is deliberately tri-state. A condition is *active*, *clear*, or
//! **not known** — and "not known" is the common case, because a poll that
//! could not read a device says nothing about its CPU. Only an active verdict
//! raises, only a clear verdict resolves; anything else leaves the alert
//! exactly as it was. That is what stops a failed SNMP read from quietly
//! resolving half the alerts on a device.

use nocturne_app::model::Device;
use nocturne_app::{App, AppError, AppResult, Router, inventory, util};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::alerts::{self, AlertSpec};
use crate::db::collections::RULES;
use crate::metrics::{self, glob};
use crate::model::{AlertRule, Condition, IfStatus, InterfaceSelector, Op, Scope, Severity};
use crate::poll::Poll;
use crate::state::MonitorState;
use crate::{interfaces, scope};

// ------------------------------------------------------------- built-ins

fn builtin(
    id: &str,
    name: &str,
    severity: Severity,
    for_s: u64,
    condition: Condition,
) -> AlertRule {
    AlertRule {
        id: format!("builtin.{id}"),
        name: name.to_string(),
        enabled: true,
        builtin: true,
        severity,
        scope: Scope::default(),
        condition,
        for_s,
        notify: Vec::new(),
        created_at: 0,
        updated_at: 0,
    }
}

fn threshold(metric: &str, value: f64, clear: f64) -> Condition {
    Condition::Threshold {
        metric: metric.to_string(),
        instance: None,
        op: Op::Gt,
        value,
        clear_value: Some(clear),
    }
}

/// The rules Nocturne ships with. Each is a real document that can be edited
/// and disabled; `builtin` only means it cannot be deleted outright.
pub fn builtins() -> Vec<AlertRule> {
    vec![
        builtin(
            "device_down",
            "Device down",
            Severity::Critical,
            0,
            Condition::DeviceDown,
        ),
        builtin(
            "uplink_down",
            "Uplink down",
            Severity::High,
            0,
            Condition::InterfaceDown {
                only: InterfaceSelector::Uplinks,
            },
        ),
        builtin(
            "cpu_high",
            "CPU above 90 % for 5 minutes",
            Severity::High,
            300,
            threshold("cpu.pct", 90.0, 80.0),
        ),
        builtin(
            "memory_high",
            "Memory above 90 % for 10 minutes",
            Severity::Medium,
            600,
            threshold("mem.pct", 90.0, 85.0),
        ),
        builtin(
            "temperature_high",
            "Temperature above 70 °C",
            Severity::High,
            0,
            threshold("temp.c", 70.0, 65.0),
        ),
        builtin(
            "interface_errors",
            "Interface errors above 10/s for 5 minutes",
            Severity::Medium,
            300,
            threshold("if.*_err_ps", 10.0, 1.0),
        ),
        builtin(
            "interface_utilisation",
            "Interface above 90 % utilisation for 10 minutes",
            Severity::Medium,
            600,
            threshold("if.util_pct", 90.0, 80.0),
        ),
        builtin(
            "packet_loss",
            "Packet loss above 20 % for 3 minutes",
            Severity::Medium,
            180,
            threshold("icmp.loss_pct", 20.0, 5.0),
        ),
        builtin(
            "rtt_high",
            "Round-trip time above 5× the 24-hour median for 5 minutes",
            Severity::Low,
            300,
            Condition::RttBaseline {
                multiple: 5.0,
                window_s: 86_400,
            },
        ),
        builtin(
            "config_changed",
            "Configuration changed",
            Severity::Info,
            0,
            Condition::ConfigChanged,
        ),
    ]
}

/// Write the built-in rules that are not there yet. A rule the user has
/// edited or disabled keeps whatever they made of it.
pub fn ensure_builtins(app: &App) -> AppResult<()> {
    let now = app.state::<MonitorState>().now();
    for mut rule in builtins() {
        if app.store().get::<AlertRule>(RULES, &rule.id)?.is_some() {
            continue;
        }
        rule.created_at = now;
        rule.updated_at = now;
        app.store().put(RULES, &rule.id, &rule)?;
    }
    Ok(())
}

pub fn list(app: &App) -> AppResult<Vec<AlertRule>> {
    let mut rules: Vec<AlertRule> = app.store().list(RULES)?;
    rules.sort_by_key(|rule| (-rule.severity.rank(), rule.name.to_lowercase()));
    Ok(rules)
}

// ------------------------------------------------------------- evaluation

/// What one instance of one rule says right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The condition holds.
    Active,
    /// The condition definitely does not hold any more.
    Clear,
    /// Past the firing threshold but not yet past the clearing one: neither
    /// raise nor resolve (this is what `clear_value` buys).
    Hold,
    /// Nothing was read: say nothing.
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub instance: Option<String>,
    pub verdict: Verdict,
    pub value: Option<f64>,
    pub detail: String,
}

/// What a rule is evaluated against: a poll that has just happened, or
/// whatever is stored (for `alerts.rules.test`).
pub enum Facts<'a> {
    Live(&'a Poll),
    Stored(&'a App, &'a Device),
}

impl Facts<'_> {
    fn device_id(&self) -> &str {
        match self {
            Facts::Live(poll) => &poll.device_id,
            Facts::Stored(_, device) => &device.id,
        }
    }

    fn down(&self, app: &App) -> bool {
        match self {
            Facts::Live(poll) => poll.down,
            Facts::Stored(_, device) => app
                .store()
                .get::<crate::model::DeviceRecord>(crate::db::collections::DEVICES, &device.id)
                .ok()
                .flatten()
                .is_some_and(|record| record.down),
        }
    }

    /// `(instance, value)` of every measurement whose metric matches.
    fn measures(&self, app: &App, metric: &str) -> AppResult<Vec<(String, f64)>> {
        match self {
            Facts::Live(poll) => Ok(poll
                .measures
                .iter()
                .filter(|measure| glob(metric, &measure.metric))
                .map(|measure| (measure.instance.clone(), measure.value))
                .collect()),
            Facts::Stored(app_ref, device) => {
                let _ = app;
                Ok(metrics::latest_matching(app_ref, &device.id, metric)?
                    .into_iter()
                    .map(|latest| (latest.instance, latest.value))
                    .collect())
            }
        }
    }

    /// The interface table, or `None` when this poll did not read it.
    fn interfaces(&self, app: &App) -> AppResult<Option<Vec<IfStatus>>> {
        match self {
            Facts::Live(poll) => Ok(poll.interfaces_read.then(|| poll.interfaces.clone())),
            Facts::Stored(_, device) => {
                let rows = interfaces::of_device(app, &device.id)?;
                Ok((!rows.is_empty()).then_some(rows))
            }
        }
    }
}

/// Evaluate one rule for one device. The flag says whether the instance list
/// is complete, i.e. whether alerts for instances it does not mention may be
/// resolved.
pub fn evaluate_rule(
    app: &App,
    rule: &AlertRule,
    facts: &Facts<'_>,
    now: i64,
) -> AppResult<(Vec<Finding>, bool)> {
    match &rule.condition {
        Condition::DeviceDown => {
            let down = facts.down(app);
            Ok((
                vec![Finding {
                    instance: None,
                    verdict: if down {
                        Verdict::Active
                    } else {
                        Verdict::Clear
                    },
                    value: None,
                    detail: if down {
                        "The device has not answered for several polls in a row.".into()
                    } else {
                        "The device is answering again.".into()
                    },
                }],
                true,
            ))
        }
        Condition::InterfaceDown { only } => {
            let Some(rows) = facts.interfaces(app)? else {
                return Ok((Vec::new(), false));
            };
            let findings = rows
                .into_iter()
                .filter(|row| match only {
                    InterfaceSelector::Uplinks => row.uplink,
                    InterfaceSelector::Described => !row.description.trim().is_empty(),
                    InterfaceSelector::All => true,
                })
                .map(|row| {
                    let down = row.admin_up == Some(true) && row.oper_up == Some(false);
                    Finding {
                        detail: if down {
                            format!(
                                "{} is administratively up but its line protocol is down{}.",
                                row.name,
                                if row.description.trim().is_empty() {
                                    String::new()
                                } else {
                                    format!(" ({})", row.description.trim())
                                }
                            )
                        } else {
                            format!("{} is up again.", row.name)
                        },
                        instance: Some(row.name),
                        verdict: if down {
                            Verdict::Active
                        } else {
                            Verdict::Clear
                        },
                        value: None,
                    }
                })
                .collect();
            Ok((findings, true))
        }
        Condition::Threshold {
            metric,
            instance,
            op,
            value,
            clear_value,
        } => {
            let measures = facts.measures(app, metric)?;
            let wanted = instance.as_deref().unwrap_or("*");
            let matching: Vec<(String, f64)> = measures
                .into_iter()
                .filter(|(name, _)| glob(wanted, name))
                .collect();
            if matching.is_empty() {
                // Nothing was measured this time: a gap, not a clear.
                return Ok((Vec::new(), false));
            }
            let findings = matching
                .into_iter()
                .map(|(name, reading)| {
                    let verdict = if op.holds(reading, *value) {
                        Verdict::Active
                    } else {
                        match clear_value {
                            Some(clear) if op.holds(reading, *clear) => Verdict::Hold,
                            _ => Verdict::Clear,
                        }
                    };
                    Finding {
                        detail: format!(
                            "{metric}{} is {reading:.2}, {} {value}.",
                            if name.is_empty() {
                                String::new()
                            } else {
                                format!(" on {name}")
                            },
                            op.as_str()
                        ),
                        instance: (!name.is_empty()).then(|| name.clone()),
                        verdict,
                        value: Some(reading),
                    }
                })
                .collect();
            Ok((findings, true))
        }
        Condition::NoData { metric, for_s } => {
            let latest = metrics::latest_matching(app, facts.device_id(), metric)?;
            if latest.is_empty() {
                // The device has never reported this metric: that is not the
                // same as having stopped, and is not this rule's business.
                return Ok((Vec::new(), false));
            }
            let stale_after = i64::try_from(*for_s)
                .unwrap_or(i64::MAX)
                .saturating_mul(1_000);
            let findings = latest
                .into_iter()
                .map(|entry| {
                    let age = now - entry.at;
                    let stale = age > stale_after;
                    Finding {
                        detail: format!(
                            "{} has had no new value for {} s.",
                            entry.metric,
                            age / 1_000
                        ),
                        instance: (!entry.instance.is_empty()).then_some(entry.instance),
                        verdict: if stale {
                            Verdict::Active
                        } else {
                            Verdict::Clear
                        },
                        value: None,
                    }
                })
                .collect();
            Ok((findings, true))
        }
        Condition::RttBaseline { multiple, window_s } => {
            let current = facts
                .measures(app, "icmp.rtt_ms")?
                .into_iter()
                .map(|(_, value)| value)
                .next();
            let Some(current) = current else {
                return Ok((Vec::new(), false));
            };
            let Some(median) = rtt_median(app, facts.device_id(), *window_s, now)? else {
                return Ok((Vec::new(), false));
            };
            if median <= 0.0 {
                return Ok((Vec::new(), false));
            }
            let limit = median * multiple;
            Ok((
                vec![Finding {
                    instance: None,
                    verdict: if current > limit {
                        Verdict::Active
                    } else {
                        Verdict::Clear
                    },
                    value: Some(current),
                    detail: format!(
                        "Round-trip time is {current:.1} ms, against a {} h median of {median:.1} ms.",
                        window_s / 3_600
                    ),
                }],
                true,
            ))
        }
        // Raised from the `config.changed` event, not from a poll.
        Condition::ConfigChanged => Ok((Vec::new(), false)),
    }
}

/// The median of a device's round-trip times over a window.
fn rtt_median(app: &App, device_id: &str, window_s: u64, now: i64) -> AppResult<Option<f64>> {
    let window = i64::try_from(window_s)
        .unwrap_or(86_400)
        .saturating_mul(1_000);
    let series = app.store().series_id(device_id, "icmp.rtt_ms", "")?;
    let points = app
        .store()
        .samples_query(series, now - window, now, 1_000)?;
    // Too few readings to call anything normal.
    if points.len() < 5 {
        return Ok(None);
    }
    let mut values: Vec<f64> = points.iter().map(|point| point.avg).collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = values.len() / 2;
    Ok(Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }))
}

pub fn dedup_key(rule_id: &str, device_id: &str, instance: Option<&str>) -> String {
    format!("rule:{rule_id}:{device_id}:{}", instance.unwrap_or(""))
}

/// Run every rule in scope for the device that was just polled.
pub async fn evaluate(app: &App, poll: &Poll) {
    if let Err(err) = evaluate_inner(app, poll) {
        tracing::warn!(device = %poll.device_id, %err, "alert rules could not be evaluated");
    }
}

fn evaluate_inner(app: &App, poll: &Poll) -> AppResult<()> {
    let device = inventory::get(app, &poll.device_id)?;
    let state = app.state::<MonitorState>();
    let now = poll.at;
    let facts = Facts::Live(poll);
    for rule in list(app)? {
        if !rule.enabled || !scope::matches(&rule.scope, &device, true) {
            continue;
        }
        // One cause, one alert: while the device itself is down, the things
        // that are down *because* of it say nothing.
        if poll.down && rule.condition.suppressed_by_device_down() {
            continue;
        }
        let (findings, complete) = evaluate_rule(app, &rule, &facts, now)?;
        apply_findings(app, &state, &rule, &device, &findings, complete, now)?;
    }
    Ok(())
}

fn apply_findings(
    app: &App,
    state: &MonitorState,
    rule: &AlertRule,
    device: &Device,
    findings: &[Finding],
    complete: bool,
    now: i64,
) -> AppResult<()> {
    let mut seen: Vec<String> = Vec::new();
    for finding in findings {
        let key = dedup_key(&rule.id, &device.id, finding.instance.as_deref());
        seen.push(key.clone());
        match finding.verdict {
            Verdict::Active => {
                let open = alerts::open_by_key(app, &key)?.is_some();
                if !open && rule.for_s > 0 {
                    let since = {
                        let mut runtime = state.runtime.lock();
                        *runtime.pending.entry(key.clone()).or_insert(now)
                    };
                    let waited = now - since;
                    if waited < i64::try_from(rule.for_s).unwrap_or(0).saturating_mul(1_000) {
                        continue;
                    }
                }
                let spec = AlertSpec {
                    dedup_key: key.clone(),
                    rule_id: Some(rule.id.clone()),
                    rule_name: rule.name.clone(),
                    device_id: Some(device.id.clone()),
                    instance: finding.instance.clone(),
                    severity: rule.severity,
                    title: title(rule, device, finding),
                    detail: finding.detail.clone(),
                    value: finding.value,
                    notify: rule.notify.clone(),
                };
                alerts::fire(app, &spec, now)?;
                state.runtime.lock().pending.remove(&key);
            }
            Verdict::Clear => {
                state.runtime.lock().pending.remove(&key);
                alerts::resolve_key(app, &key, now, Some(&finding.detail), &rule.notify)?;
            }
            Verdict::Hold | Verdict::Unknown => {}
        }
    }
    if complete {
        // An instance that has gone away (a port removed, a sensor that
        // stopped reporting) cannot clear itself, so it is resolved here.
        for alert in alerts::open_of_device(app, &device.id)? {
            if alert.rule_id.as_deref() == Some(rule.id.as_str())
                && !seen.contains(&alert.dedup_key)
            {
                state.runtime.lock().pending.remove(&alert.dedup_key);
                alerts::resolve_key(
                    app,
                    &alert.dedup_key,
                    now,
                    Some("The thing this alert was about is no longer reported."),
                    &rule.notify,
                )?;
            }
        }
    }
    Ok(())
}

fn title(rule: &AlertRule, device: &Device, finding: &Finding) -> String {
    match &finding.instance {
        Some(instance) => format!("{} — {} on {}", rule.name, instance, device.name),
        None => format!("{} — {}", rule.name, device.name),
    }
}

/// Raise the `ConfigChanged` rules for a device whose configuration changed.
pub fn on_config_changed(app: &App, device_id: &str, payload: &Value) -> AppResult<()> {
    let device = inventory::get(app, device_id)?;
    let now = app.state::<MonitorState>().now();
    for rule in list(app)? {
        if !rule.enabled
            || rule.condition != Condition::ConfigChanged
            || !scope::matches(&rule.scope, &device, true)
        {
            continue;
        }
        let added = payload["added"].as_i64().unwrap_or(0);
        let removed = payload["removed"].as_i64().unwrap_or(0);
        let version = payload["version_id"].as_i64().unwrap_or(0);
        let spec = AlertSpec {
            // Every change is its own alert, so two changes are two entries.
            dedup_key: format!("rule:{}:{device_id}:{version}", rule.id),
            rule_id: Some(rule.id.clone()),
            rule_name: rule.name.clone(),
            device_id: Some(device.id.clone()),
            instance: payload["kind"].as_str().map(str::to_string),
            severity: rule.severity,
            title: format!("{} — {}", rule.name, device.name),
            detail: format!("{added} lines added, {removed} removed."),
            value: None,
            notify: rule.notify.clone(),
        };
        alerts::fire(app, &spec, now)?;
    }
    Ok(())
}

// -------------------------------------------------------------------- RPC

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SaveParams {
    pub id: Option<String>,
    pub name: String,
    pub enabled: Option<bool>,
    pub severity: Option<Severity>,
    pub scope: Option<Scope>,
    pub condition: Option<Condition>,
    pub for_s: Option<u64>,
    pub notify: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct TestParams {
    /// An existing rule to try, or …
    pub id: Option<String>,
    /// … a rule that has not been saved yet.
    pub rule: Option<AlertRule>,
    /// Try it on these devices only.
    pub device_ids: Vec<String>,
}

pub fn save(app: &App, params: SaveParams) -> AppResult<AlertRule> {
    let now = app.state::<MonitorState>().now();
    let existing = match &params.id {
        Some(id) if !id.is_empty() => {
            Some(app.store().require::<AlertRule>(RULES, id, "alert rule")?)
        }
        _ => None,
    };
    let name = if params.name.trim().is_empty() {
        existing
            .as_ref()
            .map(|rule| rule.name.clone())
            .unwrap_or_default()
    } else {
        params.name.trim().to_string()
    };
    if name.is_empty() {
        return Err(AppError::invalid("give the rule a name"));
    }
    let condition = params
        .condition
        .or_else(|| existing.as_ref().map(|rule| rule.condition.clone()))
        .ok_or_else(|| AppError::invalid("a rule needs a condition"))?;
    validate(&condition)?;
    let rule = AlertRule {
        id: existing
            .as_ref()
            .map(|rule| rule.id.clone())
            .unwrap_or_else(util::new_id),
        name,
        enabled: params
            .enabled
            .or(existing.as_ref().map(|rule| rule.enabled))
            .unwrap_or(true),
        builtin: existing.as_ref().is_some_and(|rule| rule.builtin),
        severity: params
            .severity
            .or(existing.as_ref().map(|rule| rule.severity))
            .unwrap_or(Severity::Medium),
        scope: params
            .scope
            .or_else(|| existing.as_ref().map(|rule| rule.scope.clone()))
            .unwrap_or_default(),
        condition,
        for_s: params
            .for_s
            .or(existing.as_ref().map(|rule| rule.for_s))
            .unwrap_or(0)
            .min(86_400),
        notify: params
            .notify
            .or_else(|| existing.as_ref().map(|rule| rule.notify.clone()))
            .unwrap_or_default(),
        created_at: existing.as_ref().map(|rule| rule.created_at).unwrap_or(now),
        updated_at: now,
    };
    app.store().put(RULES, &rule.id, &rule)?;
    app.audit(
        "alert.rule.save",
        None,
        format!("Saved the alert rule `{}`", rule.name),
        json!({"id": rule.id, "enabled": rule.enabled, "condition": rule.condition}),
    );
    app.publish("monitor.status_changed", &json!({"what": "rules"}));
    Ok(rule)
}

fn validate(condition: &Condition) -> AppResult<()> {
    match condition {
        Condition::Threshold { metric, value, .. } => {
            if metric.trim().is_empty() {
                return Err(AppError::invalid("a threshold rule needs a metric"));
            }
            if !value.is_finite() {
                return Err(AppError::invalid("a threshold needs a real number"));
            }
        }
        Condition::NoData { metric, for_s } => {
            if metric.trim().is_empty() {
                return Err(AppError::invalid("a no-data rule needs a metric"));
            }
            if *for_s == 0 {
                return Err(AppError::invalid(
                    "a no-data rule needs a period after which the metric counts as missing",
                ));
            }
        }
        Condition::RttBaseline { multiple, window_s } => {
            if *multiple <= 1.0 || !multiple.is_finite() {
                return Err(AppError::invalid(
                    "the round-trip multiple has to be greater than 1",
                ));
            }
            if *window_s < 600 {
                return Err(AppError::invalid(
                    "the baseline window has to be at least ten minutes",
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn register(router: &mut Router) {
    router.add(
        "alerts.rules.list",
        |app: App, _: nocturne_app::NoParams| async move { list(&app) },
    );
    router.add(
        "alerts.rules.save",
        |app: App, params: SaveParams| async move { save(&app, params) },
    );
    router.add(
        "alerts.rules.delete",
        |app: App, params: IdParams| async move {
            let rule: AlertRule = app.store().require(RULES, &params.id, "alert rule")?;
            if rule.builtin {
                return Err(AppError::forbidden(format!(
                    "`{}` is a built-in rule and cannot be deleted; switch it off instead, or change what it watches",
                    rule.name
                )));
            }
            app.store().delete(RULES, &params.id)?;
            app.audit(
                "alert.rule.delete",
                None,
                format!("Deleted the alert rule `{}`", rule.name),
                json!({"id": rule.id}),
            );
            app.publish("monitor.status_changed", &json!({"what": "rules"}));
            Ok(json!({"deleted": true}))
        },
    );
    router.add("alerts.rules.test", |app: App, params: TestParams| async move {
        let rule = match (&params.id, params.rule.clone()) {
            (Some(id), _) if !id.is_empty() => {
                app.store().require::<AlertRule>(RULES, id, "alert rule")?
            }
            (_, Some(rule)) => rule,
            _ => return Err(AppError::invalid("give a rule id, or a rule to try")),
        };
        validate(&rule.condition)?;
        let now = app.state::<MonitorState>().now();
        let candidates = if params.device_ids.is_empty() {
            scope::devices(&app, &rule.scope, true)?
        } else {
            params
                .device_ids
                .iter()
                .map(|id| inventory::get(&app, id))
                .collect::<AppResult<Vec<Device>>>()?
                .into_iter()
                .filter(|device| scope::matches(&rule.scope, device, true))
                .collect()
        };
        let mut matches = Vec::new();
        let mut checked = 0;
        for device in &candidates {
            checked += 1;
            let facts = Facts::Stored(&app, device);
            let (findings, _) = evaluate_rule(&app, &rule, &facts, now)?;
            for finding in findings {
                if finding.verdict == Verdict::Active {
                    matches.push(json!({
                        "device_id": device.id,
                        "device": device.name,
                        "instance": finding.instance,
                        "value": finding.value,
                        "detail": finding.detail,
                        "would_wait_s": rule.for_s,
                    }));
                }
            }
        }
        Ok(json!({
            "devices_in_scope": checked,
            "matches": matches,
            "note": if matches!(rule.condition, Condition::ConfigChanged) {
                "This rule fires on configuration changes as they happen, so there is nothing to try against stored data."
            } else {
                ""
            },
        }))
    });
}
