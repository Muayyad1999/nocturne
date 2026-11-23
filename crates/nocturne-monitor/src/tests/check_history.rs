//! What a check's history may claim.
//!
//! The history keeps changes, not runs, so every figure in it is an
//! inference: "up from nine to ten" rests on the runs in between having
//! happened. These tests are about the places where they did not — the
//! application was closed, the check was switched off, the old rows were
//! pruned, the list was cut short — and the history has to say *unknown*
//! rather than repeat the last thing it heard.

use std::time::Duration;

use serde_json::{Value, json};

use crate::checks::model::CheckStatus;
use crate::checks::outcome::Outcome;
use crate::testing::Harness;

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// A device that answers ping, with every rule switched off: these tests
/// are about the record, not about who is told.
async fn harness_with_device() -> (Harness, String) {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "127.0.0.1", json!({})).await;
    harness.probe.up("127.0.0.1");
    harness.silence_rules().await;
    (harness, id)
}

/// Record what a run found at a time of the test's choosing, so the
/// arithmetic owes nothing to how fast this machine ran the test.
fn found_at(harness: &Harness, check_id: &str, status: CheckStatus, at: i64) {
    let (check, _) = crate::checks::store::require(&harness.app, check_id).unwrap();
    let outcome = match status {
        CheckStatus::Down => Outcome::down("timeout", "nothing came back"),
        CheckStatus::CannotRun => Outcome::cannot_run("dns_resolve", "the name did not resolve"),
        _ => Outcome::up("ok", Some(1.0)),
    };
    crate::checks::store::record(&harness.app, &check, "SW1", &outcome, at, at)
        .unwrap()
        .expect("the check is as it was read a moment ago");
}

fn changes(history: &Value) -> Vec<(String, String)> {
    history["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| {
            (
                event["previous"].as_str().unwrap().to_string(),
                event["status"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn pairs(found: &[(String, String)]) -> Vec<(&str, &str)> {
    found
        .iter()
        .map(|(previous, status)| (previous.as_str(), status.as_str()))
        .collect()
}

// --------------------------------------------------- time nobody measured

#[tokio::test(start_paused = true)]
async fn time_in_which_nothing_ran_is_not_charged_to_the_last_status() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 600}),
        )
        .await;
    let start = harness.now();

    // An hour of answers, one run every ten minutes.
    for _ in 0..6 {
        harness.run_check(&check).await;
        tokio::time::sleep(Duration::from_secs(600)).await;
    }
    // Then it stops answering, and two runs see that.
    harness.probe.down("127.0.0.1");
    harness.run_check(&check).await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    harness.run_check(&check).await;
    let last_seen_down = harness.now();

    // The application is closed for the night. Nothing runs, and nothing
    // says so: no start-up code touches a check's state.
    tokio::time::sleep(Duration::from_secs(15 * 3_600)).await;
    harness.probe.up("127.0.0.1");
    let back = harness.run_check(&check).await;

    assert_eq!(
        back.previous,
        CheckStatus::Unknown,
        "what it was doing overnight is not known, so the run that came \
         back did not find it `down` beforehand"
    );
    assert_eq!(back.held_s, None, "and nothing lasted fifteen hours");
    assert_eq!(
        back.state.since,
        Some(harness.now()),
        "`up since` is when somebody next looked, not when it failed"
    );

    let history = harness
        .rpc("checks.history", json!({"id": check, "since": start}))
        .await;
    assert_eq!(
        pairs(&changes(&history)),
        [
            ("unknown", "up"),
            ("up", "down"),
            ("down", "unknown"),
            ("unknown", "up")
        ],
        "the watch is seen to stop, and to start again"
    );
    let stopped = &history["events"][2];
    assert_eq!(
        stopped["at"], last_seen_down,
        "it stopped at the last run, which is the last moment anything is known"
    );
    assert_eq!(stopped["held_s"], 600, "down was seen for ten minutes");

    let summary = &history["summary"];
    assert_eq!(summary["up_s"], 3_600);
    assert_eq!(
        summary["down_s"], 600,
        "ten minutes of outage were measured, not fifteen hours"
    );
    assert_eq!(summary["unknown_s"], 15 * 3_600);
    assert_eq!(summary["measured_s"], 4_200);
    assert_eq!(summary["down_count"], 1);
    assert_eq!(summary["last_down_for_s"], 600);
}

