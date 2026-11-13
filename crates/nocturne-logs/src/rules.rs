//! Pattern rules: turning lines into signals the rest of the application
//! reacts to.
//!
//! A rule is a regular expression over the line **as the device wrote it**
//! (mnemonic included, which is what vendor documentation shows and what a
//! user pastes into `logs.rules.test`), a scope, and one signal. Matching a
//! line is not interesting by itself; what matters is that the configuration
//! feature backs a switch up when it hears `logs.config_changed`, and that a
//! port flapping four hundred times raises one alert and a count.
//!
//! Throttling is per rule and per device. The first match of a window is
//! published at once — a NOC wants to hear about the first one immediately —
//! and the rest are counted; the next signal after the window carries that
//! count, so nothing is lost, only compressed.

use std::collections::BTreeMap;
use std::sync::Arc;

use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use regex::Regex;
use serde_json::{Value, json};

use crate::attribute::Index;
use crate::db::Prepared;
use crate::model::{LogRule, RuleTest, Scope, Signal, SignalPreview};
use crate::state::{LogsState, Throttle};

pub const COLLECTION: &str = "logs_rules";

/// A rule with its pattern compiled.
pub struct Compiled {
    pub rule: LogRule,
    pub pattern: Regex,
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

fn rule(
    id: &str,
    name: &str,
    pattern: &str,
    emit: Signal,
    test_line: &str,
    source: &str,
) -> LogRule {
    LogRule {
        id: id.to_string(),
        name: name.to_string(),
        builtin: true,
        pattern: pattern.to_string(),
        emit,
        test_line: test_line.to_string(),
        test_line_source: source.to_string(),
        ..LogRule::default()
    }
}

fn alert(severity: &str, title: &str) -> Signal {
    Signal::Alert {
        severity: severity.to_string(),
        title: title.to_string(),
    }
}

const IOS_DOC: &str = "Cisco IOS / IOS-XE system message documentation";
const ROS_DOC: &str = "MikroTik RouterOS logging documentation";

/// The rules that ship with the application.
///
/// Their scope is deliberately empty: the pattern itself names the vendor's
/// mnemonic, so a rule written for IOS cannot match a RouterOS line, and
/// scoping by profile would only stop the rule working on a device whose
/// profile has not been detected yet.
pub fn builtins() -> Vec<LogRule> {
    vec![
        rule(
            "builtin.ios.config_changed",
            "IOS: configuration changed",
            r"%SYS-5-CONFIG_I: Configured from (?P<from>\S+)(?: by (?P<by>\S+))?",
            Signal::ConfigChanged,
            "*Mar  1 00:12:02.123: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.link_down",
            "IOS: interface down",
            r"%LINK-3-UPDOWN: Interface (?P<interface>[\w./:-]+), changed state to (?:administratively )?down",
            Signal::InterfaceDown {
                interface_group: "interface".into(),
            },
            "*Mar  1 00:12:10.001: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.link_up",
            "IOS: interface up",
            r"%LINK-3-UPDOWN: Interface (?P<interface>[\w./:-]+), changed state to up",
            Signal::InterfaceUp {
                interface_group: "interface".into(),
            },
            "*Mar  1 00:12:14.001: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to up",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.lineproto_down",
            "IOS: line protocol down",
            r"%LINEPROTO-5-UPDOWN: Line protocol on Interface (?P<interface>[\w./:-]+), changed state to down",
            Signal::InterfaceDown {
                interface_group: "interface".into(),
            },
            "*Mar  1 00:12:11.002: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to down",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.lineproto_up",
            "IOS: line protocol up",
            r"%LINEPROTO-5-UPDOWN: Line protocol on Interface (?P<interface>[\w./:-]+), changed state to up",
            Signal::InterfaceUp {
                interface_group: "interface".into(),
            },
            "*Mar  1 00:12:15.002: %LINEPROTO-5-UPDOWN: Line protocol on Interface GigabitEthernet1/0/3, changed state to up",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.login_failed",
            "IOS: login failure",
            r"%SEC_LOGIN-4-LOGIN_FAILED: Login failed \[user: (?P<user>[^\]]*)\] \[Source: (?P<source>[^\]]*)\]",
            Signal::LoginFailure,
            "*Mar  1 00:13:01.000: %SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: admin] [Source: 10.0.0.5] [localport: 22] [Reason: Login Authentication Failed] at 00:13:01 UTC Mon Mar 1 2027",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.restart",
            "IOS: device restarted",
            r"%SYS-5-RESTART: System restarted",
            Signal::Reload,
            "*Mar  1 00:00:07.123: %SYS-5-RESTART: System restarted --\nCisco IOS Software, C2960X Software",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.spantree",
            "IOS: spanning tree event",
            r"%SPANTREE-2-(?P<event>[A-Z_0-9]+): (?P<detail>.*)",
            alert("high", "Spanning tree: {event}"),
            "*Mar  1 00:14:00.000: %SPANTREE-2-ROOTGUARD_BLOCK: Root guard blocking port GigabitEthernet1/0/5 on VLAN0010.",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.native_vlan_mismatch",
            "IOS: native VLAN mismatch",
            r"%CDP-4-NATIVE_VLAN_MISMATCH: Native VLAN mismatch discovered on (?P<interface>[\w./:-]+) \((?P<local>\d+)\), with (?P<neighbor>\S+) (?P<remote_interface>[\w./:-]+) \((?P<remote>\d+)\)",
            alert(
                "medium",
                "Native VLAN mismatch on {interface}: {local} vs {remote}",
            ),
            "*Mar  1 00:15:00.000: %CDP-4-NATIVE_VLAN_MISMATCH: Native VLAN mismatch discovered on GigabitEthernet1/0/2 (1), with SW2 GigabitEthernet1/0/2 (10).",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.duplex_mismatch",
            "IOS: duplex mismatch",
            r"%CDP-4-DUPLEX_MISMATCH: duplex mismatch discovered on (?P<interface>[\w./:-]+)",
            alert("medium", "Duplex mismatch on {interface}"),
            "*Mar  1 00:15:30.000: %CDP-4-DUPLEX_MISMATCH: duplex mismatch discovered on GigabitEthernet1/0/4 (not half duplex), with SW2 GigabitEthernet0/2 (half duplex).",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.err_disable",
            "IOS: port error-disabled",
            r"%PM-4-ERR_DISABLE: (?P<reason>[\w-]+) error detected on (?P<interface>[\w./:-]+), putting",
            alert("high", "{interface} error-disabled ({reason})"),
            "*Mar  1 00:16:00.000: %PM-4-ERR_DISABLE: bpduguard error detected on Gi1/0/7, putting Gi1/0/7 in err-disable state",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.eigrp_neighbor",
            "IOS: EIGRP neighbour change",
            r"%DUAL-5-NBRCHANGE:.*Neighbor (?P<neighbor>\S+) \((?P<interface>[^)]+)\) is (?P<state>up|down)",
            alert(
                "medium",
                "EIGRP neighbour {neighbor} is {state} on {interface}",
            ),
            "*Mar  1 00:17:00.000: %DUAL-5-NBRCHANGE: EIGRP-IPv4 100: Neighbor 10.0.0.2 (GigabitEthernet1/0/1) is down: holding time expired",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.ospf_adjacency",
            "IOS: OSPF adjacency change",
            r"%OSPF-5-ADJCHG: Process \S+, Nbr (?P<neighbor>\S+) on (?P<interface>[\w./:-]+) from (?P<from>\S+) to (?P<state>\S+)",
            alert(
                "medium",
                "OSPF neighbour {neighbor} on {interface} went {from} → {state}",
            ),
            "*Mar  1 00:18:00.000: %OSPF-5-ADJCHG: Process 1, Nbr 10.0.0.2 on GigabitEthernet1/0/1 from FULL to DOWN, Neighbor Down: Dead timer expired",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.bgp_adjacency",
            "IOS: BGP neighbour change",
            r"%BGP-5-ADJCHANGE: neighbor (?P<neighbor>\S+)(?: \S+)* (?P<state>Up|Down)",
            alert("high", "BGP neighbour {neighbor} is {state}"),
            "*Mar  1 00:19:00.000: %BGP-5-ADJCHANGE: neighbor 192.0.2.1 Down BGP Notification sent",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.mac_flap",
            "IOS: MAC address flapping",
            r"%SW_MATM-4-MACFLAP_NOTIF: Host (?P<mac>\S+) in vlan (?P<vlan>\d+) is flapping between port (?P<interface>[\w./:-]+) and port (?P<other>[\w./:-]+)",
            alert(
                "high",
                "{mac} is flapping between {interface} and {other} in VLAN {vlan}",
            ),
            "*Mar  1 00:20:00.000: %SW_MATM-4-MACFLAP_NOTIF: Host 0011.2233.4455 in vlan 10 is flapping between port Gi1/0/3 and port Gi1/0/9",
            IOS_DOC,
        ),
        rule(
            "builtin.ios.environment",
            "IOS: environment alarm",
            r"%(?:ENVMON|PLATFORM_ENV|CISCO_ENVMON|ENVIRONMENT)-[0-4]-(?P<event>[A-Z_0-9]+): (?P<detail>.*)",
            alert("critical", "Environment: {event}"),
            "*Mar  1 00:21:00.000: %PLATFORM_ENV-1-FAN_FAILED: Fan 2 not rotating.",
            IOS_DOC,
        ),
        rule(
            "builtin.routeros.config_changed",
            "RouterOS: configuration changed",
            r"system,info,?\S* .*changed by (?P<by>\S+)",
            Signal::ConfigChanged,
            "system,info,account ip address changed by admin",
            ROS_DOC,
        ),
        rule(
            "builtin.routeros.login_failed",
            "RouterOS: login failure",
            r"login failure for user (?P<user>\S+) from (?P<source>\S+) via (?P<service>\S+)",
            Signal::LoginFailure,
            "system,error,critical login failure for user admin from 10.0.0.5 via ssh",
            ROS_DOC,
        ),
        rule(
            "builtin.routeros.link_down",
            "RouterOS: interface down",
            r"interface,info (?P<interface>[\w.@/-]+) link down",
            Signal::InterfaceDown {
                interface_group: "interface".into(),
            },
            "interface,info ether3 link down",
            ROS_DOC,
        ),
        rule(
            "builtin.routeros.link_up",
            "RouterOS: interface up",
            r"interface,info (?P<interface>[\w.@/-]+) link up",
            Signal::InterfaceUp {
                interface_group: "interface".into(),
            },
            "interface,info ether3 link up (speed 1G, full duplex)",
            ROS_DOC,
        ),
        rule(
            "builtin.routeros.ospf_state",
            "RouterOS: OSPF neighbour state change",
            r"ospf\S*,?\S* .*(?:neighbor|neighbour) (?P<neighbor>\S+?):? state change from (?P<from>\w+) to (?P<state>\w+)",
            alert("medium", "OSPF neighbour {neighbor} went {from} → {state}"),
            "route,ospf,info OSPFv2 neighbor 10.0.0.2: state change from Full to Down",
            ROS_DOC,
        ),
    ]
}

/// Write the built-in rules, keeping whatever the user switched off.
pub fn ensure_builtins(app: &App) -> AppResult<()> {
    let now = now_ms();
    for mut builtin in builtins() {
        let stored: Option<LogRule> = app.store().get(COLLECTION, &builtin.id)?;
        // A built-in's definition belongs to the application; its on/off
        // switch belongs to the user, and survives an upgrade.
        builtin.enabled = stored.as_ref().map(|rule| rule.enabled).unwrap_or(true);
        builtin.updated_at = now;
        if stored.as_ref() != Some(&builtin) {
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
        .filter_map(|rule| match Regex::new(&rule.pattern) {
            Ok(pattern) => Some(Compiled { rule, pattern }),
            Err(err) => {
                tracing::warn!(rule = rule.id, %err, "a log rule's pattern does not compile");
                None
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

fn groups_of(pattern: &Regex, line: &str) -> Option<BTreeMap<String, String>> {
    let captures = pattern.captures(line)?;
    let mut groups = BTreeMap::new();
    for name in pattern.capture_names().flatten() {
        if let Some(value) = captures.name(name) {
            groups.insert(name.to_string(), value.as_str().to_string());
        }
    }
    Some(groups)
}

/// Everything one match wants to say.
struct Raised {
    rule_id: String,
    rule_name: String,
    kind: &'static str,
    severity: Option<String>,
    title: String,
    interface: Option<String>,
    groups: BTreeMap<String, String>,
    device_id: Option<String>,
    entry_id: i64,
    at: i64,
    count: u64,
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
    let mut raised: Vec<Raised> = Vec::new();
    for item in batch {
        let line = line_of(&item.entry);
        for compiled in rules.iter() {
            let rule = &compiled.rule;
            if rule.stream != item.entry.stream {
                continue;
            }
            if let Some(min) = rule.min_severity
                && !item.entry.severity.is_some_and(|severity| severity <= min)
            {
                continue;
            }
            if !in_scope(&rule.scope, index, item.entry.device_id.as_deref()) {
                continue;
            }
            let Some(groups) = groups_of(&compiled.pattern, line) else {
                continue;
            };
            // Throttle per rule and per device: a flapping port is one
            // signal with a count, not four hundred events.
            let key = (
                rule.id.clone(),
                item.entry.device_id.clone().unwrap_or_default(),
            );
            let window = i64::from(rule.throttle_s.max(1)) * 1_000;
            let count = {
                let mut throttles = state.throttles.lock();
                let throttle = throttles.entry(key).or_insert(Throttle {
                    window_started_at: 0,
                    suppressed: 0,
                });
                if item.entry.ts - throttle.window_started_at < window {
                    throttle.suppressed += 1;
                    continue;
                }
                let count = throttle.suppressed + 1;
                throttle.window_started_at = item.entry.ts;
                throttle.suppressed = 0;
                count
            };
            let interface = match &rule.emit {
                Signal::InterfaceDown { interface_group }
                | Signal::InterfaceUp { interface_group } => {
                    // What the line says, resolved against the device's own
                    // interfaces when they are known.
                    let written = groups.get(interface_group).cloned();
                    item.interface.clone().or(written)
                }
                _ => item.interface.clone(),
            };
            let (severity, title) = match &rule.emit {
                Signal::Alert { severity, title } => {
                    (Some(severity.clone()), render(title, &groups))
                }
                other => (None, format!("{}: {}", rule.name, other.kind())),
            };
            raised.push(Raised {
                rule_id: rule.id.clone(),
                rule_name: rule.name.clone(),
                kind: rule.emit.kind(),
                severity,
                title,
                interface,
                groups,
                device_id: item.entry.device_id.clone(),
                entry_id: item.entry.id,
                at: item.entry.ts,
                count,
            });
        }
    }
    for signal in raised {
        publish(app, signal);
    }
}

fn publish(app: &App, raised: Raised) {
    let device = raised
        .device_id
        .as_deref()
        .map(|id| crate::attribute::refresh(app, now_ms(), 30_000).map(|index| index.name_of(id)))
        .transpose()
        .ok()
        .flatten()
        .unwrap_or_default();
    let payload = json!({
        "rule_id": raised.rule_id,
        "rule": raised.rule_name,
        "kind": raised.kind,
        "device_id": raised.device_id,
        "device": device,
        "severity": raised.severity,
        "title": raised.title,
        "interface": raised.interface,
        "groups": raised.groups,
        "entry_id": raised.entry_id,
        "count": raised.count,
        "at": raised.at,
    });
    // Kept as well as published: a signal is the thing a correlation is
    // anchored to afterwards, and an event nobody was listening for is gone.
    let entry = LogEntry {
        id: 0,
        ts: raised.at,
        stream: "event".into(),
        device_id: raised.device_id.clone(),
        severity: None,
        source: format!("logs.{}", raised.kind),
        message: raised.title.clone(),
        body: Some(payload.clone()),
    };
    if let Err(err) = app.store().log_append(&entry) {
        tracing::error!(%err, "a log signal could not be stored");
    }
    app.publish("logs.signal", &payload);
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
        let app = app.clone();
        let dedup_key = format!(
            "logs:{}:{}:{}",
            raised.rule_id,
            raised.device_id.clone().unwrap_or_default(),
            raised.interface.clone().unwrap_or_default()
        );
        let params = json!({
            "rule": raised.rule_name,
            "device_id": raised.device_id,
            "instance": raised.interface,
            "severity": severity,
            "title": raised.title,
            "detail": raised.groups.get("detail"),
            "dedup_key": dedup_key,
        });
        // The monitoring feature may not be in this build; a feature that
        // is not there is not an error.
        app.clone().spawn("logs.alert", async move {
            if let Err(err) = app.rpc("alerts.raise", params).await
                && err.code != "unknown_method"
            {
                tracing::debug!(%err, "a log signal could not raise an alert");
            }
        });
    }
}

// ------------------------------------------------------------------- RPC

/// Check a rule against one line, without storing anything.
pub fn test(app: &App, rule: &LogRule, line: &str) -> AppResult<RuleTest> {
    let pattern = Regex::new(&rule.pattern)
        .map_err(|err| AppError::invalid(format!("the pattern does not compile: {err}")))?;
    let source = "0.0.0.0:0".parse().expect("a valid socket address");
    let parsed = nocturne_netprobe::syslog::parse(line.as_bytes(), source);
    let state = app.state::<LogsState>();
    let index = crate::attribute::refresh(app, now_ms(), 30_000)?;
    let prepared = crate::ingest::prepare(&state, &index, &parsed, now_ms(), "test", None);
    let severity = prepared.entry.severity;
    let parsed_view = json!({
        "severity": severity,
        "facility": prepared.facility,
        "mnemonic": prepared.mnemonic,
        "host": prepared.host,
        "message": prepared.entry.message,
    });
    if let Some(min) = rule.min_severity
        && !severity.is_some_and(|severity| severity <= min)
    {
        return Ok(RuleTest {
            matched: false,
            reason: format!(
                "the rule only looks at lines of severity {min} or worse; this one is {}",
                severity
                    .map(|severity| severity.to_string())
                    .unwrap_or_else(|| "unknown".into())
            ),
            groups: BTreeMap::new(),
            signal: None,
            parsed: Some(parsed_view),
        });
    }
    let Some(groups) = groups_of(&pattern, line) else {
        return Ok(RuleTest {
            matched: false,
            reason: "the pattern does not match this line".into(),
            groups: BTreeMap::new(),
            signal: None,
            parsed: Some(parsed_view),
        });
    };
    let (severity_name, title) = match &rule.emit {
        Signal::Alert { severity, title } => (Some(severity.clone()), render(title, &groups)),
        other => (None, format!("{}: {}", rule.name, other.kind())),
    };
    let interface = match &rule.emit {
        Signal::InterfaceDown { interface_group } | Signal::InterfaceUp { interface_group } => {
            groups.get(interface_group).cloned()
        }
        _ => None,
    };
    Ok(RuleTest {
        matched: true,
        reason: String::new(),
        groups,
        signal: Some(SignalPreview {
            kind: rule.emit.kind().to_string(),
            title,
            severity: severity_name,
            interface,
        }),
        parsed: Some(parsed_view),
    })
}

pub fn list(app: &App) -> AppResult<Vec<LogRule>> {
    let mut rules: Vec<LogRule> = app.store().list(COLLECTION)?;
    rules.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
    Ok(rules)
}

pub fn save(app: &App, mut rule: LogRule) -> AppResult<LogRule> {
    if rule.name.trim().is_empty() {
        return Err(AppError::invalid("a rule needs a name"));
    }
    Regex::new(&rule.pattern)
        .map_err(|err| AppError::invalid(format!("the pattern does not compile: {err}")))?;
    if let Signal::Alert { severity, .. } = &rule.emit
        && !["info", "low", "medium", "high", "critical"].contains(&severity.as_str())
    {
        return Err(AppError::invalid(format!(
            "`{severity}` is not a severity (info, low, medium, high, critical)"
        )));
    }
    let existing: Option<LogRule> = if rule.id.is_empty() {
        None
    } else {
        app.store().get(COLLECTION, &rule.id)?
    };
    if let Some(existing) = &existing
        && existing.builtin
    {
        // A built-in's definition is the application's; only its switch and
        // its scope are the user's, and a deleted one would come back anyway.
        let changed = LogRule {
            enabled: rule.enabled,
            scope: rule.scope.clone(),
            throttle_s: rule.throttle_s,
            ..existing.clone()
        };
        rule = changed;
    } else {
        rule.builtin = false;
        if rule.id.is_empty() {
            rule.id = nocturne_app::util::new_id();
        }
    }
    rule.updated_at = now_ms();
    app.store().put(COLLECTION, &rule.id, &rule)?;
    invalidate(app);
    app.audit(
        "logs.rule.save",
        None,
        format!("Saved log rule `{}`", rule.name),
        json!({"rule_id": rule.id, "enabled": rule.enabled}),
    );
    Ok(rule)
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

    #[test]
    fn every_built_in_rule_matches_its_own_test_line() {
        for rule in builtins() {
            let pattern = Regex::new(&rule.pattern)
                .unwrap_or_else(|err| panic!("{} does not compile: {err}", rule.id));
            assert!(
                pattern.is_match(&rule.test_line),
                "{} does not match the line it ships with:\n  {}",
                rule.id,
                rule.test_line
            );
            assert!(
                !rule.test_line_source.is_empty(),
                "{} does not say where its test line came from",
                rule.id
            );
            if let Signal::InterfaceDown { interface_group }
            | Signal::InterfaceUp { interface_group } = &rule.emit
            {
                let groups = groups_of(&pattern, &rule.test_line).expect("it matched");
                assert!(
                    groups.contains_key(interface_group),
                    "{} promises an interface in `{interface_group}` and captures {groups:?}",
                    rule.id
                );
            }
        }
    }

    #[test]
    fn rules_do_not_match_each_others_lines() {
        let rules = builtins();
        for rule in &rules {
            let pattern = Regex::new(&rule.pattern).expect("compiles");
            for other in &rules {
                if other.id == rule.id {
                    continue;
                }
                // Up and down share a mnemonic; everything else must be
                // distinguishable or a single event raises two signals.
                let pair = rule.id.replace("_up", "_down") == other.id.replace("_up", "_down");
                if pair {
                    continue;
                }
                assert!(
                    !pattern.is_match(&other.test_line),
                    "{} also matches the test line of {}",
                    rule.id,
                    other.id
                );
            }
        }
    }

    #[test]
    fn a_title_is_rendered_from_what_the_pattern_captured() {
        let groups = BTreeMap::from([
            ("interface".to_string(), "Gi1/0/7".to_string()),
            ("reason".to_string(), "bpduguard".to_string()),
        ]);
        assert_eq!(
            render("{interface} error-disabled ({reason})", &groups),
            "Gi1/0/7 error-disabled (bpduguard)"
        );
        assert_eq!(render("nothing to fill", &groups), "nothing to fill");
    }
}
