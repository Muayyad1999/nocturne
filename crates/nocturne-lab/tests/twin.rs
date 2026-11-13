//! `labs.twin`: a lab built from collected data and backups, which says
//! exactly how faithful it is.

mod common;

use common::*;
use nocturne_app::App;
use nocturne_app::jobs;
use nocturne_drivers::model::Kind;
use serde_json::{Value, json};

const R1_CONFIG: &str = "\
hostname R1
!
interface GigabitEthernet0/0
 description To R2
 ip address 10.0.0.1 255.255.255.252
 no shutdown
!
interface GigabitEthernet0/1
 ip address 192.168.1.1 255.255.255.0
 no shutdown
!
interface GigabitEthernet0/9
 ip address 10.9.9.1 255.255.255.0
!
ip route 0.0.0.0 0.0.0.0 10.0.0.2
frobnicate the widgets
!
end
";

const R2_CONFIG: &str = "\
hostname R2
!
interface GigabitEthernet0/0
 description To R1
 ip address 10.0.0.2 255.255.255.252
 no shutdown
!
ip route 192.168.1.0 255.255.255.0 10.0.0.1
!
end
";

async fn real_device(app: &App, name: &str, address: &str) -> String {
    let device = app
        .rpc(
            "devices.create",
            json!({"name": name, "address": address, "profile": "cisco_ios", "site": "HQ"}),
        )
        .await
        .unwrap();
    device["id"].as_str().unwrap().to_string()
}

fn interface(name: &str, ip: Option<&str>) -> Value {
    json!({"name": name, "admin_up": true, "oper_up": true,
           "ipv4": ip.map(|ip| vec![ip]).unwrap_or_default()})
}

async fn with_data(
    app: &App,
    name: &str,
    address: &str,
    interfaces: Vec<Value>,
    neighbors: Vec<Value>,
    config: &str,
) -> String {
    let id = real_device(app, name, address).await;
    nocturne_app::collect::store_many(
        app,
        &id,
        vec![
            (
                Kind::Facts,
                vec![json!({"hostname": name, "vendor": "Cisco", "model": "ISR4331"})],
            ),
            (Kind::Interfaces, interfaces),
            (Kind::Neighbors, neighbors),
        ],
        "cli",
    )
    .unwrap();
    app.store()
        .config_insert(&id, "running", config, "manual", "")
        .unwrap();
    id
}

