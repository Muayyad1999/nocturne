//! HSRP: election, virtual gateway, failover and pre-emption.

mod common;

use common::*;
use nocturne_netsim::{HostConfig, Lab, LabEvent};

/// Two routers share the gateway role for a client LAN and a server LAN.
fn gateways() -> (Lab, [u32; 2], u32, u32) {
    let mut lab = Lab::new("hsrp");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let sw1 = node(&mut lab, "switch", "SW1");
    let sw2 = node(&mut lab, "switch", "SW2");
    let pc1 = pc(&mut lab, "PC1", "10.0.10.10", Some("10.0.10.1"));
    let srv = node(&mut lab, "server", "SRV");
    lab.set_host_config(
        srv,
        HostConfig {
            services: vec!["http".into()],
            ..host("10.0.20.10", "255.255.255.0", Some("10.0.20.1"))
        },
    )
    .unwrap();
    link(&mut lab, r1, "g0/0", sw1, "gi0/1");
    link(&mut lab, r2, "g0/0", sw1, "gi0/2");
    link(&mut lab, pc1, "Ethernet0", sw1, "fa0/1");
    link(&mut lab, r1, "g0/1", sw2, "gi0/1");
    link(&mut lab, r2, "g0/1", sw2, "gi0/2");
    link(&mut lab, srv, "Ethernet0", sw2, "fa0/1");
    cfg(
        &mut lab,
        r1,
        "interface g0/0\n ip address 10.0.10.2 255.255.255.0\n standby 1 ip 10.0.10.1\n standby 1 priority 110\n standby 1 preempt\n no shutdown\ninterface g0/1\n ip address 10.0.20.2 255.255.255.0\n standby 2 ip 10.0.20.1\n standby 2 priority 110\n no shutdown",
    );
    cfg(
        &mut lab,
        r2,
        "interface g0/0\n ip address 10.0.10.3 255.255.255.0\n standby 1 ip 10.0.10.1\n no shutdown\ninterface g0/1\n ip address 10.0.20.3 255.255.255.0\n standby 2 ip 10.0.20.1\n no shutdown",
    );
    lab.settle(120 * SECOND);
    (lab, [r1, r2], pc1, srv)
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

fn brief_row(lab: &mut Lab, node: u32, iface: &str) -> String {
    exec(lab, node, "show standby brief")
        .lines()
        .find(|l| l.starts_with(iface))
        .unwrap_or("")
        .to_string()
}

#[test]
fn election_virtual_gateway_and_failover() {
    let (mut lab, [r1, r2], pc1, _srv) = gateways();
    let logs = syslog(&mut lab, r1);
    assert!(
        logs.iter().any(|l| l
            .ends_with("%HSRP-5-STATECHANGE: GigabitEthernet0/0 Grp 1 state Standby -> Active")),
        "{logs:#?}"
    );
    // The higher priority wins the election; the other router stands by.
    assert_eq!(
        brief_row(&mut lab, r1, "Gi0/0"),
        "Gi0/0       1    110 P Active  local           10.0.10.3       10.0.10.1"
    );
    assert_eq!(
        brief_row(&mut lab, r2, "Gi0/0"),
        "Gi0/0       1    100   Standby 10.0.10.2       local           10.0.10.1"
    );
    let detail = exec(&mut lab, r1, "show standby");
    assert!(
        detail.starts_with("GigabitEthernet0/0 - Group 1\n  State is Active\n"),
        "{detail}"
    );
    assert!(
        detail.contains(
            "  Virtual IP address is 10.0.10.1\n  Active virtual MAC address is 0000.0c07.ac01\n"
        ),
        "{detail}"
    );
    assert!(detail.contains("  Preemption enabled\n  Active router is local\n  Standby router is 10.0.10.3, priority 100 (expires in "), "{detail}");
    let run = lab.running_config(r1).unwrap();
    assert!(run.contains(" ip address 10.0.10.2 255.255.255.0\n standby 1 ip 10.0.10.1\n standby 1 priority 110\n standby 1 preempt\n"), "{run}");

    // Hosts use the virtual address as their gateway and resolve it to the virtual MAC.
    exec(&mut lab, pc1, "ping 10.0.20.10");
    assert_eq!(replies(&exec(&mut lab, pc1, "ping 10.0.20.10")), 4);
    assert_eq!(replies(&exec(&mut lab, pc1, "ping 10.0.10.1")), 4);
    assert!(exec(&mut lab, pc1, "arp -a").contains("10.0.10.1             00-00-0c-07-ac-01"));
    let probe = lab.probe_ping(pc1, "10.0.20.10".parse().unwrap());
    assert!(
        probe.ok
            && probe.hops.iter().any(|h| h.name == "R1")
            && !probe.hops.iter().any(|h| h.name == "R2"),
        "{probe:?}"
    );
    // The standby router does not answer for the virtual address.
    assert!(
        lab.probe_ping(r2, "10.0.10.1".parse().unwrap())
            .hops
            .iter()
            .any(|h| h.name == "R1")
    );

    // The active router dies: after the hold time the standby router takes over,
    // and the hosts notice nothing because the virtual MAC moves with the role.
    exec(&mut lab, r1, "write memory");
    lab.take_events();
    lab.set_power(r1, false);
    lab.advance(5 * SECOND);
    assert!(
        brief_row(&mut lab, r2, "Gi0/0").contains("Standby 10.0.10.2"),
        "not yet: the hold time is 10 seconds"
    );
    assert!(!lab.probe_ping(pc1, "10.0.20.10".parse().unwrap()).ok);
    lab.advance(7 * SECOND);
    assert_eq!(
        brief_row(&mut lab, r2, "Gi0/0"),
        "Gi0/0       1    100   Active  local           unknown         10.0.10.1"
    );
    let logs = syslog(&mut lab, r2);
    assert!(
        logs.iter().any(|l| l
            .ends_with("%HSRP-5-STATECHANGE: GigabitEthernet0/0 Grp 1 state Standby -> Active")),
        "{logs:#?}"
    );
    exec(&mut lab, pc1, "ping 10.0.20.10");
    assert_eq!(replies(&exec(&mut lab, pc1, "ping 10.0.20.10")), 4);
    assert!(exec(&mut lab, pc1, "arp -a").contains("10.0.10.1             00-00-0c-07-ac-01"));
    let probe = lab.probe_ping(pc1, "10.0.20.10".parse().unwrap());
    assert!(
        probe.ok && probe.hops.iter().any(|h| h.name == "R2"),
        "{probe:?}"
    );

    // R1 returns. With `preempt` it takes group 1 back; without it R2 keeps group 2.
    lab.set_power(r1, true);
    lab.settle(120 * SECOND);
    assert_eq!(
        brief_row(&mut lab, r1, "Gi0/0"),
        "Gi0/0       1    110 P Active  local           10.0.10.3       10.0.10.1"
    );
    assert_eq!(
        brief_row(&mut lab, r2, "Gi0/0"),
        "Gi0/0       1    100   Standby 10.0.10.2       local           10.0.10.1"
    );
    assert_eq!(
        brief_row(&mut lab, r1, "Gi0/1"),
        "Gi0/1       2    110   Standby 10.0.20.3       local           10.0.20.1"
    );
    assert_eq!(
        brief_row(&mut lab, r2, "Gi0/1"),
        "Gi0/1       2    100   Active  local           10.0.20.2       10.0.20.1"
    );
    let logs = syslog(&mut lab, r2);
    assert!(
        logs.iter()
            .any(|l| l
                .ends_with("%HSRP-5-STATECHANGE: GigabitEthernet0/0 Grp 1 state Active -> Speak")),
        "{logs:#?}"
    );
    exec(&mut lab, pc1, "ping 10.0.20.10");
    assert_eq!(
        replies(&exec(&mut lab, pc1, "ping 10.0.20.10")),
        4,
        "asymmetric paths still work"
    );
}

#[test]
fn priorities_versions_and_removal() {
    let (mut lab, [r1, r2], pc1, _srv) = gateways();
    // Raising the priority only matters once the router may pre-empt.
    cfg(&mut lab, r2, "interface g0/1\n standby 2 priority 200");
    lab.settle(60 * SECOND);
    assert!(brief_row(&mut lab, r2, "Gi0/1").contains("200   Standby"));
    cfg(&mut lab, r2, "interface g0/1\n standby 2 preempt");
    lab.settle(60 * SECOND);
    assert_eq!(
        brief_row(&mut lab, r2, "Gi0/1"),
        "Gi0/1       2    200 P Active  local           10.0.20.2       10.0.20.1"
    );
    assert!(brief_row(&mut lab, r1, "Gi0/1").contains("Standby 10.0.20.3       local"));

    // Version 2 uses another virtual MAC address (and group range).
    cfg(&mut lab, r1, "interface g0/0\n standby version 2");
    cfg(&mut lab, r2, "interface g0/0\n standby version 2");
    lab.settle(60 * SECOND);
    let detail = exec(&mut lab, r1, "show standby");
    assert!(
        detail.contains("GigabitEthernet0/0 - Group 1 (version 2)\n")
            && detail.contains("  Active virtual MAC address is 0000.0c9f.f001\n"),
        "{detail}"
    );
    assert!(
        lab.running_config(r1)
            .unwrap()
            .contains(" standby version 2\n standby 1 ip 10.0.10.1\n")
    );
    exec(&mut lab, pc1, "ping 10.0.10.1");
    exec(&mut lab, pc1, "ping 10.0.20.10");
    assert_eq!(replies(&exec(&mut lab, pc1, "ping 10.0.20.10")), 4);

    // A virtual address must not be the interface's own, and should be on its subnet.
    let outcome = lab
        .configure(
            r1,
            &[
                "interface g0/1".into(),
                " standby 2 ip 10.0.20.2".into(),
                " standby 3 ip 172.16.0.1".into(),
                " standby 300 ip 10.0.20.9".into(),
            ],
        )
        .unwrap();
    assert_eq!(
        outcome.lines[1].output,
        "% Address cannot equal interface IP address\n"
    );
    assert_eq!(
        outcome.lines[2].output,
        "% Warning: address is not within a subnet on this interface\n"
    );
    assert!(
        outcome.lines[3].error,
        "group 300 needs version 2: {outcome:?}"
    );
    assert!(
        exec(&mut lab, r1, "show standby").contains(
            "GigabitEthernet0/1 - Group 3\n  State is Init (virtual IP in wrong subnet)\n"
        )
    );

    // Removing the group on the active router hands the role over at once.
    cfg(&mut lab, r1, "interface g0/0\n no standby 1 ip");
    lab.advance(SECOND);
    assert!(
        brief_row(&mut lab, r2, "Gi0/0").contains("Active  local"),
        "{}",
        exec(&mut lab, r2, "show standby brief")
    );
    assert!(brief_row(&mut lab, r1, "Gi0/0").is_empty());
}
