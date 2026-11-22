//! What the person at the keyboard can turn down, off, or up.
//!
//! Every number a check compares against lives here, with a default chosen so
//! that the check is defensible on a real campus network rather than merely
//! sensitive. A threshold is never hard-coded inside a check.

use std::collections::BTreeMap;

use nocturne_app::{App, AppResult};
use serde::{Deserialize, Serialize};

use crate::catalogue;
use crate::model::Severity;

pub const SECTION: &str = "insights";

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

pub fn load(app: &App) -> AppResult<Settings> {
    nocturne_app::settings::get(app, SECTION)
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
}
