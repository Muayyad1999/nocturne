//! Handing a fix to the change pipeline.
//!
//! This feature never changes a device. When something should change — a
//! compliance fix, drift, a restore — it creates a change through
//! `changes.create` and stops there: planning, approval and applying belong
//! to the change pipeline.

use nocturne_app::{App, AppError, AppResult};
use serde_json::{Value, json};

/// One device and what should become of its configuration.
pub(crate) struct ChangeTarget {
    pub device_id: String,
    pub intent: Value,
}

impl ChangeTarget {
    /// Configuration-mode lines exactly as they would be typed.
    pub fn lines(device_id: &str, lines: Vec<String>) -> ChangeTarget {
        ChangeTarget {
            device_id: device_id.to_string(),
            intent: json!({"type": "lines", "lines": lines}),
        }
    }

    /// A desired configuration; `exact` also removes what it does not contain.
    pub fn intended(device_id: &str, config: &str, exact: bool) -> ChangeTarget {
        ChangeTarget {
            device_id: device_id.to_string(),
            intent: json!({"type": "intended", "config": config, "exact": exact}),
        }
    }
}

/// Create a draft change and return its id.
pub(crate) async fn create_change(
    app: &App,
    title: &str,
    description: &str,
    origin: &str,
    targets: Vec<ChangeTarget>,
) -> AppResult<String> {
    let targets: Vec<Value> = targets
        .into_iter()
        .map(|target| json!({"device_id": target.device_id, "intent": target.intent}))
        .collect();
    let request = json!({
        "title": title,
        "description": description,
        "origin": origin,
        "targets": targets,
    });
    let created = app.rpc("changes.create", request).await.map_err(|err| {
        if err.code == "unknown_method" {
            AppError::unavailable(
                "the change pipeline is not part of this build (`changes.create` is not available), \
                 so the fix cannot be handed over. Nothing was changed.",
            )
        } else {
            err
        }
    })?;
    // `changes.create` returns the change, or `{change, job_id}` when it also plans.
    created["id"]
        .as_str()
        .or_else(|| created["change"]["id"].as_str())
        .map(str::to_string)
        .ok_or_else(|| AppError::internal("`changes.create` returned no change id"))
}
