//! SSH sessions and the channels opened on them.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use russh::client::{self, DisconnectReason};
use russh::keys::PublicKeyOrCertificate;
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Disconnect, Sig};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use super::algorithms::{CryptoPolicy, Negotiated, client_config, describe_no_common};
use super::auth;
use super::jump;
use super::keys::{HostKeyVerifier, PresentedKey};
use super::sftp::Sftp;
use super::{LegacyCrypto, SshAuth, SshParams};
use crate::connection::{
    Backend, Connection, ConnectionInfo, Features, Input, Output, TransportKind,
};
use crate::error::TransportError;
use crate::net::host_port;
use crate::pump::{Outcome, ReadEnd, WriteEnd, drive};
use crate::relay::Relay;

/// Unread shell output staged per connection before the transport stops
/// reading from the SSH session (see [`crate::relay`]).
const OUTPUT_HIGH_WATER: usize = 4 * 1024 * 1024;
/// russh drops whatever the server sends on a channel once we asked to close
/// it, so there is nothing to wait for beyond what is already in flight.
const CLOSE_GRACE: Duration = Duration::from_millis(100);
/// How long a polite goodbye may take.
const DISCONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Authentication back-ends (RADIUS, TACACS+) can be slow; each exchange gets
/// at least this long.
const MIN_AUTH_TIMEOUT: Duration = Duration::from_secs(30);

const BREAK_UNSUPPORTED: &str = "a break cannot be sent over SSH: russh 0.65 has no way to send \
     the RFC 4335 \"break\" channel request";

// ------------------------------------------------------------------ handler

/// What the russh handler learns about a session, shared with its users.
pub(crate) struct Shared {
    state: Mutex<State>,
    /// The host-key verifier is running (it may be waiting for the user).
    verifying: AtomicBool,
    /// Flips to `true` when the session loop has ended.
    ended: watch::Sender<bool>,
}

#[derive(Default)]
struct State {
    negotiated: Option<Negotiated>,
    server_id: Option<String>,
    banner: Option<String>,
    host_key: Option<PresentedKey>,
    host_key_rejection: Option<String>,
    disconnect: Option<DisconnectCause>,
}

#[derive(Clone, Debug)]
enum DisconnectCause {
    /// The server sent `SSH_MSG_DISCONNECT`.
    Remote { message: String, kex_failed: bool },
    /// The session ended on an error; `None` when we hung up ourselves.
    Local(Option<String>),
}

impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State::default()),
            verifying: AtomicBool::new(false),
            ended: watch::Sender::new(false),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Why the session ended, in words fit for `Output::Closed`.
    pub(crate) fn disconnect_reason(&self) -> Option<String> {
        match self.state().disconnect.clone()? {
            DisconnectCause::Remote { message, .. } if message.trim().is_empty() => {
                Some("disconnected by the server".to_owned())
            }
            DisconnectCause::Remote { message, .. } => {
                Some(format!("disconnected by the server: {}", message.trim()))
            }
            DisconnectCause::Local(reason) => reason,
        }
    }

    /// Resolves once the session loop has ended.
    async fn ended(&self) {
        let mut ended = self.ended.subscribe();
        // An error means the sender is gone, which cannot outlive `self`.
        let _ = ended.wait_for(|ended| *ended).await;
    }
}

/// The russh event handler of one client session.
pub(crate) struct ClientHandler {
    host: String,
    port: u16,
    verifier: Arc<dyn HostKeyVerifier>,
    shared: Arc<Shared>,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let presented = PresentedKey::from_public(&server_public_key.public_key());
        self.shared.verifying.store(true, Ordering::Release);
        let verdict = self
            .verifier
            .verify(&self.host, self.port, &presented)
            .await;
        self.shared.verifying.store(false, Ordering::Release);

