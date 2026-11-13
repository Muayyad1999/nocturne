//! Shared scaffolding: an in-memory app with the change pipeline registered
//! and a handful of fake switches to point it at.

#![allow(dead_code)]

use nocturne_app::jobs::{self, Job, JobState};
use nocturne_app::testing::{self, Lab};
use nocturne_app::{App, AppResult};
use nocturne_transport::Connection;
use nocturne_transport::serve::{ServerHandle, SessionHandler, SshServerConfig, serve_ssh};
use serde_json::{Value, json};

/// The configuration the fake switch starts with, as `FakeDevice::new` loads
/// it. Tests that need a different starting point load a variant of it.
pub const BASE_CONFIG: &str = "\
hostname SW1
ip domain-name lab.example
vlan 10
 name users
vlan 20
 name voice
interface GigabitEthernet0/1
 description uplink
 switchport mode trunk
interface GigabitEthernet0/2
 description desk 12
 switchport mode access
 switchport access vlan 10
interface Vlan1
 ip address 10.0.0.2 255.255.255.0
ip default-gateway 10.0.0.1
line vty 0 4
 transport input ssh
";

/// Replace a fake device's configuration (running and startup).
pub fn load(lab: &Lab, name: &str, config: &str) {
    lab.fake(name).with(|state| state.load(config));
}

/// The device type the fake switches are driven with in most tests: the
/// one `nocturne_app::testing` ships, marked `verified` so that the risk of
/// an ordinary change is not raised for "nobody has ever checked this device
/// type". `test_ios` itself (experimental) is still there for the tests that
/// want an unverified platform.
pub const VERIFIED_PROFILE: &str = "test_ios_verified";

pub fn verified_profile() -> String {
    format!(
        "{}\nmaturity: verified\n",
        testing::LAB_IOS.replace("id: test_ios", &format!("id: {VERIFIED_PROFILE}"))
    )
}

/// An app with `nocturne-changes` registered and `names` as fake devices.
pub async fn lab(names: &[&str]) -> Lab {
    let mut builder = testing::builder();
    nocturne_changes::init(&mut builder);
    // Registered as a built-in: only built-in device types may call
    // themselves verified.
    nocturne_app::profiles::register(&mut builder, &verified_profile());
    let lab = testing::lab(builder, names).await;
    for name in names {
        lab.app
            .rpc(
                "devices.update",
                json!({"id": lab.id(name), "profile": VERIFIED_PROFILE}),
            )
            .await
            .expect("the verified device type");
    }
    lab
}

/// Wait for the job an RPC started and return it.
pub async fn finish(app: &App, started: &Value) -> Job {
    let job_id = started["job_id"]
        .as_str()
        .unwrap_or_else(|| panic!("a job id in {started}"))
        .to_string();
    jobs::wait(app, &job_id).await.expect("the job record")
}

/// Run an RPC that starts a job and wait for it; panics if the job failed.
pub async fn run_job(app: &App, method: &str, params: Value) -> Job {
    let started = app
        .rpc(method, params)
        .await
        .unwrap_or_else(|err| panic!("{method}: {err}"));
    let job = finish(app, &started).await;
    assert_eq!(
        job.state,
        JobState::Succeeded,
        "{method} failed: {:?}\n{:#?}",
        job.error,
        job.log
    );
    job
}

/// Run a job and give back its record whatever happened.
pub async fn try_job(app: &App, method: &str, params: Value) -> AppResult<Job> {
    let started = app.rpc(method, params).await?;
    Ok(finish(app, &started).await)
}

/// Create a change with one device and one intent, and plan it.
pub async fn planned(app: &App, device_id: &str, intent: Value, options: Value) -> Value {
    let created = app
        .rpc(
            "changes.create",
            json!({
                "title": "test change",
                "targets": [{"device_id": device_id, "intent": intent}],
                "options": options,
            }),
        )
        .await
        .expect("create");
    let id = created["id"].as_str().expect("id").to_string();
    run_job(app, "changes.plan", json!({"id": id})).await;
    app.rpc("changes.get", json!({"id": id}))
        .await
        .expect("get")
}

/// The plan of the first target.
pub fn plan(change: &Value) -> &Value {
    &change["targets"][0]["plan"]
}