#[tokio::test(start_paused = true)]
async fn a_check_nobody_has_run_for_days_is_unknown_from_its_last_run_on() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 600}),
        )
        .await;
    let start = harness.now();
    for _ in 0..6 {
        harness.run_check(&check).await;
        tokio::time::sleep(Duration::from_secs(600)).await;
    }
    harness.run_check(&check).await;

    // Switched off. From that moment nothing is running it, and that is
    // known at once — not only once it has been quiet for longer than a
    // late run could explain.
    harness
        .rpc("checks.save", json!({"id": check, "enabled": false}))
        .await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    let soon = harness
        .rpc("checks.history", json!({"id": check, "since": start}))
        .await;
    assert_eq!(soon["summary"]["up_s"], 3_600);
    assert_eq!(
        soon["summary"]["unknown_s"], 600,
        "ten minutes switched off are ten minutes nobody measured"
    );

    // And left off for three days. No run comes afterwards to notice the
    // hole, so the history has to notice it for itself.
    tokio::time::sleep(Duration::from_secs(3 * 86_400 - 600)).await;

    let history = harness
        .rpc("checks.history", json!({"id": check, "since": start}))
        .await;
    let summary = &history["summary"];
    assert_eq!(
        summary["up_s"], 3_600,
        "an hour of answers is an hour of uptime, not three days and an hour"
    );
    assert_eq!(summary["unknown_s"], 3 * 86_400);
    assert_eq!(summary["measured_s"], 3_600);
    assert_eq!(summary["availability_pct"], 100.0);
}

#[tokio::test(start_paused = true)]
async fn a_run_that_is_merely_late_does_not_break_the_record() {
    // The other side of the rule: one skipped run is a late measurement,
    // not a hole. Without this the history of a busy installation would be
    // full of `unknown`.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 600}),
        )
        .await;
    let start = harness.now();
    harness.run_check(&check).await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    harness.run_check(&check).await;
    tokio::time::sleep(Duration::from_secs(1_200)).await;
    let late = harness.run_check(&check).await;
    assert!(!late.changed, "it said `up` before and says `up` now");

    let history = harness
        .rpc("checks.history", json!({"id": check, "since": start}))
        .await;
    assert_eq!(pairs(&changes(&history)), [("unknown", "up")]);
    assert_eq!(history["summary"]["up_s"], 1_800);
    assert_eq!(history["summary"]["unknown_s"], 0);
}

#[tokio::test(start_paused = true)]
async fn a_watch_that_resumes_on_what_it_last_heard_is_history_but_not_news() {
    // Every check comes back with its first run after a night with the
    // application closed. The history has to show the hole. The device's
    // timeline must not: a line per check per start, each saying that what
    // was answering is answering, is how a timeline stops being read.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 600}),
        )
        .await;
    let start = harness.now();
    let said = || -> Vec<String> {
        harness
            .events_of(&device)
            .into_iter()
            .map(|entry| entry.message)
            .filter(|message| message.contains("`ping`"))
            .collect()
    };

    harness.run_check(&check).await;
    assert_eq!(said().len(), 1, "the first answer is news: {:?}", said());

    tokio::time::sleep(Duration::from_secs(15 * 3_600)).await;
    harness.run_check(&check).await;
    let history = harness
        .rpc("checks.history", json!({"id": check, "since": start}))
        .await;
    assert_eq!(
        pairs(&changes(&history)),
        [("unknown", "up"), ("up", "unknown"), ("unknown", "up")],
        "the hole is on record"
    );
    assert_eq!(
        said().len(),
        1,
        "but finding it as it was left is not an event: {:?}",
        said()
    );

    // Finding it otherwise is, and the line claims no duration: how long it
    // had been down by then is exactly what is not known.
    tokio::time::sleep(Duration::from_secs(15 * 3_600)).await;
    harness.probe.down("127.0.0.1");
    harness.run_check(&check).await;
    let lines = said();
    assert_eq!(lines.len(), 2, "{lines:?}");
    let news = lines
        .iter()
        .find(|line| line.contains("not answering"))
        .unwrap_or_else(|| panic!("no line about the outage: {lines:?}"));
    assert!(!news.contains("it was"), "{news}");
}

