//! Backups: fetching device configurations into versioned storage.
//!
//! A backup is a job. Per device it opens one CLI session, reads each
//! requested configuration with the device type's own command, and stores it
//! through the core store, which keeps one version per distinct content. A
//! device that cannot be backed up at all (no CLI access, no configuration
//! command) is reported as skipped with the reason; one that could not be
//! read this time is reported as failed.

use std::collections::HashMap;
use std::time::Duration;

use nocturne_app::connect::{self, Cli, CliOptions};
use nocturne_app::inventory::DeviceFilter;
use nocturne_app::jobs::{self, Job, JobCtx, JobSpec};
use nocturne_app::model::{Device, Transport};
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult, Event, Router, profiles};
use nocturne_drivers::hier;
use nocturne_drivers::policy::CommandClass;
use nocturne_drivers::profile::Platform;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::time::Instant;

use crate::scope::{self, Target};
use crate::settings::{self, ConfigSettings};
use crate::versions::{self, Kind, Stored};
use crate::{db, intended};

pub(crate) const STATUS: &str = "config_backup_status";

/// How long a device must have been quiet after a change notification before it is backed up.
const DEBOUNCE: Duration = Duration::from_secs(60);
/// However long change notifications keep coming, a backup is not put off for longer than this.
const MAX_DELAY: Duration = Duration::from_secs(600);

/// Who asked for a backup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    User,
    Schedule,
    Assistant,
    /// Nocturne itself, after a device reported a configuration change.
    System,
}

impl Origin {
    fn job(self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::Schedule => "schedule",
            Origin::Assistant => "assistant",
            Origin::System => "system",
        }
    }

    /// What versions stored by this backup carry as their `origin`.
    fn version(self) -> &'static str {
        match self {
            Origin::User => "manual",
            Origin::Schedule => "scheduled",
            Origin::Assistant => "assistant",
            Origin::System => "event",
        }
    }
}

/// What is known about backing up one device.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupStatus {
    pub device_id: String,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    /// Why the last attempt failed. A success clears it.
    pub last_error: Option<String>,
    /// Why this device is not backed up at all.
    pub skipped: Option<String>,
    /// What the last successful backup read.
    pub captured: Captured,
}

/// The versions one backup read in one session — the pair that can be
/// compared to tell whether the device has unsaved changes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Captured {
    pub running: Option<i64>,
    pub startup: Option<i64>,
    /// Why the running-config is not part of this backup.
    pub running_note: Option<String>,
    /// Why the startup-config is not part of this backup.
    pub startup_note: Option<String>,
}

pub(crate) fn status_of(app: &App, device_id: &str) -> AppResult<BackupStatus> {
    Ok(app
        .store()
        .get::<BackupStatus>(STATUS, device_id)?
        .unwrap_or_else(|| BackupStatus {
            device_id: device_id.to_string(),
            ..BackupStatus::default()
        }))
}

fn update_status(app: &App, device_id: &str, change: impl FnOnce(&mut BackupStatus)) {
    let saved = status_of(app, device_id).and_then(|mut status| {
        status.last_attempt_at = Some(now_ms());
        change(&mut status);
        app.store().put(STATUS, device_id, &status)
    });
    if let Err(err) = saved {
        tracing::error!(%err, device_id, "backup status could not be saved");
    }
}

/// How a device's stored configuration relates to what was there before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Unchanged,
    /// Nothing changed, and at least one kind was stored for the first time.
    First,
    Changed,
}

enum Report {
    Done(Outcome),
    Skipped(String),
}

fn command_for(platform: &Platform, kind: Kind) -> Option<String> {
    let config = &platform.profile.config;
    match kind {
        Kind::Running => Some(config.running.clone()),
        Kind::Startup => config.startup.clone(),
    }
    .filter(|command| !command.trim().is_empty())
}

/// Why none of `kinds` can be read from this kind of device, if that is so.
fn unreadable(platform: &Platform, kinds: &[Kind]) -> Option<String> {
    kinds
        .iter()
        .all(|kind| command_for(platform, *kind).is_none())
        .then(|| {
            format!(
                "the `{}` device type does not define a command to read the configuration",
                platform.profile.name
            )
        })
}

