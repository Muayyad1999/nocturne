//! A core application with this feature registered, and fakes for the
//! features it reaches by name.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{Value, json};

use nocturne_app::store::{LogEntry, LogQuery};
use nocturne_app::testing::Lab;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, AppConfig, AppError, inventory};

use crate::watch::{SYNC, SYNCED};

/// Long enough for a loaded machine, short enough to fail instead of hanging.
const PATIENCE: Duration = Duration::from_secs(30);

pub const MINUTE: i64 = 60_000;

/// Which of the other features are in the build under test.
#[derive(Clone, Copy, Debug)]
pub struct Build {
    pub changes: bool,
    pub maintenance: bool,
    pub alerts: bool,
    pub logs: bool,
    pub config: bool,
}

impl Build {
    pub const FULL: Build = Build {
        changes: true,
        maintenance: true,
        alerts: true,
        logs: true,
        config: true,
    };
    /// Nothing but the core and this feature.
    pub const CORE: Build = Build {
        changes: false,
        maintenance: false,
        alerts: false,
        logs: false,
        config: false,
    };
}

/// What the fakes answer, and what they were asked.
#[derive(Clone, Default)]
pub struct Fakes {
    /// Every change there is; `changes.list` filters by device like the real one.
    pub changes: Arc<Mutex<Vec<Value>>>,
    /// When set, `changes.list` fails with this message.
    pub changes_fail: Arc<Mutex<Option<String>>>,
    /// While a test holds this, `changes.list` does not answer: a change
    /// engine that is busy, and a judge that waits for it.
    pub changes_hold: Arc<tokio::sync::Mutex<()>>,
    /// When set, `changes.list` panics: a fault in another feature, met
    /// where this one calls it.
    pub changes_panic: Arc<AtomicBool>,
    pub windows: Arc<Mutex<Vec<Value>>>,
    pub windows_fail: Arc<Mutex<Option<String>>>,
    /// The parameters of every `alerts.raise`, in order.
    pub raised: Arc<Mutex<Vec<Value>>>,
    /// Version id → when a fetch last returned exactly that configuration,
    /// which the config feature keeps and the core's store does not. A
    /// version that is not here was last seen when it was first stored.
    pub last_seen: Arc<Mutex<HashMap<i64, i64>>>,
}

impl Fakes {
    pub fn raised_with(&self, dedup_key: &str) -> Vec<Value> {
        self.raised
            .lock()
            .iter()
            .filter(|params| params["dedup_key"] == dedup_key)
            .cloned()
            .collect()
    }
}

pub struct World {
    pub app: App,
    pub fakes: Fakes,
    /// Kept so the devices and their fakes live as long as the test.
    pub lab: Lab,
    /// The data directory, for the tests that need a real key file.
    _dir: Option<tempfile::TempDir>,
}

