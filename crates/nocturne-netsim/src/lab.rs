//! The public face of the simulator.

use std::collections::BTreeMap;
use std::net::Ipv4Addr;

use crate::api::{
    CableKind, CliKind, ConfigLineResult, ConfigOutcome, ConsoleId, Endpoint, Fault, FaultId, FlowId, FlowSpec, FlowState,
    HostConfig, LabError, LabEvent, LabSnapshot, LabSpec, LinkId, LinkSpec, NodeId, NodeMetrics, NodeSpec, NodeTables,
    NoteSpec, PduKind, ProbeResult, SimTime, TraceId,
};
use crate::device::cli::{Console, Next, Reply, Shell, Tag};
use crate::device::config::Vlan;
use crate::device::host::HostShell;
use crate::device::hostcfg;
use crate::device::ios::{IosShell, Mode, runcfg};
use crate::sim::World;
use crate::sim::flows::FlowRuntime;
use crate::sim::node::Node;
use crate::util::net::parse_ip;
use crate::views;

/// How long the control plane must stay unchanged before `settle` returns.
const QUIET_MICROS: u64 = 12_000_000;
const SETTLE_STEP_MICROS: u64 = 250_000;
/// Simulated time a single automation command may take before it is aborted.
const EXEC_LIMIT_MICROS: u64 = 600_000_000;
const MAX_QUEUED_EVENTS: usize = 200_000;

/// A simulated network lab: devices, cables and simulated time.
///
/// All methods are synchronous and deterministic; nothing happens between
/// calls. Time only moves in [`Lab::advance`], [`Lab::settle`] and the
/// automation helpers that have to wait for a command to finish.
#[derive(Clone, Debug)]
pub struct Lab {
    world: World,
    consoles: BTreeMap<ConsoleId, Console>,
    next_console: ConsoleId,
    notes: Vec<NoteSpec>,
}

fn vlan_dat_text(vlans: &BTreeMap<u16, Vlan>) -> String {
    let mut out = String::new();
    for (id, vlan) in vlans.iter().filter(|(id, _)| **id != 1) {
        out.push_str(&format!("vlan {id}\n"));
        if vlan.name != Vlan::default_name(*id) {
            out.push_str(&format!(" name {}\n", vlan.name));
        }
        if !vlan.active {
            out.push_str(" state suspend\n");
        }
    }
    out
}

fn parse_vlan_dat(text: &str) -> Result<BTreeMap<u16, Vlan>, LabError> {
    let bad = |line: &str| LabError::InvalidSpec(format!("vlan_dat: cannot understand \"{line}\""));
    let mut vlans = BTreeMap::from([(1, Vlan::new(1))]);
    let mut current: Option<u16> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('!')) {
        let words: Vec<&str> = line.split_whitespace().collect();
        match (words.as_slice(), current) {
            (["vlan", id], _) => {
                let id: u16 = id.parse().ok().filter(|id| (1..=4094).contains(id)).ok_or_else(|| bad(line))?;
                vlans.entry(id).or_insert_with(|| Vlan::new(id));
                current = Some(id);
            }
            (["name", name], Some(id)) => {
                if let Some(vlan) = vlans.get_mut(&id) {
                    vlan.name = (*name).to_string();
                }
            }
            (["state", state], Some(id)) => {
                if let Some(vlan) = vlans.get_mut(&id) {
                    vlan.active = *state != "suspend";
                }
            }
            _ => return Err(bad(line)),
        }
    }
    Ok(vlans)
}

impl Lab {
    /// An empty lab.
    pub fn new(name: &str) -> Lab {
        Lab { world: World::new(name, 0), consoles: BTreeMap::new(), next_console: 1, notes: Vec::new() }
    }

