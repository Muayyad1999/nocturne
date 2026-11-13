//! Ready-made labs: the campus reference network and five small teaching labs.
//!
//! Everything here is plain data (`LabSpec`): device configurations are the
//! same text a person would type, so each lab doubles as a worked example.

use crate::api::{
    CableKind, Endpoint, FlowPattern, FlowSpec, HostConfig, LabSpec, LinkSpec, NodeSpec, NoteSpec,
};

fn device(name: &str, model: &str, x: f32, y: f32, config: &str) -> NodeSpec {
    NodeSpec {
        name: name.to_string(),
        model: model.to_string(),
        x,
        y,
        startup_config: Some(config.to_string()),
        ..NodeSpec::default()
    }
}

fn end_host(name: &str, model: &str, x: f32, y: f32, host: HostConfig) -> NodeSpec {
    NodeSpec {
        name: name.to_string(),
        model: model.to_string(),
        x,
        y,
        host: Some(host),
        ..NodeSpec::default()
    }
}

fn dhcp_client() -> HostConfig {
    HostConfig {
        dhcp: true,
        ..HostConfig::default()
    }
}

fn fixed(ip: &str, mask: &str, gateway: Option<&str>) -> HostConfig {
    HostConfig {
        ip: Some(ip.to_string()),
        mask: Some(mask.to_string()),
        gateway: gateway.map(str::to_string),
        ..HostConfig::default()
    }
}

fn link(a: &str, a_if: &str, b: &str, b_if: &str) -> LinkSpec {
    let end = |node: &str, interface: &str| Endpoint {
        node: node.to_string(),
        interface: interface.to_string(),
    };
    LinkSpec {
        a: end(a, a_if),
        b: end(b, b_if),
        cable: CableKind::Auto,
    }
}

fn note(x: f32, y: f32, w: f32, h: f32, text: &str, color: &str) -> NoteSpec {
    NoteSpec {
        x,
        y,
        w,
        h,
        text: text.to_string(),
        color: color.to_string(),
    }
}

fn flow(name: &str, src: &str, dst_ip: &str, bps: u64, pattern: FlowPattern) -> FlowSpec {
    FlowSpec {
        name: name.to_string(),
        src: src.to_string(),
        dst_ip: dst_ip.to_string(),
        bps,
        pattern,
        enabled: true,
    }
}

/// The Internet as the labs see it: one upstream address and a well-known
/// public host that answers pings, DNS and HTTP.
fn internet(x: f32, y: f32) -> NodeSpec {
    end_host(
        "Internet",
        "cloud",
        x,
        y,
        HostConfig {
            extra_ips: vec!["8.8.8.8/32".to_string()],
            services: vec!["dns".to_string(), "http".to_string()],
            ..fixed("203.0.113.1", "255.255.255.252", None)
        },
    )
}

/// What every managed device in the campus has in common.
const MANAGEMENT: &str = "\
ip domain-name campus.lab
username admin privilege 15 secret nocturne
snmp-server community public RO
snmp-server location Campus building A
logging host 10.10.99.100
line vty 0 4
 login local
 transport input ssh
line vty 5 15
 login local
 transport input ssh
";

const CAMPUS_VLANS: &str = "\
vlan 10
 name Staff
vlan 20
 name Guest
vlan 30
 name Voice
vlan 99
 name Management
";

const EDGE: &str = "\
hostname EDGE
interface Loopback0
 description Management
 ip address 10.255.0.1 255.255.255.255
interface GigabitEthernet0/0
 description Internet uplink
 ip address 203.0.113.2 255.255.255.252
 ip nat outside
 no shutdown
interface GigabitEthernet0/1
 description To DS1
 ip address 10.0.0.1 255.255.255.252
 ip nat inside
 no shutdown
interface GigabitEthernet0/2
 description To DS2
 ip address 10.0.0.5 255.255.255.252
 ip nat inside
 no shutdown
router ospf 1
 router-id 10.255.0.1
 network 10.0.0.0 0.0.0.7 area 0
 network 10.255.0.1 0.0.0.0 area 0
 default-information originate
