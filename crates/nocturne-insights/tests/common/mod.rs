//! A lab for the integration tests: the core, this feature, and stubs for the
//! features it reads through RPC when they happen to be installed.
//!
//! The stubs are deliberately minimal — they answer in the shape the real
//! features' specifications fix, and record what they were called with — so
//! that these tests prove the wire contract without depending on another
//! feature crate.

#![allow(dead_code)] // each test binary uses a different part of this.

use std::sync::Arc;

use nocturne_app::App;
use nocturne_app::store::Store;
use nocturne_app::testing::{self, Lab};
use nocturne_drivers::model::Kind;
use parking_lot::Mutex;
use serde_json::{Value, json};

/// What the stubbed features were asked to do.
#[derive(Default)]
pub struct Recorder {
    pub changes: Mutex<Vec<Value>>,
    pub alerts: Mutex<Vec<Value>>,
}

/// Which other features exist in a given test.
#[derive(Clone, Copy, Default)]
pub struct With {
    pub changes: bool,
    pub alerts: bool,
    pub topology: bool,
}

impl With {
    pub fn all() -> With {
        With {
            changes: true,
            alerts: true,
            topology: true,
        }
    }
}

/// The topology graph the stub serves, if any.
#[derive(Default)]
pub struct Graph(pub Mutex<Value>);

pub async fn lab(names: &[&str], with: With) -> Lab {
    let mut builder = testing::builder();
    nocturne_insights::init(&mut builder);
    builder.state(Recorder::default());
    builder.state(Graph(Mutex::new(json!({"nodes": [], "links": []}))));
    if with.changes {
        builder
            .router
            .add("changes.create", |app: App, params: Value| async move {
                app.state::<Recorder>().changes.lock().push(params);
                Ok(json!({"id": "chg-1", "state": "draft"}))
            });
        builder
            .router
            .add("changes.list", |_: App, _: Value| async move {
                Ok(Value::Array(Vec::new()))
            });
    }
    if with.alerts {
        builder
            .router
            .add("alerts.raise", |app: App, params: Value| async move {
                app.state::<Recorder>().alerts.lock().push(params);
                Ok(json!({"id": "alert-1", "state": "firing"}))
            });
    }
    if with.topology {
        builder
            .router
            .add("topology.get", |app: App, _: Value| async move {
                Ok(app.state::<Graph>().0.lock().clone())
            });
    }
    // `config.latest` is only ever probed for its presence: the stored
    // versions themselves are read from the core store.
    builder
        .router
        .add("config.latest", |_: App, _: Value| async move {
            Ok(Value::Null)
        });
    testing::lab(builder, names).await
}

/// Store collected rows for a device, as a collector would.
pub fn collected(app: &App, device_id: &str, kind: Kind, rows: Vec<Value>) {
    nocturne_app::collect::store(app, device_id, kind, rows, "cli").expect("collected rows stored");
}

/// Store a configuration snapshot, as the configuration feature would.
pub fn config(store: &Arc<Store>, device_id: &str, kind: &str, text: &str) {
    store
        .config_insert(device_id, kind, text, "scheduled", "")
        .expect("configuration stored");
}

/// Run a scan and wait for its job to finish, returning the job's result.
pub async fn scan(app: &App, params: Value) -> Value {
    let started = app
        .rpc("insights.scan", params)
        .await
        .expect("scan started");
    let job = nocturne_app::jobs::wait(app, started["job_id"].as_str().expect("job id"))
        .await
        .expect("the scan job finishes");
    assert_eq!(
        job.state,
        nocturne_app::jobs::JobState::Succeeded,
        "{:?}",
        job.error
    );
    job.result.expect("a scan result")
}

/// The open insights of a check, by subject.
pub async fn open_subjects(app: &App, check: &str) -> Vec<String> {
    let listed = app
        .rpc("insights.list", json!({"check": check, "limit": 500}))
        .await
        .expect("list");
    let mut subjects: Vec<String> = listed["insights"]
        .as_array()
        .expect("insights")
        .iter()
        .map(|insight| {
            format!(
                "{}:{}",
                insight["device"].as_str().unwrap_or_default(),
                insight["subject"].as_str().unwrap_or_default()
            )
        })
        .collect();
    subjects.sort();
    subjects
}

/// Every open insight as `(check, device, subject)`.
pub async fn all_open(app: &App) -> Vec<(String, String, String)> {
    let listed = app
        .rpc("insights.list", json!({"limit": 500}))
        .await
        .expect("list");
    let mut rows: Vec<(String, String, String)> = listed["insights"]
        .as_array()
        .expect("insights")
        .iter()
        .map(|insight| {
            (
                insight["check"].as_str().unwrap_or_default().to_string(),
                insight["device"].as_str().unwrap_or_default().to_string(),
                insight["subject"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}
