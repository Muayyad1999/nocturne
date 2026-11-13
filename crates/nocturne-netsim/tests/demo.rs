//! The reference campus and the teaching labs.

mod common;

use std::net::Ipv4Addr;

use common::*;
use nocturne_netsim::{CliKind, Fault, Lab, LabSpec, Light, NodeKind, demo_campus, examples};

const INTERNET: Ipv4Addr = Ipv4Addr::new(8, 8, 8, 8);

/// Build a lab from a spec, requiring that every configuration line was accepted.
fn build(spec: &LabSpec) -> Lab {
    let mut lab = Lab::from_spec(spec).unwrap_or_else(|e| panic!("{}: {e}", spec.name));
    for node in lab.snapshot().nodes {
        let errors = lab.startup_errors(node.id);
        assert!(
            errors.is_empty(),
            "{} / {}: rejected startup lines {errors:#?}",
            spec.name,
            node.name
        );
    }
    lab.settle(180 * SECOND);
    lab
}

fn id(lab: &Lab, name: &str) -> u32 {
    lab.node_by_name(name)
        .unwrap_or_else(|| panic!("no node {name}"))
}

#[test]
fn campus_comes_up_and_everything_is_reachable() {
    let spec = demo_campus();
    assert_eq!(spec.flows.len(), 3);
    let mut lab = build(&spec);
    assert!(
        lab.now() < 60 * SECOND,
        "the campus settles quickly: {} s",
        lab.now() / SECOND
    );
    let snapshot = lab.snapshot();
    assert_eq!(snapshot.nodes.len(), 16);

    // Every end host has an address (six of them by DHCP) and reaches the Internet.
    let hosts: Vec<_> = snapshot
        .nodes
        .iter()
        .filter(|n| {
            !matches!(
                n.kind,
                NodeKind::Router | NodeKind::Switch | NodeKind::L3Switch | NodeKind::Cloud
            )
        })
        .collect();
    assert_eq!(hosts.len(), 9);
    let leased = hosts
        .iter()
        .filter(|n| lab.host_config(n.id).is_some_and(|h| h.dhcp))
        .count();
    assert_eq!(leased, 6);
    for host in &hosts {
        let probe = lab.probe_ping(host.id, INTERNET);
        assert!(
            probe.ok,
            "{} cannot reach 8.8.8.8: {}",
            host.name, probe.reason
        );
        assert!(
            probe
                .hops
                .iter()
                .any(|h| h.name == "EDGE" && h.note.contains("NAT")),
            "{}: {:?}",
            host.name,
            probe.hops
        );
    }
    // Typing the command works too (the very first packets may be lost to ARP).
    let pc = id(&lab, "Staff-PC1");
    exec(&mut lab, pc, "ping 8.8.8.8");
    assert_eq!(replies(&exec(&mut lab, pc, "ping 8.8.8.8")), 4);
    assert!(
        exec(&mut lab, pc, "ipconfig /all")
            .contains("Default Gateway . . . . . . . . . : 10.10.10.1")
    );

    // The management station reaches every device on its management address.
    let nms = id(&lab, "NMS");
    for node in &snapshot.nodes {
        let mgmt: Ipv4Addr = node
            .mgmt_ip
            .as_deref()
            .unwrap_or_else(|| panic!("{} has no management address", node.name))
            .parse()
            .unwrap();
        let probe = lab.probe_ping(nms, mgmt);
        assert!(
            probe.ok,
            "NMS cannot reach {} at {mgmt}: {}",
            node.name, probe.reason
        );
    }
    // ...and can log in to the network devices over SSH (and only SSH).
    for name in ["EDGE", "DS1", "DS2", "AS1", "AS2", "AS3"] {
        let node = snapshot.nodes.iter().find(|n| n.name == name).unwrap();
        assert_eq!(node.cli, CliKind::Ios);
        let mgmt: Ipv4Addr = node.mgmt_ip.as_deref().unwrap().parse().unwrap();
        assert!(lab.probe_tcp(nms, mgmt, 22).ok, "ssh to {name}");
        assert!(
            !lab.probe_tcp(nms, mgmt, 23).ok,
            "telnet to {name} should be refused"
        );
    }
    let console = lab.console_open(nms).unwrap();
    output(&mut lab, console);
    lab.console_input(console, b"ssh -l admin 10.255.0.1\r");
    lab.advance(2 * SECOND);
    lab.console_input(console, b"nocturne\r");
    lab.advance(SECOND);
    assert!(output(&mut lab, console).ends_with("\r\nEDGE#"));

    // Routing: OSPF adjacencies everywhere, the default route comes from the edge.
    let (edge, ds1, ds2) = (id(&lab, "EDGE"), id(&lab, "DS1"), id(&lab, "DS2"));
    assert_eq!(
        lab.node_tables(edge)
            .unwrap()
            .ospf_neighbors
            .iter()
            .filter(|n| n.state.starts_with("FULL"))
            .count(),
        2
    );
    assert_eq!(
        lab.node_tables(ds1)
            .unwrap()
            .ospf_neighbors
            .iter()
            .filter(|n| n.state.starts_with("FULL"))
            .count(),
        2
    );
    assert!(
        exec(&mut lab, ds2, "show ip route").contains("O*E2  0.0.0.0/0 [110/1] via 10.0.0.5, ")
    );
    // Gateways: DS1 is active for Staff and Management, DS2 for Guest and Voice.
    let brief = exec(&mut lab, ds1, "show standby brief");
    assert!(
        brief.contains("Vl10        10   110 P Active  local           10.10.10.3      10.10.10.1"),
        "{brief}"
    );
    assert!(
        brief.contains("Vl20        20   100   Standby 10.10.20.3      local           10.10.20.1"),
        "{brief}"
    );
    assert!(
        exec(&mut lab, ds2, "show standby brief").contains("Vl30        30   110 P Active  local")
    );
    // Switching: the inter-switch channel is bundled and every access switch has
    // one uplink blocked per VLAN.
    assert!(
        exec(&mut lab, ds1, "show etherchannel summary")
            .contains("Po1(SU)         LACP      Te1/1/1(P)  Te1/1/2(P)")
    );
    for name in ["AS1", "AS2", "AS3"] {
        let switch = id(&lab, name);
        let stp = lab.node_tables(switch).unwrap().stp;
        for vlan in [10u16, 20, 30, 99] {
            let blocked = stp
                .iter()
                .filter(|r| r.vlan == vlan && r.role == "Altn" && r.state == "BLK")
                .count();
            assert_eq!(blocked, 1, "{name} VLAN {vlan}: {stp:?}");
        }
        let view = snapshot.nodes.iter().find(|n| n.name == name).unwrap();
        assert!(
            view.interfaces
                .iter()
                .filter(|i| i.name.starts_with("Gigabit"))
                .all(|i| i.oper_up)
        );
    }
    let as1 = lab
        .snapshot()
        .nodes
        .into_iter()
        .find(|n| n.name == "AS1")
        .unwrap();
    assert!(
        as1.interfaces.iter().any(|i| i.light == Light::Amber),
        "a blocked uplink shows amber"
    );
    // DHCP: all six leases come from DS1.
    assert_eq!(lab.node_tables(ds1).unwrap().dhcp_bindings.len(), 6);

    // Guests reach the Internet but neither the staff network nor management.
    let guest = id(&lab, "Guest-Laptop1");
    let denied = lab.probe_ping(guest, "10.10.10.10".parse().unwrap());
    assert!(
        !denied.ok && denied.reason.contains("GUEST-IN"),
        "{}",
        denied.reason
    );
    assert!(!lab.probe_tcp(guest, "10.10.99.11".parse().unwrap(), 22).ok);
    assert!(lab.probe_tcp(guest, INTERNET, 80).ok);
    assert!(
        lab.probe_tcp(pc, "10.10.10.10".parse().unwrap(), 80).ok,
        "staff reach their server"
    );

    // The background flows deliver traffic and move interface counters.
    lab.advance(30 * SECOND);
    let flows = lab.flows();
    assert!(
        flows
            .iter()
            .all(|(_, _, state)| state.active && state.delivered_octets > 0),
        "{flows:#?}"
    );
    assert!(
        lab.metrics(edge)
            .unwrap()
            .interfaces
            .iter()
            .any(|i| i.in_octets > 100_000)
    );
}

