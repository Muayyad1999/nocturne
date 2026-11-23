//! A lab device's QoS statistics are read the way hardware's are: by the
//! `cisco_ios` profile, from what the simulator prints for `show policy-map
//! interface`. A full collection of a lab device must come back with the
//! kind collected — rows where a policy is attached, an empty table where
//! none is — and never with a failed read.

mod common;

use std::time::{Duration, Instant};

use common::*;
use nocturne_app::{App, jobs};
use nocturne_drivers::model::{Direction, QosClass};
use serde_json::{Value, json};

/// The inventory id of a node of a running lab.
fn device_id(lab: &Value, node: &str) -> String {
    lab["devices"][node]
        .as_str()
        .unwrap_or_else(|| panic!("a device for {node}: {}", lab["devices"]))
        .to_string()
}

/// Collect everything each device's profile defines, through `collect.run`,
/// and return the job's entry per device.
async fn collect_all(app: &App, device_ids: &[&str]) -> Vec<Value> {
    let started = app
        .rpc("collect.run", json!({"device_ids": device_ids}))
        .await
        .expect("the job starts");
    let job = jobs::wait(app, started["job_id"].as_str().expect("a job id"))
        .await
        .expect("the job ends");
    assert_eq!(job.state, jobs::JobState::Succeeded, "{:?}", job.error);
    job.result.expect("a result")["devices"]
        .as_array()
        .expect("devices")
        .clone()
}

fn qos_problems(outcome: &Value) -> Vec<&Value> {
    outcome["problems"]
        .as_array()
        .expect("problems")
        .iter()
        .filter(|problem| problem["kind"] == "qos")
        .collect()
}

fn collected_qos(outcome: &Value) -> bool {
    outcome["collected"]
        .as_array()
        .expect("collected")
        .contains(&json!("qos"))
}

#[tokio::test]
async fn a_full_collection_reads_a_lab_routers_policies_and_that_a_switch_has_none() {
    let app = app();
    fast_ticks(&app).await;
    let lab = start_example(&app, "demo", "QoS read").await;
    let (edge, switch, host) = (
        device_id(&lab, "EDGE"),
        device_id(&lab, "AS1"),
        device_id(&lab, "Staff-PC1"),
    );

    // Nothing was ever read: no document, which is not the same as no policy.
    assert_eq!(
        app.rpc("collect.get", json!({"id": switch, "kind": "qos"}))
            .await
            .unwrap(),
        Value::Null
    );

    // Let the lab's flows carry something through EDGE's uplink first, so
    // that the counters read below are not all zero.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let outcome = collect_all(&app, &[&edge]).await;
        assert_eq!(qos_problems(&outcome[0]), Vec::<&Value>::new());
        let stored = nocturne_app::collect::latest(&app, &edge, nocturne_drivers::model::Kind::Qos)
            .unwrap()
            .expect("a qos document for EDGE");
        let voice_seen = stored
            .typed::<QosClass>()
            .iter()
            .any(|class| class.class == "VOICE" && class.packets.is_some_and(|n| n > 0));
        if voice_seen {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no voice traffic reached EDGE: {:?}",
            stored.rows
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // One job, no kinds named: everything each profile defines.
    let outcomes = collect_all(&app, &[&edge, &switch, &host]).await;
    for outcome in &outcomes[..2] {
        assert!(collected_qos(outcome), "{outcome}");
        assert_eq!(qos_problems(outcome), Vec::<&Value>::new(), "{outcome}");
        assert_eq!(outcome["error"], Value::Null, "{outcome}");
    }
    // A simulated host is another device type, with no QoS read: the kind is
    // not collected and nobody pretends it is empty.
    assert!(!collected_qos(&outcomes[2]), "{}", outcomes[2]);

    // EDGE: the uplink's shaper with the queueing policy inside it, and the
    // policer on the two inside interfaces.
    let shown = app
        .rpc("collect.get", json!({"id": edge, "kind": "qos"}))
        .await
        .unwrap();
    assert_eq!(shown["source"], "lab");
    assert_eq!(shown["unverified"], true, "{shown}");
    let classes: Vec<QosClass> = shown["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| serde_json::from_value(row.clone()).expect("a QosClass"))
        .collect();
    let place = |class: &QosClass| {
        (
            class.interface.clone(),
            class.direction,
            class.policy.clone(),
            class.class.clone(),
            class.parent.clone(),
        )
    };
    let places: Vec<_> = classes.iter().map(place).collect();
    let uplink = "GigabitEthernet0/0".to_string();
    let inner = |class: &str| {
        (
            uplink.clone(),
            Direction::Out,
            "WAN-QUEUE".to_string(),
            class.to_string(),
            Some("class-default".to_string()),
        )
    };
    let guests = |interface: &str, class: &str| {
        (
            interface.to_string(),
            Direction::In,
            "LAN-IN".to_string(),
            class.to_string(),
            None,
        )
    };
    assert_eq!(
        places,
        vec![
            (
                uplink.clone(),
                Direction::Out,
                "WAN-OUT".to_string(),
                "class-default".to_string(),
                None
            ),
            inner("VOICE"),
            inner("SCAVENGER"),
            inner("class-default"),
            guests("GigabitEthernet0/1", "GUEST"),
            guests("GigabitEthernet0/1", "class-default"),
            guests("GigabitEthernet0/2", "GUEST"),
            guests("GigabitEthernet0/2", "class-default"),
        ],
        "{classes:#?}"
    );
    // What is configured, as the simulator prints it and the profile reads it.
    let rate = |index: usize| classes[index].rate_bps;
    assert_eq!(rate(0), Some(6_000_000), "shape average 6000000");
    assert_eq!(rate(1), Some(512_000), "priority 512");
    assert_eq!(rate(2), Some(600_000), "bandwidth percent 10 of the shaper");
    assert_eq!(rate(3), None);
    assert_eq!(rate(4), Some(3_000_000), "police 3000000");
    // Traffic was counted, and each class keeps its two kinds of drop apart:
    // a queue's in one field, a policer's in the other, never both.
    assert!(classes[1].packets.is_some_and(|packets| packets > 0));
    assert!(classes[1].bytes.is_some_and(|bytes| bytes > 0));
    for class in &classes {
        let policed = class.class == "GUEST";
        assert_eq!(class.police_dropped_packets.is_some(), policed, "{class:?}");
        // A class of the input policy has no queue; every class of the output one counts drops.
        let queued = class.direction == Direction::Out;
        assert_eq!(class.queue_dropped_packets.is_some(), queued, "{class:?}");
        assert_eq!(
            class.dropped_packets,
            class.police_dropped_packets.or(class.queue_dropped_packets),
            "{class:?}"
        );
        assert!(class.offered_rate_bps.is_some() && class.drop_rate_bps.is_some());
    }

    // The switch has no policy: collected, and empty.
    let none = app
        .rpc("collect.get", json!({"id": switch, "kind": "qos"}))
        .await
        .unwrap();
    assert_eq!(none["rows"], json!([]), "{none}");
    assert_eq!(none["unverified"], true, "{none}");
    assert_eq!(none["source"], "lab");
    let status = app
        .rpc("collect.status", json!({"id": switch}))
        .await
        .unwrap();
    assert!(
        status
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["kind"] == "qos" && entry["rows"] == 0),
        "{status}"
    );
    // The host: still no document at all.
    assert_eq!(
        app.rpc("collect.get", json!({"id": host, "kind": "qos"}))
            .await
            .unwrap(),
        Value::Null
    );
}
