//! Rules that count, the security rules that ship, and alerts that clear.
//!
//! Lines are stored through the receiver's own path at times of the test's
//! choosing (`support::receive`). The monitoring feature is a fake that
//! records what it was asked (`support::lab_with_monitor`): this crate
//! reaches `alerts.raise` and `alerts.clear` by name and links to nothing.
//!
//! Two clocks appear here, as they do in the code. A line carries its own
//! time (`T0 + …`), and everything decided when a line arrives is decided
//! on that. What is decided when no line arrives — a quiet period ending —
//! is decided on the wall clock, which the tests move by calling
//! `rules::sweep` with a later time rather than by waiting.

mod support;

use nocturne_app::util::now_ms;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;

const T0: i64 = 1_763_000_000_000;

const LOGIN_BURST: &str = "builtin.ios.login_burst";
const LOGIN_AFTER_BURST: &str = "builtin.ios.login_after_burst";
const FLAP_STORM: &str = "builtin.ios.flap_storm";
const BGP: &str = "builtin.ios.bgp_adjacency";

/// A failed login as an IOS device with `login on-failure log` sends it.
fn failed_login(source: &str) -> String {
    format!(
        "<188>61: *Nov 13 09:10:01.000: %SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: admin] [Source: {source}] [localport: 22] [Reason: Login Authentication Failed] at 09:10:01 UTC Fri Nov 13 2026"
    )
}

fn successful_login(source: &str) -> String {
    format!(
        "<189>70: *Nov 13 09:11:00.000: %SEC_LOGIN-5-LOGIN_SUCCESS: Login Success [user: admin] [Source: {source}] [localport: 22] at 09:11:00 UTC Fri Nov 13 2026"
    )
}

fn link_down(interface: &str) -> String {
    format!(
        "<187>45: *Nov 13 09:00:00.000: %LINK-3-UPDOWN: Interface {interface}, changed state to down"
    )
}

fn bgp(neighbor: &str, state: &str) -> String {
    let reason = if state == "Down" {
        " BGP Notification sent"
    } else {
        ""
    };
    format!("<189>80: *Nov 13 09:20:00.000: %BGP-5-ADJCHANGE: neighbor {neighbor} {state}{reason}")
}

/// `count` copies of one line, a second apart, starting at `from`.
fn repeated(line: &str, count: i64, from: i64) -> Vec<(&str, &str, i64)> {
    (0..count)
        .map(|index| (line, "127.0.0.1", from + index * 1_000))
        .collect()
}

async fn one_switch() -> (nocturne_app::testing::Lab, FakeMonitor, String) {
    let (lab, monitor) = lab_with_monitor(&["SW1"]).await;
    let id = lab.id("SW1");
    set_address(&lab.app, &id, "127.0.0.1").await;
    (lab, monitor, id)
}

// ------------------------------------------------------------- thresholds

#[tokio::test]
async fn seven_failed_logins_from_one_address_raise_nothing_and_the_eighth_does() {
    let (lab, _monitor, id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    let attacker = failed_login("10.0.0.5");
    let other = failed_login("10.0.0.6");
    receive(app, &repeated(&attacker, 7, T0));
    // Another address failing in between is another count, not this one's.
    receive(app, &repeated(&other, 7, T0 + 7_000));
    assert!(
        of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST).is_empty(),
        "seven from each of two addresses is fourteen lines and no burst"
    );

    receive(app, &[(attacker.as_str(), "127.0.0.1", T0 + 20_000)]);
    let bursts = of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST);
    assert_eq!(bursts.len(), 1, "{bursts:#?}");
    assert_eq!(bursts[0]["kind"], json!("alert"));
    assert_eq!(bursts[0]["instance"], json!("10.0.0.5"));
    assert_eq!(
        bursts[0]["title"],
        json!("Repeated failed logins from 10.0.0.5")
    );
    assert_eq!(
        bursts[0]["count"],
        json!(8),
        "the signal stands for the eight lines that made it"
    );

    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(raised["severity"], json!("high"));
    assert_eq!(raised["instance"], json!("10.0.0.5"));
    assert_eq!(
        raised["dedup_key"],
        json!(format!("logs:{LOGIN_BURST}:{id}:10.0.0.5"))
    );
}

#[tokio::test]
async fn failed_logins_spread_over_an_hour_are_not_a_burst() {
    let (lab, _monitor, _id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    let line = failed_login("10.0.0.5");
    let slow: Vec<(&str, &str, i64)> = (0..12)
        .map(|index| (line.as_str(), "127.0.0.1", T0 + index * 300_000))
        .collect();
    receive(app, &slow);
    assert!(
        of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST).is_empty(),
        "one failure every five minutes is never eight in a minute"
    );
}

#[tokio::test]
async fn the_eighth_failure_a_whole_minute_after_the_first_does_not_count_with_it() {
    let (lab, _monitor, _id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    // Seven a second apart, and the eighth exactly sixty seconds after the
    // first: the window is half open, so the first has just left it.
    let edge = failed_login("10.0.0.5");
    receive(app, &repeated(&edge, 7, T0));
    receive(app, &[(edge.as_str(), "127.0.0.1", T0 + 60_000)]);
    assert!(
        of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST).is_empty(),
        "eight lines of which the first and the last are a full minute apart are not eight in a minute"
    );
    // The window slides rather than starting again: the next line is a
    // millisecond inside a minute of the second one, and makes eight.
    receive(app, &[(edge.as_str(), "127.0.0.1", T0 + 60_999)]);
    let bursts = of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST);
    assert_eq!(bursts.len(), 1, "{bursts:#?}");
    assert_eq!(bursts[0]["count"], json!(8));

    // A millisecond less than a minute from first to eighth is a burst.
    let inside = failed_login("10.0.0.6");
    receive(app, &repeated(&inside, 7, T0));
    receive(app, &[(inside.as_str(), "127.0.0.1", T0 + 59_999)]);
    let bursts = of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST);
    assert_eq!(bursts.len(), 1, "{bursts:#?}");
    assert_eq!(bursts[0]["instance"], json!("10.0.0.6"));
}

