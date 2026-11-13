//! The scheduler: spread, the concurrency cap, and a cycle the size of a
//! campus.

use std::time::Duration;

use serde_json::json;

use crate::state::MonitorState;
use crate::testing::{Harness, HostState};

/// Let the scheduler run for `seconds` of virtual time, in small steps so
/// that its own tick is seen.
async fn run_for(seconds: u64) {
    for _ in 0..(seconds * 4) {
        tokio::time::sleep(Duration::from_millis(250)).await;
        tokio::task::yield_now().await;
    }
}

/// How many polls a pinned device gets in a window.
///
/// A pinned device is dispatched on the scheduler's first tick, one tick
/// after the window opens, and every `interval_s` after that. The `- 1`
/// leaves a tick of slack at each end so the count does not depend on where
/// the 250 ms steps of [`run_for`] and the scheduler's own tick fall
/// relative to one another.
fn polls_in(window_s: u64, interval_s: u64) -> usize {
    (window_s / interval_s) as usize - 1
}

#[tokio::test(start_paused = true)]
async fn jitter_spreads_the_polls_across_the_interval() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let mut ids = Vec::new();
    for index in 0..40 {
        ids.push(
            harness
                .device(
                    &format!("SW{index}"),
                    &format!("10.0.{}.{}", index / 250, index % 250),
                    json!({"monitor": {"interval_s": 60}}),
                )
                .await,
        );
    }
    // The devices are deliberately *not* pinned here: the spread is what
    // this test is about. What it checks is that the scheduler puts each
    // device in the slot `jitter` gives it, so the expected count is read
    // back out of that same function rather than guessed at — the ids are
    // random, so any fixed range would only hold by luck.
    let jitters: Vec<i64> = ids
        .iter()
        .map(|id| crate::scheduler::jitter(id, 60_000))
        .collect();
    crate::scheduler::spawn(&harness.app);

    // A third of the way into a 60 s interval, the devices whose slot has
    // come round have been polled and the rest have not. A device is due at
    // the first tick at which `1 s + jitter` has passed, so the bounds allow
    // a tick either side.
    run_for(20).await;
    let polled = harness.probe.pinged.lock().len();
    let at_least = jitters.iter().filter(|slot| **slot <= 18_000).count();
    let at_most = jitters.iter().filter(|slot| **slot <= 21_000).count();
    assert!(
        (at_least..=at_most).contains(&polled),
        "{polled} devices polled in the first 20 s; the jitter of these ids puts it between \
         {at_least} and {at_most}"
    );
    // And a 60 s interval really is spread over the whole minute, rather
    // than every device being probed on one tick.
    assert!(
        at_most < ids.len(),
        "these ids all happen to fall in the first third ({jitters:?}); \
         `jitter`'s own test covers the distribution"
    );

    // By the end of the interval every one of them has been polled once.
    run_for(45).await;
    for id in &ids {
        let device = nocturne_app::inventory::get(&harness.app, id).unwrap();
        assert!(
            harness.probe.count(&device.address) >= 1,
            "{} was never polled",
            device.name
        );
    }
    harness.app.shutdown();
}

#[tokio::test(start_paused = true)]
async fn the_concurrency_cap_holds() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"concurrency": 4}}),
        )
        .await;
    for index in 0..60 {
        let address = format!("10.1.{}.{}", index / 250, index % 250);
        // Every probe takes a second, so they overlap if they are allowed to.
        harness.probe.set(
            &address,
            HostState {
                delay: Duration::from_secs(1),
                ..HostState::default()
            },
        );
        let id = harness
            .device(
                &format!("SW{index}"),
                &address,
                json!({"monitor": {"interval_s": 60}}),
            )
            .await;
        // All sixty are due on the same tick, so the cap is the only thing
        // that can hold them back: the peak is then the cap exactly, rather
        // than whatever the jitter of sixty random ids happens to overlap.
        harness.due_now(&id, 60);
    }
    crate::scheduler::spawn(&harness.app);
    run_for(70).await;
    assert_eq!(
        harness.probe.peak_in_flight(),
        4,
        "sixty devices wanted at once, four allowed in flight"
    );
    // Sixty devices four at a time is fifteen seconds of probing, so the
    // window also catches the start of their next interval: what matters is
    // that the cap costs nobody their poll.
    let seen: std::collections::HashSet<String> =
        harness.probe.pinged.lock().iter().cloned().collect();
    assert_eq!(
        seen.len(),
        60,
        "every device still gets polled, four at a time"
    );
    harness.app.shutdown();
}

