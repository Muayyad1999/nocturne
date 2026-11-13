//! The shapes this feature's RPC surface returns. The UI and
//! `ui/src/api/types/logs.ts` are written against these.

use serde::{Deserialize, Serialize};

/// What a receiver is doing, as `logs.status` reports it.
///
/// `received` is datagrams and lines taken off the socket, `stored` what was
/// written, `dropped` what was deliberately thrown away (the queue was full,
/// or the source is unknown and the setting says drop). The three are
/// counted since this process started and they do not have to add up while a
/// batch is in flight.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverStatus {
    pub enabled: bool,
    /// The addresses actually bound, e.g. `udp 0.0.0.0:5514`. Empty when the
    /// receiver is off or could not start.
    pub listening: Vec<String>,
    /// Why the receiver is not listening although it is enabled.
    pub error: Option<String>,
    pub received: u64,
    pub stored: u64,
    pub dropped: u64,
    /// Lines waiting to be written right now.
    pub queued: u64,
    /// Lines received in the last minute.
    pub rate_per_minute: u64,
    pub last_at: Option<i64>,
}

/// A receiver the specification asks for that this build does not have.
/// It is reported rather than left out, and it never claims to be running.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotBuilt {
    pub enabled: bool,
    pub implemented: bool,
    pub error: String,
}

impl NotBuilt {
    pub fn new(what: &str) -> NotBuilt {
        NotBuilt {
            enabled: false,
            implemented: false,
            error: format!("{what} is not implemented in this build"),
        }
    }
}

/// How much is stored and what retention last did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreStatus {
    pub rows: i64,
    pub oldest_at: Option<i64>,
    pub retention_days: u32,
    pub max_rows: u64,
    pub last_prune_at: Option<i64>,
    pub pruned_by_age: u64,
    pub pruned_by_rows: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogsStatus {
    pub syslog: ReceiverStatus,
    pub traps: NotBuilt,
    pub flows: NotBuilt,
    pub store: StoreStatus,
    /// Addresses that have sent and that no device in the inventory claims.
    pub unattributed_sources: usize,
}

/// One bucket of `logs.histogram`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    /// Start of the bucket, milliseconds since the epoch.
    pub t: i64,
    pub count: i64,
    /// Counts by syslog severity, index 0 (emergency) … 7 (debug). Entries
    /// with no severity at all are counted in `count` and in `no_severity`.
    pub by_severity: [i64; 8],
    pub no_severity: i64,
}

/// One row of `logs.top`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopRow {
    pub key: String,
    /// A human label when the key is an opaque id (a device name).
    pub label: String,
    pub count: i64,
}

/// A run of similar lines: what a screen draws as "this happened 240 times".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Burst {
    pub device_id: Option<String>,
    pub device: String,
    /// The vendor mnemonic the lines share, when they have one.
    pub mnemonic: Option<String>,
    pub interface: Option<String>,
    pub severity: Option<u8>,
    pub count: i64,
    pub first_at: i64,
    pub last_at: i64,
    /// The first line of the burst, as an example.
    pub example: String,
    pub first_id: i64,
    pub last_id: i64,
}

/// Where a correlated list came from, so that an empty list is not read as
/// "nothing happened" when it means "that feature is not in this build".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceState {
    /// The feature answered.
    Ok,
    /// The feature is not registered in this build.
    Unavailable,
    /// The feature answered with an error.
    Failed,
}

/// A rule that turns log lines into a signal other features react to.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// Built-in rules ship with the application and cannot be deleted.
    pub builtin: bool,
    /// Which stream the rule reads (only `syslog` is produced today).
    pub stream: String,
    /// A regular expression over the message. Named groups are available to
    /// the signal's title template and are reported with the signal.
    pub pattern: String,
    /// Only lines at least this severe (numerically ≤, as syslog counts).
    pub min_severity: Option<u8>,
    pub scope: Scope,
    pub emit: Signal,
    /// At most one signal per device per rule per window; the rest are counted.
    pub throttle_s: u32,
    /// A line the rule is meant to match, from vendor documentation or a
    /// real device. `logs.rules.test` runs the rule against it.
    pub test_line: String,
    /// Where `test_line` came from.
    pub test_line_source: String,
    pub updated_at: i64,
}

impl Default for LogRule {
    fn default() -> Self {
        LogRule {
            id: String::new(),
            name: String::new(),
            enabled: true,
            builtin: false,
            stream: crate::db::STREAM.to_string(),
            pattern: String::new(),
            min_severity: None,
            scope: Scope::default(),
            emit: Signal::Alert {
                severity: "medium".into(),
                title: String::new(),
            },
            throttle_s: 60,
            test_line: String::new(),
            test_line_source: String::new(),
            updated_at: 0,
        }
    }
}

/// Which devices a rule applies to. Every list empty = all of them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scope {
    pub device_ids: Vec<String>,
    pub sites: Vec<String>,
    pub roles: Vec<String>,
    pub tags: Vec<String>,
    /// Device profile ids (`cisco_ios`, `mikrotik_routeros`).
    pub profiles: Vec<String>,
}

/// What a rule raises when it matches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Signal {
    /// The device's configuration changed: `logs.config_changed`.
    ConfigChanged,
    /// A link went down. `interface_group` names the regex group holding the
    /// interface, so the signal can say which one.
    InterfaceDown {
        interface_group: String,
    },
    InterfaceUp {
        interface_group: String,
    },
    LoginFailure,
    Reload,
    /// Hand to `alerts.raise` when the monitoring feature is present.
    /// `title` may use `{group}` placeholders from the pattern.
    Alert {
        severity: String,
        title: String,
    },
}

impl Signal {
    pub fn kind(&self) -> &'static str {
        match self {
            Signal::ConfigChanged => "config_changed",
            Signal::InterfaceDown { .. } => "interface_down",
            Signal::InterfaceUp { .. } => "interface_up",
            Signal::LoginFailure => "login_failure",
            Signal::Reload => "reload",
            Signal::Alert { .. } => "alert",
        }
    }
}

/// What a rule did with one line, as `logs.rules.test` reports it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RuleTest {
    pub matched: bool,
    /// Why it did not match, when it did not.
    pub reason: String,
    /// Named groups the pattern captured.
    pub groups: std::collections::BTreeMap<String, String>,
    /// The signal it would raise, with its title rendered.
    pub signal: Option<SignalPreview>,
    /// How the line itself was understood (severity, mnemonic, …).
    pub parsed: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalPreview {
    pub kind: String,
    pub title: String,
    pub severity: Option<String>,
    pub interface: Option<String>,
}
