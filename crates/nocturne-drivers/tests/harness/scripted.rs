//! `session/<case>.yml`: a scripted device and what the driver must do with it.
//!
//! The transcript lists, in order, every line the driver has to send and
//! what the device prints back. The driver is then put through the actions
//! (log in, run commands, configure, save); it passes only if it sends
//! exactly the transcript, in that order, and gets the listed results.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nocturne_drivers::session::{Channel, CliSession, Credentials, Mode, SessionOptions};

use super::cases::{Action, Exchange, Transcript};
use super::{Subject, entries, name_of};

/// The device's side of the conversation.
struct Script {
    newline: String,
    remaining: VecDeque<Exchange>,
    position: usize,
    outbox: VecDeque<Vec<u8>>,
    /// The first thing the driver did that the transcript does not allow.
    violation: Option<String>,
}

struct Scripted(Arc<Mutex<Script>>);

#[async_trait]
impl Channel for Scripted {
    async fn send(&mut self, data: &[u8]) -> Result<(), String> {
        let mut script = self.0.lock().map_err(|err| err.to_string())?;
        let sent = String::from_utf8_lossy(data).into_owned();
        let position = script.position;
        let violation = match script.remaining.pop_front() {
            None => Some(format!(
                "the driver sent {sent:?} after the transcript's last dialogue entry"
            )),
            Some(exchange) => {
                let wanted = match (&exchange.recv, &exchange.key) {
                    (Some(line), None) => format!("{line}{}", script.newline),
                    (None, Some(key)) => key.clone(),
                    _ => String::new(),
                };
                if wanted == sent {
                    script.position += 1;
                    if !exchange.send.is_empty() {
                        script.outbox.push_back(exchange.send.into_bytes());
                    }
                    None
                } else {
                    Some(format!(
                        "dialogue[{position}]: the driver sent {sent:?} where the transcript has {wanted:?}"
                    ))
                }
            }
        };
        match violation {
            None => Ok(()),
            Some(violation) => {
                script.violation.get_or_insert(violation.clone());
                Err(violation)
            }
        }
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, String> {
        let next = self
            .0
            .lock()
            .map_err(|err| err.to_string())?
            .outbox
            .pop_front();
        match next {
            Some(chunk) => Ok(Some(chunk)),
            None => {
                // Nothing to say: behave like a quiet line.
                tokio::time::sleep(Duration::from_secs(3600)).await;
                Ok(None)
            }
        }
    }
}

pub async fn run(subject: &mut Subject<'_>) {
    let files: Vec<_> = entries(&subject.dir.join("session"))
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| ext == "yml" || ext == "yaml")
        })
        .collect();
    for path in files {
        let file = format!("session/{}", name_of(&path));
        let Some(transcript) = subject.load::<Transcript>(&file) else {
            continue;
        };
        if transcript.description.trim().is_empty() {
            subject.fail(
                &file,
                "`description` is empty: say what this session shows, and whether the device's \
                 side was recorded from a real device or written by hand",
            );
        }
        play(subject, &file, transcript).await;
        subject.tally.sessions += 1;
    }
}

