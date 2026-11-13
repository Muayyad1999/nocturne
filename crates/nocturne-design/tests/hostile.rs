//! Requirements nobody would write on purpose: nothing may panic, nothing may
//! loop, and whatever cannot be built has to be explained in words a network
//! engineer would use.

mod common;

use common::{assert_invariants, assert_no_real_secrets, boot, failures};
use nocturne_design::{
    Addressing, InternetEdge, Purpose, Redundancy, Requirements, Routing, Services, Site, VlanReq,
    Wan, WanTopology, document, generate, validate,
};

fn site(users: u32) -> Site {
    Site {
        name: "Site".into(),
        users,
        ..Site::default()
    }
}

/// Every problem is reported at once, so one pass is enough to fix them all.
#[test]
fn problems_are_reported_together_and_in_words() {
    let problems = validate(&Requirements {
        name: String::new(),
        sites: Vec::new(),
        addressing: Addressing {
            base: "2001:db8::/32".into(),
            p2p_len: 8,
            mgmt_vlan: 9999,
            per_site_len: None,
        },
        access_ports: 7,
        ..Requirements::default()
    });
    assert!(problems.len() >= 4, "{problems:#?}");
    let all = problems.join("\n");
    for expected in ["no sites", "IPv4", "/30", "1 to 4094", "24 or 48"] {
        assert!(
            all.contains(expected),
            "{expected:?} is missing from:\n{all}"
        );
    }
    // Nothing is shouted in code; the messages read like sentences.
    for problem in &problems {
        assert!(
            !problem.contains("Err") && !problem.contains("unwrap") && problem.len() > 20,
            "{problem:?}"
        );
        assert!(
            problem
                .chars()
                .next()
                .is_some_and(|c| c.is_lowercase() || c == '"'),
            "a problem list reads as a list: {problem:?}"
        );
    }
    assert!(generate(&Requirements::default()).is_err());
}

#[test]
fn nothing_user_typed_reaches_a_configuration_line() {
    let nasty = "Branch\nhostname EVIL\nno ip access-list standard MGMT-HOSTS\n";
    let design = generate(&Requirements {
        name: nasty.into(),
        sites: vec![Site {
            name: nasty.into(),
            users: 4,
            vlans: vec![VlanReq {
                name: "X\n no shutdown\n".into(),
                purpose: Purpose::Users,
                hosts: 3,
                id: None,
            }],
            ..Site::default()
        }],
        ..Requirements::default()
    })
    .unwrap();
    for node in &design.lab.nodes {
        let Some(config) = &node.startup_config else {
            continue;
        };
        for line in config.lines() {
            // The injected text may survive as a label or inside a banner,
            // but never as a command of its own.
            assert!(
                line.trim() != "hostname EVIL"
                    && line.trim() != "no ip access-list standard MGMT-HOSTS",
                "{}: an injected command got through: {line:?}",
                node.name
            );
        }
        assert_eq!(
            config
                .lines()
                .filter(|l| l.starts_with("hostname "))
                .count(),
            1,
            "exactly one hostname line, not two:\n{config}"
        );
        let banners = config.lines().filter(|l| l.starts_with("banner ")).count();
        assert_eq!(banners, 1, "{}:\n{config}", node.name);
    }
    assert_no_real_secrets(&design, "nasty");
    // And it still works.
    let mut lab = boot(&design, "nasty");
    assert!(failures(&design, &mut lab).is_empty());
    // The document keeps the name the user typed — as prose, on one line,
    // never as a line that could be pasted into a device.
    let doc = document(&design);
    let title = doc.lines().next().unwrap_or_default();
    assert!(
        title.starts_with("# ") && title.contains("Branch hostname EVIL"),
        "{title:?}"
    );
    for line in doc.lines() {
        assert_ne!(line.trim(), "hostname EVIL");
        assert_ne!(line.trim(), "no ip access-list standard MGMT-HOSTS");
    }
}