#[tokio::test(start_paused = true)]
async fn tightening_a_checks_interval_does_not_read_as_a_stretch_nobody_measured() {
    // An hourly check, forty minutes after its last run, is made a
    // one-minute check. Forty minutes is far longer than a one-minute
    // check may go unrun, but nobody stopped watching: the hourly check was
    // on time, and the edit is the moment the new cadence begins.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 3_600}),
        )
        .await;
    let start = harness.now();
    harness.run_check(&check).await;
    tokio::time::sleep(Duration::from_secs(3_600)).await;
    harness.run_check(&check).await;
    let up_since = harness.check_state(&check).since;

    tokio::time::sleep(Duration::from_secs(2_400)).await;
    harness
        .rpc("checks.save", json!({"id": check, "interval_s": 60}))
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    let ran = harness.run_check(&check).await;

    assert!(!ran.changed, "it was up and is up: {ran:?}");
    assert_eq!(
        ran.state.since, up_since,
        "`up since` does not start again because the check was edited"
    );
    let history = harness
        .rpc("checks.history", json!({"id": check, "since": start}))
        .await;
    assert_eq!(pairs(&changes(&history)), [("unknown", "up")]);
    assert_eq!(history["summary"]["unknown_s"], 0);
    assert_eq!(history["summary"]["up_s"], 3_600 + 2_400 + 60);
}

#[tokio::test(start_paused = true)]
async fn an_edit_after_a_night_of_silence_does_not_paper_over_the_night() {
    // The other side of the rule above. The application was closed
    // overnight, and the first thing anybody does in the morning is rename
    // the check, before it has run again. The edit is not a measurement:
    // the night stays a night nobody measured.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 600}),
        )
        .await;
    let start = harness.now();
    harness.run_check(&check).await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    harness.run_check(&check).await;
    let last_run = harness.now();

    tokio::time::sleep(Duration::from_secs(15 * 3_600)).await;
    harness
        .rpc("checks.save", json!({"id": check, "name": "echo"}))
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    let back = harness.run_check(&check).await;

    assert_eq!(back.previous, CheckStatus::Unknown, "{back:?}");
    assert_eq!(back.state.since, Some(harness.now()));
    let history = harness
        .rpc("checks.history", json!({"id": check, "since": start}))
        .await;
    assert_eq!(
        pairs(&changes(&history)),
        [("unknown", "up"), ("up", "unknown"), ("unknown", "up")]
    );
    assert_eq!(history["events"][1]["at"], last_run);
    assert_eq!(history["summary"]["up_s"], 600);
    assert_eq!(history["summary"]["unknown_s"], 15 * 3_600 + 60);
}

