//! What survives a stop and what does not — and what the user is told before
//! it happens.

mod common;

use std::time::Duration;

use common::*;
use nocturne_app::connect::{self, CliOptions};
use nocturne_app::{App, inventory};
use nocturne_transport::Output;
use serde_json::{Value, json};

async fn configure(app: &App, device_id: &str, lines: &[&str], save: bool) {
    let mut cli = connect::cli(app, device_id, CliOptions::default())
        .await
        .expect("a session");
    let lines: Vec<String> = lines.iter().map(|line| line.to_string()).collect();
    let result = cli
        .session()
        .configure(&lines)
        .await
        .expect("the configuration is applied");
    assert!(result.ok(), "{result:?}");
    if save {
        cli.session().save().await.expect("write memory");
    }
    cli.close().await;
}

async fn running_config(app: &App, lab_id: &str, node: &str) -> String {
    app.rpc(
        "labs.node.config",
        json!({"lab_id": lab_id, "node": node, "kind": "running"}),
    )
    .await
    .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn unsaved(app: &App, lab_id: &str) -> Vec<String> {
    app.rpc("labs.get", json!({ "id": lab_id })).await.unwrap()["unsaved_nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn saved_configuration_survives_a_restart_and_unsaved_does_not() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "ospf_triangle", "Keep").await;
    let id = lab["id"].as_str().unwrap().to_string();
    let r1 = lab["devices"]["R1"].as_str().unwrap().to_string();

    assert_eq!(unsaved(&app, &id).await, Vec::<String>::new());

    // An edit that is saved.
    configure(&app, &r1, &["ntp server 192.0.2.1"], false).await;
    until_rpc(
        &app,
        Duration::from_secs(2),
        "R1 to be reported as unsaved",
        "labs.get",
        json!({ "id": id }),
        |view| view["unsaved_nodes"] == json!(["R1"]),
    )
    .await;
    configure(&app, &r1, &["ntp server 192.0.2.1"], true).await;
    until_rpc(
        &app,
        Duration::from_secs(2),
        "R1 to be saved again",
        "labs.get",
        json!({ "id": id }),
        |view| view["unsaved_nodes"] == json!([]),
    )
    .await;

    // And an edit that is not.
    configure(&app, &r1, &["hostname EDGE"], false).await;
    let listed = until_rpc(
        &app,
        Duration::from_secs(2),
        "the unsaved hostname change",
        "labs.get",
        json!({ "id": id }),
        |view| view["unsaved_nodes"] == json!(["R1"]),
    )
    .await;
    assert_eq!(listed["unsaved_nodes"], json!(["R1"]));

    let stopped = app.rpc("labs.stop", json!({ "id": id })).await.unwrap();
    assert_eq!(
        stopped["unsaved_nodes"],
        json!(["R1"]),
        "stopping says what was lost"
    );
    app.rpc("labs.start", json!({ "id": id })).await.unwrap();

    let config = running_config(&app, &id, "R1").await;
    assert!(
        config.contains("ntp server 192.0.2.1"),
        "what was written to memory came back: {config}"
    );
    assert!(
        !config.contains("hostname EDGE") && config.contains("hostname R1"),
        "what was not saved did not: {config}"
    );
    assert_eq!(unsaved(&app, &id).await, Vec::<String>::new());
}

#[tokio::test]
async fn a_lab_outlives_the_application() {
    let directory = tempfile::tempdir().unwrap();
    let (lab_id, device_id) = {
        let app = builder_at(directory.path()).build();
        fast_ticks(&app).await;
        let lab = start_example(&app, "basic_switching", "Durable").await;
        let id = lab["id"].as_str().unwrap().to_string();
        let sw1 = lab["devices"]["SW1"].as_str().unwrap().to_string();
        configure(
            &app,
            &sw1,
            &["vlan 42", "name finance", "ip default-gateway 192.168.1.1"],
            true,
        )
        .await;
        // Stopping writes the specification back before the lab goes away.
        app.rpc("labs.stop", json!({ "id": id })).await.unwrap();
        app.shutdown();
        // Shutting down is asynchronous: give the background tasks their turn
        // so that the data directory is released.
        tokio::time::sleep(Duration::from_millis(100)).await;
        (id, sw1)
    };
    // A second application on the same data directory: this only works
    // because nothing in the lab module keeps the first one alive.
    let app = builder_at(directory.path()).build();
    fast_ticks(&app).await;
    let labs = app.rpc("labs.list", Value::Null).await.unwrap();
    assert_eq!(labs.as_array().unwrap().len(), 1);
    assert_eq!(labs[0]["name"], "Durable");
    assert_eq!(labs[0]["running"], false);
    // The device survived with it, with the same id.
    let device = inventory::get(&app, &device_id).expect("the device is still there");
    assert_eq!(device.name, "Durable/SW1");

    app.rpc("labs.start", json!({ "id": lab_id }))
        .await
        .unwrap();
    let config = running_config(&app, &lab_id, "SW1").await;
    assert!(
        config.contains("ip default-gateway 192.168.1.1"),
        "{config}"
    );
    // The VLAN database is not part of the configuration file, and is kept too.
    let tables = app
        .rpc("labs.node.tables", json!({"lab_id": lab_id, "node": "SW1"}))
        .await
        .unwrap();
    assert!(
        tables["vlans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|vlan| vlan["id"] == 42 && vlan["name"] == "finance"),
        "{}",
        tables["vlans"]
    );
    app.shutdown();
}

#[tokio::test]
async fn an_edit_on_a_stopped_lab_is_stored_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let app = builder_at(directory.path()).build();
    let lab = app
        .rpc(
            "labs.create",
            json!({"name": "Cold", "example": "basic_switching"}),
        )
        .await
        .unwrap();
    let id = lab["id"].as_str().unwrap().to_string();
    app.rpc(
        "labs.node.add",
        json!({"lab_id": id, "model": "router", "name": "R9", "x": 10, "y": 10}),
    )
    .await
    .unwrap();
    app.shutdown();
    drop(app);
    tokio::time::sleep(Duration::from_millis(100)).await;

    let app = builder_at(directory.path()).build();
    let view = app.rpc("labs.get", json!({ "id": id })).await.unwrap();
    assert!(
        view["snapshot"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["name"] == "R9"),
        "{}",
        view["snapshot"]["nodes"]
    );
    assert!(view["devices"]["R9"].is_string());
    app.shutdown();
}

#[tokio::test]
async fn a_startup_configuration_can_be_replaced_and_says_what_was_refused() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "basic_switching", "Paste").await;
    let id = lab["id"].as_str().unwrap().to_string();

    let result = app
        .rpc(
            "labs.node.config_set",
            json!({"lab_id": id, "node": "SW1", "startup":
                   "hostname ACCESS1\nvlan 7\n name printers\ninterface FastEthernet0/1\n description printer\nbogus command here\nend\n"}),
        )
        .await
        .unwrap();
    assert_eq!(result["rejected"].as_array().unwrap().len(), 1);
    assert_eq!(result["rejected"][0]["line"], "bogus command here");
    assert!(
        result["rejected"][0]["error"]
            .as_str()
            .unwrap()
            .contains("Invalid input"),
        "{result}"
    );
    assert!(result["accepted"].as_u64().unwrap() >= 5);
    assert!(
        result["text"]
            .as_str()
            .unwrap()
            .contains("hostname ACCESS1")
    );

    // The node really rebooted with it, and nothing is unsaved.
    let config = running_config(&app, &id, "SW1").await;
    assert!(config.contains("hostname ACCESS1"), "{config}");
    assert!(config.contains("description printer"), "{config}");
    assert_eq!(unsaved(&app, &id).await, Vec::<String>::new());

    // And it survives a restart, because it is the startup configuration.
    app.rpc("labs.stop", json!({ "id": id })).await.unwrap();
    app.rpc("labs.start", json!({ "id": id })).await.unwrap();
    assert!(
        running_config(&app, &id, "SW1")
            .await
            .contains("hostname ACCESS1")
    );
    let startup = app
        .rpc(
            "labs.node.config",
            json!({"lab_id": id, "node": "SW1", "kind": "startup"}),
        )
        .await
        .unwrap();
    assert!(
        startup["text"]
            .as_str()
            .unwrap()
            .contains("description printer")
    );
    // VLANs live in `vlan.dat`, not in the configuration file, and the
    // specification keeps them too.
    let tables = app
        .rpc("labs.node.tables", json!({"lab_id": id, "node": "SW1"}))
        .await
        .unwrap();
    assert!(
        tables["vlans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|vlan| vlan["name"] == "printers"),
        "{}",
        tables["vlans"]
    );
}

#[tokio::test]
async fn power_and_reset_put_a_node_back_to_its_saved_state() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "ospf_triangle", "Reset").await;
    let id = lab["id"].as_str().unwrap().to_string();
    let r1 = lab["devices"]["R1"].as_str().unwrap().to_string();
    configure(&app, &r1, &["hostname TEMPORARY"], false).await;
    assert!(running_config(&app, &id, "R1").await.contains("TEMPORARY"));

    app.rpc("labs.reset", json!({ "id": id })).await.unwrap();
    let config = running_config(&app, &id, "R1").await;
    assert!(!config.contains("TEMPORARY"), "{config}");
    assert_eq!(unsaved(&app, &id).await, Vec::<String>::new());

    // Powering a node off and on again does the same for that node alone.
    configure(&app, &r1, &["hostname AGAIN"], false).await;
    app.rpc(
        "labs.node.update",
        json!({"lab_id": id, "node": "R1", "powered": false}),
    )
    .await
    .unwrap();
    let off = app
        .rpc("labs.node.config", json!({"lab_id": id, "node": "R1"}))
        .await
        .unwrap_err();
    assert!(off.message.contains("powered off"), "{off:?}");
    app.rpc(
        "labs.node.update",
        json!({"lab_id": id, "node": "R1", "powered": true}),
    )
    .await
    .unwrap();
    assert!(!running_config(&app, &id, "R1").await.contains("AGAIN"));
}