    /// Build a lab from its saved form. Devices boot from their startup
    /// configurations; call [`Lab::settle`] to let the control plane converge.
    pub fn from_spec(spec: &LabSpec) -> Result<Lab, LabError> {
        let mut lab = Lab::new(&spec.name);
        lab.world = World::new(&spec.name, spec.seed);
        lab.world.description = spec.description.clone();
        for node in &spec.nodes {
            let id = lab.world.insert_node(&node.model, Some(&node.name), node.x, node.y)?;
            let Some(n) = lab.world.nodes.get_mut(&id) else { continue };
            if n.model.is_network_os() {
                n.startup = node.startup_config.clone();
                n.rsa_bits = node.rsa_key_bits;
                if let Some(text) = &node.vlan_dat {
                    n.vlan_dat = Some(parse_vlan_dat(text)?);
                }
            } else if let Some(host) = &node.host {
                hostcfg::validate(n.model, host)?;
                n.host = Some(host.clone());
            }
            n.powered = node.powered;
            if node.powered {
                lab.world.boot(id);
            }
        }
        for link in &spec.links {
            let find = |end: &Endpoint| lab.world.node_by_name(&end.node).ok_or_else(|| LabError::UnknownNodeName(end.node.clone()));
            let (a, b) = (find(&link.a)?, find(&link.b)?);
            lab.world.add_link(a, &link.a.interface, b, &link.b.interface, link.cable)?;
        }
        lab.notes = spec.notes.clone();
        for flow in &spec.flows {
            lab.add_flow(flow.clone())?;
        }
        lab.world.touch();
        lab.world.reconcile();
        // Loading is not something the host needs to hear about event by event.
        lab.world.events.clear();
        lab.world.changed.clear();
        Ok(lab)
    }

    /// The lab in its saved form. Round-trips: `Lab::from_spec(&lab.to_spec())`
    /// behaves identically. `startup_config` is each device's *startup*
    /// configuration, so unsaved running configuration is not included.
    pub fn to_spec(&self) -> LabSpec {
        let name_of = |id: NodeId| self.world.nodes.get(&id).map(|n| n.name.clone()).unwrap_or_default();
        let nodes = self
            .world
            .nodes
            .values()
            .map(|n| NodeSpec {
                name: n.name.clone(),
                model: n.model.id.to_string(),
                x: n.x,
                y: n.y,
                powered: n.powered,
                startup_config: if n.model.is_network_os() { n.startup.clone() } else { None },
                host: n.host.clone(),
                vlan_dat: n.vlan_dat.as_ref().filter(|_| n.model.is_switch()).map(vlan_dat_text),
                rsa_key_bits: n.rsa_bits,
            })
            .collect();
        let mut links: Vec<(LinkId, LinkSpec)> = self
            .world
            .links
            .iter()
            .map(|(id, l)| {
                let spec = LinkSpec {
                    a: Endpoint { node: name_of(l.a.0), interface: l.a.1.name() },
                    b: Endpoint { node: name_of(l.b.0), interface: l.b.1.name() },
                    cable: l.cable,
                };
                (*id, spec)
            })
            .collect();
        links.extend(self.world.console_links.iter().map(|(id, l)| {
            let spec = LinkSpec {
                a: Endpoint { node: name_of(l.a.0), interface: l.a.1.clone() },
                b: Endpoint { node: name_of(l.b.0), interface: l.b.1.clone() },
                cable: CableKind::Console,
            };
            (*id, spec)
        }));
        links.sort_by_key(|(id, _)| *id);
        LabSpec {
            name: self.world.name.clone(),
            description: self.world.description.clone(),
            nodes,
            links: links.into_iter().map(|(_, spec)| spec).collect(),
            notes: self.notes.clone(),
            flows: self.world.flows.values().map(|f| FlowSpec { src: name_of(f.src), ..f.spec.clone() }).collect(),
            seed: self.world.seed,
        }
    }

    pub fn name(&self) -> &str {
        &self.world.name
    }

    pub fn description(&self) -> &str {
        &self.world.description
    }

    pub fn set_description(&mut self, description: &str) {
        self.world.description = description.to_string();
    }

    // ------------------------------------------------------------ editing

