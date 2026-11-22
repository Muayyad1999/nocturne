//! Detections: a short, fixed catalogue of things worth telling somebody
//! about that no single log line and no single metric shows, because each
//! needs two sources put side by side.
//!
//! Specification: `docs/dev/features/detect.md` (conventions:
//! `docs/dev/features/README.md`).
//!
//! It is not a rule language and not a SIEM. A detection only **tells**: a
//! hit goes into the `event` stream with the ids of the records it was
//! decided from, is published as `detections.hit`, and is raised through
//! `alerts.raise` when this build has an alert list. Nothing here connects
//! to a device, drafts a change or triggers a write.
//!
//! This crate depends on the core only. Every other feature is reached by
//! method name ([`sources`]) or heard through the events it publishes
//! ([`watch`]), and a detection whose source is not in the build lists
//! itself as unavailable, with the reason, and stays silent
//! ([`catalogue::unavailable_reason`]).

pub mod catalogue;
mod checks;
mod hits;
pub mod model;
mod planned;
mod rpc;
pub mod settings;
mod sources;
mod watch;

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::Mutex;

use nocturne_app::AppBuilder;

use crate::model::CheckError;

/// More remembered findings than this is not a busy day, it is a leak.
const REMEMBERED: usize = 4_096;

/// How long after a log line the same text from the same device is the same
/// line received again — a datagram the network duplicated, a device that
/// logs to two addresses which both end here — and not a second event. Long
/// enough for a relay, short enough that a device which stamps nothing on
/// its lines can still report a second edit.
const SAME_LINE_MS: i64 = 10_000;

/// This feature's runtime state. None of it is the record of anything: hits
/// are in the `event` stream and settings in the store, so a restart loses
/// only what is rebuilt from the next event.
#[derive(Default)]
pub struct DetectState {
    /// Which neighbouring features are in the build (see [`sources::features`]).
    features: OnceLock<sources::Features>,
    /// Detection id → the last check that could not be made.
    errors: Mutex<HashMap<&'static str, CheckError>>,
    /// Finding → when it was last reported, for the one detection whose
    /// source repeats itself on every connection attempt.
    sightings: Mutex<HashMap<String, i64>>,
    /// Change id → the `updated_at` it was judged at, so a change announced
    /// twice in the same state is judged once.
    judged: Mutex<HashMap<String, i64>>,
    /// Detection and device → the fingerprint of the last log line that was
    /// reported for them and when, so the same line arriving twice is one
    /// sighting.
    lines: Mutex<HashMap<String, (u64, i64)>>,
    /// The switches as last announced, so `detections.changed` is published
    /// once per change however it was made.
    announced: Mutex<Option<(bool, Vec<String>)>>,
}

impl DetectState {
    /// Whether this finding has not been reported within `repeat_ms`; if so
    /// it is now.
    fn first_sighting(&self, finding: &str, at: i64, repeat_ms: i64) -> bool {
        let mut sightings = self.sightings.lock();
        if sightings
            .get(finding)
            .is_some_and(|last| at - last < repeat_ms)
        {
            return false;
        }
        if sightings.len() >= REMEMBERED {
            sightings.retain(|_, last| at - *last < repeat_ms);
        }
        if sightings.len() >= REMEMBERED {
            // Still full of live findings: forgetting them costs a repeated
            // hit, keeping them costs memory without bound.
            sightings.clear();
        }
        sightings.insert(finding.to_string(), at);
        true
    }

    /// Whether this state of this change has not been judged yet; if so it
    /// is now.
    fn first_judgement(&self, change_id: &str, updated_at: i64) -> bool {
        let mut judged = self.judged.lock();
        if judged.get(change_id) == Some(&updated_at) {
            return false;
        }
        if judged.len() >= REMEMBERED {
            judged.clear();
        }
        judged.insert(change_id.to_string(), updated_at);
        true
    }

    /// Whether this is the line last reported for this detection and device,
    /// received again within moments; if it is not, it is now the last one.
    fn same_line_again(&self, detection: &str, device_id: &str, fingerprint: u64, at: i64) -> bool {
        let key = format!("{detection}:{device_id}");
        let mut lines = self.lines.lock();
        if lines.get(&key).is_some_and(|(last, last_at)| {
            *last == fingerprint && (at - last_at).abs() < SAME_LINE_MS
        }) {
            return true;
        }
        if lines.len() >= REMEMBERED {
            lines.clear();
        }
        lines.insert(key, (fingerprint, at));
        false
    }

