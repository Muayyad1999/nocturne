//! Who may apply a change: the matrix, as the change engine answers it.
//!
//! The assistant's crate does not decide this and neither does the model. The
//! only apply its tools can reach is `changes.assistant_apply`, and what these
//! tests prove is that whatever the engine answers — applied, or refused with
//! a reason — comes back to the model in words it can relay to the user, and
//! that a refusal stops there.

mod common;

use common::{Harness, Options};
use nocturne_ai::provider::scripted::Step;
use serde_json::{Value, json};

/// Draft a one-line change on `device` and then ask to apply it.
async fn propose_then_apply(harness: &Harness, device: &str, lines: Value) -> (String, String) {
    harness.script(vec![
        Step::tool(
            "propose_change",
            json!({"title": format!("change on {device}"),
                   "targets": [{"device": device, "lines": lines}]}),
        ),
        Step::Dynamic(Box::new(|request| {
            let id = request
                .messages
                .iter()
                .rev()
                .flat_map(|message| message.content.iter())
                .filter_map(|block| match block {
                    nocturne_ai::Block::ToolResult { content, .. } => Some(content.clone()),
                    _ => None,
                })
                .find_map(|content| {
                    content
                        .split("id: ")
                        .nth(1)
                        .and_then(|rest| rest.split_whitespace().next())
                        .map(str::to_string)
                })
                .expect("the change id came back to the model");
            Step::tool("apply_change", json!({"id": id}))
        })),
        Step::text("that is where I got to"),
    ]);
    let turn = harness.send(&format!("configure {device}")).await;
    assert_eq!(turn.stop(), "end", "{:#?}", turn.kinds());
    let results = turn.tool_results();
    assert_eq!(results.len(), 2, "{results:#?}");
    (results[1].1.clone(), results[1].2.clone())
}

async fn harness_with(autonomy: &str, devices: Vec<&'static str>) -> Harness {
    Harness::with(Options {
        devices,
        settings: json!({"autonomy": autonomy}),
        ..Options::default()
    })
    .await
}

/// A device that is not simulated, for the "real network" half of the matrix.
async fn add_real_device(harness: &Harness, name: &str, protected: bool) {
    harness
        .app()
        .rpc(
            "devices.create",
            json!({"name": name, "address": "198.51.100.7", "profile": "test_ios",
                   "site": "HQ", "role": "access", "protected": protected}),
        )
        .await
        .expect("device created");
}

#[tokio::test]
async fn in_draft_mode_nothing_is_ever_applied() {
    let harness = harness_with("draft", vec!["SW1"]).await;
    let (status, message) = propose_then_apply(
        &harness,
        "SW1",
        json!(["interface GigabitEthernet0/2", " switchport access vlan 40"]),
    )
    .await;
    assert_eq!(status, "error");
    assert!(
        message.contains("will not apply"),
        "the refusal is relayed: {message}"
    );
    let sent = harness.sent_to_model();
    assert!(
        sent.contains("autonomy mode is `draft`") && sent.contains("Changes screen"),
        "the model is told what the user has to do: {sent}"
    );
    let changes = harness.app().rpc("changes.list", json!({})).await.unwrap();
    assert_eq!(changes[0]["state"], "planned", "it stays a draft");
}

#[tokio::test]
async fn in_lab_mode_a_lab_device_is_applied_and_a_real_one_is_not() {
    let harness = harness_with("lab", vec!["SW1"]).await;
    let (status, summary) = propose_then_apply(
        &harness,
        "SW1",
        json!(["interface GigabitEthernet0/2", " switchport access vlan 40"]),
    )
    .await;
    assert_eq!(status, "ok", "{summary}");
    let changes = harness.app().rpc("changes.list", json!({})).await.unwrap();
    assert_eq!(changes[0]["state"], "applied");
    assert_eq!(changes[0]["approval"]["by"], "policy:lab");
    assert!(
        harness.sent_to_model().contains("check config: ok"),
        "the verification result is reported back"
    );

    // The same change on a device that is not simulated is refused.
    add_real_device(&harness, "HQ-ACC1", false).await;
    let (status, message) =
        propose_then_apply(&harness, "HQ-ACC1", json!(["ntp server 192.0.2.1"])).await;
    assert_eq!(status, "error");
    assert!(message.contains("will not apply"), "{message}");
    assert!(
        harness.sent_to_model().contains("not only on lab devices"),
        "the reason is the one the engine gave"
    );
}

