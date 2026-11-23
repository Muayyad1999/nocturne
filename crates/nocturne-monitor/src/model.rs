//! The records monitoring keeps: alerts, alert rules, maintenance windows,
//! notification channels and the interface table.
//!
//! These are the shapes the UI, the operations record and the assistant are
//! written against (`docs/dev/features/monitor.md`), so field names and enum
//! spellings here are a contract.

use nocturne_drivers::model::Direction;
use serde::{Deserialize, Serialize};

// --------------------------------------------------------------- severity

/// How much attention something wants. Ordered: `Info` < … < `Critical`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Low,
    #[default]
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    /// Rank, for storing in an indexed column and for ordering.
    pub fn rank(self) -> i64 {
        match self {
            Severity::Info => 0,
            Severity::Low => 1,
            Severity::Medium => 2,
            Severity::High => 3,
            Severity::Critical => 4,
        }
    }

    pub fn from_rank(rank: i64) -> Severity {
        match rank {
            0 => Severity::Info,
            1 => Severity::Low,
            3 => Severity::High,
            4 => Severity::Critical,
            _ => Severity::Medium,
        }
    }

    /// Syslog-style severity for the `event` log stream.
    pub fn syslog(self) -> u8 {
        match self {
            Severity::Info => 6,
            Severity::Low => 5,
            Severity::Medium => 4,
            Severity::High => 3,
            Severity::Critical => 2,
        }
    }
}

// ------------------------------------------------------------------ alert

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertState {
    #[default]
    Firing,
    Acknowledged,
    Resolved,
}

impl AlertState {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertState::Firing => "firing",
            AlertState::Acknowledged => "acknowledged",
            AlertState::Resolved => "resolved",
        }
    }

    /// An alert that has not resolved itself yet.
    pub fn is_open(self) -> bool {
        !matches!(self, AlertState::Resolved)
    }
}

/// Something that needs a human, or did.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Alert {
    pub id: String,
    pub rule_id: Option<String>,
    /// The rule's name, or the feature that raised it through `alerts.raise`.
    pub rule_name: String,
    pub device_id: Option<String>,
    /// The device's name at the time the alert was raised.
    pub device: Option<String>,
    /// Interface name, sensor, or whatever else the rule is keyed on.
    pub instance: Option<String>,
    pub severity: Severity,
    pub state: AlertState,
    pub title: String,
    pub detail: String,
    /// The measurement that fired the rule, when there is one.
    pub value: Option<f64>,
    pub started_at: i64,
    pub last_seen_at: i64,
    pub resolved_at: Option<i64>,
    pub acked_at: Option<i64>,
    pub ack_note: Option<String>,
    pub snoozed_until: Option<i64>,
    /// How often the condition has been seen since this alert was raised.
    /// Seen means measured: a rule looking again at what a service check
    /// last found is not another sighting.
    pub count: u64,
    /// Whether a maintenance window covered the alert the last time it was
    /// seen, announced or resolved. A record of that moment, not a live
    /// answer.
    pub muted_by_maintenance: bool,
    /// When the firing notification went out: the first look — by the rules,
    /// by `alerts.raise`, or by the scheduler's own pass over what is still
    /// owed — that found the alert firing, outside every maintenance window,
    /// not snoozed, and not about a lab device while those are kept quiet.
    /// `None` while nobody has been told — and then nobody is told when it
    /// resolves either.
    pub notified_at: Option<i64>,
    /// The channels `alerts.raise` named for this alert. `None` is the usual
    /// routing (the rule's channels, or every enabled one), an empty list is
    /// nobody, and such an alert is never announced. Kept here because the
    /// notice can be due long after the call that named them: when a window
    /// closes, and again when the alert resolves.
    pub notify: Option<Vec<String>>,
    /// The alert is about a simulated device. Its notices go out only while
    /// the `notify_for_lab` setting is on.
    pub lab: bool,
    /// What makes two sightings the same alert. Rules build it from
    /// (rule, device, instance); `alerts.raise` takes it from the caller.
    pub dedup_key: String,
}