ip nat inside source list 1 interface GigabitEthernet0/0 overload
ip route 0.0.0.0 0.0.0.0 203.0.113.1
access-list 1 permit 10.0.0.0 0.255.255.255
";

/// One distribution switch. `n` is 1 or 2; DS1 is the primary gateway and
/// spanning-tree root for Staff and Management, DS2 for Guest and Voice.
fn distribution(n: u8) -> String {
    let host = n + 1;
    let primary = |mine: bool| {
        if mine {
            "\n standby {g} priority 110\n standby {g} preempt"
        } else {
            ""
        }
    };
    let svi = |vlan: u16, name: &str, mine: bool, extra: &str| {
        let hsrp = primary(mine).replace("{g}", &vlan.to_string());
        format!(
            "interface Vlan{vlan}\n description {name}\n ip address 10.10.{vlan}.{host} 255.255.255.0{extra}\n standby {vlan} ip 10.10.{vlan}.1{hsrp}\n no shutdown\n"
        )
    };
    let first = n == 1;
    let (own_root, other_root) = if first {
        ("1,10,99", "20,30")
    } else {
        ("20,30", "1,10,99")
    };
    // DS1 hands out addresses; DS2 relays to it in case it is the one that hears a client.
    let dhcp = if first {
        "\
ip dhcp excluded-address 10.10.10.1 10.10.10.99
ip dhcp excluded-address 10.10.20.1 10.10.20.99
ip dhcp excluded-address 10.10.30.1 10.10.30.99
ip dhcp pool STAFF
 network 10.10.10.0 255.255.255.0
 default-router 10.10.10.1
 dns-server 8.8.8.8
 domain-name campus.lab
ip dhcp pool GUEST
 network 10.10.20.0 255.255.255.0
 default-router 10.10.20.1
 dns-server 8.8.8.8
 lease 0 8
ip dhcp pool VOICE
 network 10.10.30.0 255.255.255.0
 default-router 10.10.30.1
 dns-server 8.8.8.8
"
    } else {
        ""
    };
    let helper = if first {
        ""
    } else {
        "\n ip helper-address 10.255.0.2"
    };
    let guest_acl = "\n ip access-group GUEST-IN in";
    let edge_port = if first {
        "To EDGE Gi0/1"
    } else {
        "To EDGE Gi0/2"
    };
    let (edge_ip, server_port) = if first {
        (
            "10.0.0.2",
            "interface GigabitEthernet1/0/24\n description NMS\n switchport mode access\n switchport access vlan 99\n spanning-tree portfast\n",
        )
    } else {
        (
            "10.0.0.6",
            "interface GigabitEthernet1/0/24\n description SRV1\n switchport mode access\n switchport access vlan 10\n spanning-tree portfast\n",
        )
    };
    let mut cfg = format!(
        "hostname DS{n}\nip routing\n{CAMPUS_VLANS}spanning-tree vlan {own_root} root primary\nspanning-tree vlan {other_root} root secondary\n{dhcp}"
    );
    cfg.push_str(&format!("interface Loopback0\n description Management\n ip address 10.255.0.{host} 255.255.255.255\n"));
    cfg.push_str("interface Port-channel1\n description Inter-switch trunk\n switchport trunk encapsulation dot1q\n switchport mode trunk\n");
    cfg.push_str(&format!("interface GigabitEthernet1/0/1\n description {edge_port}\n no switchport\n ip address {edge_ip} 255.255.255.252\n no shutdown\n"));
    for access in 1..=3 {
        cfg.push_str(&format!(
            "interface GigabitEthernet1/0/1{access}\n description AS{access} uplink\n switchport trunk encapsulation dot1q\n switchport mode trunk\n"
        ));
    }
    cfg.push_str(server_port);
    for port in 1..=2 {
        cfg.push_str(&format!(
            "interface TenGigabitEthernet1/1/{port}\n description Inter-switch trunk\n switchport trunk encapsulation dot1q\n switchport mode trunk\n channel-group 1 mode {}\n",
            if first { "active" } else { "passive" }
        ));
    }
    cfg.push_str(&svi(10, "Staff", first, helper));
    cfg.push_str(&svi(20, "Guest", !first, &format!("{helper}{guest_acl}")));
    cfg.push_str(&svi(30, "Voice", !first, helper));
    cfg.push_str(&svi(99, "Management", first, ""));
    cfg.push_str(&format!(
        "router ospf 1\n router-id 10.255.0.{host}\n passive-interface default\n no passive-interface GigabitEthernet1/0/1\n no passive-interface Vlan99\n network 10.0.0.0 0.0.0.7 area 0\n network 10.10.0.0 0.0.255.255 area 0\n network 10.255.0.{host} 0.0.0.0 area 0\n"
    ));
    // Guests reach the Internet, not the staff or management networks.
    cfg.push_str(
        "ip access-list extended GUEST-IN\n permit icmp any any echo-reply\n deny ip 10.10.20.0 0.0.0.255 10.10.10.0 0.0.0.255\n deny ip 10.10.20.0 0.0.0.255 10.10.99.0 0.0.0.255\n permit ip any any\n",
    );
    cfg.push_str(MANAGEMENT);
    cfg
}

