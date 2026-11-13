//! Shared scaffolding for this crate's tests.
//!
//! Everything here builds on `nocturne_app::testing`: an in-memory app with
//! the core modules, this feature, and fake IOS switches whose running
//! configuration really changes. Nothing reaches the network, the clock or
//! the file system except through a `tempfile`.

#![allow(dead_code)]

use std::sync::Arc;

use nocturne_app::store::{LogEntry, LogQuery};
use nocturne_app::testing::Lab;
use nocturne_app::{App, AppBuilder, AppResult, Event};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::broadcast::{self, error::TryRecvError};

/// A configuration with one of every shape of secret this crate must hide.
/// It is written the way a device prints it, so it can be loaded into a fake
/// device and read back through a real backup.
pub const SECRETS: &str = "\
enable secret 5 $1$mERr$Th1sIsTheEnableSecretHash
enable password 7 0822455D0A1655
username admin privilege 15 secret 9 $9$NiceTryMate$scryptHashGoesHere0123
username backup password 7 104D000A0618
snmp-server community S3cr3tRead RO
snmp-server community PublicWrite42 RW
snmp-server host 10.0.0.9 version 2c TrapCommunity99
tacacs-server host 10.1.1.1 key 7 094F471A1A0A464058
radius server AUTH
 key 7 13061E010803557878
line vty 0 4
 password 7 030752180500
 transport input ssh
line con 0
 password 7 070C285F4D06
interface Dialer1
 ppp chap password 7 02050D480809
crypto isakmp key MySharedVpnKey address 198.51.100.7
key chain OSPF-KEYS
 key 1
  key-string 7 00071A150754
dot11 ssid CORP
 wpa-psk ascii 7 112A4F1B0A0B5C
ntp authentication-key 1 md5 11223344556677 7
";

/// What a test learns about the calls this crate made to `changes.create`.
#[derive(Default)]
pub struct ChangesStub {
    calls: Mutex<Vec<Value>>,
    /// When set, `changes.create` answers with this error code instead.
    fail: Mutex<Option<(&'static str, String)>>,
}

impl ChangesStub {
    /// Every request sent to `changes.create`, oldest first.
    pub fn calls(&self) -> Vec<Value> {
        self.calls.lock().clone()
    }

    pub fn last(&self) -> Value {
        self.calls
            .lock()
            .last()
            .cloned()
            .expect("a call to `changes.create`")
    }

    /// The lines one device would be sent by the last call.
    pub fn lines_for(&self, device_id: &str) -> Vec<String> {
        let call = self.last();
        let targets = call["targets"].as_array().cloned().unwrap_or_default();
        let target = targets
            .iter()
            .find(|target| target["device_id"] == device_id)
            .unwrap_or_else(|| panic!("`{device_id}` is not a target of the change"));
        target["intent"]["lines"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|line| line.as_str().unwrap_or_default().to_string())
            .collect()
    }

    pub fn fail_with(&self, code: &'static str, message: &str) {
        *self.fail.lock() = Some((code, message.to_string()));
    }
}

/// Register a stand-in for the change pipeline that records what it was sent.
pub fn stub_changes(builder: &mut AppBuilder) -> Arc<ChangesStub> {
    let stub = Arc::new(ChangesStub::default());
    let handle = stub.clone();
    builder
        .router
        .add("changes.create", move |_app: App, params: Value| {
            let handle = handle.clone();
            async move {
                if let Some((code, message)) = handle.fail.lock().clone() {
                    return Err(nocturne_app::AppError::new(code, message));
                }
                handle.calls.lock().push(params);
                Ok(json!({
                    "id": format!("change-{}", handle.calls.lock().len()),
                    "state": "draft",
                }))
            }
        });
    stub
}

/// An app with the core, this feature and `names` as fake switches.
pub async fn lab(names: &[&str]) -> Lab {
    let mut builder = nocturne_app::testing::builder();
    nocturne_config::init(&mut builder);
    nocturne_app::testing::lab(builder, names).await
}

/// The same, with a stand-in for `changes.create`.
pub async fn lab_with_changes(names: &[&str]) -> (Lab, Arc<ChangesStub>) {
    let mut builder = nocturne_app::testing::builder();
    nocturne_config::init(&mut builder);
    let stub = stub_changes(&mut builder);
    (nocturne_app::testing::lab(builder, names).await, stub)
}

/// Everything the app published since this was created.
pub struct Events {
    events: broadcast::Receiver<Event>,
}

impl Events {
    pub fn new(app: &App) -> Events {
        Events {
            events: app.subscribe(),
        }
    }

