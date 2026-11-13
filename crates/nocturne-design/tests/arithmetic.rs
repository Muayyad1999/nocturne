//! The sums: switch counts, address carving, growth head-room, determinism.

mod common;

use std::net::Ipv4Addr;

use common::Net;
use nocturne_design::{
    Addressing, DeviceRole, InternetEdge, Purpose, Redundancy, Requirements, Routing, Services,
    Site, Wan, WanTopology, generate, validate,
};

fn branch(users: u32) -> Requirements {
    Requirements {
        name: "Acme branch".into(),
        sites: vec![Site {
            name: "Branch".into(),
            users,
            voice: true,
            guest_wifi: true,
            servers: 2,
            printers: 3,
            vlans: Vec::new(),
        }],
        ..Requirements::default()
    }
}

#[test]
fn the_access_switch_count_is_the_port_count_with_head_room() {
    // 120 users + 3 printers + 1 access point = 124 ports; 25 % head-room
    // makes 155; 24 per switch makes seven.
    let design = generate(&branch(120)).unwrap();
    let access = design
        .plan
        .devices
        .iter()
        .filter(|d| d.role == DeviceRole::Access)
        .count();
    assert_eq!(access, 7, "{:#?}", design.decisions);
    assert!(
        design
            .decisions
            .iter()
            .any(|d| d.topic == "Access switches" && d.text.contains("155")),
        "the arithmetic is not written down: {:#?}",
        design.decisions
    );

    // No growth at all: 124 ports is six switches minus two ports.
    let design = generate(&Requirements {
        growth_pct: 0,
        ..branch(120)
    })
    .unwrap();
    assert_eq!(
        design
            .plan
            .devices
            .iter()
            .filter(|d| d.role == DeviceRole::Access)
            .count(),
        6
    );

    // One user still needs one switch; no users at all needs none.
    assert_eq!(
        generate(&branch(1))
            .unwrap()
            .plan
            .devices
            .iter()
            .filter(|d| d.role == DeviceRole::Access)
            .count(),
        1
    );
    let empty = generate(&Requirements {
        sites: vec![Site {
            name: "Empty".into(),
            ..Site::default()
        }],
        ..Requirements::default()
    })
    .unwrap();
    assert!(
        empty
            .plan
            .devices
            .iter()
            .all(|d| d.role != DeviceRole::Access),
        "a site with nothing in it needs no access switches"
    );
    assert_eq!(
        empty
            .plan
            .devices
            .iter()
            .filter(|d| d.role == DeviceRole::Core)
            .count(),
        1,
        "but it still gets a core to be managed on"
    );
}

#[test]
fn phones_share_the_users_port_and_access_points_come_per_forty_guests() {
    let with_voice = generate(&branch(120)).unwrap();
    let without = generate(&Requirements {
        sites: vec![Site {
            voice: false,
            ..branch(120).sites[0].clone()
        }],
        ..branch(120)
    })
    .unwrap();
    let access = |d: &nocturne_design::Design| {
        d.plan
            .devices
            .iter()
            .filter(|x| x.role == DeviceRole::Access)
            .count()
    };
    assert_eq!(
        access(&with_voice),
        access(&without),
        "telephones daisy-chain, so they must not add switch ports"
    );
    assert!(
        with_voice
            .plan
            .vlans
            .iter()
            .any(|v| v.purpose == Purpose::Voice),
        "but they do get their own VLAN"
    );

    // 30 planned guests is one access point; 200 is five.
    for (users, aps) in [(120u32, 1usize), (800, 5)] {
        let design = generate(&branch(users)).unwrap();
        assert_eq!(
            design
                .plan
                .devices
                .iter()
                .filter(|d| d.role == DeviceRole::AccessPoint)
                .count(),
            aps,
            "{users} users"
        );
    }
}