/// One access switch: two trunk uplinks and host ports in the given VLANs.
fn access(n: u8, ports: &[(u8, u16, &str)]) -> String {
    let mut cfg = format!("hostname AS{n}\n{CAMPUS_VLANS}");
    for (port, vlan, who) in ports {
        cfg.push_str(&format!(
            "interface FastEthernet0/{port}\n description {who}\n switchport mode access\n switchport access vlan {vlan}\n spanning-tree portfast\n spanning-tree bpduguard enable\n"
        ));
    }
    for (port, peer) in [(1, "DS1"), (2, "DS2")] {
        cfg.push_str(&format!("interface GigabitEthernet0/{port}\n description Uplink to {peer}\n switchport mode trunk\n"));
    }
    cfg.push_str(&format!(
        "interface Vlan99\n description Management\n ip address 10.10.99.1{n} 255.255.255.0\n no shutdown\nip default-gateway 10.10.99.1\n"
    ));
    cfg.push_str(MANAGEMENT);
    cfg
}

/// The reference campus network: an Internet edge with NAT, two layer-3
/// distribution switches (OSPF, HSRP, DHCP), three access switches with
/// redundant uplinks, user VLANs, a server and a management station.
pub fn demo_campus() -> LabSpec {
    let mut nodes = vec![
        internet(620.0, 40.0),
        device(
            "EDGE",
            "router",
            620.0,
            170.0,
            &format!("{EDGE}{MANAGEMENT}"),
        ),
        device("DS1", "switch-l3", 420.0, 320.0, &distribution(1)),
        device("DS2", "switch-l3", 820.0, 320.0, &distribution(2)),
        device(
            "AS1",
            "switch",
            220.0,
            480.0,
            &access(1, &[(1, 10, "Staff-PC1"), (2, 10, "Staff-PC2")]),
        ),
        device(
            "AS2",
            "switch",
            620.0,
            480.0,
            &access(2, &[(1, 10, "Staff-Laptop"), (2, 20, "Guest-Laptop1")]),
        ),
        device(
            "AS3",
            "switch",
            1020.0,
            480.0,
            &access(
                3,
                &[
                    (1, 20, "Guest-Laptop2"),
                    (2, 30, "Phone1"),
                    (3, 10, "Printer1"),
                ],
            ),
        ),
        end_host(
            "NMS",
            "nms",
            220.0,
            320.0,
            HostConfig {
                dns: Some("8.8.8.8".to_string()),
                ..fixed("10.10.99.100", "255.255.255.0", Some("10.10.99.1"))
            },
        ),
        end_host(
            "SRV1",
            "server",
            1020.0,
            320.0,
            HostConfig {
                dns: Some("8.8.8.8".to_string()),
                services: ["http", "ssh", "tftp", "syslog", "ntp"]
                    .map(String::from)
                    .to_vec(),
                ..fixed("10.10.10.10", "255.255.255.0", Some("10.10.10.1"))
            },
        ),
        end_host("Staff-PC1", "pc", 140.0, 620.0, dhcp_client()),
        end_host("Staff-PC2", "pc", 300.0, 620.0, dhcp_client()),
        end_host("Staff-Laptop", "laptop", 540.0, 620.0, dhcp_client()),
        end_host("Guest-Laptop1", "laptop", 700.0, 620.0, dhcp_client()),
        end_host("Guest-Laptop2", "laptop", 920.0, 620.0, dhcp_client()),
        end_host("Phone1", "phone", 1020.0, 620.0, dhcp_client()),
        end_host(
            "Printer1",
            "printer",
            1120.0,
            620.0,
            fixed("10.10.10.50", "255.255.255.0", Some("10.10.10.1")),
        ),
    ];
    // Network devices can be reached over SSH: they come with a host key.
    for node in nodes.iter_mut().filter(|n| n.startup_config.is_some()) {
        node.rsa_key_bits = Some(2048);
    }
    let mut links = vec![
        link("Internet", "Ethernet0", "EDGE", "GigabitEthernet0/0"),
        link("EDGE", "GigabitEthernet0/1", "DS1", "GigabitEthernet1/0/1"),
        link("EDGE", "GigabitEthernet0/2", "DS2", "GigabitEthernet1/0/1"),
        link(
            "DS1",
            "TenGigabitEthernet1/1/1",
            "DS2",
            "TenGigabitEthernet1/1/1",
        ),
        link(
            "DS1",
            "TenGigabitEthernet1/1/2",
            "DS2",
            "TenGigabitEthernet1/1/2",
        ),
        link("NMS", "Ethernet0", "DS1", "GigabitEthernet1/0/24"),
        link("SRV1", "Ethernet0", "DS2", "GigabitEthernet1/0/24"),
    ];
    for n in 1..=3 {
        let switch = format!("AS{n}");
        links.push(link(
            &switch,
            "GigabitEthernet0/1",
            "DS1",
            &format!("GigabitEthernet1/0/1{n}"),
        ));
        links.push(link(
            &switch,
            "GigabitEthernet0/2",
            "DS2",
            &format!("GigabitEthernet1/0/1{n}"),
        ));
    }
    for (host, switch, port) in [
        ("Staff-PC1", "AS1", 1),
        ("Staff-PC2", "AS1", 2),
        ("Staff-Laptop", "AS2", 1),
        ("Guest-Laptop1", "AS2", 2),
        ("Guest-Laptop2", "AS3", 1),
        ("Phone1", "AS3", 2),
        ("Printer1", "AS3", 3),
    ] {
        links.push(link(
            host,
            "Ethernet0",
            switch,
            &format!("FastEthernet0/{port}"),
        ));
    }
    LabSpec {
        name: "Campus".to_string(),
        description: "A small campus: Internet edge with NAT, two layer-3 distribution switches sharing the gateways \
                      (HSRP) and routing with OSPF, three access switches with redundant uplinks, Staff, Guest, Voice \
                      and Management VLANs, DHCP for the users, a server and the management station. Log in to any \
                      network device over SSH as admin / nocturne."
            .to_string(),
        nodes,
        links,
        notes: vec![
            note(360.0, 110.0, 520.0, 110.0, "Edge: NAT overload to the Internet, default route originated into OSPF", "#f6e3b4"),
            note(160.0, 270.0, 920.0, 110.0, "Distribution: OSPF area 0, HSRP gateways (.1), DHCP on DS1, guests fenced in by ACL GUEST-IN", "#cfe3f7"),
            note(80.0, 430.0, 1100.0, 250.0, "Access: VLAN 10 Staff, 20 Guest, 30 Voice, 99 Management. One uplink per VLAN is blocked by spanning tree.", "#d8f0d2"),
        ],
        flows: vec![
            flow("Staff web browsing", "Staff-PC1", "8.8.8.8", 2_000_000, FlowPattern::Sine { period_s: 120 }),
            flow("Nightly backup to SRV1", "Staff-PC2", "10.10.10.10", 20_000_000, FlowPattern::Burst { on_s: 40, off_s: 80 }),
            flow("Guest video stream", "Guest-Laptop1", "8.8.8.8", 5_000_000, FlowPattern::Constant),
        ],
        seed: 0x00C0_FFEE,
    }
}

