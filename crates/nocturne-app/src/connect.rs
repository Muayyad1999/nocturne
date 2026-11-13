//! Reaching devices.
//!
//! One place turns an inventory device into a live connection: it resolves
//! the credential, builds the jump-host chain, checks the SSH host key against
//! what Nocturne has seen before, and — for automation — logs in and hands
//! back a CLI session bound to the device's platform profile.
//!
//! Two entry points:
//!
//! * [`open`] gives the raw byte stream (what a terminal tab needs).
//! * [`cli`] gives a logged-in [`Cli`]: [`Cli::read`] runs a command only if
//!   the platform's policy classes it as read-only; anything else has to go
//!   through [`Cli::session`], which the change engine uses deliberately.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use nocturne_drivers::detect::{self, Detection};
use nocturne_drivers::policy::{Classification, CommandClass};
use nocturne_drivers::profile::Platform;
use nocturne_drivers::session::{
    Channel, CliSession, CommandOutput, Credentials, DriverError, SessionOptions, Transcript,
};
use nocturne_transport::{
    Connection, ConnectionInfo, HostKeyVerifier, LegacyCrypto, Output, PresentedKey, SerialParams,
    SshAuth, SshHop, SshParams, SshSession, TelnetParams, TransportError, connect_serial,
    connect_ssh, connect_telnet,
};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::app::{App, AppBuilder, NoParams};
use crate::credentials::{self, Resolved};
use crate::error::{AppError, AppResult};
use crate::model::{Device, LabRef, Transport};
use crate::util::now_ms;
use crate::{inventory, profiles, settings};

const KNOWN_HOSTS: &str = "known_hosts";
/// Profile used when nothing identifies the device.
pub const GENERIC_PROFILE: &str = "generic";
/// How far a jump-host chain may go before it is certainly a mistake.
pub const MAX_JUMP_HOSTS: usize = 5;

// ------------------------------------------------------------------ settings

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionSettings {
    pub connect_timeout_s: u64,
    /// Seconds between SSH keepalives; `0` turns them off.
    pub keepalive_s: u64,
    /// Seconds allowed from "connected" to the first usable prompt.
    pub login_timeout_s: u64,
    /// `auto` (try modern algorithms, fall back and flag the session), `never` or `always`.
    pub legacy_crypto: String,
    /// `tofu` (remember the key on first contact, refuse a changed one),
    /// `strict` (refuse unknown keys) or `accept` (no checking — lab use only).
    pub host_key_policy: String,
    /// Automated sessions Nocturne may hold to one device at a time.
    pub max_sessions_per_device: u32,
}

impl Default for ConnectionSettings {
    fn default() -> Self {
        ConnectionSettings {
            connect_timeout_s: 10,
            keepalive_s: 30,
            login_timeout_s: 25,
            legacy_crypto: "auto".into(),
            host_key_policy: "tofu".into(),
            max_sessions_per_device: 1,
        }
    }
}

pub fn connection_settings(app: &App) -> ConnectionSettings {
    settings::get(app, "connections").unwrap_or_default()
}

// --------------------------------------------------------------------- state

/// Supplied by the lab module: opens a console on a simulated device.
#[async_trait]
pub trait SimConnector: Send + Sync {
    async fn open(&self, lab: &LabRef, cols: u16, rows: u16) -> AppResult<Connection>;
}

#[derive(Default)]
pub struct ConnectState {
    sim: RwLock<Option<Arc<dyn SimConnector>>>,
    gates: Mutex<HashMap<String, Arc<Semaphore>>>,
    /// What the verifier refused most recently, by `host:port`. The transport
    /// can only hand a rejection back as a sentence, so the fingerprints are
    /// kept here and attached to the error the caller sees.
    refusals: Mutex<HashMap<String, Value>>,
}

impl ConnectState {
    /// Called once by the lab module at start-up.
    pub fn set_sim_connector(&self, connector: Arc<dyn SimConnector>) {
        *self.sim.write() = Some(connector);
    }
}

impl From<TransportError> for AppError {
    fn from(err: TransportError) -> Self {
        // The transport's codes are the UI's: "auth", "host_key", "refused", "unreachable", …
        AppError::new(err.code(), err.to_string())
    }
}

// ----------------------------------------------------------------- host keys

/// A host key Nocturne has accepted.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct KnownHost {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// `SHA256:…`
    pub fingerprint: String,
    /// The public key in OpenSSH format.
    pub openssh: String,
    pub first_seen: i64,
    pub last_seen: i64,
}

fn known_host_id(host: &str, port: u16) -> String {
    format!("{}:{port}", host.trim().to_ascii_lowercase())
}

struct StoreVerifier {
    app: App,
}

