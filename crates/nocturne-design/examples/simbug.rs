//! Minimal reproductions of the two simulator limitations the design works around.

use nocturne_netsim::{CableKind, Endpoint, HostConfig, Lab, LabSpec, LinkSpec, NodeSpec};

fn main() {
    // --- 1. an access point's radio cannot be cabled -----------------------
    let mut lab = Lab::new("ap");
    let ap = lab.add_node("ap", Some("AP1"), 0.0, 0.0).unwrap();
    let sw = lab.add_node("switch", Some("SW1"), 0.0, 0.0).unwrap();
    println!("free_interfaces(AP1) = {:?}", lab.free_interfaces(ap));
    println!(
        "add_link(AP1 Dot11Radio0 -> SW1 Fa0/1) = {:?}",
        lab.add_link(ap, "Dot11Radio0", sw, "FastEthernet0/1", CableKind::Auto)
    );

    // --- 2. a flooded frame that dies on one branch fails the whole probe --
    let switch = "\
hostname SW1
vlan 10
 name USERS
vlan 20
 name VOICE
interface FastEthernet0/1
 description PC on a port with a voice VLAN
 switchport mode access
 switchport access vlan 10
 switchport voice vlan 20
 spanning-tree portfast
interface FastEthernet0/2
 description Telephone
 switchport mode access
 switchport access vlan 20
 spanning-tree portfast
interface Vlan20
 ip address 10.0.20.1 255.255.255.0
 no shutdown
";
    let host = |name: &str, model: &str, ip: &str| NodeSpec {
        name: name.into(),
        model: model.into(),
        host: Some(HostConfig {
            ip: Some(ip.into()),
            mask: Some("255.255.255.0".into()),
            gateway: Some("10.0.20.1".into()),
            ..HostConfig::default()
        }),
        ..NodeSpec::default()
    };
    let link = |a: &str, ai: &str, b: &str, bi: &str| LinkSpec {
        a: Endpoint {
            node: a.into(),
            interface: ai.into(),
        },
        b: Endpoint {
            node: b.into(),
            interface: bi.into(),
        },
        cable: CableKind::Auto,
    };
    let spec = LabSpec {
        name: "voice".into(),
        nodes: vec![
            NodeSpec {
                name: "SW1".into(),
                model: "switch".into(),
                startup_config: Some(switch.into()),
                ..NodeSpec::default()
            },
            host("PC1", "pc", "10.0.10.5"),
            host("PH1", "laptop", "10.0.20.5"),
        ],
        links: vec![
            link("PC1", "Ethernet0", "SW1", "FastEthernet0/1"),
            link("PH1", "Ethernet0", "SW1", "FastEthernet0/2"),
        ],
        ..LabSpec::default()
    };
    let mut lab = Lab::from_spec(&spec).unwrap();
    lab.settle(60_000_000);
    let ph1 = lab.node_by_name("PH1").unwrap();
    let probe = lab.probe_ping(ph1, "10.0.20.1".parse().unwrap());
    println!("probe_ping(PH1 -> 10.0.20.1).ok = {}", probe.ok);
    println!("  reason: {}", probe.reason);
    // The same thing typed on the console works, because the second packet
    // goes out after ARP has resolved.
    let out = lab.exec(ph1, "ping 10.0.20.1").unwrap_or_default();
    println!(
        "console ping replies: {}",
        out.matches("Reply from").count()
    );
    println!("{out}");
}