        let mut state = self.shared.state();
        state.host_key = Some(presented);
        match verdict {
            Ok(()) => Ok(true),
            Err(reason) => {
                state.host_key_rejection = Some(reason);
                Ok(false)
            }
        }
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let mut state = self.shared.state();
        state.negotiated = Some(Negotiated::from_names(names));
        state.server_id = Some(String::from_utf8_lossy(session.remote_sshid()).into_owned());
        Ok(())
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let mut state = self.shared.state();
        match &mut state.banner {
            Some(existing) => existing.push_str(banner),
            None => state.banner = Some(banner.to_owned()),
        }
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: DisconnectReason<Self::Error>,
    ) -> Result<(), Self::Error> {
        let (cause, result) = match reason {
            DisconnectReason::ReceivedDisconnect(info) => (
                DisconnectCause::Remote {
                    kex_failed: matches!(info.reason_code, Disconnect::KeyExchangeFailed),
                    message: info.message,
                },
                Ok(()),
            ),
            DisconnectReason::Error(error) => {
                (DisconnectCause::Local(describe_session_end(&error)), Err(error))
            }
        };
        self.shared.state().disconnect.get_or_insert(cause);
        self.shared.ended.send_replace(true);
        result
    }
}

/// Words for the error a session loop ended on; `None` for our own hang-up.
fn describe_session_end(error: &russh::Error) -> Option<String> {
    use std::io::ErrorKind;
    Some(match error {
        russh::Error::Disconnect => return None,
        russh::Error::IO(io) => match io.kind() {
            ErrorKind::UnexpectedEof => "connection closed by remote host".to_owned(),
            ErrorKind::ConnectionReset => "connection reset by remote host".to_owned(),
            _ => format!("connection lost: {io}"),
        },
        russh::Error::KeepaliveTimeout => {
            "connection lost: the server stopped answering keep-alives".to_owned()
        }
        other => format!("connection lost: {other}"),
    })
}

// ------------------------------------------------------------------ session

/// An authenticated russh session, shared by everything that runs over it.
pub(crate) struct SessionInner {
    handle: client::Handle<ClientHandler>,
    shared: Arc<Shared>,
    /// The previous hop of a jump chain: kept alive by, and dropped after, us.
    via: Option<Arc<SessionInner>>,
}

impl SessionInner {
    /// Says goodbye to the server and waits briefly for the session to end.
    async fn disconnect(&self) {
        if self.handle.is_closed() {
            return;
        }
        let goodbye = self.handle.disconnect(Disconnect::ByApplication, "", "en");
        if let Ok(Ok(())) = tokio::time::timeout(DISCONNECT_TIMEOUT, goodbye).await {
            let _ = tokio::time::timeout(DISCONNECT_TIMEOUT, self.shared.ended()).await;
        }
    }

    /// Opens a TCP tunnel to `host:port` through this session (`ProxyJump`).
    pub(crate) async fn open_tunnel(
        &self,
        host: &str,
        port: u16,
        limit: Duration,
    ) -> Result<russh::ChannelStream<client::Msg>, TransportError> {
        let opening = self
            .handle
            .channel_open_direct_tcpip(host, u32::from(port), "127.0.0.1", 0);
        let target = host_port(host, port);
        match tokio::time::timeout(limit, opening).await {
            Err(_) => Err(TransportError::ConnectTimeout),
            Ok(Ok(channel)) => Ok(channel.into_stream()),
            Ok(Err(russh::Error::ChannelOpenFailure(reason))) => Err(match reason {
                russh::ChannelOpenFailure::ConnectFailed => TransportError::Unreachable(format!(
                    "the jump host could not connect to {target}"
                )),
                russh::ChannelOpenFailure::AdministrativelyProhibited => TransportError::Protocol(
                    format!("the jump host does not allow forwarding to {target}"),
                ),
                other => TransportError::Protocol(format!(
                    "the jump host refused to open a tunnel to {target}: {}",
                    other.description()
                )),
            }),
            Ok(Err(error)) => Err(channel_error(error)),
        }
    }

}

