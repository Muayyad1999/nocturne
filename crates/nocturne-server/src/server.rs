//! Listening, serving connections and shutting down.

use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::extract::ConnectInfo;
use axum::response::Response;
use hyper::body::Incoming;
use hyper::server::conn::{http1, http2};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use nocturne_suite::App;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::guard::{OriginPolicy, Scheme};
use crate::options::{ServerOptions, Timing, is_loopback};
use crate::state::{InFlight, ServerState};
use crate::ui::{self, UiSource};
use crate::{router, tls};

/// Connections served at once. Beyond this, new ones wait in the kernel's
/// queue instead of using up the process's file descriptors.
const MAX_CONNECTIONS: usize = 512;

/// HTTP/2 connections are pinged so that dead peers do not linger.
const HTTP2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(20);
const HTTP2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// The first byte of every TLS connection: a handshake record.
const TLS_HANDSHAKE_RECORD: u8 = 0x16;

/// A running server.
///
/// Dropping the handle does not stop the server: it then runs until the tokio
/// runtime ends. Call [`shutdown`](Self::shutdown) to stop it in an orderly way.
pub struct ServerHandle {
    local_addr: SocketAddr,
    scheme: Scheme,
    token: String,
    shutdown: CancellationToken,
    task: JoinHandle<()>,
}

impl ServerHandle {
    /// The address the server listens on, with the real port when `0` was asked for.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The address to reach the server at from this machine, without the
    /// token: `http://127.0.0.1:7878/`.
    pub fn base_url(&self) -> String {
        // The wildcard address is where it listens, not where it can be reached.
        let host = match self.local_addr.ip() {
            IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.to_string(),
            IpAddr::V6(ip) if ip.is_unspecified() => format!("[{}]", Ipv6Addr::LOCALHOST),
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        format!(
            "{}://{host}:{}/",
            self.scheme.as_str(),
            self.local_addr.port()
        )
    }

    /// The link to open in a browser. Its fragment carries the access token,
    /// which the UI picks up and removes from the address bar.
    pub fn url(&self) -> String {
        format!("{}#token={}", self.base_url(), self.token)
    }

    /// Stop the server: no new connections, WebSockets are closed, and
    /// requests in progress get the grace period (5 s) to finish.
    pub async fn shutdown(self) {
        self.shutdown.cancel();
        if let Err(err) = self.task.await {
            tracing::error!(%err, "the server task failed");
        }
    }
}

impl fmt::Debug for ServerHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerHandle")
            .field("url", &self.base_url())
            .finish_non_exhaustive()
    }
}

/// Start serving `app`. Returns as soon as the listener is bound.
pub async fn serve(app: App, options: ServerOptions) -> anyhow::Result<ServerHandle> {
    serve_with_timing(app, options, Timing::default()).await
}

/// [`serve`] with other protocol timings than the documented ones.
pub async fn serve_with_timing(
    app: App,
    options: ServerOptions,
    timing: Timing,
) -> anyhow::Result<ServerHandle> {
    options.validate()?;
    let tls_config = options.tls.as_ref().map(tls::server_config).transpose()?;
    let ui = match &options.ui_dir {
        Some(dir) => UiSource::directory(dir)?,
        None => UiSource::embedded(),
    };

    let listener = TcpListener::bind(options.bind)
        .await
        .map_err(|err| explain_bind_failure(options.bind, err))?;
    let local_addr = listener
        .local_addr()
        .context("the listening socket has no local address")?;
    let scheme = if tls_config.is_some() {
        Scheme::Https
    } else {
        Scheme::Http
    };

    if scheme == Scheme::Http && !is_loopback(local_addr.ip()) {
        tracing::warn!(
            address = %local_addr,
            "serving plain HTTP on a network address: the access token and device passwords \
             travel unencrypted"
        );
    }
    if options.token.len() < 16 {
        tracing::warn!("the access token is shorter than 16 characters and can be guessed");
    }
    tracing::info!(
        address = %local_addr,
        scheme = scheme.as_str(),
        ui = match &options.ui_dir {
            Some(_) => "directory",
            None if ui::ui_is_embedded() => "embedded",
            None => "not built (fallback page)",
        },
        "listening"
    );

    let shutdown = CancellationToken::new();
    let state = ServerState::new(
        app,
        &options.token,
        OriginPolicy::new(scheme, local_addr),
        ui,
        timing,
        shutdown.clone(),
    );
    let in_flight = state.in_flight.clone();
    let connections = Arc::new(Connections {
        router: router::build(state),
        tls: tls_config.map(TlsAcceptor::from),
        timing,
        shutdown: shutdown.clone(),
    });
    let task = tokio::spawn(accept_loop(listener, connections, in_flight));

    Ok(ServerHandle {
        local_addr,
        scheme,
        token: options.token,
        shutdown,
        task,
    })
}

