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
    /// Lines a lab handed over through `logs.ingest`. They never crossed a
    /// socket, so they are not in `received`; they are in `stored`.
    pub from_lab: u64,
}

/// What the SNMP trap receiver is doing, as `logs.status` reports it.
///
/// Every datagram taken off the socket (`received`) ends up in exactly one
/// of `stored`, `dropped`, `rejected`, `unauthenticated`, `malformed` and
/// `duplicates`, counted since this process started. None of them, and
/// nothing else here, ever holds a community.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrapStatus {
    pub enabled: bool,
    /// Always true. It is here because a build once reported a receiver it
    /// did not have with `implemented: false`, and a screen written against
    /// that still asks.
    pub implemented: bool,
    /// The address actually bound, e.g. `udp 0.0.0.0:1162`. Empty when the
    /// receiver is off or could not start.
    pub listening: Vec<String>,
    /// Why the receiver is not listening although it is enabled.
    pub error: Option<String>,
    pub received: u64,
    pub stored: u64,
    /// Thrown away on purpose: the queue was full, or the source is
    /// unknown and the setting says drop.
    pub dropped: u64,
    /// Notifications waiting to be written right now.
    pub queued: u64,
    pub rate_per_minute: u64,
    pub last_at: Option<i64>,
    /// SNMPv1/v2c notifications whose community is not the one stored for
    /// the device their source address belongs to (or that came from no
    /// device, or from one with no community stored). Not kept.
    pub rejected: u64,
    /// SNMPv3 notifications that are authenticated or encrypted. Verifying
    /// one needs the sender's user and keys, which this receiver does not
    /// do yet, so they are counted and not kept as if they had been
    /// verified.
    pub unauthenticated: u64,
    /// Datagrams that are not an SNMP notification at all.
    pub malformed: u64,
    /// Informs that arrived again because their acknowledgement was lost
    /// or late. Acknowledged again, stored once.
    pub duplicates: u64,
    /// Informs acknowledged — after they were stored, never before.
    pub acknowledged: u64,
    /// The `require_known_community` setting, as the receiver is applying it.
    pub require_known_community: bool,
    /// The sources turned away most recently, newest first (at most 20):
    /// which address, which device it belongs to if any, and why. Kept in
    /// memory for at most 64 addresses.
    pub rejections: Vec<Rejection>,
    /// What a lab contributes to these numbers: nothing, and why.
    pub lab: String,
}

/// One source whose notifications were not kept, and why — in words that
/// never include what it sent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rejection {
    pub address: String,
    pub device_id: Option<String>,
    pub reason: String,
    pub count: u64,
    pub last_at: i64,
}

/// What the flow collector is doing, as `logs.status` reports it.
///
/// A flow export datagram carries flow records. `received` and `malformed`
/// count datagrams; everything else counts records. Every record decoded
/// (`records`) ends in exactly one of `counted` and `dropped`, or is still
/// in `queued`. All of it is counted since this process started, and none
/// of it is stored: there is no flow in the database.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowStatus {
    pub enabled: bool,
    /// Always true; see [`TrapStatus::implemented`].
    pub implemented: bool,
    /// The address actually bound, e.g. `udp 0.0.0.0:2055`. Empty when the
    /// collector is off or could not start.
    pub listening: Vec<String>,
    /// Why the collector is not listening although it is enabled.
    pub error: Option<String>,
    /// Datagrams taken off the socket.
    pub received: u64,
    /// Datagrams that are not a NetFlow v5, NetFlow v9 or IPFIX export.
    pub malformed: u64,
    /// Flow records decoded.
    pub records: u64,
    /// Of those, the ones added to the buckets `flows.top` answers from.
    pub counted: u64,
    /// And the ones that were not: the sum of `drops`.
    pub dropped: u64,
    pub drops: FlowDrops,
    /// Flow records decoded and waiting to be counted right now.
    pub queued: u64,
    /// Datagrams from which something was taken in the last minute.
    pub rate_per_minute: u64,
    pub last_at: Option<i64>,
    /// Data sets that arrived before the template that describes them. What
    /// they held is unknown and is in no number here.
    pub sets_without_template: u64,
    /// Exporters followed right now (at most 256).
    pub exporters: u64,
    /// Of those, the ones no device in the inventory claims.
    pub unattributed_exporters: u64,
    pub memory: FlowMemory,
    /// What a lab contributes to these numbers: nothing, and why.
    pub lab: String,
}

