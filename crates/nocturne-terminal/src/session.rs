//! Live sessions: the registry, and the task that pumps each connection.
//!
//! A session owns its connection. One task per session reads whatever the far
//! end prints, keeps the newest part in the scrollback ring, and announces it
//! as `terminal.output` events — coalesced, so that a `show tech` is a few
//! hundred events instead of ten thousand. The task never waits for a UI:
//! events go to the application's broadcast bus, and a UI that fell behind
//! re-attaches and gets the ring. That makes the ring the bound on memory and
//! keeps one flooding session from holding up the others.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::Bytes;
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppError, AppResult};
use nocturne_transport::{
    Connection, ConnectionInfo, ConnectionReader, ConnectionWriter, Output, TransportKind,
};
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::watch;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

use crate::autologin::{self, AutoLogin};
use crate::logs;
use crate::model::{
    Attached, ClosedEvent, OutputEvent, RemovedEvent, SessionInfo, SessionState, kind_name,
};
use crate::pastemode::PasteMode;
use crate::ring::Ring;
use crate::sessionlog::{SessionLog, Subject};
use crate::settings::TerminalSettings;
use crate::text;

/// Sessions the backend holds at once, closed ones awaiting dismissal included.
pub(crate) const MAX_SESSIONS: usize = 64;
/// Output is announced at most this often …
const FLUSH_INTERVAL: Duration = Duration::from_millis(8);
/// … unless this much has piled up. Also the largest `terminal.output` event.
const FLUSH_BYTES: usize = 32 * 1024;
/// How long a transport may take to accept input before the call gives up.
const INPUT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a transport gets to hang up (release the port, say goodbye).
const TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);
/// How often idle sessions are looked for.
const IDLE_CHECK: Duration = Duration::from_secs(15);
/// How long the answer to a login prompt may wait for the transport.
const LOGIN_SEND_TIMEOUT: Duration = Duration::from_secs(2);

/// What is known about a session before it connects.
pub(crate) struct Opening {
    pub title: String,
    pub kind: TransportKind,
    pub device_id: Option<String>,
    pub lab: bool,
    /// Where the connection goes, for display while it is being made.
    pub peer: String,
    pub cols: u16,
    pub rows: u16,
}

struct Shared {
    info: SessionInfo,
    ring: Ring,
    /// Number of the last `terminal.output` event.
    seq: u64,
    /// Output that is in the ring but has not been announced yet.
    pending: Vec<u8>,
    /// Whether the far end has asked for pastes to be bracketed.
    paste_mode: PasteMode,
}

pub(crate) struct Session {
    pub id: String,
    shared: Mutex<Shared>,
    /// The sending half of the connection, once there is one.
    writer: OnceLock<ConnectionWriter>,
    log: tokio::sync::Mutex<Option<SessionLog>>,
    /// Cancelled to end the session (or the connection attempt).
    stop: CancellationToken,
    stop_reason: Mutex<Option<String>>,
    /// Turns true when the connection has been torn down.
    finished: watch::Sender<bool>,
    /// When someone last did something with the session.
    last_activity: Mutex<Instant>,
    /// The user typed something other than Return: login prompts are theirs.
    typed: AtomicBool,
}

/// The sessions of the application.
#[derive(Default)]
pub struct TerminalState {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl TerminalState {
    pub(crate) fn get(&self, id: &str) -> AppResult<Arc<Session>> {
        self.sessions
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::not_found(format!("terminal session `{id}`")))
    }

    /// Every session, oldest first.
    pub(crate) fn all(&self) -> Vec<Arc<Session>> {
        let mut sessions: Vec<Arc<Session>> = self.sessions.lock().values().cloned().collect();
        sessions.sort_by_cached_key(|session| (session.info().opened_at, session.id.clone()));
        sessions
    }

