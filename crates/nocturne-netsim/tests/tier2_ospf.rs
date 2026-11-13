//! OSPF: adjacencies, SPF results, reconvergence, areas, externals.

mod common;

use common::*;
use nocturne_netsim::{Fault, Lab, LabEvent};

const R1: &str = "interface Loopback0\n ip address 1.1.1.1 255.255.255.255\ninterface g0/0\n ip address 10.0.12.1 255.255.255.252\n no shutdown\ninterface g0/1\n ip address 10.0.13.1 255.255.255.252\n no shutdown\ninterface g0/2\n ip address 10.1.0.1 255.255.255.0\n no shutdown\nrouter ospf 1\n network 10.0.0.0 0.0.255.255 area 0\n network 10.1.0.0 0.0.0.255 area 0\n network 1.1.1.1 0.0.0.0 area 0\n passive-interface GigabitEthernet0/2";
const R2: &str = "interface Loopback0\n ip address 2.2.2.2 255.255.255.255\ninterface g0/0\n ip address 10.0.12.2 255.255.255.252\n no shutdown\ninterface g0/1\n ip address 10.0.23.1 255.255.255.252\n no shutdown\nrouter ospf 1\n network 10.0.0.0 0.0.255.255 area 0\n network 2.2.2.2 0.0.0.0 area 0";
const R3: &str = "interface Loopback0\n ip address 3.3.3.3 255.255.255.255\ninterface g0/0\n ip address 10.0.23.2 255.255.255.252\n no shutdown\ninterface g0/1\n ip address 10.0.13.2 255.255.255.252\n no shutdown\ninterface g0/2\n ip address 10.3.0.1 255.255.255.0\n no shutdown\nrouter ospf 1\n network 10.0.0.0 0.0.255.255 area 0\n network 10.3.0.0 0.0.0.255 area 0\n network 3.3.3.3 0.0.0.0 area 0\n passive-interface GigabitEthernet0/2";

/// Three routers in a triangle, a host behind R1 and one behind R3.
/// Returns the lab, the routers, the hosts and the links (R1-R2, R2-R3, R1-R3).
fn triangle() -> (Lab, [u32; 3], [u32; 2], [u32; 3]) {
    let mut lab = Lab::new("ospf");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let r3 = node(&mut lab, "router", "R3");
    let h1 = pc(&mut lab, "PC1", "10.1.0.10", Some("10.1.0.1"));
    let h3 = pc(&mut lab, "PC3", "10.3.0.10", Some("10.3.0.1"));
    let l12 = link(&mut lab, r1, "g0/0", r2, "g0/0");
    let l23 = link(&mut lab, r2, "g0/1", r3, "g0/0");
    let l13 = link(&mut lab, r1, "g0/1", r3, "g0/1");
    link(&mut lab, h1, "Ethernet0", r1, "g0/2");
    link(&mut lab, h3, "Ethernet0", r3, "g0/2");
    cfg(&mut lab, r1, R1);
    cfg(&mut lab, r2, R2);
    cfg(&mut lab, r3, R3);
    lab.settle(120 * SECOND);
    (lab, [r1, r2, r3], [h1, h3], [l12, l23, l13])
}

fn syslog(lab: &mut Lab, node: u32) -> Vec<String> {
    lab.take_events()
        .into_iter()
        .filter_map(|e| match e {
            LabEvent::Syslog { node: n, line, .. } if n == node => Some(line),
            _ => None,
        })
        .collect()
}

/// The route line for a prefix in `show ip route`, without the age.
fn route(lab: &mut Lab, node: u32, prefix: &str) -> String {
    let table = exec(lab, node, "show ip route");
    table
        .lines()
        .find(|l| l.contains(&format!(" {prefix} ")))
        .unwrap_or("")
        .to_string()
}