#[tokio::test]
async fn a_made_up_user_name_cannot_move_a_failed_login_to_another_address() {
    let (lab, _monitor, _id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    // The user name is whatever was typed at the prompt. Here it is typed
    // to look like the rest of the line, with a different address each
    // time, so that no address ever reaches eight.
    let ios: Vec<String> = (0..8)
        .map(|attempt| {
            format!(
                "<188>61: *Nov 13 09:10:01.000: %SEC_LOGIN-4-LOGIN_FAILED: Login failed [user: x] [Source: 198.51.100.{attempt}] [localport: 22] [Reason: y] [Source: 10.0.0.5] [localport: 22] [Reason: Login Authentication Failed] at 09:10:01 UTC Fri Nov 13 2026"
            )
        })
        .collect();
    let lines: Vec<(&str, &str, i64)> = ios
        .iter()
        .enumerate()
        .map(|(index, line)| (line.as_str(), "127.0.0.1", T0 + index as i64 * 1_000))
        .collect();
    receive(app, &lines);
    let bursts = of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST);
    assert_eq!(bursts.len(), 1, "{bursts:#?}");
    assert_eq!(
        bursts[0]["instance"],
        json!("10.0.0.5"),
        "counted under the address the device wrote, not one the name carried"
    );
    assert_eq!(
        bursts[0]["title"],
        json!("Repeated failed logins from 10.0.0.5")
    );

    let routeros: Vec<String> = (0..8)
        .map(|attempt| {
            format!(
                "<131>system,error,critical login failure for user root from 198.51.100.{attempt} via ssh from 10.0.0.7 via telnet"
            )
        })
        .collect();
    let lines: Vec<(&str, &str, i64)> = routeros
        .iter()
        .enumerate()
        .map(|(index, line)| {
            (
                line.as_str(),
                "127.0.0.1",
                T0 + 100_000 + index as i64 * 1_000,
            )
        })
        .collect();
    receive(app, &lines);
    let bursts = of_rule(
        &drained(&mut events, "logs.signal"),
        "builtin.routeros.login_burst",
    );
    assert_eq!(bursts.len(), 1, "{bursts:#?}");
    assert_eq!(bursts[0]["instance"], json!("10.0.0.7"));
}

#[tokio::test]
async fn a_burst_that_goes_on_is_one_signal_a_window_with_the_lines_it_swallowed() {
    let (lab, _monitor, _id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    let line = failed_login("10.0.0.5");
    // Forty failures in forty seconds: the threshold is reached five times.
    receive(app, &repeated(&line, 40, T0));
    let bursts = of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST);
    assert_eq!(bursts.len(), 1, "one alert, not five");
    assert_eq!(bursts[0]["count"], json!(8));

    // After the window the next burst reports the thirty-two in between.
    receive(app, &repeated(&line, 8, T0 + 70_000));
    let bursts = of_rule(&drained(&mut events, "logs.signal"), LOGIN_BURST);
    assert_eq!(bursts.len(), 1);
    assert_eq!(bursts[0]["count"], json!(40));
}

