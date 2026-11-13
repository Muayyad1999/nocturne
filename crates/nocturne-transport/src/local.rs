//! Local shell in a pseudo-terminal (ConPTY on Windows).
//!
//! `portable-pty` hands out blocking readers and writers, so each connection
//! has a reading and a writing thread that talk to an async pump through
//! channels; the pump also watches for the child to exit.

use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use bytes::Bytes;
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::connection::{
    Backend, Connection, ConnectionInfo, Features, Input, Output, TransportKind,
};
use crate::error::TransportError;
use crate::pump::{Outcome, ReadEnd, WriteEnd, drive};
use crate::relay::Relay;

const READ_BUFFER: usize = 16 * 1024;
/// Unread output staged before the reading thread is made to wait.
const OUTPUT_HIGH_WATER: usize = 1024 * 1024;
/// How often the child is checked for having exited.
const EXIT_POLL: Duration = Duration::from_millis(100);
/// After the child exited, how long its last output may take to arrive.
const DRAIN_AFTER_EXIT: Duration = Duration::from_millis(300);
/// After the terminal reported end-of-file, how long the exit status may take.
const STATUS_AFTER_EOF: Duration = Duration::from_millis(500);
/// How long stopping the child may take when closing.
const KILL_TIMEOUT: Duration = Duration::from_secs(3);
/// What the terminal on the other end of a `Connection` emulates.
const TERM: &str = "xterm-256color";

/// What to run in the local pseudo-terminal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalParams {
    /// `$SHELL` (or `/bin/bash`) on Unix and `powershell.exe` on Windows
    /// when `None`.
    pub program: Option<String>,
    pub args: Vec<String>,
    /// The user's home directory when `None`.
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    /// Added to (or overriding) the environment of this process.
    pub env: Vec<(String, String)>,
}

impl Default for LocalParams {
    fn default() -> Self {
        Self {
            program: None,
            args: Vec::new(),
            cwd: None,
            cols: 120,
            rows: 40,
            env: Vec::new(),
        }
    }
}

/// The user's own shell in a PTY (ConPTY on Windows).
///
/// The session ends with `Closed { reason: Some("exit status N") }` when the
/// program exits; closing the connection hangs up on it. A local terminal has
/// no break signal ([`Connection::supports_break`] is `false`).
pub async fn connect_local(params: &LocalParams) -> Result<Connection, TransportError> {
    let params = params.clone();
    let program = params.program.clone().unwrap_or_else(default_program);
    let spawning = tokio::task::spawn_blocking(move || Terminal::spawn(&params));
    let terminal = spawning
        .await
        .map_err(|error| TransportError::Io(io::Error::other(error)))??;

    let info = ConnectionInfo {
        kind: Some(TransportKind::Local),
        peer: program,
        ..ConnectionInfo::default()
    };
    let features = Features {
        break_unsupported: Some("a local shell has no break signal"),
    };
    let (connection, backend) = Connection::pair_with(info, Connection::DEFAULT_BUFFER, features);
    tokio::spawn(run(terminal, backend));
    Ok(connection)
}

#[cfg(unix)]
fn default_program() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| {
            let bash = "/bin/bash";
            if Path::new(bash).exists() {
                bash
            } else {
                "/bin/sh"
            }
            .to_owned()
        })
}

#[cfg(not(unix))]
fn default_program() -> String {
    "powershell.exe".to_owned()
}

/// A program running in a pseudo-terminal.
struct Terminal {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
}

impl Terminal {
    fn spawn(params: &LocalParams) -> Result<Self, TransportError> {
        if let Some(cwd) = &params.cwd
            && !Path::new(cwd).is_dir()
        {
            // The PTY layer would silently fall back to the home directory.
            return Err(TransportError::Io(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{cwd} is not a directory"),
            )));
        }
        let program = params.program.clone().unwrap_or_else(default_program);
        let mut command = CommandBuilder::new(&program);
        command.args(&params.args);
        if let Some(cwd) = &params.cwd {
            command.cwd(cwd);
        }
        command.env("TERM", TERM);
        for (name, value) in &params.env {
            command.env(name, value);
        }