#[tokio::test(start_paused = true)]
async fn edits_to_a_check_that_is_switched_off_do_not_keep_its_watch_alive() {
    // Switched off, renamed ten minutes later, switched on a quarter of an
    // hour after that. No two of those are far apart, but none of them is a
    // run either: a ten-minute check went twenty-five minutes unrun, and a
    // check that is off has no cadence for an edit to restart.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 600}),
        )
        .await;
    harness.run_check(&check).await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    harness.run_check(&check).await;

    tokio::time::sleep(Duration::from_secs(10)).await;
    harness
        .rpc("checks.save", json!({"id": check, "enabled": false}))
        .await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    harness
        .rpc("checks.save", json!({"id": check, "name": "echo"}))
        .await;
    tokio::time::sleep(Duration::from_secs(900)).await;
    harness
        .rpc("checks.save", json!({"id": check, "enabled": true}))
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    let back = harness.run_check(&check).await;

    assert_eq!(
        back.previous,
        CheckStatus::Unknown,
        "nothing ran for twenty-five minutes: {back:?}"
    );
    assert_eq!(back.held_s, None);
}

#[tokio::test]
async fn what_became_of_a_check_later_does_not_change_what_an_earlier_window_says() {
    // Up for three days, then pointed at another host — after which the
    // check, as it stands, has never run. A window that ends before any of
    // that is about the days in which it did run, and those were measured.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({"interval_s": 86_400}),
        )
        .await;
    let base = harness.now() - 4 * DAY;
    for day in 0..=3 {
        found_at(&harness, &check, CheckStatus::Up, base + day * DAY);
    }
    harness
        .rpc("checks.save", json!({"id": check, "host": "127.0.0.9"}))
        .await;
    assert_eq!(harness.check_state(&check).last_run_at, None);

    let earlier = harness
        .rpc(
            "checks.history",
            json!({"id": check, "since": base, "until": base + 2 * DAY}),
        )
        .await;
    assert_eq!(
        earlier["summary"]["up_s"],
        2 * 86_400,
        "the row that ends `up` is dated a day after this window closes"
    );
    assert_eq!(earlier["summary"]["unknown_s"], 0);

    // And a window that reaches past the repoint stops knowing where the
    // old target was last measured.
    let whole = harness
        .rpc("checks.history", json!({"id": check, "since": base}))
        .await;
    assert_eq!(whole["summary"]["up_s"], 3 * 86_400);
    assert_eq!(whole["summary"]["unknown_s"], 86_400);
}

// ----------------------------------------------------------------- pruning

#[tokio::test(start_paused = true)]
async fn pruning_old_changes_does_not_make_a_long_stable_check_unknown() {
    let (harness, device) = harness_with_device().await;
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"check_history_days": 1}}),
        )
        .await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 86_400}),
        )
        .await;
    // Up on the first day and on every day since: one change, long ago.
    harness.run_check(&check).await;
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(86_400)).await;
        harness.run_check(&check).await;
    }
    let now = harness.now();
    let ask = json!({"id": check, "since": now - 2 * DAY});

    let before = harness.rpc("checks.history", ask.clone()).await;
    assert_eq!(before["summary"]["up_s"], 2 * 86_400);

    // The hourly pass, with a retention shorter than the check has been up.
    crate::scheduler::run_maintenance(&harness.app, now).unwrap();

    let after = harness.rpc("checks.history", ask).await;
    assert_eq!(
        after["summary"], before["summary"],
        "pruning is housekeeping: it may not change what the history says \
         about a window that is still inside the retention"
    );
    assert_eq!(after["summary"]["availability_pct"], 100.0);
    assert_eq!(after["current"]["status"], "up");
}

#[tokio::test(start_paused = true)]
async fn a_history_that_was_already_pruned_bare_still_knows_what_the_check_has_been_saying() {
    // What an installation that ran the earlier pruning is left with: a
    // check that has been up for months and not one row to show for it.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 86_400}),
        )
        .await;
    harness.run_check(&check).await;
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(86_400)).await;
        harness.run_check(&check).await;
    }
    harness
        .app
        .store()
        .with(|conn| conn.execute("DELETE FROM monitor_check_events", []))
        .unwrap();

    let now = harness.now();
    let history = harness
        .rpc(
            "checks.history",
            json!({"id": check, "since": now - 2 * DAY}),
        )
        .await;
    assert_eq!(
        history["summary"]["up_s"],
        2 * 86_400,
        "`up since three days ago` is on the check itself, and runs have \
         kept confirming it"
    );
    assert_eq!(history["summary"]["unknown_s"], 0);
}

