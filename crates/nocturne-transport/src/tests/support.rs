//! A scripted lab device and helpers shared by the in-process tests.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;

use crate::serve::SessionHandler;
use crate::{Backend, Connection, ConnectionInfo, ConnectionReader, Input, Output, TransportKind};

/// Generous bound for anything a test waits for.
pub(crate) const PATIENCE: Duration = Duration::from_secs(20);

/// A [`SessionHandler`] whose sessions are fake devices (see [`device`]).
#[derive(Default)]
pub(crate) struct Lab {
    /// Sessions opened so far.
    pub opened: AtomicUsize,
    /// Sessions currently running.
    pub active: Arc<AtomicUsize>,
    /// Refuse every session.
    pub closed_for_business: bool,
}

impl Lab {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Waits until no device session is left running.
    pub async fn wait_idle(&self) {
        let idle = async {
            while self.active.load(Ordering::Acquire) > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(PATIENCE, idle)
            .await
            .expect("device sessions should have ended");
    }
}

#[async_trait::async_trait]
impl SessionHandler for Lab {
    async fn open(
        &self,
        _peer: SocketAddr,
        user: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Option<Connection> {
        if self.closed_for_business {
            return None;
        }
        self.opened.fetch_add(1, Ordering::AcqRel);
        let info = ConnectionInfo {
            kind: Some(TransportKind::Sim),
            peer: "lab:device".to_owned(),
            ..ConnectionInfo::default()
        };
        let (connection, backend) = Connection::pair(info, Connection::DEFAULT_BUFFER);
        let running = Running::new(&self.active);
        let user = user.unwrap_or("nobody").to_owned();
        tokio::spawn(async move {
            device(backend, &user, cols, rows).await;
            drop(running);
        });
        Some(connection)
    }
}

/// Counts a running session for as long as it lives.
struct Running(Arc<AtomicUsize>);

impl Running {
    fn new(active: &Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(active))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The byte at position `index` of the `flood` pattern.
pub(crate) fn pattern(index: usize) -> u8 {
    b'a' + (index % 26) as u8
}

/// A fake device console.
///
/// It greets with `Welcome <user> <cols>x<rows>` and a `lab> ` prompt, echoes
/// every chunk it receives verbatim, and understands a few commands:
///
/// * `exit` — say `bye` and close the session;
/// * `flood <bytes> <chunk>` — print `bytes` bytes of [`pattern`] in chunks
///   of `chunk` bytes, then a new prompt;
/// * `size` — print the current window size;
/// * `raw` — print bytes that need escaping on some transports;
/// * `hang` — stop reading input and never print again.
///
/// Resizes and breaks are reported as `[resize <cols>x<rows>]` / `[break]`.
async fn device(mut backend: Backend, user: &str, mut cols: u16, mut rows: u16) {
    let output = backend.output.clone();
    let say = async |text: Vec<u8>| output.send(Output::Data(Bytes::from(text))).await.is_ok();
    if !say(format!("Welcome {user} {cols}x{rows}\r\nlab> ").into_bytes()).await {
        return;
    }
    let mut line = Vec::new();
    while let Some(input) = backend.input.recv().await {
        let data = match input {
            Input::Data(data) => data,
            Input::Resize { cols: c, rows: r } => {
                (cols, rows) = (c, r);
                if !say(format!("\r\n[resize {cols}x{rows}]\r\n").into_bytes()).await {
                    return;
                }
                continue;
            }
            Input::Break => {
                if !say(b"\r\n[break]\r\n".to_vec()).await {
                    return;
                }
                continue;
            }
            Input::Close => break,
        };
        if !say(data.to_vec()).await {
            return;
        }
        for &byte in &data[..] {
            if byte != b'\r' && byte != b'\n' {
                line.push(byte);
                continue;
            }
            let command = String::from_utf8_lossy(&std::mem::take(&mut line)).into_owned();
            let words: Vec<&str> = command.split_whitespace().collect();
            let reply: Vec<u8> = match words[..] {
                ["exit"] => {
                    let _ = say(b"\r\nbye\r\n".to_vec()).await;
                    let _ = output
                        .send(Output::Closed {
                            reason: Some("device exit".to_owned()),
                        })
                        .await;
                    return;
                }
                ["hang"] => std::future::pending().await,
                ["flood", total, chunk] => {
                    let total: usize = total.parse().unwrap_or(0);
                    let chunk: usize = chunk.parse().unwrap_or(1).max(1);
                    let mut sent = 0;
                    while sent < total {
                        let len = chunk.min(total - sent);
                        let piece: Vec<u8> = (sent..sent + len).map(pattern).collect();
                        if !say(piece).await {
                            return;
                        }
                        sent += len;
                    }
                    b"\r\nlab> ".to_vec()
                }
                ["size"] => format!("\r\n{cols}x{rows}\r\nlab> ").into_bytes(),
                ["raw"] => [&b"\r\n"[..], &[0xFF, 0x00, 0xFF, b'\r', b'x'], b"\r\nlab> "].concat(),
                _ => b"\r\nlab> ".to_vec(),
            };
            if !say(reply).await {
                return;
            }
        }
    }
    let _ = output.send(Output::Closed { reason: None }).await;
}

/// Something output can be read from.
pub(crate) trait Receive {
    async fn next(&mut self) -> Option<Output>;
}

impl Receive for Connection {
    async fn next(&mut self) -> Option<Output> {
        self.recv().await
    }
}

impl Receive for ConnectionReader {
    async fn next(&mut self) -> Option<Output> {
        self.recv().await
    }
}

/// A connection (or its reading half) together with the output that was
/// received but not asked for yet, so that tests can consume the byte stream
/// in exact pieces whatever the chunking was.
pub(crate) struct Tap<S> {
    pub inner: S,
    pending: Vec<u8>,
}

/// Wraps a connection for exact reading.
pub(crate) fn tap<S: Receive>(inner: S) -> Tap<S> {
    Tap {
        inner,
        pending: Vec::new(),
    }
}

impl<S> std::ops::Deref for Tap<S> {
    type Target = S;