        let pty = native_pty_system()
            .openpty(PtySize {
                rows: params.rows.max(1),
                cols: params.cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|error| pty_error("cannot open a pseudo-terminal", &format!("{error:#}")))?;
        let child = pty.slave.spawn_command(command).map_err(|error| {
            pty_error(&format!("cannot start {program}"), &format!("{error:#}"))
        })?;
        // Only the child may hold the other end, or its exit would never
        // show up as end-of-file.
        drop(pty.slave);
        let reader = pty.master.try_clone_reader().map_err(|error| {
            pty_error(
                "cannot read from the pseudo-terminal",
                &format!("{error:#}"),
            )
        })?;
        let writer = pty.master.take_writer().map_err(|error| {
            pty_error("cannot write to the pseudo-terminal", &format!("{error:#}"))
        })?;
        Ok(Self {
            master: pty.master,
            child,
            reader,
            writer,
        })
    }
}

fn pty_error(what: &str, detail: &str) -> TransportError {
    let kind = if detail.contains("doesn't exist")
        || detail.contains("does not exist")
        || detail.contains("No viable candidates")
        || detail.contains("cannot find")
    {
        io::ErrorKind::NotFound
    } else {
        io::ErrorKind::Other
    };
    TransportError::Io(io::Error::new(kind, format!("{what}: {detail}")))
}

/// The controlling end of the terminal, shared by the half that resizes it
/// and the half that closes it when the child is gone.
type Master = Mutex<Option<Box<dyn MasterPty + Send>>>;

async fn run(terminal: Terminal, backend: Backend) {
    let Terminal {
        master,
        mut child,
        reader,
        writer,
    } = terminal;
    let Backend { mut input, output } = backend;
    let mut relay = Relay::for_output(output.clone(), OUTPUT_HIGH_WATER);
    let master: Master = Mutex::new(Some(master));
    let (screen_tx, mut screen) = mpsc::channel::<Bytes>(Connection::DEFAULT_BUFFER);
    let (keyboard, keyboard_rx) = mpsc::channel::<Bytes>(Connection::DEFAULT_BUFFER);

    let threads = [
        spawn_thread("nocturne-pty-rx", move || read_terminal(reader, &screen_tx)),
        spawn_thread("nocturne-pty-tx", move || {
            write_terminal(writer, keyboard_rx)
        }),
    ];
    let mut exited = false;
    let outcome = if threads.iter().all(Result::is_ok) {
        drive(
            read_screen(
                &mut screen,
                &mut relay,
                &output,
                child.as_mut(),
                &master,
                &mut exited,
            ),
            write_keyboard(&mut input, &keyboard, &master),
            Duration::ZERO,
        )
        .await
    } else {
        Outcome::Closed(Some("cannot start the terminal threads".to_owned()))
    };

    // Hang up before announcing the end. The threads end by themselves once
    // their channels and the terminal are gone.
    drop((keyboard, screen, input, output));
    drop(master);
    if !exited {
        let stopping = tokio::task::spawn_blocking(move || {
            let _ = child.kill();
            let _ = child.wait();
        });
        let _ = tokio::time::timeout(KILL_TIMEOUT, stopping).await;
    }
    if let Outcome::Closed(reason) = outcome {
        relay.finish(reason).await;
    }
}

fn spawn_thread(
    name: &str,
    work: impl FnOnce() + Send + 'static,
) -> io::Result<thread::JoinHandle<()>> {
    thread::Builder::new().name(name.to_owned()).spawn(work)
}

/// Reading thread: terminal → pump. Ends at end-of-file, on error, or when
/// the pump is gone.
fn read_terminal(mut reader: Box<dyn Read + Send>, screen: &mpsc::Sender<Bytes>) {
    let mut buffer = vec![0u8; READ_BUFFER];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => return,
            Ok(count) => {
                if screen
                    .blocking_send(Bytes::copy_from_slice(&buffer[..count]))
                    .is_err()
                {
                    return;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            // A closed terminal reads as an error on some platforms.
            Err(_) => return,
        }
    }
}

/// Writing thread: pump → terminal.
fn write_terminal(mut writer: Box<dyn Write + Send>, mut keyboard: mpsc::Receiver<Bytes>) {
    while let Some(data) = keyboard.blocking_recv() {
        if writer
            .write_all(&data)
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
    }
}

