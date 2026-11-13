//! What is backed up and what is not saved: the coverage table and the
//! devices whose running configuration differs from their startup configuration.

use std::collections::HashMap;

use nocturne_app::inventory::DeviceFilter;
use nocturne_app::{App, AppResult, Router};
use nocturne_drivers::hier;
use serde::{Deserialize, Serialize};

use crate::backup::{self, BackupStatus};
use crate::scope::{self, Target};
use crate::versions::{self, ConfigVersion, Kind};
use crate::{db, outline};

/// The latest stored version of one kind, as the coverage table shows it.
#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub version_id: i64,
    /// When a fetch last returned this content.
    pub fetched_at: i64,
    pub lines: u32,
    pub sha256: String,
}

impl From<ConfigVersion> for Snapshot {
    fn from(version: ConfigVersion) -> Self {
        Snapshot {
            version_id: version.id,
            fetched_at: version.last_seen,
            lines: version.lines,
            sha256: version.sha256,
        }
    }
}

/// One row of the backup coverage table.
#[derive(Debug, Serialize)]
pub struct Coverage {
    pub device_id: String,
    pub name: String,
    /// The device type in use; empty when it is not known yet.
    pub profile: String,
    pub lab: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup: Option<Snapshot>,
    /// Stored versions of both kinds.
    pub versions: u64,
    /// When the running configuration was last seen to differ from the version before it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_change_at: Option<i64>,
    /// Why the most recent backup attempt failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<i64>,
    /// Why this device is not backed up at all (monitor-only, no configuration command).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

fn statuses(app: &App) -> AppResult<HashMap<String, BackupStatus>> {
    Ok(app
        .store()
        .list::<BackupStatus>(backup::STATUS)?
        .into_iter()
        .map(|status| (status.device_id.clone(), status))
        .collect())
}

pub(crate) fn devices(app: &App, targets: Vec<Target>) -> AppResult<Vec<Coverage>> {
    let ids: Vec<&str> = targets
        .iter()
        .map(|target| target.device.id.as_str())
        .collect();
    let mut running = versions::latest_of_each(app, &ids, Kind::Running)?;
    let mut startup = versions::latest_of_each(app, &ids, Kind::Startup)?;
    let mut statuses = statuses(app)?;
    // (all versions, running versions) per device.
    let mut counts: HashMap<String, (u64, u64)> = HashMap::new();
    for (device_id, kind, count) in db::counts(app.store())? {
        let entry = counts.entry(device_id).or_default();
        entry.0 += count;
        if kind == Kind::Running.as_str() {
            entry.1 += count;
        }
    }

    Ok(targets
        .into_iter()
        .map(|target| {
            let id = &target.device.id;
            let (versions, running_versions) = counts.get(id).copied().unwrap_or_default();
            let running = running.remove(id);
            let status = statuses.remove(id).unwrap_or_default();
            Coverage {
                // A first backup is not a change: there has to be a version it differs from.
                last_change_at: running
                    .as_ref()
                    .filter(|_| running_versions > 1)
                    .map(|version| version.first_seen),
                running: running.map(Snapshot::from),
                startup: startup.remove(id).map(Snapshot::from),
                versions,
                last_error: status.last_error,
                last_attempt_at: status.last_attempt_at,
                skipped: status.skipped,
                lab: target.device.lab.is_some(),
                profile: target.profile,
                name: target.device.name,
                device_id: target.device.id,
            }
        })
        .collect())
}

/// A device whose running configuration differs from its startup configuration.
#[derive(Debug, Serialize)]
pub struct Unsaved {
    pub device_id: String,
    pub name: String,
    pub lab: bool,
    pub running_version_id: i64,
    pub startup_version_id: i64,
    /// Lines in the running configuration that the startup configuration lacks.
    pub added: usize,
    /// Lines in the startup configuration that the running configuration lacks.
    pub removed: usize,
    /// When the two were read (in one session).
    pub fetched_at: i64,
}

/// A device for which the question cannot be answered, and why.
#[derive(Debug, Serialize)]
pub struct NotComparable {
    pub device_id: String,
    pub name: String,
    pub lab: bool,
    pub reason: String,
}

#[derive(Debug, Default, Serialize)]
pub struct UnsavedReport {
    pub devices: Vec<Unsaved>,
    /// Devices whose running and startup configurations agree.
    pub saved: usize,
    pub not_comparable: Vec<NotComparable>,
}

/// Whether a device's running configuration is saved, as far as its last backup can tell.
enum Saved {
    Yes,
    No(Unsaved),
    /// There is no pair of configurations to compare; says why.
    Unknown(String),
}

fn saved(app: &App, target: &Target, status: &BackupStatus) -> AppResult<Saved> {
    let Some(fetched_at) = status.last_success_at else {
        return Ok(Saved::Unknown(
            match (&status.skipped, &status.last_error) {
                (Some(reason), _) => format!("not backed up: {reason}"),
                (None, Some(error)) => format!("no backup yet; the last attempt failed: {error}"),
                (None, None) => "no backup yet".to_string(),
            },
        ));
    };
    let captured = &status.captured;
    let absent = |kind: Kind, note: &Option<String>| {
        note.clone()
            .unwrap_or_else(|| format!("the {} was not part of the last backup", kind.label()))
    };
    let (running_id, startup_id) = match (captured.running, captured.startup) {
        (Some(running), Some(startup)) => (running, startup),
        (None, _) => {
            return Ok(Saved::Unknown(absent(
                Kind::Running,
                &captured.running_note,
            )));
        }
        (_, None) => {
            return Ok(Saved::Unknown(absent(
                Kind::Startup,
                &captured.startup_note,
            )));
        }
    };
    let (Ok((running, running_text)), Ok((startup, startup_text))) = (
        versions::read(app, running_id),
        versions::read(app, startup_id),
    ) else {
        return Ok(Saved::Unknown(
            "the versions of the last backup are no longer stored; back the device up again".into(),
        ));
    };
    if running.sha256 == startup.sha256 {
        return Ok(Saved::Yes);
    }
    // Compare what the device is configured to do, not how the two commands
    // print it: re-apply what the device type ignores, drop comments and display lines.
    let syntax = target.syntax(app);
    let platform = target.platform(app);
    let comparable = |text: &str| match &platform {
        Some(platform) => outline::canonical(&platform.clean_config(text), syntax),
        None => outline::canonical(text, syntax),
    };
    let diff = hier::text_diff(
        &comparable(&startup_text),
        &comparable(&running_text),
        Some(0),
    );
    if diff.added == 0 && diff.removed == 0 {
        return Ok(Saved::Yes);
    }
    Ok(Saved::No(Unsaved {
        device_id: target.device.id.clone(),
        name: target.device.name.clone(),
        lab: target.device.lab.is_some(),
        running_version_id: running_id,
        startup_version_id: startup_id,
        added: diff.added,
        removed: diff.removed,
        fetched_at,
    }))
}

pub(crate) fn unsaved(app: &App, targets: Vec<Target>) -> AppResult<UnsavedReport> {
    let mut statuses = statuses(app)?;
    let mut report = UnsavedReport::default();
    for target in targets {
        let status = statuses.remove(&target.device.id).unwrap_or_default();
        match saved(app, &target, &status)? {
            Saved::No(unsaved) => report.devices.push(unsaved),
            Saved::Yes => report.saved += 1,
            Saved::Unknown(reason) => report.not_comparable.push(NotComparable {
                lab: target.device.lab.is_some(),
                name: target.device.name,
                device_id: target.device.id,
                reason,
            }),
        }
    }
    Ok(report)
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct ScopeParams {
    device_ids: Option<Vec<String>>,
    filter: Option<DeviceFilter>,
}

pub(crate) fn register(router: &mut Router) {
    router.add(
        "config.devices",
        |app: App, params: ScopeParams| async move {
            let targets =
                scope::targets(&app, params.device_ids.as_deref(), params.filter.as_ref())?;
            devices(&app, targets)
        },
    );
    router.add(
        "config.unsaved",
        |app: App, params: ScopeParams| async move {
            let targets =
                scope::targets(&app, params.device_ids.as_deref(), params.filter.as_ref())?;
            unsaved(&app, targets)
        },
    );
}
