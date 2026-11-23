//! Fleet-wide and device-wide checks, case by case.

use nocturne_drivers::model::Facts;

use crate::settings::Settings;
use crate::testdata::*;
use crate::world::World;

// ------------------------------------------------------- software versions

fn fleet(devices: &[(&str, &str, &str)]) -> World {
    world(
        devices
            .iter()
            .map(|(id, model, version)| {
                device(id, &format!("SW-{id}")).facts(facts(model, version, 100_000))
            })
            .collect(),
    )
}

#[test]
fn software_inconsistent_table() {
    // Positive: three of a model on one version, one left behind.
    let found = run(
        "software_inconsistent",
        &fleet(&[
            ("a", "WS-C2960-24TT-L", "15.2(4)E10"),
            ("b", "WS-C2960-24TT-L", "15.2(4)E10"),
            ("c", "WS-C2960-24TT-L", "15.2(4)E10"),
            ("d", "WS-C2960-24TT-L", "12.2(55)SE"),
        ]),
    );
    assert_eq!(subjects(&found), ["d:12.2(55)SE"]);
    assert!(found.findings[0].title.contains("15.2(4)E10"));
    assert_eq!(found.findings[0].related.len(), 3);

    // Negative: everybody on the same version.
    assert!(
        run(
            "software_inconsistent",
            &fleet(&[
                ("a", "WS-C2960-24TT-L", "15.2(4)E10"),
                ("b", "WS-C2960-24TT-L", "15.2(4)E10"),
                ("c", "WS-C2960-24TT-L", "15.2(4)E10"),
            ])
        )
        .findings
        .is_empty()
    );

    // Look-alike: different models. Two different switches on two different
    // versions is not an inconsistency.
    assert!(
        run(
            "software_inconsistent",
            &fleet(&[
                ("a", "WS-C2960-24TT-L", "15.2(4)E10"),
                ("b", "WS-C3850-24T", "16.12.5b"),
                ("c", "WS-C3850-48U", "17.3.4"),
            ])
        )
        .findings
        .is_empty()
    );

    // Not enough data: two devices on two versions. There is no majority, so
    // there is no way to say which is the odd one out.
    let two = run(
        "software_inconsistent",
        &fleet(&[
            ("a", "WS-C2960-24TT-L", "15.2(4)E10"),
            ("b", "WS-C2960-24TT-L", "12.2(55)SE"),
        ]),
    );
    assert!(two.findings.is_empty());
    assert_eq!(two.skips().count(), 2);

    // Not enough data: no model or version collected.
    let unknown = run("software_inconsistent", &world(vec![device("a", "SW1")]));
    assert_eq!(unknown.skips().count(), 1);
}

// --------------------------------------------------------------- uptime

/// A device whose uptime was `was` at the previous collection, `minutes_ago`,
/// and is `now_uptime` now.
fn uptimes(was: u64, now_uptime: u64, minutes_ago: i64) -> World {
    world(vec![
        device("a", "SW1")
            .facts(facts("WS-C2960-24TT-L", "15.2(4)E10", now_uptime))
            .previous_facts(
                facts("WS-C2960-24TT-L", "15.2(4)E10", was),
                NOW - minutes_ago * 60_000,
            ),
    ])
}

