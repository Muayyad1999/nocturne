//! Configuration housekeeping checks, case by case.

use nocturne_app::model::Transport;

use crate::settings::Settings;
use crate::testdata::*;
use crate::world::{AppliedChange, World};

const RUNNING: &str = "\
hostname SW1
ip domain-name lab.example
interface GigabitEthernet0/1
 description uplink
 switchport mode trunk
line vty 0 4
 transport input ssh
";

// --------------------------------------------------------- unsaved config

#[test]
fn unsaved_config_table() {
    // Positive: a line in the running configuration that is not in the startup one.
    let found = run(
        "unsaved_config",
        &world(vec![
            device("a", "SW1")
                .config(&format!("{RUNNING}ntp server 192.0.2.1\n"))
                .startup(RUNNING),
        ]),
    );
    assert_eq!(subjects(&found), ["a:running-config"]);
    assert!(
        found.findings[0]
            .evidence
            .iter()
            .any(|item| item.label == "Only in running" && item.value.contains("ntp server")),
        "the unsaved line is the evidence"
    );

    // Negative: the two are the same.
    assert!(
        run(
            "unsaved_config",
            &world(vec![device("a", "SW1").config(RUNNING).startup(RUNNING)])
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: the only differences are volatile lines. Every pair of
    // reads differs by these, and reporting them would mean every device in
    // the estate is permanently "unsaved".
    let volatile = format!(
        "! Last configuration change at 12:01:02 UTC Mon Nov 10 2025 by admin\n\
         ! NVRAM config last updated at 09:00:00 UTC Mon Nov 10 2025\n\
         {RUNNING}ntp clock-period 17179860\n"
    );
    let report = run(
        "unsaved_config",
        &world(vec![device("a", "SW1").config(&volatile).startup(
            &format!(
                "! Last configuration change at 08:00:00 UTC Sun Nov 9 2025 by someone\n{RUNNING}"
            ),
        )]),
    );
    assert!(report.findings.is_empty());
    assert_eq!(report.evaluated().len(), 1, "it was decided, not skipped");

    // Look-alike 2: the platform's own ignore patterns count as volatile too.
    // `Current configuration : 1234 bytes` changes with every byte written.
    assert!(
        run(
            "unsaved_config",
            &world(vec![
                device("a", "SW1")
                    .config(&format!("Current configuration : 1543 bytes\n{RUNNING}"))
                    .startup(&format!("Current configuration : 1498 bytes\n{RUNNING}")),
            ])
        )
        .findings
        .is_empty()
    );

    // Not enough data: no startup snapshot.
    let no_startup = run(
        "unsaved_config",
        &world(vec![device("a", "SW1").config(RUNNING)]),
    );
    assert!(no_startup.findings.is_empty());
    assert_eq!(no_startup.skips().count(), 1);

    // Not enough data: the snapshots are hours apart, so the device may well
    // have been saved in between.
    let apart = run(
        "unsaved_config",
        &world(vec![
            device("a", "SW1")
                .config_at(&format!("{RUNNING}ntp server 192.0.2.1\n"), NOW)
                .startup_at(RUNNING, NOW - 6 * 3_600_000),
        ]),
    );
    assert!(apart.findings.is_empty());
    assert_eq!(apart.skips().count(), 1);
}

// --------------------------------------------------------- no recent backup

#[test]
fn no_recent_backup_table() {
    // Positive: the newest backup is older than the threshold.
    let found = run(
        "no_recent_backup",
        &world(vec![
            device("a", "SW1").config_at(RUNNING, NOW - 20 * 86_400_000),
        ]),
    );
    assert_eq!(subjects(&found), ["a:running-config"]);
    assert!(found.findings[0].title.contains("20 day"));

    // Positive: never backed up at all, which is worse than an ageing one.
    let never = run("no_recent_backup", &world(vec![device("a", "SW1")]));
    assert_eq!(never.findings.len(), 1);
    assert_eq!(never.findings[0].severity, None, "the check's own severity");
    assert_eq!(
        found.findings[0].severity,
        Some(crate::model::Severity::Low)
    );

    // Negative: backed up today.
    assert!(
        run(
            "no_recent_backup",
            &world(vec![device("a", "SW1").config_at(RUNNING, NOW - 3_600_000)])
        )
        .findings
        .is_empty()
    );

    // Look-alike: a device with no CLI access at all. Nothing can back up a
    // device that is only pinged.
    let mut monitored = device("a", "PRINTER");
    monitored.device.transport = Transport::None;
    let report = run("no_recent_backup", &world(vec![monitored]));
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 1);

    // Not evaluated: without the configuration feature nothing is taking
    // backups at all, which is a setup matter and not a device's fault.
    let mut without = world(vec![device("a", "SW1")]);
    without.config_feature = false;
    let report = run("no_recent_backup", &without);
    assert!(report.findings.is_empty());
    assert_eq!(
        report.skips().next().map(|(_, reason)| reason),
        Some("the configuration feature is not installed, so nothing is taking backups")
    );
}

// --------------------------------------------- changed outside the pipeline

/// A device with two stored running configurations, the newest at `at`.
fn with_versions(at: i64, source: &str) -> World {
    let world = world(vec![device("a", "SW1").config_at(RUNNING, at)]);
    world
        .store()
        .config_insert("a", "running", RUNNING, "scheduled", "")
        .expect("first version");
    world
        .store()
        .config_insert(
            "a",
            "running",
            &format!("{RUNNING}ntp server 192.0.2.1\n"),
            source,
            "",
        )
        .expect("second version");
    world
}

/// The stored versions carry their own `taken_at` (`now`), so the window is
/// measured from the world's clock.
fn at_now() -> i64 {
    nocturne_app::util::now_ms()
}

#[test]
fn config_changed_outside_nocturne_table() {
    // Positive: two versions, no change applied anywhere near them.
    let mut found = with_versions(at_now(), "scheduled");
    found.now = at_now();
    found.applied_changes = Some(Default::default());
    let report = run("config_changed_outside_nocturne", &found);
    assert_eq!(report.findings.len(), 1);
    assert!(report.findings[0].title.contains("no Nocturne change"));

    // Negative: a change Nocturne applied at the same time accounts for it.
    let mut explained = with_versions(at_now(), "scheduled");
    explained.now = at_now();
    let newest = explained
        .store()
        .config_latest("a", "running")
        .unwrap()
        .unwrap();
    explained.applied_changes = Some(
        [(
            "a".to_string(),
            vec![AppliedChange {
                id: "chg1".into(),
                title: "Add an NTP server".into(),
                at: newest.taken_at,
            }],
        )]
        .into_iter()
        .collect(),
    );
    assert!(
        run("config_changed_outside_nocturne", &explained)
            .findings
            .is_empty()
    );

    // Look-alike 1: the snapshot the change pipeline took itself.
    let mut ours = with_versions(at_now(), "post-change");
    ours.now = at_now();
    ours.applied_changes = Some(Default::default());
    assert!(
        run("config_changed_outside_nocturne", &ours)
            .findings
            .is_empty()
    );

    // Look-alike 2: a change from a month ago. It has been there long enough
    // to have been seen; it is not re-reported for ever.
    let mut old = with_versions(at_now(), "scheduled");
    old.now = at_now() + 40 * 86_400_000;
    old.applied_changes = Some(Default::default());
    let report = run("config_changed_outside_nocturne", &old);
    assert!(report.findings.is_empty());
    assert_eq!(report.evaluated().len(), 1);

    // Not enough data: one stored version.
    let mut single = world(vec![device("a", "SW1").config(RUNNING)]);
    single.applied_changes = Some(Default::default());
    single
        .store()
        .config_insert("a", "running", RUNNING, "manual", "")
        .unwrap();
    let report = run("config_changed_outside_nocturne", &single);
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 1);

    // Not evaluated: without the changes feature, Nocturne's own changes
    // cannot be ruled out.
    let without = with_versions(at_now(), "scheduled");
    let report = run("config_changed_outside_nocturne", &without);
    assert!(report.findings.is_empty());
    assert!(
        report
            .skips()
            .any(|(_, reason)| reason.contains("changes feature is not installed"))
    );
}

#[test]
fn the_backup_age_is_a_setting() {
    let world = world(vec![
        device("a", "SW1").config_at(RUNNING, NOW - 10 * 86_400_000),
    ]);
    assert_eq!(run("no_recent_backup", &world).findings.len(), 1);
    let relaxed = Settings {
        thresholds: crate::settings::Thresholds {
            backup_age_days: 30,
            ..Default::default()
        },
        ..Settings::default()
    };
    assert!(
        run_with("no_recent_backup", &world, &relaxed)
            .findings
            .is_empty()
    );
}