#[test]
fn the_address_plan_carves_the_base_block_and_keeps_its_promise() {
    let design = generate(&Requirements {
        addressing: Addressing {
            base: "10.20.0.0/16".into(),
            ..Addressing::default()
        },
        ..branch(120)
    })
    .unwrap();
    let base = Net::parse("10.20.0.0/16").unwrap();
    let site = &design.plan.sites[0];
    let block = Net::parse(&site.block).unwrap();
    assert!(base.overlaps(block) && block.len >= base.len);

    for vlan in &design.plan.vlans {
        let net = Net::parse(&vlan.subnet).unwrap();
        assert!(
            block.contains(net.addr) && block.contains(net.last()),
            "{vlan:?}"
        );
        // The head-room that was promised is really there.
        assert!(vlan.hosts_available >= vlan.hosts_needed, "{vlan:?}");
        if vlan.purpose == Purpose::Users {
            assert!(
                vlan.hosts_needed >= 150,
                "120 users with 25 % head-room is 150, not {}",
                vlan.hosts_needed
            );
        }
        // The gateway is the first usable address, and the pool starts ten in.
        let gateway: Ipv4Addr = vlan.gateway.parse().unwrap();
        assert_eq!(u32::from(gateway), net.addr + 1, "{vlan:?}");
        if !vlan.dhcp_first.is_empty() {
            let first: Ipv4Addr = vlan.dhcp_first.parse().unwrap();
            assert_eq!(u32::from(first), net.addr + 11, "{vlan:?}");
        }
    }
    // Loopbacks and point-to-point links come from the top of the base block,
    // outside every site block.
    let infra: Vec<Net> = design
        .plan
        .links
        .iter()
        .filter(|l| !l.subnet.is_empty() && l.kind != nocturne_design::LinkKind::Internet)
        .map(|l| Net::parse(&l.subnet).unwrap())
        .collect();
    for net in infra {
        assert!(base.contains(net.addr), "{net} is outside {base}");
        assert!(
            !block.overlaps(net),
            "{net} overlaps the site block {block}"
        );
    }
}

#[test]
fn point_to_point_links_are_sized_as_asked() {
    for (len, size) in [(30u8, 4u32), (31, 2)] {
        let design = generate(&Requirements {
            addressing: Addressing {
                p2p_len: len,
                ..Addressing::default()
            },
            ..branch(20)
        })
        .unwrap();
        let link = design
            .plan
            .links
            .iter()
            .find(|l| l.kind == nocturne_design::LinkKind::EdgeUplink)
            .expect("a core to edge link");
        let net = Net::parse(&link.subnet).unwrap();
        assert_eq!(net.len, len);
        assert_eq!(net.last() - net.addr + 1, size);
    }
    assert!(
        validate(&Requirements {
            addressing: Addressing {
                p2p_len: 24,
                ..Addressing::default()
            },
            ..branch(20)
        })
        .iter()
        .any(|p| p.contains("/30") && p.contains("/31")),
        "an unusable point-to-point prefix has to be explained"
    );
}

#[test]
fn a_fixed_per_site_block_is_honoured_or_refused_with_the_size_needed() {
    let four_sites = |len: Option<u8>| Requirements {
        name: "Four".into(),
        sites: (1..=4)
            .map(|i| Site {
                name: format!("Site {i}"),
                users: 50,
                ..Site::default()
            })
            .collect(),
        addressing: Addressing {
            base: "10.0.0.0/14".into(),
            per_site_len: len,
            ..Addressing::default()
        },
        wan: Wan {
            topology: WanTopology::HubAndSpoke,
            hub: None,
            internet: InternetEdge::AtHub,
        },
        ..Requirements::default()
    };
    let design = generate(&four_sites(Some(20))).unwrap();
    for site in &design.plan.sites {
        assert_eq!(Net::parse(&site.block).unwrap().len, 20, "{site:?}");
    }
    // Too small, and the message says what it would take.
    let problems = validate(&four_sites(Some(29)));
    assert!(
        problems.iter().any(|p| p.contains("per_site_len is /29")),
        "{problems:?}"
    );
}

#[test]
fn a_base_block_that_is_too_small_says_how_much_is_needed() {
    let problems = validate(&Requirements {
        addressing: Addressing {
            base: "10.0.0.0/24".into(),
            ..Addressing::default()
        },
        ..branch(400)
    });
    assert_eq!(problems.len(), 1, "{problems:?}");
    let message = &problems[0];
    assert!(
        message.contains("10.0.0.0/24")
            && message.contains("256 addresses")
            && message.contains("needs")
            && message.contains("site"),
        "{message}"
    );
    // And it names a block that would work.
    let suggested = message
        .split_whitespace()
        .find(|w| w.starts_with("10.0.0.0/"))
        .and_then(|w| w.trim_end_matches('.').parse::<u8>().ok().map(|_| w))
        .or_else(|| message.split_whitespace().find(|w| w.contains('/')))
        .unwrap_or("");
    assert!(!suggested.is_empty(), "{message}");
}