async fn fetch(cli: &mut Cli, kind: Kind, command: &str) -> AppResult<String> {
    // The command comes from a profile, which is data someone wrote: hold it
    // to the same standard as any other command sent outside a change.
    let verdict = cli.classify(command);
    if verdict.class != CommandClass::ReadOnly {
        return Err(AppError::forbidden(format!(
            "the `{}` device type reads the {} with `{command}`, which its own command policy does not class as read-only ({}). \
             Add the command to `policy.read_only` in the device type.",
            cli.platform.profile.name,
            kind.label(),
            verdict.reason
        )));
    }
    let text = cli
        .session()
        .fetch_config(kind == Kind::Startup)
        .await
        .map_err(|err| {
            let mut err = AppError::from(err);
            err.message = format!("could not read the {}: {}", kind.label(), err.message);
            err
        })?;
    if text.trim().is_empty() {
        return Err(AppError::new(
            "device_error",
            format!("the device printed an empty {}", kind.label()),
        ));
    }
    Ok(text)
}

/// Tell the UI and the audit trail what storing a fetched configuration did,
/// and apply retention when a version was added.
fn announce(app: &App, device: &Device, kind: Kind, text: &str, stored: &Stored) -> AppResult<Outcome> {
    if !stored.new {
        return Ok(Outcome::Unchanged);
    }
    let version = &stored.version;
    let outcome = match &stored.previous {
        None => {
            app.publish(
                "config.first_backup",
                &json!({
                    "device_id": device.id, "name": device.name, "kind": kind,
                    "version_id": version.id, "lines": version.lines,
                }),
            );
            Outcome::First
        }
        Some(previous) => {
            let (_, before) = app.store().config_get(previous.id)?;
            let diff = hier::text_diff(&before, text, Some(0));
            let detail = json!({
                "device_id": device.id, "name": device.name, "kind": kind,
                "version_id": version.id, "previous_id": previous.id,
                "added": diff.added, "removed": diff.removed,
            });
            app.publish("config.changed", &detail);
            app.audit(
                "config.changed",
                Some(&device.id),
                format!(
                    "The {} of `{}` changed ({} lines added, {} removed)",
                    kind.label(),
                    device.name,
                    diff.added,
                    diff.removed
                ),
                detail,
            );
            Outcome::Changed
        }
    };
    let keep = ConfigSettings::load(app).keep_versions;
    versions::prune(app, &device.id, kind.as_str(), keep)?;
    Ok(outcome)
}

async fn read_and_store(
    ctx: &JobCtx,
    cli: &mut Cli,
    kinds: &[Kind],
    origin: Origin,
) -> AppResult<(Outcome, Captured)> {
    let app = ctx.app();
    let device = cli.device.clone();
    let platform = cli.platform.clone();
    let mut captured = Captured::default();
    let mut outcome = Outcome::Unchanged;
    let mut stored_any = false;

    for kind in [Kind::Running, Kind::Startup] {
        let note = match kind {
            Kind::Running => &mut captured.running_note,
            Kind::Startup => &mut captured.startup_note,
        };
        if !kinds.contains(&kind) {
            *note = Some(format!("the {} was not asked for in the last backup", kind.label()));
            continue;
        }
        let Some(command) = command_for(&platform, kind) else {
            *note = Some(format!(
                "the `{}` device type defines no command to read the {}",
                platform.profile.name,
                kind.label()
            ));
            continue;
        };
        let text = match fetch(cli, kind, &command).await {
            Ok(text) => text,
            // What was read so far is a good backup; say what is missing instead of failing it.
            Err(err) if stored_any => {
                ctx.device(&device.id, "warn", err.message.clone());
                *note = Some(err.message);
                continue;
            }
            Err(err) => return Err(err),
        };
        let stored = versions::store_fetched(app, &device.id, kind, &text, origin.version())?;
        outcome = outcome.max(announce(app, &device, kind, &text, &stored)?);
        match kind {
            Kind::Running => captured.running = Some(stored.version.id),
            Kind::Startup => captured.startup = Some(stored.version.id),
        }
        stored_any = true;
    }
    Ok((outcome, captured))
}

async fn back_up(ctx: &JobCtx, target: &Target, kinds: &[Kind], origin: Origin) -> AppResult<Report> {
    let app = ctx.app();
    let device = &target.device;
    if device.transport == Transport::None {
        return Ok(Report::Skipped(
            "monitor-only: no CLI access is configured for this device".into(),
        ));
    }
    // A device type that cannot print its configuration is known before connecting.
    if !target.profile.is_empty()
        && let Some(reason) = unreadable(&*profiles::platform(app, &target.profile)?, kinds)
    {
        return Ok(Report::Skipped(reason));
    }
    let mut cli = connect::cli(app, &device.id, CliOptions::default()).await?;
    if let Some(reason) = unreadable(&cli.platform, kinds) {
        cli.close().await;
        return Ok(Report::Skipped(reason));
    }
    let result = read_and_store(ctx, &mut cli, kinds, origin).await;
    cli.close().await;
    let (outcome, captured) = result?;
    update_status(app, &device.id, |status| {
        status.last_success_at = status.last_attempt_at;
        status.last_error = None;
        status.skipped = None;
        status.captured = captured;
    });
    Ok(Report::Done(outcome))
}

