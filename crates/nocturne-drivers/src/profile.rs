//! Device profiles: a platform described as data.
//!
//! A profile is a YAML document that tells Nocturne how to talk to one kind
//! of device — what its prompts look like, how to turn paging off, how to
//! enter configuration mode and save, which commands are safe, and how to map
//! its `show` output onto the common [`model`](crate::model). Supporting a new
//! device type means writing (or asking the AI to draft) one of these files;
//! no code changes, no rebuild.
//!
//! Profiles can `extends:` another profile; the child is merged over the
//! parent key by key (maps merge, lists and scalars replace). Unknown keys are
//! rejected so that a typo is reported instead of silently ignored.
//!
//! Loading a profile checks everything that can be checked without a device:
//! every pattern and expression compiles, every template exists, every field a
//! collector fills is a field of the model, and every command a collector
//! runs is read-only under the profile's own policy. All problems are
//! reported together, each with the place in the file it came from.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use indexmap::IndexMap;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::Value as Yaml;

use crate::expr::Expressions;
use crate::hier::Syntax;
use crate::model::Kind;
use crate::parse;
use crate::policy::Policy;

static BUILTIN: include_dir::Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/profiles");

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    /// Unique key, e.g. `cisco_ios`. Ids starting with `_` are abstract bases.
    pub id: String,
    pub name: String,
    pub vendor: String,
    /// Operating system family reported in facts ("IOS", "RouterOS", …).
    pub os: String,
    pub description: String,
    pub extends: Option<String>,
    /// Platform key in the ntc-templates index, for `parser: {ntc: auto}`.
    pub ntc_platform: Option<String>,
    /// How far this profile has been checked against the real platform.
    /// Never inherited through `extends`: every profile states its own.
    pub maturity: Maturity,
    /// Kinds this profile collects with a parser that has not yet met output
    /// recorded from a real device of this platform. It says so about one
    /// read without taking the word `verified` — and with it write access —
    /// from a profile that has earned it for everything else. Inherited like
    /// any list: a child that inherits the steps inherits the caveat.
    pub unverified: Vec<Kind>,
    /// How interface names in collected data are spelled.
    pub interface_names: InterfaceNames,
    pub detect: Detect,
    pub cli: Cli,
    pub config: ConfigSpec,
    pub policy: PolicySpec,
    pub collect: IndexMap<Kind, Vec<CollectStep>>,
    pub snmp: SnmpSpec,
}

impl Profile {
    /// Whether what this profile reads for `kind` has yet to be checked
    /// against a real device (`unverified: [<kind>]`).
    pub fn is_unverified(&self, kind: Kind) -> bool {
        self.unverified.contains(&kind)
    }
}

/// How much a profile can be trusted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Maturity {
    /// Commands, prompts and parsers are checked against recorded output of
    /// the real platform by this repository's tests.
    Verified,
    /// Written from vendor documentation and other tools' definitions; not
    /// yet checked against a device.
    Community,
    /// A draft.
    #[default]
    Experimental,
}

impl Maturity {
    pub fn as_str(self) -> &'static str {
        match self {
            Maturity::Verified => "verified",
            Maturity::Community => "community",
            Maturity::Experimental => "experimental",
        }
    }
}

/// What the collector does with interface names.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceNames {
    /// Expand Cisco-style abbreviations to the long form (`Gi1/0/1` →
    /// `GigabitEthernet1/0/1`), so that names from different commands agree.
    #[default]
    Cisco,
    /// Keep names exactly as the device prints them. For platforms whose
    /// names are already unique and would be mangled by the Cisco rules
    /// (`vlan10` on RouterOS, `eth0` and `lo` on Linux).
    Verbatim,
}

/// How to recognise the platform.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Detect {
    /// Patterns for the SSH identification string, login banner or first screen.
    pub banner: Vec<String>,
    /// Patterns for SNMP `sysDescr`.
    pub sysdescr: Vec<String>,
    /// `sysObjectID` prefixes, e.g. `1.3.6.1.4.1.9.`.
    pub sysobjectid: Vec<String>,
    /// Commands to try on an unidentified session, with the pattern their output must match.
    pub probes: Vec<Probe>,
    /// Higher wins when several profiles match.
    pub priority: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Probe {
    pub command: String,
    pub matches: String,
}

/// Interactive session behaviour.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cli {
    /// Matches a complete prompt line. A named group `host` lets the driver
    /// pin the prompt to this device's hostname.
    pub prompt: String,
    /// Matches the prompt of the unprivileged mode (needs `enable`).
    pub user_prompt: Option<String>,
    /// Matches the prompt inside configuration mode.
    pub config_prompt: Option<String>,
    pub enable: Option<Enable>,
    pub login: Login,
    /// Sent once after login (turn paging off, widen the terminal).
    pub init: Vec<String>,
    pub pager: Option<Pager>,
    /// What "Enter" is on this platform.
    pub newline: String,
    pub exit: String,
    /// Output patterns that mean an exec command was rejected.
    pub errors: Vec<String>,
    /// Interactive questions to answer automatically in exec mode.
    pub answers: Vec<Answer>,
    /// Lines the device prints by itself at any moment (log messages on a
    /// console line). When they follow the prompt, the prompt still counts.
    pub unsolicited: Vec<String>,
    /// Seconds without output before a command is considered hung.
    pub timeout_s: u64,
}