fn basic_switching() -> LabSpec {
    let host = |n: u8| fixed(&format!("192.168.1.1{n}"), "255.255.255.0", None);
    LabSpec {
        name: "Basic switching".to_string(),
        description: "Three PCs on one switch. Ping between them and watch the switch learn where each MAC address \
                      lives (show mac address-table); the first ping loses a packet while ARP resolves."
            .to_string(),
        nodes: vec![
            device("SW1", "switch", 400.0, 200.0, "hostname SW1\n"),
            end_host("PC1", "pc", 200.0, 380.0, host(1)),
            end_host("PC2", "pc", 400.0, 380.0, host(2)),
            end_host("PC3", "pc", 600.0, 380.0, host(3)),
        ],
        links: vec![
            link("PC1", "Ethernet0", "SW1", "FastEthernet0/1"),
            link("PC2", "Ethernet0", "SW1", "FastEthernet0/2"),
            link("PC3", "Ethernet0", "SW1", "FastEthernet0/3"),
        ],
        notes: vec![note(140.0, 120.0, 520.0, 40.0, "Try: ping 192.168.1.12 from PC1, then show mac address-table on SW1", "#f6e3b4")],
        flows: Vec::new(),
        seed: 1,
    }
}

fn vlans_and_trunk() -> LabSpec {
    let switch = |n: u8| {
        format!(
            "hostname SW{n}\nvlan 10\n name Sales\nvlan 20\n name Engineering\ninterface FastEthernet0/1\n switchport mode access\n switchport access vlan 10\n spanning-tree portfast\ninterface FastEthernet0/2\n switchport mode access\n switchport access vlan 20\n spanning-tree portfast\ninterface GigabitEthernet0/1\n description Trunk to the other switch\n switchport mode trunk\n switchport trunk allowed vlan 10,20\n"
        )
    };
    LabSpec {
        name: "VLANs and a trunk".to_string(),
        description: "Two switches joined by an 802.1Q trunk. Sales (VLAN 10) and Engineering (VLAN 20) each span both \
                      switches; hosts reach their own VLAN across the trunk but never the other one. Remove a VLAN from \
                      the trunk's allowed list to see it cut off."
            .to_string(),
        nodes: vec![
            device("SW1", "switch", 300.0, 200.0, &switch(1)),
            device("SW2", "switch", 700.0, 200.0, &switch(2)),
            end_host("Sales1", "pc", 200.0, 380.0, fixed("10.0.10.11", "255.255.255.0", None)),
            end_host("Eng1", "pc", 400.0, 380.0, fixed("10.0.20.11", "255.255.255.0", None)),
            end_host("Sales2", "pc", 600.0, 380.0, fixed("10.0.10.12", "255.255.255.0", None)),
            end_host("Eng2", "pc", 800.0, 380.0, fixed("10.0.20.12", "255.255.255.0", None)),
        ],
        links: vec![
            link("SW1", "GigabitEthernet0/1", "SW2", "GigabitEthernet0/1"),
            link("Sales1", "Ethernet0", "SW1", "FastEthernet0/1"),
            link("Eng1", "Ethernet0", "SW1", "FastEthernet0/2"),
            link("Sales2", "Ethernet0", "SW2", "FastEthernet0/1"),
            link("Eng2", "Ethernet0", "SW2", "FastEthernet0/2"),
        ],
        notes: vec![note(240.0, 120.0, 520.0, 40.0, "Try: show vlan brief, show interfaces trunk, ping 10.0.10.12 from Sales1", "#cfe3f7")],
        flows: Vec::new(),
        seed: 2,
    }
}

