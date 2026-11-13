//! `topology.path`: longest-prefix match over collected routing tables, then
//! ARP and the MAC tables — and an honest stop wherever the data runs out.

use nocturne_app::App;
use nocturne_drivers::model::Kind;
use serde_json::json;

use super::*;
use crate::path::{self, Params, Trace};

const HOST_MAC: &str = "00:1b:21:aa:00:55";
const HOST_IP: &str = "10.1.10.55";

/// `R1 — R2 — VLAN 10`, with an access switch under R2 and a stub router R3
/// nothing has been collected from.
struct Routed {
    app: App,
    r1: String,
    r2: String,
    r3: String,
    sw1: String,
}

fn routed_network() -> Routed {
    let app = app();
    let r1 = device(&app, "R1", "10.255.0.1", "core", "HQ");
    let r2 = device(&app, "R2", "10.255.0.2", "distribution", "HQ");
    let r3 = device(&app, "R3", "10.255.0.3", "distribution", "North");
    let sw1 = device(&app, "SW1", "10.0.2.1", "access", "HQ");

    store(
        &app,
        &r1,
        vec![
            (
                Kind::Interfaces,
                vec![
                    routed("Loopback0", "10.255.0.1/32"),
                    routed("TenGigabitEthernet1/1/1", "10.10.0.1/30"),
                    routed("TenGigabitEthernet1/1/2", "10.10.1.1/30"),
                ],
            ),
            (
                Kind::Routes,
                vec![
                    route("10.10.0.0/30", "connected", "", "TenGigabitEthernet1/1/1"),
                    route("10.10.1.0/30", "connected", "", "TenGigabitEthernet1/1/2"),
                    route("10.1.10.0/24", "ospf", "10.10.0.2", "Te1/1/1"),
                    route("10.255.0.0/24", "ospf", "10.10.0.2", "Te1/1/1"),
                    route("0.0.0.0/0", "static", "198.51.100.1", "Te1/1/1"),
                ],
            ),
        ],
    );
    store(
        &app,
        &r2,
        vec![
            (
                Kind::Interfaces,
                vec![
                    routed("Loopback0", "10.255.0.2/32"),
                    routed("TenGigabitEthernet1/1/1", "10.10.0.2/30"),
                    routed("Vlan10", "10.1.10.1/24"),
                ],
            ),
            (
                Kind::Routes,
                vec![
                    route("10.1.10.0/24", "connected", "", "Vlan10"),
                    route("10.10.0.0/30", "connected", "", "Te1/1/1"),
                    route("10.255.0.0/24", "ospf", "10.10.0.1", "Te1/1/1"),
                    route("0.0.0.0/0", "static", "10.10.0.1", "Te1/1/1"),
                ],
            ),
            (Kind::Arp, vec![arp_entry(HOST_IP, HOST_MAC, "Vlan10")]),
            (
                Kind::Neighbors,
                vec![neighbor("Gi1/0/1", "SW1", "Gi1/0/48", "lldp")],
            ),
        ],
    );
    // R3 is in the inventory, but no routing table has been collected from it.
    store(
        &app,
        &r3,
        vec![(Kind::Interfaces, vec![routed("Vlan1", "10.10.1.2/30")])],
    );
    store(
        &app,
        &sw1,
        vec![
            (
                Kind::Neighbors,
                vec![neighbor("Gi1/0/48", "R2", "Gi1/0/1", "lldp")],
            ),
            (
                Kind::Interfaces,
                vec![trunk("Gi1/0/48", 1_000_000_000), access_port("Gi1/0/5", 10)],
            ),
            (
                Kind::MacTable,
                vec![
                    mac_entry(HOST_MAC, "Gi1/0/5", 10),
                    // The same MAC is learnt on the uplink too, as it would be.
                    mac_entry(HOST_MAC, "Gi1/0/48", 10),
                ],
            ),
        ],
    );
    Routed {
        app,
        r1,
        r2,
        r3,
        sw1,
    }
}

fn trace(app: &App, from: &str, to: &str) -> Trace {
    path::trace(
        app,
        &Params {
            from: from.to_string(),
            to: to.to_string(),
        },
    )
    .expect("the trace runs")
}

