//! The three QoS checks through `app.rpc`: the catalogue, a scan, the list
//! with its evidence, the coverage report, the fix that is not offered, and
//! the threshold in the settings.
//!
//! The interface sections are real (`asr-subinterfaces.txt`, the
//! `show running-config interface` output of an ASR from the ntc-templates
//! corpus — see the driver fixtures' `config/SOURCE.md`). The configuration
//! wrapped around them, the policy-maps and the metric series are written by
//! hand, the series in the format `docs/dev/spec-qos.md` fixes for
//! monitoring: no part of this feature links to the monitor.

mod common;

use common::{With, config, lab, open_subjects, scan};
use nocturne_app::App;
use nocturne_app::util::now_ms;
use serde_json::{Value, json};

/// Real. Eight policies attached to six sub-interfaces, none defined.
const REAL_ASR_INTERFACES: &str =
    include_str!("../../nocturne-drivers/tests/profiles/cisco_ios/config/asr-subinterfaces.txt");

/// Written by hand around the real sections: a whole configuration that
/// defines seven of the eight attached policies (not `VIPSIP_POLICY_800_OUT`)
/// and one that nothing attaches (`OLD_POLICY_512_OUT`).
fn whole_configuration() -> String {
    let mut text = String::from("version 16.9\nhostname WAN1\n!\n");
    for name in [
        "VIPSIP_POLICY_2048_V1",
        "VIPSIP_POLICY_2048_OUT",
        "DATA_POLICY_100M_V1",
        "DATA_POLICY_100M_OUT",
        "VIPSIP_POLICY_1000_V1",
        "VIPSIP_POLICY_1000_OUT",
        "VIPSIP_POLICY_800_V1",
        "OLD_POLICY_512_OUT",
    ] {
        text.push_str(&format!(
            "policy-map {name}\n class class-default\n  police cir 2048000\n"
        ));
    }
    text.push_str(REAL_ASR_INTERFACES);
    text.push_str("!\nline vty 0 4\n transport input ssh\n!\nend\n");
    text
}

const VOICE: &str = "GigabitEthernet2/0/4.223415|out|VIPSIP_POLICY_2048_OUT/VOICE";

/// Twelve samples across the last fifty minutes, as a 300-second poll leaves.
fn sampled(app: &App, device_id: &str, metric: &str, instance: &str, value: f64) {
    let series = app
        .store()
        .series_id(device_id, metric, instance)
        .expect("series");
    let now = now_ms();
    let samples: Vec<(i64, i64, f64)> = (0..12)
        .map(|step| (series, now - (50 - step * 4) * 60_000, value))
        .collect();
    app.store().samples_insert(&samples).expect("samples");
}

async fn listed(app: &App, check: &str) -> Vec<Value> {
    app.rpc("insights.list", json!({"check": check, "limit": 500}))
        .await
        .expect("list")["insights"]
        .as_array()
        .expect("insights")
        .clone()
}

fn coverage_of<'a>(coverage: &'a Value, check: &str) -> &'a Value {
    coverage["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .find(|entry| entry["check"] == check)
        .unwrap_or_else(|| panic!("coverage has no entry for {check}"))
}

fn evidence_value<'a>(insight: &'a Value, label: &str) -> &'a str {
    insight["evidence"]
        .as_array()
        .expect("evidence")
        .iter()
        .find(|item| item["label"] == label)
        .unwrap_or_else(|| panic!("no evidence labelled `{label}` in {insight}"))["value"]
        .as_str()
        .expect("a value")
}

/// WAN1 has a whole configuration with one missing and one unattached
/// policy, and a class dropping to congestion. PE1 has only the real partial
/// capture and no QoS samples at all.
async fn estate(with: With) -> nocturne_app::testing::Lab {
    let lab = lab(&["WAN1", "PE1"], with).await;
    let (app, wan, pe) = (&lab.app, lab.id("WAN1"), lab.id("PE1"));
    config(app.store(), &wan, "running", &whole_configuration());
    config(app.store(), &pe, "running", REAL_ASR_INTERFACES);
    sampled(app, &wan, "qos.queue_drop_ps", VOICE, 42.0);
    sampled(app, &wan, "qos.drop_ps", VOICE, 42.0);
    sampled(app, &wan, "qos.drop_pct", VOICE, 3.5);
    sampled(app, &wan, "qos.bps", VOICE, 1_200_000.0);
    lab
}