#[tokio::test]
async fn a_twin_reports_exactly_how_faithful_it_is() {
    let app = app();
    fast_ticks(&app).await;
    let r1 = with_data(
        &app,
        "R1",
        "192.0.2.1",
        vec![
            interface("GigabitEthernet0/0", Some("10.0.0.1/30")),
            interface("GigabitEthernet0/1", Some("192.168.1.1/24")),
            interface("GigabitEthernet0/9", Some("10.9.9.1/24")),
            interface("Loopback0", Some("1.1.1.1/32")),
        ],
        vec![
            json!({"device": "R2.example.com", "local_interface": "GigabitEthernet0/0",
                    "remote_interface": "GigabitEthernet0/0", "protocol": "cdp"}),
        ],
        R1_CONFIG,
    )
    .await;
    let r2 = with_data(
        &app,
        "R2",
        "192.0.2.2",
        vec![interface("GigabitEthernet0/0", Some("10.0.0.2/30"))],
        vec![json!({"device": "R1", "local_interface": "Gi0/0",
                    "remote_interface": "Gi0/0", "protocol": "cdp"})],
        R2_CONFIG,
    )
    .await;
    // A device nothing has been collected from yet.
    let bare = real_device(&app, "SW-NEW", "192.0.2.9").await;

    let started = app
        .rpc(
            "labs.twin",
            json!({"device_ids": [r1, r2, bare], "name": "HQ twin"}),
        )
        .await
        .unwrap();
    let job = jobs::wait(&app, started["job_id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(job.state, jobs::JobState::Succeeded, "{job:?}");
    let result = job.result.expect("a result");

    // The device with nothing collected is refused, with the reason.
    let refused = result["refused"].as_array().unwrap();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["device"], "SW-NEW");
    assert!(
        refused[0]["reason"].as_str().unwrap().contains("collected"),
        "{refused:?}"
    );

    let nodes = result["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    let node = nodes.iter().find(|node| node["device"] == "R1").unwrap();
    assert_eq!(node["node"], "R1");
    assert_eq!(node["model"], "router");
    assert_eq!(node["config_source"], "running");

    // The port the chosen model does not have is named, not hidden.
    assert_eq!(
        node["interfaces_unmapped"],
        json!(["GigabitEthernet0/9"]),
        "{node}"
    );
    assert_eq!(
        node["interfaces_mapped"],
        json!(["GigabitEthernet0/0", "GigabitEthernet0/1"])
    );

    // And so is every line the simulated device refused.
    let rejected = node["rejected"].as_array().unwrap();
    let lines: Vec<&str> = rejected
        .iter()
        .map(|line| line["line"].as_str().unwrap().trim())
        .collect();
    assert!(
        lines.contains(&"frobnicate the widgets"),
        "the nonsense line: {lines:?}"
    );
    assert!(
        lines.iter().any(|line| line.contains("GigabitEthernet0/9")),
        "the interface the model does not have: {lines:?}"
    );
    assert!(
        rejected
            .iter()
            .all(|line| !line["error"].as_str().unwrap().is_empty()),
        "each refusal carries what the device said: {rejected:?}"
    );
    assert_eq!(
        node["accepted"].as_u64().unwrap(),
        node["config_lines"].as_u64().unwrap() - rejected.len() as u64
    );

    // The neighbour tables became a cable, whichever spelling they used.
    let links = result["links"].as_array().unwrap();
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(links[0]["a_interface"], "GigabitEthernet0/0");
    assert_eq!(links[0]["b_interface"], "GigabitEthernet0/0");

    // The lab is real: it has devices of its own and it runs.
    let lab_id = result["lab_id"].as_str().unwrap().to_string();
    let view = app.rpc("labs.get", json!({ "id": lab_id })).await.unwrap();
    assert_eq!(view["name"], "HQ twin");
    assert_eq!(view["devices"].as_object().unwrap().len(), 2);
    app.rpc("labs.start", json!({ "id": lab_id }))
        .await
        .unwrap();
    let config = app
        .rpc(
            "labs.node.config",
            json!({"lab_id": lab_id, "node": "R1", "kind": "running"}),
        )
        .await
        .unwrap();
    let text = config["text"].as_str().unwrap();
    assert!(text.contains("description To R2"), "{text}");
    assert!(text.contains("ip route 0.0.0.0 0.0.0.0 10.0.0.2"), "{text}");
    assert!(
        !text.contains("10.9.9.1"),
        "what the twin could not reproduce is not there: {text}"
    );

    // The twinned routers can reach each other, which is the point.
    let probe = app
        .rpc(
            "labs.probe",
            json!({"lab_id": lab_id, "from": "R1", "to": "10.0.0.2"}),
        )
        .await
        .unwrap();
    assert_eq!(probe["ok"], true, "{probe}");
}

#[tokio::test]
async fn a_twin_needs_something_to_work_from() {
    let app = app();
    let bare = real_device(&app, "NOTHING", "192.0.2.3").await;
    // Collected data but no backup.
    nocturne_app::collect::store(
        &app,
        &bare,
        Kind::Interfaces,
        vec![interface("GigabitEthernet0/0", None)],
        "cli",
    )
    .unwrap();

    let started = app
        .rpc("labs.twin", json!({"device_ids": [bare]}))
        .await
        .unwrap();
    let job = jobs::wait(&app, started["job_id"].as_str().unwrap())
        .await
        .unwrap();
    assert_eq!(job.state, jobs::JobState::Failed);
    let error = job.error.unwrap();
    assert!(error.contains("back them up"), "{error}");
    assert_eq!(
        app.rpc("labs.twin", json!({"device_ids": []}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(app.rpc("labs.list", Value::Null).await.unwrap(), json!([]));
}
