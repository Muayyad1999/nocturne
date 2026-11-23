//! The lab API: persistence, power, flows, faults, tracing, determinism.

mod common;

use common::*;
use nocturne_netsim::{
    CableKind, Fault, FlowPattern, FlowSpec, HostConfig, Lab, LabError, LabEvent, LabSpec,
    PacketEventKind, PduKind, catalog,
};

/// PC1 - SW1 - R1 - R2 - PC2, fully configured and settled.
fn small_network() -> (Lab, [u32; 5], [u32; 4]) {
    let mut lab = Lab::new("small");
    let h1 = pc(&mut lab, "PC1", "10.1.0.10", Some("10.1.0.1"));
    let sw = node(&mut lab, "switch", "SW1");
    let r1 = node(&mut lab, "router", "R1");
    let r2 = node(&mut lab, "router", "R2");
    let h2 = pc(&mut lab, "PC2", "10.2.0.10", Some("10.2.0.1"));
    let l1 = link(&mut lab, h1, "Ethernet0", sw, "fa0/1");
    let l2 = link(&mut lab, sw, "gi0/1", r1, "g0/0");
    let l3 = link(&mut lab, r1, "g0/1", r2, "g0/1");
    let l4 = link(&mut lab, r2, "g0/0", h2, "Ethernet0");
    cfg(
        &mut lab,
        r1,
        "interface g0/0\n ip address 10.1.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.0.0.1 255.255.255.252\n no shutdown\nip route 10.2.0.0 255.255.255.0 10.0.0.2",
    );
    cfg(
        &mut lab,
        r2,
        "interface g0/0\n ip address 10.2.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.0.0.2 255.255.255.252\n no shutdown\nip route 10.1.0.0 255.255.255.0 10.0.0.1",
    );
    lab.settle(60 * SECOND);
    (lab, [h1, sw, r1, r2, h2], [l1, l2, l3, l4])
}

#[test]
fn power_cycle_keeps_only_the_startup_configuration() {
    let (mut lab, [h1, sw, r1, _, _], _) = small_network();
    assert_eq!(lab.startup_config(r1).unwrap(), "");
    // Saved configuration survives; later, unsaved changes do not.
    let saved = exec(&mut lab, r1, "write memory");
    assert_eq!(saved, "Building configuration...\n[OK]\n");
    assert_eq!(
        lab.startup_config(r1).unwrap(),
        lab.running_config(r1).unwrap()
    );
    cfg(
        &mut lab,
        r1,
        "hostname Changed\ninterface loopback 0\n ip address 1.1.1.1 255.255.255.255",
    );
    assert!(lab.running_config(r1).unwrap().contains("hostname Changed"));
    lab.set_power(r1, false);
    assert!(
        !lab.snapshot()
            .nodes
            .iter()
            .find(|n| n.id == r1)
            .unwrap()
            .powered
    );
    assert!(matches!(
        lab.exec(r1, "show version"),
        Err(LabError::PoweredOff(_))
    ));
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.1.0.1 -n 2")), 0);
    lab.set_power(r1, true);
    lab.settle(60 * SECOND);
    let run = lab.running_config(r1).unwrap();
    assert!(
        run.contains("hostname R1") && !run.contains("Loopback0"),
        "{run}"
    );
    assert_eq!(run, lab.startup_config(r1).unwrap());
    exec(&mut lab, h1, "ping 10.2.0.10");
    assert_eq!(replies(&exec(&mut lab, h1, "ping 10.2.0.10")), 4);
    // Uptime restarted and dynamic state was relearned from scratch.
    assert!(lab.metrics(r1).unwrap().uptime_s < 120);

    // A device that was never saved comes back with factory defaults...
    cfg(&mut lab, sw, "hostname Fancy\nvlan 77\n name Lab");
    lab.set_power(sw, false);
    lab.set_power(sw, true);
    let run = lab.running_config(sw).unwrap();
    assert!(run.contains("hostname SW1"), "{run}");
    // ...except for its VLAN database, which lives in flash:vlan.dat.
    assert!(exec(&mut lab, sw, "show vlan brief").contains("77   Lab"));
    assert!(exec(&mut lab, sw, "dir").contains("vlan.dat"));

    // erase startup-config + reload: back to factory settings.
    exec(&mut lab, r1, "erase startup-config");
    assert_eq!(lab.startup_config(r1).unwrap(), "");
    exec(&mut lab, r1, "reload");
    let run = lab.running_config(r1).unwrap();
    assert!(
        run.contains(
            "interface GigabitEthernet0/0\n no ip address\n shutdown\n duplex auto\n speed auto\n"
        ),
        "{run}"
    );
    assert!(exec(&mut lab, r1, "show startup-config").contains("startup-config is not present"));
}

