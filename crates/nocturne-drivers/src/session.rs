//! Driving a device's command line.
//!
//! [`CliSession`] turns an interactive byte stream into request/response: it
//! logs in when the transport has not already (Telnet, console), finds the
//! prompt, enters privileged mode, turns paging off, and from then on runs a
//! command and returns exactly its output — handling pagers, confirmation
//! questions, echoes and terminal control codes on the way. Configuration is
//! sent line by line, checking each reply for the platform's error patterns
//! so a rejected line stops the change where it happened.
//!
//! The session talks to a [`Channel`], not to a socket, so the same driver
//! runs over SSH, Telnet, a serial console or a simulated lab device — and in
//! tests, over a scripted fake.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::UnboundedSender;

use crate::profile::Platform;
use crate::term::Screen;

/// A duplex byte stream to a device's CLI.
#[async_trait]
pub trait Channel: Send {
    async fn send(&mut self, data: &[u8]) -> Result<(), String>;
    /// The next chunk of output; `Ok(None)` once the far end has closed.
    async fn recv(&mut self) -> Result<Option<Vec<u8>>, String>;
}

#[derive(thiserror::Error, Debug, Clone, PartialEq)]
pub enum DriverError {
    #[error("connection problem: {0}")]
    Channel(String),
    #[error("the device closed the session while {during}")]
    Closed { during: String, partial: String },
    #[error("no response for {seconds}s while {waiting_for}")]
    Timeout {
        waiting_for: String,
        seconds: u64,
        last_output: String,
    },
    #[error("login failed: {0}")]
    AuthFailed(String),
    #[error("could not enter privileged mode: {0}")]
    EnableFailed(String),
    #[error("could not enter configuration mode: {0}")]
    ConfigModeFailed(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("could not parse the output of `{command}`: {reason}")]
    Parse { command: String, reason: String },
    #[error("the device rejected `{command}`: {error}")]
    Rejected { command: String, error: String },
}

