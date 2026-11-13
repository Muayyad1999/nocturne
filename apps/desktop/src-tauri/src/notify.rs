//! System notifications for alerts that fire while the window is away.
//!
//! The rule is deliberately narrow: only alerts, only the two worst
//! severities, only when the window is not in front, and at most one every
//! ten seconds. An operations tool that interrupts its user for everything
//! is one whose notifications get turned off.

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
        let alert = payload.get("alert").unwrap_or(payload);
        if alert.get("state").and_then(Value::as_str) != Some("firing") {
            return None;
        }
        let severity = alert
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(severity, "critical" | "high") {
            return None;
        }
        // A repeat of an alert already counted is not news.
        if alert.get("count").and_then(Value::as_u64).unwrap_or(1) > 1 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn alert(severity: &str, state: &str) -> Value {
        json!({"alert": {"title": "Device down", "device": "CORE-SW1", "severity": severity, "state": state, "count": 1}})
    }

    #[test]
    fn only_new_serious_alerts_and_only_while_the_window_is_away() {
        let mut throttle = Throttle::new();
        let now = Instant::now();

        assert_eq!(
            throttle.consider(&alert("critical", "firing"), true, now),
            None,
            "the user is looking at it"
        );
        let shown = throttle
            .consider(&alert("critical", "firing"), false, now)
            .expect("a notification");
        assert_eq!(shown.title, "Device down");
        assert_eq!(shown.body, "CORE-SW1 — critical");

        // Within the gap, nothing more.
        assert_eq!(
            throttle.consider(
                &alert("high", "firing"),
                false,
                now + Duration::from_secs(9)
            ),
            None
        );
        assert!(
            throttle
                .consider(
                    &alert("high", "firing"),
                    false,
                    now + Duration::from_secs(11)
                )
                .is_some()
        );

        let mut throttle = Throttle::new();
        for (severity, state) in [
            ("medium", "firing"),
            ("low", "firing"),
            ("critical", "resolved"),
            ("critical", "acknowledged"),
        ] {
            assert_eq!(
                throttle.consider(&alert(severity, state), false, now),
                None,
                "{severity}/{state}"
            );
        }
        // A repeat of a known alert is not news.
        let mut repeat = alert("critical", "firing");
        repeat["alert"]["count"] = json!(4);
        assert_eq!(throttle.consider(&repeat, false, now), None);
        // Nor is a payload that is not an alert at all.
        assert_eq!(throttle.consider(&json!({"cleared": 3}), false, now), None);
    }
}
