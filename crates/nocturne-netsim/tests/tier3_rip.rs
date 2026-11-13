//! RIP: updates, hop counts, summarisation, withdrawal, administrative distance.

mod common;

use common::*;
use nocturne_netsim::{Fault, Lab};

const RIP: &str = "router rip\n version 2\n no auto-summary\n network 10.0.0.0";

/// R1 -- R2 -- R3 in a row, a host behind R1 and one behind R3.
fn row() -> (Lab, [u32; 3], [u32; 2], [u32; 2]) {
    let mut lab = Lab::new("rip");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let r3 = node(&mut lab, "router", "R3");
    let h1 = pc(&mut lab, "PC1", "10.1.0.10", Some("10.1.0.1"));
    let h3 = pc(&mut lab, "PC3", "10.3.0.10", Some("10.3.0.1"));
    link(&mut lab, h1, "Ethernet0", r1, "g0/0");
    let l12 = link(&mut lab, r1, "g0/1", r2, "g0/0");
    let l23 = link(&mut lab, r2, "g0/1", r3, "g0/0");
    link(&mut lab, r3, "g0/1", h3, "Ethernet0");
    cfg(
        &mut lab,
        r1,
        &format!(
            "interface g0/0\n ip address 10.1.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.0.12.1 255.255.255.252\n no shutdown\n{RIP}\n passive-interface GigabitEthernet0/0"
        ),
    );
    cfg(
        &mut lab,
        r2,
        &format!(
            "interface g0/0\n ip address 10.0.12.2 255.255.255.252\n no shutdown\ninterface g0/1\n ip address 10.0.23.1 255.255.255.252\n no shutdown\n{RIP}"
        ),
    );
    cfg(
        &mut lab,
        r3,
        &format!(
            "interface g0/0\n ip address 10.0.23.2 255.255.255.252\n no shutdown\ninterface g0/1\n ip address 10.3.0.1 255.255.255.0\n no shutdown\n{RIP}\n passive-interface GigabitEthernet0/1"
        ),
    );
    lab.settle(120 * SECOND);
    (lab, [r1, r2, r3], [h1, h3], [l12, l23])
}

fn route(lab: &mut Lab, node: u32, prefix: &str) -> String {
    let table = exec(lab, node, "show ip route");
    table
        .lines()
        .find(|l| l.contains(&format!(" {prefix} ")))
        .unwrap_or("")
        .to_string()
}

#[test]
fn routes_are_learned_with_hop_counts() {
    let (mut lab, [r1, r2, r3], [h1, _], _) = row();
    assert!(
        route(&mut lab, r1, "10.3.0.0/24")
            .starts_with("R        10.3.0.0/24 [120/2] via 10.0.12.2, 00:00:"),
        "{}",
        exec(&mut lab, r1, "show ip route")
    );
    assert!(
        route(&mut lab, r1, "10.0.23.0/30")
            .starts_with("R        10.0.23.0/30 [120/1] via 10.0.12.2, ")
    );
    assert!(
        route(&mut lab, r2, "10.1.0.0/24")
            .starts_with("R        10.1.0.0/24 [120/1] via 10.0.12.1, ")
    );
    assert!(route(&mut lab, r3, "10.1.0.0/24").ends_with(", GigabitEthernet0/0"));
    let rows = lab.node_tables(r1).unwrap().routes;
    assert!(
        rows.iter()
            .any(|r| r.code == "R" && r.prefix == "10.3.0.0/24" && r.ad == 120 && r.metric == 2),
        "{rows:?}"
    );
    exec(&mut lab, h1, "ping 10.3.0.10");
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.3.0.10")), 4);

    // The age of a RIP route is the time since the last update: never much more than 30 s.
    lab.advance(100 * SECOND);
    let line = route(&mut lab, r1, "10.3.0.0/24");
    let age: u32 = line
        .split("00:00:")
        .nth(1)
        .and_then(|rest| rest[..2].parse().ok())
        .unwrap_or(99);
    assert!(age <= 31, "{line}");

    let protocols = exec(&mut lab, r1, "show ip protocols");
    assert!(
        protocols.contains("Routing Protocol is \"rip\"\n"),
        "{protocols}"
    );
    assert!(
        protocols.contains("  Sending updates every 30 seconds, next due in "),
        "{protocols}"
    );
    assert!(
        protocols.contains("  Default version control: send version 2, receive version 2\n"),
        "{protocols}"
    );
    assert!(
        protocols.contains("    GigabitEthernet0/1    2     2\n")
            && !protocols.contains("    GigabitEthernet0/0    2"),
        "{protocols}"
    );
    assert!(
        protocols.contains("  Automatic network summarization is not in effect\n"),
        "{protocols}"
    );
    assert!(protocols.contains("  Routing for Networks:\n    10.0.0.0\n  Passive Interface(s):\n    GigabitEthernet0/0\n"), "{protocols}");
    assert!(
        protocols.contains("    10.0.12.2            120      00:00:"),
        "{protocols}"
    );
    let database = exec(&mut lab, r1, "show ip rip database");
    assert!(
        database.contains("10.0.12.0/30    directly connected, GigabitEthernet0/1\n"),
        "{database}"
    );
    assert!(
        database.contains("10.3.0.0/24\n    [2] via 10.0.12.2, 00:00:"),
        "{database}"
    );
    let run = lab.running_config(r1).unwrap();
    assert!(run.contains("router rip\n version 2\n passive-interface GigabitEthernet0/0\n network 10.0.0.0\n no auto-summary\n"), "{run}");

    // A default route can be handed out as well.
    cfg(&mut lab, r3, "router rip\n default-information originate");
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r1, "0.0.0.0/0").starts_with("R*    0.0.0.0/0 [120/2] via 10.0.12.2, "),
        "{}",
        exec(&mut lab, r1, "show ip route")
    );
    assert!(
        exec(&mut lab, r1, "show ip route")
            .contains("Gateway of last resort is 10.0.12.2 to network 0.0.0.0")
    );
}