    /// Everything published so far, oldest first.
    pub fn drain(&mut self) -> Vec<Event> {
        let mut all = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => all.push(event),
                Err(TryRecvError::Lagged(_)) => continue,
                Err(_) => return all,
            }
        }
    }

    /// The payloads published on one topic.
    pub fn of(&mut self, topic: &str) -> Vec<Value> {
        self.drain()
            .into_iter()
            .filter(|event| event.topic == topic)
            .map(|event| event.payload)
            .collect()
    }

    /// The topics seen so far, in order.
    pub fn topics(&mut self) -> Vec<String> {
        self.drain().into_iter().map(|event| event.topic).collect()
    }
}

/// Audit entries whose action (`source`) starts with `prefix`, oldest first.
pub fn audits(app: &App, prefix: &str) -> Vec<LogEntry> {
    let mut entries = app
        .store()
        .log_query(&LogQuery {
            stream: Some("audit".into()),
            limit: Some(500),
            ..LogQuery::default()
        })
        .expect("the audit trail can be read");
    entries.reverse();
    entries.retain(|entry| entry.source.starts_with(prefix));
    entries
}

/// Run a backup of every device and wait for the job to finish.
pub async fn backup(lab: &Lab, params: Value) -> AppResult<nocturne_app::jobs::Job> {
    let started = lab.app.rpc("config.backup", params).await?;
    let job_id = started["job_id"].as_str().expect("job id").to_string();
    nocturne_app::jobs::wait(&lab.app, &job_id).await
}

/// Back `name` up and return the job's result.
pub async fn backup_device(lab: &Lab, name: &str) -> Value {
    let job = backup(lab, json!({"device_ids": [lab.id(name)]}))
        .await
        .expect("the backup starts");
    assert_eq!(
        job.state,
        nocturne_app::jobs::JobState::Succeeded,
        "{:?} / {:?}",
        job.error,
        job.log
    );
    job.result.unwrap_or(Value::Null)
}

/// Load a configuration into a fake device, as if it had always had it.
pub fn load(lab: &Lab, name: &str, config: &str) {
    lab.fake(name).with(|state| state.load(config));
}

/// Change a device the way a person at its console would: over a real CLI
/// session, leaving the startup configuration behind (so the device then has
/// unsaved changes). `save` writes it to memory as `write memory` would.
pub async fn configure(lab: &Lab, name: &str, lines: &[&str], save: bool) {
    let id = lab.id(name);
    let mut cli =
        nocturne_app::connect::cli(&lab.app, &id, nocturne_app::connect::CliOptions::default())
            .await
            .expect("a session to the fake device");
    let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    let result = cli
        .session()
        .configure(&lines)
        .await
        .expect("the fake device accepts the lines");
    assert!(
        result.ok(),
        "the fake device refused a line: {:?}",
        result.failed_at
    );
    if save {
        cli.session().save().await.expect("write memory");
    }
    cli.close().await;
}

/// Every secret value in [`SECRETS`]. If any of these ever appears in an RPC
/// result, an event, a job log or an exported file that was not explicitly
/// asked for unredacted, the crate has leaked a credential.
pub const SECRET_VALUES: &[&str] = &[
    "$1$mERr$Th1sIsTheEnableSecretHash",
    "0822455D0A1655",
    "$9$NiceTryMate$scryptHashGoesHere0123",
    "104D000A0618",
    "S3cr3tRead",
    "PublicWrite42",
    "TrapCommunity99",
    "094F471A1A0A464058",
    "13061E010803557878",
    "030752180500",
    "070C285F4D06",
    "02050D480809",
    "MySharedVpnKey",
    "00071A150754",
    "112A4F1B0A0B5C",
    "11223344556677",
];

/// Fail when any secret of [`SECRETS`] is visible in `text`.
#[track_caller]
pub fn assert_no_secrets(what: &str, text: &str) {
    for secret in SECRET_VALUES {
        assert!(
            !text.contains(secret),
            "{what} leaked the secret `{secret}`:\n{text}"
        );
    }
}

/// The same, for anything serialisable (an RPC result, an event payload).
#[track_caller]
pub fn assert_clean<T: serde::Serialize>(what: &str, value: &T) {
    let text = serde_json::to_string_pretty(value).expect("serialisable");
    assert_no_secrets(what, &text);
}
