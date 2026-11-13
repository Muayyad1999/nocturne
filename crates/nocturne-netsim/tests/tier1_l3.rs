//! Layer 3: ARP, routed interfaces, sub-interfaces, SVIs, static routing, ICMP.

mod common;

use std::net::Ipv4Addr;

use common::*;
use nocturne_netsim::Lab;

/// R1 -- R2 -- R3 in a row, a host behind R1 and one behind R3.
fn three_routers() -> (Lab, [u32; 3], [u32; 2]) {
    let mut lab = Lab::new("static");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let r3 = node(&mut lab, "router", "R3");
    let h1 = pc(&mut lab, "PC1", "10.1.0.10", Some("10.1.0.1"));
    let h3 = pc(&mut lab, "PC3", "10.3.0.10", Some("10.3.0.1"));
    link(&mut lab, h1, "Ethernet0", r1, "g0/0");
    link(&mut lab, r1, "g0/1", r2, "g0/0");
    link(&mut lab, r2, "g0/1", r3, "g0/0");
    link(&mut lab, r3, "g0/1", h3, "Ethernet0");
    cfg(&mut lab, r1, "interface g0/0\n ip address 10.1.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.0.12.1 255.255.255.252\n no shutdown\nip route 10.3.0.0 255.255.255.0 10.0.12.2\nip route 10.0.23.0 255.255.255.252 10.0.12.2");
    cfg(&mut lab, r2, "interface g0/0\n ip address 10.0.12.2 255.255.255.252\n no shutdown\ninterface g0/1\n ip address 10.0.23.1 255.255.255.252\n no shutdown\nip route 10.1.0.0 255.255.255.0 10.0.12.1\nip route 10.3.0.0 255.255.255.0 10.0.23.2");
    cfg(&mut lab, r3, "interface g0/0\n ip address 10.0.23.2 255.255.255.252\n no shutdown\ninterface g0/1\n ip address 10.3.0.1 255.255.255.0\n no shutdown\nip route 0.0.0.0 0.0.0.0 10.0.23.1");
    lab.settle(60 * SECOND);
    (lab, [r1, r2, r3], [h1, h3])
}

#[test]
fn static_routing_across_three_routers_and_traceroute() {
    let (mut lab, [r1, r2, r3], [h1, _h3]) = three_routers();
    // ARP has to resolve hop by hop, so the very first pings lose packets.
    let first = exec(&mut lab, h1, "ping 10.3.0.10 -n 6");
    assert!(replies(&first) >= 1 && replies(&first) < 6, "{first}");
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.3.0.10")), 4);
    // TTL: host default 128, three routers on the way back.
    assert!(exec(&mut lab, h1, "ping 10.3.0.10 -n 1").contains("TTL=125"));

    let trace = exec(&mut lab, r1, "traceroute 10.3.0.10");
    let hops: Vec<&str> = trace.lines().filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit())).collect();
    assert_eq!(hops.len(), 3, "{trace}");
    assert!(hops[0].trim_start().starts_with("1 10.0.12.2 "), "{trace}");
    assert!(hops[1].trim_start().starts_with("2 10.0.23.2 "), "{trace}");
    assert!(hops[2].trim_start().starts_with("3 10.3.0.10 "), "{trace}");
    let tracert = exec(&mut lab, h1, "tracert 10.3.0.10");
    for hop in ["10.1.0.1", "10.0.12.2", "10.0.23.2", "10.3.0.10"] {
        assert!(tracert.contains(hop), "{tracert}");
    }
    assert!(tracert.contains("Trace complete."), "{tracert}");

    let table = exec(&mut lab, r1, "show ip route");
    assert!(table.contains("Gateway of last resort is not set"), "{table}");
    assert!(table.contains("C        10.1.0.0/24 is directly connected, GigabitEthernet0/0"), "{table}");
    assert!(table.contains("L        10.1.0.1/32 is directly connected, GigabitEthernet0/0"), "{table}");
    assert!(table.contains("S        10.3.0.0/24 [1/0] via 10.0.12.2"), "{table}");
    let table = exec(&mut lab, r3, "show ip route");
    assert!(table.contains("Gateway of last resort is 10.0.23.1 to network 0.0.0.0"), "{table}");
    assert!(table.contains("S*    0.0.0.0/0 [1/0] via 10.0.23.1"), "{table}");
    let rows = lab.node_tables(r3).unwrap().routes;
    assert!(rows.iter().any(|r| r.code == "S*" && r.prefix == "0.0.0.0/0" && r.next_hop == "10.0.23.1"), "{rows:?}");
    assert!(rows.iter().all(|r| r.code != "L"));

    // No route on R2 for an unknown destination: ICMP unreachable comes back.
    let out = exec(&mut lab, r1, "ping 10.0.23.2");
    assert_eq!(ping_marks(&out), "!!!!!", "{out}");
    cfg(&mut lab, r1, "ip route 172.16.0.0 255.255.0.0 10.0.12.2");
    let out = exec(&mut lab, r1, "ping 172.16.1.1");
    assert_eq!(ping_marks(&out), "UUUUU", "{out}");
    let out = exec(&mut lab, h1, "ping 172.16.1.1 -n 2");
    assert!(out.contains("Reply from 10.0.12.2: Destination host unreachable."), "{out}");
    // No route at all on the origin.
    let out = exec(&mut lab, r2, "ping 192.0.2.1 repeat 2");
    assert_eq!(ping_marks(&out), "..", "{out}");
    let _ = r3;
}

