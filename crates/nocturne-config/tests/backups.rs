//! Backups: what is stored, what is not stored twice, and what is reported
//! when a device cannot be read.

mod support;

use nocturne_app::jobs::{self, JobState};
use serde_json::json;
use support::{Events, audits, backup, backup_device, lab, load};

#[tokio::test]
async fn the_first_backup_stores_both_configurations_and_announces_itself() {
    let lab = lab(&["SW1"]).await;
    let mut events = Events::new(&lab.app);
    let result = backup_device(&lab, "SW1").await;

    assert_eq!(result["backed_up"], 1);
    assert_eq!(result["first"], 1);
    assert_eq!(result["changed"], 0);
    assert_eq!(result["skipped"], json!([]));
    assert_eq!(result["failed"], json!([]));

    let first = events.of("config.first_backup");
    assert_eq!(first.len(), 2, "one per kind: {first:?}");
    assert_eq!(first[0]["device_id"], lab.id("SW1"));
    assert_eq!(first[0]["kind"], "running");
    assert_eq!(first[1]["kind"], "startup");

    let latest = lab
        .app
        .rpc("config.latest", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the stored configuration");
    assert!(latest["text"].as_str().unwrap().contains("hostname SW1"));
    assert_eq!(latest["redacted"], true);
    assert_eq!(latest["version"]["kind"], "running");
    assert_eq!(latest["version"]["origin"], "manual");
    assert!(latest["version"]["lines"].as_u64().unwrap() > 5);
    assert!(latest["version"]["bytes"].as_u64().unwrap() > 50);

    // The display lines the device prints are not part of the configuration.
    let text = latest["text"].as_str().unwrap();
    assert!(!text.contains("Building configuration"), "{text}");
    assert!(!text.contains("Current configuration"), "{text}");

    assert_eq!(audits(&lab.app, "config.backup").len(), 1);
}

#[tokio::test]
async fn an_unchanged_device_does_not_get_a_second_version() {
    let lab = lab(&["SW1"]).await;
    backup_device(&lab, "SW1").await;
    let before = lab
        .app
        .rpc("config.versions", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("versions");
    let first_id = before[0]["id"].clone();
    let first_seen = before[0]["first_seen"].as_i64().unwrap();

    let mut events = Events::new(&lab.app);
    let result = backup_device(&lab, "SW1").await;
    assert_eq!(result["unchanged"], 1);
    assert_eq!(result["changed"], 0);
    assert_eq!(result["first"], 0);

    let after = lab
        .app
        .rpc("config.versions", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("versions");
    assert_eq!(after.as_array().unwrap().len(), 1, "no new version");
    assert_eq!(after[0]["id"], first_id);
    assert_eq!(after[0]["first_seen"].as_i64().unwrap(), first_seen);
    assert!(
        after[0]["last_seen"].as_i64().unwrap() >= first_seen,
        "the version was seen again"
    );
    assert!(events.of("config.changed").is_empty());
    assert!(events.of("config.first_backup").is_empty());
}

#[tokio::test]
async fn a_changed_configuration_is_a_new_version_with_an_event_an_audit_and_a_diff() {
    let lab = lab(&["SW1"]).await;
    backup_device(&lab, "SW1").await;

    support::configure(
        &lab,
        "SW1",
        &[
            "ntp server 192.0.2.1",
            "interface GigabitEthernet0/2",
            "description desk 14",
        ],
        false,
    )
    .await;

    let mut events = Events::new(&lab.app);
    let result = backup_device(&lab, "SW1").await;
    assert_eq!(result["changed"], 1);
    assert_eq!(result["backed_up"], 1);

    let changed = events.of("config.changed");
    let running = changed
        .iter()
        .find(|event| event["kind"] == "running")
        .expect("the running configuration changed");
    assert_eq!(running["device_id"], lab.id("SW1"));
    assert_eq!(running["added"], 2);
    assert_eq!(running["removed"], 1, "the description was replaced");
    assert!(running["previous_id"].as_i64().unwrap() < running["version_id"].as_i64().unwrap());

    // Only the running configuration moved: the change was never written to memory.
    let audit = audits(&lab.app, "config.changed");
    assert_eq!(audit.len(), 1, "only the running-config changed");
    assert_eq!(audit[0].device_id.as_deref(), Some(lab.id("SW1").as_str()));
    assert!(
        audit[0].message.contains("running-config of `SW1` changed"),
        "{}",
        audit[0].message
    );

    let diff = lab
        .app
        .rpc("config.diff", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the latest diff");
    assert_eq!(diff["diff"]["added"], 2);
    let added: Vec<&str> = diff["diff"]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|line| line["tag"] == "+")
        .map(|line| line["text"].as_str().unwrap())
        .collect();
    assert_eq!(added, vec![" description desk 14", "ntp server 192.0.2.1"]);

    // The change is filed under the section it happened in.
    let sections: Vec<(Vec<String>, i64, i64)> = diff["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| {
            (
                section["path"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|line| line.as_str().unwrap().to_string())
                    .collect(),
                section["added"].as_i64().unwrap(),
                section["removed"].as_i64().unwrap(),
            )
        })
        .collect();
    assert!(
        sections.contains(&(vec!["interface GigabitEthernet0/2".to_string()], 1, 1)),
        "{sections:?}"
    );
    assert!(sections.contains(&(Vec::new(), 1, 0)), "{sections:?}");
}

#[tokio::test]
async fn a_device_that_cannot_be_read_is_reported_as_failed_not_as_empty() {
    let lab = lab(&["SW1", "SW2"]).await;
    // The device answers the configuration command with nothing at all.
    lab.fake("SW2").with(|state| {
        state
            .outputs
            .insert("show running-config".into(), String::new());
    });

    let job = backup(&lab, json!({})).await.expect("the job starts");
    assert_eq!(job.state, JobState::Succeeded);
    let result = job.result.clone().unwrap();
    assert_eq!(result["backed_up"], 1);
    let failed = result["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0]["name"], "SW2");
    assert!(
        failed[0]["error"]
            .as_str()
            .unwrap()
            .contains("empty running-config"),
        "{failed:?}"
    );

    // Nothing was invented for it.
    let latest = lab
        .app
        .rpc("config.latest", json!({"device_id": lab.id("SW2")}))
        .await;
    let err = latest.expect_err("there is no configuration of SW2");
    assert_eq!(err.code, "not_found");
    assert!(
        err.message.contains("no running-config backup yet"),
        "{err}"
    );

    // And the coverage table says why.
    let devices = lab
        .app
        .rpc("config.devices", json!({}))
        .await
        .expect("coverage");
    let sw2 = devices
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "SW2")
        .unwrap();
    assert!(sw2["running"].is_null());
    assert_eq!(sw2["versions"], 0);
    assert!(sw2["last_error"].as_str().unwrap().contains("empty"));
}

#[tokio::test]
async fn a_monitor_only_device_is_skipped_with_its_reason() {
    let lab = lab(&["SW1"]).await;
    // A device that is only pinged and polled: no CLI, so no configuration.
    lab.app
        .rpc(
            "devices.create",
            json!({"name": "UPS1", "address": "10.0.0.50", "transport": "none"}),
        )
        .await
        .expect("the monitor-only device is created");

    let job = backup(&lab, json!({"device_ids": ["UPS1"]}))
        .await
        .expect("the job starts");
    let result = job.result.clone().unwrap();
    assert_eq!(result["backed_up"], 0);
    assert_eq!(result["failed"], json!([]));
    let skipped = result["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1);
    assert!(
        skipped[0]["reason"]
            .as_str()
            .unwrap()
            .contains("monitor-only"),
        "{skipped:?}"
    );

    let devices = lab
        .app
        .rpc("config.devices", json!({"device_ids": ["UPS1"]}))
        .await
        .expect("coverage");
    assert!(
        devices[0]["skipped"]
            .as_str()
            .unwrap()
            .contains("no CLI access"),
        "{devices}"
    );
}

#[tokio::test]
async fn a_device_type_without_a_configuration_command_is_skipped_not_failed() {
    let lab = lab(&["SW1"]).await;
    // `generic` is the profile for a device nobody has identified: it has no
    // configuration command, so there is nothing to back up.
    lab.app
        .rpc(
            "devices.update",
            json!({"id": lab.id("SW1"), "profile": "generic"}),
        )
        .await
        .expect("the device type changes");

    let job = backup(&lab, json!({})).await.expect("the job starts");
    let result = job.result.clone().unwrap();
    let skipped = result["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 1, "{result}");
    assert!(
        skipped[0]["reason"]
            .as_str()
            .unwrap()
            .contains("does not define a command"),
        "{skipped:?}"
    );
    assert_eq!(result["failed"], json!([]));
    // Nothing was opened to find that out.
    assert_eq!(lab.fake("SW1").with(|state| state.sessions), 0);
}

#[tokio::test]
async fn only_the_kinds_that_were_asked_for_are_fetched() {
    let lab = lab(&["SW1"]).await;
    backup(
        &lab,
        json!({"device_ids": [lab.id("SW1")], "kinds": ["running"]}),
    )
    .await
    .expect("the job runs");

    let versions = lab
        .app
        .rpc(
            "config.versions",
            json!({"device_id": lab.id("SW1"), "kind": "startup"}),
        )
        .await
        .expect("versions");
    assert_eq!(versions.as_array().unwrap().len(), 0);
    assert!(
        !lab.fake("SW1")
            .seen()
            .contains(&"show startup-config".to_string())
    );

    let err = lab
        .app
        .rpc(
            "config.backup",
            json!({"device_ids": [lab.id("SW1")], "kinds": []}),
        )
        .await
        .expect_err("an empty list of kinds is a mistake");
    assert_eq!(err.code, "invalid");
}

#[tokio::test]
async fn unsaved_changes_are_the_difference_between_running_and_startup() {
    let lab = lab(&["SW1"]).await;
    backup_device(&lab, "SW1").await;
    let saved = lab
        .app
        .rpc("config.unsaved", json!({}))
        .await
        .expect("the report");
    assert_eq!(saved["saved"], 1);
    assert_eq!(saved["devices"], json!([]));
    assert_eq!(saved["not_comparable"], json!([]));

    // A change made at the console and not written to memory.
    support::configure(&lab, "SW1", &["ntp server 192.0.2.1"], false).await;
    backup_device(&lab, "SW1").await;
    let unsaved = lab
        .app
        .rpc("config.unsaved", json!({}))
        .await
        .expect("the report");
    assert_eq!(unsaved["saved"], 0);
    let device = &unsaved["devices"][0];
    assert_eq!(device["name"], "SW1");
    assert_eq!(device["added"], 1);
    assert_eq!(device["removed"], 0);
    assert_ne!(device["running_version_id"], device["startup_version_id"]);

    // Once it is saved, the two agree again.
    support::configure(&lab, "SW1", &["ntp server 192.0.2.2"], true).await;
    backup_device(&lab, "SW1").await;
    let saved = lab
        .app
        .rpc("config.unsaved", json!({}))
        .await
        .expect("the report");
    assert_eq!(saved["saved"], 1, "{saved}");
}

#[tokio::test]
async fn a_device_whose_startup_was_never_read_is_not_comparable() {
    let lab = lab(&["SW1"]).await;
    backup(
        &lab,
        json!({"device_ids": [lab.id("SW1")], "kinds": ["running"]}),
    )
    .await
    .expect("the job runs");

    let report = lab
        .app
        .rpc("config.unsaved", json!({}))
        .await
        .expect("the report");
    assert_eq!(report["saved"], 0);
    assert_eq!(report["devices"], json!([]));
    let reason = report["not_comparable"][0]["reason"].as_str().unwrap();
    assert!(reason.contains("startup-config"), "{reason}");
}

#[tokio::test]
async fn backing_up_nothing_is_refused_rather_than_reported_as_success() {
    let lab = lab(&["SW1"]).await;
    let err = lab
        .app
        .rpc("config.backup", json!({"filter": {"site": "nowhere"}}))
        .await
        .expect_err("no device matches");
    assert_eq!(err.code, "invalid");
    assert!(err.message.contains("nothing to back up"), "{err}");
}

#[tokio::test]
async fn a_cancelled_job_stops_and_says_what_it_managed() {
    let lab = lab(&["SW1", "SW2", "SW3", "SW4", "SW5", "SW6"]).await;
    // One session at a time, so the job is still working when it is cancelled.
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "config", "value": {"concurrency": 1}}),
        )
        .await
        .expect("the setting is accepted");

    let started = lab
        .app
        .rpc("config.backup", json!({}))
        .await
        .expect("the job starts");
    let job_id = started["job_id"].as_str().unwrap().to_string();
    assert!(jobs::cancel(&lab.app, &job_id).expect("cancel"));

    let job = jobs::wait(&lab.app, &job_id).await.expect("the job ends");
    assert_eq!(job.state, JobState::Cancelled, "{:?}", job.error);
    assert!(
        job.log
            .iter()
            .any(|line| line.message.contains("cancelled")),
        "{:?}",
        job.log
    );
    // Whatever was read before the cancellation is stored; nothing is invented.
    let devices = lab
        .app
        .rpc("config.devices", json!({}))
        .await
        .expect("coverage");
    let backed_up = devices
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| !row["running"].is_null())
        .count();
    assert!(backed_up < 6, "the job stopped early: {backed_up} of 6");
}

