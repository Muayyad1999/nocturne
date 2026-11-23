//! What the person at the keyboard can turn down, off, or up.
//!
//! Every number a check compares against lives here, with a default chosen so
//! that the check is defensible on a real campus network rather than merely
//! sensitive. A threshold is never hard-coded inside a check.
//!
//! A value of the wrong kind is caught twice. `settings.set` stores whatever
//! JSON it is handed, and one fraction where a whole number of days belongs
//! used to leave the whole section unreadable: the catalogue, the list and
//! every scan failed, automatic scans stopped without a word, and the screen
//! that would have put it right was the one that stopped drawing. So a write
//! is checked before it is stored ([`check_write`]) and refused with the
//! setting and what it has to be, and the section is read key by key
//! ([`load`]): a value stored before writes were checked — or by hand — takes
//! its default, is reported in [`Settings::problems`], and stops the checks
//! that read it, which say they could not run rather than run on a number
//! nobody chose.

use std::collections::BTreeMap;

use nocturne_app::{App, AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::catalogue::{self, Check};
use crate::model::Severity;

pub const SECTION: &str = "insights";

/// The keys whose value is an object of settings by name. A write merges
/// them key by key, and they are read and checked one entry at a time.
const NESTED: [&str; 2] = ["enabled_checks", "thresholds"];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Check id → whether it runs. Declared with every check in it, so the
    /// settings screen can list them without asking the catalogue.
    pub enabled_checks: BTreeMap<String, bool>,
    pub thresholds: Thresholds,
    /// Include simulated (lab) devices in scans.
    pub include_lab: bool,
    /// Open insights at or above this severity call `alerts.raise`.
    pub raise_alerts_from: Severity,
    /// Scan automatically (debounced) after `collect.updated` / `config.changed`.
    pub auto_scan: bool,
    /// How long to wait for more events before an automatic scan runs.
    pub auto_scan_delay_ms: u64,
    /// Stored values that could not be read, each replaced by its default.
    /// Worked out when the section is read, never stored. A check that
    /// reads one of them does not run ([`Settings::problems_for`]).
    #[serde(skip)]
    pub problems: Vec<Problem>,
}

/// A setting that cannot be used, said so that a person can put it right.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    /// Where it is in the section: `thresholds.backup_age_days`. Empty for
    /// the section as a whole.
    pub key: String,
    /// What was written or is stored there.
    pub value: Value,
    /// The setting, what it has to be, and what it is: "`thresholds.backup_age_days`
    /// must be a whole number of days (0 or more), not 7.5".
    pub message: String,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            enabled_checks: catalogue::CHECKS
                .iter()
                .map(|check| (check.id.to_string(), check.default_enabled))
                .collect(),
            thresholds: Thresholds::default(),
            include_lab: true,
            raise_alerts_from: Severity::High,
            auto_scan: true,
            auto_scan_delay_ms: 5_000,
            problems: Vec::new(),
        }
    }
}

impl Settings {
    pub fn enabled(&self, check: &str) -> bool {
        match self.enabled_checks.get(check) {
            Some(enabled) => *enabled,
            // A check added by a newer version that the stored settings have
            // never seen falls back to the catalogue, not to "on".
            None => catalogue::get(check).is_some_and(|check| check.default_enabled),
        }
    }

    /// The problems among the settings a check reads: its own switch, and the
    /// thresholds it compares against. While there is one the check does not
    /// run. Run on the default instead, it would answer with a number nobody
    /// chose, and every finding it then did not see would count as clean.
    pub fn problems_for(&self, check: &Check) -> Vec<Problem> {
        self.problems
            .iter()
            .filter(|problem| match problem.key.split_once('.') {
                Some(("thresholds", threshold)) => check.thresholds.contains(&threshold),
                Some(("enabled_checks", id)) => id == check.id,
                Some(_) => false,
                None => match problem.key.as_str() {
                    "" | "enabled_checks" => true,
                    "thresholds" => !check.thresholds.is_empty(),
                    _ => false,
                },
            })
            .cloned()
            .collect()
    }