impl Alert {
    /// Whether this alert's firing notification is held back right now by
    /// something the alert itself records: a maintenance window covered it
    /// when it was last looked at, or it is snoozed. The lab setting is not
    /// on the alert, and is asked about where a notice is decided
    /// (`alerts::announce_if_due`).
    pub fn muted(&self, now: i64) -> bool {
        self.muted_by_maintenance || self.snoozed_until.is_some_and(|until| until > now)
    }
}

// ------------------------------------------------------------------- rule

/// Which devices a rule or a maintenance window covers. Empty lists mean "any".
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    pub sites: Vec<String>,
    pub roles: Vec<String>,
    pub tags: Vec<String>,
    pub device_ids: Vec<String>,
    /// Lab devices are left alone unless a rule opts in.
    pub include_lab: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceSelector {
    /// Interfaces with a collected LLDP/CDP neighbour.
    #[default]
    Uplinks,
    /// Interfaces with a description.
    Described,
    All,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    #[default]
    Gt,
    Lt,
}

impl Op {
    pub fn holds(self, value: f64, threshold: f64) -> bool {
        match self {
            Op::Gt => value > threshold,
            Op::Lt => value < threshold,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Op::Gt => "above",
            Op::Lt => "below",
        }
    }
}

/// Which state of a service check a rule fires on.
///
/// Three values, not one, because they are three different things to be told:
/// a service is down, a service is about to stop working, and *we do not know
/// whether a service is working* because the check cannot run. The third is
/// the one that is usually missing from monitoring, and it is the one that
/// hides outages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckAlertState {
    /// The check ran and the service failed it.
    #[default]
    Down,
    /// The check ran, the service answered, and something it measures is
    /// inside its warning margin — a certificate about to expire.
    Warning,
    /// The check could not be attempted at all: no credential, nothing to
    /// aim at, a machine that may not send what it needs to send. Not an
    /// outage — a blind spot.
    CannotRun,
}

impl CheckAlertState {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckAlertState::Down => "down",
            CheckAlertState::Warning => "warning",
            CheckAlertState::CannotRun => "cannot_run",
        }
    }
}

/// What a rule watches for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    /// The damped reachability state says the device is down.
    DeviceDown,
    /// Administratively up but the line protocol is down.
    InterfaceDown { only: InterfaceSelector },
    /// A metric crossed a threshold. `clear_value` gives hysteresis: the
    /// alert resolves only once the metric crosses back past it.
    Threshold {
        /// A metric name, or a glob over metric names (`if.*_err_ps`).
        metric: String,
        /// A glob over instances (interface names, sensors); `None` = any.
        #[serde(default)]
        instance: Option<String>,
        op: Op,
        value: f64,
        #[serde(default)]
        clear_value: Option<f64>,
    },
    /// Nothing has been recorded for `metric` in the last `for_s` seconds.
    NoData { metric: String, for_s: u64 },
    /// A named service check on a device in scope is in `state`.
    ///
    /// This is what lets a rule say "the web server on this box", which a
    /// metric threshold cannot: `check` is a glob over check names (`web`,
    /// `*`, `cert-*`), and each matching check is its own alert, keyed on its
    /// name, so two services failing on one device are two alerts.
    CheckState {
        /// A glob over check names; `None` or `*` is every check.
        #[serde(default)]
        check: Option<String>,
        /// Only checks of these kinds; empty is every kind.
        #[serde(default)]
        kinds: Vec<crate::checks::model::CheckKind>,
        #[serde(default)]
        state: CheckAlertState,
    },
    /// The `config.changed` event fired for a device in scope.
    ConfigChanged,
    /// Round-trip time far above what this device normally does. Not in the
    /// original condition list, but the built-in "RTT > 5× the 24 h median"
    /// rule cannot be written with the others.
    RttBaseline {
        /// How many times the median counts as abnormal.
        multiple: f64,
        /// How far back the median is taken over.
        window_s: u64,
    },
}

