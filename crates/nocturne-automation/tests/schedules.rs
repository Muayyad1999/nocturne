//! Schedules: the allow-list, the next-run computation, catching up once, and
//! never two runs of the same schedule at the same time.

mod common;

use common::{Gate, Scans, app_on, crash_copy, eventually, lab, lab_at, tick, tick_until};
use nocturne_automation::clock::Clock;
use nocturne_automation::db;
use nocturne_automation::state::AutomationState;
use serde_json::{Value, json};

/// 2024-06-10 10:00 UTC, a Monday.
const MONDAY: i64 = 1_718_013_600_000;

fn scans(lab: &nocturne_app::testing::Lab) -> u32 {
    *lab.app.state::<Scans>().0.lock().expect("lock")
}

async fn save(lab: &nocturne_app::testing::Lab, when: Value) -> Value {
    lab.app
        .rpc(
            "schedules.save",
            json!({"name": "Nightly scan", "when": when,
                   "action": {"method": "insights.scan", "params": {}}}),
        )
        .await
        .expect("schedules.save")
}

/// Rewrite a stored schedule's `next_run_at`, which is what a process that was
/// not running for a while comes back to.
fn set_due(lab: &nocturne_app::testing::Lab, id: &str, at: i64) {
    let mut stored: Value = lab
        .app
        .store()
        .get(db::SCHEDULES, id)
        .expect("the schedule")
        .expect("it exists");
    stored["next_run_at"] = json!(at);
    lab.app
        .store()
        .put(db::SCHEDULES, id, &stored)
        .expect("stored");
}

#[tokio::test]
async fn only_the_allow_listed_methods_can_be_put_on_a_schedule() {
    let lab = lab(&["SW1"]).await;
    // Anything that configures a device, opens a session, or is not a method
    // at all: never.
    for method in [
        "changes.apply",
        "changes.create",
        "devices.exec",
        "devices.delete",
        "nonsense.verb",
        "",
    ] {
        let err = lab
            .app
            .rpc(
                "schedules.save",
                json!({"name": "No", "when": {"type": "interval", "every_s": 3600},
                       "action": {"method": method}}),
            )
            .await
            .err()
            .unwrap_or_else(|| panic!("`{method}` must not be schedulable"));
        assert!(
            matches!(err.code, "forbidden" | "invalid"),
            "`{method}`: {err:?}"
        );
    }
    assert!(
        lab.app
            .rpc("schedules.list", json!({}))
            .await
            .expect("list")
            .as_array()
            .expect("array")
            .is_empty()
    );
}

#[tokio::test]
async fn a_method_that_changes_a_device_is_refused_and_the_message_lists_what_is_allowed() {
    let lab = lab(&["SW1"]).await;
    let err = lab
        .app
        .rpc(
            "schedules.save",
            json!({"name": "No", "when": {"type": "interval", "every_s": 3600},
                   "action": {"method": "changes.apply"}}),
        )
        .await
        .expect_err("not allowed");
    assert_eq!(err.code, "forbidden");
    assert!(
        err.message.contains("config.backup") && err.message.contains("a person approves"),
        "{}",
        err.message
    );

    // On the list, but not part of this build: a schedule that would never work.
    let err = lab
        .app
        .rpc(
            "schedules.save",
            json!({"name": "No", "when": {"type": "interval", "every_s": 3600},
                   "action": {"method": "reports.generate"}}),
        )
        .await
        .expect_err("not in this build");
    assert_eq!(err.code, "invalid");
    assert!(
        err.message.contains("not part of this build"),
        "{}",
        err.message
    );

    // A `when` that is not a schedule at all.
    let err = lab
        .app
        .rpc(
            "schedules.save",
            json!({"name": "No", "when": {"type": "cron", "expr": "oops"},
                   "action": {"method": "insights.scan"}}),
        )
        .await
        .expect_err("not a cron expression");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("five fields"), "{}", err.message);
}