/// Why flow records were decoded and not counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowDrops {
    /// More arrived than could be counted in time: the queue between the
    /// socket and the buckets was at its limit (`queue_limit` records).
    pub queue_full: u64,
    /// From an exporter there was no room to follow: 256 are followed, and
    /// an address no device claims does not take the place of another.
    pub no_room: u64,
    /// From an exporter no device claims, with `unknown_sources: "drop"`.
    /// The exporter is still listed.
    pub unclaimed: u64,
    /// The inventory could not be read to say whose the exporter is.
    pub unreadable: u64,
}

/// What the buckets hold, and how far back they reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowMemory {
    /// One-minute buckets held, over all exporters.
    pub minutes: u64,
    /// Keys held in them (a source, a conversation, a port …).
    pub keys: u64,
    /// The ceiling on the keys of finished minutes.
    pub max_keys: u64,
    /// Buckets let go before they were a day old, to stay under it.
    pub evicted_minutes: u64,
    /// The start of the oldest bucket held.
    pub oldest_at: Option<i64>,
}

/// One row of `flows.top`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowRow {
    /// What the row is about, as a stable text: an address, `a <-> b`,
    /// `tcp/443`, a DSCP number, or `<exporter> <ifIndex> <in|out>`.
    pub key: String,
    /// The same for a person: `tcp/443 (https)`, `EF (46)`,
    /// `GigabitEthernet0/3 in`.
    pub label: String,
    /// As the exporter sent them — a floor when the minute held more keys
    /// than are kept, never an estimate, and never multiplied by a
    /// sampling rate.
    pub bytes: u64,
    pub packets: u64,
    /// Flow records added up.
    pub flows: u64,
    /// For `by: "interface"` only: whose interface it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interface: Option<FlowInterface>,
}

/// The interface a row of `flows.top {by: "interface"}` is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowInterface {
    /// The exporter's address.
    pub exporter: String,
    pub device_id: Option<String>,
    /// The ifIndex the exporter reported; 0 when it reported none.
    pub if_index: u32,
    /// Its name, when the monitoring feature has one for that index.
    pub name: Option<String>,
    /// `in`: the traffic came in through it. `out`: it left by it.
    pub direction: String,
}

/// Bytes, packets and flow records added up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowCounts {
    pub bytes: u64,
    pub packets: u64,
    pub flows: u64,
}

/// What an answer of `flows.top` is made of, and what it cannot say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowCoverage {
    /// Whether the collector is listening right now. When it is not, an
    /// empty answer means "not collecting", not "no traffic".
    pub collecting: bool,
    /// The start of the window that was asked for (a whole minute).
    pub window_start: i64,
    /// When the answer was made, which is where the window ends.
    pub until: i64,
    /// The start of the first minute in the window that has anything in
    /// it. `null`: nothing arrived in the window from the exporters asked
    /// about.
    pub since: Option<i64>,
    /// Flow records the answer adds up.
    pub records: u64,
    /// Data sets that arrived in the window before their template: traffic
    /// the exporters reported that is in no row and not in `total`.
    pub sets_without_template: u64,
    /// The "1 in N" rates the exporters said they sample at, ascending
    /// (at most eight). Empty when none said one. The counts are as
    /// exported: nothing is multiplied.
    pub sampling: Vec<u32>,
    /// How many of `records` came with such a rate.
    pub sampled_records: u64,
    /// Exporters with something in the window.
    pub exporters: u64,
}

/// The answer of `flows.top`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowTop {
    pub rows: Vec<FlowRow>,
    /// Everything the exporters reported in the window, whatever it is
    /// looked at by.
    pub total: FlowCounts,
    /// Of that, what is under no key: a minute keeps only its biggest
    /// talkers and adds the rest up here.
    pub other: FlowCounts,
    pub coverage: FlowCoverage,
}