    /// Add a device of a catalog model. Without a name one is chosen (`R1`, `SW2`, ...).
    pub fn add_node(&mut self, model: &str, name: Option<&str>, x: f32, y: f32) -> Result<NodeId, LabError> {
        let id = self.world.add_node(model, name, x, y)?;
        self.pump();
        Ok(id)
    }

    /// Remove a device together with its links, flows, faults and consoles.
    pub fn remove_node(&mut self, id: NodeId) -> Result<(), LabError> {
        self.world.remove_node(id)?;
        let orphaned: Vec<ConsoleId> =
            self.consoles.iter().filter(|(_, c)| c.root_node() == Some(id)).map(|(cid, _)| *cid).collect();
        for console in orphaned {
            self.console_close(console);
        }
        self.pump();
        Ok(())
    }

    /// Change the name shown on the canvas. End hosts take it as their
    /// hostname; a network device keeps the hostname in its configuration.
    pub fn rename_node(&mut self, id: NodeId, name: &str) -> Result<(), LabError> {
        self.world.rename_node(id, name)
    }

    pub fn move_node(&mut self, id: NodeId, x: f32, y: f32) {
        if let Some(node) = self.world.nodes.get_mut(&id) {
            node.x = x;
            node.y = y;
        }
    }

    /// Power off: the running configuration and all learned state are lost.
    /// Power on: the device boots from its startup configuration.
    pub fn set_power(&mut self, id: NodeId, on: bool) {
        self.world.set_power(id, on);
        self.pump();
    }

    /// Replace an end host's addressing and services.
    pub fn set_host_config(&mut self, id: NodeId, cfg: HostConfig) -> Result<(), LabError> {
        let node = self.world.nodes.get_mut(&id).ok_or(LabError::UnknownNode(id))?;
        if node.model.is_network_os() {
            return Err(LabError::NotAHost(node.name.clone()));
        }
        hostcfg::validate(node.model, &cfg)?;
        node.host = Some(cfg);
        if node.powered {
            hostcfg::apply(node);
        }
        self.world.mark_dirty(id);
        self.world.touch();
        self.world.reconcile();
        self.pump();
        Ok(())
    }

    pub fn host_config(&self, id: NodeId) -> Option<HostConfig> {
        self.world.nodes.get(&id).and_then(|n| n.host.clone())
    }

    /// Cable two interfaces. Interface names may be abbreviated (`g0/1`).
    pub fn add_link(&mut self, a: NodeId, a_if: &str, b: NodeId, b_if: &str, cable: CableKind) -> Result<LinkId, LabError> {
        let id = self.world.add_link(a, a_if, b, b_if, cable)?;
        self.pump();
        Ok(id)
    }

    pub fn remove_link(&mut self, id: LinkId) -> Result<(), LabError> {
        self.world.remove_link(id)?;
        self.pump();
        Ok(())
    }

    /// Full names of the node's interfaces that have no cable.
    pub fn free_interfaces(&self, id: NodeId) -> Vec<String> {
        let Some(node) = self.world.nodes.get(&id) else { return Vec::new() };
        node.model
            .ports
            .iter()
            .filter(|port| node.ifs.get(port).is_some_and(|s| s.link.is_none()))
            .map(|port| port.name())
            .collect()
    }

    pub fn node_by_name(&self, name: &str) -> Option<NodeId> {
        self.world.node_by_name(name)
    }

    /// The node that owns an address (on an up interface, or else configured).
    pub fn node_by_ip(&self, ip: Ipv4Addr) -> Option<NodeId> {
        self.world.node_by_ip(ip)
    }

    pub fn set_notes(&mut self, notes: Vec<NoteSpec>) {
        self.notes = notes;
    }

    // --------------------------------------------------------------- time

    pub fn now(&self) -> SimTime {
        self.world.now
    }

    /// Run the simulation forward by `micros`. Cheap when idle.
    pub fn advance(&mut self, micros: u64) {
        self.world.run_until(self.world.now.saturating_add(micros));
        self.pump();
    }