#[test]
fn absurd_numbers_are_refused_or_absorbed_without_panicking() {
    let cases: Vec<(&str, Requirements)> = vec![
        (
            "no users anywhere",
            Requirements {
                sites: vec![site(0), site(0)],
                wan: Wan {
                    topology: WanTopology::FullMesh,
                    hub: None,
                    internet: InternetEdge::PerSite,
                },
                ..Requirements::default()
            },
        ),
        (
            "every count at its maximum",
            Requirements {
                sites: vec![Site {
                    name: "Huge".into(),
                    users: u32::MAX,
                    voice: true,
                    guest_wifi: true,
                    servers: u32::MAX,
                    printers: u32::MAX,
                    vlans: Vec::new(),
                }],
                growth_pct: u32::MAX,
                hosts_per_vlan: u32::MAX,
                ..Requirements::default()
            },
        ),
        (
            "a /32 base block",
            Requirements {
                sites: vec![site(10)],
                addressing: Addressing {
                    base: "10.0.0.1/32".into(),
                    ..Addressing::default()
                },
                ..Requirements::default()
            },
        ),
        (
            "the whole internet as a base block",
            Requirements {
                sites: vec![site(10)],
                addressing: Addressing {
                    base: "0.0.0.0/0".into(),
                    ..Addressing::default()
                },
                ..Requirements::default()
            },
        ),
        (
            "thirteen sites",
            Requirements {
                sites: (0..13).map(|_| site(10)).collect(),
                ..Requirements::default()
            },
        ),
        (
            "two sites with the same name",
            Requirements {
                sites: vec![site(10), site(10)],
                ..Requirements::default()
            },
        ),
        (
            "a hub that is not a site",
            Requirements {
                sites: vec![
                    site(10),
                    Site {
                        name: "Other".into(),
                        users: 5,
                        ..Site::default()
                    },
                ],
                wan: Wan {
                    topology: WanTopology::HubAndSpoke,
                    hub: Some("Nowhere".into()),
                    internet: InternetEdge::AtHub,
                },
                ..Requirements::default()
            },
        ),
        (
            "the Internet at a hub with no WAN",
            Requirements {
                sites: vec![
                    site(10),
                    Site {
                        name: "Other".into(),
                        users: 5,
                        ..Site::default()
                    },
                ],
                wan: Wan {
                    topology: WanTopology::None,
                    hub: None,
                    internet: InternetEdge::AtHub,
                },
                ..Requirements::default()
            },
        ),
        (
            "two VLANs with the same id",
            Requirements {
                sites: vec![Site {
                    name: "Clash".into(),
                    users: 10,
                    vlans: vec![
                        VlanReq {
                            name: "A".into(),
                            purpose: Purpose::Users,
                            hosts: 5,
                            id: Some(77),
                        },
                        VlanReq {
                            name: "B".into(),
                            purpose: Purpose::Servers,
                            hosts: 5,
                            id: Some(77),
                        },
                    ],
                    ..Site::default()
                }],
                ..Requirements::default()
            },
        ),
        (
            "a VLAN id of zero and one past the end",
            Requirements {
                sites: vec![Site {
                    name: "Edges".into(),
                    users: 10,
                    vlans: vec![
                        VlanReq {
                            name: "A".into(),
                            purpose: Purpose::Users,
                            hosts: 5,
                            id: Some(0),
                        },
                        VlanReq {
                            name: "B".into(),
                            purpose: Purpose::Servers,
                            hosts: 5,
                            id: Some(4095),
                        },
                    ],
                    ..Site::default()
                }],
                ..Requirements::default()
            },
        ),
        (
            "VLAN names that collide once they are sanitised",
            Requirements {
                sites: vec![Site {
                    name: "Dup".into(),
                    users: 10,
                    vlans: vec![
                        VlanReq {
                            name: "data net".into(),
                            purpose: Purpose::Users,
                            hosts: 5,
                            id: None,
                        },
                        VlanReq {
                            name: "DATA-NET".into(),
                            purpose: Purpose::Other,
                            hosts: 5,
                            id: None,
                        },
                        VlanReq {
                            name: "!!!".into(),
                            purpose: Purpose::Other,
                            hosts: 5,
                            id: None,
                        },
                        VlanReq {
                            name: String::new(),
                            purpose: Purpose::Other,
                            hosts: 0,
                            id: None,
                        },
                    ],
                    ..Site::default()
                }],
                ..Requirements::default()
            },
        ),
        (
            "no services at all and no Internet",
            Requirements {
                sites: vec![site(10)],
                services: Services {
                    dhcp: false,
                    nat: false,
                    ntp: Some("not an address".into()),
                    syslog: Some(String::new()),
                    snmp: Some("8.8.8.8".into()),
                },
                wan: Wan {
                    topology: WanTopology::None,
                    hub: None,
                    internet: InternetEdge::None,
                },
                routing: Routing::Ospf,
                redundancy: Redundancy::Full,
                ..Requirements::default()
            },
        ),
    ];

    for (label, requirements) in cases {
        let problems = validate(&requirements);
        match generate(&requirements) {
            Ok(design) => {
                assert!(
                    problems.is_empty(),
                    "{label}: validate complained but generate succeeded: {problems:?}"
                );
                assert_invariants(&design, label);
                let doc = document(&design);
                assert!(doc.len() > 200, "{label}: the document is empty");
            }
            Err(error) => {
                assert!(
                    !problems.is_empty(),
                    "{label}: generate failed but validate was happy"
                );
                assert_eq!(error.problems(), problems, "{label}");
                for problem in &problems {
                    assert!(problem.len() > 15, "{label}: {problem:?}");
                }
            }
        }
    }
}

