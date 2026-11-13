//! SSH server front-end for in-process sessions.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Response, Session};
use russh::{
    Channel, ChannelId, ChannelMsg, ChannelOpenFailure, ChannelReadHalf, ChannelWriteHalf,
    Disconnect, MethodKind, MethodSet, Pty,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

use super::{ServerHandle, SessionHandler, ShutdownSignal, spawn_server};
use crate::connection::{Connection, ConnectionReader, Input, Output};
use crate::error::TransportError;
use crate::relay::Relay;
use crate::ssh::{CryptoPolicy, Role, decode_private_key, preferred_algorithms};

/// Terminal size assumed until the client says otherwise.
const DEFAULT_SIZE: (u16, u16) = (80, 24);
/// Unconsumed client input staged per channel before the server stops
/// reading from that client (see [`crate::relay`]).
const INPUT_HIGH_WATER: usize = 1024 * 1024;
/// Session channels one connection may hold open.
const MAX_CHANNELS: usize = 16;
/// A client must have identified itself within this time…
const HELLO_TIMEOUT: Duration = Duration::from_secs(30);
/// …and be logged in within this one.
const LOGIN_GRACE: Duration = Duration::from_secs(120);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);
/// How long a goodbye (to the client, or to the bridged session) may take.
const GOODBYE_TIMEOUT: Duration = Duration::from_secs(2);

/// How [`serve_ssh`] listens and whom it lets in.
#[derive(Clone)]
pub struct SshServerConfig {
    pub bind: SocketAddr,
    /// user → password. Empty = accept any credentials.
    pub users: Vec<(String, String)>,
    /// OpenSSH private key; generated (Ed25519) when None.
    pub host_key: Option<String>,
    /// Shown to clients before they authenticate.
    pub banner: Option<String>,
}

impl Default for SshServerConfig {
    /// Loopback, any free port, any credentials, a fresh host key.
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], 0)),
            users: Vec::new(),
            host_key: None,
            banner: None,
        }
    }
}

/// Passwords and the host key stay out of logs.
impl fmt::Debug for SshServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let users: Vec<&str> = self.users.iter().map(|(user, _)| user.as_str()).collect();
        f.debug_struct("SshServerConfig")
            .field("bind", &self.bind)
            .field("users", &users)
            .field("host_key", &self.host_key.as_ref().map(|_| "<redacted>"))
            .field("banner", &self.banner)
            .finish()
    }
}

/// Knobs that are not part of the public configuration (tests exercise
/// legacy algorithms, restricted authentication and jump hosts with them).
#[derive(Default)]
pub(crate) struct SshServerOptions {
    /// Algorithms to offer instead of the modern set.
    pub preferred: Option<russh::Preferred>,
    /// Authentication methods to offer instead of the default ones.
    pub methods: Option<Vec<MethodKind>>,
    /// Serve `direct-tcpip` channels, i.e. act as a jump host.
    pub allow_tcp_forwarding: bool,
    /// Keep-alive interval and tolerated misses, instead of 30 s × 3.
    pub keepalive: Option<(Duration, usize)>,
}

/// Serves SSH on `config.bind`, bridging every shell to the connection the
/// handler opens for it.
pub async fn serve_ssh(
    config: SshServerConfig,
    handler: Arc<dyn SessionHandler>,
) -> Result<ServerHandle, TransportError> {
    serve_ssh_with(config, handler, SshServerOptions::default()).await
}