#[test]
fn campus_survives_the_loss_of_a_distribution_switch() {
    let mut lab = build(&demo_campus());
    let (ds1, pc, guest) = (
        id(&lab, "DS1"),
        id(&lab, "Staff-PC1"),
        id(&lab, "Guest-Laptop2"),
    );
    lab.set_power(ds1, false);
    lab.settle(180 * SECOND);
    // Spanning tree unblocks the other uplinks, HSRP moves the gateways and OSPF
    // the routes: the users keep their Internet access.
    for host in [pc, guest] {
        let probe = lab.probe_ping(host, INTERNET);
        assert!(
            probe.ok && probe.hops.iter().any(|h| h.name == "DS2"),
            "{}",
            probe.reason
        );
    }
    let ds2 = id(&lab, "DS2");
    assert!(
        exec(&mut lab, ds2, "show standby brief")
            .contains("Vl10        10   100   Active  local           unknown")
    );
    // A cut uplink on the survivor's side isolates only that access switch.
    let uplink = lab
        .snapshot()
        .links
        .into_iter()
        .find(|l| l.a_if == "GigabitEthernet0/2" && lab.node_by_name("AS1") == Some(l.a))
        .unwrap();
    lab.inject(Fault::LinkDown { link: uplink.id }).unwrap();
    lab.settle(60 * SECOND);
    assert!(!lab.probe_ping(pc, INTERNET).ok);
    assert!(lab.probe_ping(guest, INTERNET).ok);
}

