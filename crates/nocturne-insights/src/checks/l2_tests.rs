//! VLAN and trunk checks, case by case.

use nocturne_drivers::model::Interface;

use crate::testdata::*;
use crate::world::World;

/// A trunk port as the collectors report one.
fn trunk(name: &str, native: Option<u64>) -> Interface {
    Interface {
        mode: "trunk".into(),
        vlan: native,
        ..port(name, 1_000_000_000, "full")
    }
}

fn access(name: &str, vlan: u64) -> Interface {
    Interface {
        mode: "access".into(),
        vlan: Some(vlan),
        ..port(name, 1_000_000_000, "full")
    }
}

/// Two switches with a trunk between them, each configured as given.
fn trunked(a_native: Option<u64>, a_config: &str, b_native: Option<u64>, b_config: &str) -> World {
    world(vec![
        device("a", "SW1")
            .interfaces(vec![trunk("GigabitEthernet0/1", a_native)])
            .neighbors(vec![neighbor(
                "GigabitEthernet0/1",
                "SW2",
                "GigabitEthernet0/2",
            )])
            .vlans(vec![
                vlan(10, "users"),
                vlan(20, "voice"),
                vlan(30, "guests"),
            ])
            .config(a_config),
        device("b", "SW2")
            .interfaces(vec![trunk("GigabitEthernet0/2", b_native)])
            .neighbors(vec![neighbor(
                "GigabitEthernet0/2",
                "SW1",
                "GigabitEthernet0/1",
            )])
            .vlans(vec![
                vlan(10, "users"),
                vlan(20, "voice"),
                vlan(30, "guests"),
            ])
            .config(b_config),
    ])
}

const TRUNK_A: &str = "interface GigabitEthernet0/1\n switchport mode trunk\n";
const TRUNK_B: &str = "interface GigabitEthernet0/2\n switchport mode trunk\n";

// ------------------------------------------------------------- native VLAN

#[test]
fn native_vlan_mismatch_table() {
    // Positive: the two ends put untagged frames in different VLANs.
    let found = run(
        "native_vlan_mismatch",
        &trunked(Some(1), TRUNK_A, Some(99), TRUNK_B),
    );
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    assert!(found.findings[0].title.contains("99"));

    // Negative: both ends on 99.
    assert!(
        run(
            "native_vlan_mismatch",
            &trunked(Some(99), TRUNK_A, Some(99), TRUNK_B)
        )
        .findings
        .is_empty()
    );

    // Look-alike: one end says `native vlan 1`, the other says nothing. On an
    // IOS-style trunk that *is* VLAN 1, so the two agree.
    let implicit = world(vec![
        device("a", "SW1")
            .interfaces(vec![trunk("GigabitEthernet0/1", None)])
            .neighbors(vec![neighbor("GigabitEthernet0/1", "SW2", "GigabitEthernet0/2")])
            .config("interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk native vlan 1\n"),
        device("b", "SW2")
            .interfaces(vec![trunk("GigabitEthernet0/2", None)])
            .neighbors(vec![neighbor("GigabitEthernet0/2", "SW1", "GigabitEthernet0/1")])
            .config(TRUNK_B),
    ]);
    let report = run("native_vlan_mismatch", &implicit);
    assert!(report.findings.is_empty());
    assert_eq!(report.evaluated().len(), 2, "both ends were decided for");

    // Not enough data: one end is not known to be a trunk.
    let unknown = world(vec![
        device("a", "SW1")
            .interfaces(vec![trunk("GigabitEthernet0/1", Some(99))])
            .neighbors(vec![neighbor(
                "GigabitEthernet0/1",
                "SW2",
                "GigabitEthernet0/2",
            )]),
        device("b", "SW2")
            .interfaces(vec![port("GigabitEthernet0/2", 1_000_000_000, "full")])
            .neighbors(vec![neighbor(
                "GigabitEthernet0/2",
                "SW1",
                "GigabitEthernet0/1",
            )]),
    ]);
    let report = run("native_vlan_mismatch", &unknown);
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 2);
}