    /// Advance until the control plane is quiescent (spanning tree, routing
    /// protocols and DHCP have settled), bounded by `max_micros`.
    pub fn settle(&mut self, max_micros: u64) {
        let deadline = self.world.now.saturating_add(max_micros);
        while self.world.now < deadline {
            let quiet = self.world.now.saturating_sub(self.world.last_cp_change);
            if quiet >= QUIET_MICROS && !self.world.converging() {
                break;
            }
            self.world.run_until((self.world.now + SETTLE_STEP_MICROS).min(deadline));
        }
        self.pump();
    }

    /// Use real-world protocol timers (30 s spanning-tree forward delay, 40 s
    /// OSPF wait) instead of the short defaults.
    pub fn set_realistic_timers(&mut self, realistic: bool) {
        self.world.realistic = realistic;
    }

    // ----------------------------------------------------------- consoles

    /// Open the device's console line. The returned stream behaves like a
    /// serial console: raw bytes in, raw bytes (with `\r\n` line ends) out.
    pub fn console_open(&mut self, node: NodeId) -> Result<ConsoleId, LabError> {
        let console = Console::open(&mut self.world, node)?;
        let id = self.next_console;
        self.next_console += 1;
        self.consoles.insert(id, console);
        Ok(id)
    }

    pub fn console_input(&mut self, console: ConsoleId, data: &[u8]) {
        if let Some(c) = self.consoles.get_mut(&console) {
            c.input(&mut self.world, data);
        }
        self.pump();
    }

    /// Drain the console's pending output.
    pub fn console_output(&mut self, console: ConsoleId) -> Vec<u8> {
        self.consoles.get_mut(&console).map(Console::take_output).unwrap_or_default()
    }

    pub fn console_resize(&mut self, console: ConsoleId, cols: u16, rows: u16) {
        if let Some(c) = self.consoles.get_mut(&console) {
            c.cols = cols;
            c.rows = rows;
        }
    }

    pub fn console_close(&mut self, console: ConsoleId) {
        if let Some(mut c) = self.consoles.remove(&console) {
            c.close(&mut self.world);
        }
    }

    /// Let every console show what happened since it last looked.
    fn pump(&mut self) {
        for console in self.consoles.values_mut() {
            console.pump(&mut self.world);
        }
        if self.world.events.len() > MAX_QUEUED_EVENTS {
            // Nobody is draining events: keep the most recent ones.
            let excess = self.world.events.len() - MAX_QUEUED_EVENTS / 2;
            self.world.events.drain(..excess);
        }
    }

    // --------------------------------------------------------- automation

    fn cli_node(&self, node: NodeId) -> Result<&Node, LabError> {
        let n = self.world.nodes.get(&node).ok_or(LabError::UnknownNode(node))?;
        if n.model.cli == CliKind::None {
            return Err(LabError::NoCli(n.name.clone()));
        }
        if !n.powered {
            return Err(LabError::PoweredOff(n.name.clone()));
        }
        Ok(n)
    }

    /// Carry a shell reply through to the end: apply configuration changes,
    /// wait for processes, answer questions with their defaults.
    fn finish(&mut self, node: NodeId, shell: &mut Shell, mut reply: Reply, out: &mut String) -> bool {
        let mut error = false;
        for _ in 0..8 {
            if reply.touched {
                self.world.mark_dirty(node);
                self.world.touch();
                self.world.reconcile();
            }
            out.push_str(&reply.out);
            error |= reply.error;
            reply = match reply.next {
                Next::Prompt | Next::Close => break,
                Next::Job(proc) | Next::Connect { proc, .. } => {
                    out.push_str(&self.wait_for(node, proc));
                    break;
                }
                Next::Ask { tag, .. } => {
                    let answer = match tag {
                        Tag::ReloadSave => "no",
                        Tag::ClearOspf => "yes",
                        _ => "",
                    };
                    self.world.with_node(node, |n, fx| shell.answer(n, fx, tag, answer)).unwrap_or_default()
                }
                Next::Key { tag, .. } => self.world.with_node(node, |n, fx| shell.answer(n, fx, tag, "\n")).unwrap_or_default(),
                Next::Reload => {
                    self.world.boot(node);
                    self.world.touch();
                    self.world.reconcile();
                    break;
                }
            };
        }
        error
    }

