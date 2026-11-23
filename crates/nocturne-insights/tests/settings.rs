//! The `insights` settings section through `app.rpc`.
//!
//! A threshold of the wrong kind — a fraction where a whole number of days
//! is meant, a word where a number is — used to be stored as given, and from
//! then on the section could not be read: the catalogue, the list and every
//! scan failed, and the screen that would have put it right was the one that
//! no longer drew. Now such a write is refused with the setting and what it
//! has to be, and a value stored before writes were checked (or by hand) is
//! reported, and stops only the checks that read it.

mod common;

use common::{With, lab, scan};
use nocturne_app::{App, AppError};
use serde_json::{Value, json};

async fn refused(app: &App, value: Value) -> AppError {
    app.rpc(
        "settings.set",
        json!({"section": "insights", "value": value}),
    )
    .await
    .expect_err("the write is refused")
}

async fn stored(app: &App) -> Value {
    app.rpc("settings.get", json!({"section": "insights"}))
        .await
        .expect("the section")
}

fn row(coverage: &Value, check: &str) -> Value {
    coverage["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|row| row["check"] == check)
        .unwrap_or_else(|| panic!("no coverage for {check}"))
        .clone()
}

#[tokio::test]
async fn a_value_of_the_wrong_kind_is_refused_in_words_and_nothing_of_the_write_is_stored() {
    let lab = lab(&["SW1"], With::default()).await;
    let app = &lab.app;

    let fraction = refused(app, json!({"thresholds": {"backup_age_days": 7.5}})).await;
    println!(
        "settings.set {{thresholds: {{backup_age_days: 7.5}}}} → {}: {} {}",
        fraction.code,
        fraction.message,
        fraction.details.clone().unwrap_or_default()
    );
    assert_eq!(fraction.code, "invalid");
    assert_eq!(
        fraction.message,
        "`thresholds.backup_age_days` must be a whole number of days (0 or more), not 7.5"
    );
    assert_eq!(
        fraction.details.expect("the problems, one per setting")["problems"],
        json!([{
            "key": "thresholds.backup_age_days",
            "value": 7.5,
            "message": fraction.message,
        }])
    );

    // A write is one decision: a good value beside a bad one is not stored
    // either.
    let mixed = refused(
        app,
        json!({"thresholds": {"cpu_pct": 95.0, "sustained_minutes": 1.5}}),
    )
    .await;
    assert_eq!(
        mixed.details.unwrap()["problems"][0]["key"],
        "thresholds.sustained_minutes"
    );
    let now = stored(app).await;
    assert_eq!(now["thresholds"]["backup_age_days"], 7);
    assert_eq!(now["thresholds"]["cpu_pct"], 85.0);
    assert_eq!(now["thresholds"]["sustained_minutes"], 15);

    for (value, key) in [
        (
            json!({"thresholds": {"history_hours": -1}}),
            "thresholds.history_hours",
        ),
        (
            json!({"thresholds": {"stale_data_hours": 5_000_000_000u64}}),
            "thresholds.stale_data_hours",
        ),
        (
            json!({"thresholds": {"cpu_pct": "high"}}),
            "thresholds.cpu_pct",
        ),
        (
            json!({"thresholds": {"qos_drop_ps": null}}),
            "thresholds.qos_drop_ps",
        ),
        (json!({"thresholds": 5}), "thresholds"),
        (
            json!({"enabled_checks": {"duplex_mismatch": "off"}}),
            "enabled_checks.duplex_mismatch",
        ),
        (json!({"enabled_checks": []}), "enabled_checks"),
        (json!({"raise_alerts_from": "urgent"}), "raise_alerts_from"),
        (json!({"include_lab": 1}), "include_lab"),
        (json!({"auto_scan_delay_ms": 2.5}), "auto_scan_delay_ms"),
    ] {
        let refusal = refused(app, value.clone()).await;
        assert_eq!(refusal.code, "invalid", "{value}");
        assert_eq!(
            refusal.details.as_ref().expect("details")["problems"][0]["key"],
            key,
            "{value}"
        );
        assert!(
            refusal.message.starts_with(&format!("`{key}` must be ")),
            "{value}: {}",
            refusal.message
        );
    }
    assert_eq!(
        stored(app).await,
        now,
        "nothing that was refused was stored"
    );

    // What is right is stored, and the feature reads it.
    app.rpc(
        "settings.set",
        json!({"section": "insights", "value": {"thresholds": {"backup_age_days": 30, "cpu_pct": 95}}}),
    )
    .await
    .expect("a good write");
    let now = stored(app).await;
    assert_eq!(now["thresholds"]["backup_age_days"], 30);
    assert_eq!(now["thresholds"]["cpu_pct"].as_f64(), Some(95.0));
    app.rpc("insights.checks", json!({}))
        .await
        .expect("the catalogue");
}

#[tokio::test]
async fn a_bad_value_already_stored_is_reported_and_only_the_checks_that_read_it_stand_aside() {
    let lab = lab(&["SW1"], With::default()).await;
    let app = &lab.app;
    // SW1 has never been backed up.
    scan(app, json!({})).await;
    let found = app
        .rpc("insights.list", json!({"check": "no_recent_backup"}))
        .await
        .unwrap()["insights"][0]
        .clone();
    assert_eq!(found["subject"], "running-config", "{found}");

    // What an earlier version let `settings.set` store, or a hand edit of the
    // database: the key the core keeps the section under.
    app.store()
        .kv_set(
            "settings.insights",
            &json!({"thresholds": {"backup_age_days": 7.5}}),
        )
        .expect("stored behind the feature's back");

    // The catalogue still answers, and says which checks cannot run and why.
    let checks = app
        .rpc("insights.checks", json!({}))
        .await
        .expect("the catalogue answers");
    let check = |id: &str| -> Value {
        checks
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == id)
            .unwrap_or_else(|| panic!("no {id}"))
            .clone()
    };
    for id in ["no_recent_backup", "config_changed_outside_nocturne"] {
        let entry = check(id);
        println!("insights.checks → {id}: problems {}", entry["problems"]);
        assert_eq!(
            entry["problems"],
            json!([{
                "key": "thresholds.backup_age_days",
                "value": 7.5,
                "message": "`thresholds.backup_age_days` must be a whole number of days (0 or more), not 7.5",
            }]),
            "{id}"
        );
    }
    assert!(
        check("telnet_enabled").get("problems").is_none(),
        "a check that does not read it is not affected"
    );
    app.rpc("insights.list", json!({}))
        .await
        .expect("the list answers");

    // Scans finish. The checks that read the setting did not run, say so,
    // and leave their findings as they were: neither seen nor clean.
    for _ in 0..2 {
        scan(app, json!({})).await;
    }
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    assert_eq!(
        coverage["settings_problems"][0]["key"],
        "thresholds.backup_age_days"
    );
    let backup = row(&coverage, "no_recent_backup");
    println!("insights.coverage → no_recent_backup: {backup}");
    assert_eq!(backup["enabled"], true, "it is not switched off: {backup}");
    assert_eq!(backup["evaluated"], 0);
    assert_eq!(backup["skipped"], 1);
    let reason = backup["reasons"][0]["reason"].as_str().unwrap();
    assert!(
        reason.contains("could not run") && reason.contains("thresholds.backup_age_days"),
        "{reason}"
    );
    let telnet = row(&coverage, "telnet_enabled");
    assert!(
        telnet["reasons"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("no stored configuration"),
        "the other checks ran, and said what they say: {telnet}"
    );
    let still = app
        .rpc("insights.get", json!({"id": found["id"]}))
        .await
        .unwrap();
    assert_eq!(still["state"], "open");
    assert_eq!(still["occurrences"], 1);

    // Putting it right on the screen puts the check back.
    app.rpc(
        "settings.set",
        json!({"section": "insights", "value": {"thresholds": {"backup_age_days": 7}}}),
    )
    .await
    .expect("the correction is stored");
    let checks = app.rpc("insights.checks", json!({})).await.unwrap();
    assert!(
        checks
            .as_array()
            .unwrap()
            .iter()
            .all(|check| check.get("problems").is_none()),
        "{checks}"
    );
    scan(app, json!({})).await;
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    assert_eq!(coverage["settings_problems"], json!([]));
    assert_eq!(row(&coverage, "no_recent_backup")["evaluated"], 1);
}