#[test]
fn failures_withdraw_routes() {
    let (mut lab, [r1, r2, _r3], _, [_, l23]) = row();
    // A link that goes down is noticed at once and the bad news travels by triggered update.
    let fault = lab.inject(Fault::LinkDown { link: l23 }).unwrap();
    lab.advance(5 * SECOND);
    assert!(route(&mut lab, r2, "10.3.0.0/24").is_empty());
    assert!(
        route(&mut lab, r1, "10.3.0.0/24").is_empty(),
        "{}",
        exec(&mut lab, r1, "show ip route")
    );
    assert!(route(&mut lab, r1, "10.0.23.0/30").is_empty());
    lab.clear_fault(fault);
    lab.settle(60 * SECOND);
    assert!(route(&mut lab, r1, "10.3.0.0/24").contains("[120/2]"));

    // A neighbour that just falls silent is believed for 180 seconds.
    cfg(
        &mut lab,
        r2,
        "router rip\n passive-interface GigabitEthernet0/0",
    );
    lab.advance(120 * SECOND);
    assert!(
        route(&mut lab, r1, "10.3.0.0/24").contains("[120/2]"),
        "still within the invalid timer"
    );
    lab.advance(100 * SECOND);
    assert!(route(&mut lab, r1, "10.3.0.0/24").is_empty());
    assert!(!exec(&mut lab, r1, "show ip route").contains("\nR "));
    // R2 still listens on the passive interface.
    assert!(route(&mut lab, r2, "10.1.0.0/24").contains("via 10.0.12.1"));
}

