//! Neighbour discovery, DHCP and access lists.

mod common;

use common::*;
use nocturne_netsim::{HostConfig, Lab, LabEvent};

fn dhcp_host(lab: &mut Lab, name: &str) -> u32 {
    let id = node(lab, "pc", name);
    lab.set_host_config(
        id,
        HostConfig {
            dhcp: true,
            ..HostConfig::default()
        },
    )
    .unwrap();
    id
}

fn syslog(lab: &mut Lab) -> Vec<String> {
    lab.take_events()
        .into_iter()
        .filter_map(|e| match e {
            LabEvent::Syslog { line, .. } => Some(line),
            _ => None,
        })
        .collect()
}

#[test]
fn cdp_and_lldp_neighbour_tables() {
    let mut lab = Lab::new("neighbours");
    let s1 = node(&mut lab, "switch", "SW1");
    let s2 = node(&mut lab, "switch", "SW2");
    let r1 = node(&mut lab, "router", "R1");
    link(&mut lab, s1, "gi0/1", s2, "gi0/2");
    link(&mut lab, s1, "fa0/24", r1, "g0/0");
    cfg(
        &mut lab,
        r1,
        "interface g0/0\n ip address 192.168.1.1 255.255.255.0\n no shutdown",
    );
    cfg(
        &mut lab,
        s2,
        "interface vlan 1\n ip address 192.168.1.3 255.255.255.0\n no shutdown",
    );
    lab.settle(60 * SECOND);

    let table = exec(&mut lab, s1, "show cdp neighbors");
    assert!(
        table.contains(
            "Device ID        Local Intrfce     Holdtme    Capability  Platform  Port ID"
        ),
        "{table}"
    );
    assert!(
        table.contains("R1               Fas 0/24          1"),
        "{table}"
    );
    assert!(table.contains("R     NV-ROUTER Gig 0/0"), "{table}");
    assert!(
        table.contains("SW2              Gig 0/1           1")
            && table.contains("S I   NV-SWITCH Gig 0/2"),
        "{table}"
    );
    assert!(table.contains("Total cdp entries displayed : 2"), "{table}");
    let detail = exec(&mut lab, s1, "show cdp neighbors detail");
    assert!(detail.contains("Device ID: R1\nEntry address(es): \n  IP address: 192.168.1.1\nPlatform: Nocturne NV-ROUTER-4GE,  Capabilities: Router \n"), "{detail}");
    assert!(
        detail
            .contains("Interface: FastEthernet0/24,  Port ID (outgoing port): GigabitEthernet0/0"),
        "{detail}"
    );
    let rows = lab.node_tables(s1).unwrap().neighbors;
    assert_eq!(rows.len(), 2);
    let sw2 = rows.iter().find(|r| r.device == "SW2").unwrap();
    assert_eq!(
        (
            sw2.protocol.as_str(),
            sw2.local_if.as_str(),
            sw2.remote_if.as_str(),
            sw2.mgmt_ip.as_str()
        ),
        (
            "CDP",
            "GigabitEthernet0/1",
            "GigabitEthernet0/2",
            "192.168.1.3"
        )
    );

    // LLDP is off until `lldp run`, and needs both ends.
    assert!(exec(&mut lab, s1, "show lldp neighbors").contains("% LLDP is not enabled"));
    cfg(&mut lab, s1, "lldp run");
    cfg(&mut lab, s2, "lldp run");
    lab.advance(SECOND);
    let lldp = exec(&mut lab, s1, "show lldp neighbors");
    assert!(
        lldp.contains("SW2                 Gi0/1          1")
            && lldp.contains("B               Gi0/2"),
        "{lldp}"
    );
    assert!(lldp.contains("Total entries displayed: 1"), "{lldp}");
    let detail = exec(&mut lab, s1, "show lldp neighbors detail");
    assert!(
        detail.contains("Local Intf: Gi0/1\nChassis id: 024e.")
            && detail.contains("System Name: SW2\n")
            && detail.contains("    IP: 192.168.1.3\n"),
        "{detail}"
    );
    assert_eq!(
        lab.node_tables(s1)
            .unwrap()
            .neighbors
            .iter()
            .filter(|r| r.protocol == "LLDP")
            .count(),
        1
    );

    // Per-interface and global switches.
    cfg(&mut lab, r1, "interface g0/0\n no cdp enable");
    lab.advance(200 * SECOND);
    assert!(
        !exec(&mut lab, s1, "show cdp neighbors").contains("R1 "),
        "the entry aged out after the hold time"
    );
    cfg(&mut lab, s1, "no cdp run");
    assert!(exec(&mut lab, s1, "show cdp neighbors").contains("% CDP is not enabled"));
    assert!(
        lab.node_tables(s1)
            .unwrap()
            .neighbors
            .iter()
            .all(|r| r.protocol == "LLDP")
    );
    cfg(&mut lab, s1, "cdp run");
    // SW1 hears SW2 again with its next periodic advertisement.
    lab.advance(61 * SECOND);
    assert!(exec(&mut lab, s1, "show cdp neighbors").contains("SW2 "));

    // CDP complains about mismatched native VLANs and duplex settings.
    cfg(
        &mut lab,
        s1,
        "vlan 10\ninterface gi0/1\n switchport mode trunk\n switchport trunk native vlan 10",
    );
    cfg(&mut lab, s2, "interface gi0/2\n switchport mode trunk");
    syslog(&mut lab);
    lab.advance(70 * SECOND);
    let logs = syslog(&mut lab);
    assert!(
        logs.iter().any(|l| l.contains("%CDP-4-NATIVE_VLAN_MISMATCH: Native VLAN mismatch discovered on GigabitEthernet0/1 (10), with SW2 GigabitEthernet0/2 (1).")),
        "{logs:#?}"
    );
    cfg(&mut lab, r1, "interface g0/0\n cdp enable");
    cfg(&mut lab, s1, "interface fa0/24\n duplex full\n speed 100");
    lab.advance(70 * SECOND);
    let logs = syslog(&mut lab);
    assert!(
        logs.iter().any(|l| l.contains("%CDP-4-DUPLEX_MISMATCH: duplex mismatch discovered on FastEthernet0/24 (not half duplex), with R1 GigabitEthernet0/0 (half duplex).")),
        "{logs:#?}"
    );
}

