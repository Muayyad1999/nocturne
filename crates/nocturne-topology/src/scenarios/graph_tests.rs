//! Messy real-world neighbour data: every one of these must produce *one*
//! link with the right confidence — not two, and not none.

use nocturne_app::App;
use nocturne_app::collect;
use nocturne_drivers::model::Kind;
use serde_json::json;

use super::*;
use crate::graph::{self, Options};
use crate::model::{Confidence, Graph, Layer, Link, LinkKind, NodeKind, Status};

async fn physical(app: &App) -> Graph {
    graph::build(app, &Options::default())
        .await
        .expect("the graph is built")
}

async fn l3(app: &App) -> Graph {
    graph::build(
        app,
        &Options {
            layer: Layer::L3,
            ..Options::default()
        },
    )
    .await
    .expect("the graph is built")
}

/// Every link that joins two nodes, whichever way round the ends are.
fn between<'graph>(graph: &'graph Graph, one: &str, other: &str) -> Vec<&'graph Link> {
    graph
        .links
        .iter()
        .filter(|link| {
            (link.a.node == one && link.b.node == other)
                || (link.a.node == other && link.b.node == one)
        })
        .collect()
}

/// The two ends of a link as `(interface, interface)` for the given nodes.
fn ports(link: &Link, node: &str) -> (Option<String>, Option<String>) {
    if link.a.node == node {
        (link.a.interface.clone(), link.b.interface.clone())
    } else {
        (link.b.interface.clone(), link.a.interface.clone())
    }
}

#[tokio::test]
async fn short_and_long_interface_names_on_the_two_ends_are_one_link() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let other = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    // SW1 writes it out, SW2 abbreviates, and the local names differ in form too.
    store(
        &app,
        &one,
        vec![(
            Kind::Neighbors,
            vec![neighbor("GigabitEthernet0/1", "SW2", "Gi0/2", "lldp")],
        )],
    );
    store(
        &app,
        &other,
        vec![(
            Kind::Neighbors,
            vec![neighbor("gi 0/2", "SW1", "GigabitEthernet0/1", "lldp")],
        )],
    );

    let graph = physical(&app).await;
    let links = between(&graph, &one, &other);
    assert_eq!(links.len(), 1, "{:#?}", graph.links);
    assert_eq!(links[0].confidence, Confidence::Confirmed);
    assert_eq!(
        ports(links[0], &one),
        (
            Some("GigabitEthernet0/1".into()),
            Some("GigabitEthernet0/2".into())
        )
    );
    assert_eq!(graph.stats.links, 1);
    assert_eq!(graph.stats.unmanaged, 0);
}