#[test]
fn uptime_reset_table() {
    // Positive: up for 40 days, now up for 5 minutes, collected an hour apart.
    let found = run("uptime_reset", &uptimes(40 * 86_400, 300, 60));
    assert_eq!(subjects(&found), ["a:uptime"]);
    assert!(found.findings[0].title.contains("40d"));

    // Negative: uptime went up, as uptime does.
    assert!(
        run(
            "uptime_reset",
            &uptimes(40 * 86_400, 40 * 86_400 + 3_600, 60)
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: a counter that wrapped. `sysUpTime` wraps after 497 days,
    // and a wrap is not a reboot.
    let wrapped = run("uptime_reset", &uptimes(497 * 86_400, 300, 60));
    assert!(wrapped.findings.is_empty());
    assert_eq!(wrapped.skips().count(), 1);

    // Look-alike 2: uptime went backwards by more than the time that passed —
    // 10 days of uptime cannot have been gained in an hour. That is a changed
    // source (CLI against SNMP, a stack member answering), not a restart.
    let impossible = run("uptime_reset", &uptimes(40 * 86_400, 10 * 86_400, 60));
    assert!(impossible.findings.is_empty());
    assert_eq!(impossible.skips().count(), 1);

    // Not enough data: one collection only.
    let once = run(
        "uptime_reset",
        &world(vec![device("a", "SW1").facts(facts(
            "WS-C2960-24TT-L",
            "15.2(4)E10",
            100,
        ))]),
    );
    assert!(once.findings.is_empty());
    assert_eq!(once.skips().count(), 1);

    // Not enough data: no uptime in the facts.
    let no_uptime = world(vec![
        device("a", "SW1")
            .facts(Facts {
                uptime_s: None,
                ..facts("WS-C2960-24TT-L", "15.2(4)E10", 0)
            })
            .previous_facts(facts("WS-C2960-24TT-L", "15.2(4)E10", 100), NOW - 3_600_000),
    ]);
    assert_eq!(run("uptime_reset", &no_uptime).skips().count(), 1);
}

#[test]
fn uptime_reset_carries_the_reason_when_the_device_reported_one() {
    let mut now = facts("WS-C2960-24TT-L", "15.2(4)E10", 300);
    now.extra
        .insert("last_reload_reason".into(), "power-on".into());
    let world = world(vec![device("a", "SW1").facts(now).previous_facts(
        facts("WS-C2960-24TT-L", "15.2(4)E10", 40 * 86_400),
        NOW - 3_600_000,
    )]);
    let found = run("uptime_reset", &world);
    assert!(
        found.findings[0]
            .evidence
            .iter()
            .any(|item| item.value.contains("power-on"))
    );
}

// ---------------------------------------------------- sustained resources

fn metric(world: &mut World, device_id: &str, name: &str, values: &[(i64, f64)]) {
    let series = world
        .store()
        .series_id(device_id, name, "")
        .expect("series");
    let samples: Vec<(i64, i64, f64)> = values
        .iter()
        .map(|(at, value)| (series, *at, *value))
        .collect();
    world.store().samples_insert(&samples).expect("samples");
    world.reindex_series();
}

fn samples(level: f64) -> Vec<(i64, f64)> {
    (0..12)
        .map(|index| (NOW - (50 - index * 4) * 60_000, level))
        .collect()
}

#[test]
fn high_cpu_memory_and_temperature_tables() {
    for (check, name, over, under) in [
        ("high_cpu", "cpu.pct", 95.0, 20.0),
        ("high_memory", "mem.pct", 96.0, 40.0),
        ("high_temperature", "temp.c", 75.0, 30.0),
    ] {
        // Positive: over the threshold for the whole window.
        let mut hot = world(vec![device("a", "SW1")]);
        metric(&mut hot, "a", name, &samples(over));
        let found = run(check, &hot);
        assert_eq!(found.findings.len(), 1, "{check}");
        assert!(found.findings[0].title.contains("has been over"), "{check}");

        // Negative: comfortably under.
        let mut cool = world(vec![device("a", "SW1")]);
        metric(&mut cool, "a", name, &samples(under));
        assert!(run(check, &cool).findings.is_empty(), "{check}");

        // Look-alike: one spike. A `show tech` or a backup is not a capacity
        // problem.
        let mut spike = world(vec![device("a", "SW1")]);
        let mut values = samples(under);
        values[5] = (values[5].0, over + 3.0);
        metric(&mut spike, "a", name, &values);
        assert!(run(check, &spike).findings.is_empty(), "{check}");

        // Not enough data: the metric is not polled at all.
        let unpolled = run(check, &world(vec![device("a", "SW1")]));
        assert!(unpolled.findings.is_empty(), "{check}");
        assert_eq!(unpolled.skips().count(), 1, "{check}");

        // Not enough data: two samples, five minutes apart.
        let mut thin = world(vec![device("a", "SW1")]);
        metric(
            &mut thin,
            "a",
            name,
            &[(NOW - 300_000, over), (NOW - 60_000, over)],
        );
        assert!(run(check, &thin).findings.is_empty(), "{check}");
    }
}

#[test]
fn resource_samples_that_cannot_be_read_are_not_said_to_be_missing() {
    let mut hot = world(vec![device("a", "SW1")]);
    metric(&mut hot, "a", "cpu.pct", &samples(95.0));
    let series = hot.store().series_id("a", "cpu.pct", "").expect("series");
    hot.store()
        .with(|conn| {
            conn.execute(
                "INSERT INTO samples (series_id, ts, value) VALUES (?1, ?2, 'unreadable')",
                rusqlite::params![series, NOW - 60_000],
            )
        })
        .expect("the sample is stored");
    let report = run("high_cpu", &hot);
    assert!(report.findings.is_empty());
    assert!(report.evaluated().is_empty());
    assert_eq!(
        report.skips().map(|(_, reason)| reason).collect::<Vec<_>>(),
        ["the samples of this metric could not be read"]
    );
}

#[test]
fn resource_thresholds_are_settings() {
    let mut world = world(vec![device("a", "SW1")]);
    metric(&mut world, "a", "cpu.pct", &samples(70.0));
    assert!(run("high_cpu", &world).findings.is_empty());
    let strict = Settings {
        thresholds: crate::settings::Thresholds {
            cpu_pct: 60.0,
            ..Default::default()
        },
        ..Settings::default()
    };
    assert_eq!(run_with("high_cpu", &world, &strict).findings.len(), 1);
}