impl Condition {
    /// Rules that are pointless while the device itself is down: one cause,
    /// one alert.
    pub fn suppressed_by_device_down(&self) -> bool {
        !matches!(self, Condition::DeviceDown | Condition::ConfigChanged)
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Condition::DeviceDown => "device_down",
            Condition::InterfaceDown { .. } => "interface_down",
            Condition::Threshold { .. } => "threshold",
            Condition::NoData { .. } => "no_data",
            Condition::CheckState { .. } => "check_state",
            Condition::ConfigChanged => "config_changed",
            Condition::RttBaseline { .. } => "rtt_baseline",
        }
    }

    /// Whether this condition is about service checks, which are evaluated
    /// after a check runs as well as after a poll.
    pub fn is_check(&self) -> bool {
        matches!(self, Condition::CheckState { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Shipped with Nocturne. Built-in rules can be edited and disabled; a
    /// deleted one comes back as its default at the next start.
    pub builtin: bool,
    pub severity: Severity,
    pub scope: Scope,
    pub condition: Condition,
    /// The condition must hold this long before the alert fires.
    pub for_s: u64,
    /// Notification channel ids.
    pub notify: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Default for AlertRule {
    fn default() -> Self {
        AlertRule {
            id: String::new(),
            name: String::new(),
            enabled: true,
            builtin: false,
            severity: Severity::Medium,
            scope: Scope::default(),
            condition: Condition::DeviceDown,
            for_s: 0,
            notify: Vec::new(),
            created_at: 0,
            updated_at: 0,
        }
    }
}

// ------------------------------------------------------------ maintenance

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Recurrence {
    /// The same weekday and time every week.
    Weekly,
}

/// A planned outage: alerts inside it are recorded but not notified.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaintenanceWindow {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub starts_at: i64,
    pub ends_at: i64,
    pub recurring: Option<Recurrence>,
    pub note: String,
    pub created_at: i64,
    pub updated_at: i64,
}

// --------------------------------------------------------------- channels

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    /// A JSON POST of the alert itself.
    #[default]
    Webhook,
    Slack,
    Teams,
    Discord,
    /// SMTP, with STARTTLS where the server offers it.
    Email,
}

impl ChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelKind::Webhook => "webhook",
            ChannelKind::Slack => "slack",
            ChannelKind::Teams => "teams",
            ChannelKind::Discord => "discord",
            ChannelKind::Email => "email",
        }
    }

    /// Whether the channel's secret is the URL it posts to.
    pub fn is_webhook(self) -> bool {
        !matches!(self, ChannelKind::Email)
    }
}

/// Everything an e-mail channel needs that is not the password.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EmailSettings {
    pub host: String,
    pub port: u16,
    /// `auto` (STARTTLS when the server offers it), `required` or `never`.
    pub starttls: String,
    pub username: String,
    pub from: String,
    pub to: Vec<String>,
}

/// Where notifications go. The webhook URL and the SMTP password are never
/// part of this record: they live in the vault under
/// `monitor:channel:<id>:secret`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
    pub enabled: bool,
    /// Alerts below this severity are not sent to this channel.
    pub min_severity: Severity,
    pub email: Option<EmailSettings>,
    /// Whether a URL / password is stored for this channel.
    pub has_secret: bool,
    pub last_sent_at: Option<i64>,
    /// Why the last attempt failed (never contains the URL or the password).
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

// -------------------------------------------------------------- interfaces

/// One interface as monitoring last saw it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IfStatus {
    pub device_id: String,
    /// The device's name, for lists that span devices.
    pub device: String,
    pub name: String,
    pub description: String,
    pub admin_up: Option<bool>,
    pub oper_up: Option<bool>,
    pub speed_bps: Option<u64>,
    /// Rates are `null` when the interval could not be measured (first poll
    /// after a start, a reboot, a failed poll, a counter cleared on the
    /// device) — a gap, never a zero.
    pub in_bps: Option<f64>,
    pub out_bps: Option<f64>,
    pub util_pct: Option<f64>,
    pub in_err_ps: Option<f64>,
    pub out_err_ps: Option<f64>,
    pub in_disc_ps: Option<f64>,
    pub out_disc_ps: Option<f64>,
    /// Whether a neighbour has been collected on this interface (an uplink).
    pub uplink: bool,
    pub lab: bool,
    pub at: i64,
}

// --------------------------------------------------------------------- QoS

