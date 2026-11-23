//! `logs.ingest`: a lab's simulated devices have no socket to send from, so
//! the lab hands their lines over by name — and from there on they are
//! lines like any other.

mod support;

use nocturne_netsim::{CableKind, HostConfig, Lab as SimLab, LabEvent};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use support::*;

/// The lines as a simulated IOS device writes them: no priority in front,
/// the severity is in the mnemonic.
const CONFIG_CHANGED: &str =
    "*Mar  1 00:12:02.123: %SYS-5-CONFIG_I: Configured from console by admin on vty0 (10.0.0.5)";
const LINK_DOWN: &str =
    "*Mar  1 00:12:10.001: %LINK-3-UPDOWN: Interface GigabitEthernet0/1, changed state to down";

#[tokio::test]
async fn a_line_cannot_be_written_for_hardware_through_the_api() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let hardware = app
        .rpc(
            "devices.create",
            json!({"name": "CORE1", "address": "192.0.2.10", "profile": "test_ios"}),
        )
        .await
        .unwrap();
    let mut events = app.subscribe();

    let refused = app
        .rpc(
            "logs.ingest",
            json!({"device_id": hardware["id"], "lines": [CONFIG_CHANGED]}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden", "{refused}");
    assert!(refused.message.contains("not a lab device"), "{refused}");

    // Nothing was stored, nothing was signalled, nothing was counted.
    assert!(query(app, json!({"limit": 10})).await.is_empty());
    assert!(drained(&mut events, "logs.config_changed").is_empty());
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["syslog"]["from_lab"], json!(0));
    assert_eq!(status["syslog"]["stored"], json!(0));

    assert_eq!(
        app.rpc(
            "logs.ingest",
            json!({"device_id": "no-such-device", "lines": [CONFIG_CHANGED]}),
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
}

#[tokio::test]
async fn no_spelling_of_the_request_writes_a_line_for_hardware() {
    let (lab, monitor) = lab_with_monitor(&["SW1"]).await;
    let app = &lab.app;
    let hardware = app
        .rpc(
            "devices.create",
            json!({"name": "CORE1", "address": "192.0.2.10", "profile": "test_ios"}),
        )
        .await
        .unwrap();
    let hardware_id = hardware["id"].as_str().unwrap().to_string();
    let sim = lab.id("SW1");
    // An alert-raising line, so that a line that did get through would show
    // as more than a row.
    let line = "*Mar  1 00:16:00.000: %PM-4-ERR_DISABLE: bpduguard error detected on Gi1/0/7, putting Gi1/0/7 in err-disable state";
    let lab_ref = json!({"lab_id": "testlab", "node": "CORE1"});

    // Every way the inventory knows the device by, and everything a caller
    // might add to make it pass for a simulated one.
    let attempts = [
        json!({"device_id": hardware_id, "lines": [line]}),
        json!({"device_id": "CORE1", "lines": [line]}),
        json!({"device_id": "core1", "lines": [line]}),
        json!({"device_id": "  CORE1 ", "lines": [line]}),
        json!({"device_id": "192.0.2.10", "lines": [line]}),
        json!({"device_id": hardware_id, "lines": [line], "lab": true}),
        json!({"device_id": hardware_id, "lines": [line], "lab": lab_ref, "transport": "sim"}),
        json!({"device_id": hardware_id, "lines": [line], "via": "lab", "source": ""}),
        json!({"device_id": hardware_id, "lines": [line], "device": sim, "name": "SW1"}),
        json!({"device_id": hardware_id, "lab_id": "testlab", "node": "SW1", "lines": [line]}),
        // Parameters by position are the same parameters.
        json!([hardware_id, [line]]),
    ];
    for attempt in attempts {
        let refused = app.rpc("logs.ingest", attempt.clone()).await.unwrap_err();
        assert_eq!(refused.code, "forbidden", "{attempt}: {refused}");
    }
    // A request that is not the method's shape names no device at all.
    let malformed = [
        json!({"device_id": [sim, hardware_id], "lines": [line]}),
        json!({"device_id": {"id": sim}, "lines": [line]}),
        json!({"device": hardware_id, "lines": [line]}),
        json!({"device_id": "", "lines": [line]}),
        json!({"device_id": hardware_id, "lines": line}),
        // An empty reference must not "find" a device that has no address.
        json!({"device_id": " ", "lines": [line]}),
    ];
    for attempt in malformed {
        let refused = app.rpc("logs.ingest", attempt.clone()).await.unwrap_err();
        assert_eq!(refused.code, "invalid", "{attempt}: {refused}");
    }
    // Too many lines for one call does not get past the check either.
    let many: Vec<&str> = std::iter::repeat_n(line, 10_001).collect();
    let refused = app
        .rpc(
            "logs.ingest",
            json!({"device_id": hardware_id, "lines": many}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden", "{refused}");

    // A record that names a lab and is still reached over SSH is a real
    // device with a label on it. The inventory does not let one be saved;
    // this is one that got there some other way.
    let mut labelled: nocturne_app::model::Device = app
        .store()
        .get(nocturne_app::model::collections::DEVICES, &hardware_id)
        .unwrap()
        .expect("the device is stored");
    labelled.lab = Some(nocturne_app::model::LabRef {
        lab_id: "testlab".into(),
        node: "CORE1".into(),
    });
    assert_eq!(labelled.transport, nocturne_app::model::Transport::Ssh);
    app.store()
        .put(
            nocturne_app::model::collections::DEVICES,
            &hardware_id,
            &labelled,
        )
        .unwrap();
    let refused = app
        .rpc(
            "logs.ingest",
            json!({"device_id": hardware_id, "lines": [line]}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden", "{refused}");
    // And an alert about it goes where alerts go: it is not a lab's.
    let mut events = app.subscribe();
    let from_the_wire = format!("<188>50: {line}");
    receive(
        app,
        &[(from_the_wire.as_str(), "192.0.2.10", 1_763_000_000_000)],
    );
    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(raised["device_id"], json!(hardware_id));
    assert!(
        raised.get("notify").is_none(),
        "a label does not silence a real device: {raised:#}"
    );

    // After all of that: the one line that came off the wire, and nothing
    // from `logs.ingest`.
    let stored = query(app, json!({"limit": 50})).await;
    assert_eq!(stored.len(), 1, "{stored:#?}");
    assert_eq!(stored[0]["body"]["via"], json!("syslog"));
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["syslog"]["from_lab"], json!(0));
    assert_eq!(monitor.raise_calls(), 1);
    let audit = query(app, json!({"stream": "audit", "limit": 100})).await;
    assert!(
        audit
            .iter()
            .all(|entry| entry["source"] != json!("logs.ingest")),
        "nothing was handed over, so nothing says it was"
    );
}

#[tokio::test]
async fn a_lab_device_named_like_hardware_gets_its_own_lines_and_only_those() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let hardware = app
        .rpc(
            "devices.create",
            json!({"name": "CORE1", "address": "192.0.2.10", "profile": "test_ios"}),
        )
        .await
        .unwrap();
    // A simulated device whose *name* is the hardware's address.
    let twin = app
        .rpc(
            "devices.create",
            json!({"name": "192.0.2.10", "profile": "test_ios", "lab": {"lab_id": "testlab", "node": "twin"}}),
        )
        .await
        .unwrap();

    let stored = app
        .rpc(
            "logs.ingest",
            json!({"device_id": "192.0.2.10", "lines": [LINK_DOWN]}),
        )
        .await
        .unwrap();
    assert_eq!(stored, json!({"stored": 1}));
    let entries = query(app, json!({"limit": 10})).await;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["device_id"], twin["id"],
        "the line belongs to the simulated device that was found"
    );
    assert!(
        query(app, json!({"device_id": hardware["id"], "limit": 10}))
            .await
            .is_empty(),
        "and nothing was written for the switch that has that address"
    );
}

#[tokio::test]
async fn a_lab_line_takes_the_path_of_a_datagram() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    let mut events = app.subscribe();

    let result = app
        .rpc(
            "logs.ingest",
            json!({"device_id": id, "lines": [CONFIG_CHANGED, "", LINK_DOWN]}),
        )
        .await
        .unwrap();
    assert_eq!(result, json!({"stored": 2}), "the empty line is not a line");

    // Stored, attributed without an address, and marked for what it is.
    let entries = query(app, json!({"limit": 10})).await;
    assert_eq!(entries.len(), 2);
    for entry in &entries {
        assert_eq!(entry["device_id"], json!(id));
        assert_eq!(entry["body"]["via"], json!("lab"));
        assert_eq!(
            entry["source"],
            json!(""),
            "a simulated device sent from no address, so none is made up"
        );
    }
    assert_eq!(
        entries[0]["severity"],
        json!(3),
        "the severity the device wrote into its mnemonic"
    );

    // Indexed: the filters a log screen uses find it.
    let by_mnemonic = messages(app, json!({"mnemonic": "SYS-5-CONFIG_I"})).await;
    assert_eq!(by_mnemonic.len(), 1, "{by_mnemonic:#?}");
    let bursts = app
        .rpc("logs.bursts", json!({"min_count": 1}))
        .await
        .unwrap();
    assert_eq!(bursts.as_array().unwrap().len(), 2, "{bursts:#}");

    // The rules ran: the configuration feature hears about the change.
    let published = drain_all(&mut events);
    let changed = topic(&published, "logs.config_changed");
    assert_eq!(changed.len(), 1, "{published:#?}");
    assert_eq!(changed[0]["device_id"], json!(id));
    assert_eq!(changed[0]["by"], json!("admin"));
    let kinds: Vec<&str> = topic(&published, "logs.signal")
        .iter()
        .map(|signal| signal["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["config_changed", "interface_down"]);

    // On the live tail.
    nocturne_logs::tail::flush(app, 0);
    let appended = next_event(&mut events, "logs.appended").await;
    assert_eq!(appended["entries"].as_array().unwrap().len(), 2);

    // Counted as what it is: stored, from a lab, never received on a socket.
    let status = app.rpc("logs.status", Value::Null).await.unwrap();
    assert_eq!(status["syslog"]["from_lab"], json!(2));
    assert_eq!(status["syslog"]["stored"], json!(2));
    assert_eq!(status["syslog"]["received"], json!(0));
    // And no address was invented for the list of senders.
    let sources = app.rpc("logs.sources", Value::Null).await.unwrap();
    assert_eq!(sources, json!([]));
}

#[tokio::test]
async fn the_hand_over_is_audited_once_per_device_and_never_with_the_lines() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");

    for _ in 0..3 {
        app.rpc(
            "logs.ingest",
            json!({"device_id": id, "lines": [LINK_DOWN]}),
        )
        .await
        .unwrap();
    }
    let audit = query(app, json!({"stream": "audit", "limit": 100})).await;
    let hand_overs: Vec<&Value> = audit
        .iter()
        .filter(|entry| entry["source"] == json!("logs.ingest"))
        .collect();
    assert_eq!(
        hand_overs.len(),
        1,
        "a lab talks many times a second; the trail says that it does, once"
    );
    assert_eq!(hand_overs[0]["device_id"], json!(id));
    let written = serde_json::to_string(hand_overs[0]).unwrap();
    assert!(
        !written.contains("UPDOWN") && !written.contains("GigabitEthernet"),
        "the audit entry carries no log text: {written}"
    );
}

#[tokio::test]
async fn a_secret_in_a_lab_line_is_not_stored_either() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");

    let line = "*Mar  1 00:30:00.000: %PARSER-5-CFGLOG_LOGGEDCMD: User:admin  logged command:snmp-server community Str0ngR0 RO";
    app.rpc("logs.ingest", json!({"device_id": id, "lines": [line]}))
        .await
        .unwrap();
    let stored = serde_json::to_string(&query(app, json!({"limit": 10})).await).unwrap();
    assert!(
        !stored.contains("Str0ngR0"),
        "a simulated community is redacted like a real one: {stored}"
    );
    assert!(stored.contains("<secret:"), "{stored}");
}

#[tokio::test]
async fn more_lines_than_one_call_may_carry_are_refused_not_truncated() {
    let lab = lab(&["SW1"]).await;
    let app = &lab.app;
    let lines: Vec<&str> = std::iter::repeat_n(LINK_DOWN, 10_001).collect();
    let refused = app
        .rpc(
            "logs.ingest",
            json!({"device_id": lab.id("SW1"), "lines": lines}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "invalid", "{refused}");
    assert!(query(app, json!({"limit": 1})).await.is_empty());
}

/// A simulated switch with port security on the port that leads to three
/// hosts, allowed two addresses: the third host's first frame is a
/// violation. Returns what the switch logged, as the simulator wrote it.
fn simulated_violation() -> Vec<String> {
    let mut sim = SimLab::new("psec");
    let add = |sim: &mut SimLab, model: &str, name: &str| {
        sim.add_node(model, Some(name), 0.0, 0.0)
            .unwrap_or_else(|err| panic!("add {name}: {err}"))
    };
    let s1 = add(&mut sim, "switch", "SW1");
    let s2 = add(&mut sim, "switch", "SW2");
    let mut hosts = Vec::new();
    for (index, ip) in ["10.0.0.1", "10.0.0.2", "10.0.0.4", "10.0.0.3"]
        .iter()
        .enumerate()
    {
        let host = add(&mut sim, "pc", &format!("PC{}", index + 1));
        sim.set_host_config(
            host,
            HostConfig {
                ip: Some(ip.to_string()),
                mask: Some("255.255.255.0".into()),
                ..HostConfig::default()
            },
        )
        .expect("the host takes an address");
        hosts.push(host);
    }
    let mut link = |a, a_if: &str, b, b_if: &str| {
        sim.add_link(a, a_if, b, b_if, CableKind::Auto)
            .unwrap_or_else(|err| panic!("link {a_if} - {b_if}: {err}"));
    };
    link(s1, "fa0/1", s2, "fa0/24");
    for (index, host) in hosts.iter().take(3).enumerate() {
        link(*host, "Ethernet0", s2, &format!("fa0/{}", index + 1));
    }
    link(hosts[3], "Ethernet0", s1, "fa0/3");
    sim.settle(60_000_000);

    let config: Vec<String> = [
        "interface fa0/1",
        " switchport mode access",
        " switchport port-security",
        " switchport port-security maximum 2",
        " switchport port-security violation restrict",
    ]
    .iter()
    .map(|line| line.to_string())
    .collect();
    let outcome = sim
        .configure(s1, &config)
        .expect("the switch is configured");
    assert!(
        outcome.lines.iter().all(|line| !line.error),
        "{:#?}",
        outcome.lines
    );
    sim.take_events();
    for host in hosts.iter().take(3) {
        sim.exec(*host, "ping 10.0.0.3 -n 2")
            .expect("the ping runs");
    }
    sim.take_events()
        .into_iter()
        .filter_map(|event| match event {
            LabEvent::Syslog { node, line, .. } if node == s1 => Some(line),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_port_security_violation_on_a_lab_switch_raises_its_alert() {
    let lines = simulated_violation();
    assert!(
        lines.iter().any(|line| line.contains("PSECURE_VIOLATION")),
        "the simulated switch logged no violation: {lines:#?}"
    );

    let (lab, monitor) = lab_with_monitor(&["SW1"]).await;
    let app = &lab.app;
    let id = lab.id("SW1");
    let mut events = app.subscribe();

    // What the lab runtime does with a simulated device's lines.
    let stored = app
        .rpc("logs.ingest", json!({"device_id": id, "lines": lines}))
        .await
        .unwrap();
    assert_eq!(stored["stored"], json!(lines.len()));

    let raised = next_event(&mut events, "test.alert_raised").await;
    assert_eq!(raised["rule"], json!("IOS: port-security violation"));
    assert_eq!(raised["device_id"], json!(id));
    assert_eq!(raised["severity"], json!("high"));
    assert_eq!(raised["instance"], json!("FastEthernet0/1"));
    assert!(
        raised["title"]
            .as_str()
            .unwrap()
            .starts_with("Port-security violation on FastEthernet0/1 by "),
        "{raised:#}"
    );
    assert!(
        raised.get("notify").is_none(),
        "it is a lab, and whether a lab's alert reaches a channel is the monitor's \
         `notify_for_lab` to say: nothing here rules a channel out: {raised:#}"
    );
    assert_eq!(
        monitor.open_keys(),
        [format!(
            "logs:builtin.ios.port_security:{id}:FastEthernet0/1"
        )]
    );
}
