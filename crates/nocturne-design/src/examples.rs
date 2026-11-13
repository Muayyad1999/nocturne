//! Worked examples: a starting point for the user interface and a fixture for
//! the tests.

use crate::requirements::{
    Addressing, InternetEdge, Redundancy, Requirements, Routing, Services, Site, Wan, WanTopology,
};

fn site(name: &str, users: u32, voice: bool, guest: bool, servers: u32, printers: u32) -> Site {
    Site {
        name: name.to_string(),
        users,
        voice,
        guest_wifi: guest,
        servers,
        printers,
        vlans: Vec::new(),
    }
}

/// Three designs that between them cover what most people ask for.
pub fn examples() -> Vec<(String, Requirements)> {
    vec![
        (
            "Small office".to_string(),
            Requirements {
                name: "Small office".to_string(),
                sites: vec![site("Office", 24, false, true, 1, 2)],
                redundancy: Redundancy::None,
                wan: Wan {
                    topology: WanTopology::None,
                    hub: None,
                    internet: InternetEdge::PerSite,
                },
                addressing: Addressing {
                    base: "10.10.0.0/16".to_string(),
                    per_site_len: None,
                    p2p_len: 30,
                    mgmt_vlan: 99,
                },
                routing: Routing::Static,
                growth_pct: 25,
                access_ports: 24,
                services: Services::default(),
                hosts_per_vlan: 1,
                seed: 1,
            },
        ),
        (
            "Campus with two buildings".to_string(),
            Requirements {
                name: "Riverside campus".to_string(),
                sites: vec![
                    site("Building A", 240, true, true, 8, 6),
                    site("Building B", 160, true, true, 0, 4),
                ],
                redundancy: Redundancy::Full,
                wan: Wan {
                    topology: WanTopology::FullMesh,
                    hub: Some("Building A".to_string()),
                    internet: InternetEdge::AtHub,
                },
                addressing: Addressing {
                    base: "10.20.0.0/15".to_string(),
                    per_site_len: None,
                    p2p_len: 31,
                    mgmt_vlan: 99,
                },
                routing: Routing::Ospf,
                growth_pct: 30,
                access_ports: 24,
                services: Services {
                    dhcp: true,
                    nat: true,
                    ntp: Some("10.20.0.10".to_string()),
                    syslog: Some("10.20.0.11".to_string()),
                    snmp: Some("10.20.0.11".to_string()),
                },
                hosts_per_vlan: 2,
                seed: 2,
            },
        ),
        (
            "HQ and three branches".to_string(),
            Requirements {
                name: "Acme".to_string(),
                sites: vec![
                    site("HQ", 300, true, true, 12, 10),
                    site("Leeds", 120, true, true, 0, 3),
                    site("Cardiff", 60, true, false, 0, 2),
                    site("Belfast", 25, false, true, 0, 1),
                ],
                redundancy: Redundancy::Uplinks,
                wan: Wan {
                    topology: WanTopology::HubAndSpoke,
                    hub: Some("HQ".to_string()),
                    internet: InternetEdge::PerSite,
                },
                addressing: Addressing {
                    base: "10.30.0.0/14".to_string(),
                    per_site_len: None,
                    p2p_len: 30,
                    mgmt_vlan: 99,
                },
                routing: Routing::Ospf,
                growth_pct: 25,
                access_ports: 24,
                services: Services {
                    dhcp: true,
                    nat: true,
                    ntp: Some("10.30.0.10".to_string()),
                    syslog: Some("10.30.0.11".to_string()),
                    snmp: None,
                },
                hosts_per_vlan: 1,
                seed: 3,
            },
        ),
    ]
}
