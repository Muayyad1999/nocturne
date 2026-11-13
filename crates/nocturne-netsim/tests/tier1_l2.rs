//! Layer 2: link state, switching, VLANs and trunks, spanning tree.

mod common;

use common::*;
use nocturne_netsim::{Lab, Light};

#[test]
fn two_hosts_on_a_switch_first_ping_loses_one_packet_to_arp() {
    let mut lab = Lab::new("two hosts");
    let sw = node(&mut lab, "switch", "SW1");
    let a = pc(&mut lab, "PC1", "192.168.1.10", None);
    let b = pc(&mut lab, "PC2", "192.168.1.11", None);
    link(&mut lab, a, "Ethernet0", sw, "fa0/1");
    link(&mut lab, b, "Ethernet0", sw, "fa0/2");
    lab.settle(60 * SECOND);

    let first = exec(&mut lab, a, "ping 192.168.1.11 -n 5");
    assert!(first.contains("Request timed out."), "{first}");
    assert_eq!(replies(&first), 4, "{first}");
    assert!(
        first.contains("Packets: Sent = 5, Received = 4, Lost = 1 (20% loss)"),
        "{first}"
    );
    let second = exec(&mut lab, a, "ping 192.168.1.11 -n 5");
    assert_eq!(replies(&second), 5, "{second}");
    assert!(
        second.contains("Reply from 192.168.1.11: bytes=32 time<1ms TTL=128"),
        "{second}"
    );

    // The same from an IOS device: the classic `.!!!!` then `!!!!!`.
    cfg(
        &mut lab,
        sw,
        "interface vlan 1\n ip address 192.168.1.2 255.255.255.0\n no shutdown",
    );
    lab.settle(30 * SECOND);
    let out = exec(&mut lab, sw, "ping 192.168.1.10");
    assert_eq!(ping_marks(&out), ".!!!!", "{out}");
    assert!(out.contains("Success rate is 80 percent (4/5)"), "{out}");
    let out = exec(&mut lab, sw, "ping 192.168.1.10");
    assert_eq!(ping_marks(&out), "!!!!!", "{out}");
    assert!(
        out.contains("Success rate is 100 percent (5/5), round-trip min/avg/max = 1/1/1 ms"),
        "{out}"
    );

    // The switch learned both hosts.
    let table = exec(&mut lab, sw, "show mac address-table");
    assert!(
        table.contains("Fa0/1") && table.contains("Fa0/2"),
        "{table}"
    );
    let macs = lab.node_tables(sw).unwrap().mac;
    assert_eq!(
        macs.iter()
            .filter(|m| m.vlan == 1 && m.kind == "DYNAMIC")
            .count(),
        2,
        "{macs:?}"
    );
}

#[test]
fn link_lights_follow_cabling_shutdown_and_spanning_tree() {
    let mut lab = Lab::new("lights");
    let sw = node(&mut lab, "switch", "SW1");
    let a = pc(&mut lab, "PC1", "192.168.1.10", None);
    let light = |lab: &Lab, name: &str| {
        let snap = lab.snapshot();
        let n = snap.nodes.iter().find(|n| n.name == "SW1").unwrap();
        n.interfaces.iter().find(|i| i.short == name).unwrap().light
    };
    assert_eq!(light(&lab, "Fa0/1"), Light::Off);
    let l = link(&mut lab, a, "Ethernet0", sw, "fa0/1");
    // A new switch port listens and learns before it forwards.
    assert_eq!(light(&lab, "Fa0/1"), Light::Amber);
    lab.advance(3 * SECOND);
    assert_eq!(light(&lab, "Fa0/1"), Light::Green);
    assert!(lab.snapshot().links.iter().any(|v| v.id == l && v.up));

    cfg(&mut lab, sw, "interface fa0/1\n shutdown");
    assert_eq!(light(&lab, "Fa0/1"), Light::Red);
    assert!(!lab.snapshot().links[0].up);
    let events = lab.take_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, nocturne_netsim::LabEvent::LinkState { up: false, .. })),
        "{events:?}"
    );
    let logged: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            nocturne_netsim::LabEvent::Syslog { line, .. } => Some(line.clone()),
            _ => None,
        })
        .collect();
    assert!(
        logged.iter().any(|l| l.contains(
            "%LINK-5-CHANGED: Interface FastEthernet0/1, changed state to administratively down"
        )),
        "{logged:#?}"
    );
    cfg(&mut lab, sw, "interface fa0/1\n no shutdown");
    lab.advance(3 * SECOND);
    assert_eq!(light(&lab, "Fa0/1"), Light::Green);
    lab.set_power(a, false);
    assert_eq!(light(&lab, "Fa0/1"), Light::Red);
}

