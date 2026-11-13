//! Notification channels: where an alert goes besides the screen.
//!
//! A channel's webhook URL — which for Slack, Teams and Discord *is* the
//! secret — and an SMTP password live in the vault under
//! `monitor:channel:<id>:secret`. The channel document holds the kind, the
//! name and, for e-mail, the server and addresses; nothing else. Errors are
//! written so that they never carry the URL either, so a failed delivery in
//! the event log cannot leak it.

use std::sync::OnceLock;
use std::time::Duration;

use nocturne_app::store::LogEntry;
use nocturne_app::{App, AppError, AppResult, Router, util};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::db::{channel_secret_id, collections::CHANNELS};
use crate::model::{Alert, AlertState, Channel, ChannelKind, EmailSettings, Severity};
use crate::state::MonitorState;

/// Attempts per notification, the first included.
const ATTEMPTS: u32 = 3;
/// How long to wait before each retry.
const BACKOFF: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(5)];
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const SMTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Firing,
    Resolved,
}

impl Stage {
    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Firing => "firing",
            Stage::Resolved => "resolved",
        }
    }
}

// ----------------------------------------------------------------- payloads

fn colour(alert: &Alert, stage: Stage) -> &'static str {
    if stage == Stage::Resolved {
        return "#2eb886";
    }
    match alert.severity {
        Severity::Critical | Severity::High => "#cc0000",
        Severity::Medium => "#d99000",
        _ => "#3aa3e3",
    }
}

/// The one-line summary every channel starts from.
pub fn headline(alert: &Alert, stage: Stage) -> String {
    let what = match stage {
        Stage::Firing => "ALERT",
        Stage::Resolved => "RESOLVED",
    };
    match &alert.device {
        Some(device) => format!(
            "[{what}] {} — {} ({})",
            device,
            alert.title,
            alert.severity.as_str()
        ),
        None => format!("[{what}] {} ({})", alert.title, alert.severity.as_str()),
    }
}

fn facts(alert: &Alert) -> Vec<(String, String)> {
    let mut facts = Vec::new();
    if let Some(device) = &alert.device {
        facts.push(("Device".into(), device.clone()));
    }
    if let Some(instance) = &alert.instance {
        facts.push(("Instance".into(), instance.clone()));
    }
    facts.push(("Severity".into(), alert.severity.as_str().into()));
    facts.push(("Rule".into(), alert.rule_name.clone()));
    if let Some(value) = alert.value {
        facts.push(("Value".into(), format!("{value:.2}")));
    }
    facts.push(("Started".into(), stamp(alert.started_at)));
    if alert.count > 1 {
        facts.push(("Seen".into(), format!("{} times", alert.count)));
    }
    if alert.lab {
        facts.push(("Lab".into(), "yes".into()));
    }
    facts
}

fn stamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|time| time.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// What one channel kind posts.
pub fn body(kind: ChannelKind, alert: &Alert, stage: Stage) -> Value {
    let text = headline(alert, stage);
    match kind {
        // The generic webhook gets the alert itself, so that whatever is on
        // the other end can do its own routing.
        ChannelKind::Webhook => json!({
            "event": stage.as_str(),
            "text": text,
            "alert": alert,
            "source": "nocturne",
        }),
        ChannelKind::Slack => json!({
            "text": text,
            "attachments": [{
                "color": colour(alert, stage),
                "title": alert.title,
                "text": alert.detail,
                "fields": facts(alert).into_iter().map(|(title, value)| json!({
                    "title": title, "value": value, "short": true
                })).collect::<Vec<_>>(),
                "ts": alert.last_seen_at / 1000,
            }],
        }),
        ChannelKind::Teams => json!({
            "@type": "MessageCard",
            "@context": "https://schema.org/extensions",
            "themeColor": colour(alert, stage).trim_start_matches('#'),
            "summary": text,
            "title": text,
            "sections": [{
                "text": alert.detail,
                "facts": facts(alert).into_iter().map(|(name, value)| json!({
                    "name": name, "value": value
                })).collect::<Vec<_>>(),
            }],
        }),
        ChannelKind::Discord => json!({
            "content": text,
            "embeds": [{
                "title": alert.title,
                "description": alert.detail,
                "color": i64::from_str_radix(colour(alert, stage).trim_start_matches('#'), 16)
                    .unwrap_or(0),
                "fields": facts(alert).into_iter().map(|(name, value)| json!({
                    "name": name, "value": value, "inline": true
                })).collect::<Vec<_>>(),
            }],
        }),
        // E-mail is text, not JSON; this is what the body is built from.
        ChannelKind::Email => json!({"subject": text, "body": email_body(alert, stage)}),
    }
}