fn router_on_a_stick() -> LabSpec {
    const ROUTER: &str = "\
hostname R1
interface GigabitEthernet0/0
 description Trunk to SW1
 no shutdown
interface GigabitEthernet0/0.10
 description Users
 encapsulation dot1Q 10
 ip address 192.168.10.1 255.255.255.0
interface GigabitEthernet0/0.20
 description Servers
 encapsulation dot1Q 20
 ip address 192.168.20.1 255.255.255.0
";
    const SWITCH: &str = "\
hostname SW1
vlan 10
 name Users
vlan 20
 name Servers
interface FastEthernet0/1
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
interface FastEthernet0/2
 switchport mode access
 switchport access vlan 20
 spanning-tree portfast
interface GigabitEthernet0/1
 description Trunk to R1
 switchport mode trunk
";
    LabSpec {
        name: "Router on a stick".to_string(),
        description: "One router interface carries both VLANs as tagged sub-interfaces and routes between them. \
                      Trace a ping from PC1 to SRV1: it enters the router tagged for VLAN 10 and leaves tagged for VLAN 20 \
                      on the same cable."
            .to_string(),
        nodes: vec![
            device("R1", "router", 500.0, 120.0, ROUTER),
            device("SW1", "switch", 500.0, 280.0, SWITCH),
            end_host("PC1", "pc", 360.0, 440.0, fixed("192.168.10.10", "255.255.255.0", Some("192.168.10.1"))),
            end_host(
                "SRV1",
                "server",
                640.0,
                440.0,
                HostConfig { services: vec!["http".to_string()], ..fixed("192.168.20.10", "255.255.255.0", Some("192.168.20.1")) },
            ),
        ],
        links: vec![
            link("R1", "GigabitEthernet0/0", "SW1", "GigabitEthernet0/1"),
            link("PC1", "Ethernet0", "SW1", "FastEthernet0/1"),
            link("SRV1", "Ethernet0", "SW1", "FastEthernet0/2"),
        ],
        notes: vec![note(300.0, 40.0, 420.0, 40.0, "Try: tracert 192.168.20.10 from PC1, show ip route on R1", "#d8f0d2")],
        flows: vec![flow("PC1 to the web server", "PC1", "192.168.20.10", 1_000_000, FlowPattern::Constant)],
        seed: 3,
    }
}