    /// A check could not be made. Returns whether that is news: the
    /// detection had no failure on record, or a different one.
    fn check_failed(&self, detection: &'static str, at: i64, message: String) -> bool {
        let before = self.errors.lock().insert(
            detection,
            CheckError {
                at,
                message: message.clone(),
            },
        );
        before.is_none_or(|before| before.message != message)
    }

    /// A check went through. Returns whether that clears an earlier failure.
    fn check_made(&self, detection: &'static str) -> bool {
        self.errors.lock().remove(detection).is_some()
    }

    fn last_error(&self, detection: &str) -> Option<CheckError> {
        self.errors.lock().get(detection).cloned()
    }
}

/// Register this feature's settings, state, RPC methods and its listener.
pub fn init(builder: &mut AppBuilder) {
    settings::declare(builder);
    builder.state(DetectState::default());
    rpc::register(builder);
    builder.on_start(|app| {
        // Subscribed here, not inside the task: an event published between
        // the application being built and the task first running would
        // otherwise be missed.
        let events = app.subscribe();
        rpc::remember(app, &settings::Settings::load(app));
        watch::start(app, events);
    });
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod state_tests {
    use super::*;

    const DAY: i64 = 86_400_000;

    #[test]
    fn the_same_finding_is_reported_once_until_the_repeat_time_has_passed() {
        let state = DetectState::default();
        assert!(state.first_sighting("sw1:22:SHA256:a", 1_000, DAY));
        assert!(!state.first_sighting("sw1:22:SHA256:a", 2_000, DAY));
        assert!(!state.first_sighting("sw1:22:SHA256:a", 1_000 + DAY - 1, DAY));
        assert!(state.first_sighting("sw1:22:SHA256:a", 1_000 + DAY, DAY));
        // Another key from the same host is another finding.
        assert!(state.first_sighting("sw1:22:SHA256:b", 2_000, DAY));
    }

    #[test]
    fn a_change_is_judged_once_per_state_it_reaches() {
        let state = DetectState::default();
        assert!(state.first_judgement("c1", 100));
        assert!(!state.first_judgement("c1", 100));
        // Rolled back later: saved again, so judged again.
        assert!(state.first_judgement("c1", 200));
        assert!(state.first_judgement("c2", 100));
    }

    #[test]
    fn the_same_line_again_within_moments_is_the_same_line_and_any_other_is_not() {
        let state = DetectState::default();
        assert!(!state.same_line_again("d", "sw1", 11, 1_000));
        assert!(state.same_line_again("d", "sw1", 11, 1_040));
        assert!(state.same_line_again("d", "sw1", 11, 1_000 + SAME_LINE_MS - 1));
        // Another line, another device and another detection are not it.
        assert!(!state.same_line_again("d", "sw1", 12, 1_050));
        assert!(!state.same_line_again("d", "sw2", 12, 1_050));
        assert!(!state.same_line_again("e", "sw1", 12, 1_050));
        // The same words a while later are a device saying them again.
        assert!(!state.same_line_again("d", "sw1", 12, 1_050 + SAME_LINE_MS));
    }

    #[test]
    fn a_failed_check_is_news_once_until_it_fails_differently() {
        let state = DetectState::default();
        assert!(state.check_failed("d", 1, "the database is locked".into()));
        assert!(!state.check_failed("d", 2, "the database is locked".into()));
        assert_eq!(state.last_error("d").unwrap().at, 2);
        assert!(state.check_failed("d", 3, "no answer".into()));
        assert!(state.check_made("d"));
        assert!(state.check_failed("d", 4, "no answer".into()));
    }

    #[test]
    fn remembered_findings_do_not_grow_without_bound() {
        let state = DetectState::default();
        for n in 0..(REMEMBERED as i64 + 10) {
            assert!(state.first_sighting(&format!("host{n}"), n, DAY));
        }
        assert!(state.sightings.lock().len() <= REMEMBERED);
    }
}
