//! Every method over the wire, with the shapes the UI is written against —
//! and every way each one can refuse.

use std::time::Duration;

use nocturne_app::App;
use nocturne_drivers::model::Kind;
use serde_json::{Value, json};

use super::*;
use crate::history;

async fn call(app: &App, method: &str, params: Value) -> Value {
    app.rpc(method, params)
        .await
        .unwrap_or_else(|err| panic!("{method} failed: {err}"))
}

#[tokio::test]
async fn topology_get_takes_a_view_a_layer_and_a_filter() {
    let campus = campus();
    let app = &campus.app;

    let graph = call(app, "topology.get", Value::Null).await;
    assert_eq!(graph["view"], "default");
    assert_eq!(graph["layer"], "physical");
    assert_eq!(graph["stats"]["devices"], 26);
    assert!(graph["generated_at"].as_i64().unwrap() > 0);
    assert!(graph["data_age"]["newest_at"].as_i64().is_some());
    let node = &graph["nodes"][0];
    assert!(node["id"].is_string() && node["name"].is_string());
    assert!(node["x"].is_number() && node["y"].is_number());
    assert_eq!(node["pinned"], false);
    assert_eq!(node["unmanaged"], false);
    let link = &graph["links"][0];
    assert!(link["a"]["node"].is_string() && link["b"]["node"].is_string());
    assert!(["confirmed", "one_sided", "inferred"].contains(&link["confidence"].as_str().unwrap()));

    let l3 = call(app, "topology.get", json!({"layer": "l3"})).await;
    assert_eq!(l3["layer"], "l3");

    let filtered = call(
        app,
        "topology.get",
        json!({"filter": {"site": "North Campus"}}),
    )
    .await;
    assert_eq!(filtered["stats"]["devices"], 12);

    let with_hosts = call(app, "topology.get", json!({"include_endpoints": true})).await;
    assert_eq!(with_hosts["stats"]["endpoints"], 3);

    assert_eq!(
        app.rpc("topology.get", json!({"view": "nope"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        app.rpc("topology.get", json!({"layer": "l7"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn views_are_saved_listed_and_deleted() {
    let campus = campus();
    let app = &campus.app;

    let views = call(app, "topology.views", Value::Null).await;
    assert_eq!(views.as_array().unwrap().len(), 1);
    assert_eq!(views[0]["id"], "default");

    let saved = call(
        app,
        "topology.view_save",
        json!({
            "name": "North only",
            "layer": "l3",
            "filter": {"site": "North Campus"},
            "include_endpoints": true,
        }),
    )
    .await;
    let id = saved["id"].as_str().expect("an id").to_string();
    assert_eq!(saved["name"], "North only");
    assert!(saved["created_at"].as_i64().unwrap() > 0);

    // A saved view supplies the defaults for `topology.get`.
    let graph = call(app, "topology.get", json!({"view": id})).await;
    assert_eq!(graph["layer"], "l3");
    assert_eq!(graph["view"], id);
    assert_eq!(graph["stats"]["devices"], 12);
    // And an explicit parameter still wins over it.
    let overridden = call(
        app,
        "topology.get",
        json!({"view": id, "layer": "physical"}),
    )
    .await;
    assert_eq!(overridden["layer"], "physical");

    // Saving again with the same id is an edit, not a second view.
    let edited = call(
        app,
        "topology.view_save",
        json!({"id": id, "name": "North campus", "layer": "physical"}),
    )
    .await;
    assert_eq!(edited["created_at"], saved["created_at"]);
    assert_eq!(
        call(app, "topology.views", Value::Null)
            .await
            .as_array()
            .unwrap()
            .len(),
        2
    );

    assert_eq!(
        app.rpc("topology.view_save", json!({"name": "  "}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        app.rpc("topology.view_delete", json!({"id": "default"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        call(app, "topology.view_delete", json!({"id": id})).await["deleted"],
        true
    );
    assert_eq!(
        call(app, "topology.view_delete", json!({"id": "gone"})).await["deleted"],
        false
    );
}

#[tokio::test]
async fn positions_are_saved_and_the_automatic_layout_respects_pins() {
    let campus = campus();
    let app = &campus.app;
    let pinned = campus.core[0].clone();

    let saved = call(
        app,
        "topology.layout.save",
        json!({"positions": {pinned.clone(): {"x": 11.0, "y": 22.0, "pinned": true}}}),
    )
    .await;
    assert_eq!(saved["view"], "default");
    assert_eq!(saved["positions"][&pinned]["x"], 11.0);

    // The graph shows the saved place, and everything else is placed around it.
    let graph = call(app, "topology.get", Value::Null).await;
    let node = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == json!(pinned))
        .expect("the pinned node");
    assert_eq!((&node["x"], &node["y"]), (&json!(11.0), &json!(22.0)));
    assert_eq!(node["pinned"], true);
    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["x"].is_number())
    );

    let laid_out = call(app, "topology.autolayout", json!({"algorithm": "layered"})).await;
    assert_eq!(laid_out["algorithm"], "layered");
    assert_eq!(laid_out["positions"][&pinned]["x"], 11.0);
    assert_eq!(laid_out["positions"].as_object().unwrap().len(), 27);
    // It was stored, so asking for the graph again gives the same picture.
    let after = call(app, "topology.get", Value::Null).await;
    for (one, other) in graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .zip(after["nodes"].as_array().unwrap())
    {
        assert_eq!(one["id"], other["id"]);
    }

    let radial = call(
        app,
        "topology.autolayout",
        json!({"algorithm": "radial", "root": "CORE2", "save": false}),
    )
    .await;
    assert_eq!(radial["positions"][&campus.core[1]]["x"], 0.0);

    assert_eq!(
        app.rpc("topology.autolayout", json!({"root": "nowhere"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
    assert_eq!(
        app.rpc("topology.layout.save", json!({"positions": {}}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn topology_neighbors_is_the_connected_to_table() {
    let campus = campus();
    let app = &campus.app;
    let table = call(app, "topology.neighbors", json!({"device_id": "DIST1"})).await;
    assert_eq!(table["device"], "DIST1");
    let rows = table["rows"].as_array().expect("rows");
    // Two cores, nine single-homed access switches and one port-channel.
    assert_eq!(rows.len(), 12, "{rows:#?}");
    let row = &rows[0];
    assert!(row["local_interface"].is_string());
    assert!(row["peer"]["name"].is_string());
    assert!(row["peer_interface"].is_string());
    assert_eq!(row["confidence"], "confirmed");
    assert_eq!(row["status"], "up");
    assert!(row["speed_bps"].as_u64().unwrap() > 0);
    assert!(rows.iter().any(|row| row["aggregate"].is_string()));
    // Rows are ordered by local interface, the way a port list reads.
    let interfaces: Vec<&str> = rows
        .iter()
        .map(|row| row["local_interface"].as_str().unwrap())
        .collect();
    assert!(
        interfaces[0].starts_with("GigabitEthernet"),
        "{interfaces:?}"
    );

    // An access switch sees its uplinks, and its host once asked for.
    let with_hosts = call(
        app,
        "topology.neighbors",
        json!({"id": "ACC02", "include_endpoints": true}),
    )
    .await;
    assert!(
        with_hosts["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["kind"] == "mac")
    );

    assert_eq!(
        app.rpc("topology.neighbors", json!({"device_id": "nope"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn path_and_locate_are_reachable_over_the_wire() {
    let app = app();
    let router = device(&app, "R1", "10.1.10.1", "core", "HQ");
    let switch = device(&app, "SW1", "10.1.10.2", "access", "HQ");
    store(
        &app,
        &router,
        vec![
            (Kind::Interfaces, vec![routed("Vlan10", "10.1.10.1/24")]),
            (
                Kind::Routes,
                vec![route("10.1.10.0/24", "connected", "", "Vlan10")],
            ),
            (
                Kind::Arp,
                vec![arp_entry("10.1.10.55", "00:1b:21:aa:00:55", "Vlan10")],
            ),
            (
                Kind::Neighbors,
                vec![neighbor("Gi1/0/1", "SW1", "Gi1/0/48", "lldp")],
            ),
        ],
    );
    store(
        &app,
        &switch,
        vec![
            (
                Kind::Neighbors,
                vec![neighbor("Gi1/0/48", "R1", "Gi1/0/1", "lldp")],
            ),
            (
                Kind::Interfaces,
                vec![trunk("Gi1/0/48", 1_000_000_000), access_port("Gi1/0/5", 10)],
            ),
            (
                Kind::MacTable,
                vec![mac_entry("00:1b:21:aa:00:55", "Gi1/0/5", 10)],
            ),
        ],
    );

    let found = call(
        &app,
        "topology.path",
        json!({"from": "R1", "to": "10.1.10.55"}),
    )
    .await;
    assert_eq!(found["complete"], true);
    assert_eq!(found["hops"].as_array().unwrap().len(), 3);
    assert_eq!(found["hops"][0]["via"], "connected");
    assert_eq!(found["hops"][2]["via"], "mac");
    assert_eq!(found["destination"], "10.1.10.55");

    let located = call(&app, "locate.host", json!({"query": "10.1.10.55"})).await;
    assert_eq!(located["mac"], "00:1b:21:aa:00:55");
    assert_eq!(located["best"], 0);
    assert_eq!(located["locations"][0]["device"], "SW1");
    assert_eq!(located["locations"][0]["interface"], "GigabitEthernet1/0/5");
    assert_eq!(located["locations"][0]["kind"], "access");
    assert!(located["how"].as_array().unwrap().len() >= 3);

    assert_eq!(
        app.rpc("topology.path", json!({"from": "R1"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
    assert_eq!(
        app.rpc("locate.host", json!({})).await.unwrap_err().code,
        "invalid"
    );
}

#[tokio::test]
async fn export_returns_a_file_name_and_the_text() {
    let campus = campus();
    let app = &campus.app;
    for (format, extension, needle) in [
        ("svg", "svg", "<svg"),
        ("drawio", "drawio", "<mxfile"),
        ("mermaid", "mmd", "graph TD"),
        ("dot", "dot", "graph topology {"),
        ("json", "json", "\"nodes\""),
    ] {
        let exported = call(app, "topology.export", json!({"format": format})).await;
        assert_eq!(exported["format"], format);
        assert_eq!(
            exported["filename"],
            format!("topology-physical.{extension}")
        );
        assert!(
            exported["text"].as_str().unwrap().contains(needle),
            "the {format} export does not look like one"
        );
    }
    let l3 = call(
        app,
        "topology.export",
        json!({"format": "svg", "layer": "l3"}),
    )
    .await;
    assert_eq!(l3["filename"], "topology-l3.svg");
    assert_eq!(
        app.rpc("topology.export", json!({"format": "pdf"}))
            .await
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn manual_links_are_listed_saved_and_deleted() {
    let app = app();
    let _switch = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    assert!(
        call(&app, "topology.manual.list", Value::Null)
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    let link = call(
        &app,
        "topology.manual.save",
        json!({
            "a": {"device": "SW1", "interface": "Gi0/24"},
            "b": {"name": "MPLS circuit 44/118", "kind": "cloud"},
            "label": "100 Mbit/s EFM",
        }),
    )
    .await;
    let id = link["id"].as_str().expect("an id").to_string();
    assert_eq!(link["a"]["interface"], "GigabitEthernet0/24");
    assert_eq!(
        call(&app, "topology.manual.list", Value::Null)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let graph = call(&app, "topology.get", Value::Null).await;
    assert_eq!(graph["stats"]["links"], 1);
    assert_eq!(graph["stats"]["unmanaged"], 1);
    assert!(
        graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["kind"] == "cloud")
    );

    // Both ends need to be something.
    assert_eq!(
        app.rpc(
            "topology.manual.save",
            json!({"a": {"device": "SW1"}, "b": {}})
        )
        .await
        .unwrap_err()
        .code,
        "invalid"
    );
    assert_eq!(
        app.rpc(
            "topology.manual.save",
            json!({"a": {"device": "nope"}, "b": {"name": "x"}})
        )
        .await
        .unwrap_err()
        .code,
        "not_found"
    );
    assert_eq!(
        call(&app, "topology.manual.delete", json!({"id": id})).await["deleted"],
        true
    );
    assert_eq!(
        app.rpc("topology.manual.delete", json!({"id": "gone"}))
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn topology_changes_compares_stored_snapshots() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let two = device(&app, "SW2", "10.0.0.2", "access", "HQ");

    let empty = call(&app, "topology.changes", json!({"since": 0})).await;
    assert_eq!(empty["snapshots"], 0);
    assert!(
        empty["notes"][0]
            .as_str()
            .unwrap()
            .contains("no topology snapshot has been stored yet")
    );

    store(
        &app,
        &one,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/1", "SW2", "Gi0/2", "lldp")],
        )],
    );
    store(
        &app,
        &two,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/2", "SW1", "Gi0/1", "lldp")],
        )],
    );
    assert!(history::record(&app).await.unwrap(), "the first snapshot");
    assert!(
        !history::record(&app).await.unwrap(),
        "nothing changed, so nothing is stored"
    );
    let first_at = history::snapshots(&app).unwrap()[0].at;

    // The cable is pulled and a third switch appears.
    store(&app, &one, vec![(Kind::Neighbors, vec![])]);
    store(&app, &two, vec![(Kind::Neighbors, vec![])]);
    let three = device(&app, "SW3", "10.0.0.3", "access", "HQ");
    assert!(history::record(&app).await.unwrap());

    let changes = call(&app, "topology.changes", json!({"since": first_at})).await;
    assert_eq!(changes["snapshots"], 2);
    assert_eq!(changes["from_at"], first_at);
    assert!(changes["to_at"].as_i64().unwrap() > first_at);
    assert_eq!(changes["links_removed"].as_array().unwrap().len(), 1);
    assert_eq!(changes["links_added"].as_array().unwrap().len(), 0);
    let removed = &changes["links_removed"][0];
    assert_eq!(removed["kind"], "lldp");
    assert_eq!(removed["confidence"], "confirmed");
    let names = [
        removed["a_name"].as_str().unwrap(),
        removed["b_name"].as_str().unwrap(),
    ];
    assert!(
        names.contains(&"SW1") && names.contains(&"SW2"),
        "{names:?}"
    );
    assert_eq!(changes["nodes_added"].as_array().unwrap().len(), 1);
    assert_eq!(changes["nodes_added"][0]["name"], "SW3");
    assert_eq!(changes["nodes_added"][0]["device_id"], three);
    assert_eq!(changes["nodes_removed"].as_array().unwrap().len(), 0);

    // Asking about now says nothing has changed since.
    let recent = call(
        &app,
        "topology.changes",
        json!({"since": nocturne_app::util::now_ms() + 1_000}),
    )
    .await;
    assert!(recent["notes"].as_array().unwrap().iter().any(|note| {
        note.as_str()
            .unwrap()
            .contains("has not changed since then")
    }));
    // And asking about before the oldest snapshot says so rather than lying.
    let ancient = call(&app, "topology.changes", json!({"since": 1})).await;
    assert!(
        ancient["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|note| note.as_str().unwrap().contains("no snapshot is as old as")),
        "{:#?}",
        ancient["notes"]
    );
}

#[tokio::test(start_paused = true)]
async fn a_collection_redraws_the_graph_and_announces_it() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let two = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    let mut events = app.subscribe();

    store(
        &app,
        &one,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/1", "SW2", "Gi0/2", "lldp")],
        )],
    );
    store(
        &app,
        &two,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/2", "SW1", "Gi0/1", "lldp")],
        )],
    );

    let announced = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let event = events.recv().await.expect("the bus stays open");
            if event.topic == "topology.changed" && event.payload["reason"] == "collect" {
                return event;
            }
        }
    })
    .await
    .expect("`topology.changed` arrives after a collection");
    assert_eq!(announced.payload["reason"], "collect");

    // Both collections were folded into one redraw, and a snapshot was kept.
    let snapshots = history::snapshots(&app).unwrap();
    assert!(!snapshots.is_empty());
    let latest = snapshots.last().unwrap();
    assert_eq!(latest.nodes.len(), 2);
    assert_eq!(latest.links.len(), 1);
}

#[tokio::test]
async fn the_settings_section_is_declared_with_its_defaults() {
    let app = app();
    let settings = call(&app, "settings.get", json!({"section": "topology"})).await;
    assert_eq!(settings["stale_after_hours"], 24);
    assert_eq!(settings["endpoint_max_macs"], 3);
    assert_eq!(settings["fold_port_channels"], true);
    assert_eq!(settings["default_algorithm"], "layered");

    // Changing a setting changes what the graph does.
    let switch = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    store(
        &app,
        &switch,
        vec![
            (
                Kind::Interfaces,
                vec![access_port("Gi1/0/5", 10), access_port("Gi1/0/6", 10)],
            ),
            (
                Kind::MacTable,
                vec![
                    mac_entry("00:1b:21:aa:00:01", "Gi1/0/5", 10),
                    mac_entry("00:1b:21:aa:00:02", "Gi1/0/6", 10),
                    mac_entry("00:1b:21:aa:00:03", "Gi1/0/6", 10),
                ],
            ),
        ],
    );
    let before = call(&app, "topology.get", json!({"include_endpoints": true})).await;
    assert_eq!(before["stats"]["endpoints"], 3);
    call(
        &app,
        "settings.set",
        json!({"section": "topology", "value": {"endpoint_max_macs": 1}}),
    )
    .await;
    let after = call(&app, "topology.get", json!({"include_endpoints": true})).await;
    assert_eq!(
        after["stats"]["endpoints"], 1,
        "the port with two MACs is no longer an edge port"
    );
}

#[tokio::test]
async fn the_other_features_fill_in_lab_links_utilisation_and_alerts() {
    // The lab, monitor and alert features are separate crates that may or may
    // not be in a build. When they are, the graph uses what they know; when
    // they are not, it carries on without them (every other test here).
    let mut builder = builder();
    builder.router.add(
        "labs.links",
        |app: App, _: nocturne_app::NoParams| async move {
            let one = nocturne_app::inventory::find(&app, "SW1")?;
            let two = nocturne_app::inventory::find(&app, "SW2")?;
            Ok(json!([{
                "lab_id": "testlab",
                "lab": "Test lab",
                "link": 1,
                "a": {"device_id": one.id, "node": "SW1", "interface": "Gi0/1"},
                "b": {"device_id": two.id, "node": "SW2", "interface": "Gi0/2"},
                "up": true,
                "running": true,
            }]))
        },
    );
    builder.router.add(
        "monitor.interfaces",
        |app: App, _: nocturne_app::NoParams| async move {
            let one = nocturne_app::inventory::find(&app, "SW1")?;
            Ok(json!([
                {"device_id": one.id, "name": "GigabitEthernet0/1", "util_pct": 42.5},
            ]))
        },
    );
    builder.router.add(
        "alerts.list",
        |app: App, _: nocturne_app::NoParams| async move {
            let one = nocturne_app::inventory::find(&app, "SW1")?;
            Ok(json!([
                {"device_id": one.id, "severity": "warning"},
                {"device_id": one.id, "severity": "critical"},
            ]))
        },
    );
    let app = builder.build();

    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let two = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    store(
        &app,
        &one,
        vec![
            (
                Kind::Neighbors,
                vec![neighbor("Gi0/1", "SW2", "Gi0/2", "lldp")],
            ),
            (Kind::Interfaces, vec![trunk("Gi0/1", 1_000_000_000)]),
        ],
    );
    store(
        &app,
        &two,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/2", "SW1", "Gi0/1", "lldp")],
        )],
    );

    let graph = call(&app, "topology.get", Value::Null).await;
    let links = graph["links"].as_array().expect("links");
    assert_eq!(
        links.len(),
        1,
        "the lab's own wiring replaces the reported link"
    );
    assert_eq!(links[0]["kind"], "lab");
    assert_eq!(links[0]["confidence"], "confirmed");
    assert_eq!(links[0]["status"], "up");
    assert_eq!(links[0]["label"], "lab Test lab");
    assert_eq!(links[0]["util_pct"], 42.5);
    assert_eq!(links[0]["speed_bps"], 1_000_000_000u64);

    let node = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == json!(one))
        .expect("SW1");
    assert_eq!(node["alerts"], 2);
    let other = graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"] == json!(two))
        .expect("SW2");
    assert!(other["alerts"].is_null(), "no alerts is not zero alerts");
}

#[tokio::test]
async fn a_lab_feature_that_fails_is_said_to_have_failed_and_the_reported_links_are_drawn() {
    let mut builder = builder();
    builder.router.add(
        "labs.links",
        |_: App, _: nocturne_app::NoParams| async move {
            Err::<Value, nocturne_app::AppError>(nocturne_app::AppError::unavailable(
                "the lab store is locked",
            ))
        },
    );
    let app = builder.build();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let two = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    store(
        &app,
        &one,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/1", "SW2", "Gi0/2", "lldp")],
        )],
    );
    store(
        &app,
        &two,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/2", "SW1", "Gi0/1", "lldp")],
        )],
    );

    let graph = call(&app, "topology.get", Value::Null).await;
    let links = graph["links"].as_array().expect("links");
    assert_eq!(links.len(), 1, "what the devices report is still drawn");
    assert_eq!(links[0]["kind"], "lldp");
    let warnings: Vec<&str> = graph["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        warnings.iter().any(|warning| {
            warning.starts_with("the lab feature's wiring could not be read")
                && warning.contains("the lab store is locked")
        }),
        "{warnings:?}"
    );
}
