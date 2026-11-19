//! Service checks: the five states, the history, the samples, and the
//! alerting that goes through the rules engine like everything else.
//!
//! The checks here are driven against servers the test starts on loopback
//! (`listeners`), and — for ICMP and SNMP — through the same fakes the device
//! poller's tests use, because those are the two paths where "the probe could
//! not be attempted" is a thing a test needs to be able to cause on purpose.

use std::time::Duration;

use serde_json::json;

use crate::checks::model::CheckStatus;
use crate::model::Severity;
use crate::testing::{AgentData, Harness, HostState};
use crate::tests::listeners;

/// A device that answers ping, so that a failing check cannot be blamed on
/// the box being unreachable.
async fn harness_with_device() -> (Harness, String) {
    let harness = Harness::new().await;
    let id = harness.device("SW1", "127.0.0.1", json!({})).await;
    harness.probe.up("127.0.0.1");
    (harness, id)
}

// ------------------------------------------------------- the five states

#[tokio::test]
async fn a_check_that_has_never_run_knows_nothing_rather_than_assuming_the_best() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({}),
        )
        .await;
    let state = harness.check_state(&check);
    assert_eq!(state.status, CheckStatus::Unknown);
    assert_eq!(state.last_run_at, None);
    assert_eq!(state.since, None);
    assert!(
        harness.samples(&device, "check.up", &check).is_empty(),
        "nothing has been measured, so there is nothing in the series"
    );
}

#[tokio::test]
async fn a_service_that_stops_answering_goes_down_while_the_box_stays_up() {
    // The sentence the whole feature exists for: "the web server on this
    // host is down but the host answers ping".
    let (harness, device) = harness_with_device().await;
    let server = listeners::tcp_server().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": server.port}),
            json!({}),
        )
        .await;

    let up = harness.run_check(&check).await;
    assert_eq!(up.state.status, CheckStatus::Up);
    assert_eq!(up.previous, CheckStatus::Unknown);
    assert!(up.changed);
    assert!(
        up.state
            .rtt_ms
            .is_some_and(|rtt| rtt.is_finite() && rtt >= 0.0),
        "a connection that was made has a response time: {:?}",
        up.state.rtt_ms
    );

    // Take the service away, and wait until it really is gone before asking
    // again: the point of the assertion is what the check says about a
    // service that is not there, not how quickly a socket closes.
    server.stop().await;
    let down = harness.run_check(&check).await;
    assert_eq!(down.state.status, CheckStatus::Down, "{:?}", down.state);
    // Unix refuses a connection to a port with nothing on it; Windows does
    // not always answer at all — on a CI runner a closed loopback port is
    // dropped rather than rejected, and the check times out. Both are the
    // same fact about the service, and the status above is the assertion
    // that matters. The code only says how the host put it.
    assert!(
        matches!(down.state.code.as_str(), "refused" | "timeout"),
        "a service that has gone is refused or times out: {:?}",
        down.state
    );
    let said = &down.state.message;
    assert!(
        said.contains("nothing is listening") || said.contains("did not answer"),
        "the message has to say which it was: {said}"
    );

    // And the device itself is untouched by any of it.
    let poll = harness.poll(&device).await;
    assert!(poll.ok && !poll.down, "the box still answers ping");
    let view = harness
        .rpc("monitor.device", json!({"device_id": device}))
        .await;
    assert_eq!(view["down"], false);
    assert_eq!(view["checks"][0]["state"]["status"], "down");
    assert_eq!(view["checks"][0]["name"], "web");
}

#[tokio::test]
async fn a_check_that_could_not_run_is_not_an_outage_and_writes_no_sample() {
    // The mistake this is here to prevent: a container that may not send
    // ICMP reporting every service in the building as down.
    let (harness, device) = harness_with_device().await;
    harness.probe.set(
        "127.0.0.1",
        HostState {
            error: Some("ICMP sockets are not available to this process".into()),
            ..HostState::default()
        },
    );
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;

    let recorded = harness.run_check(&check).await;
    assert_eq!(recorded.state.status, CheckStatus::CannotRun);
    assert_ne!(recorded.state.status, CheckStatus::Down);
    assert_eq!(recorded.state.code, "probe_unavailable");
    assert!(
        recorded.state.message.contains("ICMP sockets"),
        "the reason has to survive: {}",
        recorded.state.message
    );
    assert_eq!(recorded.state.rtt_ms, None);
    assert!(
        harness.samples(&device, "check.up", &check).is_empty(),
        "a gap: `check.up` = 0 here would be an invented outage"
    );

    // And the service-down rule has not fired, because nothing was found.
    assert!(
        harness
            .open_alerts()
            .iter()
            .all(|alert| alert.rule_id.as_deref() != Some("builtin.service_check_down")),
        "{:?}",
        harness.open_alerts()
    );
}

