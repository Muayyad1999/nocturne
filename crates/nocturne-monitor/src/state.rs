//! What monitoring keeps in memory between polls.
//!
//! Counters live here and nowhere else: they are only meaningful next to the
//! reading before them, and a restart deliberately loses them so that the
//! first poll afterwards produces a gap rather than a rate computed across an
//! unknown amount of time.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::clock::{Clock, SharedClock, SystemClock};
use crate::probe::{NetProbe, Reachability, SharedProbe};
use crate::rates::{Counter, TimeBase};
use crate::snmp::{RealSnmp, Sessions, SharedSnmp};

/// The counters of one interface as last read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IfCounters {
    pub in_octets: Option<Counter>,
    pub out_octets: Option<Counter>,
    pub in_errors: Option<Counter>,
    pub out_errors: Option<Counter>,
    pub in_discards: Option<Counter>,
    pub out_discards: Option<Counter>,
}

/// Which kind of read produced a set of counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CounterSource {
    Snmp,
    Cli,
}

/// What the previous poll of one device left behind.
#[derive(Clone, Debug, Default)]
pub struct DeviceRuntime {
    /// The device's own notion of time at the previous reading.
    pub time_base: Option<TimeBase>,
    /// The poller's clock at the previous reading, for `TimeBase::Wall`.
    pub counters_at: i64,
    pub interfaces: HashMap<String, IfCounters>,
    /// The read the counters above came from. The two sources run on
    /// different cadences, so a poll in which SNMP failed says nothing about
    /// counters the CLI left, and the other way round.
    pub counters_from: Option<CounterSource>,
    /// When this device was last polled and when its CLI was last read, as
    /// readings of [`Clock::monotonic_ms`] and `None` for never. Not times
    /// of day: these three only ever answer "is it due yet?", and that must
    /// not depend on what the machine currently believes the date to be.
    pub last_poll_mono: Option<i64>,
    pub last_cli_mono: Option<i64>,
    /// When SNMP interfaces were last handed to the collected-data store.
    pub last_collect_mono: Option<i64>,
}

impl DeviceRuntime {
    /// Forget the counters but keep the schedule: used after a reboot and
    /// after a poll that could not read them.
    pub fn reset_counters(&mut self) {
        self.time_base = None;
        self.interfaces.clear();
        self.counters_from = None;
    }
}

/// Counts polls over the last minute, for `monitor.status`.
#[derive(Debug, Default)]
pub struct PollRate {
    /// When the recent polls happened, oldest first, as readings of
    /// [`Clock::monotonic_ms`]: a minute is a length of time, not a stretch
    /// of the calendar.
    recent: Vec<i64>,
}

impl PollRate {
    pub fn record(&mut self, at: i64) {
        self.recent.push(at);
        self.trim(at);
    }

    pub fn per_minute(&mut self, now: i64) -> u64 {
        self.trim(now);
        self.recent.len() as u64
    }

    fn trim(&mut self, now: i64) {
        self.recent.retain(|at| now - *at < 60_000);
    }
}

/// What the scheduler remembers about one service check.
///
/// Only when it last ran: everything a check *found* is in the database, so
/// that a restart loses the schedule (which is cheap to rebuild) and not the
/// state (which is not).
#[derive(Clone, Copy, Debug, Default)]
pub struct CheckRuntime {
    /// When the last run began, as a reading of [`Clock::monotonic_ms`];
    /// `None` until the scheduler has given the check its slot.
    pub last_run_mono: Option<i64>,
}

#[derive(Default)]
pub struct Runtime {
    pub devices: HashMap<String, DeviceRuntime>,
    /// Devices being polled right now, so a slow poll is not started twice.
    pub in_flight: HashSet<String>,
    /// When each service check last ran, and which are running now — the
    /// same pair of facts the device poller keeps, for the same reasons.
    pub checks: HashMap<String, CheckRuntime>,
    pub checks_in_flight: HashSet<String>,
    /// A check was saved or deleted since the scheduler last read its list,
    /// so the copy it holds is not to be run from.
    pub checks_changed: bool,
    /// How long the last full round of polls took, in milliseconds.
    pub last_cycle_ms: u64,
    /// When the round now in flight started ([`Clock::monotonic_ms`]).
    pub cycle_start: i64,
    pub polls: PollRate,
    /// Conditions that are true but have not yet held for their `for_s`,
    /// keyed by the alert's dedup key.
    pub pending: HashMap<String, i64>,
    /// Whether the built-in rules have been seeded in this process.
    pub seeded: bool,
}

