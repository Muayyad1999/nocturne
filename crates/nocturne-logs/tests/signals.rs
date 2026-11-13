//! Rules, the signals they raise, the live tail, and correlation.

mod support;

use nocturne_app::collect;
use nocturne_drivers::model::Kind;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;

const T0: i64 = 1_763_000_000_000;

const CONFIG_CHANGED: &str = "<189>48: *Nov 13 09:02:00.000: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)";
const LINK_DOWN: &str = "<187>45: *Nov 13 09:00:00.000: %LINK-3-UPDOWN: Interface GigabitEthernet1/0/3, changed state to down";
const ERR_DISABLE: &str = "<188>50: *Nov 13 09:05:00.000: %PM-4-ERR_DISABLE: bpduguard error detected on Gi1/0/7, putting Gi1/0/7 in err-disable state";

/// An app whose `alerts.raise` records what it was asked to raise, so the
/// hand-off to the monitoring feature can be seen without linking to it.
async fn lab_with_alerts(names: &[&str]) -> nocturne_app::testing::Lab {
    let mut builder = nocturne_app::testing::builder();
    nocturne_logs::init(&mut builder);
    builder.router.add(
        "alerts.raise",
        |app: nocturne_app::App, params: Value| async move {
            app.publish("test.alert_raised", &params);
            Ok(json!({"id": "alert1"}))
        },
    );
    nocturne_app::testing::lab(builder, names).await
}

#[tokio::test]
async fn a_configuration_change_becomes_the_event_the_config_feature_waits_for() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    let mut events = app.subscribe();

    receive(app, &[(CONFIG_CHANGED, "127.0.0.1", T0)]);

    let published = drain_all(&mut events);
    let changed = topic(&published, "logs.config_changed");
    assert_eq!(changed.len(), 1, "{published:#?}");
    assert_eq!(changed[0]["device_id"], json!(id));
    assert_eq!(changed[0]["by"], json!("admin"));
    assert_eq!(changed[0]["from"], json!("console"));
    assert!(changed[0]["entry_id"].as_i64().unwrap() > 0);

    let signal = topic(&published, "logs.signal")[0];
    assert_eq!(signal["kind"], json!("config_changed"));
    assert_eq!(signal["rule_id"], json!("builtin.ios.config_changed"));
    assert_eq!(signal["device"], json!("SW1"));
    assert_eq!(signal["count"], json!(1));

    // A signal is kept, so the correlation can be made afterwards too.
    let stored = query(app, json!({"stream": "event"})).await;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0]["source"], json!("logs.config_changed"));
    assert_eq!(stored[0]["device_id"], json!(id));
}

#[tokio::test]
async fn an_interface_signal_names_the_port_the_device_actually_has() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    collect::store(
        app,
        &id,
        Kind::Interfaces,
        vec![json!({"name": "GigabitEthernet1/0/7"})],
        "test",
    )
    .unwrap();
    let mut events = app.subscribe();

    // The line writes the port short; the device's own list writes it long.
    receive(app, &[(ERR_DISABLE, "127.0.0.1", T0)]);
    let signal = next_event(&mut events, "logs.signal").await;
    assert_eq!(signal["kind"], json!("alert"));
    assert_eq!(
        signal["interface"],
        json!("GigabitEthernet1/0/7"),
        "resolved against the interfaces collected from that device"
    );
    assert_eq!(
        signal["title"],
        json!("Gi1/0/7 error-disabled (bpduguard)"),
        "the title is rendered from what the pattern captured"
    );
}