#[test]
fn floating_static_takes_over_when_the_primary_path_fails() {
    let (mut lab, [r1, r2, _r3], [h1, _]) = three_routers();
    // A second, worse path R1 -> R2 over another pair of interfaces.
    link(&mut lab, r1, "g0/2", r2, "g0/2");
    cfg(&mut lab, r1, "interface g0/2\n ip address 10.0.99.1 255.255.255.252\n no shutdown\nip route 10.3.0.0 255.255.255.0 10.0.99.2 200");
    cfg(&mut lab, r2, "interface g0/2\n ip address 10.0.99.2 255.255.255.252\n no shutdown\nip route 10.1.0.0 255.255.255.0 10.0.99.1 200");
    lab.settle(30 * SECOND);
    let route = |lab: &Lab| {
        lab.node_tables(r1).unwrap().routes.into_iter().find(|r| r.prefix == "10.3.0.0/24").unwrap()
    };
    assert_eq!((route(&lab).next_hop.as_str(), route(&lab).ad), ("10.0.12.2", 1));
    // The primary next hop disappears with its interface; the floating static appears.
    cfg(&mut lab, r1, "interface g0/1\n shutdown");
    lab.settle(30 * SECOND);
    assert_eq!((route(&lab).next_hop.as_str(), route(&lab).ad), ("10.0.99.2", 200));
    let show = exec(&mut lab, r1, "show ip route static");
    assert!(show.contains("S        10.3.0.0/24 [200/0] via 10.0.99.2"), "{show}");
    exec(&mut lab, h1, "ping 10.3.0.10");
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.3.0.10")), 4);
    // Recursive static: a route whose next hop is reached through another static.
    cfg(&mut lab, r1, "ip route 192.0.2.0 255.255.255.0 10.3.0.10");
    lab.settle(5 * SECOND);
    let recursive = lab.node_tables(r1).unwrap().routes.into_iter().find(|r| r.prefix == "192.0.2.0/24");
    assert!(recursive.is_some_and(|r| r.next_hop == "10.3.0.10"));
}

#[test]
fn router_on_a_stick_routes_between_vlans() {
    let mut lab = Lab::new("stick");
    let r = node(&mut lab, "router", "R1");
    let sw = node(&mut lab, "switch", "SW1");
    let a = pc(&mut lab, "A", "10.0.10.10", Some("10.0.10.1"));
    let b = pc(&mut lab, "B", "10.0.20.10", Some("10.0.20.1"));
    link(&mut lab, r, "g0/0", sw, "gi0/1");
    link(&mut lab, a, "Ethernet0", sw, "fa0/1");
    link(&mut lab, b, "Ethernet0", sw, "fa0/2");
    cfg(&mut lab, sw, "vlan 10,20\ninterface fa0/1\n switchport mode access\n switchport access vlan 10\ninterface fa0/2\n switchport mode access\n switchport access vlan 20\ninterface gi0/1\n switchport mode trunk");
    cfg(&mut lab, r, "interface g0/0\n no shutdown\ninterface g0/0.10\n encapsulation dot1Q 10\n ip address 10.0.10.1 255.255.255.0\ninterface g0/0.20\n encapsulation dot1Q 20\n ip address 10.0.20.1 255.255.255.0");
    lab.settle(60 * SECOND);
    exec(&mut lab, a, "ping 10.0.20.10");
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.20.10")), 4);
    let brief = exec(&mut lab, r, "show ip interface brief");
    assert!(brief.contains("GigabitEthernet0/0.10      10.0.10.1       YES manual up                    up"), "{brief}");
    let shown = exec(&mut lab, r, "show interfaces g0/0.20");
    assert!(shown.contains("Encapsulation 802.1Q Virtual LAN, Vlan ID  20."), "{shown}");
    // The sub-interface needs its encapsulation before it can route.
    cfg(&mut lab, r, "interface g0/0.20\n no encapsulation dot1Q");
    lab.settle(5 * SECOND);
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.20.10 -n 2")), 0);
}

