//! Where "now" comes from.
//!
//! Everything in this crate that needs the time asks the clock in
//! [`MonitorState`](crate::state::MonitorState) rather than the system clock,
//! so a test can pause tokio's timer and have timestamps, scheduling and
//! `for_s` windows all move together.
//!
//! The clock answers two different questions, and they must not be mixed up:
//!
//! * [`Clock::now_ms`] is the **time of day** — what a sample, an alert or a
//!   history row is stamped with. It is whatever the machine believes, and
//!   the machine changes its mind: NTP steps it, a person sets it, a laptop
//!   boots with a flat RTC battery an hour out.
//! * [`Clock::monotonic_ms`] is **how much time has gone by**. It only ever
//!   goes forwards and says nothing about the date. Every "is it time yet?"
//!   in this crate — the next poll, the next run of a check, the next CLI
//!   read — is a difference of two of these, so that a clock put right by an
//!   hour does not stop the polling for an hour.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

/// The time, as this crate sees it.
pub trait Clock: Send + Sync + 'static {
    /// Milliseconds since the epoch: the time of day, for timestamps. It can
    /// jump in either direction.
    fn now_ms(&self) -> i64;

    /// Milliseconds on a clock that only goes forwards, for cadence. The
    /// origin is arbitrary (and may be negative): only the difference of two
    /// readings means anything, and a reading is never comparable with
    /// [`Clock::now_ms`].
    fn monotonic_ms(&self) -> i64;
}

/// The real clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        nocturne_app::util::now_ms()
    }

    fn monotonic_ms(&self) -> i64 {
        // One origin for the whole process, so that readings taken through
        // different copies of this (unit) clock are comparable.
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        ORIGIN.get_or_init(Instant::now).elapsed().as_millis() as i64
    }
}

/// A clock that follows tokio's timer, so `tokio::time::pause` and
/// `advance` move it. Used by the tests.
#[derive(Clone, Debug)]
pub struct VirtualClock {
    base: i64,
    start: tokio::time::Instant,
}

impl VirtualClock {
    /// A clock reading `base` right now. Must be created inside a runtime.
    pub fn new(base: i64) -> VirtualClock {
        VirtualClock {
            base,
            start: tokio::time::Instant::now(),
        }
    }
}

impl Clock for VirtualClock {
    fn now_ms(&self) -> i64 {
        self.base + self.monotonic_ms()
    }

    fn monotonic_ms(&self) -> i64 {
        self.start.elapsed().as_millis() as i64
    }
}

/// A shared clock handle.
pub type SharedClock = Arc<dyn Clock>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test(start_paused = true)]
    async fn the_virtual_clock_follows_paused_time() {
        let clock = VirtualClock::new(1_700_000_000_000);
        assert_eq!(clock.now_ms(), 1_700_000_000_000);
        let before = clock.monotonic_ms();
        tokio::time::sleep(Duration::from_secs(90)).await;
        assert_eq!(clock.now_ms(), 1_700_000_090_000);
        assert_eq!(clock.monotonic_ms() - before, 90_000);
    }

    #[test]
    fn the_system_clock_measures_elapsed_time_from_one_origin() {
        // Two handles on the real clock have to agree, or a time written by
        // the scheduler could not be compared with one read by the poller.
        let (first, second) = (SystemClock, SystemClock);
        let earlier = first.monotonic_ms();
        std::thread::sleep(Duration::from_millis(20));
        let later = second.monotonic_ms();
        assert!(
            (15..5_000).contains(&(later - earlier)),
            "{} ms went by",
            later - earlier
        );
    }
}
