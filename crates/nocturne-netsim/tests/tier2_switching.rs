//! EtherChannel and port security.

mod common;

use common::*;
use nocturne_netsim::{Fault, Lab, LabEvent, Light};

/// Two switches joined by two parallel links, one host on each.
fn pair() -> (Lab, [u32; 2], [u32; 2], [u32; 2]) {
    let mut lab = Lab::new("channel");
    let s1 = node(&mut lab, "switch", "SW1");
    let s2 = node(&mut lab, "switch", "SW2");
    let a = pc(&mut lab, "PC1", "10.0.0.1", None);
    let b = pc(&mut lab, "PC2", "10.0.0.2", None);
    let l1 = link(&mut lab, s1, "gi0/1", s2, "gi0/1");
    let l2 = link(&mut lab, s1, "gi0/2", s2, "gi0/2");
    link(&mut lab, a, "Ethernet0", s1, "fa0/1");
    link(&mut lab, b, "Ethernet0", s2, "fa0/1");
    (lab, [s1, s2], [a, b], [l1, l2])
}

fn channel_row(lab: &mut Lab, node: u32) -> String {
    exec(lab, node, "show etherchannel summary")
        .lines()
        .find(|l| l.starts_with("1 "))
        .unwrap_or("")
        .to_string()
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

#[test]
fn lacp_bundle_forms_carries_traffic_and_survives_a_member_failure() {
    let (mut lab, [s1, s2], [a, _b], [l1, _l2]) = pair();
    lab.settle(60 * SECOND);
    // Without a channel, spanning tree has to block one of the parallel links.
    let stp = exec(&mut lab, s2, "show spanning-tree vlan 1");
    assert!(
        stp.contains("Root FWD 4") && stp.contains("Altn BLK 4"),
        "{stp}"
    );

    let out = lab
        .configure(
            s1,
            &[
                "interface range gi0/1 - 2".into(),
                " switchport mode trunk".into(),
                " channel-group 1 mode active".into(),
            ],
        )
        .unwrap();
    assert!(
        out.lines.iter().any(|l| l
            .output
            .contains("Creating a port-channel interface Port-channel 1")),
        "{out:?}"
    );
    cfg(
        &mut lab,
        s2,
        "interface range gi0/1 - 2\n switchport mode trunk\n channel-group 1 mode passive",
    );
    lab.settle(60 * SECOND);

    assert_eq!(
        channel_row(&mut lab, s1),
        "1      Po1(SU)         LACP      Gi0/1(P)    Gi0/2(P)"
    );
    assert_eq!(
        channel_row(&mut lab, s2),
        "1      Po1(SU)         LACP      Gi0/1(P)    Gi0/2(P)"
    );
    // Spanning tree now sees one logical port with the cost of the bundle.
    let stp = exec(&mut lab, s2, "show spanning-tree vlan 1");
    assert!(
        stp.contains("Po1                 Root FWD 3 ")
            && !stp.contains("Gi0/1")
            && !stp.contains("BLK"),
        "{stp}"
    );
    assert!(
        exec(&mut lab, s1, "show interfaces trunk")
            .contains("Po1         on               802.1q         trunking      1")
    );
    let status = exec(&mut lab, s1, "show interfaces status");
    assert!(
        status.contains("Po1                          connected    trunk      a-full a-1000"),
        "{status}"
    );
    assert!(exec(&mut lab, s1, "show interfaces port-channel 1").contains("BW 2000000 Kbit/sec"));

    exec(&mut lab, a, "ping 10.0.0.2");
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.2")), 4);
    let macs = exec(&mut lab, s1, "show mac address-table");
    assert!(
        macs.lines()
            .any(|l| l.contains("DYNAMIC") && l.trim_end().ends_with("Po1")),
        "{macs}"
    );
    let run = lab.running_config(s1).unwrap();
    assert!(
        run.contains("interface Port-channel1\n switchport mode trunk\n!"),
        "{run}"
    );
    assert!(
        run.contains(
            "interface GigabitEthernet0/1\n switchport mode trunk\n channel-group 1 mode active\n!"
        ),
        "{run}"
    );

    // One member fails: the channel stays up on the other, at a higher cost.
    lab.take_events();
    lab.inject(Fault::LinkDown { link: l1 }).unwrap();
    lab.settle(60 * SECOND);
    assert_eq!(
        channel_row(&mut lab, s1),
        "1      Po1(SU)         LACP      Gi0/1(D)    Gi0/2(P)"
    );
    assert!(
        exec(&mut lab, s2, "show spanning-tree vlan 1").contains("Po1                 Root FWD 4 ")
    );
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.2")), 4);
    let logs = syslog(&mut lab, s1);
    assert!(
        !logs
            .iter()
            .any(|l| l.contains("Port-channel1, changed state to down")),
        "{logs:#?}"
    );
}

#[test]
fn channels_need_compatible_modes_and_settings() {
    let (mut lab, [s1, s2], [a, _b], _) = pair();
    // Two passive ends wait for each other forever.
    cfg(
        &mut lab,
        s1,
        "interface range gi0/1 - 2\n switchport mode trunk\n channel-group 1 mode passive",
    );
    cfg(
        &mut lab,
        s2,
        "interface range gi0/1 - 2\n switchport mode trunk\n channel-group 1 mode passive",
    );
    lab.settle(60 * SECOND);
    assert_eq!(
        channel_row(&mut lab, s1),
        "1      Po1(SD)         LACP      Gi0/1(I)    Gi0/2(I)"
    );
    assert_eq!(
        replies(&exec(&mut lab, a, "ping 10.0.0.2 -n 2")),
        0,
        "ports outside the bundle do not forward"
    );
    // The two protocols do not mix on one channel.
    let outcome = lab
        .configure(
            s1,
            &[
                "interface gi0/1".into(),
                " channel-group 1 mode desirable".into(),
            ],
        )
        .unwrap();
    assert!(outcome.lines[1].error, "{outcome:?}");

    cfg(
        &mut lab,
        s1,
        "interface range gi0/1 - 2\n channel-group 1 mode active",
    );
    lab.settle(60 * SECOND);
    assert_eq!(
        channel_row(&mut lab, s1),
        "1      Po1(SU)         LACP      Gi0/1(P)    Gi0/2(P)"
    );
    exec(&mut lab, a, "ping 10.0.0.2");
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.2 -n 2")), 2);

    // A member edited on its own no longer matches the channel and is suspended...
    lab.take_events();
    cfg(&mut lab, s1, "interface gi0/2\n switchport mode access");
    lab.settle(60 * SECOND);
    assert_eq!(
        channel_row(&mut lab, s1),
        "1      Po1(SU)         LACP      Gi0/1(P)    Gi0/2(s)"
    );
    let logs = syslog(&mut lab, s1);
    assert!(
        logs.iter().any(|l| l.ends_with("%EC-5-CANNOT_BUNDLE2: Gi0/2 is not compatible with Po1 and will be suspended (trunk mode of Gi0/2 is access, Po1 is trunk)")),
        "{logs:#?}"
    );
    // ...while settings entered on the port-channel reach every member.
    cfg(
        &mut lab,
        s1,
        "interface port-channel 1\n switchport trunk native vlan 99\n switchport mode trunk",
    );
    cfg(
        &mut lab,
        s2,
        "interface port-channel 1\n switchport trunk native vlan 99",
    );
    lab.settle(60 * SECOND);
    assert_eq!(
        channel_row(&mut lab, s1),
        "1      Po1(SU)         LACP      Gi0/1(P)    Gi0/2(P)"
    );
    let run = lab.running_config(s1).unwrap();
    assert!(run.contains("interface GigabitEthernet0/2\n switchport trunk native vlan 99\n switchport mode trunk\n channel-group 1 mode active\n"), "{run}");

    // Static channels (mode on) do not negotiate at all.
    cfg(
        &mut lab,
        s1,
        "interface range gi0/1 - 2\n no channel-group\nno interface port-channel 1",
    );
    cfg(
        &mut lab,
        s2,
        "interface range gi0/1 - 2\n no channel-group\nno interface port-channel 1",
    );
    assert!(!lab.running_config(s1).unwrap().contains("Port-channel"));
    cfg(
        &mut lab,
        s1,
        "interface range gi0/1 - 2\n channel-group 2 mode on",
    );
    cfg(
        &mut lab,
        s2,
        "interface range gi0/1 - 2\n channel-group 2 mode on",
    );
    lab.settle(60 * SECOND);
    assert_eq!(
        exec(&mut lab, s1, "show etherchannel summary")
            .lines()
            .find(|l| l.starts_with("2 "))
            .unwrap_or(""),
        "2      Po2(SU)          -        Gi0/1(P)    Gi0/2(P)"
    );
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.2 -n 2")), 2);

    // Shutting the port-channel shuts its members.
    cfg(&mut lab, s1, "interface port-channel 2\n shutdown");
    assert!(lab.running_config(s1).unwrap().contains("interface GigabitEthernet0/1\n switchport trunk native vlan 99\n switchport mode trunk\n shutdown\n channel-group 2 mode on\n"), "{}", lab.running_config(s1).unwrap());
}

/// SW1 fa0/1 leads to an unmanaged downstream switch with three hosts; a
/// fourth host sits directly on SW1.
fn secured() -> (Lab, u32, [u32; 4]) {
    let mut lab = Lab::new("psec");
    let s1 = node(&mut lab, "switch", "SW1");
    let s2 = node(&mut lab, "switch", "SW2");
    let hosts = ["10.0.0.1", "10.0.0.2", "10.0.0.4", "10.0.0.3"];
    let ids: Vec<u32> = hosts
        .iter()
        .enumerate()
        .map(|(i, ip)| pc(&mut lab, &format!("PC{}", i + 1), ip, None))
        .collect();
    link(&mut lab, s1, "fa0/1", s2, "fa0/24");
    for (i, id) in ids.iter().take(3).enumerate() {
        link(&mut lab, *id, "Ethernet0", s2, &format!("fa0/{}", i + 1));
    }
    link(&mut lab, ids[3], "Ethernet0", s1, "fa0/3");
    lab.settle(60 * SECOND);
    (lab, s1, [ids[0], ids[1], ids[2], ids[3]])
}

#[test]
fn port_security_limits_restricts_and_shuts_down() {
    let (mut lab, s1, [a, b, c, _d]) = secured();
    // Port security needs a port that is statically access (or trunk).
    let out = lab
        .configure(
            s1,
            &["interface fa0/1".into(), " switchport port-security".into()],
        )
        .unwrap();
    assert_eq!(
        out.lines[1].output,
        "Command rejected: FastEthernet0/1 is a dynamic port.\n"
    );
    cfg(
        &mut lab,
        s1,
        "interface fa0/1\n switchport mode access\n switchport port-security\n switchport port-security maximum 2\n switchport port-security violation restrict",
    );
    lab.take_events();
    exec(&mut lab, a, "ping 10.0.0.3 -n 1");
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.3 -n 2")), 2);
    exec(&mut lab, b, "ping 10.0.0.3 -n 1");
    assert_eq!(replies(&exec(&mut lab, b, "ping 10.0.0.3 -n 2")), 2);
    // The third address behind the port is one too many.
    assert_eq!(replies(&exec(&mut lab, c, "ping 10.0.0.3 -n 2")), 0);
    let logs = syslog(&mut lab, s1);
    assert!(logs.iter().any(|l| l.contains("%PORT_SECURITY-2-PSECURE_VIOLATION: Security violation occurred, caused by MAC address ") && l.ends_with(" on port FastEthernet0/1.")), "{logs:#?}");
    let table = exec(&mut lab, s1, "show port-security");
    let row = table
        .lines()
        .find(|l| l.trim_start().starts_with("Fa0/1"))
        .unwrap_or("");
    let fields: Vec<&str> = row.split_whitespace().collect();
    assert_eq!(
        (fields[1], fields[2], fields[4]),
        ("2", "2", "Restrict"),
        "{table}"
    );
    assert!(fields[3].parse::<u32>().unwrap() >= 1, "{table}");
    let detail = exec(&mut lab, s1, "show port-security interface fa0/1");
    assert!(
        detail.contains(
            "Port Status                : Secure-up\nViolation Mode             : Restrict\n"
        ),
        "{detail}"
    );
    assert!(
        detail.contains("Maximum MAC Addresses      : 2\nTotal MAC Addresses        : 2\n"),
        "{detail}"
    );
    let addresses = exec(&mut lab, s1, "show port-security address");
    assert_eq!(
        addresses
            .matches("SecureDynamic                 Fa0/1")
            .count(),
        2,
        "{addresses}"
    );
    assert_eq!(
        exec(&mut lab, s1, "show mac address-table")
            .matches("STATIC      Fa0/1")
            .count(),
        2
    );
    // A restricted port stays up and keeps serving the addresses it knows.
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.3 -n 2")), 2);

    // Protect drops just as well, but silently.
    cfg(
        &mut lab,
        s1,
        "interface fa0/1\n switchport port-security violation protect",
    );
    let before = exec(&mut lab, s1, "show port-security interface fa0/1");
    lab.take_events();
    assert_eq!(replies(&exec(&mut lab, c, "ping 10.0.0.3 -n 2")), 0);
    assert!(syslog(&mut lab, s1).is_empty());
    let after = exec(&mut lab, s1, "show port-security interface fa0/1");
    assert_eq!(
        before.lines().last(),
        after.lines().last(),
        "the violation counter does not move"
    );

    // Sticky learning writes the addresses into the configuration.
    cfg(
        &mut lab,
        s1,
        "interface fa0/1\n switchport port-security violation shutdown\n switchport port-security mac-address sticky",
    );
    let run = exec(&mut lab, s1, "show running-config interface fa0/1");
    assert_eq!(
        run.matches(" switchport port-security mac-address sticky 0")
            .count(),
        2,
        "{run}"
    );
    assert!(run.contains(" switchport port-security maximum 2\n switchport port-security\n switchport port-security mac-address sticky\n"), "{run}");
    assert_eq!(
        exec(&mut lab, s1, "show port-security address")
            .matches("SecureSticky")
            .count(),
        2
    );

    // The default violation mode shuts the port down.
    lab.take_events();
    assert_eq!(replies(&exec(&mut lab, c, "ping 10.0.0.3 -n 2")), 0);
    let logs = syslog(&mut lab, s1);
    assert!(logs.iter().any(|l| l.ends_with("%PM-4-ERR_DISABLE: psecure-violation error detected on Fa0/1, putting Fa0/1 in err-disable state")), "{logs:#?}");
    assert!(
        logs.iter().any(|l| l.ends_with(
            "%LINEPROTO-5-UPDOWN: Line protocol on Interface FastEthernet0/1, changed state to down"
        )),
        "{logs:#?}"
    );
    assert!(
        exec(&mut lab, s1, "show interfaces status")
            .contains("Fa0/1                        err-disabled 1")
    );
    assert!(
        exec(&mut lab, s1, "show interfaces fa0/1")
            .starts_with("FastEthernet0/1 is down, line protocol is down (err-disabled)\n")
    );
    assert!(
        exec(&mut lab, s1, "show port-security interface fa0/1")
            .contains("Port Status                : Secure-shutdown\n")
    );
    assert_eq!(
        replies(&exec(&mut lab, a, "ping 10.0.0.3 -n 2")),
        0,
        "everything behind the port is cut off"
    );
    let snapshot = lab.snapshot();
    let port = snapshot
        .nodes
        .iter()
        .find(|n| n.id == s1)
        .and_then(|n| n.interfaces.iter().find(|i| i.name == "FastEthernet0/1"))
        .unwrap();
    assert!(!port.oper_up && port.light == Light::Red, "{port:?}");

    // Nothing recovers on its own; shutdown / no shutdown brings the port back.
    lab.advance(120 * SECOND);
    assert!(exec(&mut lab, s1, "show interfaces status").contains("err-disabled"));
    cfg(&mut lab, s1, "interface fa0/1\n shutdown\n no shutdown");
    lab.settle(60 * SECOND);
    exec(&mut lab, a, "ping 10.0.0.3 -n 1");
    assert_eq!(replies(&exec(&mut lab, a, "ping 10.0.0.3 -n 2")), 2);

    // Sticky addresses survive a save and reload; dynamic ones would not.
    exec(&mut lab, s1, "write memory");
    lab.set_power(s1, false);
    lab.set_power(s1, true);
    lab.settle(60 * SECOND);
    assert_eq!(
        exec(&mut lab, s1, "show port-security address")
            .matches("SecureSticky")
            .count(),
        2
    );
    exec(&mut lab, s1, "clear port-security sticky");
    assert_eq!(
        exec(&mut lab, s1, "show port-security address")
            .matches("SecureSticky")
            .count(),
        0
    );
    cfg(
        &mut lab,
        s1,
        "interface fa0/1\n no switchport port-security",
    );
    assert!(
        exec(&mut lab, s1, "show port-security")
            .lines()
            .all(|l| !l.contains("Fa0/1"))
    );
}