#[tokio::test]
async fn saving_a_schedule_works_out_when_it_next_runs_and_preview_agrees() {
    let lab = lab(&["SW1"]).await;
    let saved = save(&lab, json!({"type": "daily", "at": "02:30"})).await;
    let next = saved["next_run_at"].as_i64().expect("a next run");
    assert!(next > nocturne_app::util::now_ms(), "it is in the future");

    let preview = lab
        .app
        .rpc(
            "schedules.preview",
            json!({"when": {"type": "daily", "at": "02:30"}, "count": 5}),
        )
        .await
        .expect("schedules.preview");
    let times: Vec<i64> = preview["next"]
        .as_array()
        .expect("times")
        .iter()
        .map(|time| time.as_i64().expect("ms"))
        .collect();
    assert_eq!(times.len(), 5);
    assert!(times.windows(2).all(|pair| pair[0] < pair[1]), "{times:?}");
    assert_eq!(times[0], next, "the preview is the same computation");
    // One day apart, give or take a clock change.
    for pair in times.windows(2) {
        let gap = pair[1] - pair[0];
        assert!(
            (23 * 3_600_000..=25 * 3_600_000).contains(&gap),
            "a daily schedule is a day apart: {gap} ms"
        );
    }

    // A disabled schedule has no next run.
    let disabled = lab
        .app
        .rpc(
            "schedules.save",
            json!({"id": saved["id"], "name": "Nightly scan", "enabled": false,
                   "when": {"type": "daily", "at": "02:30"},
                   "action": {"method": "insights.scan"}}),
        )
        .await
        .expect("schedules.save");
    assert!(disabled["next_run_at"].is_null());
}

#[tokio::test(start_paused = true)]
async fn the_scheduler_runs_a_due_schedule_and_records_what_happened() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let saved = save(&lab, json!({"type": "interval", "every_s": 60})).await;
    let id = saved["id"].as_str().expect("an id").to_string();
    assert_eq!(scans(&lab), 0);

    tick_until(90, || scans(&lab) == 1).await;

    let listed = lab
        .app
        .rpc("schedules.list", json!({}))
        .await
        .expect("schedules.list");
    let schedule = &listed[0];
    assert_eq!(schedule["last_ok"], json!(true));
    assert!(schedule["last_run_at"].as_i64().expect("a time") >= MONDAY);
    assert!(schedule["last_error"].is_null());
    let next = schedule["next_run_at"].as_i64().expect("a next run");
    assert!(
        next > schedule["last_run_at"].as_i64().expect("a time"),
        "the next run is after this one"
    );

    // And again, once.
    tick_until(90, || scans(&lab) == 2).await;
    assert_eq!(scans(&lab), 2);

    lab.app
        .rpc("schedules.delete", json!({"id": id}))
        .await
        .expect("schedules.delete");
    tick(120).await;
    assert_eq!(scans(&lab), 2, "a deleted schedule does not fire");
}

#[tokio::test(start_paused = true)]
async fn a_disabled_schedule_never_fires() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let saved = lab
        .app
        .rpc(
            "schedules.save",
            json!({"name": "Off", "enabled": false,
                   "when": {"type": "interval", "every_s": 30},
                   "action": {"method": "insights.scan"}}),
        )
        .await
        .expect("schedules.save");
    assert!(saved["next_run_at"].is_null());
    tick(120).await;
    assert_eq!(scans(&lab), 0);
}

#[tokio::test(start_paused = true)]
async fn a_schedule_missed_while_the_process_was_down_catches_up_once_not_once_per_slot() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let saved = save(&lab, json!({"type": "interval", "every_s": 60})).await;
    let id = saved["id"].as_str().expect("an id").to_string();
    // Seven slots went by with nobody at home.
    set_due(&lab, &id, MONDAY - 7 * 60_000);

    tick_until(5, || scans(&lab) == 1).await;
    // Several ticks later it is still one: the next run was computed from now,
    // not from the slot that was missed.
    tick(10).await;
    assert_eq!(
        scans(&lab),
        1,
        "a week of missed backups is not seven backups"
    );
}

#[tokio::test(start_paused = true)]
async fn a_schedule_missed_by_longer_than_the_catch_up_window_moves_on_without_running() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "automation", "value": {"catch_up_within_s": 3600}}),
        )
        .await
        .expect("a one-hour window");
    let saved = save(&lab, json!({"type": "interval", "every_s": 60})).await;
    let id = saved["id"].as_str().expect("an id").to_string();
    set_due(&lab, &id, MONDAY - 30 * 24 * 3_600_000);

    tick_until(5, || {
        lab.app
            .store()
            .get::<Value>(db::SCHEDULES, &id)
            .ok()
            .flatten()
            .and_then(|schedule| schedule["next_run_at"].as_i64())
            .is_some_and(|next| next > MONDAY)
    })
    .await;
    assert_eq!(scans(&lab), 0, "a month-old slot is not worth running");
}

