//! What a check's definition may say, and what happens to what it found when
//! the definition changes under it: a limit that means what it reads as, a
//! host that is used or refused, a run that outlives the definition it was
//! started from, a check that is moved.

use std::time::Duration;

use serde_json::{Value, json};

use crate::checks::model::{CheckStatus, ServiceCheck};
use crate::testing::{AgentData, Harness, HostState};

fn titles(alerts: &[crate::model::Alert]) -> Vec<&str> {
    alerts.iter().map(|alert| alert.title.as_str()).collect()
}

async fn refused(harness: &Harness, params: Value) -> nocturne_app::AppError {
    harness
        .app
        .rpc("checks.save", params.clone())
        .await
        .err()
        .unwrap_or_else(|| panic!("`checks.save` accepted {params}"))
}

fn history_rows(harness: &Harness, check_id: &str) -> Vec<(String, String, String)> {
    harness
        .app
        .store()
        .with(|conn| {
            let mut statement = conn.prepare(
                "SELECT device_id, previous, status FROM monitor_check_events
                 WHERE check_id = ?1 ORDER BY at, id",
            )?;
            statement
                .query_map([check_id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect()
        })
        .unwrap()
}

// ------------------------------------------------------------ the loss limit

#[tokio::test]
async fn a_loss_limit_of_zero_means_any_loss_is_an_outage_not_that_no_loss_is() {
    let harness = Harness::new().await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.only_rule("builtin.service_check_down").await;
    let check = harness
        .check_on(
            &device,
            "ping",
            json!({"kind": "icmp", "loss_pct_down": 0}),
            json!({}),
        )
        .await;

    // Every echo comes back.
    let answer = harness.rpc("checks.run_now", json!({"id": check})).await;
    let ran = &answer["checks"][0];
    assert_eq!(ran["facts"]["loss_pct"], 0.0);
    assert_eq!(
        ran["status"], "up",
        "a host that answered every echo has not lost any: {ran}"
    );
    assert_eq!(
        titles(&harness.open_alerts()),
        Vec::<&str>::new(),
        "and nobody is told it is down"
    );
    assert_eq!(
        harness
            .samples(&device, "check.up", &check)
            .last()
            .map(|sample| sample.1),
        Some(1.0)
    );

    // One echo in three goes missing: this is what the limit is there for.
    harness.probe.set(
        "10.0.0.1",
        HostState {
            lost: 1,
            ..HostState::default()
        },
    );
    let answer = harness.rpc("checks.run_now", json!({"id": check})).await;
    let ran = &answer["checks"][0];
    assert_eq!(ran["status"], "down", "{ran}");
    assert_eq!(ran["code"], "loss");
    assert!(
        ran["message"].as_str().unwrap().contains("any loss"),
        "the sentence has to make sense for a limit of nought: {ran}"
    );
}

#[tokio::test]
async fn a_loss_limit_that_is_not_a_percentage_is_refused() {
    let harness = Harness::new().await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    for bad in [json!(-5), json!(100.5), json!(1e9)] {
        let error = refused(
            &harness,
            json!({"device_id": device, "name": "ping",
                   "spec": {"kind": "icmp", "loss_pct_down": bad}}),
        )
        .await;
        assert_eq!(error.code, "invalid", "{bad}: {}", error.message);
        assert!(error.message.contains("0 to 100"), "{}", error.message);
    }
    // Both ends of the range mean something and are accepted.
    for (name, good) in [("any", json!(0)), ("all", json!(100))] {
        harness
            .check_on(
                &device,
                name,
                json!({"kind": "icmp", "loss_pct_down": good}),
                json!({}),
            )
            .await;
    }
}

#[tokio::test]
async fn a_check_stored_with_a_limit_that_is_now_refused_still_runs_sanely_and_can_be_edited() {
    // What an installation may hold from before the range was enforced.
    let harness = Harness::new().await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.silence_rules().await;
    let check = harness
        .check_on(&device, "ping", json!({"kind": "icmp"}), json!({}))
        .await;
    let (stored, _) = crate::checks::store::require(&harness.app, &check).unwrap();
    crate::checks::store::save(
        &harness.app,
        &ServiceCheck {
            spec: crate::checks::model::CheckSpec::Icmp {
                count: None,
                loss_pct_down: Some(-5.0),
            },
            ..stored
        },
    )
    .unwrap();

    let ran = harness.run_check(&check).await;
    assert_eq!(
        ran.state.status,
        CheckStatus::Up,
        "a healthy host is not down because of a limit below nought: {:?}",
        ran.state
    );
    // Switching it off, or renaming it, does not first demand that the
    // limit be put right: the save does not touch it.
    let saved = harness
        .rpc("checks.save", json!({"id": check, "enabled": false}))
        .await;
    assert_eq!(saved["enabled"], false);
    // Sending the spec again does.
    let error = refused(
        &harness,
        json!({"id": check, "spec": {"kind": "icmp", "loss_pct_down": -5}}),
    )
    .await;
    assert_eq!(error.code, "invalid");
}

// ------------------------------------------------- an SNMP check and a host

async fn router_with_an_agent(harness: &Harness) -> String {
    harness.with_snmp_credential("nocturne-test").await;
    harness
        .snmp_device(
            "R1",
            "10.0.0.1",
            AgentData {
                uptime_ticks: Some(1_234_500),
                ..AgentData::default()
            },
        )
        .await
}

/// Give a stored check a host without going through `checks.save`: the row
/// an installation was left with by a version that accepted one.
fn stored_with_host(harness: &Harness, check_id: &str, host: &str) {
    let (check, _) = crate::checks::store::require(&harness.app, check_id).unwrap();
    crate::checks::store::save(
        &harness.app,
        &ServiceCheck {
            host: host.to_string(),
            ..check
        },
    )
    .unwrap();
}

#[tokio::test]
async fn an_snmp_check_cannot_name_a_host_because_it_only_ever_asks_the_devices_own_agent() {
    let harness = Harness::new().await;
    let device = router_with_an_agent(&harness).await;

    let error = refused(
        &harness,
        json!({"device_id": device, "name": "agent",
               "spec": {"kind": "snmp"}, "host": "10.0.0.99"}),
    )
    .await;
    assert_eq!(error.code, "invalid");
    assert!(
        error.message.contains("own agent"),
        "the refusal has to say why: {}",
        error.message
    );

    // Nor by the back door: a check that names a host cannot be turned into
    // an SNMP check with the host left standing.
    let web = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "tcp", "port": 80}),
            json!({"host": "10.0.0.99"}),
        )
        .await;
    let error = refused(&harness, json!({"id": web, "spec": {"kind": "snmp"}})).await;
    assert_eq!(error.code, "invalid");
    let cleared = harness
        .rpc(
            "checks.save",
            json!({"id": web, "spec": {"kind": "snmp"}, "host": ""}),
        )
        .await;
    assert_eq!(cleared["kind"], "snmp");
    assert_eq!(cleared["host"], "");
}