    fn deref(&self) -> &S {
        &self.inner
    }
}

impl<S> std::ops::DerefMut for Tap<S> {
    fn deref_mut(&mut self) -> &mut S {
        &mut self.inner
    }
}

impl Tap<Connection> {
    pub async fn close(self) {
        soon(self.inner.close()).await;
    }
}

impl<S: Receive> Tap<S> {
    /// Reads more output; panics if the session ends or stays silent.
    async fn more(&mut self, waiting_for: &str) {
        match tokio::time::timeout(PATIENCE, self.inner.next()).await {
            Ok(Some(Output::Data(data))) => self.pending.extend_from_slice(&data),
            Ok(other) => panic!(
                "session ended ({other:?}) while waiting for {waiting_for}; got {}",
                preview(&self.pending)
            ),
            Err(_) => panic!(
                "timed out waiting for {waiting_for}; got {}",
                preview(&self.pending)
            ),
        }
    }

    /// Consumes the output up to and including the first `needle`.
    pub async fn until(&mut self, needle: &[u8]) -> Vec<u8> {
        loop {
            if let Some(start) = find(&self.pending, needle) {
                return self.pending.drain(..start + needle.len()).collect();
            }
            self.more(&format!("{:?}", String::from_utf8_lossy(needle)))
                .await;
        }
    }

    /// Consumes exactly `count` bytes of output.
    pub async fn exactly(&mut self, count: usize) -> Vec<u8> {
        while self.pending.len() < count {
            self.more(&format!("{count} bytes")).await;
        }
        self.pending.drain(..count).collect()
    }

    /// Consumes the rest of the session: its output and the close reason.
    /// Checks that nothing follows `Closed`.
    pub async fn until_closed(&mut self) -> (Vec<u8>, Option<String>) {
        loop {
            match tokio::time::timeout(PATIENCE, self.inner.next()).await {
                Ok(Some(Output::Data(data))) => self.pending.extend_from_slice(&data),
                Ok(Some(Output::Closed { reason })) => {
                    let after = tokio::time::timeout(PATIENCE, self.inner.next()).await;
                    assert!(
                        matches!(after, Ok(None)),
                        "nothing may follow Closed: {after:?}"
                    );
                    return (std::mem::take(&mut self.pending), reason);
                }
                Ok(None) => panic!("the stream ended without Closed"),
                Err(_) => panic!(
                    "timed out waiting for the session to close; got {}",
                    preview(&self.pending)
                ),
            }
        }
    }
}

/// The end of what was received, for failure messages.
fn preview(seen: &[u8]) -> String {
    const TAIL: usize = 300;
    let tail = &seen[seen.len().saturating_sub(TAIL)..];
    format!(
        "{} bytes ending in {:?}",
        seen.len(),
        String::from_utf8_lossy(tail)
    )
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len().max(1))
        .position(|window| window == needle)
}

pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

/// Bounds a future that should complete promptly.
pub(crate) async fn soon<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(PATIENCE, future)
        .await
        .expect("timed out")
}