#[test]
fn spec_round_trip_preserves_the_lab() {
    let (mut lab, [_, sw, r1, r2, _], _) = small_network();
    cfg(
        &mut lab,
        sw,
        "vlan 10\n name Staff\ninterface fa0/2\n switchport mode access\n switchport access vlan 10",
    );
    for device in [sw, r1, r2] {
        exec(&mut lab, device, "write memory");
    }
    lab.add_flow(FlowSpec {
        name: "web".into(),
        src: "PC1".into(),
        dst_ip: "10.2.0.10".into(),
        bps: 2_000_000,
        ..FlowSpec::default()
    })
    .unwrap();
    lab.set_notes(vec![nocturne_netsim::NoteSpec {
        x: 1.0,
        y: 2.0,
        w: 30.0,
        h: 10.0,
        text: "core".into(),
        color: "#fff".into(),
    }]);
    lab.move_node(r1, 120.0, 80.0);
    let spec = lab.to_spec();
    assert_eq!(spec.nodes.len(), 5);
    assert_eq!(spec.links.len(), 4);
    assert_eq!(spec.flows.len(), 1);
    let r1_spec = spec.nodes.iter().find(|n| n.name == "R1").unwrap();
    assert_eq!(
        (r1_spec.x, r1_spec.y, r1_spec.model.as_str()),
        (120.0, 80.0, "router")
    );
    assert!(
        r1_spec
            .startup_config
            .as_ref()
            .unwrap()
            .contains("ip route 10.2.0.0 255.255.255.0 10.0.0.2")
    );
    let sw_spec = spec.nodes.iter().find(|n| n.name == "SW1").unwrap();
    assert_eq!(sw_spec.vlan_dat.as_deref(), Some("vlan 10\n name Staff\n"));
    assert!(
        spec.nodes
            .iter()
            .find(|n| n.name == "PC1")
            .unwrap()
            .host
            .as_ref()
            .unwrap()
            .ip
            .is_some()
    );

    // Through JSON and back: the same lab.
    let json = serde_json::to_string_pretty(&spec).unwrap();
    assert!(json.contains("\"cable\": \"auto\""), "{json}");
    let parsed: LabSpec = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, spec);
    let mut copy = Lab::from_spec(&parsed).unwrap();
    copy.settle(60 * SECOND);
    assert_eq!(copy.to_spec(), spec);
    for name in ["SW1", "R1", "R2"] {
        let (a, b) = (
            lab.node_by_name(name).unwrap(),
            copy.node_by_name(name).unwrap(),
        );
        assert_eq!(
            lab.running_config(a).unwrap(),
            copy.running_config(b).unwrap(),
            "{name}"
        );
    }
    let h1 = copy.node_by_name("PC1").unwrap();
    exec(&mut copy, h1, "ping 10.2.0.10");
    assert_eq!(replies(&exec(&mut copy, h1, "ping 10.2.0.10")), 4);
    assert_eq!(copy.snapshot().notes.len(), 1);

    // A minimal hand-written spec fills in defaults.
    let minimal: LabSpec = serde_json::from_str(
        r#"{"name":"tiny","nodes":[{"name":"A","model":"pc","host":{"ip":"10.0.0.1","mask":"255.255.255.0"}},
            {"name":"B","model":"pc","host":{"ip":"10.0.0.2"}}],
            "links":[{"a":{"node":"A","interface":"Ethernet0"},"b":{"node":"B","interface":"e0"}}]}"#,
    )
    .unwrap();
    assert!(minimal.nodes[0].powered);
    let mut tiny = Lab::from_spec(&minimal).unwrap();
    let a = tiny.node_by_name("A").unwrap();
    exec(&mut tiny, a, "ping 10.0.0.2 -n 1");
    assert_eq!(replies(&exec(&mut tiny, a, "ping 10.0.0.2 -n 2")), 2);

    // Bad specs are refused with a reason.
    let mut bad = spec.clone();
    bad.nodes[0].model = "toaster".into();
    assert_eq!(
        Lab::from_spec(&bad).err(),
        Some(LabError::UnknownModel("toaster".into()))
    );
    let mut bad = spec.clone();
    bad.links[0].a.node = "Nobody".into();
    assert_eq!(
        Lab::from_spec(&bad).err(),
        Some(LabError::UnknownNodeName("Nobody".into()))
    );
    let mut bad = spec;
    bad.nodes[1].name = "PC1".into();
    assert_eq!(
        Lab::from_spec(&bad).err(),
        Some(LabError::DuplicateName("PC1".into()))
    );
}

