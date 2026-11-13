//! Where "now" comes from.
//!
//! Everything in this crate that needs the time asks the clock on
//! [`AutomationState`](crate::state::AutomationState), so a test can pause
//! tokio's timer and have scheduling, next-run computation and the timestamps
//! on run records all move together.

use std::sync::Arc;

/// Milliseconds since the epoch, as this crate sees them.
pub trait Clock: Send + Sync + 'static {
    fn now_ms(&self) -> i64;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        nocturne_app::util::now_ms()
    }
}

/// A clock that follows tokio's timer, so `tokio::time::pause` and `advance`
/// move it.
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

pub type SharedClock = Arc<dyn Clock>;
