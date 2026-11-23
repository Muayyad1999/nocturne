//! System notifications for alerts that fire while the window is away.
//!
//! The rule is deliberately narrow: only the firing notice of an alert, only
//! the two worst severities, only when the window is not in front, and at
//! most one every ten seconds. An operations tool that interrupts its user
//! for everything is one whose notifications get turned off.
//!
//! Which change to an alert *is* its firing notice is not decided here. The
//! monitor decides it — once, for the webhooks, the mail and this — and says
//! so in the event (`notification`, `docs/dev/features/monitor.md`). The
//! shell cannot see maintenance windows, and guessing from the alert itself
//! ("firing, and seen once, so it is new") is wrong three ways: it announces
//! an outage inside a maintenance window that every channel kept quiet
//! about, announces an alert again when somebody snoozes it, and never
//! announces one whose notice was held back until the window closed.

use std::time::{Duration, Instant};

use serde_json::Value;

/// The quietest gap between two notifications.
pub const MIN_GAP: Duration = Duration::from_secs(10);

/// What to show, if anything.
#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

#[derive(Debug, Default)]
pub struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    pub fn new() -> Throttle {
        Throttle::default()
    }

    /// Decide what an `alerts.changed` payload deserves. `focused` is whether
    /// the window is in front; `now` makes the gap testable.
    pub fn consider(
        &mut self,
        payload: &Value,
        focused: bool,
        now: Instant,
    ) -> Option<Notification> {
        if focused {
            return None;
        }
        // The one event on which the engine handed the firing notice to its
        // channels. Every other change to the alert — counted again,
        // snoozed, acknowledged, resolved, or raised where a maintenance
        // window covers it — carries something else here, or nothing.
        if payload.get("notification").and_then(Value::as_str) != Some("firing") {
            return None;
        }
        let alert = payload.get("alert")?;
        let severity = alert
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(severity, "critical" | "high") {
            return None;
        }
        if self
            .last
            .is_some_and(|last| now.duration_since(last) < MIN_GAP)
        {
            return None;
        }
        self.last = Some(now);
        let title = alert
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Alert")
            .to_string();
        let device = alert
            .get("device")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let body = match (severity, device) {
            (severity, "") => severity.to_string(),
            (severity, device) => format!("{device} — {severity}"),
        };
        Some(Notification { title, body })
    }
}

/// Whether what was done to a notification should bring the window forward.
/// A click on the notification itself is `tap` on every platform
/// (`tauri-plugin-notification`); these notifications offer nothing else.
pub fn brings_the_window_forward(action: &str) -> bool {
    action == "tap"
}