#[tokio::test]
async fn a_path_to_a_host_ends_at_the_switch_port_it_is_plugged_into() {
    let net = routed_network();
    let found = trace(&net.app, "R1", HOST_IP);
    assert!(found.complete, "{found:#?}");
    assert_eq!(found.stopped_because, None);
    let steps: Vec<(&str, &str)> = found
        .hops
        .iter()
        .map(|hop| (hop.node.as_str(), hop.via.as_str()))
        .collect();
    assert_eq!(
        steps,
        [
            ("R1", "route"),
            ("R2", "connected"),
            (HOST_IP, "arp"),
            (HOST_IP, "mac"),
        ],
        "{found:#?}"
    );
    assert_eq!(
        found.hops[0].out_interface.as_deref(),
        Some("TenGigabitEthernet1/1/1")
    );
    assert!(found.hops[0].detail.contains("10.1.10.0/24"));
    assert!(found.hops[0].detail.contains("ospf"));
    assert_eq!(
        found.hops[1].in_interface.as_deref(),
        Some("TenGigabitEthernet1/1/1"),
        "R2 was entered through the interface that owns the next hop"
    );
    assert_eq!(found.hops[1].out_interface.as_deref(), Some("Vlan10"));
    assert_eq!(found.hops[3].device_id.as_deref(), Some(net.sw1.as_str()));
    assert_eq!(
        found.hops[3].in_interface.as_deref(),
        Some("GigabitEthernet1/0/5"),
        "the access port, not the uplink the MAC was also seen on"
    );
    assert!(found.notes.is_empty(), "{:#?}", found.notes);
}

#[tokio::test]
async fn a_path_between_two_devices_stops_when_it_arrives() {
    let net = routed_network();
    let found = trace(&net.app, "R1", "R2");
    assert!(found.complete, "{found:#?}");
    assert_eq!(found.hops.len(), 2);
    assert_eq!(found.hops[1].node, "R2");
    assert_eq!(found.hops[1].via, "connected");
    assert!(found.hops[1].detail.contains("an address of this device"));
    // Starting and ending in the same place is one hop, not a refusal.
    let same = trace(&net.app, "R1", "R1");
    assert!(same.complete && same.hops.len() == 1, "{same:#?}");
    let _ = &net.r1;
}

#[tokio::test]
async fn the_default_route_is_used_and_the_edge_is_an_honest_stop() {
    let net = routed_network();
    let found = trace(&net.app, "R2", "8.8.8.8");
    assert!(!found.complete);
    let steps: Vec<&str> = found.hops.iter().map(|hop| hop.node.as_str()).collect();
    assert_eq!(steps, ["R2", "R1"], "{found:#?}");
    assert!(
        found.hops[1].detail.starts_with("default route 0.0.0.0/0"),
        "{:?}",
        found.hops[1].detail
    );
    let because = found.stopped_because.expect("a reason");
    assert!(
        because.contains("198.51.100.1") && because.contains("not an address of any device"),
        "{because}"
    );
}

#[tokio::test]
async fn missing_route_data_stops_the_walk_and_names_the_device() {
    let net = routed_network();
    // R1's routing table is replaced by one that reaches R3 but has no default
    // route, so the walk gets one hop further and then stops at the device
    // whose routing table is missing.
    store(
        &net.app,
        &net.r1,
        vec![(
            Kind::Routes,
            vec![
                route("10.10.1.0/30", "connected", "", "Te1/1/2"),
                route("10.20.0.0/16", "ospf", "10.10.1.2", "Te1/1/2"),
            ],
        )],
    );
    let found = trace(&net.app, "R1", "10.20.0.9");
    assert!(!found.complete);
    assert_eq!(found.hops.last().map(|hop| hop.node.as_str()), Some("R3"));
    let because = found.stopped_because.expect("a reason");
    assert!(
        because.starts_with("no route data collected for R3"),
        "{because}"
    );
    assert!(because.contains("`routes`"), "{because}");
    let _ = &net.r3;

    // With no route that matches at all, R1 itself is the honest stop.
    let nowhere = trace(&net.app, "R1", "203.0.113.9");
    assert!(!nowhere.complete);
    assert_eq!(
        nowhere.stopped_because.as_deref(),
        Some("R1 has no route to 203.0.113.9")
    );
    assert!(
        nowhere.hops[0]
            .detail
            .contains("no route to 203.0.113.9 among the 2 routes collected"),
        "{:?}",
        nowhere.hops[0].detail
    );
}

#[tokio::test]
async fn equal_cost_paths_are_reported_and_the_first_is_followed() {
    let net = routed_network();
    store(
        &net.app,
        &net.r1,
        vec![(
            Kind::Routes,
            vec![
                route("10.10.0.0/30", "connected", "", "Te1/1/1"),
                route("10.10.1.0/30", "connected", "", "Te1/1/2"),
                route("10.1.10.0/24", "ospf", "10.10.0.2", "Te1/1/1"),
                route("10.1.10.0/24", "ospf", "10.10.1.2", "Te1/1/2"),
            ],
        )],
    );
    let found = trace(&net.app, "R1", HOST_IP);
    assert!(found.complete, "{found:#?}");
    assert!(
        found
            .notes
            .iter()
            .any(|note| note.contains("2 equal-cost paths")
                && note.contains("10.10.0.2")
                && note.contains("10.10.1.2")),
        "{:#?}",
        found.notes
    );
    assert_eq!(found.hops[1].node, "R2", "the first of the two is followed");
    let _ = &net.r2;
}

