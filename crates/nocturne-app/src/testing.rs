//! Test support shared by the core and the feature crates (enable the
//! `testing` feature of this crate in `dev-dependencies`).
//!
//! [`FakeDevice`] is a small IOS-flavoured switch behind a transport
//! `Connection`: it has user/privileged/config modes, an enable secret, a real
//! hierarchical running configuration that configuration commands change,
//! `show running-config` / `show startup-config`, `write memory`, a reload
//! timer, canned `show` outputs, and failure injection. It is not the lab
//! simulator — just enough device to test code that reads, plans and applies
//! configuration without hardware.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nocturne_transport::{Backend, Connection, ConnectionInfo, Input, Output, TransportKind};
use parking_lot::Mutex;
use serde_json::json;

use crate::app::{App, AppBuilder, AppConfig};
use crate::connect::{ConnectState, SimConnector};
use crate::error::{AppError, AppResult};
use crate::model::LabRef;

/// The profile the fake device is driven with.
pub const LAB_IOS: &str = r#"
id: test_ios
name: Test IOS
vendor: Test
os: IOS
cli:
  prompt: '^(?P<host>[\w.\-]{1,63})(\([\w\-/. ]+\))?[>#]\s?$'
  user_prompt: '^[\w.\-]{1,63}>\s?$'
  config_prompt: '^[\w.\-]{1,63}\([\w\-/. ]+\)#\s?$'
  enable: {}
  init: ['terminal length 0']
  errors: ['^% ']
config:
  syntax: ios
  enter: ['configure terminal']
  exit: ['end']
  save: ['write memory']
  running: 'show running-config'
  startup: 'show startup-config'
  ignore: ['^Building configuration', '^Current configuration\s*:']
  errors: ['^% ']
  guard:
    name: reload timer
    arm: ['reload in {minutes}']
    disarm: ['reload cancel']
    effect: the device reloads and comes back with its saved configuration
policy:
  read_only: ['^show ', '^ping ', '^traceroute ', '^terminal (length|width) ']
  dangerous: ['^reload', '^erase ', '^write erase', '^delete ', '^format ']
collect:
  facts:
    - command: show version
      parser: {regex: 'Version (?P<version>[^,\s]+)'}
      map: {os_version: version, vendor: "'Cisco'", os: "'IOS'"}
    - command: show version
      parser: {regex: '(?m)^(?P<host>\S+) uptime is (?P<uptime>.+)$'}
      map: {hostname: host, uptime_s: 'uptime | duration'}
    - command: show version
      parser: {regex: '(?m)^cisco (?P<model>\S+) .*processor'}
      map: {model: model}
    - command: show version
      parser: {regex: 'Processor board ID (?P<serial>\S+)'}
      map: {serial: serial}
  interfaces:
    - command: show ip interface brief
      parser: {regex: '(?m)^(?P<name>[A-Z][\w/.\-]+)\s+(?P<ip>\S+)\s+YES\s+\S+\s+(?P<status>administratively down|up|down)\s+(?P<protocol>up|down)\s*$'}
      map: {name: name, admin_up: "status != 'administratively down'", oper_up: "protocol == 'up'"}
  neighbors:
    - command: show cdp neighbors detail
      parser: {regex: 'Device ID: (?P<device>\S+)[\s\S]*?IP address: (?P<address>\S+)[\s\S]*?Platform: (?P<platform>[^,]+),[\s\S]*?Interface: (?P<local>[^,]+),\s+Port ID \(outgoing port\): (?P<remote>\S+)'}
      map: {device: device, address: address, platform: platform, local_interface: local, remote_interface: remote, protocol: "'cdp'"}
  vlans:
    - command: show vlan brief
      parser: {regex: '(?m)^(?P<id>\d+)\s+(?P<name>\S+)\s+(?P<status>active|suspended)\s*(?P<ports>.*)$'}
      map: {id: id, name: name, status: status}
"#;

/// One top-level configuration line and the lines beneath it.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    line: String,
    children: Vec<String>,
}

