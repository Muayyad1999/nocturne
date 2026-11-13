//! Hand-built collected data for the per-check tests.
//!
//! Every check is tested as a table of cases: the problem, the healthy case,
//! the look-alike that must stay quiet, and the case where the data does not
//! allow a verdict. Building a [`World`] by hand keeps each case to the few
//! rows it is actually about.

use std::collections::HashMap;
use std::sync::Arc;

use nocturne_app::model::Device;
use nocturne_app::store::Store;
use nocturne_drivers::hier::{ConfigTree, Syntax};
use nocturne_drivers::model::{
    ArpEntry, Counters, Facts, Interface, MacEntry, Neighbor, Route, Vlan,
};

use crate::links;
use crate::report::CheckReport;
use crate::settings::Settings;
use crate::world::{Config, Data, DeviceData, Startup, World, canonical};

/// A moment far enough from zero that "an hour ago" is still positive.
pub const NOW: i64 = 1_700_000_000_000;

pub fn device(id: &str, name: &str) -> DeviceData {
    DeviceData {
        device: Device {
            id: id.to_string(),
            name: name.to_string(),
            profile: "cisco_ios".into(),
            ..Device::default()
        },
        syntax: Syntax::Ios,
        ..DeviceData::default()
    }
}

fn data<T>(rows: Vec<T>, at: i64) -> Data<T> {
    Data {
        rows,
        at,
        source: "cli".into(),
    }
}

/// Builder methods used by the check tests.
impl DeviceData {
    pub fn role(mut self, role: &str) -> DeviceData {
        self.device.role = role.to_string();
        self
    }

    pub fn lab_node(mut self) -> DeviceData {
        self.device.lab = Some(nocturne_app::model::LabRef {
            lab_id: "testlab".into(),
            node: self.device.name.clone(),
        });
        self
    }

    pub fn syntax(mut self, syntax: Syntax) -> DeviceData {
        self.syntax = syntax;
        self
    }

    pub fn address(mut self, address: &str) -> DeviceData {
        self.device.address = address.to_string();
        self
    }

    pub fn interfaces(self, rows: Vec<Interface>) -> DeviceData {
        self.interfaces_at(rows, NOW)
    }

    pub fn interfaces_at(mut self, rows: Vec<Interface>, at: i64) -> DeviceData {
        self.interface_index = rows
            .iter()
            .enumerate()
            .map(|(position, interface)| (canonical(&interface.name), position))
            .collect();
        self.interfaces = Some(data(rows, at));
        self
    }

    pub fn previous_interfaces(mut self, rows: Vec<Interface>, at: i64) -> DeviceData {
        self.previous_interfaces = Some(data(rows, at));
        self
    }

    pub fn neighbors(mut self, rows: Vec<Neighbor>) -> DeviceData {
        self.neighbors = Some(data(rows, NOW));
        self
    }

    pub fn previous_neighbors(mut self, rows: Vec<Neighbor>) -> DeviceData {
        self.previous_neighbors = Some(data(rows, NOW - 3_600_000));
        self
    }

    pub fn vlans(mut self, rows: Vec<Vlan>) -> DeviceData {
        self.vlans = Some(data(rows, NOW));
        self
    }

    pub fn mac_table(mut self, rows: Vec<MacEntry>) -> DeviceData {
        self.mac_table = Some(data(rows, NOW));
        self
    }

    pub fn previous_mac_table(mut self, rows: Vec<MacEntry>) -> DeviceData {
        self.previous_mac_table = Some(data(rows, NOW - 300_000));
        self
    }

    pub fn arp(self, rows: Vec<ArpEntry>) -> DeviceData {
        self.arp_at(rows, NOW)
    }

    pub fn arp_at(mut self, rows: Vec<ArpEntry>, at: i64) -> DeviceData {
        self.arp = Some(data(rows, at));
        self
    }

    pub fn routes(mut self, rows: Vec<Route>) -> DeviceData {
        self.routes = Some(data(rows, NOW));
        self
    }

    pub fn facts(mut self, facts: Facts) -> DeviceData {
        self.facts = Some(data(vec![facts], NOW));
        self
    }

    pub fn previous_facts(mut self, facts: Facts, at: i64) -> DeviceData {
        self.previous_facts = Some(data(vec![facts], at));
        self
    }

    pub fn config(self, text: &str) -> DeviceData {
        self.config_at(text, NOW)
    }

    pub fn config_at(mut self, text: &str, at: i64) -> DeviceData {
        self.config = Some(Config::parsed(1, at, self.syntax, text));
        self
    }