    /// Advance simulated time until a process finishes; returns its output.
    fn wait_for(&mut self, node: NodeId, proc: u32) -> String {
        let limit = self.world.now.saturating_add(EXEC_LIMIT_MICROS);
        let running = |world: &World| world.nodes.get(&node).and_then(|n| n.procs.get(proc)).is_some_and(|p| !p.done);
        while running(&self.world) {
            match self.world.next_event_at() {
                Some(at) if at <= limit => self.world.run_until(at),
                _ => break,
            }
        }
        let Some(n) = self.world.nodes.get_mut(&node) else { return String::new() };
        n.proc_abort(proc);
        let text = n.procs.get(proc).map(|p| p.out.clone()).unwrap_or_default();
        n.procs.remove(proc);
        text
    }

    /// Run one command and return its output (`\n` line ends). On a network
    /// device the command runs in privileged EXEC mode with paging off; on an
    /// end host it runs in the host shell. Long-running commands (ping,
    /// traceroute) run to completion by advancing simulated time. A command the
    /// device rejects is not an `Err`: the device's error text is the output.
    pub fn exec(&mut self, node: NodeId, command: &str) -> Result<String, LabError> {
        let mut shell = match self.cli_node(node)?.model.cli {
            CliKind::Ios => Shell::Ios(IosShell::automation(Mode::Priv)),
            _ => Shell::Host(HostShell::new()),
        };
        let reply = self.world.with_node(node, |n, fx| shell.execute(n, fx, command)).unwrap_or_default();
        let mut out = String::new();
        self.finish(node, &mut shell, reply, &mut out);
        self.pump();
        Ok(out)
    }

    /// Apply configuration lines in global configuration mode, as if typed
    /// after `configure terminal`. Every line is attempted; `ok` is false if
    /// any was rejected.
    pub fn configure(&mut self, node: NodeId, lines: &[String]) -> Result<ConfigOutcome, LabError> {
        let n = self.cli_node(node)?;
        if n.model.cli != CliKind::Ios {
            return Err(LabError::NoCli(n.name.clone()));
        }
        let mut shell = Shell::Ios(IosShell::automation(Mode::Config));
        let mut outcome = ConfigOutcome { lines: Vec::new(), ok: true };
        for line in lines {
            let reply = self.world.with_node(node, |n, fx| shell.execute(n, fx, line)).unwrap_or_default();
            let mut output = String::new();
            let error = self.finish(node, &mut shell, reply, &mut output);
            outcome.ok &= !error;
            outcome.lines.push(ConfigLineResult { line: line.clone(), output, error });
        }
        if let Shell::Ios(ios) = &mut shell
            && ios.in_config()
        {
            self.world.with_node(node, |n, fx| ios.leave_config(n, fx));
        }
        self.pump();
        Ok(outcome)
    }

    /// The running configuration, from the first `!` to `end`.
    pub fn running_config(&self, node: NodeId) -> Result<String, LabError> {
        let n = self.cli_node(node)?;
        if n.model.cli != CliKind::Ios {
            return Err(LabError::NoCli(n.name.clone()));
        }
        Ok(runcfg::render(n))
    }

    /// The saved configuration; empty if the device was never saved.
    pub fn startup_config(&self, node: NodeId) -> Result<String, LabError> {
        let n = self.world.nodes.get(&node).ok_or(LabError::UnknownNode(node))?;
        if n.model.cli != CliKind::Ios {
            return Err(LabError::NoCli(n.name.clone()));
        }
        Ok(n.startup.clone().unwrap_or_default())
    }

    /// Startup-configuration lines the device rejected when it last booted,
    /// each with the device's response.
    pub fn startup_errors(&self, node: NodeId) -> Vec<(String, String)> {
        self.world.nodes.get(&node).map(|n| n.boot_errors.clone()).unwrap_or_default()
    }

    // --------------------------------------------------------- inspection

