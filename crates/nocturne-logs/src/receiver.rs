//! The supervisor: every receiver follows the `logs` settings section.
//!
//! A receiver is something that listens on a socket — syslog
//! ([`crate::syslog`]) and SNMP traps ([`crate::traps`]), which turn what
//! arrives into log rows, and the flow collector ([`crate::flows`]), which
//! only counts. The supervisor knows nothing about any of them
//! beyond "apply the settings": each receiver's `apply` compares the
//! settings with what it is running, does nothing when they are the same,
//! and otherwise stops and starts itself, keeping a bind failure where
//! `logs.status` can show it rather than returning it.
//!
//! **Where a new receiver plugs in** (the trap receiver is the worked
//! example of one that stores; the flow collector does steps 1, 2, 3 and
//! 5, and step 4 not at all — it stores nothing, so nothing of it is
//! tailed, forwarded or shown to a rule):
//!
//! 1. its settings are a `#[serde(default)]` block in `settings::LogsSettings`,
//!    off by default, on an unprivileged port;
//! 2. its state is a block beside `state::SyslogState` in `state::LogsState`:
//!    what holds its socket, a `ReceiverCounters`, a bounded queue;
//! 3. its module has an `apply(&App)` like `syslog::apply`, and [`apply`]
//!    below calls it — that one line is what makes it follow the settings
//!    and `logs.restart`;
//! 4. what it receives becomes `db::Prepared` rows (attributed with the
//!    index, every stored string through `guard::clean`) and goes to
//!    `ingest::commit` with its own counters; that is where the row is
//!    written, tailed and shown to the rules, and where anything that taps
//!    stored batches belongs;
//! 5. its numbers are a field of `model::LogsStatus`, filled in by
//!    `status::snapshot`.

use nocturne_app::{App, AppResult};

/// Bring every receiver in line with the settings.
///
/// Never returns an error for a port that cannot be bound: that is a state
/// of the receiver, not a failure of the call that tried to start it.
pub async fn apply(app: &App) -> AppResult<()> {
    crate::syslog::apply(app).await?;
    crate::traps::apply(app).await?;
    crate::flows::apply(app).await
}

/// Follow the settings: a change to the `logs` section is applied to every
/// receiver, and each restarts only if its own part changed.
pub fn spawn(app: &App) {
    let app = app.clone();
    tokio::spawn(async move {
        // Subscribed before the first apply, so a settings change made while
        // the application is starting cannot slip between the two and leave
        // the receiver following settings nobody asked for any more.
        let mut events = app.subscribe();
        if let Err(err) = apply(&app).await {
            tracing::error!(%err, "the receivers could not be started");
        }
        let shutdown = app.shutdown_token();
        loop {
            let event = tokio::select! {
                _ = shutdown.cancelled() => break,
                event = events.recv() => event,
            };
            match event {
                Ok(event)
                    if event.topic == "settings.changed"
                        && event.payload["section"] == crate::settings::SECTION =>
                {
                    if let Err(err) = apply(&app).await {
                        tracing::error!(%err, "the receivers could not be restarted");
                    }
                }
                Ok(event)
                    if event.topic == "devices.changed" || event.topic == "collect.updated" =>
                {
                    crate::attribute::invalidate(&app);
                    // A device may have been given another credential.
                    crate::traps::forget(&app);
                }
                // What the trap receiver compares a community with is read
                // from the vault and kept for a few seconds; a changed
                // credential, or a vault that was locked, must count at once.
                Ok(event)
                    if event.topic == "credentials.changed" || event.topic == "vault.changed" =>
                {
                    crate::traps::forget(&app);
                }
                Ok(_) => {}
                // Falling behind the bus loses events, not correctness: the
                // next one still restarts the receivers. What was missed may
                // have been a changed device or credential, so what is
                // remembered of those is let go.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    crate::attribute::invalidate(&app);
                    crate::traps::forget(&app);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