#[tokio::test]
async fn in_low_risk_mode_a_low_risk_change_goes_through_and_a_risky_one_does_not() {
    let harness = harness_with("low_risk", vec!["SW1"]).await;
    add_real_device(&harness, "HQ-ACC1", false).await;

    let (status, _) =
        propose_then_apply(&harness, "HQ-ACC1", json!(["ntp server 192.0.2.1"])).await;
    assert_eq!(status, "ok", "an additive change to an access switch");
    let changes = harness.app().rpc("changes.list", json!({})).await.unwrap();
    assert_eq!(changes[0]["approval"]["by"], "policy:low_risk");

    // Management access is always a human's decision, whatever the mode.
    let (status, message) = propose_then_apply(
        &harness,
        "HQ-ACC1",
        json!(["line vty 0 4", " transport input telnet"]),
    )
    .await;
    assert_eq!(status, "error");
    assert!(
        message.contains("will not apply"),
        "touching the way in is never automatic: {message}"
    );
    assert!(
        harness.sent_to_model().contains("protected resources"),
        "and the model is told why"
    );
}

#[tokio::test]
async fn a_protected_device_is_never_applied_to_whatever_the_mode() {
    let harness = harness_with("low_risk", vec!["SW1"]).await;
    add_real_device(&harness, "CORE1", true).await;
    let (status, message) =
        propose_then_apply(&harness, "CORE1", json!(["ntp server 192.0.2.1"])).await;
    assert_eq!(status, "error");
    assert!(message.contains("will not apply"), "{message}");
    assert!(
        harness.sent_to_model().contains("protected device"),
        "{}",
        harness.sent_to_model()
    );
}

#[tokio::test]
async fn a_trunk_vlan_list_that_would_replace_the_list_is_flagged_and_not_applied() {
    let harness = harness_with("lab", vec!["SW1"]).await;
    let (status, _) = propose_then_apply(
        &harness,
        "SW1",
        json!([
            "interface GigabitEthernet0/1",
            " switchport trunk allowed vlan 40"
        ]),
    )
    .await;
    assert_eq!(
        status, "error",
        "replacing a trunk's VLAN list is high risk, so it waits for a human"
    );
    let sent = harness.sent_to_model();
    assert!(
        sent.contains("allowed VLAN list") && sent.contains("it replaces the list"),
        "the protected hit is explained to the model: {sent}"
    );
    // The model saw the risk before it ever asked to apply.
    assert!(sent.contains("risk: high"), "{sent}");
}

#[tokio::test]
async fn without_a_change_pipeline_there_is_no_proposing_at_all() {
    let harness = Harness::with(Options {
        devices: vec!["SW1"],
        changes: false,
        ..Options::default()
    })
    .await;
    harness.script(vec![
        Step::tool("propose_change", json!({"title": "x", "targets": [{"device": "SW1", "lines": ["ntp server 192.0.2.1"]}]})),
        Step::text("I cannot draft changes in this installation."),
    ]);
    let turn = harness.send("set the ntp server").await;
    assert_eq!(turn.stop(), "end");
    // The tool is not offered, so the agent itself says there is no such tool.
    let results = turn.tool_results();
    assert_eq!(results[0].1, "error");
    assert!(
        harness
            .sent_to_model()
            .contains("no tool named \\\"propose_change\\\"")
            || harness.sent_to_model().contains("no tool named"),
        "{}",
        harness.sent_to_model()
    );
    let offered: Vec<String> = harness.provider().requests()[0]
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    assert!(!offered.contains(&"propose_change".to_string()));
    assert!(!offered.contains(&"apply_change".to_string()));
    assert!(offered.contains(&"run_commands".to_string()));
    // And the model is told, so it does not promise a change it cannot make.
    let briefing = harness.provider().requests()[0].messages[0].text();
    assert!(
        briefing.contains("no change pipeline registered")
            && briefing.contains("they will have to do it themselves"),
        "{briefing}"
    );
}

#[tokio::test]
async fn the_status_method_says_which_mode_is_in_force() {
    let harness = harness_with("low_risk", vec!["SW1"]).await;
    let status = harness.app().rpc("ai.status", json!({})).await.unwrap();
    assert_eq!(status["configured"], true);
    assert_eq!(status["autonomy"], "low_risk");
    assert_eq!(status["redaction"], true);
    assert_eq!(status["default"]["model"], "scripted");
    assert!(status["default"]["has_key"].as_bool().unwrap());
    assert!(status["tools"].as_u64().unwrap() > 20);

    // An unknown mode in the settings file is read as `draft`, never as more.
    harness
        .app()
        .rpc(
            "settings.set",
            json!({"section": "assistant", "value": {"autonomy": "full-self-driving"}}),
        )
        .await
        .unwrap();
    let status = harness.app().rpc("ai.status", json!({})).await.unwrap();
    assert_eq!(status["autonomy"], "draft");
}