#[tokio::test]
async fn clearing_the_host_an_snmp_check_never_used_does_not_throw_away_what_it_found() {
    let harness = Harness::new().await;
    let device = router_with_an_agent(&harness).await;
    let check = harness
        .check_on(&device, "agent", json!({"kind": "snmp"}), json!({}))
        .await;
    stored_with_host(&harness, &check, "10.0.0.99");

    let ran = harness.run_check(&check).await;
    assert_eq!(ran.state.status, CheckStatus::Up, "{:?}", ran.state);
    assert_eq!(
        *harness.snmp.opened.lock(),
        ["10.0.0.1:161"],
        "the device's own agent is what was asked, whatever the host said"
    );

    let saved = harness
        .rpc("checks.save", json!({"id": check, "host": ""}))
        .await;
    assert_eq!(
        saved["state"]["status"], "up",
        "the host was never where this check looked, so removing it \
         has not pointed the check anywhere else"
    );

    // And a save that does not mention the host tidies a stored one away
    // rather than refusing to rename the check until it has been.
    stored_with_host(&harness, &check, "10.0.0.99");
    let renamed = harness
        .rpc("checks.save", json!({"id": check, "name": "snmp agent"}))
        .await;
    assert_eq!(renamed["host"], "");
}

#[tokio::test]
async fn a_stored_host_does_not_get_an_snmp_check_past_the_simulated_device_guard() {
    let harness = Harness::new().await;
    harness.with_snmp_credential("nocturne-test").await;
    // The hazard: a lab device carrying an address that belongs to a real
    // host somewhere else, which happens to have an agent.
    harness.snmp.set(
        "10.9.9.9:161",
        AgentData {
            uptime_ticks: Some(100),
            ..AgentData::default()
        },
    );
    let lab = harness
        .device(
            "LAB1",
            "10.9.9.9",
            json!({"lab": {"lab_id": "testlab", "node": "LAB1"},
                   "credential_id": harness.credential_id}),
        )
        .await;
    let check = harness
        .check_on(&lab, "agent", json!({"kind": "snmp"}), json!({}))
        .await;
    stored_with_host(&harness, &check, "10.0.0.99");

    let ran = harness.run_check(&check).await;
    assert_eq!(ran.state.code, "sim", "{:?}", ran.state);
    assert_eq!(ran.state.status, CheckStatus::CannotRun);
    assert_eq!(
        harness.snmp.opens("10.9.9.9:161"),
        0,
        "nothing was sent to the stranger whose address the lab device carries"
    );
}

// ------------------------------------- a run that outlives its definition

/// A check whose probe takes eight seconds to fail, and a run of it started
/// through `checks.run_now` on another task.
async fn slow_check_under_way(
    harness: &Harness,
) -> (
    String,
    String,
    tokio::task::JoinHandle<nocturne_app::AppResult<Value>>,
) {
    harness.silence_rules().await;
    let device = harness.device("SW1", "10.0.0.1", json!({})).await;
    harness.probe.set(
        "10.0.0.5",
        HostState {
            up: false,
            tcp_up: false,
            delay: Duration::from_secs(8),
            ..HostState::default()
        },
    );
    let check = harness
        .check_on(
            &device,
            "web",
            json!({"kind": "icmp"}),
            json!({"host": "10.0.0.5", "timeout_ms": 10_000}),
        )
        .await;
    let running = tokio::spawn({
        let app = harness.app.clone();
        let id = check.clone();
        async move { app.rpc("checks.run_now", json!({"id": id})).await }
    });
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(harness.probe.count("10.0.0.5"), 1, "the run is under way");
    (device, check, running)
}

