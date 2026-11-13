//! The harness the integration tests share: a real app with the core
//! registered, this feature registered, fake devices in the inventory, and
//! small stubs for the methods other feature crates would provide (this crate
//! must build and be tested on its own, and it reaches its neighbours only by
//! method name).

// Every integration test file gets its own copy of this module and uses a
// different part of it.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use nocturne_app::testing::{self, Lab};
use nocturne_app::{App, AppBuilder, jobs};
use nocturne_automation::clock::VirtualClock;
use nocturne_automation::state::AutomationState;
use serde_json::{Value, json};

/// Every `changes.create` the feature made, in order — the stub stands in for
/// `nocturne-changes`, which this crate must not depend on.
#[derive(Default)]
pub struct Changes {
    pub created: Mutex<Vec<Value>>,
    /// What `changes.get` should answer with, by id.
    pub records: Mutex<Vec<Value>>,
}

/// Devices `config.backup` was asked for.
#[derive(Default)]
pub struct Backups(pub Mutex<Vec<String>>);

fn stubs(builder: &mut AppBuilder) {
    builder.state(Changes::default());
    builder.state(Backups::default());
    builder
        .router
        .add("changes.create", |app: App, params: Value| async move {
            let state = app.state::<Changes>();
            let mut created = state.created.lock().expect("lock");
            let id = format!("chg-{}", created.len() + 1);
            created.push(params.clone());
            let record = json!({
                "id": id,
                "change_id": id,
                "title": params.get("title").cloned().unwrap_or(Value::Null),
                "state": "draft",
                "targets": params.get("targets").cloned().unwrap_or(Value::Null),
            });
            state.records.lock().expect("lock").push(record);
            Ok(json!({"change_id": id, "state": "draft"}))
        });
    builder
        .router
        .add("changes.get", |app: App, params: Value| async move {
            let wanted = params.get("id").and_then(Value::as_str).unwrap_or_default();
            let state = app.state::<Changes>();
            let records = state.records.lock().expect("lock");
            records
                .iter()
                .find(|record| record.get("id").and_then(Value::as_str) == Some(wanted))
                .cloned()
                .ok_or_else(|| nocturne_app::AppError::not_found("change"))
        });
    builder
        .router
        .add("config.backup", |app: App, params: Value| async move {
            let ids: Vec<String> = params
                .get("device_ids")
                .and_then(Value::as_array)
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            app.state::<Backups>()
                .0
                .lock()
                .expect("lock")
                .extend(ids.clone());
            let job = jobs::start(
                &app,
                jobs::JobSpec::new("config.backup", "Back up").devices(&ids),
                |_ctx| async move { Ok(json!({"backed_up": 1})) },
            );
            Ok(jobs::started(&job))
        });
    // Stands in for something a schedule may call that counts its calls.
    builder
        .router
        .add("insights.scan", |app: App, _params: Value| async move {
            let state = app.state::<Scans>();
            let mut count = state.0.lock().expect("lock");
            *count += 1;
            Ok(json!({"scanned": *count}))
        });
    builder.state(Scans::default());
}

#[derive(Default)]
pub struct Scans(pub Mutex<u32>);

/// A lab with `names` as fake devices, this feature and the stubs registered.
pub async fn lab(names: &[&str]) -> Lab {
    let mut builder = testing::builder();
    nocturne_automation::init(&mut builder);
    stubs(&mut builder);
    testing::lab(builder, names).await
}

/// The same, with a clock that follows tokio's paused timer.
pub async fn lab_at(names: &[&str], base_ms: i64) -> (Lab, Arc<VirtualClock>) {
    let mut builder = testing::builder();
    nocturne_automation::init(&mut builder);
    stubs(&mut builder);
    let clock = Arc::new(VirtualClock::new(base_ms));
    // After `init`, so this replaces the state it registered.
    builder.state(AutomationState::with_clock(clock.clone()));
    let lab = testing::lab(builder, names).await;
    (lab, clock)
}

/// Spin the runtime until `ready` holds, or give up. For paused-time tests,
/// where "the other task has got as far as X" is not a matter of waiting.
pub async fn until(mut ready: impl FnMut() -> bool) {
    for _ in 0..2000 {
        if ready() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the condition never became true");
}

/// Let paused time pass one second at a time, yielding in between.
///
/// A background task that has not been polled yet has no timer registered, so
/// one big `advance` would step straight over it. One second at a time gives
/// every loop its turn.
pub async fn tick(seconds: u32) {
    for _ in 0..seconds {
        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        tokio::task::yield_now().await;
    }
}

/// [`tick`] until `ready` holds, at most `seconds` of it.
pub async fn tick_until(seconds: u32, mut ready: impl FnMut() -> bool) {
    for _ in 0..seconds {
        if ready() {
            return;
        }
        tick(1).await;
    }
    // One last chance for work that the final tick started.
    until(ready).await;
}

/// Wait for a job to finish and return its record.
pub async fn finished(app: &App, result: &Value) -> jobs::Job {
    let job_id = result
        .get("job_id")
        .and_then(Value::as_str)
        .expect("the result carries a job id");
    jobs::wait(app, job_id).await.expect("the job record")
}

/// Everything one run recorded, by device name.
pub async fn results(app: &App, run_id: &str) -> Vec<Value> {
    let detail = app
        .rpc("runner.get", json!({"run_id": run_id}))
        .await
        .expect("runner.get");
    detail["results"]
        .as_array()
        .cloned()
        .expect("results array")
}

pub fn state_of(results: &[Value], name: &str) -> String {
    results
        .iter()
        .find(|result| result["name"] == json!(name))
        .unwrap_or_else(|| panic!("`{name}` has no result in this run"))
        .get("state")
        .and_then(Value::as_str)
        .expect("a state")
        .to_string()
}