pub fn commands(change: &Value) -> Vec<String> {
    plan(change)["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .map(|line| line.as_str().unwrap_or_default().to_string())
        .collect()
}

pub fn rollback(change: &Value) -> Vec<String> {
    plan(change)["rollback"]
        .as_array()
        .expect("rollback")
        .iter()
        .map(|line| line.as_str().unwrap_or_default().to_string())
        .collect()
}

pub fn hit_kinds(change: &Value) -> Vec<String> {
    plan(change)["protected_hits"]
        .as_array()
        .expect("protected_hits")
        .iter()
        .map(|hit| hit["kind"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Approve every target of a change, typing out the name of each device that
/// needs it.
pub async fn approve(app: &App, change: &Value) -> AppResult<Value> {
    let id = change["id"].as_str().expect("id");
    let mut hashes = Vec::new();
    let mut confirm = serde_json::Map::new();
    for target in change["targets"].as_array().expect("targets") {
        let plan = &target["plan"];
        hashes.push(plan["plan_hash"].clone());
        if !plan["protected_hits"]
            .as_array()
            .map(Vec::is_empty)
            .unwrap_or(true)
        {
            let device = app
                .rpc("devices.get", json!({"id": target["device_id"]}))
                .await?;
            let name = device["name"].as_str().unwrap_or_default().to_string();
            confirm.insert(name.clone(), json!(name));
        }
    }
    app.rpc(
        "changes.approve",
        json!({"id": id, "plan_hashes": hashes, "confirm": confirm}),
    )
    .await
}

/// Plan, approve and apply one change; returns the change record afterwards.
pub async fn apply(app: &App, change: &Value) -> Value {
    approve(app, change).await.expect("approve");
    let id = change["id"].as_str().expect("id");
    run_job(app, "changes.apply", json!({"id": id})).await;
    app.rpc("changes.get", json!({"id": id}))
        .await
        .expect("get")
}

/// The state of the first target.
pub fn target_state(change: &Value) -> String {
    change["targets"][0]["result"]["state"]
        .as_str()
        .unwrap_or("(none)")
        .to_string()
}

pub fn steps(change: &Value) -> Vec<String> {
    change["targets"][0]["result"]["steps"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .map(|step| step["name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The configuration of a fake device as the hierarchy engine sees it, so
/// tests can compare two devices (or two moments) without caring about where
/// the device chose to print a line.
pub fn canonical(text: &str) -> Vec<String> {
    use nocturne_drivers::hier::{ConfigTree, Syntax};
    nocturne_changes::compare::lines_of(&ConfigTree::parse(text, Syntax::Ios))
        .into_iter()
        .collect()
}

/// Serve one of the fake switches over a real SSH socket on localhost and
/// put it in the inventory as an ordinary device. It is the only way to have
/// a device the pipeline treats as production rather than as lab gear.
pub async fn production(lab: &Lab, fake: &str, name: &str) -> (String, ServerHandle) {
    struct Handler(nocturne_app::testing::FakeDevice);

    #[async_trait::async_trait]
    impl SessionHandler for Handler {
        async fn open(
            &self,
            _peer: std::net::SocketAddr,
            _user: Option<&str>,
            _cols: u16,
            _rows: u16,
        ) -> Option<Connection> {
            Some(self.0.connection())
        }
    }

    let server = serve_ssh(
        SshServerConfig {
            bind: "127.0.0.1:0".parse().expect("a local address"),
            users: vec![("admin".into(), "pw".into())],
            host_key: None,
            banner: None,
        },
        std::sync::Arc::new(Handler(lab.fake(fake).clone())),
    )
    .await
    .expect("an ssh server");
    let device = lab
        .app
        .rpc(
            "devices.create",
            json!({
                "name": name,
                "address": "127.0.0.1",
                "port": server.local_addr().port(),
                "profile": VERIFIED_PROFILE,
                "credential_id": lab.credential_id,
            }),
        )
        .await
        .expect("a production device");
    (device["id"].as_str().expect("id").to_string(), server)
}

/// Run `work` the first time a step of the apply pipeline is announced, so a
/// test can change the world in the middle of a change (pull a neighbour,
/// take the device away) without racing it.
pub fn when_step(app: &App, step: &str, work: impl FnOnce() + Send + 'static) {
    let mut events = app.subscribe();
    let step = step.to_string();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if event.topic == "changes.progress" && event.payload["step"] == step.as_str() {
                work();
                return;
            }
        }
    });
}

/// Every audit message recorded so far.
pub fn audit(app: &App) -> Vec<(String, String)> {
    app.store()
        .log_query(&nocturne_app::store::LogQuery {
            stream: Some("audit".into()),
            limit: Some(500),
            ..Default::default()
        })
        .expect("the audit trail")
        .into_iter()
        .map(|entry| (entry.source, entry.message))
        .collect()
}

/// The stored configuration versions of a device, newest first.
pub fn versions(app: &App, device_id: &str) -> Vec<(i64, String, String)> {
    app.store()
        .config_versions(device_id, Some("running"))
        .expect("versions")
        .into_iter()
        .map(|version| (version.id, version.source, version.note))
        .collect()
}

/// Make the pipeline give up quickly on a device that has gone away.
pub async fn impatient(app: &App) {
    app.rpc(
        "settings.set",
        json!({"section": "changes", "value": {"reconnect_attempts": 2, "reconnect_interval_s": 1, "connect_wait_s": 2}}),
    )
    .await
    .expect("settings");
}
