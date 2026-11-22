//! The other features, reached by method name.
//!
//! This crate links to none of them. What it needs from one it asks for
//! through `App::rpc`, and every answer is one of three things: the data,
//! "that feature is not in this build" ([`Gap::Absent`]) or "it is, and it
//! failed" ([`Gap::Failed`]). The difference matters more here than
//! anywhere: "no approved change was found" must never be concluded from a
//! change engine that is missing or did not answer.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use nocturne_app::App;

use crate::DetectState;

pub const CHANGES_LIST: &str = "changes.list";
pub const MAINTENANCE_LIST: &str = "maintenance.list";
pub const ALERTS_RAISE: &str = "alerts.raise";
/// An event has no registration to look for, so the feature that publishes
/// it is recognised by a method it always registers: the log rules are what
/// produce `logs.signal` and `logs.config_changed`.
pub const LOGS_PROBE: &str = "logs.rules.list";
/// The backup job is what publishes `config.changed`, and the same feature
/// writes the `config.view_unredacted` audit entries.
pub const CONFIG_PROBE: &str = "config.backup";
/// Where the config feature says when it last fetched a version unchanged,
/// which the core's own record of a version does not hold.
pub const CONFIG_VERSIONS: &str = "config.versions";

/// How long another feature gets to answer. These are in-process calls over
/// a local database; one that takes longer than this is stuck, and a stuck
/// neighbour must not stop every later event from being judged.
const BUDGET: Duration = Duration::from_secs(10);

/// Which of the features this one leans on are in the build.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub changes: bool,
    pub maintenance: bool,
    pub alerts: bool,
    pub logs: bool,
    pub config: bool,
}

/// The method table cannot change after the application is built, so this is
/// read once.
pub fn features(app: &App) -> Features {
    *app.state::<DetectState>().features.get_or_init(|| {
        let methods = app.methods();
        let has = |name: &str| methods.binary_search(&name).is_ok();
        Features {
            changes: has(CHANGES_LIST),
            maintenance: has(MAINTENANCE_LIST),
            alerts: has(ALERTS_RAISE),
            logs: has(LOGS_PROBE),
            config: has(CONFIG_PROBE),
        }
    })
}

/// Why another feature gave no answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gap {
    /// The method is not registered in this build.
    Absent(&'static str),
    /// The method is there and did not give a usable answer.
    Failed {
        method: &'static str,
        message: String,
    },
}

impl Gap {
    pub fn failed(method: &'static str, message: impl Into<String>) -> Gap {
        Gap::Failed {
            method,
            message: message.into(),
        }
    }

    pub fn sentence(&self) -> String {
        match self {
            Gap::Absent(method) => format!("`{method}` is not in this build"),
            Gap::Failed { method, message } => format!("`{method}` failed: {message}"),
        }
    }
}

pub async fn call(app: &App, method: &'static str, params: Value) -> Result<Value, Gap> {
    match tokio::time::timeout(BUDGET, app.rpc(method, params)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) if err.code == "unknown_method" => Err(Gap::Absent(method)),
        Ok(Err(err)) => Err(Gap::failed(method, err.message)),
        Err(_) => Err(Gap::failed(
            method,
            format!("no answer within {} seconds", BUDGET.as_secs()),
        )),
    }
}

// ----------------------------------------------------------------- changes

/// What this feature reads of a change in `changes.list`
/// (`docs/dev/features/changes.md`). Everything it does not read is ignored,
/// so the change engine may add fields freely.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Change {
    pub id: String,
    pub title: String,
    /// `draft`, `planned`, `approved`, `applying`, `applied`, `failed`,
    /// `rolled_back`, `partially_applied` or `cancelled`.
    pub state: String,
    /// Stamped on every save, so for a finished change it is the time it
    /// reached the state it is in.
    pub updated_at: i64,
    /// Set while a job — an apply or a rollback — is working on the change.
    pub job_id: Option<String>,
    pub protected_hits: u64,
    pub devices: Vec<ChangeDevice>,
}