/// Back one device up, and record how it went.
async fn back_up_recorded(ctx: JobCtx, target: Target, kinds: Vec<Kind>, origin: Origin) -> AppResult<Report> {
    let app = ctx.app();
    let id = &target.device.id;
    let report = back_up(&ctx, &target, &kinds, origin).await;
    match &report {
        Ok(Report::Done(outcome)) => {
            let what = match outcome {
                Outcome::Unchanged => "configuration unchanged",
                Outcome::First => "first backup stored",
                Outcome::Changed => "configuration changed; new version stored",
            };
            ctx.device(id, "info", what);
        }
        Ok(Report::Skipped(reason)) => {
            update_status(app, id, |status| status.skipped = Some(reason.clone()));
            ctx.device(id, "warn", format!("skipped: {reason}"));
        }
        Err(err) => {
            update_status(app, id, |status| {
                status.last_error = Some(err.message.clone());
                status.skipped = None;
            });
            ctx.device(id, "error", err.message.clone());
        }
    }
    report
}

/// What a backup job reports when it ends.
#[derive(Debug, Default, Serialize)]
struct BackupResult {
    /// Devices whose configuration was read and is stored: `unchanged + changed + first`.
    backed_up: usize,
    unchanged: usize,
    changed: usize,
    /// Devices backed up for the first time.
    first: usize,
    skipped: Vec<Value>,
    failed: Vec<Value>,
}

/// What to back up, and for whom.
pub struct Request {
    pub(crate) targets: Vec<Target>,
    pub kinds: Vec<Kind>,
    pub origin: Origin,
}

/// The kinds a backup reads when the caller does not say.
pub(crate) fn default_kinds(app: &App) -> Vec<Kind> {
    if ConfigSettings::load(app).backup_startup {
        vec![Kind::Running, Kind::Startup]
    } else {
        vec![Kind::Running]
    }
}

/// Start the backup job.
pub fn start(app: &App, request: Request) -> Job {
    let Request {
        targets,
        kinds,
        origin,
    } = request;
    let ids: Vec<String> = targets.iter().map(|target| target.device.id.clone()).collect();
    let names: Vec<String> = targets.iter().map(|target| target.device.name.clone()).collect();
    let title = match names.as_slice() {
        [name] if origin == Origin::System => {
            format!("Back up `{name}` after a configuration change")
        }
        [name] => format!("Back up `{name}`"),
        names if origin == Origin::System => {
            format!("Back up {} devices after configuration changes", names.len())
        }
        names => format!("Back up {} devices", names.len()),
    };
    let concurrency = ConfigSettings::load(app).concurrency as usize;
    let spec = JobSpec::new("config.backup", title)
        .devices(&ids)
        .origin(origin.job());

    jobs::start(app, spec, move |ctx| async move {
        let reports = ctx
            .for_each(targets, concurrency, |ctx, target| {
                back_up_recorded(ctx, target, kinds.clone(), origin)
            })
            .await;

        let mut result = BackupResult::default();
        let mut cancelled = 0;
        for ((id, name), report) in ids.iter().zip(&names).zip(reports) {
            match report {
                Ok(Report::Done(outcome)) => {
                    result.backed_up += 1;
                    match outcome {
                        Outcome::Unchanged => result.unchanged += 1,
                        Outcome::First => result.first += 1,
                        Outcome::Changed => result.changed += 1,
                    }
                }
                Ok(Report::Skipped(reason)) => result
                    .skipped
                    .push(json!({"device_id": id, "name": name, "reason": reason})),
                Err(err) if err.code == "cancelled" => cancelled += 1,
                Err(err) => result
                    .failed
                    .push(json!({"device_id": id, "name": name, "error": err.message})),
            }
        }
        if cancelled > 0 {
            ctx.info(format!(
                "cancelled with {cancelled} of {} devices not backed up ({} done, {} failed)",
                ids.len(),
                result.backed_up,
                result.failed.len()
            ));
            return Err(jobs::cancelled());
        }
        Ok(serde_json::to_value(result)?)
    })
}

// -------------------------------------------------------------- background

/// A device waiting for its configuration session to go quiet.
struct Waiting {
    since: Instant,
    due: Instant,
}

