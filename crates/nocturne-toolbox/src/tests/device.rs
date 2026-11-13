//! Probing from a device: the command that is built, the gate it goes
//! through, and the output that comes back.
//!
//! The fake device of `nocturne_app::testing` is an IOS-flavoured switch, so
//! the IOS family is exercised end to end against it. The other families are
//! exercised by giving a device a profile that says it is one of them (a
//! device type is data, which is exactly why this works) and asserting the
//! command the device was sent — `FakeDevice::seen` records every line it
//! received, which is also how the injection tests prove that nothing was sent
//! at all.

use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::support;

/// Written from the documented IOS ping output (the form the simulator
/// renders and the parsers in this crate read), not recorded from hardware.
const IOS_PING: &str = "Type escape sequence to abort.\nSending 5, 100-byte ICMP Echos to 10.0.0.9, timeout is 2 seconds:\n!!!!!\nSuccess rate is 100 percent (5/5), round-trip min/avg/max = 1/2/4 ms";
const IOS_PING_LOST: &str = "Type escape sequence to abort.\nSending 5, 100-byte ICMP Echos to 10.0.0.9, timeout is 2 seconds:\n.....\nSuccess rate is 0 percent (0/5)";
const IOS_TRACE: &str = "Type escape sequence to abort.\nTracing the route to 10.0.0.9\n\n  1 10.0.0.1 0 msec 0 msec 0 msec\n  2 10.0.0.9 1 msec 1 msec *";
const ROUTEROS_PING: &str = "  SEQ HOST                                     SIZE TTL TIME       STATUS\n    0 10.0.0.9                                   56  64 1ms\n    1 10.0.0.9                                   56  64 2ms\n    sent=5 received=5 packet-loss=0% min-rtt=1ms avg-rtt=2ms max-rtt=3ms";

/// The test profile with a different platform written into it. A device type
/// is data: this is how a family that no built-in profile covers is tested.
fn profile_as(id: &str, os: &str, vendor: &str, read_only: &str) -> String {
    nocturne_app::testing::LAB_IOS
        .replace("id: test_ios", &format!("id: {id}"))
        .replace("name: Test IOS", &format!("name: Test {os}"))
        .replace("vendor: Test", &format!("vendor: {vendor}"))
        .replace("os: IOS", &format!("os: {os}"))
        // The collectors of the test profile read with `show`, and a profile
        // whose policy did not allow that would not load at all, so the
        // replacement adds to the read-only list rather than replacing it.
        .replace(
            "read_only: ['^show ',",
            &format!("read_only: [{read_only}, '^show ',"),
        )
}

/// The test profile with `ping` and `traceroute` taken off its read-only
/// list: a platform whose policy says a ping is not a read.
fn profile_as_without_ping(id: &str) -> String {
    nocturne_app::testing::LAB_IOS
        .replace("id: test_ios", &format!("id: {id}"))
        .replace("name: Test IOS", &format!("name: Test {id}"))
        .replace("'^ping ', '^traceroute ', ", "")
}

/// Add a device served by the fake `node`, with `profile`.
async fn device_with_profile(
    lab: &nocturne_app::testing::Lab,
    name: &str,
    profile: &str,
) -> String {
    let created = lab
        .app
        .rpc(
            "devices.create",
            json!({"name": name, "profile": profile, "credential_id": lab.credential_id,
                   "lab": {"lab_id": "testlab", "node": "SW1"}}),
        )
        .await
        .expect("the device is created");
    created["id"].as_str().expect("an id").to_string()
}

