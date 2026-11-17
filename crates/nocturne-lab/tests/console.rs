//! A simulated device must behave like a real one to the rest of Nocturne:
//! the same `connect::cli`, the same read-only gate, the same collectors.

mod common;

use std::time::{Duration, Instant};

use common::*;
use nocturne_app::connect::{self, CliOptions, OpenOptions};
use nocturne_app::inventory;
use nocturne_drivers::model::Kind;
use nocturne_transport::{Output, TransportKind};
use serde_json::{Value, json};

/// The inventory id of a node of a running lab.
fn device_id(lab: &Value, node: &str) -> String {
    lab["devices"][node]
        .as_str()
        .unwrap_or_else(|| panic!("a device for {node}: {}", lab["devices"]))
        .to_string()
}

#[tokio::test]
async fn a_console_is_an_ordinary_cli_session() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "vlans_and_a_trunk", "Consoles").await;
    let sw1 = device_id(&lab, "SW1");

    let device = inventory::get(&app, &sw1).unwrap();
    assert_eq!(device.transport, nocturne_app::model::Transport::Sim);
    assert_eq!(device.profile, "cisco_ios");
    assert_eq!(device.lab.as_ref().unwrap().node, "SW1");
    assert!(device.address.is_empty(), "a lab address is not an address");

    let mut cli = connect::cli(&app, &sw1, CliOptions::default())
        .await
        .expect("the console logs in");
    assert_eq!(cli.hostname(), "SW1");
    assert_eq!(cli.info.kind, Some(TransportKind::Sim));
    assert!(
        cli.info.peer.starts_with("lab:Consoles/SW1"),
        "{:?}",
        cli.info
    );

    let version = cli.read("show version").await.unwrap();
    assert!(
        version.output.contains("Nocturne Virtual Software"),
        "{version:?}"
    );
    assert_eq!(version.error, None);

    // The read-only gate is the platform's, not ours.
    let refused = cli.read("configure terminal").await.unwrap_err();
    assert_eq!(refused.code, "forbidden");
    assert!(refused.message.contains("read-only"), "{refused:?}");
    assert_eq!(cli.read("reload").await.unwrap_err().code, "forbidden");

    // Real `show` output, parsed by the real collectors.
    let brief = cli.read("show ip interface brief").await.unwrap();
    assert!(brief.output.contains("FastEthernet0/1"), "{brief:?}");

    // One automated session per device at a time.
    let busy = connect::cli(
        &app,
        &sw1,
        CliOptions {
            wait: Duration::from_millis(50),
            ..CliOptions::default()
        },
    )
    .await;
    assert_eq!(busy.err().map(|err| err.code), Some("unavailable"));
    cli.close().await;
    connect::cli(&app, &sw1, CliOptions::default())
        .await
        .unwrap()
        .close()
        .await;
}

#[tokio::test]
async fn collecting_from_a_lab_device_returns_real_data() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "vlans_and_a_trunk", "Collect").await;
    let sw1 = device_id(&lab, "SW1");

    let snapshot = nocturne_app::collect::run(
        &app,
        &sw1,
        &[Kind::Interfaces, Kind::Neighbors, Kind::Vlans],
    )
    .await
    .expect("a collection");
    let interfaces = snapshot.interfaces.as_ref().expect("interfaces");
    assert!(
        interfaces.len() >= 26,
        "24 FastEthernet and 2 Gigabit ports: {}",
        interfaces.len()
    );
    let uplink = interfaces
        .iter()
        .find(|interface| interface.name == "GigabitEthernet0/1")
        .expect("the trunk port");
    assert_eq!(uplink.oper_up, Some(true));
    assert_eq!(uplink.mode, "trunk", "{uplink:?}");
    assert!(uplink.description.contains("Trunk"), "{uplink:?}");

    let neighbors = snapshot.neighbors.as_ref().expect("neighbours");
    let sw2 = neighbors
        .iter()
        .find(|neighbor| neighbor.device.starts_with("SW2"))
        .unwrap_or_else(|| panic!("SW2 among {neighbors:?}"));
    assert_eq!(sw2.local_interface, "GigabitEthernet0/1");
    assert_eq!(sw2.remote_interface, "GigabitEthernet0/1");
    assert_eq!(sw2.protocol, "cdp");

    let vlans = snapshot.vlans.as_ref().expect("vlans");
    assert!(vlans.iter().any(|vlan| vlan.name == "Sales"), "{vlans:?}");

    // Stored like any other collected data, and marked as coming from a lab.
    let stored = nocturne_app::collect::latest(&app, &sw1, Kind::Interfaces)
        .unwrap()
        .unwrap();
    assert_eq!(stored.source, "lab");
    let shown = app
        .rpc("collect.get", json!({"id": sw1, "kind": "neighbors"}))
        .await
        .unwrap();
    assert!(!shown["rows"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_host_is_a_device_too() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "dhcp_and_nat", "Hosts").await;
    let pc1 = device_id(&lab, "PC1");
    let device = inventory::get(&app, &pc1).unwrap();
    assert_eq!(device.profile, "nocturne_sim_host");
    assert_eq!(device.role, "host");
    assert!(!device.monitor.enabled, "a lab host is not polled");

    let mut cli = connect::cli(&app, &pc1, CliOptions::default())
        .await
        .expect("the host shell logs in");
    let hostname = cli.read("hostname").await.unwrap();
    assert!(hostname.output.contains("PC1"), "{hostname:?}");
    // Addresses come from the router's DHCP pool.
    let config = cli.read("ipconfig /all").await.unwrap();
    assert!(config.output.contains("IPv4 Address"), "{config:?}");
    // Releasing an address is not a read: the gate stops it.
    assert_eq!(
        cli.read("ipconfig /release").await.unwrap_err().code,
        "forbidden"
    );
    cli.close().await;

    let snapshot = nocturne_app::collect::run(&app, &pc1, &[Kind::Facts, Kind::Interfaces])
        .await
        .unwrap();
    assert_eq!(
        snapshot.facts.as_ref().map(|facts| facts.hostname.as_str()),
        Some("PC1")
    );
    let interfaces = snapshot.interfaces.as_ref().expect("interfaces");
    assert_eq!(interfaces.len(), 1, "{interfaces:#?}");
    assert_eq!(interfaces[0].name, "Ethernet0");
    assert!(
        interfaces[0]
            .ipv4
            .first()
            .is_some_and(|ip| ip.contains('/')),
        "a DHCP address in CIDR form: {interfaces:?}"
    );
    assert_eq!(interfaces[0].oper_up, Some(true));
}