#[test]
fn two_runs_are_identical_and_the_seed_only_reaches_the_simulator() {
    let requirements = branch(120);
    let a = generate(&requirements).unwrap();
    let b = generate(&requirements).unwrap();
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );

    let mut seeded = requirements.clone();
    seeded.seed = 0xDEAD_BEEF;
    let c = generate(&seeded).unwrap();
    assert_eq!(c.lab.seed, 0xDEAD_BEEF);
    assert_eq!(
        a.plan, c.plan,
        "the seed must not move an address or a port"
    );
    assert_eq!(a.decisions, c.decisions);
    let strip = |d: &nocturne_design::Design| {
        let mut lab = d.lab.clone();
        lab.seed = 0;
        serde_json::to_string(&lab).unwrap()
    };
    assert_eq!(
        strip(&a),
        strip(&c),
        "the seed must not change the lab either"
    );
}

#[test]
fn the_hardware_list_matches_the_devices_that_were_designed() {
    let design = generate(&Requirements {
        redundancy: Redundancy::Full,
        ..branch(300)
    })
    .unwrap();
    let count = |role: DeviceRole| {
        design
            .plan
            .devices
            .iter()
            .filter(|d| d.role == role)
            .count() as u32
    };
    let bom = |class: &str| {
        design
            .plan
            .bill_of_materials
            .iter()
            .filter(|l| l.class == class)
            .map(|l| l.quantity)
            .sum::<u32>()
    };
    assert_eq!(bom("Layer-3 core switch"), count(DeviceRole::Core));
    assert_eq!(bom("Access switch"), count(DeviceRole::Access));
    assert_eq!(
        bom("Edge router") + bom("Edge router (high port count)"),
        count(DeviceRole::Edge)
    );
    assert_eq!(bom("Wireless access point"), count(DeviceRole::AccessPoint));
    assert_eq!(bom("Server"), 2, "the servers that were asked for");
    assert_eq!(bom("IP telephone"), 300, "one per user");
    for line in &design.plan.bill_of_materials {
        assert!(line.quantity > 0 && !line.ports.is_empty(), "{line:?}");
        // Generic classes only: nothing that reads like a catalogue number.
        let digits = line
            .class
            .split(|c: char| !c.is_ascii_digit())
            .map(str::len)
            .max()
            .unwrap_or(0);
        assert!(digits <= 1, "no invented part numbers: {line:?}");
    }
}

#[test]
fn redundancy_changes_the_shape_of_the_site() {
    let shape = |redundancy| {
        let design = generate(&Requirements {
            redundancy,
            ..branch(100)
        })
        .unwrap();
        let cores = design
            .plan
            .devices
            .iter()
            .filter(|d| d.role == DeviceRole::Core)
            .count();
        let edges = design
            .plan
            .devices
            .iter()
            .filter(|d| d.role == DeviceRole::Edge)
            .count();
        let uplinks = design
            .plan
            .links
            .iter()
            .filter(|l| l.kind == nocturne_design::LinkKind::AccessUplink && l.a.ends_with("ACC1"))
            .count();
        let interlinks = design
            .plan
            .links
            .iter()
            .filter(|l| l.kind == nocturne_design::LinkKind::CoreInterlink)
            .count();
        (cores, edges, uplinks, interlinks)
    };
    assert_eq!(shape(Redundancy::None), (1, 1, 1, 0));
    assert_eq!(shape(Redundancy::Uplinks), (1, 1, 2, 0));
    assert_eq!(shape(Redundancy::Full), (2, 2, 2, 2));
}