#[derive(Default)]
pub struct FakeState {
    pub hostname: String,
    pub enable_secret: Option<String>,
    running: Vec<Entry>,
    startup: Vec<Entry>,
    /// Exact exec command → what it prints.
    pub outputs: HashMap<String, String>,
    /// A configuration line containing any of these is rejected.
    pub reject: Vec<String>,
    /// Receiving this configuration line cuts the connection (a change that locks you out).
    pub drop_on: Option<String>,
    /// Addresses `ping` cannot reach.
    pub unreachable: Vec<String>,
    /// Every command received, in order, across all sessions.
    pub seen: Vec<String>,
    /// Minutes until the armed reload, if any.
    pub reload_in: Option<u32>,
    /// Times the reload timer was armed and cancelled.
    pub reload_armed: u32,
    pub reload_cancelled: u32,
    /// Sessions opened so far.
    pub sessions: u32,
    /// What `show cdp neighbors detail` reports.
    pub neighbors: Vec<FakeNeighbor>,
    /// Interfaces whose link is down (line protocol down although not shut).
    pub link_down: Vec<String>,
}

/// A CDP neighbour of a [`FakeDevice`].
#[derive(Clone, Debug, PartialEq)]
pub struct FakeNeighbor {
    pub local_interface: String,
    pub device: String,
    pub remote_interface: String,
    pub address: String,
    pub platform: String,
}

/// Commands that start a configuration section.
const SECTIONS: [&str; 12] = [
    "interface ",
    "router ",
    "line ",
    "vlan ",
    "ip access-list ",
    "class-map ",
    "policy-map ",
    "route-map ",
    "vrf definition ",
    "key chain ",
    "ip dhcp pool ",
    "archive",
];

/// Global commands that end a section when typed inside one.
const GLOBALS: [&str; 22] = [
    "hostname ",
    "ntp ",
    "logging ",
    "snmp-server ",
    "banner ",
    "service ",
    "username ",
    "enable ",
    "aaa ",
    "clock ",
    "vtp ",
    "access-list ",
    "ip route ",
    "ip domain",
    "ip name-server ",
    "ip http ",
    "ip ssh ",
    "ip default-gateway ",
    "ip routing",
    "spanning-tree mode ",
    "spanning-tree vlan ",
    "lldp run",
];

/// Sub-commands of which a section holds one: setting it again replaces it.
const SLOTS: [&str; 12] = [
    "description ",
    "ip address ",
    "switchport mode ",
    "switchport access vlan ",
    "switchport trunk native vlan ",
    "speed ",
    "duplex ",
    "mtu ",
    "encapsulation ",
    "channel-group ",
    "name ",
    "router-id ",
];

const TRUNK_ALLOWED: &str = "switchport trunk allowed vlan ";

fn vlan_list(text: &str) -> Vec<u16> {
    let mut vlans = Vec::new();
    for part in text.split(',') {
        match part.trim().split_once('-') {
            Some((low, high)) => {
                if let (Ok(low), Ok(high)) = (low.trim().parse::<u16>(), high.trim().parse::<u16>())
                {
                    vlans.extend(low..=high);
                }
            }
            None => vlans.extend(part.trim().parse::<u16>().ok()),
        }
    }
    vlans.sort_unstable();
    vlans.dedup();
    vlans
}

fn vlan_text(vlans: &[u16]) -> String {
    let mut parts = Vec::new();
    let mut index = 0;
    while index < vlans.len() {
        let start = vlans[index];
        let mut end = start;
        while index + 1 < vlans.len() && vlans[index + 1] == end + 1 {
            index += 1;
            end = vlans[index];
        }
        parts.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        index += 1;
    }
    parts.join(",")
}

impl FakeState {
    fn render(entries: &[Entry]) -> String {
        let mut body = String::from("!\n");
        for entry in entries {
            body.push_str(&entry.line);
            body.push('\n');
            for child in &entry.children {
                body.push(' ');
                body.push_str(child);
                body.push('\n');
            }
            if !entry.children.is_empty() {
                body.push_str("!\n");
            }
        }
        body.push_str("end\n");
        body
    }