#[test]
fn triangle_forms_adjacencies_and_routes() {
    let (mut lab, [r1, r2, r3], [h1, h3], _) = triangle();
    let logs = syslog(&mut lab, r1);
    assert!(
        logs.iter().any(|l| l.ends_with("%OSPF-5-ADJCHG: Process 1, Nbr 2.2.2.2 on GigabitEthernet0/0 from LOADING to FULL, Loading Done")),
        "{logs:#?}"
    );

    let neighbors = exec(&mut lab, r1, "show ip ospf neighbor");
    assert!(
        neighbors.contains(
            "Neighbor ID     Pri   State           Dead Time   Address         Interface\n"
        ),
        "{neighbors}"
    );
    assert!(
        neighbors.contains("2.2.2.2           1   FULL/DR         00:00:"),
        "{neighbors}"
    );
    assert!(
        neighbors.contains("   10.0.13.2       GigabitEthernet0/1\n"),
        "{neighbors}"
    );
    let rows = lab.node_tables(r1).unwrap().ospf_neighbors;
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|r| r.state == "FULL/DR"
            && r.area == "0"
            && r.dead_time_s > 20
            && r.dead_time_s <= 40),
        "{rows:?}"
    );
    // The higher router id wins the election on each segment.
    assert!(exec(&mut lab, r3, "show ip ospf neighbor").contains("1.1.1.1           1   FULL/BDR"));

    let table = exec(&mut lab, r1, "show ip route");
    assert!(table.contains("      2.0.0.0/32 is subnetted, 1 subnets\nO        2.2.2.2 [110/2] via 10.0.12.2, 00:00:"), "{table}");
    assert!(
        table.contains("O        10.3.0.0/24 [110/2] via 10.0.13.2, "),
        "{table}"
    );
    // Two equal-cost paths to the far side of the triangle.
    let ecmp = table
        .lines()
        .skip_while(|l| !l.starts_with("O        10.0.23.0/30 [110/2] via 10.0.12.2"))
        .nth(1)
        .unwrap_or("");
    assert!(
        ecmp.starts_with("                      [110/2] via 10.0.13.2, ")
            && ecmp.ends_with("GigabitEthernet0/1"),
        "{table}"
    );
    let only = exec(&mut lab, r2, "show ip route ospf");
    assert!(
        only.contains("O        10.1.0.0/24 [110/2] via 10.0.12.1") && !only.contains("\nC "),
        "{only}"
    );
    let routes = lab.node_tables(r1).unwrap().routes;
    assert!(
        routes.iter().any(|r| r.code == "O"
            && r.prefix == "3.3.3.3/32"
            && r.ad == 110
            && r.metric == 2
            && r.next_hop == "10.0.13.2"),
        "{routes:?}"
    );

    // Hosts on the passive LANs reach each other; no hellos leak onto the LAN.
    exec(&mut lab, h1, "ping 10.3.0.10");
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.3.0.10")), 4);
    let probe = lab.probe_ping(h1, "10.3.0.10".parse().unwrap());
    assert!(probe.ok, "{probe:?}");
    assert_eq!(
        probe
            .hops
            .iter()
            .map(|h| h.name.as_str())
            .collect::<Vec<_>>(),
        ["PC1", "R1", "R3", "PC3"]
    );
    let _ = h3;

    let brief = exec(&mut lab, r1, "show ip ospf interface brief");
    assert!(
        brief.contains("Lo0          1     0               1.1.1.1/32         1     LOOP  0/0\n"),
        "{brief}"
    );
    assert!(
        brief.contains("Gi0/0        1     0               10.0.12.1/30       1     BDR   1/1\n"),
        "{brief}"
    );
    assert!(
        brief.contains("Gi0/2        1     0               10.1.0.1/24        1     DR    0/0\n"),
        "{brief}"
    );
    let detail = exec(&mut lab, r1, "show ip ospf interface");
    assert!(
        detail.contains("  Designated Router (ID) 2.2.2.2, Interface address 10.0.12.2\n"),
        "{detail}"
    );
    assert!(
        detail.contains("    No Hellos (Passive interface) \n")
            && detail.contains("    Adjacent with neighbor 3.3.3.3  (Designated Router)\n"),
        "{detail}"
    );
    let process = exec(&mut lab, r1, "show ip ospf");
    assert!(
        process.starts_with(" Routing Process \"ospf 1\" with ID 1.1.1.1\n"),
        "{process}"
    );
    assert!(
        process.contains(
            "    Area BACKBONE(0)\n        Number of interfaces in this area is 4 (1 loopback)\n"
        ),
        "{process}"
    );
    let database = exec(&mut lab, r1, "show ip ospf database");
    assert!(database.contains("            OSPF Router with ID (1.1.1.1) (Process ID 1)\n\n\t\tRouter Link States (Area 0)\n"), "{database}");
    assert!(
        database.contains("\t\tNet Link States (Area 0)\n")
            && database.contains("\n10.0.12.2       2.2.2.2         "),
        "{database}"
    );
    let protocols = exec(&mut lab, r1, "show ip protocols");
    assert!(
        protocols.contains("Routing Protocol is \"ospf 1\"\n")
            && protocols.contains("    10.0.0.0 0.0.255.255 area 0\n"),
        "{protocols}"
    );
    assert!(
        protocols.contains("  Passive Interface(s):\n    GigabitEthernet0/2\n"),
        "{protocols}"
    );
    assert!(exec(&mut lab, r1, "show ip ospf neighbor detail").contains(" Neighbor 2.2.2.2, interface address 10.0.12.2\n    In the area 0 via interface GigabitEthernet0/0\n    Neighbor priority is 1, State is FULL, "));

    // Cost steers the choice: make the direct link expensive and R1 goes around.
    cfg(&mut lab, r1, "interface g0/1\n ip ospf cost 50");
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r1, "10.3.0.0/24")
            .starts_with("O        10.3.0.0/24 [110/3] via 10.0.12.2, ")
    );
    assert!(lab.running_config(r1).unwrap().contains(
        "interface GigabitEthernet0/1\n ip address 10.0.13.1 255.255.255.252\n ip ospf cost 50\n"
    ));
}

