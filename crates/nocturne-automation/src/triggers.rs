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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use crate::db;
use crate::model::{ActionCall, Trigger};
use crate::schedules;
use crate::settings::Settings;
use crate::state::AutomationState;

/// How often pending triggers are checked.
const TICK: Duration = Duration::from_millis(250);

/// Events a trigger's own run publishes, which a trigger must never watch: a
/// trigger on `automation.runs.changed` that starts a run is a loop. Job
/// events are among them because every method a trigger may call starts a
/// job, and the core announces each one on `jobs.changed` as it starts, moves
/// and ends: a trigger on them is set off by its own job, again and again.
const OWN_TOPICS: &[&str] = &[
    "automation.",
    "schedules.",
    "triggers.",
    "tasks.",
    "templates.",
    "playbooks.",
    "jobs.",
];

/// Is this an event a trigger's own run could have published?
fn is_own(topic: &str) -> bool {
    OWN_TOPICS
        .iter()
        .any(|own| topic.starts_with(own) || topic == own.trim_end_matches('.'))
}

/// Could a trigger on this pattern be set off by one of those events? A
/// prefix pattern covers every topic that starts with it, so `*`, `t*` and
/// `jobs.` are refused as surely as `jobs.changed` is.
fn takes_in_own(pattern: &str) -> bool {
    let pattern = pattern.trim();
    let prefix = pattern
        .strip_suffix('*')
        .or_else(|| pattern.ends_with('.').then_some(pattern));
    match prefix {
        Some(prefix) => OWN_TOPICS
            .iter()
            .any(|own| own.starts_with(prefix) || prefix.starts_with(own)),
        None => is_own(pattern),
    }
}

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

/// A device the event was about, if it names one. An event's own `id` is a
/// device only when the event is about devices: an alert or a change has an
/// id of its own, and passed on as a device it names none.
fn device_of(topic: &str, payload: &Value) -> Option<String> {
    let pointers: &[&str] = if topic.starts_with("devices.") {
        &["/device_id", "/device/id", "/id"]
    } else {
        &["/device_id", "/device/id"]
    };
    pointers
        .iter()
        .find_map(|pointer| payload.pointer(pointer).and_then(Value::as_str))
        .map(str::to_string)
}

/// A burst a trigger is collecting. It is also written to the store (as
/// `db::TRIGGER_PENDING`, under the trigger's id), so a restart inside the
/// window still fires it.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
struct Pending {
    trigger_id: String,
    fire_at: i64,
    device_ids: Vec<String>,
    events: u64,
    topic: String,
}

fn keep(app: &App, id: &str, pending: &Pending) {
    if let Err(err) = app.store().put(db::TRIGGER_PENDING, id, pending) {
        tracing::warn!(trigger = id, %err, "a trigger's burst could not be kept on disk");
    }
}

fn forget(app: &App, id: &str) {
    let _ = app.store().delete(db::TRIGGER_PENDING, id);
}

#[derive(Default)]
pub struct Pendings(Mutex<HashMap<String, Pending>>);

