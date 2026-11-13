//! Hand-built networks and the tests that use them.
//!
//! Nothing here touches a device: every network is made by storing collected
//! rows with `collect::store_many`, exactly as the CLI collectors, SNMP
//! polling and the lab simulator do.

use nocturne_app::collect;
use nocturne_app::inventory::DeviceInput;
use nocturne_app::{App, AppBuilder, inventory};
use nocturne_drivers::model::Kind;
use serde_json::{Value, json};

mod exports;
mod graph_tests;
mod layout_tests;
mod locating;
mod paths;
mod rpc;

/// An app with the core modules and this feature registered.
pub fn builder() -> AppBuilder {
    let mut builder = nocturne_app::testing::builder();
    crate::init(&mut builder);
    builder
}

pub fn app() -> App {
    builder().build()
}

/// Add a device to the inventory and return its id.
pub fn device(app: &App, name: &str, address: &str, role: &str, site: &str) -> String {
    let row = inventory::create(
        app,
        DeviceInput {
            name: Some(name.to_string()),
            address: Some(address.to_string()),
            role: Some(role.to_string()),
            site: Some(site.to_string()),
            transport: Some(nocturne_app::model::Transport::Ssh),
            ..DeviceInput::default()
        },
    )
    .expect("the device is created");
    inventory::update_state(app, &row.device.id, |state| state.reachable = Some(true))
        .expect("state");
    row.device.id
}

/// Store collected rows for a device.
pub fn store(app: &App, device_id: &str, data: Vec<(Kind, Vec<Value>)>) {
    collect::store_many(app, device_id, data, "cli").expect("collected data is stored");
}

/// A neighbour row, as a profile's `map` would produce it.
pub fn neighbor(local: &str, reported: &str, remote: &str, protocol: &str) -> Value {
    json!({
        "local_interface": local,
        "device": reported,
        "remote_interface": remote,
        "protocol": protocol,
    })
}

/// A neighbour row with the extra fields the protocols carry.
pub fn neighbor_full(
    local: &str,
    reported: &str,
    remote: &str,
    protocol: &str,
    address: &str,
    platform: &str,
    capabilities: &str,
) -> Value {
    json!({
        "local_interface": local,
        "device": reported,
        "remote_interface": remote,
        "protocol": protocol,
        "address": address,
        "platform": platform,
        "capabilities": capabilities,
    })
}

/// An interface row for a trunk or uplink port.
pub fn trunk(name: &str, speed_bps: u64) -> Value {
    json!({"name": name, "mode": "trunk", "admin_up": true, "oper_up": true, "speed_bps": speed_bps})
}

/// An interface row for an access port.
pub fn access_port(name: &str, vlan: u64) -> Value {
    json!({"name": name, "mode": "access", "vlan": vlan, "admin_up": true, "oper_up": true, "speed_bps": 1_000_000_000u64})
}

/// A routed interface row.
pub fn routed(name: &str, cidr: &str) -> Value {
    json!({"name": name, "mode": "routed", "admin_up": true, "oper_up": true, "ipv4": [cidr], "speed_bps": 10_000_000_000u64})
}

pub fn mac_entry(mac: &str, interface: &str, vlan: u64) -> Value {
    json!({"mac": mac, "interface": interface, "vlan": vlan, "kind": "dynamic"})
}

pub fn arp_entry(ip: &str, mac: &str, interface: &str) -> Value {
    json!({"ip": ip, "mac": mac, "interface": interface, "age": "4"})
}

pub fn route(prefix: &str, protocol: &str, next_hop: &str, interface: &str) -> Value {
    json!({"prefix": prefix, "protocol": protocol, "next_hop": next_hop, "interface": interface, "distance": 110, "metric": 20})
}

/// A three-tier campus: two cores, four distribution switches and twenty
/// access switches, each dual-homed, with hosts on three of them, a
/// port-channel, and an access point nobody manages.
pub struct Campus {
    pub app: App,
    pub core: Vec<String>,
    pub dist: Vec<String>,
    pub access: Vec<String>,
}

