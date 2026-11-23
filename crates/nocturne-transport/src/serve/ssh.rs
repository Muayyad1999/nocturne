//! SSH server front-end for in-process sessions.
//!
//! Clients log in with a password (as such, or through keyboard-interactive);
//! a server without users lets everybody in, whatever they present. Every
//! shell a client asks for is bridged to a connection of the handler: the
//! terminal size goes into `open`, later changes arrive as `Input::Resize`,
//! and a client that has nothing more to send (`ssh host < script`) shows up
//! as `Input::Close` behind what it did send. When the device closes the
//! session, the client is told exit status 0. `exec`, subsystems (SFTP) and
//! forwarding are refused.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Response, Session};
use russh::{
    Channel, ChannelId, ChannelMsg, ChannelOpenFailure, ChannelReadHalf, ChannelWriteHalf,
    Disconnect, MethodKind, MethodSet, Pty,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::{ServerHandle, SessionHandler, ShutdownSignal, spawn_server};
use crate::connection::{Connection, ConnectionReader, Input, Output};
use crate::drain;
use crate::error::TransportError;
use crate::relay::{Relay, Staging};
use crate::ssh::{CryptoPolicy, Role, decode_private_key, preferred_algorithms};

/// Terminal size assumed until the client says otherwise.
const DEFAULT_SIZE: (u16, u16) = (80, 24);
/// The receive window of a channel: what a client may send before the server
/// has seen it. Consoles need little.
const WINDOW: u32 = 256 * 1024;
/// The window while a device is behind on its input (see [`Congestion`]).
const TRICKLE_WINDOW: u32 = 2;
/// How often a channel is read while its device is behind.
const TRICKLE_PERIOD: Duration = Duration::from_millis(10);
/// Staged input above which a device counts as behind…
const PAUSE_ABOVE: usize = 256 * 1024;
/// …and below which it has caught up.
const RESUME_BELOW: usize = 64 * 1024;
/// Staged input at which a channel is closed on its client. A client that
/// respects its window stays far below (russh may hand over a hundred
/// packets before the window shrinks, [`WINDOW`] is still open then, and
/// the trickle adds next to nothing); this is for those that do not.
const INPUT_LIMIT: usize = 8 * 1024 * 1024;
/// What a client that is cut off for flooding is told (on stderr).
const FLOOD_NOTICE: &str = "\r\nSession closed: too much input that the device does not read.\r\n";
/// Session channels one connection may hold open.
const MAX_CHANNELS: usize = 16;
/// What is taken off a connection while its session is not reading (see
/// [`crate::drain`]): twice what a client that respects its windows can
/// have in flight on every channel it may hold.
const UNREAD_HIGH_WATER: usize = 2 * MAX_CHANNELS * WINDOW as usize;
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
    /// Staged input at which a channel is closed, instead of
    /// [`INPUT_LIMIT`].
    pub input_limit: Option<usize>,
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
    let bind = config.bind;
    let (russh_config, shared) = settings(config, handler, options)?;
    let listener = TcpListener::bind(bind).await?;
    let local_addr = listener.local_addr()?;
    Ok(spawn_server(
        listener,
        local_addr,
        move |stream, peer, shutdown| {
            serve_connection(
                stream,
                peer,
                Arc::clone(&russh_config),
                Arc::clone(&shared),
                shutdown,
            )
        },
    ))
}

/// Serves the one client at the other end of `stream`, whatever kind of
/// stream it is, until that client is gone.
#[cfg(test)]
pub(crate) async fn serve_ssh_over<S>(
    stream: S,
    handler: Arc<dyn SessionHandler>,
    options: SshServerOptions,
) -> Result<(), TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (russh_config, shared) = settings(SshServerConfig::default(), handler, options)?;
    let peer = SocketAddr::from(([127, 0, 0, 1], 0));
    // Nobody asks this server to stop; the sender only has to outlive it.
    let (_running, stopped) = tokio::sync::watch::channel(false);
    serve_connection(stream, peer, russh_config, shared, ShutdownSignal(stopped)).await;
    Ok(())
}