    /// Register a session that is about to connect. When the table is full
    /// the closed session that ended longest ago makes room; live sessions
    /// are never pushed out.
    pub(crate) fn reserve(
        app: &App,
        opening: Opening,
        scrollback: usize,
    ) -> AppResult<Reservation> {
        let state = app.state::<TerminalState>();
        let session = Arc::new(Session::new(opening, scrollback));
        let evicted = {
            let mut sessions = state.sessions.lock();
            let evicted = if sessions.len() >= MAX_SESSIONS {
                let oldest_closed = sessions
                    .values()
                    .filter_map(|session| Some((session.info().closed_at?, session.id.clone())))
                    .min();
                let Some((_, id)) = oldest_closed else {
                    return Err(AppError::new(
                        "limit",
                        format!(
                            "{MAX_SESSIONS} terminal sessions are open, which is the most Nocturne holds at once; close one before opening another"
                        ),
                    ));
                };
                sessions.remove(&id);
                Some(id)
            } else {
                None
            };
            sessions.insert(session.id.clone(), session.clone());
            evicted
        };
        if let Some(session_id) = evicted {
            app.publish("terminal.removed", &RemovedEvent { session_id });
        }
        Ok(Reservation {
            app: app.clone(),
            session,
            kept: false,
        })
    }

    fn remove(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions.lock().remove(id)
    }
}

/// A session that is still connecting. Unless it is [kept](Self::keep),
/// dropping it takes the session out of the registry again — also when the
/// call that was opening it is abandoned half-way.
pub(crate) struct Reservation {
    app: App,
    session: Arc<Session>,
    kept: bool,
}

impl Reservation {
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    pub fn keep(mut self) -> Arc<Session> {
        self.kept = true;
        self.session.clone()
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.kept {
            self.app.state::<TerminalState>().remove(&self.session.id);
            self.session.finished.send_replace(true);
        }
    }
}

/// How a session came to its end.
enum Ending {
    /// Nocturne ended it; the text says why.
    Local(String),
    /// The far end (or the transport) did.
    Remote(Option<String>),
}

struct Absorbed {
    /// At least one event went out.
    announced: bool,
    /// Output is still waiting to be announced.
    pending: bool,
}

impl Session {
    fn new(opening: Opening, scrollback: usize) -> Session {
        let info = SessionInfo {
            id: new_id(),
            title: opening.title,
            kind: opening.kind,
            device_id: opening.device_id,
            lab: opening.lab,
            opened_at: now_ms(),
            state: SessionState::Connecting,
            close_reason: None,
            closed_at: None,
            cols: opening.cols,
            rows: opening.rows,
            info: ConnectionInfo {
                kind: Some(opening.kind),
                peer: opening.peer,
                ..ConnectionInfo::default()
            },
            bytes_in: 0,
            bytes_out: 0,
            logging: None,
            log_error: None,
        };
        Session {
            id: info.id.clone(),
            shared: Mutex::new(Shared {
                info,
                ring: Ring::new(scrollback),
                seq: 0,
                pending: Vec::new(),
                paste_mode: PasteMode::default(),
            }),
            writer: OnceLock::new(),
            log: tokio::sync::Mutex::new(None),
            stop: CancellationToken::new(),
            stop_reason: Mutex::new(None),
            finished: watch::Sender::new(false),
            last_activity: Mutex::new(Instant::now()),
            typed: AtomicBool::new(false),
        }
    }

    pub fn info(&self) -> SessionInfo {
        self.shared.lock().info.clone()
    }

    /// Resolves when the session is asked to end.
    pub async fn stopped(&self) {
        self.stop.cancelled().await;
    }

    /// Ask the session to end. The first reason given is the one reported.
    pub fn stop(&self, reason: &str) {
        self.stop_reason
            .lock()
            .get_or_insert_with(|| reason.to_string());
        self.stop.cancel();
    }

    fn stop_reason(&self) -> String {
        self.stop_reason
            .lock()
            .clone()
            .unwrap_or_else(|| "closed".to_string())
    }

    /// Wait (a bounded time) until the connection has been torn down.
    async fn wait_finished(&self, patience: Duration) {
        let mut finished = self.finished.subscribe();
        let _ = tokio::time::timeout(patience, finished.wait_for(|done| *done)).await;
    }

    // ----------------------------------------------------------- start-up