/// A design built from hostile but buildable requirements still has to boot.
#[test]
fn the_awkward_ones_still_boot() {
    let awkward = [
        Requirements {
            name: "  ".into(),
            sites: vec![Site {
                name: "\t\n".into(),
                users: 1,
                voice: true,
                guest_wifi: true,
                servers: 1,
                printers: 1,
                vlans: Vec::new(),
            }],
            growth_pct: 0,
            hosts_per_vlan: 99,
            addressing: Addressing {
                base: "172.16.0.0/20".into(),
                per_site_len: None,
                p2p_len: 31,
                mgmt_vlan: 1,
            },
            redundancy: Redundancy::Full,
            routing: Routing::Ospf,
            ..Requirements::default()
        },
        Requirements {
            name: "Explicit everything".into(),
            sites: vec![Site {
                name: "One".into(),
                users: 0,
                voice: true,
                guest_wifi: false,
                servers: 0,
                printers: 0,
                vlans: vec![
                    VlanReq {
                        name: "Tills".into(),
                        purpose: Purpose::Other,
                        hosts: 1,
                        id: Some(4094),
                    },
                    VlanReq {
                        name: "Cameras".into(),
                        purpose: Purpose::Other,
                        hosts: 300,
                        id: Some(2),
                    },
                ],
            }],
            addressing: Addressing {
                base: "192.168.0.0/17".into(),
                per_site_len: Some(18),
                p2p_len: 30,
                mgmt_vlan: 4000,
            },
            access_ports: 48,
            ..Requirements::default()
        },
    ];
    for (i, requirements) in awkward.iter().enumerate() {
        let label = format!("awkward {i}");
        let design = generate(requirements).unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_invariants(&design, &label);
        let mut lab = boot(&design, &label);
        let problems = failures(&design, &mut lab);
        assert!(problems.is_empty(), "{label}:\n{}", problems.join("\n"));
    }
}

