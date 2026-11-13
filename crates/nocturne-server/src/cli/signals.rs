//! The operating system's ways of asking the server to stop.

use std::io;

/// Listeners for stop requests. Created before the server announces itself,
/// so that a request arriving right after the announcement is already handled.
pub(super) struct Signals {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
    #[cfg(windows)]
    ctrl_c: tokio::signal::windows::CtrlC,
    #[cfg(windows)]
    ctrl_break: tokio::signal::windows::CtrlBreak,
    #[cfg(windows)]
    ctrl_close: tokio::signal::windows::CtrlClose,
    #[cfg(windows)]
    ctrl_shutdown: tokio::signal::windows::CtrlShutdown,
}

impl Signals {
    // SIGHUP is deliberately left alone: handling it would make a server
    // started with `nohup` stop when its terminal closes.
    #[cfg(unix)]
    pub(super) fn install() -> io::Result<Signals> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Signals {
            terminate: signal(SignalKind::terminate())?,
            interrupt: signal(SignalKind::interrupt())?,
        })
    }

    #[cfg(windows)]
    pub(super) fn install() -> io::Result<Signals> {
        use tokio::signal::windows;
        Ok(Signals {
            ctrl_c: windows::ctrl_c()?,
            ctrl_break: windows::ctrl_break()?,
            ctrl_close: windows::ctrl_close()?,
            ctrl_shutdown: windows::ctrl_shutdown()?,
        })
    }

    #[cfg(not(any(unix, windows)))]
    pub(super) fn install() -> io::Result<Signals> {
        Ok(Signals {})
    }

    /// Wait for the next stop request and name it.
    #[cfg(unix)]
    pub(super) async fn wait(&mut self) -> &'static str {
        tokio::select! {
            _ = self.terminate.recv() => "SIGTERM",
            _ = self.interrupt.recv() => "SIGINT",
        }
    }

    #[cfg(windows)]
    pub(super) async fn wait(&mut self) -> &'static str {
        tokio::select! {
            _ = self.ctrl_c.recv() => "Ctrl-C",
            _ = self.ctrl_break.recv() => "Ctrl-Break",
            _ = self.ctrl_close.recv() => "console closed",
            _ = self.ctrl_shutdown.recv() => "system shutdown",
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub(super) async fn wait(&mut self) -> &'static str {
        match tokio::signal::ctrl_c().await {
            Ok(()) => "Ctrl-C",
            // No way to be told to stop: run until killed.
            Err(_) => std::future::pending().await,
        }
    }
}
