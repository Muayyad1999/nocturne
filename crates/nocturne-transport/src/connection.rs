//! The duplex byte stream every session runs over.
//!
//! A [`Connection`] is the application side; a [`Backend`] is the side a
//! transport drives. They are joined by two bounded channels, so a slow
//! consumer exerts back-pressure instead of growing memory without limit.
//!
//! # Lifecycle
//!
//! * The transport ends a session by sending [`Output::Closed`] and dropping
//!   its sender. [`Connection::recv`] yields that `Closed` item exactly once,
//!   as the last item, and `None` from then on — even if a backend vanishes
//!   without saying goodbye.
//! * The application ends a session with [`Connection::close`] (or
//!   [`Input::Close`] through a writer). Dropping the reading half is the
//!   hard stop: the backend sees its `output` channel close and tears down.
//! * Dropping every writer only means "no more input"; the transport keeps
//!   delivering output until one of the above happens.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::error::TransportError;

/// How long [`Connection::close`] waits for room to queue its `Close` request.
const CLOSE_REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
/// How long [`Connection::close`] waits for the transport to wind down.
const CLOSE_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// What the application sends to a transport.
#[derive(Debug)]
pub enum Input {
    /// Bytes for the remote end.
    Data(Bytes),
    /// The terminal was resized.
    Resize { cols: u16, rows: u16 },
    /// Send a break condition (serial line break, Telnet `BRK`).
    Break,
    /// Close the session.
    Close,
}

/// What a transport delivers to the application.
#[derive(Debug, Clone)]
pub enum Output {
    /// Bytes from the remote end.
    Data(Bytes),
    /// The session is over. Always the last item.
    Closed {
        /// Why it ended, when there is something worth telling (exit status,
        /// disconnect message, I/O error). `None` for a locally requested close.
        reason: Option<String>,
    },
}

/// Which transport a connection runs over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    Ssh,
    Telnet,
    Serial,
    Local,
    Sim,
}

/// Facts about an established connection.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ConnectionInfo {
    pub kind: Option<TransportKind>,
    /// `"10.0.0.1:22"`, `"COM3"`, `"lab:R1"`.
    pub peer: String,
    /// For SSH: the server identification string, followed — after a newline —
    /// by the pre-authentication banner when the server sent one.
    pub server_banner: Option<String>,
    pub kex: Option<String>,
    pub cipher: Option<String>,
    /// `None` when the cipher authenticates itself (AEAD).
    pub mac: Option<String>,
    pub host_key_algorithm: Option<String>,
    /// `"SHA256:…"`.
    pub host_key_fingerprint: Option<String>,
    /// The session was negotiated only thanks to the legacy algorithm set.
    pub legacy_crypto: bool,
    /// Jump hosts traversed, first hop first.
    pub via: Vec<String>,
}

/// Capabilities of a connection that are fixed when it is created.
#[derive(Debug, Default)]
pub(crate) struct Features {
    /// Set when the transport cannot deliver [`Input::Break`]; holds the reason.
    pub break_unsupported: Option<&'static str>,
}

#[derive(Debug)]
struct Shared {
    info: ConnectionInfo,
    features: Features,
}

/// The application side of a session: a duplex byte stream with resize and
/// break side-channels.
#[derive(Debug)]
pub struct Connection {
    writer: ConnectionWriter,
    reader: ConnectionReader,
}

/// The sending half of a [`Connection`]. Cheap to clone.
#[derive(Clone, Debug)]
pub struct ConnectionWriter {
    pub(crate) input: mpsc::Sender<Input>,
    shared: Arc<Shared>,
}

/// The receiving half of a [`Connection`].
#[derive(Debug)]
pub struct ConnectionReader {
    output: mpsc::Receiver<Output>,
    shared: Arc<Shared>,
    finished: bool,
}

/// The transport side of a [`Connection`].
#[derive(Debug)]
pub struct Backend {
    /// What the application sends. `None` means every writer is gone.
    pub input: mpsc::Receiver<Input>,
    /// Where output goes. Closed once the application dropped its reader.
    pub output: mpsc::Sender<Output>,
}

impl Connection {
    /// A channel capacity that suits interactive sessions.
    pub const DEFAULT_BUFFER: usize = 64;

    /// Build a connection and the backend half a custom transport drives.
    ///
    /// `buffer` is the capacity, in items, of each direction.
    pub fn pair(info: ConnectionInfo, buffer: usize) -> (Connection, Backend) {
        Self::pair_with(info, buffer, Features::default())
    }

