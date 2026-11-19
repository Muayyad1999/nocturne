//! Shared test scaffolding: an app with the terminal feature on it, an event
//! recorder that checks the output protocol as it goes, and scripted devices
//! for the login dialogues a `FakeDevice` does not have.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use nocturne_app::connect::{ConnectState, SimConnector};
use nocturne_app::model::LabRef;
use nocturne_app::testing::{self, FakeSim};
use nocturne_app::{App, AppBuilder, AppConfig, AppError, AppResult, Event};
use nocturne_transport::{Backend, Connection, ConnectionInfo, Input, Output, TransportKind};
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::sync::broadcast::error::RecvError;

/// Generous bound for anything a test waits for. Nothing here talks to a
/// network, so reaching it means something is wedged.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// The password of the credential [`scripted`] and [`served`] devices use.
/// Distinctive on purpose: a test can look for it in events, logs and files
/// and be sure a chance match is not what it found.
pub const SECRET: &str = "Zq7-passw0rd-Xk";

// --------------------------------------------------------------- the app

/// A builder with the core and this feature registered.
pub fn builder() -> AppBuilder {
    let mut builder = testing::builder();
    nocturne_terminal::init(&mut builder);
    builder
}

/// The same, but with a data directory (session logs need one).
pub fn builder_at(dir: &std::path::Path) -> AppBuilder {
    let mut builder = AppBuilder::new(AppConfig::at(dir)).expect("app");
    nocturne_app::init(&mut builder);
    nocturne_terminal::init(&mut builder);
    builder
}

/// The same, for a backend whose users are not at this machine.
pub fn builder_remote() -> AppBuilder {
    let mut builder = AppBuilder::new(AppConfig::in_memory().remote_users()).expect("app");
    nocturne_app::init(&mut builder);
    nocturne_terminal::init(&mut builder);
    builder
}

/// An app with fake devices in its inventory, as `nocturne_app::testing::lab`
/// builds it.
pub async fn lab(names: &[&str]) -> testing::Lab {
    testing::lab(builder(), names).await
}

/// Add a credential and return its id.
pub async fn credential(app: &App, name: &str, username: &str, password: &str) -> String {
    let created = app
        .rpc(
            "credentials.create",
            json!({"name": name, "username": username, "password": password}),
        )
        .await
        .expect("credential");
    created["id"].as_str().expect("id").to_string()
}

/// Add a simulated device whose console the scripted connector serves.
pub async fn sim_device(app: &App, name: &str, credential_id: Option<&str>) -> String {
    let created = app
        .rpc(
            "devices.create",
            json!({
                "name": name,
                "profile": "test_ios",
                "credential_id": credential_id,
                "lab": {"lab_id": "scripted", "node": name},
            }),
        )
        .await
        .expect("device");
    created["id"].as_str().expect("id").to_string()
}

// ------------------------------------------------------------- the events

/// Collects the feature's events and checks the output protocol while it does.
pub struct Events {
    rx: tokio::sync::broadcast::Receiver<Event>,
    /// Output seen per session: the bytes, and the last `seq`.
    streams: HashMap<String, (Vec<u8>, u64)>,
    pub seen: Vec<Event>,
    /// `terminal.output` events taken in, per session.
    pub counts: HashMap<String, usize>,
    /// The size of each of those events, in order.
    pub sizes: HashMap<String, Vec<usize>>,
    /// Events that begin with a UTF-8 continuation byte: the previous event
    /// ended in the middle of a character.
    pub split_characters: HashMap<String, usize>,
}

impl Events {
    pub fn new(app: &App) -> Events {
        Events {
            rx: app.subscribe(),
            streams: HashMap::new(),
            seen: Vec::new(),
            counts: HashMap::new(),
            sizes: HashMap::new(),
            split_characters: HashMap::new(),
        }
    }

    /// Start counting output from `seq`, as `terminal.attach` hands it over.
    pub fn attached(&mut self, session_id: &str, scrollback: &[u8], seq: u64) {
        self.streams
            .insert(session_id.to_string(), (scrollback.to_vec(), seq));
    }