/// Terminal → application; also notices the child exiting.
async fn read_screen(
    screen: &mut mpsc::Receiver<Bytes>,
    relay: &mut Relay<Output>,
    output: &mpsc::Sender<Output>,
    child: &mut (dyn Child + Send + Sync),
    master: &Master,
    exited: &mut bool,
) -> ReadEnd {
    let mut exit_poll = tokio::time::interval(EXIT_POLL);
    let mut exit: Option<String> = None;
    // Set once the child is known to be gone: its last output gets until
    // then to arrive.
    let mut deadline: Option<Instant> = None;
    loop {
        tokio::select! {
            biased;
            forwarded = relay.forward_one(), if !relay.is_empty() => {
                if forwarded.is_err() {
                    return ReadEnd::Abandoned;
                }
            }
            () = output.closed() => return ReadEnd::Abandoned,
            chunk = screen.recv(), if relay.has_room() => match chunk {
                Some(data) => relay.push_data(data),
                None => {
                    if exit.is_none() {
                        exit = wait_for_exit(child, exited).await;
                    }
                    let reason = exit.unwrap_or_else(|| "the terminal was closed".to_owned());
                    return ReadEnd::Finished(Some(reason));
                }
            },
            _ = exit_poll.tick(), if !*exited => {
                if let Ok(Some(status)) = child.try_wait() {
                    *exited = true;
                    exit = Some(describe_exit(&status));
                    deadline = Some(Instant::now() + DRAIN_AFTER_EXIT);
                    // On Windows the output pipe only ends once the
                    // pseudo-console is closed.
                    close_master(master);
                }
            }
            () = wait_until(deadline), if deadline.is_some() => {
                return ReadEnd::Finished(exit);
            }
        }
    }
}

/// The exit status of a child whose terminal just reported end-of-file.
async fn wait_for_exit(child: &mut (dyn Child + Send + Sync), exited: &mut bool) -> Option<String> {
    let deadline = Instant::now() + STATUS_AFTER_EOF;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            *exited = true;
            return Some(describe_exit(&status));
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn close_master(master: &Master) {
    master.lock().unwrap_or_else(PoisonError::into_inner).take();
}

fn describe_exit(status: &ExitStatus) -> String {
    match status.signal() {
        Some(signal) => format!("terminated by signal: {signal}"),
        None => format!("exit status {}", status.exit_code()),
    }
}

/// Application → terminal.
async fn write_keyboard(
    input: &mut mpsc::Receiver<Input>,
    keyboard: &mpsc::Sender<Bytes>,
    master: &Master,
) -> WriteEnd {
    while let Some(item) = input.recv().await {
        match item {
            Input::Data(data) => {
                if keyboard.send(data).await.is_err() {
                    return WriteEnd::Failed("the terminal was closed".to_owned());
                }
            }
            Input::Resize { cols, rows } => {
                let size = PtySize {
                    rows: rows.max(1),
                    cols: cols.max(1),
                    pixel_width: 0,
                    pixel_height: 0,
                };
                let master = master.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(master) = master.as_ref()
                    && let Err(error) = master.resize(size)
                {
                    tracing::debug!(%error, "cannot resize the pseudo-terminal");
                }
            }
            Input::Break => {}
            Input::Close => return WriteEnd::CloseRequested,
        }
    }
    WriteEnd::InputClosed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_default_and_partial_json() {
        let params: LocalParams = serde_json::from_str(r#"{"program":"/bin/sh"}"#).unwrap();
        assert_eq!(params.program.as_deref(), Some("/bin/sh"));
        assert_eq!((params.cols, params.rows), (120, 40));
        assert!(params.args.is_empty() && params.env.is_empty() && params.cwd.is_none());

        let params: LocalParams =
            serde_json::from_str(r#"{"env":[["A","1"]],"args":["-c","true"]}"#).unwrap();
        assert_eq!(params.env, vec![("A".to_owned(), "1".to_owned())]);
        assert_eq!(params.args, ["-c", "true"]);
    }

    #[test]
    fn there_is_always_a_default_program() {
        assert!(!default_program().is_empty());
    }
}
