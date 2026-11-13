//! Addressing and routing checks, case by case.

use nocturne_drivers::model::{Interface, Route};

use crate::settings::Settings;
use crate::testdata::*;
use crate::world::World;

fn addressed(name: &str, cidr: &str) -> Interface {
    Interface {
        ipv4: vec![cidr.to_string()],
        ..iface(name)
    }
}

// ------------------------------------------------------------- duplicate IP

/// Two devices each holding `cidr` on an interface, with the configurations given.
fn two_holders(a_name: &str, a_cidr: &str, a_config: &str, b_cidr: &str, b_config: &str) -> World {
    world(vec![
        device("a", "SW1")
            .interfaces(vec![addressed(a_name, a_cidr)])
            .config(a_config),
        device("b", "SW2")
            .interfaces(vec![addressed("Vlan10", b_cidr)])
            .config(b_config),
    ])
}

#[test]
fn duplicate_ip_table() {
    // Positive: one address configured on two devices in one subnet, with
    // both configurations stored (so neither can be a redundancy group).
    let found = two_holders(
        "Vlan10",
        "10.0.0.5/24",
        "interface Vlan10\n ip address 10.0.0.5 255.255.255.0\n",
        "10.0.0.5/24",
        "interface Vlan10\n ip address 10.0.0.5 255.255.255.0\n",
    );
    let found = run("duplicate_ip", &found);
    assert_eq!(subjects(&found), ["a:10.0.0.5"]);
    assert_eq!(found.findings[0].related[0].device_id, "b");
    assert_eq!(
        found.findings[0].evidence.len(),
        2,
        "both holders are evidence"
    );

    // Negative: different addresses.
    assert!(
        run(
            "duplicate_ip",
            &two_holders(
                "Vlan10",
                "10.0.0.5/24",
                "interface Vlan10\n ip address 10.0.0.5 255.255.255.0\n",
                "10.0.0.6/24",
                "interface Vlan10\n ip address 10.0.0.6 255.255.255.0\n",
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: an HSRP virtual address. A redundant pair is *supposed*
    // to answer for it, and it is in both configurations as `standby ip`.
    let hsrp = world(vec![
        device("a", "DIST1")
            .interfaces(vec![addressed("Vlan10", "10.0.0.1/24")])
            .config(
                "interface Vlan10\n ip address 10.0.0.1 255.255.255.0\n standby 10 ip 10.0.0.1\n",
            ),
        device("b", "DIST2")
            .interfaces(vec![addressed("Vlan10", "10.0.0.1/24")])
            .config(
                "interface Vlan10\n ip address 10.0.0.2 255.255.255.0\n standby 10 ip 10.0.0.1\n",
            ),
    ]);
    assert!(run("duplicate_ip", &hsrp).findings.is_empty());

    // Look-alike 2: an anycast loopback. A /32 on a loopback is meant to be
    // in several places at once.
    let anycast = world(vec![
        device("a", "R1")
            .interfaces(vec![addressed("Loopback0", "10.255.0.1/32")])
            .config("interface Loopback0\n ip address 10.255.0.1 255.255.255.255\n"),
        device("b", "R2")
            .interfaces(vec![addressed("Loopback0", "10.255.0.1/32")])
            .config("interface Loopback0\n ip address 10.255.0.1 255.255.255.255\n"),
    ]);
    assert!(run("duplicate_ip", &anycast).findings.is_empty());

    // Look-alike 3: the same address in two different subnets — two isolated
    // networks that both use 10.0.0.1 are not in conflict.
    assert!(
        run(
            "duplicate_ip",
            &two_holders(
                "Vlan10",
                "10.0.0.1/24",
                "interface Vlan10\n ip address 10.0.0.1 255.255.255.0\n",
                "10.0.0.1/30",
                "interface Vlan10\n ip address 10.0.0.1 255.255.255.252\n",
            )
        )
        .findings
        .is_empty()
    );

    // Not enough data: no configuration for one of the holders. Without it,
    // a shared address could be a redundancy group that nothing here can see
    // — which is exactly what the HSRP case above looks like from outside.
    let unknown = world(vec![
        device("a", "SW1")
            .interfaces(vec![addressed("Vlan10", "10.0.0.5/24")])
            .config("interface Vlan10\n ip address 10.0.0.5 255.255.255.0\n"),
        device("b", "SW2").interfaces(vec![addressed("Vlan10", "10.0.0.5/24")]),
    ]);
    let report = run("duplicate_ip", &unknown);
    assert!(report.findings.is_empty());
    assert!(
        report
            .skips()
            .any(|(_, reason)| reason.contains("first-hop redundancy group")),
        "the reason names what is missing"
    );

    // Not enough data: one device's data is older than the staleness threshold.
    let stale = world(vec![
        device("a", "SW1")
            .interfaces_at(vec![addressed("Vlan10", "10.0.0.5/24")], NOW)
            .config("interface Vlan10\n ip address 10.0.0.5 255.255.255.0\n"),
        device("b", "SW2")
            .interfaces_at(
                vec![addressed("Vlan10", "10.0.0.5/24")],
                NOW - 30 * 86_400_000,
            )
            .config("interface Vlan10\n ip address 10.0.0.5 255.255.255.0\n"),
    ]);
    let report = run("duplicate_ip", &stale);
    assert!(report.findings.is_empty());
    assert!(report.skips().any(|(device, _)| device == "b"));
}

#[test]
fn duplicate_ip_in_arp_tables_table() {
    let seen_by = |a_mac: &str, b_mac: &str, apart_minutes: i64| {
        world(vec![
            device("a", "R1").arp_at(vec![arp("10.0.0.50", a_mac, "Vlan10")], NOW),
            device("b", "R2").arp_at(
                vec![arp("10.0.0.50", b_mac, "Vlan10")],
                NOW - apart_minutes * 60_000,
            ),
        ])
    };

    // Positive: two routers, read minutes apart, disagree about the MAC.
    let found = run(
        "duplicate_ip",
        &seen_by("aabb.cc00.0001", "aabb.cc00.0002", 5),
    );
    assert_eq!(subjects(&found), ["a:10.0.0.50 (ARP)"]);

    // Negative: they agree.
    assert!(
        run(
            "duplicate_ip",
            &seen_by("aabb.cc00.0001", "aabb.cc00.0001", 5)
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: one of them is a redundancy group's virtual MAC.
    assert!(
        run(
            "duplicate_ip",
            &seen_by("aabb.cc00.0001", "0000.0c07.ac0a", 5)
        )
        .findings
        .is_empty()
    );
    assert!(
        run(
            "duplicate_ip",
            &seen_by("0000.5e00.0114", "aabb.cc00.0002", 5)
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: a MAC that belongs to one of our own devices — a router
    // answering for the address (proxy ARP, or it *is* the gateway).
    let proxy = world(vec![
        device("a", "R1").arp(vec![arp("10.0.0.50", "aabb.cc00.0001", "Vlan10")]),
        device("b", "R2").arp(vec![arp("10.0.0.50", "0011.2233.4455", "Vlan10")]),
        device("c", "R3").interfaces(vec![Interface {
            mac: "0011.2233.4455".into(),
            ..iface("Vlan10")
        }]),
    ]);
    assert!(run("duplicate_ip", &proxy).findings.is_empty());

    // Look-alike 3: an incomplete ARP entry is not a second holder.
    let incomplete = world(vec![
        device("a", "R1").arp(vec![arp("10.0.0.50", "aabb.cc00.0001", "Vlan10")]),
        device("b", "R2").arp(vec![arp("10.0.0.50", "Incomplete", "Vlan10")]),
    ]);
    assert!(run("duplicate_ip", &incomplete).findings.is_empty());

    // Not enough data: tables read hours apart. The host may simply have been
    // replaced in between.
    let apart = run(
        "duplicate_ip",
        &seen_by("aabb.cc00.0001", "aabb.cc00.0002", 600),
    );
    assert!(apart.findings.is_empty());
}

// --------------------------------------------------------- mask mismatch

fn linked(a: Interface, b: Interface) -> World {
    let (a_name, b_name) = (a.name.clone(), b.name.clone());
    world(vec![
        device("a", "R1")
            .interfaces(vec![a])
            .neighbors(vec![neighbor(&a_name, "R2", &b_name)]),
        device("b", "R2")
            .interfaces(vec![b])
            .neighbors(vec![neighbor(&b_name, "R1", &a_name)]),
    ])
}

#[test]
fn subnet_mask_mismatch_table() {
    // Positive: /24 against /25 on one link.
    let found = run(
        "subnet_mask_mismatch",
        &linked(
            addressed("GigabitEthernet0/1", "10.0.0.1/24"),
            addressed("GigabitEthernet0/2", "10.0.0.2/25"),
        ),
    );
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);

    // Negative: the same prefix length.
    assert!(
        run(
            "subnet_mask_mismatch",
            &linked(
                addressed("GigabitEthernet0/1", "10.0.0.1/24"),
                addressed("GigabitEthernet0/2", "10.0.0.2/24"),
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: addresses that do not overlap at all. That is a different
    // fault, and point-to-point links and unnumbered interfaces make guessing
    // at it unreliable.
    assert!(
        run(
            "subnet_mask_mismatch",
            &linked(
                addressed("GigabitEthernet0/1", "10.0.0.1/24"),
                addressed("GigabitEthernet0/2", "10.9.9.2/30"),
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: secondary addressing. An interface with two addresses is
    // doing something deliberate.
    let secondary = linked(
        Interface {
            ipv4: vec!["10.0.0.1/24".into(), "10.0.1.1/25".into()],
            ..iface("GigabitEthernet0/1")
        },
        addressed("GigabitEthernet0/2", "10.0.0.2/25"),
    );
    let report = run("subnet_mask_mismatch", &secondary);
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 2);

    // Not enough data: a switchport with no address.
    let report = run(
        "subnet_mask_mismatch",
        &linked(
            iface("GigabitEthernet0/1"),
            addressed("GigabitEthernet0/2", "10.0.0.2/25"),
        ),
    );
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 2);
}

// ----------------------------------------------------- overlapping subnets

#[test]
fn overlapping_subnets_table() {
    let with = |addresses: &[(&str, &str)]| {
        world(vec![
            device("a", "R1").interfaces(
                addresses
                    .iter()
                    .map(|(name, cidr)| addressed(name, cidr))
                    .collect(),
            ),
        ])
    };

    // Positive: a typed mask leaves two interfaces overlapping.
    let found = run(
        "overlapping_subnets",
        &with(&[("Vlan10", "10.0.0.1/24"), ("Vlan20", "10.0.0.129/25")]),
    );
    assert_eq!(found.findings.len(), 1);
    assert!(found.findings[0].subject.contains("Vlan10"));

    // Negative: ordinary, separate subnets.
    assert!(
        run(
            "overlapping_subnets",
            &with(&[("Vlan10", "10.0.0.1/24"), ("Vlan20", "10.0.1.1/24")])
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: loopbacks and point-to-point links. /32s and /31s overlap
    // with everything by design.
    assert!(
        run(
            "overlapping_subnets",
            &with(&[
                ("Loopback0", "10.255.0.1/32"),
                ("Vlan10", "10.0.0.1/24"),
                ("GigabitEthernet0/1", "10.0.0.5/31"),
            ])
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: two addresses on one interface are secondary addressing,
    // not two overlapping interfaces.
    assert!(
        run(
            "overlapping_subnets",
            &world(vec![device("a", "R1").interfaces(vec![Interface {
                ipv4: vec!["10.0.0.1/24".into(), "10.0.0.129/25".into()],
                ..iface("Vlan10")
            }])])
        )
        .findings
        .is_empty()
    );

    // Not enough data: one addressed interface.
    let thin = run("overlapping_subnets", &with(&[("Vlan10", "10.0.0.1/24")]));
    assert!(thin.findings.is_empty());
    assert_eq!(thin.skips().count(), 1);
}

// ------------------------------------------------------- no default route

fn route(prefix: &str, protocol: &str) -> Route {
    Route {
        prefix: prefix.to_string(),
        protocol: protocol.to_string(),
        ..Route::default()
    }
}

#[test]
fn no_default_route_table() {
    // Positive: a routing device with no default.
    let found = run(
        "no_default_route",
        &world(vec![device("a", "R1").role("access").routes(vec![
            route("10.0.0.0/24", "connected"),
            route("10.1.0.0/16", "ospf"),
        ])]),
    );
    assert_eq!(subjects(&found), ["a:0.0.0.0/0"]);
    assert_eq!(
        found.findings[0].severity,
        Some(crate::model::Severity::Low)
    );

    // Negative: there is a default.
    assert!(
        run(
            "no_default_route",
            &world(vec![device("a", "R1").routes(vec![
                route("0.0.0.0/0", "static"),
                route("10.0.0.0/24", "connected")
            ]),])
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: a layer-2 switch. Its "routes" are not a routing table,
    // and `ip default-gateway` is how it reaches the world.
    let switch = run(
        "no_default_route",
        &world(vec![
            device("a", "SW1")
                .routes(vec![route("10.0.0.0/24", "connected")])
                .config("ip default-gateway 10.0.0.1\n"),
        ]),
    );
    assert!(switch.findings.is_empty());
    assert_eq!(switch.skips().count(), 1);

    // Look-alike 2: a core device in a default-free core. Reported, but as
    // information: the design is deliberate.
    let core = run(
        "no_default_route",
        &world(vec![
            device("a", "CORE1")
                .role("core")
                .routes(vec![route("10.0.0.0/8", "ospf")]),
        ]),
    );
    assert_eq!(
        core.findings[0].severity,
        Some(crate::model::Severity::Info)
    );

    // Not enough data: no routing table collected.
    let unknown = run("no_default_route", &world(vec![device("a", "R1")]));
    assert!(unknown.findings.is_empty());
    assert_eq!(unknown.skips().count(), 1);
}

#[test]
fn the_staleness_threshold_is_a_setting() {
    let stale = world(vec![
        device("a", "SW1")
            .interfaces_at(vec![addressed("Vlan10", "10.0.0.5/24")], NOW)
            .config("interface Vlan10\n ip address 10.0.0.5 255.255.255.0\n"),
        device("b", "SW2")
            .interfaces_at(
                vec![addressed("Vlan10", "10.0.0.5/24")],
                NOW - 10 * 86_400_000,
            )
            .config("interface Vlan10\n ip address 10.0.0.5 255.255.255.0\n"),
    ]);
    assert!(run("duplicate_ip", &stale).findings.is_empty());
    let patient = Settings {
        thresholds: crate::settings::Thresholds {
            stale_data_hours: 24 * 30,
            ..Default::default()
        },
        ..Settings::default()
    };
    assert_eq!(run_with("duplicate_ip", &stale, &patient).findings.len(), 1);
}
