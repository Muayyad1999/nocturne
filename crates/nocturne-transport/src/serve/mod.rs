//! Small SSH and Telnet servers that expose in-process sessions: lab devices
//! reachable from an ordinary terminal, and the far end of this crate's tests.
//!
//! Each accepted session is bridged to the [`Connection`](crate::Connection)
//! a [`SessionHandler`] returns for it.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::{JoinHandle, JoinSet};

mod ssh;
mod telnet;

pub use ssh::{SshServerConfig, serve_ssh};
pub use telnet::serve_telnet;

#[cfg(test)]
pub(crate) use ssh::{SshServerOptions, serve_ssh_with};

/// How long sessions get to hang up after a shutdown was requested.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);
/// Pause after a failed `accept` (typically: out of file descriptors).
const ACCEPT_BACKOFF: Duration = Duration::from_millis(100);

/// Called for every accepted session; return the connection to bridge it to, or None to refuse.
#[async_trait::async_trait]
pub trait SessionHandler: Send + Sync {
    /// `user` is the authenticated user name (SSH only); `cols` × `rows` is
    /// the size of the client's terminal as far as it is known.
    async fn open(
        &self,
        peer: SocketAddr,
        user: Option<&str>,
        cols: u16,
        rows: u16,
    ) -> Option<super::Connection>;
}

/// A running server. Stops when [`shutdown`](Self::shutdown) is called or the
/// handle is dropped.
#[derive(Debug)]
pub struct ServerHandle {
    local_addr: SocketAddr,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// The address the server listens on (useful after binding port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Stops accepting, disconnects the open sessions and waits for them to
    /// end.
    pub async fn shutdown(mut self) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

/// Tells a session that the server is shutting down.
#[derive(Clone)]
pub(crate) struct ShutdownSignal(watch::Receiver<bool>);

impl ShutdownSignal {
    /// Resolves once shutdown was requested (or the server handle is gone).
    pub async fn requested(&mut self) {
        let _ = self.0.wait_for(|stop| *stop).await;
    }
}

/// Accepts connections on `listener`, running `serve` for each in its own
/// task, until the returned handle says stop.
pub(crate) fn spawn_server<F, Fut>(listener: TcpListener, local_addr: SocketAddr, serve: F) -> ServerHandle
where
    F: Fn(TcpStream, SocketAddr, ShutdownSignal) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let (stop, stopped) = watch::channel(false);
    let task = tokio::spawn(accept_loop(listener, ShutdownSignal(stopped), serve));
    ServerHandle {
        local_addr,
        stop,
        task: Some(task),
    }
}

async fn accept_loop<F, Fut>(listener: TcpListener, mut shutdown: ShutdownSignal, serve: F)
where
    F: Fn(TcpStream, SocketAddr, ShutdownSignal) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let mut sessions = JoinSet::new();
    loop {
        tokio::select! {
            () = shutdown.requested() => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, peer)) => {
                    let _ = stream.set_nodelay(true);
                    sessions.spawn(serve(stream, peer, shutdown.clone()));
                }
                Err(error) => {
                    tracing::warn!(%error, "cannot accept a connection");
                    tokio::time::sleep(ACCEPT_BACKOFF).await;
                }
            },
            Some(_) = sessions.join_next(), if !sessions.is_empty() => {}
        }
    }
    drop(listener);

    // Sessions watch the same signal and hang up by themselves.
    let drained = async { while sessions.join_next().await.is_some() {} };
    if tokio::time::timeout(SHUTDOWN_GRACE, drained).await.is_err() {
        sessions.shutdown().await;
    }
}
