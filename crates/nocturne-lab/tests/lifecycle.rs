//! Creating, starting, stopping, duplicating, importing and deleting labs,
//! with the inventory following along at every step.

mod common;

use std::time::Duration;

use common::*;
use serde_json::{Value, json};

async fn devices(app: &nocturne_app::App) -> Vec<Value> {
    app.rpc("devices.list", json!({"lab": true}))
        .await
        .unwrap()
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn every_example_loads_starts_and_stops_with_its_devices() {
    let app = app();
    fast_ticks(&app).await;
    let examples = app.rpc("labs.examples", Value::Null).await.unwrap();
    let examples = examples.as_array().unwrap().clone();
    assert!(
        examples.len() >= 6,
        "the campus and five teaching labs: {examples:?}"
    );
    assert_eq!(examples[0]["id"], "demo");
    assert!(!examples[1]["teaches"].as_array().unwrap().is_empty());

    for example in &examples {
        let id = example["id"].as_str().unwrap();
        let view = app
            .rpc("labs.create", json!({ "example": id }))
            .await
            .unwrap();
        let lab_id = view["id"].as_str().unwrap().to_string();
        assert_eq!(view["running"], false);
        assert!(
            view["snapshot"]["nodes"].as_array().unwrap().len()
                == example["nodes"].as_u64().unwrap() as usize,
            "{id}: the stopped lab still shows its nodes"
        );

        // Every node with a command line became a device.
        let created = devices(&app).await;
        let expected = view["devices"].as_object().unwrap().len();
        assert_eq!(created.len(), expected, "{id}");
        for device in &created {
            assert_eq!(device["transport"], "sim");
            assert_eq!(device["lab"]["lab_id"], lab_id.as_str());
            assert!(
                device["site"].as_str().unwrap().starts_with("Lab: "),
                "{device}"
            );
            assert!(device["name"].as_str().unwrap().contains('/'));
            assert!(
                device["profile"] == "cisco_ios" || device["profile"] == "nocturne_sim_host",
                "{device}"
            );
        }

        let started = app
            .rpc("labs.start", json!({ "id": lab_id }))
            .await
            .unwrap();
        assert_eq!(started["running"], true);
        assert!(started["sim_time"].as_u64().unwrap() > 0, "{id} settled");
        let stopped = app.rpc("labs.stop", json!({ "id": lab_id })).await.unwrap();
        assert_eq!(stopped["running"], false);
        assert_eq!(
            stopped["unsaved_nodes"],
            json!([]),
            "{id}: nothing was edited, so nothing is unsaved"
        );
        // Starting again works, and so does deleting while stopped.
        app.rpc("labs.start", json!({ "id": lab_id }))
            .await
            .unwrap();
        let deleted = app
            .rpc("labs.delete", json!({ "id": lab_id }))
            .await
            .unwrap();
        assert_eq!(deleted["deleted"], true);
        assert!(devices(&app).await.is_empty(), "{id}: devices went too");
        assert_eq!(
            app.rpc("labs.get", json!({ "id": lab_id }))
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    }
    assert_eq!(app.rpc("labs.list", Value::Null).await.unwrap(), json!([]));
}

#[tokio::test]
async fn names_are_unique_and_devices_follow_a_rename() {
    let app = app();
    let first = app
        .rpc("labs.create", json!({"name": "Campus", "example": "demo"}))
        .await
        .unwrap();
    assert_eq!(
        app.rpc("labs.create", json!({"name": "campus"}))
            .await
            .unwrap_err()
            .code,
        "conflict"
    );
    // Without a name the example's own is made unique.
    let second = app
        .rpc("labs.create", json!({"example": "demo"}))
        .await
        .unwrap();
    assert_eq!(second["name"], "Campus 2");

    let id = first["id"].as_str().unwrap().to_string();
    let before = devices(&app).await;
    let a_device = before
        .iter()
        .find(|device| device["lab"]["lab_id"] == id.as_str())
        .unwrap()
        .clone();
    assert!(a_device["name"].as_str().unwrap().starts_with("Campus/"));

    app.rpc("labs.update", json!({"id": id, "name": "Lab West"}))
        .await
        .unwrap();
    let after = app
        .rpc("devices.get", json!({"id": a_device["id"]}))
        .await
        .unwrap();
    assert!(
        after["name"].as_str().unwrap().starts_with("Lab West/"),
        "{after}"
    );
    assert_eq!(after["site"], "Lab: Lab West");
    assert_eq!(
        after["id"], a_device["id"],
        "the device was renamed, not replaced"
    );
}

#[tokio::test]
async fn nodes_and_links_are_edited_while_the_lab_is_stopped() {
    let app = app();
    let lab = app
        .rpc("labs.create", json!({"name": "Build"}))
        .await
        .unwrap();
    let id = lab["id"].as_str().unwrap().to_string();

    let router = app
        .rpc(
            "labs.node.add",
            json!({"lab_id": id, "model": "router", "name": "R1", "x": 100, "y": 100}),
        )
        .await
        .unwrap();
    assert_eq!(router["name"], "R1");
    let switch = app
        .rpc(
            "labs.node.add",
            json!({"lab_id": id, "model": "switch", "x": 300, "y": 100}),
        )
        .await
        .unwrap();
    assert_eq!(switch["name"], "SW1", "the simulator names it");
    app.rpc(
        "labs.node.add",
        json!({"lab_id": id, "model": "pc", "name": "PC1", "x": 300, "y": 300}),
    )
    .await
    .unwrap();

    // A missing interface picks a free one that suits the cable.
    let link = app
        .rpc(
            "labs.link.add",
            json!({"lab_id": id, "a": {"node": "R1"}, "b": {"node": "SW1", "interface": "Gi0/1"}}),
        )
        .await
        .unwrap();
    assert_eq!(link["a_if"], "GigabitEthernet0/0");
    assert_eq!(link["b_if"], "GigabitEthernet0/1");
    assert_eq!(
        app.rpc(
            "labs.link.add",
            json!({"lab_id": id, "a": {"node": "R1", "interface": "Gi0/0"}, "b": {"node": "SW1"}}),
        )
        .await
        .unwrap_err()
        .code,
        "conflict",
        "the interface is already cabled"
    );
    let same = app
        .rpc(
            "labs.link.add",
            json!({"lab_id": id, "a": {"node": "R1"}, "b": {"node": "R1"}}),
        )
        .await
        .unwrap_err();
    assert!(same.message.contains("two different nodes"), "{same:?}");

    // Moving is quiet and persistent.
    app.rpc(
        "labs.node.move",
        json!({"lab_id": id, "positions": {"PC1": {"x": 420.0, "y": 360.0}}}),
    )
    .await
    .unwrap();
    let view = app.rpc("labs.get", json!({ "id": id })).await.unwrap();
    let pc = view["snapshot"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["name"] == "PC1")
        .unwrap();
    assert_eq!(
        (pc["x"].as_f64(), pc["y"].as_f64()),
        (Some(420.0), Some(360.0))
    );

    // The devices exist for all three nodes, and a removal takes one away.
    assert_eq!(devices(&app).await.len(), 3);
    app.rpc("labs.node.remove", json!({"lab_id": id, "node": "PC1"}))
        .await
        .unwrap();
    assert_eq!(devices(&app).await.len(), 2);
    let links = app.rpc("labs.links", json!({"lab_id": id})).await.unwrap();
    let links = links.as_array().unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["a"]["node"], "R1");
    assert!(links[0]["a"]["device_id"].is_string());
    assert_eq!(links[0]["up"], false, "a stopped lab has nothing up");

    app.rpc(
        "labs.link.remove",
        json!({"lab_id": id, "link": links[0]["link"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        app.rpc("labs.links", json!({"lab_id": id}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn import_and_export_round_trip() {
    let app = app();
    let created = app
        .rpc("labs.create", json!({"example": "ospf_triangle"}))
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let yaml = app
        .rpc("labs.export", json!({"id": id, "format": "yaml"}))
        .await
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    let json_text = app
        .rpc("labs.export", json!({"id": id, "format": "json"}))
        .await
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();

    let from_yaml = app.rpc("labs.import", json!({"spec": yaml})).await.unwrap();
    let from_json = app
        .rpc(
            "labs.import",
            json!({"spec": json_text, "name": "From JSON"}),
        )
        .await
        .unwrap();
    assert_eq!(from_json["name"], "From JSON");
    assert_eq!(
        from_yaml["snapshot"]["nodes"].as_array().unwrap().len(),
        created["snapshot"]["nodes"].as_array().unwrap().len()
    );
    assert_eq!(
        from_json["snapshot"]["links"].as_array().unwrap().len(),
        created["snapshot"]["links"].as_array().unwrap().len()
    );

    // A specification the simulator cannot load is refused, and says where.
    let broken = app
        .rpc(
            "labs.import",
            json!({"spec": {"name": "Broken", "nodes": [{"name": "R1", "model": "router"}],
                            "links": [{"a": {"node": "R1", "interface": "Gi0/0"},
                                       "b": {"node": "Ghost", "interface": "Gi0/0"}}]}}),
        )
        .await
        .unwrap_err();
    assert!(broken.message.contains("Ghost"), "{broken:?}");
    let bad_interface = app
        .rpc(
            "labs.import",
            json!({"spec": {"name": "Broken2", "nodes": [{"name": "R1", "model": "router"},
                                                         {"name": "R2", "model": "router"}],
                            "links": [{"a": {"node": "R1", "interface": "Gi9/9"},
                                       "b": {"node": "R2", "interface": "Gi0/0"}}]}}),
        )
        .await
        .unwrap_err();
    assert!(
        bad_interface.message.contains("Gi9/9") && bad_interface.message.contains("R1"),
        "{bad_interface:?}"
    );
    assert_eq!(
        app.rpc("labs.import", json!({"spec": "not a lab"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn duplicating_copies_the_saved_state_only() {
    let app = app();
    let created = app
        .rpc("labs.create", json!({"example": "basic_switching"}))
        .await
        .unwrap();
    let id = created["id"].as_str().unwrap().to_string();
    let copy = app
        .rpc("labs.duplicate", json!({"id": id, "name": "Copy"}))
        .await
        .unwrap();
    assert_eq!(copy["name"], "Copy");
    assert_ne!(copy["id"], created["id"]);
    assert_eq!(
        copy["devices"].as_object().unwrap().len(),
        created["devices"].as_object().unwrap().len()
    );
    // Both labs' devices exist side by side with distinct names.
    let all = devices(&app).await;
    assert_eq!(all.len(), 2 * created["devices"].as_object().unwrap().len());
}

#[tokio::test]
async fn only_so_many_labs_run_at_once() {
    let app = app();
    fast_ticks(&app).await;
    app.rpc(
        "settings.set",
        json!({"section": "labs", "value": {"max_running": 2}}),
    )
    .await
    .unwrap();
    let mut ids = Vec::new();
    for n in 0..3 {
        let lab = app
            .rpc("labs.create", json!({"name": format!("Lab {n}")}))
            .await
            .unwrap();
        ids.push(lab["id"].as_str().unwrap().to_string());
    }
    app.rpc("labs.start", json!({"id": ids[0]})).await.unwrap();
    app.rpc("labs.start", json!({"id": ids[1]})).await.unwrap();
    let refused = app
        .rpc("labs.start", json!({"id": ids[2]}))
        .await
        .unwrap_err();
    assert_eq!(refused.code, "conflict");
    assert!(refused.message.contains("2 labs"), "{refused:?}");
    // Starting one that already runs is not a refusal.
    app.rpc("labs.start", json!({"id": ids[0]})).await.unwrap();
    app.rpc("labs.stop", json!({"id": ids[0]})).await.unwrap();
    app.rpc("labs.start", json!({"id": ids[2]})).await.unwrap();
}

/// The lab's simulated clock, in microseconds.
async fn sim_time(app: &nocturne_app::App, id: &str) -> u64 {
    app.rpc("labs.get", json!({ "id": id })).await.unwrap()["sim_time"]
        .as_u64()
        .expect("a simulated time")
}

/// How far the lab's clock moves in `window` of real time at `speed`.
async fn clock_advance(app: &nocturne_app::App, id: &str, speed: u64, window: Duration) -> u64 {
    app.rpc("labs.speed", json!({"id": id, "speed": speed}))
        .await
        .expect("the speed is set");
    let before = sim_time(app, id).await;
    tokio::time::sleep(window).await;
    sim_time(app, id).await - before
}

#[tokio::test]
async fn speed_drives_the_clock_and_zero_pauses_it() {
    let app = app();
    fast_ticks(&app).await;
    let view = start_example(&app, "basic_switching", "Clock").await;
    let id = view["id"].as_str().unwrap().to_string();

    app.rpc("labs.speed", json!({"id": id, "speed": 0}))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let paused = app.rpc("labs.get", json!({ "id": id })).await.unwrap()["sim_time"]
        .as_u64()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let still = app.rpc("labs.get", json!({ "id": id })).await.unwrap()["sim_time"]
        .as_u64()
        .unwrap();
    assert_eq!(paused, still, "a paused lab's clock does not move");

    // `speed` is a multiple of real time, and the honest way to assert a
    // multiple is to compare two speeds against each other rather than one
    // speed against a wall clock. A tick advances the simulator by
    // `tick_ms × speed` microseconds, and missed ticks are delayed rather
    // than caught up (`runtime::ticker`), so starving the tick task drags
    // *any* single speed towards real time: at 16× a runner that gives the
    // task an eighth of the turns it asked for looks exactly like 2×, and a
    // test comparing one speed against the clock would be measuring the
    // runner. Two windows taken moments apart on the same machine are
    // starved alike, so the ratio between them survives a busy one.
    const WINDOW: Duration = Duration::from_millis(500);
    let at_one = clock_advance(&app, &id, 1, WINDOW).await;
    let at_sixteen = clock_advance(&app, &id, 16, WINDOW).await;
    assert!(
        at_one > 0,
        "a speed above zero restarts the clock, and at 1× it moved {at_one} µs in {WINDOW:?}"
    );
    assert!(
        at_sixteen > at_one * 4,
        "16× must pass several times the simulated time 1× does in the same window, \
         and passed {at_sixteen} µs against {at_one} µs"
    );
    assert_eq!(
        app.rpc("labs.speed", json!({"id": id, "speed": 100}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn the_catalog_is_what_the_simulator_offers() {
    let app = app();
    let catalog = app.rpc("labs.catalog", Value::Null).await.unwrap();
    let catalog = catalog.as_array().unwrap();
    assert_eq!(catalog.len(), nocturne_netsim::catalog().len());
    let router = catalog.iter().find(|info| info["id"] == "router").unwrap();
    assert_eq!(router["cli"], "ios");
    assert_eq!(router["interfaces"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn a_lab_that_is_not_running_says_what_to_do() {
    let app = app();
    let lab = app
        .rpc("labs.create", json!({"name": "Idle"}))
        .await
        .unwrap();
    let id = lab["id"].as_str().unwrap().to_string();
    for (method, params) in [
        ("labs.probe", json!({"lab_id": id, "from": "a", "to": "b"})),
        ("labs.capture", json!({"lab_id": id, "on": true})),
        ("labs.node.tables", json!({"lab_id": id, "node": "a"})),
    ] {
        let err = app.rpc(method, params).await.unwrap_err();
        assert_eq!(err.code, "unavailable", "{method}");
        assert!(err.message.contains("not running"), "{method}: {err:?}");
    }
    until(Duration::from_secs(1), "nothing", || true).await;
}

#[tokio::test]
async fn metrics_notes_and_the_link_list_across_labs() {
    let app = app();
    fast_ticks(&app).await;
    let first = start_example(&app, "ospf_triangle", "Alpha").await;
    let alpha = first["id"].as_str().unwrap().to_string();
    let second = app
        .rpc(
            "labs.create",
            json!({"name": "Beta", "example": "basic_switching"}),
        )
        .await
        .unwrap();
    let beta = second["id"].as_str().unwrap().to_string();

    // Live counters come from the running simulator.
    let metrics = app
        .rpc("labs.node.metrics", json!({"lab_id": alpha, "node": "R1"}))
        .await
        .unwrap();
    assert_eq!(metrics["hostname"], "R1");
    assert!(metrics["interfaces"].as_array().unwrap().len() >= 4);
    assert!(metrics["mem_total"].as_u64().unwrap() > 0);
    let uplink = metrics["interfaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|interface| interface["name"] == "GigabitEthernet0/0")
        .unwrap();
    assert_eq!(uplink["oper_up"], true);
    assert!(uplink["speed_bps"].as_u64().unwrap() > 0);

    // Canvas notes are part of the lab and come back with it.
    app.rpc(
        "labs.update",
        json!({"id": alpha, "description": "My rehearsal",
               "notes": [{"x": 1.0, "y": 2.0, "w": 100.0, "h": 50.0, "text": "cut here", "color": "#fff"}]}),
    )
    .await
    .unwrap();
    let view = app.rpc("labs.get", json!({ "id": alpha })).await.unwrap();
    assert_eq!(view["description"], "My rehearsal");
    assert_eq!(view["snapshot"]["notes"][0]["text"], "cut here");
    assert_eq!(view["snapshot"]["name"], "Alpha");

    // Every lab's cables at once: the topology feature's view of the labs.
    let all = app.rpc("labs.links", json!({})).await.unwrap();
    let rows = all.as_array().unwrap();
    assert!(
        rows.iter()
            .any(|row| row["lab"] == "Alpha" && row["running"] == true)
    );
    assert!(
        rows.iter()
            .any(|row| row["lab"] == "Beta" && row["running"] == false)
    );
    assert!(
        rows.iter()
            .all(|row| row["a"]["device_id"].is_string() || row["a"]["node"] == "Internet"),
        "{rows:?}"
    );
    assert_eq!(
        rows.iter()
            .filter(|row| row["lab_id"] == beta.as_str())
            .count(),
        second["snapshot"]["links"].as_array().unwrap().len()
    );
}

#[tokio::test]
async fn editing_a_running_lab_keeps_the_inventory_in_step() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "basic_switching", "Live edits").await;
    let id = lab["id"].as_str().unwrap().to_string();
    let before = devices(&app).await.len();

    app.rpc(
        "labs.node.add",
        json!({"lab_id": id, "model": "router", "name": "R5", "x": 50, "y": 50}),
    )
    .await
    .unwrap();
    let view = app.rpc("labs.get", json!({ "id": id })).await.unwrap();
    let new_device = view["devices"]["R5"]
        .as_str()
        .unwrap_or_else(|| panic!("a device for R5: {}", view["devices"]))
        .to_string();
    assert_eq!(devices(&app).await.len(), before + 1);
    // The lab was written back as well, so a restart keeps the new node.
    app.rpc("labs.stop", json!({ "id": id })).await.unwrap();
    app.rpc("labs.start", json!({ "id": id })).await.unwrap();
    assert!(app.rpc("labs.get", json!({ "id": id })).await.unwrap()["devices"]["R5"].is_string());

    app.rpc("labs.node.remove", json!({"lab_id": id, "node": "R5"}))
        .await
        .unwrap();
    assert_eq!(devices(&app).await.len(), before);
    assert_eq!(
        app.rpc("devices.get", json!({"id": new_device}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}
