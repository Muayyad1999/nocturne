//! Runtime state: the clock, and who is allowed to touch which device.
//!
//! The one rule this file exists for: **two runs never work on the same
//! device at the same time.** A schedule that fires while its previous run is
//! still going, a trigger that fires in the middle of a scheduled backup, a
//! person pressing Run twice — all of them end up here. A device that is
//! already taken is recorded as *skipped, with the run that holds it named*,
//! which is a result; it is never silently dropped and the second run never
//! starts a second session.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::clock::{SharedClock, SystemClock};

/// Who is holding a device.
#[derive(Clone, Debug, PartialEq)]
pub struct Busy {
    pub run_id: String,
    pub run_name: String,
    pub since: i64,
}

pub struct AutomationState {
    pub clock: SharedClock,
    devices: Mutex<HashMap<String, Busy>>,
    /// Schedules and triggers with a run in flight, by id.
    firing: Mutex<HashSet<String>>,
}

impl Default for AutomationState {
    fn default() -> Self {
        AutomationState::with_clock(Arc::new(SystemClock))
    }
}

impl AutomationState {
    pub fn with_clock(clock: SharedClock) -> AutomationState {
        AutomationState {
            clock,
            devices: Mutex::new(HashMap::new()),
            firing: Mutex::new(HashSet::new()),
        }
    }

    pub fn now_ms(&self) -> i64 {
        self.clock.now_ms()
    }

    /// Take a device for the length of a run, or say who has it.
    ///
    /// The lease is released when the returned guard is dropped — including
    /// when the run is cancelled or panics, which is why it is a guard and not
    /// a pair of calls.
    pub fn claim(
        self: &Arc<Self>,
        device_id: &str,
        run_id: &str,
        run_name: &str,
    ) -> Result<DeviceLease, Busy> {
        let mut devices = self.devices.lock();
        if let Some(busy) = devices.get(device_id) {
            return Err(busy.clone());
        }
        devices.insert(
            device_id.to_string(),
            Busy {
                run_id: run_id.to_string(),
                run_name: run_name.to_string(),
                since: self.clock.now_ms(),
            },
        );
        Ok(DeviceLease {
            state: self.clone(),
            device_id: device_id.to_string(),
        })
    }

    pub fn holder(&self, device_id: &str) -> Option<Busy> {
        self.devices.lock().get(device_id).cloned()
    }

    /// Mark a schedule or trigger as firing. `None` means it already is, which
    /// is how "the previous run has not finished" is answered.
    pub fn start_firing(self: &Arc<Self>, id: &str) -> Option<FiringGuard> {
        if !self.firing.lock().insert(id.to_string()) {
            return None;
        }
        Some(FiringGuard {
            state: self.clone(),
            id: id.to_string(),
        })
    }

    pub fn is_firing(&self, id: &str) -> bool {
        self.firing.lock().contains(id)
    }
}

/// Holds one device for one run. Released on drop.
pub struct DeviceLease {
    state: Arc<AutomationState>,
    device_id: String,
}

impl std::fmt::Debug for DeviceLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceLease")
            .field("device_id", &self.device_id)
            .finish()
    }
}

impl DeviceLease {
    pub fn device_id(&self) -> &str {
        &self.device_id
    }
}

impl Drop for DeviceLease {
    fn drop(&mut self) {
        self.state.devices.lock().remove(&self.device_id);
    }
}

/// Holds a schedule or trigger "in flight". Released on drop.
pub struct FiringGuard {
    state: Arc<AutomationState>,
    id: String,
}

impl Drop for FiringGuard {
    fn drop(&mut self) {
        self.state.firing.lock().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_is_held_by_one_run_until_the_lease_is_dropped() {
        let state = Arc::new(AutomationState::default());
        let lease = state.claim("dev1", "run1", "Nightly backup").unwrap();
        let busy = state.claim("dev1", "run2", "Ad-hoc").unwrap_err();
        assert_eq!(busy.run_id, "run1");
        assert_eq!(busy.run_name, "Nightly backup");
        // Another device is unaffected.
        assert!(state.claim("dev2", "run2", "Ad-hoc").is_ok());
        drop(lease);
        assert!(state.claim("dev1", "run2", "Ad-hoc").is_ok());
    }

    #[test]
    fn a_schedule_that_is_still_running_does_not_fire_again() {
        let state = Arc::new(AutomationState::default());
        let guard = state.start_firing("sched1").expect("first");
        assert!(state.start_firing("sched1").is_none());
        assert!(state.is_firing("sched1"));
        drop(guard);
        assert!(!state.is_firing("sched1"));
        assert!(state.start_firing("sched1").is_some());
    }
}