    pub(crate) fn pair_with(
        info: ConnectionInfo,
        buffer: usize,
        features: Features,
    ) -> (Connection, Backend) {
        let buffer = buffer.max(1);
        let (input_tx, input_rx) = mpsc::channel(buffer);
        let (output_tx, output_rx) = mpsc::channel(buffer);
        let shared = Arc::new(Shared { info, features });
        let connection = Connection {
            writer: ConnectionWriter {
                input: input_tx,
                shared: Arc::clone(&shared),
            },
            reader: ConnectionReader {
                output: output_rx,
                shared,
                finished: false,
            },
        };
        let backend = Backend {
            input: input_rx,
            output: output_tx,
        };
        (connection, backend)
    }

    pub fn info(&self) -> &ConnectionInfo {
        self.reader.info()
    }

    /// Queue bytes for the remote end, waiting for room if the transport is
    /// busy. Fails with [`TransportError::Closed`] once the session is over.
    pub async fn send(&self, data: impl Into<Bytes>) -> Result<(), TransportError> {
        self.writer.send(data).await
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), TransportError> {
        self.writer.resize(cols, rows).await
    }

    /// Ask the transport to send a break condition. Fails with
    /// [`TransportError::Unsupported`] where the transport has no such notion
    /// (see [`supports_break`](Self::supports_break)).
    pub async fn send_break(&self) -> Result<(), TransportError> {
        self.writer.send_break().await
    }

    /// Whether [`send_break`](Self::send_break) can work on this connection.
    pub fn supports_break(&self) -> bool {
        self.writer.supports_break()
    }

    /// Next chunk of output. `None` once the connection is closed and drained;
    /// the last item before `None` is `Output::Closed`.
    pub async fn recv(&mut self) -> Option<Output> {
        self.reader.recv().await
    }

    /// Like [`recv`](Self::recv) but never waits: `None` means "nothing
    /// available right now" (or the connection is finished, see
    /// [`is_closed`](Self::is_closed)).
    pub fn try_recv(&mut self) -> Option<Output> {
        self.reader.try_recv()
    }

    /// Whether `Output::Closed` has already been returned.
    pub fn is_closed(&self) -> bool {
        self.reader.is_closed()
    }

    /// A handle for sending from other tasks while this one keeps receiving.
    pub fn writer(&self) -> ConnectionWriter {
        self.writer.clone()
    }

    pub fn split(self) -> (ConnectionWriter, ConnectionReader) {
        (self.writer, self.reader)
    }

    /// Close the session and wait (a bounded time) for the transport to
    /// release its resources — the socket, the serial port, the child process.
    /// Unread output is discarded.
    pub async fn close(self) {
        let Connection { writer, mut reader } = self;
        let asked = writer.request_close().await;
        drop(writer);
        if asked {
            let drained = async { while reader.recv().await.is_some() {} };
            if tokio::time::timeout(CLOSE_DRAIN_TIMEOUT, drained)
                .await
                .is_err()
            {
                tracing::debug!(peer = %reader.info().peer, "transport did not wind down in time");
            }
        }
        // Dropping the reader is the hard stop for whatever is left.
    }
}

impl ConnectionWriter {
    pub fn info(&self) -> &ConnectionInfo {
        &self.shared.info
    }

    /// See [`Connection::send`].
    pub async fn send(&self, data: impl Into<Bytes>) -> Result<(), TransportError> {
        let data = data.into();
        if data.is_empty() {
            return self.ensure_open();
        }
        self.submit(Input::Data(data)).await
    }

    pub async fn resize(&self, cols: u16, rows: u16) -> Result<(), TransportError> {
        self.submit(Input::Resize { cols, rows }).await
    }

    /// See [`Connection::send_break`].
    pub async fn send_break(&self) -> Result<(), TransportError> {
        if let Some(reason) = self.shared.features.break_unsupported {
            return Err(TransportError::Unsupported(reason.to_owned()));
        }
        self.submit(Input::Break).await
    }

    /// See [`Connection::supports_break`].
    pub fn supports_break(&self) -> bool {
        self.shared.features.break_unsupported.is_none()
    }

    /// Ask the transport to close the session. The reading half then receives
    /// the remaining output followed by `Output::Closed`. Closing an already
    /// closed connection is a no-op.
    pub async fn close(&self) {
        self.request_close().await;
    }

    /// Whether the transport has stopped accepting input.
    pub fn is_closed(&self) -> bool {
        self.input.is_closed()
    }

    /// Queues `Input::Close`; `false` if the transport is gone or too
    /// congested to take the request in time.
    async fn request_close(&self) -> bool {
        matches!(
            tokio::time::timeout(CLOSE_REQUEST_TIMEOUT, self.input.send(Input::Close)).await,
            Ok(Ok(()))
        )
    }

