//! The designs that matter, booted in the simulator and probed — including
//! what happens when a core or an uplink is taken away.

mod common;

use std::net::Ipv4Addr;

use common::{SECOND, SETTLE, assert_invariants, assert_json_round_trip, boot, failures, probe};
use nocturne_design::{
    Design, Expect, InternetEdge, Purpose, Redundancy, Requirements, Routing, Services, Site, Wan,
    WanTopology, examples, generate,
};
use nocturne_netsim::{CliKind, Fault, Lab, Light};

/// The question in the specification, answered end to end.
fn the_branch() -> Requirements {
    Requirements {
        name: "Acme branch".into(),
        sites: vec![Site {
            name: "Branch".into(),
            users: 120,
            voice: true,
            guest_wifi: true,
            servers: 2,
            printers: 3,
            vlans: Vec::new(),
        }],
        redundancy: Redundancy::Uplinks,
        wan: Wan {
            topology: WanTopology::None,
            hub: None,
            internet: InternetEdge::PerSite,
        },
        routing: Routing::Static,
        services: Services {
            dhcp: true,
            nat: true,
            ntp: None,
            syslog: None,
            snmp: None,
        },
        hosts_per_vlan: 2,
        ..Requirements::default()
    }
}

#[test]
fn the_branch_boots_and_every_check_holds() {
    let design = generate(&the_branch()).unwrap();
    assert_invariants(&design, "branch");
    assert_json_round_trip(&design.lab, "branch");
    let mut lab = boot(&design, "branch");
    let failures = failures(&design, &mut lab);
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    // The lab is a real network: the devices answer on their consoles, the
    // interfaces are up, and the pictures do not overlap.
    let core = lab.node_by_name("BRANCH-CORE1").expect("a core switch");
    let routes = lab.exec(core, "show ip route").unwrap();
    assert!(routes.contains("S*    0.0.0.0/0"), "{routes}");
    let vlans = lab.exec(core, "show vlan brief").unwrap();
    for name in ["USERS", "VOICE", "GUEST", "SERVERS", "PRINTERS", "MGMT"] {
        assert!(vlans.contains(name), "VLAN {name} is missing:\n{vlans}");
    }
    let channel = lab
        .exec(
            lab.node_by_name("BRANCH-ACC1").unwrap(),
            "show etherchannel summary",
        )
        .unwrap();
    assert!(
        channel.contains("(P)"),
        "the uplink bundle is not up:\n{channel}"
    );
    let bindings = lab.node_tables(core).unwrap().dhcp_bindings;
    assert!(!bindings.is_empty(), "nothing took a DHCP lease");

    // Unused ports really are shut, and every cabled one is up.
    for node in lab.snapshot().nodes {
        for iface in &node.interfaces {
            if iface.description == "Unused" {
                assert!(
                    !iface.admin_up && iface.link.is_none(),
                    "{} {} is parked but live",
                    node.name,
                    iface.name
                );
            }
            if iface.link.is_some() && node.cli == CliKind::Ios {
                assert!(
                    iface.admin_up,
                    "{} {} is cabled but shut",
                    node.name, iface.name
                );
                assert_ne!(
                    iface.light,
                    Light::Red,
                    "{} {} is cabled and down",
                    node.name,
                    iface.name
                );
            }
        }
    }
}

#[test]
fn every_example_boots_and_proves_itself() {
    for (label, requirements) in examples() {
        let design = generate(&requirements).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_invariants(&design, &label);
        assert_json_round_trip(&design.lab, &label);
        assert!(
            !design.plan.checks.is_empty() && !design.decisions.is_empty(),
            "{label}: a design with no checks and no reasoning is not a design"
        );
        let mut lab = boot(&design, &label);
        let problems = failures(&design, &mut lab);
        assert!(problems.is_empty(), "{label}:\n{}", problems.join("\n"));
    }
}