#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct ChangeDevice {
    pub device_id: String,
    /// `pending`, `applying`, `applied`, `failed`, `rolled_back`,
    /// `rollback_failed`, `skipped` or `guard_fired`; absent before an apply.
    pub state: Option<String>,
    pub protected_hits: u64,
}

impl Change {
    pub fn device(&self, device_id: &str) -> Option<&ChangeDevice> {
        self.devices
            .iter()
            .find(|device| device.device_id == device_id)
    }
}

pub fn parse_change(value: Value) -> Result<Change, Gap> {
    let change: Change = serde_json::from_value(value).map_err(|err| {
        Gap::failed(
            CHANGES_LIST,
            format!("a change came back in a shape this feature cannot read ({err})"),
        )
    })?;
    // Without these two nothing can be decided from it; guessing a state
    // would be deciding from nothing.
    if change.id.is_empty() || change.state.is_empty() {
        return Err(Gap::failed(
            CHANGES_LIST,
            "a change came back without an id or a state",
        ));
    }
    Ok(change)
}

/// The page size `changes.list` allows at most.
const CHANGES_PAGE: usize = 500;
/// Twenty-five thousand changes for one device is past anything real; the
/// bound is there so a cursor that never ends cannot hold the judge for ever.
const CHANGES_PAGES: usize = 50;

/// Every change that names a device.
///
/// All of them rather than the first page: the list is ordered by when a
/// change was created, and one drafted months ago may have been applied a
/// minute ago.
pub async fn changes_for(app: &App, device_id: &str) -> Result<Vec<Change>, Gap> {
    let mut changes: Vec<Change> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..CHANGES_PAGES {
        let mut params = json!({ "device_id": device_id, "limit": CHANGES_PAGE });
        if let Some(cursor) = &cursor {
            params["cursor"] = json!(cursor);
        }
        let Value::Array(rows) = call(app, CHANGES_LIST, params).await? else {
            return Err(Gap::failed(CHANGES_LIST, "the answer was not a list"));
        };
        let full = rows.len() >= CHANGES_PAGE;
        for row in rows {
            changes.push(parse_change(row)?);
        }
        if !full {
            return Ok(changes);
        }
        cursor = changes.last().map(|change| change.id.clone());
    }
    Err(Gap::failed(
        CHANGES_LIST,
        "the list of changes for this device did not end",
    ))
}

// ------------------------------------------------------------------ config

/// What this feature reads of a stored configuration version in
/// `config.versions` (`docs/dev/features/config.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub struct Version {
    pub id: i64,
    /// When this content was first fetched.
    pub first_seen: i64,
    /// When a fetch last returned exactly this content: the version was
    /// still the device's configuration then.
    pub last_seen: i64,
}

/// One stored version of a device's configuration, or `None` when it is no
/// longer stored.
///
/// Asked of the config feature rather than read from the core's store: only
/// that feature knows `last_seen`, and without it the stretch a difference
/// was made in would start when the earlier version was first stored —
/// weeks back, where the approved change that produced that version is.
pub async fn version(
    app: &App,
    device_id: &str,
    kind: &str,
    id: i64,
) -> Result<Option<Version>, Gap> {
    let Some(above) = id.checked_add(1) else {
        return Ok(None);
    };
    // Newest first below `before_id`, so the first row is this version when
    // it is still there and an older one when it is not.
    let params = json!({"device_id": device_id, "kind": kind, "before_id": above, "limit": 1});
    let Value::Array(rows) = call(app, CONFIG_VERSIONS, params).await? else {
        return Err(Gap::failed(CONFIG_VERSIONS, "the answer was not a list"));
    };
    for row in rows {
        let version: Version = serde_json::from_value(row).map_err(|err| {
            Gap::failed(
                CONFIG_VERSIONS,
                format!("a version came back in a shape this feature cannot read ({err})"),
            )
        })?;
        if version.id == id {
            return Ok(Some(version));
        }
    }
    Ok(None)
}