/// What the device may ask for after the transport is up.
#[derive(Clone, Debug, Default)]
pub struct Credentials {
    pub username: Option<String>,
    pub password: Option<String>,
    pub enable_secret: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SessionOptions {
    /// Total time allowed to reach the first prompt.
    pub login_timeout: Duration,
    /// Enter privileged mode when the session starts unprivileged.
    pub enable: bool,
    /// Run the profile's `cli.init` commands (paging off, terminal width).
    pub init: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        SessionOptions {
            login_timeout: Duration::from_secs(25),
            enable: true,
            init: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    User,
    Privileged,
    Config,
}

/// What passed over the session, for the audit trail and for watching automation live.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "direction", rename_all = "snake_case")]
pub enum Transcript {
    /// A line the driver sent (secrets are replaced by `********`).
    Sent { text: String },
    /// Raw bytes received, lossily decoded.
    Received { text: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CommandOutput {
    pub command: String,
    pub output: String,
    /// The line that matched one of the platform's error patterns, if any.
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConfigResult {
    /// One entry per line that was sent, in order.
    pub lines: Vec<CommandOutput>,
    /// Index into the *input* lines of the first rejected line.
    pub failed_at: Option<usize>,
    /// Error from the commit step on candidate-configuration platforms.
    pub commit_error: Option<String>,
}

impl ConfigResult {
    pub fn ok(&self) -> bool {
        self.failed_at.is_none() && self.commit_error.is_none()
    }
}

/// How long the device must stay quiet after its first prompt for the login to count as done.
const SETTLE: Duration = Duration::from_millis(150);

enum Target<'a> {
    /// This device's prompt.
    Prompt,
    Pattern(&'a regex::Regex),
}

struct Seen {
    lines: Vec<String>,
    /// The line that satisfied the wait.
    line: String,
    /// Index of the target that matched.
    which: usize,
}

pub struct CliSession {
    channel: Box<dyn Channel>,
    platform: Arc<Platform>,
    screen: Screen,
    /// Hostname part of the prompt, learned at login.
    host: String,
    prompt_line: String,
    mode: Mode,
    tap: Option<UnboundedSender<Transcript>>,
    warnings: Vec<String>,
    /// The line most recently sent, to recognise a prompt it renamed.
    last_sent: String,
    /// What the device showed before its first prompt (banner, message of the day).
    login_screen: String,
}

impl CliSession {
    /// Take over a freshly opened channel and bring it to a usable prompt.
    pub async fn start(
        channel: Box<dyn Channel>,
        platform: Arc<Platform>,
        credentials: &Credentials,
        options: &SessionOptions,
        tap: Option<UnboundedSender<Transcript>>,
    ) -> Result<CliSession, DriverError> {
        let mut session = CliSession {
            channel,
            platform,
            screen: Screen::new(),
            host: String::new(),
            prompt_line: String::new(),
            mode: Mode::Privileged,
            tap,
            warnings: Vec::new(),
            last_sent: String::new(),
            login_screen: String::new(),
        };
        session.login(credentials, options.login_timeout).await?;
        session.prepare(credentials, options).await?;
        Ok(session)
    }

    /// Enter privileged mode and run the profile's `cli.init` commands, as far as `options` ask.
    async fn prepare(
        &mut self,
        credentials: &Credentials,
        options: &SessionOptions,
    ) -> Result<(), DriverError> {
        if options.enable {
            self.enable(credentials).await?;
        }
        if options.init {
            let init = self.platform.profile.cli.init.clone();
            for command in init {
                // Failing to turn paging off is survivable: the pager is handled anyway.
                if let Err(err) = self
                    .roundtrip(&command, self.default_timeout(), false)
                    .await
                {
                    match err {
                        DriverError::Timeout { .. } => {
                            self.warnings.push(format!("`{command}` did not complete"))
                        }
                        other => return Err(other),
                    }
                }
            }
        }
        Ok(())
    }

    /// Drive the same session with another platform's profile, without
    /// reconnecting: its prompt patterns, privileged mode and `cli.init`
    /// commands. This is how a session opened with the permissive `generic`
    /// profile continues once the device has been identified.
    ///
    /// The device is asked for a fresh prompt first. If that prompt is not
    /// one the new profile recognises, the session goes back to the profile
    /// it had and stays usable; the error says what the device showed. A
    /// failure after that (a wrong enable secret, say) leaves the session
    /// bound to the new profile.
    pub async fn rebind(
        &mut self,
        platform: Arc<Platform>,
        credentials: &Credentials,
        options: &SessionOptions,
    ) -> Result<(), DriverError> {
        let previous = std::mem::replace(&mut self.platform, platform);
        let previous_host = std::mem::take(&mut self.host);
        let wait = self.default_timeout().min(options.login_timeout);
        match self.sync_within(wait).await {
            Ok(()) => {}
            Err(err) => {
                let shown = match &err {
                    DriverError::Timeout { last_output, .. } => last_output
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or_default()
                        .trim()
                        .to_string(),
                    _ => String::new(),
                };
                let wanted = std::mem::replace(&mut self.platform, previous);
                self.host = previous_host;
                return Err(match err {
                    DriverError::Timeout { seconds, .. } => DriverError::Timeout {
                        waiting_for: format!(
                            "waiting for a prompt that matches the `{}` profile; the device shows `{shown}`",
                            wanted.id()
                        ),
                        seconds,
                        last_output: shown,
                    },
                    other => other,
                });
            }
        }
        self.prepare(credentials, options).await
    }

    /// Send an empty line and wait for the prompt. Anything still arriving
    /// from an earlier command is discarded, so the next command starts clean.
    pub async fn sync(&mut self) -> Result<(), DriverError> {
        self.sync_within(self.default_timeout()).await
    }

    async fn sync_within(&mut self, wait: Duration) -> Result<(), DriverError> {
        self.drain().await;
        self.screen.clear_current_line();
        let newline = self.platform.profile.cli.newline.clone();
        self.send_raw(newline.as_bytes()).await?;
        let seen = self
            .expect(
                &[Target::Prompt],
                wait,
                "waiting for the prompt",
                false,
                false,
            )
            .await?;
        self.adopt_prompt(&seen.line, self.host.is_empty());
        Ok(())
    }

    /// What the device showed between connecting and its first prompt: the
    /// banner, the message of the day, a "last login" line.
    pub fn login_screen(&self) -> &str {
        &self.login_screen
    }

    /// Start, replace or stop the live transcript of this session.
    pub fn set_tap(&mut self, tap: Option<UnboundedSender<Transcript>>) {
        self.tap = tap;
    }

    pub fn platform(&self) -> &Arc<Platform> {
        &self.platform
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The hostname shown in the prompt.
    pub fn hostname(&self) -> &str {
        &self.host
    }

    /// The most recent prompt line, verbatim.
    pub fn prompt(&self) -> &str {
        &self.prompt_line
    }

    /// Non-fatal problems noticed so far (e.g. "no enable secret configured").
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Run one exec-mode command with the platform's default timeout.
    pub async fn exec(&mut self, command: &str) -> Result<CommandOutput, DriverError> {
        self.exec_with_timeout(command, self.default_timeout())
            .await
    }

    /// Run one exec-mode command; `idle` is how long the device may stay silent.
    pub async fn exec_with_timeout(
        &mut self,
        command: &str,
        idle: Duration,
    ) -> Result<CommandOutput, DriverError> {
        let started = Instant::now();
        let output = self.roundtrip(command, idle, false).await?;
        let error = self.platform.find_error(&output, false).map(str::to_string);
        Ok(CommandOutput {
            command: command.to_string(),
            output,
            error,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// Apply configuration lines. Stops at the first rejected line; lines
    /// before it have already taken effect on platforms without a candidate
    /// configuration, which is why the caller keeps a backup.
    pub async fn configure(&mut self, lines: &[String]) -> Result<ConfigResult, DriverError> {
        let config = self.platform.profile.config.clone();
        let timeout = self.default_timeout();

        for command in &config.enter {
            let output = self.roundtrip(command, timeout, true).await?;
            if let Some(error) = self.platform.find_error(&output, false) {
                return Err(DriverError::ConfigModeFailed(error.to_string()));
            }
        }
        if !config.enter.is_empty() && self.mode != Mode::Config {
            return Err(DriverError::ConfigModeFailed(format!(
                "after `{}` the prompt is still `{}`",
                config.enter.join("`, `"),
                self.prompt_line
            )));
        }

        let mut result = ConfigResult {
            lines: Vec::with_capacity(lines.len()),
            failed_at: None,
            commit_error: None,
        };
        for (index, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let started = Instant::now();
            let output = self.roundtrip(line, timeout, true).await?;
            let error = self
                .platform
                .find_error(&output, !config.enter.is_empty())
                .map(str::to_string);
            let failed = error.is_some();
            result.lines.push(CommandOutput {
                command: line.clone(),
                output,
                error,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
            if failed {
                result.failed_at = Some(index);
                break;
            }
        }

        // Candidate-configuration platforms: activate, or throw the candidate away.
        if result.failed_at.is_none() {
            for command in &config.commit {
                let output = self
                    .roundtrip(command, timeout.max(Duration::from_secs(120)), true)
                    .await?;
                if let Some(error) = self.platform.find_error(&output, true) {
                    result.commit_error = Some(error.to_string());
                    break;
                }
            }
        }
        if !result.ok() {
            for command in &config.abort {
                self.roundtrip(command, timeout, true).await?;
            }
        }
        for command in &config.exit {
            self.roundtrip(command, timeout, true).await?;
        }
        Ok(result)
    }

    /// Persist the running configuration.
    pub async fn save(&mut self) -> Result<Vec<CommandOutput>, DriverError> {
        let commands = self.platform.profile.config.save.clone();
        if commands.is_empty() {
            return Err(DriverError::Unsupported(format!(
                "the `{}` profile does not define how to save the configuration",
                self.platform.id()
            )));
        }
        let mut outputs = Vec::new();
        for command in commands {
            let started = Instant::now();
            let output = self
                .roundtrip(
                    &command,
                    self.default_timeout().max(Duration::from_secs(120)),
                    true,
                )
                .await?;
            let error = self.platform.find_error(&output, false).map(str::to_string);
            outputs.push(CommandOutput {
                command,
                output,
                error,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        Ok(outputs)
    }

    /// Fetch the running (or startup) configuration with volatile lines removed.
    pub async fn fetch_config(&mut self, startup: bool) -> Result<String, DriverError> {
        let config = &self.platform.profile.config;
        let command = if startup {
            config.startup.clone()
        } else {
            Some(config.running.clone())
        }
        .filter(|command| !command.is_empty())
        .ok_or_else(|| {
            DriverError::Unsupported(format!(
                "the `{}` profile does not define how to read the {} configuration",
                self.platform.id(),
                if startup { "startup" } else { "running" }
            ))
        })?;
        let output = self
            .exec_with_timeout(
                &command,
                self.default_timeout().max(Duration::from_secs(120)),
            )
            .await?;
        if let Some(error) = output.error {
            return Err(DriverError::Rejected { command, error });
        }
        Ok(self.platform.clean_config(&output.output))
    }

    /// Say goodbye politely and drop the channel.
    pub async fn close(mut self) {
        let exit = self.platform.profile.cli.exit.clone();
        if !exit.is_empty() {
            let _ = self.send_line(&exit, false).await;
        }
    }

    fn default_timeout(&self) -> Duration {
        Duration::from_secs(self.platform.profile.cli.timeout_s.clamp(1, 3600))
    }

    // ------------------------------------------------------------------ login

    async fn login(
        &mut self,
        credentials: &Credentials,
        total: Duration,
    ) -> Result<(), DriverError> {
        let platform = self.platform.clone();
        let deadline = Instant::now() + total;
        let mut nudges = 0;
        let mut usernames_sent = 0;
        let mut passwords_sent = 0;
        let mut everything: Vec<String> = Vec::new();

        loop {
            let targets = [
                Target::Prompt,
                Target::Pattern(&platform.username_prompt),
                Target::Pattern(&platform.password_prompt),
            ];
            // Interactive, so that a question the profile knows the answer to
            // (RouterOS offering to show its licence, say) does not block the login.
            match self
                .expect(
                    &targets,
                    Duration::from_millis(2500),
                    "logging in",
                    true,
                    false,
                )
                .await
            {
                Ok(seen) => {
                    everything.extend(seen.lines);
                    let failure = everything
                        .iter()
                        .rev()
                        .take(6)
                        .find(|line| platform.login_failure.is_match(line))
                        .cloned();
                    match seen.which {
                        0 => {
                            // A banner line can end like a prompt. The real
                            // prompt is the one the device stops after.
                            if self.more_arrives(SETTLE, "logging in").await? {
                                everything.push(seen.line);
                                continue;
                            }
                            self.adopt_prompt(&seen.line, true);
                            self.login_screen = everything
                                .iter()
                                .map(|line| line.trim_end())
                                .collect::<Vec<_>>()
                                .join("\n")
                                .trim_matches('\n')
                                .to_string();
                            return Ok(());
                        }
                        1 => {
                            if usernames_sent >= 1 && failure.is_some() || usernames_sent >= 3 {
                                return Err(DriverError::AuthFailed(failure.unwrap_or_else(
                                    || "the device keeps asking for a username".into(),
                                )));
                            }
                            let Some(username) = credentials
                                .username
                                .as_deref()
                                .filter(|name| !name.is_empty())
                            else {
                                return Err(DriverError::AuthFailed(
                                    "the device asked for a username but the credential has none"
                                        .into(),
                                ));
                            };
                            let username =
                                format!("{username}{}", platform.profile.cli.login.username_suffix);
                            self.screen.clear_current_line();
                            self.send_line(&username, false).await?;
                            usernames_sent += 1;
                        }
                        _ => {
                            if passwords_sent >= 1 && failure.is_some() || passwords_sent >= 3 {
                                return Err(DriverError::AuthFailed(failure.unwrap_or_else(
                                    || "the device rejected the password".into(),
                                )));
                            }
                            let Some(password) = credentials.password.as_deref() else {
                                return Err(DriverError::AuthFailed(
                                    "the device asked for a password but the credential has none"
                                        .into(),
                                ));
                            };
                            self.screen.clear_current_line();
                            self.send_line(password, true).await?;
                            passwords_sent += 1;
                        }
                    }
                }
                Err(DriverError::Timeout { last_output, .. }) => {
                    if Instant::now() >= deadline || nudges >= 6 {
                        everything.push(last_output);
                        let seen: Vec<&str> = everything
                            .iter()
                            .map(String::as_str)
                            .filter(|line| !line.trim().is_empty())
                            .collect();
                        let tail = seen
                            .iter()
                            .rev()
                            .take(4)
                            .rev()
                            .copied()
                            .collect::<Vec<_>>()
                            .join(" / ");
                        return Err(DriverError::Timeout {
                            waiting_for: format!(
                                "waiting for a prompt that matches the `{}` profile (last output: {})",
                                platform.id(),
                                if tail.is_empty() {
                                    "nothing"
                                } else {
                                    tail.as_str()
                                }
                            ),
                            seconds: total.as_secs(),
                            last_output: tail.clone(),
                        });
                    }
                    // Consoles and some Telnet servers wait for a keypress before showing anything.
                    nudges += 1;
                    let newline = platform.profile.cli.newline.clone();
                    self.send_raw(newline.as_bytes()).await?;
                }
                Err(DriverError::Closed { partial, .. }) => {
                    everything.push(partial);
                    let failure = everything
                        .iter()
                        .rev()
                        .find(|line| platform.login_failure.is_match(line))
                        .cloned();
                    return Err(match failure {
                        Some(failure) => DriverError::AuthFailed(failure),
                        None => DriverError::Closed {
                            during: "logging in".into(),
                            partial: everything.join("\n"),
                        },
                    });
                }
                Err(other) => return Err(other),
            }
        }
    }

    async fn enable(&mut self, credentials: &Credentials) -> Result<(), DriverError> {
        let platform = self.platform.clone();
        let Some(enable) = platform.profile.cli.enable.clone() else {
            return Ok(());
        };
        if self.mode != Mode::User {
            return Ok(());
        }
        let Some(password_prompt) = platform.enable_password.as_ref() else {
            return Ok(());
        };
        let timeout = self.default_timeout();
        self.drain().await;
        self.screen.clear_current_line();
        self.send_line(&enable.command, false).await?;

        for attempt in 0..3 {
            let targets = [Target::Prompt, Target::Pattern(password_prompt)];
            let seen = self
                .expect(&targets, timeout, "entering privileged mode", false, false)
                .await?;
            if seen.which == 0 {
                self.adopt_prompt(&seen.line, false);
                if self.mode == Mode::User {
                    let reason = seen
                        .lines
                        .iter()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .cloned()
                        .unwrap_or_default();
                    return match credentials.enable_secret {
                        Some(_) => Err(DriverError::EnableFailed(if reason.is_empty() {
                            "the enable secret was not accepted".into()
                        } else {
                            reason
                        })),
                        None => {
                            self.warnings.push(
                                "Still in unprivileged mode: the credential has no enable secret, so some commands will be refused.".into(),
                            );
                            Ok(())
                        }
                    };
                }
                return Ok(());
            }
            self.screen.clear_current_line();
            match credentials.enable_secret.as_deref() {
                Some(secret) if attempt == 0 => self.send_line(secret, true).await?,
                // No (or a wrong) secret: answer with empty lines until the device gives up.
                _ => self.send_line("", false).await?,
            }
        }
        Err(DriverError::EnableFailed(
            "the device kept asking for the enable secret".into(),
        ))
    }

    // --------------------------------------------------------------- plumbing

    /// Send a command and return its output without the echo and the prompt.
    async fn roundtrip(
        &mut self,
        command: &str,
        idle: Duration,
        config_answers: bool,
    ) -> Result<String, DriverError> {
        self.drain().await;
        self.screen.clear_current_line();
        self.send_line(command, false).await?;
        let seen = self
            .expect(
                &[Target::Prompt],
                idle,
                &format!("running `{}`", first_line(command)),
                true,
                config_answers,
            )
            .await?;
        self.adopt_prompt(&seen.line, false);
        Ok(strip_echo(seen.lines, command))
    }

    /// Read until the line being drawn satisfies one of `targets`.
    async fn expect(
        &mut self,
        targets: &[Target<'_>],
        idle: Duration,
        doing: &str,
        interactive: bool,
        config_answers: bool,
    ) -> Result<Seen, DriverError> {
        let platform = self.platform.clone();
        let mut lines: Vec<String> = Vec::new();
        let mut answered: Option<String> = None;
        loop {
            lines.extend(self.screen.take_lines());
            let line = self.screen.current_line();
            if line.is_empty()
                && let Some(which) = targets
                    .iter()
                    .position(|target| matches!(target, Target::Prompt))
                && let Some(at) = self.prompt_before_unsolicited(&lines)
            {
                let line = lines[at].clone();
                lines.truncate(at);
                return Ok(Seen { lines, line, which });
            }
            if !line.is_empty() {
                if interactive {
                    if let (Some(pager), Some(spec)) =
                        (&platform.pager, &platform.profile.cli.pager)
                        && pager.is_match(&line)
                    {
                        self.screen.clear_current_line();
                        self.send_raw(spec.key.as_bytes()).await?;
                        continue;
                    }
                    let answers = if config_answers && !platform.config_answers.is_empty() {
                        &platform.config_answers
                    } else {
                        &platform.exec_answers
                    };
                    if answered.as_deref() != Some(line.as_str())
                        && let Some((_, answer)) =
                            answers.iter().find(|(pattern, _)| pattern.is_match(&line))
                    {
                        answered = Some(line.clone());
                        lines.push(line.clone());
                        self.screen.clear_current_line();
                        self.send_line(answer, false).await?;
                        continue;
                    }
                }
                for (which, target) in targets.iter().enumerate() {
                    let hit = match target {
                        Target::Prompt => self.is_prompt(&line),
                        Target::Pattern(pattern) => pattern.is_match(&line),
                    };
                    if hit {
                        return Ok(Seen { lines, line, which });
                    }
                }
            }

            match tokio::time::timeout(idle, self.channel.recv()).await {
                Err(_) => {
                    lines.push(line);
                    let tail: Vec<&str> = lines
                        .iter()
                        .rev()
                        .take(5)
                        .rev()
                        .map(String::as_str)
                        .collect();
                    return Err(DriverError::Timeout {
                        waiting_for: doing.to_string(),
                        seconds: idle.as_secs().max(1),
                        last_output: tail.join("\n"),
                    });
                }
                Ok(Err(err)) => return Err(DriverError::Channel(err)),
                Ok(Ok(None)) => {
                    lines.push(line);
                    return Err(DriverError::Closed {
                        during: doing.to_string(),
                        partial: lines.join("\n"),
                    });
                }
                Ok(Ok(Some(data))) => {
                    if let Some(tap) = &self.tap {
                        let _ = tap.send(Transcript::Received {
                            text: String::from_utf8_lossy(&data).into_owned(),
                        });
                    }
                    self.screen.feed(&data);
                }
            }
        }
    }

    /// A console line prints log messages whenever it likes, also right after
    /// the prompt — which then is no longer the line being drawn. If the
    /// output so far ends with our prompt followed only by such messages,
    /// this is the index of that prompt line.
    fn prompt_before_unsolicited(&self, lines: &[String]) -> Option<usize> {
        let mut unsolicited = 0;
        for (index, line) in lines.iter().enumerate().rev() {
            if line.trim().is_empty() {
                continue;
            }
            if self.platform.is_unsolicited(line) {
                unsolicited += 1;
                continue;
            }
            return (unsolicited > 0 && self.is_prompt(line)).then_some(index);
        }
        None
    }

    /// Wait briefly for more output and take it in. `false` when the device stays quiet.
    async fn more_arrives(&mut self, within: Duration, doing: &str) -> Result<bool, DriverError> {
        match tokio::time::timeout(within, self.channel.recv()).await {
            Err(_) => Ok(false),
            Ok(Err(err)) => Err(DriverError::Channel(err)),
            Ok(Ok(None)) => Err(DriverError::Closed {
                during: doing.to_string(),
                partial: self.screen.current_line(),
            }),
            Ok(Ok(Some(data))) => {
                if let Some(tap) = &self.tap {
                    let _ = tap.send(Transcript::Received {
                        text: String::from_utf8_lossy(&data).into_owned(),
                    });
                }
                self.screen.feed(&data);
                Ok(true)
            }
        }
    }

    /// Swallow output that arrived between commands (log messages, redrawn prompts).
    async fn drain(&mut self) {
        while let Ok(Ok(Some(data))) =
            tokio::time::timeout(Duration::from_millis(2), self.channel.recv()).await
        {
            if let Some(tap) = &self.tap {
                let _ = tap.send(Transcript::Received {
                    text: String::from_utf8_lossy(&data).into_owned(),
                });
            }
            self.screen.feed(&data);
        }
        self.screen.take_lines();
    }

    fn is_prompt(&self, line: &str) -> bool {
        let Some(caps) = self.platform.prompt.captures(line) else {
            return false;
        };
        match (caps.name("host"), self.host.is_empty()) {
            // A different hostname is still our prompt if the command just
            // sent is what renamed the device (`hostname NEW`, `sysname NEW`, …).
            (Some(host), false) => {
                same_host(host.as_str(), &self.host)
                    || (host.as_str().len() >= 2 && self.last_sent.contains(host.as_str()))
            }
            _ => true,
        }
    }

    /// Record a prompt line: remember it, learn the hostname, work out the mode.
    fn adopt_prompt(&mut self, line: &str, learn_host: bool) {
        self.prompt_line = line.to_string();
        if let Some(host) = self
            .platform
            .prompt
            .captures(line)
            .and_then(|caps| caps.name("host"))
        {
            // The hostname may legitimately change mid-session (`hostname NEW`).
            if learn_host || self.host.is_empty() || !same_host(host.as_str(), &self.host) {
                self.host = host.as_str().to_string();
            }
        }
        self.mode = if self.platform.is_config_prompt(line) {
            Mode::Config
        } else if self.platform.is_user_prompt(line) {
            Mode::User
        } else {
            Mode::Privileged
        };
    }

    async fn send_line(&mut self, text: &str, secret: bool) -> Result<(), DriverError> {
        if let Some(tap) = &self.tap {
            let shown = if secret {
                "********".to_string()
            } else {
                text.to_string()
            };
            let _ = tap.send(Transcript::Sent { text: shown });
        }
        if !secret {
            self.last_sent = text.to_string();
        }
        let newline = self.platform.profile.cli.newline.clone();
        // A multi-line command (a banner) is sent with the platform's own line ending.
        let mut payload = text.replace("\r\n", "\n").replace('\n', &newline);
        payload.push_str(&newline);
        self.send_raw(payload.as_bytes()).await
    }

    async fn send_raw(&mut self, data: &[u8]) -> Result<(), DriverError> {
        self.channel.send(data).await.map_err(DriverError::Channel)
    }
}

/// Prompts abbreviate long hostnames and add mode decorations, so compare loosely.
fn same_host(seen: &str, known: &str) -> bool {
    if seen == known {
        return true;
    }
    let seen_lower = seen.to_ascii_lowercase();
    let known_lower = known.to_ascii_lowercase();
    let prefix = known_lower.chars().take(10).collect::<String>();
    seen_lower.starts_with(&prefix)
        || (seen_lower.len() >= 4 && known_lower.starts_with(&seen_lower))
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

/// Remove the device's echo of the command from the head of the output.
fn strip_echo(mut lines: Vec<String>, command: &str) -> String {
    let sent: Vec<&str> = command
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let mut drop = 0;
    for expected in &sent {
        match lines.get(drop) {
            Some(line) if line.trim_end().ends_with(expected) || line.contains(expected) => {
                drop += 1
            }
            _ => break,
        }
    }
    lines.drain(..drop);
    // Trailing blank lines before the prompt are layout, not output.
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    while lines.first().is_some_and(|line| line.trim().is_empty()) {
        lines.remove(0);
    }
    lines.join("\n")
}

#[cfg(test)]
pub(crate) mod testing {
    //! A scripted device that behaves like an IOS-style switch over a raw
    //! byte stream: echo, paging, enable, config mode, error markers.

    use std::collections::VecDeque;

    use super::*;

    pub struct FakeDevice {
        pub hostname: String,
        /// Bytes waiting to be read by the driver.
        outbox: VecDeque<Vec<u8>>,
        input: String,
        stage: Stage,
        privileged: bool,
        config_depth: u8,
        paging: bool,
        /// Remaining lines of an output stopped at `--More--`.
        paused: Option<Vec<String>>,
        pub username: String,
        pub password: String,
        pub enable_secret: Option<String>,
        pub applied: Vec<String>,
        pub saved: bool,
        closed: bool,
        /// Emit data in tiny chunks to exercise reassembly.
        pub dribble: bool,
        /// Behave like a console line: log leaving configuration mode right after the prompt.
        pub console_logging: bool,
    }

    #[derive(PartialEq)]
    enum Stage {
        /// Nothing shown until a key is pressed.
        Asleep,
        Username,
        Password,
        Shell,
        EnablePassword,
        Confirm,
    }

    impl FakeDevice {
        /// A device reached over SSH: already authenticated, lands in user mode.
        pub fn ssh(hostname: &str) -> FakeDevice {
            let mut device = FakeDevice::blank(hostname, Stage::Shell);
            device.emit("\r\nAuthorised access only!\r\n\r\n");
            device.prompt();
            device
        }

        /// A device on a console or Telnet line: shows nothing until Enter, then asks for a login.
        pub fn console(hostname: &str) -> FakeDevice {
            FakeDevice::blank(hostname, Stage::Asleep)
        }

        fn blank(hostname: &str, stage: Stage) -> FakeDevice {
            FakeDevice {
                hostname: hostname.to_string(),
                outbox: VecDeque::new(),
                input: String::new(),
                stage,
                privileged: false,
                config_depth: 0,
                paging: true,
                paused: None,
                username: "admin".into(),
                password: "pw".into(),
                enable_secret: Some("secret".into()),
                applied: Vec::new(),
                saved: false,
                closed: false,
                dribble: false,
                console_logging: false,
            }
        }

        fn emit(&mut self, text: &str) {
            if self.dribble {
                for chunk in text.as_bytes().chunks(3) {
                    self.outbox.push_back(chunk.to_vec());
                }
            } else {
                self.outbox.push_back(text.as_bytes().to_vec());
            }
        }

        fn prompt(&mut self) {
            let text = match (self.config_depth, self.privileged) {
                (0, false) => format!("{}>", self.hostname),
                (0, true) => format!("{}#", self.hostname),
                (1, _) => format!("{}(config)#", self.hostname),
                _ => format!("{}(config-if)#", self.hostname),
            };
            self.emit(&text);
        }

        fn page(&mut self, lines: Vec<String>) {
            if !self.paging || lines.len() <= 3 {
                self.emit(&(lines.join("\r\n") + "\r\n"));
                self.prompt();
                return;
            }
            let (first, rest) = lines.split_at(3);
            self.emit(&(first.join("\r\n") + "\r\n --More-- "));
            self.paused = Some(rest.to_vec());
        }

        fn key(&mut self, byte: u8) {
            if let Some(rest) = self.paused.take() {
                // Erase the pager text the way IOS does, then continue.
                self.emit(&format!("{0}{1}{0}", "\u{8}".repeat(10), " ".repeat(10)));
                if byte == b' ' {
                    self.page(rest);
                } else {
                    self.emit("\r\n");
                    self.prompt();
                }
                return;
            }
            match byte {
                b'\r' | b'\n' => {
                    let line = std::mem::take(&mut self.input);
                    self.line(&line);
                }
                _ => self.input.push(byte as char),
            }
        }

        fn line(&mut self, line: &str) {
            match self.stage {
                Stage::Asleep => {
                    self.stage = Stage::Username;
                    self.emit("\r\n\r\nUser Access Verification\r\n\r\nUsername: ");
                }
                Stage::Username => {
                    self.emit(&format!("{line}\r\nPassword: "));
                    self.input = line.to_string();
                    self.stage = Stage::Password;
                    // Remember the offered username in `applied` scratch space.
                    self.applied.push(format!("login:{line}"));
                    self.input.clear();
                }
                Stage::Password => {
                    let user_ok = self
                        .applied
                        .last()
                        .is_some_and(|entry| entry == &format!("login:{}", self.username));
                    self.applied.pop();
                    if user_ok && line == self.password {
                        self.stage = Stage::Shell;
                        self.emit("\r\n");
                        self.prompt();
                    } else {
                        self.stage = Stage::Username;
                        self.emit("\r\n% Login invalid\r\n\r\nUsername: ");
                    }
                }
                Stage::EnablePassword => {
                    self.emit("\r\n");
                    if Some(line) == self.enable_secret.as_deref() {
                        self.privileged = true;
                        self.stage = Stage::Shell;
                        self.prompt();
                    } else {
                        self.stage = Stage::Shell;
                        self.emit("% Bad secrets\r\n\r\n");
                        self.prompt();
                    }
                }
                Stage::Confirm => {
                    self.stage = Stage::Shell;
                    self.saved = true;
                    self.emit(&format!("{line}\r\nBuilding configuration...\r\n[OK]\r\n"));
                    self.prompt();
                }
                Stage::Shell => {
                    self.emit(&format!("{line}\r\n"));
                    self.command(line.trim());
                }
            }
        }

        fn command(&mut self, command: &str) {
            if self.config_depth > 0 {
                match command {
                    "end" if self.console_logging => {
                        self.config_depth = 0;
                        // One write, as when the message is already queued behind the prompt.
                        let hostname = self.hostname.clone();
                        self.emit(&format!(
                            "{hostname}#\r\n*Mar  1 00:07:20.001: %SYS-5-CONFIG_I: Configured from console by console\r\n"
                        ));
                        return;
                    }
                    "end" => self.config_depth = 0,
                    "exit" => self.config_depth -= 1,
                    line if line.starts_with("interface ") => {
                        self.config_depth = 2;
                        self.applied.push(line.to_string());
                    }
                    line if line.starts_with("bogus") => {
                        self.emit(
                            "                ^\r\n% Invalid input detected at '^' marker.\r\n\r\n",
                        );
                    }
                    line if line.starts_with("hostname ") => {
                        self.hostname = line["hostname ".len()..].to_string();
                        self.applied.push(line.to_string());
                    }
                    "" => {}
                    line => self.applied.push(line.to_string()),
                }
                self.prompt();
                return;
            }
            match command {
                "" => self.prompt(),
                "enable" if self.privileged => self.prompt(),
                "enable" => match self.enable_secret {
                    Some(_) => {
                        self.stage = Stage::EnablePassword;
                        self.emit("Password: ");
                    }
                    None => {
                        self.privileged = true;
                        self.prompt();
                    }
                },
                "terminal length 0" => {
                    self.paging = false;
                    self.prompt();
                }
                "terminal width 511" => self.prompt(),
                "show clock" => self.page(vec!["*12:00:00.000 UTC Mon Mar 1 1993".into()]),
                "show vlan brief" => self.page(
                    [
                        "VLAN Name    Status",
                        "---- ------- ------",
                        "1    default active",
                        "10   Staff   active",
                        "20   Guest   active",
                        "30   Voice   active",
                    ]
                    .map(String::from)
                    .to_vec(),
                ),
                "show running-config" if !self.privileged => {
                    self.emit("            ^\r\n% Invalid input detected at '^' marker.\r\n\r\n");
                    self.prompt();
                }
                "show running-config" => self.page(
                    [
                        "Building configuration...",
                        "",
                        "Current configuration : 120 bytes",
                        "!",
                        "hostname X",
                        "!",
                        "vlan 10",
                        " name Staff",
                        "!",
                        "end",
                    ]
                    .map(String::from)
                    .to_vec(),
                ),
                "configure terminal" if self.privileged => {
                    self.config_depth = 1;
                    self.emit("Enter configuration commands, one per line.  End with CNTL/Z.\r\n");
                    self.prompt();
                }
                "write memory" if self.privileged => {
                    self.saved = true;
                    self.emit("Building configuration...\r\n[OK]\r\n");
                    self.prompt();
                }
                "copy running-config startup-config" if self.privileged => {
                    self.stage = Stage::Confirm;
                    self.emit("Destination filename [startup-config]? ");
                }
                "exit" => self.closed = true,
                "hang" => {}
                _ => {
                    self.emit("       ^\r\n% Invalid input detected at '^' marker.\r\n\r\n");
                    self.prompt();
                }
            }
        }
    }

    #[async_trait]
    impl Channel for FakeDevice {
        async fn send(&mut self, data: &[u8]) -> Result<(), String> {
            for byte in data {
                self.key(*byte);
            }
            Ok(())
        }

        async fn recv(&mut self) -> Result<Option<Vec<u8>>, String> {
            match self.outbox.pop_front() {
                Some(chunk) => Ok(Some(chunk)),
                None if self.closed => Ok(None),
                None => {
                    // Nothing to say: behave like a quiet socket.
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Ok(None)
                }
            }
        }
    }

    pub const IOS_PROFILE: &str = r#"
id: test_ios
name: Test IOS
cli:
  prompt: '^(?P<host>[\w.\-]+)(?:\([\w\-]+\))?[>#]\s?$'
  user_prompt: '>\s?$'
  config_prompt: '\(config[^)]*\)#\s?$'
  enable: {}
  init: ["terminal length 0", "terminal width 511"]
  pager: {pattern: '--More--', key: " "}
  errors: ['^% ']
  answers:
    - {prompt: 'Destination filename \[[^\]]*\]\?\s*$', answer: ""}
  unsolicited: ['%[A-Z0-9_]+-[0-7]-[A-Z0-9_]+: ']
  timeout_s: 2
config:
  syntax: ios
  enter: [configure terminal]
  exit: [end]
  save: [copy running-config startup-config]
  running: show running-config
  ignore: ['^Building configuration', '^Current configuration', '^\s*$']
  errors: ['^% ']
"#;

    pub fn platform() -> Arc<Platform> {
        let mut registry = crate::profile::Registry::empty();
        registry
            .add(IOS_PROFILE, crate::profile::Origin::Builtin)
            .unwrap()
    }

    pub fn credentials() -> Credentials {
        Credentials {
            username: Some("admin".into()),
            password: Some("pw".into()),
            enable_secret: Some("secret".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    async fn open(device: FakeDevice) -> CliSession {
        CliSession::start(
            Box::new(device),
            platform(),
            &credentials(),
            &SessionOptions::default(),
            None,
        )
        .await
        .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn logs_in_enables_and_runs_commands() {
        let mut session = open(FakeDevice::ssh("SW1")).await;
        assert_eq!(session.mode(), Mode::Privileged);
        assert_eq!(session.hostname(), "SW1");
        assert_eq!(session.prompt(), "SW1#");

        let clock = session.exec("show clock").await.unwrap();
        assert_eq!(clock.output, "*12:00:00.000 UTC Mon Mar 1 1993");
        assert_eq!(clock.error, None);

        let vlans = session.exec("show vlan brief").await.unwrap();
        assert_eq!(
            vlans.output.lines().count(),
            6,
            "output is complete and free of echo/prompt:\n{}",
            vlans.output
        );
        assert!(
            vlans.output.starts_with("VLAN Name") && vlans.output.ends_with("30   Voice   active")
        );

        let bad = session.exec("show nonsense").await.unwrap();
        assert_eq!(
            bad.error.as_deref(),
            Some("% Invalid input detected at '^' marker.")
        );
    }

    #[tokio::test(start_paused = true)]
    async fn console_login_wakes_the_line_and_authenticates() {
        let mut session = open(FakeDevice::console("EDGE-RTR")).await;
        assert_eq!(
            (session.hostname(), session.mode()),
            ("EDGE-RTR", Mode::Privileged)
        );
        assert_eq!(
            session.exec("show clock").await.unwrap().output,
            "*12:00:00.000 UTC Mon Mar 1 1993"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn wrong_password_is_an_auth_failure_with_the_device_message() {
        let mut device = FakeDevice::console("SW1");
        device.password = "different".into();
        let result = CliSession::start(
            Box::new(device),
            platform(),
            &credentials(),
            &SessionOptions::default(),
            None,
        )
        .await;
        match result {
            Err(DriverError::AuthFailed(reason)) => {
                assert!(reason.contains("Login invalid"), "{reason}")
            }
            other => panic!("expected an auth failure, got {:?}", other.map(|_| ())),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn pager_is_handled_when_paging_cannot_be_disabled() {
        let options = SessionOptions {
            init: false,
            ..SessionOptions::default()
        };
        let mut device = FakeDevice::ssh("SW1");
        device.dribble = true;
        let mut session =
            CliSession::start(Box::new(device), platform(), &credentials(), &options, None)
                .await
                .unwrap();
        let vlans = session.exec("show vlan brief").await.unwrap();
        assert_eq!(vlans.output.lines().count(), 6, "{:?}", vlans.output);
        assert!(!vlans.output.contains("More"));
        let config = session.fetch_config(false).await.unwrap();
        assert_eq!(config, "!\nhostname X\n!\nvlan 10\n name Staff\n!\nend\n");
    }

    #[tokio::test(start_paused = true)]
    async fn missing_or_wrong_enable_secret() {
        let no_secret = Credentials {
            enable_secret: None,
            ..credentials()
        };
        let mut session = CliSession::start(
            Box::new(FakeDevice::ssh("SW1")),
            platform(),
            &no_secret,
            &SessionOptions::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(session.mode(), Mode::User);
        assert!(session.warnings()[0].contains("no enable secret"));
        // Unprivileged: the device refuses, and the driver reports it rather than hiding it.
        assert!(matches!(
            session.fetch_config(false).await,
            Err(DriverError::Rejected { .. })
        ));

        let wrong = Credentials {
            enable_secret: Some("nope".into()),
            ..credentials()
        };
        let result = CliSession::start(
            Box::new(FakeDevice::ssh("SW1")),
            platform(),
            &wrong,
            &SessionOptions::default(),
            None,
        )
        .await;
        assert!(
            matches!(result, Err(DriverError::EnableFailed(ref reason)) if reason.contains("Bad secrets")),
            "{:?}",
            result.map(|_| ())
        );
    }

    #[tokio::test(start_paused = true)]
    async fn configuration_stops_at_the_first_rejected_line_and_leaves_config_mode() {
        let mut session = open(FakeDevice::ssh("SW1")).await;
        let lines: Vec<String> = [
            "vlan 30",
            "name Voice",
            "interface Gi0/1",
            "bogus command",
            "description never sent",
        ]
        .map(String::from)
        .to_vec();
        let result = session.configure(&lines).await.unwrap();
        assert!(!result.ok());
        assert_eq!(result.failed_at, Some(3));
        assert_eq!(result.lines.len(), 4);
        assert_eq!(
            result.lines[3].error.as_deref(),
            Some("% Invalid input detected at '^' marker.")
        );
        assert_eq!(
            session.mode(),
            Mode::Privileged,
            "must not be left in config mode"
        );

        let good = session
            .configure(&["vlan 40".to_string(), "name Lab".to_string()])
            .await
            .unwrap();
        assert!(good.ok() && good.lines.len() == 2);
    }

    #[tokio::test(start_paused = true)]
    async fn hostname_change_and_interactive_save() {
        let mut session = open(FakeDevice::ssh("SW1")).await;
        let result = session
            .configure(&["hostname CORE-SW".to_string()])
            .await
            .unwrap();
        assert!(result.ok());
        assert_eq!(session.hostname(), "CORE-SW");
        assert_eq!(
            session.exec("show clock").await.unwrap().output,
            "*12:00:00.000 UTC Mon Mar 1 1993"
        );

        // `copy run start` asks "Destination filename [startup-config]?" — answered from the profile.
        let saved = session.save().await.unwrap();
        assert_eq!(saved.len(), 1);
        assert!(saved[0].output.contains("[OK]"), "{:?}", saved[0].output);
        assert_eq!(saved[0].error, None);
    }

    #[tokio::test(start_paused = true)]
    async fn silence_times_out_and_a_closed_session_is_reported() {
        let mut session = open(FakeDevice::ssh("SW1")).await;
        match session.exec("hang").await {
            Err(DriverError::Timeout {
                waiting_for,
                seconds,
                ..
            }) => {
                assert_eq!(seconds, 2);
                assert!(waiting_for.contains("hang"));
            }
            other => panic!("expected a timeout, got {other:?}"),
        }
        assert!(matches!(
            session.exec("exit").await,
            Err(DriverError::Closed { .. })
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn transcript_masks_secrets() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let session = CliSession::start(
            Box::new(FakeDevice::console("SW1")),
            platform(),
            &credentials(),
            &SessionOptions::default(),
            Some(tx),
        )
        .await
        .unwrap();
        drop(session);
        let mut sent = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            if let Transcript::Sent { text } = entry {
                sent.push(text);
            }
        }
        assert!(sent.contains(&"admin".to_string()) && sent.contains(&"enable".to_string()));
        assert!(
            sent.iter().all(|text| text != "pw" && text != "secret"),
            "{sent:?}"
        );
        assert_eq!(
            sent.iter()
                .filter(|text| text.as_str() == "********")
                .count(),
            2
        );
    }

    #[tokio::test(start_paused = true)]
    async fn log_messages_after_the_prompt_do_not_hide_it() {
        let mut device = FakeDevice::ssh("SW1");
        device.console_logging = true;
        let mut session = open(device).await;
        // The device answers `end` with its prompt and, in the same breath, a log line:
        // the prompt is no longer the last thing on the screen.
        let result = session
            .configure(&["vlan 40".to_string(), "name Lab".to_string()])
            .await
            .unwrap();
        assert!(result.ok());
        assert_eq!(
            (session.mode(), session.prompt()),
            (Mode::Privileged, "SW1#")
        );
        let clock = session.exec("show clock").await.unwrap();
        assert_eq!(
            clock.output, "*12:00:00.000 UTC Mon Mar 1 1993",
            "the log line is not taken for output of the next command"
        );
    }

    /// A profile that knows nothing about the device: the default prompt, no enable, no init.
    fn permissive() -> Arc<Platform> {
        let mut registry = crate::profile::Registry::empty();
        registry
            .add(
                "id: test_generic\nname: Generic\ncli:\n  timeout_s: 2\n",
                crate::profile::Origin::Builtin,
            )
            .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn rebinding_continues_the_session_with_another_profile() {
        let mut session = CliSession::start(
            Box::new(FakeDevice::ssh("SW1")),
            permissive(),
            &credentials(),
            &SessionOptions::default(),
            None,
        )
        .await
        .unwrap();
        assert_eq!(session.platform().id(), "test_generic");
        assert_eq!(session.prompt(), "SW1>");
        assert_eq!(session.login_screen(), "Authorised access only!");
        // The permissive profile neither enables nor turns paging off, but it can run commands.
        assert_eq!(
            session.exec("show clock").await.unwrap().output,
            "*12:00:00.000 UTC Mon Mar 1 1993"
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        session.set_tap(Some(tx));
        session
            .rebind(platform(), &credentials(), &SessionOptions::default())
            .await
            .unwrap();
        assert_eq!(session.platform().id(), "test_ios");
        assert_eq!(
            (session.mode(), session.hostname(), session.prompt()),
            (Mode::Privileged, "SW1", "SW1#")
        );
        let mut sent = Vec::new();
        while let Ok(entry) = rx.try_recv() {
            if let Transcript::Sent { text } = entry {
                sent.push(text);
            }
        }
        assert_eq!(
            sent,
            [
                "enable",
                "********",
                "terminal length 0",
                "terminal width 511"
            ],
            "privileged mode and the init commands of the new profile"
        );
        let config = session.fetch_config(false).await.unwrap();
        assert!(config.starts_with("!\nhostname X\n"), "{config}");

        session.sync().await.unwrap();
        assert_eq!(session.prompt(), "SW1#");
    }

    #[tokio::test(start_paused = true)]
    async fn rebinding_to_a_profile_that_does_not_fit_keeps_the_session_usable() {
        let mut session = CliSession::start(
            Box::new(FakeDevice::ssh("SW1")),
            permissive(),
            &credentials(),
            &SessionOptions::default(),
            None,
        )
        .await
        .unwrap();
        let mut registry = crate::profile::Registry::empty();
        let routeros_like = registry
            .add(
                "id: odd\nname: Odd\ncli:\n  prompt: '^\\[\\w+@(?P<host>\\w+)\\] > $'\n  timeout_s: 2\n  init: ['never sent']\n",
                crate::profile::Origin::Builtin,
            )
            .unwrap();
        match session
            .rebind(routeros_like, &credentials(), &SessionOptions::default())
            .await
        {
            Err(DriverError::Timeout { waiting_for, .. }) => assert_eq!(
                waiting_for,
                "waiting for a prompt that matches the `odd` profile; the device shows `SW1>`"
            ),
            other => panic!("expected a timeout, got {other:?}"),
        }
        assert_eq!(session.platform().id(), "test_generic");
        assert_eq!(session.hostname(), "SW1");
        assert_eq!(
            session.exec("show clock").await.unwrap().output,
            "*12:00:00.000 UTC Mon Mar 1 1993"
        );

        // A wrong enable secret is reported, and the session stays on the profile that fitted.
        let wrong = Credentials {
            enable_secret: Some("nope".into()),
            ..credentials()
        };
        let result = session
            .rebind(platform(), &wrong, &SessionOptions::default())
            .await;
        assert!(
            matches!(result, Err(DriverError::EnableFailed(_))),
            "{result:?}"
        );
        assert_eq!(session.platform().id(), "test_ios");
        assert_eq!(session.mode(), Mode::User);
    }

    #[test]
    fn echo_stripping_and_host_matching() {
        let lines = ["show clock", "*12:00", ""].map(String::from).to_vec();
        assert_eq!(strip_echo(lines, "show clock"), "*12:00");
        let wrapped = ["SW1#show clock", "", "12:00"].map(String::from).to_vec();
        assert_eq!(strip_echo(wrapped, "show clock"), "12:00");
        assert_eq!(
            strip_echo(vec!["unrelated".into()], "show clock"),
            "unrelated"
        );
        assert!(same_host("SW1", "SW1"));
        assert!(same_host(
            "VeryLongHostnameTruncat",
            "VeryLongHostnameTruncated-01"
        ));
        assert!(!same_host("SW2", "SW1"));
    }
}