fn on_event(app: &App, waiting: &mut HashMap<String, Waiting>, event: &Event) {
    match event.topic.as_str() {
        "logs.config_changed" => {
            let Some(device_id) = event.payload["device_id"].as_str() else {
                return;
            };
            if !ConfigSettings::load(app).backup_on_change_event {
                return;
            }
            let now = Instant::now();
            let entry = waiting.entry(device_id.to_string()).or_insert(Waiting {
                since: now,
                due: now,
            });
            entry.due = (now + DEBOUNCE).min(entry.since + MAX_DELAY);
        }
        "devices.changed" if event.payload["change"] == "deleted" => {
            // The inventory removes a deleted device's snapshots; what this feature keeps about it goes too.
            let ids = event.payload["ids"].as_array().into_iter().flatten();
            for device_id in ids.filter_map(Value::as_str) {
                waiting.remove(device_id);
                let forgotten = app
                    .store()
                    .delete(STATUS, device_id)
                    .and_then(|_| intended::forget(app, device_id));
                if let Err(err) = forgotten {
                    tracing::error!(%err, device_id, "records of a deleted device could not be removed");
                }
            }
            if let Err(err) = db::sweep(app.store()) {
                tracing::error!(%err, "metadata of deleted configuration versions could not be removed");
            }
        }
        "settings.changed" if event.payload["section"] == settings::SECTION => {
            match versions::prune_all(app) {
                Ok(0) => {}
                Ok(removed) => tracing::info!(removed, "pruned configuration versions beyond the new limit"),
                Err(err) => tracing::error!(%err, "configuration versions could not be pruned"),
            }
        }
        _ => {}
    }
}

/// Back up the devices whose change notifications have gone quiet.
fn back_up_due(app: &App, device_ids: Vec<String>) {
    let targets: Vec<Target> = device_ids
        .iter()
        // A device deleted in the meantime has nothing to back up.
        .filter_map(|id| scope::target(app, id).ok())
        .collect();
    if targets.is_empty() {
        return;
    }
    start(
        app,
        Request {
            targets,
            kinds: default_kinds(app),
            origin: Origin::System,
        },
    );
}

/// Follow the event stream: back devices up after they report a
/// configuration change, forget deleted devices, apply a lowered retention.
async fn watch(app: App, mut events: broadcast::Receiver<Event>) {
    let mut waiting: HashMap<String, Waiting> = HashMap::new();
    loop {
        let next_due = waiting.values().map(|entry| entry.due).min();
        let quiet = async {
            match next_due {
                Some(due) => tokio::time::sleep_until(due).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => on_event(&app, &mut waiting, &event),
                // Events that were missed are gone; the next backup still sees the change.
                Err(RecvError::Lagged(_)) => {}
                Err(RecvError::Closed) => break,
            },
            _ = quiet => {
                let now = Instant::now();
                let mut due: Vec<String> = waiting
                    .iter()
                    .filter(|(_, entry)| entry.due <= now)
                    .map(|(device_id, _)| device_id.clone())
                    .collect();
                due.sort();
                for device_id in &due {
                    waiting.remove(device_id);
                }
                back_up_due(&app, due);
            }
        }
    }
}

pub(crate) fn spawn_watcher(app: &App) {
    // Subscribe before the task first runs, so nothing published right after start-up is missed.
    let events = app.subscribe();
    app.spawn("config.events", watch(app.clone(), events));
}

// ----------------------------------------------------------------------- rpc

#[derive(Default, Deserialize)]
#[serde(default)]
struct BackupParams {
    device_ids: Option<Vec<String>>,
    filter: Option<DeviceFilter>,
    kinds: Option<Vec<Kind>>,
    origin: Option<Origin>,
}

pub(crate) fn register(router: &mut Router) {
    router.add("config.backup", |app: App, params: BackupParams| async move {
        let targets = scope::targets(&app, params.device_ids.as_deref(), params.filter.as_ref())?;
        if targets.is_empty() {
            return Err(AppError::invalid(
                "no device matches, so there is nothing to back up",
            ));
        }
        let kinds = match params.kinds {
            None => default_kinds(&app),
            Some(kinds) if kinds.is_empty() => {
                return Err(AppError::invalid(
                    "`kinds` is empty: ask for `running`, `startup` or both, or leave it out",
                ));
            }
            Some(kinds) => kinds,
        };
        let origin = params.origin.unwrap_or_default();
        let devices = targets.len();
        let job = start(
            &app,
            Request {
                targets,
                kinds: kinds.clone(),
                origin,
            },
        );
        app.audit(
            "config.backup",
            None,
            format!(
                "Started a configuration backup of {devices} device{}",
                if devices == 1 { "" } else { "s" }
            ),
            json!({"job_id": job.id, "devices": devices, "kinds": kinds, "origin": origin}),
        );
        Ok(jobs::started(&job))
    });
}