#[async_trait]
impl HostKeyVerifier for StoreVerifier {
    async fn verify(&self, host: &str, port: u16, key: &PresentedKey) -> Result<(), String> {
        let app = &self.app;
        let policy = connection_settings(app).host_key_policy;
        let id = known_host_id(host, port);
        let known: Option<KnownHost> = app
            .store()
            .get(KNOWN_HOSTS, &id)
            .map_err(|err| err.message)?;
        let now = now_ms();
        let remember = |first_seen: i64| {
            let record = KnownHost {
                host: host.trim().to_ascii_lowercase(),
                port,
                algorithm: key.algorithm.clone(),
                fingerprint: key.fingerprint.clone(),
                openssh: key.openssh.clone(),
                first_seen,
                last_seen: now,
            };
            app.store()
                .put(KNOWN_HOSTS, &id, &record)
                .map_err(|err| err.message)
        };
        match known {
            Some(known) if known.fingerprint == key.fingerprint => {
                // Refresh "last seen" at most once an hour: this runs on every connection.
                if now - known.last_seen > 3_600_000 {
                    remember(known.first_seen)?;
                }
                forget_refusal(app, &id);
                Ok(())
            }
            Some(known) => {
                let details = json!({
                    "host": host, "port": port,
                    "known": {"algorithm": known.algorithm, "fingerprint": known.fingerprint, "first_seen": known.first_seen},
                    "presented": {"algorithm": key.algorithm, "fingerprint": key.fingerprint, "openssh": key.openssh},
                });
                app.publish("connect.host_key_changed", &details);
                remember_refusal(app, &id, &details);
                if policy == "accept" {
                    tracing::warn!(
                        host,
                        port,
                        "host key changed; accepted because host key checking is off"
                    );
                    remember(now)?;
                    return Ok(());
                }
                app.audit(
                    "connect.host_key_changed",
                    None,
                    format!("Refused to connect to {host}:{port}: its SSH host key changed"),
                    details,
                );
                Err(format!(
                    "the SSH host key of {host}:{port} has changed (was {} {}, now {} {}). \
                     If the device was replaced or re-keyed, trust the new key under Settings → Security → Known hosts and connect again; \
                     if not, someone may be intercepting the connection.",
                    known.algorithm, known.fingerprint, key.algorithm, key.fingerprint
                ))
            }
            None if policy == "strict" => {
                let details = json!({
                    "host": host, "port": port,
                    "presented": {"algorithm": key.algorithm, "fingerprint": key.fingerprint, "openssh": key.openssh},
                });
                app.publish("connect.host_key_unknown", &details);
                remember_refusal(app, &id, &details);
                Err(format!(
                    "{host}:{port} presented an SSH host key Nocturne has not seen ({} {}), and the host key policy is strict. \
                     Verify the fingerprint and trust it under Settings → Security → Known hosts.",
                    key.algorithm, key.fingerprint
                ))
            }
            None => {
                if policy != "accept" {
                    remember(now)?;
                }
                forget_refusal(app, &id);
                Ok(())
            }
        }
    }
}

fn remember_refusal(app: &App, id: &str, details: &Value) {
    app.state::<ConnectState>()
        .refusals
        .lock()
        .insert(id.to_string(), details.clone());
}

fn forget_refusal(app: &App, id: &str) {
    app.state::<ConnectState>().refusals.lock().remove(id);
}

/// Attach the fingerprints to a rejected-host-key error, so the interface can
/// show both keys and offer to trust the new one without listening for events.
fn with_host_key_details(app: &App, device: &Device, err: AppError) -> AppError {
    if err.code != "host_key" {
        return err;
    }
    let port = device
        .port
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or(22);
    let id = known_host_id(&device.address, port);
    match app.state::<ConnectState>().refusals.lock().get(&id) {
        Some(details) => err.with_details(details.clone()),
        None => err,
    }
}

/// The host-key verifier every SSH connection made by Nocturne must use.
pub fn verifier(app: &App) -> Arc<dyn HostKeyVerifier> {
    Arc::new(StoreVerifier { app: app.clone() })
}

// ------------------------------------------------------------ raw connection

#[derive(Clone, Debug)]
pub struct OpenOptions {
    pub cols: u16,
    pub rows: u16,
    pub term: String,
}

impl Default for OpenOptions {
    fn default() -> Self {
        // Wide and tall: automation wants unwrapped lines and no paging.
        OpenOptions {
            cols: 200,
            rows: 50,
            term: "xterm-256color".into(),
        }
    }
}

fn legacy(setting: &str) -> LegacyCrypto {
    match setting {
        "never" => LegacyCrypto::Never,
        "always" => LegacyCrypto::Always,
        _ => LegacyCrypto::Auto,
    }
}

fn port_u16(device: &Device, default: u16) -> AppResult<u16> {
    match device.port {
        None => Ok(default),
        Some(port) => u16::try_from(port)
            .ok()
            .filter(|port| *port != 0)
            .ok_or_else(|| {
                AppError::invalid(format!(
                    "{port} is not a valid TCP port for `{}`",
                    device.name
                ))
            }),
    }
}

fn resolved_credential(app: &App, device: &Device) -> AppResult<Resolved> {
    match &device.credential_id {
        Some(id) => credentials::resolve(app, id),
        None => Ok(Resolved::default()),
    }
}

/// How to authenticate over SSH: a password typed for this session first,
/// then what a stored credential holds, most specific first.
///
/// A key goes before any password because it cannot lock an account, and the
/// same password is never offered twice — that would only burn one of the
/// device's few login attempts.
pub fn ssh_auth_methods(password: Option<&str>, credential: Option<&Resolved>) -> Vec<SshAuth> {
    let mut auth = Vec::new();
    let mut add_password = |password: &str| {
        auth.push(SshAuth::Password {
            password: password.to_string(),
        });
        // Network devices often offer only keyboard-interactive, asking for the same password.
        auth.push(SshAuth::KeyboardInteractive {
            response: password.to_string(),
        });
    };
    if let Some(password) = password {
        add_password(password);
    }
    let stored = credential.and_then(|credential| credential.password.as_ref());
    if let Some(stored) = stored
        && password != Some(stored.as_str())
    {
        add_password(stored);
    }
    if let Some(key) = credential.and_then(|credential| credential.private_key.as_ref()) {
        let passphrase = credential
            .and_then(|credential| credential.passphrase.as_ref())
            .map(|passphrase| passphrase.to_string());
        auth.insert(
            0,
            SshAuth::PrivateKey {
                key: key.to_string(),
                passphrase,
            },
        );
    }
    if auth.is_empty() {
        // No secret at hand: the user's agent may hold a key, and lab gear often needs none.
        auth.push(SshAuth::Agent);
        auth.push(SshAuth::None);
    }
    auth
}

/// Where an SSH connection goes and as whom. Built from an inventory device
/// by [`device_endpoint`], or from what a user typed for an ad-hoc session.
#[derive(Clone, Debug)]
pub struct SshEndpoint {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub auth: Vec<SshAuth>,
    /// The device to go through, if any; its own jump host comes before it.
    pub jump_device_id: Option<String>,
}