    fn take(&mut self, event: Event) {
        if event.topic == "terminal.output" {
            let session_id = event.payload["session_id"].as_str().unwrap_or_default();
            let seq = event.payload["seq"].as_u64().expect("a seq");
            let data = BASE64
                .decode(event.payload["data"].as_str().expect("base64"))
                .expect("base64 output");
            assert!(!data.is_empty(), "an output event carried no bytes");
            let stream = self
                .streams
                .entry(session_id.to_string())
                .or_insert_with(|| (Vec::new(), 0));
            if seq > stream.1 {
                assert_eq!(
                    seq,
                    stream.1 + 1,
                    "output event {seq} of {session_id} leaves a gap after {}",
                    stream.1
                );
                if matches!(data.first(), Some(0x80..=0xBF)) {
                    *self
                        .split_characters
                        .entry(session_id.to_string())
                        .or_default() += 1;
                }
                self.sizes
                    .entry(session_id.to_string())
                    .or_default()
                    .push(data.len());
                stream.0.extend_from_slice(&data);
                stream.1 = seq;
                *self.counts.entry(session_id.to_string()).or_default() += 1;
            }
        }
        self.seen.push(event);
    }

    /// Everything received on `session_id` so far, in order.
    pub fn output(&self, session_id: &str) -> Vec<u8> {
        self.streams
            .get(session_id)
            .map(|(bytes, _)| bytes.clone())
            .unwrap_or_default()
    }

    /// The largest `terminal.output` event seen for a session.
    pub fn largest(&self, session_id: &str) -> usize {
        self.sizes
            .get(session_id)
            .and_then(|sizes| sizes.iter().max().copied())
            .unwrap_or(0)
    }

    pub fn text(&self, session_id: &str) -> String {
        String::from_utf8_lossy(&self.output(session_id)).into_owned()
    }

    /// Read events until `ready` says there are enough.
    ///
    /// On a timeout it says what it was waiting for, which topics arrived and
    /// what each session had actually printed. "Timed out waiting for events"
    /// is the same sentence whichever wait in a test gave up, and on a
    /// platform you cannot run yourself that is the difference between a
    /// diagnosis and a guess.
    pub async fn until(&mut self, what: &str, ready: impl Fn(&Events) -> bool) {
        if ready(self) {
            return;
        }
        let waiting = async {
            loop {
                match self.rx.recv().await {
                    Ok(event) => {
                        self.take(event);
                        if ready(self) {
                            return;
                        }
                    }
                    Err(RecvError::Lagged(missed)) => panic!("the test fell {missed} behind"),
                    Err(RecvError::Closed) => panic!("the event bus closed"),
                }
            }
        };
        let outcome = tokio::time::timeout(PATIENCE, waiting).await;
        if outcome.is_err() {
            let topics: Vec<&str> = self.seen.iter().map(|event| event.topic.as_str()).collect();
            let printed: Vec<String> = self
                .streams
                .keys()
                .map(|id| format!("{id}: {:?}", self.text(id)))
                .collect();
            panic!(
                "timed out after {PATIENCE:?} waiting for {what}\n  topics seen: {topics:?}\n  \
                 printed: {printed:?}"
            );
        }
    }

    /// Read events until the session's output contains `needle`.
    pub async fn wait_for(&mut self, session_id: &str, needle: &str) {
        let id = session_id.to_string();
        let wanted = needle.to_string();
        let what = format!("session {session_id} to print {needle:?}");
        self.until(&what, move |events| events.text(&id).contains(&wanted))
            .await;
    }

    /// Read events until one of `topic` arrives, and return it.
    pub async fn wait_topic(&mut self, topic: &str) -> Event {
        let wanted = topic.to_string();
        let from = self.seen.len();
        let what = format!("a `{topic}` event");
        self.until(&what, |events| {
            events.seen[from..]
                .iter()
                .any(|event| event.topic == wanted)
        })
        .await;
        self.seen[from..]
            .iter()
            .find(|event| event.topic == wanted)
            .cloned()
            .expect("the event just seen")
    }

    /// Take in everything already published without waiting.
    pub fn drain(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.take(event);
        }
    }

    /// Every event of `topic` seen so far.
    pub fn of(&self, topic: &str) -> Vec<&Event> {
        self.seen
            .iter()
            .filter(|event| event.topic == topic)
            .collect()
    }

    /// Whether `text` occurs in any event's payload.
    pub fn any_payload_contains(&self, text: &str) -> bool {
        self.seen
            .iter()
            .any(|event| event.payload.to_string().contains(text))
    }
}