impl Default for Cli {
    fn default() -> Self {
        Cli {
            prompt: r"^(?P<host>[\w.\-:/@~\[\]]{1,64})\s?[>#$%]\s?$".into(),
            user_prompt: None,
            config_prompt: None,
            enable: None,
            login: Login::default(),
            init: Vec::new(),
            pager: None,
            newline: "\n".into(),
            exit: "exit".into(),
            errors: Vec::new(),
            answers: Vec::new(),
            unsolicited: Vec::new(),
            timeout_s: 30,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Enable {
    pub command: String,
    pub password_prompt: String,
}

impl Default for Enable {
    fn default() -> Self {
        Enable {
            command: "enable".into(),
            password_prompt: r"(?i)password:\s*$".into(),
        }
    }
}

/// Prompts seen when the transport does not authenticate for us (Telnet, console).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Login {
    pub username_prompt: String,
    pub password_prompt: String,
    pub failure: String,
    /// Appended to the username (RouterOS uses `+ct511w4098h` to disable colours and paging).
    pub username_suffix: String,
}

impl Default for Login {
    fn default() -> Self {
        Login {
            username_prompt: r"(?i)(?:user ?name|login|user):\s*$".into(),
            password_prompt: r"(?i)password:\s*$".into(),
            failure: r"(?i)(?:authentication fail|login (?:invalid|incorrect|failed)|access denied|bad passwords?|permission denied)"
                .into(),
            username_suffix: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Pager {
    /// Matches the "more" prompt on the line being drawn.
    pub pattern: String,
    /// What to send to continue.
    pub key: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Answer {
    pub prompt: String,
    pub answer: String,
}

/// How configuration is read and written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ConfigSpec {
    pub syntax: Syntax,
    /// Commands that enter configuration mode. Empty = the platform has no config mode.
    pub enter: Vec<String>,
    pub exit: Vec<String>,
    /// Commands that activate a candidate configuration (Junos `commit`).
    pub commit: Vec<String>,
    /// Commands that discard a candidate configuration after a failure.
    pub abort: Vec<String>,
    /// Commands that persist the running configuration.
    pub save: Vec<String>,
    /// Command that prints the running configuration.
    pub running: String,
    pub startup: Option<String>,
    /// Lines to drop from a fetched configuration (timestamps, byte counts)
    /// so that two fetches of an unchanged device compare equal.
    pub ignore: Vec<String>,
    /// Output patterns that mean a configuration line was rejected.
    pub errors: Vec<String>,
    pub answers: Vec<Answer>,
    pub guard: Option<Guard>,
    /// Extra secret patterns for redaction (named group `secret`).
    pub secrets: Vec<String>,
}

impl Default for ConfigSpec {
    fn default() -> Self {
        ConfigSpec {
            syntax: Syntax::Flat,
            enter: Vec::new(),
            exit: Vec::new(),
            commit: Vec::new(),
            abort: Vec::new(),
            save: Vec::new(),
            running: String::new(),
            startup: None,
            ignore: Vec::new(),
            errors: Vec::new(),
            answers: Vec::new(),
            guard: None,
            secrets: Vec::new(),
        }
    }
}

/// A dead-man switch: something the device will do by itself unless told
/// not to, so a change that cuts management access undoes itself.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Guard {
    /// Human name of the mechanism ("reload timer", "commit confirmed").
    pub name: String,
    /// Exec commands that arm it; `{minutes}` is substituted.
    pub arm: Vec<String>,
    /// Exec commands that cancel it once the change is verified.
    pub disarm: Vec<String>,
    /// What happens if it fires, in words shown to the user.
    pub effect: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PolicySpec {
    pub read_only: Vec<String>,
    pub dangerous: Vec<String>,
    /// The CLI is a POSIX shell: `|`, `&&`, `||` and `&` chain commands that
    /// are each classified, and a redirection or substitution is never
    /// read-only. See [`Policy::shell`].
    pub shell: bool,
}

/// One command to run and how to turn its output into model rows.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CollectStep {
    pub command: String,
    pub parser: Parser,
    /// Model field → expression over the parsed row.
    pub map: IndexMap<String, String>,
    /// Keep a row only when this expression is truthy.
    pub when: Option<String>,
    /// A failing optional step is skipped instead of failing the collection.
    pub optional: bool,
    /// Merge this step's rows into earlier rows that have the same value in this model field.
    pub merge_on: Option<String>,
}

/// Written in YAML as a bare word (`kv`, `colon`, `raw`) or a one-key map
/// (`{ntc: cisco_ios_show_version}`, `{regex: '…'}`, `{json: interfaces.*}`).
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Parser {
    /// A bundled ntc-templates template by name, or `auto` to resolve the command through the index.
    Ntc(String),
    /// A TextFSM template written inline.
    Textfsm(String),
    /// A regular expression; each match is a row of its named groups.
    Regex(String),
    /// RouterOS-style `key=value` listing.
    Kv,
    /// `key: value` property sheet → one row.
    Colon,
    /// JSON output; the value is the path to the rows (`interfaces.*`).
    Json(String),
    /// The whole output as one row with a single `output` field.
    #[default]
    Raw,
}

impl Serialize for Parser {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let (key, value) = match self {
            Parser::Kv => return serializer.serialize_str("kv"),
            Parser::Colon => return serializer.serialize_str("colon"),
            Parser::Raw => return serializer.serialize_str("raw"),
            Parser::Ntc(name) => ("ntc", name),
            Parser::Textfsm(template) => ("textfsm", template),
            Parser::Regex(pattern) => ("regex", pattern),
            Parser::Json(path) => ("json", path),
        };
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(key, value)?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Parser {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Word(String),
            Map(IndexMap<String, String>),
        }
        const HELP: &str = "expected `kv`, `colon`, `raw`, or a one-key map with `ntc`, `textfsm`, `regex` or `json`";
        match Written::deserialize(deserializer)? {
            Written::Word(word) => match word.as_str() {
                "kv" => Ok(Parser::Kv),
                "colon" => Ok(Parser::Colon),
                "raw" => Ok(Parser::Raw),
                "json" => Ok(Parser::Json(String::new())),
                other => Err(D::Error::custom(format!(
                    "unknown parser `{other}`: {HELP}"
                ))),
            },
            Written::Map(map) => {
                let mut entries = map.into_iter();
                match (entries.next(), entries.next()) {
                    (Some((key, value)), None) => match key.as_str() {
                        "ntc" => Ok(Parser::Ntc(value)),
                        "textfsm" => Ok(Parser::Textfsm(value)),
                        "regex" => Ok(Parser::Regex(value)),
                        "json" => Ok(Parser::Json(value)),
                        other => Err(D::Error::custom(format!(
                            "unknown parser `{other}`: {HELP}"
                        ))),
                    },
                    _ => Err(D::Error::custom(format!(
                        "a parser takes exactly one key: {HELP}"
                    ))),
                }
            }
        }
    }
}

/// Where to find health data over SNMP.
///
/// Nothing is scaled, turned round or filtered unless the entry says so: an
/// object that is idle rather than busy, free rather than used, in tenths, or
/// one kind of row among several must say so or be left out.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SnmpSpec {
    /// A CPU percentage, busy. The first entry that gives a number is the
    /// reading, its rows combined by its `combine`; the entries after it are
    /// not read.
    pub cpu: Vec<SnmpEntry>,
    /// Memory quantities, in any unit as long as the three agree. Per list,
    /// the first number of the first entry that gives one. The figure is
    /// used ÷ total, else used ÷ (used + free), else (total − free) ÷ total.
    pub memory_used: Vec<SnmpEntry>,
    pub memory_free: Vec<SnmpEntry>,
    pub memory_total: Vec<SnmpEntry>,
    /// An object that is already "percent of memory in use", for when the
    /// three lists above give no figure.
    pub memory_percent: Vec<SnmpEntry>,
    /// Temperatures in °C. Every entry is read and every row it gives is a
    /// sensor, so two entries are two sensors; an entry marked `fallback` is
    /// read only where the entries above it gave none.
    pub temperature: Vec<SnmpEntry>,
}

impl SnmpSpec {
    /// The lists by name, in the order a profile writes them.
    pub fn lists(&self) -> [(&'static str, &[SnmpEntry]); 6] {
        [
            ("cpu", &self.cpu),
            ("memory_used", &self.memory_used),
            ("memory_free", &self.memory_free),
            ("memory_total", &self.memory_total),
            ("memory_percent", &self.memory_percent),
            ("temperature", &self.temperature),
        ]
    }
}

/// One place a health reading is taken from.
///
/// Written as a bare OID (`1.3.6.1.2.1.25.3.3.1.2`), meaning what a bare OID
/// has always meant, or as a mapping that says which instance, which rows and
/// what arithmetic. See `docs/profiles.md`, section `snmp`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SnmpEntry {
    /// Text ending in `.0` is one object, fetched with GET; anything else is
    /// a column, walked, and every row counts.
    pub oid: Option<String>,
    /// Exactly this OID, fetched with GET whatever it ends in. An agent that
    /// does not have it gives no reading, which is not an error.
    pub instance: Option<String>,
    /// The agent sends a signed 32-bit object in an unsigned type: a
    /// Counter32, Gauge32 or TimeTicks value of 2^31 or more is the negative
    /// number value − 2^32. Applied to the raw value, before `absent`.
    pub signed: bool,
    /// `where`: tests on sibling columns of `oid`, matched by row index. A
    /// row counts only if every test holds; a row with a test cell missing
    /// does not count.
    pub tests: Vec<SnmpTest>,
    /// Raw values the MIB defines as "no reading"; such a row is dropped
    /// before any arithmetic.
    pub absent: Vec<f64>,
    /// A sibling column holding an RFC 3433 `EntitySensorDataScale`.
    pub si_scale: Option<String>,
    /// A sibling column holding an RFC 3433 `EntitySensorPrecision`.
    pub precision: Option<String>,
    /// Multiply by this: `0.1` for tenths.
    pub scale: Option<f64>,
    /// The reading is this number minus the value: `100` for a percentage
    /// that is idle or free.
    pub complement: Option<f64>,
    /// `cpu` only: how the rows of one entry make one figure.
    pub combine: Option<Combine>,
    /// `temperature` only: the sensor's name in the series.
    pub label: Option<String>,
    /// `temperature` only: read this entry only when none of the entries
    /// above it that are not fallbacks gave a sensor. For an older object
    /// that reports the same sensors as a newer one: listed plainly beside
    /// it, every sensor would be counted twice.
    pub fallback: bool,
    /// Written as a bare string. A bare string is checked no further than it
    /// always was (one that does not parse is skipped at poll time), so a
    /// stored profile that loaded before still loads.
    bare: bool,
}

impl SnmpEntry {
    /// An entry written as a bare OID string.
    pub fn bare(oid: &str) -> SnmpEntry {
        SnmpEntry {
            oid: Some(oid.to_string()),
            bare: true,
            ..SnmpEntry::default()
        }
    }

    /// The OID as written: the instance, or the object or column.
    pub fn target(&self) -> Option<&str> {
        self.instance.as_deref().or(self.oid.as_deref())
    }

    /// Whether the entry is a column, walked row by row. An `oid` that ends
    /// in `.0` is a scalar; an `instance` is one object whatever it ends in.
    pub fn is_column(&self) -> bool {
        self.instance.is_none()
            && self
                .oid
                .as_deref()
                .is_some_and(|oid| !oid.trim().ends_with(".0"))
    }

    /// Whether it says nothing a bare string could not.
    fn is_plain(&self) -> bool {
        self.instance.is_none()
            && !self.signed
            && self.tests.is_empty()
            && self.absent.is_empty()
            && self.si_scale.is_none()
            && self.precision.is_none()
            && self.scale.is_none()
            && self.complement.is_none()
            && self.combine.is_none()
            && self.label.is_none()
            && !self.fallback
    }
}

/// How the rows of one `cpu` entry make one figure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combine {
    /// The rows are cores of one machine.
    #[default]
    Mean,
    /// The rows are separate processors (stack members, route processors,
    /// line cards): the busiest is the figure.
    Max,
}

/// A test on a sibling column, for one row: exactly one of `is`, `in` and
/// `matches`. Values are compared as text with the cell's plain rendering:
/// integers in decimal, object identifiers dotted with no leading dot,
/// strings trimmed.
#[derive(Clone, Debug, Default)]
pub struct SnmpTest {
    pub oid: String,
    pub is: Option<String>,
    pub any_of: Option<Vec<String>>,
    pub matches: Option<String>,
    /// `matches`, compiled once when the profile is loaded rather than once
    /// per row of every poll.
    pattern: Option<Regex>,
}

impl PartialEq for SnmpTest {
    fn eq(&self, other: &SnmpTest) -> bool {
        (&self.oid, &self.is, &self.any_of, &self.matches)
            == (&other.oid, &other.is, &other.any_of, &other.matches)
    }
}

impl SnmpTest {
    /// Whether a cell, in its plain rendering, passes the test.
    pub fn holds(&self, text: &str) -> bool {
        if let Some(expected) = &self.is {
            return text == expected;
        }
        if let Some(expected) = &self.any_of {
            return expected.iter().any(|value| value == text);
        }
        match (&self.pattern, &self.matches) {
            (Some(pattern), _) => pattern.is_match(text),
            // Not loaded through a profile, so not compiled yet.
            (None, Some(pattern)) => Regex::new(pattern).is_ok_and(|regex| regex.is_match(text)),
            (None, None) => false,
        }
    }
}

const ENTRY_KEYS: &str = "`oid`, `instance`, `signed`, `where`, `absent`, `si_scale`, `precision`, `scale`, `complement`, `combine`, `label`, `fallback`";
const TEST_KEYS: &str = "`oid` and one of `is`, `in`, `matches`";

impl<'de> Deserialize<'de> for SnmpEntry {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = SnmpEntry;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an OID, or a mapping with `oid` or `instance`")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<SnmpEntry, E> {
                Ok(SnmpEntry::bare(text))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<SnmpEntry, A::Error> {
                use serde::de::Error;
                let mut entry = SnmpEntry::default();
                // By hand rather than derived, so that a mistyped key is
                // named instead of an untagged enum's "did not match any
                // variant".
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "oid" => entry.oid = Some(map.next_value()?),
                        "instance" => entry.instance = Some(map.next_value()?),
                        "signed" => entry.signed = map.next_value()?,
                        "where" => entry.tests = map.next_value()?,
                        "absent" => entry.absent = map.next_value()?,
                        "si_scale" => entry.si_scale = Some(map.next_value()?),
                        "precision" => entry.precision = Some(map.next_value()?),
                        "scale" => entry.scale = Some(map.next_value()?),
                        "complement" => entry.complement = Some(map.next_value()?),
                        "combine" => entry.combine = Some(map.next_value()?),
                        "label" => entry.label = Some(map.next_value()?),
                        "fallback" => entry.fallback = map.next_value()?,
                        other => {
                            return Err(A::Error::custom(format!(
                                "an SNMP entry has no key `{other}`; it takes {ENTRY_KEYS}"
                            )));
                        }
                    }
                }
                Ok(entry)
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl Serialize for SnmpEntry {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        // What was a bare string goes back as one, so a profile written
        // before entries could be mappings reads back exactly as it was.
        // Decided by what the entry says, not by how it was written: a key
        // set on an entry made with `bare` is not dropped on the way out.
        if let (Some(oid), true) = (&self.oid, self.is_plain()) {
            return serializer.serialize_str(oid);
        }
        let mut map = serializer.serialize_map(None)?;
        if let Some(oid) = &self.oid {
            map.serialize_entry("oid", oid)?;
        }
        if let Some(instance) = &self.instance {
            map.serialize_entry("instance", instance)?;
        }
        if self.signed {
            map.serialize_entry("signed", &true)?;
        }
        if !self.tests.is_empty() {
            map.serialize_entry("where", &self.tests)?;
        }
        if !self.absent.is_empty() {
            map.serialize_entry("absent", &self.absent)?;
        }
        if let Some(column) = &self.si_scale {
            map.serialize_entry("si_scale", column)?;
        }
        if let Some(column) = &self.precision {
            map.serialize_entry("precision", column)?;
        }
        if let Some(scale) = &self.scale {
            map.serialize_entry("scale", scale)?;
        }
        if let Some(complement) = &self.complement {
            map.serialize_entry("complement", complement)?;
        }
        if let Some(combine) = &self.combine {
            map.serialize_entry("combine", combine)?;
        }
        if let Some(label) = &self.label {
            map.serialize_entry("label", label)?;
        }
        if self.fallback {
            map.serialize_entry("fallback", &true)?;
        }
        map.end()
    }
}