#[tokio::test]
async fn pruning_still_drops_every_old_change_but_the_one_the_retention_opens_on() {
    let (harness, device) = harness_with_device().await;
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"check_history_days": 1}}),
        )
        .await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({"interval_s": 86_400}),
        )
        .await;
    // Up, down a day later, up again the day after, and quiet since.
    let base = harness.now() - 5 * DAY;
    found_at(&harness, &check, CheckStatus::Up, base);
    found_at(&harness, &check, CheckStatus::Down, base + DAY);
    for day in 2..=5 {
        found_at(&harness, &check, CheckStatus::Up, base + day * DAY);
    }
    let now = base + 5 * DAY;

    crate::scheduler::run_maintenance(&harness.app, now).unwrap();

    let (kept, total) = crate::checks::store::events(&harness.app, &check, 0, now, 100).unwrap();
    assert_eq!(total, 1, "three changes are older than a day; one is kept");
    assert_eq!(
        (kept[0].at, kept[0].status),
        (base + 2 * DAY, CheckStatus::Up),
        "the newest of them: it is what the check was doing when the \
         retention begins"
    );
    let history = harness
        .rpc(
            "checks.history",
            json!({"id": check, "since": now - DAY, "until": now}),
        )
        .await;
    assert_eq!(history["summary"]["up_s"], 86_400);
}

#[tokio::test]
async fn pruning_keeps_the_change_the_retention_opens_on_when_newer_changes_follow_it() {
    // The row to keep is the newest of the *old* ones, not the newest the
    // check has: a change inside the retention does not make the one the
    // retention opens on dispensable.
    let (harness, device) = harness_with_device().await;
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"check_history_days": 1}}),
        )
        .await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({"interval_s": 86_400}),
        )
        .await;
    // Up, down a day later, up again the day after; then down twelve hours
    // before now, which is inside the one day that is kept.
    let base = harness.now() - 5 * DAY;
    found_at(&harness, &check, CheckStatus::Up, base);
    found_at(&harness, &check, CheckStatus::Down, base + DAY);
    for day in 2..=4 {
        found_at(&harness, &check, CheckStatus::Up, base + day * DAY);
    }
    found_at(
        &harness,
        &check,
        CheckStatus::Down,
        base + 4 * DAY + 12 * HOUR,
    );
    found_at(&harness, &check, CheckStatus::Down, base + 5 * DAY);
    let now = base + 5 * DAY;

    crate::scheduler::run_maintenance(&harness.app, now).unwrap();

    let (kept, total) = crate::checks::store::events(&harness.app, &check, 0, now, 100).unwrap();
    assert_eq!(
        kept.iter()
            .map(|change| (change.at, change.status))
            .collect::<Vec<_>>(),
        [
            (base + 2 * DAY, CheckStatus::Up),
            (base + 4 * DAY + 12 * HOUR, CheckStatus::Down)
        ]
    );
    assert_eq!(total, 2);
}

#[tokio::test]
async fn pruning_takes_the_history_of_a_check_that_no_longer_exists() {
    // Rows left behind by a run that was recorded after its check had been
    // deleted, which an earlier version allowed.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;
    harness
        .app
        .store()
        .with(|conn| {
            conn.execute(
                "INSERT INTO monitor_check_events
                    (check_id, device_id, at, status, previous, held_s, code, message)
                 VALUES ('gone', ?1, ?2, 'down', 'unknown', NULL, 'timeout', '')",
                rusqlite::params![device, harness.now()],
            )
        })
        .unwrap();

    crate::scheduler::run_maintenance(&harness.app, harness.now()).unwrap();

    let left: Vec<String> = harness
        .app
        .store()
        .with(|conn| {
            let mut statement =
                conn.prepare("SELECT DISTINCT check_id FROM monitor_check_events")?;
            statement.query_map([], |row| row.get(0))?.collect()
        })
        .unwrap();
    assert_eq!(
        left,
        [check],
        "the live check keeps its row; the orphan goes"
    );
}