/// The audit trail as text, for checking that a secret is not in it.
pub fn audit_text(app: &App) -> String {
    app.store()
        .log_query(&nocturne_app::store::LogQuery {
            stream: Some("audit".into()),
            ..Default::default()
        })
        .expect("audit")
        .iter()
        .map(|entry| format!("{} {:?}", entry.message, entry.body))
        .collect::<Vec<_>>()
        .join("\n")
}

// --------------------------------------------------------- scripted devices

/// What a scripted console does when a session opens.
#[derive(Clone)]
pub struct Script {
    /// Printed the moment the session opens, in one write.
    pub banner: String,
    /// Printed after `banner`, once the far end has been quiet for `pause`.
    pub after_pause: Option<(Duration, String)>,
    /// Ask for a user name before the password.
    pub ask_username: bool,
    /// Echo what is typed, as a console with echo on does.
    pub echo: bool,
    /// Refuse this many logins before accepting one.
    pub refusals: usize,
    pub username: String,
    pub password: String,
    /// The command prompt once logged in.
    pub prompt: String,
    /// Bytes to print, in one write, as soon as the session opens (instead of
    /// a login dialogue). For the coalescing tests.
    pub blast: Option<Vec<u8>>,
    /// Print the banner and then only listen: no login dialogue at all.
    pub listen_only: bool,
    /// Throw away whatever is typed during `after_pause`, as a console that
    /// has not started reading yet does with the keypress that wakes it.
    pub wake_sink: bool,
}

impl Default for Script {
    fn default() -> Self {
        Script {
            banner: String::new(),
            after_pause: None,
            ask_username: true,
            echo: false,
            refusals: 0,
            username: "admin".into(),
            password: "pw".into(),
            prompt: "SW1>".into(),
            blast: None,
            listen_only: false,
            wake_sink: false,
        }
    }
}

/// What a scripted console was told, for the test to check.
#[derive(Default)]
pub struct Heard {
    /// Complete lines received, in order.
    pub lines: Mutex<Vec<String>>,
    /// Sessions opened on it.
    pub sessions: AtomicUsize,
    /// Bytes received in total.
    pub bytes: AtomicUsize,
}

impl Heard {
    pub fn lines(&self) -> Vec<String> {
        self.lines.lock().clone()
    }

    pub fn count(&self, line: &str) -> usize {
        self.lines
            .lock()
            .iter()
            .filter(|seen| seen.as_str() == line)
            .count()
    }
}

/// A console that plays `script`, on a `Connection::pair` like the lab does.
pub struct ScriptedDevice {
    script: Script,
    pub heard: Arc<Heard>,
}

impl ScriptedDevice {
    pub fn new(script: Script) -> ScriptedDevice {
        ScriptedDevice {
            script,
            heard: Arc::new(Heard::default()),
        }
    }

    pub fn connection(&self, peer: &str) -> Connection {
        let info = ConnectionInfo {
            kind: Some(TransportKind::Sim),
            peer: peer.to_string(),
            ..ConnectionInfo::default()
        };
        let (connection, backend) = Connection::pair(info, 64);
        self.heard.sessions.fetch_add(1, Ordering::AcqRel);
        tokio::spawn(play(self.script.clone(), self.heard.clone(), backend));
        connection
    }
}

async fn send(backend: &Backend, text: &str) -> bool {
    backend
        .output
        .send(Output::Data(text.as_bytes().to_vec().into()))
        .await
        .is_ok()
}

/// Reads one line, echoing it if the script says the console does.
async fn read_line(script: &Script, heard: &Heard, backend: &mut Backend) -> Option<String> {
    let mut line = String::new();
    loop {
        let Input::Data(bytes) = backend.input.recv().await? else {
            continue;
        };
        heard.bytes.fetch_add(bytes.len(), Ordering::AcqRel);
        for byte in bytes.iter().copied() {
            if byte == b'\r' || byte == b'\n' {
                if script.echo {
                    send(backend, "\r\n").await;
                }
                heard.lines.lock().push(std::mem::take(&mut line));
                return Some(heard.lines.lock().last().cloned().unwrap_or_default());
            }
            line.push(byte as char);
            if script.echo {
                send(backend, &(byte as char).to_string()).await;
            }
        }
    }
}