#[tokio::test]
async fn the_catalogue_lists_the_three_qos_checks_with_what_they_need_and_cannot_see() {
    let lab = estate(With::default()).await;
    let checks = lab.app.rpc("insights.checks", json!({})).await.unwrap();
    let check = |id: &str| -> Value {
        checks
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == id)
            .unwrap_or_else(|| panic!("the catalogue has no {id}"))
            .clone()
    };

    let missing = check("qos_policy_missing");
    println!("insights.checks → entry: {missing}");
    assert_eq!(missing["category"], "config");
    assert_eq!(missing["default_severity"], "medium");
    assert_eq!(missing["enabled"], true);
    assert_eq!(missing["needs"], json!(["running configuration"]));
    assert!(
        missing["description"]
            .as_str()
            .unwrap()
            .contains("IOS-style")
    );
    // What keeps it from answering is said where a person choosing checks
    // reads it: another grammar, a section capture, a listing cut short.
    let not_evaluated = missing["not_evaluated"].as_str().unwrap();
    for said in [
        "typed QoS policies",
        "IOS XR",
        "`hostname`",
        "closing `end`",
    ] {
        assert!(not_evaluated.contains(said), "{said}: {not_evaluated}");
    }

    let unattached = check("qos_policy_unattached");
    assert_eq!(unattached["category"], "hygiene");
    assert_eq!(unattached["default_severity"], "info");
    assert!(
        unattached["description"]
            .as_str()
            .unwrap()
            .contains("child policy")
    );

    let dropping = check("qos_class_dropping");
    assert_eq!(dropping["category"], "capacity");
    assert_eq!(dropping["default_severity"], "medium");
    assert!(
        dropping["needs"][0]
            .as_str()
            .unwrap()
            .contains("qos.queue_drop_ps")
    );
    assert!(
        dropping["description"]
            .as_str()
            .unwrap()
            .contains("policer")
    );
    assert!(
        dropping["not_evaluated"]
            .as_str()
            .unwrap()
            .contains("the read is failing")
    );
}

#[tokio::test]
async fn a_scan_reports_the_missing_the_unattached_and_the_dropping_with_their_evidence() {
    let lab = estate(With::default()).await;
    let app = &lab.app;
    scan(app, json!({})).await;

    // The policy an interface names and nothing defines: the exact line.
    let missing = listed(app, "qos_policy_missing").await;
    assert_eq!(missing.len(), 1, "{missing:?}");
    println!(
        "insights.list {{check: qos_policy_missing}} → insight: {}",
        missing[0]
    );
    assert_eq!(missing[0]["device"], "WAN1");
    assert_eq!(missing[0]["subject"], "policy-map VIPSIP_POLICY_800_OUT");
    assert_eq!(missing[0]["category"], "config");
    assert_eq!(missing[0]["severity"], "medium");
    assert_eq!(
        evidence_value(&missing[0], "interface GigabitEthernet2/0/4.22341020"),
        "service-policy output VIPSIP_POLICY_800_OUT"
    );

    // The policy nothing names.
    let unattached = listed(app, "qos_policy_unattached").await;
    assert_eq!(unattached.len(), 1, "{unattached:?}");
    println!(
        "insights.list {{check: qos_policy_unattached}} → insight: {}",
        unattached[0]
    );
    assert_eq!(unattached[0]["subject"], "policy-map OLD_POLICY_512_OUT");
    assert_eq!(unattached[0]["category"], "hygiene");
    assert_eq!(unattached[0]["severity"], "info");
    assert_eq!(
        evidence_value(&unattached[0], "Configuration"),
        "policy-map OLD_POLICY_512_OUT"
    );

    // The class that is congested: the figures.
    let dropping = listed(app, "qos_class_dropping").await;
    assert_eq!(dropping.len(), 1, "{dropping:?}");
    println!(
        "insights.list {{check: qos_class_dropping}} → insight: {}",
        dropping[0]
    );
    assert_eq!(
        dropping[0]["subject"],
        "GigabitEthernet2/0/4.223415 out VIPSIP_POLICY_2048_OUT/VOICE"
    );
    assert_eq!(dropping[0]["category"], "capacity");
    assert_eq!(dropping[0]["severity"], "medium");
    assert_eq!(
        evidence_value(&dropping[0], "Congestion drops"),
        "average 42.0/s, peak 42.0/s (threshold 10.0/s)"
    );
    assert_eq!(
        evidence_value(&dropping[0], "Share of offered packets dropped"),
        "average 3.5 %, peak 3.5 % (every drop, whatever the reason)"
    );
    assert_eq!(
        evidence_value(&dropping[0], "Traffic in the class"),
        "average 1.2 Mb/s, peak 1.2 Mb/s"
    );
    assert_eq!(
        dropping[0]["suggestion"]["commands"],
        json!(["show policy-map interface GigabitEthernet2/0/4.223415 output"])
    );

    // PE1's partial capture names eight policies and defines none. Nothing
    // is reported about it, and the coverage says that it was not judged.
    for check in [
        "qos_policy_missing",
        "qos_policy_unattached",
        "qos_class_dropping",
    ] {
        assert!(
            open_subjects(app, check)
                .await
                .iter()
                .all(|subject| subject.starts_with("WAN1:")),
            "{check} reported something about PE1"
        );
    }
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    for check in ["qos_policy_missing", "qos_policy_unattached"] {
        let entry = coverage_of(&coverage, check);
        println!("insights.coverage → checks[{check}]: {entry}");
        assert_eq!(entry["evaluated"], 1, "{check}");
        assert_eq!(entry["skipped"], 1, "{check}");
        assert_eq!(entry["findings"], 1, "{check}");
        assert_eq!(entry["reasons"][0]["devices"], 1);
        let reason = entry["reasons"][0]["reason"].as_str().unwrap();
        assert!(reason.contains("only part of one"), "{check}: {reason}");
    }
    let entry = coverage_of(&coverage, "qos_class_dropping");
    println!("insights.coverage → checks[qos_class_dropping]: {entry}");
    assert_eq!(entry["evaluated"], 1);
    assert_eq!(entry["skipped"], 1);
    let reason = entry["reasons"][0]["reason"].as_str().unwrap();
    assert!(
        reason.starts_with("no QoS samples for this device"),
        "{reason}"
    );
}