/// One class of a QoS policy as monitoring last read it: what it is, and
/// what it did over the interval that ended with that read.
///
/// The identity and the counters are the device's own
/// (`nocturne_drivers::model::QosClass`); the rates are computed here, from
/// two readings of the counters over the device's own uptime between them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QosStatus {
    pub device_id: String,
    /// The interface the policy is attached to; empty where the platform
    /// attaches it to something else.
    pub interface: String,
    /// `in` or `out`, seen from the device.
    pub direction: Direction,
    pub policy: String,
    pub class: String,
    /// The class this one's policy sits in. A parent's counters include its
    /// children's.
    pub parent: Option<String>,
    /// The instance of this class's `qos.*` series, to chart it by. `null`
    /// when it has none: it is past `qos_max_series`, or it cannot be told
    /// from another class of the device by name.
    pub instance: Option<String>,
    /// The configured rate, where the platform prints one.
    pub rate_bps: Option<u64>,
    /// The rates are `null` when the interval could not be measured (the
    /// first read, a restart, a failed read, a counter that went backwards)
    /// or the platform does not print the counter — a gap, never a zero.
    pub bps: Option<f64>,
    pub drop_ps: Option<f64>,
    /// Congestion drops only; `null` also where the device counts one kind
    /// of drop.
    pub queue_drop_ps: Option<f64>,
    /// Drops as a share of the packets offered over the interval; `null`
    /// also when none were.
    pub drop_pct: Option<f64>,
    /// Queue depth at the read: a gauge, known from the first read on.
    pub queue_len: Option<u64>,
    /// The counters as last read.
    pub packets: Option<u64>,
    pub dropped_packets: Option<u64>,
    /// Losing packets to congestion, as far as the device can tell: its
    /// congestion drops are above zero, or — where the device keeps one drop
    /// counter for a class and so cannot say which — its drops are. A class
    /// whose every drop is a policer's is the policy working, and is not.
    /// What `problem_only` selects.
    pub dropping: bool,
    /// When the counters were read. Later than the device's last successful
    /// read only never; earlier when reads have been failing since.
    pub at: i64,
    /// The read that produced this row has not been checked against a real
    /// device of its platform.
    pub unverified: bool,
}

/// Where a device stands with the QoS read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QosState {
    /// The last read worked. No rows then means no policy is attached.
    Ok,
    /// Nothing can be asked: the device's type has no QoS read, or there is
    /// no command line to ask it on.
    Unsupported,
    /// The last read did not work; `error` says why.
    Failed,
    /// It can be read and has not been yet.
    #[default]
    Never,
}

/// One device's entry in the coverage of `monitor.qos`: what the absence of
/// rows means for it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QosDeviceCoverage {
    pub device_id: String,
    pub state: QosState,
    /// When the read that `state` describes was made.
    pub at: Option<i64>,
    pub unverified: bool,
    /// The device has more classes than `qos_max_series`: the table holds
    /// them all, the series and the alert rules only the first so many.
    pub series_capped: bool,
    /// Why the read failed. With `ok`: what a read that worked could not
    /// place and left out.
    pub error: Option<String>,
    /// Classes held for the device, and how many of them have series.
    pub classes: u64,
    pub series: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct QosCoverage {
    pub devices: Vec<QosDeviceCoverage>,
}

/// What monitoring remembers of a device's last QoS read. `state` is `None`
/// until one has been attempted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct QosRecord {
    /// `Ok` or `Failed`; the other two states are not outcomes of a read.
    pub state: Option<QosState>,
    pub at: Option<i64>,
    pub error: Option<String>,
    pub unverified: bool,
    pub series_capped: bool,
    pub classes: u64,
    pub series: u64,
}

// ------------------------------------------------------------------ health

/// The device-level numbers of the last poll that produced them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceHealth {
    pub cpu_pct: Option<f64>,
    pub mem_pct: Option<f64>,
    pub temp_c: Option<f64>,
    pub uptime_s: Option<u64>,
    pub at: Option<i64>,
}

/// How one source of data for a device is doing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SourceStatus {
    pub enabled: bool,
    /// `None` until it has been tried.
    pub ok: Option<bool>,
    pub at: Option<i64>,
    /// How reachability was established: `icmp`, `tcp/22`, `snmp`, `sim`.
    pub via: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sources {
    pub icmp: SourceStatus,
    pub snmp: SourceStatus,
    pub cli: SourceStatus,
}