#[test]
fn dhcp_server_on_the_local_subnet() {
    let mut lab = Lab::new("dhcp");
    let r = node(&mut lab, "router", "R1");
    let sw = node(&mut lab, "switch", "SW1");
    let a = dhcp_host(&mut lab, "PC1");
    let b = dhcp_host(&mut lab, "PC2");
    link(&mut lab, r, "g0/0", sw, "gi0/1");
    link(&mut lab, a, "Ethernet0", sw, "fa0/1");
    link(&mut lab, b, "Ethernet0", sw, "fa0/2");
    cfg(
        &mut lab,
        r,
        "interface g0/0\n ip address 192.168.10.1 255.255.255.0\n no shutdown\nip dhcp excluded-address 192.168.10.1 192.168.10.10\nip dhcp pool LAN\n network 192.168.10.0 255.255.255.0\n default-router 192.168.10.1\n dns-server 192.168.10.5 8.8.8.8\n lease 0 12",
    );
    lab.settle(120 * SECOND);

    let ips: Vec<String> = [a, b]
        .iter()
        .map(|n| {
            lab.snapshot()
                .nodes
                .iter()
                .find(|v| v.id == *n)
                .unwrap()
                .mgmt_ip
                .clone()
                .unwrap_or_default()
        })
        .collect();
    let mut sorted = ips.clone();
    sorted.sort();
    assert_eq!(
        sorted,
        ["192.168.10.11", "192.168.10.12"],
        "excluded addresses are skipped"
    );
    let config = exec(&mut lab, a, "ipconfig /all");
    assert!(
        config.contains("DHCP Enabled. . . . . . . . . . . : Yes"),
        "{config}"
    );
    assert!(
        config.contains(&format!("IPv4 Address. . . . . . . . . . . : {}", ips[0])),
        "{config}"
    );
    assert!(config.contains("Subnet Mask . . . . . . . . . . . : 255.255.255.0"));
    assert!(config.contains("Default Gateway . . . . . . . . . : 192.168.10.1"));
    assert!(config.contains("DHCP Server . . . . . . . . . . . : 192.168.10.1"));
    assert!(
        config.contains("DNS Servers . . . . . . . . . . . : 192.168.10.5"),
        "{config}"
    );
    // The learned gateway works.
    exec(&mut lab, a, "ping 192.168.10.1");
    assert_eq!(replies(&exec(&mut lab, a, "ping 192.168.10.1")), 4);
    exec(&mut lab, a, &format!("ping {}", ips[1]));
    assert_eq!(replies(&exec(&mut lab, a, &format!("ping {}", ips[1]))), 4);

    let bindings = exec(&mut lab, r, "show ip dhcp binding");
    assert!(
        bindings.contains("192.168.10.11       0102.4e43.") && bindings.contains("Automatic"),
        "{bindings}"
    );
    let rows = lab.node_tables(r).unwrap().dhcp_bindings;
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|row| row.pool == "LAN"
            && row.expires_s > 11 * 3600
            && row.expires_s <= 12 * 3600),
        "{rows:?}"
    );
    let pool = exec(&mut lab, r, "show ip dhcp pool");
    assert!(
        pool.contains("Pool LAN :") && pool.contains("Leased addresses               : 2"),
        "{pool}"
    );

    // Releasing gives the address back; renewing gets (the same) one again.
    let released = exec(&mut lab, a, "ipconfig /release");
    assert!(released.contains("released"), "{released}");
    lab.advance(SECOND / 10);
    assert_eq!(lab.node_tables(r).unwrap().dhcp_bindings.len(), 1);
    lab.settle(60 * SECOND);
    assert_eq!(lab.node_tables(r).unwrap().dhcp_bindings.len(), 2);
    // Clearing the bindings on the server.
    exec(&mut lab, r, "clear ip dhcp binding *");
    assert!(lab.node_tables(r).unwrap().dhcp_bindings.is_empty());
    // A host switched back to static keeps working without the server.
    lab.set_host_config(
        b,
        host("192.168.10.50", "255.255.255.0", Some("192.168.10.1")),
    )
    .unwrap();
    lab.settle(30 * SECOND);
    exec(&mut lab, b, "ping 192.168.10.1");
    assert_eq!(replies(&exec(&mut lab, b, "ping 192.168.10.1")), 4);
}