/// A value a test compares with: a YAML string or number, as text.
struct TestValue(String);

impl<'de> Deserialize<'de> for TestValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = TestValue;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a number, an OID or a string")
            }

            fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<TestValue, E> {
                let text = text.trim();
                // An OID cell renders without a leading dot, so one written
                // with it would otherwise never match.
                let dotted = text
                    .strip_prefix('.')
                    .filter(|rest| oid_problem(rest).is_none());
                Ok(TestValue(dotted.unwrap_or(text).to_string()))
            }

            fn visit_i64<E: serde::de::Error>(self, number: i64) -> Result<TestValue, E> {
                Ok(TestValue(number.to_string()))
            }

            fn visit_u64<E: serde::de::Error>(self, number: u64) -> Result<TestValue, E> {
                Ok(TestValue(number.to_string()))
            }

            fn visit_f64<E: serde::de::Error>(self, number: f64) -> Result<TestValue, E> {
                Ok(TestValue(number.to_string()))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl<'de> Deserialize<'de> for SnmpTest {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = SnmpTest;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(f, "a mapping with {TEST_KEYS}")
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<SnmpTest, A::Error> {
                use serde::de::Error;
                let mut test = SnmpTest::default();
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "oid" => test.oid = map.next_value()?,
                        "is" => test.is = Some(map.next_value::<TestValue>()?.0),
                        "in" => {
                            let values: Vec<TestValue> = map.next_value()?;
                            test.any_of = Some(values.into_iter().map(|value| value.0).collect());
                        }
                        "matches" => test.matches = Some(map.next_value()?),
                        other => {
                            return Err(A::Error::custom(format!(
                                "a `where` test has no key `{other}`; it takes {TEST_KEYS}"
                            )));
                        }
                    }
                }
                Ok(test)
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

impl Serialize for SnmpTest {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("oid", &self.oid)?;
        if let Some(value) = &self.is {
            map.serialize_entry("is", value)?;
        }
        if let Some(values) = &self.any_of {
            map.serialize_entry("in", values)?;
        }
        if let Some(pattern) = &self.matches {
            map.serialize_entry("matches", pattern)?;
        }
        map.end()
    }
}

/// Why `text` cannot be sent as an OID, if it cannot: dotted decimal, at
/// least two arcs, and the first two arcs within what BER can encode.
fn oid_problem(text: &str) -> Option<String> {
    let dotted = text.trim();
    let dotted = dotted.strip_prefix('.').unwrap_or(dotted);
    let arcs: Option<Vec<u32>> = dotted
        .split('.')
        .map(|arc| {
            arc.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| arc.parse().ok())
                .flatten()
        })
        .collect();
    let reason = match arcs.as_deref() {
        None => "write it as dotted decimal numbers, such as 1.3.6.1.2.1.25.3.3.1.2",
        Some([] | [_]) => "an OID has at least two arcs",
        Some([first, ..]) if *first > 2 => "the first arc must be 0, 1 or 2",
        Some([first, second, ..]) if *first < 2 && *second > 39 => {
            "the second arc must be below 40"
        }
        Some(_) => return None,
    };
    Some(format!("`{text}` is not an OID: {reason}"))
}

/// Compile the `matches` of every `where` test (each was checked by
/// [`check_snmp`] already).
fn compile_snmp_patterns(snmp: &mut SnmpSpec) {
    for list in [
        &mut snmp.cpu,
        &mut snmp.memory_used,
        &mut snmp.memory_free,
        &mut snmp.memory_total,
        &mut snmp.memory_percent,
        &mut snmp.temperature,
    ] {
        for test in list.iter_mut().flat_map(|entry| entry.tests.iter_mut()) {
            test.pattern = test
                .matches
                .as_deref()
                .and_then(|pattern| Regex::new(pattern).ok());
        }
    }
}