    pub fn startup(mut self, text: &str) -> DeviceData {
        self.startup = Some(Startup {
            version_id: 2,
            taken_at: NOW,
            text: text.to_string(),
        });
        self
    }

    pub fn startup_at(mut self, text: &str, at: i64) -> DeviceData {
        self.startup = Some(Startup {
            version_id: 2,
            taken_at: at,
            text: text.to_string(),
        });
        self
    }
}

impl Config {
    /// A configuration parsed outside the cache (tests, and the one-off
    /// comparison `unsaved_config` makes).
    pub fn parsed(version_id: i64, taken_at: i64, syntax: Syntax, text: &str) -> Config {
        let tree = ConfigTree::parse(text, syntax);
        let sections = crate::world::interface_sections(&tree);
        Config {
            version_id,
            taken_at,
            syntax,
            tree: Arc::new(tree),
            sections: Arc::new(sections),
        }
    }
}

pub fn iface(name: &str) -> Interface {
    Interface {
        name: name.to_string(),
        admin_up: Some(true),
        oper_up: Some(true),
        ..Interface::default()
    }
}

/// An interface with the fields the physical checks read.
pub fn port(name: &str, speed_bps: u64, duplex: &str) -> Interface {
    Interface {
        speed_bps: Some(speed_bps),
        duplex: duplex.to_string(),
        ..iface(name)
    }
}

pub fn counters(in_errors: u64, crc: u64, in_packets: u64) -> Counters {
    Counters {
        in_errors: Some(in_errors),
        crc: Some(crc),
        in_packets: Some(in_packets),
        ..Counters::default()
    }
}

pub fn neighbor(local: &str, device: &str, remote: &str) -> Neighbor {
    Neighbor {
        local_interface: local.to_string(),
        device: device.to_string(),
        remote_interface: remote.to_string(),
        protocol: "cdp".into(),
        ..Neighbor::default()
    }
}

pub fn vlan(id: u64, name: &str) -> Vlan {
    Vlan {
        id: Some(id),
        name: name.to_string(),
        status: "active".into(),
        interfaces: Vec::new(),
    }
}

pub fn mac(vlan: u64, address: &str, interface: &str) -> MacEntry {
    MacEntry {
        vlan: Some(vlan),
        mac: address.to_string(),
        interface: interface.to_string(),
        kind: "dynamic".into(),
    }
}

pub fn arp(ip: &str, address: &str, interface: &str) -> ArpEntry {
    ArpEntry {
        ip: ip.to_string(),
        mac: address.to_string(),
        interface: interface.to_string(),
        age: "5".into(),
    }
}

pub fn facts(model: &str, version: &str, uptime_s: u64) -> Facts {
    Facts {
        model: model.to_string(),
        os_version: version.to_string(),
        uptime_s: Some(uptime_s),
        vendor: "Cisco".into(),
        os: "IOS".into(),
        ..Facts::default()
    }
}

/// A world over hand-built devices, with links derived the way a scan does
/// and every device a target.
pub fn world(devices: Vec<DeviceData>) -> World {
    let links = links::from_neighbors(&devices);
    world_with_links(devices, links)
}

pub fn world_with_links(devices: Vec<DeviceData>, links: Vec<links::Link>) -> World {
    let index: HashMap<String, usize> = devices
        .iter()
        .enumerate()
        .map(|(position, data)| (data.device.id.clone(), position))
        .collect();
    let targets: Vec<usize> = (0..devices.len()).collect();
    let target_flags = vec![true; devices.len()];
    let store = Arc::new(Store::in_memory().expect("in-memory store"));
    World {
        now: NOW,
        devices,
        index,
        targets,
        target_flags,
        links,
        link_source: "neighbors",
        config_feature: true,
        applied_changes: None,
        series: Default::default(),
        store,
    }
}

/// Run one check over a world with the default settings.
pub fn run(check: &'static str, world: &World) -> CheckReport {
    run_with(check, world, &Settings::default())
}

pub fn run_with(check: &'static str, world: &World, settings: &Settings) -> CheckReport {
    let entry = crate::catalogue::get(check).expect("a check of that id");
    let mut report = CheckReport::new(entry.id);
    (entry.run)(world, settings, &mut report);
    report
}

/// The subjects a check reported, sorted, for compact assertions.
pub fn subjects(report: &CheckReport) -> Vec<String> {
    let mut subjects: Vec<String> = report
        .findings
        .iter()
        .map(|finding| format!("{}:{}", finding.device_id, finding.subject))
        .collect();
    subjects.sort();
    subjects
}