#[tokio::test]
async fn a_ping_from_an_ios_device_is_built_sent_and_parsed() {
    let lab = support::lab(&["SW1"]).await;
    lab.fake("SW1").with(|state| {
        state
            .outputs
            .insert("ping 10.0.0.9 repeat 5".into(), IOS_PING.into());
        state
            .outputs
            .insert("traceroute 10.0.0.9".into(), IOS_TRACE.into());
    });
    let id = lab.id("SW1");

    let result = lab
        .app
        .rpc(
            "probe.device.ping",
            json!({"device_id": id, "target": "10.0.0.9"}),
        )
        .await
        .unwrap();
    assert_eq!(result["command"], "ping 10.0.0.9 repeat 5");
    assert_eq!(result["family"], "ios-like (IOS, IOS-XE, EOS)");
    assert_eq!(result["from"], "SW1 itself");
    assert_eq!(result["device_name"], "SW1");
    assert_eq!(result["summary"]["sent"], 5);
    assert_eq!(result["summary"]["received"], 5);
    assert_eq!(result["summary"]["loss_pct"], 0.0);
    assert_eq!(result["summary"]["avg_ms"], 2.0);
    assert!(result["output"].as_str().unwrap().contains("Success rate"));
    // It really went to the device.
    assert!(
        lab.fake("SW1")
            .seen()
            .contains(&"ping 10.0.0.9 repeat 5".to_string()),
        "{:?}",
        lab.fake("SW1").seen()
    );

    // The count, the source interface and the VRF go where IOS puts them.
    lab.fake("SW1").with(|state| {
        state.outputs.insert(
            "ping vrf MGMT 10.0.0.9 repeat 2 source Vlan10".into(),
            IOS_PING.into(),
        );
    });
    let with_options = lab
        .app
        .rpc(
            "probe.device.ping",
            json!({"device_id": id, "target": "10.0.0.9", "count": 2, "source": "Vlan10", "vrf": "MGMT"}),
        )
        .await
        .unwrap();
    assert_eq!(
        with_options["command"],
        "ping vrf MGMT 10.0.0.9 repeat 2 source Vlan10"
    );

    // A traceroute has no summary to parse, and does not pretend to.
    let trace = lab
        .app
        .rpc(
            "probe.device.traceroute",
            json!({"device_id": id, "target": "10.0.0.9"}),
        )
        .await
        .unwrap();
    assert_eq!(trace["command"], "traceroute 10.0.0.9");
    assert_eq!(trace["summary"], Value::Null);
    assert!(
        trace["output"]
            .as_str()
            .unwrap()
            .contains("Tracing the route")
    );

    // Both runs are in the history, aimed at the device.
    let runs = lab
        .app
        .rpc(
            "probe.history",
            json!({"kind": "device.ping,device.traceroute"}),
        )
        .await
        .unwrap();
    assert_eq!(runs["total"], 3);
    assert_eq!(runs["runs"][0]["device"]["name"], "SW1");
    assert_eq!(runs["runs"][0]["from"], "SW1 itself");
    assert!(
        runs["runs"][0]["method"]
            .as_str()
            .unwrap()
            .starts_with("traceroute "),
        "{}",
        runs["runs"][0]["method"]
    );
}

#[tokio::test]
async fn a_device_that_answers_nothing_useful_is_reported_without_a_summary() {
    let lab = support::lab(&["SW1"]).await;
    let id = lab.id("SW1");
    // The fake's own `ping` answers for an unreachable address with dots.
    lab.fake("SW1").with(|state| {
        state
            .outputs
            .insert("ping 10.0.0.9 repeat 5".into(), IOS_PING_LOST.into());
    });
    let lost = lab
        .app
        .rpc(
            "probe.device.ping",
            json!({"device_id": id, "target": "10.0.0.9"}),
        )
        .await
        .unwrap();
    // Total loss is an answer about the network: 0 of 5, not "no summary".
    assert_eq!(lost["summary"]["received"], 0);
    assert_eq!(lost["summary"]["loss_pct"], 100.0);
    assert_eq!(lost["summary"]["avg_ms"], Value::Null);

    // Output in no recognised format gets no summary at all, and the device's
    // own words are returned whole.
    lab.fake("SW1").with(|state| {
        state.outputs.insert(
            "ping 10.0.0.8 repeat 5".into(),
            "something nobody has ever parsed".into(),
        );
    });
    let unknown = lab
        .app
        .rpc(
            "probe.device.ping",
            json!({"device_id": id, "target": "10.0.0.8"}),
        )
        .await
        .unwrap();
    assert_eq!(unknown["summary"], Value::Null);
    assert_eq!(unknown["output"], "something nobody has ever parsed");
}