/// The endpoint for an inventory device, with its stored credential.
pub fn device_endpoint(app: &App, device: &Device) -> AppResult<SshEndpoint> {
    let credential = resolved_credential(app, device)?;
    if credential.username.is_empty() {
        return Err(AppError::unauthorized(format!(
            "`{}` has no credential with a user name; SSH needs one. Assign a credential to the device.",
            device.name
        )));
    }
    Ok(SshEndpoint {
        host: device.address.clone(),
        port: port_u16(device, 22)?,
        auth: ssh_auth_methods(None, Some(&credential)),
        username: credential.username,
        jump_device_id: device.jump_device_id.clone(),
    })
}

/// Everything needed to open an SSH connection, with the connection settings
/// of this installation applied. One place builds these, so the host-key
/// policy, the jump chain and the legacy-algorithm choice cannot drift apart
/// between the terminal, the collectors and the change pipeline.
pub fn ssh_params(
    app: &App,
    endpoint: SshEndpoint,
    term: &str,
    (cols, rows): (u16, u16),
    keepalive: bool,
) -> AppResult<SshParams> {
    let settings = connection_settings(app);
    let target = format!("{}@{}", endpoint.username, endpoint.host);
    Ok(SshParams {
        jump: jump_chain(app, endpoint.jump_device_id.as_deref(), &target)?,
        host: endpoint.host,
        port: endpoint.port,
        username: endpoint.username,
        auth: endpoint.auth,
        term: term.to_string(),
        cols,
        rows,
        connect_timeout_ms: settings.connect_timeout_s.clamp(1, 120) * 1000,
        keepalive_s: if keepalive { settings.keepalive_s } else { 0 },
        legacy: legacy(&settings.legacy_crypto),
    })
}

/// An authenticated SSH session, which can carry several channels — a shell
/// and SFTP at once, for instance. Uses the same host-key policy as every
/// other connection Nocturne makes.
pub async fn ssh_session(app: &App, params: &SshParams) -> AppResult<SshSession> {
    SshSession::connect(params, verifier(app))
        .await
        .map_err(Into::into)
}

/// The hops to reach a target through `first`, outermost hop first.
pub fn jump_chain(app: &App, first: Option<&str>, target: &str) -> AppResult<Vec<SshHop>> {
    let mut hops = Vec::new();
    let mut next = first.map(str::to_string);
    while let Some(id) = next {
        if hops.len() >= MAX_JUMP_HOSTS {
            return Err(AppError::invalid(format!(
                "the jump-host chain of {target} is more than {MAX_JUMP_HOSTS} hops long"
            )));
        }
        let hop = inventory::find(app, &id).map_err(|_| {
            AppError::invalid(format!(
                "the jump host of {target} (`{id}`) is not in the inventory; pick another"
            ))
        })?;
        if hop.transport != Transport::Ssh {
            return Err(AppError::invalid(format!(
                "`{}` cannot be a jump host: it is not reached over SSH",
                hop.name
            )));
        }
        let credential = resolved_credential(app, &hop)?;
        if credential.username.is_empty() {
            return Err(AppError::unauthorized(format!(
                "the jump host `{}` has no credential with a user name",
                hop.name
            )));
        }
        hops.push(SshHop {
            host: hop.address.clone(),
            port: port_u16(&hop, 22)?,
            username: credential.username.clone(),
            auth: ssh_auth_methods(None, Some(&credential)),
        });
        next = hop.jump_device_id.clone();
    }
    hops.reverse();
    Ok(hops)
}

/// Open the transport to a device. No login automation beyond what the
/// transport itself does (SSH authentication).
pub async fn open(app: &App, device: &Device, options: &OpenOptions) -> AppResult<Connection> {
    let settings = connection_settings(app);
    match device.transport {
        Transport::Ssh => {
            let params = ssh_params(
                app,
                device_endpoint(app, device)?,
                &options.term,
                (options.cols, options.rows),
                true,
            )?;
            connect_ssh(&params, verifier(app))
                .await
                .map_err(|err| with_host_key_details(app, device, err.into()))
        }
        Transport::Telnet => {
            if device.jump_device_id.is_some() {
                return Err(AppError::invalid(format!(
                    "`{}` uses Telnet, which cannot go through a jump host; reach it over SSH or open a terminal on the jump host",
                    device.name
                )));
            }
            let params = TelnetParams {
                host: device.address.clone(),
                port: port_u16(device, 23)?,
                term: options.term.clone(),
                cols: options.cols,
                rows: options.rows,
                connect_timeout_ms: settings.connect_timeout_s.clamp(1, 120) * 1000,
            };
            Ok(connect_telnet(&params).await?)
        }
        Transport::Serial => {
            let params = SerialParams {
                port: device.address.clone(),
                baud: device.port.unwrap_or(9600),
                ..SerialParams::default()
            };
            Ok(connect_serial(&params).await?)
        }
        Transport::Sim => {
            let lab = device.lab.as_ref().ok_or_else(|| {
                AppError::invalid(format!(
                    "`{}` is marked as simulated but belongs to no lab",
                    device.name
                ))
            })?;
            let connector = app.state::<ConnectState>().sim.read().clone();
            match connector {
                Some(connector) => connector.open(lab, options.cols, options.rows).await,
                None => Err(AppError::unavailable(
                    "the lab simulator is not available in this build",
                )),
            }
        }
        Transport::None => Err(AppError::invalid(format!(
            "`{}` is monitor-only (no CLI access configured). Set its connection method to SSH, Telnet or serial first.",
            device.name
        ))),
    }
}

// ------------------------------------------------------------- CLI sessions

/// Adapts a transport connection to the driver's byte channel.
struct ConnChannel {
    connection: Connection,
}

