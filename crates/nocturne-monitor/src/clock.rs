//! Where "now" comes from.
//!
//! Everything in this crate that needs the time asks the clock in
//! [`MonitorState`](crate::state::MonitorState) rather than the system clock,
//! so a test can pause tokio's timer and have timestamps, scheduling and
//! `for_s` windows all move together.

use std::sync::Arc;

/// Milliseconds since the epoch, as this crate sees them.
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
}

/// The real clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        nocturne_app::util::now_ms()
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
        self.base + self.start.elapsed().as_millis() as i64
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
        tokio::time::sleep(Duration::from_secs(90)).await;
        assert_eq!(clock.now_ms(), 1_700_000_090_000);
    }
}