async fn play(subject: &mut Subject<'_>, file: &str, transcript: Transcript) {
    let platform = subject.platform.clone();
    for (index, exchange) in transcript.dialogue.iter().enumerate() {
        if exchange.recv.is_some() == exchange.key.is_some() {
            subject.fail(
                file,
                format!(
                    "dialogue[{index}]: give exactly one of `recv` (a line) or `key` (raw keys)"
                ),
            );
            return;
        }
        // A block scalar's final newline would put the prompt on a finished line, where no driver looks for it.
        if let Some(body) = exchange.send.strip_suffix('\n')
            && body
                .lines()
                .next_back()
                .is_some_and(|last| platform.is_prompt(last))
        {
            subject.fail(
                file,
                format!("dialogue[{index}]: `send` ends with a line break after the prompt; write the block as `|-` so the prompt is the last thing sent"),
            );
            return;
        }
    }

    let script = Arc::new(Mutex::new(Script {
        newline: platform.profile.cli.newline.clone(),
        remaining: transcript.dialogue.into(),
        position: 0,
        outbox: VecDeque::from_iter(
            (!transcript.greeting.is_empty()).then(|| transcript.greeting.clone().into_bytes()),
        ),
        violation: None,
    }));
    let credentials = Credentials {
        username: transcript.credentials.username,
        password: transcript.credentials.password,
        enable_secret: transcript.credentials.enable_secret,
    };
    let options = SessionOptions {
        enable: transcript.enable.unwrap_or(true),
        init: transcript.init.unwrap_or(true),
        ..SessionOptions::default()
    };

    let mut actions = VecDeque::from(transcript.actions);
    // Logging in is the first action whether or not the transcript lists it.
    let start_listed = actions.front().is_some_and(|action| action.start.is_some());
    let start = if start_listed {
        actions.pop_front().unwrap_or_default()
    } else {
        Action::default()
    };
    let started = CliSession::start(
        Box::new(Scripted(script.clone())),
        platform,
        &credentials,
        &options,
        None,
    )
    .await;
    let mut session = match (started, &start.error) {
        (Ok(session), None) => session,
        (Ok(_), Some(wanted)) => {
            subject.fail(
                file,
                format!("start: the login succeeded, but it must fail with `{wanted}`"),
            );
            return;
        }
        (Err(err), Some(wanted)) => {
            if !err.to_string().contains(wanted.as_str()) {
                subject.fail(
                    file,
                    format!("start: the login failed with `{err}`, expected an error containing `{wanted}`"),
                );
            }
            leftovers(subject, file, &script);
            return;
        }
        (Err(err), None) => {
            subject.fail(
                file,
                format!("start: {}", explain(&err.to_string(), &script)),
            );
            return;
        }
    };
    let expected = start.start.unwrap_or_default();
    let mode_after_start = start.mode;
    if let Some(hostname) = &expected.hostname
        && session.hostname() != hostname
    {
        subject.fail(
            file,
            format!(
                "start: the hostname read from the prompt is `{}`, expected `{hostname}`",
                session.hostname()
            ),
        );
    }
    if let Some(prompt) = &expected.prompt
        && session.prompt() != prompt
    {
        subject.fail(
            file,
            format!(
                "start: the prompt is `{}`, expected `{prompt}`",
                session.prompt()
            ),
        );
    }
    if let Some(text) = &expected.login_screen
        && !session.login_screen().contains(text.as_str())
    {
        subject.fail(
            file,
            format!(
                "start: the login screen is {:?}, expected it to contain {text:?}",
                session.login_screen()
            ),
        );
    }
    if session.warnings().len() != expected.warnings {
        subject.fail(
            file,
            format!(
                "start: {} warning(s), expected {}: {:?}",
                session.warnings().len(),
                expected.warnings,
                session.warnings()
            ),
        );
    }
    check_mode(subject, file, "start", mode_after_start, &session);

    for (index, action) in actions.into_iter().enumerate() {
        let at = format!("actions[{}]", index + usize::from(start_listed));
        if let Some(command) = &action.exec {
            match session.exec(command).await {
                Ok(result) => {
                    if let Some(output) = &action.output
                        && result.output.trim_end() != output.trim_end()
                    {
                        subject.fail(
                            file,
                            format!(
                                "{at}: `{command}` returned {:?}, expected {:?}",
                                result.output,
                                output.trim_end()
                            ),
                        );
                    }
                    if result.error.as_deref() != action.error.as_deref().map(str::trim) {
                        subject.fail(
                            file,
                            format!(
                                "{at}: `{command}` reported the error {:?}, expected {:?}",
                                result.error, action.error
                            ),
                        );
                    }
                }
                Err(err) => subject.fail(
                    file,
                    format!("{at}: `{command}`: {}", explain(&err.to_string(), &script)),
                ),
            }
        } else if let Some(lines) = &action.configure {
            match session.configure(lines).await {
                Ok(result) => {
                    if result.failed_at != action.failed_at {
                        subject.fail(
                            file,
                            format!(
                                "{at}: configure stopped at line {:?}, expected {:?}{}",
                                result.failed_at,
                                action.failed_at,
                                result
                                    .lines
                                    .last()
                                    .and_then(|line| line.error.as_ref())
                                    .map(|error| format!(" (the device said `{error}`)"))
                                    .unwrap_or_default()
                            ),
                        );
                    }
                    if let Some(wanted) = &action.error {
                        let reported = result
                            .lines
                            .last()
                            .and_then(|line| line.error.clone())
                            .or(result.commit_error.clone());
                        if reported.as_deref() != Some(wanted.trim()) {
                            subject.fail(
                                file,
                                format!("{at}: configure reported the error {reported:?}, expected `{}`", wanted.trim()),
                            );
                        }
                    } else if let Some(error) = &result.commit_error {
                        subject.fail(file, format!("{at}: the commit failed: {error}"));
                    }
                }
                Err(err) => subject.fail(
                    file,
                    format!("{at}: configure: {}", explain(&err.to_string(), &script)),
                ),
            }
        } else if action.save == Some(true) {
            match session.save().await {
                Ok(outputs) => {
                    for output in outputs {
                        if let Some(error) = output.error {
                            subject.fail(
                                file,
                                format!("{at}: `{}` reported the error `{error}`", output.command),
                            );
                        }
                    }
                }
                Err(err) => {
                    let text = err.to_string();
                    match &action.error {
                        Some(wanted) if text.contains(wanted.as_str()) => {}
                        _ => subject.fail(file, format!("{at}: save: {}", explain(&text, &script))),
                    }
                }
            }
        } else if let Some(which) = &action.fetch_config {
            match session.fetch_config(which == "startup").await {
                Ok(config) => {
                    if let Some(wanted) = &action.config
                        && config.trim_end() != wanted.trim_end()
                    {
                        subject.fail(
                            file,
                            format!(
                                "{at}: the {which} configuration is\n{config}\nexpected\n{wanted}"
                            ),
                        );
                    }
                }
                Err(err) => subject.fail(
                    file,
                    format!("{at}: fetch_config: {}", explain(&err.to_string(), &script)),
                ),
            }
        } else {
            subject.fail(
                file,
                format!("{at}: needs one of `exec`, `configure`, `save: true` or `fetch_config`"),
            );
        }
        check_mode(subject, file, &at, action.mode, &session);
    }
    leftovers(subject, file, &script);
}