#[test]
fn dhcp_through_a_relay_and_a_router_as_client() {
    let mut lab = Lab::new("relay");
    let server = node(&mut lab, "router", "DHCP");
    let relay = node(&mut lab, "router", "R1");
    let client_router = node(&mut lab, "router", "CPE");
    let sw = node(&mut lab, "switch", "SW1");
    let a = dhcp_host(&mut lab, "PC1");
    link(&mut lab, server, "g0/0", relay, "g0/1");
    link(&mut lab, relay, "g0/0", sw, "gi0/1");
    link(&mut lab, a, "Ethernet0", sw, "fa0/1");
    link(&mut lab, client_router, "g0/0", sw, "fa0/2");
    cfg(
        &mut lab,
        server,
        "interface g0/0\n ip address 10.0.0.2 255.255.255.252\n no shutdown\nip route 172.16.5.0 255.255.255.0 10.0.0.1\nip dhcp pool REMOTE\n network 172.16.5.0 /24\n default-router 172.16.5.1\n domain-name lab.example\n lease infinite",
    );
    cfg(
        &mut lab,
        relay,
        "interface g0/1\n ip address 10.0.0.1 255.255.255.252\n no shutdown\ninterface g0/0\n ip address 172.16.5.1 255.255.255.0\n ip helper-address 10.0.0.2\n no shutdown",
    );
    cfg(
        &mut lab,
        client_router,
        "interface g0/0\n ip address dhcp\n no shutdown",
    );
    lab.settle(120 * SECOND);

    let config = exec(&mut lab, a, "ipconfig /all");
    assert!(
        config.contains("IPv4 Address. . . . . . . . . . . : 172.16.5."),
        "{config}"
    );
    assert!(
        config.contains("Default Gateway . . . . . . . . . : 172.16.5.1"),
        "{config}"
    );
    assert!(
        config.contains("DHCP Server . . . . . . . . . . . : 10.0.0.2"),
        "{config}"
    );
    // Across the relay to the server itself.
    exec(&mut lab, a, "ping 10.0.0.2");
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.2")), 4);
    let bindings = exec(&mut lab, server, "show ip dhcp binding");
    assert_eq!(bindings.matches("Infinite").count(), 2, "{bindings}");

    // The router interface took an address and a default route from DHCP.
    let brief = exec(&mut lab, client_router, "show ip interface brief");
    assert!(
        brief.contains("YES DHCP   up                    up"),
        "{brief}"
    );
    let routes = exec(&mut lab, client_router, "show ip route");
    assert!(
        routes.contains("S*    0.0.0.0/0 [254/0] via 172.16.5.1"),
        "{routes}"
    );
    let logged = exec(&mut lab, client_router, "show logging");
    assert!(
        logged.contains(
            "%DHCP-6-ADDRESS_ASSIGN: Interface GigabitEthernet0/0 assigned DHCP address 172.16.5."
        ),
        "{logged}"
    );
    assert!(
        lab.running_config(client_router)
            .unwrap()
            .contains("interface GigabitEthernet0/0\n ip address dhcp\n")
    );
    let out = exec(&mut lab, client_router, "ping 10.0.0.2");
    assert!(ping_marks(&out).ends_with("!!!"), "{out}");

    // Without the helper address nobody answers and the client keeps trying.
    cfg(&mut lab, relay, "interface g0/0\n no ip helper-address");
    exec(&mut lab, a, "ipconfig /release");
    lab.settle(60 * SECOND);
    assert!(exec(&mut lab, a, "ipconfig").contains("(waiting for DHCP)"));
    cfg(
        &mut lab,
        relay,
        "interface g0/0\n ip helper-address 10.0.0.2",
    );
    lab.advance(40 * SECOND);
    assert!(exec(&mut lab, a, "ipconfig").contains(": 172.16.5."));
}