#[tokio::test]
async fn a_login_that_succeeds_after_a_burst_is_its_own_alert() {
    let (lab, _monitor, id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    // People log in all day; without a burst before it that is nothing.
    let innocent = successful_login("10.0.0.9");
    receive(app, &[(innocent.as_str(), "127.0.0.1", T0)]);
    assert!(
        of_rule(&drained(&mut events, "logs.signal"), LOGIN_AFTER_BURST).is_empty(),
        "an ordinary login is not an alert"
    );

    let attacker = failed_login("10.0.0.5");
    receive(app, &repeated(&attacker, 8, T0 + 1_000));
    // Somebody else logging in during the attack is still somebody else.
    receive(app, &[(innocent.as_str(), "127.0.0.1", T0 + 10_000)]);
    assert!(of_rule(&drained(&mut events, "logs.signal"), LOGIN_AFTER_BURST).is_empty());

    let got_in = successful_login("10.0.0.5");
    receive(app, &[(got_in.as_str(), "127.0.0.1", T0 + 11_000)]);
    let after = of_rule(&drained(&mut events, "logs.signal"), LOGIN_AFTER_BURST);
    assert_eq!(after.len(), 1, "{after:#?}");
    assert_eq!(after[0]["severity"], json!("critical"));
    assert_eq!(
        after[0]["title"],
        json!("A login from 10.0.0.5 succeeded straight after repeated failures")
    );

    let keys: Vec<Value> = [
        next_event(&mut events, "test.alert_raised").await,
        next_event(&mut events, "test.alert_raised").await,
    ]
    .iter()
    .map(|raised| raised["dedup_key"].clone())
    .collect();
    assert_eq!(
        keys,
        [
            json!(format!("logs:{LOGIN_BURST}:{id}:10.0.0.5")),
            json!(format!("logs:{LOGIN_AFTER_BURST}:{id}:10.0.0.5")),
        ]
    );
}

#[tokio::test]
async fn a_threshold_of_ones_own_is_saved_explained_and_counted_per_group() {
    let (lab, _monitor, _id) = one_switch().await;
    let app = &lab.app;

    let refused = app
        .rpc(
            "logs.rules.save",
            json!({
                "name": "ACL denies",
                "pattern": r"%SEC-6-IPACCESSLOGP: list (?P<acl>\S+) denied",
                "threshold": {"count": 3, "window_s": 60, "group_by": ["list"]},
                "emit": {"kind": "alert", "severity": "medium", "title": "{acl} is denying a lot"},
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid");
    assert!(
        refused.message.contains("no group named `list`"),
        "{refused}"
    );

    let saved = app
        .rpc(
            "logs.rules.save",
            json!({
                "name": "ACL denies",
                "pattern": r"%SEC-6-IPACCESSLOGP: list (?P<acl>\S+) denied",
                "threshold": {"count": 3, "window_s": 60, "group_by": ["acl"]},
                "clear_after_s": 300,
                "emit": {"kind": "alert", "severity": "medium", "title": "{acl} is denying a lot"},
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        saved["threshold"],
        json!({"count": 3, "window_s": 60, "group_by": ["acl"]})
    );
    assert_eq!(saved["clear_after_s"], json!(300));
    let rule_id = saved["id"].as_str().unwrap().to_string();

    let deny = |acl: &str| {
        format!(
            "<190>90: *Nov 13 09:30:00.000: %SEC-6-IPACCESSLOGP: list {acl} denied tcp 10.0.0.5(1234) -> 10.0.9.9(22), 1 packet"
        )
    };
    let tested = app
        .rpc(
            "logs.rules.test",
            json!({"rule": saved, "line": deny("EDGE-IN")}),
        )
        .await
        .unwrap();
    assert_eq!(tested["matched"], json!(true));
    assert_eq!(tested["instance"], json!("EDGE-IN"));
    assert!(
        tested["note"]
            .as_str()
            .unwrap()
            .contains("3 of them arrive inside 60 s"),
        "a pasted line that matches must not read as `this raises the alert`: {tested:#}"
    );

    let mut events = app.subscribe();
    let (edge, core) = (deny("EDGE-IN"), deny("CORE-IN"));
    receive(
        app,
        &[
            (edge.as_str(), "127.0.0.1", T0),
            (core.as_str(), "127.0.0.1", T0 + 1_000),
            (edge.as_str(), "127.0.0.1", T0 + 2_000),
            (core.as_str(), "127.0.0.1", T0 + 3_000),
        ],
    );
    assert!(
        of_rule(&drained(&mut events, "logs.signal"), &rule_id).is_empty(),
        "two each: four lines, and neither list has three"
    );
    receive(app, &[(edge.as_str(), "127.0.0.1", T0 + 4_000)]);
    let signals = of_rule(&drained(&mut events, "logs.signal"), &rule_id);
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0]["title"], json!("EDGE-IN is denying a lot"));
    assert_eq!(signals[0]["instance"], json!("EDGE-IN"));
}

#[tokio::test]
async fn a_flood_of_made_up_sources_cannot_grow_a_rule_without_bound() {
    let (lab, monitor, _id) = one_switch().await;
    let app = &lab.app;

    // Each line claims a different attacker: 50,000 group keys for one rule.
    let lines: Vec<String> = (0..50_000u32)
        .map(|index| {
            format!(
                "<131>system,error,critical login failure for user root from 10.{}.{}.{} via telnet",
                index >> 16,
                (index >> 8) & 255,
                index & 255
            )
        })
        .collect();
    let flood: Vec<(&str, &str, i64)> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| (line.as_str(), "127.0.0.1", T0 + index as i64))
        .collect();
    receive(app, &flood);

    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    let rules = &status["rules"];
    assert_eq!(rules["max_per_rule"], json!(10_000));
    assert_eq!(
        rules["forgotten"],
        json!(40_000),
        "the oldest tenth went each time the rule filled up, and the status counts every one: {rules:#}"
    );
    let tracked = rules["tracked"].as_u64().unwrap();
    assert!(
        (10_000..=10_010).contains(&tracked),
        "the rule holds its cap and no more, plus the other rules' few keys: {tracked}"
    );
    // One line each is nobody's burst: nothing was raised over it.
    settle().await;
    assert_eq!(monitor.raise_calls(), 0);
    assert_eq!(rules["open_alerts"], json!(0));
    // And nothing was lost from the store over it.
    assert_eq!(status["store"]["rows"], json!(50_000));
}

#[tokio::test]
async fn a_capture_as_long_as_the_line_is_cut_before_it_is_a_key_or_a_title() {
    let (lab, monitor, _id) = one_switch().await;
    let app = &lab.app;
    let saved = app
        .rpc(
            "logs.rules.save",
            json!({
                "name": "Thing",
                "pattern": r"%TEST-3-THING: (?P<what>\S+) (?P<detail>.*)",
                "threshold": {"count": 1, "window_s": 60, "group_by": ["what"]},
                "emit": {"kind": "alert", "severity": "low", "title": "Thing: {what}"},
            }),
        )
        .await
        .unwrap();
    let mut events = app.subscribe();

    // A line of the largest size the receiver takes, whose captures are
    // nearly all of it, with an escape sequence and a direction override in
    // front for good measure.
    let line = format!(
        "<187>%TEST-3-THING: \u{1b}[31m\u{202e}{} {}",
        "A".repeat(50_000),
        "B".repeat(9_000)
    );
    receive(app, &[(line.as_str(), "127.0.0.1", T0)]);
    eventually("the alert to be raised", || monitor.raise_calls() == 1).await;
    let published = drain_all(&mut events);
    let raised = topic(&published, "test.alert_raised")[0].clone();

    // The signal is an event on the bus and a row in the event stream: it
    // carries what was captured, not the line all over again.
    let signal = topic(&published, "logs.signal")[0];
    for (name, value) in signal["groups"].as_object().unwrap() {
        let value = value.as_str().unwrap();
        assert!(value.chars().count() <= 2_000, "{name}: {}", value.len());
        assert!(!value.chars().any(char::is_control), "{name}");
    }
    assert!(
        serde_json::to_string(signal).unwrap().len() < 8_000,
        "a signal about a 59 KiB line is not 59 KiB itself"
    );
    // The line itself is stored whole.
    let stored = query(app, json!({"limit": 1})).await;
    assert!(stored[0]["body"]["raw"].as_str().unwrap().len() > 59_000);

    let instance = raised["instance"].as_str().unwrap();
    assert!(
        (190..=200).contains(&instance.chars().count()),
        "{instance}"
    );
    assert!(instance.ends_with('…'), "what was cut says so: {instance}");
    assert!(
        instance.starts_with("[31m AAAA"),
        "the escape and the override are spaces, the text is still text: {instance}"
    );
    let title = raised["title"].as_str().unwrap();
    assert_eq!(title.chars().count(), 300);
    assert!(title.starts_with("Thing:  [31m AAAA"), "{title}");
    assert_eq!(raised["detail"].as_str().unwrap().chars().count(), 2_000);
    assert!(
        raised["dedup_key"].as_str().unwrap().len() < 400,
        "a key is a key, not a copy of the line"
    );
    for text in [instance, title, raised["dedup_key"].as_str().unwrap()] {
        assert!(
            !text.chars().any(|ch| ch.is_control() || ch == '\u{202e}'),
            "nothing that moves a cursor or turns the text round: {text:?}"
        );
    }

    // The rule tester counts the line under the same instance.
    let tested = app
        .rpc("logs.rules.test", json!({"rule": saved, "line": line}))
        .await
        .unwrap();
    assert_eq!(tested["instance"], raised["instance"]);
}

#[tokio::test]
async fn a_sender_inventing_a_port_per_line_gets_a_bounded_number_of_signals() {
    let (lab, _monitor, _id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    // Three hundred ports error-disabled in one second, on one switch.
    let lines: Vec<String> = (0..300)
        .map(|port| {
            format!(
                "<188>50: *Nov 13 09:05:00.000: %PM-4-ERR_DISABLE: bpduguard error detected on Gi9/0/{port}, putting Gi9/0/{port} in err-disable state"
            )
        })
        .collect();
    let storm: Vec<(&str, &str, i64)> = lines
        .iter()
        .map(|line| (line.as_str(), "127.0.0.1", T0))
        .collect();
    receive(app, &storm);

    let signals = of_rule(
        &drained(&mut events, "logs.signal"),
        "builtin.ios.err_disable",
    );
    assert_eq!(
        signals.len(),
        256,
        "each port is its own signal, up to what one device can plausibly mean"
    );
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(
        status["rules"]["held_back"],
        json!(44),
        "the rest are held back and the status says how many: {:#}",
        status["rules"]
    );
    // Every line is stored all the same.
    assert_eq!(status["store"]["rows"], json!(300));
}

// ------------------------------------------------- the security built-ins

#[tokio::test]
async fn a_port_security_violation_raises_an_alert_that_clears_after_a_quiet_half_hour() {
    let (lab, monitor, id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    // The line as Cisco's documentation prints it (see the rule's source).
    let violation = "<186>00:22:51: %PORT_SECURITY-2-PSECURE_VIOLATION: Security violation occurred, caused by MAC address 0011.8565.4B75 on port FastEthernet1/0/2.";
    receive(app, &[(violation, "127.0.0.1", T0)]);
    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(
        raised["title"],
        json!("Port-security violation on FastEthernet1/0/2 by 0011.8565.4B75")
    );
    assert_eq!(
        raised["instance"],
        json!("FastEthernet1/0/2"),
        "the full stop that ends the sentence is not part of the port"
    );
    let key = format!("logs:builtin.ios.port_security:{id}:FastEthernet1/0/2");
    assert_eq!(raised["dedup_key"], json!(key));
    assert_eq!(monitor.open_keys(), vec![key.clone()]);
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["rules"]["open_alerts"], json!(1));

    // Twenty-nine quiet minutes are not thirty.
    let wall = now_ms();
    nocturne_logs::rules::sweep(app, wall + 29 * 60_000);
    assert!(drained(&mut events, "logs.signal").is_empty());
    assert_eq!(monitor.open_keys(), vec![key.clone()]);

    nocturne_logs::rules::sweep(app, wall + 31 * 60_000);
    let cleared = next_event(&mut events, "logs.signal").await;
    assert_eq!(cleared["kind"], json!("cleared"));
    assert_eq!(cleared["reason"], json!("quiet"));
    assert_eq!(cleared["rule_id"], json!("builtin.ios.port_security"));
    assert_eq!(
        cleared["entry_id"],
        Value::Null,
        "no line said so: time did"
    );
    let asked = next_event(&mut events, "test.alert_cleared").await;
    assert_eq!(asked["dedup_key"], json!(key));
    assert_eq!(asked["cleared"], json!(true));
    assert!(
        asked["note"].as_str().unwrap().contains("1800 s"),
        "the alert's own record says why it closed: {asked:#}"
    );
    assert!(monitor.open_keys().is_empty());

    // A clearing is kept like any other signal.
    let stored = query(app, json!({"stream": "event"})).await;
    assert_eq!(stored[0]["source"], json!("logs.cleared"));
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["rules"]["open_alerts"], json!(0));
}

#[tokio::test]
async fn dhcp_snooping_and_arp_inspection_drops_are_alerts() {
    let (lab, _monitor, id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    let rogue_server = "<189>%DHCP_SNOOPING-5-DHCP_SNOOPING_UNTRUSTED_PORT: DHCP_SNOOPING drop message on untrusted port, message type: DHCPOFFER, MAC sa: 5c5a.c7d6.9e1f";
    receive(app, &[(rogue_server, "127.0.0.1", T0)]);
    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(raised["severity"], json!("medium"));
    assert_eq!(
        raised["title"],
        json!("DHCP snooping dropped a message (UNTRUSTED_PORT)")
    );
    assert_eq!(
        raised["dedup_key"],
        json!(format!(
            "logs:builtin.ios.dhcp_snooping:{id}:UNTRUSTED_PORT"
        ))
    );
    assert!(
        raised["detail"].as_str().unwrap().contains("DHCPOFFER"),
        "what was dropped is in the detail: {raised:#}"
    );

    let spoofed_arp = "<188>%SW_DAI-4-DHCP_SNOOPING_DENY: 1 Invalid ARPs (Req) on Gi1/0/2, vlan 10.([0011.2233.4455/10.0.0.3/0000.0000.0000/10.0.0.1/02:46:49 UTC Mon Mar 1 2027])";
    receive(app, &[(spoofed_arp, "127.0.0.1", T0 + 1_000)]);
    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(raised["severity"], json!("high"));
    assert_eq!(
        raised["title"],
        json!("Dynamic ARP inspection dropped ARP packets on Gi1/0/2 (VLAN 10)")
    );
    assert_eq!(raised["instance"], json!("Gi1/0/2"));
}

#[tokio::test]
async fn a_port_that_keeps_going_down_is_a_flap_storm_and_one_that_went_down_is_not() {
    let (lab, _monitor, id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    let flapping = link_down("GigabitEthernet1/0/3");
    let steady = link_down("GigabitEthernet1/0/9");
    // Four downs a minute apart on one port, and one on another.
    let mut lines: Vec<(&str, &str, i64)> = (0..4)
        .map(|index| (flapping.as_str(), "127.0.0.1", T0 + index * 60_000))
        .collect();
    lines.push((steady.as_str(), "127.0.0.1", T0 + 30_000));
    receive(app, &lines);
    assert!(
        of_rule(&drained(&mut events, "logs.signal"), FLAP_STORM).is_empty(),
        "four on one port and one on another are not five on one"
    );

    receive(app, &[(flapping.as_str(), "127.0.0.1", T0 + 240_000)]);
    let storms = of_rule(&drained(&mut events, "logs.signal"), FLAP_STORM);
    assert_eq!(storms.len(), 1, "{storms:#?}");
    assert_eq!(
        storms[0]["title"],
        json!("GigabitEthernet1/0/3 is flapping: it keeps going down")
    );
    assert_eq!(storms[0]["count"], json!(5));
    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(
        raised["dedup_key"],
        json!(format!("logs:{FLAP_STORM}:{id}:GigabitEthernet1/0/3"))
    );
}

#[tokio::test]
async fn routeros_has_its_own_burst_and_flap_rules() {
    let (lab, _monitor, _id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    let failure = "<131>system,error,critical login failure for user root from 10.0.0.1 via telnet";
    receive(app, &repeated(failure, 7, T0));
    assert!(
        of_rule(
            &drained(&mut events, "logs.signal"),
            "builtin.routeros.login_burst"
        )
        .is_empty()
    );
    receive(app, &[(failure, "127.0.0.1", T0 + 8_000)]);
    let bursts = of_rule(
        &drained(&mut events, "logs.signal"),
        "builtin.routeros.login_burst",
    );
    assert_eq!(bursts.len(), 1, "{bursts:#?}");
    assert_eq!(bursts[0]["instance"], json!("10.0.0.1"));

    let down = "<134>interface,info ether1 link down";
    receive(app, &repeated(down, 5, T0 + 20_000));
    let storms = of_rule(
        &drained(&mut events, "logs.signal"),
        "builtin.routeros.flap_storm",
    );
    assert_eq!(storms.len(), 1, "{storms:#?}");
    assert_eq!(
        storms[0]["title"],
        json!("ether1 is flapping: it keeps going down")
    );
}

#[tokio::test]
async fn the_rule_list_says_which_platform_each_rule_is_for() {
    let (lab, _monitor, _id) = one_switch().await;
    let rules = lab.app.rpc("logs.rules.list", Value::Null).await.unwrap();
    let rules = rules.as_array().unwrap();
    let platforms: std::collections::BTreeSet<&str> = rules
        .iter()
        .map(|rule| rule["platform"].as_str().unwrap())
        .collect();
    assert_eq!(
        platforms.into_iter().collect::<Vec<_>>(),
        [
            "Any SNMP agent (SNMPv2-MIB, IF-MIB)",
            "Cisco IOS / IOS-XE",
            "MikroTik RouterOS"
        ],
        "two platforms are covered, and the standard notifications of any agent; \
         the list names them and nothing is left unlabelled"
    );
    for rule in rules {
        assert_eq!(
            rule["stream"] == json!("trap"),
            rule["platform"] == json!("Any SNMP agent (SNMPv2-MIB, IF-MIB)"),
            "a rule that is no vendor's reads traps, and a vendor's reads its syslog: {rule:#}"
        );
    }
    let burst = rules
        .iter()
        .find(|rule| rule["id"] == json!(LOGIN_BURST))
        .expect("the security rules are installed");
    assert_eq!(
        burst["threshold"],
        json!({"count": 8, "window_s": 60, "group_by": ["source"]})
    );
    assert_eq!(burst["clear_after_s"], json!(600));
    assert!(
        burst["test_line_source"]
            .as_str()
            .unwrap()
            .starts_with("Synthetic: "),
        "a line built from a documented format says so"
    );
}

// ------------------------------------------------------- alerts that clear

#[tokio::test]
async fn a_neighbour_coming_back_clears_the_alert_its_going_down_raised() {
    let (lab, monitor, id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    let (down, up) = (bgp("192.0.2.1", "Down"), bgp("192.0.2.1", "Up"));
    receive(app, &[(down.as_str(), "127.0.0.1", T0)]);
    let raised = next_event(&mut events, "test.alert_raised").await;
    let key = format!("logs:{BGP}:{id}:192.0.2.1");
    assert_eq!(raised["dedup_key"], json!(key));
    assert_eq!(raised["title"], json!("BGP neighbour 192.0.2.1 is down"));

    // Another neighbour coming up is not this one coming back.
    let other_up = bgp("192.0.2.77", "Up");
    receive(app, &[(other_up.as_str(), "127.0.0.1", T0 + 100_000)]);
    let asked = next_event(&mut events, "test.alert_cleared").await;
    assert_eq!(asked["cleared"], json!(false));
    assert_eq!(monitor.open_keys(), vec![key.clone()]);
    assert!(
        drained(&mut events, "logs.signal").is_empty(),
        "an `up` that clears nothing says nothing"
    );

    // Two minutes later it is back.
    let entries = receive(app, &[(up.as_str(), "127.0.0.1", T0 + 120_000)]);
    let signals = drained(&mut events, "logs.signal");
    assert_eq!(signals.len(), 1, "the `up` is not an alert of its own");
    assert_eq!(signals[0]["kind"], json!("cleared"));
    assert_eq!(signals[0]["reason"], json!("opposite"));
    assert_eq!(signals[0]["instance"], json!("192.0.2.1"));
    assert_eq!(signals[0]["entry_id"], json!(entries[0].entry.id));
    let asked = next_event(&mut events, "test.alert_cleared").await;
    assert_eq!(
        asked["dedup_key"],
        json!(key),
        "cleared with the key it was raised with"
    );
    assert_eq!(asked["cleared"], json!(true));
    assert!(monitor.open_keys().is_empty());
}

#[tokio::test]
async fn a_neighbour_that_bounces_inside_a_minute_is_one_alert_cleared_once() {
    let (lab, monitor, id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    let (down, up) = (bgp("192.0.2.1", "Down"), bgp("192.0.2.1", "Up"));
    receive(
        app,
        &[
            (down.as_str(), "127.0.0.1", T0),
            (up.as_str(), "127.0.0.1", T0 + 5_000),
            (down.as_str(), "127.0.0.1", T0 + 10_000),
            (up.as_str(), "127.0.0.1", T0 + 15_000),
            (down.as_str(), "127.0.0.1", T0 + 20_000),
        ],
    );
    let signals = drained(&mut events, "logs.signal");
    assert_eq!(
        signals
            .iter()
            .map(|signal| signal["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["alert"],
        "three downs and two ups in twenty seconds are one alert and no clearing"
    );
    next_event(&mut events, "test.alert_raised").await;

    // It ended down: when the window is over the alert is still open.
    nocturne_logs::rules::sweep(app, now_ms() + 61_000);
    assert!(drained(&mut events, "logs.signal").is_empty());
    assert_eq!(
        monitor.open_keys(),
        [format!("logs:{BGP}:{id}:192.0.2.1")],
        "the last word was `down`"
    );

    // It comes up inside the window and stays up: cleared when the window
    // ends, once.
    let entries = receive(app, &[(up.as_str(), "127.0.0.1", T0 + 25_000)]);
    assert!(drained(&mut events, "logs.signal").is_empty());
    nocturne_logs::rules::sweep(app, now_ms() + 61_000);
    let cleared = next_event(&mut events, "logs.signal").await;
    assert_eq!(cleared["kind"], json!("cleared"));
    assert_eq!(cleared["reason"], json!("opposite"));
    assert_eq!(
        cleared["entry_id"],
        json!(entries[0].entry.id),
        "the line that said it was back"
    );
    next_event(&mut events, "test.alert_cleared").await;
    assert!(monitor.open_keys().is_empty());
    nocturne_logs::rules::sweep(app, now_ms() + 600_000);
    assert!(
        drained(&mut events, "test.alert_cleared").is_empty(),
        "and it is not cleared a second time"
    );
}

#[tokio::test]
async fn an_alert_from_before_a_restart_is_still_cleared_by_its_opposite() {
    let (lab, monitor, id) = one_switch().await;
    let app = &lab.app;
    // The monitor has it open; this process never raised it.
    let key = format!("logs:{BGP}:{id}:192.0.2.1");
    monitor.already_open(&key);
    let mut events = app.subscribe();

    let up = bgp("192.0.2.1", "Up");
    receive(app, &[(up.as_str(), "127.0.0.1", T0)]);
    let asked = next_event(&mut events, "test.alert_cleared").await;
    assert_eq!(asked["dedup_key"], json!(key));
    assert_eq!(asked["cleared"], json!(true));
    let cleared = drained(&mut events, "logs.signal");
    assert_eq!(
        cleared.len(),
        1,
        "the monitor had something to clear, so it is recorded"
    );
    assert_eq!(cleared[0]["kind"], json!("cleared"));

    // A second `up` a moment later is not a second call.
    receive(app, &[(up.as_str(), "127.0.0.1", T0 + 1_000)]);
    settle().await;
    assert!(drained(&mut events, "test.alert_cleared").is_empty());
    assert_eq!(monitor.clear_calls(), 1);
}

#[tokio::test]
async fn a_flood_of_made_up_neighbours_coming_up_is_not_a_flood_of_calls() {
    let (lab, monitor, _id) = one_switch().await;
    let app = &lab.app;

    // Five thousand neighbours nobody ever saw go down all "come back".
    let lines: Vec<String> = (0..5_000)
        .map(|index| bgp(&format!("198.51.{}.{}", index / 250, index % 250), "Up"))
        .collect();
    let flood: Vec<(&str, &str, i64)> = lines
        .iter()
        .map(|line| (line.as_str(), "127.0.0.1", T0))
        .collect();
    receive(app, &flood);

    eventually("the calls that are made", || monitor.clear_calls() >= 256).await;
    settle().await;
    assert_eq!(
        monitor.clear_calls(),
        256,
        "the monitor is asked for as many neighbours as one device can plausibly have lost"
    );
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(
        status["rules"]["held_back"],
        json!(4_744),
        "the rest are counted: {:#}",
        status["rules"]
    );
    assert!(
        status["rules"]["tracked"].as_u64().unwrap() <= 260,
        "and leave nothing behind: {:#}",
        status["rules"]
    );
    assert!(
        query(app, json!({"stream": "event"})).await.is_empty(),
        "an `up` that cleared nothing is not recorded as a clearing"
    );
}

#[tokio::test]
async fn a_neighbour_back_and_lost_again_at_once_leaves_its_alert_open() {
    let (lab, monitor, id) = one_switch().await;
    let app = &lab.app;
    // A monitor that takes its time over a clear: were the calls made side
    // by side, the raise that follows would overtake it.
    monitor
        .clear_delay_ms
        .store(80, std::sync::atomic::Ordering::SeqCst);

    let (down, up) = (bgp("192.0.2.1", "Down"), bgp("192.0.2.1", "Up"));
    receive(app, &[(down.as_str(), "127.0.0.1", T0)]);
    eventually("the alert to be raised", || monitor.raise_calls() == 1).await;

    // Two minutes later it comes back and is lost again fifty milliseconds
    // after: a clear, then a raise.
    receive(
        app,
        &[
            (up.as_str(), "127.0.0.1", T0 + 120_000),
            (down.as_str(), "127.0.0.1", T0 + 120_050),
        ],
    );
    eventually("both calls to be made", || {
        monitor.clear_calls() == 1 && monitor.raise_calls() == 2
    })
    .await;
    assert_eq!(
        monitor.open_keys(),
        [format!("logs:{BGP}:{id}:192.0.2.1")],
        "the last word was `down`, and the monitor heard it last"
    );
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["rules"]["open_alerts"], json!(1));
}

#[tokio::test]
async fn a_rule_that_is_switched_off_does_not_clear_its_alerts_by_falling_silent() {
    let (lab, monitor, id) = one_switch().await;
    let app = &lab.app;
    let violation = "<186>00:22:51: %PORT_SECURITY-2-PSECURE_VIOLATION: Security violation occurred, caused by MAC address 0011.8565.4B75 on port FastEthernet1/0/2.";
    receive(app, &[(violation, "127.0.0.1", T0)]);
    eventually("the alert to be raised", || monitor.raise_calls() == 1).await;
    let key = format!("logs:builtin.ios.port_security:{id}:FastEthernet1/0/2");
    let open_alerts = || async {
        app.rpc("logs.status", Value::Null).await.unwrap()["rules"]["open_alerts"].clone()
    };
    assert_eq!(open_alerts().await, json!(1));

    // A different throttle is the same rule watching the same thing.
    app.rpc(
        "logs.rules.save",
        json!({"id": "builtin.ios.port_security", "enabled": true, "throttle_s": 30}),
    )
    .await
    .unwrap();
    assert_eq!(open_alerts().await, json!(1));

    // Switched off, it sees no more violations — which is not the same as
    // there being none.
    app.rpc(
        "logs.rules.save",
        json!({"id": "builtin.ios.port_security", "enabled": false}),
    )
    .await
    .unwrap();
    assert_eq!(open_alerts().await, json!(0));
    let mut events = app.subscribe();
    nocturne_logs::rules::sweep(app, now_ms() + 31 * 60_000);
    settle().await;
    assert!(
        drained(&mut events, "logs.signal").is_empty(),
        "no quiet period ended: nobody was listening"
    );
    assert_eq!(monitor.clear_calls(), 0);
    assert_eq!(
        monitor.open_keys(),
        [key],
        "the alert stays as it is, for a person to close"
    );
}

#[tokio::test]
async fn what_a_person_changed_on_a_built_in_rule_survives_a_restart() {
    let (lab, _monitor, id) = one_switch().await;
    let app = &lab.app;
    let rule_of = |rules: Value| -> Value {
        rules
            .as_array()
            .unwrap()
            .iter()
            .find(|rule| rule["id"] == json!(FLAP_STORM))
            .cloned()
            .expect("the rule is installed")
    };
    let shipped = rule_of(app.rpc("logs.rules.list", Value::Null).await.unwrap());

    let saved = app
        .rpc(
            "logs.rules.save",
            json!({
                "id": FLAP_STORM,
                "enabled": false,
                "throttle_s": 900,
                "scope": {"device_ids": [id]},
            }),
        )
        .await
        .unwrap();
    assert_eq!(saved["pattern"], shipped["pattern"]);

    // What the application does every time it starts.
    nocturne_logs::rules::ensure_builtins(app).unwrap();
    let after = rule_of(app.rpc("logs.rules.list", Value::Null).await.unwrap());
    assert_eq!(after["enabled"], json!(false));
    assert_eq!(
        after["throttle_s"],
        json!(900),
        "a throttle somebody chose is theirs, not the default again"
    );
    assert_eq!(after["scope"]["device_ids"], json!([id]));
    assert_eq!(
        after["updated_at"], saved["updated_at"],
        "starting is not a change to the rule"
    );
    assert_eq!(after["threshold"], shipped["threshold"]);
}

#[tokio::test]
async fn an_alert_rule_without_a_title_is_refused_when_it_is_saved() {
    let (lab, _monitor, _id) = one_switch().await;
    let refused = lab
        .app
        .rpc(
            "logs.rules.save",
            json!({"name": "No title", "pattern": "x", "emit": {"kind": "alert", "severity": "low", "title": " "}}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid", "{refused}");
    assert!(refused.message.contains("needs a title"), "{refused}");
    // And nothing was stored for it.
    let rules = lab.app.rpc("logs.rules.list", Value::Null).await.unwrap();
    assert!(
        rules
            .as_array()
            .unwrap()
            .iter()
            .all(|rule| rule["name"] != json!("No title"))
    );
}

#[tokio::test]
async fn calls_the_monitor_cannot_keep_up_with_are_dropped_and_counted() {
    let (lab, monitor, _id) = one_switch().await;
    let app = &lab.app;
    // A monitor that has stopped answering.
    monitor
        .hold_raises
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Twenty minutes in which a different 256 ports are error-disabled
    // every minute: 5,120 alerts, each one allowed by the per-device cap.
    let lines: Vec<String> = (0..20)
        .flat_map(|minute| {
            (0..256).map(move |port| {
                format!(
                    "<188>50: *Nov 13 09:05:00.000: %PM-4-ERR_DISABLE: bpduguard error detected on Gi{minute}/0/{port}, putting Gi{minute}/0/{port} in err-disable state"
                )
            })
        })
        .collect();
    let storm: Vec<(&str, &str, i64)> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            (
                line.as_str(),
                "127.0.0.1",
                T0 + (index as i64 / 256) * 61_000,
            )
        })
        .collect();
    receive(app, &storm);

    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    let undelivered = status["rules"]["undelivered"].as_u64().unwrap();
    assert!(
        (1_023..=1_024).contains(&undelivered),
        "4,096 calls wait and the rest are dropped and counted, not queued for ever: {:#}",
        status["rules"]
    );
    assert_eq!(status["rules"]["held_back"], json!(0));
    assert_eq!(
        status["store"]["rows"],
        json!(5_120),
        "every line is stored"
    );

    // When the monitor answers again it gets what was waiting, and the
    // count of what it never got stays.
    monitor
        .hold_raises
        .store(false, std::sync::atomic::Ordering::SeqCst);
    eventually("the waiting calls to be made", || {
        monitor.raise_calls() + undelivered == 5_120
    })
    .await;
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["rules"]["undelivered"], json!(undelivered));
}

#[tokio::test]
async fn clearing_copes_with_a_build_that_has_no_monitor() {
    // Neither `alerts.raise` nor `alerts.clear` exists here.
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    set_address(app, &lab.id("SW1"), "127.0.0.1").await;
    let mut events = app.subscribe();

    let (down, up) = (bgp("192.0.2.1", "Down"), bgp("192.0.2.1", "Up"));
    receive(app, &[(down.as_str(), "127.0.0.1", T0)]);
    receive(app, &[(up.as_str(), "127.0.0.1", T0 + 120_000)]);
    let kinds: Vec<String> = drained(&mut events, "logs.signal")
        .iter()
        .map(|signal| signal["kind"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        ["alert", "cleared"],
        "the signals are this crate's own and need nobody else"
    );
    // The calls were made and answered "no such method": none is left
    // waiting for a monitor that will never come, and none was dropped.
    settle().await;
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["rules"]["undelivered"], json!(0));
}

#[tokio::test]
async fn an_ospf_adjacency_is_raised_when_it_is_lost_and_cleared_when_it_is_full_again() {
    let (lab, monitor, id) = one_switch().await;
    let app = &lab.app;
    let mut events = app.subscribe();

    // The lines the simulator's own routers write (nocturne-netsim, ospf).
    let lost = "<189>*Mar  1 00:10:00.000: %OSPF-5-ADJCHG: Process 1, Nbr 2.2.2.2 on GigabitEthernet0/0 from FULL to DOWN, Neighbor Down: Dead timer expired";
    let back = "<189>*Mar  1 00:13:00.000: %OSPF-5-ADJCHG: Process 1, Nbr 2.2.2.2 on GigabitEthernet0/0 from LOADING to FULL, Loading Done";

    // Coming up for the first time is not an alert.
    receive(app, &[(back, "127.0.0.1", T0)]);
    next_event(&mut events, "test.alert_cleared").await;
    assert!(drained(&mut events, "logs.signal").is_empty());

    receive(app, &[(lost, "127.0.0.1", T0 + 600_000)]);
    let raised = next_event(&mut events, "test.alert_raised").await;
    let key = format!("logs:builtin.ios.ospf_adjacency:{id}:2.2.2.2 GigabitEthernet0/0");
    assert_eq!(raised["dedup_key"], json!(key));
    assert_eq!(
        raised["title"],
        json!("OSPF neighbour 2.2.2.2 on GigabitEthernet0/0 went FULL → DOWN")
    );

    receive(app, &[(back, "127.0.0.1", T0 + 780_000)]);
    let asked = next_event(&mut events, "test.alert_cleared").await;
    assert_eq!(asked["dedup_key"], json!(key));
    assert!(monitor.open_keys().is_empty());
}