#[test]
fn vlans_isolate_and_trunks_carry_them() {
    let mut lab = Lab::new("vlans");
    let s1 = node(&mut lab, "switch", "SW1");
    let s2 = node(&mut lab, "switch", "SW2");
    let a10 = pc(&mut lab, "A10", "10.0.10.1", None);
    let b10 = pc(&mut lab, "B10", "10.0.10.2", None);
    let a20 = pc(&mut lab, "A20", "10.0.10.3", None);
    let b20 = pc(&mut lab, "B20", "10.0.10.4", None);
    link(&mut lab, a10, "Ethernet0", s1, "fa0/1");
    link(&mut lab, a20, "Ethernet0", s1, "fa0/2");
    link(&mut lab, b10, "Ethernet0", s2, "fa0/1");
    link(&mut lab, b20, "Ethernet0", s2, "fa0/2");
    link(&mut lab, s1, "gi0/1", s2, "gi0/1");
    let access = "vlan 10\n name Staff\nvlan 20\n name Guest\ninterface fa0/1\n switchport mode access\n switchport access vlan 10\ninterface fa0/2\n switchport mode access\n switchport access vlan 20\ninterface gi0/1\n switchport mode trunk";
    cfg(&mut lab, s1, access);
    cfg(&mut lab, s2, access);
    lab.settle(60 * SECOND);

    let ping = |lab: &mut Lab, from, to: &str| {
        exec(lab, from, &format!("ping {to} -n 2"));
        replies(&exec(lab, from, &format!("ping {to} -n 2")))
    };
    // Same VLAN across the trunk works; a different VLAN in the same subnet does not.
    assert_eq!(ping(&mut lab, a10, "10.0.10.2"), 2);
    assert_eq!(ping(&mut lab, a20, "10.0.10.4"), 2);
    assert_eq!(ping(&mut lab, a10, "10.0.10.3"), 0);
    assert_eq!(ping(&mut lab, a10, "10.0.10.4"), 0);

    // Pruning VLAN 20 from the trunk cuts only VLAN 20.
    cfg(
        &mut lab,
        s1,
        "interface gi0/1\n switchport trunk allowed vlan remove 20",
    );
    lab.settle(30 * SECOND);
    assert_eq!(ping(&mut lab, a10, "10.0.10.2"), 2);
    assert_eq!(ping(&mut lab, a20, "10.0.10.4"), 0);
    let trunk = exec(&mut lab, s1, "show interfaces trunk");
    assert!(trunk.contains("Gi0/1       1-19,21-4094"), "{trunk}");
    cfg(
        &mut lab,
        s1,
        "interface gi0/1\n switchport trunk allowed vlan add 20",
    );
    lab.settle(30 * SECOND);
    assert_eq!(ping(&mut lab, a20, "10.0.10.4"), 2);

    // Native VLAN: untagged frames on the trunk belong to VLAN 10 on both ends,
    // so VLAN 10 still works, and a mismatch leaks between VLANs.
    cfg(
        &mut lab,
        s1,
        "interface gi0/1\n switchport trunk native vlan 10",
    );
    cfg(
        &mut lab,
        s2,
        "interface gi0/1\n switchport trunk native vlan 10",
    );
    lab.settle(30 * SECOND);
    assert_eq!(ping(&mut lab, a10, "10.0.10.2"), 2);
    cfg(
        &mut lab,
        s2,
        "interface gi0/1\n switchport trunk native vlan 20",
    );
    lab.settle(30 * SECOND);
    // SW1 sends VLAN 10 untagged; SW2 puts untagged frames into VLAN 20.
    assert_eq!(ping(&mut lab, a10, "10.0.10.4"), 2);
    assert_eq!(ping(&mut lab, a10, "10.0.10.2"), 0);

    // A VLAN that does not exist on a switch is not forwarded there.
    cfg(
        &mut lab,
        s2,
        "interface gi0/1\n switchport trunk native vlan 1\nno vlan 20",
    );
    cfg(
        &mut lab,
        s1,
        "interface gi0/1\n switchport trunk native vlan 1",
    );
    lab.settle(30 * SECOND);
    assert_eq!(ping(&mut lab, a20, "10.0.10.4"), 0);
    let vlans = exec(&mut lab, s1, "show vlan brief");
    assert!(
        vlans.contains("10   Staff                            active    Fa0/1"),
        "{vlans}"
    );
    assert!(
        vlans.contains("20   Guest                            active    Fa0/2"),
        "{vlans}"
    );
}