#[tokio::test]
async fn a_flapping_port_raises_one_signal_a_window_and_counts_the_rest() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let mut events = app.subscribe();

    // Twenty link-downs inside one 60-second throttle window.
    let burst: Vec<(&str, &str, i64)> = (0..20)
        .map(|index| (LINK_DOWN, "127.0.0.1", T0 + index * 1_000))
        .collect();
    receive(app, &burst);
    let signals = drained(&mut events, "logs.signal");
    assert_eq!(
        signals.len(),
        1,
        "one signal, not twenty: {}",
        signals.len()
    );
    assert_eq!(signals[0]["count"], json!(1));

    // The next window reports how many were swallowed by the last one.
    receive(app, &[(LINK_DOWN, "127.0.0.1", T0 + 61_000)]);
    let signals = drained(&mut events, "logs.signal");
    assert_eq!(signals.len(), 1);
    assert_eq!(
        signals[0]["count"],
        json!(20),
        "nineteen suppressed plus this one: nothing is lost, only compressed"
    );

    // Every line is still stored and still findable.
    assert_eq!(query(app, json!({"limit": 100})).await.len(), 21);
}

#[tokio::test]
async fn an_alert_signal_is_handed_to_the_monitoring_feature_when_it_is_there() {
    let lab = lab_with_alerts(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    let mut events = app.subscribe();

    receive(app, &[(ERR_DISABLE, "127.0.0.1", T0)]);
    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(raised["device_id"], json!(id));
    assert_eq!(raised["severity"], json!("high"));
    assert_eq!(raised["title"], json!("Gi1/0/7 error-disabled (bpduguard)"));
    assert_eq!(
        raised["dedup_key"],
        json!(format!("logs:builtin.ios.err_disable:{id}:")),
        "the same port error-disabling twice is one alert"
    );
}

#[tokio::test]
async fn a_missing_monitoring_feature_is_not_an_error() {
    // No `alerts.raise` in this build at all.
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let mut events = app.subscribe();

    receive(app, &[(ERR_DISABLE, "127.0.0.1", T0)]);
    let signal = next_event(&mut events, "logs.signal").await;
    assert_eq!(signal["kind"], json!("alert"));
    // And the line is stored, and the app is still working.
    assert_eq!(query(app, json!({"limit": 10})).await.len(), 1);
    assert!(app.rpc("logs.status", Value::Null).await.is_ok());
}

#[tokio::test]
async fn every_builtin_rule_does_what_it_says_with_the_line_it_ships_with() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;

    let rules = app.rpc("logs.rules.list", Value::Null).await.unwrap();
    let rules = rules.as_array().unwrap();
    assert!(rules.len() >= 20, "{} rules", rules.len());
    for rule in rules {
        assert_eq!(rule["builtin"], json!(true));
        let result = app
            .rpc(
                "logs.rules.test",
                json!({"rule": rule, "line": rule["test_line"]}),
            )
            .await
            .unwrap();
        assert_eq!(
            result["matched"],
            json!(true),
            "{} did not match its own test line: {result:#}",
            rule["id"]
        );
        assert!(
            !result["signal"]["kind"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "{result:#}"
        );
        assert!(
            result["parsed"]["mnemonic"].is_string() || result["parsed"]["severity"].is_number(),
            "a pasted line is shown as it was understood: {result:#}"
        );
    }

    // And a line it is not about is refused with a reason.
    let refused = app
        .rpc(
            "logs.rules.test",
            json!({"rule": rules[0], "line": "nothing to see here"}),
        )
        .await
        .unwrap();
    assert_eq!(refused["matched"], json!(false));
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("does not match"),
        "{refused:#}"
    );
}

