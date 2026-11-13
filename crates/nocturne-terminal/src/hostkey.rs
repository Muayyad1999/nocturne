//! Telling the UI which keys a refused SSH host key was about.
//!
//! The core's host-key verifier refuses a changed (or, under the strict
//! policy, an unknown) key with a sentence, and announces the fingerprints
//! involved as an event. A UI that wants to offer "trust the new key" needs
//! them with the error, so a connection attempt listens for that event and
//! attaches its payload as the error's details.

use std::future::Future;

use nocturne_app::{App, AppResult};
use tokio::sync::broadcast::error::TryRecvError;

const ANNOUNCEMENTS: [&str; 2] = ["connect.host_key_changed", "connect.host_key_unknown"];

/// Run a connection attempt; if it fails on a host key, add
/// `{host, port, known?: {algorithm, fingerprint, first_seen}, presented: {algorithm, fingerprint, openssh}}`
/// to the error.
pub(crate) async fn with_details<T>(
    app: &App,
    attempt: impl Future<Output = AppResult<T>>,
) -> AppResult<T> {
    let mut events = app.subscribe();
    let mut err = match attempt.await {
        Err(err) if err.code == "host_key" && err.details.is_none() => err,
        other => return other,
    };
    loop {
        match events.try_recv() {
            Ok(event) if ANNOUNCEMENTS.contains(&event.topic.as_str()) => {
                // Other connections may be failing at the same moment: the
                // message names the host this refusal is about.
                let endpoint = format!(
                    "{}:{}",
                    event.payload["host"].as_str().unwrap_or_default(),
                    event.payload["port"]
                );
                if mentions(&err.message, &endpoint) {
                    err.details = Some(event.payload);
                }
            }
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
        }
    }
    Err(err)
}

/// Whether `message` names `endpoint` (`host:port`) itself, not a longer
/// name or port that merely contains it (`10.0.0.1:22` in `10.0.0.1:2222`).
fn mentions(message: &str, endpoint: &str) -> bool {
    message.match_indices(endpoint).any(|(start, found)| {
        let before = message[..start].chars().next_back();
        let after = message[start + found.len()..].chars().next();
        !before.is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '.' | '-' | '_'))
            && !after.is_some_and(|ch| ch.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nocturne_app::{AppBuilder, AppConfig, AppError};
    use serde_json::json;

    fn app() -> App {
        AppBuilder::new(AppConfig::in_memory()).unwrap().build()
    }

    #[tokio::test]
    async fn the_announcement_for_the_refused_host_becomes_the_details() {
        let app = app();
        let refused = with_details(&app, async {
            app.publish("connect.host_key_changed", &json!({"host": "10.0.0.9", "port": 22, "presented": {"fingerprint": "SHA256:other"}}));
            app.publish("devices.changed", &json!({"ids": []}));
            app.publish("connect.host_key_changed", &json!({"host": "10.0.0.1", "port": 2222, "known": {"fingerprint": "SHA256:old"}, "presented": {"fingerprint": "SHA256:new"}}));
            Err::<(), _>(AppError::new(
                "host_key",
                "host key rejected: the SSH host key of 10.0.0.1:2222 has changed",
            ))
        })
        .await
        .unwrap_err();
        let details = refused.details.expect("details");
        assert_eq!(details["port"], 2222);
        assert_eq!(details["known"]["fingerprint"], "SHA256:old");
        assert_eq!(details["presented"]["fingerprint"], "SHA256:new");
    }

    #[test]
    fn an_endpoint_is_matched_whole() {
        let message = "the SSH host key of 10.0.0.1:2222 has changed (was a, now b)";
        assert!(mentions(message, "10.0.0.1:2222"));
        assert!(!mentions(message, "10.0.0.1:22"));
        assert!(!mentions(message, "0.0.0.1:2222"));
        assert!(mentions("sw1.lab:22: refused", "sw1.lab:22"));
        assert!(!mentions("core-sw1.lab:22: refused", "sw1.lab:22"));
    }

    #[tokio::test]
    async fn other_outcomes_pass_through_untouched() {
        let app = app();
        assert_eq!(with_details(&app, async { Ok(7) }).await.unwrap(), 7);
        let refused = with_details(&app, async {
            app.publish(
                "connect.host_key_changed",
                &json!({"host": "10.0.0.1", "port": 22}),
            );
            Err::<(), _>(AppError::new("auth", "authentication failed at 10.0.0.1:22"))
        })
        .await
        .unwrap_err();
        assert_eq!((refused.code, refused.details), ("auth", None));
    }
}
