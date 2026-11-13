//! The rest of the `terminal` RPC surface: typing, resizing, attaching,
//! snapshots, closing, listing, classifying a paste and the serial ports.

use nocturne_app::{App, AppError, AppResult, NoParams, Router};
use nocturne_drivers::policy::CommandClass;
use nocturne_transport::SerialPortInfo;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::model::{Attached, SessionInfo};
use crate::platform;
use crate::session::{self, TerminalState};
use crate::text;

/// Most lines `terminal.classify` looks at in one call. A paste larger than
/// this is not something to warn about line by line; it is a file.
const MAX_CLASSIFY_LINES: usize = 20_000;
/// Lines `terminal.snapshot` returns by default, and at most.
const DEFAULT_SNAPSHOT_LINES: usize = 200;
const MAX_SNAPSHOT_LINES: usize = 10_000;
/// Most sessions one `terminal.input_many` may type into.
const MAX_BROADCAST: usize = 64;

#[derive(Debug, Deserialize)]
struct SessionRef {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct InputParams {
    session_id: String,
    /// What the user typed or pasted, as UTF-8.
    data: String,
}

#[derive(Debug, Deserialize)]
struct InputManyParams {
    session_ids: Vec<String>,
    data: String,
}

#[derive(Debug, Serialize)]
struct InputManyResult {
    /// Sessions the input reached.
    sent: usize,
    /// Sessions it did not, and why.
    failed: Vec<NotSent>,
}

#[derive(Debug, Serialize)]
struct NotSent {
    session_id: String,
    code: &'static str,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ResizeParams {
    session_id: String,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Deserialize)]
struct SnapshotParams {
    session_id: String,
    /// Lines from the end of the scrollback; 200 by default.
    #[serde(default)]
    lines: Option<usize>,
    /// `false` returns the text as the device printed it; by default the
    /// device's own secrets are hidden.
    #[serde(default)]
    redact: Option<bool>,
}

#[derive(Debug, Serialize)]
struct SnapshotResult {
    text: String,
    /// Lines in `text`.
    lines: usize,
    /// Something in the output was hidden.
    redacted: bool,
}