#[tokio::test]
async fn a_higher_metric_route_is_not_an_equal_cost_path() {
    let net = routed_network();
    store(
        &net.app,
        &net.r1,
        vec![(
            Kind::Routes,
            vec![
                route("10.1.10.0/24", "ospf", "10.10.0.2", "Te1/1/1"),
                json!({"prefix": "10.1.10.0/24", "protocol": "ospf", "next_hop": "10.10.1.2", "distance": 110, "metric": 200}),
            ],
        )],
    );
    let found = trace(&net.app, "R1", HOST_IP);
    assert!(found.notes.is_empty(), "{:#?}", found.notes);
    assert!(found.complete);
}

#[tokio::test]
async fn a_routing_loop_in_the_data_is_reported_as_one() {
    let app = app();
    let one = device(&app, "R1", "10.0.0.1", "core", "HQ");
    let two = device(&app, "R2", "10.0.0.2", "core", "HQ");
    store(
        &app,
        &one,
        vec![
            (Kind::Interfaces, vec![routed("Gi0/0", "10.10.0.1/30")]),
            (
                Kind::Routes,
                vec![route("0.0.0.0/0", "static", "10.10.0.2", "Gi0/0")],
            ),
        ],
    );
    store(
        &app,
        &two,
        vec![
            (Kind::Interfaces, vec![routed("Gi0/0", "10.10.0.2/30")]),
            (
                Kind::Routes,
                vec![route("0.0.0.0/0", "static", "10.10.0.1", "Gi0/0")],
            ),
        ],
    );
    let found = trace(&app, "R1", "203.0.113.9");
    assert!(!found.complete);
    assert!(
        found
            .stopped_because
            .as_deref()
            .is_some_and(|because| because.contains("routing loop")),
        "{found:#?}"
    );
    assert!(found.hops.len() >= 2);
}

#[tokio::test]
async fn a_connected_destination_with_no_arp_entry_says_so() {
    let net = routed_network();
    store(&net.app, &net.r2, vec![(Kind::Arp, vec![])]);
    let found = trace(&net.app, "R1", "10.1.10.77");
    assert!(!found.complete);
    let because = found.stopped_because.expect("a reason");
    assert!(
        because.contains("connected to R2") && because.contains("no ARP entry"),
        "{because}"
    );
}

#[tokio::test]
async fn an_address_nobody_owns_cannot_be_a_starting_point() {
    let net = routed_network();
    let found = trace(&net.app, "192.0.2.7", HOST_IP);
    assert!(!found.complete && found.hops.is_empty());
    assert!(
        found
            .stopped_because
            .as_deref()
            .is_some_and(|because| because.contains("not a device Nocturne manages")),
        "{found:#?}"
    );
    // A name that is neither a device nor an address is a refusal, not a guess.
    let error = path::trace(
        &net.app,
        &Params {
            from: "R1".into(),
            to: "printer-upstairs".into(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "not_found");
    assert!(error.message.contains("neither a device"), "{error}");
}

#[tokio::test]
async fn the_last_hop_over_a_trunk_only_is_flagged() {
    let net = routed_network();
    // SW1 forgets its access port, so the MAC is only on the uplink.
    store(
        &net.app,
        &net.sw1,
        vec![(Kind::MacTable, vec![mac_entry(HOST_MAC, "Gi1/0/48", 10)])],
    );
    let found = trace(&net.app, "R1", HOST_IP);
    assert!(found.complete);
    assert!(
        found
            .notes
            .iter()
            .any(|note| note.contains("trunk or an uplink")),
        "{:#?}",
        found.notes
    );
}

#[tokio::test]
async fn another_vrf_is_only_used_when_the_default_table_has_no_route() {
    let net = routed_network();
    // The only route to the host is in VRF `blue`.
    store(
        &net.app,
        &net.r1,
        vec![(
            Kind::Routes,
            vec![
                route("10.10.0.0/30", "connected", "", "Te1/1/1"),
                json!({"prefix": "10.1.10.0/24", "protocol": "bgp", "next_hop": "10.10.0.2", "interface": "Te1/1/1", "vrf": "blue"}),
            ],
        )],
    );
    let found = trace(&net.app, "R1", HOST_IP);
    assert!(found.complete, "{found:#?}");
    assert!(
        found
            .notes
            .iter()
            .any(|note| note.contains("default routing table of R1") && note.contains("VRF blue")),
        "{:#?}",
        found.notes
    );

    // Give the default table a route of its own and it wins, quietly.
    store(
        &net.app,
        &net.r1,
        vec![(
            Kind::Routes,
            vec![
                route("10.10.0.0/30", "connected", "", "Te1/1/1"),
                route("10.1.10.0/24", "ospf", "10.10.0.2", "Te1/1/1"),
                json!({"prefix": "10.1.10.0/24", "protocol": "bgp", "next_hop": "10.10.1.2", "interface": "Te1/1/2", "vrf": "blue"}),
            ],
        )],
    );
    let found = trace(&net.app, "R1", HOST_IP);
    assert!(found.complete && found.notes.is_empty(), "{found:#?}");
    assert!(found.hops[0].detail.contains("ospf"));
}
