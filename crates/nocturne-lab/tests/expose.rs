//! `labs.expose`: a real TCP port in front of a simulated console, driven by
//! the ordinary transport client.

mod common;

use common::*;
use nocturne_app::connect::{self, CliOptions};
use nocturne_app::{App, AppBuilder, AppConfig};
use serde_json::{Value, json};

/// A device that reaches the exposed port the way any other client would.
async fn client_device(app: &App, name: &str, transport: &str, address: &str) -> String {
    let credential = app
        .rpc(
            "credentials.create",
            json!({"name": name, "username": "admin", "password": "nocturne"}),
        )
        .await
        .unwrap();
    let (host, port) = address.rsplit_once(':').expect("host:port");
    let device = app
        .rpc(
            "devices.create",
            json!({"name": name, "address": host, "port": port.parse::<u16>().unwrap(),
                   "transport": transport, "profile": "cisco_ios",
                   "credential_id": credential["id"]}),
        )
        .await
        .unwrap();
    device["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn ssh_and_telnet_ports_reach_the_console() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "basic_switching", "Exposed").await;
    let id = lab["id"].as_str().unwrap().to_string();

    for (protocol, name) in [("ssh", "over-ssh"), ("telnet", "over-telnet")] {
        let exposed = app
            .rpc(
                "labs.expose",
                json!({"lab_id": id, "node": "SW1", "protocol": protocol, "port": 0}),
            )
            .await
            .unwrap();
        let address = exposed["address"].as_str().unwrap().to_string();
        assert!(address.starts_with("127.0.0.1:"), "{exposed}");
        assert_ne!(address, "127.0.0.1:0", "a real port was chosen");
        assert_eq!(exposed["node"], "SW1");

        let device = client_device(&app, name, protocol, &address).await;
        let mut cli = connect::cli(&app, &device, CliOptions::default())
            .await
            .unwrap_or_else(|err| panic!("{protocol}: {err}"));
        assert_eq!(cli.hostname(), "SW1");
        let version = cli.read("show version").await.unwrap();
        assert!(
            version.output.contains("Nocturne Virtual Software"),
            "{protocol}: {version:?}"
        );
        let brief = cli.read("show ip interface brief").await.unwrap();
        assert!(brief.output.contains("Vlan1"), "{protocol}: {brief:?}");
        cli.close().await;

        // It is listed, and closing it really closes the port.
        let listed = app.rpc("labs.exposed", Value::Null).await.unwrap();
        let ours = listed
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["address"] == address.as_str())
            .unwrap_or_else(|| panic!("{address} among {listed}"));
        assert_eq!(ours["protocol"], protocol);
        assert_eq!(ours["lab"], "Exposed");

        app.rpc("labs.unexpose", json!({"id": ours["id"]}))
            .await
            .unwrap();
        assert!(
            connect::cli(&app, &device, CliOptions::default())
                .await
                .is_err(),
            "{protocol}: the port is closed"
        );
    }
    assert_eq!(
        app.rpc("labs.exposed", Value::Null).await.unwrap(),
        json!([])
    );
}

#[tokio::test]
async fn exposing_is_refused_where_it_makes_no_sense() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "dhcp_and_nat", "Refusals").await;
    let id = lab["id"].as_str().unwrap().to_string();

    let unknown_node = app
        .rpc(
            "labs.expose",
            json!({"lab_id": id, "node": "Nowhere", "protocol": "ssh"}),
        )
        .await
        .unwrap_err();
    assert_eq!(unknown_node.code, "not_found");
    let no_cli = app
        .rpc(
            "labs.expose",
            json!({"lab_id": id, "node": "Internet", "protocol": "ssh"}),
        )
        .await
        .unwrap_err();
    assert!(no_cli.message.contains("no command line"), "{no_cli:?}");
    let protocol = app
        .rpc(
            "labs.expose",
            json!({"lab_id": id, "node": "R1", "protocol": "http"}),
        )
        .await
        .unwrap_err();
    assert!(protocol.message.contains("ssh"), "{protocol:?}");
    assert_eq!(
        app.rpc("labs.unexpose", json!({"id": "nope"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn a_server_cannot_open_a_port_to_the_network() {
    // The same app, but reached over the network rather than from the desktop.
    let mut builder = AppBuilder::new(AppConfig::in_memory().remote_users()).unwrap();
    nocturne_app::init(&mut builder);
    nocturne_lab::init(&mut builder);
    let app = builder.build();
    let lab = app
        .rpc(
            "labs.create",
            json!({"name": "Server", "example": "basic_switching"}),
        )
        .await
        .unwrap();
    let id = lab["id"].as_str().unwrap().to_string();

    let refused = app
        .rpc(
            "labs.expose",
            json!({"lab_id": id, "node": "SW1", "protocol": "telnet", "bind": "0.0.0.0"}),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "forbidden");
    // Loopback is still fine.
    let local = app
        .rpc(
            "labs.expose",
            json!({"lab_id": id, "node": "SW1", "protocol": "telnet"}),
        )
        .await
        .unwrap();
    assert!(local["address"].as_str().unwrap().starts_with("127.0.0.1:"));
    app.rpc("labs.unexpose", json!({"id": local["id"]}))
        .await
        .unwrap();
}