#[tokio::test(start_paused = true)]
async fn a_schedule_whose_last_run_is_still_going_does_not_start_a_second_one() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let saved = save(&lab, json!({"type": "interval", "every_s": 30})).await;
    let id = saved["id"].as_str().expect("an id").to_string();
    let state = lab.app.state::<AutomationState>();

    // Exactly what a run in flight does: the schedule is marked as firing.
    let guard = state.start_firing(&id).expect("the first claim");
    tick(120).await;
    assert_eq!(
        scans(&lab),
        0,
        "four slots went by and none of them started"
    );
    assert_eq!(
        lab.app
            .rpc("schedules.run_now", json!({"id": id}))
            .await
            .expect_err("still running")
            .code,
        "conflict"
    );

    // When it ends, the schedule is still due and starts at the next tick.
    drop(guard);
    tick_until(5, || scans(&lab) == 1).await;
}

#[tokio::test]
async fn run_now_runs_the_action_and_returns_its_result() {
    let lab = lab(&["SW1"]).await;
    let saved = save(&lab, json!({"type": "daily", "at": "02:30"})).await;
    let result = lab
        .app
        .rpc("schedules.run_now", json!({"id": saved["id"]}))
        .await
        .expect("schedules.run_now");
    assert_eq!(result["scanned"], json!(1));
    assert_eq!(scans(&lab), 1);
    let listed = lab
        .app
        .rpc("schedules.list", json!({}))
        .await
        .expect("schedules.list");
    assert_eq!(listed[0]["last_ok"], json!(true));
}

#[tokio::test]
async fn a_schedule_can_run_a_task_and_the_run_says_a_schedule_started_it() {
    let lab = lab(&["SW1"]).await;
    let task = lab
        .app
        .rpc(
            "tasks.save",
            json!({"name": "Nightly versions", "select": {"filter": {"site": "Lab"}},
                   "action": {"type": "commands", "commands": ["show version"]}}),
        )
        .await
        .expect("tasks.save");
    let saved = lab
        .app
        .rpc(
            "schedules.save",
            json!({"name": "Nightly", "when": {"type": "daily", "at": "02:30"},
                   "action": {"method": "tasks.run", "params": {"id": task["id"]}}}),
        )
        .await
        .expect("schedules.save");

    let result = lab
        .app
        .rpc("schedules.run_now", json!({"id": saved["id"]}))
        .await
        .expect("schedules.run_now");
    let run_id = result["run_id"].as_str().expect("a run id").to_string();
    let run = lab
        .app
        .rpc("runner.get", json!({"run_id": run_id}))
        .await
        .expect("runner.get")["run"]
        .clone();
    assert_eq!(run["origin"], json!("schedule"));
    assert_eq!(run["task_id"], task["id"]);
    assert_eq!(run["ok"], json!(1));
    assert!(
        lab.fake("SW1")
            .seen()
            .iter()
            .any(|line| line == "show version"),
        "the task really ran"
    );
}

// ------------------------------------------- what a run in flight leaves

fn gate(app: &nocturne_app::App) -> std::sync::Arc<Gate> {
    app.state::<Gate>()
}

fn stored(app: &nocturne_app::App, id: &str) -> Option<Value> {
    app.store()
        .get::<Value>(db::SCHEDULES, id)
        .expect("the store answers")
}

/// A schedule on `discovery.scan`, which answers only when the test lets it,
/// due at the scheduler's next tick.
async fn held(app: &nocturne_app::App, now: i64) -> String {
    let saved = app
        .rpc(
            "schedules.save",
            json!({"name": "Hourly discovery", "when": {"type": "interval", "every_s": 3600},
                   "action": {"method": "discovery.scan", "params": {}}}),
        )
        .await
        .expect("schedules.save");
    let id = saved["id"].as_str().expect("an id").to_string();
    let mut due = stored(app, &id).expect("it was stored");
    due["next_run_at"] = json!(now - 1000);
    app.store().put(db::SCHEDULES, &id, &due).expect("stored");
    id
}