#[test]
fn editing_api_reports_errors() {
    let mut lab = Lab::new("edit");
    assert_eq!(lab.name(), "edit");
    let r = lab.add_node("router", None, 0.0, 0.0).unwrap();
    let s = lab.add_node("switch", None, 0.0, 0.0).unwrap();
    let names: Vec<String> = lab
        .snapshot()
        .nodes
        .iter()
        .map(|n| n.name.clone())
        .collect();
    assert_eq!(names, ["R1", "SW1"]);
    assert_eq!(
        lab.add_node("router", Some("R1"), 0.0, 0.0),
        Err(LabError::DuplicateName("R1".into()))
    );
    assert_eq!(
        lab.add_node("router", Some("bad name"), 0.0, 0.0),
        Err(LabError::InvalidName("bad name".into()))
    );
    assert_eq!(
        lab.add_node("nope", None, 0.0, 0.0),
        Err(LabError::UnknownModel("nope".into()))
    );
    assert_eq!(lab.free_interfaces(r).len(), 4);
    let l = lab
        .add_link(r, "g0/0", s, "FastEthernet0/1", CableKind::Copper)
        .unwrap();
    assert_eq!(
        lab.free_interfaces(r),
        [
            "GigabitEthernet0/1",
            "GigabitEthernet0/2",
            "GigabitEthernet0/3"
        ]
    );
    assert!(matches!(
        lab.add_link(r, "g0/0", s, "fa0/2", CableKind::Auto),
        Err(LabError::InterfaceInUse { .. })
    ));
    assert!(matches!(
        lab.add_link(r, "g0/9", s, "fa0/2", CableKind::Auto),
        Err(LabError::UnknownInterface { .. })
    ));
    assert!(matches!(
        lab.add_link(r, "vlan1", s, "fa0/2", CableKind::Auto),
        Err(LabError::UnknownInterface { .. })
    ));
    assert_eq!(
        lab.add_link(r, "g0/1", r, "g0/2", CableKind::Auto),
        Err(LabError::SelfLink)
    );
    assert_eq!(lab.remove_link(999), Err(LabError::UnknownLink(999)));
    lab.rename_node(r, "Edge").unwrap();
    assert_eq!(lab.node_by_name("Edge"), Some(r));
    assert_eq!(
        lab.rename_node(s, "Edge"),
        Err(LabError::DuplicateName("Edge".into()))
    );
    assert!(matches!(
        lab.set_host_config(r, HostConfig::default()),
        Err(LabError::NotAHost(_))
    ));
    assert_eq!(lab.host_config(r), None);
    let printer = lab.add_node("printer", None, 0.0, 0.0).unwrap();
    assert!(matches!(lab.console_open(printer), Err(LabError::NoCli(_))));
    assert!(matches!(
        lab.exec(printer, "ping 1.1.1.1"),
        Err(LabError::NoCli(_))
    ));
    assert!(matches!(
        lab.set_host_config(printer, host("10.0.0.300", "255.255.255.0", None)),
        Err(LabError::InvalidAddress(_))
    ));
    // A console cable needs the dedicated ports.
    let pc1 = lab.add_node("pc", None, 0.0, 0.0).unwrap();
    assert!(
        lab.add_link(pc1, "Ethernet0", r, "g0/3", CableKind::Console)
            .is_err()
    );
    let console_cable = lab
        .add_link(pc1, "RS232", r, "Console", CableKind::Console)
        .unwrap();
    assert!(
        lab.snapshot()
            .links
            .iter()
            .any(|v| v.id == console_cable && v.cable == CableKind::Console && v.up)
    );

    lab.remove_node(r).unwrap();
    assert_eq!(
        lab.snapshot().links.len(),
        0,
        "links of a removed node go with it"
    );
    assert_eq!(lab.remove_link(l), Err(LabError::UnknownLink(l)));
    assert_eq!(lab.remove_node(r), Err(LabError::UnknownNode(r)));
    assert_eq!(lab.node_by_name("Edge"), None);
    // Ids are never reused.
    let again = lab.add_node("router", Some("Edge"), 0.0, 0.0).unwrap();
    assert!(again > pc1);
    assert_eq!(catalog().len(), 13);
}