/// What monitoring remembers about a device between polls. One document per
/// device, so a restart does not lose the last known picture.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceRecord {
    pub device_id: String,
    pub last_poll_at: Option<i64>,
    pub last_ok_at: Option<i64>,
    pub health: DeviceHealth,
    pub sources: Sources,
    /// Consecutive failed polls, for damping across restarts.
    pub failures: u32,
    /// The damped verdict: `true` once `down_after` polls in a row failed.
    pub down: bool,
    /// When the device was last declared up again after being down; `None`
    /// until that has happened. Whatever a service check found before this
    /// was found on a device that was not there, and says nothing about the
    /// service now.
    pub up_since: Option<i64>,
    /// The outcome of the last QoS read.
    pub qos: QosRecord,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn conditions_round_trip_through_their_tagged_form() {
        let threshold = Condition::Threshold {
            metric: "cpu.pct".into(),
            instance: None,
            op: Op::Gt,
            value: 90.0,
            clear_value: Some(80.0),
        };
        let encoded = serde_json::to_value(&threshold).unwrap();
        assert_eq!(
            encoded,
            json!({"type": "threshold", "metric": "cpu.pct", "instance": null,
                   "op": "gt", "value": 90.0, "clear_value": 80.0})
        );
        assert_eq!(
            serde_json::from_value::<Condition>(encoded).unwrap(),
            threshold
        );
        assert_eq!(
            serde_json::from_value::<Condition>(json!({"type": "device_down"})).unwrap(),
            Condition::DeviceDown
        );
        assert_eq!(
            serde_json::from_value::<Condition>(
                json!({"type": "interface_down", "only": "uplinks"})
            )
            .unwrap(),
            Condition::InterfaceDown {
                only: InterfaceSelector::Uplinks
            }
        );
    }

    #[test]
    fn severities_order_and_round_trip_through_their_rank() {
        assert!(Severity::Critical > Severity::High && Severity::Low > Severity::Info);
        for severity in [
            Severity::Info,
            Severity::Low,
            Severity::Medium,
            Severity::High,
            Severity::Critical,
        ] {
            assert_eq!(Severity::from_rank(severity.rank()), severity);
            assert_eq!(
                serde_json::to_value(severity).unwrap(),
                json!(severity.as_str())
            );
        }
    }

    #[test]
    fn a_check_condition_names_the_check_and_the_state_it_fires_on() {
        let condition = Condition::CheckState {
            check: Some("web".into()),
            kinds: vec![crate::checks::model::CheckKind::Https],
            state: CheckAlertState::CannotRun,
        };
        let encoded = serde_json::to_value(&condition).unwrap();
        assert_eq!(
            encoded,
            json!({"type": "check_state", "check": "web", "kinds": ["https"],
                   "state": "cannot_run"})
        );
        assert_eq!(
            serde_json::from_value::<Condition>(encoded).unwrap(),
            condition
        );
        assert_eq!(condition.kind(), "check_state");
        assert!(condition.is_check());
        assert!(!Condition::DeviceDown.is_check());

        // Everything but the state may be left out: "any check, down".
        let bare: Condition = serde_json::from_value(json!({"type": "check_state"})).unwrap();
        assert_eq!(
            bare,
            Condition::CheckState {
                check: None,
                kinds: Vec::new(),
                state: CheckAlertState::Down
            }
        );
    }

    #[test]
    fn device_down_suppresses_the_rules_it_would_explain() {
        assert!(!Condition::DeviceDown.suppressed_by_device_down());
        assert!(!Condition::ConfigChanged.suppressed_by_device_down());
        assert!(
            Condition::InterfaceDown {
                only: InterfaceSelector::All
            }
            .suppressed_by_device_down()
        );
        assert!(
            Condition::NoData {
                metric: "cpu.pct".into(),
                for_s: 600
            }
            .suppressed_by_device_down()
        );
        assert!(
            Condition::CheckState {
                check: None,
                kinds: Vec::new(),
                state: CheckAlertState::Down
            }
            .suppressed_by_device_down(),
            "a box that is down explains its own web server being down"
        );
    }
}