#[tokio::test]
async fn a_listing_that_stops_early_and_a_read_that_stopped_are_not_evaluated_and_say_why() {
    // CUT1's backup stopped after the policy-maps (the session dropped, say):
    // every policy defined, no interface printed, no closing `end`. And its
    // QoS read worked until twenty-two minutes ago, with a class dropping
    // hard, and has failed since — so the series stop.
    let lab = lab(&["CUT1"], With::default()).await;
    let (app, cut) = (&lab.app, lab.id("CUT1"));
    let whole = whole_configuration();
    let first_interface = whole.find("interface ").expect("an interface");
    config(app.store(), &cut, "running", &whole[..first_interface]);
    let now = now_ms();
    for metric in ["qos.queue_drop_ps", "qos.drop_ps"] {
        let series = app.store().series_id(&cut, metric, VOICE).expect("series");
        let samples: Vec<(i64, i64, f64)> = (0..10)
            .map(|step| (series, now - (58 - step * 4) * 60_000, 90.0))
            .collect();
        app.store().samples_insert(&samples).expect("samples");
    }

    scan(app, json!({})).await;
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    for (check, expected) in [
        ("qos_policy_missing", "does not close with `end`"),
        ("qos_policy_unattached", "does not close with `end`"),
        (
            "qos_class_dropping",
            "the newest QoS sample is older than the sustained period",
        ),
    ] {
        assert!(
            listed(app, check).await.is_empty(),
            "{check} judged what it could not know"
        );
        let entry = coverage_of(&coverage, check);
        println!("insights.coverage → checks[{check}]: {entry}");
        assert_eq!(entry["evaluated"], 0, "{check}: never called clean");
        assert_eq!(entry["skipped"], 1, "{check}");
        assert_eq!(entry["findings"], 0, "{check}");
        let reason = entry["reasons"][0]["reason"].as_str().unwrap();
        assert!(reason.contains(expected), "{check}: {reason}");
    }

    // The next backup is whole and the read works again: now it is judged.
    config(app.store(), &cut, "running", &whole);
    sampled(app, &cut, "qos.queue_drop_ps", VOICE, 90.0);
    scan(app, json!({})).await;
    assert_eq!(
        open_subjects(app, "qos_policy_unattached").await,
        ["CUT1:policy-map OLD_POLICY_512_OUT"]
    );
    assert_eq!(
        open_subjects(app, "qos_policy_missing").await,
        ["CUT1:policy-map VIPSIP_POLICY_800_OUT"]
    );
    assert_eq!(listed(app, "qos_class_dropping").await.len(), 1);
}