#[tokio::test]
async fn a_builtin_rule_can_be_switched_off_but_not_deleted() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;

    let refused = app
        .rpc(
            "logs.rules.delete",
            json!({"id": "builtin.ios.config_changed"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden");
    assert!(refused.message.contains("switch it off"), "{refused}");

    let rule: Value = app
        .rpc("logs.rules.list", Value::Null)
        .await
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|rule| rule["id"] == json!("builtin.ios.config_changed"))
        .cloned()
        .expect("the rule is there");
    let mut off = rule.clone();
    off["enabled"] = json!(false);
    off["pattern"] = json!("something else entirely");
    let saved = app.rpc("logs.rules.save", off).await.unwrap();
    assert_eq!(saved["enabled"], json!(false));
    assert_eq!(
        saved["pattern"], rule["pattern"],
        "a built-in's definition belongs to the application; only the switch is the user's"
    );

    let mut events = app.subscribe();
    receive(app, &[(CONFIG_CHANGED, "127.0.0.1", T0)]);
    assert!(
        drained(&mut events, "logs.config_changed").is_empty(),
        "a rule that is off says nothing"
    );
}

#[tokio::test]
async fn a_rule_of_ones_own_can_be_written_tested_and_deleted() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;

    let broken = app
        .rpc(
            "logs.rules.save",
            json!({"name": "unbalanced", "pattern": "(oops", "emit": {"kind": "reload"}}),
        )
        .await
        .unwrap_err();
    assert_eq!(broken.code, "invalid");
    assert!(broken.message.contains("does not compile"), "{broken}");

    let saved = app
        .rpc(
            "logs.rules.save",
            json!({
                "name": "Temperature warning",
                "pattern": r"%ENVMON-\d-TEMP: (?P<sensor>[\w ]+) temperature (?P<state>\w+)",
                "emit": {"kind": "alert", "severity": "critical", "title": "{sensor} is {state}"},
                "throttle_s": 300,
            }),
        )
        .await
        .unwrap();
    assert_eq!(saved["builtin"], json!(false));
    let id = saved["id"].as_str().unwrap().to_string();
    assert!(!id.is_empty());

    let result = app
        .rpc(
            "logs.rules.test",
            json!({"rule": saved, "line": "<185>60: *Nov 13 09:09:00.000: %ENVMON-1-TEMP: Inlet temperature critical"}),
        )
        .await
        .unwrap();
    assert_eq!(result["matched"], json!(true));
    assert_eq!(result["groups"]["sensor"], json!("Inlet"));
    assert_eq!(result["signal"]["title"], json!("Inlet is critical"));
    assert_eq!(result["signal"]["severity"], json!("critical"));

    assert_eq!(
        app.rpc(
            "logs.rules.save",
            json!({"name": "bad severity", "pattern": "x",
                   "emit": {"kind": "alert", "severity": "urgent", "title": "x"}}),
        )
        .await
        .unwrap_err()
        .code,
        "invalid"
    );

    app.rpc("logs.rules.delete", json!({"id": id}))
        .await
        .unwrap();
    let left = app.rpc("logs.rules.list", Value::Null).await.unwrap();
    assert!(
        left.as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["name"] != json!("Temperature warning"))
    );
}

#[tokio::test]
async fn the_live_tail_batches_and_says_when_it_had_to_sample() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let mut events = app.subscribe();

    let storm: Vec<(&str, &str, i64)> = (0..1_000)
        .map(|index| (LINK_DOWN, "127.0.0.1", T0 + index))
        .collect();
    receive(app, &storm);
    nocturne_logs::tail::flush(app, T0);

    let appended = drained(&mut events, "logs.appended");
    assert_eq!(appended.len(), 1, "one event for a thousand lines");
    let event = &appended[0];
    assert_eq!(event["entries"].as_array().unwrap().len(), 200);
    assert_eq!(event["received"], json!(1_000));
    assert_eq!(event["dropped"], json!(800));
    assert_eq!(event["sampled"], json!(true));
    let first = event["entries"][0]["id"].as_i64().unwrap();
    let last = event["entries"][199]["id"].as_i64().unwrap();
    assert!(
        last - first > 900,
        "a sample spans the window rather than being its first two hundred lines"
    );

    // A quiet moment passes everything through and says it did not sample.
    receive(app, &[(CONFIG_CHANGED, "127.0.0.1", T0 + 2_000)]);
    nocturne_logs::tail::flush(app, T0);
    let appended = drained(&mut events, "logs.appended");
    assert_eq!(appended[0]["entries"].as_array().unwrap().len(), 1);
    assert_eq!(appended[0]["sampled"], json!(false));
    assert_eq!(appended[0]["dropped"], json!(0));
}