pub fn campus() -> Campus {
    let app = app();
    let mut core = Vec::new();
    let mut dist = Vec::new();
    let mut access = Vec::new();

    for index in 1..=2 {
        core.push(device(
            &app,
            &format!("CORE{index}"),
            &format!("10.0.0.{index}"),
            "core",
            "HQ",
        ));
    }
    for index in 1..=4 {
        let site = if index <= 2 { "HQ" } else { "North Campus" };
        dist.push(device(
            &app,
            &format!("DIST{index}"),
            &format!("10.0.1.{index}"),
            "distribution",
            site,
        ));
    }
    for index in 1..=20 {
        let site = if index <= 10 { "HQ" } else { "North Campus" };
        access.push(device(
            &app,
            &format!("ACC{index:02}"),
            &format!("10.0.2.{index}"),
            "access",
            site,
        ));
    }

    let mut neighbours: Vec<Vec<Value>> = vec![Vec::new(); core.len() + dist.len() + access.len()];
    let mut interfaces: Vec<Vec<Value>> = vec![Vec::new(); neighbours.len()];
    let core_at = |index: usize| index;
    let dist_at = |index: usize| core.len() + index;
    let access_at = |index: usize| core.len() + dist.len() + index;

    // Core ↔ distribution: both ends run LLDP, so every link is confirmed.
    for core_index in 0..core.len() {
        for dist_index in 0..dist.len() {
            let core_port = format!("TenGigabitEthernet1/1/{}", dist_index + 1);
            let dist_port = format!("TenGigabitEthernet1/0/{}", core_index + 1);
            neighbours[core_at(core_index)].push(neighbor(
                &core_port,
                &format!("DIST{}", dist_index + 1),
                &dist_port,
                "lldp",
            ));
            interfaces[core_at(core_index)].push(trunk(&core_port, 10_000_000_000));
            neighbours[dist_at(dist_index)].push(neighbor(
                &dist_port,
                &format!("CORE{}", core_index + 1),
                &core_port,
                "lldp",
            ));
            interfaces[dist_at(dist_index)].push(trunk(&dist_port, 10_000_000_000));
        }
    }

    // Each access switch is dual-homed to the two distribution switches of its
    // site; one end reports short interface names, the other long ones.
    for index in 0..access.len() {
        let pair = if index < 10 { [0, 1] } else { [2, 3] };
        for (slot, dist_index) in pair.into_iter().enumerate() {
            let access_port = format!("GigabitEthernet1/0/4{}", slot + 7);
            let dist_port = format!("GigabitEthernet1/0/{}", index % 10 + 1);
            neighbours[access_at(index)].push(neighbor(
                &access_port,
                &format!("DIST{}", dist_index + 1),
                &nocturne_drivers::ifname::short(&dist_port),
                "lldp",
            ));
            interfaces[access_at(index)].push(trunk(&access_port, 1_000_000_000));
            neighbours[dist_at(dist_index)].push(neighbor(
                &dist_port,
                &format!("ACC{:02}", index + 1),
                &access_port,
                "lldp",
            ));
            interfaces[dist_at(dist_index)].push(trunk(&dist_port, 1_000_000_000));
        }
    }

    // ACC01 reaches DIST1 over a two-member port-channel instead of a single
    // uplink, so the pair has two parallel links to fold.
    neighbours[access_at(0)].clear();
    neighbours[dist_at(0)].retain(|row| row["device"] != json!("ACC01"));
    interfaces[access_at(0)].clear();
    neighbours[access_at(0)].push(neighbor(
        "GigabitEthernet1/0/48",
        "DIST2",
        "Gi1/0/1",
        "lldp",
    ));
    interfaces[access_at(0)].push(trunk("GigabitEthernet1/0/48", 1_000_000_000));
    for member in 1..=2 {
        let access_port = format!("GigabitEthernet1/0/4{}", member + 4);
        let dist_port = format!("GigabitEthernet1/0/2{member}");
        neighbours[access_at(0)].push(neighbor(&access_port, "DIST1", &dist_port, "lldp"));
        neighbours[dist_at(0)].push(neighbor(&dist_port, "ACC01", &access_port, "lldp"));
        interfaces[access_at(0)].push(trunk(&access_port, 1_000_000_000));
        interfaces[dist_at(0)].push(trunk(&dist_port, 1_000_000_000));
    }
    interfaces[access_at(0)].push(trunk("Port-channel1", 1_000_000_000));
    interfaces[dist_at(0)].push(trunk("Port-channel10", 1_000_000_000));

    // An access point nobody manages, seen by CDP from one end only.
    neighbours[access_at(3)].push(neighbor_full(
        "GigabitEthernet1/0/20",
        "AP-LOBBY",
        "GigabitEthernet0",
        "cdp",
        "10.1.20.9",
        "cisco AIR-CAP2702I-E-K9",
        "Bridge, WLAN Access Point",
    ));
    interfaces[access_at(3)].push(trunk("GigabitEthernet1/0/20", 1_000_000_000));

    // Every switch has a management address, and the cores a routed /30
    // between them, so the layer 3 view has something to show.
    for (index, _) in neighbours.iter().enumerate() {
        interfaces[index].push(json!({
            "name": "Vlan99",
            "ipv4": [format!("10.0.99.{}/24", index + 1)],
            "oper_up": true,
            "mode": "routed",
        }));
    }
    interfaces[core_at(0)].push(routed("TenGigabitEthernet1/1/8", "10.0.255.1/30"));
    interfaces[core_at(1)].push(routed("TenGigabitEthernet1/1/8", "10.0.255.2/30"));

    for (index, id) in core
        .iter()
        .chain(dist.iter())
        .chain(access.iter())
        .enumerate()
    {
        let name = inventory::get(&app, id).expect("device").name;
        store(
            &app,
            id,
            vec![
                (Kind::Neighbors, neighbours[index].clone()),
                (Kind::Interfaces, interfaces[index].clone()),
                (
                    Kind::Facts,
                    vec![json!({
                        "hostname": name,
                        "vendor": "Cisco",
                        "model": if index < 2 {
                            "C9500-24Y4C"
                        } else if index < 6 {
                            "C9300-48P"
                        } else {
                            "WS-C2960X-48FPS-L"
                        },
                    })],
                ),
            ],
        );
    }

    // Hosts on the first three access switches.
    for index in 0..3 {
        let port = format!("GigabitEthernet1/0/{}", index + 5);
        let host_mac = format!("00:1b:21:aa:00:0{}", index + 1);
        let mut ports = interfaces[access_at(index)].clone();
        ports.push(access_port(&port, 10));
        store(
            &app,
            &access[index],
            vec![
                (Kind::Interfaces, ports),
                (Kind::MacTable, vec![mac_entry(&host_mac, &port, 10)]),
                (
                    Kind::Arp,
                    vec![arp_entry(
                        &format!("10.1.10.{}", index + 50),
                        &host_mac,
                        "Vlan10",
                    )],
                ),
            ],
        );
    }

    Campus {
        app,
        core,
        dist,
        access,
    }
}