#[derive(Debug, Deserialize)]
struct LoggingParams {
    session_id: String,
    /// `true` starts a log for a session that has none, `false` completes it.
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ClassifyParams {
    /// The device whose platform policy decides; the generic policy without it.
    device_id: Option<String>,
    /// What the user is about to paste or type.
    text: String,
}

#[derive(Debug, Serialize)]
struct LineClass {
    line: String,
    class: CommandClass,
    reason: String,
}

async fn input(app: &App, params: InputParams) -> AppResult<serde_json::Value> {
    app.state::<TerminalState>()
        .get(&params.session_id)?
        .input(&params.data)
        .await?;
    Ok(json!({}))
}

/// Type the same thing into several sessions. One session that has ended does
/// not stop the others: the result says which took it and which did not.
async fn input_many(app: &App, params: InputManyParams) -> AppResult<InputManyResult> {
    if params.session_ids.is_empty() {
        return Err(AppError::invalid(
            "terminal.input_many needs at least one session in `session_ids`",
        ));
    }
    if params.session_ids.len() > MAX_BROADCAST {
        return Err(AppError::invalid(format!(
            "terminal.input_many types into at most {MAX_BROADCAST} sessions at once"
        )));
    }
    let state = app.state::<TerminalState>();
    let mut sent = 0;
    let mut failed = Vec::new();
    for session_id in params.session_ids {
        let outcome = match state.get(&session_id) {
            Ok(session) => session.input(&params.data).await,
            Err(err) => Err(err),
        };
        match outcome {
            Ok(()) => sent += 1,
            Err(err) => failed.push(NotSent {
                session_id,
                code: err.code,
                message: err.message,
            }),
        }
    }
    Ok(InputManyResult { sent, failed })
}

async fn resize(app: &App, params: ResizeParams) -> AppResult<serde_json::Value> {
    let (cols, rows) = (params.cols.max(1), params.rows.max(1));
    app.state::<TerminalState>()
        .get(&params.session_id)?
        .resize(app, cols, rows)
        .await?;
    Ok(json!({}))
}

async fn send_break(app: &App, params: SessionRef) -> AppResult<serde_json::Value> {
    app.state::<TerminalState>()
        .get(&params.session_id)?
        .send_break()
        .await?;
    Ok(json!({}))
}

/// The scrollback plus the number of the last output event it covers. The
/// client applies `terminal.output` events with a higher `seq` to it.
async fn attach(app: &App, params: SessionRef) -> AppResult<Attached> {
    Ok(app
        .state::<TerminalState>()
        .get(&params.session_id)?
        .attach())
}

async fn snapshot(app: &App, params: SnapshotParams) -> AppResult<SnapshotResult> {
    let session = app.state::<TerminalState>().get(&params.session_id)?;
    let lines = params
        .lines
        .unwrap_or(DEFAULT_SNAPSHOT_LINES)
        .clamp(1, MAX_SNAPSHOT_LINES);
    let info = session.info();
    let text = session.recent_lines(lines);
    let (text, redacted) = if params.redact == Some(false) {
        (text, false)
    } else {
        let rules = platform::secret_rules(app, info.device_id.as_deref());
        let redaction = text::redact_transcript(&text, &rules);
        (redaction.text, redaction.redacted)
    };
    Ok(SnapshotResult {
        lines: if text.is_empty() {
            0
        } else {
            text.split('\n').count()
        },
        text,
        redacted,
    })
}

async fn close(app: &App, params: SessionRef) -> AppResult<serde_json::Value> {
    session::close(app, &params.session_id).await?;
    Ok(json!({}))
}

async fn list(app: &App) -> AppResult<Vec<SessionInfo>> {
    Ok(app
        .state::<TerminalState>()
        .all()
        .iter()
        .map(|session| session.info())
        .collect())
}

async fn logging(app: &App, params: LoggingParams) -> AppResult<SessionInfo> {
    let session = app.state::<TerminalState>().get(&params.session_id)?;
    let info = session.set_logging(app, params.enabled).await?;
    app.audit(
        "terminal.logging",
        info.device_id.as_deref(),
        format!(
            "Turned session logging {} for the terminal session to {}",
            if params.enabled { "on" } else { "off" },
            info.title
        ),
        json!({"session_id": info.id, "log": info.logging}),
    );
    Ok(info)
}

/// What the device's platform policy thinks of each line about to be sent.
async fn classify(app: &App, params: ClassifyParams) -> AppResult<Vec<LineClass>> {
    let platform = match params.device_id.as_deref() {
        Some(device_id) => {
            let device = nocturne_app::inventory::find(app, device_id)?;
            platform::of_device(app, &device)?
        }
        None => platform::generic(app),
    };
    let platform = platform.ok_or_else(|| {
        AppError::unavailable(
            "no device profile is loaded, so Nocturne cannot say what these commands would do",
        )
    })?;
    let lines: Vec<&str> = params.text.split('\n').collect();
    if lines.len() > MAX_CLASSIFY_LINES {
        return Err(AppError::invalid(format!(
            "terminal.classify looks at at most {MAX_CLASSIFY_LINES} lines at a time; this is {}",
            lines.len()
        )));
    }
    Ok(lines
        .into_iter()
        .map(|line| {
            let line = line.trim_end_matches('\r');
            let verdict = platform.policy.classify(line);
            LineClass {
                line: line.to_string(),
                class: verdict.class,
                reason: verdict.reason,
            }
        })
        .collect())
}

/// The serial ports of this machine. Only the desktop app has any.
async fn serial_ports(app: &App) -> AppResult<Vec<SerialPortInfo>> {
    app.require_local_access("listing serial ports")?;
    // Enumerating ports reads sysfs or the registry: off the runtime's
    // worker. A failure is reported, never passed off as "no ports".
    tokio::task::spawn_blocking(nocturne_transport::list_serial_ports)
        .await
        .map_err(|err| {
            AppError::internal(format!(
                "the serial ports of this machine could not be read: {err}"
            ))
        })
}

pub(crate) fn register(router: &mut Router) {
    router.add(
        "terminal.input",
        |app: App, params: InputParams| async move { input(&app, params).await },
    );
    router.add(
        "terminal.input_many",
        |app: App, params: InputManyParams| async move { input_many(&app, params).await },
    );
    router.add(
        "terminal.resize",
        |app: App, params: ResizeParams| async move { resize(&app, params).await },
    );
    router.add(
        "terminal.send_break",
        |app: App, params: SessionRef| async move { send_break(&app, params).await },
    );
    router.add(
        "terminal.attach",
        |app: App, params: SessionRef| async move { attach(&app, params).await },
    );
    router.add(
        "terminal.snapshot",
        |app: App, params: SnapshotParams| async move { snapshot(&app, params).await },
    );
    router.add(
        "terminal.close",
        |app: App, params: SessionRef| async move { close(&app, params).await },
    );
    router.add("terminal.list", |app: App, _params: NoParams| async move {
        list(&app).await
    });
    router.add(
        "terminal.logging",
        |app: App, params: LoggingParams| async move { logging(&app, params).await },
    );
    router.add(
        "terminal.classify",
        |app: App, params: ClassifyParams| async move { classify(&app, params).await },
    );
    router.add(
        "terminal.serial_ports",
        |app: App, _params: NoParams| async move { serial_ports(&app).await },
    );
}