    /// Whether the check's own switch could be read: when it could not,
    /// whether the check is meant to be on is not known.
    pub fn switch_readable(&self, check: &str) -> bool {
        let own = format!("enabled_checks.{check}");
        !self.problems.iter().any(|problem| {
            problem.key.is_empty() || problem.key == "enabled_checks" || problem.key == own
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// Up/down transitions within an hour that count as flapping.
    pub flap_transitions_per_hour: u32,
    /// Input errors per million packets, between the last two collections.
    pub interface_error_ppm: f64,
    /// …and at least this many new errors, so that a handful on a busy port
    /// (a cable reseat, a speed change) is not a finding.
    pub interface_error_min_delta: u64,
    /// Interface utilisation that counts as congested.
    pub utilisation_pct: f64,
    /// CPU / memory / temperature that count as too high.
    pub cpu_pct: f64,
    pub memory_pct: f64,
    pub temperature_c: f64,
    /// Packets a second a QoS class must be dropping to count as dropping:
    /// congestion drops where the device reports them apart, every drop
    /// where it does not. The same figure as monitoring's built-in rule, so
    /// an alert and an insight about one class agree on what "dropping" is.
    pub qos_drop_ps: f64,
    /// A metric must stay over its threshold for this long to count as sustained.
    pub sustained_minutes: u32,
    /// How far back the metric and log history is read.
    pub history_hours: u32,
    /// A backup older than this is "no recent backup".
    pub backup_age_days: u32,
    /// Collected data older than this is not used for cross-device checks.
    pub stale_data_hours: u32,
    /// MAC addresses that must move between the same two ports before it
    /// counts as a flap (one host moving desks is not a loop).
    pub mac_flap_min_macs: u32,
    /// Devices of one model needed before a version majority means anything.
    pub software_min_group: u32,
    /// Running and startup must have been captured this close together for
    /// "unsaved configuration" to be decidable.
    pub saved_pair_window_minutes: u32,
    /// Two ARP tables must have been collected this close together before
    /// disagreeing about an address counts as a duplicate.
    pub arp_window_minutes: u32,
    /// How far from a configuration change an applied Nocturne change may be
    /// and still account for it.
    pub change_window_minutes: u32,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds {
            flap_transitions_per_hour: 4,
            interface_error_ppm: 100.0,
            interface_error_min_delta: 10,
            utilisation_pct: 80.0,
            cpu_pct: 85.0,
            memory_pct: 90.0,
            temperature_c: 65.0,
            qos_drop_ps: 10.0,
            sustained_minutes: 15,
            history_hours: 24,
            backup_age_days: 7,
            stale_data_hours: 168,
            mac_flap_min_macs: 3,
            software_min_group: 3,
            saved_pair_window_minutes: 60,
            arp_window_minutes: 60,
            change_window_minutes: 120,
        }
    }
}

/// The section as stored. A stored value that cannot be read never makes this
/// fail (see the module comment); a store that cannot be read at all does, as
/// it does everywhere else.
pub fn load(app: &App) -> AppResult<Settings> {
    Ok(read(&nocturne_app::settings::get_value(app, SECTION)?))
}

/// The section, key by key: whatever can be read is kept, and every value
/// that cannot takes its default and is reported.
pub(crate) fn read(stored: &Value) -> Settings {
    // Nearly always the whole section reads at once.
    if let Ok(settings) = serde_json::from_value::<Settings>(stored.clone()) {
        return settings;
    }
    let Some(fields) = stored.as_object() else {
        return Settings {
            problems: vec![problem("", stored)],
            ..Settings::default()
        };
    };
    let mut usable = fields.clone();
    let mut problems = Vec::new();
    for (key, value) in fields {
        for (name, bad) in unreadable(key, value) {
            match name {
                Some(name) => {
                    if let Some(entries) = usable.get_mut(key).and_then(Value::as_object_mut) {
                        entries.remove(name);
                    }
                }
                None => {
                    usable.remove(key);
                }
            }
            problems.push(problem(&path(key, name), bad));
        }
    }
    // By key, not by where the store happened to keep each one.
    problems.sort_by(|a, b| a.key.cmp(&b.key));
    match serde_json::from_value::<Settings>(Value::Object(usable)) {
        Ok(settings) => Settings {
            problems,
            ..settings
        },
        // Every value left was read on its own, so this is not expected. If
        // it happens all the same, nothing stored is used, and that is said.
        Err(_) => Settings {
            problems: vec![problem("", stored)],
            ..Settings::default()
        },
    }
}

/// Whether a write to the section may be stored: every value it sets has to
/// be readable as what its key holds. The core's `settings.set` asks before it
/// stores anything, so a refused write changes nothing at all. Keys this
/// version does not know are let through, as they always were: they do
/// nothing, and refusing them would refuse a section sent back as it was read.
pub fn check_write(patch: &Value) -> AppResult<()> {
    let Some(fields) = patch.as_object() else {
        // The core refuses a write that is not an object of keys itself.
        return Ok(());
    };
    let mut problems: Vec<Problem> = fields
        .iter()
        .flat_map(|(key, value)| {
            unreadable(key, value)
                .into_iter()
                .map(move |(name, bad)| problem(&path(key, name), bad))
        })
        .collect();
    if problems.is_empty() {
        return Ok(());
    }
    problems.sort_by(|a, b| a.key.cmp(&b.key));
    let message = problems
        .iter()
        .map(|problem| problem.message.as_str())
        .collect::<Vec<_>>()
        .join("; ");
    Err(AppError::invalid(message).with_details(json!({
        "section": SECTION,
        "problems": problems,
    })))
}

/// The values under one top-level key that cannot be read as what it holds,
/// as `(name inside it, value)` — `None` when the value as a whole is of the
/// wrong kind. Each is tried on its own over the defaults, so serde decides
/// what fits exactly as it does when the section is read.
fn unreadable<'a>(key: &'a str, value: &'a Value) -> Vec<(Option<&'a str>, &'a Value)> {
    if !NESTED.contains(&key) {
        return if readable(one(key, value.clone())) {
            Vec::new()
        } else {
            vec![(None, value)]
        };
    }
    let Some(entries) = value.as_object() else {
        return vec![(None, value)];
    };
    entries
        .iter()
        .filter(|(name, entry)| !readable(one(key, one(name, (*entry).clone()))))
        .map(|(name, entry)| (Some(name.as_str()), entry))
        .collect()
}

fn readable(probe: Value) -> bool {
    serde_json::from_value::<Settings>(probe).is_ok()
}

fn one(key: &str, value: Value) -> Value {
    let mut object = Map::new();
    object.insert(key.to_string(), value);
    Value::Object(object)
}

fn path(key: &str, name: Option<&str>) -> String {
    match name {
        Some(name) => format!("{key}.{name}"),
        None => key.to_string(),
    }
}

fn problem(key: &str, value: &Value) -> Problem {
    let mut shown = value.to_string();
    if shown.chars().count() > 40 {
        shown = format!("{}…", shown.chars().take(40).collect::<String>());
    }
    let wanted = expected(key);
    let message = if key.is_empty() {
        format!("the `{SECTION}` settings must be an object of settings by name, not {shown}")
    } else if wanted.starts_with("a whole number") && value.as_u64().is_some() {
        // A whole number that is not 0 or more is negative, and one that
        // still does not fit is past what the setting can hold.
        format!("`{key}` must be {wanted}, not {shown}, which is too large")
    } else {
        format!("`{key}` must be {wanted}, not {shown}")
    };
    Problem {
        key: key.to_string(),
        value: value.clone(),
        message,
    }
}

/// What a setting has to be, in words.
fn expected(key: &str) -> &'static str {
    match key {
        "enabled_checks" => "an object of check ids, each true or false",
        "thresholds" => "an object of thresholds by name",
        "include_lab" | "auto_scan" => "true or false",
        "raise_alerts_from" => "one of info, low, medium, high or critical",
        "auto_scan_delay_ms" => "a whole number of milliseconds (0 or more)",
        "thresholds.flap_transitions_per_hour" => "a whole number of transitions (0 or more)",
        "thresholds.interface_error_ppm" => "a number of errors per million packets",
        "thresholds.interface_error_min_delta" => "a whole number of errors (0 or more)",
        "thresholds.utilisation_pct" | "thresholds.cpu_pct" | "thresholds.memory_pct" => {
            "a number (a percentage)"
        }
        "thresholds.temperature_c" => "a number of degrees Celsius",
        "thresholds.qos_drop_ps" => "a number of packets a second",
        "thresholds.sustained_minutes"
        | "thresholds.saved_pair_window_minutes"
        | "thresholds.arp_window_minutes"
        | "thresholds.change_window_minutes" => "a whole number of minutes (0 or more)",
        "thresholds.history_hours" | "thresholds.stale_data_hours" => {
            "a whole number of hours (0 or more)"
        }
        "thresholds.backup_age_days" => "a whole number of days (0 or more)",
        "thresholds.mac_flap_min_macs" => "a whole number of addresses (0 or more)",
        "thresholds.software_min_group" => "a whole number of devices (0 or more)",
        key if key.starts_with("enabled_checks.") => "true or false",
        _ => "the kind of value its default is",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_check_has_a_switch_and_unknown_ones_fall_back_to_the_catalogue() {
        let settings = Settings::default();
        assert_eq!(settings.enabled_checks.len(), catalogue::CHECKS.len());
        assert!(settings.enabled("duplex_mismatch"));
        assert!(!settings.enabled("not_a_check"));
        let mut trimmed = settings.clone();
        trimmed.enabled_checks.clear();
        assert!(trimmed.enabled("duplex_mismatch"));
    }

    #[test]
    fn settings_round_trip_through_json() {
        let json = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(json["raise_alerts_from"], "high");
        assert_eq!(json["thresholds"]["flap_transitions_per_hour"], 4);
        assert!(json.get("problems").is_none(), "worked out, never stored");
        let back: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(back, Settings::default());
        // A stored patch of one threshold keeps the rest.
        let patched: Settings =
            serde_json::from_value(serde_json::json!({"thresholds": {"cpu_pct": 95.0}})).unwrap();
        assert_eq!(patched.thresholds.cpu_pct, 95.0);
        assert_eq!(
            patched.thresholds.memory_pct,
            Thresholds::default().memory_pct
        );
    }

    #[test]
    fn every_setting_is_described_in_words() {
        let defaults = serde_json::to_value(Settings::default()).unwrap();
        let mut keys: Vec<String> = Vec::new();
        for (key, value) in defaults.as_object().unwrap() {
            keys.push(key.clone());
            if key == "thresholds" {
                keys.extend(
                    value
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(|name| format!("thresholds.{name}")),
                );
            }
        }
        keys.extend(
            catalogue::CHECKS
                .iter()
                .map(|check| format!("enabled_checks.{}", check.id)),
        );
        for key in keys {
            assert_ne!(
                expected(&key),
                expected("not.a.setting"),
                "`{key}` has no words"
            );
        }
    }

    #[test]
    fn a_section_stored_with_bad_values_keeps_the_good_ones_and_names_the_bad() {
        let read = read(&json!({
            "enabled_checks": {"duplex_mismatch": false, "telnet_enabled": "no"},
            "thresholds": {"backup_age_days": 7.5, "cpu_pct": 95, "history_hours": 48},
            "raise_alerts_from": "urgent",
            "auto_scan": false,
        }));
        let keys: Vec<&str> = read
            .problems
            .iter()
            .map(|problem| problem.key.as_str())
            .collect();
        assert_eq!(
            keys,
            [
                "enabled_checks.telnet_enabled",
                "raise_alerts_from",
                "thresholds.backup_age_days"
            ]
        );
        // What could be read is what was stored...
        assert!(!read.enabled("duplex_mismatch"));
        assert_eq!(read.thresholds.cpu_pct, 95.0);
        assert_eq!(read.thresholds.history_hours, 48);
        assert!(!read.auto_scan);
        // ...and what could not is the default, with its problem beside it.
        assert_eq!(read.thresholds.backup_age_days, 7);
        assert_eq!(read.raise_alerts_from, Severity::High);
        assert_eq!(
            read.problems[2].message,
            "`thresholds.backup_age_days` must be a whole number of days (0 or more), not 7.5"
        );
        assert!(!read.switch_readable("telnet_enabled"));
        assert!(read.switch_readable("duplex_mismatch"));

        // A threshold that is not an object, and a section that is not one.
        let flat = super::read(&json!({"thresholds": 5}));
        assert_eq!(flat.problems[0].key, "thresholds");
        assert_eq!(flat.thresholds, Thresholds::default());
        let nothing = super::read(&json!(3));
        assert_eq!(nothing.problems[0].key, "");
        assert!(
            nothing.problems[0]
                .message
                .starts_with("the `insights` settings must be"),
            "{}",
            nothing.problems[0].message
        );
    }

    #[test]
    fn a_problem_stops_the_checks_that_read_it_and_no_others() {
        let read = read(&json!({"thresholds": {"sustained_minutes": "fifteen"}}));
        let blocked: Vec<&str> = catalogue::CHECKS
            .iter()
            .filter(|check| !read.problems_for(check).is_empty())
            .map(|check| check.id)
            .collect();
        assert_eq!(
            blocked,
            [
                "high_utilisation",
                "high_cpu",
                "high_memory",
                "high_temperature",
                "qos_class_dropping"
            ]
        );
        let switches = read_switch_problem();
        assert_eq!(
            catalogue::CHECKS
                .iter()
                .filter(|check| !switches.problems_for(check).is_empty())
                .map(|check| check.id)
                .collect::<Vec<_>>(),
            ["telnet_enabled"]
        );
    }

    fn read_switch_problem() -> Settings {
        read(&json!({"enabled_checks": {"telnet_enabled": null}}))
    }

    #[test]
    fn a_write_is_refused_for_what_it_sets_and_not_for_what_is_already_stored() {
        assert!(check_write(&json!({"thresholds": {"backup_age_days": 30}})).is_ok());
        assert!(check_write(&json!({"thresholds": {"cpu_pct": 95}})).is_ok());
        assert!(
            check_write(&json!({"thresholds": {"not_a_threshold": "x"}})).is_ok(),
            "a key this version does not know does nothing, as before"
        );
        let refused = check_write(&json!({
            "thresholds": {"backup_age_days": 7.5, "cpu_pct": "high"},
            "include_lab": "yes",
        }))
        .unwrap_err();
        assert_eq!(refused.code, "invalid");
        let keys: Vec<String> = refused.details.as_ref().unwrap()["problems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|problem| problem["key"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            keys,
            [
                "include_lab",
                "thresholds.backup_age_days",
                "thresholds.cpu_pct"
            ]
        );
        assert!(
            refused
                .message
                .contains("`include_lab` must be true or false, not \"yes\""),
            "{}",
            refused.message
        );
        let large =
            check_write(&json!({"thresholds": {"history_hours": 4_294_967_296u64}})).unwrap_err();
        assert!(
            large.message.ends_with("which is too large"),
            "{}",
            large.message
        );
    }
}