#[tokio::test]
async fn typing_is_echoed_without_waiting_for_a_tick() {
    let app = app();
    // One tick every five seconds: an echo that waits for one takes seconds,
    // and one that does not takes milliseconds. The gap between the two
    // behaviours has to be much wider than what a busy machine adds, or the
    // bound below stops being about the console and becomes a measurement of
    // the runner — with a one-second tick there were only a few hundred
    // milliseconds between "answered at once" and "waited for a tick".
    const TICK_MS: u64 = 5_000;
    app.rpc(
        "settings.set",
        json!({"section": "labs", "value": {"tick_ms": TICK_MS}}),
    )
    .await
    .unwrap();
    let lab = start_example(&app, "basic_switching", "Latency").await;
    let sw1 = device_id(&lab, "SW1");
    let device = inventory::get(&app, &sw1).unwrap();

    let connection = connect::open(&app, &device, &OpenOptions::default())
        .await
        .unwrap();
    let (writer, mut reader) = connection.split();
    // The greeting and the first prompt.
    let greeting = tokio::time::timeout(Duration::from_millis(500), reader.recv())
        .await
        .expect("a greeting arrives at once")
        .expect("output");
    assert!(matches!(greeting, Output::Data(_)));

    let started = Instant::now();
    writer.send("show clock\r").await.unwrap();
    // Read until the answer is complete or the tick interval runs out —
    // waiting for the condition rather than for a fixed slice of the clock,
    // so that the only thing the elapsed time below can mean is "this waited
    // for a tick".
    let mut seen = String::new();
    let answer_by = tokio::time::Instant::now() + Duration::from_millis(TICK_MS / 2);
    while !seen.contains("UTC") {
        match tokio::time::timeout_at(answer_by, reader.recv()).await {
            Ok(Some(Output::Data(bytes))) => seen.push_str(&String::from_utf8_lossy(&bytes)),
            _ => break,
        }
    }
    assert!(
        seen.contains("show clock"),
        "the keystrokes are echoed at once, not once a tick: {seen:?}"
    );
    assert!(
        seen.contains("UTC"),
        "and the command answered without waiting for a tick: {seen:?}"
    );
    assert!(
        started.elapsed() < Duration::from_millis(TICK_MS / 2),
        "it took {:?}, which is a tick's worth of waiting",
        started.elapsed()
    );
}

#[tokio::test]
async fn two_consoles_on_one_node_work_side_by_side() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "basic_switching", "Two").await;
    let sw1 = device_id(&lab, "SW1");
    let device = inventory::get(&app, &sw1).unwrap();

    let first = connect::open(&app, &device, &OpenOptions::default())
        .await
        .unwrap();
    let second = connect::open(&app, &device, &OpenOptions::default())
        .await
        .unwrap();
    for connection in [&first, &second] {
        connection.send("show clock\r").await.unwrap();
    }
    for (name, connection) in [("first", first), ("second", second)] {
        let (_, mut reader) = connection.split();
        let mut seen = String::new();
        while !seen.contains("UTC") {
            match tokio::time::timeout(Duration::from_secs(2), reader.recv()).await {
                Ok(Some(Output::Data(bytes))) => seen.push_str(&String::from_utf8_lossy(&bytes)),
                other => panic!("{name} console: {other:?} after {seen:?}"),
            }
        }
    }
}