#[tokio::test]
async fn a_hostname_an_fqdn_and_a_chassis_mac_all_find_the_same_device() {
    let app = app();
    let core = device(&app, "CORE1", "10.0.0.1", "core", "HQ");
    let by_name = device(&app, "SW-NAME", "10.0.0.2", "access", "HQ");
    let by_fqdn = device(&app, "SW-FQDN", "10.0.0.3", "access", "HQ");
    let by_mac = device(&app, "SW-MAC", "10.0.0.4", "access", "HQ");
    let by_address = device(&app, "SW-ADDR", "10.0.0.5", "access", "HQ");

    // The chassis MAC only matches because the interface was collected.
    store(
        &app,
        &by_mac,
        vec![(
            Kind::Interfaces,
            vec![json!({"name": "Vlan1", "mac": "00:1b:21:de:ad:01"})],
        )],
    );
    store(
        &app,
        &by_fqdn,
        vec![(
            Kind::Facts,
            vec![json!({"hostname": "sw-fqdn.campus.example.net"})],
        )],
    );
    store(
        &app,
        &core,
        vec![(
            Kind::Neighbors,
            vec![
                neighbor("Gi1/0/1", "SW-NAME", "Gi0/1", "lldp"),
                neighbor("Gi1/0/2", "sw-fqdn.campus.example.net", "Gi0/1", "lldp"),
                neighbor("Gi1/0/3", "001b.21de.ad01", "Gi0/1", "lldp"),
                // Cisco's `name(serial)` form, and a report with only an address.
                neighbor_full("Gi1/0/4", "", "Gi0/1", "cdp", "10.0.0.5", "", ""),
                neighbor("Gi1/0/5", "SW-NAME(FOC1234X5)", "Gi0/2", "cdp"),
            ],
        )],
    );

    let graph = physical(&app).await;
    assert_eq!(graph.stats.unmanaged, 0, "{:#?}", graph.nodes);
    for (expected, port) in [
        (&by_name, "GigabitEthernet1/0/1"),
        (&by_fqdn, "GigabitEthernet1/0/2"),
        (&by_mac, "GigabitEthernet1/0/3"),
        (&by_address, "GigabitEthernet1/0/4"),
        (&by_name, "GigabitEthernet1/0/5"),
    ] {
        let found = between(&graph, &core, expected);
        assert!(
            found
                .iter()
                .any(|link| ports(link, &core).0.as_deref() == Some(port)),
            "no link on {port} to {}",
            inventory::get(&app, expected).unwrap().name
        );
    }
    // Every one of them is one-sided: only CORE1 was collected.
    assert!(
        graph
            .links
            .iter()
            .all(|link| link.confidence == Confidence::OneSided)
    );
    assert_eq!(graph.links.len(), 5);
}

#[tokio::test]
async fn a_name_two_devices_answer_to_is_refused_not_guessed() {
    let app = app();
    let core = device(&app, "CORE1", "10.0.0.1", "core", "HQ");
    // Two sites, one switch called `sw1` in each: the short name is ambiguous.
    let hq = device(&app, "sw1.hq.example", "10.1.0.1", "access", "HQ");
    let north = device(&app, "sw1.north.example", "10.2.0.1", "access", "North");
    store(
        &app,
        &core,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi1/0/1", "sw1", "Gi0/1", "lldp")],
        )],
    );

    let graph = physical(&app).await;
    assert!(between(&graph, &core, &hq).is_empty());
    assert!(between(&graph, &core, &north).is_empty());
    assert_eq!(graph.links.len(), 0);
    assert!(
        graph
            .warnings
            .iter()
            .any(|warning| warning.contains("more than one device")),
        "{:#?}",
        graph.warnings
    );
}

#[tokio::test]
async fn the_same_link_reported_by_lldp_and_cdp_is_one_link() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let other = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    store(
        &app,
        &one,
        vec![(
            Kind::Neighbors,
            vec![
                neighbor("Gi0/1", "SW2", "Gi0/2", "lldp"),
                // CDP spells both names differently and gives no remote port.
                neighbor("GigabitEthernet0/1", "SW2.lab.example", "", "cdp"),
            ],
        )],
    );
    store(
        &app,
        &other,
        vec![(
            Kind::Neighbors,
            vec![
                neighbor("Gi0/2", "SW1", "Gi0/1", "cdp"),
                neighbor("Gi0/2", "SW1", "GigabitEthernet0/1", "lldp"),
            ],
        )],
    );

    let graph = physical(&app).await;
    let links = between(&graph, &one, &other);
    assert_eq!(links.len(), 1, "{:#?}", graph.links);
    assert_eq!(links[0].confidence, Confidence::Confirmed);
    assert_eq!(links[0].protocols, vec!["lldp", "cdp"]);
}