#[test]
fn layer3_switch_routes_between_svis() {
    let mut lab = Lab::new("svi");
    let sw = node(&mut lab, "switch-l3", "MLS1");
    let a = pc(&mut lab, "A", "10.0.10.10", Some("10.0.10.1"));
    let b = pc(&mut lab, "B", "10.0.20.10", Some("10.0.20.1"));
    let r = node(&mut lab, "router", "R1");
    link(&mut lab, a, "Ethernet0", sw, "g1/0/1");
    link(&mut lab, b, "Ethernet0", sw, "g1/0/2");
    link(&mut lab, r, "g0/0", sw, "g1/0/24");
    cfg(&mut lab, sw, "vlan 10,20\ninterface g1/0/1\n switchport mode access\n switchport access vlan 10\ninterface g1/0/2\n switchport mode access\n switchport access vlan 20\ninterface vlan 10\n ip address 10.0.10.1 255.255.255.0\ninterface vlan 20\n ip address 10.0.20.1 255.255.255.0\ninterface g1/0/24\n no switchport\n ip address 10.0.0.1 255.255.255.252");
    cfg(&mut lab, r, "interface g0/0\n ip address 10.0.0.2 255.255.255.252\n no shutdown\nip route 10.0.0.0 255.255.0.0 10.0.0.1");
    lab.settle(60 * SECOND);
    // Without `ip routing` the switch does not route between its SVIs.
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.20.10 -n 2")), 0);
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.10.1 -n 2")), 2);
    cfg(&mut lab, sw, "ip routing");
    lab.settle(10 * SECOND);
    exec(&mut lab, a, "ping 10.0.20.10");
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.20.10")), 4);
    // Through the routed port to the router and back.
    exec(&mut lab, a, "ping 10.0.0.2");
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.2")), 4);
    // An SVI is down while no port in its VLAN is up.
    cfg(&mut lab, sw, "vlan 30\ninterface vlan 30\n ip address 10.0.30.1 255.255.255.0");
    let brief = exec(&mut lab, sw, "show ip interface brief | include Vlan");
    assert!(brief.contains("Vlan10                     10.0.10.1       YES manual up                    up"), "{brief}");
    assert!(brief.contains("Vlan30                     10.0.30.1       YES manual down                  down"), "{brief}");
    let arp = exec(&mut lab, sw, "show ip arp");
    assert!(arp.contains("Internet  10.0.10.10") && arp.contains("Vlan10"), "{arp}");
}

#[test]
fn instant_probes_agree_with_the_simulation_and_leave_no_trace() {
    let (mut lab, [r1, r2, r3], [h1, h3]) = three_routers();
    let before = (lab.now(), lab.node_tables(r2).unwrap().arp.len(), lab.metrics(r2).unwrap().interfaces[0].in_pkts);
    let dst: Ipv4Addr = "10.3.0.10".parse().unwrap();
    let probe = lab.probe_ping(h1, dst);
    assert!(probe.ok, "{probe:?}");
    assert!(probe.rtt_us > 0 && probe.rtt_us < 10_000);
    let names: Vec<&str> = probe.hops.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["PC1", "R1", "R2", "R3", "PC3"]);
    assert_eq!(probe.hops[1].in_if, "GigabitEthernet0/0");
    assert_eq!(probe.hops[1].out_if, "GigabitEthernet0/1");
    assert_eq!(probe.hops[4].out_if, "");
    assert!(probe.hops[2].note.contains("routing table"), "{:?}", probe.hops[2]);
    // Nothing moved: no time passed, no ARP entries, no counters.
    let after = (lab.now(), lab.node_tables(r2).unwrap().arp.len(), lab.metrics(r2).unwrap().interfaces[0].in_pkts);
    assert_eq!(before, after);
    assert!(lab.take_events().iter().all(|e| !matches!(e, nocturne_netsim::LabEvent::Packet(_))));

    // And the timed simulation agrees.
    exec(&mut lab, h1, "ping 10.3.0.10 -n 8");
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.3.0.10")), 4);

    // A broken path is reported with the place and the reason.
    cfg(&mut lab, r2, "no ip route 10.3.0.0 255.255.255.0");
    let probe = lab.probe_ping(h1, dst);
    assert!(!probe.ok);
    assert!(probe.reason.contains("R2") && probe.reason.contains("no route"), "{}", probe.reason);
    assert_eq!(probe.hops.last().map(|h| h.name.as_str()), Some("R2"), "{:?}", probe.hops);
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.3.0.10 -n 2")), 0);

    // TCP: a closed port answers with a reset; telnet is open; SSH needs host keys.
    let probe = lab.probe_tcp(h1, "10.0.12.2".parse().unwrap(), 80);
    assert!(!probe.ok && probe.reason.contains("RST"), "{probe:?}");
    assert!(lab.probe_tcp(h1, "10.0.12.2".parse().unwrap(), 23).ok);
    assert!(!lab.probe_tcp(h1, "10.0.12.2".parse().unwrap(), 22).ok, "no RSA key yet");
    let _ = (r1, r3, h3);
}