    async fn submit(&self, input: Input) -> Result<(), TransportError> {
        self.input
            .send(input)
            .await
            .map_err(|_| TransportError::Closed)
    }

    fn ensure_open(&self) -> Result<(), TransportError> {
        if self.input.is_closed() {
            Err(TransportError::Closed)
        } else {
            Ok(())
        }
    }
}

impl ConnectionReader {
    pub fn info(&self) -> &ConnectionInfo {
        &self.shared.info
    }

    /// See [`Connection::recv`].
    pub async fn recv(&mut self) -> Option<Output> {
        if self.finished {
            return None;
        }
        let item = self.output.recv().await;
        Some(self.track(item))
    }

    /// See [`Connection::try_recv`].
    pub fn try_recv(&mut self) -> Option<Output> {
        if self.finished {
            return None;
        }
        match self.output.try_recv() {
            Ok(item) => Some(self.track(Some(item))),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => Some(self.track(None)),
        }
    }

    /// See [`Connection::is_closed`].
    pub fn is_closed(&self) -> bool {
        self.finished
    }

    /// Enforces the "exactly one `Closed`, and it is last" contract whatever
    /// the backend does.
    fn track(&mut self, item: Option<Output>) -> Output {
        match item {
            Some(data @ Output::Data(_)) => data,
            Some(closed @ Output::Closed { .. }) => {
                self.finish();
                closed
            }
            // The backend went away without announcing it.
            None => {
                self.finish();
                Output::Closed { reason: None }
            }
        }
    }

