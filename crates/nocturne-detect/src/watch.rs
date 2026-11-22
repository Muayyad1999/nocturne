//! Listening: which event wakes which detection.
//!
//! Two tasks. The listener takes every event off the bus and keeps only the
//! handful of topics a detection is decided from, so it never falls behind
//! however busy a terminal is. The judge works through what was kept, one
//! event at a time and in order — a detection asks other features questions
//! (`changes.list`, `maintenance.list`), and two events judged at once could
//! both count the same audit entries.

use serde_json::{Value, json};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::mpsc;

use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_app::{App, Event};

use crate::catalogue::{
    self, FAILED_UNLOCKS, HOST_KEY_CHANGED, PROTECTED_CHANGE_OUTSIDE_WINDOW, SECRETS_CHANGED,
    UNPLANNED_CONFIG_CHANGE, UNPLANNED_RELOAD, UNREDACTED_CONFIG_VIEWS,
};
use crate::checks::{self, audit};
use crate::model::Finding;
use crate::settings::{self, Settings};
use crate::sources::{self, Gap};
use crate::{DetectState, hits, rpc};

/// Events waiting to be judged. They are rare (a configuration change, an
/// audit entry of six particular kinds), so a queue this long that fills up
/// means the judge is stuck, and the listener says so rather than wait.
const QUEUE: usize = 1_024;

/// A marker the behaviour tests send down the same path as every other
/// event. Both tasks are first-in first-out, so when the judge reaches the
/// marker everything published before it has been judged — which is the only
/// way a test can tell "stayed silent" from "has not got to it yet".
#[cfg(test)]
pub(crate) const SYNC: &str = "detect.test_sync";
#[cfg(test)]
pub(crate) const SYNCED: &str = "detect.test_synced";

pub fn start(app: &App, events: broadcast::Receiver<Event>) {
    let (queue, work) = mpsc::channel::<Event>(QUEUE);
    app.spawn("detect.listen", listen(app.clone(), events, queue));
    app.spawn("detect.judge", judge(app.clone(), work));
}

/// Whether any detection is decided from this event.
fn wanted(event: &Event) -> bool {
    match event.topic.as_str() {
        "logs.config_changed"
        | "config.changed"
        | "connect.host_key_changed"
        | "changes.changed" => true,
        "logs.signal" => event.payload["kind"] == "reload",
        "audit.appended" => event.payload["source"]
            .as_str()
            .is_some_and(audit::is_watched),
        "settings.changed" => event.payload["section"] == settings::SECTION,
        #[cfg(test)]
        SYNC => true,
        _ => false,
    }
}

