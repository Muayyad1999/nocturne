//! Alert rules: the built-in set, the store behind `alerts.rules.*`, and the
//! evaluation that runs after every poll.
//!
//! Evaluation is deliberately tri-state. A condition is *active*, *clear*, or
//! **not known** — and "not known" is the common case, because a poll that
//! could not read a device says nothing about its CPU. Only an active verdict
//! raises, only a clear verdict resolves; anything else leaves the alert
//! exactly as it was. That is what stops a failed SNMP read from quietly
//! resolving half the alerts on a device.
//!
//! A verdict is also only as good as the measurement behind it. Most rules
//! judge the poll in hand; a service-check rule judges what the check last
//! *found*, which may be minutes old, so each of its findings says when that
//! was ([`Finding::observed_at`]). What a check found before its device came
//! back from an outage is not known to be true any more; and looking twice at
//! one run is one sighting, not two.

use nocturne_app::model::Device;
use nocturne_app::{App, AppError, AppResult, Router, inventory, util};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::alerts::{self, AlertSpec};
use crate::checks::model::CheckStatus;
use crate::db::collections::RULES;
use crate::metrics::{self, glob};
use crate::model::{
    AlertRule, CheckAlertState, Condition, DeviceRecord, IfStatus, InterfaceSelector, Op, Scope,
    Severity,
};
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
        // The three service-check rules. They are separate rules rather than
        // one, because they are three different things to tell somebody and
        // they do not deserve the same severity: a service being down is an
        // incident, a certificate expiring next week is a job for Monday,
        // and a check that cannot run is neither — it is monitoring that has
        // stopped being able to answer, which has to be visible or it hides
        // the first two.
        builtin(
            "service_check_down",
            "Service check down",
            Severity::High,
            0,
            Condition::CheckState {
                check: None,
                kinds: Vec::new(),
                state: CheckAlertState::Down,
            },
        ),
        builtin(
            "service_check_warning",
            "Certificate about to expire",
            Severity::Low,
            0,
            Condition::CheckState {
                check: None,
                kinds: Vec::new(),
                state: CheckAlertState::Warning,
            },
        ),
        builtin(
            "service_check_cannot_run",
            "Service check cannot run",
            Severity::Medium,
            // Ten minutes: a credential being saved, or a name coming back,
            // should not wake anybody.
            600,
            Condition::CheckState {
                check: None,
                kinds: Vec::new(),
                state: CheckAlertState::CannotRun,
            },
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
    /// When what this finding rests on was measured, where that was not the
    /// evaluation itself: the run of the service check it judges. `None` for
    /// a finding read off the poll in hand.
    pub observed_at: Option<i64>,
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
            Facts::Stored(_, device) => {
                device_record(app, &device.id).is_some_and(|record| record.down)
            }
        }
    }

    /// When the device last came back from being down, if it ever has.
    fn up_since(&self, app: &App) -> Option<i64> {
        match self {
            Facts::Live(poll) => poll.up_since,
            Facts::Stored(_, device) => {
                device_record(app, &device.id).and_then(|record| record.up_since)
            }
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

fn device_record(app: &App, device_id: &str) -> Option<DeviceRecord> {
    app.store()
        .get::<DeviceRecord>(crate::db::collections::DEVICES, device_id)
        .ok()
        .flatten()
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
                    observed_at: None,
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
                        observed_at: None,
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
                        observed_at: None,
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
                        observed_at: None,
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
                    observed_at: None,
                }],
                true,
            ))
        }
        Condition::CheckState {
            check,
            kinds,
            state,
        } => {
            // Judged from the checks' stored state rather than from the poll
            // that happens to be in hand, so the same code answers after a
            // poll, after one check has run, and for `alerts.rules.test`.
            let wanted = check.as_deref().unwrap_or("*");
            let rows = crate::checks::store::of_device(app, facts.device_id())?;
            let up_since = facts.up_since(app);
            let mut findings = Vec::new();
            for (service, current) in rows {
                if !service.enabled
                    || !glob(wanted, &service.name)
                    || (!kinds.is_empty() && !kinds.contains(&service.kind()))
                {
                    continue;
                }
                // The poller is usually first to notice that a device is
                // back, and what its checks hold is then still what they
                // found while it was away: `down`, for every one of them,
                // recorded without an alert because the outage explained
                // it. Judged now, that raises a service-down alert for each
                // check after the outage is over. A verdict from before the
                // device came back is not a verdict on the device that came
                // back; the check's own next run is.
                let from_before_the_outage_ended =
                    matches!((current.last_run_at, up_since), (Some(ran), Some(up)) if ran < up);
                let verdict = match (*state, current.status) {
                    _ if from_before_the_outage_ended => Verdict::Unknown,
                    // Nothing has been found yet, or the check has just been
                    // pointed somewhere else: say nothing either way.
                    (_, CheckStatus::Unknown) => Verdict::Unknown,
                    (CheckAlertState::Down, CheckStatus::Down) => Verdict::Active,
                    (CheckAlertState::Warning, CheckStatus::Warning) => Verdict::Active,
                    (CheckAlertState::CannotRun, CheckStatus::CannotRun) => Verdict::Active,
                    // A check that cannot run has not found the service
                    // healthy — it has found nothing. Resolving a
                    // service-down alert on the strength of it would read as
                    // a recovery that never happened.
                    (CheckAlertState::Down | CheckAlertState::Warning, CheckStatus::CannotRun) => {
                        Verdict::Unknown
                    }
                    // Nor has a run that found the service down learned
                    // anything about its certificate: a timeout while the web
                    // server restarts does not move the expiry date. The one
                    // exception is a run that did see the certificate and
                    // found it expired — it is no longer *about* to expire,
                    // and the outage it has become is the down rule's.
                    (CheckAlertState::Warning, CheckStatus::Down) => {
                        if current.code == "tls_expired" {
                            Verdict::Clear
                        } else {
                            Verdict::Unknown
                        }
                    }
                    _ => Verdict::Clear,
                };
                let detail = match verdict {
                    Verdict::Active if current.message.is_empty() => format!(
                        "The {} check `{}` is {}.",
                        service.kind().as_str(),
                        service.name,
                        current.status.as_str()
                    ),
                    Verdict::Active => format!(
                        "The {} check `{}` is {}: {}",
                        service.kind().as_str(),
                        service.name,
                        current.status.as_str(),
                        current.message
                    ),
                    _ => format!(
                        "The {} check `{}` is {}.",
                        service.kind().as_str(),
                        service.name,
                        current.status.as_str()
                    ),
                };
                findings.push(Finding {
                    instance: Some(service.name.clone()),
                    verdict,
                    // For a certificate warning the measurement *is* the
                    // number of days, which is what a person wants to see.
                    value: match state {
                        CheckAlertState::Warning => {
                            current.facts.cert_expires_in_days.map(|days| days as f64)
                        }
                        _ => None,
                    },
                    detail,
                    observed_at: current.last_run_at,
                });
            }
            // The stored state is the whole truth about this device's
            // checks, so an alert about a check that is no longer there can
            // be resolved.
            Ok((findings, true))
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

/// Run the service-check rules for a device, after one of its checks has run.
///
/// A check running is not a poll, but it is still the rules engine that
/// decides whether anybody hears about it: the same scope, the same `for_s`
/// damping, the same hysteresis, the same maintenance windows, the same
/// one-cause-one-alert suppression while the device itself is down. The only
/// thing this does differently from [`evaluate`] is leave the rules that are
/// not about checks alone, because nothing they watch has moved.
pub async fn evaluate_checks(app: &App, device_id: &str) {
    if let Err(err) = evaluate_checks_inner(app, device_id) {
        tracing::warn!(device = %device_id, %err, "the service-check rules could not be evaluated");
    }
}

fn evaluate_checks_inner(app: &App, device_id: &str) -> AppResult<()> {
    let device = inventory::get(app, device_id)?;
    let state = app.state::<MonitorState>();
    let now = state.now();
    if !device.monitor.enabled {
        return stand_down(app, &device, now);
    }
    let facts = Facts::Stored(app, &device);
    let down = facts.down(app);
    for rule in list(app)? {
        if !rule.enabled
            || !rule.condition.is_check()
            || !scope::matches(&rule.scope, &device, true)
        {
            continue;
        }
        if down && rule.condition.suppressed_by_device_down() {
            continue;
        }
        let (findings, complete) = evaluate_rule(app, &rule, &facts, now)?;
        apply_findings(app, &state, &rule, &device, &findings, complete, now)?;
    }
    Ok(())
}

fn evaluate_inner(app: &App, poll: &Poll) -> AppResult<()> {
    let device = inventory::get(app, &poll.device_id)?;
    let state = app.state::<MonitorState>();
    let now = poll.at;
    // `monitor.poll_now` will poll any device it is given. What it finds on
    // one whose monitoring is off is an answer for whoever asked, not a
    // condition anybody is watching.
    if !device.monitor.enabled {
        return stand_down(app, &device, now);
    }
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
    // From here on what the device's alerts say is from this run of the
    // application, and a notice they still owe may go out on it.
    state.device(&device.id, |runtime| runtime.evaluated = true);
    Ok(())
}

/// Close what the rules have open on a device whose monitoring is switched
/// off, and say nothing new about it.
///
/// An alert is a claim that something is wrong *now*, kept true by the next
/// poll or the next run. With monitoring off for the device there is no next
/// one: nothing would ever resolve the alert, and the screen would go on
/// saying "web on SW9 is down" about a switch somebody unplugged on purpose
/// last month. So the rules stand down: whatever they raised on the device is
/// resolved, with a note that says why, when the switch is thrown
/// (`devices.changed`, in the scheduler's event task) and again at any
/// evaluation that still comes its way.
///
/// Two kinds of alert are left alone. One raised by another feature through
/// `alerts.raise` is not the rules' to close. And a configuration-change
/// alert records something that happened, not a state somebody has to keep
/// watching for it to stay true.
pub fn stand_down(app: &App, device: &Device, now: i64) -> AppResult<()> {
    let state = app.state::<MonitorState>();
    let rules = list(app)?;
    for alert in alerts::open_of_device(app, &device.id)? {
        let Some(rule_id) = alert.rule_id.as_deref() else {
            continue;
        };
        let rule = rules.iter().find(|rule| rule.id == rule_id);
        if rule.is_some_and(|rule| matches!(rule.condition, Condition::ConfigChanged)) {
            continue;
        }
        alerts::resolve_key(
            app,
            &alert.dedup_key,
            now,
            None,
            Some(
                "Monitoring was switched off for this device, so this is no longer being watched.",
            ),
            rule.map(|rule| rule.notify.as_slice()).unwrap_or_default(),
        )?;
    }
    // Conditions that were waiting out their `for_s` start again from
    // nothing if monitoring comes back: the wait is for a condition that
    // has been *seen* to hold, and nobody has been looking.
    let ours = format!(":{}:", device.id);
    state
        .runtime
        .lock()
        .pending
        .retain(|key, _| !key.contains(&ours));
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
                        let since = runtime.pending.entry(key.clone()).or_insert(now);
                        // `now` is a time of day and can be set back. A wait
                        // that began "later than now" would then last for
                        // the size of the step on top of `for_s`; it starts
                        // over instead.
                        if *since > now {
                            *since = now;
                        }
                        *since
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
                    only: None,
                    observed_at: finding.observed_at,
                };
                alerts::fire(app, &spec, now)?;
                state.runtime.lock().pending.remove(&key);
            }
            Verdict::Clear => {
                state.runtime.lock().pending.remove(&key);
                alerts::resolve_key(
                    app,
                    &key,
                    now,
                    finding.observed_at,
                    Some(&finding.detail),
                    &rule.notify,
                )?;
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
                    None,
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
            only: None,
            observed_at: None,
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