// ------------------------------------------------------------- maintenance

/// What this feature reads of a maintenance window in `maintenance.list`
/// (`docs/dev/features/monitor.md`).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Window {
    pub id: String,
    pub name: String,
    pub scope: Scope,
    pub starts_at: i64,
    pub ends_at: i64,
    /// Absent for a one-off window; `weekly` is the only recurrence there is.
    pub recurring: Option<String>,
}

/// Which devices a window covers. Empty lists mean "any".
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Scope {
    pub sites: Vec<String>,
    pub roles: Vec<String>,
    pub tags: Vec<String>,
    pub device_ids: Vec<String>,
}

pub async fn windows(app: &App) -> Result<Vec<Window>, Gap> {
    let Value::Array(rows) = call(app, MAINTENANCE_LIST, json!({})).await? else {
        return Err(Gap::failed(MAINTENANCE_LIST, "the answer was not a list"));
    };
    let mut windows = Vec::with_capacity(rows.len());
    for row in rows {
        let window: Window = serde_json::from_value(row).map_err(|err| {
            Gap::failed(
                MAINTENANCE_LIST,
                format!("a window came back in a shape this feature cannot read ({err})"),
            )
        })?;
        // A recurrence this feature has never heard of cannot be evaluated,
        // and treating it as a one-off would report changes made inside it.
        if let Some(recurring) = &window.recurring
            && recurring != "weekly"
        {
            return Err(Gap::failed(
                MAINTENANCE_LIST,
                format!(
                    "the window `{}` recurs `{recurring}`, which this feature does not know how to evaluate",
                    window.name
                ),
            ));
        }
        windows.push(window);
    }
    Ok(windows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_read_from_what_the_change_engine_really_sends() {
        // Copied from `changes.list` in a build with the change engine
        // (`nocturne-changes/src/model.rs`, `ChangeSummary`).
        let change = parse_change(json!({
            "id": "c1", "title": "NTP", "description": "", "origin": "user",
            "state": "applied", "ticket": null, "created_at": 10, "updated_at": 20,
            "created_by": "user", "job_id": null, "risk": "low", "protected_hits": 2,
            "approved_by": "user",
            "devices": [{"device_id": "d1", "name": "SW1", "planned": true, "risk": "low",
                         "protected_hits": 2, "commands": 3, "state": "applied"}],
        }))
        .unwrap();
        assert_eq!((change.state.as_str(), change.updated_at), ("applied", 20));
        assert_eq!(
            change.device("d1").unwrap().state.as_deref(),
            Some("applied")
        );
        assert_eq!(change.device("d1").unwrap().protected_hits, 2);
        assert!(change.device("d2").is_none());
    }

    #[test]
    fn a_version_is_read_from_what_the_config_feature_really_sends() {
        // The fields of `ConfigVersion` in `nocturne-config/src/versions.rs`,
        // as `config.versions` serialises them.
        let version: Version = serde_json::from_value(json!({
            "id": 7, "device_id": "d1", "kind": "running", "sha256": "ab", "lines": 40,
            "bytes": 900, "first_seen": 100, "last_seen": 250, "origin": "scheduled",
            "pinned": false,
        }))
        .unwrap();
        assert_eq!(
            version,
            Version {
                id: 7,
                first_seen: 100,
                last_seen: 250
            }
        );
        // Without the times there is nothing to bound a change with, and a
        // missing time is not the year 1970.
        assert!(serde_json::from_value::<Version>(json!({"id": 7})).is_err());
    }

    #[test]
    fn a_change_without_a_state_is_a_failed_read_not_an_idle_change() {
        let gap = parse_change(json!({"id": "c1"})).unwrap_err();
        assert!(matches!(gap, Gap::Failed { .. }), "{gap:?}");
        let gap = parse_change(json!("c1")).unwrap_err();
        assert!(gap.sentence().contains("changes.list"), "{gap:?}");
    }
}