#[test]
fn flows_move_counters_and_stop_when_the_path_is_cut() {
    let (mut lab, [h1, sw, r1, r2, _], [_, _, core, _]) = small_network();
    let flow = lab
        .add_flow(FlowSpec {
            name: "backup".into(),
            src: "PC1".into(),
            dst_ip: "10.2.0.10".into(),
            bps: 8_000_000,
            ..FlowSpec::default()
        })
        .unwrap();
    let octets = |lab: &Lab, node: u32, iface: &str| {
        let m = lab.metrics(node).unwrap();
        let i = m
            .interfaces
            .iter()
            .find(|i| i.name == iface)
            .unwrap()
            .clone();
        (i.in_octets, i.out_octets, i.in_discards)
    };
    let before = octets(&lab, r1, "GigabitEthernet0/1");
    lab.advance(10 * SECOND);
    let after = octets(&lab, r1, "GigabitEthernet0/1");
    // 8 Mbit/s for ten seconds is ten megabytes out, plus a tenth coming back.
    let sent = after.1 - before.1;
    assert!((9_000_000..=10_100_000).contains(&sent), "{sent}");
    let returned = after.0 - before.0;
    assert!((900_000..=1_010_000).contains(&returned), "{returned}");
    // Every device on the path counts it, switches included.
    assert!(octets(&lab, sw, "FastEthernet0/1").0 > 9_000_000);
    assert!(octets(&lab, r2, "GigabitEthernet0/0").1 > 9_000_000);
    let (_, spec, state) = lab.flows().into_iter().next().unwrap();
    assert_eq!(spec.name, "backup");
    assert!(
        state.active && state.current_bps == 8_000_000 && state.delivered_octets > 9_000_000,
        "{state:?}"
    );
    assert_eq!(state.path, ["PC1", "SW1", "R1", "R2", "PC2"]);
    let view = lab
        .snapshot()
        .links
        .into_iter()
        .find(|l| l.id == core)
        .unwrap();
    assert_eq!((view.bps_ab, view.bps_ba), (8_000_000, 800_000));
    let shown = exec(&mut lab, r1, "show interfaces g0/1");
    let rate: u64 = shown
        .lines()
        .find(|l| l.contains("5 minute output rate"))
        .unwrap()
        .split_whitespace()
        .nth(4)
        .unwrap()
        .parse()
        .unwrap();
    assert!(rate > 100_000, "the smoothed rate is rising: {shown}");

    // Cut the core link: counters beyond the cut stop, the last interface reached discards.
    let fault = lab.inject(Fault::LinkDown { link: core }).unwrap();
    lab.advance(3 * SECOND);
    let frozen = octets(&lab, r2, "GigabitEthernet0/0");
    let at_r1 = octets(&lab, r1, "GigabitEthernet0/0");
    lab.advance(10 * SECOND);
    assert_eq!(octets(&lab, r2, "GigabitEthernet0/0"), frozen);
    let now_r1 = octets(&lab, r1, "GigabitEthernet0/0");
    assert!(
        now_r1.0 > at_r1.0 + 9_000_000 && now_r1.2 > at_r1.2,
        "{at_r1:?} -> {now_r1:?}"
    );
    let state = lab.flows().remove(0).2;
    assert!(!state.active && state.reason.contains("R1"), "{state:?}");
    assert_eq!(
        lab.snapshot()
            .links
            .into_iter()
            .find(|l| l.id == core)
            .unwrap()
            .bps_ab,
        0
    );
    lab.clear_fault(fault);
    lab.settle(30 * SECOND);
    lab.advance(3 * SECOND);
    assert!(lab.flows()[0].2.active);

    // Patterns and removal.
    lab.remove_flow(flow);
    assert!(lab.flows().is_empty());
    let burst = lab
        .add_flow(FlowSpec {
            name: "burst".into(),
            src: "PC1".into(),
            dst_ip: "10.2.0.10".into(),
            bps: 1_000_000,
            pattern: FlowPattern::Burst { on_s: 2, off_s: 8 },
            ..FlowSpec::default()
        })
        .unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..12 {
        lab.advance(SECOND);
        seen.insert(lab.flows()[0].2.current_bps);
    }
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), [0, 5_000_000]);
    lab.remove_flow(burst);
    assert!(matches!(
        lab.add_flow(FlowSpec {
            src: "Nobody".into(),
            dst_ip: "1.1.1.1".into(),
            ..FlowSpec::default()
        }),
        Err(LabError::UnknownNodeName(_))
    ));
    assert!(matches!(
        lab.add_flow(FlowSpec {
            src: "PC1".into(),
            dst_ip: "not-an-ip".into(),
            ..FlowSpec::default()
        }),
        Err(LabError::InvalidAddress(_))
    ));
    let _ = h1;
}