#[tokio::test(start_paused = true)]
async fn a_schedule_whose_action_is_still_going_has_already_moved_on_to_its_next_slot() {
    let (lab, clock) = lab_at(&["SW1"], MONDAY).await;
    let id = held(&lab.app, MONDAY).await;
    tick_until(5, || gate(&lab.app).calls() == 1).await;

    // The action has not answered. If the process ended now, this is what the
    // next start would find: the slot it is running is spent, and the run is
    // recorded as started with no outcome yet.
    let during = stored(&lab.app, &id).expect("still there");
    let started = during["last_run_at"]
        .as_i64()
        .expect("the run's start is recorded");
    assert!(started >= MONDAY);
    assert_eq!(
        during["next_run_at"],
        json!(started + 3_600_000),
        "the next slot is an hour after this run, not the one being run: {during}"
    );
    assert!(during["last_ok"].is_null(), "no outcome yet: {during}");

    gate(&lab.app).open(1);
    tick_until(5, || {
        stored(&lab.app, &id).is_some_and(|schedule| schedule["last_ok"] == json!(true))
    })
    .await;
    let after = stored(&lab.app, &id).expect("still there");
    assert_eq!(after["last_run_at"], json!(started));
    assert!(after["next_run_at"].as_i64().expect("a next run") > clock.now_ms());
}

#[tokio::test]
async fn a_scheduled_run_the_process_did_not_live_through_is_not_started_again_when_it_comes_back()
{
    let dir = tempfile::tempdir().expect("a data directory");
    let app = app_on(dir.path());
    let id = held(&app, nocturne_app::util::now_ms()).await;
    eventually(10, || gate(&app).calls() == 1).await;

    // The action never answers: the process ends here, and what it left on
    // disk is what the next start opens.
    let left = crash_copy(dir.path());
    let restarted = app_on(left.path());
    // Three of the scheduler's one-second ticks.
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    assert_eq!(
        gate(&restarted).calls(),
        0,
        "the slot was spent before the action was called, so it is not due again"
    );
    let after = stored(&restarted, &id).expect("the schedule is still there");
    assert_eq!(after["last_ok"], json!(false), "{after}");
    assert!(
        after["last_error"]
            .as_str()
            .is_some_and(|error| error.contains("Nocturne was closed")),
        "it says why there is no outcome: {after}"
    );
    assert!(after["next_run_at"].as_i64().expect("a next run") > nocturne_app::util::now_ms());
}

#[tokio::test(start_paused = true)]
async fn a_schedule_deleted_while_it_runs_stays_deleted() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let id = held(&lab.app, MONDAY).await;
    tick_until(5, || gate(&lab.app).calls() == 1).await;

    lab.app
        .rpc("schedules.delete", json!({"id": id}))
        .await
        .expect("schedules.delete");
    gate(&lab.app).open(1);
    let state = lab.app.state::<AutomationState>();
    tick_until(5, || !state.is_firing(&id)).await;

    assert!(
        stored(&lab.app, &id).is_none(),
        "the run did not bring it back"
    );
    tick(7200).await;
    assert_eq!(gate(&lab.app).calls(), 1, "and it never ran again");
}

#[tokio::test(start_paused = true)]
async fn a_schedule_switched_off_while_it_runs_stays_off_and_still_records_the_run() {
    let (lab, _clock) = lab_at(&["SW1"], MONDAY).await;
    let id = held(&lab.app, MONDAY).await;
    tick_until(5, || gate(&lab.app).calls() == 1).await;

    lab.app
        .rpc(
            "schedules.save",
            json!({"id": id, "name": "Renamed while it ran", "enabled": false,
                   "when": {"type": "interval", "every_s": 3600},
                   "action": {"method": "discovery.scan", "params": {}}}),
        )
        .await
        .expect("schedules.save");
    gate(&lab.app).open(1);
    let state = lab.app.state::<AutomationState>();
    tick_until(5, || !state.is_firing(&id)).await;

    let after = stored(&lab.app, &id).expect("still there");
    assert_eq!(after["enabled"], json!(false), "{after}");
    assert_eq!(after["name"], json!("Renamed while it ran"));
    assert!(
        after["next_run_at"].is_null(),
        "a schedule that is off has no next run"
    );
    assert_eq!(
        after["last_ok"],
        json!(true),
        "the run that was going is recorded"
    );
    tick(7200).await;
    assert_eq!(gate(&lab.app).calls(), 1, "and it does not run again");
}
