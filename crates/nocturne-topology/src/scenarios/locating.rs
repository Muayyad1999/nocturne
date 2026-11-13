//! `locate.host`: which switch port is this plugged into, and how we know.

use nocturne_app::App;
use nocturne_drivers::model::Kind;

use super::*;
use crate::locate::{self, Located, Params};

const HOST_MAC: &str = "00:1b:21:aa:00:55";
const HOST_IP: &str = "10.1.10.55";

/// A core switch and two access switches. The host is on ACC01 Gi1/0/5; the
/// core and ACC02 only see the MAC on their trunks.
struct Campus3 {
    app: App,
    core: String,
    acc1: String,
    acc2: String,
}

fn three_switches() -> Campus3 {
    let app = app();
    let core = device(&app, "CORE1", "10.1.10.1", "core", "HQ");
    let acc1 = device(&app, "ACC01", "10.1.10.11", "access", "HQ");
    let acc2 = device(&app, "ACC02", "10.1.10.12", "access", "HQ");

    store(
        &app,
        &core,
        vec![
            (
                Kind::Neighbors,
                vec![
                    neighbor("Gi1/0/1", "ACC01", "Gi1/0/48", "lldp"),
                    neighbor("Gi1/0/2", "ACC02", "Gi1/0/48", "lldp"),
                ],
            ),
            (
                Kind::Interfaces,
                vec![
                    trunk("Gi1/0/1", 1_000_000_000),
                    trunk("Gi1/0/2", 1_000_000_000),
                    routed("Vlan10", "10.1.10.1/24"),
                ],
            ),
            (Kind::Arp, vec![arp_entry(HOST_IP, HOST_MAC, "Vlan10")]),
            (Kind::MacTable, vec![mac_entry(HOST_MAC, "Gi1/0/1", 10)]),
        ],
    );
    store(
        &app,
        &acc1,
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
                    mac_entry(HOST_MAC, "Gi1/0/5", 10),
                    mac_entry(HOST_MAC, "Gi1/0/48", 10),
                ],
            ),
        ],
    );
    store(
        &app,
        &acc2,
        vec![
            (
                Kind::Neighbors,
                vec![neighbor("Gi1/0/48", "CORE1", "Gi1/0/2", "lldp")],
            ),
            (Kind::Interfaces, vec![trunk("Gi1/0/48", 1_000_000_000)]),
            (Kind::MacTable, vec![mac_entry(HOST_MAC, "Gi1/0/48", 10)]),
        ],
    );
    Campus3 {
        app,
        core,
        acc1,
        acc2,
    }
}

fn locate(app: &App, query: &str) -> Located {
    locate::locate(
        app,
        &Params {
            query: query.to_string(),
        },
    )
    .expect("the search runs")
}

#[tokio::test]
async fn an_ip_is_followed_through_arp_to_the_edge_port() {
    let net = three_switches();
    let found = locate(&net.app, HOST_IP);
    assert_eq!(found.mac.as_deref(), Some("00:1b:21:aa:00:55"));
    assert_eq!(found.ip.as_deref(), Some(HOST_IP));
    assert_eq!(found.locations.len(), 4, "{:#?}", found.locations);

    let best = found.best.expect("a best port");
    let chosen = &found.locations[best];
    assert_eq!(chosen.device, "ACC01");
    assert_eq!(chosen.interface, "GigabitEthernet1/0/5");
    assert_eq!(chosen.kind, "access");
    assert_eq!(chosen.vlan, Some(10));
    assert_eq!(chosen.macs_on_port, 1);
    assert_eq!(chosen.device_id, net.acc1);

    // Everything else is a trunk or an uplink, and is reported as the path.
    let others: Vec<&str> = found
        .locations
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != best)
        .map(|(_, location)| location.kind.as_str())
        .collect();
    assert_eq!(
        others,
        ["uplink", "uplink", "uplink"],
        "{:#?}",
        found.locations
    );

    let how = found.how.join("\n");
    assert!(
        how.contains("is at 00:1b:21:aa:00:55 in the ARP table of CORE1"),
        "{how}"
    );
    assert!(how.contains("in the MAC table of 4 ports"), "{how}");
    assert!(
        how.contains("ACC01 GigabitEthernet1/0/5 is the answer"),
        "{how}"
    );
    assert!(
        how.contains("path frames take to reach it, not where it is plugged in"),
        "{how}"
    );
    let _ = (&net.core, &net.acc2);
}

#[tokio::test]
async fn a_mac_in_any_notation_finds_the_same_port() {
    let net = three_switches();
    for query in [
        "00:1b:21:aa:00:55",
        "001b.21aa.0055",
        "00-1B-21-AA-00-55",
        "001b21aa0055",
    ] {
        let found = locate(&net.app, query);
        let best = found.best.expect("a best port");
        assert_eq!(
            (
                found.locations[best].device.as_str(),
                found.locations[best].interface.as_str()
            ),
            ("ACC01", "GigabitEthernet1/0/5"),
            "{query}"
        );
        assert_eq!(found.ip.as_deref(), Some(HOST_IP), "{query}");
        assert!(found.how[0].contains("read as the MAC address"), "{query}");
    }
}