#[test]
fn explicit_vlans_are_used_as_given() {
    let design = generate(&Requirements {
        name: "Explicit".into(),
        sites: vec![Site {
            name: "Lab".into(),
            users: 10,
            vlans: vec![
                nocturne_design::VlanReq {
                    name: "Research".into(),
                    purpose: Purpose::Users,
                    hosts: 40,
                    id: Some(111),
                },
                nocturne_design::VlanReq {
                    name: "Instruments".into(),
                    purpose: Purpose::Other,
                    hosts: 20,
                    id: None,
                },
            ],
            ..Site::default()
        }],
        routing: Routing::Ospf,
        services: Services {
            dhcp: false,
            ..Services::default()
        },
        ..Requirements::default()
    })
    .unwrap();
    let ids: Vec<u16> = design.plan.vlans.iter().map(|v| v.id).collect();
    assert!(ids.contains(&111), "{ids:?}");
    assert!(
        ids.contains(&99),
        "the management VLAN is always added: {ids:?}"
    );
    let research = design
        .plan
        .vlans
        .iter()
        .find(|v| v.id == 111)
        .expect("the VLAN that was asked for");
    assert_eq!(research.name, "RESEARCH");
    assert!(
        research.hosts_needed >= 50,
        "40 hosts plus 25 %: {research:?}"
    );
    assert!(
        design.plan.vlans.iter().all(|v| v.dhcp_first.is_empty()),
        "DHCP was turned off"
    );
}

#[test]
fn the_picture_reads_top_down_with_the_sites_side_by_side() {
    let design = generate(&Requirements {
        name: "Three".into(),
        sites: vec![
            Site {
                name: "North".into(),
                users: 60,
                guest_wifi: true,
                servers: 2,
                ..Site::default()
            },
            Site {
                name: "Middle".into(),
                users: 30,
                ..Site::default()
            },
            Site {
                name: "South".into(),
                users: 90,
                printers: 2,
                ..Site::default()
            },
        ],
        redundancy: Redundancy::Full,
        wan: Wan {
            topology: WanTopology::HubAndSpoke,
            hub: Some("North".into()),
            internet: InternetEdge::PerSite,
        },
        addressing: Addressing {
            base: "10.0.0.0/14".into(),
            ..Addressing::default()
        },
        ..Requirements::default()
    })
    .unwrap();

    // One band per layer, in that order, with the Internet above everything.
    let row = |role: DeviceRole| -> Vec<f32> {
        design
            .plan
            .devices
            .iter()
            .filter(|d| d.role == role)
            .filter_map(|d| {
                design
                    .lab
                    .nodes
                    .iter()
                    .find(|n| n.name == d.name)
                    .map(|n| n.y)
            })
            .collect()
    };
    let band = |role: DeviceRole| {
        let ys = row(role);
        assert!(!ys.is_empty(), "{role:?} has no devices");
        assert!(
            ys.iter().all(|y| (*y - ys[0]).abs() < f32::EPSILON),
            "{role:?} is not one row"
        );
        ys[0]
    };
    let internet = band(DeviceRole::Internet);
    let edge = band(DeviceRole::Edge);
    let core = band(DeviceRole::Core);
    let access = band(DeviceRole::Access);
    let hosts = band(DeviceRole::Host);
    assert!(
        internet < edge && edge < core && core < access && access < hosts,
        "{internet} {edge} {core} {access} {hosts}"
    );

    // Nothing sits on top of anything else.
    let mut seen: Vec<(i32, i32)> = Vec::new();
    for node in &design.lab.nodes {
        let key = (node.x.round() as i32, node.y.round() as i32);
        assert!(
            !seen.contains(&key),
            "{} overlaps another node at {key:?}",
            node.name
        );
        seen.push(key);
        assert!(
            node.x >= 0.0 && node.y >= 0.0,
            "{} is off the canvas",
            node.name
        );
    }

    // Each site keeps to its own column, and has a note box over it.
    let column = |site: &str| -> (f32, f32) {
        let xs: Vec<f32> = design
            .plan
            .devices
            .iter()
            .filter(|d| d.site == site && d.role != DeviceRole::Internet)
            .filter_map(|d| {
                design
                    .lab
                    .nodes
                    .iter()
                    .find(|n| n.name == d.name)
                    .map(|n| n.x)
            })
            .collect();
        (
            xs.iter().copied().fold(f32::MAX, f32::min),
            xs.iter().copied().fold(f32::MIN, f32::max),
        )
    };
    let (_, north_right) = column("North");
    let (middle_left, middle_right) = column("Middle");
    let (south_left, _) = column("South");
    assert!(north_right < middle_left, "North runs into Middle");
    assert!(middle_right < south_left, "Middle runs into South");
    assert_eq!(design.lab.notes.len(), 3, "one note box per site");
    for note in &design.lab.notes {
        assert!(
            note.w > 100.0 && note.h > 20.0 && !note.text.is_empty(),
            "{note:?}"
        );
    }
}