pub(crate) async fn serve_ssh_with(
    config: SshServerConfig,
    handler: Arc<dyn SessionHandler>,
    options: SshServerOptions,
) -> Result<ServerHandle, TransportError> {
    let host_key = match &config.host_key {
        Some(text) => decode_private_key(text, None)
            .map_err(|reason| TransportError::Protocol(format!("invalid host key: {reason}")))?,
        None => PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519)
            .map_err(|error| {
                TransportError::Protocol(format!("cannot generate a host key: {error}"))
            })?,
    };
    let open_house = config.users.is_empty();
    let methods = options.methods.unwrap_or_else(|| {
        let mut methods = vec![MethodKind::Password, MethodKind::KeyboardInteractive];
        if open_house {
            methods.extend([MethodKind::PublicKey, MethodKind::None]);
        }
        methods
    });
    let (keepalive_interval, keepalive_max) = options.keepalive.unwrap_or((KEEPALIVE_INTERVAL, 3));

    let russh_config = Arc::new(server::Config {
        server_id: russh::SshId::Standard(Cow::Borrowed(concat!(
            "SSH-2.0-Nocturne_",
            env!("CARGO_PKG_VERSION")
        ))),
        methods: MethodSet::from(methods.as_slice()),
        // Lab credentials are not worth slowing every wrong guess for long,
        // and the "none" probe every client starts with should cost nothing.
        auth_rejection_time: Duration::from_millis(300),
        auth_rejection_time_initial: Some(Duration::ZERO),
        max_auth_attempts: 6,
        keys: vec![host_key],
        preferred: options
            .preferred
            .unwrap_or_else(|| preferred_algorithms(CryptoPolicy::Modern, Role::Server)),
        // Idle terminals are normal; dead peers are found by keep-alives.
        inactivity_timeout: None,
        keepalive_interval: Some(keepalive_interval),
        keepalive_max,
        ..server::Config::default()
    });
    let shared = Arc::new(Shared {
        users: config.users,
        methods,
        handler,
        banner: config.banner,
        allow_tcp_forwarding: options.allow_tcp_forwarding,
    });

    let listener = TcpListener::bind(config.bind).await?;
    let local_addr = listener.local_addr()?;
    Ok(spawn_server(listener, local_addr, move |stream, peer, shutdown| {
        serve_connection(
            stream,
            peer,
            Arc::clone(&russh_config),
            Arc::clone(&shared),
            shutdown,
        )
    }))
}

/// What every connection of one server shares.
struct Shared {
    users: Vec<(String, String)>,
    /// The authentication methods on offer.
    methods: Vec<MethodKind>,
    handler: Arc<dyn SessionHandler>,
    banner: Option<String>,
    allow_tcp_forwarding: bool,
}

impl Shared {
    /// Whether clients are let in without a matching entry in `users`.
    fn open_house(&self) -> bool {
        self.users.is_empty()
    }

    fn offers(&self, method: MethodKind) -> bool {
        self.methods.contains(&method)
    }
}

/// Runs one client connection to its end.
async fn serve_connection(
    stream: TcpStream,
    peer: SocketAddr,
    config: Arc<server::Config>,
    shared: Arc<Shared>,
    mut shutdown: ShutdownSignal,
) {
    let logged_in = Arc::new(AtomicBool::new(false));
    let handler = ClientSession {
        peer,
        shared,
        user: None,
        channels: HashMap::new(),
        logged_in: Arc::clone(&logged_in),
    };
    let hello = server::run_stream(config, stream, handler);
    let session = match tokio::time::timeout(HELLO_TIMEOUT, hello).await {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            tracing::debug!(%peer, %error, "SSH handshake failed");
            return;
        }
        Err(_) => {
            tracing::debug!(%peer, "client never identified itself");
            return;
        }
    };
    let handle = session.handle();
    tokio::pin!(session);

    let login_deadline = tokio::time::sleep(LOGIN_GRACE);
    tokio::pin!(login_deadline);
    let mut awaiting_login = true;
    let goodbye = loop {
        tokio::select! {
            result = &mut session => {
                if let Err(error) = result {
                    tracing::debug!(%peer, %error, "SSH session ended");
                }
                return;
            }
            () = shutdown.requested() => break "the server is shutting down",
            () = &mut login_deadline, if awaiting_login => {
                if !logged_in.load(Ordering::Acquire) {
                    break "login timed out";
                }
                awaiting_login = false;
            }
        }
    };
    let farewell = handle.disconnect(Disconnect::ByApplication, goodbye.to_owned(), "en".to_owned());
    if let Ok(Ok(())) = tokio::time::timeout(GOODBYE_TIMEOUT, farewell).await {
        let _ = tokio::time::timeout(GOODBYE_TIMEOUT, &mut session).await;
    }
}

/// The russh event handler of one client connection.
struct ClientSession {
    peer: SocketAddr,
    shared: Arc<Shared>,
    /// Set once authentication succeeded.
    user: Option<String>,
    channels: HashMap<ChannelId, ChannelState>,
    logged_in: Arc<AtomicBool>,
}

