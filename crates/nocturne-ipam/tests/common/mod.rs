//! A campus whose devices really are configured.
//!
//! The reconciliation tests need evidence, and evidence that a test wrote by
//! hand proves very little: it says what the author believed a switch would
//! say. So these tests run Nocturne's deterministic simulator
//! (`nocturne-netsim`, an engine — no feature crate is linked) with its
//! reference campus: an Internet edge, two layer-3 distribution switches with
//! SVIs, HSRP, OSPF and DHCP, three access switches and real hosts that have
//! taken leases.
//!
//! Each device is then put in the inventory with the real `cisco_ios` profile
//! and collected through the real collectors: `show interfaces`, `show ip
//! arp` and `show vlan` as the simulated device prints them, parsed by the
//! same ntc-templates a Catalyst's output goes through. What IPAM then reads
//! is what a device said about itself.

use std::sync::Arc;

use nocturne_app::collect;
use nocturne_app::connect::ConnectState;
use nocturne_app::testing::{FakeDevice, FakeSim};
use nocturne_app::{App, AppResult};
use nocturne_drivers::model::Kind;
use nocturne_netsim::{Lab, NodeId, demo_campus};
use serde_json::{Value, json};

/// The lab the devices of this campus belong to, and so the namespace their
/// addresses live in (`lab:campus`).
pub const LAB_ID: &str = "campus";

/// The managed devices of the reference campus, with the site each is in.
pub const CAMPUS: [(&str, &str); 6] = [
    ("EDGE", "Edge"),
    ("DS1", "Campus"),
    ("DS2", "Campus"),
    ("AS1", "Building A"),
    ("AS2", "Building A"),
    ("AS3", "Building B"),
];

/// What a collection reads from each device.
const COMMANDS: [&str; 5] = [
    "show version",
    "show interfaces",
    "show ip arp",
    "show vlan",
    "show ip interface brief",
];

pub struct Campus {
    pub app: App,
    pub lab: Lab,
    devices: Vec<(String, String, FakeDevice)>,
}

impl Campus {
    /// Build the campus, put `nodes` in the inventory and collect from them.
    pub async fn start(nodes: &[&str]) -> Campus {
        let mut lab = Lab::from_spec(&demo_campus()).expect("the reference campus builds");
        // Long enough for spanning tree, OSPF and the DHCP leases to settle,
        // so the ARP tables below are the ones a real network would show.
        lab.settle(60_000_000);

        let mut builder = nocturne_app::testing::builder();
        nocturne_ipam::init(&mut builder);
        let app = builder.build();

        let fakes: Vec<FakeDevice> = nodes.iter().map(|name| FakeDevice::new(name)).collect();
        app.state::<ConnectState>()
            .set_sim_connector(Arc::new(FakeSim {
                devices: fakes.clone(),
            }));
        // These tests drive the sync themselves, so the watcher is not also
        // doing it in the background half way through an assertion. One test
        // turns it back on to check that it works.
        app.rpc(
            "settings.set",
            json!({"section": "ipam", "value": {"sync_on_collect": false}}),
        )
        .await
        .expect("the settings are declared");
        let credential = app
            .rpc(
                "credentials.create",
                json!({"name": "lab", "username": "admin", "password": "pw", "enable_secret": "ena"}),
            )
            .await
            .expect("a credential");
        let credential_id = credential["id"].as_str().expect("id").to_string();

        let mut devices = Vec::new();
        for (name, fake) in nodes.iter().zip(fakes) {
            let site = CAMPUS
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, site)| *site)
                .unwrap_or("Campus");
            let device = app
                .rpc(
                    "devices.create",
                    json!({
                        "name": name,
                        "profile": "cisco_ios",
                        "credential_id": credential_id,
                        "site": site,
                        "lab": {"lab_id": LAB_ID, "node": name},
                    }),
                )
                .await
                .expect("a device");
            devices.push((
                (*name).to_string(),
                device["id"].as_str().expect("id").to_string(),
                fake,
            ));
        }
        let mut campus = Campus { app, lab, devices };
        for (name, ..) in campus.devices.clone() {
            campus.collect(&name).await;
        }
        campus
    }

    pub fn id(&self, name: &str) -> String {
        self.devices
            .iter()
            .find(|(known, ..)| known == name)
            .map(|(_, id, _)| id.clone())
            .unwrap_or_else(|| panic!("a device called {name}"))
    }

    fn node(&self, name: &str) -> NodeId {
        self.lab
            .node_by_name(name)
            .unwrap_or_else(|| panic!("a lab node called {name}"))
    }

    /// Read the device again: what the simulated device prints now goes
    /// through the real collectors.
    pub async fn collect(&mut self, name: &str) {
        let node = self.node(name);
        let (_, id, fake) = self
            .devices
            .iter()
            .find(|(known, ..)| known == name)
            .cloned()
            .unwrap_or_else(|| panic!("a device called {name}"));
        for command in COMMANDS {
            let output = self
                .lab
                .exec(node, command)
                .unwrap_or_else(|err| panic!("{name} answers `{command}`: {err:?}"));
            fake.with(|state| state.outputs.insert(command.to_string(), output));
        }
        let snapshot = collect::run(
            &self.app,
            &id,
            &[Kind::Facts, Kind::Interfaces, Kind::Arp, Kind::Vlans],
        )
        .await
        .expect("the collection runs");
        // A router has no `show vlan`, and the collector records that as a
        // failed read rather than an empty table — which is the behaviour
        // this crate relies on, so it is asserted rather than ignored.
        for problem in &snapshot.problems {
            assert_eq!(
                problem.kind,
                Kind::Vlans,
                "{name} could not be read: {problem:?}"
            );
        }
        assert!(
            snapshot.rows(Kind::Interfaces).is_some(),
            "{name}: the interface table was read"
        );
    }

    /// Change the real configuration of a simulated device, through its own
    /// CLI, and read it again.
    pub async fn configure(&mut self, name: &str, lines: &[&str]) {
        let node = self.node(name);
        let outcome = self
            .lab
            .configure(
                node,
                &lines
                    .iter()
                    .map(|line| (*line).to_string())
                    .collect::<Vec<_>>(),
            )
            .expect("the device accepts configuration");
        let refused: Vec<&str> = outcome
            .lines
            .iter()
            .filter(|line| line.error)
            .map(|line| line.line.as_str())
            .collect();
        assert!(refused.is_empty(), "{name} refused {refused:?}");
        self.lab.settle(10_000_000);
        self.collect(name).await;
    }

    /// A device in the inventory that has never been collected — the honest
    /// hole in any report's coverage.
    pub async fn add_uncollected(&mut self, name: &str) -> String {
        let device = self
            .app
            .rpc(
                "devices.create",
                json!({"name": name, "profile": "cisco_ios", "site": "Campus",
                       "lab": {"lab_id": LAB_ID, "node": name}}),
            )
            .await
            .expect("a device");
        device["id"].as_str().expect("id").to_string()
    }

    pub async fn rpc(&self, method: &str, params: Value) -> AppResult<Value> {
        self.app.rpc(method, params).await
    }

    /// `rpc`, for a call that must work.
    pub async fn ok(&self, method: &str, params: Value) -> Value {
        self.rpc(method, params)
            .await
            .unwrap_or_else(|err| panic!("{method}: {err}"))
    }
}