pub fn email_body(alert: &Alert, stage: Stage) -> String {
    let mut text = String::new();
    text.push_str(&headline(alert, stage));
    text.push_str("\n\n");
    if !alert.detail.is_empty() {
        text.push_str(&alert.detail);
        text.push_str("\n\n");
    }
    for (name, value) in facts(alert) {
        text.push_str(&format!("{name}: {value}\n"));
    }
    if stage == Stage::Resolved
        && let Some(at) = alert.resolved_at
    {
        text.push_str(&format!("Resolved: {}\n", stamp(at)));
    }
    text.push_str("\n-- \nSent by Nocturne.\n");
    text
}

// ----------------------------------------------------------------- delivery

fn http() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let _ = crate::crypto_provider();
        reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!("nocturne/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default()
    })
}

/// The secret of a channel: its URL, or its SMTP password.
fn secret(app: &App, channel: &Channel) -> AppResult<Option<String>> {
    Ok(app
        .vault()
        .get(&channel_secret_id(&channel.id))?
        .map(|secret| secret.to_string()))
}

/// One attempt. The error never contains the URL.
pub async fn deliver(
    app: &App,
    channel: &Channel,
    alert: &Alert,
    stage: Stage,
) -> Result<(), String> {
    let secret = secret(app, channel)
        .map_err(|err| err.message)?
        .filter(|secret| !secret.is_empty());
    match channel.kind {
        ChannelKind::Email => {
            let settings = channel
                .email
                .clone()
                .ok_or("this e-mail channel has no server set")?;
            let mail = crate::smtp::Mail {
                host: settings.host,
                port: if settings.port == 0 {
                    587
                } else {
                    settings.port
                },
                starttls: if settings.starttls.is_empty() {
                    "auto".into()
                } else {
                    settings.starttls
                },
                username: settings.username,
                password: secret,
                from: settings.from,
                to: settings.to,
                subject: headline(alert, stage),
                body: email_body(alert, stage),
            };
            crate::smtp::send(&mail, SMTP_TIMEOUT).await
        }
        kind => {
            let url = secret.ok_or("this channel has no URL set")?;
            let response = http()
                .post(&url)
                .json(&body(kind, alert, stage))
                .send()
                .await
                .map_err(|err| describe(&err))?;
            let status = response.status();
            if status.is_success() {
                return Ok(());
            }
            let detail = response
                .text()
                .await
                .unwrap_or_default()
                .chars()
                .take(200)
                .collect::<String>();
            Err(format!(
                "the channel answered HTTP {}{}",
                status.as_u16(),
                if detail.trim().is_empty() {
                    String::new()
                } else {
                    format!(": {}", detail.trim())
                }
            ))
        }
    }
}

/// Reqwest's own message carries the URL, so the failure is described from
/// its kind instead.
fn describe(error: &reqwest::Error) -> String {
    if error.is_timeout() {
        "the channel did not answer in time".into()
    } else if error.is_connect() {
        "the channel could not be reached".into()
    } else if error.is_redirect() {
        "the channel redirected too many times".into()
    } else if error.is_body() || error.is_decode() {
        "the channel's answer could not be read".into()
    } else {
        "the request to the channel failed".into()
    }
}