/// Maps an error from a channel operation on an established session.
fn channel_error(error: russh::Error) -> TransportError {
    match error {
        russh::Error::SendError | russh::Error::RecvError | russh::Error::Disconnect => {
            TransportError::Closed
        }
        russh::Error::ChannelOpenFailure(reason) => TransportError::Protocol(format!(
            "the server refused to open a channel: {}",
            reason.description()
        )),
        russh::Error::IO(error) => TransportError::Io(error),
        other => TransportError::Protocol(other.to_string()),
    }
}

/// Drops a session reference; whoever holds the last one says goodbye
/// properly, down the whole jump chain.
pub(crate) async fn release(session: Arc<SessionInner>) {
    let mut next = Some(session);
    while let Some(session) = next {
        let Some(mut inner) = Arc::into_inner(session) else {
            return; // still in use elsewhere
        };
        inner.disconnect().await;
        next = inner.via.take();
    }
}

/// Where to log in, and as whom.
pub(crate) struct Endpoint<'a> {
    pub host: &'a str,
    pub port: u16,
    pub username: &'a str,
    pub auth: &'a [SshAuth],
}

/// Settings shared by every hop of a connection attempt.
pub(crate) struct ConnectOptions {
    pub timeout: Duration,
    pub keepalive: Option<Duration>,
    pub legacy: LegacyCrypto,
    pub verifier: Arc<dyn HostKeyVerifier>,
}

/// A session that completed the handshake and authentication.
pub(crate) struct Established {
    pub session: Arc<SessionInner>,
    /// Logging in needed a SHA-1 RSA signature.
    pub legacy_signature: bool,
}

/// Runs the SSH handshake and authentication over a connected byte stream.
pub(crate) async fn establish<S>(
    stream: S,
    endpoint: &Endpoint<'_>,
    policy: CryptoPolicy,
    options: &ConnectOptions,
    via: Option<Arc<SessionInner>>,
) -> Result<Established, TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let shared = Arc::new(Shared::new());
    let handler = ClientHandler {
        host: endpoint.host.to_owned(),
        port: endpoint.port,
        verifier: Arc::clone(&options.verifier),
        shared: Arc::clone(&shared),
    };
    let config = Arc::new(client_config(policy, options.keepalive));
    let mut handle = handshake(config, stream, handler, &shared, options.timeout, policy).await?;

    let authenticated = auth::authenticate(
        &mut handle,
        &shared,
        endpoint.username,
        endpoint.auth,
        options.legacy != LegacyCrypto::Never,
        (options.timeout * 3).max(MIN_AUTH_TIMEOUT),
    )
    .await;
    let session = SessionInner {
        handle,
        shared,
        via,
    };
    match authenticated {
        Ok(report) => Ok(Established {
            session: Arc::new(session),
            legacy_signature: report.legacy_signature,
        }),
        Err(error) => {
            session.disconnect().await;
            Err(error)
        }
    }
}