#[tokio::test]
async fn a_check_that_goes_blind_does_not_resolve_the_outage_it_can_no_longer_see() {
    // A service is down; then the check loses the ability to look. The alert
    // must stay: "we cannot tell any more" is not "it recovered".
    let (harness, device) = harness_with_device().await;
    harness.only_rule("builtin.service_check_down").await;
    harness.probe.down("127.0.0.1");
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;

    harness.run_check(&check).await;
    assert_eq!(harness.check_state(&check).status, CheckStatus::Down);
    let raised = harness.open_alerts();
    assert_eq!(raised.len(), 1, "{raised:?}");
    assert_eq!(raised[0].instance.as_deref(), Some("ping"));
    let alert_id = raised[0].id.clone();

    harness.probe.set(
        "127.0.0.1",
        HostState {
            error: Some("the host name could not be resolved".into()),
            ..HostState::default()
        },
    );
    harness.run_check(&check).await;
    assert_eq!(harness.check_state(&check).status, CheckStatus::CannotRun);
    let still = harness.open_alerts();
    assert_eq!(
        still
            .iter()
            .map(|alert| alert.id.as_str())
            .collect::<Vec<_>>(),
        [alert_id.as_str()],
        "the outage alert is still open, because nothing has said otherwise"
    );

    // Only a run that actually finds the service healthy resolves it.
    harness.probe.up("127.0.0.1");
    harness.run_check(&check).await;
    assert_eq!(harness.check_state(&check).status, CheckStatus::Up);
    assert!(harness.open_alerts().is_empty());
}

#[tokio::test]
async fn a_simulated_device_is_never_probed_on_an_address_that_is_not_its_own() {
    let harness = Harness::new().await;
    // A simulated device whose `address` field holds somebody else's
    // address, which is the hazard: 10.9.9.9 is a real host on a real
    // network, and it is not this one.
    let lab = harness
        .device(
            "LAB1",
            "10.9.9.9",
            json!({"lab": {"lab_id": "testlab", "node": "LAB1"}}),
        )
        .await;
    let check = harness
        .check_on(&lab, "web", json!({"kind": "tcp", "port": 80}), json!({}))
        .await;
    let recorded = harness.run_check(&check).await;
    assert_eq!(recorded.state.status, CheckStatus::CannotRun);
    assert_eq!(recorded.state.code, "sim");
    assert!(
        recorded.state.message.contains("simulated"),
        "{}",
        recorded.state.message
    );
    assert!(
        harness.probe.pinged.lock().is_empty(),
        "nothing was sent at the address a simulated device happens to carry"
    );
}

#[tokio::test]
async fn an_snmp_check_with_no_credential_says_so_instead_of_blaming_the_agent() {
    let harness = Harness::new().await;
    let device = harness
        .device("SW1", "127.0.0.1", json!({"monitor": {"snmp": false}}))
        .await;
    let check = harness
        .check_on(&device, "agent", json!({"kind": "snmp"}), json!({}))
        .await;
    let recorded = harness.run_check(&check).await;
    assert_eq!(recorded.state.status, CheckStatus::CannotRun);
    assert_eq!(recorded.state.code, "no_credential");
    assert!(
        recorded.state.message.contains("credential"),
        "{}",
        recorded.state.message
    );
}