fn ospf_triangle() -> LabSpec {
    // Router n: LAN 10.n.0.0/24, loopback n.n.n.n, links 10.0.<ab>.0/30.
    let router = |n: u8, a: (u8, u8), b: (u8, u8)| {
        let link = |port: u8, (peer, host): (u8, u8)| {
            let net = if n < peer {
                format!("{n}{peer}")
            } else {
                format!("{peer}{n}")
            };
            format!(
                "interface GigabitEthernet0/{port}\n description To R{peer}\n ip address 10.0.{net}.{host} 255.255.255.252\n no shutdown\n"
            )
        };
        format!(
            "hostname R{n}\ninterface Loopback0\n ip address {n}.{n}.{n}.{n} 255.255.255.255\n{}{}interface GigabitEthernet0/2\n description LAN\n ip address 10.{n}.0.1 255.255.255.0\n no shutdown\nrouter ospf 1\n router-id {n}.{n}.{n}.{n}\n passive-interface GigabitEthernet0/2\n network 10.0.0.0 0.0.255.255 area 0\n network 10.{n}.0.0 0.0.0.255 area 0\n network {n}.{n}.{n}.{n} 0.0.0.0 area 0\n",
            link(0, a),
            link(1, b)
        )
    };
    let pc = |n: u8, x: f32, y: f32| {
        end_host(
            &format!("PC{n}"),
            "pc",
            x,
            y,
            fixed(
                &format!("10.{n}.0.10"),
                "255.255.255.0",
                Some(&format!("10.{n}.0.1")),
            ),
        )
    };
    LabSpec {
        name: "OSPF triangle".to_string(),
        description: "Three routers in a triangle run OSPF in area 0. Every LAN is reachable over two paths: cut a \
                      link and watch the neighbours go down, the routes move to the remaining path and the pings keep \
                      working."
            .to_string(),
        nodes: vec![
            device("R1", "router", 500.0, 120.0, &router(1, (2, 1), (3, 1))),
            device("R2", "router", 300.0, 320.0, &router(2, (1, 2), (3, 1))),
            device("R3", "router", 700.0, 320.0, &router(3, (2, 2), (1, 2))),
            pc(1, 500.0, 20.0),
            pc(2, 160.0, 440.0),
            pc(3, 840.0, 440.0),
        ],
        links: vec![
            link("R1", "GigabitEthernet0/0", "R2", "GigabitEthernet0/0"),
            link("R2", "GigabitEthernet0/1", "R3", "GigabitEthernet0/0"),
            link("R1", "GigabitEthernet0/1", "R3", "GigabitEthernet0/1"),
            link("PC1", "Ethernet0", "R1", "GigabitEthernet0/2"),
            link("PC2", "Ethernet0", "R2", "GigabitEthernet0/2"),
            link("PC3", "Ethernet0", "R3", "GigabitEthernet0/2"),
        ],
        notes: vec![note(560.0, 180.0, 420.0, 40.0, "Try: show ip ospf neighbor, show ip route ospf, then cut R1-R3", "#f6e3b4")],
        flows: vec![flow("PC1 to PC3", "PC1", "10.3.0.10", 500_000, FlowPattern::Constant)],
        seed: 4,
    }
}