    /// The running configuration as `show running-config` prints it.
    pub fn running_config(&self) -> String {
        let body = Self::render(&self.running);
        format!(
            "Building configuration...\n\nCurrent configuration : {} bytes\n{body}",
            body.len()
        )
    }

    pub fn startup_config(&self) -> String {
        Self::render(&self.startup)
    }

    /// `show ip interface brief`, derived from the running configuration.
    pub fn interface_brief(&self) -> String {
        let mut text = String::from(
            "Interface              IP-Address      OK? Method Status                Protocol\n",
        );
        for entry in self
            .running
            .iter()
            .filter(|entry| entry.line.starts_with("interface "))
        {
            let name = entry.line.trim_start_matches("interface ").trim();
            let address = entry
                .children
                .iter()
                .find_map(|child| child.strip_prefix("ip address "))
                .and_then(|rest| rest.split_whitespace().next())
                .unwrap_or("unassigned");
            let method = if address == "unassigned" {
                "unset"
            } else {
                "manual"
            };
            let (status, protocol) = if entry.children.iter().any(|child| child == "shutdown") {
                ("administratively down", "down")
            } else if self.link_down.iter().any(|down| down == name) {
                ("down", "down")
            } else {
                ("up", "up")
            };
            text.push_str(&format!(
                "{name:<22} {address:<15} YES {method:<6} {status:<21} {protocol}\n"
            ));
        }
        text
    }

    /// `show vlan brief`, derived from the running configuration.
    pub fn vlan_brief(&self) -> String {
        let mut text = String::from(
            "VLAN Name                             Status    Ports\n---- -------------------------------- --------- -------------------------------\n",
        );
        for entry in self
            .running
            .iter()
            .filter(|entry| entry.line.starts_with("vlan "))
        {
            let id = entry.line.trim_start_matches("vlan ").trim();
            let name = entry
                .children
                .iter()
                .find_map(|child| child.strip_prefix("name "))
                .map(str::to_string)
                .unwrap_or_else(|| format!("VLAN{id:0>4}"));
            let access = format!("switchport access vlan {id}");
            let ports: Vec<String> = self
                .running
                .iter()
                .filter(|port| port.children.iter().any(|child| child == &access))
                .map(|port| {
                    port.line
                        .trim_start_matches("interface ")
                        .replace("GigabitEthernet", "Gi")
                        .replace("FastEthernet", "Fa")
                })
                .collect();
            text.push_str(&format!(
                "{id:<4} {name:<32} active    {}\n",
                ports.join(", ")
            ));
        }
        text
    }

    /// `show cdp neighbors detail`.
    pub fn cdp_neighbors_detail(&self) -> String {
        let mut text = String::new();
        for neighbor in &self.neighbors {
            text.push_str(&format!(
                "-------------------------\nDevice ID: {}\nEntry address(es): \n  IP address: {}\nPlatform: {},  Capabilities: Switch IGMP \nInterface: {},  Port ID (outgoing port): {}\nHoldtime : 150 sec\n\n",
                neighbor.device, neighbor.address, neighbor.platform, neighbor.local_interface, neighbor.remote_interface
            ));
        }
        if self.neighbors.is_empty() {
            text.push_str("Total cdp entries displayed : 0\n");
        }
        text
    }

    pub fn saved(&self) -> bool {
        self.running == self.startup
    }

    /// Load a configuration (as text) into running and startup.
    pub fn load(&mut self, text: &str) {
        self.running.clear();
        let mut section = None;
        for line in text.lines() {
            // A line the device would reject is simply not part of the loaded configuration.
            let _ = self.configure(line, &mut section);
        }
        self.startup = self.running.clone();
    }

    fn is_section(line: &str) -> bool {
        SECTIONS
            .iter()
            .any(|start| line.starts_with(start) || line == start.trim_end())
    }