/// Turn the engine's alert events into notifications until it stops.
///
/// Fed from the engine's own event stream, which only the engine writes to.
/// The channel the windows listen on is not that: a script in the page may
/// `emit` on it, and when this read from there, one such call made the shell
/// announce an outage no device had reported.
///
/// Subscribed when this is called, not when the future first runs, so that
/// nothing published in between is missed.
pub fn watch(
    app: &nocturne_suite::App,
    focused: impl Fn() -> bool + Send + 'static,
    show: impl Fn(Notification) + Send + 'static,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    let mut events = app.subscribe();
    let shutdown = app.shutdown_token();
    async move {
        let mut throttle = Throttle::new();
        loop {
            let received = tokio::select! {
                biased;
                _ = shutdown.cancelled() => return,
                received = events.recv() => received,
            };
            match received {
                Ok(event) if event.topic == "alerts.changed" => {
                    if let Some(notification) =
                        throttle.consider(&event.payload, focused(), Instant::now())
                    {
                        show(notification);
                    }
                }
                Ok(_) => {}
                // An alert missed in a burst is on the Alerts screen; the
                // next one is announced as usual.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "alert notifications fell behind the event stream");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// An `alerts.changed` payload as the monitor publishes it: the alert,
    /// and which notice — if any — went to the channels with this change.
    fn alert(severity: &str, notification: Option<&str>) -> Value {
        let state = match notification {
            Some("resolved") => "resolved",
            _ => "firing",
        };
        json!({
            "alert": {"title": "Device down", "device": "CORE-SW1", "severity": severity, "state": state, "count": 1},
            "notification": notification,
        })
    }

    #[test]
    fn only_the_firing_notice_of_a_serious_alert_and_only_while_the_window_is_away() {
        let mut throttle = Throttle::new();
        let now = Instant::now();
        let firing = Some("firing");

        assert_eq!(
            throttle.consider(&alert("critical", firing), true, now),
            None,
            "the user is looking at it"
        );
        let shown = throttle
            .consider(&alert("critical", firing), false, now)
            .expect("a notification");
        assert_eq!(shown.title, "Device down");
        assert_eq!(shown.body, "CORE-SW1 — critical");

        // Within the gap, nothing more.
        assert_eq!(
            throttle.consider(&alert("high", firing), false, now + Duration::from_secs(9)),
            None
        );
        assert!(
            throttle
                .consider(&alert("high", firing), false, now + Duration::from_secs(11))
                .is_some()
        );

        let mut throttle = Throttle::new();
        for (severity, notification) in [
            ("medium", firing),
            ("low", firing),
            // The incident closing is not an interruption.
            ("critical", Some("resolved")),
            // A change the channels heard nothing about.
            ("critical", None),
        ] {
            assert_eq!(
                throttle.consider(&alert(severity, notification), false, now),
                None,
                "{severity}/{notification:?}"
            );
        }
        // A payload that says nothing about a notice is not one: an event
        // from an engine that does not send the field must not be guessed at.
        let unsaid = json!({"alert": {"title": "Device down", "severity": "critical", "state": "firing", "count": 1}});
        assert_eq!(throttle.consider(&unsaid, false, now), None);
        // Nor is a payload that is not an alert at all.
        assert_eq!(throttle.consider(&json!({"cleared": 3}), false, now), None);
        assert_eq!(
            throttle.consider(&json!({"notification": "firing"}), false, now),
            None
        );
    }

    // ------------------------------------------------------------------
    // The three below take what the engine really publishes, through the
    // one door, rather than a payload written here: the shell and the
    // monitor were each right about a payload the other never sent.

    use nocturne_suite::{App, AppConfig};

    fn wall_clock_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is past 1970")
            .as_millis() as i64
    }

    /// Every `alerts.changed` payload published since the last call.
    fn alert_events(
        events: &mut tokio::sync::broadcast::Receiver<nocturne_suite::Event>,
    ) -> Vec<Value> {
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            if event.topic == "alerts.changed" {
                seen.push(event.payload);
            }
        }
        seen
    }

    async fn raise(app: &App) -> Value {
        app.rpc(
            "alerts.raise",
            json!({"severity": "critical", "title": "Device down", "dedup_key": "core-sw1.down"}),
        )
        .await
        .expect("alerts.raise")
    }

    /// A window that is open now and covers everything.
    async fn open_maintenance_window(app: &App) -> String {
        let now = wall_clock_ms();
        let window = app
            .rpc(
                "maintenance.save",
                json!({"name": "Planned work", "starts_at": now - 60_000, "ends_at": now + 3_600_000}),
            )
            .await
            .expect("maintenance.save");
        window["id"]
            .as_str()
            .expect("a window has an id")
            .to_string()
    }

    #[tokio::test]
    async fn an_alert_raised_inside_a_maintenance_window_is_not_announced_until_the_window_is_gone()
    {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
        let mut events = app.subscribe();
        let mut throttle = Throttle::new();
        let start = Instant::now();

        let window = open_maintenance_window(&app).await;
        raise(&app).await;
        let published = alert_events(&mut events);
        assert_eq!(published.len(), 1, "{published:?}");
        // What the test stands on: the engine did hold this one back.
        assert_eq!(published[0]["alert"]["muted_by_maintenance"], true);
        assert_eq!(published[0]["alert"]["state"], "firing");
        assert_eq!(
            throttle.consider(&published[0], false, start),
            None,
            "the channels were told nothing about this alert, and neither is the person"
        );

        // The window goes; the outage is still on. The next sighting is the
        // one the engine announces, and it is not the alert's first.
        app.rpc("maintenance.delete", json!({"id": window}))
            .await
            .expect("maintenance.delete");
        raise(&app).await;
        let published = alert_events(&mut events);
        assert_eq!(published.len(), 1, "{published:?}");
        assert_eq!(published[0]["alert"]["count"], 2);
        let shown = throttle
            .consider(&published[0], false, start + MIN_GAP * 2)
            .expect("the notice that was held back is shown when the engine sends it");
        assert_eq!(shown.title, "Device down");

        // Seen again a moment later: counted, and not announced twice.
        raise(&app).await;
        let published = alert_events(&mut events);
        assert_eq!(published.len(), 1, "{published:?}");
        assert_eq!(
            throttle.consider(&published[0], false, start + MIN_GAP * 4),
            None
        );
    }

    #[tokio::test]
    async fn a_snoozed_alert_is_not_announced_and_snoozing_one_is_not_news() {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
        let mut events = app.subscribe();
        let mut throttle = Throttle::new();
        let start = Instant::now();

        // Raised where nobody is told, so that its notice is still owed.
        let window = open_maintenance_window(&app).await;
        let alert = raise(&app).await;
        app.rpc("alerts.snooze", json!({"id": alert["id"], "minutes": 60}))
            .await
            .expect("alerts.snooze");
        app.rpc("maintenance.delete", json!({"id": window}))
            .await
            .expect("maintenance.delete");
        raise(&app).await;

        let published = alert_events(&mut events);
        assert_eq!(
            published.len(),
            3,
            "raised, snoozed, seen again: {published:?}"
        );
        assert!(published[2]["alert"]["snoozed_until"].as_i64() > Some(wall_clock_ms()));
        assert_eq!(published[2]["alert"]["muted_by_maintenance"], false);
        for (step, payload) in published.iter().enumerate() {
            assert_eq!(
                throttle.consider(payload, false, start + MIN_GAP * 2 * step as u32),
                None,
                "event {step}: {payload}"
            );
        }

        // The snooze is cancelled: still nothing for the click itself, and
        // then the notice at the sighting the engine announces.
        app.rpc("alerts.snooze", json!({"id": alert["id"], "minutes": 0}))
            .await
            .expect("alerts.snooze");
        raise(&app).await;
        let published = alert_events(&mut events);
        assert_eq!(published.len(), 2, "{published:?}");
        assert_eq!(
            throttle.consider(&published[0], false, start + MIN_GAP * 10),
            None,
            "cancelling a snooze is somebody clicking, not an outage"
        );
        assert!(
            throttle
                .consider(&published[1], false, start + MIN_GAP * 12)
                .is_some()
        );
    }

    /// The notifications come from the engine's own event stream. They used
    /// to come from the channel the windows listen on, which a script in the
    /// page could write to as well: one `emit` and the shell announced an
    /// outage nobody had seen.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_alert_the_engine_raises_reaches_the_person_while_the_window_is_away() {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
        let (sender, mut shown) = tokio::sync::mpsc::unbounded_channel();
        let watcher = tokio::spawn(watch(
            &app,
            || false,
            move |notification| {
                let _ = sender.send(notification);
            },
        ));

        raise(&app).await;
        let notification = tokio::time::timeout(std::time::Duration::from_secs(5), shown.recv())
            .await
            .expect("the alert became a notification")
            .expect("the channel is open");
        assert_eq!(notification.title, "Device down");
        assert_eq!(notification.body, "critical");

        app.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), watcher)
            .await
            .expect("watching stops with the engine")
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn nothing_is_shown_while_the_person_is_looking_at_the_window() {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
        let (sender, mut shown) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(watch(
            &app,
            || true,
            move |notification| {
                let _ = sender.send(notification);
            },
        ));
        raise(&app).await;
        let waited =
            tokio::time::timeout(std::time::Duration::from_millis(500), shown.recv()).await;
        assert!(waited.is_err(), "shown while focused: {waited:?}");
        app.shutdown();
    }

    #[test]
    fn a_click_on_a_notification_brings_the_window_forward() {
        // `tap` is what the plugin reports for a click on the notification
        // itself, on every platform; nothing else is offered on one.
        assert!(brings_the_window_forward("tap"));
        assert!(!brings_the_window_forward("dismiss"));
    }

    /// A notification shown before a click handler exists reports nothing
    /// (`tauri-plugin-notification`, `Notification::on_action`), and on most
    /// Linux notification servers and on Windows a click on one then does
    /// nothing at all. Read from the source, because only a notification
    /// server can click.
    #[test]
    fn clicks_are_listened_for_before_the_first_notification_can_be_shown() {
        let shell = include_str!("lib.rs");
        let listened = shell
            .find(".on_action(")
            .expect("the shell does not listen for clicks on its notifications");
        let watched = shell
            .find("notify::watch(")
            .expect("the shell does not turn alerts into notifications");
        assert!(listened < watched);
        assert!(
            !shell.contains("listen(events::CHANNEL"),
            "the shell listens to the page-writable channel again"
        );
    }

    #[tokio::test]
    async fn an_alert_is_announced_once_and_not_again_when_somebody_snoozes_it() {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
        let mut events = app.subscribe();
        let mut throttle = Throttle::new();
        let start = Instant::now();

        let alert = raise(&app).await;
        app.rpc("alerts.snooze", json!({"id": alert["id"], "minutes": 5}))
            .await
            .expect("alerts.snooze");
        let published = alert_events(&mut events);
        assert_eq!(published.len(), 2, "{published:?}");
        assert!(throttle.consider(&published[0], false, start).is_some());
        // Still firing, still its first sighting — and already announced.
        assert_eq!(published[1]["alert"]["state"], "firing");
        assert_eq!(published[1]["alert"]["count"], 1);
        assert_eq!(
            throttle.consider(&published[1], false, start + MIN_GAP * 2),
            None,
            "the same alert was announced a second time because somebody snoozed it"
        );
    }
}