fn check_mode(
    subject: &mut Subject<'_>,
    file: &str,
    at: &str,
    wanted: Option<Mode>,
    session: &CliSession,
) {
    if let Some(mode) = wanted
        && session.mode() != mode
    {
        subject.fail(
            file,
            format!(
                "{at}: the session is in {:?} mode afterwards, expected {mode:?}",
                session.mode()
            ),
        );
    }
}

/// Prefer the transcript's own account of what went wrong over the driver's.
fn explain(error: &str, script: &Arc<Mutex<Script>>) -> String {
    match script
        .lock()
        .ok()
        .and_then(|script| script.violation.clone())
    {
        Some(violation) => violation,
        None => error.to_string(),
    }
}

/// Everything in the transcript must have been needed.
fn leftovers(subject: &mut Subject<'_>, file: &str, script: &Arc<Mutex<Script>>) {
    let Ok(script) = script.lock() else { return };
    if let Some(next) = script.remaining.front() {
        let position = script.position;
        let wanted = next.recv.clone().or(next.key.clone()).unwrap_or_default();
        subject.fail(
            file,
            format!(
                "dialogue[{position}]: the driver never sent {wanted:?} ({} dialogue entr{} left unused)",
                script.remaining.len(),
                if script.remaining.len() == 1 { "y was" } else { "ies were" }
            ),
        );
    }
}