/// Several thousand pseudo-random requirements: no panic, no hang, and any
/// design that does come out is internally consistent.
#[test]
fn random_requirements_never_panic() {
    let mut state = 0x1234_5678_9abc_def0u64;
    let mut next = move || {
        // xorshift64*, so the run is reproducible.
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let names = [
        "",
        " ",
        "HQ",
        "hq",
        "Head Office",
        "\n",
        "a\0b",
        "Köln",
        "x".repeat(200).leak(),
        "10.0.0.0/8",
        "--",
        "Site\nhostname X",
    ];
    // Mostly buildable, with a steady drip of nonsense: a run that only ever
    // refuses proves nothing about the code that builds a design.
    let bases = [
        "10.0.0.0/8",
        "10.0.0.0/12",
        "10.0.0.0/12",
        "10.0.0.0/16",
        "10.0.0.0/12",
        "172.16.0.0/12",
        "10.0.0.0/24",
        "10.0.0.0/32",
        "0.0.0.0/0",
        "2001:db8::/32",
        "999.1.1.1/8",
        "10.0.0.0",
        "/16",
        "",
    ];
    let mut built = 0usize;
    let mut refused = 0usize;
    for _ in 0..3000 {
        let r = next();
        // One run in eight is allowed to be nonsense; the rest have to be
        // buildable, or the loop would only ever test the refusal path.
        let nonsense = |bit: u32| (r >> bit) & 7 == 0;
        let sites = 1 + (r % 4) as usize;
        let requirements = Requirements {
            name: names[(r >> 8) as usize % names.len()].to_string(),
            sites: (0..sites)
                .map(|i| {
                    let s = next();
                    Site {
                        // Numbered, so most runs have usable, distinct names.
                        name: format!("{} {i}", names[(s >> (i % 8)) as usize % names.len()]),
                        users: match s % 8 {
                            0 => 0,
                            1 => (s >> 16) as u32 % 50,
                            2 => (s >> 16) as u32 % 900,
                            3 if nonsense(4) => u32::MAX,
                            _ => (s >> 16) as u32 % 300,
                        },
                        voice: s & 1 == 1,
                        guest_wifi: s & 2 == 2,
                        servers: (s >> 20) as u32 % 7,
                        printers: (s >> 24) as u32 % 5,
                        vlans: if s & 4 == 4 {
                            vec![VlanReq {
                                name: names[(s >> 32) as usize % names.len()].to_string(),
                                purpose: Purpose::Users,
                                hosts: (s >> 40) as u32 % 2000,
                                id: if s & 8 == 8 {
                                    Some((s >> 44) as u16 % 5000)
                                } else {
                                    None
                                },
                            }]
                        } else {
                            Vec::new()
                        },
                    }
                })
                .collect(),
            redundancy: [Redundancy::None, Redundancy::Uplinks, Redundancy::Full]
                [(r >> 16) as usize % 3],
            wan: Wan {
                topology: [
                    WanTopology::None,
                    WanTopology::HubAndSpoke,
                    WanTopology::FullMesh,
                ][(r >> 20) as usize % 3],
                hub: if r & 0x10 == 0 {
                    None
                } else {
                    Some(names[(r >> 24) as usize % names.len()].to_string())
                },
                internet: [
                    InternetEdge::None,
                    InternetEdge::AtHub,
                    InternetEdge::PerSite,
                ][(r >> 28) as usize % 3],
            },
            addressing: Addressing {
                base: if nonsense(8) {
                    bases[(r >> 32) as usize % bases.len()].to_string()
                } else {
                    ["10.0.0.0/12", "10.0.0.0/8", "172.16.0.0/12"][(r >> 32) as usize % 3]
                        .to_string()
                },
                per_site_len: (nonsense(12)).then_some((r >> 36) as u8 % 40),
                p2p_len: if nonsense(16) {
                    [24u8, 0, 32][(r >> 40) as usize % 3]
                } else {
                    [30u8, 31][(r >> 40) as usize % 2]
                },
                mgmt_vlan: if nonsense(20) {
                    [0u16, 9999][(r >> 44) as usize % 2]
                } else {
                    [99u16, 900, 4094, 1][(r >> 44) as usize % 4]
                },
            },
            routing: [Routing::Static, Routing::Ospf][(r >> 48) as usize % 2],
            growth_pct: if nonsense(24) {
                [10_000u32, u32::MAX][(r >> 50) as usize % 2]
            } else {
                [0u32, 25, 100][(r >> 50) as usize % 3]
            },
            access_ports: if nonsense(28) {
                [0u32, 1, 10_000][(r >> 52) as usize % 3]
            } else {
                [24u32, 48][(r >> 52) as usize % 2]
            },
            services: Services {
                dhcp: r & 0x40 == 0,
                nat: r & 0x80 == 0,
                ntp: None,
                syslog: None,
                snmp: None,
            },
            hosts_per_vlan: (r >> 56) as u32 % 7,
            seed: r,
        };
        let problems = validate(&requirements);
        match generate(&requirements) {
            Ok(design) => {
                built += 1;
                assert!(problems.is_empty(), "{problems:?} for {requirements:?}");
                // Cheap consistency: the plan has to agree with the lab.
                assert_eq!(design.plan.devices.len(), design.lab.nodes.len());
                assert_eq!(design.plan.links.len(), design.lab.links.len());
                for node in &design.lab.nodes {
                    assert!(!node.name.is_empty() && node.name.len() <= 63);
                }
            }
            Err(error) => {
                refused += 1;
                assert_eq!(error.problems(), problems);
                assert!(!problems.is_empty());
            }
        }
    }
    assert!(
        built > 300 && refused > 300,
        "the run has to exercise both paths: built {built}, refused {refused}"
    );
}