#[tokio::test]
async fn a_renamed_node_keeps_its_device_and_its_history() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "basic_switching", "Renames").await;
    let id = lab["id"].as_str().unwrap().to_string();
    let sw1 = lab["devices"]["SW1"].as_str().unwrap().to_string();
    nocturne_app::collect::store(
        &app,
        &sw1,
        nocturne_drivers::model::Kind::Vlans,
        vec![json!({"id": 1, "name": "default"})],
        "lab",
    )
    .unwrap();

    app.rpc(
        "labs.node.update",
        json!({"lab_id": id, "node": "SW1", "name": "CORE"}),
    )
    .await
    .unwrap();
    let device = inventory::get(&app, &sw1).unwrap();
    assert_eq!(device.name, "Renames/CORE");
    assert_eq!(device.lab.unwrap().node, "CORE");
    assert!(
        nocturne_app::collect::latest(&app, &sw1, nocturne_drivers::model::Kind::Vlans)
            .unwrap()
            .is_some(),
        "what was collected is still there"
    );
    let view = app.rpc("labs.get", json!({ "id": id })).await.unwrap();
    assert!(view["devices"]["CORE"].is_string());
    assert!(view["devices"]["SW1"].is_null());

    // The console follows the new name.
    let device = inventory::get(&app, &sw1).unwrap();
    let connection = connect::open(&app, &device, &Default::default())
        .await
        .unwrap();
    let (_writer, mut reader) = connection.split();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), reader.recv())
            .await
            .unwrap(),
        Some(Output::Data(_))
    ));
}
