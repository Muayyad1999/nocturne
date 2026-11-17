//! The campus: probes, the connectivity matrix, faults, capture, traces,
//! flows and the simulator's syslog landing in the log store.

mod common;

use std::time::Duration;

use common::*;
use nocturne_app::App;
use nocturne_app::store::LogQuery;
use serde_json::{Value, json};

async fn campus(app: &App) -> Value {
    fast_ticks(app).await;
    start_example(app, "demo", "Campus").await
}

async fn probe(app: &App, lab: &str, from: &str, to: &str) -> Value {
    app.rpc(
        "labs.probe",
        json!({"lab_id": lab, "from": from, "to": to, "kind": "ping"}),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn the_campus_is_reachable_until_a_fault_breaks_it() {
    let app = app();
    let lab = campus(&app).await;
    let id = lab["id"].as_str().unwrap().to_string();

    // What the example itself promises: every host reaches the Internet.
    for host in ["Staff-PC1", "Staff-Laptop", "Guest-Laptop1", "SRV1"] {
        let result = probe(&app, &id, host, "8.8.8.8").await;
        assert_eq!(result["ok"], true, "{host} → 8.8.8.8: {result}");
        assert!(
            result["hops"].as_array().unwrap().len() >= 3,
            "{host} goes through the access switch, the distribution and the edge: {result}"
        );
    }
    // And the management station reaches every network device.
    for device in ["EDGE", "DS1", "DS2", "AS1", "AS2", "AS3"] {
        let result = probe(&app, &id, "NMS", device).await;
        assert_eq!(result["ok"], true, "NMS → {device}: {result}");
    }
    // A probe that cannot work says why rather than pretending.
    let nowhere = probe(&app, &id, "Staff-PC1", "203.0.113.200").await;
    assert_eq!(nowhere["ok"], false);
    assert!(!nowhere["reason"].as_str().unwrap().is_empty(), "{nowhere}");
    let tcp = app
        .rpc(
            "labs.probe",
            json!({"lab_id": id, "from": "Staff-PC1", "to": "SRV1", "kind": "tcp", "port": 80}),
        )
        .await
        .unwrap();
    assert_eq!(tcp["ok"], true, "SRV1 serves http: {tcp}");

    // The matrix over the user hosts, before and after a cut.
    let hosts = json!(["Staff-PC1", "Staff-PC2", "Staff-Laptop", "SRV1", "NMS"]);
    let before = app
        .rpc(
            "labs.connectivity",
            json!({"lab_id": id, "nodes": hosts.clone()}),
        )
        .await
        .unwrap();
    assert_eq!(before["nodes"], hosts);
    assert_eq!(before["matrix"].as_array().unwrap().len(), 5);
    assert_eq!(
        before["reachable_pct"], 100.0,
        "staff, the server and the NOC all reach each other: {before}"
    );

    // Pull the cable to AS1 and the two staff PCs behind it go dark.
    let links = app.rpc("labs.links", json!({"lab_id": id})).await.unwrap();
    let uplinks: Vec<u64> = links
        .as_array()
        .unwrap()
        .iter()
        .filter(|link| {
            (link["a"]["node"] == "AS1" && link["b"]["node"].as_str().unwrap().starts_with("DS"))
                || (link["b"]["node"] == "AS1"
                    && link["a"]["node"].as_str().unwrap().starts_with("DS"))
        })
        .map(|link| link["link"].as_u64().unwrap())
        .collect();
    assert_eq!(uplinks.len(), 2, "AS1 is dual-homed: {links}");
    let mut faults = Vec::new();
    for link in &uplinks {
        let injected = app
            .rpc(
                "labs.fault.inject",
                json!({"lab_id": id, "fault": {"link_down": {"link": link}}}),
            )
            .await
            .unwrap();
        faults.push(injected["fault_id"].as_u64().unwrap());
    }
    let listed = app
        .rpc("labs.fault.list", json!({"lab_id": id}))
        .await
        .unwrap();
    assert_eq!(listed.as_array().unwrap().len(), 2);

    let after = until_rpc(
        &app,
        Duration::from_secs(10),
        "the matrix to notice the cut",
        "labs.connectivity",
        json!({"lab_id": id, "nodes": hosts.clone()}),
        |value| value["reachable_pct"].as_f64().unwrap_or(100.0) < 100.0,
    )
    .await;
    let cell = &after["matrix"][0][3];
    assert_eq!(cell["ok"], false, "Staff-PC1 → SRV1 after the cut: {after}");
    assert!(
        !cell["reason"].as_str().unwrap().is_empty(),
        "and it says why: {after}"
    );

    // Put the cables back and it recovers.
    for fault in faults {
        app.rpc("labs.fault.clear", json!({"lab_id": id, "fault_id": fault}))
            .await
            .unwrap();
    }
    until_rpc(
        &app,
        Duration::from_secs(20),
        "the campus to recover",
        "labs.connectivity",
        json!({"lab_id": id, "nodes": hosts}),
        |value| value["reachable_pct"] == 100.0,
    )
    .await;
}

#[tokio::test]
async fn capture_and_trace_arrive_in_order() {
    let app = app();
    let lab = campus(&app).await;
    let id = lab["id"].as_str().unwrap().to_string();
    let mut events = app.subscribe();

    app.rpc("labs.capture", json!({"lab_id": id, "on": true}))
        .await
        .unwrap();
    let start = |to: &'static str| {
        let app = app.clone();
        let id = id.clone();
        async move {
            app.rpc(
                "labs.trace",
                json!({"lab_id": id, "from": "Staff-PC1", "to": to, "pdu": "icmp"}),
            )
            .await
            .unwrap()["trace_id"]
                .as_u64()
                .unwrap()
        }
    };
    let wait = |trace_id: u64,
                events: &mut tokio::sync::broadcast::Receiver<nocturne_app::Event>| {
        let id = id.clone();
        let mut events = events.resubscribe();
        async move {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            while tokio::time::Instant::now() < deadline {
                if let Ok(Ok(event)) =
                    tokio::time::timeout(Duration::from_secs(5), events.recv()).await
                    && event.topic == "labs.trace_done"
                    && event.payload["trace_id"] == trace_id
                {
                    assert_eq!(event.payload["lab_id"], id.as_str());
                    return event.payload.clone();
                }
            }
            panic!("no labs.trace_done for trace {trace_id}");
        }
    };

    // The very first packet to an unresolved neighbour is dropped while ARP
    // resolves — exactly as on real gear — and the trace says so in words.
    let first = start("SRV1").await;
    let dropped = wait(first, &mut events).await;
    assert_eq!(dropped["ok"], false, "{dropped}");
    assert!(
        dropped["summary"].as_str().unwrap().contains("ARP"),
        "{dropped}"
    );
    let trace_id = start("SRV1").await;
    let done = wait(trace_id, &mut events).await;
    assert_eq!(done["ok"], true, "{done}");
    assert!(!done["summary"].as_str().unwrap().is_empty());

    // Every hop is in the buffer, in order, with a monotonic sequence.
    let packets = until_rpc(
        &app,
        Duration::from_secs(5),
        "the traced packets",
        "labs.packets",
        json!({"lab_id": id}),
        |value| {
            value["events"]
                .as_array()
                .is_some_and(|rows| rows.len() > 4)
        },
    )
    .await;
    let rows = packets["events"].as_array().unwrap();
    let mut last = 0_u64;
    for row in rows {
        let seq = row["seq"].as_u64().unwrap();
        assert!(
            seq > last,
            "sequence numbers only go up: {seq} after {last}"
        );
        last = seq;
        assert!(!row["summary"].as_str().unwrap().is_empty());
        assert!(!row["decision"].as_str().unwrap().is_empty());
    }
    assert_eq!(packets["next_seq"].as_u64().unwrap(), last + 1);
    let traced: Vec<&Value> = rows
        .iter()
        .filter(|row| row["trace"].as_u64() == Some(trace_id))
        .collect();
    assert!(
        traced.len() >= 2,
        "the traced PDU's own hops: {}",
        rows.len()
    );
    assert_eq!(traced[0]["kind"], "created");
    assert!(
        traced
            .iter()
            .any(|row| row["protocol"] == "ICMP" && row["kind"] == "delivered"),
        "the echo reached SRV1"
    );

    // Paging works from a sequence number.
    let page = app
        .rpc(
            "labs.packets",
            json!({"lab_id": id, "after_seq": rows[0]["seq"], "limit": 2}),
        )
        .await
        .unwrap();
    assert_eq!(page["events"].as_array().unwrap().len(), 2);
    assert_eq!(page["events"][0]["seq"], rows[1]["seq"]);

    // Turning capture off empties the buffer.
    app.rpc("labs.capture", json!({"lab_id": id, "on": false}))
        .await
        .unwrap();
    let empty = app
        .rpc("labs.packets", json!({"lab_id": id}))
        .await
        .unwrap();
    assert_eq!(empty["events"], json!([]));
}

#[tokio::test]
async fn the_simulator_s_syslog_lands_in_the_log_store() {
    let app = app();
    let lab = campus(&app).await;
    let id = lab["id"].as_str().unwrap().to_string();
    let as1 = lab["devices"]["AS1"].as_str().unwrap().to_string();

    let links = app.rpc("labs.links", json!({"lab_id": id})).await.unwrap();
    let uplink = links
        .as_array()
        .unwrap()
        .iter()
        .find(|link| link["a"]["node"] == "AS1" || link["b"]["node"] == "AS1")
        .unwrap()["link"]
        .as_u64()
        .unwrap();
    app.rpc(
        "labs.fault.inject",
        json!({"lab_id": id, "fault": {"link_down": {"link": uplink}}}),
    )
    .await
    .unwrap();

    let store = app.store().clone();
    let device = as1.clone();
    until(Duration::from_secs(10), "a syslog line from AS1", || {
        let entries = store
            .log_query(&LogQuery {
                stream: Some("syslog".into()),
                device_id: Some(device.clone()),
                ..LogQuery::default()
            })
            .unwrap();
        entries
            .iter()
            .any(|entry| entry.source.contains("UPDOWN") && entry.message.contains("down"))
    })
    .await;

    let entries = app
        .store()
        .log_query(&LogQuery {
            stream: Some("syslog".into()),
            device_id: Some(as1.clone()),
            ..LogQuery::default()
        })
        .unwrap();
    let entry = entries
        .iter()
        .find(|entry| entry.source.contains("LINK"))
        .unwrap();
    assert_eq!(entry.device_id.as_deref(), Some(as1.as_str()));
    assert!(entry.severity.unwrap() <= 5, "{entry:?}");
    assert!(
        entry.body.as_ref().unwrap()["line"]
            .as_str()
            .unwrap()
            .contains("%LINK"),
        "the fully formatted line is kept: {entry:?}"
    );
    assert_eq!(entry.body.as_ref().unwrap()["lab_id"], id.as_str());
}

#[tokio::test]
async fn flows_are_listed_added_and_removed() {
    let app = app();
    let lab = campus(&app).await;
    let id = lab["id"].as_str().unwrap().to_string();

    let flows = app
        .rpc("labs.flow.list", json!({"lab_id": id}))
        .await
        .unwrap();
    let flows = flows.as_array().unwrap();
    assert_eq!(flows.len(), 3, "the campus ships three background flows");
    assert!(
        flows
            .iter()
            .any(|flow| flow["flow"]["name"] == "Staff web browsing"),
        "{flows:?}"
    );

    let added = app
        .rpc(
            "labs.flow.add",
            json!({"lab_id": id, "flow": {"name": "Backup", "src": "SRV1", "dst_ip": "8.8.8.8",
                                          "bps": 1_000_000, "pattern": "constant", "enabled": true}}),
        )
        .await
        .unwrap();
    let flow_id = added["flow_id"].as_u64().unwrap();
    let listed = until_rpc(
        &app,
        Duration::from_secs(10),
        "the new flow to find its path",
        "labs.flow.list",
        json!({"lab_id": id}),
        |value| {
            value.as_array().is_some_and(|flows| {
                flows
                    .iter()
                    .any(|flow| flow["id"] == flow_id && flow["state"]["active"] == true)
            })
        },
    )
    .await;
    assert_eq!(listed.as_array().unwrap().len(), 4);
    let ours = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|flow| flow["id"] == flow_id)
        .unwrap();
    assert_eq!(ours["state"]["active"], true, "{ours}");
    assert!(
        !ours["state"]["path"].as_array().unwrap().is_empty(),
        "the path the traffic takes: {ours}"
    );

    // A flow from nowhere is refused with the name in the message.
    let bad = app
        .rpc(
            "labs.flow.add",
            json!({"lab_id": id, "flow": {"name": "X", "src": "Nowhere", "dst_ip": "8.8.8.8", "bps": 1}}),
        )
        .await
        .unwrap_err();
    assert!(bad.message.contains("Nowhere"), "{bad:?}");

    app.rpc(
        "labs.flow.remove",
        json!({"lab_id": id, "flow_id": flow_id}),
    )
    .await
    .unwrap();
    assert_eq!(
        app.rpc("labs.flow.list", json!({"lab_id": id}))
            .await
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
async fn the_canvas_is_told_what_moves() {
    let app = app();
    let lab = campus(&app).await;
    let id = lab["id"].as_str().unwrap().to_string();
    let mut events = app.subscribe();

    // Background flows keep the rates moving, so ticks keep arriving. Wait
    // for the second one rather than counting how many fit in five seconds —
    // how many that is, is the runner's answer, not the lab's, and a loaded
    // one delivering only the first read as a lab that had stopped reporting.
    let mut ticks = 0;
    // `labs.tick` is published at most every 250 ms, so two of them are half
    // a second's work; thirty seconds is a ceiling on a lab that has stopped
    // reporting, not an estimate of how long two ticks take.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while ticks < 2 {
        let event = tokio::time::timeout_at(deadline, events.recv())
            .await
            .unwrap_or_else(|_| {
                panic!("only {ticks} of 2 ticks arrived: a running lab must keep reporting")
            })
            .expect("the event bus stayed open");
        if event.topic == "labs.tick" && event.payload["lab_id"] == id.as_str() {
            assert!(event.payload["sim_time"].as_u64().unwrap() > 0);
            let links = event.payload["links"].as_array().unwrap();
            assert!(!links.is_empty());
            assert!(links[0]["a_light"].is_string());
            ticks += 1;
        }
    }
    assert_eq!(ticks, 2, "a running lab reports what its links are doing");
}

#[tokio::test]
async fn the_whole_matrix_is_the_default() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "vlans_and_a_trunk", "Matrix").await;
    let id = lab["id"].as_str().unwrap().to_string();

    let matrix = app
        .rpc("labs.connectivity", json!({"lab_id": id}))
        .await
        .unwrap();
    let nodes = matrix["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 4, "the four hosts have addresses: {nodes:?}");
    assert_eq!(matrix["matrix"].as_array().unwrap().len(), 4);
    // Sales and Engineering are separate VLANs with no router between them:
    // half of the pairs must fail, and each cell says why.
    assert!(
        (matrix["reachable_pct"].as_f64().unwrap() - 50.0).abs() < 20.0,
        "{matrix}"
    );
    let row = matrix["matrix"][0].as_array().unwrap();
    assert_eq!(row[0]["ok"], true, "a node reaches itself");
    let blocked = row.iter().find(|cell| cell["ok"] == false).unwrap();
    assert!(!blocked["reason"].as_str().unwrap().is_empty(), "{matrix}");

    // Asking for a node that cannot be probed says so instead of guessing.
    let unknown = app
        .rpc(
            "labs.connectivity",
            json!({"lab_id": id, "nodes": ["Sales1", "Nowhere"]}),
        )
        .await
        .unwrap_err();
    assert!(unknown.message.contains("Nowhere"), "{unknown:?}");
}
