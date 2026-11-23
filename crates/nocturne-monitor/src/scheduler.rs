//! The scheduler: one task that decides what to poll and when.
//!
//! Every monitored device gets its own slot inside its interval, chosen from
//! a hash of its id, so that a thousand switches on a 60-second interval are
//! spread across the minute instead of all being probed on the tick. Each
//! poll runs as its own task behind a semaphore, which is what makes "one
//! slow device delays nothing else" and "never more than `concurrency`
//! probes in flight" both true at once.

use std::collections::HashSet;
use std::sync::Arc;

use nocturne_app::{App, inventory};
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::settings::MonitorSettings;
use crate::state::MonitorState;
use crate::{alerts, checks, interfaces, poll, rules};

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
/// How often the scheduler looks for firing notices that were held back by
/// a maintenance window or a snooze and are not any more. An alert nothing
/// evaluates again is announced at most this long after its window closes.
const ANNOUNCE_MS: i64 = 15_000;

/// Whether the pass over the alerts that still owe a notice is due.
///
/// Like everything else here it is a difference of two readings of the
/// clock that cannot be set back: a timer armed for "when the window ends"
/// would be a date, and the machine changes its mind about dates.
pub(crate) fn announcing_is_due(last: Option<i64>, now: i64) -> bool {
    last.is_none_or(|last| now - last >= ANNOUNCE_MS)
}

/// Whether a check whose last run *began* at `last_run_at` is due at `now`.
///
/// Its own interval decides and nothing else does: not the device's poll
/// interval, not another check's. Worth a named function because this is the
/// one claim the scheduler makes about cadence that can be stated exactly —
/// what a check's realised spacing turns out to be also depends on how long
/// its probe holds the slot, which is not a promise.
///
/// The two times only have to be readings of the same clock. The scheduler
/// hands it readings of the one that cannot go backwards
/// ([`Clock::monotonic_ms`](crate::clock::Clock::monotonic_ms)); see the
/// note in [`scheduler`] for why.
pub(crate) fn check_is_due(last_run_at: i64, interval_ms: i64, now: i64) -> bool {
    last_run_at + interval_ms <= now
}

/// An offset inside an interval, in milliseconds. Stable for an id, evenly
/// spread across ids.
///
/// Used for devices and for service checks alike: a thousand checks on a
/// 60-second interval are spread across the minute rather than all firing on
/// the first tick after a start.
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
    // Devices that are monitored at all. Wider than `devices`, which only
    // holds the ones there is a way to *poll*: a device with no ICMP, no
    // SNMP and no command line can still have service checks on it.
    let mut monitored: HashSet<String> = HashSet::new();
    let mut services: Vec<crate::checks::model::ServiceCheck> = Vec::new();
    // `None` forces the first tick to read the inventory.
    let mut listed_at: Option<i64> = None;
    let mut announced_at: Option<i64> = None;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(TICK_MS)).await;
        // Before the switch below, not after it: with polling switched off
        // the log rules, the detections and the insights still raise alerts,
        // and what those owe is owed all the same.
        let tick = state.monotonic();
        if announcing_is_due(announced_at, tick) {
            announced_at = Some(tick);
            if let Err(err) = alerts::announce_owed(&app, state.now()) {
                tracing::warn!(%err, "the alerts that still owe a notification could not be read");
            }
        }
        let settings = MonitorSettings::load(&app);
        if !settings.enabled {
            continue;
        }
        if settings.concurrency != limit {
            limit = settings.concurrency;
            gate = Arc::new(Semaphore::new(limit as usize));
        }
        // Everything this loop decides is cadence — has an interval gone by
        // since the last poll, the last run, the last look at the inventory
        // — so `now` here is a reading of the clock that only goes forwards,
        // never the time of day.
        //
        // The time of day is the machine's opinion, and the machine changes
        // it: NTP steps it, a person sets it, a laptop boots an hour fast
        // and is corrected a minute later. When due-ness was worked out from
        // it, a clock put back by an hour stopped every poll and every check
        // for an hour — `last + interval <= now` stays false until `now`
        // catches up — with no gap and no error to show for it, and devices
        // added in that hour were never picked up.
        //
        // The alternative was to keep the time of day and notice the step:
        // if `now` is before a stored time, clamp that time. That needs a
        // rule for each stored time (and a new one for each that is added
        // later), it re-polls everything at once after the step, and it does
        // nothing for a step *forwards*, which makes everything due at once.
        // Asking a clock that cannot step has no such cases. What a poll or
        // a run *finds* is still stamped with the time of day, by the poll
        // and the run themselves.
        let now = state.monotonic();
        // The list is a few seconds old by design, except across a change
        // to a check: run from the old copy, an edited check would be
        // probed once more at the target it has just been taken off (and
        // the result thrown away, with its turn, when it came back).
        let checks_changed = std::mem::take(&mut state.runtime.lock().checks_changed);
        if checks_changed || listed_at.is_none_or(|listed| now - listed >= INVENTORY_MS) {
            match inventory::list(&app, &inventory::DeviceFilter::default()) {
                Ok(rows) => {
                    let all: Vec<nocturne_app::model::Device> =
                        rows.into_iter().map(|row| row.device).collect();
                    monitored = all
                        .iter()
                        .filter(|device| device.monitor.enabled)
                        .map(|device| device.id.clone())
                        .collect();
                    devices = all
                        .into_iter()
                        .filter(|device| device.monitor.enabled && poll::plan(device).any())
                        .collect();
                    listed_at = Some(now);
                }
                Err(err) => {
                    tracing::warn!(%err, "the inventory could not be read");
                    continue;
                }
            }
            match checks::store::enabled(&app) {
                Ok(rows) => services = rows,
                Err(err) => tracing::warn!(%err, "the service checks could not be read"),
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
                // First sight: put the device in its own slot inside the
                // interval rather than polling everything at once.
                let last = *entry
                    .last_poll_mono
                    .get_or_insert_with(|| now - interval_ms + jitter(&device.id, interval_ms));
                let due = last + interval_ms <= now;
                if due {
                    entry.last_poll_mono = Some(now);
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
                let finished = state.monotonic();
                let mut runtime = state.runtime.lock();
                runtime.in_flight.remove(&device_id);
                if runtime.in_flight.is_empty() {
                    runtime.last_cycle_ms = (finished - runtime.cycle_start).max(0) as u64;
                }
            });
        }

        // ------------------------------------------------- service checks
        //
        // The same tick, the same semaphore and the same jitter as the
        // devices above: one scheduler, so a thousand checks cannot escape
        // the concurrency cap by being a different kind of work.
        for check in &services {
            // Switching monitoring off for a device switches off everything
            // about it, services included; so do the switches for the whole
            // installation. One rule, shared with everything else that asks
            // whether a check is live.
            let watched = checks::watched(
                &settings,
                monitored.contains(&check.device_id),
                check.enabled,
            );
            if watched.is_err() {
                continue;
            }
            let interval_ms = i64::from(check.interval_s.max(5)) * 1_000;
            let due = {
                let mut runtime = state.runtime.lock();
                if runtime.checks_in_flight.contains(&check.id) {
                    // Still running from last time: a check that takes
                    // longer than its interval runs as often as it can,
                    // never twice at once.
                    continue;
                }
                let entry = runtime.checks.entry(check.id.clone()).or_default();
                let last = *entry
                    .last_run_mono
                    .get_or_insert_with(|| now - interval_ms + jitter(&check.id, interval_ms));
                let due = check_is_due(last, interval_ms, now);
                if due {
                    entry.last_run_mono = Some(now);
                    runtime.checks_in_flight.insert(check.id.clone());
                }
                due
            };
            if !due {
                continue;
            }
            let app = app.clone();
            let gate = gate.clone();
            let state = state.clone();
            let check = check.clone();
            tokio::spawn(async move {
                let _permit = gate.acquire_owned().await;
                if let Err(err) = checks::run_and_record(&app, &check).await {
                    tracing::debug!(check = %check.id, %err, "a service check could not be recorded");
                }
                state.runtime.lock().checks_in_flight.remove(&check.id);
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
    checks::store::prune(
        app,
        now - i64::from(settings.check_history_days) * 86_400_000,
    )?;
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
            // Created, updated or imported: any of them can be the moment
            // monitoring is switched off for a device.
            "devices.changed" => stand_down_unmonitored(&app, &event.payload),
            _ => {}
        }
    }
}