fn dhcp_and_nat() -> LabSpec {
    const ROUTER: &str = "\
hostname R1
ip dhcp excluded-address 192.168.1.1 192.168.1.99
ip dhcp pool LAN
 network 192.168.1.0 255.255.255.0
 default-router 192.168.1.1
 dns-server 8.8.8.8
interface GigabitEthernet0/0
 description LAN
 ip address 192.168.1.1 255.255.255.0
 ip nat inside
 no shutdown
interface GigabitEthernet0/1
 description Internet
 ip address 203.0.113.2 255.255.255.252
 ip nat outside
 no shutdown
ip nat inside source list 1 interface GigabitEthernet0/1 overload
ip route 0.0.0.0 0.0.0.0 203.0.113.1
access-list 1 permit 192.168.1.0 0.0.0.255
";
    LabSpec {
        name: "DHCP and NAT".to_string(),
        description: "A home or branch office: the router hands out private addresses by DHCP and hides them behind \
                      its one public address (NAT overload). The Internet has no route back to 192.168.1.0/24, so \
                      without the translation nothing would come back."
            .to_string(),
        nodes: vec![
            internet(500.0, 40.0),
            device("R1", "router", 500.0, 180.0, ROUTER),
            device("SW1", "switch", 500.0, 320.0, "hostname SW1\n"),
            end_host("PC1", "pc", 360.0, 460.0, dhcp_client()),
            end_host("PC2", "laptop", 640.0, 460.0, dhcp_client()),
        ],
        links: vec![
            link("Internet", "Ethernet0", "R1", "GigabitEthernet0/1"),
            link("R1", "GigabitEthernet0/0", "SW1", "GigabitEthernet0/1"),
            link("PC1", "Ethernet0", "SW1", "FastEthernet0/1"),
            link("PC2", "Ethernet0", "SW1", "FastEthernet0/2"),
        ],
        notes: vec![note(560.0, 240.0, 440.0, 40.0, "Try: ipconfig /all on PC1, ping 8.8.8.8, show ip nat translations on R1", "#cfe3f7")],
        flows: vec![flow("PC2 downloads", "PC2", "8.8.8.8", 3_000_000, FlowPattern::Burst { on_s: 20, off_s: 20 })],
        seed: 5,
    }
}

/// Five small teaching labs: basic switching, VLANs and a trunk, router on a
/// stick, an OSPF triangle, and DHCP with NAT.
pub fn examples() -> Vec<LabSpec> {
    vec![
        basic_switching(),
        vlans_and_trunk(),
        router_on_a_stick(),
        ospf_triangle(),
        dhcp_and_nat(),
    ]
}