#[tokio::test]
async fn an_snmp_check_reads_one_object_over_the_devices_own_session() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("nocturne-test").await;
    let device = harness
        .snmp_device(
            "SW1",
            "127.0.0.1",
            AgentData {
                uptime_ticks: Some(1_234_500),
                ..AgentData::default()
            },
        )
        .await;
    let check = harness
        .check_on(&device, "agent", json!({"kind": "snmp"}), json!({}))
        .await;
    let recorded = harness.run_check(&check).await;
    assert_eq!(
        recorded.state.status,
        CheckStatus::Up,
        "{:?}",
        recorded.state
    );
    assert!(
        recorded.state.facts.snmp_value.is_some(),
        "the value it read is what proves it answered"
    );

    // An object the agent does not implement is the check's fault, not an
    // outage: the agent answered.
    let missing = harness
        .check_on(
            &device,
            "cpu",
            json!({"kind": "snmp", "oid": "1.3.6.1.4.1.9.9.109.1.1.1.1.6.1"}),
            json!({}),
        )
        .await;
    let recorded = harness.run_check(&missing).await;
    assert_eq!(recorded.state.status, CheckStatus::CannotRun);
    assert_eq!(recorded.state.code, "snmp_no_such_object");
}

// ------------------------------------------------------------- the history