    /// Apply configuration lines as if they had been typed in configuration
    /// mode, leaving the startup configuration alone — which is how a test
    /// produces a device with unsaved changes. Stops at the first line the
    /// device would reject and returns what it printed.
    pub fn apply(&mut self, lines: &[&str]) -> Result<(), String> {
        let mut section = None;
        for line in lines {
            self.configure(line, &mut section)?;
        }
        Ok(())
    }

    /// Apply one configuration-mode line. `Err` is what the device prints.
    fn configure(&mut self, raw: &str, section: &mut Option<usize>) -> Result<(), String> {
        let indented = raw.starts_with(' ');
        let line = raw.trim();
        if line.is_empty() || line.starts_with('!') || line == "end" {
            if !indented {
                *section = None;
            }
            return Ok(());
        }
        if self.reject.iter().any(|bad| line.contains(bad.as_str())) {
            return Err("% Invalid input detected at '^' marker.".into());
        }

        if let Some(target) = line.strip_prefix("no ") {
            // Inside a section first, as the device does; then globally.
            if let Some(index) = *section {
                let children = &mut self.running[index].children;
                if target.starts_with(TRUNK_ALLOWED.trim_end()) {
                    children.retain(|child| !child.starts_with(TRUNK_ALLOWED));
                    return Ok(());
                }
                let before = children.len();
                children
                    .retain(|child| child != target && !child.starts_with(&format!("{target} ")));
                if children.len() != before || target == "shutdown" {
                    return Ok(());
                }
            }
            let before = self.running.len();
            self.running.retain(|entry| {
                entry.line != target && !entry.line.starts_with(&format!("{target} "))
            });
            if self.running.len() != before {
                *section = None;
            }
            // Removing something that is not there is not an error on IOS.
            return Ok(());
        }

        if Self::is_section(line) {
            let index = match self.running.iter().position(|entry| entry.line == line) {
                Some(index) => index,
                None => {
                    self.running.push(Entry {
                        line: line.to_string(),
                        children: Vec::new(),
                    });
                    self.running.len() - 1
                }
            };
            *section = Some(index);
            return Ok(());
        }

        // An indented line in a loaded configuration belongs to its section whatever it says.
        let global = !indented
            && GLOBALS
                .iter()
                .any(|start| line.starts_with(start) || line == start.trim_end());
        match *section {
            Some(index) if !global => {
                let children = &mut self.running[index].children;
                if let Some(rest) = line.strip_prefix(TRUNK_ALLOWED) {
                    let current: Vec<u16> = children
                        .iter()
                        .find_map(|child| child.strip_prefix(TRUNK_ALLOWED))
                        .map(vlan_list)
                        .unwrap_or_default();
                    let had_list = children
                        .iter()
                        .any(|child| child.starts_with(TRUNK_ALLOWED));
                    let next = if let Some(added) = rest.strip_prefix("add ") {
                        // On a trunk with no list (all VLANs allowed) `add` changes nothing.
                        if !had_list {
                            return Ok(());
                        }
                        let mut merged = current;
                        merged.extend(vlan_list(added));
                        merged.sort_unstable();
                        merged.dedup();
                        merged
                    } else if let Some(removed) = rest.strip_prefix("remove ") {
                        let removed = vlan_list(removed);
                        let base = if had_list {
                            current
                        } else {
                            (1..=4094).collect()
                        };
                        base.into_iter()
                            .filter(|vlan| !removed.contains(vlan))
                            .collect()
                    } else {
                        vlan_list(rest)
                    };
                    children.retain(|child| !child.starts_with(TRUNK_ALLOWED));
                    children.push(format!("{TRUNK_ALLOWED}{}", vlan_text(&next)));
                    return Ok(());
                }
                // A slot keeps its place when its value changes, as in a real running-config.
                let held = SLOTS
                    .iter()
                    .find(|slot| line.starts_with(**slot))
                    .and_then(|slot| children.iter().position(|child| child.starts_with(slot)));
                match held {
                    Some(position) => children[position] = line.to_string(),
                    None if !children.iter().any(|child| child == line) => {
                        children.push(line.to_string())
                    }
                    None => {}
                }
            }
            _ => {
                *section = None;
                if let Some(name) = line.strip_prefix("hostname ") {
                    self.hostname = name.trim().to_string();
                }
                for slot in [
                    "hostname ",
                    "enable secret ",
                    "ip default-gateway ",
                    "ip domain-name ",
                    "ip domain name ",
                ] {
                    if line.starts_with(slot) {
                        self.running.retain(|entry| !entry.line.starts_with(slot));
                    }
                }
                if !self.running.iter().any(|entry| entry.line == line) {
                    // Keep sections after plain global lines, as a real configuration reads.
                    self.running.push(Entry {
                        line: line.to_string(),
                        children: Vec::new(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// A fake device. Clones share the same state.
#[derive(Clone)]
pub struct FakeDevice {
    state: Arc<Mutex<FakeState>>,
}

impl FakeDevice {
    /// A switch called `hostname` with enable secret `ena` and a small starting configuration.
    pub fn new(hostname: &str) -> FakeDevice {
        let mut state = FakeState {
            hostname: hostname.to_string(),
            enable_secret: Some("ena".into()),
            ..FakeState::default()
        };
        state.load(&format!(
            "hostname {hostname}\n\
             ip domain-name lab.example\n\
             vlan 10\n name users\n\
             vlan 20\n name voice\n\
             interface GigabitEthernet0/1\n description uplink\n switchport mode trunk\n\
             interface GigabitEthernet0/2\n description desk 12\n switchport mode access\n switchport access vlan 10\n\
             interface Vlan1\n ip address 10.0.0.2 255.255.255.0\n\
             ip default-gateway 10.0.0.1\n\
             line vty 0 4\n transport input ssh\n"
        ));
        state.neighbors.push(FakeNeighbor {
            local_interface: "GigabitEthernet0/1".into(),
            device: "CORE1.lab.example".into(),
            remote_interface: "GigabitEthernet1/0/1".into(),
            address: "10.0.0.1".into(),
            platform: "cisco WS-C3850-24T".into(),
        });
        FakeDevice {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// Inspect or change the device from a test.
    pub fn with<T>(&self, work: impl FnOnce(&mut FakeState) -> T) -> T {
        work(&mut self.state.lock())
    }

    pub fn hostname(&self) -> String {
        self.with(|state| state.hostname.clone())
    }

    pub fn seen(&self) -> Vec<String> {
        self.with(|state| state.seen.clone())
    }

    pub fn clear_seen(&self) {
        self.with(|state| state.seen.clear());
    }

    pub fn running_config(&self) -> String {
        self.with(|state| state.running_config())
    }

    /// A new console session on the device.
    pub fn connection(&self) -> Connection {
        let peer = format!("lab:{}", self.hostname());
        let info = ConnectionInfo {
            kind: Some(TransportKind::Sim),
            peer,
            ..ConnectionInfo::default()
        };
        let (connection, backend) = Connection::pair(info, 256);
        self.with(|state| state.sessions += 1);
        tokio::spawn(self.clone().serve(backend));
        connection
    }

    async fn serve(self, mut backend: Backend) {
        let mut privileged = false;
        let mut config = false;
        let mut awaiting_secret = false;
        let mut section: Option<usize> = None;
        let mut line = String::new();
        let output = backend.output.clone();
        let send = |text: String| {
            let output = output.clone();
            async move {
                let _ = output
                    .send(Output::Data(text.replace('\n', "\r\n").into_bytes().into()))
                    .await;
            }
        };
        let prompt =
            |device: &FakeDevice, privileged: bool, config: bool, section: Option<usize>| {
                let state = device.state.lock();
                match (privileged, config, section) {
                    (_, true, Some(index)) => {
                        let kind = state
                            .running
                            .get(index)
                            .map(|entry| entry.line.as_str())
                            .unwrap_or_default();
                        let mode = if kind.starts_with("interface") {
                            "config-if"
                        } else if kind.starts_with("line") {
                            "config-line"
                        } else if kind.starts_with("vlan") {
                            "config-vlan"
                        } else if kind.starts_with("router") {
                            "config-router"
                        } else {
                            "config-sub"
                        };
                        format!("{}({mode})#", state.hostname)
                    }
                    (_, true, None) => format!("{}(config)#", state.hostname),
                    (true, ..) => format!("{}#", state.hostname),
                    _ => format!("{}>", state.hostname),
                }
            };

        send(format!("\n{}", prompt(&self, privileged, config, section))).await;
        while let Some(input) = backend.input.recv().await {
            let bytes = match input {
                Input::Data(bytes) => bytes,
                Input::Close => break,
                _ => continue,
            };
            for byte in bytes.iter().copied() {
                if byte != b'\n' && byte != b'\r' {
                    line.push(byte as char);
                    if !awaiting_secret {
                        send((byte as char).to_string()).await;
                    }
                    continue;
                }
                let command = std::mem::take(&mut line).trim().to_string();
                send("\n".into()).await;

                if awaiting_secret {
                    awaiting_secret = false;
                    if Some(&command) == self.state.lock().enable_secret.as_ref() {
                        privileged = true;
                    } else {
                        send("% Access denied\n".into()).await;
                    }
                } else if !command.is_empty() {
                    self.state.lock().seen.push(command.clone());
                    if config {
                        if self.state.lock().drop_on.as_deref() == Some(command.as_str()) {
                            let _ = backend.output.send(Output::Closed { reason: None }).await;
                            return;
                        }
                        match command.as_str() {
                            "end" => {
                                config = false;
                                section = None;
                            }
                            "exit" if section.is_some() => section = None,
                            "exit" => config = false,
                            _ => {
                                let outcome = self.state.lock().configure(&command, &mut section);
                                if let Err(message) = outcome {
                                    send(format!("{message}\n")).await;
                                }
                            }
                        }
                    } else {
                        let canned = self.state.lock().outputs.get(&command).cloned();
                        match command.as_str() {
                            _ if canned.is_some() => {
                                send(format!("{}\n", canned.unwrap_or_default().trim_end())).await
                            }
                            "enable" if privileged => {}
                            "enable" => {
                                if self.state.lock().enable_secret.is_some() {
                                    awaiting_secret = true;
                                    send("Password: ".into()).await;
                                    continue;
                                }
                                privileged = true;
                            }
                            "exit" | "logout" | "quit" => {
                                let _ = backend.output.send(Output::Closed { reason: None }).await;
                                return;
                            }
                            _ if !privileged
                                && !command.starts_with("show ")
                                && !command.starts_with("ping ") =>
                            {
                                send("% Invalid input detected at '^' marker.\n".into()).await
                            }
                            "terminal length 0" | "terminal width 511" | "terminal width 0" => {}
                            "configure terminal" => {
                                config = true;
                                send("Enter configuration commands, one per line.  End with CNTL/Z.\n".into()).await;
                            }
                            "show running-config" => {
                                let text = self.state.lock().running_config();
                                send(text).await;
                            }
                            "show startup-config" => {
                                let text = self.state.lock().startup_config();
                                send(text).await;
                            }
                            "write memory" | "copy running-config startup-config" => {
                                {
                                    let mut state = self.state.lock();
                                    state.startup = state.running.clone();
                                }
                                send("Building configuration...\n[OK]\n".into()).await;
                            }
                            "reload cancel" => {
                                {
                                    let mut state = self.state.lock();
                                    state.reload_in = None;
                                    state.reload_cancelled += 1;
                                }
                                send("***\n*** --- SHUTDOWN ABORTED ---\n***\n".into()).await;
                            }
                            "show version" => {
                                let hostname = self.hostname();
                                send(format!(
                                    "Cisco IOS Software, C2960 Software (C2960-LANBASEK9-M), Version 15.2(4)E10, RELEASE SOFTWARE (fc2)\n\
                                     {hostname} uptime is 1 week, 2 days, 3 hours, 4 minutes\n\
                                     cisco WS-C2960-24TT-L (PowerPC405) processor (revision B0) with 65536K bytes of memory.\n\
                                     Processor board ID FOC1234X5YZ\n"
                                ))
                                .await;
                            }
                            "show clock" => send("12:00:00.000 UTC Fri Nov 13 2026\n".into()).await,
                            "show ip interface brief" => {
                                let text = self.state.lock().interface_brief();
                                send(text).await;
                            }
                            "show vlan brief" => {
                                let text = self.state.lock().vlan_brief();
                                send(text).await;
                            }
                            "show cdp neighbors detail" => {
                                let text = self.state.lock().cdp_neighbors_detail();
                                send(text).await;
                            }
                            other => {
                                if let Some(minutes) = other
                                    .strip_prefix("reload in ")
                                    .and_then(|minutes| minutes.trim().parse::<u32>().ok())
                                {
                                    {
                                        let mut state = self.state.lock();
                                        state.reload_in = Some(minutes);
                                        state.reload_armed += 1;
                                    }
                                    send(format!("Reload scheduled in {minutes} minutes\n")).await;
                                } else if let Some(target) = other.strip_prefix("ping ") {
                                    let unreachable = self
                                        .state
                                        .lock()
                                        .unreachable
                                        .iter()
                                        .any(|address| address == target.trim());
                                    let (marks, rate) = if unreachable {
                                        (".....", "0 percent (0/5)")
                                    } else {
                                        ("!!!!!", "100 percent (5/5)")
                                    };
                                    send(format!(
                                        "Type escape sequence to abort.\nSending 5, 100-byte ICMP Echos to {}, timeout is 2 seconds:\n{marks}\nSuccess rate is {rate}\n",
                                        target.trim()
                                    ))
                                    .await;
                                } else {
                                    send("% Invalid input detected at '^' marker.\n".into()).await;
                                }
                            }
                        }
                    }
                }
                send(prompt(&self, privileged, config, section)).await;
            }
        }
    }
}

/// Serves [`FakeDevice`]s the way the lab simulator serves its nodes.
pub struct FakeSim {
    pub devices: Vec<FakeDevice>,
}

#[async_trait]
impl SimConnector for FakeSim {
    async fn open(&self, lab: &LabRef, _cols: u16, _rows: u16) -> AppResult<Connection> {
        self.devices
            .iter()
            .find(|device| device.hostname().eq_ignore_ascii_case(&lab.node))
            .map(FakeDevice::connection)
            .ok_or_else(|| AppError::not_found(format!("lab node `{}`", lab.node)))
    }
}

/// An in-memory builder with every core module registered. A feature crate
/// adds its own `init` and then calls [`lab`].
pub fn builder() -> AppBuilder {
    let mut builder = AppBuilder::new(AppConfig::in_memory()).expect("in-memory app");
    crate::init(&mut builder);
    builder
}

/// A running app with fake devices in its inventory.
pub struct Lab {
    pub app: App,
    devices: Vec<(String, String, FakeDevice)>,
    pub credential_id: String,
}

impl Lab {
    /// The inventory id of the device called `name`.
    pub fn id(&self, name: &str) -> String {
        self.devices
            .iter()
            .find(|(known, ..)| known == name)
            .map(|(_, id, _)| id.clone())
            .expect("a lab device of that name")
    }

    pub fn fake(&self, name: &str) -> &FakeDevice {
        self.devices
            .iter()
            .find(|(known, ..)| known == name)
            .map(|(.., fake)| fake)
            .expect("a lab device of that name")
    }

    pub fn ids(&self) -> Vec<String> {
        self.devices.iter().map(|(_, id, _)| id.clone()).collect()
    }
}

/// Build the app and add one fake device per name: profile `test_ios`
/// ([`LAB_IOS`]), a credential called `lab` (admin / pw / enable `ena`),
/// reached through the simulator hook.
pub async fn lab(builder: AppBuilder, names: &[&str]) -> Lab {
    let app = builder.build();
    let fakes: Vec<FakeDevice> = names.iter().map(|name| FakeDevice::new(name)).collect();
    app.state::<ConnectState>()
        .set_sim_connector(Arc::new(FakeSim {
            devices: fakes.clone(),
        }));
    crate::profiles::save(&app, LAB_IOS).expect("the test profile loads");
    let credential = app
        .rpc(
            "credentials.create",
            json!({"name": "lab", "username": "admin", "password": "pw", "enable_secret": "ena"}),
        )
        .await
        .expect("credential");
    let credential_id = credential["id"].as_str().expect("id").to_string();
    let mut devices = Vec::new();
    for (name, fake) in names.iter().zip(fakes) {
        let device = app
            .rpc(
                "devices.create",
                json!({"name": name, "profile": "test_ios", "credential_id": credential_id, "site": "Lab", "lab": {"lab_id": "testlab", "node": name}}),
            )
            .await
            .expect("device");
        devices.push((
            name.to_string(),
            device["id"].as_str().expect("id").to_string(),
            fake,
        ));
    }
    Lab {
        app,
        devices,
        credential_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_changes_the_running_configuration_without_saving_it() {
        let device = FakeDevice::new("SW1");
        assert!(
            device.with(|state| state.saved()),
            "a new device starts saved"
        );
        device
            .with(|state| {
                state.apply(&[
                    "interface GigabitEthernet0/2",
                    "description desk 7",
                    "ntp server 192.0.2.1",
                ])
            })
            .unwrap();
        let running = device.running_config();
        assert!(
            running.contains(" description desk 7") && running.contains("ntp server 192.0.2.1")
        );
        assert!(
            !device.with(|state| state.saved()),
            "the change is not saved yet"
        );
        assert!(
            !device
                .with(|state| state.startup_config())
                .contains("ntp server")
        );
        // A line the device rejects comes back as the device's own words.
        device.with(|state| state.reject.push("bogus".into()));
        let refused = device
            .with(|state| state.apply(&["bogus command"]))
            .unwrap_err();
        assert!(refused.contains("Invalid input"), "{refused}");
    }

    #[test]
    fn the_fake_keeps_a_hierarchical_configuration() {
        let device = FakeDevice::new("SW1");
        let mut section = None;
        device.with(|state| {
            for line in [
                "interface GigabitEthernet0/2",
                "description desk 14",
                "switchport access vlan 20",
                "interface GigabitEthernet0/1",
                "switchport trunk allowed vlan add 30",
                "switchport trunk allowed vlan 10,20",
                "switchport trunk allowed vlan add 30-32",
                "switchport trunk allowed vlan remove 20",
                "ntp server 192.0.2.1",
                "no ip domain-name",
                "vlan 30",
                "name guests",
                "no vlan 20",
            ] {
                state.configure(line, &mut section).unwrap();
            }
            state.reject.push("bogus".into());
            assert!(state.configure("bogus command", &mut section).is_err());
        });
        let config = device.running_config();
        assert!(config.contains("interface GigabitEthernet0/2\n description desk 14\n switchport mode access\n switchport access vlan 20\n!"), "{config}");
        // `add` on an unrestricted trunk is a no-op; a plain list replaces; add and remove edit it.
        assert!(
            config.contains(" switchport trunk allowed vlan 10,30-32\n"),
            "{config}"
        );
        assert!(
            config.contains("ntp server 192.0.2.1\n") && config.contains("vlan 30\n name guests\n")
        );
        assert!(
            !config.contains("ip domain-name")
                && !config.contains("\nvlan 20\n")
                && !config.contains("bogus")
        );
        assert!(!device.with(|state| state.saved()));
        assert!(config.starts_with("Building configuration...\n\nCurrent configuration : "));
    }
}