    fn finish(&mut self) {
        self.finished = true;
        // Lets a backend that is still producing notice nobody listens.
        self.output.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(buffer: usize) -> (Connection, Backend) {
        let info = ConnectionInfo {
            kind: Some(TransportKind::Sim),
            peer: "lab:R1".into(),
            ..ConnectionInfo::default()
        };
        Connection::pair(info, buffer)
    }

    #[tokio::test]
    async fn data_flows_both_ways() {
        let (mut conn, mut backend) = pair(8);
        assert_eq!(conn.info().peer, "lab:R1");
        assert_eq!(conn.info().kind, Some(TransportKind::Sim));

        conn.send("show version\r").await.unwrap();
        conn.resize(132, 50).await.unwrap();
        conn.send_break().await.unwrap();
        assert!(
            matches!(backend.input.recv().await, Some(Input::Data(d)) if &d[..] == b"show version\r")
        );
        assert!(matches!(
            backend.input.recv().await,
            Some(Input::Resize {
                cols: 132,
                rows: 50
            })
        ));
        assert!(matches!(backend.input.recv().await, Some(Input::Break)));

        backend
            .output
            .send(Output::Data(Bytes::from_static(b"IOS 15.2")))
            .await
            .unwrap();
        assert!(matches!(conn.recv().await, Some(Output::Data(d)) if &d[..] == b"IOS 15.2"));
    }

    #[tokio::test]
    async fn closed_is_last_and_then_none() {
        let (mut conn, backend) = pair(8);
        backend
            .output
            .send(Output::Data(Bytes::from_static(b"bye")))
            .await
            .unwrap();
        backend
            .output
            .send(Output::Closed {
                reason: Some("exit status 0".into()),
            })
            .await
            .unwrap();
        // Anything after `Closed` must never surface.
        backend
            .output
            .send(Output::Data(Bytes::from_static(b"late")))
            .await
            .unwrap();

        assert!(matches!(conn.recv().await, Some(Output::Data(_))));
        assert!(!conn.is_closed());
        assert!(
            matches!(conn.recv().await, Some(Output::Closed { reason: Some(r) }) if r == "exit status 0")
        );
        assert!(conn.is_closed());
        assert!(conn.recv().await.is_none());
        assert!(conn.recv().await.is_none());
        assert!(conn.try_recv().is_none());
        // The backend can tell that nobody listens any more.
        assert!(backend.output.is_closed());
    }

    #[tokio::test]
    async fn vanished_backend_still_yields_closed() {
        let (mut conn, backend) = pair(8);
        drop(backend);
        assert!(matches!(
            conn.recv().await,
            Some(Output::Closed { reason: None })
        ));
        assert!(conn.recv().await.is_none());
        assert!(matches!(conn.send("x").await, Err(TransportError::Closed)));
        assert!(matches!(
            conn.resize(1, 1).await,
            Err(TransportError::Closed)
        ));
    }

    #[tokio::test]
    async fn try_recv_distinguishes_empty_from_closed() {
        let (mut conn, backend) = pair(8);
        assert!(conn.try_recv().is_none());
        assert!(!conn.is_closed());
        backend
            .output
            .send(Output::Data(Bytes::from_static(b"a")))
            .await
            .unwrap();
        assert!(matches!(conn.try_recv(), Some(Output::Data(_))));
        drop(backend);
        assert!(matches!(conn.try_recv(), Some(Output::Closed { .. })));
        assert!(conn.is_closed());
        assert!(conn.try_recv().is_none());
    }

    #[tokio::test]
    async fn send_applies_back_pressure() {
        let (conn, mut backend) = pair(2);
        conn.send("1").await.unwrap();
        conn.send("2").await.unwrap();
        let blocked = tokio::time::timeout(Duration::from_millis(50), conn.send("3")).await;
        assert!(blocked.is_err(), "a full queue must make send wait");
        assert!(matches!(backend.input.recv().await, Some(Input::Data(_))));
        conn.send("3").await.unwrap();
    }

    #[tokio::test]
    async fn empty_sends_are_not_forwarded() {
        let (conn, mut backend) = pair(2);
        conn.send(Bytes::new()).await.unwrap();
        conn.send("x").await.unwrap();
        assert!(matches!(backend.input.recv().await, Some(Input::Data(d)) if &d[..] == b"x"));
    }

    #[tokio::test]
    async fn split_halves_work_from_different_tasks() {
        let (conn, mut backend) = pair(8);
        let (writer, mut reader) = conn.split();
        assert_eq!(writer.info().peer, "lab:R1");
        assert_eq!(reader.info().peer, "lab:R1");

        let echo = tokio::spawn(async move {
            while let Some(input) = backend.input.recv().await {
                match input {
                    Input::Data(data) => backend.output.send(Output::Data(data)).await.unwrap(),
                    Input::Close => break,
                    _ => {}
                }
            }
            backend
                .output
                .send(Output::Closed { reason: None })
                .await
                .unwrap();
        });

        let second = writer.clone();
        tokio::spawn(async move { second.send("ping").await.unwrap() });
        assert!(matches!(reader.recv().await, Some(Output::Data(d)) if &d[..] == b"ping"));
        writer.close().await;
        assert!(matches!(reader.recv().await, Some(Output::Closed { .. })));
        assert!(reader.recv().await.is_none());
        echo.await.unwrap();
        assert!(writer.is_closed());
    }

    #[tokio::test]
    async fn close_waits_for_the_backend_to_finish() {
        let (conn, mut backend) = pair(8);
        let (released_tx, released_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            backend
                .output
                .send(Output::Data(Bytes::from_static(b"unread")))
                .await
                .unwrap();
            while let Some(input) = backend.input.recv().await {
                if matches!(input, Input::Close) {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            released_tx.send(()).unwrap();
            let _ = backend.output.send(Output::Closed { reason: None }).await;
        });
        conn.close().await;
        // `close` returned only after the backend released its resources.
        assert!(released_rx.await.is_ok());
    }

    #[tokio::test]
    async fn close_does_not_hang_on_a_stuck_backend() {
        let (conn, backend) = pair(1);
        // Never serviced: the input queue stays full.
        conn.send("x").await.unwrap();
        let started = std::time::Instant::now();
        conn.close().await;
        assert!(started.elapsed() < Duration::from_secs(4));
        drop(backend);
    }

    #[tokio::test]
    async fn dropping_the_reader_signals_the_backend() {
        let (conn, backend) = pair(1);
        let (writer, reader) = conn.split();
        drop(reader);
        tokio::time::timeout(Duration::from_secs(1), backend.output.closed())
            .await
            .expect("backend must observe the reader going away");
        // The writing half is still usable until the backend drops its end.
        writer.send("x").await.unwrap();
    }

    #[tokio::test]
    async fn break_can_be_declared_unsupported() {
        let (conn, mut backend) = Connection::pair_with(
            ConnectionInfo::default(),
            4,
            Features {
                break_unsupported: Some("no break here"),
            },
        );
        assert!(!conn.supports_break());
        let error = conn.send_break().await.unwrap_err();
        assert_eq!(error.code(), "unsupported");
        assert!(error.to_string().contains("no break here"));
        assert!(backend.input.try_recv().is_err());

        let (plain, _backend) = pair(1);
        assert!(plain.supports_break());
    }

    #[test]
    fn info_round_trips_through_json() {
        let info: ConnectionInfo =
            serde_json::from_str(r#"{"kind":"telnet","peer":"10.0.0.1:23"}"#).unwrap();
        assert_eq!(info.kind, Some(TransportKind::Telnet));
        assert!(!info.legacy_crypto);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["kind"], "telnet");
        assert_eq!(json["via"], serde_json::json!([]));
    }

    #[test]
    fn zero_buffer_is_clamped() {
        let (_conn, _backend) = pair(0);
    }
}