#[test]
fn dynamic_trunk_negotiation() {
    let mut lab = Lab::new("dtp");
    let s1 = node(&mut lab, "switch", "SW1");
    let s2 = node(&mut lab, "switch", "SW2");
    link(&mut lab, s1, "gi0/1", s2, "gi0/1");
    let mode = |lab: &mut Lab, node| {
        let out = exec(lab, node, "show interfaces gi0/1 switchport");
        out.lines()
            .find(|l| l.starts_with("Operational Mode:"))
            .unwrap()
            .to_string()
    };
    // Two ports in the default dynamic auto mode stay access ports.
    assert_eq!(mode(&mut lab, s1), "Operational Mode: static access");
    cfg(
        &mut lab,
        s1,
        "interface gi0/1\n switchport mode dynamic desirable",
    );
    assert_eq!(mode(&mut lab, s1), "Operational Mode: trunk");
    assert_eq!(mode(&mut lab, s2), "Operational Mode: trunk");
    cfg(
        &mut lab,
        s1,
        "interface gi0/1\n switchport mode trunk\n switchport nonegotiate",
    );
    // Without DTP frames the dynamic side falls back to access.
    assert_eq!(mode(&mut lab, s1), "Operational Mode: trunk");
    assert_eq!(mode(&mut lab, s2), "Operational Mode: static access");
}

#[test]
fn spanning_tree_blocks_one_port_of_a_triangle_and_fails_over() {
    let mut lab = Lab::new("stp");
    let s1 = node(&mut lab, "switch", "SW1");
    let s2 = node(&mut lab, "switch", "SW2");
    let s3 = node(&mut lab, "switch", "SW3");
    let h1 = pc(&mut lab, "PC1", "10.0.0.1", None);
    let h2 = pc(&mut lab, "PC2", "10.0.0.2", None);
    let l12 = link(&mut lab, s1, "gi0/1", s2, "gi0/1");
    link(&mut lab, s2, "gi0/2", s3, "gi0/2");
    link(&mut lab, s1, "gi0/2", s3, "gi0/1");
    link(&mut lab, h1, "Ethernet0", s2, "fa0/1");
    link(&mut lab, h2, "Ethernet0", s3, "fa0/1");
    // Make SW1 the root by priority.
    cfg(&mut lab, s1, "spanning-tree vlan 1 priority 4096");
    lab.settle(60 * SECOND);

    let stp = |lab: &Lab, node| lab.node_tables(node).unwrap().stp;
    assert!(
        stp(&lab, s1)
            .iter()
            .all(|r| r.is_root && r.state == "FWD" && r.role == "Desg"),
        "{:?}",
        stp(&lab, s1)
    );
    let all: Vec<_> = [s1, s2, s3].iter().flat_map(|n| stp(&lab, *n)).collect();
    let blocked: Vec<_> = all.iter().filter(|r| r.state == "BLK").collect();
    assert_eq!(blocked.len(), 1, "exactly one blocked port: {all:#?}");
    assert_eq!(blocked[0].role, "Altn");
    assert!(
        all.iter().all(|r| r.root_id.starts_with("4097 ")),
        "{all:#?}"
    );
    let show = exec(&mut lab, s1, "show spanning-tree");
    assert!(show.contains("This bridge is the root"), "{show}");
    let show = exec(&mut lab, s2, "show spanning-tree vlan 1");
    assert!(
        show.contains("Root ID    Priority    4097") && show.contains("Root FWD 4"),
        "{show}"
    );

    exec(&mut lab, h1, "ping 10.0.0.2 -n 2");
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.0.0.2 -n 3")), 3);

    // Cut SW1-SW2: SW2 must reach the root through SW3 and traffic recovers.
    lab.remove_link(l12).unwrap();
    lab.settle(60 * SECOND);
    let all: Vec<_> = [s1, s2, s3].iter().flat_map(|n| stp(&lab, *n)).collect();
    assert!(
        all.iter().all(|r| r.state == "FWD"),
        "no loop left, nothing blocked: {all:#?}"
    );
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.0.0.2 -n 3")), 3);
}
