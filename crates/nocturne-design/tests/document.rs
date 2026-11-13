//! `document` has to say exactly what the plan says, so the tables are parsed
//! back out of the Markdown and compared with it.

mod common;

use std::collections::BTreeMap;

use nocturne_design::{
    Design, Expect, Purpose, Redundancy, Requirements, Routing, Services, Site, document, examples,
    generate,
};

/// Rows of a GitHub-flavoured Markdown table, by the heading above it.
fn tables(markdown: &str) -> BTreeMap<String, Vec<Vec<String>>> {
    let mut out: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    let mut heading = String::new();
    let mut header: Option<Vec<String>> = None;
    for line in markdown.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            heading = rest.trim().to_string();
            header = None;
            continue;
        }
        if !line.starts_with('|') {
            header = None;
            continue;
        }
        let cells: Vec<String> = line
            .trim()
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(|c| c.trim().trim_matches('`').to_string())
            .collect();
        if header.is_none() {
            header = Some(cells);
            continue;
        }
        if cells
            .iter()
            .all(|c| c.chars().all(|ch| ch == '-' || ch == ':'))
        {
            continue;
        }
        out.entry(heading.clone()).or_default().push(cells);
    }
    out
}

fn mid_sized() -> Design {
    generate(&Requirements {
        name: "Riverside".into(),
        sites: vec![
            Site {
                name: "Head office".into(),
                users: 140,
                voice: true,
                guest_wifi: true,
                servers: 3,
                printers: 4,
                vlans: Vec::new(),
            },
            Site {
                name: "Workshop".into(),
                users: 40,
                voice: false,
                guest_wifi: true,
                servers: 0,
                printers: 2,
                vlans: Vec::new(),
            },
        ],
        redundancy: Redundancy::Full,
        wan: nocturne_design::Wan {
            topology: nocturne_design::WanTopology::HubAndSpoke,
            hub: Some("Head office".into()),
            internet: nocturne_design::InternetEdge::PerSite,
        },
        addressing: nocturne_design::Addressing {
            base: "10.40.0.0/14".into(),
            per_site_len: None,
            p2p_len: 31,
            mgmt_vlan: 99,
        },
        routing: Routing::Ospf,
        growth_pct: 25,
        access_ports: 24,
        services: Services {
            dhcp: true,
            nat: true,
            ntp: Some("10.40.0.10".into()),
            syslog: Some("10.40.0.11".into()),
            snmp: Some("10.40.0.11".into()),
        },
        hosts_per_vlan: 2,
        seed: 7,
    })
    .unwrap()
}

#[test]
fn every_table_matches_the_plan_it_came_from() {
    let design = mid_sized();
    let doc = document(&design);
    let tables = tables(&doc);

    let vlans = tables
        .get("VLANs and addressing")
        .expect("an addressing table");
    assert_eq!(vlans.len(), design.plan.vlans.len());
    for (row, vlan) in vlans.iter().zip(&design.plan.vlans) {
        assert_eq!(row[0], vlan.site);
        assert_eq!(row[1], vlan.id.to_string());
        assert_eq!(row[2], vlan.name);
        assert_eq!(row[4], vlan.subnet);
        assert_eq!(row[5], vlan.gateway);
        if vlan.dhcp_first.is_empty() {
            assert_eq!(row[6], "static");
        } else {
            assert_eq!(row[6], format!("{} – {}", vlan.dhcp_first, vlan.dhcp_last));
        }
        assert_eq!(row[7], vlan.hosts_needed.to_string());
        assert_eq!(row[8], vlan.hosts_available.to_string());
    }

    let sites = tables.get("Sites").expect("a site table");
    assert_eq!(sites.len(), design.plan.sites.len());
    for (row, site) in sites.iter().zip(&design.plan.sites) {
        assert!(row[0].starts_with(&site.name), "{row:?}");
        assert_eq!(row[1], site.users.to_string());
        assert_eq!(row[2], site.block);
    }

    let cabling = tables.get("Cabling").expect("a cabling table");
    assert_eq!(cabling.len(), design.plan.links.len());
    for (row, link) in cabling.iter().zip(&design.plan.links) {
        assert_eq!((&row[0], &row[1]), (&link.a, &link.a_if));
        assert_eq!((&row[2], &row[3]), (&link.b, &link.b_if));
    }

    let devices = tables.get("Devices").expect("a device table");
    let expected: Vec<_> = design
        .plan
        .devices
        .iter()
        .filter(|d| {
            d.role != nocturne_design::DeviceRole::Host
                && d.role != nocturne_design::DeviceRole::Internet
        })
        .collect();
    assert_eq!(devices.len(), expected.len());
    for (row, dev) in devices.iter().zip(&expected) {
        assert_eq!(row[0], dev.name);
        assert_eq!(row[3], dev.model);
    }

    let hardware = tables.get("Hardware").expect("a hardware table");
    assert_eq!(hardware.len(), design.plan.bill_of_materials.len());
    for (row, line) in hardware.iter().zip(&design.plan.bill_of_materials) {
        assert_eq!(row[0], line.class);
        assert_eq!(row[1], line.quantity.to_string());
    }

    let checks = tables
        .get("What a correct build satisfies")
        .expect("a check table");
    assert_eq!(checks.len(), design.plan.checks.len());
    for (row, check) in checks.iter().zip(&design.plan.checks) {
        assert_eq!(row[0], check.from);
        assert!(row[1].starts_with(&check.to), "{row:?}");
        assert_eq!(
            row[3],
            match check.expect {
                Expect::Reachable => "reachable",
                Expect::Blocked => "blocked",
            }
        );
    }
}

