#![allow(dead_code)] // each test binary uses a different part of this module

//! Shared test scaffolding: an in-memory app with the core and this feature.

use std::time::{Duration, Instant};

use nocturne_app::{App, AppBuilder, AppConfig};
use serde_json::{Value, json};

/// A builder with the core modules and the lab feature registered.
pub fn builder() -> AppBuilder {
    let mut builder = nocturne_app::testing::builder();
    nocturne_lab::init(&mut builder);
    builder
}

/// An app whose labs live in `dir`, so it can be restarted.
pub fn builder_at(dir: &std::path::Path) -> AppBuilder {
    let mut builder = AppBuilder::new(AppConfig::at(dir)).expect("an app on disk");
    nocturne_app::init(&mut builder);
    nocturne_lab::init(&mut builder);
    builder
}

pub fn app() -> App {
    builder().build()
}

/// Make the ticker quick so tests do not wait on real time.
pub async fn fast_ticks(app: &App) {
    app.rpc(
        "settings.set",
        json!({"section": "labs", "value": {"tick_ms": 10, "autosave_ms": 50}}),
    )
    .await
    .expect("settings");
}

/// Create a lab from an example and start it.
pub async fn start_example(app: &App, example: &str, name: &str) -> Value {
    let lab = app
        .rpc("labs.create", json!({"name": name, "example": example}))
        .await
        .expect("the lab is created");
    let id = lab["id"].as_str().expect("an id").to_string();
    app.rpc("labs.start", json!({ "id": id }))
        .await
        .expect("the lab starts")
}

/// Wait until `check` is happy, or fail after `within`.
pub async fn until<F>(within: Duration, what: &str, mut check: F)
where
    F: FnMut() -> bool,
{
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// The same, for a check that needs the app.
pub async fn until_rpc(
    app: &App,
    within: Duration,
    what: &str,
    method: &str,
    params: Value,
    mut ok: impl FnMut(&Value) -> bool,
) -> Value {
    let deadline = Instant::now() + within;
    loop {
        let value = app.rpc(method, params.clone()).await.unwrap_or(Value::Null);
        if ok(&value) {
            return value;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {what}: {value}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Why the simulated-host profile did not load, if it did not.
pub async fn profile_problems(app: &App) -> Value {
    app.rpc(
        "profiles.validate",
        json!({"yaml": nocturne_lab::host_profile_yaml()}),
    )
    .await
    .unwrap_or(Value::Null)
}