async fn listen(app: App, mut events: broadcast::Receiver<Event>, queue: mpsc::Sender<Event>) {
    loop {
        match events.recv().await {
            Ok(event) => {
                if wanted(&event) && queue.try_send(event).is_err() {
                    missed(
                        &app,
                        "an event was not judged because the detections were still busy with earlier ones",
                    );
                }
            }
            Err(RecvError::Lagged(count)) => {
                tracing::debug!(count, "the detections' listener fell behind the event bus");
                missed(
                    &app,
                    "events passed while Nocturne was too busy to read them; one of them may have been something this detection is decided from",
                );
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// An event that was lost cannot be judged afterwards, and a detection that
/// may have missed one must not look as if it had seen everything.
///
/// `message` says the same thing however many were lost, so a flood that
/// drops ten thousand events is said once — in the log and to the interface
/// — until a check goes through again, not ten thousand times.
fn missed(app: &App, message: &str) {
    let state = app.state::<DetectState>();
    let at = now_ms();
    let mut news = false;
    for detection in &catalogue::CATALOGUE {
        news |= state.check_failed(detection.id, at, message.to_string());
    }
    if news {
        tracing::warn!(message, "detections missed events");
        app.publish("detections.changed", &json!({}));
    }
}

async fn judge(app: App, mut work: mpsc::Receiver<Event>) {
    while let Some(event) = work.recv().await {
        // Each event is judged in a task of its own and waited for, so the
        // order stays what it was. A check calls other features' handlers
        // directly (`App::rpc` runs them in the calling task), and a panic
        // in one of them — or in this crate — would otherwise end the
        // judge: every detection would then stay silent until a restart,
        // looking exactly like a quiet network.
        let topic = event.topic.clone();
        let judging = tokio::spawn({
            let app = app.clone();
            async move { handle(&app, &event).await }
        });
        if let Err(err) = judging.await
            && err.is_panic()
        {
            tracing::error!(
                topic,
                "a detection stopped unexpectedly while judging an event"
            );
            missed(
                &app,
                "a check stopped unexpectedly, so what it was judging was not judged; this is a fault in Nocturne, and its log says where",
            );
        }
    }
}

/// Whether a detection is switched on and this build can run it.
fn runs(app: &App, settings: &Settings, id: &str) -> bool {
    settings.is_on(id) && catalogue::unavailable_reason(id, &sources::features(app)).is_none()
}

async fn handle(app: &App, event: &Event) {
    let settings = Settings::load(app);
    let payload = &event.payload;
    match event.topic.as_str() {
        "settings.changed" => rpc::announce(app, &settings, None),
        "logs.config_changed" if runs(app, &settings, UNPLANNED_CONFIG_CHANGE) => {
            let checked = checks::config_change::from_log(app, &settings, payload, event.at).await;
            conclude(app, UNPLANNED_CONFIG_CHANGE, checked).await;
        }
        "config.changed" if runs(app, &settings, UNPLANNED_CONFIG_CHANGE) => {
            let checked = checks::config_change::from_backup(app, &settings, payload).await;
            conclude(app, UNPLANNED_CONFIG_CHANGE, checked).await;
        }
        "logs.signal" if runs(app, &settings, UNPLANNED_RELOAD) => {
            let checked = checks::reload::check(app, &settings, payload, event.at).await;
            conclude(app, UNPLANNED_RELOAD, checked).await;
        }
        "connect.host_key_changed" if runs(app, &settings, HOST_KEY_CHANGED) => {
            let found = checks::host_key::check(app, payload, event.at);
            conclude(app, HOST_KEY_CHANGED, Ok(found)).await;
        }
        "changes.changed" if runs(app, &settings, PROTECTED_CHANGE_OUTSIDE_WINDOW) => {
            let checked = checks::protected::check(app, &settings, payload, event.at).await;
            conclude(app, PROTECTED_CHANGE_OUTSIDE_WINDOW, checked).await;
        }
        "audit.appended" => audited(app, &settings, payload).await,
        #[cfg(test)]
        SYNC => app.publish(SYNCED, payload),
        _ => {}
    }
}

/// An audit entry of a kind some detection reads.
async fn audited(app: &App, settings: &Settings, payload: &Value) {
    let Ok(entry) = serde_json::from_value::<LogEntry>(payload.clone()) else {
        return;
    };
    match entry.source.as_str() {
        audit::VIEW_UNREDACTED if runs(app, settings, UNREDACTED_CONFIG_VIEWS) => {
            let checked = audit::unredacted_views(app, settings, &entry);
            conclude(app, UNREDACTED_CONFIG_VIEWS, checked).await;
        }
        audit::CREDENTIAL_SAVE | audit::CREDENTIAL_DELETE | audit::VAULT_SET_PASSWORD
            if runs(app, settings, SECRETS_CHANGED) =>
        {
            conclude(app, SECRETS_CHANGED, Ok(audit::secrets_changed(&entry))).await;
        }
        audit::VAULT_WRONG_PASSWORD | audit::SERVER_TOKEN_REFUSED
            if runs(app, settings, FAILED_UNLOCKS) =>
        {
            let checked = audit::failed_unlocks(app, settings, &entry);
            conclude(app, FAILED_UNLOCKS, checked).await;
        }
        _ => {}
    }
}

/// Act on what a check concluded: a hit, nothing, or "it could not be made".
async fn conclude(app: &App, detection: &'static str, checked: Result<Option<Finding>, Gap>) {
    let state = app.state::<DetectState>();
    match checked {
        Ok(found) => {
            if state.check_made(detection) {
                app.publish("detections.changed", &json!({ "id": detection }));
            }
            if let Some(finding) = found {
                hits::record(app, finding).await;
            }
        }
        // Not being able to rule the pipeline out is not the same as having
        // ruled it out: nobody is accused, and the list says the check was
        // not made.
        // Said once while it keeps failing the same way: a source that is
        // down during a burst of events is one piece of news, not one each.
        Err(gap) => {
            if state.check_failed(detection, now_ms(), gap.sentence()) {
                tracing::warn!(
                    detection,
                    reason = gap.sentence(),
                    "a detection could not be checked"
                );
                app.publish("detections.changed", &json!({ "id": detection }));
            }
        }
    }
}