    /// Start logging the session. A log that cannot be started is noted on
    /// the session instead of failing it: getting onto the device comes first.
    pub async fn start_log(&self, app: &App) {
        let created = {
            let info = self.info();
            SessionLog::create(
                app,
                Subject {
                    title: &info.title,
                    device_id: info.device_id.as_deref(),
                    kind: info.kind,
                    peer: &info.info.peer,
                },
            )
            .await
        };
        let mut slot = self.log.lock().await;
        let mut shared = self.shared.lock();
        match created {
            Ok(log) => {
                shared.info.logging = Some(log.name().to_string());
                shared.info.log_error = None;
                *slot = Some(log);
            }
            Err(err) => {
                shared.info.logging = None;
                shared.info.log_error = Some(err.message);
            }
        }
    }

    /// Take over an established connection: the session is open from here on.
    pub fn start(
        self: &Arc<Self>,
        app: &App,
        connection: Connection,
        auto_login: Option<AutoLogin>,
    ) -> SessionInfo {
        let (writer, reader) = connection.split();
        let info = {
            let mut shared = self.shared.lock();
            shared.info.state = SessionState::Open;
            shared.info.info = reader.info().clone();
            shared.info.clone()
        };
        // A session is started once; a second writer would be a bug in `open`.
        let _ = self.writer.set(writer.clone());
        self.touch();
        tokio::spawn(pump(app.clone(), self.clone(), reader, writer, auto_login));
        info
    }

    // -------------------------------------------------------------- output

    /// Take in output from the far end and announce what is due.
    fn absorb(&self, app: &App, data: &[u8], announce_rest: bool) -> Absorbed {
        let mut shared = self.shared.lock();
        shared.info.bytes_in += data.len() as u64;
        shared.ring.push(data);
        shared.paste_mode.feed(data);
        let mut announced = false;
        let mut rest = data;
        loop {
            let room = FLUSH_BYTES - shared.pending.len();
            let (fits, later) = rest.split_at(room.min(rest.len()));
            shared.pending.extend_from_slice(fits);
            rest = later;
            if shared.pending.len() < FLUSH_BYTES {
                break;
            }
            self.announce(app, &mut shared);
            announced = true;
        }
        if announce_rest && !shared.pending.is_empty() {
            self.announce(app, &mut shared);
            announced = true;
        }
        Absorbed {
            announced,
            pending: !shared.pending.is_empty(),
        }
    }

    /// Publish the pending output as the next event. Called with the session
    /// locked, so that events leave in the order of their numbers and an
    /// attach sees a consistent pair of scrollback and number.
    fn announce(&self, app: &App, shared: &mut Shared) {
        if shared.pending.is_empty() {
            return;
        }
        shared.seq += 1;
        let event = OutputEvent {
            session_id: self.id.clone(),
            seq: shared.seq,
            data: BASE64.encode(&shared.pending),
        };
        shared.pending.clear();
        app.publish("terminal.output", &event);
    }

    fn announce_pending(&self, app: &App) {
        self.announce(app, &mut self.shared.lock());
    }

