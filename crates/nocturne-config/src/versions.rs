//! Stored configuration versions: reading, pinning, deleting and retention.

use std::collections::BTreeMap;

use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult, Router, inventory};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::db;
use crate::redact::Redaction;
use crate::scope;
use crate::settings::ConfigSettings;

/// Which of a device's configurations a version is.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    Running,
    Startup,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Running => "running",
            Kind::Startup => "startup",
        }
    }

    /// The name engineers use for it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Running => "running-config",
            Kind::Startup => "startup-config",
        }
    }
}

/// One stored version of a device's configuration (without its text).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigVersion {
    pub id: i64,
    pub device_id: String,
    /// `running` or `startup`.
    pub kind: String,
    pub sha256: String,
    pub lines: u32,
    pub bytes: u64,
    /// When this content was first fetched.
    pub first_seen: i64,
    /// When a fetch last returned exactly this content.
    pub last_seen: i64,
    /// What stored it: `manual`, `scheduled`, `assistant`, `event`, `pre-change`, `post-change`, …
    pub origin: String,
    /// Pinned versions are never pruned or deleted.
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pin_reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

fn from_row(app: &App, row: db::VersionRow) -> AppResult<ConfigVersion> {
    // A version the change pipeline stored has no metadata yet: measure it once.
    let bytes = match row.bytes {
        Some(bytes) => bytes,
        None => {
            let (_, text) = app.store().config_get(row.id)?;
            let bytes = text.len() as u64;
            db::record(app.store(), row.id, row.taken_at, bytes)?;
            bytes
        }
    };
    Ok(ConfigVersion {
        id: row.id,
        device_id: row.device_id,
        kind: row.kind,
        sha256: row.sha256,
        lines: row.lines,
        bytes,
        first_seen: row.taken_at,
        last_seen: row.last_seen.unwrap_or(row.taken_at).max(row.taken_at),
        origin: row.source,
        pinned: !row.pins.is_empty(),
        pin_reasons: row.pins,
        note: row.note,
    })
}

fn not_found(id: i64) -> AppError {
    AppError::not_found(format!("configuration version {id}"))
}

pub fn get(app: &App, id: i64) -> AppResult<ConfigVersion> {
    let row = db::version(app.store(), id)?.ok_or_else(|| not_found(id))?;
    from_row(app, row)
}

/// A version and its text, exactly as stored.
pub fn read(app: &App, id: i64) -> AppResult<(ConfigVersion, String)> {
    let version = get(app, id)?;
    let (_, text) = app.store().config_get(id)?;
    Ok((version, text))
}

/// The newest `count` versions of a device, newest first.
pub fn newest(
    app: &App,
    device_id: &str,
    kind: Kind,
    count: usize,
) -> AppResult<Vec<ConfigVersion>> {
    db::newest(app.store(), device_id, kind.as_str(), count)?
        .into_iter()
        .map(|row| from_row(app, row))
        .collect()
}

pub fn latest(app: &App, device_id: &str, kind: Kind) -> AppResult<Option<ConfigVersion>> {
    Ok(newest(app, device_id, kind, 1)?.pop())
}

/// The latest version of a device, or the `not_found` error that says there is no backup yet.
pub fn require_latest(
    app: &App,
    device_name: &str,
    device_id: &str,
    kind: Kind,
) -> AppResult<ConfigVersion> {
    latest(app, device_id, kind)?.ok_or_else(|| no_backup(device_name, kind))
}

pub(crate) fn no_backup(device_name: &str, kind: Kind) -> AppError {
    AppError::new(
        "not_found",
        format!(
            "`{device_name}` has no {} backup yet; back the device up first",
            kind.label()
        ),
    )
}

/// The latest version of each of `device_ids` that has one, keyed by device id.
pub(crate) fn latest_of_each(
    app: &App,
    device_ids: &[&str],
    kind: Kind,
) -> AppResult<BTreeMap<String, ConfigVersion>> {
    db::latest_of_each(app.store(), device_ids, kind.as_str())?
        .into_iter()
        .map(|row| from_row(app, row).map(|version| (version.device_id.clone(), version)))
        .collect()
}

/// What storing a freshly fetched configuration did.
#[derive(Clone, Debug)]
pub struct Stored {
    pub version: ConfigVersion,
    /// The version this one replaced as the latest; `None` when nothing new
    /// was stored or when this is the first version.
    pub previous: Option<ConfigVersion>,
    /// Whether the content differed from the latest version (or there was none).
    pub new: bool,
}

/// Store a fetched configuration. Identical content is not stored twice: the
/// latest version's `last_seen` moves instead.
pub fn store_fetched(
    app: &App,
    device_id: &str,
    kind: Kind,
    text: &str,
    origin: &str,
) -> AppResult<Stored> {
    let previous = latest(app, device_id, kind)?;
    let (stored, new) = app
        .store()
        .config_insert(device_id, kind.as_str(), text, origin, "")?;
    db::record(app.store(), stored.id, now_ms(), text.len() as u64)?;
    Ok(Stored {
        version: get(app, stored.id)?,
        previous: previous.filter(|_| new),
        new,
    })
}

