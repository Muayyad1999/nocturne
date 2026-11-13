//! Lab nodes are inventory devices.
//!
//! Every node that has a command line gets a device named `<lab>/<node>` with
//! `transport: sim`, a `lab` reference and the site `Lab: <lab>`, so that the
//! terminal, the collectors and the change pipeline reach it the same way they
//! reach real hardware. Network devices are driven with the `cisco_ios`
//! profile; end hosts with `nocturne_sim_host`, which this crate ships.

use std::collections::BTreeMap;

use nocturne_app::inventory::{self, DeviceFilter, DeviceInput};
use nocturne_app::model::{Device, LabRef, MonitorSpec, Transport};
use nocturne_app::{App, AppError, AppResult};
use nocturne_netsim::{CliKind, LabSpec, NodeKind};

use crate::model::LabRecord;

/// The profile network nodes are driven with: the simulator speaks IOS.
pub const NETWORK_PROFILE: &str = "cisco_ios";
/// The profile end hosts are driven with (registered by this crate's `init`).
pub const HOST_PROFILE: &str = "nocturne_sim_host";

/// The command line of the simulator's end hosts, as a device profile.
///
/// The platform this describes is `nocturne-netsim`'s host shell, and the
/// collectors are checked against its real output by this crate's tests.
/// (`verified` is reserved for the driver pack's own profiles.)
pub const SIM_HOST_PROFILE: &str = r#"
id: nocturne_sim_host
name: Simulated host
vendor: Nocturne
os: Host
description: >-
  An end host inside a Nocturne lab: a PC, laptop, server or the management
  station. Windows-style shell (ipconfig, ping, tracert, arp) with no
  configuration mode, so there is nothing to back up and nothing to push.
maturity: community
# Ethernet0, RS232: the names are already unique and must not be put through
# the Cisco abbreviation rules.
interface_names: verbatim

detect:
  probes:
    - command: hostname
      matches: '^\S+\s*$'
  # Below every network platform: this profile only fits what the lab serves.
  priority: 5

cli:
  # The only prompt the host shell has: `C:\>`.
  prompt: '^C:\\>\s?$'
  init: []
  errors:
    - 'is not recognized as an internal or external command'
    - '^Error: '
    - '^Usage: '
    - '^\*\*\* '
  timeout_s: 20

config:
  # A host has no configuration mode and no command that prints its state as
  # configuration: Nocturne says so rather than inventing a backup.
  syntax: flat

policy:
  read_only:
    - '^hostname$'
    - '^ipconfig(\s+/all)?$'
    - '^arp\s+-a$'
    - '^ping\s+\S'
    - '^(?:tracert|traceroute)\s+\S'
    - '^nslookup\s+\S'
    - '^netstat$'
    - '^(?:help|\?)$'
    - '^(?:cls|clear)$'
    - '^(?:exit|logout|quit)$'
  dangerous:
    # Give up the address, or take a new one: the host loses its connectivity.
    - '^ipconfig\s+/(?:release|renew)\b'

collect:
  facts:
    - command: hostname
      parser: raw
      map:
        hostname: output | trim
        vendor: "'Nocturne'"
        os: "'Host'"
  interfaces:
    - command: ipconfig /all
      parser:
        textfsm: |
          Value Required INTERFACE (\S+)
          Value MAC ([0-9A-Fa-f]{2}(?:-[0-9A-Fa-f]{2}){5})
          Value IP (\d+\.\d+\.\d+\.\d+)
          Value MASK (\d+\.\d+\.\d+\.\d+)
          Value MEDIA (.+?)

          Start
            ^Ethernet adapter \S+: -> Continue.Record
            ^Ethernet adapter ${INTERFACE}:
            ^\s+Media State[\s.]*: ${MEDIA}\s*$$
            ^\s+Physical Address[\s.]*: ${MAC}\s*$$
            ^\s+IPv4 Address[\s.]*: ${IP}\s*$$
            ^\s+Subnet Mask[\s.]*: ${MASK}\s*$$
      map:
        name: INTERFACE
        mac: MAC | mac
        ipv4: "[cidr(IP, MASK)] if IP else []"
        # A host's adapter is never shut down administratively; the media
        # state is the only thing it reports.
        admin_up: "true"
        oper_up: "not MEDIA"
        mode: "'host'"
  arp:
    - command: arp -a
      parser:
        regex: '(?m)^\s+(?P<ip>\d+\.\d+\.\d+\.\d+)\s+(?P<mac>[0-9A-Fa-f]{2}(?:-[0-9A-Fa-f]{2}){5})\s+\w+\s*$'
      map:
        ip: ip
        mac: mac | mac
"#;

/// The site every device of a lab belongs to.
pub fn site_of(lab_name: &str) -> String {
    format!("Lab: {lab_name}")
}

/// The inventory name of a node.
pub fn device_name(lab_name: &str, node: &str) -> String {
    format!("{lab_name}/{node}")
}

/// What a node is, for the inventory: `None` when it has no command line and
/// so cannot be a device at all (access point, printer, phone, cloud).
pub fn node_profile(model: &str) -> Option<(&'static str, String)> {
    let info = nocturne_netsim::catalog()
        .into_iter()
        .find(|info| info.id == model)?;
    match info.cli {
        CliKind::Ios => Some((NETWORK_PROFILE, role_of(info.kind))),
        CliKind::Host => Some((HOST_PROFILE, role_of(info.kind))),
        CliKind::None => None,
    }
}