#[test]
fn summarisation_and_versions() {
    // R1's LAN is a subnet of 10.0.0.0/8; the link to R2 lives in 192.168.12.0/24.
    let mut lab = Lab::new("summary");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    link(&mut lab, r1, "g0/1", r2, "g0/0");
    cfg(
        &mut lab,
        r1,
        "interface g0/0\n ip address 10.1.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 192.168.12.1 255.255.255.0\n no shutdown\nrouter rip\n version 2\n network 10.0.0.0\n network 192.168.12.0",
    );
    cfg(
        &mut lab,
        r2,
        "interface g0/0\n ip address 192.168.12.2 255.255.255.0\n no shutdown\nrouter rip\n version 2\n network 192.168.12.0",
    );
    // g0/0 on R1 has no cable; a loopback stands in for the LAN.
    cfg(
        &mut lab,
        r1,
        "interface Loopback1\n ip address 10.2.0.1 255.255.255.0",
    );
    lab.settle(60 * SECOND);
    // By default the subnet is announced as its classful network across the boundary.
    assert!(
        route(&mut lab, r2, "10.0.0.0/8")
            .starts_with("R     10.0.0.0/8 [120/1] via 192.168.12.1, "),
        "{}",
        exec(&mut lab, r2, "show ip route")
    );
    assert!(
        exec(&mut lab, r1, "show ip protocols")
            .contains("  Automatic network summarization is in effect\n")
    );
    assert!(exec(&mut lab, r1, "show ip rip database").contains("10.0.0.0/8    auto-summary\n"));

    cfg(&mut lab, r1, "router rip\n no auto-summary");
    lab.settle(60 * SECOND);
    let table = exec(&mut lab, r2, "show ip route");
    assert!(table.contains("      10.0.0.0/24 is subnetted, 1 subnets\nR        10.2.0.0 [120/1] via 192.168.12.1, "), "{table}");
    assert!(
        route(&mut lab, r2, "10.0.0.0/8").is_empty(),
        "the summary is withdrawn"
    );

    // A version 2 router ignores version 1 updates.
    cfg(&mut lab, r1, "router rip\n no version");
    lab.advance(200 * SECOND);
    assert!(
        !exec(&mut lab, r2, "show ip route").contains("\nR "),
        "{}",
        exec(&mut lab, r2, "show ip route")
    );
    assert!(
        exec(&mut lab, r1, "show ip protocols")
            .contains("  Default version control: send version 1, receive any version\n")
    );
    // Version 1 has no masks: R2 (also version 1 now) can only learn the classful network.
    cfg(&mut lab, r2, "router rip\n no version");
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r2, "10.0.0.0/8")
            .starts_with("R     10.0.0.0/8 [120/1] via 192.168.12.1, "),
        "{}",
        exec(&mut lab, r2, "show ip route")
    );
}

#[test]
fn administrative_distance_decides_between_protocols() {
    let (mut lab, [r1, r2, r3], _, _) = row();
    // OSPF (110) is preferred over RIP (120) for the same destination...
    for r in [r1, r2, r3] {
        cfg(
            &mut lab,
            r,
            "router ospf 1\n network 10.0.0.0 0.255.255.255 area 0",
        );
    }
    lab.settle(120 * SECOND);
    assert!(
        route(&mut lab, r1, "10.3.0.0/24")
            .starts_with("O        10.3.0.0/24 [110/3] via 10.0.12.2, "),
        "{}",
        exec(&mut lab, r1, "show ip route")
    );
    assert!(
        exec(&mut lab, r1, "show ip rip database").contains("10.3.0.0/24\n    [2] via 10.0.12.2"),
        "RIP still knows the route"
    );
    let protocols = exec(&mut lab, r1, "show ip protocols");
    assert!(
        protocols.contains("Routing Protocol is \"ospf 1\"")
            && protocols.contains("Routing Protocol is \"rip\""),
        "{protocols}"
    );
    // ...a static route (1) over both, and a floating static (130) over neither.
    cfg(&mut lab, r1, "ip route 10.3.0.0 255.255.255.0 10.0.12.2");
    assert!(
        route(&mut lab, r1, "10.3.0.0/24").starts_with("S        10.3.0.0/24 [1/0] via 10.0.12.2")
    );
    cfg(
        &mut lab,
        r1,
        "no ip route 10.3.0.0 255.255.255.0 10.0.12.2\nip route 10.3.0.0 255.255.255.0 10.0.12.2 130",
    );
    assert!(route(&mut lab, r1, "10.3.0.0/24").starts_with("O        10.3.0.0/24 [110/3]"));
    // Without OSPF the RIP route takes over again; without RIP the floating static does.
    cfg(&mut lab, r1, "no router ospf 1");
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r1, "10.3.0.0/24")
            .starts_with("R        10.3.0.0/24 [120/2] via 10.0.12.2, "),
        "{}",
        exec(&mut lab, r1, "show ip route")
    );
    cfg(&mut lab, r1, "no router rip");
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r1, "10.3.0.0/24")
            .starts_with("S        10.3.0.0/24 [130/0] via 10.0.12.2"),
        "{}",
        exec(&mut lab, r1, "show ip route")
    );
    assert_eq!(exec(&mut lab, r1, "show ip rip database"), "");
}