fn register_fakes(builder: &mut AppBuilder, build: Build) -> Fakes {
    let fakes = Fakes::default();
    if build.changes {
        let (changes, fail) = (fakes.changes.clone(), fakes.changes_fail.clone());
        let (hold, panics) = (fakes.changes_hold.clone(), fakes.changes_panic.clone());
        builder
            .router
            .add("changes.list", move |_app: App, params: Value| {
                let (changes, fail, hold) = (changes.clone(), fail.clone(), hold.clone());
                let panics = panics.clone();
                async move {
                    drop(hold.lock().await);
                    assert!(
                        !panics.load(Ordering::Relaxed),
                        "the change engine met a fault of its own"
                    );
                    if let Some(message) = fail.lock().clone() {
                        return Err(AppError::internal(message));
                    }
                    // As `nocturne-changes/src/db.rs` filters: a change is
                    // listed for a device when one of its targets is it.
                    let wanted = params["device_id"].as_str().map(str::to_string);
                    let rows: Vec<Value> = changes
                        .lock()
                        .iter()
                        .filter(|change| {
                            wanted.as_ref().is_none_or(|wanted| {
                                change["devices"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .any(|device| device["device_id"] == wanted.as_str())
                            })
                        })
                        .cloned()
                        .collect();
                    Ok(rows)
                }
            });
    }
    if build.maintenance {
        let (windows, fail) = (fakes.windows.clone(), fakes.windows_fail.clone());
        builder
            .router
            .add("maintenance.list", move |_app: App, _params: Value| {
                let (windows, fail) = (windows.clone(), fail.clone());
                async move {
                    if let Some(message) = fail.lock().clone() {
                        return Err(AppError::internal(message));
                    }
                    Ok(windows.lock().clone())
                }
            });
    }
    if build.alerts {
        let raised = fakes.raised.clone();
        builder
            .router
            .add("alerts.raise", move |app: App, params: Value| {
                let raised = raised.clone();
                async move {
                    // The refusals of the real one
                    // (`nocturne-monitor/src/alerts.rs`): no title, no key,
                    // or a device the inventory does not know.
                    if params["title"].as_str().unwrap_or_default().is_empty()
                        || params["dedup_key"].as_str().unwrap_or_default().is_empty()
                    {
                        return Err(AppError::invalid("an alert needs a title and a dedup_key"));
                    }
                    if let Some(device_id) = params["device_id"].as_str() {
                        inventory::get(&app, device_id)?;
                    }
                    let mut raised = raised.lock();
                    raised.push(params.clone());
                    Ok(json!({
                        "id": format!("alert-{}", raised.len()),
                        "dedup_key": params["dedup_key"],
                        "title": params["title"],
                    }))
                }
            });
    }
    if build.logs {
        builder
            .router
            .add("logs.rules.list", |_app: App, _params: Value| async move {
                Ok(json!([]))
            });
    }
    if build.config {
        builder
            .router
            .add("config.backup", |_app: App, _params: Value| async move {
                Ok(json!({"job_id": "none"}))
            });
        let last_seen = fakes.last_seen.clone();
        builder
            .router
            .add("config.versions", move |app: App, params: Value| {
                let last_seen = last_seen.clone();
                async move {
                    // As `nocturne-config/src/versions.rs` answers: the
                    // device has to be in the inventory, and the versions of
                    // one kind come newest first by id, below `before_id`.
                    let device = inventory::find(&app, params["device_id"].as_str().unwrap_or(""))?;
                    let kind = params["kind"].as_str().unwrap_or("running");
                    let before_id = params["before_id"].as_i64().unwrap_or(i64::MAX);
                    let limit = params["limit"].as_u64().unwrap_or(50).clamp(1, 500) as usize;
                    let mut versions = app.store().config_versions(&device.id, Some(kind))?;
                    versions.retain(|version| version.id < before_id);
                    versions.sort_by_key(|version| std::cmp::Reverse(version.id));
                    versions.truncate(limit);
                    let last_seen = last_seen.lock();
                    Ok(versions
                        .into_iter()
                        .map(|version| {
                            json!({
                                "id": version.id, "device_id": version.device_id,
                                "kind": version.kind, "sha256": version.sha256,
                                "lines": version.lines, "bytes": 0,
                                "first_seen": version.taken_at,
                                "last_seen": last_seen
                                    .get(&version.id)
                                    .copied()
                                    .unwrap_or(version.taken_at)
                                    .max(version.taken_at),
                                "origin": version.source, "pinned": false,
                            })
                        })
                        .collect::<Vec<Value>>())
                }
            });
    }
    fakes
}

async fn assemble(mut builder: AppBuilder, build: Build, dir: Option<tempfile::TempDir>) -> World {
    crate::init(&mut builder);
    let fakes = register_fakes(&mut builder, build);
    // One simulated device comes with the lab; hardware is added by `device`.
    let lab = nocturne_app::testing::lab(builder, &["LAB-SW1"]).await;
    let world = World {
        app: lab.app.clone(),
        fakes,
        lab,
        _dir: dir,
    };
    // Creating the lab's credential is itself something `secrets_changed`
    // reports; let it be judged before the test starts counting.
    settle(&world.app).await;
    world
}

pub async fn world(build: Build) -> World {
    assemble(nocturne_app::testing::builder(), build, None).await
}

/// The same on a data directory, where the vault can have a master password.
pub async fn world_on_disk(build: Build) -> World {
    let dir = tempfile::tempdir().unwrap();
    let mut builder = AppBuilder::new(AppConfig::at(dir.path())).unwrap();
    nocturne_app::init(&mut builder);
    assemble(builder, build, Some(dir)).await
}

impl World {
    /// Add a hardware device (not a lab one) and return its id.
    pub async fn device(&self, name: &str, address: &str) -> String {
        let device = self
            .app
            .rpc(
                "devices.create",
                json!({
                    "name": name, "address": address, "profile": "test_ios",
                    "credential_id": self.lab.credential_id, "site": "HQ", "role": "access",
                }),
            )
            .await
            .unwrap();
        device["id"].as_str().unwrap().to_string()
    }

    pub fn lab_device(&self) -> String {
        self.lab.id("LAB-SW1")
    }
}

/// Wait until everything published so far has been judged.
pub async fn settle(app: &App) {
    let token = new_id();
    let mut events = app.subscribe();
    app.publish(SYNC, &json!({ "token": token }));
    let synced = async {
        loop {
            match events.recv().await {
                Ok(event) if event.topic == SYNCED && event.payload["token"] == token.as_str() => {
                    return;
                }
                Ok(_) => {}
                Err(err) => panic!("the event bus failed while waiting for the judge: {err}"),
            }
        }
    };
    tokio::time::timeout(PATIENCE, synced)
        .await
        .expect("the detections judged everything published so far");
}

pub fn now() -> i64 {
    now_ms()
}

/// `detections.recent` with a filter.
pub async fn recent(app: &App, filter: Value) -> Vec<Value> {
    app.rpc("detections.recent", filter)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

/// The hits of one detection, newest first.
pub async fn hits_of(app: &App, detection: &str) -> Vec<Value> {
    recent(app, json!({ "id": detection })).await
}

/// One row of `detections.list`.
pub async fn listed(app: &App, detection: &str) -> Value {
    app.rpc("detections.list", Value::Null)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == detection)
        .cloned()
        .expect("the detection is in the catalogue")
}

/// What is stored in the `event` stream under one detection.
pub fn stored_events(app: &App, detection: &str) -> Vec<LogEntry> {
    app.store()
        .log_query(&LogQuery {
            stream: Some("event".into()),
            ..LogQuery::default()
        })
        .unwrap()
        .into_iter()
        .filter(|entry| entry.source == format!("detect.{detection}"))
        .collect()
}

/// Store a syslog line the way the log feature does, and return its id.
///
/// The body is the part of `nocturne-logs/src/ingest.rs`'s that says how the
/// line arrived and what was received (`raw`). The text is synthetic. Every
/// call is another line: it carries a sequence number of its own, as a
/// device's lines do.
pub fn syslog_line(app: &App, device_id: &str, via: &str, at: i64) -> i64 {
    static SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    syslog_text(
        app,
        device_id,
        via,
        at,
        &format!(
            "<189>{sequence}: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)"
        ),
    )
}

/// The same, with the line as it was received given by the test: two calls
/// with one `raw` are one line that arrived twice.
pub fn syslog_text(app: &App, device_id: &str, via: &str, at: i64, raw: &str) -> i64 {
    app.store()
        .log_append(&LogEntry {
            id: 0,
            ts: at,
            stream: "syslog".into(),
            device_id: Some(device_id.to_string()),
            severity: Some(5),
            source: "SYS-5-CONFIG_I".into(),
            message: "Configured from console by admin on vty0 (10.0.0.5)".into(),
            body: Some(json!({"raw": raw, "via": via})),
        })
        .unwrap()
}

/// Two stored versions of a device's configuration, the first of them
/// `hours_apart` hours older than the second, and the `config.changed` event
/// the backup publishes for them (`nocturne-config/src/backup.rs`).
pub fn backed_up_twice(app: &App, device_id: &str, hours_apart: i64) -> (i64, i64) {
    let store = app.store();
    let (previous, _) = store
        .config_insert(device_id, "running", "hostname SW1\n", "scheduled", "")
        .unwrap();
    let (version, _) = store
        .config_insert(
            device_id,
            "running",
            "hostname SW1\nntp server 192.0.2.1\n",
            "scheduled",
            "",
        )
        .unwrap();
    // The store stamps a version with the time it is written. The first one
    // is moved back to where a real earlier backup would be — measured from
    // the second, not from its own stamp, which is a moment earlier by
    // however long the machine took between the two writes.
    store
        .with(|conn| {
            conn.execute(
                "UPDATE configs SET taken_at = ?1 WHERE id = ?2",
                (version.taken_at - hours_apart * 60 * MINUTE, previous.id),
            )
        })
        .unwrap();
    app.publish(
        "config.changed",
        &json!({
            "device_id": device_id, "name": "SW1", "kind": "running",
            "version_id": version.id, "previous_id": previous.id, "added": 1, "removed": 0,
        }),
    );
    (previous.id, version.id)
}

/// A change as `changes.list` and `changes.changed` carry it
/// (`ChangeSummary` in `nocturne-changes/src/model.rs`).
pub fn change(id: &str, device_id: &str, state: &str, updated_at: i64) -> Value {
    json!({
        "id": id, "title": format!("Change {id}"), "description": "", "origin": "user",
        "state": state, "ticket": null, "created_at": updated_at - 5 * MINUTE,
        "updated_at": updated_at, "created_by": "user", "job_id": null, "risk": "low",
        "protected_hits": 0, "approved_by": "user",
        "devices": [{
            "device_id": device_id, "name": "SW1", "planned": true, "risk": "low",
            "protected_hits": 0, "commands": 3, "state": null,
        }],
    })
}

pub async fn set(app: &App, patch: Value) {
    app.rpc("settings.set", json!({"section": "detect", "value": patch}))
        .await
        .unwrap();
}
