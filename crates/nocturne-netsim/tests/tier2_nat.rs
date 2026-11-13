//! NAT towards the Internet cloud.

mod common;

use common::*;
use nocturne_netsim::{HostConfig, Lab};

/// Two hosts and a web server behind R1; the cloud answers for 8.8.8.8 and
/// has no route back to the private network.
fn edge() -> (Lab, u32, u32, [u32; 3]) {
    let mut lab = Lab::new("nat");
    let r = node(&mut lab, "router", "R1");
    let sw = node(&mut lab, "switch", "SW1");
    let a = pc(&mut lab, "PC1", "192.168.1.10", Some("192.168.1.1"));
    let b = pc(&mut lab, "PC2", "192.168.1.11", Some("192.168.1.1"));
    let web = node(&mut lab, "server", "WEB");
    lab.set_host_config(
        web,
        HostConfig {
            ip: Some("192.168.1.20".into()),
            mask: Some("255.255.255.0".into()),
            gateway: Some("192.168.1.1".into()),
            services: vec!["http".into()],
            ..HostConfig::default()
        },
    )
    .unwrap();
    let cloud = node(&mut lab, "cloud", "Internet");
    lab.set_host_config(
        cloud,
        HostConfig {
            ip: Some("203.0.113.1".into()),
            mask: Some("255.255.255.248".into()),
            extra_ips: vec!["8.8.8.8/32".into()],
            services: vec!["http".into(), "dns".into()],
            ..HostConfig::default()
        },
    )
    .unwrap();
    link(&mut lab, a, "Ethernet0", sw, "fa0/1");
    link(&mut lab, b, "Ethernet0", sw, "fa0/2");
    link(&mut lab, web, "Ethernet0", sw, "fa0/3");
    link(&mut lab, sw, "gi0/1", r, "g0/0");
    link(&mut lab, r, "g0/1", cloud, "Ethernet0");
    cfg(
        &mut lab,
        r,
        "interface g0/0\n ip address 192.168.1.1 255.255.255.0\n ip nat inside\n no shutdown\ninterface g0/1\n ip address 203.0.113.2 255.255.255.248\n ip nat outside\n no shutdown\nip route 0.0.0.0 0.0.0.0 203.0.113.1\naccess-list 1 permit 192.168.1.0 0.0.0.255\nip nat inside source list 1 interface GigabitEthernet0/1 overload",
    );
    lab.settle(60 * SECOND);
    (lab, r, cloud, [a, b, web])
}

#[test]
fn overload_lets_private_hosts_reach_the_internet() {
    let (mut lab, r, cloud, [a, b, _web]) = edge();
    exec(&mut lab, a, "ping 8.8.8.8");
    let out = exec(&mut lab, a, "ping 8.8.8.8");
    assert_eq!(replies(&out), 4, "{out}");
    assert!(
        out.contains("Reply from 8.8.8.8: bytes=32 time<1ms TTL=63"),
        "{out}"
    );
    exec(&mut lab, b, "ping 8.8.8.8");
    assert_eq!(replies(&exec(&mut lab, b, "ping 8.8.8.8")), 4);

    let table = exec(&mut lab, r, "show ip nat translations");
    assert!(
        table.starts_with(
            "Pro Inside global      Inside local       Outside local      Outside global\n"
        ),
        "{table}"
    );
    assert!(
        table.contains("icmp 203.0.113.2:")
            && table.contains("192.168.1.10:")
            && table.contains("8.8.8.8:"),
        "{table}"
    );
    let rows = lab.node_tables(r).unwrap().nat;
    assert!(
        rows.iter()
            .all(|row| row.protocol == "icmp" && row.inside_global.starts_with("203.0.113.2:")),
        "{rows:?}"
    );
    // Both hosts use the same echo identifiers, so the router had to pick
    // different ones on the outside for the second host.
    let globals: std::collections::BTreeSet<&str> =
        rows.iter().map(|row| row.inside_global.as_str()).collect();
    assert_eq!(
        globals.len(),
        rows.len(),
        "every translation has its own global port: {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row.inside_local.starts_with("192.168.1.11:")
                && row.inside_global != row.inside_local.replace("192.168.1.11", "203.0.113.2")),
        "{rows:?}"
    );
    let stats = exec(&mut lab, r, "show ip nat statistics");
    assert!(
        stats.contains(
            "Outside interfaces:\n  GigabitEthernet0/1\nInside interfaces:\n  GigabitEthernet0/0\n"
        ),
        "{stats}"
    );
    assert!(
        stats.contains("[Id: 1] access-list 1 interface GigabitEthernet0/1"),
        "{stats}"
    );
    assert!(
        exec(&mut lab, r, "show access-lists")
            .contains("permit 192.168.1.0, wildcard bits 0.0.0.255 (")
    );

    // TCP and traceroute work through the translation as well.
    assert!(lab.probe_tcp(a, "8.8.8.8".parse().unwrap(), 80).ok);
    let tracert = exec(&mut lab, a, "tracert 8.8.8.8");
    assert!(
        tracert.contains("192.168.1.1")
            && tracert.contains("8.8.8.8")
            && tracert.contains("Trace complete."),
        "{tracert}"
    );
    // The cloud never sees a private address: it has no route to one.
    let direct = lab.probe_ping(cloud, "192.168.1.10".parse().unwrap());
    assert!(!direct.ok, "{direct:?}");

    // Idle ICMP translations expire after a minute; clearing empties the table.
    lab.advance(61 * SECOND);
    assert_eq!(exec(&mut lab, r, "show ip nat translations"), "");
    exec(&mut lab, a, "ping 8.8.8.8 -n 1");
    assert!(!lab.node_tables(r).unwrap().nat.is_empty());
    exec(&mut lab, r, "clear ip nat translation *");
    assert!(lab.node_tables(r).unwrap().nat.is_empty());

    // Without `ip nat inside` packets leave untranslated and the answers never return.
    cfg(&mut lab, r, "interface g0/0\n no ip nat inside");
    assert_eq!(replies(&exec(&mut lab, a, "ping 8.8.8.8 -n 2")), 0);
    let probe = lab.probe_ping(a, "8.8.8.8".parse().unwrap());
    assert!(
        !probe.ok && probe.reason.contains("Internet") && probe.reason.contains("no route"),
        "{}",
        probe.reason
    );
}

