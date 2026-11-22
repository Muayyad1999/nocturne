//! The catalogue: seven detections, fixed.
//!
//! This is deliberately a table in the source and not a rule language. Each
//! entry says what it is decided from and what it cannot see, in words that
//! are shown on the screen beside it, because a detection whose blind spots
//! are not written down gets trusted for things it never looked at.

use crate::model::Severity;
use crate::sources::{self, Features};

pub const UNPLANNED_CONFIG_CHANGE: &str = "unplanned_config_change";
pub const UNPLANNED_RELOAD: &str = "unplanned_reload";
pub const HOST_KEY_CHANGED: &str = "host_key_changed";
pub const UNREDACTED_CONFIG_VIEWS: &str = "unredacted_config_views";
pub const SECRETS_CHANGED: &str = "secrets_changed";
pub const FAILED_UNLOCKS: &str = "failed_unlocks";
pub const PROTECTED_CHANGE_OUTSIDE_WINDOW: &str = "protected_change_outside_window";

pub struct Detection {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// The severity it usually has; a hit may carry another when the
    /// catalogue says so (`secrets_changed` is `high` for a removed master
    /// password).
    pub severity: Severity,
    pub decided_from: &'static str,
    pub cannot_see: &'static str,
}

pub const CATALOGUE: [Detection; 7] = [
    Detection {
        id: UNPLANNED_CONFIG_CHANGE,
        name: "Unplanned configuration change",
        description: "A device's configuration changed while no approved change was being applied to it.",
        severity: Severity::High,
        decided_from: "A log line in which the device says it was configured, with who and from where when it says so (`logs.config_changed`), and a backup that differs from the one before it (`config.changed`, with when each version was seen from `config.versions`) — each compared with the changes the pipeline ran on that device (`changes.list`).",
        cannot_see: "A change on a device that sends no syslog and is not backed up before the next schedule. Who made it, when the device does not log that. A change by hand made between the same two backups as an approved one — including one the pipeline's own backup, taken as an approved change starts, is the first to see.",
    },
    Detection {
        id: UNPLANNED_RELOAD,
        name: "Unplanned restart",
        description: "A device restarted and nothing Nocturne was doing explains it.",
        severity: Severity::Medium,
        decided_from: "A log rule's `reload` signal (`logs.signal`), compared with the changes the pipeline ran on that device (`changes.list`), with jobs that can change a device (`changes.apply`, `changes.rollback`, `automation.ansible`) and, for a simulated device, with its lab being started, reset or given a new startup configuration (the audit trail).",
        cannot_see: "A restart of a device that sends no syslog. Why it restarted: power, a crash and somebody typing `reload` at its console all look the same from here.",
    },
    Detection {
        id: HOST_KEY_CHANGED,
        name: "SSH host key changed",
        description: "A device presented an SSH host key that is not the one Nocturne remembers for it.",
        severity: Severity::Critical,
        decided_from: "`connect.host_key_changed`, published by the core whenever a connection meets a key that differs from the remembered one.",
        cannot_see: "A key that changed on a device nobody has connected to since. Whether the device was replaced or re-keyed, or the connection is being intercepted: both look exactly like this.",
    },
    Detection {
        id: UNREDACTED_CONFIG_VIEWS,
        name: "Repeated unredacted configuration views",
        description: "Configurations were read with their secrets showing, several times in a short while.",
        severity: Severity::Medium,
        decided_from: "The audit trail: `config.view_unredacted` entries, counted over a window.",
        cannot_see: "Who: Nocturne has one operator identity, not users. An unredacted export to disk, which is one audit entry (`config.export`) however many devices it covers.",
    },
    Detection {
        id: SECRETS_CHANGED,
        name: "Credentials or master password changed",
        description: "A credential was saved or deleted, or the vault's master password was set, changed or removed.",
        severity: Severity::Info,
        decided_from: "The audit trail: `credential.save`, `credential.delete` and `vault.set_password` entries.",
        cannot_see: "Whether a saved credential is new or replaces an old one: the audit entry does not say. Who did it.",
    },
    Detection {
        id: FAILED_UNLOCKS,
        name: "Repeated failed unlocks",
        description: "Wrong vault passwords, or requests to the server with a wrong access token, several times in a short while.",
        severity: Severity::High,
        decided_from: "The audit trail: `vault.wrong_password` entries, and `server.token_refused` entries, which fold a minute of refused requests into one entry with a count.",
        cannot_see: "Somebody who guesses right the first time. Requests with no token at all are recorded but not counted here: that is a scanner or a stale browser tab, not a guess.",
    },
    Detection {
        id: PROTECTED_CHANGE_OUTSIDE_WINDOW,
        name: "Protected change outside a maintenance window",
        description: "An approved change touched protected resources on a device that no maintenance window covered at the time.",
        severity: Severity::Info,
        decided_from: "`changes.changed`, when a change with protected hits finishes, compared with the maintenance windows (`maintenance.list`).",
        cannot_see: "Whether the change was right: it was approved, by a person, with the protected resources named. This only says nobody had announced the work. A device whose change failed: the summary does not say whether anything was sent to it before it failed. When each device of a long change was done: the change is judged at the moment all of it finished.",
    },
];

