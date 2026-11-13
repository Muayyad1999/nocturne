//! Forwarding backend events to the windows.
//!
//! Every [`Event`] the backend publishes is emitted to all windows under one
//! name, `nocturne://event`, with the payload the UI expects. The forwarder
//! is written against a trait so it can be tested without a WebView.

use nocturne_suite::{App, Event};
use serde::Serialize;
use tokio::sync::broadcast::error::RecvError;

/// The event name every window listens on.
pub const CHANNEL: &str = "nocturne://event";

/// What a window receives. The same shape the server sends over its
/// WebSocket, so one client works with both.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Payload {
    pub topic: String,
    pub payload: serde_json::Value,
    /// Milliseconds since the epoch.
    pub at: i64,
}

impl From<Event> for Payload {
    fn from(event: Event) -> Self {
        Payload {
            topic: event.topic,
            payload: event.payload,
            at: event.at,
        }
    }
}

/// Somewhere to send events. Implemented by the Tauri application handle.
pub trait Windows: Send + Sync + 'static {
    fn emit(&self, payload: &Payload);
}

/// Read events until the application stops, and emit each one.
///
/// If the forwarder cannot keep up — a burst larger than the channel — the
/// windows are told how many they missed rather than left with a quietly
/// incomplete picture, so the UI can refetch what it shows.
pub async fn forward(app: App, windows: impl Windows) {
    let mut events = app.subscribe();
    let shutdown = app.shutdown_token();
    loop {
        let received = tokio::select! {
            biased;
            _ = shutdown.cancelled() => return,
            received = events.recv() => received,
        };
        match received {
            Ok(event) => windows.emit(&event.into()),
            Err(RecvError::Lagged(missed)) => {
                tracing::warn!(missed, "the interface fell behind the event stream");
                windows.emit(&Payload {
                    topic: "events.lagged".to_string(),
                    payload: serde_json::json!({ "missed": missed }),
                    at: now_ms(),
                });
            }
            Err(RecvError::Closed) => return,
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_suite::AppConfig;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    struct Collect(mpsc::UnboundedSender<Payload>);

    impl Windows for Arc<Collect> {
        fn emit(&self, payload: &Payload) {
            let _ = self.0.send(payload.clone());
        }
    }

    #[tokio::test]
    async fn events_reach_the_windows_in_order_and_stop_at_shutdown() {
        let app = nocturne_suite::start(AppConfig::in_memory()).unwrap();
        // A tokio channel, awaited: a blocking receive would hold the only
        // thread of the test runtime and the forwarder could never be polled.
        let (sender, mut received) = mpsc::unbounded_channel();
        let forwarder = tokio::spawn(forward(app.clone(), Arc::new(Collect(sender))));
        // Subscribing happens inside the task: wait for it before publishing.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        app.publish("one", &serde_json::json!({"n": 1}));
        app.publish("two", &serde_json::json!({"n": 2}));
        async fn next(received: &mut mpsc::UnboundedReceiver<Payload>) -> Payload {
            tokio::time::timeout(std::time::Duration::from_secs(5), received.recv())
                .await
                .expect("an event reaches the windows")
                .expect("the channel is open")
        }
        let first = next(&mut received).await;
        let second = next(&mut received).await;
        assert_eq!(
            (first.topic.as_str(), first.payload["n"].as_i64()),
            ("one", Some(1))
        );
        assert_eq!(second.topic, "two");
        assert!(first.at > 0 && second.at >= first.at);

        app.shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), forwarder)
            .await
            .expect("the forwarder stops when the application does")
            .unwrap();
    }
}