#[tokio::test(start_paused = true)]
async fn one_slow_device_delays_nothing_else() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    harness.probe.set(
        "10.2.0.1",
        HostState {
            // A device that holds the line open for most of the interval.
            delay: Duration::from_secs(45),
            ..HostState::default()
        },
    );
    let slow = harness
        .device("SLOW", "10.2.0.1", json!({"monitor": {"interval_s": 60}}))
        .await;
    // Pinned, so the slow poll is under way from the first tick. Left to the
    // jitter of a random id, its one 60 s slot would fall outside this 40 s
    // window about a third of the time and there would be nothing slow to
    // work around.
    harness.due_now(&slow, 60);
    let mut quick = Vec::new();
    for index in 0..10 {
        let address = format!("10.2.1.{index}");
        let id = harness
            .device(
                &format!("SW{index}"),
                &address,
                json!({"monitor": {"interval_s": 10}}),
            )
            .await;
        harness.due_now(&id, 10);
        quick.push((address.clone(), id));
    }
    crate::scheduler::spawn(&harness.app);
    run_for(40).await;

    // Four 10 s slots fit in the window; three is the guaranteed floor.
    let wanted = polls_in(40, 10);
    for (address, _) in &quick {
        assert!(
            harness.probe.count(address) >= wanted,
            "{address} was polled {} times, not {wanted}, while one device was slow",
            harness.probe.count(address)
        );
    }
    assert_eq!(
        harness.probe.count("10.2.0.1"),
        1,
        "the slow device is still inside its single poll"
    );
    // And it is not started again while it is still going.
    let in_flight = harness
        .app
        .state::<MonitorState>()
        .runtime
        .lock()
        .in_flight
        .contains(&slow);
    assert!(in_flight);
    harness.app.shutdown();
}

#[tokio::test(start_paused = true)]
async fn a_two_thousand_device_cycle_finishes_inside_its_interval() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let count = 2_000u32;
    let mut addresses = Vec::with_capacity(count as usize);
    for index in 0..count {
        let address = format!(
            "10.{}.{}.{}",
            16 + index / 65_000,
            (index / 250) % 250,
            index % 250
        );
        harness
            .device(
                &format!("SW{index:04}"),
                &address,
                json!({"monitor": {"interval_s": 60}}),
            )
            .await;
        addresses.push(address);
    }
    let status = harness.rpc("monitor.status", json!({})).await;
    assert_eq!(status["devices_monitored"], count);

    // Not pinned: this is the realistic case, two thousand devices spread
    // across their interval by their own ids. Whatever those ids are, the
    // jitter is less than the interval, so one interval has to reach every
    // one of them — which is what "the cycle completes" means.
    crate::scheduler::spawn(&harness.app);
    run_for(75).await;

    let seen: std::collections::HashSet<String> =
        harness.probe.pinged.lock().iter().cloned().collect();
    let missed: Vec<&String> = addresses
        .iter()
        .filter(|address| !seen.contains(*address))
        .collect();
    assert!(
        missed.is_empty(),
        "{} of {count} devices were never polled in one interval, e.g. {:?}",
        missed.len(),
        &missed[..missed.len().min(5)]
    );
    // The cap is never exceeded. It is not reached either: these probes
    // answer instantly, so one finishes before the next is dispatched —
    // `the_concurrency_cap_holds` is the test that makes them overlap.
    let peak = harness.probe.peak_in_flight();
    assert!(peak <= 32, "{peak} probes in flight, the default cap is 32");

    let status = harness.rpc("monitor.status", json!({})).await;
    assert!(status["polls_per_min"].as_u64().unwrap() > 0, "{status:#?}");
    assert!(
        status["last_cycle_ms"].as_u64().unwrap() <= 60_000,
        "a cycle has to fit inside the interval: {status:#?}"
    );
    harness.app.shutdown();
}

#[tokio::test(start_paused = true)]
async fn the_scheduler_leaves_devices_alone_when_monitoring_is_switched_off() {
    let harness = Harness::new().await;
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"enabled": false}}),
        )
        .await;
    harness.device("SW1", "10.3.0.1", json!({})).await;
    harness
        .device("SW2", "10.3.0.2", json!({"monitor": {"enabled": false}}))
        .await;
    crate::scheduler::spawn(&harness.app);
    run_for(90).await;
    assert!(harness.probe.pinged.lock().is_empty());

    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"enabled": true}}),
        )
        .await;
    run_for(90).await;
    assert!(
        harness.probe.count("10.3.0.1") > 0,
        "the enabled device is polled again"
    );
    assert_eq!(
        harness.probe.count("10.3.0.2"),
        0,
        "a device with monitoring switched off stays untouched"
    );
    harness.app.shutdown();
}

#[tokio::test(start_paused = true)]
async fn deleting_a_device_clears_what_monitoring_kept_about_it() {
    let harness = Harness::new().await;
    harness.silence_rules().await;
    let id = harness.device("SW1", "10.4.0.1", json!({})).await;
    harness.poll(&id).await;
    crate::interfaces::replace(
        &harness.app,
        &id,
        &[crate::model::IfStatus {
            device_id: id.clone(),
            name: "Gi0/1".into(),
            ..crate::model::IfStatus::default()
        }],
    )
    .unwrap();
    harness
        .rpc(
            "alerts.raise",
            json!({"severity": "high", "title": "x", "device_id": id, "dedup_key": "k"}),
        )
        .await;

    crate::scheduler::spawn(&harness.app);
    harness.rpc("devices.delete", json!({"ids": [id]})).await;
    run_for(2).await;

    assert!(
        crate::interfaces::of_device(&harness.app, &id)
            .unwrap()
            .is_empty()
    );
    assert!(
        crate::alerts::open_of_device(&harness.app, &id)
            .unwrap()
            .is_empty()
    );
    assert!(
        harness
            .app
            .store()
            .get::<crate::model::DeviceRecord>(crate::db::collections::DEVICES, &id)
            .unwrap()
            .is_none()
    );
    harness.app.shutdown();
}