// ------------------------------------------------------------------ pinning

pub fn pin(app: &App, version_id: i64, reason: &str) -> AppResult<ConfigVersion> {
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    if reason.is_empty() {
        return Err(AppError::invalid(
            "say why the version is pinned: the reason is shown next to it and is what a later unpin refers to",
        ));
    }
    let version = get(app, version_id)?;
    db::pin(app.store(), version_id, &reason, now_ms())?;
    app.audit(
        "config.pin",
        Some(&version.device_id),
        format!(
            "Pinned configuration version {version_id} of `{}`: {reason}",
            device_name(app, &version.device_id)
        ),
        json!({"version_id": version_id, "reason": reason}),
    );
    announce_pin(app, &version, true);
    get(app, version_id)
}

/// Remove the pin with this `reason`, or every pin of the version.
pub fn unpin(app: &App, version_id: i64, reason: Option<&str>) -> AppResult<ConfigVersion> {
    let version = get(app, version_id)?;
    let reason = reason.map(|reason| reason.split_whitespace().collect::<Vec<_>>().join(" "));
    db::unpin(app.store(), version_id, reason.as_deref())?;
    let version_now = get(app, version_id)?;
    if version.pin_reasons != version_now.pin_reasons {
        app.audit(
            "config.unpin",
            Some(&version.device_id),
            format!(
                "Unpinned configuration version {version_id} of `{}`{}",
                device_name(app, &version.device_id),
                match &reason {
                    Some(reason) => format!(": {reason}"),
                    None => String::new(),
                }
            ),
            json!({"version_id": version_id, "reason": reason}),
        );
    }
    if version.pinned != version_now.pinned {
        announce_pin(app, &version_now, false);
    }
    Ok(version_now)
}

fn announce_pin(app: &App, version: &ConfigVersion, pinned: bool) {
    app.publish(
        "config.pinned",
        &json!({"version_id": version.id, "device_id": version.device_id, "pinned": pinned}),
    );
}

// --------------------------------------------------------- delete, retention

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Deleted {
    pub deleted: usize,
    /// Versions that were asked for but stay because they are pinned.
    pub kept_pinned: Vec<i64>,
}

fn device_name(app: &App, device_id: &str) -> String {
    inventory::get(app, device_id).map_or_else(|_| device_id.to_string(), |device| device.name)
}

fn announce_deleted(app: &App, by_device: &BTreeMap<String, Vec<i64>>) {
    for (device_id, ids) in by_device {
        app.audit(
            "config.delete",
            Some(device_id),
            format!(
                "Deleted {} stored configuration version{} of `{}`",
                ids.len(),
                if ids.len() == 1 { "" } else { "s" },
                device_name(app, device_id)
            ),
            json!({"version_ids": ids}),
        );
    }
    if !by_device.is_empty() {
        let device_ids: Vec<&String> = by_device.keys().collect();
        let version_ids: Vec<i64> = by_device.values().flatten().copied().collect();
        app.publish(
            "config.deleted",
            &json!({"device_ids": device_ids, "version_ids": version_ids}),
        );
    }
}

/// Delete the named versions, except pinned ones. An unknown id refuses the
/// whole request before anything is removed.
pub fn delete_versions(app: &App, ids: &[i64]) -> AppResult<Deleted> {
    let mut by_device: BTreeMap<String, Vec<i64>> = BTreeMap::new();
    let mut kept_pinned = Vec::new();
    for id in ids {
        let row = db::version(app.store(), *id)?.ok_or_else(|| not_found(*id))?;
        if !row.pins.is_empty() {
            kept_pinned.push(row.id);
        } else if !by_device.values().flatten().any(|known| known == id) {
            by_device.entry(row.device_id).or_default().push(row.id);
        }
    }
    let removable: Vec<i64> = by_device.values().flatten().copied().collect();
    let deleted = db::delete(app.store(), &removable)?;
    announce_deleted(app, &by_device);
    Ok(Deleted {
        deleted,
        kept_pinned,
    })
}

/// Delete every stored version of a device, of both kinds, except pinned ones.
pub fn delete_device(app: &App, device_id: &str) -> AppResult<Deleted> {
    let (removable, kept_pinned) = db::of_device(app.store(), device_id)?;
    let deleted = db::delete(app.store(), &removable)?;
    if !removable.is_empty() {
        announce_deleted(app, &BTreeMap::from([(device_id.to_string(), removable)]));
    }
    Ok(Deleted {
        deleted,
        kept_pinned,
    })
}