fn explain_bind_failure(address: SocketAddr, err: io::Error) -> anyhow::Error {
    let hint = match err.kind() {
        io::ErrorKind::AddrInUse => {
            ": another program (perhaps another Nocturne) already uses that port; stop it or \
             choose another port with --port"
        }
        io::ErrorKind::AddrNotAvailable => {
            ": that address is not configured on any interface of this machine"
        }
        io::ErrorKind::PermissionDenied => {
            ": the system does not allow this program to use that port (ports below 1024 \
             usually need elevated rights)"
        }
        _ => "",
    };
    anyhow::Error::new(err).context(format!("could not listen on {address}{hint}"))
}

/// What every connection needs.
struct Connections {
    router: Router,
    tls: Option<TlsAcceptor>,
    timing: Timing,
    shutdown: CancellationToken,
}

async fn accept_loop(listener: TcpListener, connections: Arc<Connections>, in_flight: InFlight) {
    let limit = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = connections.shutdown.cancelled() => break,
            Some(finished) = tasks.join_next(), if !tasks.is_empty() => report(finished),
            accepted = accept(&listener, &limit) => match accepted {
                Ok((stream, remote, permit)) => {
                    let connections = connections.clone();
                    tasks.spawn(async move {
                        serve_connection(stream, remote, &connections).await;
                        drop(permit);
                    });
                }
                Err(err) if is_connection_error(&err) => {}
                Err(err) => {
                    // Usually "too many open files". Waiting lets descriptors be
                    // returned; spinning on the error would not.
                    tracing::error!(%err, "could not accept a connection");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            },
        }
    }
    drop(listener);

    // Connections were told to finish by the same token that ended the loop.
    let grace = connections.timing.shutdown_grace;
    let drained = async {
        while let Some(finished) = tasks.join_next().await {
            report(finished);
        }
        in_flight.drained().await;
    };
    if tokio::time::timeout(grace, drained).await.is_err() {
        tracing::warn!(
            connections = tasks.len(),
            "requests were still running {} s after shutdown began; closing their connections",
            grace.as_secs()
        );
        tasks.shutdown().await;
    }
}

async fn accept(
    listener: &TcpListener,
    limit: &Arc<Semaphore>,
) -> io::Result<(TcpStream, SocketAddr, OwnedSemaphorePermit)> {
    let permit = limit
        .clone()
        .acquire_owned()
        .await
        .expect("the connection limit is never closed");
    let (stream, remote) = listener.accept().await?;
    Ok((stream, remote, permit))
}

/// Errors that concern one incoming connection, not the listener.
fn is_connection_error(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
    )
}

fn report(finished: Result<(), tokio::task::JoinError>) {
    if let Err(err) = finished
        && err.is_panic()
    {
        tracing::error!(%err, "a connection task panicked");
    }
}

async fn serve_connection(stream: TcpStream, remote: SocketAddr, connections: &Connections) {
    // Replies and events are small and latency matters more than packet count.
    let _ = stream.set_nodelay(true);

    let Some(acceptor) = &connections.tls else {
        return serve_http1(TokioIo::new(stream), remote, connections).await;
    };
    // The handshake is bounded like the request head: an idle peer must not hold the connection.
    let handshake = tokio::time::timeout(
        connections.timing.header_read_timeout,
        tls_handshake(stream, acceptor),
    );
    let stream = tokio::select! {
        biased;
        _ = connections.shutdown.cancelled() => return,
        outcome = handshake => match outcome {
            Ok(Ok(Some(stream))) => stream,
            Ok(Ok(None)) => return,
            Ok(Err(err)) => {
                tracing::debug!(%remote, %err, "TLS handshake failed");
                return;
            }
            Err(_) => {
                tracing::debug!(%remote, "TLS handshake timed out");
                return;
            }
        },
    };
    let negotiated_http2 = stream.get_ref().1.alpn_protocol() == Some(b"h2".as_slice());
    if negotiated_http2 {
        serve_http2(TokioIo::new(stream), remote, connections).await;
    } else {
        serve_http1(TokioIo::new(stream), remote, connections).await;
    }
}