#[tokio::test]
async fn the_concurrency_setting_bounds_the_sessions_in_flight() {
    let lab = lab(&["SW1", "SW2", "SW3", "SW4", "SW5", "SW6"]).await;
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "config", "value": {"concurrency": 2}}),
        )
        .await
        .expect("the setting is accepted");

    let job = backup(&lab, json!({})).await.expect("the job runs");
    assert_eq!(job.state, JobState::Succeeded);
    assert_eq!(job.result.unwrap()["backed_up"], 6);
    // Every device was reached, each over one session.
    for name in ["SW1", "SW2", "SW3", "SW4", "SW5", "SW6"] {
        assert_eq!(lab.fake(name).with(|state| state.sessions), 1, "{name}");
    }
}

#[tokio::test]
async fn retention_keeps_the_newest_versions_and_everything_pinned() {
    let lab = lab(&["SW1"]).await;
    lab.app
        .rpc(
            "settings.set",
            json!({"section": "config", "value": {"keep_versions": 2, "backup_startup": false}}),
        )
        .await
        .expect("the setting is accepted");

    let mut ids = Vec::new();
    for index in 1..=4 {
        support::configure(
            &lab,
            "SW1",
            &[&format!("ntp server 192.0.2.{index}")],
            false,
        )
        .await;
        backup_device(&lab, "SW1").await;
        let versions = lab
            .app
            .rpc("config.versions", json!({"device_id": lab.id("SW1")}))
            .await
            .expect("versions");
        ids.push(versions[0]["id"].as_i64().unwrap());
        if index == 1 {
            // Pin the oldest one, as the change pipeline does for a version it refers to.
            lab.app
                .rpc(
                    "config.pin",
                    json!({"version_id": ids[0], "reason": "change-7 before"}),
                )
                .await
                .expect("the version is pinned");
        }
    }

    let versions = lab
        .app
        .rpc("config.versions", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("versions");
    let kept: Vec<i64> = versions
        .as_array()
        .unwrap()
        .iter()
        .map(|version| version["id"].as_i64().unwrap())
        .collect();
    assert_eq!(
        kept,
        vec![ids[3], ids[2], ids[0]],
        "the newest two and the pinned one"
    );
    assert_eq!(versions[2]["pinned"], true);
    assert_eq!(versions[2]["pin_reasons"], json!(["change-7 before"]));

    // A pinned version is not deleted on request either.
    let deleted = lab
        .app
        .rpc("config.delete", json!({"version_ids": [ids[0], ids[2]]}))
        .await
        .expect("the deletion runs");
    assert_eq!(deleted["deleted"], 1);
    assert_eq!(deleted["kept_pinned"], json!([ids[0]]));

    // Pinning and unpinning are both recorded.
    let pinned = audits(&lab.app, "config.pin");
    assert_eq!(pinned.len(), 1);
    assert!(
        pinned[0].message.contains("change-7 before"),
        "{}",
        pinned[0].message
    );

    // Unpinned, it goes with the next pass of retention.
    lab.app
        .rpc("config.unpin", json!({"version_id": ids[0]}))
        .await
        .expect("the pin is removed");
    assert_eq!(audits(&lab.app, "config.unpin").len(), 1);
    support::configure(&lab, "SW1", &["ntp server 192.0.2.9"], false).await;
    backup_device(&lab, "SW1").await;
    let versions = lab
        .app
        .rpc("config.versions", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("versions");
    assert_eq!(versions.as_array().unwrap().len(), 2);
    assert!(
        !versions
            .as_array()
            .unwrap()
            .iter()
            .any(|version| version["id"].as_i64() == Some(ids[0]))
    );
}

#[tokio::test]
async fn deleting_a_device_s_versions_removes_them_all() {
    let lab = lab(&["SW1"]).await;
    backup_device(&lab, "SW1").await;
    let deleted = lab
        .app
        .rpc("config.delete", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("the deletion runs");
    assert_eq!(deleted["deleted"], 2, "running and startup");
    let devices = lab
        .app
        .rpc("config.devices", json!({}))
        .await
        .expect("coverage");
    assert_eq!(devices[0]["versions"], 0);

    let err = lab
        .app
        .rpc("config.delete", json!({}))
        .await
        .expect_err("saying nothing is a mistake");
    assert_eq!(err.code, "invalid");
}

#[tokio::test(start_paused = true)]
async fn a_configuration_change_reported_by_the_log_feature_queues_a_backup() {
    let lab = lab(&["SW1"]).await;
    backup_device(&lab, "SW1").await;
    support::configure(&lab, "SW1", &["ntp server 192.0.2.4"], false).await;

    // What the log feature raises when it sees `%SYS-5-CONFIG_I` in syslog.
    lab.app.publish(
        "logs.config_changed",
        &json!({"device_id": lab.id("SW1"), "by": "console"}),
    );

    // The backup is debounced by a minute (the clock is paused, so no test waits for it).
    //
    // Bounded, because a loop with no deadline does not fail when the backup
    // never starts — it hangs, and a hung job in CI says far less than a
    // failure naming what never happened.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let job = loop {
        let started = jobs::list(&lab.app, &jobs::JobFilter::default()).expect("jobs");
        let system = started
            .iter()
            .find(|job| job.kind == "config.backup" && job.origin == "system");
        match system {
            Some(job) => break jobs::wait(&lab.app, &job.id).await.expect("the job ends"),
            None => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "a configuration change never started a system backup; the jobs \
                     that did start were {:?}",
                    started
                        .iter()
                        .map(|job| (job.kind.as_str(), job.origin.as_str()))
                        .collect::<Vec<_>>()
                );
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    };
    assert!(
        job.title.contains("after a configuration change"),
        "{job:?}"
    );
    assert_eq!(job.result.unwrap()["changed"], 1);
    let versions = lab
        .app
        .rpc("config.versions", json!({"device_id": lab.id("SW1")}))
        .await
        .expect("versions");
    assert_eq!(versions[0]["origin"], "event");
}

#[tokio::test]
async fn a_backup_never_sends_a_command_the_device_type_does_not_call_read_only() {
    let lab = lab(&["SW1"]).await;
    load(&lab, "SW1", "hostname SW1\n");
    backup_device(&lab, "SW1").await;
    let seen = lab.fake("SW1").seen();
    for command in &seen {
        assert!(
            command.starts_with("show ")
                || command.starts_with("terminal ")
                || matches!(command.as_str(), "enable" | "exit" | "end"),
            "a backup sent `{command}`"
        );
    }
    assert!(seen.contains(&"show running-config".to_string()));
}