/// Version exchange, key exchange and host-key verification, bounded by
/// `limit` — except while the verifier runs, since it may be asking the user.
async fn handshake<S>(
    config: Arc<client::Config>,
    stream: S,
    handler: ClientHandler,
    shared: &Shared,
    limit: Duration,
    policy: CryptoPolicy,
) -> Result<client::Handle<ClientHandler>, TransportError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let connecting = client::connect_stream(config, stream, handler);
    tokio::pin!(connecting);
    let mut deadline = Instant::now() + limit;
    let failure = loop {
        tokio::select! {
            result = &mut connecting => match result {
                Ok(handle) => return Ok(handle),
                Err(error) => break error,
            },
            () = tokio::time::sleep_until(deadline) => {
                if !shared.verifying.load(Ordering::Acquire) {
                    return Err(TransportError::ConnectTimeout);
                }
                deadline = Instant::now() + limit;
            }
        }
    };

    let mut state = shared.state();
    Err(match failure {
        russh::Error::NoCommonAlgo { kind, theirs, .. } => {
            TransportError::NoCommonAlgorithm(describe_no_common(&kind, &theirs, policy))
        }
        russh::Error::UnknownKey => TransportError::HostKeyRejected(
            state
                .host_key_rejection
                .take()
                .unwrap_or_else(|| "the host key was not accepted".to_owned()),
        ),
        russh::Error::Disconnect => match state.disconnect.take() {
            // Some servers find out first that nothing matches and say so.
            Some(DisconnectCause::Remote {
                message,
                kex_failed: true,
            }) => TransportError::NoCommonAlgorithm(format!(
                "the server aborted the key exchange: {}",
                message.trim()
            )),
            Some(DisconnectCause::Remote { message, .. }) => TransportError::Protocol(format!(
                "the server disconnected during the handshake: {}",
                message.trim()
            )),
            _ => closed_during_handshake(),
        },
        russh::Error::IO(error) => match error.kind() {
            std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe => closed_during_handshake(),
            _ => TransportError::Io(error),
        },
        russh::Error::Version => TransportError::Protocol(
            "the server did not identify itself as an SSH-2 server".to_owned(),
        ),
        russh::Error::WrongServerSig => TransportError::Protocol(
            "the server's signature over the key exchange is invalid".to_owned(),
        ),
        russh::Error::KexInit | russh::Error::Kex => TransportError::Protocol(
            "the key exchange failed (the server may have proposed a Diffie-Hellman group \
             smaller than 2048 bits)"
                .to_owned(),
        ),
        russh::Error::ConnectionTimeout
        | russh::Error::InactivityTimeout
        | russh::Error::Elapsed(_) => TransportError::ConnectTimeout,
        other => TransportError::Protocol(other.to_string()),
    })
}

fn closed_during_handshake() -> TransportError {
    TransportError::Protocol("the server closed the connection during the SSH handshake".to_owned())
}

/// An authenticated SSH connection that can open several channels.
///
/// The underlying connection stays up while the session, or anything opened
/// on it (shells, SFTP), is alive; [`SshSession::close`] ends it at once.
pub struct SshSession {
    session: Arc<SessionInner>,
    info: ConnectionInfo,
    term: String,
    cols: u16,
    rows: u16,
    /// Bounds channel set-up requests.
    request_timeout: Duration,
}

impl fmt::Debug for SshSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SshSession")
            .field("info", &self.info)
            .finish_non_exhaustive()
    }
}

impl SshSession {
    /// Connects (through the jump hosts, if any) and authenticates.
    pub async fn connect(
        params: &SshParams,
        verifier: Arc<dyn HostKeyVerifier>,
    ) -> Result<SshSession, TransportError> {
        let timeout = Duration::from_millis(params.connect_timeout_ms.max(1));
        let options = ConnectOptions {
            timeout,
            keepalive: (params.keepalive_s > 0).then(|| Duration::from_secs(params.keepalive_s)),
            legacy: params.legacy,
            verifier,
        };
        let (established, via) = jump::open_chain(params, &options).await?;
        let session = established.session;

        let info = {
            let state = session.shared.state();
            let negotiated = state.negotiated.clone().unwrap_or_default();
            ConnectionInfo {
                kind: Some(TransportKind::Ssh),
                peer: host_port(&params.host, params.port),
                server_banner: server_banner(state.server_id.as_deref(), state.banner.as_deref()),
                legacy_crypto: negotiated.uses_legacy() || established.legacy_signature,
                mac: negotiated.mac(),
                kex: Some(negotiated.kex),
                cipher: Some(negotiated.cipher),
                host_key_algorithm: Some(negotiated.host_key),
                host_key_fingerprint: state.host_key.as_ref().map(|key| key.fingerprint.clone()),
                via,
            }
        };
        Ok(SshSession {
            session,
            info,
            term: params.term.clone(),
            cols: params.cols,
            rows: params.rows,
            request_timeout: timeout.max(Duration::from_secs(5)),
        })
    }

    pub fn info(&self) -> &ConnectionInfo {
        &self.info
    }