/// One exporter, as `flows.exporters` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowExporter {
    pub address: String,
    pub device_id: Option<String>,
    /// The device's name, when a device claims the address.
    pub device: Option<String>,
    /// Why no device does (empty when one does).
    pub reason: String,
    /// What it last spoke: 5, 9 or 10 (IPFIX).
    pub version: u8,
    pub first_at: i64,
    pub last_at: i64,
    pub datagrams: u64,
    /// Flow records counted from it.
    pub records: u64,
    /// Flow records it sent that were not counted because no device claims
    /// it and `unknown_sources` is `drop`.
    pub dropped: u64,
    /// Templates known for it right now.
    pub templates: u32,
    /// Data sets it sent before the template that describes them.
    pub sets_without_template: u64,
    /// The sampling rates it has said.
    pub sampling: Vec<u32>,
}

/// How much is stored and what retention last did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreStatus {
    pub rows: i64,
    pub oldest_at: Option<i64>,
    pub retention_days: u32,
    pub max_rows: u64,
    pub last_prune_at: Option<i64>,
    /// Entries removed for being older than `retention_days`: syslog lines
    /// and audit entries alike.
    pub pruned_by_age: u64,
    /// Syslog lines removed to stay under `max_rows`.
    pub pruned_by_rows: u64,
}

/// What the pattern rules are holding in memory.
///
/// A rule keeps a little state per device and instance (the throttle window,
/// the lines counted towards a threshold, the alert it has open). The keys
/// come out of text a device wrote, so their number is capped, and what the
/// cap threw away is counted here rather than quietly lost.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleStatus {
    /// (rule, device, instance) keys held right now.
    pub tracked: u64,
    /// Keys forgotten since this process started because one rule reached
    /// its cap. A forgotten key starts counting again from nothing.
    pub forgotten: u64,
    /// The cap, per rule.
    pub max_per_rule: u64,
    /// Matches held back since this process started because one rule had
    /// already spoken for 256 instances of one device inside one throttle
    /// window. Each speaks, with its count, on its next matching line.
    /// Opposite events for alerts this process does not know of are held
    /// back at the same cap, and counted here too.
    pub held_back: u64,
    /// Alerts a rule raised and is waiting to clear — by its opposite event
    /// or its quiet period — as far as this process remembers (a restart
    /// forgets them). An alert whose rule has neither is closed by a person
    /// and is not counted: nothing here would hear of it.
    pub open_alerts: u64,
    /// Calls to the monitoring feature (`alerts.raise`, `alerts.clear`)
    /// dropped since this process started because 4,096 were already
    /// waiting to be made. The signal behind each was stored and published
    /// all the same.
    pub undelivered: u64,
}

/// Where a forwarder sends, and over what.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardKind {
    /// One message per datagram. Nothing says whether it arrived.
    #[default]
    SyslogUdp,
    SyslogTcp,
    /// Syslog over TLS (RFC 5425). The collector's certificate is checked.
    SyslogTls,
    /// JSON posted to an HTTP event collector (HEC). The URL and the token
    /// are a secret and are not part of the document.
    HttpsJson,
}

impl ForwardKind {
    pub fn is_syslog(self) -> bool {
        self != ForwardKind::HttpsJson
    }

    /// Whether a certificate is checked on the way, and so whether
    /// `ca_pem` means anything.
    pub fn is_encrypted(self) -> bool {
        matches!(self, ForwardKind::SyslogTls | ForwardKind::HttpsJson)
    }
}

/// The shape of what is sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwardFormat {
    #[default]
    Rfc5424,
    Json,
}

/// A destination everything stored is handed on to: a SIEM, a log platform.
///
/// Nothing here is a secret. An HTTPS forwarder's URL and token are in the
/// vault and are only ever *sent to* `logs.forward.save`; `has_secret` says
/// whether they are there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Forwarder {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub kind: ForwardKind,
    /// The collector, for the syslog kinds. Empty for `https_json`, whose
    /// destination is the secret.
    pub host: String,
    pub port: u16,
    pub format: ForwardFormat,
    /// `syslog`, `trap`, `event`, `audit`. The audit trail is operator
    /// activity and leaves the machine only when it is named here.
    pub streams: Vec<String>,
    /// Only entries at least this severe (numerically ≤, as syslog counts).
    /// An entry with no severity at all — every trap — is not held back by
    /// it: nothing says it is less severe. Neither is the audit trail,
    /// whose entries all carry the same severity.
    pub min_severity: Option<u8>,
    /// Also send what a lab's simulated devices write and what is recorded
    /// about them. Off unless asked for: a log platform would take a lab's
    /// port-security violation for a real one.
    pub lab: bool,
    /// The certificate authority (PEM) the collector's certificate has to
    /// be signed by, for a collector with a certificate from a private CA.
    /// Absent: the authorities this machine trusts.
    pub ca_pem: Option<String>,
    pub has_secret: bool,
    pub updated_at: i64,
}

