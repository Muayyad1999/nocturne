//! Pattern rules: turning lines into signals the rest of the application
//! reacts to.
//!
//! A rule is a regular expression over the line **as the device wrote it**
//! (mnemonic included, which is what vendor documentation shows and what a
//! user pastes into `logs.rules.test`) — or, for a rule on the `trap`
//! stream, over the message the trap receiver wrote for the notification
//! (`linkDown ifIndex=3 …`) — a scope, and one signal. Matching a
//! line is not interesting by itself; what matters is that the configuration
//! feature backs a switch up when it hears `logs.config_changed`, and that a
//! port flapping four hundred times raises one alert and a count.
//!
//! Every matching line passes through three stages, in this order and all
//! of them here, because this is the only place every line passes:
//!
//! 1. **The threshold**, when the rule has one: the line is counted for its
//!    device and its group, and nothing more happens until `count` of them
//!    have arrived inside `window_s`. This cannot be done from the outside —
//!    the signals are throttled, so a listener sees the first match of a
//!    window and never the other seven.
//! 2. **The throttle**, per rule, device and instance (the interface, or
//!    the group). The first match of a window is published at once — a NOC
//!    wants to hear about the first one immediately — and the rest are
//!    counted; the next signal after the window carries that count, so
//!    nothing is lost, only compressed. A second port going down inside the
//!    window is a second instance and a second signal.
//! 3. **The signal**: stored in the `event` stream, published, and for an
//!    `Alert` handed to `alerts.raise` when the monitor is in the build.
//!
//! An alert a rule raised is cleared again — `alerts.clear` with the same
//! dedup key — by the rule's opposite event (`clear_pattern`) or after
//! `clear_after_s` without a matching line. What a rule remembers in order
//! to do all this is in [`track`], and is bounded.
//!
//! Everything a signal says about a line comes out of text a device wrote,
//! on a port anybody can write to. It is treated as text and nothing else:
//! what becomes a key, a title or an alert's detail is cut to a length and
//! loses its control characters ([`plain`]), and the calls to the monitor
//! leave through one bounded queue in the order they were decided
//! ([`Outbox`]).

pub mod builtin;
pub mod track;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Value, json};
use tokio::sync::mpsc::{Receiver, Sender, channel};

use self::track::{Ask, Key, OpenAlert, Opposite, Reason, Verdict};
use crate::attribute::Index;
use crate::db::Prepared;
use crate::model::{LogRule, RuleStatus, RuleTest, Scope, Signal, SignalPreview};
use crate::state::LogsState;

pub use self::builtin::builtins;

pub const COLLECTION: &str = "logs_rules";

/// The most lines a threshold may ask for. Each key keeps that many line
/// times while it counts, and a rule may hold ten thousand keys.
pub const MAX_THRESHOLD_COUNT: u32 = 1_000;
/// The longest window a threshold may count over (a day).
pub const MAX_WINDOW_S: u32 = 86_400;
/// The longest quiet period before an alert is cleared (thirty days).
pub const MAX_CLEAR_AFTER_S: u32 = 30 * 86_400;
/// How often what is due on the wall clock is looked at: an alert clears
/// within this long of its quiet period ending.
pub const SWEEP_INTERVAL_MS: u64 = 2_000;
/// The longest instance a rule keys on, in characters. An instance is a
/// port, a neighbour, an address; what is longer is not one, and a rule
/// holds ten thousand of them.
pub const MAX_INSTANCE_CHARS: usize = 200;
/// The longest title a signal carries. A title goes to notification
/// channels, and a pattern's capture can be most of a 64 KiB line.
pub const MAX_TITLE_CHARS: usize = 300;
/// The longest value of one capture a signal carries — and so the longest
/// `detail` handed to `alerts.raise`, and the longest `by` in
/// `logs.config_changed`. The line itself is in the store, whole; a signal
/// is published on the event bus, which keeps its ring resident, and a
/// greedy capture can be most of a 64 KiB line.
pub const MAX_DETAIL_CHARS: usize = 2_000;
/// How many calls to the monitor may wait to be made.
pub const MAX_WAITING_CALLS: usize = 4_096;

/// A rule with its patterns compiled.
pub struct Compiled {
    pub rule: LogRule,
    pub pattern: Regex,
    /// The rule's opposite event, when it has one.
    pub clear: Option<Regex>,
}

/// The line a rule sees: what the device actually wrote.
fn line_of(entry: &LogEntry) -> &str {
    entry
        .body
        .as_ref()
        .and_then(|body| body.get("raw"))
        .and_then(Value::as_str)
        .unwrap_or(&entry.message)
}

/// Write the built-in rules, keeping what the user changed on them.
pub fn ensure_builtins(app: &App) -> AppResult<()> {
    let now = now_ms();
    for mut builtin in builtins() {
        let stored: Option<LogRule> = app.store().get(COLLECTION, &builtin.id)?;
        // A built-in's definition belongs to the application. Its switch,
        // its scope and its throttle belong to the user — they are what
        // `logs.rules.save` lets them change — and survive a restart and
        // an upgrade.
        if let Some(stored) = &stored {
            builtin.enabled = stored.enabled;
            builtin.scope = stored.scope.clone();
            builtin.throttle_s = stored.throttle_s;
            builtin.updated_at = stored.updated_at;
        }
        // Written only when the definition is new or has changed, so that
        // `updated_at` says when that was and not when the application
        // last started.
        if stored.as_ref() != Some(&builtin) {
            builtin.updated_at = now;
            app.store().put(COLLECTION, &builtin.id, &builtin)?;
        }
    }
    invalidate(app);
    Ok(())
}

pub fn invalidate(app: &App) {
    *app.state::<LogsState>().rules.write() = None;
}