    /// PTY + shell channel as a `Connection`.
    ///
    /// The connection keeps the session alive for as long as it is open.
    /// [`Connection::send_break`] is not available over SSH (see
    /// [`Connection::supports_break`]).
    pub async fn shell(&self) -> Result<Connection, TransportError> {
        let opening = async {
            let mut channel = self.open_channel().await?;
            let mut early = Vec::new();
            let (cols, rows) = (u32::from(self.cols), u32::from(self.rows));
            channel
                .request_pty(true, &self.term, cols, rows, 0, 0, &[])
                .await
                .map_err(channel_error)?;
            if !request_accepted(&mut channel, &mut early).await? {
                tracing::warn!(peer = %self.info.peer, "the server refused to allocate a terminal");
            }
            channel
                .request_shell(true)
                .await
                .map_err(channel_error)?;
            if !request_accepted(&mut channel, &mut early).await? {
                let _ = channel.close().await;
                return Err(TransportError::Protocol(
                    "the server refused to start a shell".to_owned(),
                ));
            }
            Ok((channel, early))
        };
        let (channel, early) = self.bounded(opening).await?;

        let features = Features {
            break_unsupported: Some(BREAK_UNSUPPORTED),
        };
        let (connection, backend) =
            Connection::pair_with(self.info.clone(), Connection::DEFAULT_BUFFER, features);
        tokio::spawn(run_shell(
            channel,
            backend,
            Arc::clone(&self.session),
            early,
        ));
        Ok(connection)
    }

    /// One-shot exec channel (no PTY).
    ///
    /// Collects everything the command prints until the server closes the
    /// channel; fails with [`TransportError::Timeout`] if that takes longer
    /// than `timeout`.
    pub async fn exec(
        &self,
        command: &str,
        timeout: Duration,
    ) -> Result<ExecOutput, TransportError> {
        let deadline = Instant::now() + timeout;
        let Ok(channel) = tokio::time::timeout_at(deadline, self.open_channel()).await else {
            return Err(TransportError::Timeout);
        };
        let mut channel = channel?;
        channel
            .exec(true, command)
            .await
            .map_err(channel_error)?;
        match tokio::time::timeout_at(deadline, collect_exec(&mut channel)).await {
            Ok(result) => result,
            Err(_) => {
                let _ = tokio::time::timeout(DISCONNECT_TIMEOUT, channel.close()).await;
                Err(TransportError::Timeout)
            }
        }
    }

    /// Opens the SFTP subsystem.
    pub async fn sftp(&self) -> Result<Sftp, TransportError> {
        let opening = async {
            let mut channel = self.open_channel().await?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(channel_error)?;
            if !request_accepted(&mut channel, &mut Vec::new()).await? {
                let _ = channel.close().await;
                return Err(TransportError::Unsupported(
                    "the server does not offer the SFTP subsystem".to_owned(),
                ));
            }
            Sftp::start(channel.into_stream(), Arc::clone(&self.session)).await
        };
        self.bounded(opening).await
    }

    /// Disconnects now, ending every shell and SFTP session opened on it.
    pub async fn close(self) {
        self.session.disconnect().await;
        release(self.session).await;
    }

    async fn open_channel(&self) -> Result<Channel<client::Msg>, TransportError> {
        self.session
            .handle
            .channel_open_session()
            .await
            .map_err(channel_error)
    }

    async fn bounded<T>(
        &self,
        work: impl Future<Output = Result<T, TransportError>>,
    ) -> Result<T, TransportError> {
        match tokio::time::timeout(self.request_timeout, work).await {
            Ok(result) => result,
            Err(_) => Err(TransportError::Timeout),
        }
    }
}

/// Shorthand: connect + shell. The session lives as long as the connection.
pub async fn connect_ssh(
    params: &SshParams,
    verifier: Arc<dyn HostKeyVerifier>,
) -> Result<Connection, TransportError> {
    let session = SshSession::connect(params, verifier).await?;
    match session.shell().await {
        // The shell holds the session from here on.
        Ok(connection) => Ok(connection),
        Err(error) => {
            session.close().await;
            Err(error)
        }
    }
}