#[test]
fn native_vlan_mismatch_fixes_the_end_that_was_forgotten() {
    // SW2 is explicitly on 99; SW1 is on 1 only because nobody set it. The
    // finding, and the fix, belong to SW1.
    let found = run(
        "native_vlan_mismatch",
        &trunked(
            None,
            TRUNK_A,
            None,
            "interface GigabitEthernet0/2\n switchport mode trunk\n switchport trunk native vlan 99\n",
        ),
    );
    assert_eq!(found.findings[0].device_id, "a");
    let suggestion = found.findings[0].suggestion.as_ref().unwrap();
    assert_eq!(
        suggestion.config_lines.as_deref(),
        Some(
            [
                "interface GigabitEthernet0/1".to_string(),
                "switchport trunk native vlan 99".to_string()
            ]
            .as_slice()
        )
    );

    // Both ends explicit: no fix is offered, because which one is right is a
    // design decision and not something to guess at.
    let both = run(
        "native_vlan_mismatch",
        &trunked(
            Some(10),
            "interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk native vlan 10\n",
            Some(99),
            "interface GigabitEthernet0/2\n switchport mode trunk\n switchport trunk native vlan 99\n",
        ),
    );
    assert_eq!(
        both.findings[0].suggestion.as_ref().unwrap().config_lines,
        None
    );
}

// --------------------------------------------------------- trunk VLAN lists