#[tokio::test]
async fn the_history_says_when_it_went_down_and_for_how_long() {
    let (harness, device) = harness_with_device().await;
    harness.silence_rules().await;
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;

    let start = harness.now();
    harness.run_check(&check).await; // up
    harness.probe.down("127.0.0.1");
    harness.run_check(&check).await; // down
    harness.run_check(&check).await; // still down: not a second entry
    harness.probe.up("127.0.0.1");
    harness.run_check(&check).await; // up again

    let history = harness
        .rpc(
            "checks.history",
            json!({"id": check, "since": start - 1_000}),
        )
        .await;
    let events = history["events"].as_array().expect("events").clone();
    assert_eq!(
        events
            .iter()
            .map(|event| (
                event["previous"].as_str().unwrap(),
                event["status"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        [("unknown", "up"), ("up", "down"), ("down", "up")],
        "one entry per change, not one per run"
    );
    assert_eq!(history["summary"]["down_count"], 1);
    assert!(
        history["summary"]["last_down_at"].as_i64().is_some(),
        "the outage is dated"
    );
    assert!(
        history["summary"]["last_down_for_s"].as_i64().is_some(),
        "and has a length"
    );
    // `held_s` says how long the status before each change had lasted.
    assert!(events[1]["held_s"].as_i64().is_some());
    assert_eq!(history["current"]["status"], "up");
}

#[tokio::test]
async fn availability_leaves_out_the_time_that_could_not_be_measured() {
    // Hand-built changes, so the arithmetic is exact and owes nothing to how
    // fast this machine ran the test.
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({}),
        )
        .await;
    let (stored, _) = crate::checks::store::require(&harness.app, &check).unwrap();
    let minute = 60_000;
    let base = harness.now() - 10 * minute;
    let mut at = base;
    for (status, code) in [
        (CheckStatus::Up, "ok"),
        (CheckStatus::Down, "timeout"),
        (CheckStatus::CannotRun, "dns_resolve"),
    ] {
        let outcome = crate::checks::outcome::Outcome {
            status,
            code: code.into(),
            message: String::new(),
            rtt_ms: None,
            facts: Default::default(),
        };
        crate::checks::store::record(&harness.app, &stored, "SW1", &outcome, at).unwrap();
        at += 2 * minute;
    }
    let until = base + 6 * minute;
    let events = crate::checks::store::events(&harness.app, &check, base, until, 100).unwrap();
    let summary =
        crate::checks::store::summarise(&harness.app, &check, base, until, &events).unwrap();
    assert_eq!(summary.up_s, 120, "two minutes up before it failed");
    assert_eq!(summary.down_s, 120);
    assert_eq!(summary.cannot_run_s, 120);
    assert_eq!(
        summary.measured_s, 240,
        "the blind two minutes are not in it"
    );
    assert_eq!(
        summary.availability_pct,
        Some(50.0),
        "two minutes up out of four measured, not out of six"
    );
}

#[tokio::test]
async fn a_window_in_which_nothing_was_measured_has_no_availability_rather_than_a_hundred_percent()
{
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({}),
        )
        .await;
    let now = harness.now();
    let history = harness
        .rpc(
            "checks.history",
            json!({"id": check, "since": now - 3_600_000, "until": now}),
        )
        .await;
    assert_eq!(history["summary"]["availability_pct"], json!(null));
    assert_eq!(history["summary"]["measured_s"], 0);
    assert!(history["summary"]["unknown_s"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn response_times_go_into_the_series_every_other_metric_uses() {
    let (harness, device) = harness_with_device().await;
    let server = listeners::tcp_server().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": server.port}),
            json!({}),
        )
        .await;
    harness.run_check(&check).await;

    let up = harness.samples(&device, "check.up", &check);
    assert_eq!(up.len(), 1);
    assert_eq!(up[0].1, 1.0);
    assert_eq!(harness.samples(&device, "check.rtt_ms", &check).len(), 1);

    // And it comes back through `metrics.query`, keyed by the check's id.
    let series = harness
        .rpc(
            "metrics.query",
            json!({"device_id": device, "metric": "check.rtt_ms", "instance": check,
                   "since": harness.now() - 60_000}),
        )
        .await;
    assert_eq!(series["points"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn renaming_a_check_keeps_its_history_and_its_series() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;
    harness
        .rpc("checks.save", json!({"id": check, "name": "echo"}))
        .await;
    let state = harness.check_state(&check);
    assert_eq!(
        state.status,
        CheckStatus::Up,
        "a rename is not a change of what is being watched"
    );
    assert_eq!(
        harness.samples(&device, "check.up", &check).len(),
        1,
        "the series is keyed by id, so it does not split"
    );
}

#[tokio::test]
async fn pointing_a_check_at_something_else_throws_away_what_it_found_before() {
    let (harness, device) = harness_with_device().await;
    let server = listeners::tcp_server().await;
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": server.port}),
            json!({}),
        )
        .await;
    harness.run_check(&check).await;
    assert_eq!(harness.check_state(&check).status, CheckStatus::Up);

    let moved = harness
        .rpc(
            "checks.save",
            json!({"id": check, "spec": {"kind": "tcp", "port": listeners::closed_port().await}}),
        )
        .await;
    assert_eq!(
        moved["state"]["status"], "unknown",
        "the old verdict was about a different service"
    );
}

// ---------------------------------------------------------------- alerting

#[tokio::test]
async fn a_service_that_is_down_raises_one_alert_per_check_and_resolves_itself() {
    let (harness, device) = harness_with_device().await;
    harness.only_rule("builtin.service_check_down").await;
    harness.probe.down("127.0.0.1");
    let ping = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    let web = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": listeners::closed_port().await}),
            json!({}),
        )
        .await;

    harness.run_check(&ping).await;
    harness.run_check(&web).await;
    let open = harness.open_alerts();
    let mut names: Vec<&str> = open
        .iter()
        .filter_map(|alert| alert.instance.as_deref())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["ping", "web"],
        "two services failing on one device are two alerts, not one"
    );
    assert!(open.iter().all(|alert| alert.severity == Severity::High));
    assert!(
        open.iter()
            .any(|alert| alert.title.contains("ping") && alert.title.contains("SW1")),
        "{open:?}"
    );

    // One of them comes back; only its alert resolves.
    harness.probe.up("127.0.0.1");
    harness.run_check(&ping).await;
    let open = harness.open_alerts();
    assert_eq!(
        open.iter()
            .filter_map(|alert| alert.instance.as_deref())
            .collect::<Vec<_>>(),
        ["web"]
    );
    // And the resolved one is kept, so "down three times this week" is
    // answerable.
    assert!(
        harness
            .all_alerts()
            .iter()
            .any(|alert| alert.instance.as_deref() == Some("ping")
                && alert.state == crate::model::AlertState::Resolved)
    );
}

#[tokio::test]
async fn a_rule_can_name_one_check_and_leave_the_others_alone() {
    let (harness, device) = harness_with_device().await;
    harness.silence_rules().await;
    harness
        .rpc(
            "alerts.rules.save",
            json!({"name": "The resolver matters", "severity": "critical",
                   "condition": {"type": "check_state", "check": "resolver", "state": "down"}}),
        )
        .await;
    harness.probe.down("127.0.0.1");
    let ping = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    let resolver = harness
        .check_on(&device, "resolver", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&ping).await;
    harness.run_check(&resolver).await;

    let open = harness.open_alerts();
    assert_eq!(
        open.iter()
            .filter_map(|alert| alert.instance.as_deref())
            .collect::<Vec<_>>(),
        ["resolver"],
        "the rule named one check: {open:?}"
    );
    assert_eq!(open[0].severity, Severity::Critical);
}

#[tokio::test]
async fn a_check_that_cannot_run_raises_a_different_alert_from_a_service_that_is_down() {
    let (harness, device) = harness_with_device().await;
    harness.only_rule("builtin.service_check_cannot_run").await;
    // The built-in rule waits ten minutes before it fires, so that a
    // credential being saved does not wake anybody; the damping itself is
    // the subject of its own test, so here it is turned down to nothing.
    harness
        .rpc(
            "alerts.rules.save",
            json!({"id": "builtin.service_check_cannot_run", "enabled": true, "for_s": 0}),
        )
        .await;
    harness.probe.set(
        "127.0.0.1",
        HostState {
            error: Some("this machine may not send ICMP".into()),
            ..HostState::default()
        },
    );
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;

    let open = harness.open_alerts();
    assert_eq!(open.len(), 1, "{open:?}");
    assert_eq!(
        open[0].rule_id.as_deref(),
        Some("builtin.service_check_cannot_run")
    );
    assert_eq!(open[0].severity, Severity::Medium);
    assert!(
        open[0].detail.contains("cannot_run"),
        "the alert says which state it is about: {}",
        open[0].detail
    );
}

#[tokio::test(start_paused = true)]
async fn a_blind_spot_has_to_last_before_anybody_is_woken_for_it() {
    // The existing `for_s` damping, applied to a check: this is the test
    // that the rules engine is doing the work rather than a second path.
    let harness = Harness::new().await;
    let device = harness.device("SW1", "127.0.0.1", json!({})).await;
    harness.only_rule("builtin.service_check_cannot_run").await;
    harness.probe.set(
        "127.0.0.1",
        HostState {
            error: Some("this machine may not send ICMP".into()),
            ..HostState::default()
        },
    );
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;

    harness.run_check(&check).await;
    assert!(
        harness.open_alerts().is_empty(),
        "ten minutes have not passed"
    );
    tokio::time::sleep(Duration::from_secs(9 * 60)).await;
    harness.run_check(&check).await;
    assert!(harness.open_alerts().is_empty(), "nor now");
    tokio::time::sleep(Duration::from_secs(2 * 60)).await;
    harness.run_check(&check).await;
    assert_eq!(harness.open_alerts().len(), 1, "but now");
}

#[tokio::test]
async fn a_maintenance_window_mutes_a_service_check_alert_without_hiding_it() {
    let (harness, device) = harness_with_device().await;
    harness.only_rule("builtin.service_check_down").await;
    let now = harness.now();
    harness
        .rpc(
            "maintenance.save",
            json!({"name": "Rebuilding the web server", "starts_at": now - 1_000,
                   "ends_at": now + 3_600_000}),
        )
        .await;
    harness.probe.down("127.0.0.1");
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;

    let open = harness.open_alerts();
    assert_eq!(open.len(), 1, "it is still recorded");
    assert!(
        open[0].muted_by_maintenance,
        "but nobody is woken up for it"
    );
}

#[tokio::test]
async fn a_box_that_is_down_does_not_also_complain_about_its_web_server() {
    // One cause, one alert — the rule the device poller already follows,
    // applied to checks without a second mechanism.
    let (harness, device) = harness_with_device().await;
    harness.only_rule("builtin.service_check_down").await;
    harness.probe.down("127.0.0.1");
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;

    // Two failed polls make the device down, which is what suppresses the
    // rest.
    harness.poll_and_evaluate(&device).await;
    harness.poll_and_evaluate(&device).await;
    assert!(harness.poll(&device).await.down);

    harness.run_check(&check).await;
    assert_eq!(
        harness.check_state(&check).status,
        CheckStatus::Down,
        "the check still records what it found"
    );
    assert!(
        harness.open_alerts().is_empty(),
        "but the alert about it is suppressed while the box itself is down: {:?}",
        harness.open_alerts()
    );
}

#[tokio::test]
async fn deleting_a_check_closes_the_alert_it_had_raised() {
    let (harness, device) = harness_with_device().await;
    harness.only_rule("builtin.service_check_down").await;
    harness.probe.down("127.0.0.1");
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;
    assert_eq!(harness.open_alerts().len(), 1);

    harness.rpc("checks.delete", json!({"id": check})).await;
    assert!(
        harness.open_alerts().is_empty(),
        "an alert about a check that no longer exists is not an open problem"
    );
}

// -------------------------------------------------------------- scheduling

#[tokio::test(start_paused = true)]
async fn a_short_check_and_a_long_one_on_one_device_do_not_share_a_cadence() {
    let harness = Harness::new().await;
    let device = harness
        .device("SW1", "127.0.0.1", json!({"monitor": {"interval_s": 3600}}))
        .await;
    harness.silence_rules().await;
    let quick = harness
        .check_on(
            &device,
            "quick",
            json!({"kind": "icmp"}),
            json!({"interval_s": 10, "timeout_ms": 1_000}),
        )
        .await;
    let slow = harness
        .check_on(
            &device,
            "slow",
            json!({"kind": "icmp"}),
            json!({"interval_s": 120, "timeout_ms": 1_000}),
        )
        .await;
    harness.check_due_now(&quick, 10);
    harness.check_due_now(&slow, 120);
    crate::scheduler::spawn(&harness.app);

    tokio::time::sleep(Duration::from_secs(61)).await;
    let runs = |id: &str| harness.samples(&device, "check.up", id);
    let quick_runs = runs(&quick);
    let slow_runs = runs(&slow);

    // No count is asserted here, and a range did not make one safe — it
    // failed about one run in five. Under a paused clock a probe's real
    // socket round trip costs *virtual* time: the runtime has nothing left
    // to run while it waits, so tokio winds the clock on to the scheduler's
    // next tick, again and again until the real reply lands. Measured, that
    // came to 18 virtual seconds a run, the same whether the probe was
    // answered or refused outright — so the quick check's spacing here is
    // set by how long its probe holds the slot, not by its interval, and
    // how many runs fall inside a fixed window turns on where the last one
    // lands. The interval rule itself is pinned exactly, with no socket in
    // it, by `scheduler::tests::a_check_is_due_on_its_own_interval_and_on_no_other`.
    //
    // What is left for this test is what that one cannot see: two checks
    // with different intervals, on one device, on the one shared scheduler,
    // keeping separate cadences — and the floor holding even when a probe
    // overruns its own interval.
    let gaps: Vec<i64> = quick_runs
        .windows(2)
        .map(|pair| pair[1].0 - pair[0].0)
        .collect();
    for gap in &gaps {
        assert!(
            *gap >= 10_000,
            "two runs of a ten-second check {gap}ms apart, in {gaps:?}"
        );
    }
    assert!(
        quick_runs.len() > slow_runs.len(),
        "a ten-second check should run more often than a two-minute one on \
         the same device, itself polled hourly: {} against {}",
        quick_runs.len(),
        slow_runs.len()
    );
    assert_eq!(
        slow_runs.len(),
        1,
        "a two-minute check runs once in a minute"
    );
}

#[tokio::test(start_paused = true)]
async fn a_thousand_checks_do_not_all_run_on_the_first_tick() {
    // The same staggering the device poller gets, for the same reason: a
    // cold start must not be a thundering herd.
    let interval_ms = 60_000;
    let mut slots = std::collections::HashSet::new();
    for index in 0..1_000 {
        slots.insert(crate::scheduler::jitter(&format!("check{index}"), interval_ms) / 1_000);
    }
    assert!(
        slots.len() > 50,
        "only {} of the interval's seconds are used",
        slots.len()
    );
}

#[tokio::test(start_paused = true)]
async fn a_device_whose_monitoring_is_switched_off_has_its_services_left_alone() {
    let harness = Harness::new().await;
    let device = harness.device("SW1", "127.0.0.1", json!({})).await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 5, "timeout_ms": 1_000}),
        )
        .await;
    harness
        .rpc(
            "devices.update",
            json!({"id": device, "monitor": {"enabled": false, "icmp": true}}),
        )
        .await;
    harness.check_due_now(&check, 5);
    crate::scheduler::spawn(&harness.app);
    tokio::time::sleep(Duration::from_secs(20)).await;

    assert_eq!(harness.check_state(&check).status, CheckStatus::Unknown);
    assert!(harness.probe.pinged.lock().is_empty());
}