/// The identification string, then the pre-authentication banner if any.
fn server_banner(server_id: Option<&str>, banner: Option<&str>) -> Option<String> {
    let banner = banner.map(str::trim_end).filter(|text| !text.is_empty());
    match (server_id, banner) {
        (Some(id), Some(banner)) => Some(format!("{id}\n{banner}")),
        (Some(id), None) => Some(id.to_owned()),
        (None, banner) => banner.map(str::to_owned),
    }
}

/// Waits for the reply to a channel request, setting aside any data that
/// arrives first.
async fn request_accepted(
    channel: &mut Channel<client::Msg>,
    early: &mut Vec<Bytes>,
) -> Result<bool, TransportError> {
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => return Ok(true),
            Some(ChannelMsg::Failure) => return Ok(false),
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                early.push(data);
            }
            Some(ChannelMsg::Close) | None => return Err(TransportError::Closed),
            Some(_) => {}
        }
    }
}

// -------------------------------------------------------------------- shell

/// Bridges a shell channel and a [`Backend`] until either side is done.
async fn run_shell(
    channel: Channel<client::Msg>,
    backend: Backend,
    session: Arc<SessionInner>,
    early: Vec<Bytes>,
) {
    let (mut source, sink) = channel.split();
    let Backend { mut input, output } = backend;
    let mut relay = Relay::for_output(output.clone(), OUTPUT_HIGH_WATER);
    for chunk in early {
        relay.push_data(chunk);
    }

    let outcome = drive(
        read_shell(&mut source, &mut relay, &output, &session.shared),
        write_shell(&mut input, &sink),
        CLOSE_GRACE,
    )
    .await;

    // Release the channel and the session before telling the application, so
    // that `Closed` means "everything is torn down".
    let _ = tokio::time::timeout(DISCONNECT_TIMEOUT, sink.close()).await;
    drop((source, sink, input, output));
    release(session).await;
    if let Outcome::Closed(reason) = outcome {
        relay.finish(reason).await;
    }
}

/// Server → application. Always keeps draining the channel (below the
/// high-water mark) so that the session loop is never held up by a slow reader.
async fn read_shell(
    channel: &mut ChannelReadHalf,
    relay: &mut Relay<Output>,
    output: &mpsc::Sender<Output>,
    shared: &Shared,
) -> ReadEnd {
    let mut exit: Option<String> = None;
    loop {
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !relay.is_empty() => {
                if forwarded.is_err() {
                    return ReadEnd::Abandoned;
                }
            }
            () = output.closed() => return ReadEnd::Abandoned,
            message = channel.wait(), if relay.has_room() => match message {
                Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                    relay.push_data(data);
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    exit = Some(format!("exit status {exit_status}"));
                }
                Some(ChannelMsg::ExitSignal { signal_name, error_message, .. }) => {
                    exit = Some(describe_signal(&signal_name, &error_message));
                }
                Some(ChannelMsg::Close) => {
                    let reason = exit.unwrap_or_else(|| "closed by remote host".to_owned());
                    return ReadEnd::Finished(Some(reason));
                }
                None => {
                    let reason = shared
                        .disconnect_reason()
                        .or(exit)
                        .unwrap_or_else(|| "connection lost".to_owned());
                    return ReadEnd::Finished(Some(reason));
                }
                Some(_) => {}
            }
        }
    }
}