    pub fn snapshot(&self) -> LabSnapshot {
        views::snapshot(&self.world, &self.notes)
    }

    pub fn node_tables(&self, node: NodeId) -> Result<NodeTables, LabError> {
        let n = self.world.nodes.get(&node).ok_or(LabError::UnknownNode(node))?;
        Ok(views::tables(n, self.world.now))
    }

    pub fn metrics(&self, node: NodeId) -> Result<NodeMetrics, LabError> {
        let n = self.world.nodes.get(&node).ok_or(LabError::UnknownNode(node))?;
        Ok(views::metrics(n, self.world.now, self.world.seed))
    }

    // ------------------------------------------------------ instant probes

    /// Can `from` ping `dst` right now? No simulated time passes and nothing
    /// in the lab changes (ARP caches, counters and logs are untouched).
    pub fn probe_ping(&mut self, from: NodeId, dst: Ipv4Addr) -> ProbeResult {
        self.world.probe(from, dst, PduKind::Icmp, false).into_result(&self.world)
    }

    /// Can `from` open a TCP connection to `dst:port` right now?
    pub fn probe_tcp(&mut self, from: NodeId, dst: Ipv4Addr, port: u16) -> ProbeResult {
        self.world.probe(from, dst, PduKind::Tcp { port }, false).into_result(&self.world)
    }

    // ----------------------------------------------------- simulation mode

    /// Record a `PacketEvent` for every hop of every frame.
    pub fn set_capture(&mut self, on: bool) {
        self.world.capture = on;
    }

    /// Inject one PDU and trace it (and the ARP and replies it causes). Its
    /// events carry the returned id whether or not capture is on, and a
    /// `TraceDone` event reports how it ended.
    pub fn start_trace(&mut self, from: NodeId, dst: Ipv4Addr, pdu: PduKind) -> Result<TraceId, LabError> {
        let trace = self.world.start_trace(from, dst, pdu)?;
        self.pump();
        Ok(trace)
    }

    /// Drain the events produced since the last call.
    pub fn take_events(&mut self) -> Vec<LabEvent> {
        let mut events = std::mem::take(&mut self.world.events);
        let changed = std::mem::take(&mut self.world.changed);
        events.extend(changed.into_iter().filter(|id| self.world.nodes.contains_key(id)).map(|node| LabEvent::NodeChanged { node }));
        events
    }

    // -------------------------------------------------- traffic and faults

    pub fn add_flow(&mut self, flow: FlowSpec) -> Result<FlowId, LabError> {
        let src = self.world.node_by_name(&flow.src).ok_or_else(|| LabError::UnknownNodeName(flow.src.clone()))?;
        let dst = parse_ip(flow.dst_ip.trim()).ok_or_else(|| LabError::InvalidAddress(flow.dst_ip.clone()))?;
        let id = self.world.next_flow;
        self.world.next_flow += 1;
        self.world.flows.insert(id, FlowRuntime::new(flow, src, dst, self.world.now));
        self.world.arm_flow_tick();
        Ok(id)
    }

    pub fn remove_flow(&mut self, id: FlowId) {
        self.world.flows.remove(&id);
    }

    pub fn flows(&self) -> Vec<(FlowId, FlowSpec, FlowState)> {
        self.world
            .flows
            .iter()
            .map(|(id, flow)| {
                let name = self.world.nodes.get(&flow.src).map(|n| n.name.clone()).unwrap_or_default();
                (*id, FlowSpec { src: name, ..flow.spec.clone() }, flow.state(&self.world))
            })
            .collect()
    }

    pub fn inject(&mut self, fault: Fault) -> Result<FaultId, LabError> {
        let id = self.world.inject(fault)?;
        self.pump();
        Ok(id)
    }

    pub fn clear_fault(&mut self, id: FaultId) {
        self.world.clear_fault(id);
        self.pump();
    }

    pub fn faults(&self) -> Vec<(FaultId, Fault)> {
        self.world.faults.iter().map(|(id, fault)| (*id, fault.clone())).collect()
    }
}