#[async_trait]
impl Channel for ConnChannel {
    async fn send(&mut self, data: &[u8]) -> Result<(), String> {
        self.connection
            .send(data.to_vec())
            .await
            .map_err(|err| err.to_string())
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, String> {
        loop {
            return match self.connection.recv().await {
                Some(Output::Data(bytes)) if bytes.is_empty() => continue,
                Some(Output::Data(bytes)) => Ok(Some(bytes.to_vec())),
                Some(Output::Closed {
                    reason: Some(reason),
                }) if !reason.is_empty() => Err(reason),
                Some(Output::Closed { .. }) | None => Ok(None),
            };
        }
    }
}

#[derive(Clone, Debug)]
pub struct CliOptions {
    /// Enter privileged mode after login.
    pub enable: bool,
    /// How long to wait for another automated session on the same device to finish.
    pub wait: Duration,
}

impl Default for CliOptions {
    fn default() -> Self {
        CliOptions {
            enable: true,
            wait: Duration::from_secs(90),
        }
    }
}

/// A logged-in command-line session on a device.
pub struct Cli {
    pub device: Device,
    pub platform: Arc<Platform>,
    pub info: ConnectionInfo,
    /// The profile was worked out during this connection rather than being known in advance.
    pub detected: bool,
    /// How the profile was worked out, when it was. `profile_id` is `generic`
    /// when nothing identified the device, and `evidence` then says what was tried.
    pub detection: Option<Detection>,
    session: CliSession,
    transcript: UnboundedReceiver<Transcript>,
    _permit: OwnedSemaphorePermit,
}

impl std::fmt::Debug for Cli {
    /// Enough to tell one session from another, and nothing a device said.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cli")
            .field("device", &self.device.name)
            .field("platform", &self.platform.profile.id)
            .field("peer", &self.info.peer)
            .field("detected", &self.detected)
            .finish()
    }
}

impl Cli {
    pub fn classify(&self, command: &str) -> Classification {
        self.platform.policy.classify(command)
    }

    /// Run a command that only reads. Anything the platform's policy does not
    /// recognise as read-only is refused before it is sent.
    pub async fn read(&mut self, command: &str) -> AppResult<CommandOutput> {
        let command = command.trim();
        if command.is_empty() {
            return Err(AppError::invalid("no command given"));
        }
        if command.contains(['\n', '\r']) {
            return Err(AppError::invalid(
                "one command at a time: the command contains a line break",
            ));
        }
        let verdict = self.classify(command);
        if verdict.class == CommandClass::ReadOnly {
            return Ok(self.session.exec(command).await?);
        }
        Err(AppError::forbidden(format!(
            "`{command}` is not a read-only command on {} ({}). Commands that change a device go through a change, where they are reviewed, backed up and can be rolled back.",
            self.platform.profile.name, verdict.reason,
        ))
        .with_details(json!({"command": command, "class": verdict.class, "reason": verdict.reason})))
    }

    /// The driver session itself. Nothing is policy-checked here: this is for
    /// the change engine and other callers that have their own approval step.
    pub fn session(&mut self) -> &mut CliSession {
        &mut self.session
    }

    pub fn hostname(&self) -> &str {
        self.session.hostname()
    }

    /// Everything sent and received so far, as text, and forget it.
    pub fn take_transcript(&mut self) -> String {
        let mut text = String::new();
        while let Ok(item) = self.transcript.try_recv() {
            match item {
                Transcript::Sent { .. } => {}
                Transcript::Received { text: received } => text.push_str(&received),
            }
        }
        text
    }

    pub async fn close(self) {
        self.session.close().await;
    }
}

fn gate(app: &App, device_id: &str, limit: u32) -> Arc<Semaphore> {
    app.state::<ConnectState>()
        .gates
        .lock()
        .entry(device_id.to_string())
        .or_insert_with(|| Arc::new(Semaphore::new(limit.clamp(1, 16) as usize)))
        .clone()
}

/// The profile to drive a device with, or `None` when nothing says what the
/// device is and it has to be found out by probing.
fn which_platform(app: &App, device: &Device) -> AppResult<Option<Arc<Platform>>> {
    if !device.profile.is_empty() {
        return Ok(Some(profiles::platform(app, &device.profile)?));
    }
    // What probing found last time, as long as that profile still exists.
    let remembered = inventory::state_of(app, &device.id)?.detected_profile;
    Ok(remembered.and_then(|id| profiles::registry(app).get(&id)))
}

/// Connect to a device and log in to its CLI.
pub async fn cli(app: &App, device_id: &str, options: CliOptions) -> AppResult<Cli> {
    cli_staged(app, device_id, options)
        .await
        .map_err(|(_, err)| err)
}

/// [`cli`], with failures tagged by how far the attempt got: `connect` or `login`.
async fn cli_staged(
    app: &App,
    device_id: &str,
    options: CliOptions,
) -> Result<Cli, (&'static str, AppError)> {
    let connecting = |err: AppError| ("connect", err);
    let device = inventory::find(app, device_id).map_err(connecting)?;
    let settings = connection_settings(app);

    let semaphore = gate(app, &device.id, settings.max_sessions_per_device);
    let permit = tokio::time::timeout(options.wait, semaphore.acquire_owned())
        .await
        .map_err(|_| {
            connecting(AppError::unavailable(format!(
                "`{}` is busy with another operation; try again when it has finished",
                device.name
            )))
        })?
        .map_err(|_| connecting(AppError::internal("device gate closed")))?;

    let credential = resolved_credential(app, &device).map_err(connecting)?;
    let connection = open(app, &device, &OpenOptions::default())
        .await
        .map_err(connecting)?;
    let info = connection.info().clone();

    let logging_in = |err: AppError| ("login", err);
    let known = which_platform(app, &device).map_err(logging_in)?;
    let driver_credentials = Credentials {
        username: Some(credential.username.clone()).filter(|name| !name.is_empty()),
        password: credential
            .password
            .as_ref()
            .map(|password| password.to_string()),
        enable_secret: credential
            .enable_secret
            .as_ref()
            .map(|secret| secret.to_string()),
    };
    let session_options = SessionOptions {
        login_timeout: Duration::from_secs(settings.login_timeout_s.clamp(5, 300)),
        enable: options.enable,
        init: true,
    };
    let (tap, transcript) = tokio::sync::mpsc::unbounded_channel();
    let channel = Box::new(ConnChannel { connection });
    let (mut session, detection) = match known {
        Some(platform) => {
            let session = CliSession::start(
                channel,
                platform,
                &driver_credentials,
                &session_options,
                Some(tap.clone()),
            )
            .await
            .map_err(|err| logging_in(login_error(&device, err)))?;
            (session, None)
        }
        // Nothing says what this device is: log in with the permissive profile,
        // ask it, and rebind the session to whatever answers.
        None => {
            let registry = profiles::registry(app);
            let (session, detection) = detect::probe_session(
                channel,
                &registry,
                &driver_credentials,
                &session_options,
                info.server_banner.as_deref(),
            )
            .await
            .map_err(|err| logging_in(login_error(&device, err)))?;
            (session, Some(detection))
        }
    };
    // `probe_session` keeps no transcript of its own probing; from here on it is tapped.
    session.set_tap(Some(tap));

    let platform = session.platform().clone();
    let detected = detection
        .as_ref()
        .is_some_and(|detection| detection.profile_id != GENERIC_PROFILE);
    if detected {
        let id = platform.profile.id.clone();
        inventory::update_state(app, &device.id, |state| state.detected_profile = Some(id))
            .map_err(logging_in)?;
    }
    Ok(Cli {
        device,
        platform,
        info,
        detected,
        detection,
        session,
        transcript,
        _permit: permit,
    })
}