#[tokio::test]
async fn a_follow_gets_only_what_matches_the_filter_it_was_opened_with() {
    let lab = lab(&["SW1", "SW2"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    set_address(app, &lab.id("SW2"), "127.0.0.2").await;

    let follow = app
        .rpc(
            "logs.follow",
            json!({"device_id": lab.id("SW2"), "max_severity": 3}),
        )
        .await
        .unwrap();
    let follow_id = follow["follow_id"].as_str().unwrap().to_string();
    assert_eq!(follow["event"], json!("logs.tail"));

    let mut events = app.subscribe();
    receive(
        app,
        &[
            (LINK_DOWN, "127.0.0.1", T0),          // SW1, severity 3
            (LINK_DOWN, "127.0.0.2", T0 + 1),      // SW2, severity 3
            (CONFIG_CHANGED, "127.0.0.2", T0 + 2), // SW2, severity 5
        ],
    );
    nocturne_logs::tail::flush(app, T0);

    let published = drain_all(&mut events);
    let tail = topic(&published, "logs.tail");
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0]["follow_id"], json!(follow_id));
    let entries = tail[0]["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "the filter is the one the search uses: {entries:#?}"
    );
    assert_eq!(entries[0]["device_id"], json!(lab.id("SW2")));
    assert_eq!(entries[0]["severity"], json!(3));

    let appended = topic(&published, "logs.appended");
    assert_eq!(
        appended[0]["entries"].as_array().unwrap().len(),
        3,
        "the unfiltered tail still carries everything"
    );

    assert_eq!(
        app.rpc("logs.unfollow", json!({"id": follow_id}))
            .await
            .unwrap(),
        json!({"closed": true})
    );
    assert_eq!(
        app.rpc("logs.unfollow", json!({"id": "gone"}))
            .await
            .unwrap(),
        json!({"closed": false})
    );
}

#[tokio::test]
async fn around_a_line_is_what_else_was_known_at_that_moment() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    set_address(app, &id, "127.0.0.1").await;
    collect::store(
        app,
        &id,
        Kind::Interfaces,
        vec![json!({"name": "GigabitEthernet1/0/3"})],
        "test",
    )
    .unwrap();

    let flap: Vec<(&str, &str, i64)> = (0..6)
        .map(|index| (LINK_DOWN, "127.0.0.1", T0 + index * 1_000))
        .collect();
    receive(app, &flap);
    let entries = receive(app, &[(CONFIG_CHANGED, "127.0.0.1", T0 + 7_000)]);
    let entry_id = entries[0].entry.id;

    let around = app
        .rpc("logs.around", json!({"entry_id": entry_id, "window_s": 60}))
        .await
        .unwrap();
    assert_eq!(around["center"]["id"], json!(entry_id));
    assert_eq!(around["device_id"], json!(id));
    assert_eq!(around["entries"].as_array().unwrap().len(), 7);
    let bursts = around["bursts"].as_array().unwrap();
    assert_eq!(bursts.len(), 1, "{bursts:#?}");
    assert_eq!(bursts[0]["count"], json!(6));
    assert_eq!(bursts[0]["interface"], json!("GigabitEthernet1/0/3"));
    assert_eq!(
        around["signals"].as_array().unwrap().len(),
        2,
        "the link-down and the configuration change this crate's own rules raised"
    );

    // Neither monitoring nor the change pipeline is in this build, and the
    // result says so instead of reading as "nothing happened".
    assert_eq!(around["alerts"]["state"], json!("unavailable"));
    assert!(
        around["alerts"]["note"]
            .as_str()
            .unwrap()
            .contains("alerts.list"),
        "{:#}",
        around["alerts"]
    );
    assert_eq!(around["changes"]["state"], json!("unavailable"));

    assert_eq!(
        app.rpc("logs.around", json!({"entry_id": 999_999}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}