    /// Note that someone is using the session: it is not idle.
    pub fn touch(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    /// The scrollback and the number of the last event it includes.
    pub fn attach(&self) -> Attached {
        self.touch();
        let (info, scrollback, seq) = {
            let shared = self.shared.lock();
            // What is pending will arrive as the next event; leaving it out
            // here is what makes the hand-over free of gaps and duplicates.
            let scrollback = shared.ring.snapshot(shared.pending.len());
            (shared.info.clone(), scrollback, shared.seq)
        };
        Attached {
            info,
            scrollback: BASE64.encode(scrollback),
            seq,
        }
    }

    /// Everything the scrollback still holds, as the lines a person would
    /// have read. All of it, also when only the last few are wanted: whether
    /// those are part of a private key is decided by the lines before them.
    pub fn plain_lines(&self) -> Vec<String> {
        let raw = self.shared.lock().ring.snapshot(0);
        text::plain_lines(&raw)
    }

    /// The program at the far end has asked for pastes to be bracketed
    /// (DECSET 2004): a shell or an editor, never a network device.
    pub fn wants_bracketed_paste(&self) -> bool {
        self.shared.lock().paste_mode.is_on()
    }

    async fn log_output(&self, app: &App, data: &[u8]) {
        let mut slot = self.log.lock().await;
        let Some(log) = slot.as_mut() else { return };
        let Err(err) = log.feed(data).await else {
            return;
        };
        let name = log.name().to_string();
        *slot = None;
        logs::mark_ended(app, &name);
        let info = {
            let mut shared = self.shared.lock();
            shared.info.logging = None;
            shared.info.log_error = Some(format!("logging to {name} stopped: {err}"));
            shared.info.clone()
        };
        app.publish("terminal.changed", &info);
    }

    // --------------------------------------------------------------- input

    /// The writer of an open session, or why there is none.
    fn live_writer(&self) -> AppResult<&ConnectionWriter> {
        let shared = self.shared.lock();
        let title = &shared.info.title;
        match shared.info.state {
            SessionState::Open => self
                .writer
                .get()
                .ok_or_else(|| AppError::internal("an open session has no connection")),
            SessionState::Connecting => Err(AppError::unavailable(format!(
                "the session to {title} is still connecting"
            ))),
            SessionState::Closed => Err(ended(
                title,
                shared.info.close_reason.as_deref().unwrap_or("closed"),
            )),
        }
    }

    /// Why the session cannot be typed into, when it cannot.
    pub fn ensure_open(&self) -> AppResult<()> {
        self.live_writer().map(|_| ())
    }

    fn title(&self) -> String {
        self.shared.lock().info.title.clone()
    }

    /// Hand something to the transport, giving up on one that has stalled.
    async fn submit<T>(
        &self,
        what: &str,
        work: impl Future<Output = Result<T, nocturne_transport::TransportError>>,
    ) -> AppResult<T> {
        match tokio::time::timeout(INPUT_TIMEOUT, work).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(nocturne_transport::TransportError::Closed)) => {
                let info = self.info();
                Err(ended(
                    &info.title,
                    info.close_reason.as_deref().unwrap_or("connection closed"),
                ))
            }
            Ok(Err(err)) => {
                let mut err = AppError::from(err);
                err.message = format!("{}: {}", self.title(), err.message);
                Err(err)
            }
            Err(_) => Err(AppError::timeout(format!(
                "{} did not take {what} within {} s: the connection is stalled. Close the session and reconnect.",
                self.title(),
                INPUT_TIMEOUT.as_secs()
            ))),
        }
    }

    /// Send what the user typed or pasted.
    pub async fn input(&self, data: &str) -> AppResult<()> {
        let writer = self.live_writer()?;
        if autologin::is_real_input(data.as_bytes()) {
            self.typed.store(true, Ordering::Relaxed);
        }
        self.touch();
        self.submit(
            "the input",
            writer.send(Bytes::copy_from_slice(data.as_bytes())),
        )
        .await?;
        self.shared.lock().info.bytes_out += data.len() as u64;
        Ok(())
    }

    pub async fn resize(&self, app: &App, cols: u16, rows: u16) -> AppResult<()> {
        let writer = self.live_writer()?;
        self.touch();
        let current = {
            let shared = self.shared.lock();
            (shared.info.cols, shared.info.rows)
        };
        if current == (cols, rows) {
            return Ok(());
        }
        self.submit("the new size", writer.resize(cols, rows))
            .await?;
        let info = {
            let mut shared = self.shared.lock();
            shared.info.cols = cols;
            shared.info.rows = rows;
            shared.info.clone()
        };
        app.publish("terminal.changed", &info);
        Ok(())
    }

    pub async fn send_break(&self) -> AppResult<()> {
        let writer = self.live_writer()?;
        self.touch();
        if !writer.supports_break() {
            let info = self.info();
            let how = match info.kind {
                TransportKind::Ssh => "SSH cannot carry a break signal in this version",
                TransportKind::Local => "a local shell has no break signal",
                _ => "this connection cannot carry a break signal",
            };
            return Err(AppError::new(
                "unsupported",
                format!(
                    "{}: {how}. To interrupt a boot (ROMMON, password recovery) use the device's serial console or a Telnet console server.",
                    info.title
                ),
            ));
        }
        self.submit("the break", writer.send_break()).await
    }

    // ------------------------------------------------------------- logging

    /// Turn logging of a running session on or off.
    pub async fn set_logging(&self, app: &App, enabled: bool) -> AppResult<SessionInfo> {
        self.live_writer()?;
        let mut slot = self.log.lock().await;
        let info = match (enabled, slot.take()) {
            (true, Some(log)) => {
                *slot = Some(log);
                return Ok(self.info());
            }
            (false, None) => return Ok(self.info()),
            (true, None) => {
                let info = self.info();
                let log = SessionLog::create(
                    app,
                    Subject {
                        title: &info.title,
                        device_id: info.device_id.as_deref(),
                        kind: info.kind,
                        peer: &info.info.peer,
                    },
                )
                .await?;
                let mut shared = self.shared.lock();
                shared.info.logging = Some(log.name().to_string());
                shared.info.log_error = None;
                *slot = Some(log);
                shared.info.clone()
            }
            (false, Some(log)) => {
                if let Err(err) = log.finish(app, "logging turned off").await {
                    tracing::warn!(%err, session = %self.id, "the session log could not be completed");
                }
                let mut shared = self.shared.lock();
                shared.info.logging = None;
                shared.info.clone()
            }
        };
        drop(slot);
        app.publish("terminal.changed", &info);
        Ok(info)
    }

    // ----------------------------------------------------------------- end

    fn is_idle(&self, limit: Duration) -> bool {
        self.shared.lock().info.state == SessionState::Open
            && self.last_activity.lock().elapsed() >= limit
    }

    /// Complete the log, mark the session closed and tell everyone.
    async fn conclude(&self, app: &App, reason: &str) {
        if let Some(log) = self.log.lock().await.take()
            && let Err(err) = log.finish(app, reason).await
        {
            tracing::warn!(%err, session = %self.id, "the session log could not be completed");
        }
        self.mark_closed(app, reason);
    }

    fn mark_closed(&self, app: &App, reason: &str) {
        let info = {
            let mut shared = self.shared.lock();
            if shared.info.state == SessionState::Closed {
                return;
            }
            self.announce(app, &mut shared);
            shared.info.state = SessionState::Closed;
            shared.info.close_reason = Some(reason.to_string());
            shared.info.closed_at = Some(now_ms());
            shared.info.clone()
        };
        app.publish(
            "terminal.closed",
            &ClosedEvent {
                session_id: self.id.clone(),
                reason: reason.to_string(),
            },
        );
        app.audit(
            "terminal.close",
            info.device_id.as_deref(),
            format!("Terminal session to {} ended: {reason}", info.title),
            json!({
                "session_id": info.id,
                "kind": kind_name(info.kind),
                "peer": info.info.peer,
                "duration_ms": info.closed_at.unwrap_or(info.opened_at) - info.opened_at,
                "bytes_in": info.bytes_in,
                "bytes_out": info.bytes_out,
            }),
        );
    }
}