/// Say which device a login failure belongs to; the driver only knows the session.
fn login_error(device: &Device, err: DriverError) -> AppError {
    let hint = match &err {
        DriverError::AuthFailed(_) => " Check the credential assigned to the device.",
        DriverError::EnableFailed(_) => " Check the enable secret in the device's credential.",
        DriverError::Timeout { .. } => {
            " The device did not show a prompt Nocturne recognises — check that the device type is right."
        }
        _ => "",
    };
    let mut app_err = AppError::from(err);
    app_err.message = format!("{}: {}{hint}", device.name, app_err.message);
    app_err
}

// ----------------------------------------------------------------------- rpc

/// The outcome of "Test connection".
#[derive(Debug, Serialize)]
pub struct ConnectionTest {
    pub ok: bool,
    /// `connect` or `login`: how far it got when it failed.
    pub failed_at: Option<&'static str>,
    pub error: Option<AppError>,
    pub elapsed_ms: u64,
    pub info: Option<ConnectionInfo>,
    pub hostname: Option<String>,
    pub profile: Option<String>,
    pub profile_detected: bool,
    /// Present when the device type was worked out during this test: how, and from what.
    pub detection: Option<Detection>,
    pub warnings: Vec<String>,
}

pub async fn test(app: &App, device_id: &str) -> AppResult<ConnectionTest> {
    let started = Instant::now();
    let device = inventory::find(app, device_id)?;
    let elapsed = |started: Instant| started.elapsed().as_millis() as u64;
    match cli_staged(
        app,
        &device.id,
        CliOptions {
            wait: Duration::from_secs(20),
            ..CliOptions::default()
        },
    )
    .await
    {
        Ok(session) => {
            let mut warnings: Vec<String> = session.session.warnings().to_vec();
            if session.info.legacy_crypto {
                warnings.push(
                    "Connected with legacy SSH algorithms that modern clients refuse; the device's SSH configuration or software should be updated.".into(),
                );
            }
            let result = ConnectionTest {
                ok: true,
                failed_at: None,
                error: None,
                elapsed_ms: elapsed(started),
                info: Some(session.info.clone()),
                hostname: Some(session.hostname().to_string()).filter(|name| !name.is_empty()),
                profile: Some(session.platform.profile.id.clone()),
                profile_detected: session.detected,
                detection: session.detection.clone(),
                warnings,
            };
            session.close().await;
            Ok(result)
        }
        Err((stage, err)) => Ok(ConnectionTest {
            ok: false,
            failed_at: Some(stage),
            error: Some(err),
            elapsed_ms: elapsed(started),
            info: None,
            hostname: None,
            profile: None,
            profile_detected: false,
            detection: None,
            warnings: Vec::new(),
        }),
    }
}

/// Run read-only commands on a device and return what it printed.
pub async fn exec(
    app: &App,
    device_id: &str,
    commands: &[String],
) -> AppResult<Vec<CommandOutput>> {
    if commands.is_empty() {
        return Err(AppError::invalid("no commands given"));
    }
    if commands.len() > 50 {
        return Err(AppError::invalid("at most 50 commands per call"));
    }
    let mut session = cli(app, device_id, CliOptions::default()).await?;
    // Refuse the whole batch up front rather than stopping half-way.
    if let Some(refused) = commands
        .iter()
        .find(|command| session.classify(command.trim()).class != CommandClass::ReadOnly)
    {
        let err = session
            .read(refused)
            .await
            .expect_err("classified as not read-only");
        session.close().await;
        return Err(err);
    }
    let mut outputs = Vec::with_capacity(commands.len());
    let mut failure = None;
    for command in commands {
        match session.read(command).await {
            Ok(output) => outputs.push(output),
            Err(err) => {
                failure = Some(err);
                break;
            }
        }
    }
    session.close().await;
    match failure {
        // Keep what was collected: the caller sees which command broke the session.
        Some(err) if outputs.is_empty() => Err(err),
        Some(err) => {
            outputs.push(CommandOutput {
                command: commands[outputs.len()].clone(),
                output: String::new(),
                error: Some(err.message),
                elapsed_ms: 0,
            });
            Ok(outputs)
        }
        None => Ok(outputs),
    }
}

#[derive(Deserialize)]
struct IdParams {
    id: String,
}

#[derive(Deserialize)]
struct ExecParams {
    id: String,
    #[serde(default)]
    commands: Vec<String>,
    /// Convenience for a single command.
    #[serde(default)]
    command: Option<String>,
}

#[derive(Deserialize)]
struct HostParams {
    host: String,
    #[serde(default = "default_ssh_port")]
    port: u16,
}