#[tokio::test]
async fn a_report_with_no_remote_port_learns_it_from_the_other_end() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let other = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    store(
        &app,
        &one,
        vec![(Kind::Neighbors, vec![neighbor("Gi0/1", "SW2", "", "lldp")])],
    );
    store(
        &app,
        &other,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/2", "SW1", "Gi0/1", "lldp")],
        )],
    );

    let graph = physical(&app).await;
    let links = between(&graph, &one, &other);
    assert_eq!(links.len(), 1, "{:#?}", graph.links);
    assert_eq!(links[0].confidence, Confidence::Confirmed);
    assert_eq!(
        ports(links[0], &one),
        (
            Some("GigabitEthernet0/1".into()),
            Some("GigabitEthernet0/2".into())
        ),
        "the port SW1 did not name came from SW2"
    );
}

#[tokio::test]
async fn one_end_silent_is_a_one_sided_link_not_a_missing_one() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let other = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    store(
        &app,
        &one,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/1", "SW2", "Gi0/2", "lldp")],
        )],
    );

    let graph = physical(&app).await;
    let links = between(&graph, &one, &other);
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].confidence, Confidence::OneSided);
    assert!(
        graph
            .warnings
            .iter()
            .any(|warning| warning.contains("no neighbour data") && warning.contains("SW2")),
        "{:#?}",
        graph.warnings
    );
}

#[tokio::test]
async fn a_device_that_is_its_own_neighbour_makes_one_stack_link() {
    let app = app();
    let stack = device(&app, "ACC-STACK", "10.0.0.1", "access", "HQ");
    store(
        &app,
        &stack,
        vec![(
            Kind::Neighbors,
            vec![
                // Both stack members report the other end of the same cable.
                neighbor("Gi1/0/49", "ACC-STACK", "Gi2/0/49", "cdp"),
                neighbor("Gi2/0/49", "ACC-STACK", "Gi1/0/49", "cdp"),
            ],
        )],
    );

    let graph = physical(&app).await;
    assert_eq!(graph.links.len(), 1, "{:#?}", graph.links);
    let link = &graph.links[0];
    assert_eq!(link.a.node, link.b.node);
    assert_eq!(link.confidence, Confidence::Confirmed);
    assert_eq!(
        (link.a.interface.as_deref(), link.b.interface.as_deref()),
        (Some("GigabitEthernet1/0/49"), Some("GigabitEthernet2/0/49"))
    );
    assert_eq!(graph.stats.unmanaged, 0, "a stack is not a second device");
}

#[tokio::test]
async fn a_neighbour_that_is_not_in_the_inventory_becomes_an_unmanaged_node() {
    let app = app();
    let switch = device(&app, "ACC01", "10.0.0.1", "access", "HQ");
    store(
        &app,
        &switch,
        vec![(
            Kind::Neighbors,
            vec![
                neighbor_full(
                    "Gi1/0/20",
                    "AP-LOBBY.wifi.example",
                    "GigabitEthernet0",
                    "cdp",
                    "10.1.20.9",
                    "cisco AIR-CAP2702I",
                    "Bridge, WLAN Access Point",
                ),
                // The same access point seen again by LLDP, short name only.
                neighbor("Gi1/0/20", "AP-LOBBY", "Gi0", "lldp"),
                // Something that only reports a chassis MAC.
                neighbor_full(
                    "Gi1/0/21",
                    "b8:27:eb:11:22:33",
                    "",
                    "lldp",
                    "",
                    "",
                    "Station",
                ),
            ],
        )],
    );

    let graph = physical(&app).await;
    assert_eq!(graph.stats.unmanaged, 2, "{:#?}", graph.nodes);
    assert_eq!(graph.links.len(), 2, "{:#?}", graph.links);
    let access_point = graph
        .nodes
        .iter()
        .find(|node| node.name.starts_with("ap-lobby"))
        .expect("the access point is a node");
    assert_eq!(access_point.kind, NodeKind::AccessPoint);
    assert!(access_point.unmanaged && access_point.device_id.is_none());
    assert_eq!(access_point.address.as_deref(), Some("10.1.20.9"));
    // The MAC-only neighbour is named after whoever made it.
    let unnamed = graph
        .nodes
        .iter()
        .find(|node| node.id.starts_with("u:b827eb"))
        .expect("the MAC-only neighbour is a node");
    assert!(
        unnamed.name.ends_with("11:22:33"),
        "named `{}`",
        unnamed.name
    );
    assert!(
        graph
            .links
            .iter()
            .all(|link| link.confidence == Confidence::OneSided),
        "an unmanaged device cannot confirm anything"
    );
}