#[tokio::test]
async fn a_window_that_opens_before_the_oldest_row_reads_what_that_row_says_came_before() {
    // The window reaches back past what is left of the history. The oldest
    // row still says what it replaced and for how long, and that is used
    // rather than thrown away.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({"interval_s": 86_400}),
        )
        .await;
    let base = harness.now() - 4 * DAY;
    for day in 0..=3 {
        found_at(&harness, &check, CheckStatus::Up, base + day * DAY);
    }
    // Down an hour into the fourth day, having been up for three days and
    // an hour; still down an hour after that.
    found_at(&harness, &check, CheckStatus::Down, base + 3 * DAY + HOUR);
    found_at(
        &harness,
        &check,
        CheckStatus::Down,
        base + 3 * DAY + 2 * HOUR,
    );
    harness
        .app
        .store()
        .with(|conn| {
            conn.execute(
                "DELETE FROM monitor_check_events WHERE at < ?1",
                [base + DAY],
            )
        })
        .unwrap();

    let history = harness
        .rpc(
            "checks.history",
            json!({"id": check, "since": base + DAY, "until": base + 3 * DAY + 2 * HOUR}),
        )
        .await;
    assert_eq!(pairs(&changes(&history)), [("up", "down")]);
    let summary = &history["summary"];
    assert_eq!(summary["up_s"], 2 * 86_400 + 3_600);
    assert_eq!(summary["down_s"], 3_600);
    assert_eq!(summary["unknown_s"], 0);
}

// ------------------------------------------------------------ a short list

#[tokio::test]
async fn the_summary_is_computed_from_every_change_however_short_the_list_is_cut() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({"interval_s": 86_400}),
        )
        .await;
    // Three one-minute outages, then six quiet days.
    let base = harness.now() - 7 * DAY;
    for (step, status) in [
        CheckStatus::Up,
        CheckStatus::Down,
        CheckStatus::Up,
        CheckStatus::Down,
        CheckStatus::Up,
        CheckStatus::Down,
        CheckStatus::Up,
    ]
    .into_iter()
    .enumerate()
    {
        found_at(&harness, &check, status, base + step as i64 * MINUTE);
    }
    for day in 1..=6 {
        found_at(&harness, &check, CheckStatus::Up, base + day * DAY);
    }
    let until = base + 6 * DAY;

    let whole = harness
        .rpc(
            "checks.history",
            json!({"id": check, "since": base, "until": until}),
        )
        .await;
    assert_eq!(whole["events"].as_array().unwrap().len(), 7);
    assert_eq!(whole["summary"]["down_s"], 180);
    assert_eq!(whole["summary"]["up_s"], 6 * 86_400 - 180);
    assert_eq!(whole["summary"]["down_count"], 3);

    let cut = harness
        .rpc(
            "checks.history",
            json!({"id": check, "since": base, "until": until, "limit": 2}),
        )
        .await;
    assert_eq!(
        cut["summary"], whole["summary"],
        "how many rows the caller wanted to look at does not change how \
         the check spent the week"
    );
    assert_eq!(
        pairs(&changes(&cut)),
        [("up", "down"), ("down", "up")],
        "a list cut short keeps the newest changes, oldest first"
    );
    assert_eq!(cut["events"][1]["at"], base + 6 * MINUTE);
    assert_eq!(cut["truncated"], true, "and says that it was cut");
    assert_eq!(cut["events_total"], 7);
    assert_eq!(whole["truncated"], false);
}