fn default_ssh_port() -> u16 {
    22
}

#[derive(Deserialize)]
struct TrustParams {
    host: String,
    #[serde(default = "default_ssh_port")]
    port: u16,
    algorithm: String,
    fingerprint: String,
    #[serde(default)]
    openssh: String,
}

pub fn init(builder: &mut AppBuilder) {
    builder.state(ConnectState::default());
    settings::declare(builder, "connections", &ConnectionSettings::default());

    let router = &mut builder.router;
    router.add("devices.test", |app: App, params: IdParams| async move {
        test(&app, &params.id).await
    });
    router.add("devices.exec", |app: App, params: ExecParams| async move {
        let mut commands = params.commands;
        commands.extend(params.command);
        let outputs = exec(&app, &params.id, &commands).await?;
        Ok(json!({"outputs": outputs}))
    });

    router.add("hostkeys.list", |app: App, _: NoParams| async move {
        let mut hosts: Vec<KnownHost> = app.store().list(KNOWN_HOSTS)?;
        hosts.sort_by(|a, b| (&a.host, a.port).cmp(&(&b.host, b.port)));
        Ok(hosts)
    });
    router.add(
        "hostkeys.forget",
        |app: App, params: HostParams| async move {
            let removed = app
                .store()
                .delete(KNOWN_HOSTS, &known_host_id(&params.host, params.port))?;
            if removed {
                app.audit(
                    "hostkeys.forget",
                    None,
                    format!("Forgot the SSH host key of {}:{}", params.host, params.port),
                    serde_json::Value::Null,
                );
            }
            Ok(json!({"removed": removed}))
        },
    );
    router.add(
        "hostkeys.trust",
        |app: App, params: TrustParams| async move {
            if !params.fingerprint.starts_with("SHA256:") {
                return Err(AppError::invalid(
                    "the fingerprint must be in the `SHA256:…` form",
                ));
            }
            let id = known_host_id(&params.host, params.port);
            let now = now_ms();
            let record = KnownHost {
                host: params.host.trim().to_ascii_lowercase(),
                port: params.port,
                algorithm: params.algorithm,
                fingerprint: params.fingerprint,
                openssh: params.openssh,
                first_seen: now,
                last_seen: now,
            };
            app.store().put(KNOWN_HOSTS, &id, &record)?;
            app.audit(
                "hostkeys.trust",
                None,
                format!(
                    "Trusted SSH host key {} for {}:{}",
                    record.fingerprint, record.host, record.port
                ),
                serde_json::Value::Null,
            );
            Ok(record)
        },
    );
}

#[cfg(test)]
mod tests {
    use zeroize::Zeroizing;