fn role_of(kind: NodeKind) -> String {
    match kind {
        NodeKind::Router => "router",
        NodeKind::Switch => "switch",
        NodeKind::L3Switch => "l3_switch",
        NodeKind::Firewall => "firewall",
        NodeKind::AccessPoint => "access_point",
        NodeKind::Server => "server",
        NodeKind::Nms => "nms",
        NodeKind::Cloud => "cloud",
        NodeKind::Printer => "printer",
        NodeKind::Phone => "phone",
        NodeKind::Pc | NodeKind::Laptop => "host",
    }
    .to_string()
}

/// Lab devices are not polled over the network: a lab address is not a real
/// address, and nothing here may touch the real one.
fn monitoring(profile: &str) -> MonitorSpec {
    MonitorSpec {
        enabled: profile == NETWORK_PROFILE,
        icmp: false,
        snmp: false,
        interval_s: 60,
        ..MonitorSpec::default()
    }
}

/// The devices of one lab, by node name.
pub fn of_lab(app: &App, lab_id: &str) -> AppResult<Vec<Device>> {
    Ok(inventory::list(
        app,
        &DeviceFilter {
            lab: Some(true),
            ..DeviceFilter::default()
        },
    )?
    .into_iter()
    .map(|row| row.device)
    .filter(|device| {
        device
            .lab
            .as_ref()
            .is_some_and(|lab| lab.lab_id == lab_id && !lab.node.is_empty())
    })
    .collect())
}

/// Node name → device id.
pub fn device_ids(app: &App, lab_id: &str) -> AppResult<BTreeMap<String, String>> {
    Ok(of_lab(app, lab_id)?
        .into_iter()
        .filter_map(|device| Some((device.lab.as_ref()?.node.clone(), device.id)))
        .collect())
}

/// Create, rename and remove inventory devices so that they match the lab.
pub fn sync(app: &App, record: &LabRecord) -> AppResult<()> {
    sync_spec(app, &record.id, &record.spec)
}

fn sync_spec(app: &App, lab_id: &str, spec: &LabSpec) -> AppResult<()> {
    let existing = of_lab(app, lab_id)?;
    let site = site_of(&spec.name);
    let mut keep: Vec<String> = Vec::new();

    for node in &spec.nodes {
        let Some((profile, role)) = node_profile(&node.model) else {
            continue;
        };
        let name = device_name(&spec.name, &node.name);
        let found = existing
            .iter()
            .find(|device| device.lab.as_ref().is_some_and(|lab| lab.node == node.name));
        match found {
            Some(device) => {
                keep.push(device.id.clone());
                let input = DeviceInput {
                    name: (device.name != name).then(|| name.clone()),
                    site: (device.site != site).then(|| site.clone()),
                    role: (device.role != role).then_some(role),
                    profile: (device.profile != profile).then(|| profile.to_string()),
                    ..DeviceInput::default()
                };
                let unchanged = input.name.is_none()
                    && input.site.is_none()
                    && input.role.is_none()
                    && input.profile.is_none();
                if !unchanged {
                    inventory::update(app, &device.id, input).map_err(|err| named(err, &name))?;
                }
            }
            None => {
                let row = inventory::create(
                    app,
                    DeviceInput {
                        name: Some(name.clone()),
                        profile: Some(profile.to_string()),
                        transport: Some(Transport::Sim),
                        site: Some(site.clone()),
                        role: Some(role),
                        monitor: Some(monitoring(profile)),
                        lab: Some(LabRef {
                            lab_id: lab_id.to_string(),
                            node: node.name.clone(),
                        }),
                        ..DeviceInput::default()
                    },
                )
                .map_err(|err| named(err, &name))?;
                keep.push(row.device.id);
            }
        }
    }

    let stale: Vec<String> = existing
        .iter()
        .filter(|device| !keep.contains(&device.id))
        .map(|device| device.id.clone())
        .collect();
    if !stale.is_empty() {
        inventory::delete(app, &stale)?;
    }
    Ok(())
}

/// Follow a node rename, keeping the device (and everything collected from it).
pub fn rename_node(app: &App, record: &LabRecord, from: &str, to: &str) -> AppResult<()> {
    let devices = of_lab(app, &record.id)?;
    let Some(device) = devices
        .iter()
        .find(|device| device.lab.as_ref().is_some_and(|lab| lab.node == from))
    else {
        return Ok(());
    };
    let name = device_name(record.name(), to);
    inventory::update(
        app,
        &device.id,
        DeviceInput {
            name: Some(name.clone()),
            lab: Some(LabRef {
                lab_id: record.id.clone(),
                node: to.to_string(),
            }),
            ..DeviceInput::default()
        },
    )
    .map_err(|err| named(err, &name))?;
    Ok(())
}

/// Remove every device of a lab (the lab itself is going away).
pub fn remove_all(app: &App, lab_id: &str) -> AppResult<usize> {
    let ids: Vec<String> = of_lab(app, lab_id)?
        .into_iter()
        .map(|device| device.id)
        .collect();
    if ids.is_empty() {
        return Ok(0);
    }
    inventory::delete(app, &ids)
}

/// Say which device name went wrong: the inventory only knows the name.
fn named(err: AppError, name: &str) -> AppError {
    if err.code == "conflict" {
        return AppError::conflict(format!(
            "the inventory already has a device named `{name}`; rename it, or rename the lab"
        ));
    }
    err
}