pub fn check(app: &App, trigger: &Trigger) -> AppResult<()> {
    let topic = trigger.topic.trim();
    if topic.is_empty() {
        return Err(AppError::invalid("a trigger needs an event topic to watch"));
    }
    if takes_in_own(topic) {
        return Err(AppError::invalid(format!(
            "`{topic}` takes in events a trigger's own run publishes (automation's own, and \
             `jobs.changed`, which every action's job announces itself on); a trigger on it \
             would run in a loop"
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
    // Never, whatever a stored trigger says: one saved before a topic was
    // refused, or by hand, must not loop either.
    if is_own(&event.topic) {
        return;
    }
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
        let kept = {
            let mut pending = pendings.0.lock();
            let entry = pending
                .entry(trigger.id.clone())
                .or_insert_with(|| Pending {
                    trigger_id: trigger.id.clone(),
                    fire_at: now + trigger.debounce_s as i64 * 1000,
                    topic: event.topic.clone(),
                    ..Pending::default()
                });
            entry.events += 1;
            entry.topic = event.topic.clone();
            if trigger.pass_device
                && let Some(device_id) = device_of(&event.topic, &event.payload)
                && !entry.device_ids.contains(&device_id)
            {
                entry.device_ids.push(device_id);
            }
            entry.clone()
        };
        keep(app, &trigger.id, &kept);
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
    // Switched off while a window was open, or before a restart that brought
    // one back: the burst is let go, as one arriving now would never be kept.
    let listening = Settings::load(app).triggers_enabled;
    for (id, entry) in due {
        if !listening {
            forget(app, &id);
            continue;
        }
        let Ok(trigger) = db::trigger(app, &id) else {
            forget(app, &id);
            continue;
        };
        if !trigger.enabled {
            forget(app, &id);
            continue;
        }
        // Still running from last time: hold the burst for another window
        // rather than losing it, and never run two at once.
        let Some(guard) = state.start_firing(&trigger.id) else {
            let held = Pending {
                fire_at: now + (trigger.debounce_s.max(5) as i64 * 1000),
                ..entry
            };
            keep(app, &id, &held);
            pendings.0.lock().insert(id, held);
            continue;
        };
        // The burst is spent and the firing counted before anything is
        // called, on the record as it is stored: a process that dies in the
        // action does not fire the same burst again when it comes back.
        forget(app, &id);
        let counted = {
            let _records = state.records();
            match db::trigger(app, &id) {
                Ok(mut stored) => {
                    stored.last_fired_at = Some(now);
                    stored.fired_count += 1;
                    app.store()
                        .put(db::TRIGGERS, &stored.id, &stored)
                        .ok()
                        .map(|_| stored)
                }
                Err(_) => None,
            }
        };
        let Some(trigger) = counted else {
            continue;
        };
        app.publish("triggers.changed", &trigger);

        let extra =
            (!entry.device_ids.is_empty()).then(|| json!({"device_ids": entry.device_ids.clone()}));
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
            // Onto the record as it is now: deleted while the action ran, it
            // stays deleted; edited, it keeps the edit.
            let stored = {
                let state = app.state::<AutomationState>();
                let _records = state.records();
                match db::trigger(&app, &trigger.id) {
                    Ok(mut stored) => {
                        stored.last_job_id = job_id.clone();
                        app.store()
                            .put(db::TRIGGERS, &stored.id, &stored)
                            .ok()
                            .map(|_| stored)
                    }
                    Err(_) => None,
                }
            };
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
            if let Some(stored) = stored {
                app.publish("triggers.changed", &stored);
            }
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
        let state = app.state::<AutomationState>();
        let records = state.records();
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
        drop(records);
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
        let trigger = {
            let state = app.state::<AutomationState>();
            let _records = state.records();
            let trigger = db::trigger(&app, &params.id)?;
            app.store().delete(db::TRIGGERS, &params.id)?;
            trigger
        };
        app.state::<Pendings>().0.lock().remove(&params.id);
        forget(&app, &params.id);
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

/// Bursts the previous process was still collecting when it ended. Each fires
/// when its window closes, or at the first sweep if it closed while nothing
/// was running; one that closed longer ago than a schedule would catch up on
/// is let go, and the log says so.
fn restore(app: &App) {
    let Ok(kept) = app.store().list::<Pending>(db::TRIGGER_PENDING) else {
        return;
    };
    let state = app.state::<AutomationState>();
    let window_ms = Settings::load(app).catch_up_within_s as i64 * 1000;
    let now = state.now_ms();
    let pendings = app.state::<Pendings>();
    for pending in kept {
        let id = pending.trigger_id.clone();
        if db::trigger(app, &id).is_err() {
            forget(app, &id);
            continue;
        }
        if now - pending.fire_at > window_ms {
            tracing::warn!(
                trigger = %id,
                events = pending.events,
                "a trigger's burst closed too long before this start to be fired now"
            );
            forget(app, &id);
            continue;
        }
        pendings.0.lock().insert(id, pending);
    }
}

pub fn spawn(app: &App) {
    restore(app);
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
            device_of("alerts.raised", &json!({"device_id": "d1"})).as_deref(),
            Some("d1")
        );
        assert_eq!(
            device_of("alerts.raised", &json!({"device": {"id": "d2"}})).as_deref(),
            Some("d2")
        );
        assert_eq!(device_of("alerts.raised", &json!({"nothing": true})), None);
        // An event about a device may name it by its own id; any other event's
        // id is its own (an alert's, a change's, a job's).
        assert_eq!(
            device_of("devices.changed", &json!({"id": "d3"})).as_deref(),
            Some("d3")
        );
        assert_eq!(device_of("alerts.raised", &json!({"id": "alert-17"})), None);
        assert_eq!(device_of("changes.changed", &json!({"id": "chg-1"})), None);
    }

    #[test]
    fn a_pattern_is_refused_when_it_takes_in_what_a_trigger_s_own_run_publishes() {
        for pattern in [
            "jobs.changed",
            "jobs.",
            "jobs.*",
            "jobs",
            "job*",
            "*",
            "t*",
            "auto*",
            "automation.runs.changed",
            "tasks",
        ] {
            assert!(takes_in_own(pattern), "{pattern}");
        }
        for pattern in [
            "alerts.raised",
            "alerts.",
            "alerts.*",
            "devices.state",
            "changes.",
        ] {
            assert!(!takes_in_own(pattern), "{pattern}");
        }
        assert!(is_own("jobs.changed") && is_own("triggers.fired") && !is_own("alerts.raised"));
    }
}