/// What the connections of one server are run with.
fn settings(
    config: SshServerConfig,
    handler: Arc<dyn SessionHandler>,
    options: SshServerOptions,
) -> Result<(Arc<server::Config>, Arc<Shared>), TransportError> {
    let host_key = match &config.host_key {
        Some(text) => decode_private_key(text, None)
            .map_err(|reason| TransportError::Protocol(format!("invalid host key: {reason}")))?,
        None => PrivateKey::random(&mut russh::keys::key::safe_rng(), Algorithm::Ed25519).map_err(
            |error| TransportError::Protocol(format!("cannot generate a host key: {error}")),
        )?,
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
        window_size: WINDOW,
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
        input_limit: options.input_limit.unwrap_or(INPUT_LIMIT),
    });
    Ok((russh_config, shared))
}

/// What every connection of one server shares.
struct Shared {
    users: Vec<(String, String)>,
    /// The authentication methods on offer.
    methods: Vec<MethodKind>,
    handler: Arc<dyn SessionHandler>,
    banner: Option<String>,
    allow_tcp_forwarding: bool,
    input_limit: usize,
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
async fn serve_connection<S>(
    stream: S,
    peer: SocketAddr,
    config: Arc<server::Config>,
    shared: Arc<Shared>,
    mut shutdown: ShutdownSignal,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let logged_in = Arc::new(AtomicBool::new(false));
    let handler = ClientSession {
        peer,
        shared,
        user: None,
        channels: HashMap::new(),
        behind: Arc::new(AtomicUsize::new(0)),
        logged_in: Arc::clone(&logged_in),
    };
    // russh stops reading while it has something to write; a client that
    // does the same (ours does) would wait for us as we wait for it.
    let stream = drain::always_read(stream, UNREAD_HIGH_WATER);
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
    let farewell = handle.disconnect(
        Disconnect::ByApplication,
        goodbye.to_owned(),
        "en".to_owned(),
    );
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
    /// How many channels have a device that is behind on its input.
    behind: Arc<AtomicUsize>,
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
        let congestion = Congestion::new(Arc::clone(&self.behind));
        let input_limit = self.shared.input_limit;
        tokio::spawn(run_channel(channel, attached, congestion, input_limit));
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

    /// How far russh re-opens a window it just re-opened the next time:
    /// see [`Congestion`]. (The setting is per connection, not per channel.)
    fn adjust_window(&mut self, _channel: ChannelId, _current: u32) -> u32 {
        if self.behind.load(Ordering::Acquire) > 0 {
            TRICKLE_WINDOW
        } else {
            WINDOW
        }
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
                if let Some(size) = terminal_size(col_width, row_height) {
                    state.size = size;
                }
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
        if let Some(state) = self.channels.get_mut(&channel)
            && let Some(size) = terminal_size(col_width, row_height)
        {
            state.size = size;
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

/// A terminal size as the rest of the crate knows it: `(columns, rows)`.
/// `None` when the client left it open (RFC 4254 §6.2: zero dimensions are
/// to be ignored), as `ssh -tt` does when it has no terminal of its own.
fn terminal_size(cols: u32, rows: u32) -> Option<(u16, u16)> {
    let dimension = |value: u32| u16::try_from(value).unwrap_or(u16::MAX);
    (cols > 0 && rows > 0).then(|| (dimension(cols), dimension(rows)))
}

/// Flow control for the input of one channel.
///
/// russh re-opens a channel's window as soon as data arrives, whether or not
/// anybody took it, so the window alone never slows a client down. Nor may
/// the channel simply be left unread: russh has one loop per connection, and
/// while that waits for a reader, the client's window adjustments for the
/// *output* sit behind the input nobody reads — a deadlock with every device
/// that waits for its output to be taken before it reads on.
///
/// What a server does get to choose is how far the window is re-opened
/// ([`server::Handler::adjust_window`]). While a device is behind on its
/// input, that is [`TRICKLE_WINDOW`] bytes, and its channel is read every
/// [`TRICKLE_PERIOD`] only: the client all but stops sending, and the
/// connection stays responsive.
struct Congestion {
    /// The connection's count of channels with a device that is behind.
    behind: Arc<AtomicUsize>,
    active: bool,
}

impl Congestion {
    fn new(behind: Arc<AtomicUsize>) -> Self {
        Self {
            behind,
            active: false,
        }
    }

    /// Follows the amount of input staged for the device.
    fn update(&mut self, staged: usize) {
        let active = if self.active {
            staged >= RESUME_BELOW
        } else {
            staged > PAUSE_ABOVE
        };
        if active == self.active {
            return;
        }
        self.active = active;
        if active {
            self.behind.fetch_add(1, Ordering::AcqRel);
        } else {
            self.behind.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl Drop for Congestion {
    fn drop(&mut self) {
        self.update(0);
    }
}

/// Serves one session channel: stays on top of what the client sends from
/// the moment the channel opens, and bridges it to the handler's connection
/// once a shell was requested.
async fn run_channel(
    channel: Channel<Msg>,
    attached: oneshot::Receiver<Connection>,
    congestion: Congestion,
    input_limit: usize,
) {
    let (mut client, sink) = channel.split();

    // Until the shell starts, keep whatever the client already sent.
    let mut early = Staging::for_input(input_limit);
    tokio::pin!(attached);
    let connection = loop {
        tokio::select! {
            biased;
            attached = &mut attached => match attached {
                Ok(connection) => break connection,
                Err(_) => return, // no shell will come: refused, or the client left
            },
            message = client.wait() => match message {
                Some(ChannelMsg::Close) | None => return,
                Some(message) => {
                    if let Some(input) = client_input(message) {
                        early.push(input);
                    }
                    if !early.has_room() {
                        refuse_flood(client, &sink).await;
                        return;
                    }
                }
            }
        }
    };

    let (writer, mut device) = connection.split();
    let mut relay = Relay::with_staged(writer.input.clone(), early);
    let client_end = tokio::select! {
        end = client_to_device(&mut client, &mut relay, congestion) => Some(end),
        // The device session is over and the client was told.
        () = device_to_client(&mut device, &sink) => None,
    };
    match client_end {
        None => {}
        Some(ClientEnd::Left) => {
            // End the device session, then let go of it.
            relay.push(Input::Close);
            let _ = tokio::time::timeout(GOODBYE_TIMEOUT, relay.flush()).await;
        }
        Some(ClientEnd::Flooded) => refuse_flood(client, &sink).await,
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
        } => terminal_size(col_width, row_height).map(|(cols, rows)| Input::Resize { cols, rows }),
        // The client has nothing more to say (`ssh host < script`): let the
        // session finish what was sent, then end it.
        ChannelMsg::Eof => Some(Input::Close),
        _ => None,
    }
}

/// Why the client → device direction of a session stopped.
enum ClientEnd {
    /// The client closed the channel, or its connection is gone.
    Left,
    /// The client sent more than its window allowed, and more than the
    /// server puts up with ([`INPUT_LIMIT`]).
    Flooded,
}

/// Client → device, until the client is gone. Never leaves the channel
/// unread for long, so that russh is not held up; see [`Congestion`].
async fn client_to_device(
    client: &mut ChannelReadHalf,
    relay: &mut Relay<Input>,
    mut congestion: Congestion,
) -> ClientEnd {
    // A device that stopped taking input may still have things to say: the
    // session goes on, and what the client sends is dropped.
    let mut deaf = false;
    // While the device is behind: when the channel is read next.
    let mut next_read = Instant::now();
    loop {
        congestion.update(relay.weight());
        let waiting = congestion.active && Instant::now() < next_read;
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !deaf && !relay.is_empty() => {
                if forwarded.is_err() {
                    deaf = true;
                    relay.clear();
                }
            }
            message = client.wait(), if !waiting => match message {
                Some(ChannelMsg::Close) | None => return ClientEnd::Left,
                Some(message) => {
                    if !deaf && let Some(input) = client_input(message) {
                        relay.push(input);
                    }
                    if !relay.has_room() {
                        return ClientEnd::Flooded;
                    }
                    next_read = Instant::now() + TRICKLE_PERIOD;
                }
            },
            () = tokio::time::sleep_until(next_read), if waiting => {}
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

/// Closes the channel of a client that floods it, saying why.
async fn refuse_flood(client: ChannelReadHalf, sink: &ChannelWriteHalf<Msg>) {
    // Whatever else the client sends is of no interest; russh drops it once
    // nobody listens.
    drop(client);
    // The notice needs room in the client's window, which a client that does
    // not read will not give: it must not keep the rest from going out.
    let notice = sink.extended_data_bytes(1, FLOOD_NOTICE);
    let _ = tokio::time::timeout(GOODBYE_TIMEOUT, notice).await;
    let hang_up = async {
        let _ = sink.exit_status(1).await;
        let _ = sink.eof().await;
        let _ = sink.close().await;
    };
    let _ = tokio::time::timeout(GOODBYE_TIMEOUT, hang_up).await;
}