#[test]
fn campus_spec_round_trips_and_is_deterministic() {
    let spec = demo_campus();
    assert_eq!(spec, demo_campus());
    let json = serde_json::to_string(&spec).unwrap();
    assert_eq!(serde_json::from_str::<LabSpec>(&json).unwrap(), spec);

    let mut a = build(&spec);
    let mut b = build(&spec);
    a.advance(45 * SECOND);
    b.advance(45 * SECOND);
    assert_eq!(a.snapshot(), b.snapshot());
    let edge = id(&a, "EDGE");
    assert_eq!(a.metrics(edge).unwrap(), b.metrics(edge).unwrap());
    assert_eq!(a.take_events(), b.take_events());

    // Saving the running lab and loading it again gives the same network.
    for node in a.snapshot().nodes.iter().filter(|n| n.cli == CliKind::Ios) {
        exec(&mut a, node.id, "write memory");
    }
    let saved = a.to_spec();
    let mut again = build(&saved);
    assert_eq!(again.to_spec(), saved);
    let pc = id(&again, "Staff-PC1");
    assert!(again.probe_ping(pc, INTERNET).ok);
    for name in ["EDGE", "DS1", "DS2", "AS1", "AS2", "AS3"] {
        let (x, y) = (id(&a, name), id(&again, name));
        assert_eq!(
            a.running_config(x).unwrap(),
            again.running_config(y).unwrap(),
            "{name}"
        );
    }
}