#[test]
fn the_hardening_baseline_is_on_every_device() {
    for (label, requirements) in nocturne_design::examples() {
        let design = generate(&requirements).unwrap();
        let configured: Vec<_> = design
            .lab
            .nodes
            .iter()
            .filter_map(|n| n.startup_config.as_ref().map(|c| (n.name.clone(), c)))
            .collect();
        assert!(configured.len() > 3, "{label}");
        for (name, config) in configured {
            let want = [
                "no ip domain-lookup",
                "enable secret CHANGEME-",
                "username netadmin privilege 15 secret CHANGEME-",
                "service password-encryption",
                "ip ssh version 2",
                "ip access-list standard MGMT-HOSTS",
                "banner motd ",
                "line vty 0 4",
                " login local",
                " transport input ssh",
                " access-class MGMT-HOSTS in",
                " exec-timeout 15 0",
            ];
            for line in want {
                assert!(
                    config.contains(line),
                    "{label} / {name}: the baseline is missing {line:?}"
                );
            }
            assert!(
                !config.contains("transport input telnet")
                    && !config.contains("transport input all"),
                "{label} / {name}: telnet is not switched off"
            );
            // Every interface that is in use says what is on the other end.
            let mut current = String::new();
            let mut described = true;
            for line in config.lines() {
                if let Some(rest) = line.strip_prefix("interface ") {
                    assert!(described, "{label} / {name}: {current} has no description");
                    current = rest.to_string();
                    described = false;
                } else if line.starts_with(" description ") {
                    described = true;
                }
            }
            assert!(described, "{label} / {name}: {current} has no description");
        }
        // The monitoring targets that were given are reachable through the
        // management access list.
        if let Some(host) = requirements.services.snmp.as_deref() {
            for (name, config) in design
                .lab
                .nodes
                .iter()
                .filter_map(|n| n.startup_config.as_ref().map(|c| (&n.name, c)))
            {
                assert!(
                    config.contains(&format!("permit host {host}")),
                    "{label} / {name}: the SNMP manager cannot reach the device"
                );
            }
        }
    }
}

#[test]
fn partial_json_is_enough_to_ask_for_a_design() {
    // What a user interface or a language model would send.
    let requirements: Requirements = serde_json::from_str(
        r#"{"name":"Acme branch",
            "sites":[{"name":"Branch","users":120,"voice":true,"guest_wifi":true}],
            "redundancy":"uplinks",
            "wan":{"internet":"per_site"},
            "routing":"ospf"}"#,
    )
    .expect("partial JSON has to be accepted");
    assert_eq!(requirements.redundancy, Redundancy::Uplinks);
    assert_eq!(requirements.routing, Routing::Ospf);
    assert_eq!(requirements.growth_pct, 25, "the defaults fill the rest in");
    assert_eq!(requirements.access_ports, 24);
    assert_eq!(requirements.hosts_per_vlan, 1);
    assert!(requirements.services.dhcp && requirements.services.nat);
    assert_eq!(requirements.addressing.base, "10.0.0.0/16");
    assert!(validate(&requirements).is_empty());

    let design = generate(&requirements).unwrap();
    // And everything comes back as JSON the UI can read, enums in snake_case.
    let json = serde_json::to_string(&design).unwrap();
    assert!(
        json.contains("\"access_uplink\"") && json.contains("\"reachable\""),
        "{json:.400}"
    );
    let back: nocturne_design::Design = serde_json::from_str(&json).unwrap();
    assert_eq!(back, design);

    // An empty object is a valid question with an unhelpful answer.
    let empty: Requirements = serde_json::from_str("{}").unwrap();
    assert_eq!(empty, Requirements::default());
    assert!(!validate(&empty).is_empty());

    for (label, requirements) in nocturne_design::examples() {
        assert!(validate(&requirements).is_empty(), "{label}");
    }
}