#[tokio::test]
async fn deleting_a_lab_closes_its_consoles() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "basic_switching", "Doomed").await;
    let id = lab["id"].as_str().unwrap().to_string();
    let sw1 = device_id(&lab, "SW1");
    let device = inventory::get(&app, &sw1).unwrap();

    let connection = connect::open(&app, &device, &OpenOptions::default())
        .await
        .unwrap();
    let (_writer, mut reader) = connection.split();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .unwrap(),
        Some(Output::Data(_))
    ));

    app.rpc("labs.delete", json!({ "id": id })).await.unwrap();

    let mut closed = None;
    while let Ok(Some(output)) = tokio::time::timeout(Duration::from_secs(2), reader.recv()).await {
        if let Output::Closed { reason } = output {
            closed = Some(reason);
            break;
        }
    }
    let reason = closed.expect("the console is closed, not left hanging");
    assert_eq!(reason.as_deref(), Some("the lab was deleted"));
    // Nothing is left of the lab's devices, so nothing can reconnect.
    assert_eq!(
        connect::cli(&app, &sw1, CliOptions::default())
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn opening_a_console_starts_a_stopped_lab() {
    let app = app();
    fast_ticks(&app).await;
    let created = app
        .rpc(
            "labs.create",
            json!({"name": "Asleep", "example": "basic_switching"}),
        )
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let sw1 = created["devices"]["SW1"].as_str().unwrap().to_string();
    assert_eq!(created["running"], false);

    let cli = connect::cli(&app, &sw1, CliOptions::default())
        .await
        .expect("the lab starts for the console");
    assert_eq!(cli.hostname(), "SW1");
    cli.close().await;
    let view = app.rpc("labs.get", json!({ "id": id })).await.unwrap();
    assert_eq!(view["running"], true);
}

#[tokio::test]
async fn the_host_profile_loads() {
    let app = app();
    let report = profile_problems(&app).await;
    assert_eq!(report["problems"], json!([]), "{report}");
}

#[tokio::test]
async fn a_host_s_addressing_can_be_changed_from_the_lab() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "basic_switching", "Addressing").await;
    let id = lab["id"].as_str().unwrap().to_string();
    let pc1 = device_id(&lab, "PC1");

    let updated = app
        .rpc(
            "labs.node.update",
            json!({"lab_id": id, "node": "PC1",
                   "host": {"ip": "192.168.1.90", "mask": "255.255.255.0", "gateway": "192.168.1.1"}}),
        )
        .await
        .unwrap();
    assert_eq!(updated["interfaces"][0]["ip"], "192.168.1.90/24");

    let mut cli = connect::cli(&app, &pc1, CliOptions::default())
        .await
        .unwrap();
    let config = cli.read("ipconfig").await.unwrap();
    assert!(config.output.contains("192.168.1.90"), "{config:?}");
    cli.close().await;

    // Asking a router to be a host is refused, with the node's name in it.
    let refused = app
        .rpc(
            "labs.node.update",
            json!({"lab_id": id, "node": "SW1", "host": {"ip": "10.0.0.1"}}),
        )
        .await
        .unwrap_err();
    assert!(refused.message.contains("SW1"), "{refused:?}");
}

#[tokio::test]
async fn removing_a_node_closes_the_console_on_it() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "basic_switching", "Unplugged").await;
    let id = lab["id"].as_str().unwrap().to_string();
    let pc3 = device_id(&lab, "PC3");
    let device = inventory::get(&app, &pc3).unwrap();

    let connection = connect::open(&app, &device, &OpenOptions::default())
        .await
        .unwrap();
    let (_writer, mut reader) = connection.split();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .unwrap(),
        Some(Output::Data(_))
    ));

    app.rpc("labs.node.remove", json!({"lab_id": id, "node": "PC3"}))
        .await
        .unwrap();
    let mut reason = None;
    while let Ok(Some(output)) = tokio::time::timeout(Duration::from_secs(2), reader.recv()).await {
        if let Output::Closed { reason: why } = output {
            reason = Some(why);
            break;
        }
    }
    assert_eq!(
        reason.expect("the console is closed").as_deref(),
        Some("the node was removed from the lab")
    );
    // The device went with the node, so nothing points at a node that is gone.
    assert!(inventory::get(&app, &pc3).is_err());
}