/// Close the rule alerts of every device in a `devices.changed` event whose
/// monitoring is now off.
///
/// Nothing polls such a device or runs its checks, so nothing would ever
/// evaluate it again and whatever was firing on it would fire for good. This
/// is the one moment at which something still looks.
fn stand_down_unmonitored(app: &App, payload: &Value) {
    let now = app.state::<MonitorState>().now();
    let mut any = false;
    for id in payload["ids"].as_array().into_iter().flatten() {
        let Some(device) = id.as_str().and_then(|id| inventory::get(app, id).ok()) else {
            continue;
        };
        if device.monitor.enabled {
            continue;
        }
        any = true;
        if let Err(err) = rules::stand_down(app, &device, now) {
            tracing::warn!(device = %device.id, %err, "the alerts of an unmonitored device could not be closed");
        }
    }
    if any {
        // Its checks have just stopped counting as up or down.
        app.publish(
            "monitor.status_changed",
            &serde_json::json!({"what": "checks"}),
        );
    }
}

fn forget_devices(app: &App, payload: &Value) {
    let state = app.state::<MonitorState>();
    for id in payload["ids"].as_array().into_iter().flatten() {
        let Some(id) = id.as_str() else { continue };
        state.forget(id);
        let _ = interfaces::forget(app, id);
        let _ = alerts::forget_device(app, id);
        // A check belongs to its device: when the device goes, so do its
        // checks and everything they found.
        if let Ok(rows) = checks::store::of_device(app, id) {
            for (check, _) in rows {
                state.forget_check(&check.id);
            }
        }
        let _ = checks::store::forget_device(app, id);
        let _ = app.store().delete(crate::db::collections::DEVICES, id);
        let _ = app.store().delete(crate::db::collections::IFINDEX, id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_is_due_on_its_own_interval_and_on_no_other() {
        let last = 1_700_000_000_000;
        assert!(!check_is_due(last, 10_000, last + 9_999));
        assert!(check_is_due(last, 10_000, last + 10_000));
        // The two-minute check next to it is not dragged along by the
        // ten-second one, nor the ten-second one held back by it.
        assert!(!check_is_due(last, 120_000, last + 10_000));
        assert!(check_is_due(last, 120_000, last + 120_000));
        // Nor does the device's hourly poll interval come into it.
        assert!(check_is_due(last, 10_000, last + 3_600_000));
    }

    #[test]
    fn the_pass_over_owed_notices_comes_round_every_fifteen_seconds() {
        assert!(announcing_is_due(None, 0), "and at once after a start");
        assert!(!announcing_is_due(Some(1_000), 15_999));
        assert!(announcing_is_due(Some(1_000), 16_000));
    }

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