#[tokio::test(start_paused = true)]
async fn switching_the_checks_off_stops_them_without_touching_the_device_poller() {
    let harness = Harness::new().await;
    let device = harness.device("SW1", "127.0.0.1", json!({})).await;
    harness.silence_rules().await;
    harness
        .rpc(
            "settings.set",
            json!({"section": "monitoring", "value": {"checks_enabled": false}}),
        )
        .await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp"}),
            json!({"interval_s": 5, "timeout_ms": 1_000}),
        )
        .await;
    harness.check_due_now(&check, 5);
    harness.due_now(&device, 60);
    crate::scheduler::spawn(&harness.app);
    tokio::time::sleep(Duration::from_secs(20)).await;

    assert_eq!(harness.check_state(&check).status, CheckStatus::Unknown);
    assert!(
        !harness.samples(&device, "up", "").is_empty(),
        "the device poller is still running"
    );
}

// --------------------------------------------------------------------- RPC

#[tokio::test]
async fn a_new_device_acquires_no_checks_of_its_own() {
    let harness = Harness::new().await;
    let device = harness.device("SW1", "127.0.0.1", json!({})).await;
    let listed = harness
        .rpc("checks.list", json!({"device_id": device}))
        .await;
    assert_eq!(listed, json!([]), "nothing is created for anybody");
    let status = harness.rpc("monitor.status", json!({})).await;
    assert_eq!(status["checks"]["total"], 0);
}