#[test]
fn teaching_labs_work_as_described() {
    let labs = examples();
    let names: Vec<&str> = labs.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Basic switching",
            "VLANs and a trunk",
            "Router on a stick",
            "OSPF triangle",
            "DHCP and NAT"
        ]
    );
    for spec in &labs {
        assert!(
            !spec.description.is_empty() && !spec.notes.is_empty(),
            "{}",
            spec.name
        );
        assert_eq!(
            serde_json::from_str::<LabSpec>(&serde_json::to_string(spec).unwrap()).unwrap(),
            *spec
        );
    }
    let ping = |lab: &mut Lab, from: &str, to: &str| {
        let from = id(lab, from);
        lab.probe_ping(from, to.parse().unwrap())
    };

    let mut lab = build(&labs[0]);
    assert!(ping(&mut lab, "PC1", "192.168.1.13").ok);
    let pc1 = id(&lab, "PC1");
    assert_eq!(
        replies(&exec(&mut lab, pc1, "ping 192.168.1.12")),
        3,
        "the first echo is lost while ARP resolves"
    );
    let sw1 = id(&lab, "SW1");
    // The real ping taught the switch where PC1 and PC2 are. The instant probe above
    // deliberately runs on a copy of the world, so PC3 is still unknown here.
    let table = |lab: &mut Lab| {
        exec(lab, sw1, "show mac address-table")
            .matches("DYNAMIC")
            .count()
    };
    assert_eq!(table(&mut lab), 2);
    assert_eq!(
        replies(&exec(&mut lab, pc1, "ping 192.168.1.13")),
        3,
        "ARP for PC3 has yet to resolve"
    );
    assert_eq!(table(&mut lab), 3, "now the switch knows all three");

    let mut lab = build(&labs[1]);
    assert!(ping(&mut lab, "Sales1", "10.0.10.12").ok && ping(&mut lab, "Eng1", "10.0.20.12").ok);
    assert!(
        !ping(&mut lab, "Sales1", "10.0.20.12").ok,
        "no router: the VLANs are separate networks"
    );
    let sw1 = id(&lab, "SW1");
    assert!(exec(&mut lab, sw1, "show interfaces trunk").contains("Gi0/1       10,20\n"));

    let mut lab = build(&labs[2]);
    let probe = ping(&mut lab, "PC1", "192.168.20.10");
    assert!(probe.ok, "{}", probe.reason);
    assert_eq!(
        probe
            .hops
            .iter()
            .map(|h| h.name.as_str())
            .collect::<Vec<_>>(),
        ["PC1", "SW1", "R1", "SW1", "SRV1"]
    );
    assert!(lab.flows().iter().all(|(_, _, state)| state.active));

    let mut lab = build(&labs[3]);
    for (from, to) in [
        ("PC1", "10.3.0.10"),
        ("PC2", "10.1.0.10"),
        ("PC3", "10.2.0.10"),
    ] {
        assert!(ping(&mut lab, from, to).ok, "{from} -> {to}");
    }
    let r1 = id(&lab, "R1");
    assert_eq!(lab.node_tables(r1).unwrap().ospf_neighbors.len(), 2);
    let cut = lab
        .snapshot()
        .links
        .into_iter()
        .find(|l| l.a_if == "GigabitEthernet0/1" && l.b_if == "GigabitEthernet0/1")
        .unwrap();
    lab.inject(Fault::LinkDown { link: cut.id }).unwrap();
    lab.settle(60 * SECOND);
    let detour = ping(&mut lab, "PC1", "10.3.0.10");
    assert!(
        detour.ok && detour.hops.iter().any(|h| h.name == "R2"),
        "{detour:?}"
    );

    let mut lab = build(&labs[4]);
    for host in ["PC1", "PC2"] {
        assert!(ping(&mut lab, host, "8.8.8.8").ok, "{host}");
    }
    let r1 = id(&lab, "R1");
    assert_eq!(lab.node_tables(r1).unwrap().dhcp_bindings.len(), 2);
    let pc1 = id(&lab, "PC1");
    exec(&mut lab, pc1, "ping 8.8.8.8");
    assert!(!lab.node_tables(r1).unwrap().nat.is_empty());
}