fn ended(title: &str, reason: &str) -> AppError {
    AppError::new(
        "closed",
        format!("the session to {title} has ended ({reason}); open a new one"),
    )
}

/// Marks the session closed if its pump goes away without having done so.
struct Unwind<'a> {
    app: &'a App,
    session: &'a Session,
    armed: bool,
}

impl Drop for Unwind<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.session
                .mark_closed(self.app, "the session stopped unexpectedly");
            self.session.finished.send_replace(true);
        }
    }
}

async fn sleep_until_some(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Runs a session from the moment it is connected until it is torn down.
async fn pump(
    app: App,
    session: Arc<Session>,
    mut reader: ConnectionReader,
    writer: ConnectionWriter,
    mut auto_login: Option<AutoLogin>,
) {
    let mut unwind = Unwind {
        app: &app,
        session: &session,
        armed: true,
    };
    let shutdown = app.shutdown_token();
    let shutting_down = shutdown.cancelled();
    let stopped = session.stop.cancelled();
    tokio::pin!(shutting_down, stopped);
    // When output was last announced, and when what is pending has to be.
    let mut announced_at: Option<Instant> = None;
    let mut announce_at: Option<Instant> = None;

    let ending = loop {
        if session.typed.load(Ordering::Relaxed) {
            auto_login = None;
        }
        let login_due = auto_login.as_ref().and_then(AutoLogin::due);
        tokio::select! {
            biased;
            () = &mut shutting_down => break Ending::Local("Nocturne is shutting down".to_string()),
            () = &mut stopped => break Ending::Local(session.stop_reason()),
            () = sleep_until_some(announce_at) => {
                session.announce_pending(&app);
                announced_at = Some(Instant::now());
                announce_at = None;
            }
            () = sleep_until_some(login_due) => {
                if let Some(login) = auto_login.as_mut() {
                    let reply = login.act(Instant::now());
                    if let Some(reply) = reply
                        && !session.typed.load(Ordering::Relaxed)
                    {
                        // Bounded: the pump must get back to reading output.
                        let sending = writer.send(Bytes::copy_from_slice(&reply));
                        if let Ok(Ok(())) = tokio::time::timeout(LOGIN_SEND_TIMEOUT, sending).await {
                            session.shared.lock().info.bytes_out += reply.len() as u64;
                        }
                    }
                    if login.is_finished() {
                        auto_login = None;
                    }
                }
            }
            output = reader.recv() => match output {
                Some(Output::Data(data)) => {
                    let now = Instant::now();
                    // Quiet until now: show it at once, a keystroke's echo must not wait.
                    let overdue = announced_at.is_none_or(|at| now.duration_since(at) >= FLUSH_INTERVAL);
                    let absorbed = session.absorb(&app, &data, overdue);
                    if absorbed.announced {
                        announced_at = Some(now);
                    }
                    announce_at = absorbed
                        .pending
                        .then(|| announced_at.unwrap_or(now) + FLUSH_INTERVAL);
                    session.log_output(&app, &data).await;
                    if let Some(login) = auto_login.as_mut() {
                        login.observe(&data, now);
                        if login.is_finished() {
                            auto_login = None;
                        }
                    }
                }
                Some(Output::Closed { reason }) => break Ending::Remote(reason),
                None => break Ending::Remote(None),
            },
        }
    };
    drop(auto_login);

    match ending {
        Ending::Remote(reason) => {
            let reason = reason
                .filter(|reason| !reason.trim().is_empty())
                .unwrap_or_else(|| "closed by the remote end".to_string());
            session.conclude(&app, &reason).await;
        }
        Ending::Local(reason) => {
            // Tell the UI first; hanging up properly can take a moment.
            session.conclude(&app, &reason).await;
            writer.close().await;
            let wound_down = async { while reader.recv().await.is_some() {} };
            if tokio::time::timeout(TEARDOWN_TIMEOUT, wound_down)
                .await
                .is_err()
            {
                tracing::debug!(session = %session.id, "the transport did not wind down in time");
            }
        }
    }
    drop(reader);
    unwind.armed = false;
    session.finished.send_replace(true);
}

/// Close sessions nobody has typed into for longer than the setting allows.
pub(crate) async fn reap_idle(app: App) {
    let mut tick = tokio::time::interval(IDLE_CHECK);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let minutes = TerminalSettings::load(&app).idle_timeout_min;
        if minutes == 0 {
            continue;
        }
        let limit = Duration::from_secs(u64::from(minutes) * 60);
        for session in app.state::<TerminalState>().all() {
            if session.is_idle(limit) {
                session.stop(&format!(
                    "closed after {minutes} min of inactivity (the terminal.idle_timeout_min setting)"
                ));
            }
        }
    }
}

/// End a session if it is still running and forget it.
pub(crate) async fn close(app: &App, id: &str) -> AppResult<()> {
    let state = app.state::<TerminalState>();
    let session = state
        .remove(id)
        .ok_or_else(|| AppError::not_found(format!("terminal session `{id}`")))?;
    session.stop("closed by user");
    // So that a serial port or a console line is free again when this returns.
    session.wait_finished(TEARDOWN_TIMEOUT).await;
    app.publish(
        "terminal.removed",
        &RemovedEvent {
            session_id: session.id.clone(),
        },
    );
    Ok(())
}