#[tokio::test]
async fn two_checks_on_one_device_cannot_share_a_name() {
    let (harness, device) = harness_with_device().await;
    harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({}),
        )
        .await;
    let clash = harness
        .app
        .rpc(
            "checks.save",
            json!({"device_id": device, "name": "WEB", "spec": {"kind": "tcp", "port": 8080}}),
        )
        .await
        .unwrap_err();
    assert_eq!(clash.code, "conflict");
    assert!(clash.message.contains("web") || clash.message.contains("WEB"));

    // The same name on another device is fine: a check belongs to its host.
    let other = harness.device("SW2", "127.0.0.2", json!({})).await;
    harness
        .check_on(&other, "web", json!({"kind": "tcp", "port": 80}), json!({}))
        .await;
}

#[tokio::test]
async fn a_check_is_refused_rather_than_stored_in_a_state_it_could_never_run_in() {
    let (harness, device) = harness_with_device().await;
    let refuse = |params: serde_json::Value| {
        let app = harness.app.clone();
        async move { app.rpc("checks.save", params).await.unwrap_err() }
    };
    assert_eq!(
        refuse(json!({"device_id": device, "name": "web",
                      "spec": {"kind": "tcp", "port": 0}}))
        .await
        .code,
        "invalid"
    );
    assert_eq!(
        refuse(json!({"device_id": device, "name": "web",
                      "spec": {"kind": "http"}, "host": "http://example.com/health"}))
        .await
        .code,
        "invalid",
        "a URL is not a host"
    );
    assert_eq!(
        refuse(json!({"device_id": device, "name": "dns",
                      "spec": {"kind": "dns", "name": "www.example.com", "record": "ZZZ"}}))
        .await
        .code,
        "invalid"
    );
    assert_eq!(
        refuse(
            json!({"device_id": device, "name": "web", "spec": {"kind": "tcp", "port": 80},
                      "interval_s": 5, "timeout_ms": 30_000})
        )
        .await
        .code,
        "invalid",
        "a check cannot be given longer to answer than its own interval"
    );
    assert_eq!(
        refuse(json!({"device_id": device, "name": "web"}))
            .await
            .code,
        "invalid",
        "a check has to say what it checks"
    );
}