/// Every enabled rule, compiled. A rule whose pattern does not compile is
/// skipped and complained about rather than taking the receiver down.
pub fn compiled(app: &App) -> AppResult<Arc<Vec<Compiled>>> {
    let state = app.state::<LogsState>();
    if let Some(rules) = state.rules.read().clone() {
        return Ok(rules);
    }
    let mut rules: Vec<LogRule> = app.store().list(COLLECTION)?;
    rules.sort_by(|left, right| left.id.cmp(&right.id));
    let compiled: Vec<Compiled> = rules
        .into_iter()
        .filter(|rule| rule.enabled)
        .filter_map(|rule| {
            let clear = rule
                .clear_pattern
                .as_deref()
                .filter(|pattern| !pattern.is_empty());
            match (Regex::new(&rule.pattern), clear.map(Regex::new).transpose()) {
                (Ok(pattern), Ok(clear)) => Some(Compiled {
                    rule,
                    pattern,
                    clear,
                }),
                (Err(err), _) | (_, Err(err)) => {
                    tracing::warn!(rule = rule.id, %err, "a log rule's pattern does not compile");
                    None
                }
            }
        })
        .collect();
    let compiled = Arc::new(compiled);
    *state.rules.write() = Some(compiled.clone());
    Ok(compiled)
}

fn in_scope(scope: &Scope, index: &Index, device_id: Option<&str>) -> bool {
    let empty = scope.device_ids.is_empty()
        && scope.sites.is_empty()
        && scope.roles.is_empty()
        && scope.tags.is_empty()
        && scope.profiles.is_empty();
    if empty {
        return true;
    }
    let Some(device) = device_id.and_then(|id| index.device(id)) else {
        // A scoped rule cannot say anything about a line it cannot place.
        return false;
    };
    let any =
        |list: &[String], value: &str| list.iter().any(|wanted| wanted.eq_ignore_ascii_case(value));
    any(&scope.device_ids, &device.id)
        || any(&scope.sites, &device.site)
        || any(&scope.roles, &device.role)
        || any(&scope.profiles, &device.profile)
        || device.tags.iter().any(|tag| any(&scope.tags, tag))
}

/// `{group}` → what the pattern captured.
fn render(template: &str, groups: &BTreeMap<String, String>) -> String {
    let mut out = template.to_string();
    for (name, value) in groups {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}

pub(crate) fn groups_of(pattern: &Regex, line: &str) -> Option<BTreeMap<String, String>> {
    let captures = pattern.captures(line)?;
    let mut groups = BTreeMap::new();
    for name in pattern.capture_names().flatten() {
        if let Some(value) = captures.name(name) {
            groups.insert(name.to_string(), value.as_str().to_string());
        }
    }
    Some(groups)
}

fn throttle_ms(rule: &LogRule) -> i64 {
    i64::from(rule.throttle_s.max(1)) * 1_000
}

/// The capture a rule's interface is in.
fn interface_group(rule: &LogRule) -> &str {
    match &rule.emit {
        Signal::InterfaceDown { interface_group } | Signal::InterfaceUp { interface_group } => {
            interface_group
        }
        _ => "interface",
    }
}

/// Text a device wrote, as it may become a key, a title or an alert's
/// detail: at most `max` characters, and none that moves a cursor, starts
/// an escape sequence or turns the reading direction around. What is cut
/// ends in `…`, so two values that differ only past the cut are one — which
/// is the point: the cut is what bounds a key.
pub(crate) fn plain(text: &str, max: usize) -> String {
    // Control characters, and the marks, embeddings, overrides and
    // isolates that change the direction text is read in.
    let hidden = |ch: char| {
        ch.is_control()
            || matches!(
                ch,
                '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
            )
    };
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index == max {
            out.pop();
            out.push('…');
            break;
        }
        out.push(if hidden(ch) { ' ' } else { ch });
    }
    out
}

/// The interface a line is about: resolved against the device's own
/// interfaces when they are known, otherwise what the pattern captured.
fn interface_of(
    rule: &LogRule,
    groups: &BTreeMap<String, String>,
    resolved: Option<&String>,
) -> Option<String> {
    resolved
        .or_else(|| groups.get(interface_group(rule)))
        .map(|interface| plain(interface, MAX_INSTANCE_CHARS))
}

