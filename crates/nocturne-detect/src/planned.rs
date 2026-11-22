//! The one definition of "no approved change".
//!
//! Two detections ask it (a configuration that changed, a device that
//! restarted) and it must not drift between them, so it lives here and
//! nowhere else: a change explains something that happened to a device when
//! it names that device and either
//!
//! * the pipeline is working on it now — the change is `applying`, or a job
//!   holds a change that has already run (a rollback in progress); or
//! * it has run — `applied`, `rolled_back`, `failed` or `partially_applied`,
//!   every state in which the pipeline has had its hands on a device — and it
//!   reached that state within `grace_minutes` either side of the event.
//!
//! A device the change skipped, or never reached, is not explained by it. A
//! device the change left to its dead-man switch (`guard_fired`) is explained
//! for an hour longer: what the switch does, it does after the change is over.

use serde_json::{Value, json};

use nocturne_app::App;

use crate::sources::{self, Change, Gap};

/// When something happened: a moment (`from == to`) for a log line, the
/// stretch between two backups for a configuration that differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub from: i64,
    pub to: i64,
}

impl Span {
    pub fn moment(at: i64) -> Span {
        Span { from: at, to: at }
    }

    /// How far outside the span a time lies; zero inside it.
    fn distance(self, at: i64) -> i64 {
        if at < self.from {
            self.from - at
        } else if at > self.to {
            at - self.to
        } else {
            0
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// The pipeline accounts for it; `change_id` says which change.
    Explained { change_id: String },
    /// No change accounts for it. `checked` is how many changes for the
    /// device were looked at, `nearest` the one that came closest in time —
    /// so a hit can show that the answer was "none" out of several, and how
    /// far off the best candidate was.
    Unexplained {
        checked: usize,
        nearest: Option<Value>,
    },
}

/// How long after a change is over its dead-man switch can still be heard.
///
/// A target in `guard_fired` is a device the pipeline lost and left to the
/// switch it had armed: a timed reload, or a commit that undoes itself. That
/// happens up to the switch's minutes after the change has been written up
/// as failed, and a device that reloads takes minutes more to say so. The
/// change's summary does not carry the minutes, so this is a bound on them
/// rather than the number: the default is ten.
const GUARD_REACH_MS: i64 = 60 * 60_000;

/// The pipeline has run this change, whatever came of it.
fn has_run(state: &str) -> bool {
    matches!(
        state,
        "applied" | "rolled_back" | "failed" | "partially_applied"
    )
}

/// Judge one device against the changes that name it.
pub fn judge(changes: &[Change], device_id: &str, span: Span, grace_ms: i64) -> Verdict {
    let mut nearest: Option<(i64, &Change)> = None;
    let mut checked = 0;
    for change in changes {
        // The list was asked for by device, but a build whose change engine
        // ignores the filter must not make every change explain every device.
        let target = change.device(device_id);
        if !change.devices.is_empty() && target.is_none() {
            continue;
        }
        checked += 1;
        if change.state == "applying" {
            return Verdict::Explained {
                change_id: change.id.clone(),
            };
        }
        if !has_run(&change.state) {
            continue;
        }
        let target_state = target.and_then(|target| target.state.as_deref());
        if matches!(target_state, Some("pending" | "skipped")) {
            continue;
        }
        // A planning job also sets `job_id`, but only on a change that has
        // not run; on one that has, the job is a rollback at work right now.
        if change.job_id.is_some() {
            return Verdict::Explained {
                change_id: change.id.clone(),
            };
        }
        let distance = span.distance(change.updated_at);
        // The switch fires after the change is over, never before it.
        let switch_pending = target_state == Some("guard_fired")
            && span.from > change.updated_at
            && distance <= grace_ms + GUARD_REACH_MS;
        if distance <= grace_ms || switch_pending {
            return Verdict::Explained {
                change_id: change.id.clone(),
            };
        }
        if nearest.is_none_or(|(best, _)| distance < best) {
            nearest = Some((distance, change));
        }
    }
    Verdict::Unexplained {
        checked,
        nearest: nearest.map(|(_, change)| {
            json!({
                "change_id": change.id,
                "state": change.state,
                "updated_at": change.updated_at,
            })
        }),
    }
}

/// Ask the change engine, then judge.
pub async fn approved_change(
    app: &App,
    device_id: &str,
    span: Span,
    grace_ms: i64,
) -> Result<Verdict, Gap> {
    let changes = sources::changes_for(app, device_id).await?;
    Ok(judge(&changes, device_id, span, grace_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::ChangeDevice;

    const MINUTE: i64 = 60_000;
    const GRACE: i64 = 15 * MINUTE;
    const NOON: i64 = 1_763_000_000_000;

    fn change(id: &str, state: &str, updated_at: i64) -> Change {
        Change {
            id: id.into(),
            state: state.into(),
            updated_at,
            devices: vec![ChangeDevice {
                device_id: "d1".into(),
                state: None,
                protected_hits: 0,
            }],
            ..Change::default()
        }
    }

    fn explained(changes: &[Change], span: Span) -> bool {
        matches!(judge(changes, "d1", span, GRACE), Verdict::Explained { .. })
    }

    #[test]
    fn a_change_that_ran_explains_what_happened_within_the_grace_either_side_and_no_further() {
        for state in ["applied", "rolled_back", "failed", "partially_applied"] {
            for (offset, expected) in [
                (0, true),
                (GRACE, true),
                (-GRACE, true),
                (GRACE + 1, false),
                (-GRACE - 1, false),
            ] {
                assert_eq!(
                    explained(&[change("c1", state, NOON + offset)], Span::moment(NOON)),
                    expected,
                    "{state} at {offset} ms from the event"
                );
            }
        }
    }

    #[test]
    fn a_change_that_never_ran_explains_nothing_however_recent() {
        for state in ["draft", "planned", "approved", "cancelled"] {
            assert!(
                !explained(&[change("c1", state, NOON)], Span::moment(NOON)),
                "{state}"
            );
        }
    }

    #[test]
    fn a_change_being_applied_explains_it_whenever_it_was_last_saved() {
        assert!(explained(
            &[change("c1", "applying", NOON - 3 * 60 * MINUTE)],
            Span::moment(NOON)
        ));
    }

    #[test]
    fn a_rollback_at_work_explains_it_but_a_planning_job_does_not() {
        let mut rolling_back = change("c1", "applied", NOON - 3 * 60 * MINUTE);
        rolling_back.job_id = Some("job1".into());
        assert!(explained(&[rolling_back], Span::moment(NOON)));

        let mut planning = change("c2", "planned", NOON);
        planning.job_id = Some("job2".into());
        assert!(!explained(&[planning], Span::moment(NOON)));
    }

    #[test]
    fn a_device_the_change_skipped_is_not_explained_by_it() {
        for (state, expected) in [
            ("skipped", false),
            ("pending", false),
            ("applied", true),
            ("guard_fired", true),
            ("rollback_failed", true),
        ] {
            let mut ran = change("c1", "partially_applied", NOON);
            ran.devices[0].state = Some(state.into());
            assert_eq!(explained(&[ran], Span::moment(NOON)), expected, "{state}");
        }
    }

    #[test]
    fn a_device_left_to_its_dead_man_switch_is_explained_for_an_hour_after_and_not_before() {
        let lost = |state: &str| {
            let mut failed = change("c1", "failed", NOON);
            failed.devices[0].state = Some(state.into());
            failed
        };
        let hour = 60 * MINUTE;
        for (offset, expected) in [
            // The switch reloads the device ten minutes on, and the device
            // takes ten more to come back and say so.
            (20 * MINUTE, true),
            (hour + GRACE, true),
            (hour + GRACE + 1, false),
            // Before the change there was no switch to fire.
            (-GRACE, true),
            (-GRACE - 1, false),
        ] {
            assert_eq!(
                explained(&[lost("guard_fired")], Span::moment(NOON + offset)),
                expected,
                "{offset} ms from the end of the change"
            );
        }
        // A device the change merely failed on has no switch still to fire.
        assert!(!explained(
            &[lost("failed")],
            Span::moment(NOON + 20 * MINUTE)
        ));
    }

    #[test]
    fn a_change_for_another_device_explains_nothing() {
        let mut other = change("c1", "applied", NOON);
        other.devices[0].device_id = "d2".into();
        assert_eq!(
            judge(&[other], "d1", Span::moment(NOON), GRACE),
            Verdict::Unexplained {
                checked: 0,
                nearest: None
            }
        );
    }

    #[test]
    fn a_change_anywhere_between_two_backups_explains_the_difference_between_them() {
        let span = Span {
            from: NOON - 8 * 60 * MINUTE,
            to: NOON,
        };
        assert!(explained(
            &[change("c1", "applied", NOON - 4 * 60 * MINUTE)],
            span
        ));
        assert!(explained(
            &[change("c1", "applied", span.from - GRACE)],
            span
        ));
        assert!(!explained(
            &[change("c1", "applied", span.from - GRACE - 1)],
            span
        ));
    }

    #[test]
    fn an_unexplained_verdict_names_the_change_that_came_closest() {
        let verdict = judge(
            &[
                change("far", "applied", NOON - 9 * 60 * MINUTE),
                change("near", "rolled_back", NOON + 40 * MINUTE),
                change("idle", "draft", NOON),
            ],
            "d1",
            Span::moment(NOON),
            GRACE,
        );
        assert_eq!(
            verdict,
            Verdict::Unexplained {
                checked: 3,
                nearest: Some(json!({
                    "change_id": "near",
                    "state": "rolled_back",
                    "updated_at": NOON + 40 * MINUTE,
                })),
            }
        );
    }
}