/// Apply retention to one history: keep the newest `keep` versions and
/// whatever is pinned. Returns how many versions were removed.
pub fn prune(app: &App, device_id: &str, kind: &str, keep: u32) -> AppResult<usize> {
    let expired = db::expired(app.store(), device_id, kind, keep.max(1))?;
    if expired.is_empty() {
        return Ok(0);
    }
    let removed = db::delete(app.store(), &expired)?;
    tracing::debug!(
        device_id,
        kind,
        removed,
        "pruned old configuration versions"
    );
    Ok(removed)
}

/// Apply retention everywhere (after `keep_versions` was lowered).
pub fn prune_all(app: &App) -> AppResult<usize> {
    let keep = ConfigSettings::load(app).keep_versions;
    let mut removed = 0;
    for (device_id, kind) in db::crowded(app.store(), keep)? {
        removed += prune(app, &device_id, &kind, keep)?;
    }
    Ok(removed)
}

// ----------------------------------------------------------------------- rpc

#[derive(Deserialize)]
struct VersionsParams {
    device_id: String,
    kind: Option<Kind>,
    limit: Option<usize>,
    before_id: Option<i64>,
}

#[derive(Deserialize)]
struct GetParams {
    version_id: i64,
    redact: Option<bool>,
}

#[derive(Deserialize)]
struct LatestParams {
    device_id: String,
    kind: Option<Kind>,
    redact: Option<bool>,
}

/// A version with its text, as `config.get` and `config.latest` return it.
#[derive(Serialize)]
pub struct ConfigText {
    pub version: ConfigVersion,
    pub text: String,
    pub redacted: bool,
}

#[derive(Deserialize)]
struct DeleteParams {
    device_id: Option<String>,
    version_ids: Option<Vec<i64>>,
}

#[derive(Deserialize)]
struct PinParams {
    version_id: i64,
    reason: String,
}

#[derive(Deserialize)]
struct UnpinParams {
    version_id: i64,
    reason: Option<String>,
}

/// Record that someone asked to see a configuration with its secrets.
pub(crate) fn audit_unredacted(
    app: &App,
    device_id: &str,
    device_name: &str,
    detail: serde_json::Value,
) {
    app.audit(
        "config.view_unredacted",
        Some(device_id),
        format!("Viewed unredacted configuration of `{device_name}`"),
        detail,
    );
}

/// The text of a version as it may be shown: redacted unless asked otherwise,
/// and audited when it is not.
fn present(app: &App, version: ConfigVersion, text: String, redact: bool) -> ConfigText {
    // Should the device be gone while its snapshot is not, the built-in rules still apply.
    let target = scope::target(app, &version.device_id).ok();
    let text = if redact {
        let profile = target.as_ref().map_or("", |target| target.profile.as_str());
        Redaction::for_profiles(app, [profile]).text(&text)
    } else {
        let name = target
            .as_ref()
            .map_or(version.device_id.as_str(), |target| {
                target.device.name.as_str()
            });
        audit_unredacted(
            app,
            &version.device_id,
            name,
            json!({"version_id": version.id, "kind": version.kind}),
        );
        text
    };
    ConfigText {
        version,
        text,
        redacted: redact,
    }
}

pub(crate) fn register(router: &mut Router) {
    router.add(
        "config.versions",
        |app: App, params: VersionsParams| async move {
            let device = inventory::find(&app, &params.device_id)?;
            let limit = params.limit.unwrap_or(50).clamp(1, 500);
            db::page(
                app.store(),
                &device.id,
                params.kind.unwrap_or_default().as_str(),
                params.before_id,
                limit,
            )?
            .into_iter()
            .map(|row| from_row(&app, row))
            .collect::<AppResult<Vec<ConfigVersion>>>()
        },
    );

    router.add("config.get", |app: App, params: GetParams| async move {
        let (version, text) = read(&app, params.version_id)?;
        Ok(present(&app, version, text, params.redact.unwrap_or(true)))
    });

    router.add(
        "config.latest",
        |app: App, params: LatestParams| async move {
            let device = inventory::find(&app, &params.device_id)?;
            let version = require_latest(
                &app,
                &device.name,
                &device.id,
                params.kind.unwrap_or_default(),
            )?;
            let (version, text) = read(&app, version.id)?;
            Ok(present(&app, version, text, params.redact.unwrap_or(true)))
        },
    );

    router.add("config.delete", |app: App, params: DeleteParams| async move {
        match (params.device_id, params.version_ids) {
            (Some(device_id), None) => {
                let device = inventory::find(&app, &device_id)?;
                delete_device(&app, &device.id)
            }
            (None, Some(ids)) if !ids.is_empty() => delete_versions(&app, &ids),
            _ => Err(AppError::invalid(
                "name what to delete: either `device_id` (every stored version of that device) or `version_ids`",
            )),
        }
    });

    router.add("config.pin", |app: App, params: PinParams| async move {
        pin(&app, params.version_id, &params.reason)
    });

    router.add("config.unpin", |app: App, params: UnpinParams| async move {
        unpin(&app, params.version_id, params.reason.as_deref())
    });
}