#[test]
fn reconverges_when_a_link_fails_and_recovers() {
    let (mut lab, [r1, _r2, r3], [h1, _], [_, _, l13]) = triangle();
    exec(&mut lab, h1, "ping 10.3.0.10");
    lab.take_events();
    let fault = lab.inject(Fault::LinkDown { link: l13 }).unwrap();
    lab.settle(60 * SECOND);
    // Both ends saw the carrier drop, so the adjacency goes at once.
    let logs = syslog(&mut lab, r1);
    assert!(
        logs.iter().any(|l| l.ends_with("%OSPF-5-ADJCHG: Process 1, Nbr 3.3.3.3 on GigabitEthernet0/1 from FULL to DOWN, Neighbor Down: Interface down or detached")),
        "{logs:#?}"
    );
    assert!(
        route(&mut lab, r1, "10.3.0.0/24")
            .starts_with("O        10.3.0.0/24 [110/3] via 10.0.12.2, ")
    );
    assert!(
        route(&mut lab, r3, "10.1.0.0/24")
            .starts_with("O        10.1.0.0/24 [110/3] via 10.0.23.1, ")
    );
    let probe = lab.probe_ping(h1, "10.3.0.10".parse().unwrap());
    assert!(probe.ok, "{probe:?}");
    assert_eq!(
        probe
            .hops
            .iter()
            .map(|h| h.name.as_str())
            .collect::<Vec<_>>(),
        ["PC1", "R1", "R2", "R3", "PC3"]
    );
    assert_eq!(lab.node_tables(r1).unwrap().ospf_neighbors.len(), 1);

    lab.clear_fault(fault);
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r1, "10.3.0.0/24")
            .starts_with("O        10.3.0.0/24 [110/2] via 10.0.13.2, ")
    );
    assert_eq!(lab.node_tables(r1).unwrap().ospf_neighbors.len(), 2);
}

#[test]
fn a_silent_failure_takes_the_dead_interval() {
    // R1 and R2 meet through a switch, so R1 cannot see R2's port go down.
    let mut lab = Lab::new("dead");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let sw = node(&mut lab, "switch", "SW1");
    link(&mut lab, r1, "g0/0", sw, "fa0/1");
    let far = link(&mut lab, r2, "g0/0", sw, "fa0/2");
    cfg(
        &mut lab,
        r1,
        "interface g0/0\n ip address 10.0.0.1 255.255.255.0\n no shutdown\nrouter ospf 1\n network 10.0.0.0 0.0.0.255 area 0",
    );
    cfg(
        &mut lab,
        r2,
        "interface Loopback0\n ip address 2.2.2.2 255.255.255.255\ninterface g0/0\n ip address 10.0.0.2 255.255.255.0\n no shutdown\nrouter ospf 1\n network 0.0.0.0 255.255.255.255 area 0",
    );
    lab.settle(120 * SECOND);
    assert!(route(&mut lab, r1, "2.2.2.2").starts_with("O        2.2.2.2 [110/2] via 10.0.0.2, "));
    lab.take_events();

    lab.inject(Fault::LinkDown { link: far }).unwrap();
    lab.advance(20 * SECOND);
    let rows = lab.node_tables(r1).unwrap().ospf_neighbors;
    assert!(
        rows.len() == 1 && rows[0].state.starts_with("FULL/") && rows[0].dead_time_s <= 20,
        "still waiting for hellos: {rows:?}"
    );
    assert!(
        !route(&mut lab, r1, "2.2.2.2").is_empty(),
        "the stale route stays until the dead timer fires"
    );
    lab.advance(25 * SECOND);
    assert!(lab.node_tables(r1).unwrap().ospf_neighbors.is_empty());
    assert!(route(&mut lab, r1, "2.2.2.2").is_empty());
    let logs = syslog(&mut lab, r1);
    assert!(
        logs.iter().any(|l| l.ends_with("%OSPF-5-ADJCHG: Process 1, Nbr 2.2.2.2 on GigabitEthernet0/0 from FULL to DOWN, Neighbor Down: Dead timer expired")),
        "{logs:#?}"
    );
}