pub fn find(id: &str) -> Option<&'static Detection> {
    CATALOGUE.iter().find(|detection| detection.id == id)
}

/// Why a detection cannot run in this build, when it cannot.
///
/// A detection needs every source it compares. With one missing it would
/// either never fire (nothing reports the event) or fire on everything
/// (nothing can rule the pipeline out), and both are worse than saying so.
pub fn unavailable_reason(id: &str, features: &Features) -> Option<String> {
    let changes = sources::CHANGES_LIST;
    let reason = match id {
        UNPLANNED_CONFIG_CHANGE if !features.changes => format!(
            "`{changes}` is not in this build, so a change the pipeline made could not be told from one made by hand"
        ),
        UNPLANNED_CONFIG_CHANGE if !features.logs && !features.config => {
            "neither the logs feature (`logs.config_changed`) nor the config feature (`config.changed`) is in this build, so nothing reports a configuration change".to_string()
        }
        UNPLANNED_RELOAD if !features.logs => {
            "the logs feature is not in this build, so nothing reports a restart (`logs.signal`)".to_string()
        }
        UNPLANNED_RELOAD if !features.changes => format!(
            "`{changes}` is not in this build, so a restart the change pipeline caused could not be ruled out"
        ),
        UNREDACTED_CONFIG_VIEWS if !features.config => {
            "the config feature is not in this build, so nothing records an unredacted view".to_string()
        }
        PROTECTED_CHANGE_OUTSIDE_WINDOW if !features.changes => {
            "the change engine is not in this build, so no change is ever applied (`changes.changed`)".to_string()
        }
        PROTECTED_CHANGE_OUTSIDE_WINDOW if !features.maintenance => format!(
            "`{}` is not in this build, so every protected change would look as if it were outside a window",
            sources::MAINTENANCE_LIST
        ),
        _ => return None,
    };
    Some(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_says_what_it_is_decided_from_and_what_it_cannot_see() {
        let mut ids: Vec<&str> = CATALOGUE.iter().map(|detection| detection.id).collect();
        for detection in &CATALOGUE {
            for text in [
                detection.name,
                detection.description,
                detection.decided_from,
                detection.cannot_see,
            ] {
                assert!(!text.trim().is_empty(), "{}", detection.id);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), CATALOGUE.len(), "ids are unique");
    }

    #[test]
    fn with_every_feature_present_nothing_is_unavailable() {
        let all = Features {
            changes: true,
            maintenance: true,
            alerts: true,
            logs: true,
            config: true,
        };
        for detection in &CATALOGUE {
            assert_eq!(unavailable_reason(detection.id, &all), None);
        }
        // The alert list is where a hit is raised, never what it is decided
        // from: without it every detection still runs.
        let no_alerts = Features {
            alerts: false,
            ..all
        };
        for detection in &CATALOGUE {
            assert_eq!(unavailable_reason(detection.id, &no_alerts), None);
        }
    }

    #[test]
    fn the_core_alone_runs_only_what_is_decided_from_the_core() {
        let core = Features::default();
        let available: Vec<&str> = CATALOGUE
            .iter()
            .filter(|detection| unavailable_reason(detection.id, &core).is_none())
            .map(|detection| detection.id)
            .collect();
        assert_eq!(
            available,
            [HOST_KEY_CHANGED, SECRETS_CHANGED, FAILED_UNLOCKS]
        );
    }

    #[test]
    fn one_way_of_seeing_a_configuration_change_is_enough() {
        let syslog_only = Features {
            changes: true,
            logs: true,
            ..Features::default()
        };
        assert_eq!(
            unavailable_reason(UNPLANNED_CONFIG_CHANGE, &syslog_only),
            None
        );
        let backups_only = Features {
            changes: true,
            config: true,
            ..Features::default()
        };
        assert_eq!(
            unavailable_reason(UNPLANNED_CONFIG_CHANGE, &backups_only),
            None
        );
    }
}