#[tokio::test]
async fn a_port_with_many_macs_is_not_the_answer() {
    let net = three_switches();
    // ACC02 learns the MAC on an unconfigured port that carries 40 others.
    let mut rows = vec![
        mac_entry(HOST_MAC, "Gi1/0/48", 10),
        mac_entry(HOST_MAC, "Gi1/0/24", 10),
    ];
    for index in 0..40u16 {
        rows.push(mac_entry(
            &format!("00:1b:21:bb:{:02x}:{:02x}", index / 256, index % 256),
            "Gi1/0/24",
            10,
        ));
    }
    store(&net.app, &net.acc2, vec![(Kind::MacTable, rows)]);

    let found = locate(&net.app, HOST_IP);
    let best = found.best.expect("a best port");
    assert_eq!(found.locations[best].device, "ACC01");
    let crowded = found
        .locations
        .iter()
        .find(|location| location.interface == "GigabitEthernet1/0/24")
        .expect("the crowded port is still listed");
    assert_eq!(crowded.kind, "trunk");
    assert!(
        crowded.why.contains("41 MAC addresses are learnt here"),
        "{}",
        crowded.why
    );
}

#[tokio::test]
async fn with_no_edge_port_the_best_guess_says_it_is_a_guess() {
    let net = three_switches();
    // ACC01 forgets its access port: the MAC is only on trunks now.
    store(
        &net.app,
        &net.acc1,
        vec![(Kind::MacTable, vec![mac_entry(HOST_MAC, "Gi1/0/48", 10)])],
    );
    let found = locate(&net.app, HOST_IP);
    let best = found.best.expect("a best guess");
    assert_eq!(found.locations[best].kind, "uplink");
    let how = found.how.join("\n");
    assert!(
        how.contains("best guess, not an answer") && how.contains("collect `neighbors`"),
        "{how}"
    );
}

#[tokio::test]
async fn an_address_with_no_arp_entry_says_what_to_collect() {
    let net = three_switches();
    let error = locate::locate(
        &net.app,
        &Params {
            query: "10.1.10.200".into(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "not_found");
    assert!(
        error.message.contains("no collected ARP table") && error.message.contains("`arp`"),
        "{error}"
    );
    assert_eq!(error.details.as_ref().expect("details")["step"], "arp");

    // A MAC nobody has seen is not an error: it is an answer with no locations.
    let found = locate(&net.app, "00:1b:21:ff:ff:ff");
    assert!(found.locations.is_empty() && found.best.is_none());
    assert!(
        found
            .how
            .iter()
            .any(|step| step.contains("is in no collected MAC table")
                && step.contains("`mac_table`")),
        "{:#?}",
        found.how
    );
}

#[tokio::test]
async fn a_device_name_finds_where_that_device_is_plugged_in() {
    let net = three_switches();
    store(
        &net.app,
        &net.acc1,
        vec![(
            Kind::Interfaces,
            vec![
                trunk("Gi1/0/48", 1_000_000_000),
                access_port("Gi1/0/5", 10),
                serde_json::json!({"name": "Vlan1", "mac": "00:1b:21:ac:c0:01"}),
            ],
        )],
    );
    store(
        &net.app,
        &net.core,
        vec![(
            Kind::MacTable,
            vec![
                mac_entry(HOST_MAC, "Gi1/0/1", 10),
                mac_entry("00:1b:21:ac:c0:01", "Gi1/0/1", 1),
            ],
        )],
    );

    let found = locate(&net.app, "ACC01");
    assert_eq!(found.mac.as_deref(), Some("00:1b:21:ac:c0:01"));
    let how = found.how.join("\n");
    assert!(
        how.contains("is the device ACC01 in the inventory"),
        "{how}"
    );
    assert!(
        found
            .locations
            .iter()
            .any(|location| location.device == "CORE1"
                && location.interface == "GigabitEthernet1/0/1"),
        "{:#?}",
        found.locations
    );

    // Anything that is neither an address nor a device is refused plainly.
    let error = locate::locate(
        &net.app,
        &Params {
            query: "printer-upstairs".into(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code, "not_found");
    assert!(error.message.contains("does not resolve names"), "{error}");
    assert_eq!(
        locate::locate(&net.app, &Params { query: "  ".into() })
            .unwrap_err()
            .code,
        "invalid"
    );
}

#[tokio::test]
async fn the_vendor_of_the_card_is_reported() {
    let net = three_switches();
    let found = locate(&net.app, HOST_IP);
    // 00:1b:21 is a real IEEE assignment, so the registry has a name for it.
    assert!(found.vendor.is_some(), "{found:#?}");
}