#[test]
fn areas_and_inter_area_routes() {
    // R1 (area 1) -- R2 (border) -- R3 (backbone)
    let mut lab = Lab::new("areas");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let r3 = node(&mut lab, "router", "R3");
    link(&mut lab, r1, "g0/0", r2, "g0/0");
    link(&mut lab, r2, "g0/1", r3, "g0/0");
    cfg(
        &mut lab,
        r1,
        "interface Loopback0\n ip address 1.1.1.1 255.255.255.255\n ip ospf 1 area 1\ninterface g0/0\n ip address 10.0.12.1 255.255.255.252\n ip ospf 1 area 1\n no shutdown\nrouter ospf 1\n router-id 1.1.1.1",
    );
    cfg(
        &mut lab,
        r2,
        "interface g0/0\n ip address 10.0.12.2 255.255.255.252\n no shutdown\ninterface g0/1\n ip address 10.0.23.1 255.255.255.252\n no shutdown\nrouter ospf 1\n router-id 2.2.2.2\n network 10.0.12.0 0.0.0.3 area 1\n network 10.0.23.0 0.0.0.3 area 0",
    );
    cfg(
        &mut lab,
        r3,
        "interface Loopback0\n ip address 3.3.3.3 255.255.255.255\ninterface g0/0\n ip address 10.0.23.2 255.255.255.252\n no shutdown\nrouter ospf 1\n router-id 3.3.3.3\n network 0.0.0.0 255.255.255.255 area 0",
    );
    lab.settle(120 * SECOND);

    assert!(
        route(&mut lab, r1, "3.3.3.3").starts_with("O IA     3.3.3.3 [110/3] via 10.0.12.2, "),
        "{}",
        exec(&mut lab, r1, "show ip route")
    );
    assert!(
        route(&mut lab, r1, "10.0.23.0/30")
            .starts_with("O IA     10.0.23.0/30 [110/2] via 10.0.12.2, ")
    );
    assert!(route(&mut lab, r3, "1.1.1.1").starts_with("O IA     1.1.1.1 [110/3] via 10.0.23.1, "));
    // The border router itself has intra-area routes into both areas.
    assert!(route(&mut lab, r2, "1.1.1.1").starts_with("O        1.1.1.1 [110/2] via 10.0.12.1, "));
    // Forming the adjacencies already resolved the neighbours' MAC addresses,
    // so not even the first packet is lost to ARP.
    assert_eq!(
        ping_marks(&exec(&mut lab, r1, "ping 3.3.3.3 source Loopback0")),
        "!!!!!"
    );
    assert!(exec(&mut lab, r1, "show ip arp").contains("10.0.12.2"));

    assert!(exec(&mut lab, r2, "show ip ospf").contains(" It is an area border router\n"));
    let brief = exec(&mut lab, r2, "show ip ospf interface brief");
    assert!(
        brief.contains("Gi0/0        1     1               10.0.12.2/30")
            && brief.contains("Gi0/1        1     0               10.0.23.1/30"),
        "{brief}"
    );
    let database = exec(&mut lab, r1, "show ip ospf database");
    assert!(
        database.contains("\t\tRouter Link States (Area 1)\n")
            && database.contains("\t\tSummary Net Link States (Area 1)\n"),
        "{database}"
    );
    assert!(
        database.contains("\n3.3.3.3         2.2.2.2         "),
        "the border router advertises the summary: {database}"
    );
    assert!(
        !database.contains("(Area 0)"),
        "R1 has no backbone database: {database}"
    );
    let rows = lab.node_tables(r1).unwrap().ospf_neighbors;
    assert!(rows.len() == 1 && rows[0].area == "1", "{rows:?}");
    let run = lab.running_config(r1).unwrap();
    assert!(
        run.contains(
            "interface Loopback0\n ip address 1.1.1.1 255.255.255.255\n ip ospf 1 area 1\n"
        ),
        "{run}"
    );

    // An area mismatch on a link keeps the routers apart.
    cfg(&mut lab, r1, "interface g0/0\n ip ospf 1 area 2");
    lab.settle(120 * SECOND);
    assert!(lab.node_tables(r1).unwrap().ospf_neighbors.is_empty());
    assert!(route(&mut lab, r1, "3.3.3.3").is_empty());
}