async fn play(script: Script, heard: Arc<Heard>, mut backend: Backend) {
    if let Some(blast) = &script.blast {
        // One write; the transport chunks it as it sees fit.
        let _ = backend
            .output
            .send(Output::Data(blast.clone().into()))
            .await;
        while let Some(input) = backend.input.recv().await {
            if matches!(input, Input::Close) {
                break;
            }
        }
        return;
    }
    if !script.banner.is_empty() && !send(&backend, &script.banner).await {
        return;
    }
    if let Some((pause, text)) = script.after_pause.clone() {
        if script.wake_sink {
            // A console that has not started reading yet: whatever is typed
            // to wake it up is counted but thrown away.
            let draining = async {
                while let Some(input) = backend.input.recv().await {
                    if let Input::Data(bytes) = input {
                        heard.bytes.fetch_add(bytes.len(), Ordering::AcqRel);
                    }
                }
            };
            let _ = tokio::time::timeout(pause, draining).await;
        } else {
            tokio::time::sleep(pause).await;
        }
        if !text.is_empty() && !send(&backend, &text).await {
            return;
        }
    }
    if script.listen_only {
        while read_line(&script, &heard, &mut backend).await.is_some() {}
        return;
    }
    for attempt in 0..=script.refusals {
        if script.ask_username {
            if !send(&backend, "Username: ").await {
                return;
            }
            if read_line(&script, &heard, &mut backend).await.is_none() {
                return;
            }
        }
        if !send(&backend, "Password: ").await {
            return;
        }
        if read_line(&script, &heard, &mut backend).await.is_none() {
            return;
        }
        if attempt < script.refusals {
            if !send(&backend, "\r\n% Login invalid\r\n\r\n").await {
                return;
            }
        } else if !send(&backend, &format!("\r\n{}", script.prompt)).await {
            return;
        }
    }
    // Logged in: echo whatever is typed, as a shell would.
    loop {
        let Some(line) = read_line(&script, &heard, &mut backend).await else {
            return;
        };
        if line.trim() == "exit" {
            let _ = backend.output.send(Output::Closed { reason: None }).await;
            return;
        }
        if !script.echo && !send(&backend, &format!("{line}\r\n")).await {
            return;
        }
        if !send(&backend, &script.prompt).await {
            return;
        }
    }
}

/// Serves scripted consoles by lab node name, and anything else from the
/// fake devices `nocturne_app::testing` set up.
pub struct ScriptSim {
    pub scripted: HashMap<String, Arc<ScriptedDevice>>,
    pub fallback: Option<FakeSim>,
}

#[async_trait::async_trait]
impl SimConnector for ScriptSim {
    async fn open(
        &self,
        app: &nocturne_app::App,
        lab: &LabRef,
        cols: u16,
        rows: u16,
    ) -> AppResult<Connection> {
        if let Some(device) = self.scripted.get(&lab.node) {
            return Ok(device.connection(&format!("lab:{}", lab.node)));
        }
        match &self.fallback {
            Some(fallback) => fallback.open(app, lab, cols, rows).await,
            None => Err(AppError::not_found(format!("lab node `{}`", lab.node))),
        }
    }
}

/// Put `devices` on the app as simulated consoles, keeping the fake devices.
pub fn install_scripts(app: &App, devices: &[(&str, Arc<ScriptedDevice>)]) {
    app.state::<ConnectState>()
        .set_sim_connector(Arc::new(ScriptSim {
            scripted: devices
                .iter()
                .map(|(name, device)| (name.to_string(), device.clone()))
                .collect(),
            fallback: None,
        }));
}

/// The same, but a lab's fake devices keep working.
pub fn install_scripts_with_lab(
    app: &App,
    fakes: Vec<nocturne_app::testing::FakeDevice>,
    devices: &[(&str, Arc<ScriptedDevice>)],
) {
    app.state::<ConnectState>()
        .set_sim_connector(Arc::new(ScriptSim {
            scripted: devices
                .iter()
                .map(|(name, device)| (name.to_string(), device.clone()))
                .collect(),
            fallback: Some(FakeSim { devices: fakes }),
        }));
}

// -------------------------------------------------------------- small helpers

