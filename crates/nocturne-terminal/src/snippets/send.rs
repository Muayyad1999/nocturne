//! `terminal.snippets.send`: typing a snippet into sessions, on the record.
//!
//! The interface renders a snippet, shows the person the exact lines in the
//! paste confirmation and, when they agree, calls this with the hash of what
//! they saw. Everything is then done again here — the snippet is read and
//! rendered afresh and the hash compared — because between the confirmation
//! and the send the snippet can have been edited by somebody else, and what
//! reaches a device must be what was agreed to, not what the snippet has
//! become since.
//!
//! The text goes down the road typed input takes ([`Session::input`]), in
//! the form the interface's paste funnel gives a paste: one carriage return
//! per line break, no line break at the end, and bracketed only for a single
//! session whose program asked for that.

use std::collections::BTreeSet;
use std::sync::Arc;

use nocturne_app::{App, AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{fill, template};
use crate::model::SessionInfo;
use crate::platform;
use crate::session::{Session, TerminalState};

/// Most sessions one send types into: what `terminal.input_many` allows.
const MAX_SESSIONS: usize = 64;

#[derive(Debug, Deserialize)]
pub(super) struct SendParams {
    id: String,
    #[serde(default)]
    values: Value,
    session_ids: Vec<String>,
    /// The `sha256` that `terminal.snippets.render` gave for the text the
    /// person confirmed.
    expect_sha256: String,
}

#[derive(Debug, Serialize)]
pub(super) struct SendResult {
    /// The sessions the text was typed into.
    sent: Vec<Sent>,
    /// The sessions that took none or not all of it, although they were open
    /// when the send began: a connection that stalled or dropped under it.
    /// Empty in the ordinary case.
    failed: Vec<NotSent>,
}

#[derive(Debug, Serialize)]
struct Sent {
    session_id: String,
    lines: usize,
}

#[derive(Debug, Serialize)]
struct NotSent {
    session_id: String,
    code: &'static str,
    message: String,
}

/// Why `session` must not be sent this snippet, if it must not.
fn refusal(app: &App, session: &Session, profile: Option<&str>) -> Option<AppError> {
    if let Err(err) = session.ensure_open() {
        return Some(err);
    }
    let info = session.info();
    let device_type = platform::device_type(app, info.device_id.as_deref())?;
    let profile = profile?;
    (!profile.eq_ignore_ascii_case(&device_type)).then(|| {
        AppError::invalid(format!(
            "this snippet is for `{profile}` devices and {} is a `{device_type}` device",
            info.title
        ))
    })
}

pub(super) async fn send(app: &App, params: SendParams) -> AppResult<SendResult> {
    if params.session_ids.is_empty() {
        return Err(AppError::invalid(
            "terminal.snippets.send needs at least one session in `session_ids`",
        ));
    }
    if params.session_ids.len() > MAX_SESSIONS {
        return Err(AppError::invalid(format!(
            "terminal.snippets.send types into at most {MAX_SESSIONS} sessions at once"
        )));
    }
    let distinct: BTreeSet<&str> = params.session_ids.iter().map(String::as_str).collect();
    if distinct.len() != params.session_ids.len() {
        return Err(AppError::invalid(
            "a session is named twice in `session_ids`; the snippet would be typed into it twice",
        ));
    }

    let filled = fill(app, &params.id, params.values)?;
    let (snippet, rendered) = (&filled.snippet, &filled.rendered);
    let sha256 = rendered.sha256();
    if !params.expect_sha256.trim().eq_ignore_ascii_case(&sha256) {
        return Err(AppError::conflict(format!(
            "`{}` no longer renders to the text that was confirmed (the snippet or a value has changed); render it again and confirm what it shows now",
            snippet.name
        ))
        .with_details(json!({"sha256": sha256})));
    }
    if rendered.text.is_empty() {
        return Err(AppError::invalid(format!(
            "`{}` renders to nothing with these values; there is nothing to type",
            snippet.name
        )));
    }

    // Every session is looked at before anything is typed: a snippet that
    // reaches three of four sessions is a job half done that nobody chose.
    let state = app.state::<TerminalState>();
    let mut sessions: Vec<(Arc<Session>, SessionInfo)> = Vec::new();
    let mut refused: Vec<NotSent> = Vec::new();
    let mut first_code = None;
    for session_id in &params.session_ids {
        let found = state.get(session_id).and_then(|session| {
            match refusal(app, &session, snippet.profile.as_deref()) {
                Some(err) => Err(err),
                None => Ok(session),
            }
        });
        match found {
            Ok(session) => {
                let info = session.info();
                sessions.push((session, info));
            }
            Err(err) => {
                first_code.get_or_insert(err.code);
                refused.push(NotSent {
                    session_id: session_id.clone(),
                    code: err.code,
                    message: err.message,
                });
            }
        }
    }
    if let Some(code) = first_code {
        let reasons: Vec<&str> = refused.iter().map(|not| not.message.as_str()).collect();
        let message = if params.session_ids.len() == 1 {
            reasons.join("; ")
        } else {
            format!("nothing was sent to any session: {}", reasons.join("; "))
        };
        return Err(AppError::new(code, message).with_details(json!({"refused": refused})));
    }

    let typed = template::as_typed(&rendered.text);
    // Only for the one session that asked. To a session that did not, the
    // markers are six characters typed at its prompt — so a send to several
    // is never bracketed, exactly as a broadcast paste is not.
    let bracket = sessions.len() == 1 && sessions[0].0.wants_bracketed_paste();
    let data = if bracket {
        template::bracketed(&typed)
    } else {
        typed
    };

    let mut sent = Vec::new();
    let mut failed = Vec::new();
    for (session, _) in &sessions {
        match session.input(&data).await {
            Ok(()) => sent.push(Sent {
                session_id: session.id.clone(),
                lines: rendered.lines,
            }),
            Err(err) => failed.push(NotSent {
                session_id: session.id.clone(),
                code: err.code,
                message: err.message,
            }),
        }
    }

    // The record of what was sent where. The name, the places, the size and
    // the hash — never the text and never the values, which is where a
    // password typed into a variable would be.
    let titles: Vec<&str> = sessions
        .iter()
        .map(|(_, info)| info.title.as_str())
        .collect();
    let device_ids: BTreeSet<&str> = sessions
        .iter()
        .filter_map(|(_, info)| info.device_id.as_deref())
        .collect();
    let failed_ids: BTreeSet<&str> = failed.iter().map(|not| not.session_id.as_str()).collect();
    let lines = rendered.lines;
    let count = if lines == 1 {
        "1 line".to_string()
    } else {
        format!("{lines} lines")
    };
    let places = if titles.len() == 1 {
        titles[0].to_string()
    } else {
        format!("{} sessions: {}", titles.len(), titles.join(", "))
    };
    let shortfall = if failed.is_empty() {
        String::new()
    } else {
        format!(" ({} of them did not take it)", failed.len())
    };
    // The entry is about a device when all of it went to that one device.
    let all_to_devices = sessions.iter().all(|(_, info)| info.device_id.is_some());
    let one_device = match device_ids.iter().next() {
        Some(device_id) if device_ids.len() == 1 && all_to_devices => Some(*device_id),
        _ => None,
    };
    let summary = if sent.is_empty() {
        // Typed at, and taken by nobody: the entry must not read as a send.
        format!(
            "The terminal snippet `{}` ({count}) was not taken by {places}",
            snippet.name
        )
    } else {
        format!(
            "Sent the terminal snippet `{}` ({count}) to {places}{shortfall}",
            snippet.name
        )
    };
    app.audit(
        "terminal.snippet.send",
        one_device,
        summary,
        json!({
            "snippet_id": snippet.id,
            "name": snippet.name,
            "profile": snippet.profile,
            "lines": lines,
            "sha256": sha256,
            "sessions": sessions.iter().map(|(_, info)| json!({
                "session_id": info.id,
                "title": info.title,
                "device_id": info.device_id,
                "lab": info.lab,
                "sent": !failed_ids.contains(info.id.as_str()),
            })).collect::<Vec<_>>(),
            "device_ids": device_ids,
        }),
    );
    Ok(SendResult { sent, failed })
}