/// Application → server.
async fn write_shell(
    input: &mut mpsc::Receiver<Input>,
    channel: &ChannelWriteHalf<client::Msg>,
) -> WriteEnd {
    while let Some(item) = input.recv().await {
        let sent = match item {
            Input::Data(data) => channel.data_bytes(data).await,
            Input::Resize { cols, rows } => {
                channel
                    .window_change(u32::from(cols), u32::from(rows), 0, 0)
                    .await
            }
            // `Connection::send_break` refuses up front; nothing to do for a
            // break that was injected some other way.
            Input::Break => Ok(()),
            Input::Close => {
                let _ = channel.close().await;
                return WriteEnd::CloseRequested;
            }
        };
        if sent.is_err() {
            return WriteEnd::Failed("connection lost".to_owned());
        }
    }
    WriteEnd::InputClosed
}

fn describe_signal(signal: &Sig, message: &str) -> String {
    let name = match signal {
        Sig::ABRT => "ABRT",
        Sig::ALRM => "ALRM",
        Sig::FPE => "FPE",
        Sig::HUP => "HUP",
        Sig::ILL => "ILL",
        Sig::INT => "INT",
        Sig::KILL => "KILL",
        Sig::PIPE => "PIPE",
        Sig::QUIT => "QUIT",
        Sig::SEGV => "SEGV",
        Sig::TERM => "TERM",
        Sig::USR1 => "USR1",
        Sig::Custom(name) => name,
    };
    if message.trim().is_empty() {
        format!("terminated by signal {name}")
    } else {
        format!("terminated by signal {name}: {}", message.trim())
    }
}

// --------------------------------------------------------------------- exec

/// What a command run with [`SshSession::exec`] produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    /// `None` when the server did not report one (killed by a signal, or a
    /// device that never sends it).
    pub exit_status: Option<u32>,
}

async fn collect_exec(channel: &mut Channel<client::Msg>) -> Result<ExecOutput, TransportError> {
    let mut output = ExecOutput::default();
    let mut accepted = false;
    let mut finished = false;
    loop {
        match channel.wait().await {
            Some(ChannelMsg::Success) => accepted = true,
            Some(ChannelMsg::Failure) if !accepted => {
                return Err(TransportError::Protocol(
                    "the server refused to run the command".to_owned(),
                ));
            }
            Some(ChannelMsg::Data { data }) => output.stdout.extend_from_slice(&data),
            Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
                output.stderr.extend_from_slice(&data);
            }
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                output.exit_status = Some(exit_status);
                finished = true;
            }
            Some(ChannelMsg::Eof | ChannelMsg::ExitSignal { .. }) => finished = true,
            Some(ChannelMsg::Close) => return Ok(output),
            // Some devices drop the whole connection once the command is done.
            None if finished => return Ok(output),
            None => return Err(TransportError::Closed),
            Some(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_combines_identification_and_pre_auth_text() {
        assert_eq!(
            server_banner(Some("SSH-2.0-Cisco-1.25"), None).as_deref(),
            Some("SSH-2.0-Cisco-1.25")
        );
        assert_eq!(
            server_banner(Some("SSH-2.0-Cisco-1.25"), Some("Authorized users only\r\n")).as_deref(),
            Some("SSH-2.0-Cisco-1.25\nAuthorized users only")
        );
        assert_eq!(server_banner(Some("SSH-2.0-x"), Some("  \n")).as_deref(), Some("SSH-2.0-x"));
        assert_eq!(server_banner(None, None), None);
    }

    #[test]
    fn session_end_is_described_for_humans() {
        assert_eq!(describe_session_end(&russh::Error::Disconnect), None);
        let eof = russh::Error::IO(std::io::ErrorKind::UnexpectedEof.into());
        assert_eq!(
            describe_session_end(&eof).as_deref(),
            Some("connection closed by remote host")
        );
        let keepalive = describe_session_end(&russh::Error::KeepaliveTimeout).unwrap();
        assert!(keepalive.contains("keep-alives"), "{keepalive}");
    }

    #[test]
    fn signals_are_named() {
        assert_eq!(describe_signal(&Sig::TERM, ""), "terminated by signal TERM");
        assert_eq!(
            describe_signal(&Sig::Custom("XCPU".into()), " cpu limit "),
            "terminated by signal XCPU: cpu limit"
        );
    }
}
