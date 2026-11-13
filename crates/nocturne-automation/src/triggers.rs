//! Triggers: run something when the rest of the application says something
//! happened.
//!
//! A trigger watches an event topic. Events arrive in bursts — a switch that
//! flaps produces a dozen `alerts.raised` in a second — so a trigger collects
//! what arrives for `debounce_s` and then fires **once**. It calls the same
//! allow-listed methods a schedule may call, and like a schedule it never
//! starts a second run while its own previous one is still going.

use std::collections::HashMap;
use std::time::Duration;

use nocturne_app::error::{AppError, AppResult};
use nocturne_app::util::{new_id, now_ms};
use nocturne_app::{App, AppBuilder, Event, NoParams};
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::db;
use crate::model::{ActionCall, Trigger};
use crate::schedules;
use crate::settings::Settings;
use crate::state::AutomationState;

/// How often pending triggers are checked.
const TICK: Duration = Duration::from_millis(250);

/// Events this crate publishes itself, which a trigger must never watch: a
/// trigger on `automation.runs.changed` that starts a run is a loop.
const OWN_TOPICS: &[&str] = &[
    "automation.",
    "schedules.",
    "triggers.",
    "tasks.",
    "templates.",
    "playbooks.",
];

pub fn topic_matches(topic: &str, pattern: &str) -> bool {
    let pattern = pattern.trim();
    match pattern.strip_suffix('*') {
        Some(prefix) => topic.starts_with(prefix),
        None if pattern.ends_with('.') => topic.starts_with(pattern),
        None => topic == pattern,
    }
}

/// Every pointer in `match_payload` must point at that value in the payload.
pub fn payload_matches(payload: &Value, wanted: &Value) -> bool {
    let Some(wanted) = wanted.as_object() else {
        return true;
    };
    wanted.iter().all(|(pointer, expected)| {
        let pointer = if pointer.starts_with('/') {
            pointer.clone()
        } else {
            format!("/{pointer}")
        };
        match payload.pointer(&pointer) {
            Some(found) => found == expected || found.as_str() == expected.as_str(),
            None => false,
        }
    })
}

/// A device the event was about, if it names one.
fn device_of(payload: &Value) -> Option<String> {
    ["/device_id", "/device/id", "/id"]
        .into_iter()
        .find_map(|pointer| payload.pointer(pointer).and_then(Value::as_str))
        .map(str::to_string)
}

#[derive(Debug, Default, Clone)]
struct Pending {
    fire_at: i64,
    device_ids: Vec<String>,
    events: u64,
    topic: String,
}

#[derive(Default)]
pub struct Pendings(Mutex<HashMap<String, Pending>>);

pub fn check(app: &App, trigger: &Trigger) -> AppResult<()> {
    let topic = trigger.topic.trim();
    if topic.is_empty() {
        return Err(AppError::invalid("a trigger needs an event topic to watch"));
    }
    if OWN_TOPICS
        .iter()
        .any(|own| topic.starts_with(own) || topic == own.trim_end_matches('.'))
    {
        return Err(AppError::invalid(format!(
            "`{topic}` is published by automation itself; a trigger on it would run in a loop"
        )));
    }
    if trigger.debounce_s > 3600 {
        return Err(AppError::invalid(
            "a debounce longer than an hour is a schedule, not a trigger",
        ));
    }
    if !trigger.match_payload.is_null() && !trigger.match_payload.is_object() {
        return Err(AppError::invalid(
            "`match_payload` is an object of JSON pointers to the values they must have",
        ));
    }
    schedules::check_action(app, &trigger.action)
}

/// Note an event against every trigger that wants it.
fn observe(app: &App, event: &Event) {
    let settings = Settings::load(app);
    if !settings.triggers_enabled {
        return;
    }
    let Ok(triggers) = db::triggers(app) else {
        return;
    };
    let state = app.state::<AutomationState>();
    let pendings = app.state::<Pendings>();
    let now = state.now_ms();
    for trigger in triggers {
        if !trigger.enabled
            || !topic_matches(&event.topic, &trigger.topic)
            || !payload_matches(&event.payload, &trigger.match_payload)
        {
            continue;
        }
        let mut pending = pendings.0.lock();
        let entry = pending
            .entry(trigger.id.clone())
            .or_insert_with(|| Pending {
                fire_at: now + trigger.debounce_s as i64 * 1000,
                topic: event.topic.clone(),
                ..Pending::default()
            });
        entry.events += 1;
        entry.topic = event.topic.clone();
        if trigger.pass_device
            && let Some(device_id) = device_of(&event.payload)
            && !entry.device_ids.contains(&device_id)
        {
            entry.device_ids.push(device_id);
        }
    }
}

