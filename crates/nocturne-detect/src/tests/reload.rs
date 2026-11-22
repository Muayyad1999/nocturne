//! `unplanned_reload`.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use nocturne_app::App;
use nocturne_app::jobs::{self, JobSpec};

use super::support::*;
use crate::catalogue::UNPLANNED_RELOAD as ID;

/// `logs.signal` as the log rules publish it (`publish` in
/// `nocturne-logs/src/rules/`) for a rule that emits `Reload`, such as the
/// built-in `builtin.ios.restart`.
fn signalled(app: &App, device_id: &str, kind: &str, at: i64) -> i64 {
    let entry_id = syslog_line(app, device_id, "syslog", at);
    app.publish(
        "logs.signal",
        &json!({
            "rule_id": "builtin.ios.restart", "rule": "IOS: device restarted", "kind": kind,
            "device_id": device_id, "device": "SW1", "severity": null,
            "title": "IOS: device restarted: reload", "interface": null, "groups": {},
            "entry_id": entry_id, "count": 1, "at": at,
        }),
    );
    entry_id
}

/// Run a job of this kind against a device to its end.
async fn ran(app: &App, kind: &str, device_id: &str) {
    let job = jobs::start(
        app,
        JobSpec::new(kind, "a job").devices(&[device_id.to_string()]),
        |_ctx| async move { Ok(json!({})) },
    );
    jobs::wait(app, &job.id).await.unwrap();
}

#[tokio::test]
async fn a_restart_nothing_explains_is_a_hit_naming_the_log_line() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();

    let entry_id = signalled(app, &sw1, "reload", at);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    let hit = &hits[0];
    assert_eq!(
        hit["title"],
        "SW1 restarted and nothing Nocturne was doing explains it"
    );
    assert_eq!(hit["severity"], "medium");
    assert_eq!(hit["device_id"], json!(sw1));
    assert_eq!(
        hit["evidence"],
        json!({
            "decided_from": "log",
            "log_entry_id": entry_id,
            "rule_id": "builtin.ios.restart",
            "stream": "syslog",
            "via": "syslog",
            "changes_checked": 0,
            "jobs_checked": 0,
            "grace_minutes": 15,
            "caution": "Syslog and SNMP v1/v2c traps are unauthenticated: anything that can reach the receiver could have sent this.",
        })
    );
    assert_eq!(stored_events(app, ID).len(), 1);
    let raised = world
        .fakes
        .raised_with(&format!("detect:unplanned_reload:{sw1}:reload"));
    assert_eq!(raised.len(), 1);
    assert_eq!(raised[0]["severity"], "medium");
}

#[tokio::test]
async fn any_other_signal_is_somebody_elses_business() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    for kind in ["interface_down", "login_failure", "config_changed", "alert"] {
        signalled(app, &sw1, kind, now());
    }
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
}

#[tokio::test]
async fn a_change_that_ran_within_the_grace_explains_the_restart() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();
    // The dead-man switch fired: the change failed and the device came back.
    *world.fakes.changes.lock() = vec![change("c1", &sw1, "failed", at - 9 * MINUTE)];

    signalled(app, &sw1, "reload", at);
    settle(app).await;
    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());

    // The same definition as for a configuration change: beyond the grace
    // it explains nothing.
    *world.fakes.changes.lock() = vec![change("c1", &sw1, "failed", at - 16 * MINUTE)];
    signalled(app, &sw1, "reload", at);
    settle(app).await;
    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["evidence"]["nearest_change"]["change_id"], "c1");
}

#[tokio::test]
async fn a_job_that_can_change_a_device_explains_the_restart_and_one_that_only_reads_does_not() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let sw2 = world.device("SW2", "10.0.0.2").await;

    let sw3 = world.device("SW3", "10.0.0.3").await;

    // An Ansible playbook ran against SW1 a moment ago. SW2 was only backed
    // up, and SW3 only had `show` commands run on it by the automation
    // feature — which cannot send anything else.
    ran(app, "automation.ansible", &sw1).await;
    ran(app, "config.backup", &sw2).await;
    ran(app, "automation.runner", &sw3).await;
    ran(app, "automation.playbook", &sw3).await;
    for device in [&sw1, &sw2, &sw3] {
        signalled(app, device, "reload", now());
    }
    settle(app).await;

    assert_eq!(
        recent(app, json!({"id": ID, "device_id": sw1})).await,
        Vec::<Value>::new()
    );
    for device in [&sw2, &sw3] {
        let hits = recent(app, json!({"id": ID, "device_id": device})).await;
        assert_eq!(hits.len(), 1, "{hits:#?}");
        // The jobs that only read were not counted; the evidence says how
        // many jobs were.
        assert_eq!(hits[0]["evidence"]["jobs_checked"], 0);
    }
}