/// With `Redundancy::Full` nothing in the local network may be a single point
/// of failure. Take a core away, then an uplink, and probe again.
#[test]
fn full_redundancy_survives_a_lost_core_and_a_lost_uplink() {
    let requirements = Requirements {
        name: "Resilient".into(),
        sites: vec![Site {
            name: "Tower".into(),
            users: 80,
            voice: true,
            guest_wifi: true,
            servers: 2,
            printers: 2,
            vlans: Vec::new(),
        }],
        redundancy: Redundancy::Full,
        routing: Routing::Ospf,
        hosts_per_vlan: 2,
        ..Requirements::default()
    };
    let design = generate(&requirements).unwrap();
    let mut lab = boot(&design, "resilient");
    assert!(failures(&design, &mut lab).is_empty(), "before any failure");

    // The checks that still have to hold once a device is gone: the ones that
    // do not name that device.
    let survivors = |design: &Design, gone: &[&str]| -> Vec<nocturne_design::Check> {
        design
            .plan
            .checks
            .iter()
            .filter(|c| {
                let addresses: Vec<String> = design
                    .plan
                    .devices
                    .iter()
                    .filter(|d| gone.contains(&d.name.as_str()))
                    .flat_map(|d| [d.mgmt_ip.clone(), d.loopback.clone()])
                    .collect();
                !gone.contains(&c.from.as_str()) && !addresses.contains(&c.to)
            })
            .cloned()
            .collect()
    };
    let recheck = |lab: &mut Lab, checks: &[nocturne_design::Check], when: &str| {
        for check in checks {
            let (ok, reason) =
                probe(lab, &check.from, &check.to, check.port).unwrap_or_else(|e| panic!("{e}"));
            assert_eq!(
                ok,
                check.expect == Expect::Reachable,
                "{when}: {} -> {} ({}) expected {:?}: {reason}",
                check.from,
                check.to,
                check.to_name,
                check.expect
            );
        }
    };

    // 1. The first core switch, with the active gateways of half the VLANs.
    let core1 = lab.node_by_name("TOWER-CORE1").expect("the first core");
    lab.set_power(core1, false);
    lab.settle(SETTLE);
    recheck(
        &mut lab,
        &survivors(&design, &["TOWER-CORE1", "TOWER-EDGE1"]),
        "with the first core switched off",
    );
    let core2 = lab.node_by_name("TOWER-CORE2").unwrap();
    let standby = lab.exec(core2, "show standby brief").unwrap();
    assert_eq!(
        standby
            .lines()
            .filter(|l| l.starts_with("Vl") && l.contains("Active  local"))
            .count(),
        design
            .plan
            .vlans
            .iter()
            .filter(|v| v.site == "Tower")
            .count(),
        "the survivor has to be active for every VLAN now:\n{standby}"
    );
    lab.set_power(core1, true);
    lab.settle(SETTLE);
    assert!(
        failures(&design, &mut lab).is_empty(),
        "and everything comes back when it is powered on again"
    );

    // 2. The second core's edge router: the Internet has to stay reachable.
    let edge2 = lab.node_by_name("TOWER-EDGE2").expect("the second edge");
    lab.inject(Fault::PowerOff { node: edge2 }).unwrap();
    lab.settle(SETTLE);
    recheck(
        &mut lab,
        &survivors(&design, &["TOWER-EDGE2"]),
        "with the second edge router off",
    );

    // 3. One access switch's uplink to the first core.
    let uplink = lab
        .snapshot()
        .links
        .into_iter()
        .find(|l| lab.node_by_name("TOWER-ACC1") == Some(l.a) && l.a_if == "GigabitEthernet0/1")
        .expect("the first uplink of the first access switch");
    lab.inject(Fault::LinkDown { link: uplink.id }).unwrap();
    lab.settle(SETTLE);
    recheck(
        &mut lab,
        &survivors(&design, &["TOWER-EDGE2"]),
        "with an access uplink cut",
    );
}

/// Without redundancy the same failure does take the site out — which is what
/// the warning in the design says it will.
#[test]
fn no_redundancy_means_what_it_says() {
    let design = generate(&Requirements {
        name: "Cheap".into(),
        sites: vec![Site {
            name: "Shop".into(),
            users: 20,
            ..Site::default()
        }],
        redundancy: Redundancy::None,
        ..Requirements::default()
    })
    .unwrap();
    let mut lab = boot(&design, "cheap");
    assert!(failures(&design, &mut lab).is_empty());
    let core = lab.node_by_name("SHOP-CORE1").unwrap();
    lab.set_power(core, false);
    lab.settle(60 * SECOND);
    let gateway: Ipv4Addr = design
        .plan
        .vlans
        .iter()
        .find(|v| v.purpose == Purpose::Users)
        .unwrap()
        .gateway
        .parse()
        .unwrap();
    let pc = lab.node_by_name("SHOP-PC1").unwrap();
    assert!(
        !lab.probe_ping(pc, gateway).ok,
        "one core, one point of failure"
    );
    assert!(
        design
            .decisions
            .iter()
            .any(|d| d.topic == "Redundancy" && d.text.contains("single point of failure")),
        "and the design says so: {:#?}",
        design.decisions
    );
}