/// Fire the triggers whose debounce window has closed.
async fn sweep(app: &App) {
    let state = app.state::<AutomationState>();
    let pendings = app.state::<Pendings>();
    let now = state.now_ms();
    let due: Vec<(String, Pending)> = {
        let mut pending = pendings.0.lock();
        let ready: Vec<String> = pending
            .iter()
            .filter(|(_, entry)| entry.fire_at <= now)
            .map(|(id, _)| id.clone())
            .collect();
        ready
            .into_iter()
            .filter_map(|id| pending.remove(&id).map(|entry| (id, entry)))
            .collect()
    };
    for (id, entry) in due {
        let Ok(mut trigger) = db::trigger(app, &id) else {
            continue;
        };
        if !trigger.enabled {
            continue;
        }
        // Still running from last time: hold the burst for another window
        // rather than losing it, and never run two at once.
        let Some(guard) = state.start_firing(&trigger.id) else {
            let mut pending = pendings.0.lock();
            pending.insert(
                id,
                Pending {
                    fire_at: now + (trigger.debounce_s.max(5) as i64 * 1000),
                    ..entry
                },
            );
            continue;
        };

        let extra =
            (!entry.device_ids.is_empty()).then(|| json!({"device_ids": entry.device_ids.clone()}));
        trigger.last_fired_at = Some(now);
        trigger.fired_count += 1;

        let app = app.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let outcome = schedules::call_action(
                &app,
                &trigger.id,
                &trigger.name,
                &trigger.action,
                "trigger",
                extra,
            )
            .await;
            let job_id = match outcome {
                Ok((job_id, result)) => {
                    if let Err(err) = &result {
                        tracing::warn!(trigger = %trigger.name, %err, "a trigger's action failed");
                    }
                    job_id
                }
                Err(err) => {
                    tracing::warn!(trigger = %trigger.name, %err, "a trigger could not run");
                    None
                }
            };
            trigger.last_job_id = job_id.clone();
            let _ = app.store().put(db::TRIGGERS, &trigger.id, &trigger);
            app.publish(
                "triggers.fired",
                &json!({
                    "id": trigger.id,
                    "topic": entry.topic,
                    "events": entry.events,
                    "job_id": job_id,
                    "device_ids": entry.device_ids,
                }),
            );
            app.publish("triggers.changed", &trigger);
            // Held until the work it started is done, so the next burst waits.
            schedules::wait_for(&app, &job_id).await;
        });
    }
}

async fn listener(app: App, mut events: broadcast::Receiver<Event>) {
    let mut ticker = tokio::time::interval(TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => observe(&app, &event),
                // Behind by more than the bus keeps: the triggers that would
                // have fired are lost, which is better than firing late on a
                // burst that is already over. Say so.
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "automation triggers missed events");
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = ticker.tick() => sweep(&app).await,
        }
    }
}

// --------------------------------------------------------------------- rpc