#[tokio::test]
async fn a_device_left_to_its_dead_man_switch_restarts_as_the_change_said_it_would() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let sw2 = world.device("SW2", "10.0.0.2").await;
    let at = now();
    // Twenty-five minutes ago a change lost both devices. SW1 had its
    // dead-man switch armed (ten minutes to the reload, and the device took
    // a while to come back and say so); SW2 had none.
    let lost = |id: &str, device: &str, state: &str| {
        let mut change = change(id, device, "failed", at - 25 * MINUTE);
        change["devices"][0]["state"] = json!(state);
        change
    };
    *world.fakes.changes.lock() = vec![lost("c1", &sw1, "guard_fired"), lost("c2", &sw2, "failed")];

    signalled(app, &sw1, "reload", at);
    signalled(app, &sw2, "reload", at);
    settle(app).await;

    let hits = hits_of(app, ID).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["device_id"], json!(sw2));
}

#[tokio::test]
async fn a_lab_that_was_reset_restarted_its_own_devices_and_a_fault_in_it_did_not() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let lab_sw = world.lab_device();
    let sw1 = world.device("SW1", "10.0.0.1").await;
    // What `nocturne-lab` writes when a lab is reset (`labs.reset` in its
    // `api.rs`): the lab, not the devices in it.
    let lab_did = |action: &str, lab_id: &str| {
        app.audit(action, None, "a lab action", json!({"lab_id": lab_id}));
    };

    // Resetting the lab boots every node in it, and each says so.
    lab_did("labs.reset", "testlab");
    signalled(app, &lab_sw, "reload", now());
    // Hardware restarting in the same minute has nothing to do with it.
    signalled(app, &sw1, "reload", now());
    settle(app).await;
    assert_eq!(
        recent(app, json!({"id": ID, "device_id": lab_sw})).await,
        Vec::<Value>::new()
    );
    assert_eq!(
        recent(app, json!({"id": ID, "device_id": sw1})).await.len(),
        1
    );

    // Two minutes on, another lab is reset and a fault is injected into this
    // one: somebody is breaking it to see what notices, and this does.
    let later = now() + 2 * MINUTE;
    lab_did("labs.reset", "another-lab");
    lab_did("labs.fault.inject", "testlab");
    signalled(app, &lab_sw, "reload", later);
    settle(app).await;
    let hits = recent(app, json!({"id": ID, "device_id": lab_sw})).await;
    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0]["lab"], true);
}

#[tokio::test]
async fn the_same_restart_line_arriving_twice_is_one_restart() {
    let world = world(Build::FULL).await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;
    let at = now();
    let raw = "<189>7: *Mar  1 00:00:07.123: %SYS-5-RESTART: System restarted --";
    for offset in [0, 25] {
        let entry_id = syslog_text(app, &sw1, "syslog", at + offset, raw);
        app.publish(
            "logs.signal",
            &json!({
                "rule_id": "builtin.ios.restart", "rule": "IOS: device restarted",
                "kind": "reload", "device_id": sw1, "device": "SW1", "severity": null,
                "title": "IOS: device restarted", "interface": null, "groups": {},
                "entry_id": entry_id, "count": 1, "at": at + offset,
            }),
        );
    }
    settle(app).await;

    assert_eq!(hits_of(app, ID).await.len(), 1);
}

#[tokio::test]
async fn without_the_logs_feature_it_is_unavailable_with_the_reason_and_silent() {
    let world = world(Build {
        logs: false,
        ..Build::FULL
    })
    .await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    signalled(app, &sw1, "reload", now());
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    let entry = listed(app, ID).await;
    assert_eq!(entry["available"], false);
    assert_eq!(
        entry["unavailable_reason"],
        "the logs feature is not in this build, so nothing reports a restart (`logs.signal`)"
    );
}

#[tokio::test]
async fn without_the_change_engine_a_restart_is_not_called_unplanned() {
    let world = world(Build {
        changes: false,
        ..Build::FULL
    })
    .await;
    let app = &world.app;
    let sw1 = world.device("SW1", "10.0.0.1").await;

    signalled(app, &sw1, "reload", now());
    settle(app).await;

    assert_eq!(hits_of(app, ID).await, Vec::<Value>::new());
    let reason = listed(app, ID).await["unavailable_reason"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        reason.contains("`changes.list` is not in this build"),
        "{reason}"
    );
    assert_eq!(listed(app, ID).await.get("last_error"), None);
}