#[tokio::test(start_paused = true)]
async fn a_run_under_way_when_its_check_is_repointed_says_nothing_about_the_new_target() {
    let harness = Harness::new().await;
    let (device, check, running) = slow_check_under_way(&harness).await;

    harness
        .rpc("checks.save", json!({"id": check, "host": "10.0.0.6"}))
        .await;
    let answer = running.await.unwrap();

    let state = harness.check_state(&check);
    assert_eq!(
        state.status,
        CheckStatus::Unknown,
        "10.0.0.6 has not been sent a single packet: {state:?}"
    );
    assert_eq!(state.last_down_at, None);
    assert_eq!(harness.probe.count("10.0.0.6"), 0);
    assert_eq!(history_rows(&harness, &check), []);
    assert!(
        harness.samples(&device, "check.up", &check).is_empty(),
        "nor is there a sample of the new target being down"
    );

    // The caller is told the run came to nothing, and why.
    let answer = answer.expect("the call itself did not fail");
    assert_eq!(answer["ran"], 0);
    let said = answer["checks"][0]["error"].as_str().unwrap_or_default();
    assert!(said.contains("changed"), "{answer}");
}

#[tokio::test(start_paused = true)]
async fn a_run_under_way_when_its_check_is_deleted_leaves_nothing_behind() {
    let harness = Harness::new().await;
    let (device, check, running) = slow_check_under_way(&harness).await;

    harness.rpc("checks.delete", json!({"id": check})).await;
    let answer = running.await.unwrap();

    assert_eq!(
        history_rows(&harness, &check),
        [],
        "no history for a check that no longer exists"
    );
    let said_after: Vec<String> = harness
        .events_of(&device)
        .into_iter()
        .map(|entry| entry.message)
        .filter(|message| message.contains("not answering"))
        .collect();
    assert_eq!(
        said_after,
        Vec::<String>::new(),
        "and no line in the device's timeline about it"
    );
    assert!(harness.samples(&device, "check.up", &check).is_empty());

    let answer = answer.expect("a check deleted mid-run is not a failed call");
    assert_eq!(answer["ran"], 0);
    assert_eq!(answer["checks"][0]["check_id"], check);
    assert!(answer["checks"][0]["error"].is_string(), "{answer}");
}

// ------------------------------------------------------- moving a check

#[tokio::test(start_paused = true)]
async fn moving_a_check_to_another_device_leaves_behind_what_it_found_on_the_old_one() {
    let harness = Harness::new().await;
    harness.only_rule("builtin.service_check_down").await;
    let old = harness.device("SW1", "10.0.0.1", json!({})).await;
    let new = harness.device("SW2", "10.0.0.2", json!({})).await;
    harness.probe.down("10.0.0.1");
    // No host of its own: it watches whatever device it is on.
    let check = harness
        .check_on(&old, "web", json!({"kind": "icmp"}), json!({}))
        .await;
    harness.run_check(&check).await;
    assert_eq!(
        titles(&harness.open_alerts()),
        ["Service check down — web on SW1"]
    );
    tokio::time::sleep(Duration::from_secs(60)).await;

    let moved = harness
        .rpc("checks.save", json!({"id": check, "device_id": new}))
        .await;
    assert_eq!(
        moved["state"]["status"], "unknown",
        "nothing has been sent to SW2 yet, so nothing is known about `web` there"
    );
    assert_eq!(moved["state"]["device_id"], new);
    assert_eq!(
        titles(&harness.open_alerts()),
        Vec::<&str>::new(),
        "SW1 no longer has a `web` check, so its alert is closed at once"
    );

    // SW2 answers ping. Its next poll must not announce an outage nobody
    // has measured.
    let poll = harness.poll_and_evaluate(&new).await;
    assert!(poll.ok);
    assert_eq!(titles(&harness.open_alerts()), Vec::<&str>::new());

    // The history goes with the check, and shows the old verdict ending at
    // the move rather than running on into the new device's time.
    assert_eq!(
        history_rows(&harness, &check),
        [
            (new.clone(), "unknown".to_string(), "down".to_string()),
            (new.clone(), "down".to_string(), "unknown".to_string()),
        ]
    );
    crate::checks::store::forget_device(&harness.app, &old).unwrap();
    assert_eq!(
        history_rows(&harness, &check).len(),
        2,
        "forgetting the old device does not take a check it no longer has"
    );

    let ran = harness.run_check(&check).await;
    assert_eq!(ran.state.status, CheckStatus::Up);
    assert_eq!(ran.previous, CheckStatus::Unknown);
    assert_eq!(titles(&harness.open_alerts()), Vec::<&str>::new());
}