#[derive(Debug, Deserialize)]
struct SaveParams {
    #[serde(default)]
    id: Option<String>,
    name: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    topic: String,
    #[serde(default)]
    match_payload: Value,
    #[serde(default)]
    debounce_s: u64,
    action: ActionCall,
    #[serde(default)]
    pass_device: bool,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct IdParams {
    id: String,
}

pub fn register(builder: &mut AppBuilder) {
    builder.state(Pendings::default());
    let router = &mut builder.router;

    router.add("triggers.list", |app: App, _: NoParams| async move {
        db::triggers(&app)
    });

    router.add("triggers.get", |app: App, params: IdParams| async move {
        db::trigger(&app, &params.id)
    });

    router.add("triggers.save", |app: App, params: SaveParams| async move {
        let name = db::require_name(&params.name, "trigger")?;
        let existing = params
            .id
            .as_ref()
            .filter(|id| !id.trim().is_empty())
            .and_then(|id| db::trigger(&app, id.trim()).ok());
        let trigger = Trigger {
            id: existing
                .as_ref()
                .map(|trigger| trigger.id.clone())
                .unwrap_or_else(new_id),
            name,
            enabled: params.enabled,
            topic: params.topic.trim().to_string(),
            match_payload: params.match_payload,
            debounce_s: params.debounce_s,
            action: params.action,
            pass_device: params.pass_device,
            last_fired_at: existing.as_ref().and_then(|trigger| trigger.last_fired_at),
            last_job_id: existing
                .as_ref()
                .and_then(|trigger| trigger.last_job_id.clone()),
            fired_count: existing.as_ref().map(|t| t.fired_count).unwrap_or(0),
            created_at: existing
                .as_ref()
                .map(|trigger| trigger.created_at)
                .unwrap_or_else(now_ms),
        };
        check(&app, &trigger)?;
        app.store().put(db::TRIGGERS, &trigger.id, &trigger)?;
        app.audit(
            "triggers.save",
            None,
            format!(
                "Saved the trigger `{}` on `{}` ({})",
                trigger.name, trigger.topic, trigger.action.method
            ),
            json!({"trigger_id": trigger.id, "enabled": trigger.enabled}),
        );
        app.publish("triggers.changed", &trigger);
        Ok(trigger)
    });

    router.add("triggers.delete", |app: App, params: IdParams| async move {
        let trigger = db::trigger(&app, &params.id)?;
        app.store().delete(db::TRIGGERS, &params.id)?;
        app.state::<Pendings>().0.lock().remove(&params.id);
        app.audit(
            "triggers.delete",
            None,
            format!("Deleted the trigger `{}`", trigger.name),
            json!({"trigger_id": params.id}),
        );
        app.publish(
            "triggers.changed",
            &json!({"id": params.id, "deleted": true}),
        );
        Ok(json!({"deleted": true}))
    });

    router.add(
        "triggers.fire_now",
        |app: App, params: IdParams| async move {
            let trigger = db::trigger(&app, &params.id)?;
            check(&app, &trigger)?;
            let state = app.state::<AutomationState>();
            let guard = state.start_firing(&trigger.id).ok_or_else(|| {
                AppError::conflict(format!("`{}` is still running", trigger.name))
            })?;
            let _guard = guard;
            let (job_id, result) = schedules::call_action(
                &app,
                &trigger.id,
                &trigger.name,
                &trigger.action,
                "trigger",
                None,
            )
            .await?;
            app.audit(
                "triggers.fire_now",
                None,
                format!("Fired the trigger `{}` by hand", trigger.name),
                json!({"trigger_id": trigger.id, "job_id": job_id}),
            );
            result
        },
    );
}

pub fn spawn(app: &App) {
    // Subscribed here rather than inside the task, so nothing published
    // between `spawn` and the task's first poll is missed.
    app.spawn(
        "automation.triggers",
        listener(app.clone(), app.subscribe()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_topic_matches_exactly_or_by_prefix() {
        assert!(topic_matches("alerts.raised", "alerts.raised"));
        assert!(!topic_matches("alerts.cleared", "alerts.raised"));
        assert!(topic_matches("alerts.raised", "alerts."));
        assert!(topic_matches("alerts.raised", "alerts.*"));
        assert!(!topic_matches("devices.changed", "alerts."));
    }

    #[test]
    fn a_payload_filter_must_match_every_pointer_it_names() {
        let payload = json!({"severity": "critical", "device_id": "d1", "alert": {"kind": "down"}});
        assert!(payload_matches(&payload, &json!({"/severity": "critical"})));
        assert!(payload_matches(&payload, &json!({"severity": "critical"})));
        assert!(payload_matches(&payload, &json!({"/alert/kind": "down"})));
        assert!(!payload_matches(&payload, &json!({"/severity": "warning"})));
        assert!(!payload_matches(&payload, &json!({"/nothing": "here"})));
        assert!(payload_matches(&payload, &Value::Null));
    }

    #[test]
    fn the_device_an_event_is_about_is_found_where_events_put_it() {
        assert_eq!(
            device_of(&json!({"device_id": "d1"})).as_deref(),
            Some("d1")
        );
        assert_eq!(
            device_of(&json!({"device": {"id": "d2"}})).as_deref(),
            Some("d2")
        );
        assert_eq!(device_of(&json!({"nothing": true})), None);
    }
}