/// What a rule counts, throttles and alerts on separately within one
/// device: the values of the threshold's `group_by` captures, or else the
/// interface. Empty means the device as a whole.
fn instance_of(
    rule: &LogRule,
    groups: &BTreeMap<String, String>,
    interface: Option<&str>,
) -> String {
    let group_by = rule
        .threshold
        .as_ref()
        .map(|threshold| threshold.group_by.as_slice())
        .unwrap_or_default();
    let written = if group_by.is_empty() {
        interface.unwrap_or_default().to_string()
    } else {
        group_by
            .iter()
            .map(|name| match interface {
                // Keyed the way the signal reports it, so that `Gi1/0/3` in
                // one line and `GigabitEthernet1/0/3` in the next are one
                // port.
                Some(interface) if name == interface_group(rule) => interface,
                _ => groups.get(name).map(String::as_str).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    // Trimmed because the instance ends the dedup key and the monitor trims
    // the key it is given: an instance with a space at its end would be
    // raised under one key and cleared under another.
    plain(&written, MAX_INSTANCE_CHARS).trim().to_string()
}

/// What a signal is called: the alert's title with the pattern's captures
/// filled in, or the rule's name and what it signals.
fn title_of(rule: &LogRule, groups: &BTreeMap<String, String>) -> (Option<String>, String) {
    let (severity, title) = match &rule.emit {
        Signal::Alert { severity, title } => (Some(severity.clone()), render(title, groups)),
        other => (None, format!("{}: {}", rule.name, other.kind())),
    };
    let title = plain(&title, MAX_TITLE_CHARS);
    if title.trim().is_empty() {
        // A title that is nothing but captures, on a line where they
        // captured nothing: the monitor refuses an alert without one.
        return (severity, plain(&rule.name, MAX_TITLE_CHARS));
    }
    (severity, title)
}

/// What makes two sightings the same alert, and what clears it again.
fn dedup_key(rule_id: &str, device_id: Option<&str>, instance: &str) -> String {
    format!(
        "logs:{rule_id}:{}:{instance}",
        device_id.unwrap_or_default()
    )
}

/// Everything one match wants to say.
struct Raised {
    rule_id: String,
    rule_name: String,
    kind: &'static str,
    severity: Option<String>,
    title: String,
    interface: Option<String>,
    instance: String,
    /// What the pattern captured, each value cut to [`MAX_DETAIL_CHARS`].
    groups: BTreeMap<String, String>,
    device_id: Option<String>,
    /// The device is simulated: its alert is recorded and notifies nobody.
    lab: bool,
    entry_id: i64,
    at: i64,
    count: u64,
}

/// An alert that is over.
struct Cleared {
    rule_id: String,
    rule_name: String,
    dedup_key: String,
    title: String,
    device_id: Option<String>,
    instance: String,
    reason: Reason,
    /// What `alerts.clear` is told, for the alert's own record.
    note: String,
    /// The line that said so, when a line did.
    entry_id: Option<i64>,
    at: i64,
    /// This process raised the alert and knows it was open. When it does
    /// not (the alert may predate a restart) the monitor is asked first and
    /// the clearing is only recorded if there was something to clear.
    known: bool,
}

impl Cleared {
    fn of(open: OpenAlert, reason: Reason, entry_id: Option<i64>, at: i64) -> Cleared {
        let note = match reason {
            Reason::Opposite => "the opposite event arrived".to_string(),
            Reason::Quiet => format!(
                "no matching line for {} s",
                open.clear_after_ms.unwrap_or_default() / 1_000
            ),
        };
        Cleared {
            rule_id: open.rule_id,
            rule_name: open.rule_name,
            dedup_key: open.dedup_key,
            title: open.title,
            device_id: open.device_id,
            instance: open.instance,
            reason,
            note,
            entry_id,
            at,
            known: true,
        }
    }
}

enum Said {
    Raised(Raised),
    Cleared(Cleared),
}

/// What the monitor is to be told about an alert.
enum Handoff {
    /// `alerts.raise` with these parameters.
    Raise(Value),
    /// `alerts.clear` for an alert that is over.
    Clear(Cleared),
}

/// The calls to the monitoring feature, in the order the rules decided
/// them, made one after another by one task ([`courier`]).
///
/// A task per call would let them overtake each other: a neighbour that
/// comes back and is lost again a moment later is a clear and then a raise,
/// and made the other way round they leave the alert closed on a device
/// that ended up down. And the queue is bounded, like the one in front of
/// the writer: when the monitor is this far behind, a call is dropped and
/// counted (`rules.undelivered` in the status) rather than kept in a queue
/// that grows for as long as lines arrive. The signal itself is stored and
/// published before its call is queued, so what is lost is the alert's
/// hand-off, not the record that it happened.
pub struct Outbox {
    queue: Sender<Handoff>,
    waiting: Mutex<Option<Receiver<Handoff>>>,
    undelivered: AtomicU64,
}

impl Default for Outbox {
    fn default() -> Self {
        let (queue, waiting) = channel(MAX_WAITING_CALLS);
        Outbox {
            queue,
            waiting: Mutex::new(Some(waiting)),
            undelivered: AtomicU64::new(0),
        }
    }
}

impl Outbox {
    fn hand_over(&self, handoff: Handoff) {
        // Full, or nobody is making the calls any more (the application is
        // shutting down): either way this one is not going to be made.
        if self.queue.try_send(handoff).is_err() {
            self.undelivered.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Calls dropped since the process started.
    pub fn undelivered(&self) -> u64 {
        self.undelivered.load(Ordering::Relaxed)
    }
}

/// The task that makes the calls in an [`Outbox`], in order.
///
/// The monitoring feature may not be in this build; a feature that is not
/// there is not an error, and nothing waits for it.
pub async fn courier(app: App) {
    let waiting = app.state::<LogsState>().outbox.waiting.lock().take();
    let Some(mut waiting) = waiting else {
        return;
    };
    while let Some(handoff) = waiting.recv().await {
        match handoff {
            Handoff::Raise(params) => {
                if let Err(err) = app.rpc("alerts.raise", params).await
                    && err.code != "unknown_method"
                {
                    tracing::debug!(%err, "a log signal could not raise an alert");
                }
            }
            Handoff::Clear(cleared) => tell_cleared(&app, cleared).await,
        }
    }
}

/// `alerts.clear` for one alert that is over.
async fn tell_cleared(app: &App, cleared: Cleared) {
    let params = json!({"dedup_key": cleared.dedup_key, "note": cleared.note});
    match app.rpc("alerts.clear", params).await {
        // Nothing here knew of an open alert, the monitor did: say so now.
        // Had it known of none either, there was nothing to say.
        Ok(result) if !cleared.known && result["cleared"] == json!(true) => {
            record_cleared(app, &cleared);
        }
        Ok(_) => {}
        Err(err) if err.code == "unknown_method" => {}
        Err(err) => tracing::debug!(%err, "a log rule could not clear its alert"),
    }
}

/// Run every rule over a written batch and publish what they say.
pub fn evaluate(app: &App, index: &Index, batch: &[Prepared]) {
    let rules = match compiled(app) {
        Ok(rules) if !rules.is_empty() => rules,
        Ok(_) => return,
        Err(err) => {
            tracing::error!(%err, "the log rules could not be read");
            return;
        }
    };
    let state = app.state::<LogsState>();
    let wall = now_ms();
    let mut said: Vec<Said> = Vec::new();
    {
        // One lock for the batch: nothing below waits on anything.
        let mut memory = state.memory.lock();
        for item in batch {
            let line = line_of(&item.entry);
            let device_id = item.entry.device_id.as_deref();
            for compiled in rules.iter() {
                let rule = &compiled.rule;
                if rule.stream != item.entry.stream {
                    continue;
                }
                if !in_scope(&rule.scope, index, device_id) {
                    continue;
                }
                let severe_enough = rule
                    .min_severity
                    .is_none_or(|min| item.entry.severity.is_some_and(|severity| severity <= min));
                let matched = severe_enough
                    .then(|| groups_of(&compiled.pattern, line))
                    .flatten();
                let Some(groups) = matched else {
                    // Not the rule's event. It may be its opposite, which
                    // is looked at whatever its severity: a recovery is
                    // routinely logged lower than the failure.
                    let Some(groups) = compiled
                        .clear
                        .as_ref()
                        .and_then(|clear| groups_of(clear, line))
                    else {
                        continue;
                    };
                    let interface = interface_of(rule, &groups, item.interface.as_ref());
                    let instance = instance_of(rule, &groups, interface.as_deref());
                    let key: Key = (device_id.unwrap_or_default().to_string(), instance);
                    match memory.opposite(
                        &rule.id,
                        &key,
                        throttle_ms(rule),
                        item.entry.id,
                        item.entry.ts,
                        wall,
                    ) {
                        Opposite::Cleared(open) => said.push(Said::Cleared(Cleared::of(
                            open,
                            Reason::Opposite,
                            Some(item.entry.id),
                            item.entry.ts,
                        ))),
                        Opposite::Unknown => said.push(Said::Cleared(Cleared {
                            rule_id: rule.id.clone(),
                            rule_name: rule.name.clone(),
                            dedup_key: dedup_key(&rule.id, device_id, &key.1),
                            title: rule.name.clone(),
                            device_id: item.entry.device_id.clone(),
                            instance: key.1,
                            reason: Reason::Opposite,
                            note: "the opposite event arrived".into(),
                            entry_id: Some(item.entry.id),
                            at: item.entry.ts,
                            known: false,
                        })),
                        Opposite::Held | Opposite::Nothing => {}
                    }
                    continue;
                };
                let interface = interface_of(rule, &groups, item.interface.as_ref());
                let instance = instance_of(rule, &groups, interface.as_deref());
                let key: Key = (device_id.unwrap_or_default().to_string(), instance);
                let ask = Ask {
                    rule_id: &rule.id,
                    threshold: rule
                        .threshold
                        .as_ref()
                        .map(|threshold| (threshold.count, i64::from(threshold.window_s) * 1_000)),
                    throttle_ms: throttle_ms(rule),
                    after: rule.after.as_deref(),
                };
                let Verdict::Fire { count } = memory.matched(&ask, &key, item.entry.ts, wall)
                else {
                    continue;
                };
                let (severity, title) = title_of(rule, &groups);
                if severity.is_some() {
                    memory.opened(
                        &key,
                        OpenAlert {
                            rule_id: rule.id.clone(),
                            rule_name: rule.name.clone(),
                            dedup_key: dedup_key(&rule.id, device_id, &key.1),
                            title: title.clone(),
                            device_id: item.entry.device_id.clone(),
                            instance: key.1.clone(),
                            last_match_wall: wall,
                            clear_after_ms: rule
                                .clear_after_s
                                .map(|seconds| i64::from(seconds) * 1_000),
                            hold_until_wall: wall + throttle_ms(rule),
                            recovered: None,
                            has_opposite: compiled.clear.is_some(),
                        },
                        wall,
                    );
                }
                said.push(Said::Raised(Raised {
                    rule_id: rule.id.clone(),
                    rule_name: rule.name.clone(),
                    kind: rule.emit.kind(),
                    severity,
                    title,
                    interface,
                    instance: key.1,
                    groups: groups
                        .into_iter()
                        .map(|(name, value)| (name, plain(&value, MAX_DETAIL_CHARS)))
                        .collect(),
                    device_id: item.entry.device_id.clone(),
                    lab: device_id
                        .and_then(|id| index.device(id))
                        .is_some_and(|device| device.lab),
                    entry_id: item.entry.id,
                    at: item.entry.ts,
                    count,
                }));
            }
        }
    }
    for item in said {
        match item {
            Said::Raised(raised) => publish(app, raised),
            Said::Cleared(cleared) => clear(app, cleared),
        }
    }
}

/// What is due because time passed rather than because a line arrived:
/// alerts quiet for their `clear_after_s`, and alerts whose opposite came
/// while they were held. `wall` is the wall clock, in milliseconds.
pub fn sweep(app: &App, wall: i64) {
    let due = app.state::<LogsState>().memory.lock().sweep(wall);
    for (open, reason) in due {
        let (entry_id, at) = match open.recovered {
            Some((entry_id, at)) => (Some(entry_id), at),
            None => (None, wall),
        };
        clear(app, Cleared::of(open, reason, entry_id, at));
    }
}

/// The task behind [`sweep`].
pub async fn sweeper(app: App) {
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(SWEEP_INTERVAL_MS)).await;
        sweep(&app, now_ms());
    }
}

/// What the rules hold in memory, for `logs.status`.
pub fn status(app: &App) -> RuleStatus {
    // Read and let go: nothing below needs the lock.
    let counts = app.state::<LogsState>().memory.lock().counts();
    RuleStatus {
        tracked: counts.tracked,
        forgotten: counts.forgotten,
        max_per_rule: track::MAX_KEYS_PER_RULE as u64,
        held_back: counts.held_back,
        open_alerts: counts.open,
        undelivered: app.state::<LogsState>().outbox.undelivered(),
    }
}

fn device_name(app: &App, device_id: Option<&str>) -> String {
    device_id
        .map(|id| crate::attribute::refresh(app, now_ms(), 30_000).map(|index| index.name_of(id)))
        .transpose()
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Keep a signal and tell whoever is listening. Kept as well as published:
/// a signal is the thing a correlation is anchored to afterwards, and an
/// event nobody was listening for is gone.
fn record(app: &App, kind: &str, device_id: Option<&str>, at: i64, message: &str, payload: &Value) {
    let entry = LogEntry {
        id: 0,
        ts: at,
        stream: "event".into(),
        device_id: device_id.map(str::to_string),
        severity: None,
        source: format!("logs.{kind}"),
        message: message.to_string(),
        body: Some(payload.clone()),
    };
    if let Err(err) = app.store().log_append(&entry) {
        tracing::error!(%err, "a log signal could not be stored");
    }
    app.publish("logs.signal", payload);
}

fn publish(app: &App, raised: Raised) {
    let payload = json!({
        "rule_id": raised.rule_id,
        "rule": raised.rule_name,
        "kind": raised.kind,
        "device_id": raised.device_id,
        "device": device_name(app, raised.device_id.as_deref()),
        "severity": raised.severity,
        "title": raised.title,
        "interface": raised.interface,
        "instance": raised.instance,
        "groups": raised.groups,
        "entry_id": raised.entry_id,
        "count": raised.count,
        "at": raised.at,
    });
    record(
        app,
        raised.kind,
        raised.device_id.as_deref(),
        raised.at,
        &raised.title,
        &payload,
    );
    if raised.kind == "config_changed" {
        app.publish(
            "logs.config_changed",
            &json!({
                "device_id": raised.device_id,
                "by": raised.groups.get("by"),
                "from": raised.groups.get("from"),
                "entry_id": raised.entry_id,
                "at": raised.at,
            }),
        );
    }
    if let Some(severity) = raised.severity {
        let mut params = json!({
            "rule": raised.rule_name,
            "device_id": raised.device_id,
            "instance": (!raised.instance.is_empty()).then_some(&raised.instance),
            "severity": severity,
            "title": raised.title,
            "detail": raised.groups.get("detail"),
            "dedup_key": dedup_key(&raised.rule_id, raised.device_id.as_deref(), &raised.instance),
        });
        if raised.lab {
            // A lab is where things are broken on purpose. Its alert is
            // raised like any other — recorded, shown, cleared — and sent
            // to nobody: an empty list is "no channel", where leaving the
            // field out would be "the usual ones".
            params["notify"] = json!([]);
        }
        app.state::<LogsState>()
            .outbox
            .hand_over(Handoff::Raise(params));
    }
}

/// An alert is over: tell the monitor, and keep and publish that it is.
fn clear(app: &App, cleared: Cleared) {
    if cleared.known {
        record_cleared(app, &cleared);
    }
    app.state::<LogsState>()
        .outbox
        .hand_over(Handoff::Clear(cleared));
}

fn record_cleared(app: &App, cleared: &Cleared) {
    let payload = json!({
        "rule_id": cleared.rule_id,
        "rule": cleared.rule_name,
        "kind": "cleared",
        "device_id": cleared.device_id,
        "device": device_name(app, cleared.device_id.as_deref()),
        "severity": Value::Null,
        "title": cleared.title,
        "interface": Value::Null,
        "instance": cleared.instance,
        "groups": {},
        "entry_id": cleared.entry_id,
        "count": 0,
        "at": cleared.at,
        "reason": cleared.reason.as_str(),
    });
    record(
        app,
        "cleared",
        cleared.device_id.as_deref(),
        cleared.at,
        &format!("Cleared: {}", cleared.title),
        &payload,
    );
}

// ------------------------------------------------------------------- RPC

/// What a rule has to satisfy before it is stored. `other` looks another
/// rule up by id (the one named in `after`).
pub(crate) fn check(rule: &LogRule, other: &dyn Fn(&str) -> Option<LogRule>) -> AppResult<()> {
    if rule.name.trim().is_empty() {
        return Err(AppError::invalid("a rule needs a name"));
    }
    let pattern = Regex::new(&rule.pattern)
        .map_err(|err| AppError::invalid(format!("the pattern does not compile: {err}")))?;
    let raises_alert = match &rule.emit {
        Signal::Alert { severity, title } => {
            if !["info", "low", "medium", "high", "critical"].contains(&severity.as_str()) {
                return Err(AppError::invalid(format!(
                    "`{severity}` is not a severity (info, low, medium, high, critical)"
                )));
            }
            if title.trim().is_empty() {
                // The monitor refuses an alert with no title, and it would
                // refuse it where nobody sees: when a line matches.
                return Err(AppError::invalid(
                    "an alert needs a title (it may use the pattern's groups, as in `{interface} is down`)",
                ));
            }
            true
        }
        _ => false,
    };
    let captured: Vec<&str> = pattern.capture_names().flatten().collect();
    let group_by = rule
        .threshold
        .as_ref()
        .map(|threshold| threshold.group_by.as_slice())
        .unwrap_or_default();
    if let Some(threshold) = &rule.threshold {
        if !(1..=MAX_THRESHOLD_COUNT).contains(&threshold.count) {
            return Err(AppError::invalid(format!(
                "a threshold counts between 1 and {MAX_THRESHOLD_COUNT} lines, not {}",
                threshold.count
            )));
        }
        if !(1..=MAX_WINDOW_S).contains(&threshold.window_s) {
            return Err(AppError::invalid(format!(
                "a threshold's window is between 1 and {MAX_WINDOW_S} seconds, not {}",
                threshold.window_s
            )));
        }
        if let Some(missing) = group_by
            .iter()
            .find(|name| !captured.contains(&name.as_str()))
        {
            return Err(AppError::invalid(format!(
                "the pattern has no group named `{missing}` to count by"
            )));
        }
    }
    if let Some(clear) = &rule.clear_pattern {
        if !raises_alert {
            return Err(AppError::invalid(
                "only a rule that raises an alert has something to clear",
            ));
        }
        let clear = Regex::new(clear).map_err(|err| {
            AppError::invalid(format!("the clearing pattern does not compile: {err}"))
        })?;
        let captured: Vec<&str> = clear.capture_names().flatten().collect();
        if let Some(missing) = group_by
            .iter()
            .find(|name| !captured.contains(&name.as_str()))
        {
            return Err(AppError::invalid(format!(
                "the clearing pattern has to capture `{missing}` as well, or it cannot tell which alert to clear"
            )));
        }
    }
    if let Some(seconds) = rule.clear_after_s {
        if !raises_alert {
            return Err(AppError::invalid(
                "only a rule that raises an alert has something to clear",
            ));
        }
        if !(1..=MAX_CLEAR_AFTER_S).contains(&seconds) {
            return Err(AppError::invalid(format!(
                "an alert is cleared after between 1 and {MAX_CLEAR_AFTER_S} quiet seconds, not {seconds}"
            )));
        }
    }
    if let Some(after) = &rule.after {
        if *after == rule.id {
            return Err(AppError::invalid("a rule cannot follow itself"));
        }
        match other(after) {
            Some(other) if matches!(other.emit, Signal::Alert { .. }) => {}
            Some(other) => {
                return Err(AppError::invalid(format!(
                    "`{}` raises no alert, so there is nothing for another rule to follow",
                    other.name
                )));
            }
            None => {
                return Err(AppError::invalid(format!(
                    "there is no rule `{after}` to follow"
                )));
            }
        }
    }
    Ok(())
}

/// Check a rule against one line, without storing anything.
pub fn test(app: &App, rule: &LogRule, line: &str) -> AppResult<RuleTest> {
    let pattern = Regex::new(&rule.pattern)
        .map_err(|err| AppError::invalid(format!("the pattern does not compile: {err}")))?;
    let clear = rule
        .clear_pattern
        .as_deref()
        .filter(|pattern| !pattern.is_empty())
        .map(Regex::new)
        .transpose()
        .map_err(|err| {
            AppError::invalid(format!("the clearing pattern does not compile: {err}"))
        })?;
    let (severity, resolved_interface, parsed_view) = if rule.stream == crate::db::TRAP_STREAM {
        // A trap rule reads the message the receiver wrote for the
        // notification. There is no syslog header in it to understand: it
        // has no severity, and its first word — the notification's name —
        // is what the row is indexed by.
        let name = line.split_whitespace().next();
        let view = json!({
            "severity": Value::Null,
            "facility": Value::Null,
            "mnemonic": name,
            "host": Value::Null,
            "message": line,
        });
        (None, None, view)
    } else {
        let source = "0.0.0.0:0".parse().expect("a valid socket address");
        let parsed = nocturne_netprobe::syslog::parse(line.as_bytes(), source);
        let state = app.state::<LogsState>();
        let index = crate::attribute::refresh(app, now_ms(), 30_000)?;
        let prepared = crate::ingest::prepare(&state, &index, &parsed, now_ms(), "test", None);
        let view = json!({
            "severity": prepared.entry.severity,
            "facility": prepared.facility,
            "mnemonic": prepared.mnemonic,
            "host": prepared.host,
            "message": prepared.entry.message,
        });
        (prepared.entry.severity, prepared.interface, view)
    };
    let unmatched = |reason: String| RuleTest {
        matched: false,
        reason,
        groups: BTreeMap::new(),
        signal: None,
        parsed: Some(parsed_view.clone()),
        instance: String::new(),
        note: String::new(),
        clears: false,
    };
    let severe_enough = rule
        .min_severity
        .is_none_or(|min| severity.is_some_and(|severity| severity <= min));
    let matched = severe_enough.then(|| groups_of(&pattern, line)).flatten();
    let Some(groups) = matched else {
        if let Some(groups) = clear.as_ref().and_then(|clear| groups_of(clear, line)) {
            let interface = interface_of(rule, &groups, resolved_interface.as_ref());
            let instance = instance_of(rule, &groups, interface.as_deref());
            return Ok(RuleTest {
                reason: "this line is the rule's opposite event: it raises nothing and clears the alert the rule has open for this instance".into(),
                groups,
                instance,
                clears: true,
                ..unmatched(String::new())
            });
        }
        if !severe_enough {
            let min = rule.min_severity.unwrap_or_default();
            return Ok(unmatched(format!(
                "the rule only looks at lines of severity {min} or worse; this one is {}",
                severity
                    .map(|severity| severity.to_string())
                    .unwrap_or_else(|| "unknown".into())
            )));
        }
        return Ok(unmatched("the pattern does not match this line".into()));
    };
    let (severity_name, title) = title_of(rule, &groups);
    let interface = interface_of(rule, &groups, resolved_interface.as_ref());
    let instance = instance_of(rule, &groups, interface.as_deref());
    // What else has to be true before the signal is raised: a pasted line
    // that matches must not read as "this line raises the alert" when the
    // rule wants eight of them.
    let mut note = Vec::new();
    if let Some(threshold) = rule
        .threshold
        .as_ref()
        .filter(|threshold| threshold.count > 1)
    {
        let per = if threshold.group_by.is_empty() {
            String::new()
        } else {
            format!(" and the same {}", threshold.group_by.join(", "))
        };
        note.push(format!(
            "One such line raises nothing: the signal is raised when {} of them arrive inside {} s for the same device{per}. The count is kept in memory and starts again when Nocturne restarts.",
            threshold.count, threshold.window_s
        ));
    }
    if let Some(after) = &rule.after {
        let name = app
            .store()
            .get::<LogRule>(COLLECTION, after)?
            .map(|other| other.name)
            .unwrap_or_else(|| after.clone());
        note.push(format!(
            "It only counts while `{name}` has an alert open for the same device and instance."
        ));
    }
    Ok(RuleTest {
        matched: true,
        reason: String::new(),
        groups,
        signal: Some(SignalPreview {
            kind: rule.emit.kind().to_string(),
            title,
            severity: severity_name,
            interface: match &rule.emit {
                Signal::InterfaceDown { .. } | Signal::InterfaceUp { .. } => interface,
                _ => None,
            },
        }),
        parsed: Some(parsed_view),
        instance,
        note: note.join(" "),
        clears: false,
    })
}

pub fn list(app: &App) -> AppResult<Vec<LogRule>> {
    let mut rules: Vec<LogRule> = app.store().list(COLLECTION)?;
    rules.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(rules)
}

pub fn save(app: &App, mut rule: LogRule) -> AppResult<LogRule> {
    let existing: Option<LogRule> = if rule.id.is_empty() {
        None
    } else {
        app.store().get(COLLECTION, &rule.id)?
    };
    if let Some(existing) = &existing
        && existing.builtin
    {
        // A built-in's definition is the application's; only its switch,
        // its scope and its throttle are the user's, and a deleted one
        // would come back anyway.
        let changed = LogRule {
            enabled: rule.enabled,
            scope: rule.scope.clone(),
            throttle_s: rule.throttle_s,
            ..existing.clone()
        };
        rule = changed;
    } else {
        rule.builtin = false;
        rule.platform = rule.platform.trim().to_string();
        // An empty field from a form means "none", not "a pattern that
        // matches every line".
        rule.clear_pattern = rule
            .clear_pattern
            .filter(|pattern| !pattern.trim().is_empty());
        rule.after = rule.after.filter(|after| !after.trim().is_empty());
        check(&rule, &|id| {
            app.store().get::<LogRule>(COLLECTION, id).ok().flatten()
        })?;
        if rule.id.is_empty() {
            rule.id = nocturne_app::util::new_id();
        }
    }
    rule.updated_at = now_ms();
    app.store().put(COLLECTION, &rule.id, &rule)?;
    invalidate(app);
    if existing
        .as_ref()
        .is_some_and(|before| !still_watching(before, &rule))
    {
        // What it counted, it counted as the rule it was, and an alert it
        // had open would otherwise be "cleared after a quiet period" by a
        // rule that had merely stopped looking. As after a delete, the
        // alerts stay as they are in the alert list.
        app.state::<LogsState>().memory.lock().forget_rule(&rule.id);
    }
    app.audit(
        "logs.rule.save",
        None,
        format!("Saved log rule `{}`", rule.name),
        json!({"rule_id": rule.id, "enabled": rule.enabled}),
    );
    Ok(rule)
}

/// Whether a saved rule still watches what it watched before, so that what
/// it remembers — counts, throttle windows, the alerts it has open — still
/// means something. Its name and its throttle may change; anything that
/// decides which lines it sees, how it counts them or how its alert ends
/// may not, and neither may it be switched off.
fn still_watching(before: &LogRule, after: &LogRule) -> bool {
    after.enabled
        && before.stream == after.stream
        && before.pattern == after.pattern
        && before.min_severity == after.min_severity
        && before.scope == after.scope
        && before.threshold == after.threshold
        && before.after == after.after
        && before.clear_pattern == after.clear_pattern
        && before.clear_after_s == after.clear_after_s
        && before.emit == after.emit
}

pub fn delete(app: &App, id: &str) -> AppResult<()> {
    let rule: LogRule = app.store().require(COLLECTION, id, "log rule")?;
    if rule.builtin {
        return Err(AppError::forbidden(format!(
            "`{}` ships with Nocturne and cannot be deleted; switch it off instead",
            rule.name
        )));
    }
    app.store().delete(COLLECTION, id)?;
    invalidate(app);
    // What it was counting goes with it. An alert it raised stays as it is
    // in the alert list: nothing is left that could say the condition ended.
    app.state::<LogsState>().memory.lock().forget_rule(id);
    app.audit(
        "logs.rule.delete",
        None,
        format!("Deleted log rule `{}`", rule.name),
        json!({"rule_id": id}),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Threshold;

    fn groups(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_title_is_rendered_from_what_the_pattern_captured() {
        let groups = groups(&[("interface", "Gi1/0/7"), ("reason", "bpduguard")]);
        assert_eq!(
            render("{interface} error-disabled ({reason})", &groups),
            "Gi1/0/7 error-disabled (bpduguard)"
        );
        assert_eq!(render("nothing to fill", &groups), "nothing to fill");
    }

    #[test]
    fn the_instance_is_the_group_key_or_else_the_interface() {
        let captured = groups(&[("source", "10.0.0.5"), ("user", "admin")]);
        let plain = LogRule::default();
        assert_eq!(instance_of(&plain, &captured, Some("Gi1/0/3")), "Gi1/0/3");
        assert_eq!(
            instance_of(&plain, &captured, None),
            "",
            "no interface and no group: the device as a whole"
        );

        let grouped = LogRule {
            threshold: Some(Threshold {
                count: 8,
                window_s: 60,
                group_by: vec!["source".into()],
            }),
            ..LogRule::default()
        };
        assert_eq!(
            instance_of(&grouped, &captured, Some("Gi1/0/3")),
            "10.0.0.5",
            "a rule that counts by address is not keyed by a port"
        );

        let by_port = LogRule {
            threshold: Some(Threshold {
                count: 5,
                window_s: 300,
                group_by: vec!["interface".into()],
            }),
            ..LogRule::default()
        };
        assert_eq!(
            instance_of(
                &by_port,
                &groups(&[("interface", "Gi1/0/3")]),
                Some("GigabitEthernet1/0/3")
            ),
            "GigabitEthernet1/0/3",
            "the port is keyed as the device spells it, however the line did"
        );
    }

    #[test]
    fn text_from_a_device_is_cut_and_loses_what_is_not_text() {
        assert_eq!(plain("GigabitEthernet1/0/3", 200), "GigabitEthernet1/0/3");
        assert_eq!(
            plain("exactly", 7),
            "exactly",
            "what fits is not marked as cut"
        );
        assert_eq!(plain("too long", 7), "too lo…");
        assert_eq!(plain("too long", 7).chars().count(), 7);
        // Counted in characters, cut between them.
        assert_eq!(plain("ääääää", 4), "äää…");
        assert_eq!(
            plain("a\u{1b}[2Jb\nc\u{202e}d\u{7f}e", 50),
            "a [2Jb c d e",
            "an escape sequence, a line break and a direction override are spaces"
        );
        assert_eq!(plain("", 5), "");
    }

    #[test]
    fn a_key_is_bounded_however_much_the_pattern_captured() {
        let grouped = LogRule {
            threshold: Some(Threshold {
                count: 8,
                window_s: 60,
                group_by: vec!["source".into(), "user".into()],
            }),
            ..LogRule::default()
        };
        let long = "9".repeat(60_000);
        let captured = groups(&[("source", long.as_str()), ("user", long.as_str())]);
        let instance = instance_of(&grouped, &captured, None);
        assert_eq!(instance.chars().count(), MAX_INSTANCE_CHARS);
        // The same line gives the same key again, which is all a key is for.
        assert_eq!(instance, instance_of(&grouped, &captured, None));

        // The monitor trims the key it is given, so the instance that ends
        // it has no space at either end to lose.
        let padded = groups(&[("source", "\t10.0.0.5"), ("user", " ")]);
        assert_eq!(instance_of(&grouped, &padded, None), "10.0.0.5");

        // A title made of captures that captured nothing falls back to the
        // rule's name rather than going out empty.
        let titled = LogRule {
            name: "Fan".into(),
            emit: Signal::Alert {
                severity: "high".into(),
                title: "{what}".into(),
            },
            ..LogRule::default()
        };
        assert_eq!(title_of(&titled, &groups(&[("what", "")])).1, "Fan");
        assert_eq!(
            title_of(&titled, &groups(&[("what", "2 failed")])).1,
            "2 failed"
        );

        let by_port = LogRule::default();
        let interface = interface_of(&by_port, &groups(&[("interface", long.as_str())]), None);
        assert_eq!(
            interface.as_deref().map(|name| name.chars().count()),
            Some(MAX_INSTANCE_CHARS)
        );
    }

    #[test]
    fn a_rule_keeps_what_it_remembers_only_while_it_watches_the_same_thing() {
        let before = LogRule {
            name: "r".into(),
            pattern: "x (?P<what>\\S+)".into(),
            clear_after_s: Some(600),
            ..LogRule::default()
        };
        let renamed = LogRule {
            name: "another name".into(),
            throttle_s: 5,
            ..before.clone()
        };
        assert!(still_watching(&before, &renamed));

        let changes: Vec<(&str, LogRule)> = vec![
            (
                "switched off",
                LogRule {
                    enabled: false,
                    ..before.clone()
                },
            ),
            (
                "another pattern",
                LogRule {
                    pattern: "y".into(),
                    ..before.clone()
                },
            ),
            (
                "another quiet period",
                LogRule {
                    clear_after_s: None,
                    ..before.clone()
                },
            ),
            (
                "an opposite",
                LogRule {
                    clear_pattern: Some("z".into()),
                    ..before.clone()
                },
            ),
            (
                "a severity floor",
                LogRule {
                    min_severity: Some(3),
                    ..before.clone()
                },
            ),
            (
                "a narrower scope",
                LogRule {
                    scope: Scope {
                        sites: vec!["HQ".into()],
                        ..Scope::default()
                    },
                    ..before.clone()
                },
            ),
            (
                "a threshold",
                LogRule {
                    threshold: Some(Threshold {
                        count: 3,
                        window_s: 60,
                        group_by: vec![],
                    }),
                    ..before.clone()
                },
            ),
            (
                "another signal",
                LogRule {
                    emit: Signal::Reload,
                    ..before.clone()
                },
            ),
        ];
        for (what, after) in changes {
            assert!(!still_watching(&before, &after), "{what}");
        }
    }

    #[test]
    fn a_rule_that_cannot_work_is_refused_with_the_reason() {
        let nobody = |_: &str| None;
        let alerting = |pattern: &str| LogRule {
            name: "r".into(),
            pattern: pattern.into(),
            emit: Signal::Alert {
                severity: "medium".into(),
                title: "Something happened".into(),
            },
            ..LogRule::default()
        };
        let refused = |rule: LogRule| check(&rule, &nobody).unwrap_err().message;

        let mut untitled = alerting("x");
        untitled.emit = Signal::Alert {
            severity: "medium".into(),
            title: "  ".into(),
        };
        assert!(refused(untitled).contains("needs a title"));

        let mut rule = alerting(r"from (?P<source>\S+)");
        rule.threshold = Some(Threshold {
            count: 8,
            window_s: 60,
            group_by: vec!["address".into()],
        });
        assert!(refused(rule.clone()).contains("no group named `address`"));

        rule.threshold.as_mut().unwrap().group_by = vec!["source".into()];
        assert!(check(&rule, &nobody).is_ok());

        rule.threshold.as_mut().unwrap().count = 0;
        assert!(refused(rule.clone()).contains("between 1 and"));
        rule.threshold.as_mut().unwrap().count = 8;

        // An opposite that does not say which address it is about could
        // only ever clear the wrong alert.
        rule.clear_pattern = Some("all clear".into());
        assert!(refused(rule.clone()).contains("has to capture `source`"));
        rule.clear_pattern = Some(r"cleared (?P<source>\S+)".into());
        assert!(check(&rule, &nobody).is_ok());

        let mut silent = alerting("x");
        silent.emit = Signal::Reload;
        silent.clear_after_s = Some(60);
        assert!(refused(silent).contains("raises an alert"));

        let mut follower = alerting("x");
        follower.after = Some("missing".into());
        assert!(refused(follower).contains("no rule `missing`"));
    }
}