#[tokio::test]
async fn port_channel_members_fold_into_one_link_when_both_ends_have_one() {
    let campus = campus();
    let graph = physical(&campus.app).await;
    let folded = between(&graph, &campus.access[0], &campus.dist[0]);
    assert_eq!(folded.len(), 1, "{:#?}", folded);
    let link = folded[0];
    let aggregate = link.aggregate.clone().expect("the link names its channel");
    assert!(
        aggregate.contains("Po1") && aggregate.contains("Po10") && aggregate.contains("2 members"),
        "{aggregate}"
    );
    assert_eq!(link.members.len(), 2);
    assert!(
        link.members
            .iter()
            .any(|member| member.contains("Gi1/0/45"))
    );
    assert_eq!(link.confidence, Confidence::Confirmed);
    // The member speeds add up to the aggregate's capacity.
    assert_eq!(link.speed_bps, Some(2_000_000_000));
    let (near, far) = ports(link, &campus.access[0]);
    assert_eq!(near.as_deref(), Some("Port-channel1"));
    assert_eq!(far.as_deref(), Some("Port-channel10"));
}

#[tokio::test]
async fn parallel_links_stay_separate_when_the_aggregate_cannot_be_named() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let other = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    for (near, far, ports) in [
        (&one, "SW2", [("Gi0/1", "Gi0/1"), ("Gi0/2", "Gi0/2")]),
        (&other, "SW1", [("Gi0/1", "Gi0/1"), ("Gi0/2", "Gi0/2")]),
    ] {
        store(
            &app,
            near,
            vec![(
                Kind::Neighbors,
                ports
                    .iter()
                    .map(|(local, remote)| neighbor(local, far, remote, "lldp"))
                    .collect(),
            )],
        );
    }

    let graph = physical(&app).await;
    assert_eq!(
        between(&graph, &one, &other).len(),
        2,
        "with no Port-channel interface collected there is nothing to fold into"
    );
    assert!(graph.links.iter().all(|link| link.aggregate.is_none()));
}

#[tokio::test]
async fn stale_data_is_flagged_and_still_drawn() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let other = device(&app, "SW2", "10.0.0.2", "access", "HQ");
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
        &other,
        vec![(
            Kind::Neighbors,
            vec![neighbor("Gi0/2", "SW1", "Gi0/1", "lldp")],
        )],
    );
    // Age SW2's neighbour table by three days behind the store's back.
    let mut stored = collect::latest(&app, &other, Kind::Neighbors)
        .unwrap()
        .unwrap();
    stored.collected_at = nocturne_app::util::now_ms() - 3 * 86_400_000;
    app.store()
        .put("collected.neighbors", &other, &stored)
        .unwrap();

    let graph = physical(&app).await;
    assert_eq!(graph.stats.stale_devices, 1);
    assert_eq!(graph.links.len(), 1, "stale links are drawn, not hidden");
    let node = graph.node(&other).expect("SW2");
    assert!(node.stale);
    assert!(
        graph
            .warnings
            .iter()
            .any(|warning| warning.contains("more than 24 hours") && warning.contains("SW2")),
        "{:#?}",
        graph.warnings
    );
    assert!(graph.data_age.oldest_at.unwrap() < graph.data_age.newest_at.unwrap());
}