/// Everything about the `snmp` lists that can be wrong without an agent to
/// ask, each named `snmp.<list>[<n>]`. Bare strings are left alone.
fn check_snmp(problems: &mut Vec<String>, snmp: &SnmpSpec) {
    for (list, entries) in snmp.lists() {
        let mut labels: Vec<(&str, usize)> = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if entry.bare {
                continue;
            }
            let at = format!("snmp.{list}[{index}]");
            let mut problem = |text: String| problems.push(format!("{at}: {text}"));
            match (&entry.oid, &entry.instance) {
                (Some(_), Some(_)) => problem(
                    "give `oid` or `instance`, not both: `instance` is one object fetched as it is written, `oid` an object or a column".into(),
                ),
                (None, None) => problem("needs an `oid` or an `instance`".into()),
                _ => {}
            }
            for (key, text) in [("oid", &entry.oid), ("instance", &entry.instance)] {
                if let Some(reason) = text.as_deref().and_then(oid_problem) {
                    problem(format!("{key}: {reason}"));
                }
            }
            if !entry.is_column() {
                let given: Vec<&str> = [
                    ("where", !entry.tests.is_empty()),
                    ("si_scale", entry.si_scale.is_some()),
                    ("precision", entry.precision.is_some()),
                    ("combine", entry.combine.is_some()),
                ]
                .into_iter()
                .filter_map(|(key, set)| set.then_some(key))
                .collect();
                if !given.is_empty() && (entry.oid.is_some() || entry.instance.is_some()) {
                    problem(format!(
                        "{} works on the rows of a column, and this entry is one object; give a column `oid` (one that does not end in `.0`)",
                        given
                            .iter()
                            .map(|key| format!("`{key}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            for (key, column) in [
                ("si_scale", &entry.si_scale),
                ("precision", &entry.precision),
            ] {
                if let Some(reason) = column.as_deref().and_then(oid_problem) {
                    problem(format!("{key}: {reason}"));
                }
            }
            if entry.fallback {
                if list != "temperature" {
                    problem(
                        "`fallback` is for `temperature` only: the other lists take the first entry that answers already".into(),
                    );
                } else if entries[..index].iter().all(|above| above.fallback) {
                    problem(
                        "`fallback` needs an entry above it that is not one, to fall back from"
                            .into(),
                    );
                }
            }
            if entry.combine.is_some() && list != "cpu" {
                problem("`combine` is for `cpu` only".into());
            }
            if let Some(label) = &entry.label {
                if list != "temperature" {
                    problem("`label` is for `temperature` only".into());
                } else if label.trim().is_empty() || label.trim() != label {
                    problem("`label` must be a name with no space at either end".into());
                } else if let Some((_, first)) =
                    labels.iter().find(|(seen, _)| *seen == label.as_str())
                {
                    problem(format!(
                        "the label `{label}` is already the label of snmp.{list}[{first}]; two sensors cannot share a name"
                    ));
                } else {
                    labels.push((label.as_str(), index));
                }
            }
            if let Some(scale) = entry.scale
                && (!scale.is_finite() || scale == 0.0)
            {
                problem("`scale` must be a number other than zero".into());
            }
            if entry
                .complement
                .is_some_and(|complement| !complement.is_finite())
            {
                problem("`complement` must be a number".into());
            }
            if entry.absent.iter().any(|value| !value.is_finite()) {
                problem("`absent` must list numbers".into());
            }
            for (number, test) in entry.tests.iter().enumerate() {
                let at = format!("where[{number}]");
                if let Some(reason) = oid_problem(&test.oid) {
                    problem(format!("{at}.oid: {reason}"));
                }
                let given = [
                    test.is.is_some(),
                    test.any_of.is_some(),
                    test.matches.is_some(),
                ]
                .into_iter()
                .filter(|set| *set)
                .count();
                if given != 1 {
                    problem(format!(
                        "{at}: give exactly one of `is`, `in` and `matches`"
                    ));
                }
                if test.any_of.as_ref().is_some_and(Vec::is_empty) {
                    problem(format!("{at}.in: an empty list matches no row"));
                }
                if let Some(pattern) = &test.matches
                    && let Err(err) = Regex::new(pattern)
                {
                    problem(format!(
                        "{at}.matches: invalid regular expression `{pattern}` ({})",
                        first_line(&err.to_string())
                    ));
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Builtin,
    User,
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
#[error("profile `{profile}`: {}", .problems.join("; "))]
pub struct ProfileError {
    pub profile: String,
    pub problems: Vec<String>,
}

/// A profile with everything compiled and checked.
#[allow(dead_code)] // some compiled fields are only read by the session driver
pub struct Platform {
    pub profile: Profile,
    pub origin: Origin,
    /// The YAML this platform was loaded from (before inheritance).
    pub source: String,
    pub policy: Policy,
    pub(crate) prompt: Regex,
    pub(crate) user_prompt: Option<Regex>,
    pub(crate) config_prompt: Option<Regex>,
    pub(crate) enable_password: Option<Regex>,
    pub(crate) username_prompt: Regex,
    pub(crate) password_prompt: Regex,
    pub(crate) login_failure: Regex,
    pub(crate) pager: Option<Regex>,
    pub(crate) exec_errors: Vec<Regex>,
    pub(crate) config_errors: Vec<Regex>,
    pub(crate) exec_answers: Vec<(Regex, String)>,
    pub(crate) config_answers: Vec<(Regex, String)>,
    pub(crate) config_ignore: Vec<Regex>,
    pub(crate) unsolicited: Vec<Regex>,
    pub(crate) banner: Vec<Regex>,
    pub(crate) sysdescr: Vec<Regex>,
    /// One pattern per entry of `detect.probes`, in the same order.
    pub(crate) probes: Vec<Regex>,
}

impl std::fmt::Debug for Platform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Platform")
            .field("id", &self.profile.id)
            .field("origin", &self.origin)
            .finish()
    }
}

impl Platform {
    pub fn id(&self) -> &str {
        &self.profile.id
    }

    pub fn is_abstract(&self) -> bool {
        self.profile.id.starts_with('_')
    }

    /// The compiled command policy.
    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Whether a line is one of this platform's prompts, in any mode.
    pub fn is_prompt(&self, line: &str) -> bool {
        self.prompt.is_match(line)
    }

    /// Whether a line is something the device prints unasked (`cli.unsolicited`).
    pub fn is_unsolicited(&self, line: &str) -> bool {
        self.unsolicited
            .iter()
            .any(|pattern| pattern.is_match(line))
    }

    /// The hostname a prompt line shows (the `host` group of `cli.prompt`).
    pub fn prompt_host<'a>(&self, line: &'a str) -> Option<&'a str> {
        self.prompt
            .captures(line)
            .and_then(|caps| caps.name("host"))
            .map(|host| host.as_str())
    }

    /// Whether a prompt line belongs to configuration mode.
    pub fn is_config_prompt(&self, line: &str) -> bool {
        self.config_prompt
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(line))
    }

    pub fn is_user_prompt(&self, line: &str) -> bool {
        !self.is_config_prompt(line)
            && self
                .user_prompt
                .as_ref()
                .is_some_and(|pattern| pattern.is_match(line))
    }

    /// The first line of `output` that matches one of the platform's error patterns.
    pub fn find_error<'a>(&self, output: &'a str, config_mode: bool) -> Option<&'a str> {
        let patterns = if config_mode && !self.config_errors.is_empty() {
            &self.config_errors
        } else {
            &self.exec_errors
        };
        output
            .lines()
            .find(|line| patterns.iter().any(|pattern| pattern.is_match(line)))
            .map(str::trim)
    }

    /// Drop volatile lines from a fetched configuration.
    pub fn clean_config(&self, text: &str) -> String {
        let mut cleaned = String::with_capacity(text.len());
        for line in text.lines() {
            if self
                .config_ignore
                .iter()
                .any(|pattern| pattern.is_match(line))
            {
                continue;
            }
            cleaned.push_str(line.trim_end());
            cleaned.push('\n');
        }
        // Leading/trailing blank lines are display artefacts.
        cleaned.trim_matches('\n').to_string() + "\n"
    }

    fn compile(
        profile: Profile,
        origin: Origin,
        source: String,
        expressions: &Expressions,
    ) -> Result<Platform, ProfileError> {
        let mut problems: Vec<String> = Vec::new();
        let fallback = || Regex::new("$^").expect("never-matching pattern");
        let cli = &profile.cli;

        let prompt = compile_one(&mut problems, "cli.prompt", &cli.prompt);
        let user_prompt = cli
            .user_prompt
            .as_deref()
            .and_then(|pattern| compile_one(&mut problems, "cli.user_prompt", pattern));
        let config_prompt = cli
            .config_prompt
            .as_deref()
            .and_then(|pattern| compile_one(&mut problems, "cli.config_prompt", pattern));
        let enable_password = cli.enable.as_ref().and_then(|enable| {
            compile_one(
                &mut problems,
                "cli.enable.password_prompt",
                &enable.password_prompt,
            )
        });
        let username_prompt = compile_one(
            &mut problems,
            "cli.login.username_prompt",
            &cli.login.username_prompt,
        );
        let password_prompt = compile_one(
            &mut problems,
            "cli.login.password_prompt",
            &cli.login.password_prompt,
        );
        let login_failure = compile_one(&mut problems, "cli.login.failure", &cli.login.failure);
        let pager = cli
            .pager
            .as_ref()
            .and_then(|pager| compile_one(&mut problems, "cli.pager.pattern", &pager.pattern));
        let exec_errors = compile_list(&mut problems, "cli.errors", &cli.errors);
        let config_errors = compile_list(&mut problems, "config.errors", &profile.config.errors);
        let config_ignore = compile_list(&mut problems, "config.ignore", &profile.config.ignore);
        let unsolicited = compile_list(&mut problems, "cli.unsolicited", &cli.unsolicited);
        let banner = compile_list(&mut problems, "detect.banner", &profile.detect.banner);
        let sysdescr = compile_list(&mut problems, "detect.sysdescr", &profile.detect.sysdescr);
        let exec_answers = compile_answers(&mut problems, "cli.answers", &cli.answers);
        let config_answers =
            compile_answers(&mut problems, "config.answers", &profile.config.answers);
        let probes: Vec<Regex> = profile
            .detect
            .probes
            .iter()
            .enumerate()
            .filter_map(|(index, probe)| {
                compile_one(
                    &mut problems,
                    &format!("detect.probes[{index}].matches"),
                    &probe.matches,
                )
            })
            .collect();
        for (index, pattern) in profile.config.secrets.iter().enumerate() {
            compile_one(&mut problems, &format!("config.secrets[{index}]"), pattern);
        }

        let (policy, policy_usable) =
            match Policy::new(&profile.policy.read_only, &profile.policy.dangerous) {
                Ok(policy) => (policy.shell(profile.policy.shell), true),
                Err(err) => {
                    problems.push(format!(
                        "policy: invalid regular expression ({})",
                        first_line(&err.to_string())
                    ));
                    (Policy::generic(), false)
                }
            };

        if profile.id.is_empty() {
            problems.push("id: a profile needs an id".into());
        } else if !profile
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            problems.push("id: use only letters, digits, `_` and `-`".into());
        }
        if profile.cli.newline.is_empty() {
            problems.push("cli.newline: must not be empty".into());
        }
        if !profile.config.enter.is_empty() && profile.cli.config_prompt.is_none() {
            problems.push("cli.config_prompt: required when config.enter is set, so the driver can tell it is in configuration mode".into());
        }
        for (index, probe) in profile.detect.probes.iter().enumerate() {
            let command = probe.command.trim();
            if command.is_empty() {
                problems.push(format!("detect.probes[{index}].command: must not be empty"));
            } else if policy_usable {
                // A probe is sent to devices nobody has identified yet.
                let verdict = policy.classify(command);
                if verdict.class != crate::policy::CommandClass::ReadOnly {
                    problems.push(format!(
                        "detect.probes[{index}].command: a probe must be read-only, and {}",
                        verdict.reason
                    ));
                }
            }
        }
        if origin == Origin::User && profile.maturity == Maturity::Verified {
            problems.push(
                "maturity: `verified` is reserved for built-in profiles, whose recorded device output is tested with every build; use `community` or `experimental`"
                    .into(),
            );
        }

        for (index, kind) in profile.unverified.iter().enumerate() {
            if profile.unverified[..index].contains(kind) {
                problems.push(format!("unverified[{index}]: `{kind}` is listed twice"));
            } else if !profile
                .collect
                .get(kind)
                .is_some_and(|steps| !steps.is_empty())
            {
                // A caveat about a read that does not exist is a leftover or
                // a typo of another kind, and either way says something untrue.
                problems.push(format!(
                    "unverified[{index}]: `{kind}` is listed as not yet checked against a real device, but this profile has no `collect.{kind}` steps to check"
                ));
            }
        }

        for (kind, steps) in &profile.collect {
            if !kind.is_single_row()
                && !steps.is_empty()
                && steps.iter().all(|step| step.merge_on.is_some())
            {
                problems.push(format!(
                    "collect.{kind}: every step has `merge_on`, so nothing creates the rows they would be merged into; the first step must not have it"
                ));
            }
            for (index, step) in steps.iter().enumerate() {
                let at = format!("collect.{}[{index}]", kind.as_str());
                check_step(
                    &mut problems,
                    &at,
                    *kind,
                    step,
                    &profile,
                    policy_usable.then_some(&policy),
                    expressions,
                );
            }
        }
        check_snmp(&mut problems, &profile.snmp);

        if !problems.is_empty() {
            return Err(ProfileError {
                profile: profile.id.clone(),
                problems,
            });
        }
        let mut profile = profile;
        compile_snmp_patterns(&mut profile.snmp);
        Ok(Platform {
            policy,
            prompt: prompt.unwrap_or_else(fallback),
            user_prompt,
            config_prompt,
            enable_password,
            username_prompt: username_prompt.unwrap_or_else(fallback),
            password_prompt: password_prompt.unwrap_or_else(fallback),
            login_failure: login_failure.unwrap_or_else(fallback),
            pager,
            exec_errors,
            config_errors,
            exec_answers,
            config_answers,
            config_ignore,
            unsolicited,
            banner,
            sysdescr,
            probes,
            profile,
            origin,
            source,
        })
    }
}

/// Everything about one collection step that can be wrong without a device to try it on.
fn check_step(
    problems: &mut Vec<String>,
    at: &str,
    kind: Kind,
    step: &CollectStep,
    profile: &Profile,
    policy: Option<&Policy>,
    expressions: &Expressions,
) {
    let command = step.command.trim();
    if command.is_empty() {
        problems.push(format!("{at}.command: must not be empty"));
    } else if let Some(policy) = policy {
        // The collector refuses anything that is not read-only, so say it now.
        let verdict = policy.classify(command);
        if verdict.class != crate::policy::CommandClass::ReadOnly {
            problems.push(format!(
                "{at}.command: collectors may only run read-only commands, and {}; add a `policy.read_only` pattern that covers it or use another command",
                verdict.reason
            ));
        }
    }

    match &step.parser {
        Parser::Regex(pattern) => {
            if let Some(regex) = compile_one(problems, &format!("{at}.parser.regex"), pattern)
                && regex.capture_names().flatten().next().is_none()
            {
                problems.push(format!(
                    "{at}.parser.regex: the pattern has no named groups, so its rows would have no fields; name what you capture, e.g. `(?P<name>\\S+)`"
                ));
            }
        }
        Parser::Ntc(name) if !command.is_empty() => {
            if let Err(reason) =
                parse::ntc_templates(profile.ntc_platform.as_deref(), command, name)
            {
                problems.push(format!("{at}.parser.ntc: {reason}"));
            }
        }
        Parser::Textfsm(source) => {
            if let Err(err) = nocturne_textfsm::Template::parse(source) {
                problems.push(format!("{at}.parser.textfsm: {err}"));
            }
        }
        Parser::Ntc(_) | Parser::Kv | Parser::Colon | Parser::Json(_) | Parser::Raw => {}
    }

    let model = kind.empty_row();
    for (field, expression) in &step.map {
        if let Err(reason) = check_field(&model, field) {
            problems.push(format!("{at}.map.{field}: {reason}"));
        }
        if let Err(err) = expressions.check(expression) {
            problems.push(format!("{at}.map.{field}: {err}"));
        }
    }
    if let Some(condition) = &step.when
        && let Err(err) = expressions.check(condition)
    {
        problems.push(format!("{at}.when: {err}"));
    }
    if let Some(key) = &step.merge_on {
        if !step.map.is_empty() && !step.map.contains_key(key) {
            problems.push(format!(
                "{at}.merge_on: `{key}` is not one of this step's mapped fields"
            ));
        } else if step.map.is_empty() && !model.contains_key(key) {
            problems.push(format!(
                "{at}.merge_on: `{key}` is not a field of {kind} rows ({})",
                field_list(&model)
            ));
        }
    }
}

/// Whether `field` (possibly dotted: `counters.in_errors`) is something a row of the model has.
fn check_field(
    model: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<(), String> {
    let (top, nested) = match field.split_once('.') {
        Some((top, nested)) => (top, Some(nested)),
        None => (field, None),
    };
    let Some(value) = model.get(top) else {
        return Err(format!(
            "the model has no field `{top}` here; the fields are {}",
            field_list(model)
        ));
    };
    match (nested, value) {
        (None, _) => Ok(()),
        // An empty object (`extra`) takes any key.
        (Some(_), serde_json::Value::Object(inner)) if inner.is_empty() => Ok(()),
        (Some(nested), serde_json::Value::Object(inner)) if inner.contains_key(nested) => Ok(()),
        (Some(nested), serde_json::Value::Object(inner)) => Err(format!(
            "`{top}` has no field `{nested}`; its fields are {}",
            field_list(inner)
        )),
        (Some(_), _) => Err(format!("`{top}` is a plain value and has no sub-fields")),
    }
}

fn field_list(model: &serde_json::Map<String, serde_json::Value>) -> String {
    model
        .keys()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn compile_one(problems: &mut Vec<String>, field: &str, pattern: &str) -> Option<Regex> {
    match Regex::new(pattern) {
        Ok(compiled) => Some(compiled),
        Err(err) => {
            problems.push(format!(
                "{field}: invalid regular expression `{pattern}` ({})",
                first_line(&err.to_string())
            ));
            None
        }
    }
}

fn compile_list(problems: &mut Vec<String>, field: &str, patterns: &[String]) -> Vec<Regex> {
    patterns
        .iter()
        .enumerate()
        .filter_map(|(index, pattern)| compile_one(problems, &format!("{field}[{index}]"), pattern))
        .collect()
}

fn compile_answers(
    problems: &mut Vec<String>,
    field: &str,
    answers: &[Answer],
) -> Vec<(Regex, String)> {
    answers
        .iter()
        .enumerate()
        .filter_map(|(index, answer)| {
            compile_one(
                problems,
                &format!("{field}[{index}].prompt"),
                &answer.prompt,
            )
            .map(|pattern| (pattern, answer.answer.clone()))
        })
        .collect()
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or(text)
}

/// For listing profiles in the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProfileSummary {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub os: String,
    pub description: String,
    pub origin: Origin,
    pub maturity: Maturity,
    pub extends: Option<String>,
    /// What the profile can collect.
    pub collects: Vec<Kind>,
    /// Those of `collects` that have not been checked against a real device.
    pub unverified: Vec<Kind>,
    pub can_configure: bool,
    pub syntax: Syntax,
}

/// All known platforms.
pub struct Registry {
    platforms: IndexMap<String, Arc<Platform>>,
    /// Fully merged YAML per profile, the base for children.
    merged: IndexMap<String, Yaml>,
    expressions: Arc<Expressions>,
}

impl Registry {
    pub fn empty() -> Registry {
        Registry {
            platforms: IndexMap::new(),
            merged: IndexMap::new(),
            expressions: Arc::new(Expressions::new()),
        }
    }

    /// The profiles shipped with Nocturne. Problems in a bundled profile are
    /// returned rather than hidden, and are a test failure in this crate.
    pub fn builtin() -> (Registry, Vec<ProfileError>) {
        let mut registry = Registry::empty();
        let mut pending: Vec<(String, String)> = BUILTIN
            .files()
            .filter(|file| {
                file.path()
                    .extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
            .filter_map(|file| {
                Some((
                    file.path().display().to_string(),
                    file.contents_utf8()?.to_string(),
                ))
            })
            .collect();
        pending.sort();

        // Parents must load before children; keep sweeping until nothing more loads.
        let mut errors = Vec::new();
        loop {
            let before = pending.len();
            let mut waiting = Vec::new();
            for (path, yaml) in pending {
                match registry.add(&yaml, Origin::Builtin) {
                    Ok(_) => {}
                    Err(err)
                        if err
                            .problems
                            .iter()
                            .any(|problem| problem.starts_with("extends:")) =>
                    {
                        waiting.push((path, yaml))
                    }
                    Err(mut err) => {
                        if err.profile.is_empty() {
                            err.profile = path;
                        }
                        errors.push(err);
                    }
                }
            }
            pending = waiting;
            if pending.is_empty() || pending.len() == before {
                break;
            }
        }
        for (path, yaml) in pending {
            if let Err(mut err) = registry.add(&yaml, Origin::Builtin) {
                if err.profile.is_empty() {
                    err.profile = path;
                }
                errors.push(err);
            }
        }
        (registry, errors)
    }

    /// Load every `*.yaml` in a directory as user profiles (they may replace
    /// built-ins of the same id). Files with problems are reported and skipped.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<(PathBuf, ProfileError)> {
        let mut failures = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return failures;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.extension()
                    .is_some_and(|ext| ext == "yaml" || ext == "yml")
            })
            .collect();
        paths.sort();
        for path in paths {
            match std::fs::read_to_string(&path) {
                Ok(yaml) => {
                    if let Err(err) = self.add(&yaml, Origin::User) {
                        failures.push((path, err));
                    }
                }
                Err(err) => failures.push((
                    path.clone(),
                    ProfileError {
                        profile: path.display().to_string(),
                        problems: vec![format!("could not read the file: {err}")],
                    },
                )),
            }
        }
        failures
    }

    /// Parse, merge with its parent, validate and register one profile.
    pub fn add(&mut self, yaml: &str, origin: Origin) -> Result<Arc<Platform>, ProfileError> {
        let fail = |profile: &str, problem: String| ProfileError {
            profile: profile.to_string(),
            problems: vec![problem],
        };
        let document: Yaml = serde_yaml_ng::from_str(yaml)
            .map_err(|err| fail("", format!("not valid YAML: {err}")))?;
        if !document.is_mapping() {
            return Err(fail("", "a profile must be a YAML mapping".into()));
        }
        let id = document
            .get("id")
            .and_then(Yaml::as_str)
            .unwrap_or_default()
            .to_string();

        // A profile is only as checked as its own author says: a child does
        // not become `verified` because its parent is.
        let own_maturity = document.get("maturity").cloned();
        let mut merged = match document.get("extends").and_then(Yaml::as_str) {
            Some(parent) if parent == id => {
                return Err(fail(&id, "extends: a profile cannot extend itself".into()));
            }
            Some(parent) => {
                let base = self.merged.get(parent).ok_or_else(|| {
                    fail(&id, format!("extends: unknown parent profile `{parent}`"))
                })?;
                merge(base.clone(), document).map_err(|problem| fail(&id, problem))?
            }
            None => {
                let mut found = Vec::new();
                extras(&document, "", &mut found);
                if let Some(first) = found.first() {
                    return Err(fail(
                        &id,
                        format!(
                            "{first}: adding to an inherited list needs something to inherit from; this profile has no `extends`"
                        ),
                    ));
                }
                document
            }
        };
        if let Yaml::Mapping(map) = &mut merged {
            match own_maturity {
                Some(maturity) => map.insert("maturity".into(), maturity),
                None => map.remove("maturity"),
            };
        }

        let profile: Profile =
            serde_yaml_ng::from_value(merged.clone()).map_err(|err| fail(&id, err.to_string()))?;
        let platform = Arc::new(Platform::compile(
            profile,
            origin,
            yaml.to_string(),
            &self.expressions,
        )?);
        self.merged.insert(id.clone(), merged);
        self.platforms.insert(id, platform.clone());
        Ok(platform)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.merged.shift_remove(id);
        self.platforms.shift_remove(id).is_some()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Platform>> {
        self.platforms.get(id).cloned()
    }

    /// Every loaded platform, abstract bases included, in load order.
    pub fn platforms(&self) -> impl Iterator<Item = &Arc<Platform>> {
        self.platforms.values()
    }

    pub fn expressions(&self) -> Arc<Expressions> {
        self.expressions.clone()
    }

    /// Selectable (non-abstract) profiles, sorted by vendor then name.
    pub fn list(&self) -> Vec<ProfileSummary> {
        let mut list: Vec<ProfileSummary> = self
            .platforms
            .values()
            .filter(|platform| !platform.is_abstract())
            .map(|platform| {
                let profile = &platform.profile;
                ProfileSummary {
                    id: profile.id.clone(),
                    name: profile.name.clone(),
                    vendor: profile.vendor.clone(),
                    os: profile.os.clone(),
                    description: profile.description.clone(),
                    origin: platform.origin,
                    maturity: profile.maturity,
                    extends: profile.extends.clone(),
                    collects: profile
                        .collect
                        .iter()
                        .filter(|(_, steps)| !steps.is_empty())
                        .map(|(kind, _)| *kind)
                        .collect(),
                    unverified: profile.unverified.clone(),
                    can_configure: !profile.config.enter.is_empty()
                        || profile.config.syntax == Syntax::Routeros,
                    syntax: profile.config.syntax,
                }
            })
            .collect();
        list.sort_by(|a, b| {
            (a.vendor.to_lowercase(), a.name.to_lowercase())
                .cmp(&(b.vendor.to_lowercase(), b.name.to_lowercase()))
        });
        list
    }

    /// Best match for text seen before or right after login.
    pub fn detect_banner(&self, text: &str) -> Option<Arc<Platform>> {
        self.best(|platform| platform.banner.iter().any(|pattern| pattern.is_match(text)))
    }

    /// Best match for SNMP system information.
    pub fn detect_snmp(&self, sysdescr: &str, sysobjectid: &str) -> Option<Arc<Platform>> {
        let object_id = sysobjectid.trim_start_matches('.');
        self.best(|platform| {
            platform
                .sysdescr
                .iter()
                .any(|pattern| pattern.is_match(sysdescr))
                || (!object_id.is_empty()
                    && platform
                        .profile
                        .detect
                        .sysobjectid
                        .iter()
                        .any(|prefix| object_id.starts_with(prefix.trim_start_matches('.'))))
        })
    }

    /// Probe commands to try on an unidentified session, most specific first.
    pub fn probes(&self) -> Vec<(Arc<Platform>, Probe)> {
        let mut probes: Vec<(Arc<Platform>, Probe)> = self
            .platforms
            .values()
            .filter(|platform| !platform.is_abstract())
            .flat_map(|platform| {
                platform
                    .profile
                    .detect
                    .probes
                    .iter()
                    .map(|probe| (platform.clone(), probe.clone()))
            })
            .collect();
        probes.sort_by_key(|(platform, _)| std::cmp::Reverse(platform.profile.detect.priority));
        probes
    }

    fn best(&self, matches: impl Fn(&Platform) -> bool) -> Option<Arc<Platform>> {
        self.platforms
            .values()
            .filter(|platform| !platform.is_abstract() && matches(platform))
            .max_by_key(|platform| platform.profile.detect.priority)
            .cloned()
    }
}

/// Merge `child` over `base`: mappings merge key by key, everything else is replaced.
/// The suffix that makes a child *add to* an inherited list instead of
/// replacing it.
const EXTRA: &str = "_extra";

/// Merge a child profile over its parent.
///
/// A mapping merges key by key; anything else the child states replaces what
/// the parent said. A list is "anything else" — which is the right default
/// (a child that lists its own `init` commands means *those* commands) but
/// made adding one entry to an inherited list cost a full restatement of it.
/// That is not a theoretical cost: restating `policy.read_only` to add two
/// IPv6 ping patterns means retyping eight security-relevant regular
/// expressions, and restating a list of secret patterns is how a pattern
/// that captured a key *number* instead of a key came to be in this
/// repository at all.
///
/// So `<name>_extra: [...]` appends to the parent's `<name>` list and leaves
/// the rest of it alone. The extras go on the end, which is what order means
/// for the lists where order matters (`cli.init`), and does not matter for
/// the lists where it does not (any match in `policy.read_only` wins).
fn merge(base: Yaml, child: Yaml) -> Result<Yaml, String> {
    match (base, child) {
        (Yaml::Mapping(mut base), Yaml::Mapping(child)) => {
            for (key, value) in child {
                let appending = key.as_str().and_then(|key| key.strip_suffix(EXTRA));
                if let Some(target) = appending {
                    if target.is_empty() {
                        return Err(format!("`{EXTRA}` is not a field name on its own"));
                    }
                    let Yaml::Sequence(extra) = value else {
                        return Err(format!("{target}{EXTRA}: must be a list"));
                    };
                    let key = Yaml::String(target.to_string());
                    let mut combined = match base.remove(&key) {
                        Some(Yaml::Sequence(items)) => items,
                        // Nothing inherited to add to: the extras are the
                        // whole list, which is what the author asked for.
                        None => Vec::new(),
                        Some(_) => {
                            return Err(format!(
                                "{target}{EXTRA}: `{target}` is inherited as a single value, not a list, so there is nothing to add to"
                            ));
                        }
                    };
                    combined.extend(extra);
                    base.insert(key, Yaml::Sequence(combined));
                    continue;
                }
                let merged = match base.remove(&key) {
                    Some(existing) => merge(existing, value)?,
                    None => value,
                };
                base.insert(key, merged);
            }
            Ok(Yaml::Mapping(base))
        }
        (_, child) => Ok(child),
    }
}

/// Every `<name>_extra` key in a document, by path.
///
/// Only meaningful with a parent to add to, so a profile without `extends`
/// is told that plainly rather than left to serde's "unknown field".
fn extras(document: &Yaml, path: &str, into: &mut Vec<String>) {
    if let Yaml::Mapping(map) = document {
        for (key, value) in map {
            let Some(name) = key.as_str() else { continue };
            let here = if path.is_empty() {
                name.to_string()
            } else {
                format!("{path}.{name}")
            };
            if name.ends_with(EXTRA) {
                into.push(here);
                continue;
            }
            extras(value, &here, into);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
id: _switch_base
name: Base
cli:
  prompt: '^(?P<host>[\w.-]+)(?:\(config[^)]*\))?[>#]\s?$'
  user_prompt: '>\s?$'
  config_prompt: '\(config[^)]*\)#\s?$'
  init: ["terminal length 0"]
  errors: ['^% ']
config:
  syntax: ios
  enter: [configure terminal]
  exit: [end]
  save: [write memory]
  running: show running-config
  ignore: ['^Building configuration', '^Current configuration']
policy:
  read_only: ['^show\b']
  dangerous: ['^reload\b']
collect:
  vlans:
    - command: show vlan brief
      parser: {regex: '(?m)^(?P<ID>\d+)\s+(?P<NAME>\S+)\s+(?P<STATUS>\S+)'}
      map: {id: ID, name: NAME, status: STATUS}
"#;

    const CHILD: &str = r#"
id: acme_os
name: Acme OS
vendor: Acme
os: AcmeOS
extends: _switch_base
detect:
  banner: ['(?i)acme networks']
  sysobjectid: ['1.3.6.1.4.1.99999.']
  priority: 10
cli:
  init: ["no page"]
config:
  save: [copy running-config startup-config]
collect:
  facts:
    - command: show system
      parser: colon
      map: {hostname: name, os_version: "version | regex_search('([\\d.]+)')"}
"#;

    /// `<name>_extra` adds to an inherited list rather than replacing it.
    ///
    /// This exists because the alternative was restating the list, and a
    /// restatement is a chance to weaken it. It is how a secret pattern that
    /// captured a key *number* instead of a key reached this repository: the
    /// author needed one more pattern and copied nine.
    #[test]
    fn a_child_can_add_to_an_inherited_list_without_restating_it() {
        const ADDS: &str = r#"
id: acme_adds
name: Acme, adding
extends: _switch_base
policy:
  read_only_extra: ['^ping6\s+\S']
cli:
  init_extra: ["terminal width 511"]
"#;
        let mut registry = Registry::empty();
        registry.add(BASE, Origin::Builtin).unwrap();
        let platform = registry.add(ADDS, Origin::User).unwrap();
        let profile = &platform.profile;

        // The inherited entry is still there, untouched, and the new one is
        // after it.
        assert_eq!(
            profile.policy.read_only,
            vec![r"^show\b", r"^ping6\s+\S"],
            "the parent's pattern survives and the child's is added"
        );
        assert_eq!(
            profile.policy.dangerous,
            vec![r"^reload\b"],
            "a list the child said nothing about is inherited whole"
        );
        // Order matters for commands, and the extras go last.
        assert_eq!(
            profile.cli.init,
            vec!["terminal length 0", "terminal width 511"]
        );
    }

    #[test]
    fn adding_to_a_list_is_refused_when_there_is_nothing_to_add_to() {
        let mut registry = Registry::empty();
        registry.add(BASE, Origin::Builtin).unwrap();

        // No parent at all: the author almost certainly meant `read_only`.
        let orphan = r#"
id: acme_orphan
name: Orphan
policy:
  read_only_extra: ['^show\b']
"#;
        let err = registry.add(orphan, Origin::User).unwrap_err().to_string();
        assert!(
            err.contains("policy.read_only_extra") && err.contains("no `extends`"),
            "{err}"
        );

        // Not a list in the child.
        let not_a_list = r#"
id: acme_scalar
name: Scalar
extends: _switch_base
policy:
  read_only_extra: '^show\b'
"#;
        let err = registry
            .add(not_a_list, Origin::User)
            .unwrap_err()
            .to_string();
        assert!(err.contains("must be a list"), "{err}");

        // Inherited as a single value, so there is no list to add to.
        let not_inherited_as_list = r#"
id: acme_wrong
name: Wrong
extends: _switch_base
config:
  running_extra: ['show running-config all']
"#;
        let err = registry
            .add(not_inherited_as_list, Origin::User)
            .unwrap_err()
            .to_string();
        assert!(err.contains("nothing to add to"), "{err}");
    }

    #[test]
    fn child_profiles_merge_over_their_parent() {
        let mut registry = Registry::empty();
        registry.add(BASE, Origin::Builtin).unwrap();
        let platform = registry.add(CHILD, Origin::User).unwrap();
        let profile = &platform.profile;
        assert_eq!(profile.id, "acme_os");
        assert_eq!(profile.cli.init, vec!["no page"], "lists replace");
        assert_eq!(
            profile.cli.errors,
            vec!["^% "],
            "untouched keys are inherited"
        );
        assert_eq!(profile.config.enter, vec!["configure terminal"]);
        assert_eq!(
            profile.config.save,
            vec!["copy running-config startup-config"]
        );
        assert_eq!(profile.config.syntax, Syntax::Ios);
        assert!(
            profile.collect.contains_key(&Kind::Vlans)
                && profile.collect.contains_key(&Kind::Facts)
        );
        assert_eq!(profile.collect[&Kind::Facts][0].parser, Parser::Colon);

        assert!(platform.policy.is_read_only("show vlan"));
        assert!(platform.is_config_prompt("sw1(config-if)#"));
        assert!(platform.is_user_prompt("sw1>"));
        assert!(!platform.is_user_prompt("sw1#"));
        assert_eq!(
            platform.find_error("vlan 5000\n% Invalid input", false),
            Some("% Invalid input")
        );
        assert_eq!(
            platform.clean_config("Building configuration...\n\nhostname sw1  \n!\n"),
            "hostname sw1\n!\n"
        );

        let listed = registry.list();
        assert_eq!(listed.len(), 1, "abstract bases are not selectable");
        assert_eq!(listed[0].collects, vec![Kind::Vlans, Kind::Facts]);
        assert!(listed[0].can_configure);
    }

    #[test]
    fn detection_prefers_the_highest_priority_match() {
        let mut registry = Registry::empty();
        registry.add(BASE, Origin::Builtin).unwrap();
        registry.add(CHILD, Origin::User).unwrap();
        registry
            .add("id: generic\nname: Generic\ndetect: {banner: ['.'], sysdescr: ['.'], priority: -100}\n", Origin::Builtin)
            .unwrap();
        assert_eq!(
            registry
                .detect_banner("Welcome to ACME Networks OS")
                .unwrap()
                .id(),
            "acme_os"
        );
        assert_eq!(
            registry.detect_banner("something else").unwrap().id(),
            "generic"
        );
        assert_eq!(
            registry
                .detect_snmp("whatever", ".1.3.6.1.4.1.99999.1.2")
                .unwrap()
                .id(),
            "acme_os"
        );
    }

    #[test]
    fn problems_are_reported_with_their_location() {
        let mut registry = Registry::empty();
        let unknown_key = registry
            .add("id: x\nname: X\ncli:\n  promt: 'oops'\n", Origin::User)
            .unwrap_err();
        assert!(
            unknown_key.to_string().contains("unknown field `promt`"),
            "{unknown_key}"
        );

        let bad = r#"
id: bad one
cli:
  prompt: '('
config:
  enter: [configure]
collect:
  interfaces:
    - command: ""
      parser: {regex: '['}
      map: {name: "NAME |"}
      when: "(("
      merge_on: missing
"#;
        let err = registry.add(bad, Origin::User).unwrap_err();
        let text = err.to_string();
        for expected in [
            "cli.prompt: invalid regular expression",
            "id: use only letters",
            "cli.config_prompt: required",
            "collect.interfaces[0].command",
            "collect.interfaces[0].parser.regex",
            "collect.interfaces[0].map.name",
            "collect.interfaces[0].when",
            "collect.interfaces[0].merge_on",
        ] {
            assert!(text.contains(expected), "missing `{expected}` in: {text}");
        }
        assert!(
            registry.get("bad one").is_none(),
            "a broken profile is not registered"
        );

        let orphan = registry
            .add("id: child\nextends: nowhere\n", Origin::User)
            .unwrap_err();
        assert!(
            orphan
                .to_string()
                .contains("unknown parent profile `nowhere`")
        );
        assert!(registry.add("- not\n- a mapping\n", Origin::User).is_err());
        assert!(registry.add("id: me\nextends: me\n", Origin::User).is_err());
        let bad_parser = registry
            .add("id: p\ncollect:\n  facts:\n    - {command: x, parser: {xml: y}, map: {hostname: a}}\n", Origin::User)
            .unwrap_err();
        assert!(
            bad_parser.to_string().contains("unknown parser `xml`"),
            "{bad_parser}"
        );
    }

    #[test]
    fn a_profile_says_which_of_its_reads_have_not_met_a_real_device() {
        const WITH_QOS: &str = r#"
id: qos_os
name: QoS OS
maturity: community
unverified: [qos]
policy: {read_only: ['^show ']}
collect:
  facts:
    - command: show version
      parser: {regex: 'Version (?P<version>\S+)'}
      map: {os_version: version}
  qos:
    - command: show policy-map interface
      parser: {regex: 'Class-map: (?P<class>\S+)'}
      map: {class: class, direction: "'out'"}
"#;
        let mut registry = Registry::empty();
        let platform = registry.add(WITH_QOS, Origin::User).unwrap();
        assert!(platform.profile.is_unverified(Kind::Qos));
        assert!(!platform.profile.is_unverified(Kind::Facts));
        let listed = registry.list();
        assert_eq!(listed[0].collects, vec![Kind::Facts, Kind::Qos]);
        assert_eq!(listed[0].unverified, vec![Kind::Qos]);

        // A child that inherits the steps inherits the caveat with them, and
        // may lift it only by saying so.
        let child = registry
            .add(
                "id: qos_child\nname: Child\nextends: qos_os\n",
                Origin::User,
            )
            .unwrap();
        assert!(child.profile.is_unverified(Kind::Qos));
        let lifted = registry
            .add(
                "id: qos_lifted\nname: Lifted\nextends: qos_os\nunverified: []\n",
                Origin::User,
            )
            .unwrap();
        assert!(!lifted.profile.is_unverified(Kind::Qos));

        // A caveat about a read the profile does not make is refused.
        let nothing_to_check = registry
            .add(
                "id: no_qos\nname: None\nunverified: [qos, routes]\n",
                Origin::User,
            )
            .unwrap_err()
            .to_string();
        for expected in [
            "unverified[0]: `qos` is listed as not yet checked",
            "no `collect.qos` steps",
            "unverified[1]: `routes`",
        ] {
            assert!(
                nothing_to_check.contains(expected),
                "missing `{expected}` in: {nothing_to_check}"
            );
        }
        let twice = registry
            .add(
                &WITH_QOS
                    .replace("id: qos_os", "id: twice")
                    .replace("[qos]", "[qos, qos]"),
                Origin::User,
            )
            .unwrap_err()
            .to_string();
        assert!(
            twice.contains("unverified[1]: `qos` is listed twice"),
            "{twice}"
        );
        let not_a_kind = registry
            .add(
                &WITH_QOS
                    .replace("id: qos_os", "id: typo")
                    .replace("[qos]", "[quos]"),
                Origin::User,
            )
            .unwrap_err()
            .to_string();
        assert!(not_a_kind.contains("quos"), "{not_a_kind}");
    }

    #[test]
    fn user_profiles_load_from_a_directory_and_override() {
        let dir = std::env::temp_dir().join(format!("nocturne-profiles-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a_base.yaml"), BASE).unwrap();
        std::fs::write(dir.join("b_child.yaml"), CHILD).unwrap();
        std::fs::write(
            dir.join("c_broken.yaml"),
            "id: broken\ncli: {prompt: '('}\n",
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let mut registry = Registry::empty();
        let failures = registry.load_dir(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(failures.len(), 1);
        assert!(failures[0].0.ends_with("c_broken.yaml"));
        assert_eq!(registry.get("acme_os").unwrap().origin, Origin::User);
        assert!(registry.remove("acme_os") && registry.get("acme_os").is_none());
    }

    #[test]
    fn bundled_profiles_all_load_cleanly() {
        let (registry, errors) = Registry::builtin();
        assert!(errors.is_empty(), "bundled profile problems: {errors:#?}");
        for platform in registry.platforms.values() {
            assert_eq!(platform.origin, Origin::Builtin);
        }
    }

    /// The block every profile had before an entry could be a mapping:
    /// lists of OID strings, one of them not an OID at all.
    const OLD_SNMP: &str = r#"
id: old_snmp
name: Old SNMP block
snmp:
  cpu: ['1.3.6.1.4.1.9.9.109.1.1.1.1.7', '1.3.6.1.4.1.9.2.1.57']
  memory_used: ['1.3.6.1.4.1.9.9.48.1.1.1.5']
  memory_free: ['1.3.6.1.4.1.9.9.48.1.1.1.6']
  temperature: ['1.3.6.1.4.1.9.9.13.1.3.1.3', 'not an oid']
"#;

    #[test]
    fn a_block_of_plain_oid_lists_loads_and_reads_back_exactly_as_written() {
        let mut registry = Registry::empty();
        let platform = registry
            .add(OLD_SNMP, Origin::User)
            .expect("a stored profile of plain lists still loads, even with a bad OID in it");
        let snmp = &platform.profile.snmp;
        assert_eq!(
            snmp.cpu,
            vec![
                SnmpEntry::bare("1.3.6.1.4.1.9.9.109.1.1.1.1.7"),
                SnmpEntry::bare("1.3.6.1.4.1.9.2.1.57"),
            ]
        );
        assert!(snmp.cpu[0].is_column() && snmp.cpu[1].is_column());
        assert_eq!(snmp.temperature[1].target(), Some("not an oid"));
        assert!(snmp.memory_total.is_empty() && snmp.memory_percent.is_empty());

        // What `profiles.get` hands out: the lists as the strings they were.
        let resolved = serde_json::to_value(&platform.profile.snmp).unwrap();
        assert_eq!(
            resolved,
            serde_json::json!({
                "cpu": ["1.3.6.1.4.1.9.9.109.1.1.1.1.7", "1.3.6.1.4.1.9.2.1.57"],
                "memory_used": ["1.3.6.1.4.1.9.9.48.1.1.1.5"],
                "memory_free": ["1.3.6.1.4.1.9.9.48.1.1.1.6"],
                "memory_total": [],
                "memory_percent": [],
                "temperature": ["1.3.6.1.4.1.9.9.13.1.3.1.3", "not an oid"],
            })
        );
    }

    #[test]
    fn an_entry_may_say_which_instance_which_rows_and_what_arithmetic() {
        let yaml = r#"
id: mapped_snmp
name: Mapped SNMP entries
snmp:
  cpu:
    - {oid: 1.3.6.1.4.1.9.9.109.1.1.1.1.7, combine: max}
    - {oid: 1.3.6.1.4.1.2021.11.11.0, complement: 100}
  memory_used:
    - instance: 1.3.6.1.2.1.25.2.3.1.6.65536
  memory_total:
    - oid: 1.3.6.1.2.1.25.2.3.1.5
      where:
        - {oid: 1.3.6.1.2.1.25.2.3.1.2, is: 1.3.6.1.2.1.25.2.1.2}
  memory_percent: [1.3.6.1.4.1.9.9.305.1.1.2.0]
  temperature:
    - oid: 1.3.6.1.2.1.99.1.1.1.4
      where:
        - {oid: 1.3.6.1.2.1.99.1.1.1.1, is: 8}
        - {oid: 1.3.6.1.2.1.99.1.1.1.5, in: [1, 2]}
        - {oid: 1.3.6.1.2.1.47.1.1.1.1.7, matches: '^CPU'}
      si_scale: 1.3.6.1.2.1.99.1.1.1.2
      precision: 1.3.6.1.2.1.99.1.1.1.3
      absent: [-1000000000, 1000000000]
      label: board
    - {oid: 1.3.6.1.4.1.14988.1.1.3.10.0, scale: 0.1, label: cpu}
    - {oid: 1.3.6.1.4.1.14988.1.1.3.11.0, scale: 0.1, label: processor, fallback: true}
"#;
        let mut registry = Registry::empty();
        let platform = registry.add(yaml, Origin::User).unwrap();
        let snmp = &platform.profile.snmp;
        assert_eq!(snmp.cpu[0].combine, Some(Combine::Max));
        assert!(snmp.cpu[0].is_column());
        assert_eq!(snmp.cpu[1].complement, Some(100.0));
        assert!(!snmp.cpu[1].is_column(), "a `.0` OID is one object");
        assert_eq!(
            snmp.memory_used[0].instance.as_deref(),
            Some("1.3.6.1.2.1.25.2.3.1.6.65536")
        );
        assert!(
            !snmp.memory_used[0].is_column(),
            "an instance is one object whatever it ends in"
        );
        assert_eq!(
            snmp.memory_total[0].tests[0].is.as_deref(),
            Some("1.3.6.1.2.1.25.2.1.2")
        );
        let sensor = &snmp.temperature[0];
        assert_eq!(
            sensor.tests[0].is.as_deref(),
            Some("8"),
            "a number, as text"
        );
        assert_eq!(
            sensor.tests[1].any_of,
            Some(vec!["1".to_string(), "2".to_string()])
        );
        assert_eq!(sensor.tests[2].matches.as_deref(), Some("^CPU"));
        assert_eq!(sensor.absent, vec![-1e9, 1e9]);
        assert_eq!(sensor.label.as_deref(), Some("board"));
        assert_eq!(snmp.temperature[1].scale, Some(0.1));
        assert!(!snmp.temperature[1].fallback && snmp.temperature[2].fallback);

        // A mapping goes back out as a mapping, and what was a bare string
        // as a bare string.
        let resolved = serde_json::to_value(snmp).unwrap();
        assert_eq!(
            resolved["memory_percent"],
            serde_json::json!(["1.3.6.1.4.1.9.9.305.1.1.2.0"])
        );
        assert_eq!(
            resolved["cpu"][0],
            serde_json::json!({"oid": "1.3.6.1.4.1.9.9.109.1.1.1.1.7", "combine": "max"})
        );
        assert_eq!(
            resolved["temperature"][0]["where"][1],
            serde_json::json!({"oid": "1.3.6.1.2.1.99.1.1.1.5", "in": ["1", "2"]})
        );
        assert_eq!(resolved["temperature"][2]["fallback"], true);
        assert_eq!(resolved["temperature"][1].get("fallback"), None);
        let again: SnmpSpec = serde_json::from_value(resolved).unwrap();
        assert_eq!(again.temperature, snmp.temperature);
    }

    #[test]
    fn a_row_test_compares_the_cell_as_text_and_its_pattern_is_compiled_at_load() {
        let yaml = r#"
id: tests_snmp
name: Row tests
snmp:
  temperature:
    - oid: 1.3.6.1.2.1.99.1.1.1.4
      where:
        - {oid: 1.3.6.1.2.1.25.2.3.1.2, is: .1.3.6.1.2.1.25.2.1.2}
        - {oid: 1.3.6.1.4.1.9.9.13.1.3.1.6, in: [1, 2, 3, 4]}
        - {oid: 1.3.6.1.4.1.2636.3.1.13.1.5, matches: '^(Routing Engine|Host)( \d+)?$'}
"#;
        let mut registry = Registry::empty();
        let platform = registry.add(yaml, Origin::User).unwrap();
        let tests = &platform.profile.snmp.temperature[0].tests;
        assert!(
            tests[0].holds("1.3.6.1.2.1.25.2.1.2"),
            "an OID written with a leading dot matches the cell, which has none"
        );
        assert!(!tests[0].holds("1.3.6.1.2.1.25.2.1.3"));
        assert!(tests[1].holds("4") && !tests[1].holds("5") && !tests[1].holds("40"));
        assert!(
            tests[2].pattern.is_some(),
            "compiled when the profile loaded"
        );
        assert!(tests[2].holds("Routing Engine 0") && tests[2].holds("Routing Engine"));
        assert!(!tests[2].holds("FPC: QFX5100-48S-6Q @ 0/*/*"));
        assert!(!tests[2].holds("Routing Engine 0 (backup)"));
    }

    #[test]
    fn a_mistyped_key_in_an_snmp_entry_is_named() {
        let mut registry = Registry::empty();
        for (yaml, expected) in [
            (
                "id: t\nsnmp:\n  cpu:\n    - {oid: 1.3.6.1.2.1.25.3.3.1.2, compliment: 100}\n",
                "an SNMP entry has no key `compliment`",
            ),
            (
                "id: t\nsnmp:\n  temperature:\n    - oid: 1.3.6.1.2.1.99.1.1.1.4\n      where: [{oid: 1.3.6.1.2.1.99.1.1.1.1, equals: 8}]\n",
                "a `where` test has no key `equals`",
            ),
            (
                "id: t\nsnmp:\n  cpu:\n    - {oid: 1.3.6.1.2.1.25.3.3.1.2, combine: average}\n",
                "unknown variant `average`",
            ),
            (
                "id: t\nsnmp:\n  cpu_load: ['1.3.6.1.2.1.25.3.3.1.2']\n",
                "unknown field `cpu_load`",
            ),
        ] {
            let err = registry.add(yaml, Origin::User).unwrap_err().to_string();
            assert!(err.contains(expected), "missing `{expected}` in: {err}");
        }
    }

    #[test]
    fn an_entry_that_cannot_be_read_as_written_is_refused_where_it_stands() {
        let bad = r#"
id: bad_snmp
name: Bad SNMP entries
snmp:
  cpu:
    - {oid: 1.3.6.1.2.1.25.3.3.1.2, instance: 1.3.6.1.2.1.25.3.3.1.2.196608}
    - {complement: 100}
    - {oid: 1.3.6.1.4.1.2021.11.11.0, combine: max}
  memory_used:
    - instance: 1.3.6.1.2.1.25.2.3.1.6.65536
      where: [{oid: 1.3.6.1.2.1.25.2.3.1.2, is: 1.3.6.1.2.1.25.2.1.2}]
    - {oid: 1.3.6.1.2.1.25.2.3.1.6, combine: max}
    - {oid: hrStorageUsed}
    - {oid: 1.3.6.1.2.1.25.2.3.1.6, scale: 0}
  temperature:
    - oid: 1.3.6.1.2.1.99.1.1.1.4
      where:
        - {oid: 1.3.6.1.2.1.99.1.1.1.1, is: 8, in: [8, 9]}
        - {oid: 1.3.6.1.2.1.99.1.1.1.5}
        - {oid: 1.3.6.1.2.1.47.1.1.1.1.7, matches: '(unclosed'}
        - {oid: 1.3.6.1.2.1.99.1.1.1.2, in: []}
      label: board
    - {oid: 1.3.6.1.4.1.14988.1.1.3.10.0, label: board}
    - {instance: 1.3.6.1.4.1.14988.1.1.3.11.0, si_scale: 1.3.6.1.2.1.99.1.1.1.2}
    - {oid: 1.3.6.1.4.1.14988.1.1.3.12.0, label: power, fallback: true}
  memory_percent:
    - {oid: 1.3.6.1.4.1.9.9.305.1.1.2.0, label: memory}
    - {oid: 1.3.6.1.4.1.9.9.305.1.1.2.0, fallback: true}
"#;
        let mut registry = Registry::empty();
        let err = registry.add(bad, Origin::User).unwrap_err();
        let text = err.to_string();
        for expected in [
            "snmp.cpu[0]: give `oid` or `instance`, not both",
            "snmp.cpu[1]: needs an `oid` or an `instance`",
            "snmp.cpu[2]: `combine` works on the rows of a column",
            "snmp.memory_used[0]: `where` works on the rows of a column",
            "snmp.memory_used[1]: `combine` is for `cpu` only",
            "snmp.memory_used[2]: oid: `hrStorageUsed` is not an OID",
            "snmp.memory_used[3]: `scale` must be a number other than zero",
            "snmp.temperature[0]: where[0]: give exactly one of `is`, `in` and `matches`",
            "snmp.temperature[0]: where[1]: give exactly one of",
            "snmp.temperature[0]: where[2].matches: invalid regular expression `(unclosed`",
            "snmp.temperature[0]: where[3].in: an empty list matches no row",
            "snmp.temperature[1]: the label `board` is already the label of snmp.temperature[0]",
            "snmp.temperature[2]: `si_scale` works on the rows of a column",
            "snmp.memory_percent[0]: `label` is for `temperature` only",
            "snmp.memory_percent[1]: `fallback` is for `temperature` only",
        ] {
            assert!(
                text.contains(expected),
                "missing `{expected}` in:\n{}",
                err.problems.join("\n")
            );
        }
        assert!(
            !text.contains("snmp.temperature[3]"),
            "a fallback below an entry is fine: {text}"
        );
        assert!(registry.get("bad_snmp").is_none());

        let alone = registry
            .add(
                "id: alone\nsnmp:\n  temperature:\n    - {oid: 1.3.6.1.4.1.14988.1.1.3.10.0, fallback: true}\n",
                Origin::User,
            )
            .unwrap_err()
            .to_string();
        assert!(
            alone.contains("snmp.temperature[0]: `fallback` needs an entry above it"),
            "{alone}"
        );
    }

    #[test]
    fn an_entry_whose_agent_sends_signed_values_unsigned_says_so_and_reads_back_with_it() {
        let yaml = r#"
id: signed_snmp
name: Signed entries
snmp:
  temperature:
    - oid: 1.3.6.1.4.1.14988.1.1.3.100.1.3
      signed: true
      where: [{oid: 1.3.6.1.4.1.14988.1.1.3.100.1.4, is: 1}]
    - 1.3.6.1.4.1.9.9.13.1.3.1.3
"#;
        let mut registry = Registry::empty();
        let platform = registry.add(yaml, Origin::User).unwrap();
        let snmp = &platform.profile.snmp;
        assert!(snmp.temperature[0].signed && !snmp.temperature[1].signed);
        let resolved = serde_json::to_value(snmp).unwrap();
        assert_eq!(resolved["temperature"][0]["signed"], true);
        assert_eq!(
            resolved["temperature"][1],
            serde_json::json!("1.3.6.1.4.1.9.9.13.1.3.1.3")
        );
        let again: SnmpSpec = serde_json::from_value(resolved).unwrap();
        assert_eq!(again.temperature, snmp.temperature);
        let err = registry
            .add(
                "id: t\nsnmp:\n  temperature:\n    - {oid: 1.3.6.1.4.1.9.9.13.1.3.1.3, signed: yes please}\n",
                Origin::User,
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid type"), "{err}");
    }

    #[test]
    fn what_an_entry_says_goes_out_with_it_however_it_was_made() {
        let mut tenths = SnmpEntry::bare("1.3.6.1.4.1.14988.1.1.3.10.0");
        assert_eq!(
            serde_json::to_value(&tenths).unwrap(),
            serde_json::json!("1.3.6.1.4.1.14988.1.1.3.10.0")
        );
        tenths.scale = Some(0.1);
        assert_eq!(
            serde_json::to_value(&tenths).unwrap(),
            serde_json::json!({"oid": "1.3.6.1.4.1.14988.1.1.3.10.0", "scale": 0.1}),
            "a scale set on an entry made from a bare OID is not dropped"
        );
    }
}