#[test]
fn externals_default_route_and_redistribution() {
    let (mut lab, [r1, r2, r3], [h1, _], _) = triangle();
    // Without a default route of its own R3 does not originate one...
    cfg(
        &mut lab,
        r3,
        "router ospf 1\n default-information originate",
    );
    lab.settle(60 * SECOND);
    assert!(route(&mut lab, r1, "0.0.0.0/0").is_empty());
    // ...until it has one (or `always` is given).
    cfg(
        &mut lab,
        r3,
        "ip route 0.0.0.0 0.0.0.0 10.3.0.10\nip route 172.16.5.0 255.255.255.0 10.3.0.10",
    );
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r1, "0.0.0.0/0").starts_with("O*E2  0.0.0.0/0 [110/1] via 10.0.13.2, "),
        "{}",
        exec(&mut lab, r1, "show ip route")
    );
    assert!(
        exec(&mut lab, r1, "show ip route")
            .contains("Gateway of last resort is 10.0.13.2 to network 0.0.0.0")
    );
    assert!(
        route(&mut lab, r1, "172.16.5.0").is_empty(),
        "statics are not redistributed unless asked"
    );

    cfg(&mut lab, r3, "router ospf 1\n redistribute static subnets");
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r2, "172.16.5.0")
            .starts_with("O E2     172.16.5.0 [110/20] via 10.0.23.2, "),
        "{}",
        exec(&mut lab, r2, "show ip route")
    );
    assert!(
        exec(&mut lab, r3, "show ip ospf")
            .contains(" It is an autonomous system boundary router\n")
    );
    let database = exec(&mut lab, r1, "show ip ospf database");
    assert!(
        database.contains("\t\tType-5 AS External Link States\n")
            && database.contains("\n172.16.5.0      3.3.3.3         "),
        "{database}"
    );
    let rows = lab.node_tables(r1).unwrap().routes;
    assert!(
        rows.iter()
            .any(|r| r.code == "O*E2" && r.prefix == "0.0.0.0/0"),
        "{rows:?}"
    );
    let run = lab.running_config(r3).unwrap();
    assert!(
        run.contains(" redistribute static subnets\n")
            && run.contains(" default-information originate\n"),
        "{run}"
    );
    // The host follows the default route out through R3.
    let probe = lab.probe_ping(h1, "172.16.5.1".parse().unwrap());
    assert!(probe.hops.iter().any(|h| h.name == "R3"), "{probe:?}");

    cfg(
        &mut lab,
        r3,
        "no ip route 0.0.0.0 0.0.0.0 10.3.0.10\nrouter ospf 1\n no redistribute static",
    );
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r1, "0.0.0.0/0").is_empty() && route(&mut lab, r1, "172.16.5.0").is_empty()
    );
    cfg(
        &mut lab,
        r3,
        "router ospf 1\n default-information originate always",
    );
    lab.settle(60 * SECOND);
    assert!(
        route(&mut lab, r2, "0.0.0.0/0").starts_with("O*E2  0.0.0.0/0 [110/1] via 10.0.23.2, ")
    );

    // A static route beats OSPF for the same prefix; a floating one does not.
    cfg(&mut lab, r1, "ip route 10.3.0.0 255.255.255.0 10.0.12.2");
    assert!(
        route(&mut lab, r1, "10.3.0.0/24").starts_with("S        10.3.0.0/24 [1/0] via 10.0.12.2")
    );
    cfg(
        &mut lab,
        r1,
        "no ip route 10.3.0.0 255.255.255.0 10.0.12.2\nip route 10.3.0.0 255.255.255.0 10.0.12.2 200",
    );
    assert!(
        route(&mut lab, r1, "10.3.0.0/24")
            .starts_with("O        10.3.0.0/24 [110/2] via 10.0.13.2")
    );
}