#[tokio::test]
async fn a_three_tier_campus_comes_out_whole() {
    let campus = campus();
    let graph = physical(&campus.app).await;
    assert_eq!(graph.stats.devices, 26);
    assert_eq!(graph.stats.unmanaged, 1, "the lobby access point");
    // 8 core-to-distribution, 39 access uplinks (ACC01 has one plus a channel),
    // one folded port-channel and one access point.
    assert_eq!(
        graph.stats.links,
        8 + 39 + 1 + 1,
        "{:#?}",
        graph.links.len()
    );
    assert!(
        graph
            .links
            .iter()
            .filter(|link| link.kind != LinkKind::Cdp)
            .all(|link| link.confidence == Confidence::Confirmed),
        "every link between two collected devices is confirmed"
    );
    assert!(graph.links.iter().all(|link| link.status == Status::Up));
    // Sites become groups, and every node has a place.
    let sites: Vec<&str> = graph
        .groups
        .iter()
        .filter(|group| group.kind == crate::model::GroupKind::Site)
        .map(|group| group.label.as_str())
        .collect();
    assert_eq!(sites, ["HQ", "North Campus"]);
    assert!(graph.nodes.iter().all(|node| node.x > 0.0 && node.y > 0.0));
    assert_eq!(
        graph.node(&campus.core[0]).unwrap().kind,
        NodeKind::Switch,
        "a core switch with no routing table collected is not promoted to layer 3"
    );
    assert_eq!(
        graph.node(&campus.access[0]).unwrap().model.as_deref(),
        Some("WS-C2960X-48FPS-L")
    );
}