#[test]
fn every_fault_has_its_effect_and_can_be_cleared() {
    let (mut lab, [h1, sw, r1, r2, _], [access, _, core, _]) = small_network();
    exec(&mut lab, h1, "ping 10.2.0.10");
    let reach = |lab: &mut Lab| replies(&exec(lab, h1, "ping 10.2.0.10 -n 2"));
    assert_eq!(reach(&mut lab), 2);
    lab.take_events();

    // LinkDown.
    let f = lab.inject(Fault::LinkDown { link: core }).unwrap();
    assert!(
        !lab.snapshot()
            .links
            .iter()
            .find(|l| l.id == core)
            .unwrap()
            .up
    );
    assert!(
        lab.take_events()
            .iter()
            .any(|e| matches!(e, LabEvent::LinkState { link, up: false } if *link == core))
    );
    assert_eq!(reach(&mut lab), 0);
    assert_eq!(lab.faults(), vec![(f, Fault::LinkDown { link: core })]);
    lab.clear_fault(f);
    lab.settle(30 * SECOND);
    assert!(lab.faults().is_empty());
    exec(&mut lab, h1, "ping 10.2.0.10");
    assert_eq!(reach(&mut lab), 2);

    // PowerOff / clearing powers the device back on.
    let f = lab.inject(Fault::PowerOff { node: r2 }).unwrap();
    assert!(
        !lab.snapshot()
            .nodes
            .iter()
            .find(|n| n.id == r2)
            .unwrap()
            .powered
    );
    lab.clear_fault(f);
    assert!(
        lab.snapshot()
            .nodes
            .iter()
            .find(|n| n.id == r2)
            .unwrap()
            .powered
    );
    // R2 was never saved: it lost its configuration, as a real router would.
    assert!(!lab.running_config(r2).unwrap().contains("10.2.0.1"));
    cfg(
        &mut lab,
        r2,
        "interface g0/0\n ip address 10.2.0.1 255.255.255.0\n no shutdown\ninterface g0/1\n ip address 10.0.0.2 255.255.255.252\n no shutdown\nip route 10.1.0.0 255.255.255.0 10.0.0.1",
    );
    lab.settle(30 * SECOND);
    exec(&mut lab, h1, "ping 10.2.0.10");
    assert_eq!(reach(&mut lab), 2);

    // PacketLoss: 100 % loses everything; 40 % loses some of many.
    let f = lab
        .inject(Fault::PacketLoss {
            link: core,
            percent: 100,
        })
        .unwrap();
    assert_eq!(reach(&mut lab), 0);
    lab.clear_fault(f);
    let f = lab
        .inject(Fault::PacketLoss {
            link: core,
            percent: 40,
        })
        .unwrap();
    let got = replies(&exec(&mut lab, h1, "ping 10.2.0.10 -n 40"));
    assert!(
        (3..=30).contains(&got),
        "{got} of 40 with 40% loss each way"
    );
    lab.clear_fault(f);
    assert_eq!(reach(&mut lab), 2);

    // InterfaceErrors: CRC errors accumulate at the configured rate.
    let f = lab
        .inject(Fault::InterfaceErrors {
            node: r1,
            interface: "g0/1".into(),
            crc_per_s: 5,
        })
        .unwrap();
    lab.advance(10 * SECOND);
    let crc = lab
        .metrics(r1)
        .unwrap()
        .interfaces
        .iter()
        .find(|i| i.name == "GigabitEthernet0/1")
        .unwrap()
        .crc;
    assert_eq!(crc, 50);
    assert!(exec(&mut lab, r1, "show interfaces g0/1").contains("50 input errors, 50 CRC"));
    lab.clear_fault(f);
    lab.advance(10 * SECOND);
    assert_eq!(
        lab.metrics(r1)
            .unwrap()
            .interfaces
            .iter()
            .find(|i| i.name == "GigabitEthernet0/1")
            .unwrap()
            .crc,
        50
    );
    assert!(matches!(
        lab.inject(Fault::InterfaceErrors {
            node: r1,
            interface: "g7/7".into(),
            crc_per_s: 1
        }),
        Err(LabError::UnknownInterface { .. })
    ));

    // HighCpu.
    let idle = lab.metrics(r1).unwrap().cpu_pct;
    let f = lab
        .inject(Fault::HighCpu {
            node: r1,
            percent: 90,
        })
        .unwrap();
    let busy = lab.metrics(r1).unwrap().cpu_pct;
    assert!(idle < 10.0 && busy >= 90.0, "{idle} -> {busy}");
    assert!(
        exec(&mut lab, r1, "show processes cpu").starts_with("CPU utilization for five seconds: 9")
    );
    lab.clear_fault(f);
    assert!(lab.metrics(r1).unwrap().cpu_pct < 10.0);

    // Flapping: the link goes down and up with the period, logging each time.
    lab.take_events();
    let f = lab
        .inject(Fault::Flapping {
            link: access,
            period_s: 4,
        })
        .unwrap();
    lab.advance(9 * SECOND);
    let changes: Vec<bool> = lab
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            LabEvent::LinkState { link, up } if link == access => Some(up),
            _ => None,
        })
        .collect();
    assert_eq!(changes, [false, true, false, true, false], "{changes:?}");
    lab.clear_fault(f);
    lab.settle(30 * SECOND);
    assert!(
        lab.snapshot()
            .links
            .iter()
            .find(|l| l.id == access)
            .unwrap()
            .up
    );
    let log = exec(&mut lab, sw, "show logging");
    assert!(
        log.matches("%LINK-3-UPDOWN: Interface FastEthernet0/1, changed state to down")
            .count()
            >= 3,
        "{log}"
    );

    // DuplexMismatch: errors appear on both ends under load.
    let f = lab.inject(Fault::DuplexMismatch { link: core }).unwrap();
    lab.add_flow(FlowSpec {
        name: "load".into(),
        src: "PC1".into(),
        dst_ip: "10.2.0.10".into(),
        bps: 50_000_000,
        ..FlowSpec::default()
    })
    .unwrap();
    lab.advance(10 * SECOND);
    let errors = |lab: &Lab, node: u32| {
        let m = lab.metrics(node).unwrap();
        let i = m
            .interfaces
            .iter()
            .find(|i| i.name == "GigabitEthernet0/1")
            .unwrap()
            .clone();
        (i.in_errors, i.out_errors)
    };
    let (a, b) = (errors(&lab, r1), errors(&lab, r2));
    assert!(a.0 + a.1 > 100 && b.0 + b.1 > 100, "{a:?} {b:?}");
    let shown = exec(&mut lab, r2, "show interfaces g0/1");
    assert!(
        shown.contains("Half-duplex")
            || exec(&mut lab, r1, "show interfaces g0/1").contains("Half-duplex"),
        "{shown}"
    );
    lab.clear_fault(f);
    assert!(lab.inject(Fault::LinkDown { link: 4242 }).is_err());
    assert!(
        lab.inject(Fault::HighCpu {
            node: 4242,
            percent: 1
        })
        .is_err()
    );
}