/// `terminal.open` for an inventory device, returning the session info.
pub async fn open_device(app: &App, device_id: &str) -> AppResult<Value> {
    app.rpc(
        "terminal.open",
        json!({"device_id": device_id, "cols": 100, "rows": 30}),
    )
    .await
}

pub fn session_id(info: &Value) -> String {
    info["id"].as_str().expect("session id").to_string()
}

/// Wait until a predicate over the session list holds.
pub async fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let waiting = async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    tokio::time::timeout(PATIENCE, waiting)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

// ------------------------------------------------------ a scripted console app

/// An app whose simulated devices are [`ScriptedDevice`]s.
pub struct Scripted {
    pub app: App,
    pub credential_id: String,
    devices: Vec<(String, String, Arc<ScriptedDevice>)>,
}

impl Scripted {
    pub fn id(&self, node: &str) -> String {
        self.devices
            .iter()
            .find(|(name, ..)| name == node)
            .map(|(_, id, _)| id.clone())
            .expect("a scripted device of that name")
    }

    pub fn heard(&self, node: &str) -> Arc<Heard> {
        self.devices
            .iter()
            .find(|(name, ..)| name == node)
            .map(|(.., device)| device.heard.clone())
            .expect("a scripted device of that name")
    }
}

/// Build an app with one simulated device per script. `with_credential`
/// decides whether those devices have a login to answer with.
pub async fn scripted(nodes: &[(&str, Script)], with_credential: bool) -> Scripted {
    scripted_on(builder(), nodes, with_credential).await
}

/// The same on a builder of the caller's own (one with a data directory, say).
pub async fn scripted_on(
    builder: AppBuilder,
    nodes: &[(&str, Script)],
    with_credential: bool,
) -> Scripted {
    let app = builder.build();
    nocturne_app::profiles::save(&app, testing::LAB_IOS).expect("the test profile loads");
    let credential_id = credential(&app, "lab", "admin", SECRET).await;
    let mut devices = Vec::new();
    for (name, script) in nodes {
        let device = Arc::new(ScriptedDevice::new(script.clone()));
        let id = sim_device(
            &app,
            name,
            with_credential.then_some(credential_id.as_str()),
        )
        .await;
        devices.push((name.to_string(), id, device));
    }
    install_scripts(
        &app,
        &devices
            .iter()
            .map(|(name, _, device)| (name.as_str(), device.clone()))
            .collect::<Vec<_>>(),
    );
    Scripted {
        app,
        credential_id,
        devices,
    }
}

/// Parameters for a local-shell session that runs one command and stops.
///
/// The shell differs by platform and so does the way to run a single command
/// — the product knows this (`default_program` is `$SHELL` or `/bin/sh` on
/// Unix and `powershell.exe` on Windows), so a test that hard-codes `sh` is
/// asserting something about the runner's PATH rather than about this code.
/// A GitHub Windows runner happens to have Git for Windows' `sh.exe` on it,
/// which is why only one of these ever failed there; depending on that is
/// still a bet, and `cmd.exe` is on every Windows machine there is.
pub fn local_shell(unix: &str, windows: &str) -> serde_json::Value {
    let (program, flag, script) = if cfg!(unix) {
        ("sh", "-c", unix)
    } else {
        ("cmd.exe", "/C", windows)
    };
    serde_json::json!({"program": program, "args": [flag, script]})
}

/// Answers the cursor-position report a Windows console asks for as it starts.
///
/// ConPTY opens by sending `ESC[6n` — "terminal, where is the cursor?" — and
/// waits for the answer before letting the program run. In the application a
/// real terminal answers: xterm.js in the browser replies as a matter of
/// course. A test that only collects events has no terminal in it, so it has
/// to answer for itself, and without this a local shell on Windows prints
/// nothing at all but that one query. Not a defect in the product — nothing
/// here is reached unless a client is attached — but it is why the test saw
/// `"\u{1b}[6n"` and no shell output.
pub async fn answer_console_probe(app: &App, session_id: &str, events: &mut Events) {
    if !cfg!(windows) {
        return;
    }
    events.wait_for(session_id, "\u{1b}[6n").await;
    app.rpc(
        "terminal.input",
        serde_json::json!({"session_id": session_id, "data": "\u{1b}[1;1R"}),
    )
    .await
    .expect("answering the cursor-position report");
}