    fn auth_kinds(auth: &[SshAuth]) -> Vec<&'static str> {
        auth.iter()
            .map(|method| match method {
                SshAuth::Password { .. } => "password",
                SshAuth::PrivateKey { .. } => "key",
                SshAuth::KeyFile { .. } => "key_file",
                SshAuth::KeyboardInteractive { .. } => "interactive",
                SshAuth::Agent => "agent",
                SshAuth::None => "none",
            })
            .collect()
    }

    #[test]
    fn authentication_prefers_keys_then_the_typed_password() {
        assert_eq!(auth_kinds(&ssh_auth_methods(None, None)), ["agent", "none"]);
        assert_eq!(
            auth_kinds(&ssh_auth_methods(Some("typed"), None)),
            ["password", "interactive"]
        );

        let stored = Resolved {
            username: "admin".into(),
            password: Some(Zeroizing::new("stored".into())),
            private_key: Some(Zeroizing::new("KEY".into())),
            passphrase: Some(Zeroizing::new("phrase".into())),
            ..Resolved::default()
        };
        let auth = ssh_auth_methods(Some("typed"), Some(&stored));
        assert_eq!(
            auth_kinds(&auth),
            ["key", "password", "interactive", "password", "interactive"],
            "a key first: it cannot lock an account"
        );
        assert!(matches!(&auth[1], SshAuth::Password { password } if password == "typed"));
        assert!(matches!(&auth[3], SshAuth::Password { password } if password == "stored"));

        // The same password twice would only burn a login attempt.
        assert_eq!(
            auth_kinds(&ssh_auth_methods(Some("stored"), Some(&stored))),
            ["key", "password", "interactive"]
        );
    }

    use super::*;
    use crate::testing::{FakeDevice, Lab, builder, lab};
    use nocturne_transport::serve::{ServerHandle, SessionHandler, SshServerConfig, serve_ssh};
    use serde_json::{Value, json};
    use std::net::SocketAddr;

    async fn one_device() -> (Lab, String) {
        let lab = lab(builder(), &["R1"]).await;
        let id = lab.id("R1");
        (lab, id)
    }

    #[tokio::test]
    async fn cli_logs_in_enables_and_reads() {
        let (lab, id) = one_device().await;
        let mut session = cli(&lab.app, &id, CliOptions::default()).await.unwrap();
        assert_eq!(session.hostname(), "R1");
        assert_eq!(session.platform.profile.id, "test_ios");
        let version = session.read("show version").await.unwrap();
        assert!(version.output.contains("Version 15.2(4)E10"), "{version:?}");
        assert_eq!(version.error, None);
        // A rejected command comes back with the device's error line, not as a transport failure.
        let bad = session.read("show nonsense").await.unwrap();
        assert!(bad.error.unwrap().contains("Invalid input"));
        assert!(session.take_transcript().contains("Cisco IOS Software"));
        session.close().await;
        // (`exit` is sent on close but the fake may not have processed it yet.)
        assert_eq!(
            lab.fake("R1").seen()[..4],
            [
                "enable",
                "terminal length 0",
                "show version",
                "show nonsense"
            ]
        );
    }

    #[tokio::test]
    async fn read_refuses_anything_that_is_not_read_only() {
        let (lab, id) = one_device().await;
        let (app, fake) = (&lab.app, lab.fake("R1"));
        let mut session = cli(app, &id, CliOptions::default()).await.unwrap();
        for command in [
            "configure terminal",
            "reload",
            "clear counters",
            "show version\nreload",
        ] {
            let err = session.read(command).await.unwrap_err();
            assert!(
                err.code == "forbidden" || err.code == "invalid",
                "{command}: {err:?}"
            );
        }
        session.close().await;
        assert!(
            !fake
                .seen()
                .iter()
                .any(|command| command.contains("reload") || command.contains("configure"))
        );

        // Over RPC a batch with one non-read-only command sends nothing at all.
        fake.clear_seen();
        let err = app
            .rpc(
                "devices.exec",
                json!({"id": id, "commands": ["show clock", "write erase"]}),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "forbidden");
        assert!(err.message.contains("write erase"));
        assert!(
            !fake
                .seen()
                .iter()
                .any(|command| command.starts_with("show"))
        );

        let ok = app
            .rpc("devices.exec", json!({"id": "r1", "command": "show clock"}))
            .await
            .unwrap();
        assert!(ok["outputs"][0]["output"].as_str().unwrap().contains("UTC"));
    }

    /// Add a device whose type nobody set, on the fake IOS device `R1`.
    async fn mystery_device(lab: &Lab) -> String {
        let device = lab
            .app
            .rpc(
                "devices.create",
                json!({"name": "mystery", "credential_id": lab.credential_id, "lab": {"lab_id": "testlab", "node": "R1"}}),
            )
            .await
            .unwrap();
        assert_eq!(
            device["profile"], "",
            "the device type is unknown to start with"
        );
        device["id"].as_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn an_unidentified_device_is_identified_by_probing_and_remembered() {
        let lab = lab(builder(), &["R1"]).await;
        let app = &lab.app;
        let id = mystery_device(&lab).await;

        // The fake answers `show version` the way a 2960 running IOS 15.2 does,
        // so the built-in pack should recognise it without being told anything.
        let mut session = cli(app, &id, CliOptions::default()).await.unwrap();
        assert_eq!(session.platform.profile.id, "cisco_ios");
        assert!(session.detected);
        let detection = session.detection.clone().expect("a detection");
        assert_eq!(
            detection.method,
            nocturne_drivers::detect::DetectMethod::Probe
        );
        assert!(
            detection.evidence.contains("Cisco IOS Software"),
            "{detection:?}"
        );

        // The session really is bound to the detected platform: its prompts, its
        // enable and its command policy all apply from here on.
        assert_eq!(session.hostname(), "R1");
        assert!(
            session
                .read("show clock")
                .await
                .unwrap()
                .output
                .contains("UTC")
        );
        assert_eq!(
            session.read("configure terminal").await.unwrap_err().code,
            "forbidden"
        );
        session.close().await;

        // What was learned is remembered, so the next connection does not ask again.
        let device = app.rpc("devices.get", json!({"id": id})).await.unwrap();
        assert_eq!(device["state"]["detected_profile"], "cisco_ios");
        assert_eq!(
            device["profile"], "",
            "the device's own setting is left alone"
        );
        lab.fake("R1").clear_seen();
        let again = cli(app, &id, CliOptions::default()).await.unwrap();
        assert_eq!(again.platform.profile.id, "cisco_ios");
        assert!(!again.detected && again.detection.is_none());
        again.close().await;
        assert!(
            !lab.fake("R1")
                .seen()
                .iter()
                .any(|command| command == "show version"),
            "probed again although the answer was known: {:?}",
            lab.fake("R1").seen()
        );

        let test = app.rpc("devices.test", json!({"id": id})).await.unwrap();
        assert_eq!(
            (test["ok"].as_bool(), test["profile"].as_str()),
            (Some(true), Some("cisco_ios"))
        );
    }

    #[tokio::test]
    async fn a_device_nothing_recognises_still_gets_a_usable_session() {
        let lab = lab(builder(), &["R1"]).await;
        let app = &lab.app;
        // A device that answers, but says nothing any profile knows.
        lab.fake("R1").with(|device| {
            device.outputs.insert(
                "show version".into(),
                "Frobnicator 9000 firmware 1.0".into(),
            );
        });
        let id = mystery_device(&lab).await;

        let mut session = cli(app, &id, CliOptions::default()).await.unwrap();
        assert_eq!(
            session.platform.profile.id, "generic",
            "the permissive profile is the fallback"
        );
        assert!(
            !session.detected,
            "nothing was identified, so nothing was learned"
        );
        let detection = session
            .detection
            .clone()
            .expect("a detection that says it found nothing");
        assert!(
            detection.evidence.contains("nothing matched"),
            "{detection:?}"
        );
        // Read-only commands still work, so the terminal and ad-hoc reads are usable.
        assert!(
            session
                .read("show clock")
                .await
                .unwrap()
                .output
                .contains("UTC")
        );
        session.close().await;

        // Nothing was remembered, so a later connection will try again.
        let device = app.rpc("devices.get", json!({"id": id})).await.unwrap();
        assert_eq!(device["state"]["detected_profile"], Value::Null);
    }

    #[tokio::test]
    async fn test_connection_reports_success_and_the_stage_of_failure() {
        let (lab, id) = one_device().await;
        let app = &lab.app;
        let ok = app.rpc("devices.test", json!({"id": id})).await.unwrap();
        assert_eq!(
            (
                ok["ok"].as_bool(),
                ok["hostname"].as_str(),
                ok["profile"].as_str()
            ),
            (Some(true), Some("R1"), Some("test_ios"))
        );

        // Wrong enable secret: the connection works, the login does not.
        app.rpc(
            "credentials.update",
            json!({"id": lab.credential_id, "enable_secret": "wrong"}),
        )
        .await
        .unwrap();
        let failed = app.rpc("devices.test", json!({"id": id})).await.unwrap();
        assert_eq!(
            (failed["ok"].as_bool(), failed["failed_at"].as_str()),
            (Some(false), Some("login"))
        );
        assert!(
            failed["error"]["message"]
                .as_str()
                .unwrap()
                .contains("enable secret"),
            "{failed}"
        );

        // A monitor-only device explains itself.
        let probe = app
            .rpc(
                "devices.create",
                json!({"name": "probe", "address": "192.0.2.1", "transport": "none"}),
            )
            .await
            .unwrap();
        let refused = app
            .rpc("devices.test", json!({"id": probe["id"]}))
            .await
            .unwrap();
        assert_eq!(refused["failed_at"], "connect");
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains("monitor-only")
        );
    }

    #[tokio::test]
    async fn one_automated_session_per_device_at_a_time() {
        let (lab, id) = one_device().await;
        let first = cli(&lab.app, &id, CliOptions::default()).await.unwrap();
        let busy = cli(
            &lab.app,
            &id,
            CliOptions {
                wait: Duration::from_millis(50),
                ..CliOptions::default()
            },
        )
        .await;
        assert_eq!(busy.err().map(|err| err.code), Some("unavailable"));
        first.close().await;
        cli(
            &lab.app,
            &id,
            CliOptions {
                wait: Duration::from_millis(50),
                ..CliOptions::default()
            },
        )
        .await
        .unwrap()
        .close()
        .await;
    }

    struct Handler(FakeDevice);

    #[async_trait]
    impl SessionHandler for Handler {
        async fn open(
            &self,
            _peer: SocketAddr,
            _user: Option<&str>,
            _cols: u16,
            _rows: u16,
        ) -> Option<Connection> {
            Some(self.0.connection())
        }
    }

    async fn ssh_server(bind: SocketAddr, fake: &FakeDevice) -> ServerHandle {
        let config = SshServerConfig {
            bind,
            users: vec![("admin".into(), "pw".into())],
            host_key: None,
            banner: None,
        };
        serve_ssh(config, Arc::new(Handler(fake.clone())))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn ssh_host_keys_are_learned_then_enforced() {
        let (lab, _) = one_device().await;
        let (app, fake) = (&lab.app, lab.fake("R1"));
        let server = ssh_server("127.0.0.1:0".parse().unwrap(), fake).await;
        let addr = server.local_addr();
        let device = app
            .rpc(
                "devices.create",
                json!({"name": "ssh-r1", "address": "127.0.0.1", "port": addr.port(), "profile": "test_ios", "credential_id": lab.credential_id}),
            )
            .await
            .unwrap();
        let id = device["id"].as_str().unwrap();

        // First contact: the key is remembered.
        let mut session = cli(app, id, CliOptions::default()).await.unwrap();
        assert!(
            session
                .read("show clock")
                .await
                .unwrap()
                .output
                .contains("UTC")
        );
        assert!(
            session
                .info
                .host_key_fingerprint
                .as_deref()
                .unwrap()
                .starts_with("SHA256:")
        );
        session.close().await;
        let known = app.rpc("hostkeys.list", Value::Null).await.unwrap();
        assert_eq!(known.as_array().unwrap().len(), 1);
        assert_eq!(known[0]["port"], addr.port());
        let first_fingerprint = known[0]["fingerprint"].as_str().unwrap().to_string();

        // The same port answers with a different key: refused, announced, audited.
        server.shutdown().await;
        let server = ssh_server(addr, fake).await;
        let mut events = app.subscribe();
        let refused = app.rpc("devices.test", json!({"id": id})).await.unwrap();
        assert_eq!(
            (refused["ok"].as_bool(), refused["error"]["code"].as_str()),
            (Some(false), Some("host_key")),
            "{refused}"
        );
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains(&first_fingerprint)
        );
        // The error carries both keys, so the interface can offer to trust the
        // new one without having to catch the announcement.
        let presented = refused["error"]["details"]["presented"].clone();
        assert!(
            presented["fingerprint"]
                .as_str()
                .unwrap()
                .starts_with("SHA256:"),
            "{refused}"
        );
        assert_eq!(
            refused["error"]["details"]["known"]["fingerprint"],
            first_fingerprint.as_str()
        );
        let mut announced = None;
        while let Ok(event) = events.try_recv() {
            if event.topic == "connect.host_key_changed" {
                announced = Some(event.payload["presented"].clone());
            }
        }
        assert_eq!(announced.expect("a host_key_changed event"), presented);

        // The operator trusts the new key; the connection works again.
        app.rpc(
            "hostkeys.trust",
            json!({"host": "127.0.0.1", "port": addr.port(), "algorithm": presented["algorithm"], "fingerprint": presented["fingerprint"], "openssh": presented["openssh"]}),
        )
        .await
        .unwrap();
        assert_eq!(
            app.rpc("devices.test", json!({"id": id})).await.unwrap()["ok"],
            true
        );

        // Strict policy refuses hosts it has never seen.
        assert_eq!(
            app.rpc(
                "hostkeys.forget",
                json!({"host": "127.0.0.1", "port": addr.port()})
            )
            .await
            .unwrap()["removed"],
            true
        );
        app.rpc(
            "settings.set",
            json!({"section": "connections", "value": {"host_key_policy": "strict"}}),
        )
        .await
        .unwrap();
        let strict = app.rpc("devices.test", json!({"id": id})).await.unwrap();
        assert_eq!(strict["error"]["code"], "host_key");
        assert!(
            strict["error"]["message"]
                .as_str()
                .unwrap()
                .contains("strict")
        );

        // Wrong password is an authentication failure at the connect stage.
        app.rpc(
            "settings.set",
            json!({"section": "connections", "value": {"host_key_policy": "tofu"}}),
        )
        .await
        .unwrap();
        app.rpc(
            "credentials.update",
            json!({"id": lab.credential_id, "password": "nope"}),
        )
        .await
        .unwrap();
        let denied = app.rpc("devices.test", json!({"id": id})).await.unwrap();
        assert_eq!(
            (
                denied["failed_at"].as_str(),
                denied["error"]["code"].as_str()
            ),
            (Some("connect"), Some("auth")),
            "{denied}"
        );
        server.shutdown().await;
    }
}