#[test]
fn traces_explain_every_hop() {
    let (mut lab, [h1, _sw, r1, _, h2], _) = small_network();
    lab.take_events();
    // First trace: ARP is cold, so the PDU dies at the host, and says why.
    let t1 = lab
        .start_trace(h1, "10.2.0.10".parse().unwrap(), PduKind::Icmp)
        .unwrap();
    lab.advance(SECOND);
    let events = lab.take_events();
    let done = events.iter().find_map(|e| match e {
        LabEvent::TraceDone { trace, ok, summary } if *trace == t1 => Some((*ok, summary.clone())),
        _ => None,
    });
    let (ok, summary) = done.expect("trace finished");
    assert!(
        !ok && summary.contains("PC1") && summary.contains("ARP"),
        "{summary}"
    );
    let packets: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            LabEvent::Packet(p) if p.trace == Some(t1) => Some(p.clone()),
            _ => None,
        })
        .collect();
    // The ARP exchange it triggered is part of the trace.
    assert!(
        packets
            .iter()
            .any(|p| p.protocol == "ARP" && p.summary.contains("who has 10.1.0.1")),
        "{packets:#?}"
    );
    assert!(packets.iter().any(|p| p.protocol == "ARP"
        && p.kind == PacketEventKind::Delivered
        && p.decision.contains("replies")));
    let flood = packets
        .iter()
        .find(|p| p.kind == PacketEventKind::Forwarded && p.decision.contains("broadcast"))
        .expect("flood");
    assert!(
        flood
            .decision
            .contains("flooded out of every other port in VLAN 1"),
        "{}",
        flood.decision
    );

    // Warm the path, then trace again: success, hop by hop.
    exec(&mut lab, h1, "ping 10.2.0.10 -n 6");
    lab.take_events();
    let t2 = lab
        .start_trace(h1, "10.2.0.10".parse().unwrap(), PduKind::Icmp)
        .unwrap();
    lab.advance(SECOND);
    let events = lab.take_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, LabEvent::TraceDone { trace, ok: true, .. } if *trace == t2)),
        "{events:#?}"
    );
    let packets: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            LabEvent::Packet(p) if p.trace == Some(t2) => Some(p.clone()),
            _ => None,
        })
        .collect();
    let request: Vec<_> = packets
        .iter()
        .filter(|p| p.summary == "ICMP echo request 10.1.0.10 → 10.2.0.10")
        .collect();
    assert_eq!(request.first().unwrap().kind, PacketEventKind::Created);
    assert_eq!(request.last().unwrap().kind, PacketEventKind::Delivered);
    assert_eq!(request.last().unwrap().at, h2);
    let kinds: Vec<_> = request.iter().map(|p| p.kind).collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == PacketEventKind::Received)
            .count(),
        4,
        "four cables: {kinds:?}"
    );
    // Sequence numbers increase and events chain to their cause.
    assert!(packets.windows(2).all(|w| w[0].seq < w[1].seq));
    assert!(request.iter().skip(1).all(|p| p.parent != 0));
    let routed = request
        .iter()
        .find(|p| p.at == r1 && p.kind == PacketEventKind::Forwarded)
        .unwrap();
    assert!(
        routed.decision.contains("routing table")
            && routed.decision.contains("S 10.2.0.0/24")
            && routed.decision.contains("TTL to 127"),
        "{}",
        routed.decision
    );
    assert_eq!(routed.interface, "GigabitEthernet0/1");
    let titles: Vec<_> = routed
        .layers
        .iter()
        .map(|l| (l.layer, l.title.as_str()))
        .collect();
    assert_eq!(titles, [(2, "Ethernet II"), (3, "IPv4"), (3, "ICMP")]);
    assert!(
        routed.layers[1].lines.contains(&"TTL: 127".to_string()),
        "{:?}",
        routed.layers[1]
    );
    let switched = request
        .iter()
        .find(|p| p.kind == PacketEventKind::Forwarded && p.decision.contains("MAC table"))
        .unwrap();
    assert!(
        switched.decision.contains("found on Gi0/1"),
        "{}",
        switched.decision
    );
    let received = request
        .iter()
        .find(|p| p.kind == PacketEventKind::Received)
        .unwrap();
    assert!(received.from.is_some() && received.link.is_some());
    // The reply is part of the same trace.
    assert!(
        packets
            .iter()
            .any(|p| p.summary == "ICMP echo reply 10.2.0.10 → 10.1.0.10"
                && p.kind == PacketEventKind::Delivered
                && p.at == h1)
    );

    // Capture: everything on the wire is recorded, also control traffic.
    lab.set_capture(true);
    lab.advance(3 * SECOND);
    let captured = lab.take_events();
    assert!(
        captured
            .iter()
            .any(|e| matches!(e, LabEvent::Packet(p) if p.protocol == "STP" && p.trace.is_none()))
    );
    lab.set_capture(false);
    lab.advance(3 * SECOND);
    assert!(
        lab.take_events()
            .iter()
            .all(|e| !matches!(e, LabEvent::Packet(_)))
    );

    // UDP and TCP traces.
    let t3 = lab
        .start_trace(
            h1,
            "10.2.0.10".parse().unwrap(),
            PduKind::Udp { port: 9999 },
        )
        .unwrap();
    let t4 = lab
        .start_trace(h1, "10.0.0.2".parse().unwrap(), PduKind::Tcp { port: 23 })
        .unwrap();
    let t5 = lab
        .start_trace(h1, "10.0.0.2".parse().unwrap(), PduKind::Tcp { port: 8080 })
        .unwrap();
    lab.advance(SECOND);
    let done: Vec<(u32, bool, String)> = lab
        .take_events()
        .into_iter()
        .filter_map(|e| match e {
            LabEvent::TraceDone { trace, ok, summary } => Some((trace, ok, summary)),
            _ => None,
        })
        .collect();
    assert!(done.iter().any(|(t, ok, _)| *t == t3 && *ok), "{done:?}");
    assert!(
        done.iter()
            .any(|(t, ok, s)| *t == t4 && *ok && s.contains("succeeded")),
        "{done:?}"
    );
    assert!(
        done.iter()
            .any(|(t, ok, s)| *t == t5 && !*ok && s.contains("RST")),
        "{done:?}"
    );
    assert!(
        lab.start_trace(9999, "10.0.0.2".parse().unwrap(), PduKind::Icmp)
            .is_err()
    );
}