#[test]
fn a_server_with_the_dhcp_service_hands_out_addresses() {
    let mut lab = Lab::new("service");
    let sw = node(&mut lab, "switch", "SW1");
    let srv = node(&mut lab, "server", "SRV");
    lab.set_host_config(
        srv,
        HostConfig {
            ip: Some("10.10.0.5".into()),
            mask: Some("255.255.255.0".into()),
            gateway: Some("10.10.0.1".into()),
            services: vec!["dhcp".into(), "dns".into(), "http".into()],
            ..HostConfig::default()
        },
    )
    .unwrap();
    let a = dhcp_host(&mut lab, "PC1");
    link(&mut lab, srv, "Ethernet0", sw, "fa0/1");
    link(&mut lab, a, "Ethernet0", sw, "fa0/2");
    lab.settle(120 * SECOND);
    let config = exec(&mut lab, a, "ipconfig /all");
    assert!(
        config.contains("IPv4 Address. . . . . . . . . . . : 10.10.0.100"),
        "{config}"
    );
    assert!(
        config.contains("Default Gateway . . . . . . . . . : 10.10.0.1"),
        "{config}"
    );
    assert!(
        config.contains("DNS Servers . . . . . . . . . . . : 10.10.0.5"),
        "{config}"
    );
    // The DNS service knows the lab's node names.
    let lookup = exec(&mut lab, a, "nslookup SRV");
    assert!(
        lookup.contains("Name:    SRV\nAddress:  10.10.0.5"),
        "{lookup}"
    );
    assert!(exec(&mut lab, a, "nslookup nothing-here").contains("Non-existent domain"));
    let ping = exec(&mut lab, a, "ping SRV");
    assert!(
        ping.contains("Pinging SRV [10.10.0.5] with 32 bytes of data:") && replies(&ping) >= 3,
        "{ping}"
    );
    assert!(exec(&mut lab, srv, "netstat").contains("TCP    0.0.0.0:80"));
    assert!(lab.probe_tcp(a, "10.10.0.5".parse().unwrap(), 80).ok);
    assert!(!lab.probe_tcp(a, "10.10.0.5".parse().unwrap(), 22).ok);
}

