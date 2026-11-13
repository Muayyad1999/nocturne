//! The scheduler: one task that decides what to poll and when.
//!
//! Every monitored device gets its own slot inside its interval, chosen from
//! a hash of its id, so that a thousand switches on a 60-second interval are
//! spread across the minute instead of all being probed on the tick. Each
//! poll runs as its own task behind a semaphore, which is what makes "one
//! slow device delays nothing else" and "never more than `concurrency`
//! probes in flight" both true at once.

use std::sync::Arc;

use nocturne_app::{App, inventory};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::settings::MonitorSettings;
use crate::state::MonitorState;
use crate::{alerts, interfaces, poll, rules};

/// How often the scheduler looks at the clock. The shortest interval a
/// device may have is five seconds, so a second of granularity is plenty.
const TICK_MS: u64 = 1_000;
/// How long the inventory it works from is reused. Reading a few thousand
/// devices out of the store every tick would cost more than the polling.
const INVENTORY_MS: i64 = 5_000;
/// How often the roll-up and pruning pass runs.
const MAINTENANCE_MS: u64 = 3_600_000;
/// Resolved alerts are kept this long.
const ALERT_HISTORY_DAYS: i64 = 90;

/// A device's offset inside its own interval, in milliseconds. Stable for a
/// device, evenly spread across devices.
pub fn jitter(device_id: &str, interval_ms: i64) -> i64 {
    // FNV-1a: small, no dependency, and spreads short ids well.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in device_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    if interval_ms <= 0 {
        return 0;
    }
    (hash % interval_ms as u64) as i64
}

pub fn spawn(app: &App) {
    app.spawn("monitor.scheduler", scheduler(app.clone()));
    app.spawn("monitor.maintenance", maintenance(app.clone()));
    // Subscribed here rather than inside the task, so nothing published
    // between `spawn` and the task's first poll is missed.
    app.spawn("monitor.events", events(app.clone(), app.subscribe()));
}

async fn scheduler(app: App) {
    let state = app.state::<MonitorState>();
    let mut limit = 0u32;
    let mut gate = Arc::new(Semaphore::new(1));
    let mut devices: Vec<nocturne_app::model::Device> = Vec::new();
    // Zero forces the first tick to read the inventory.
    let mut listed_at = 0i64;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(TICK_MS)).await;
        let settings = MonitorSettings::load(&app);
        if !settings.enabled {
            continue;
        }
        if settings.concurrency != limit {
            limit = settings.concurrency;
            gate = Arc::new(Semaphore::new(limit as usize));
        }
        let now = state.clock.now_ms();
        if now - listed_at >= INVENTORY_MS {
            match inventory::list(&app, &inventory::DeviceFilter::default()) {
                Ok(rows) => {
                    devices = rows
                        .into_iter()
                        .map(|row| row.device)
                        .filter(|device| device.monitor.enabled && poll::plan(device).any())
                        .collect();
                    listed_at = now;
                }
                Err(err) => {
                    tracing::warn!(%err, "the inventory could not be read");
                    continue;
                }
            }
        }
        for device in &devices {
            let interval_ms = i64::from(device.monitor.interval_s.max(5)) * 1_000;
            let due = {
                let mut runtime = state.runtime.lock();
                if runtime.in_flight.contains(&device.id) {
                    continue;
                }
                let entry = runtime.devices.entry(device.id.clone()).or_default();
                if entry.last_poll_at == 0 {
                    // First sight: put the device in its own slot inside the
                    // interval rather than polling everything at once.
                    entry.last_poll_at = now - interval_ms + jitter(&device.id, interval_ms);
                }
                let due = entry.last_poll_at + interval_ms <= now;
                if due {
                    entry.last_poll_at = now;
                    runtime.in_flight.insert(device.id.clone());
                    if runtime.in_flight.len() == 1 {
                        runtime.cycle_start = now;
                    }
                }
                due
            };
            if !due {
                continue;
            }
            let app = app.clone();
            let gate = gate.clone();
            let state = state.clone();
            let device_id = device.id.clone();
            tokio::spawn(async move {
                // The permit is taken inside the task, so the scheduler never
                // waits and a slow device holds up only itself.
                let _permit = gate.acquire_owned().await;
                match poll::poll_device(&app, &device_id).await {
                    Ok(polled) => rules::evaluate(&app, &polled).await,
                    Err(err) => {
                        tracing::debug!(device = %device_id, %err, "a device could not be polled")
                    }
                }
                let finished = state.clock.now_ms();
                let mut runtime = state.runtime.lock();
                runtime.in_flight.remove(&device_id);
                if runtime.in_flight.is_empty() {
                    runtime.last_cycle_ms = (finished - runtime.cycle_start).max(0) as u64;
                }
            });
        }
    }
}

/// Roll samples up, drop what has aged out, and forget resolved alerts.
async fn maintenance(app: App) {
    let state = app.state::<MonitorState>();
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(MAINTENANCE_MS)).await;
        let now = state.clock.now_ms();
        if let Err(err) = run_maintenance(&app, now) {
            tracing::warn!(%err, "the monitoring maintenance pass failed");
        }
    }
}

pub fn run_maintenance(app: &App, now: i64) -> nocturne_app::AppResult<()> {
    let settings = MonitorSettings::load(app);
    app.store().samples_maintain(now)?;
    // The store keeps raw samples for two days and roll-ups for just over a
    // year; the settings can only ask for less than that.
    let raw_before = now - i64::from(settings.retention_days_raw) * 86_400_000;
    let rollup_before = now - i64::from(settings.retention_days_rollup) * 86_400_000;
    app.store().with(|conn| {
        conn.execute("DELETE FROM samples WHERE ts < ?1", [raw_before])?;
        conn.execute("DELETE FROM rollups WHERE bucket < ?1", [rollup_before])
    })?;
    alerts::prune(app, now - ALERT_HISTORY_DAYS * 86_400_000)?;
    Ok(())
}

/// React to what other features announce.
async fn events(app: App, mut stream: tokio::sync::broadcast::Receiver<nocturne_app::Event>) {
    loop {
        let event = match stream.recv().await {
            Ok(event) => event,
            // Falling behind the bus loses notifications, not state.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return,
        };
        match event.topic.as_str() {
            "config.changed" => {
                if let Some(device_id) = event.payload["device_id"].as_str()
                    && let Err(err) = rules::on_config_changed(&app, device_id, &event.payload)
                {
                    tracing::debug!(%err, "a configuration-change alert could not be raised");
                }
            }
            "devices.changed" if event.payload["change"] == "deleted" => {
                forget_devices(&app, &event.payload);
            }
            _ => {}
        }
    }
}

fn forget_devices(app: &App, payload: &Value) {
    let state = app.state::<MonitorState>();
    for id in payload["ids"].as_array().into_iter().flatten() {
        let Some(id) = id.as_str() else { continue };
        state.forget(id);
        let _ = interfaces::forget(app, id);
        let _ = alerts::forget_device(app, id);
        let _ = app.store().delete(crate::db::collections::DEVICES, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_is_stable_inside_the_interval_and_spreads_devices() {
        let interval = 60_000;
        let first = jitter("abcdefghijklmnop", interval);
        assert_eq!(first, jitter("abcdefghijklmnop", interval));
        assert!((0..interval).contains(&first));
        assert_eq!(jitter("anything", 0), 0);

        // A thousand ids should land in most of the interval's seconds.
        let mut buckets = std::collections::HashSet::new();
        for index in 0..1_000 {
            buckets.insert(jitter(&format!("device{index}"), interval) / 1_000);
        }
        assert!(buckets.len() > 50, "only {} slots used", buckets.len());
    }
}