#[test]
fn static_translations_and_pools() {
    let (mut lab, r, cloud, [a, b, _web]) = edge();
    // One-to-one: the cloud can reach PC1 at a public address on the outside subnet.
    cfg(
        &mut lab,
        r,
        "ip nat inside source static 192.168.1.10 203.0.113.5",
    );
    let probe = lab.probe_ping(cloud, "203.0.113.5".parse().unwrap());
    assert!(probe.ok, "{probe:?}");
    assert_eq!(probe.hops.last().map(|h| h.name.as_str()), Some("PC1"));
    assert!(
        probe.hops.iter().any(|h| h
            .note
            .contains("static rule, destination 203.0.113.5 becomes 192.168.1.10")),
        "{:?}",
        probe.hops
    );
    // And PC1 now leaves with that address (the trace explains it).
    let out = lab.probe_ping(a, "8.8.8.8".parse().unwrap());
    assert!(
        out.ok
            && out
                .hops
                .iter()
                .any(|h| h.note.contains("source 192.168.1.10 becomes 203.0.113.5")),
        "{out:?}"
    );

    // Port forward: the web server behind the router answers on the outside address.
    cfg(
        &mut lab,
        r,
        "ip nat inside source static tcp 192.168.1.20 80 203.0.113.2 8080",
    );
    assert!(
        lab.probe_tcp(cloud, "203.0.113.2".parse().unwrap(), 8080)
            .ok
    );
    assert!(!lab.probe_tcp(cloud, "203.0.113.2".parse().unwrap(), 80).ok);
    let table = exec(&mut lab, r, "show ip nat translations");
    assert!(
        table.contains("--- 203.0.113.5        192.168.1.10       ---                ---"),
        "{table}"
    );
    assert!(
        table.contains("tcp 203.0.113.2:8080   192.168.1.20:80    ---                ---"),
        "{table}"
    );
    let run = lab.running_config(r).unwrap();
    assert!(
        run.contains("ip nat inside source list 1 interface GigabitEthernet0/1 overload\nip nat inside source static 192.168.1.10 203.0.113.5\nip nat inside source static tcp 192.168.1.20 80 203.0.113.2 8080\nip route 0.0.0.0 0.0.0.0 203.0.113.1\n"),
        "{run}"
    );

    // A pool without overload gives each inside host its own address until it runs out.
    cfg(
        &mut lab,
        r,
        "no ip nat inside source static 192.168.1.10 203.0.113.5\nno ip nat inside source list 1\nip nat pool PUBLIC 203.0.113.4 203.0.113.4 netmask 255.255.255.248\nip nat inside source list 1 pool PUBLIC",
    );
    exec(&mut lab, a, "ping 8.8.8.8");
    assert_eq!(replies(&exec(&mut lab, a, "ping 8.8.8.8 -n 2")), 2);
    let denied = exec(&mut lab, b, "ping 8.8.8.8 -n 2");
    assert_eq!(replies(&denied), 0, "{denied}");
    assert!(denied.contains("Destination host unreachable"), "{denied}");
    let table = exec(&mut lab, r, "show ip nat translations");
    assert!(
        table.contains("--- 203.0.113.4        192.168.1.10       ---                ---"),
        "{table}"
    );
    // A pool that is in use cannot be removed; an unknown pool cannot be used.
    let outcome = lab
        .configure(
            r,
            &[
                "no ip nat pool PUBLIC".to_string(),
                "ip nat inside source list 1 pool NOPE".to_string(),
            ],
        )
        .unwrap();
    assert!(outcome.lines.iter().all(|l| l.error), "{outcome:?}");
}