#[test]
fn trunk_vlan_mismatch_table() {
    let allowed = |a: &str, b: &str| {
        trunked(
            Some(1),
            &format!("interface GigabitEthernet0/1\n switchport mode trunk\n{a}"),
            Some(1),
            &format!("interface GigabitEthernet0/2\n switchport mode trunk\n{b}"),
        )
    };

    // Positive: both ends prune, and they prune differently.
    let found = run(
        "trunk_vlan_mismatch",
        &allowed(
            " switchport trunk allowed vlan 10,20\n",
            " switchport trunk allowed vlan 10,30\n",
        ),
    );
    assert_eq!(subjects(&found), ["a:GigabitEthernet0/1"]);
    let evidence: Vec<&str> = found.findings[0]
        .evidence
        .iter()
        .map(|item| item.value.as_str())
        .collect();
    assert!(
        evidence.contains(&"20") && evidence.contains(&"30"),
        "{evidence:?}"
    );
    // The fix uses `add`, never a restated list.
    let lines = found.findings[0]
        .suggestion
        .as_ref()
        .unwrap()
        .config_lines
        .clone()
        .unwrap();
    assert_eq!(lines[1], "switchport trunk allowed vlan add 30");

    // Negative: the same list at both ends.
    assert!(
        run(
            "trunk_vlan_mismatch",
            &allowed(
                " switchport trunk allowed vlan 10,20\n",
                " switchport trunk allowed vlan 10,20\n",
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: one end allows everything, the other prunes. That is how
    // a pruned trunk is normally built and must never be reported.
    assert!(
        run(
            "trunk_vlan_mismatch",
            &allowed("", " switchport trunk allowed vlan 10,20\n")
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: `1-4094` is "everything" written out.
    assert!(
        run(
            "trunk_vlan_mismatch",
            &allowed(
                " switchport trunk allowed vlan 1-4094\n",
                " switchport trunk allowed vlan 10,20\n",
            )
        )
        .findings
        .is_empty()
    );

    // Look-alike 3: the difference is a VLAN that exists on neither switch.
    assert!(
        run(
            "trunk_vlan_mismatch",
            &allowed(
                " switchport trunk allowed vlan 10,20,30\n",
                " switchport trunk allowed vlan 10,20,30,400\n",
            )
        )
        .findings
        .is_empty()
    );

    // Not enough data: no VLAN list collected, so a real VLAN cannot be told
    // from a gap in the allowed list.
    let no_vlans = world(vec![
        device("a", "SW1")
            .interfaces(vec![trunk("GigabitEthernet0/1", Some(1))])
            .neighbors(vec![neighbor("GigabitEthernet0/1", "SW2", "GigabitEthernet0/2")])
            .config("interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 10\n"),
        device("b", "SW2")
            .interfaces(vec![trunk("GigabitEthernet0/2", Some(1))])
            .neighbors(vec![neighbor("GigabitEthernet0/2", "SW1", "GigabitEthernet0/1")])
            .config("interface GigabitEthernet0/2\n switchport mode trunk\n switchport trunk allowed vlan 20\n"),
    ]);
    let report = run("trunk_vlan_mismatch", &no_vlans);
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 2);
}

// ------------------------------------------------------------ VLAN plumbing

/// An access switch with one uplink trunk and desks in VLAN 10 and 30.
fn access_switch(allowed: &str, ports_admin_up: bool) -> World {
    let mut desk = access("GigabitEthernet0/3", 30);
    desk.admin_up = Some(ports_admin_up);
    world(vec![
        device("a", "SW1")
            .interfaces(vec![
                trunk("GigabitEthernet0/1", Some(1)),
                access("GigabitEthernet0/2", 10),
                desk,
            ])
            .vlans(vec![vlan(10, "users"), vlan(30, "guests")])
            .config(&format!(
                "interface GigabitEthernet0/1\n switchport mode trunk\n{allowed}\
                 interface GigabitEthernet0/2\n switchport mode access\n switchport access vlan 10\n\
                 interface GigabitEthernet0/3\n switchport mode access\n switchport access vlan 30\n"
            )),
    ])
}

#[test]
fn vlan_not_on_trunk_table() {
    // Positive: VLAN 30 has a live access port and the uplink does not carry it.
    let found = run(
        "vlan_not_on_trunk",
        &access_switch(" switchport trunk allowed vlan 10\n", true),
    );
    assert_eq!(subjects(&found), ["a:VLAN 30"]);
    assert_eq!(
        found.findings[0]
            .suggestion
            .as_ref()
            .unwrap()
            .config_lines
            .as_deref(),
        Some(
            [
                "interface GigabitEthernet0/1".to_string(),
                "switchport trunk allowed vlan add 30".to_string()
            ]
            .as_slice()
        )
    );

    // Negative: the uplink carries both.
    assert!(
        run(
            "vlan_not_on_trunk",
            &access_switch(" switchport trunk allowed vlan 10,30\n", true)
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: the trunk allows everything, so nothing is missing.
    let unrestricted = run("vlan_not_on_trunk", &access_switch("", true));
    assert!(unrestricted.findings.is_empty());
    assert_eq!(unrestricted.evaluated().len(), 1);

    // Look-alike 2: the VLAN's only port is shut, so nobody is black-holed.
    assert!(
        run(
            "vlan_not_on_trunk",
            &access_switch(" switchport trunk allowed vlan 10\n", false)
        )
        .findings
        .is_empty()
    );

    // Not enough data: a switch with no trunk at all.
    let standalone = world(vec![
        device("a", "SW1")
            .interfaces(vec![access("GigabitEthernet0/2", 10)])
            .vlans(vec![vlan(10, "users")])
            .config("interface GigabitEthernet0/2\n switchport mode access\n switchport access vlan 10\n"),
    ]);
    let report = run("vlan_not_on_trunk", &standalone);
    assert!(report.findings.is_empty());
    assert_eq!(report.skips().count(), 1);
}

#[test]
fn unused_vlan_table() {
    // Positive: VLAN 40 is defined, on no port, and on no trunk.
    let found = run(
        "unused_vlan",
        &world(vec![
            device("a", "SW1")
                .interfaces(vec![
                    trunk("GigabitEthernet0/1", Some(1)),
                    access("GigabitEthernet0/2", 10),
                ])
                .vlans(vec![vlan(10, "users"), vlan(40, "old-project")])
                .config(
                    "interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 10\n\
                     interface GigabitEthernet0/2\n switchport mode access\n switchport access vlan 10\n",
                ),
        ]),
    );
    assert_eq!(subjects(&found), ["a:VLAN 40"]);

    // Negative: a VLAN with a port.
    assert!(
        run(
            "unused_vlan",
            &world(vec![
                device("a", "SW1")
                    .interfaces(vec![
                        trunk("GigabitEthernet0/1", Some(1)),
                        access("GigabitEthernet0/2", 10),
                    ])
                    .vlans(vec![vlan(10, "users")])
                    .config(
                        "interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 99\n\
                         interface GigabitEthernet0/2\n switchport mode access\n switchport access vlan 10\n",
                    ),
            ])
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: the VLAN is carried by a trunk. It is transiting for a
    // switch further down, which is exactly what a distribution switch does.
    assert!(
        run(
            "unused_vlan",
            &world(vec![
                device("a", "SW1")
                    .interfaces(vec![trunk("GigabitEthernet0/1", Some(1))])
                    .vlans(vec![vlan(40, "old-project")])
                    .config("interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 40\n"),
            ])
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: an SVI uses it even though no access port does.
    assert!(
        run(
            "unused_vlan",
            &world(vec![
                device("a", "SW1")
                    .interfaces(vec![trunk("GigabitEthernet0/1", Some(1)), iface("Vlan40")])
                    .vlans(vec![vlan(40, "transit")])
                    .config("interface GigabitEthernet0/1\n switchport mode trunk\n switchport trunk allowed vlan 10\n"),
            ])
        )
        .findings
        .is_empty()
    );

    // Not enough data: a trunk that allows everything could be carrying it.
    let unrestricted = run(
        "unused_vlan",
        &world(vec![
            device("a", "SW1")
                .interfaces(vec![trunk("GigabitEthernet0/1", Some(1))])
                .vlans(vec![vlan(40, "old-project")])
                .config("interface GigabitEthernet0/1\n switchport mode trunk\n"),
        ]),
    );
    assert!(unrestricted.findings.is_empty());
    assert_eq!(unrestricted.skips().count(), 1);
}

#[test]
fn vlan_name_inconsistent_table() {
    let named = |names: &[(&str, u64, &str)]| {
        world(
            names
                .iter()
                .enumerate()
                .map(|(index, (id, vlan_id, name))| {
                    device(id, &format!("SW{index}")).vlans(vec![vlan(*vlan_id, name)])
                })
                .collect(),
        )
    };

    // Positive: two switches call VLAN 20 `voice`, one calls it `printers`.
    let found = run(
        "vlan_name_inconsistent",
        &named(&[
            ("a", 20, "voice"),
            ("b", 20, "voice"),
            ("c", 20, "printers"),
        ]),
    );
    assert_eq!(subjects(&found), ["c:VLAN 20"]);
    assert!(found.findings[0].title.contains("printers"));
    assert_eq!(found.findings[0].related.len(), 2);

    // Negative: everybody agrees.
    assert!(
        run(
            "vlan_name_inconsistent",
            &named(&[("a", 20, "voice"), ("b", 20, "voice")])
        )
        .findings
        .is_empty()
    );

    // Look-alike 1: case, hyphens and underscores are not a difference.
    assert!(
        run(
            "vlan_name_inconsistent",
            &named(&[("a", 20, "Voice-VLAN"), ("b", 20, "voice_vlan")])
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: default names say nothing, so they are not a second
    // spelling. Reporting them would flag every switch that never named a VLAN.
    assert!(
        run(
            "vlan_name_inconsistent",
            &named(&[
                ("a", 20, "voice"),
                ("b", 20, "VLAN0020"),
                ("c", 20, "default")
            ])
        )
        .findings
        .is_empty()
    );

    // Not enough data: nobody named anything.
    let unnamed = run(
        "vlan_name_inconsistent",
        &named(&[("a", 20, "VLAN0020"), ("b", 20, "VLAN0020")]),
    );
    assert!(unnamed.findings.is_empty());
    assert_eq!(unnamed.skips().count(), 2);

    // Not enough data: no VLAN list at all.
    let nothing = run("vlan_name_inconsistent", &world(vec![device("a", "SW1")]));
    assert_eq!(nothing.skips().count(), 1);
}

// -------------------------------------------------------------- MAC flapping

/// A switch whose MAC table moved `count` addresses between two ports.
fn moved(count: usize, from: &str, to: &str, trunks: bool) -> World {
    let addresses: Vec<String> = (0..count)
        .map(|index| format!("0011.2233.44{index:02}"))
        .collect();
    let before: Vec<_> = addresses
        .iter()
        .map(|address| mac(10, address, from))
        .collect();
    let after: Vec<_> = addresses
        .iter()
        .map(|address| mac(10, address, to))
        .collect();
    let make = |name: &str| {
        if trunks {
            trunk(name, Some(1))
        } else {
            access(name, 10)
        }
    };
    world(vec![
        device("a", "SW1")
            .interfaces(vec![make(from), make(to)])
            .mac_table(after)
            .previous_mac_table(before),
    ])
}

#[test]
fn mac_flapping_table() {
    // Positive: four addresses move between two access ports.
    let found = run(
        "mac_flapping",
        &moved(4, "GigabitEthernet0/3", "GigabitEthernet0/4", false),
    );
    assert_eq!(
        subjects(&found),
        ["a:VLAN 10 GigabitEthernet0/3 ↔ GigabitEthernet0/4"]
    );
    assert!(found.findings[0].title.contains("4 addresses"));

    // Negative: nothing moved.
    let still = world(vec![
        device("a", "SW1")
            .interfaces(vec![access("GigabitEthernet0/3", 10)])
            .mac_table(vec![mac(10, "0011.2233.4401", "GigabitEthernet0/3")])
            .previous_mac_table(vec![mac(10, "0011.2233.4401", "GigabitEthernet0/3")]),
    ]);
    assert!(run("mac_flapping", &still).findings.is_empty());

    // Look-alike 1: one address moved. A laptop at another desk, or a
    // wireless client roaming — the most common thing a MAC table does.
    assert!(
        run(
            "mac_flapping",
            &moved(1, "GigabitEthernet0/3", "GigabitEthernet0/4", false)
        )
        .findings
        .is_empty()
    );

    // Look-alike 2: many addresses moving between two *trunks*. That is a
    // spanning-tree reconvergence doing its job.
    assert!(
        run(
            "mac_flapping",
            &moved(20, "GigabitEthernet0/1", "GigabitEthernet0/2", true)
        )
        .findings
        .is_empty()
    );

    // Look-alike 3: a port with a network neighbour on it is not an edge port.
    let via_neighbour = world(vec![
        device("a", "SW1")
            .interfaces(vec![
                access("GigabitEthernet0/3", 10),
                access("GigabitEthernet0/4", 10),
            ])
            .neighbors(vec![neighbor("GigabitEthernet0/4", "SW9", "Gi0/1")])
            .mac_table(
                (0..4)
                    .map(|index| mac(10, &format!("0011.2233.44{index:02}"), "GigabitEthernet0/4"))
                    .collect(),
            )
            .previous_mac_table(
                (0..4)
                    .map(|index| mac(10, &format!("0011.2233.44{index:02}"), "GigabitEthernet0/3"))
                    .collect(),
            ),
    ]);
    assert!(run("mac_flapping", &via_neighbour).findings.is_empty());

    // Not enough data: only one collection.
    let once = run(
        "mac_flapping",
        &world(vec![
            device("a", "SW1")
                .interfaces(vec![access("GigabitEthernet0/3", 10)])
                .mac_table(vec![mac(10, "0011.2233.4401", "GigabitEthernet0/3")]),
        ]),
    );
    assert!(once.findings.is_empty());
    assert_eq!(once.skips().count(), 1);
}
