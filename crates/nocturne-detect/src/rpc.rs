//! `detections.list`, `detections.set_enabled`, `detections.recent`.

use serde::Deserialize;
use serde_json::json;

use nocturne_app::{App, AppBuilder, AppError, AppResult, NoParams};

use crate::catalogue::{self, Detection};
use crate::hits::{self, Wanted};
use crate::model::{Entry, Hit};
use crate::settings::{self, Settings};
use crate::{DetectState, sources};

fn known(id: &str) -> AppResult<&'static Detection> {
    catalogue::find(id).ok_or_else(|| {
        AppError::invalid(format!(
            "`{id}` is not a detection; `detections.list` names the ones there are"
        ))
    })
}

/// The catalogue as it stands in this build, with these settings.
pub fn entries(app: &App, settings: &Settings) -> AppResult<Vec<Entry>> {
    let features = sources::features(app);
    let state = app.state::<DetectState>();
    let last = hits::last_hits(app);
    let counts = hits::counts_since(app, hits::day_ago())?;
    Ok(catalogue::CATALOGUE
        .iter()
        .map(|detection| {
            let unavailable_reason = catalogue::unavailable_reason(detection.id, &features);
            Entry {
                id: detection.id,
                name: detection.name,
                description: detection.description,
                severity: detection.severity,
                decided_from: detection.decided_from,
                cannot_see: detection.cannot_see,
                enabled: settings.is_on(detection.id),
                available: unavailable_reason.is_none(),
                unavailable_reason,
                last_hit_at: last.get(detection.id).copied(),
                hits_24h: counts.get(detection.id).copied().unwrap_or(0),
                last_error: state.last_error(detection.id),
            }
        })
        .collect())
}

fn entry(app: &App, settings: &Settings, id: &str) -> AppResult<Entry> {
    entries(app, settings)?
        .into_iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| AppError::not_found(format!("the detection `{id}`")))
}

fn switches(settings: &Settings) -> (bool, Vec<String>) {
    let mut disabled = settings.disabled.clone();
    disabled.sort_unstable();
    disabled.dedup();
    (settings.enabled, disabled)
}

/// Remember the switches as they are when the application starts, so the
/// first settings change is compared with something.
pub fn remember(app: &App, settings: &Settings) {
    *app.state::<DetectState>().announced.lock() = Some(switches(settings));
}

/// Publish `detections.changed` if a switch moved since it was last
/// published.
///
/// Called both by `detections.set_enabled` and when the `detect` settings
/// section is written by anything else; whichever sees the change first
/// announces it, and the other finds nothing left to say.
pub fn announce(app: &App, settings: &Settings, id: Option<&str>) {
    let now = switches(settings);
    {
        let state = app.state::<DetectState>();
        let mut announced = state.announced.lock();
        if announced.as_ref() == Some(&now) {
            return;
        }
        *announced = Some(now);
    }
    match id {
        Some(id) => app.publish("detections.changed", &json!({ "id": id })),
        None => app.publish("detections.changed", &json!({})),
    }
}

#[derive(Debug, Deserialize)]
struct SetEnabledParams {
    id: String,
    enabled: bool,
}

fn set_enabled(app: &App, params: &SetEnabledParams) -> AppResult<Entry> {
    let detection = known(&params.id)?;
    let before = Settings::load(app);
    // Taking it off the list would change nothing anybody could see: the
    // entry would still read `enabled: false`.
    if params.enabled && !before.enabled {
        return Err(AppError::conflict(
            "detections are switched off as a whole (`enabled` in the `detect` settings); switch them on there before switching one on",
        ));
    }
    let mut disabled: Vec<String> = before
        .disabled
        .iter()
        .filter(|id| id.as_str() != detection.id)
        .cloned()
        .collect();
    if !params.enabled {
        disabled.push(detection.id.to_string());
    }
    if disabled != before.disabled {
        nocturne_app::settings::set_value(app, settings::SECTION, json!({ "disabled": disabled }))?;
        app.audit(
            "detections.set_enabled",
            None,
            format!(
                "Switched the detection `{}` {}",
                detection.name,
                if params.enabled { "on" } else { "off" }
            ),
            json!({"id": detection.id, "enabled": params.enabled}),
        );
    }
    let after = Settings::load(app);
    announce(app, &after, Some(detection.id));
    entry(app, &after, detection.id)
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RecentParams {
    id: Option<String>,
    device_id: Option<String>,
    since: Option<i64>,
    limit: Option<usize>,
}

fn recent(app: &App, params: &RecentParams) -> AppResult<Vec<Hit>> {
    let detection = match params.id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) => Some(known(id)?.id),
        None => None,
    };
    hits::recent(
        app,
        &Wanted {
            detection,
            device_id: params.device_id.as_deref().filter(|id| !id.is_empty()),
            since: params.since,
            limit: params.limit.unwrap_or(100).clamp(1, 1_000),
        },
    )
}

pub fn register(builder: &mut AppBuilder) {
    let router = &mut builder.router;
    router.add("detections.list", |app: App, _: NoParams| async move {
        entries(&app, &Settings::load(&app))
    });
    router.add(
        "detections.set_enabled",
        |app: App, params: SetEnabledParams| async move { set_enabled(&app, &params) },
    );
    router.add(
        "detections.recent",
        |app: App, params: RecentParams| async move { recent(&app, &params) },
    );
}