#[test]
fn mismatches_keep_adjacencies_from_forming() {
    let (mut lab, [r1, r2, _r3], _, _) = triangle();
    lab.take_events();
    // Hello timers must agree. Nothing resets: the neighbours just stop hearing
    // acceptable hellos and time each other out.
    cfg(&mut lab, r1, "interface g0/0\n ip ospf hello-interval 5");
    lab.advance(10 * SECOND);
    assert_eq!(
        lab.node_tables(r1).unwrap().ospf_neighbors.len(),
        2,
        "not yet timed out"
    );
    lab.advance(40 * SECOND);
    let logs = syslog(&mut lab, r2);
    assert!(
        logs.iter().any(|l| l.contains(
            "Nbr 1.1.1.1 on GigabitEthernet0/0 from FULL to DOWN, Neighbor Down: Dead timer expired"
        )),
        "{logs:#?}"
    );
    assert!(!exec(&mut lab, r1, "show ip ospf neighbor").contains("2.2.2.2"));
    assert!(
        exec(&mut lab, r1, "show ip ospf interface g0/0")
            .contains("  Timer intervals configured, Hello 5, Dead 20, Wait 20, Retransmit 5\n")
    );
    cfg(&mut lab, r1, "interface g0/0\n no ip ospf hello-interval");
    lab.settle(120 * SECOND);
    assert!(exec(&mut lab, r1, "show ip ospf neighbor").contains("2.2.2.2           1   FULL/DR"));

    // An MTU mismatch gets as far as the database exchange and sticks there.
    cfg(&mut lab, r2, "interface g0/0\n mtu 1400");
    lab.settle(120 * SECOND);
    let stuck = exec(&mut lab, r1, "show ip ospf neighbor");
    assert!(stuck.contains("2.2.2.2           1   EXSTART/"), "{stuck}");
    assert!(
        route(&mut lab, r1, "2.2.2.2").contains("via 10.0.13.2"),
        "R2 is only reachable the long way round"
    );
    cfg(&mut lab, r2, "interface g0/0\n no mtu");
    lab.settle(120 * SECOND);
    assert!(route(&mut lab, r1, "2.2.2.2").contains("via 10.0.12.2"));

    // Passive interfaces send no hellos, so no neighbour forms, yet the subnet is advertised.
    cfg(
        &mut lab,
        r2,
        "router ospf 1\n passive-interface default\n no passive-interface GigabitEthernet0/1",
    );
    lab.settle(120 * SECOND);
    let neighbors = exec(&mut lab, r2, "show ip ospf neighbor");
    assert!(
        neighbors.contains("3.3.3.3") && !neighbors.contains("1.1.1.1"),
        "{neighbors}"
    );
    assert!(route(&mut lab, r1, "10.0.23.0/30").contains("via 10.0.13.2"));
    let run = lab.running_config(r2).unwrap();
    assert!(
        run.contains(" passive-interface default\n no passive-interface GigabitEthernet0/1\n"),
        "{run}"
    );
}

#[test]
fn clearing_the_process_and_priorities() {
    let (mut lab, [r1, r2, _r3], _, _) = triangle();
    // Priority 0 never becomes DR or BDR; with a higher priority R1 wins after a reset.
    cfg(&mut lab, r2, "interface g0/0\n ip ospf priority 0");
    cfg(&mut lab, r1, "interface g0/0\n ip ospf priority 200");
    lab.take_events();
    exec(&mut lab, r1, "clear ip ospf process");
    exec(&mut lab, r2, "clear ip ospf process");
    lab.settle(120 * SECOND);
    let logs = syslog(&mut lab, r1);
    assert!(
        logs.iter()
            .any(|l| l.contains("from FULL to DOWN, Neighbor Down: Interface down or detached")),
        "{logs:#?}"
    );
    assert!(
        logs.iter()
            .any(|l| l
                .contains("Nbr 2.2.2.2 on GigabitEthernet0/0 from LOADING to FULL, Loading Done")),
        "{logs:#?}"
    );
    assert!(
        exec(&mut lab, r1, "show ip ospf neighbor").contains("2.2.2.2           0   FULL/DROTHER")
    );
    assert!(exec(&mut lab, r2, "show ip ospf neighbor").contains("1.1.1.1         200   FULL/DR"));
    assert!(
        exec(&mut lab, r1, "show ip ospf interface brief")
            .contains("Gi0/0        1     0               10.0.12.1/30       1     DR    1/1")
    );

    // Removing the process withdraws everything it learned.
    cfg(&mut lab, r1, "no router ospf 1");
    lab.settle(60 * SECOND);
    assert_eq!(
        exec(&mut lab, r1, "show ip ospf neighbor"),
        "%OSPF: No router process is configured\n"
    );
    assert!(!exec(&mut lab, r1, "show ip route").contains("\nO "));
    assert!(route(&mut lab, r2, "10.1.0.0/24").is_empty());
}