struct ChannelState {
    /// The client's terminal size (columns, rows).
    size: (u16, u16),
    /// Hands the bridged connection to the channel's task; taken by the
    /// first shell request.
    attach: Option<oneshot::Sender<Connection>>,
}

impl ClientSession {
    fn password_ok(&self, user: &str, password: &str) -> bool {
        let users = &self.shared.users;
        self.shared.open_house() || users.iter().any(|(u, p)| u == user && p == password)
    }

    fn verdict(&mut self, user: &str, accepted: bool) -> Auth {
        if accepted {
            self.user = Some(user.to_owned());
            Auth::Accept
        } else {
            Auth::reject()
        }
    }
}

impl server::Handler for ClientSession {
    type Error = russh::Error;

    async fn auth_none(&mut self, user: &str) -> Result<Auth, Self::Error> {
        let accepted = self.shared.offers(MethodKind::None) && self.shared.open_house();
        Ok(self.verdict(user, accepted))
    }

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        let accepted = self.shared.offers(MethodKind::Password) && self.password_ok(user, password);
        Ok(self.verdict(user, accepted))
    }

    async fn auth_publickey_offered(
        &mut self,
        _user: &str,
        _offer: server::PublicKeyOffer<'_>,
    ) -> Result<Auth, Self::Error> {
        // Keys are only meaningful where any credentials are accepted.
        Ok(
            if self.shared.offers(MethodKind::PublicKey) && self.shared.open_house() {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }

    async fn auth_publickey(
        &mut self,
        user: &str,
        _key: server::VerifiedPublicKey<'_>,
    ) -> Result<Auth, Self::Error> {
        let accepted = self.shared.offers(MethodKind::PublicKey) && self.shared.open_house();
        Ok(self.verdict(user, accepted))
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        user: &str,
        _submethods: &str,
        response: Option<Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        if !self.shared.offers(MethodKind::KeyboardInteractive) {
            return Ok(Auth::reject());
        }
        let Some(mut answers) = response else {
            return Ok(Auth::Partial {
                name: Cow::Borrowed(""),
                instructions: Cow::Borrowed(""),
                prompts: Cow::Borrowed(&[(Cow::Borrowed("Password: "), false)]),
            });
        };
        let answer = answers.next().unwrap_or_default();
        let accepted = self.password_ok(user, &String::from_utf8_lossy(&answer));
        Ok(self.verdict(user, accepted))
    }

    async fn auth_succeeded(&mut self, _session: &mut Session) -> Result<(), Self::Error> {
        self.logged_in.store(true, Ordering::Release);
        Ok(())
    }

    async fn authentication_banner(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.shared.banner.clone())
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.channels.len() >= MAX_CHANNELS {
            reply.reject(ChannelOpenFailure::ResourceShortage).await;
            return Ok(());
        }
        let (attach, attached) = oneshot::channel();
        self.channels.insert(
            channel.id(),
            ChannelState {
                size: DEFAULT_SIZE,
                attach: Some(attach),
            },
        );
        // The task drains the channel from the start: russh delivers channel
        // messages from inside its session loop and would stall otherwise.
        tokio::spawn(run_channel(channel, attached));
        reply.accept().await;
        Ok(())
    }

    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host_to_connect: &str,
        port_to_connect: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let port = u16::try_from(port_to_connect).ok();
        let (true, Some(port)) = (self.shared.allow_tcp_forwarding, port) else {
            reply
                .reject(ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        };
        let host = host_to_connect.to_owned();
        tokio::spawn(async move {
            match TcpStream::connect((host.as_str(), port)).await {
                Ok(mut target) => {
                    reply.accept().await;
                    let mut tunnel = channel.into_stream();
                    let _ = tokio::io::copy_bidirectional(&mut tunnel, &mut target).await;
                }
                Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
            }
        });
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match self.channels.get_mut(&channel) {
            Some(state) => {
                state.size = (dimension(col_width), dimension(row_height));
                session.channel_success(channel)
            }
            None => session.channel_failure(channel),
        }
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Once the shell runs, the channel task forwards the resize in order
        // with the data; this only matters before that.
        if let Some(state) = self.channels.get_mut(&channel) {
            state.size = (dimension(col_width), dimension(row_height));
        }
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let pending = self
            .channels
            .get_mut(&channel)
            .and_then(|state| Some((state.attach.take()?, state.size)));
        let Some((attach, (cols, rows))) = pending else {
            return session.channel_failure(channel);
        };
        let opened = self
            .shared
            .handler
            .open(self.peer, self.user.as_deref(), cols, rows)
            .await;
        let started = opened.is_some_and(|connection| attach.send(connection).is_ok());
        if started {
            session.channel_success(channel)
        } else {
            session.channel_failure(channel)
        }
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)
    }

    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        _name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        _variable_name: &str,
        _variable_value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)
    }

    async fn x11_request(
        &mut self,
        channel: ChannelId,
        _single_connection: bool,
        _x11_auth_protocol: &str,
        _x11_auth_cookie: &str,
        _x11_screen_number: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)
    }
}