#[test]
fn identical_inputs_give_identical_outputs() {
    let run = || {
        let (mut lab, [h1, sw, r1, _, _], [_, _, core, _]) = small_network();
        let console = lab.console_open(sw).unwrap();
        let mut transcript = String::new();
        for line in [
            "enable",
            "terminal length 0",
            "show mac address-table",
            "show spanning-tree",
            "show version",
        ] {
            transcript.push_str(&typed(&mut lab, console, line));
        }
        lab.set_capture(true);
        transcript.push_str(&exec(&mut lab, h1, "ping 10.2.0.10"));
        let fault = lab
            .inject(Fault::PacketLoss {
                link: core,
                percent: 30,
            })
            .unwrap();
        transcript.push_str(&exec(&mut lab, h1, "ping 10.2.0.10 -n 20"));
        lab.clear_fault(fault);
        transcript.push_str(&exec(&mut lab, r1, "show interfaces"));
        transcript.push_str(&exec(&mut lab, r1, "show running-config"));
        cfg(
            &mut lab,
            r1,
            "enable secret determinism\nusername u secret p",
        );
        transcript.push_str(&lab.running_config(r1).unwrap());
        // Traffic through a policy that drops: the QoS counters are part of
        // what has to come out the same.
        cfg(
            &mut lab,
            r1,
            "class-map match-any VOICE\n match dscp ef\npolicy-map QUEUE\n class VOICE\n  priority 256\npolicy-map WAN\n class class-default\n  shape average 3000000\n  service-policy QUEUE\ninterface g0/1\n service-policy output WAN",
        );
        for (name, bps, dscp, pattern) in [
            ("voice", 400_000, Some(46), FlowPattern::Constant),
            ("bulk", 4_000_000, None, FlowPattern::Sine { period_s: 13 }),
        ] {
            lab.add_flow(FlowSpec {
                name: name.into(),
                src: "PC1".into(),
                dst_ip: "10.2.0.10".into(),
                bps,
                pattern,
                dscp,
                ..FlowSpec::default()
            })
            .unwrap();
        }
        lab.advance(30 * SECOND);
        let policy = exec(&mut lab, r1, "show policy-map interface");
        assert!(
            !policy.contains("b/w exceed drops: 0") && !policy.contains("64/0/0"),
            "the policy has to be dropping for this to mean anything: {policy}"
        );
        transcript.push_str(&policy);
        let events = serde_json::to_string(&lab.take_events()).unwrap();
        let snapshot = serde_json::to_string(&lab.snapshot()).unwrap();
        let metrics = serde_json::to_string(&lab.metrics(r1).unwrap()).unwrap();
        (transcript, events, snapshot, metrics)
    };
    let (a, b) = (run(), run());
    assert_eq!(a.0, b.0);
    assert_eq!(a.1, b.1);
    assert_eq!(a.2, b.2);
    assert_eq!(a.3, b.3);
    assert!(a.1.len() > 10_000, "the event log is substantial");
}