#[tokio::test]
async fn a_qos_finding_offers_no_configuration_fix_so_nothing_reaches_the_change_pipeline() {
    let lab = estate(With::all()).await;
    let app = &lab.app;
    scan(app, json!({})).await;
    for check in [
        "qos_policy_missing",
        "qos_policy_unattached",
        "qos_class_dropping",
    ] {
        let insight = listed(app, check).await.remove(0);
        assert_eq!(
            insight["suggestion"].get("config_lines"),
            None,
            "{check} must not guess at configuration"
        );
        let refused = app
            .rpc("insights.fix", json!({"id": insight["id"]}))
            .await
            .unwrap_err();
        assert_eq!(refused.code, "invalid", "{check}");
    }
    assert!(
        app.state::<common::Recorder>().changes.lock().is_empty(),
        "no change was created"
    );
    // None of the three is serious enough to raise an alert by default.
    assert!(app.state::<common::Recorder>().alerts.lock().is_empty());
}

/// Samples of one series at the given minutes ago.
fn sampled_at(
    app: &App,
    device_id: &str,
    metric: &str,
    instance: &str,
    minutes_ago: impl IntoIterator<Item = i64>,
    value: f64,
) {
    let series = app
        .store()
        .series_id(device_id, metric, instance)
        .expect("series");
    let now = now_ms();
    let samples: Vec<(i64, i64, f64)> = minutes_ago
        .into_iter()
        .map(|ago| (series, now - ago * 60_000, value))
        .collect();
    app.store().samples_insert(&samples).expect("samples");
}

/// What monitoring had stored for one series is gone, as if it had never
/// been read: the series row stays, as it does in the core.
fn forget_samples(app: &App, device_id: &str, metric: &str, instance: &str) {
    let series = app
        .store()
        .series_id(device_id, metric, instance)
        .expect("series");
    app.store()
        .with(|conn| conn.execute("DELETE FROM samples WHERE series_id = ?1", [series]))
        .expect("samples removed");
}

async fn insight_state(app: &App, id: &Value) -> (String, u64) {
    let insight = app
        .rpc("insights.get", json!({"id": id}))
        .await
        .expect("the insight is there");
    (
        insight["state"].as_str().unwrap_or_default().to_string(),
        insight["occurrences"].as_u64().unwrap_or_default(),
    )
}

#[tokio::test]
async fn reading_a_congestion_series_nobody_records_does_not_clear_a_class_that_is_dropping() {
    // RB1 keeps one drop counter per queue, and alice's queue is dropping
    // 25 packets a second.
    let lab = lab(&["RB1"], With::default()).await;
    let (app, rb) = (&lab.app, lab.id("RB1"));
    let alice = "ether1|out|subscribers/alice";
    sampled(app, &rb, "qos.drop_ps", alice, 25.0);
    scan(app, json!({})).await;
    let found = listed(app, "qos_class_dropping").await;
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0]["severity"], "low");

    // Somebody asks `metrics.query` for alice's congestion series, which
    // nobody records. The core creates the series row to answer, and puts
    // nothing in it.
    app.store()
        .series_id(&rb, "qos.queue_drop_ps", alice)
        .expect("the read's series row");

    for _ in 0..3 {
        scan(app, json!({})).await;
    }
    assert_eq!(
        insight_state(app, &found[0]["id"]).await,
        ("open".to_string(), 1),
        "still dropping, still reported"
    );
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    let entry = coverage_of(&coverage, "qos_class_dropping");
    assert_eq!(entry["evaluated"], 1, "{entry}");
    assert_eq!(entry["findings"], 1, "{entry}");
}