/// A terminal dimension as the rest of the crate knows it.
fn dimension(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

/// Serves one session channel: stays on top of what the client sends from
/// the moment the channel opens, and bridges it to the handler's connection
/// once a shell was requested.
async fn run_channel(channel: Channel<Msg>, attached: oneshot::Receiver<Connection>) {
    let (mut client, sink) = channel.split();

    // Until the shell starts, keep whatever the client already typed.
    let mut early = Vec::new();
    let mut early_bytes = 0;
    tokio::pin!(attached);
    let connection = loop {
        tokio::select! {
            biased;
            attached = &mut attached => match attached {
                Ok(connection) => break connection,
                Err(_) => return, // no shell will come: refused, or the client left
            },
            message = client.wait(), if early_bytes < INPUT_HIGH_WATER => match message {
                Some(ChannelMsg::Close) | None => return,
                Some(message) => {
                    if let ChannelMsg::Data { data } = &message {
                        early_bytes += data.len();
                    }
                    early.extend(client_input(message));
                }
            }
        }
    };

    let (writer, mut device) = connection.split();
    let mut relay = Relay::new(writer.input.clone(), Input::Data, INPUT_HIGH_WATER);
    for input in early {
        stage(&mut relay, input);
    }

    tokio::select! {
        () = client_to_device(&mut client, &mut relay) => {
            // The client is gone: end the device session, then let go of it.
            relay.push_item(Input::Close);
            let _ = tokio::time::timeout(GOODBYE_TIMEOUT, relay.flush()).await;
        }
        () = device_to_client(&mut device, &sink) => {}
    }
}

/// What a channel message from the client means for the bridged session.
fn client_input(message: ChannelMsg) -> Option<Input> {
    match message {
        ChannelMsg::Data { data } => Some(Input::Data(data)),
        ChannelMsg::WindowChange {
            col_width,
            row_height,
            ..
        } => Some(Input::Resize {
            cols: dimension(col_width),
            rows: dimension(row_height),
        }),
        // The client has nothing more to say (`ssh host < script`): let the
        // session finish what was sent, then end it.
        ChannelMsg::Eof => Some(Input::Close),
        _ => None,
    }
}

fn stage(relay: &mut Relay<Input>, input: Input) {
    match input {
        Input::Data(data) => relay.push_data(data),
        other => relay.push_item(other),
    }
}

/// Client → device, until the client closes the channel or the device stops
/// taking input. Keeps draining the channel so russh is never held up.
async fn client_to_device(client: &mut ChannelReadHalf, relay: &mut Relay<Input>) {
    loop {
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !relay.is_empty() => {
                if forwarded.is_err() {
                    return;
                }
            }
            message = client.wait(), if relay.has_room() => match message {
                Some(ChannelMsg::Close) | None => return,
                Some(message) => {
                    if let Some(input) = client_input(message) {
                        stage(relay, input);
                    }
                }
            }
        }
    }
}

/// Device → client, until the device session ends or the client is gone.
async fn device_to_client(device: &mut ConnectionReader, sink: &ChannelWriteHalf<Msg>) {
    loop {
        match device.recv().await {
            Some(Output::Data(data)) => {
                if sink.data_bytes(data).await.is_err() {
                    return;
                }
            }
            Some(Output::Closed { .. }) | None => {
                let _ = sink.exit_status(0).await;
                let _ = sink.eof().await;
                let _ = sink.close().await;
                return;
            }
        }
    }
}