/// This feature's runtime state, registered on the app.
pub struct MonitorState {
    pub clock: SharedClock,
    pub probe: SharedProbe,
    pub snmp: SharedSnmp,
    pub sessions: Sessions,
    pub runtime: Mutex<Runtime>,
    /// Whether the scheduler starts with the app. Tests drive polls
    /// themselves, and start the scheduler only where it is what is under
    /// test.
    pub autostart: bool,
}

impl Default for MonitorState {
    fn default() -> Self {
        MonitorState {
            clock: Arc::new(SystemClock),
            probe: Arc::new(NetProbe::new()),
            snmp: Arc::new(RealSnmp),
            sessions: Sessions::default(),
            runtime: Mutex::new(Runtime::default()),
            autostart: true,
        }
    }
}

impl MonitorState {
    /// A state with the clock and the probes replaced, for tests.
    pub fn fake(
        clock: Arc<dyn Clock>,
        probe: Arc<dyn Reachability>,
        snmp: SharedSnmp,
    ) -> MonitorState {
        MonitorState {
            clock,
            probe,
            snmp,
            sessions: Sessions::default(),
            runtime: Mutex::new(Runtime::default()),
            autostart: false,
        }
    }

    /// The time of day, for timestamps.
    pub fn now(&self) -> i64 {
        self.clock.now_ms()
    }

    /// A reading of the clock that only goes forwards, for cadence.
    pub fn monotonic(&self) -> i64 {
        self.clock.monotonic_ms()
    }

    /// Run `change` on one device's runtime record, creating it if needed.
    pub fn device<T>(&self, device_id: &str, change: impl FnOnce(&mut DeviceRuntime) -> T) -> T {
        let mut runtime = self.runtime.lock();
        change(runtime.devices.entry(device_id.to_string()).or_default())
    }

    /// Run `change` on one check's schedule, creating it if needed.
    pub fn check<T>(&self, check_id: &str, change: impl FnOnce(&mut CheckRuntime) -> T) -> T {
        let mut runtime = self.runtime.lock();
        change(runtime.checks.entry(check_id.to_string()).or_default())
    }

    /// Forget a check's schedule, so the scheduler gives it a fresh slot
    /// inside its interval, and have the scheduler read its list again
    /// before it runs anything. Called when a check is saved (its interval
    /// may have changed, or where it points) or deleted.
    pub fn forget_check(&self, check_id: &str) {
        let mut runtime = self.runtime.lock();
        runtime.checks.remove(check_id);
        runtime.checks_changed = true;
    }

    /// Drop everything remembered about a device (it left the inventory).
    pub fn forget(&self, device_id: &str) {
        self.runtime.lock().devices.remove(device_id);
        self.sessions.forget(device_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_poll_rate_only_counts_the_last_minute() {
        let mut rate = PollRate::default();
        rate.record(1_000);
        rate.record(2_000);
        assert_eq!(rate.per_minute(3_000), 2);
        assert_eq!(rate.per_minute(70_000), 0);
    }

    #[test]
    fn resetting_counters_keeps_the_schedule() {
        let mut device = DeviceRuntime {
            time_base: Some(TimeBase::Ticks(10)),
            counters_from: Some(CounterSource::Snmp),
            last_poll_mono: Some(500),
            ..DeviceRuntime::default()
        };
        device
            .interfaces
            .insert("Gi0/1".into(), IfCounters::default());
        device.reset_counters();
        assert!(device.time_base.is_none() && device.interfaces.is_empty());
        assert_eq!(device.counters_from, None);
        assert_eq!(device.last_poll_mono, Some(500));
    }
}