#[tokio::test]
async fn a_target_that_is_not_a_target_never_reaches_the_command_line() {
    let lab = support::lab(&["SW1"]).await;
    let id = lab.id("SW1");
    lab.fake("SW1").clear_seen();

    for attempt in [
        "8.8.8.8; reload",
        "8.8.8.8 && write erase",
        "8.8.8.8\nreload",
        "`reload`",
        "$(reload)",
        "8.8.8.8 | show running-config",
        "8.8.8.8 repeat 99999",
        "",
    ] {
        let refused = lab
            .app
            .rpc(
                "probe.device.ping",
                json!({"device_id": id, "target": attempt}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code, "invalid", "accepted `{attempt}`");
    }
    // The source interface and the VRF go on the same command line, so they
    // are held to the same standard.
    for (source, vrf) in [("Vlan10; reload", "MGMT"), ("Vlan10", "MGMT; reload")] {
        let refused = lab
            .app
            .rpc(
                "probe.device.ping",
                json!({"device_id": id, "target": "10.0.0.9", "source": source, "vrf": vrf}),
            )
            .await
            .unwrap_err();
        assert_eq!(refused.code, "invalid");
    }
    assert!(
        lab.fake("SW1").seen().is_empty(),
        "something was sent to the device: {:?}",
        lab.fake("SW1").seen()
    );
    // Nothing was recorded as a probe either, because none of them ran.
    assert_eq!(
        lab.app
            .rpc("probe.history", json!({"kind": "device.ping"}))
            .await
            .unwrap()["total"],
        0
    );
}

#[tokio::test]
async fn each_platform_family_gets_its_own_command_on_the_wire() {
    let lab = support::lab(&["SW1"]).await;
    nocturne_app::profiles::save(
        &lab.app,
        &profile_as(
            "test_routeros",
            "RouterOS",
            "MikroTik",
            "'^/ping ', '^/tool ', '^terminal '",
        ),
    )
    .expect("the RouterOS-flavoured test profile loads");
    let id = device_with_profile(&lab, "MT1", "test_routeros").await;
    lab.fake("SW1").with(|state| {
        state
            .outputs
            .insert("/ping 10.0.0.9 count=5".into(), ROUTEROS_PING.into());
    });

    let result = lab
        .app
        .rpc(
            "probe.device.ping",
            json!({"device_id": id, "target": "10.0.0.9"}),
        )
        .await
        .unwrap();
    assert_eq!(result["command"], "/ping 10.0.0.9 count=5");
    assert_eq!(result["family"], "routeros");
    assert_eq!(result["summary"]["sent"], 5);
    assert_eq!(result["summary"]["max_ms"], 3.0);
    assert!(
        lab.fake("SW1")
            .seen()
            .contains(&"/ping 10.0.0.9 count=5".to_string())
    );
}

#[tokio::test]
async fn the_read_only_gate_still_decides_what_may_be_sent() {
    let lab = support::lab(&["SW1"]).await;
    // A platform whose policy does not class `ping` as a read. The command is
    // built, and then refused by the core — not by this crate.
    nocturne_app::profiles::save(&lab.app, &profile_as_without_ping("test_noping"))
        .expect("the profile loads");
    let id = device_with_profile(&lab, "LOCKED1", "test_noping").await;
    lab.fake("SW1").clear_seen();

    let refused = lab
        .app
        .rpc(
            "probe.device.ping",
            json!({"device_id": id, "target": "10.0.0.9"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden");
    assert!(refused.message.contains("read-only"), "{}", refused.message);
    assert!(
        !lab.fake("SW1")
            .seen()
            .iter()
            .any(|line| line.starts_with("ping")),
        "{:?}",
        lab.fake("SW1").seen()
    );
    // The refusal is recorded as a run that could not run.
    let runs = lab
        .app
        .rpc("probe.history", json!({"kind": "device.ping"}))
        .await
        .unwrap();
    assert_eq!(runs["runs"][0]["ok"], false);
    assert_eq!(runs["runs"][0]["error_code"], "forbidden");
}

#[tokio::test]
async fn a_platform_nobody_has_taught_it_is_unsupported_rather_than_guessed_at() {
    let lab = support::lab(&["SW1"]).await;
    let id = device_with_profile(&lab, "MYSTERY1", "generic").await;
    let refused = lab
        .app
        .rpc(
            "probe.device.ping",
            json!({"device_id": id, "target": "10.0.0.9"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "unsupported");
    assert!(
        refused.message.contains("generic") && refused.message.contains("RouterOS"),
        "{}",
        refused.message
    );
    assert!(lab.fake("SW1").seen().is_empty());

    assert_eq!(
        lab.app
            .rpc(
                "probe.device.ping",
                json!({"device_id": "no such device", "target": "10.0.0.9"})
            )
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}