/// What one forwarder has done since the process started.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderStatus {
    pub id: String,
    pub name: String,
    /// `disabled`, `idle` (on, nothing sent or tried yet), `ok` (the last
    /// send worked) or `failing` (it did not; `last_error` says how).
    pub state: String,
    /// Entries handed to the network. No syslog transport acknowledges
    /// anything, so for those this is not "arrived"; for `https_json` it is
    /// what the collector answered 2xx to.
    pub sent: u64,
    /// Entries that never left: the queue was full, or every attempt to
    /// send them failed, or the forwarder was changed while they waited.
    pub dropped: u64,
    /// Why the last attempt failed, in words that never include an HTTPS
    /// forwarder's URL or token. Cleared by the next send that works.
    pub last_error: Option<String>,
    /// When something was last sent.
    pub last_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LogsStatus {
    pub syslog: ReceiverStatus,
    pub traps: TrapStatus,
    pub flows: FlowStatus,
    pub rules: RuleStatus,
    /// Every forwarder, by name.
    pub forwarders: Vec<ForwarderStatus>,
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
    /// The platform whose messages the pattern was written for, in words
    /// (`Cisco IOS / IOS-XE`). A built-in rule covers that platform and no
    /// other; empty for a rule of the user's own.
    pub platform: String,
    /// Which stream the rule reads: `syslog` or `trap`.
    pub stream: String,
    /// A regular expression over the line as the device wrote it — for a
    /// trap, over the message the receiver wrote for it (`linkDown
    /// ifIndex=3 …`). Named groups are available to the signal's title
    /// template and are reported with the signal.
    pub pattern: String,
    /// Only lines at least this severe (numerically ≤, as syslog counts).
    pub min_severity: Option<u8>,
    pub scope: Scope,
    /// Raise only when this many matching lines arrive inside a window.
    pub threshold: Option<Threshold>,
    /// The id of another rule: this one only counts a line while that
    /// rule's alert is open for the same device and instance ("a login
    /// that succeeds straight after a burst of failures").
    pub after: Option<String>,
    /// The opposite event. A line matching this clears the alert the rule
    /// raised for the same device and instance.
    pub clear_pattern: Option<String>,
    /// Clear the alert after this long without a matching line.
    pub clear_after_s: Option<u32>,
    pub emit: Signal,
    /// At most one signal per device and instance per rule per window; the
    /// rest are counted.
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
            platform: String::new(),
            stream: crate::db::STREAM.to_string(),
            pattern: String::new(),
            min_severity: None,
            scope: Scope::default(),
            threshold: None,
            after: None,
            clear_pattern: None,
            clear_after_s: None,
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

/// "Eight failed logins from one address in a minute, not one."
///
/// The lines are counted per device and per value of the `group_by`
/// captures. When `count` of them fall inside `window_s` the rule raises
/// its signal and starts counting again from nothing. The counts live in
/// memory only: a restart starts them again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Threshold {
    pub count: u32,
    pub window_s: u32,
    /// Named groups of the pattern. Their values together are the rule's
    /// instance: what is counted, throttled and alerted on separately.
    #[serde(default)]
    pub group_by: Vec<String>,
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
    /// What the rule counts this line under: the interface, or the values
    /// of the threshold's `group_by` captures. Empty = the whole device.
    pub instance: String,
    /// What else has to be true before the signal is raised, in a sentence:
    /// a rule with a threshold wants several such lines, a rule that follows
    /// another wants that one's alert open. Empty when the line is enough.
    pub note: String,
    /// The line is the rule's opposite event: it raises nothing and clears
    /// the alert of that instance.
    pub clears: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignalPreview {
    pub kind: String,
    pub title: String,
    pub severity: Option<String>,
    pub interface: Option<String>,
}