#[test]
fn access_lists_filter_count_and_log() {
    let mut lab = Lab::new("acl");
    let r = node(&mut lab, "router", "R1");
    let a = pc(&mut lab, "PC1", "10.1.0.10", Some("10.1.0.1"));
    let b = pc(&mut lab, "PC2", "10.1.0.11", Some("10.1.0.1"));
    let sw = node(&mut lab, "switch", "SW1");
    let srv = node(&mut lab, "server", "SRV");
    lab.set_host_config(
        srv,
        HostConfig {
            ip: Some("10.2.0.10".into()),
            mask: Some("255.255.255.0".into()),
            gateway: Some("10.2.0.1".into()),
            services: vec!["http".into(), "ssh".into()],
            ..HostConfig::default()
        },
    )
    .unwrap();
    link(&mut lab, a, "Ethernet0", sw, "fa0/1");
    link(&mut lab, b, "Ethernet0", sw, "fa0/2");
    link(&mut lab, sw, "gi0/1", r, "g0/0");
    link(&mut lab, r, "g0/1", srv, "Ethernet0");
    cfg(
        &mut lab,
        r,
        "interface g0/0\n ip address 10.1.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.2.0.1 255.255.255.0\n no shutdown",
    );
    lab.settle(60 * SECOND);
    exec(&mut lab, a, "ping 10.2.0.10");
    exec(&mut lab, b, "ping 10.2.0.10");
    let web: std::net::Ipv4Addr = "10.2.0.10".parse().unwrap();

    // Standard list, outbound towards the server: PC2 is shut out, PC1 passes.
    cfg(
        &mut lab,
        r,
        "access-list 10 deny host 10.1.0.11\naccess-list 10 permit 10.1.0.0 0.0.0.255\ninterface g0/1\n ip access-group 10 out",
    );
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.2.0.10 -n 3")), 3);
    let denied = exec(&mut lab, b, "ping 10.2.0.10 -n 3");
    assert_eq!(replies(&denied), 0);
    assert_eq!(
        denied
            .matches("Reply from 10.1.0.1: Destination host unreachable.")
            .count(),
        3,
        "{denied}"
    );
    let shown = exec(&mut lab, r, "show access-lists");
    assert_eq!(
        shown,
        "Standard IP access list 10\n    10 deny   10.1.0.11 (3 matches)\n    20 permit 10.1.0.0, wildcard bits 0.0.0.255 (3 matches)\n"
    );
    let run = lab.running_config(r).unwrap();
    assert!(
        run.contains("access-list 10 deny 10.1.0.11\naccess-list 10 permit 10.1.0.0 0.0.0.255\n"),
        "{run}"
    );
    assert!(run.contains("interface GigabitEthernet0/1\n ip address 10.2.0.1 255.255.255.0\n ip access-group 10 out\n"), "{run}");
    exec(&mut lab, r, "clear access-list counters");
    assert!(!exec(&mut lab, r, "show access-lists").contains("matches"));
    cfg(&mut lab, r, "interface g0/1\n no ip access-group 10 out");
    assert_eq!(replies(&exec(&mut lab, b, "ping 10.2.0.10 -n 2")), 2);

    // Extended, named, inbound: web yes, ssh no, ping from PC2 no (logged), rest implicit deny.
    cfg(
        &mut lab,
        r,
        "ip access-list extended LAN-IN\n permit tcp 10.1.0.0 0.0.0.255 host 10.2.0.10 eq www\n deny tcp any any eq 22 log\n deny icmp host 10.1.0.11 any echo log\n permit icmp any any\ninterface g0/0\n ip access-group LAN-IN in",
    );
    lab.take_events();
    assert!(lab.probe_tcp(a, web, 80).ok);
    let ssh = lab.probe_tcp(a, web, 22);
    assert!(
        !ssh.ok
            && ssh.reason.contains("R1")
            && ssh.reason.contains("ACL LAN-IN")
            && ssh.reason.contains("line 20 `deny tcp any any eq 22 log`"),
        "{}",
        ssh.reason
    );
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.2.0.10 -n 2")), 2);
    assert_eq!(replies(&exec(&mut lab, b, "ping 10.2.0.10 -n 2")), 0);
    let logs = syslog(&mut lab);
    assert!(
        logs.iter().any(|l| l.contains(
            "%SEC-6-IPACCESSLOGDP: list LAN-IN denied icmp 10.1.0.11 -> 10.2.0.10 (8/0), 1 packet"
        )),
        "{logs:#?}"
    );
    // UDP matches nothing: implicit deny, reported as such.
    let trace = lab
        .start_trace(a, web, nocturne_netsim::PduKind::Udp { port: 53 })
        .unwrap();
    lab.advance(SECOND);
    let summary = lab.take_events().into_iter().find_map(|e| match e {
        LabEvent::TraceDone {
            trace: t,
            ok: false,
            summary,
        } if t == trace => Some(summary),
        _ => None,
    });
    assert!(
        summary
            .as_deref()
            .is_some_and(|s| s.contains("implicit deny")),
        "{summary:?}"
    );
    let shown = exec(&mut lab, r, "show ip access-lists LAN-IN");
    assert!(shown.starts_with("Extended IP access list LAN-IN\n    10 permit tcp 10.1.0.0 0.0.0.255 host 10.2.0.10 eq www"), "{shown}");
    assert!(shown.contains("    30 deny icmp host 10.1.0.11 any echo log (2 matches)\n    40 permit icmp any any (2 matches)\n"), "{shown}");

    // Editing by sequence number: insert before, delete by number.
    cfg(
        &mut lab,
        r,
        "ip access-list extended LAN-IN\n 25 permit icmp host 10.1.0.11 host 10.2.0.10\n no 20",
    );
    let shown = exec(&mut lab, r, "show ip access-lists LAN-IN");
    assert!(
        shown.contains("    25 permit icmp host 10.1.0.11 host 10.2.0.10\n    30 deny"),
        "{shown}"
    );
    assert!(!shown.contains("eq 22"), "{shown}");
    assert_eq!(replies(&exec(&mut lab, b, "ping 10.2.0.10 -n 2")), 2);

    // `established` lets answers back in but no new connections.
    cfg(
        &mut lab,
        r,
        "ip access-list extended FROM-SERVERS\n permit tcp any any established\n permit icmp any any\ninterface g0/1\n ip access-group FROM-SERVERS in",
    );
    assert!(
        lab.probe_tcp(a, web, 80).ok,
        "the SYN-ACK is an established segment"
    );
    assert!(
        !lab.probe_tcp(srv, "10.1.0.1".parse().unwrap(), 23).ok,
        "a new connection from the server side is refused"
    );

    // Type and syntax checks.
    let bad: Vec<String> = [
        "access-list 10 permit tcp any any",
        "access-list 101 permit 10.0.0.0",
        "access-list 3000 permit any",
        "ip access-list standard LAN-IN",
        "ip access-list extended X\n permit tcp any any eq notaport",
        "ip access-list extended X\n permit ip any any eq 80",
    ]
    .iter()
    .flat_map(|s| s.lines().map(str::to_string))
    .collect();
    let outcome = lab.configure(r, &bad).unwrap();
    let errors: Vec<&str> = outcome
        .lines
        .iter()
        .filter(|l| l.error)
        .map(|l| l.line.as_str())
        .collect();
    assert_eq!(
        errors,
        [
            "access-list 10 permit tcp any any",
            "access-list 101 permit 10.0.0.0",
            "access-list 3000 permit any",
            "ip access-list standard LAN-IN",
            " permit tcp any any eq notaport",
            " permit ip any any eq 80"
        ]
    );
    // A list applied to an interface but not defined permits everything.
    cfg(
        &mut lab,
        r,
        "interface g0/1\n ip access-group NOT-DEFINED in\ninterface g0/0\n no ip access-group LAN-IN in",
    );
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.2.0.10 -n 2")), 2);
}