#[tokio::test]
async fn endpoints_appear_only_on_edge_ports() {
    let campus = campus();
    let graph = graph::build(
        &campus.app,
        &Options {
            include_endpoints: true,
            ..Options::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(graph.stats.endpoints, 3, "{:#?}", graph.nodes);
    let host = graph
        .nodes
        .iter()
        .find(|node| node.id == "h:001b21aa0001")
        .expect("the host on ACC01");
    assert_eq!(host.kind, NodeKind::Host);
    assert_eq!(host.name, "10.1.10.50", "named from the ARP table");
    let link = graph
        .links
        .iter()
        .find(|link| link.b.node == host.id || link.a.node == host.id)
        .expect("the host is connected");
    assert_eq!(link.kind, LinkKind::Mac);
    assert_eq!(link.confidence, Confidence::Inferred);
    assert_eq!(link.vlans.as_deref(), Some("10"));

    // Without endpoints the hosts are simply not there.
    let plain = physical(&campus.app).await;
    assert_eq!(plain.stats.endpoints, 0);
    assert!(plain.nodes.iter().all(|node| !node.id.starts_with("h:")));
}

#[tokio::test]
async fn a_mac_on_a_trunk_is_not_an_endpoint() {
    let app = app();
    let core = device(&app, "CORE1", "10.0.0.1", "core", "HQ");
    let edge = device(&app, "ACC01", "10.0.0.2", "access", "HQ");
    store(
        &app,
        &core,
        vec![
            (
                Kind::Neighbors,
                vec![neighbor("Gi1/0/1", "ACC01", "Gi1/0/48", "lldp")],
            ),
            (Kind::Interfaces, vec![trunk("Gi1/0/1", 1_000_000_000)]),
            (
                Kind::MacTable,
                vec![mac_entry("00:1b:21:aa:00:01", "Gi1/0/1", 10)],
            ),
        ],
    );
    store(
        &app,
        &edge,
        vec![
            (
                Kind::Neighbors,
                vec![neighbor("Gi1/0/48", "CORE1", "Gi1/0/1", "lldp")],
            ),
            (
                Kind::Interfaces,
                vec![trunk("Gi1/0/48", 1_000_000_000), access_port("Gi1/0/5", 10)],
            ),
            (
                Kind::MacTable,
                vec![
                    mac_entry("00:1b:21:aa:00:01", "Gi1/0/5", 10),
                    mac_entry("00:1b:21:aa:00:01", "Gi1/0/48", 10),
                ],
            ),
        ],
    );

    let graph = graph::build(
        &app,
        &Options {
            include_endpoints: true,
            ..Options::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(graph.stats.endpoints, 1);
    let link = graph
        .links
        .iter()
        .find(|link| link.kind == LinkKind::Mac)
        .expect("one host link");
    let (near, _) = ports(link, &edge);
    assert_eq!(
        near.as_deref(),
        Some("GigabitEthernet1/0/5"),
        "the host hangs off the access port, not the trunk"
    );
}

#[tokio::test]
async fn the_layer_three_view_joins_subnets_and_ignores_loopbacks() {
    let app = app();
    let one = device(&app, "R1", "10.0.0.1", "core", "HQ");
    let two = device(&app, "R2", "10.0.0.2", "core", "HQ");
    let three = device(&app, "R3", "10.0.0.3", "distribution", "HQ");
    store(
        &app,
        &one,
        vec![(
            Kind::Interfaces,
            vec![
                routed("Loopback0", "10.255.0.1/32"),
                routed("TenGigabitEthernet1/1/1", "10.10.0.1/30"),
                // A /24 all three share, plus a secondary address on it.
                json!({"name": "Vlan100", "ipv4": ["10.1.0.1/24", "172.16.0.1/24"], "oper_up": true}),
                // An HSRP address both routers answer to.
                json!({"name": "Vlan200", "ipv4": ["10.2.0.2/24", "10.2.0.1/24"], "oper_up": true}),
            ],
        )],
    );
    store(
        &app,
        &two,
        vec![(
            Kind::Interfaces,
            vec![
                routed("Loopback0", "10.255.0.2/32"),
                routed("TenGigabitEthernet1/1/1", "10.10.0.2/30"),
                json!({"name": "Vlan100", "ipv4": ["10.1.0.2/24"], "oper_up": true}),
                json!({"name": "Vlan200", "ipv4": ["10.2.0.3/24", "10.2.0.1/24"], "oper_up": true}),
            ],
        )],
    );
    store(
        &app,
        &three,
        vec![(
            Kind::Interfaces,
            vec![json!({"name": "Vlan100", "ipv4": ["10.1.0.3/24"], "oper_up": true})],
        )],
    );

    let graph = l3(&app).await;
    // The /30 is a link; the /24 with three routers is a group.
    let joined = between(&graph, &one, &two);
    assert_eq!(joined.len(), 2, "the /30 and the VLAN 200 /24: {joined:#?}");
    let point_to_point = joined
        .iter()
        .find(|link| link.label.as_deref() == Some("10.10.0.0/30"))
        .expect("the /30 is a link of its own");
    assert_eq!(point_to_point.kind, LinkKind::Subnet);
    assert_eq!(point_to_point.confidence, Confidence::Inferred);
    assert_eq!(
        point_to_point.a.interface.as_deref(),
        Some("TenGigabitEthernet1/1/1")
    );
    assert!(
        [
            point_to_point.a.ip.as_deref(),
            point_to_point.b.ip.as_deref()
        ]
        .contains(&Some("10.10.0.1"))
    );

    let subnet_groups: Vec<&str> = graph
        .groups
        .iter()
        .filter(|group| group.kind == crate::model::GroupKind::Subnet)
        .map(|group| group.label.as_str())
        .collect();
    assert_eq!(subnet_groups, ["10.1.0.0/24 (3 devices)"]);

    // The secondary /24 that only R1 has is nothing at all, and the HSRP
    // address is reported rather than drawn.
    assert!(
        graph
            .groups
            .iter()
            .all(|group| !group.label.starts_with("172.16")),
        "{:#?}",
        graph.groups
    );
    assert!(
        graph
            .warnings
            .iter()
            .any(|warning| warning.contains("10.2.0.1") && warning.contains("virtual address")),
        "{:#?}",
        graph.warnings
    );
    // The real /24 on VLAN 200 still joins the two routers.
    assert!(
        graph
            .links
            .iter()
            .any(|link| link.label.as_deref() == Some("10.2.0.0/24")),
        "{:#?}",
        graph.links
    );
    assert!(
        graph
            .links
            .iter()
            .all(|link| link.a.interface.as_deref() != Some("Loopback0")),
        "loopbacks are addresses, not cables"
    );
}

#[tokio::test]
async fn a_filter_limits_the_graph_to_what_was_asked_for() {
    let campus = campus();
    let graph = graph::build(
        &campus.app,
        &Options {
            filter: crate::model::Filter {
                site: Some("North Campus".into()),
                ..Default::default()
            },
            ..Options::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(graph.stats.devices, 12, "two distribution, ten access");
    assert!(graph.nodes.iter().all(|node| {
        node.site
            .as_deref()
            .is_none_or(|site| site == "North Campus")
    }));
    assert!(
        graph
            .links
            .iter()
            .all(|link| graph.node(&link.a.node).is_some() && graph.node(&link.b.node).is_some()),
        "no link points outside the view"
    );
}

#[tokio::test]
async fn a_manual_link_fills_in_what_discovery_cannot_see() {
    let app = app();
    let one = device(&app, "SW1", "10.0.0.1", "access", "HQ");
    let two = device(&app, "SW2", "10.0.0.2", "access", "HQ");
    let link = crate::manual::save(
        &app,
        crate::manual::ManualLink {
            a: crate::manual::ManualEnd {
                device: Some("SW1".into()),
                interface: Some("gi0/24".into()),
                ..Default::default()
            },
            b: crate::manual::ManualEnd {
                name: Some("Riser switch 3".into()),
                kind: Some(NodeKind::Switch),
                ..Default::default()
            },
            label: Some("unmanaged riser switch".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(link.a.device.as_deref(), Some(one.as_str()));
    assert_eq!(
        link.a.interface.as_deref(),
        Some("GigabitEthernet0/24"),
        "the interface name is canonicalised when it is saved"
    );

    let graph = physical(&app).await;
    assert_eq!(graph.links.len(), 1);
    assert_eq!(graph.links[0].kind, LinkKind::Manual);
    assert_eq!(graph.links[0].confidence, Confidence::Confirmed);
    let riser = graph
        .nodes
        .iter()
        .find(|node| node.id == "m:riser-switch-3")
        .expect("the riser switch is drawn");
    assert!(riser.unmanaged && riser.kind == NodeKind::Switch);
    assert!(graph.node(&two).is_some(), "SW2 is still in the graph");

    crate::manual::delete(&app, &link.id).unwrap();
    assert_eq!(physical(&app).await.links.len(), 0);
    assert_eq!(
        crate::manual::delete(&app, &link.id).unwrap_err().code,
        "not_found"
    );
}

#[tokio::test]
async fn vlans_and_down_links_come_from_the_collected_interfaces() {
    let app = app();
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
            (
                Kind::Interfaces,
                vec![
                    json!({"name": "Gi0/1", "mode": "trunk", "oper_up": false, "speed_bps": 1_000_000_000u64}),
                ],
            ),
            (
                Kind::Vlans,
                vec![
                    json!({"id": 10, "name": "users", "interfaces": ["Gi0/1"]}),
                    json!({"id": 11, "name": "voice", "interfaces": ["Gi0/1"]}),
                    json!({"id": 12, "name": "print", "interfaces": ["GigabitEthernet0/1"]}),
                    json!({"id": 99, "name": "mgmt", "interfaces": ["Gi0/5"]}),
                ],
            ),
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

    let graph = physical(&app).await;
    assert_eq!(graph.links.len(), 1);
    assert_eq!(graph.links[0].status, Status::Down);
    assert_eq!(graph.links[0].vlans.as_deref(), Some("10-12"));
    assert_eq!(graph.links[0].speed_bps, Some(1_000_000_000));
}