#[test]
fn host_shell() {
    let (mut lab, [h1, ..], _) = small_network();
    let c = lab.console_open(h1).unwrap();
    assert_eq!(output(&mut lab, c), "\r\nC:\\>");
    let out = typed(&mut lab, c, "ipconfig");
    assert!(out.contains("Ethernet adapter Ethernet0:"), "{out}");
    assert!(
        out.contains("   IPv4 Address. . . . . . . . . . . : 10.1.0.10\r\n"),
        "{out}"
    );
    assert!(out.contains("   Subnet Mask . . . . . . . . . . . : 255.255.255.0\r\n"));
    assert!(out.contains("   Default Gateway . . . . . . . . . : 10.1.0.1\r\n"));
    let all = typed(&mut lab, c, "ipconfig /all");
    assert!(
        all.contains("Physical Address. . . . . . . . . : 02-4E-43-")
            && all.contains("DHCP Enabled. . . . . . . . . . . : No"),
        "{all}"
    );
    assert_eq!(typed(&mut lab, c, "hostname"), "hostname\r\nPC1\r\nC:\\>");
    assert!(typed(&mut lab, c, "arp -a").contains("No ARP Entries Found."));
    lab.console_input(c, b"ping 10.1.0.1\r");
    lab.advance(10 * SECOND);
    let out = output(&mut lab, c);
    assert!(out.contains("Pinging 10.1.0.1 with 32 bytes of data:\r\nRequest timed out.\r\nReply from 10.1.0.1: bytes=32 time<1ms TTL=255\r\n"), "{out:?}");
    assert!(
        out.contains("    Packets: Sent = 4, Received = 3, Lost = 1 (25% loss),\r\n")
            && out.ends_with("C:\\>"),
        "{out:?}"
    );
    let arp = typed(&mut lab, c, "arp -a");
    assert!(
        arp.contains("Interface: 10.1.0.10 --- 0x2")
            && arp.contains("  10.1.0.1              02-4e-43-"),
        "{arp}"
    );
    // `?` is an ordinary character here, and unknown commands say so.
    let out = typed(&mut lab, c, "what?");
    assert!(
        out.contains("'what?' is not recognized as an internal or external command"),
        "{out}"
    );
    assert!(typed(&mut lab, c, "help").contains("tracert <address>"));
    // Quick static re-addressing from the shell is reflected in the host config.
    let out = typed(&mut lab, c, "ip 10.1.0.77 255.255.255.0 10.1.0.1");
    assert!(out.contains("Static IP configuration applied."), "{out}");
    assert_eq!(
        lab.host_config(h1).unwrap().ip.as_deref(),
        Some("10.1.0.77")
    );
    assert!(typed(&mut lab, c, "ipconfig").contains(": 10.1.0.77\r\n"));
    assert!(typed(&mut lab, c, "ip 10.1.0.77 255.0.255.0").contains("Invalid subnet mask."));
    // No gateway and an off-subnet target: unreachable at once, every time.
    typed(&mut lab, c, "ip 10.1.0.77 255.255.255.0");
    lab.console_input(c, b"ping 8.8.8.8 -n 2\r");
    lab.advance(10 * SECOND);
    let out = output(&mut lab, c);
    assert_eq!(
        out.matches("Destination host unreachable.").count(),
        2,
        "{out}"
    );
    // Ctrl-C stops an endless ping.
    lab.console_input(c, b"ping -t 10.1.0.1\r");
    lab.advance(5 * SECOND);
    lab.console_input(c, b"\x03");
    let out = output(&mut lab, c);
    assert!(
        out.contains("Ping statistics for 10.1.0.1:") && out.ends_with("C:\\>"),
        "{out}"
    );
}