/// `None` when the peer was not speaking TLS (it has been told, if it spoke HTTP) or left.
async fn tls_handshake(
    stream: TcpStream,
    acceptor: &TlsAcceptor,
) -> io::Result<Option<TlsStream<TcpStream>>> {
    let mut first = [0u8; 1];
    if stream.peek(&mut first).await? == 0 {
        return Ok(None);
    }
    if first[0] != TLS_HANDSHAKE_RECORD {
        // `GET …`, `POST …`: somebody typed http:// for an https:// port.
        if first[0].is_ascii_uppercase() {
            refuse_plain_http(stream).await;
        }
        return Ok(None);
    }
    acceptor.accept(stream).await.map(Some)
}

async fn refuse_plain_http(mut stream: TcpStream) {
    const BODY: &str = "This port speaks HTTPS, not HTTP: use https:// in the address.\n";
    let reply = format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
        BODY.len()
    );
    if stream.write_all(reply.as_bytes()).await.is_err() || stream.shutdown().await.is_err() {
        return;
    }
    // Closing with the request still unread would reset the connection, and
    // the client might never see the reply. Read it (briefly) first.
    let mut scratch = [0u8; 1024];
    let unread = async { while matches!(stream.read(&mut scratch).await, Ok(read) if read > 0) {} };
    let _ = tokio::time::timeout(Duration::from_secs(1), unread).await;
}

async fn serve_http1<I>(io: I, remote: SocketAddr, connections: &Connections)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        // Also closes keep-alive connections that stay idle this long.
        .header_read_timeout(connections.timing.header_read_timeout);
    let connection = builder
        .serve_connection(
            io,
            request_service(connections.router.clone(), remote, None),
        )
        .with_upgrades();
    drive(
        connection,
        |connection| connection.graceful_shutdown(),
        std::future::pending(),
        remote,
        connections,
    )
    .await;
}

async fn serve_http2<I>(io: I, remote: SocketAddr, connections: &Connections)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let mut builder = http2::Builder::new(TokioExecutor::new());
    builder
        .timer(TokioTimer::new())
        .keep_alive_interval(HTTP2_KEEP_ALIVE_INTERVAL)
        .keep_alive_timeout(HTTP2_KEEP_ALIVE_TIMEOUT);
    // HTTP/2 has no header-read timeout of its own: require the first request
    // to arrive in the time a request head may take.
    let asked = Arc::new(AtomicBool::new(false));
    let never_asked = {
        let asked = asked.clone();
        let patience = connections.timing.header_read_timeout;
        async move {
            tokio::time::sleep(patience).await;
            if asked.load(Ordering::Relaxed) {
                std::future::pending::<()>().await;
            }
        }
    };
    let connection = builder.serve_connection(
        io,
        request_service(connections.router.clone(), remote, Some(asked)),
    );
    drive(
        connection,
        |connection| connection.graceful_shutdown(),
        never_asked,
        remote,
        connections,
    )
    .await;
}

/// The router as a hyper service for one connection: every request learns
/// who sent it, and reports that it arrived.
fn request_service(
    router: Router,
    remote: SocketAddr,
    asked: Option<Arc<AtomicBool>>,
) -> impl hyper::service::Service<
    hyper::Request<Incoming>,
    Response = Response,
    Error = Infallible,
    Future: Send,
> + Clone {
    hyper::service::service_fn(move |mut request: hyper::Request<Incoming>| {
        if let Some(asked) = &asked {
            asked.store(true, Ordering::Relaxed);
        }
        request.extensions_mut().insert(ConnectInfo(remote));
        router.clone().oneshot(request)
    })
}

/// Run a connection until it ends, the server shuts down (the request in
/// progress may finish) or `give_up` resolves (the connection is dropped).
async fn drive<C>(
    connection: C,
    graceful_shutdown: fn(Pin<&mut C>),
    give_up: impl Future<Output = ()>,
    remote: SocketAddr,
    connections: &Connections,
) where
    C: Future<Output = Result<(), hyper::Error>>,
{
    tokio::pin!(connection);
    tokio::pin!(give_up);
    let result = tokio::select! {
        result = connection.as_mut() => result,
        _ = connections.shutdown.cancelled() => {
            graceful_shutdown(connection.as_mut());
            connection.await
        }
        _ = give_up => {
            tracing::debug!(%remote, "closing a connection that never sent a request");
            return;
        }
    };
    // Clients disconnect mid-request and time out all the time: not an operator's concern.
    if let Err(err) = result {
        tracing::debug!(%remote, %err, "connection ended with an error");
    }
}