/// Deliver with back-off. Returns the last error if every attempt failed.
pub async fn deliver_with_retries(
    app: &App,
    channel: &Channel,
    alert: &Alert,
    stage: Stage,
) -> Result<u32, String> {
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            tokio::time::sleep(BACKOFF[(attempt as usize - 1).min(BACKOFF.len() - 1)]).await;
        }
        match deliver(app, channel, alert, stage).await {
            Ok(()) => return Ok(attempt + 1),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// The channels an alert goes to: the ones the rule names, or every enabled
/// channel when it names none.
pub fn targets(app: &App, wanted: &[String]) -> AppResult<Vec<Channel>> {
    let all: Vec<Channel> = app.store().list(CHANNELS)?;
    Ok(all
        .into_iter()
        .filter(|channel| channel.enabled)
        .filter(|channel| wanted.is_empty() || wanted.contains(&channel.id))
        .collect())
}

/// Send an alert to its channels, in the background: a slow webhook must
/// never hold up polling.
pub fn send(app: &App, alert: &Alert, stage: Stage, wanted: &[String]) {
    let now = app.state::<MonitorState>().now();
    if alert.muted(now) {
        // Muted by a maintenance window or a snooze: recorded, not sent.
        return;
    }
    let channels = match targets(app, wanted) {
        Ok(channels) => channels,
        Err(err) => {
            tracing::warn!(%err, "notification channels could not be read");
            return;
        }
    };
    for channel in channels {
        if alert.severity < channel.min_severity {
            continue;
        }
        let app = app.clone();
        let alert = alert.clone();
        app.clone().spawn("monitor.notify", async move {
            let outcome = deliver_with_retries(&app, &channel, &alert, stage).await;
            record(&app, &channel, &alert, stage, outcome);
        });
    }
}

fn record(app: &App, channel: &Channel, alert: &Alert, stage: Stage, outcome: Result<u32, String>) {
    let now = app.state::<MonitorState>().now();
    let mut stored: Channel = match app.store().get(CHANNELS, &channel.id) {
        Ok(Some(stored)) => stored,
        _ => return,
    };
    match &outcome {
        Ok(attempts) => {
            stored.last_sent_at = Some(now);
            stored.last_error = None;
            tracing::debug!(channel = %stored.name, attempts, "notification sent");
        }
        Err(error) => {
            stored.last_error = Some(error.clone());
            let _ = app.store().log_append(&LogEntry {
                id: 0,
                ts: now,
                stream: "event".into(),
                device_id: alert.device_id.clone(),
                severity: Some(4),
                source: "monitor".into(),
                message: format!(
                    "The notification channel `{}` could not be reached after {ATTEMPTS} attempts: {error}",
                    stored.name
                ),
                body: Some(json!({
                    "channel_id": stored.id,
                    "kind": stored.kind,
                    "alert_id": alert.id,
                    "stage": stage.as_str(),
                })),
            });
        }
    }
    let _ = app.store().put(CHANNELS, &stored.id, &stored);
    app.publish(
        "monitor.status_changed",
        &json!({"what": "channel", "channel_id": stored.id, "ok": outcome.is_ok()}),
    );
}

// -------------------------------------------------------------------- RPC

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct SaveParams {
    pub id: Option<String>,
    pub name: String,
    pub kind: ChannelKind,
    pub enabled: Option<bool>,
    pub min_severity: Option<Severity>,
    pub email: Option<EmailSettings>,
    /// The webhook URL. Absent leaves what is stored; an empty string clears it.
    pub url: Option<String>,
    /// The SMTP password, same rules.
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct IdParams {
    pub id: String,
}

fn validate(params: &SaveParams) -> AppResult<()> {
    if params.name.trim().is_empty() {
        return Err(AppError::invalid("give the notification channel a name"));
    }
    if let Some(url) = &params.url
        && !url.is_empty()
        && !(url.starts_with("https://") || url.starts_with("http://"))
    {
        return Err(AppError::invalid(
            "a webhook URL has to start with https:// (or http:// on your own network)",
        ));
    }
    if params.kind == ChannelKind::Email {
        let email = params
            .email
            .as_ref()
            .ok_or_else(|| AppError::invalid("an e-mail channel needs a server and addresses"))?;
        if email.host.trim().is_empty() {
            return Err(AppError::invalid("give the e-mail channel an SMTP server"));
        }
        if email.from.trim().is_empty() {
            return Err(AppError::invalid(
                "give the e-mail channel a sender address",
            ));
        }
        if email.to.iter().all(|address| address.trim().is_empty()) {
            return Err(AppError::invalid(
                "give the e-mail channel at least one recipient",
            ));
        }
        if !["", "auto", "required", "never"].contains(&email.starttls.as_str()) {
            return Err(AppError::invalid(
                "STARTTLS is `auto`, `required` or `never`",
            ));
        }
    }
    Ok(())
}

pub fn save(app: &App, params: SaveParams) -> AppResult<Channel> {
    validate(&params)?;
    let now = app.state::<MonitorState>().now();
    let existing = match &params.id {
        Some(id) if !id.is_empty() => {
            Some(app.store().require::<Channel>(CHANNELS, id, "channel")?)
        }
        _ => None,
    };
    let id = existing
        .as_ref()
        .map(|channel| channel.id.clone())
        .unwrap_or_else(util::new_id);

    // The secret goes straight into the vault and never into the document.
    let incoming = if params.kind == ChannelKind::Email {
        params.password.clone()
    } else {
        params.url.clone()
    };
    if let Some(secret) = incoming {
        if secret.is_empty() {
            app.vault().delete(&channel_secret_id(&id))?;
        } else {
            app.vault().put(&channel_secret_id(&id), &secret)?;
        }
    }
    let has_secret = app.vault().get(&channel_secret_id(&id))?.is_some();
    if params.kind.is_webhook() && !has_secret {
        return Err(AppError::invalid(
            "a webhook channel needs the URL it posts to",
        ));
    }

    let channel = Channel {
        id,
        name: params.name.trim().to_string(),
        kind: params.kind,
        enabled: params
            .enabled
            .or(existing.as_ref().map(|channel| channel.enabled))
            .unwrap_or(true),
        min_severity: params
            .min_severity
            .or(existing.as_ref().map(|channel| channel.min_severity))
            .unwrap_or(Severity::Info),
        email: params
            .email
            .or_else(|| existing.as_ref().and_then(|channel| channel.email.clone())),
        has_secret,
        last_sent_at: existing.as_ref().and_then(|channel| channel.last_sent_at),
        last_error: existing
            .as_ref()
            .and_then(|channel| channel.last_error.clone()),
        created_at: existing
            .as_ref()
            .map(|channel| channel.created_at)
            .unwrap_or(now),
        updated_at: now,
    };
    app.store().put(CHANNELS, &channel.id, &channel)?;
    app.audit(
        "notify.channel.save",
        None,
        format!("Saved the notification channel `{}`", channel.name),
        json!({"id": channel.id, "kind": channel.kind}),
    );
    app.publish(
        "monitor.status_changed",
        &json!({"what": "channel", "channel_id": channel.id}),
    );
    Ok(channel)
}

/// The alert a test notification carries: clearly marked as one.
pub fn test_alert(now: i64) -> Alert {
    Alert {
        id: "test".into(),
        rule_name: "notification test".into(),
        severity: Severity::Info,
        state: AlertState::Firing,
        title: "Nocturne test notification".into(),
        detail: "This is a test of this notification channel. No device is in trouble.".into(),
        started_at: now,
        last_seen_at: now,
        count: 1,
        dedup_key: "monitor:test".into(),
        ..Alert::default()
    }
}

pub fn register(router: &mut Router) {
    router.add(
        "notify.channels.list",
        |app: App, _: nocturne_app::NoParams| async move {
            let mut channels: Vec<Channel> = app.store().list(CHANNELS)?;
            channels.sort_by_key(|channel| channel.name.to_lowercase());
            Ok(channels)
        },
    );
    router.add(
        "notify.channels.save",
        |app: App, params: SaveParams| async move { save(&app, params) },
    );
    router.add(
        "notify.channels.delete",
        |app: App, params: IdParams| async move {
            let channel: Channel = app.store().require(CHANNELS, &params.id, "channel")?;
            app.store().delete(CHANNELS, &params.id)?;
            app.vault().delete(&channel_secret_id(&params.id))?;
            app.audit(
                "notify.channel.delete",
                None,
                format!("Deleted the notification channel `{}`", channel.name),
                json!({"id": channel.id}),
            );
            app.publish(
                "monitor.status_changed",
                &json!({"what": "channel", "channel_id": channel.id}),
            );
            Ok(json!({"deleted": true}))
        },
    );
    router.add(
        "notify.channels.test",
        |app: App, params: IdParams| async move {
            let channel: Channel = app.store().require(CHANNELS, &params.id, "channel")?;
            let now = app.state::<MonitorState>().now();
            let alert = test_alert(now);
            let outcome = deliver_with_retries(&app, &channel, &alert, Stage::Firing).await;
            record(&app, &channel, &alert, Stage::Firing, outcome.clone());
            app.audit(
                "notify.channel.test",
                None,
                format!("Sent a test message to `{}`", channel.name),
                json!({"id": channel.id, "ok": outcome.is_ok()}),
            );
            match outcome {
                Ok(attempts) => Ok(json!({"sent": true, "attempts": attempts})),
                Err(error) => Err(AppError::unavailable(error)),
            }
        },
    );
}