#[test]
fn the_document_tells_the_reader_to_replace_the_passwords() {
    let design = mid_sized();
    let doc = document(&design);
    assert!(doc.contains("## Before you deploy this"));
    assert!(doc.contains("CHANGEME-enable"));
    assert!(doc.contains("CHANGEME-netadmin"));
    assert!(doc.contains("Replace all of"), "{doc:.400}");
    // And every placeholder that appears in a configuration is listed.
    for node in &design.lab.nodes {
        let Some(config) = &node.startup_config else {
            continue;
        };
        for word in config.split_whitespace() {
            if word.starts_with("CHANGEME-") {
                assert!(
                    doc.contains(word),
                    "{word} is not explained in the document"
                );
            }
        }
    }
}

#[test]
fn the_document_holds_every_configuration_and_the_reasoning() {
    let design = mid_sized();
    let doc = document(&design);
    let configured = design
        .lab
        .nodes
        .iter()
        .filter(|n| n.startup_config.is_some())
        .count();
    assert_eq!(
        doc.matches("\n### ").count(),
        configured,
        "one section per configured device"
    );
    for node in &design.lab.nodes {
        if let Some(config) = &node.startup_config {
            assert!(doc.contains(config.trim_end()), "{} is missing", node.name);
        }
    }
    for decision in &design.decisions {
        assert!(
            doc.contains(&decision.text),
            "the reasoning is not in the document: {decision:?}"
        );
    }
    for warning in &design.warnings {
        assert!(doc.contains(warning), "the warning is not in the document");
    }
    assert!(doc.contains("## Why it is built this way"));
    assert!(doc.contains("## Read this part twice"));
    // Fences balance, so the Markdown renders.
    assert_eq!(doc.matches("\n```").count() % 2, 0);
}

#[test]
fn the_document_is_deterministic_to_the_byte() {
    for (label, requirements) in examples() {
        let a = document(&generate(&requirements).unwrap());
        let b = document(&generate(&requirements).unwrap());
        assert_eq!(a, b, "{label}");
        assert!(
            a.len() > 2000,
            "{label}: {} bytes is not a document",
            a.len()
        );
    }
}

#[test]
fn a_design_with_nothing_in_it_still_documents_cleanly() {
    let design = generate(&Requirements {
        name: "Cupboard".into(),
        sites: vec![Site {
            name: "Cupboard".into(),
            ..Site::default()
        }],
        services: Services {
            dhcp: false,
            nat: false,
            ntp: None,
            syslog: None,
            snmp: None,
        },
        ..Requirements::default()
    })
    .unwrap();
    let doc = document(&design);
    let tables = tables(&doc);
    assert_eq!(tables.get("Sites").map(Vec::len), Some(1));
    assert!(
        design
            .plan
            .vlans
            .iter()
            .all(|v| v.purpose == Purpose::Management),
        "a site with nobody in it only needs a management VLAN"
    );
    assert!(doc.contains("## Cabling"));
}