#[tokio::test]
async fn a_class_that_could_not_be_judged_keeps_its_finding_rather_than_resolving_it() {
    let lab = lab(&["WAN1", "WAN2"], With::default()).await;
    let (app, wan1, wan2) = (&lab.app, lab.id("WAN1"), lab.id("WAN2"));
    let bulk = "GigabitEthernet0/0|out|WAN/BULK";
    let default = "GigabitEthernet0/0|out|WAN/class-default";
    // WAN1's bulk class and WAN2's voice class are congested.
    sampled_at(
        app,
        &wan1,
        "qos.queue_drop_ps",
        bulk,
        (0..=12).map(|step| 48 - step * 4),
        90.0,
    );
    sampled(app, &wan2, "qos.queue_drop_ps", VOICE, 90.0);
    sampled(app, &wan2, "qos.queue_drop_ps", default, 0.0);
    scan(app, json!({})).await;
    let found = listed(app, "qos_class_dropping").await;
    assert_eq!(found.len(), 2, "{found:?}");
    let id_of = |device: &str| {
        found
            .iter()
            .find(|insight| insight["device"] == device)
            .expect("a finding")["id"]
            .clone()
    };
    let (bulk_id, voice_id) = (id_of("WAN1"), id_of("WAN2"));

    // WAN1's QoS read failed for forty-two minutes and has just come back:
    // three samples before the gap, three after, every one of them 90/s.
    // Too few since the gap to call sustained — and too many over the
    // threshold to call it over.
    forget_samples(app, &wan1, "qos.queue_drop_ps", bulk);
    sampled_at(
        app,
        &wan1,
        "qos.queue_drop_ps",
        bulk,
        [58, 54, 50, 8, 4, 0],
        90.0,
    );
    // WAN2's voice class stopped being read twenty minutes ago while its
    // default class goes on being read: gone, or its counters could not be
    // compared. Not known to be dropping now, and not known to have stopped.
    forget_samples(app, &wan2, "qos.queue_drop_ps", VOICE);
    sampled_at(
        app,
        &wan2,
        "qos.queue_drop_ps",
        VOICE,
        (0..10).map(|step| 58 - step * 4),
        90.0,
    );

    for _ in 0..3 {
        scan(app, json!({})).await;
    }
    for (what, id) in [
        ("the class back from a gap", &bulk_id),
        ("the class that stopped", &voice_id),
    ] {
        assert_eq!(
            insight_state(app, id).await,
            ("open".to_string(), 1),
            "{what}: held, neither resolved nor opened again"
        );
    }
    let coverage = app.rpc("insights.coverage", json!({})).await.unwrap();
    let entry = coverage_of(&coverage, "qos_class_dropping");
    println!("insights.coverage → checks[qos_class_dropping]: {entry}");
    assert_eq!(
        entry["evaluated"], 1,
        "WAN2's default class was judged: {entry}"
    );
    assert_eq!(entry["skipped"], 1, "WAN1 could not be: {entry}");
    assert!(
        entry["reasons"][0]["reason"]
            .as_str()
            .unwrap()
            .starts_with("no class could be judged yet"),
        "{entry}"
    );

    // Measured long enough again, WAN1's class is judged once more — and it
    // is the same finding, not a second occurrence.
    sampled_at(app, &wan1, "qos.queue_drop_ps", bulk, [20, 16, 12], 90.0);
    scan(app, json!({})).await;
    assert_eq!(insight_state(app, &bulk_id).await, ("open".to_string(), 1));
}

#[tokio::test]
async fn the_drop_threshold_is_a_setting_and_moving_it_resolves_the_finding() {
    let lab = estate(With::default()).await;
    let app = &lab.app;
    let settings = app
        .rpc("settings.get", json!({"section": "insights"}))
        .await
        .unwrap();
    println!(
        "settings.get {{section: insights}} → thresholds: {}",
        settings["thresholds"]
    );
    assert_eq!(settings["thresholds"]["qos_drop_ps"], 10.0);

    scan(app, json!({})).await;
    assert_eq!(listed(app, "qos_class_dropping").await.len(), 1);

    // 42 drops a second is under a threshold of 100.
    app.rpc(
        "settings.set",
        json!({"section": "insights", "value": {"thresholds": {"qos_drop_ps": 100.0}}}),
    )
    .await
    .unwrap();
    scan(app, json!({})).await;
    scan(app, json!({})).await;
    assert!(
        listed(app, "qos_class_dropping").await.is_empty(),
        "two clean scans resolve it"
    );
    let entry = coverage_of(
        &app.rpc("insights.coverage", json!({})).await.unwrap(),
        "qos_class_dropping",
    )
    .clone();
    assert_eq!(entry["evaluated"], 1, "still measured, and now clean");
    assert_eq!(entry["findings"], 0);

    // Zero would make every class that is measured at all a dropping one.
    // That is a setting to put right: nothing is evaluated, and it says why.
    app.rpc(
        "settings.set",
        json!({"section": "insights", "value": {"thresholds": {"qos_drop_ps": 0.0}}}),
    )
    .await
    .unwrap();
    scan(app, json!({})).await;
    assert!(listed(app, "qos_class_dropping").await.is_empty());
    let entry = coverage_of(
        &app.rpc("insights.coverage", json!({})).await.unwrap(),
        "qos_class_dropping",
    )
    .clone();
    println!("insights.coverage (threshold 0) → checks[qos_class_dropping]: {entry}");
    assert_eq!(entry["evaluated"], 0);
    assert_eq!(entry["skipped"], 2);
    assert!(
        entry["reasons"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("not above zero"),
        "{entry}"
    );
}
