//! Tying a line to what else is known.
//!
//! This is the part that makes a log store worth more than `grep`: a line
//! has a device, a time and often an interface, and the application knows
//! other things that have devices, times and interfaces — alerts, changes,
//! the signals these rules raised. `logs.around` takes one moment and says
//! what else was happening; `logs.bursts` takes a window and says what was
//! shouting, which is the other end of the same question.
//!
//! What it will not do is claim a cause. An alert that started eleven
//! seconds after a burst of `%LINK-3-UPDOWN` is *reported next to it*, with
//! both timestamps, and the engineer draws the conclusion. And a list that
//! is empty because the feature that would fill it is not in this build says
//! so (`state: unavailable`) instead of looking like "nothing happened".

use nocturne_app::store::LogEntry;
use nocturne_app::util::now_ms;
use nocturne_app::{App, AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::model::{Burst, SourceState};
use crate::query::{self, Filter};

/// A list borrowed from another feature.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Borrowed {
    pub state: SourceState,
    pub items: Vec<Value>,
    /// Why the list is empty, when it is empty for a reason.
    pub note: String,
}

impl Borrowed {
    fn unavailable(method: &str) -> Borrowed {
        Borrowed {
            state: SourceState::Unavailable,
            items: Vec::new(),
            note: format!("`{method}` is not registered in this build"),
        }
    }

    fn failed(method: &str, err: &AppError) -> Borrowed {
        Borrowed {
            state: SourceState::Failed,
            items: Vec::new(),
            note: format!("`{method}` failed: {err}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Around {
    /// The entry the window is centred on, when it was asked for by id.
    pub center: Option<LogEntry>,
    pub since: i64,
    pub until: i64,
    pub device_id: Option<String>,
    pub interface: Option<String>,
    pub entries: Vec<LogEntry>,
    pub has_more: bool,
    pub bursts: Vec<Burst>,
    /// Signals this feature's own rules raised in the window.
    pub signals: Vec<LogEntry>,
    pub alerts: Borrowed,
    pub changes: Borrowed,
}

/// One entry by id.
pub fn entry(app: &App, id: i64) -> AppResult<Option<LogEntry>> {
    let (entries, _) = query::page(
        app,
        &Filter {
            stream: Some("*".into()),
            ..Filter::default()
        },
        Some(id + 1),
        Some(id - 1),
        1,
    )?;
    Ok(entries.into_iter().next())
}

/// Call another feature and cope with it not being there.
async fn borrow(app: &App, method: &str, params: Value) -> Borrowed {
    match app.rpc(method, params).await {
        Ok(Value::Array(items)) => Borrowed {
            state: SourceState::Ok,
            items,
            note: String::new(),
        },
        // `{items: [...]}` or anything else: handed on as one value rather
        // than reshaped, because this crate does not own those shapes.
        Ok(other) => Borrowed {
            state: SourceState::Ok,
            items: vec![other],
            note: String::new(),
        },
        Err(err) if err.code == "unknown_method" => Borrowed::unavailable(method),
        Err(err) => Borrowed::failed(method, &err),
    }
}

fn within(item: &Value, keys: &[&str], since: i64, until: i64) -> bool {
    keys.iter()
        .filter_map(|key| item.get(*key).and_then(Value::as_i64))
        .any(|at| at >= since && at <= until)
}

/// What else was happening around a moment.
pub async fn around(
    app: &App,
    entry_id: Option<i64>,
    at: Option<i64>,
    device_id: Option<String>,
    window_s: u32,
    limit: u32,
) -> AppResult<Around> {
    let center = match entry_id {
        Some(id) => entry(app, id)?,
        None => None,
    };
    if entry_id.is_some() && center.is_none() {
        return Err(AppError::not_found(format!(
            "log entry {}",
            entry_id.unwrap_or_default()
        )));
    }
    let at = center
        .as_ref()
        .map(|entry| entry.ts)
        .or(at)
        .unwrap_or_else(now_ms);
    let window = i64::from(window_s.clamp(1, 86_400)) * 1_000;
    let (since, until) = (at - window, at + window);
    let device_id = device_id.or_else(|| center.as_ref().and_then(|entry| entry.device_id.clone()));
    let interface = center
        .as_ref()
        .and_then(|entry| entry.body.as_ref())
        .and_then(|body| body.get("interface"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let filter = Filter {
        device_id: device_id.clone(),
        since: Some(since),
        until: Some(until),
        ..Filter::default()
    };
    let (entries, has_more) = query::page(app, &filter, None, None, limit)?;
    let bursts = query::bursts(app, &filter, 2, 20)?;
    let (signals, _) = query::page(
        app,
        &Filter {
            stream: Some("event".into()),
            ..filter.clone()
        },
        None,
        None,
        50,
    )?;

    let mut alerts = borrow(
        app,
        "alerts.list",
        json!({"since": since, "device_id": device_id, "limit": 50}),
    )
    .await;
    alerts
        .items
        .retain(|alert| within(alert, &["started_at", "last_seen_at"], since, until));
    let mut changes = borrow(
        app,
        "changes.list",
        json!({"device_id": device_id, "limit": 100}),
    )
    .await;
    changes.items.retain(|change| {
        within(
            change,
            &["created_at", "applied_at", "updated_at"],
            since,
            until,
        )
    });

    Ok(Around {
        center,
        since,
        until,
        device_id,
        interface,
        entries,
        has_more,
        bursts,
        signals,
        alerts,
        changes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_borrowed_item_is_kept_only_if_one_of_its_times_is_in_the_window() {
        let alert = json!({"started_at": 1_000, "last_seen_at": 5_000});
        assert!(within(&alert, &["started_at", "last_seen_at"], 900, 1_100));
        assert!(within(
            &alert,
            &["started_at", "last_seen_at"],
            4_000,
            6_000
        ));
        assert!(!within(
            &alert,
            &["started_at", "last_seen_at"],
            2_000,
            3_000
        ));
        assert!(
            !within(&json!({"note": "no times at all"}), &["started_at"], 0, 9),
            "something with no timestamp cannot be placed in a window"
        );
    }
}