/// Guests must not reach anything internal, in either direction, and the
/// isolation has to survive a reconvergence.
#[test]
fn guests_stay_outside() {
    let design = generate(&Requirements {
        name: "Hotel".into(),
        sites: vec![Site {
            name: "Lobby".into(),
            users: 40,
            guest_wifi: true,
            servers: 1,
            ..Site::default()
        }],
        redundancy: Redundancy::Full,
        hosts_per_vlan: 2,
        ..Requirements::default()
    })
    .unwrap();
    let mut lab = boot(&design, "hotel");
    let internal: Vec<String> = design
        .plan
        .vlans
        .iter()
        .filter(|v| v.purpose != Purpose::Guest)
        .map(|v| v.gateway.clone())
        .collect();
    assert!(internal.len() >= 3);
    for target in &internal {
        let (ok, reason) = probe(&mut lab, "LOBBY-GUEST1", target, None).unwrap();
        assert!(!ok, "a guest reached {target}: {reason}");
    }
    // Out to the Internet, though, which is the point of a guest network.
    assert!(probe(&mut lab, "LOBBY-GUEST1", "8.8.8.8", None).unwrap().0);
    assert!(
        probe(&mut lab, "LOBBY-GUEST1", "8.8.8.8", Some(80))
            .unwrap()
            .0
    );
    // And the other way: an internal host gets no answer from a guest.
    let guest_host = design
        .plan
        .devices
        .iter()
        .find(|d| d.name == "LOBBY-GUEST1")
        .map(|d| d.mgmt_ip.clone())
        .unwrap();
    assert!(!probe(&mut lab, "LOBBY-PC1", &guest_host, None).unwrap().0);

    // Switch a core off: the access list lives on both, so nothing opens up.
    let core1 = lab.node_by_name("LOBBY-CORE1").unwrap();
    lab.set_power(core1, false);
    lab.settle(SETTLE);
    for target in &internal {
        let (ok, reason) = probe(&mut lab, "LOBBY-GUEST1", target, None).unwrap();
        assert!(!ok, "a guest reached {target} after a failover: {reason}");
    }
    assert!(
        probe(&mut lab, "LOBBY-GUEST1", "8.8.8.8", None).unwrap().0,
        "and the guests keep their Internet"
    );
}

/// A site with thousands of users: a distribution layer appears, and it still
/// works.
#[test]
fn a_site_with_thousands_of_users_still_boots() {
    let design = generate(&Requirements {
        name: "Tower".into(),
        sites: vec![Site {
            name: "Tower".into(),
            users: 2000,
            voice: true,
            guest_wifi: true,
            servers: 4,
            printers: 10,
            vlans: Vec::new(),
        }],
        redundancy: Redundancy::Full,
        routing: Routing::Ospf,
        growth_pct: 0,
        addressing: nocturne_design::Addressing {
            base: "10.0.0.0/12".into(),
            ..Default::default()
        },
        ..Requirements::default()
    })
    .unwrap();
    assert_invariants(&design, "tower");
    let access = design
        .plan
        .devices
        .iter()
        .filter(|d| d.role == nocturne_design::DeviceRole::Access)
        .count();
    let distribution = design
        .plan
        .devices
        .iter()
        .filter(|d| d.role == nocturne_design::DeviceRole::Distribution)
        .count();
    assert!(access >= 84, "2010 host ports at 24 a switch is {access}");
    assert!(
        distribution > 0,
        "a collapsed core cannot take {access} access switches on its own"
    );
    let mut lab = boot(&design, "tower");
    let problems = failures(&design, &mut lab);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// A site that large with growth head-room on top does not fit behind one
/// collapsed core, and the refusal says so in words.
#[test]
fn a_site_too_large_for_a_collapsed_core_is_refused_clearly() {
    let requirements = Requirements {
        name: "Too big".into(),
        sites: vec![Site {
            name: "Megatower".into(),
            users: 9000,
            voice: true,
            guest_wifi: true,
            ..Site::default()
        }],
        redundancy: Redundancy::Uplinks,
        growth_pct: 50,
        addressing: nocturne_design::Addressing {
            base: "10.0.0.0/12".into(),
            ..Default::default()
        },
        ..Requirements::default()
    };
    let problems = nocturne_design::validate(&requirements);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("access switches")
            && problems[0].contains("collapsed core")
            && problems[0].contains("split the site"),
        "{}",
        problems[0]
    );
    assert!(generate(&requirements).is_err());
}

/// Sites that are not joined by a WAN are islands, and the design does not
/// pretend otherwise.
#[test]
fn islands_are_islands() {
    let design = generate(&Requirements {
        name: "Islands".into(),
        sites: vec![
            Site {
                name: "North".into(),
                users: 20,
                ..Site::default()
            },
            Site {
                name: "South".into(),
                users: 20,
                ..Site::default()
            },
        ],
        wan: Wan {
            topology: WanTopology::None,
            hub: None,
            internet: InternetEdge::PerSite,
        },
        ..Requirements::default()
    })
    .unwrap();
    let mut lab = boot(&design, "islands");
    assert!(failures(&design, &mut lab).is_empty());
    assert!(
        !design
            .plan
            .checks
            .iter()
            .any(|c| c.from == "NMS" && c.to_name.contains("SOUTH")),
        "there is no path to the other site, so nothing may claim there is"
    );
    let south = design
        .plan
        .vlans
        .iter()
        .find(|v| v.site == "South" && v.purpose == Purpose::Users)
        .unwrap()
        .gateway
        .clone();
    assert!(
        !probe(&mut lab, "NORTH-PC1", &south, None).unwrap().0,
        "and the lab agrees"
    );
}