#[tokio::test]
async fn run_now_answers_with_what_it_found_rather_than_starting_a_job() {
    let (harness, device) = harness_with_device().await;
    let server = listeners::tcp_server().await;
    harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": server.port}),
            json!({}),
        )
        .await;
    harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;

    let answer = harness
        .rpc("checks.run_now", json!({"device_id": device}))
        .await;
    assert_eq!(answer["ran"], 2);
    let statuses: Vec<(&str, &str)> = answer["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| {
            (
                check["name"].as_str().unwrap(),
                check["status"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(statuses, [("ping", "up"), ("web", "up")]);
}

#[tokio::test]
async fn the_status_screen_counts_what_cannot_run_apart_from_what_is_down() {
    let (harness, device) = harness_with_device().await;
    harness.silence_rules().await;
    let down = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": listeners::closed_port().await}),
            json!({}),
        )
        .await;
    let blind = harness
        .check_on(&device, "agent", json!({"kind": "snmp"}), json!({}))
        .await;
    harness.run_check(&down).await;
    harness.run_check(&blind).await;

    let status = harness.rpc("monitor.status", json!({})).await;
    assert_eq!(status["checks"]["down"], 1);
    assert_eq!(status["checks"]["cannot_run"], 1);
    assert_eq!(status["checks"]["up"], 0);
    let kinds: Vec<&str> = status["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|problem| problem["kind"].as_str())
        .collect();
    assert!(kinds.contains(&"checks"), "{kinds:?}");
    assert!(
        kinds.contains(&"checks_blind"),
        "a blind spot is its own problem, not one of the failures: {kinds:?}"
    );
}

#[tokio::test]
async fn checks_can_be_listed_by_what_they_are_saying() {
    let (harness, device) = harness_with_device().await;
    harness.silence_rules().await;
    let server = listeners::tcp_server().await;
    let up = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": server.port}),
            json!({}),
        )
        .await;
    let blind = harness
        .check_on(&device, "agent", json!({"kind": "snmp"}), json!({}))
        .await;
    let never = harness
        .check_on(
            &device,
            "mail",
            json!({"kind": "tcp", "port": 25}),
            json!({}),
        )
        .await;
    harness.run_check(&up).await;
    harness.run_check(&blind).await;

    let names = |rows: &serde_json::Value| {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|row| row["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let all = harness.rpc("checks.list", json!({})).await;
    assert_eq!(names(&all), ["agent", "mail", "web"]);
    assert_eq!(
        names(&harness.rpc("checks.list", json!({"status": "up"})).await),
        ["web"]
    );
    assert_eq!(
        names(
            &harness
                .rpc("checks.list", json!({"status": "cannot_run"}))
                .await
        ),
        ["agent"]
    );
    assert_eq!(
        names(
            &harness
                .rpc("checks.list", json!({"status": "problem"}))
                .await
        ),
        ["agent"],
        "a check that has never run is not a problem, it is a check that has never run"
    );
    assert_eq!(
        names(&harness.rpc("checks.list", json!({"kind": "snmp"})).await),
        ["agent"]
    );
    assert_eq!(
        names(&harness.rpc("checks.list", json!({"filter": "ai"})).await),
        ["mail"]
    );

    // `never` is still unknown, and asking for a state that does not exist
    // is refused rather than quietly answered with nothing.
    assert_eq!(harness.check_state(&never).status, CheckStatus::Unknown);
    let error = harness
        .app
        .rpc("checks.list", json!({"status": "flapping"}))
        .await
        .unwrap_err();
    assert_eq!(error.code, "invalid");
}

#[tokio::test]
async fn deleting_a_device_takes_its_checks_with_it() {
    let (harness, device) = harness_with_device().await;
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;
    crate::scheduler::spawn(&harness.app);
    harness
        .rpc("devices.delete", json!({"ids": [device]}))
        .await;

    // The checks go when the feature hears that the device was deleted,
    // which happens on the event bus: waited for with a deadline, not slept
    // through for a fixed time.
    while crate::checks::store::get(&harness.app, &check)
        .unwrap()
        .is_some()
    {
        assert!(
            std::time::Instant::now() < listeners::soon(),
            "the device's checks were never forgotten"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        harness.rpc("monitor.status", json!({})).await["checks"]["total"],
        0
    );
}

#[tokio::test]
async fn a_check_run_is_announced_so_a_screen_can_follow_it() {
    let (harness, device) = harness_with_device().await;
    harness.silence_rules().await;
    let mut events = harness.app.subscribe();
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;

    let mut ran = None;
    while let Ok(event) = events.try_recv() {
        if event.topic == "checks.ran" {
            ran = Some(event.payload);
        }
    }
    let ran = ran.expect("a checks.ran event");
    assert_eq!(ran["check_id"], json!(check));
    assert_eq!(ran["status"], "up");
    assert_eq!(ran["previous"], "unknown");
    assert_eq!(ran["changed"], true);
    assert_eq!(ran["kind"], "icmp");
}
