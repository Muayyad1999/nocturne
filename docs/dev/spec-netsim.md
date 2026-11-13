# Spec: `nocturne-netsim` — the lab simulator

`nocturne-netsim` is the engine behind Nocturne's **Lab**: a Packet-Tracer-style
network simulator. Users drag routers, switches and hosts onto a canvas, cable
them, and configure them through a realistic industry-standard ("IOS-style")
CLI. The same simulated devices are also registered in the NOC inventory, so
every other feature (terminal, discovery, monitoring, backups, change pipeline,
AI agent) can be exercised end to end without touching real hardware.

That second use is why fidelity matters: Nocturne's *real* device driver logs in
to a simulated device over an in-process transport and treats it exactly like a
physical switch — prompt detection, `terminal length 0`, `show` output parsed
with the public ntc-templates TextFSM library, `configure terminal`, error
detection, `write memory`.

## Hard constraints

* **Pure, synchronous, deterministic.** No threads, no tokio, no wall clock, no
  file or network IO. All randomness comes from one seeded PRNG stored in the
  lab (write a tiny xorshift; do not add a `rand` dependency). Same inputs ⇒
  byte-identical outputs. The host drives time with `Lab::advance`.
* **`Lab: Send`.** The host wraps it in an actor. No `Rc`/`RefCell` in the public
  surface (index-based arenas internally are ideal).
* **Never panics** on any console input or API call sequence: no `unwrap` on
  user-controlled data, no slicing by unchecked indices, checked arithmetic on
  parsed numbers. There is a fuzz-style test (below) that enforces this.
* Dependencies: `serde`, `serde_json`, `indexmap`, `thiserror` only (already in
  `Cargo.toml`). Ask before adding anything else — prefer writing the 30 lines.
* No trademarks in emitted text: the CLI is "IOS-style", the software banner is
  Nocturne's own (see *show version*).

## Public API (contract — the app crate is written against this)

Everything below is re-exported from the crate root. Names and shapes are fixed;
you may add fields/variants/helpers, but do not rename or remove.

```rust
pub type NodeId = u32;      // stable for the life of the lab, never reused
pub type LinkId = u32;
pub type ConsoleId = u32;
pub type FlowId = u32;
pub type FaultId = u32;
pub type TraceId = u32;
/// Simulated time in microseconds since the lab was created.
pub type SimTime = u64;

// ---------------------------------------------------------------- lab file
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct LabSpec {
    pub name: String,
    pub description: String,
    pub nodes: Vec<NodeSpec>,
    pub links: Vec<LinkSpec>,
    pub notes: Vec<NoteSpec>,
    pub flows: Vec<FlowSpec>,
    pub seed: u64,
}
#[serde(default)]
pub struct NodeSpec {
    pub name: String,               // unique; also the initial hostname
    pub model: String,              // key into `catalog()`
    pub x: f32, pub y: f32,
    pub powered: bool,              // default true
    pub startup_config: Option<String>,  // network devices: CLI config text
    pub host: Option<HostConfig>,        // end hosts
}
#[serde(default)]
pub struct HostConfig {
    pub dhcp: bool,
    pub ip: Option<String>,         // "192.168.1.10"
    pub mask: Option<String>,       // "255.255.255.0"
    pub gateway: Option<String>,
    pub dns: Option<String>,
    pub extra_ips: Vec<String>,     // "8.8.8.8/32" style extra addresses (for `internet`)
    pub services: Vec<String>,      // "http", "dns", "dhcp", "tftp", "syslog", "ntp", "ssh", "telnet"
}
pub struct LinkSpec { pub a: Endpoint, pub b: Endpoint, pub cable: CableKind }
pub struct Endpoint { pub node: String, pub interface: String }   // by node *name*
pub enum CableKind { Auto, Copper, Fiber, Console }                // serde: lowercase
pub struct NoteSpec { pub x: f32, pub y: f32, pub w: f32, pub h: f32, pub text: String, pub color: String }
pub struct FlowSpec {             // background traffic that moves interface counters
    pub name: String,
    pub src: String,              // node name
    pub dst_ip: String,
    pub bps: u64,                 // average rate
    pub pattern: FlowPattern,     // Constant | Sine { period_s } | Burst { on_s, off_s }
    pub enabled: bool,
}

// ---------------------------------------------------------------- catalog
pub struct ModelInfo {
    pub id: String,               // "router", "switch", "switch-l3", "pc", ...
    pub kind: NodeKind,
    pub label: String,            // "Router (4 × GE)"
    pub description: String,
    pub interfaces: Vec<String>,  // full interface names in display order
    pub cli: CliKind,             // Ios | Host | None
}
pub enum NodeKind { Router, Switch, L3Switch, Firewall, AccessPoint, Pc, Laptop, Server, Printer, Phone, Cloud, Nms }
pub fn catalog() -> Vec<ModelInfo>;

// ---------------------------------------------------------------- the lab
impl Lab {
    pub fn new(name: &str) -> Lab;
    pub fn from_spec(spec: &LabSpec) -> Result<Lab, LabError>;
    /// Round-trips: `Lab::from_spec(&lab.to_spec())` behaves identically.
    /// `startup_config` in the result is the device's *startup* config.
    pub fn to_spec(&self) -> LabSpec;
    pub fn name(&self) -> &str;

    // -- editing ----------------------------------------------------------
    pub fn add_node(&mut self, model: &str, name: Option<&str>, x: f32, y: f32) -> Result<NodeId, LabError>;
    pub fn remove_node(&mut self, id: NodeId) -> Result<(), LabError>;      // also removes its links
    pub fn rename_node(&mut self, id: NodeId, name: &str) -> Result<(), LabError>;
    pub fn move_node(&mut self, id: NodeId, x: f32, y: f32);
    pub fn set_power(&mut self, id: NodeId, on: bool);                     // off: running config lost, on: boots startup
    pub fn set_host_config(&mut self, id: NodeId, cfg: HostConfig) -> Result<(), LabError>;
    pub fn host_config(&self, id: NodeId) -> Option<HostConfig>;
    pub fn add_link(&mut self, a: NodeId, a_if: &str, b: NodeId, b_if: &str, cable: CableKind) -> Result<LinkId, LabError>;
    pub fn remove_link(&mut self, id: LinkId) -> Result<(), LabError>;
    pub fn free_interfaces(&self, id: NodeId) -> Vec<String>;
    pub fn node_by_name(&self, name: &str) -> Option<NodeId>;
    pub fn node_by_ip(&self, ip: std::net::Ipv4Addr) -> Option<NodeId>;
    pub fn set_notes(&mut self, notes: Vec<NoteSpec>);

    // -- time -------------------------------------------------------------
    pub fn now(&self) -> SimTime;
    /// Run the simulation forward by `micros`. Cheap when idle.
    pub fn advance(&mut self, micros: u64);
    /// Advance until the control plane is quiescent (STP/OSPF/DHCP settled),
    /// bounded by `max_micros`. Used after loading a lab and by tests.
    pub fn settle(&mut self, max_micros: u64);

    // -- interactive consoles (raw terminal bytes, like a serial console) --
    pub fn console_open(&mut self, node: NodeId) -> Result<ConsoleId, LabError>;
    pub fn console_input(&mut self, console: ConsoleId, data: &[u8]);
    pub fn console_output(&mut self, console: ConsoleId) -> Vec<u8>;       // drains pending output
    pub fn console_resize(&mut self, console: ConsoleId, cols: u16, rows: u16);
    pub fn console_close(&mut self, console: ConsoleId);

    // -- automation helpers (no terminal emulation) -------------------------
    /// Run one privileged-EXEC command with paging off and return its output.
    /// Long-running commands (ping, traceroute) are run to completion by
    /// advancing simulated time internally.
    pub fn exec(&mut self, node: NodeId, command: &str) -> Result<String, LabError>;
    /// Apply configuration lines in global config mode.
    pub fn configure(&mut self, node: NodeId, lines: &[String]) -> Result<ConfigOutcome, LabError>;
    pub fn running_config(&self, node: NodeId) -> Result<String, LabError>;
    pub fn startup_config(&self, node: NodeId) -> Result<String, LabError>;

    // -- inspection ---------------------------------------------------------
    pub fn snapshot(&self) -> LabSnapshot;
    pub fn node_tables(&self, node: NodeId) -> Result<NodeTables, LabError>;
    pub fn metrics(&self, node: NodeId) -> Result<NodeMetrics, LabError>;

    // -- instant probes (no simulated time passes, no trace events) ---------
    pub fn probe_ping(&mut self, from: NodeId, dst: std::net::Ipv4Addr) -> ProbeResult;
    pub fn probe_tcp(&mut self, from: NodeId, dst: std::net::Ipv4Addr, port: u16) -> ProbeResult;

    // -- simulation mode ------------------------------------------------------
    pub fn set_capture(&mut self, on: bool);          // record PacketEvents
    pub fn start_trace(&mut self, from: NodeId, dst: std::net::Ipv4Addr, pdu: PduKind) -> Result<TraceId, LabError>;
    pub fn take_events(&mut self) -> Vec<LabEvent>;   // drains

    // -- traffic and faults ---------------------------------------------------
    pub fn add_flow(&mut self, flow: FlowSpec) -> Result<FlowId, LabError>;
    pub fn remove_flow(&mut self, id: FlowId);
    pub fn flows(&self) -> Vec<(FlowId, FlowSpec, FlowState)>;
    pub fn inject(&mut self, fault: Fault) -> Result<FaultId, LabError>;
    pub fn clear_fault(&mut self, id: FaultId);
    pub fn faults(&self) -> Vec<(FaultId, Fault)>;
}

pub struct ConfigOutcome { pub lines: Vec<ConfigLineResult>, pub ok: bool }
pub struct ConfigLineResult { pub line: String, pub output: String, pub error: bool }

pub struct LabSnapshot {
    pub name: String, pub description: String, pub now: SimTime,
    pub nodes: Vec<NodeView>, pub links: Vec<LinkView>, pub notes: Vec<NoteSpec>,
}
pub struct NodeView {
    pub id: NodeId, pub name: String, pub hostname: String, pub model: String, pub kind: NodeKind,
    pub x: f32, pub y: f32, pub powered: bool, pub cli: CliKind,
    pub interfaces: Vec<IfView>,
    pub mgmt_ip: Option<String>,      // best address to manage the device on
}
pub struct IfView {
    pub name: String, pub short: String,          // "GigabitEthernet0/1", "Gi0/1"
    pub link: Option<LinkId>, pub admin_up: bool, pub oper_up: bool,
    pub light: Light,                              // Green | Amber | Red | Off
    pub ip: Option<String>,                        // "10.0.0.1/24"
    pub mode: String,                              // "access 10" | "trunk" | "routed" | "host" | ""
    pub description: String,
}
pub struct LinkView { pub id: LinkId, pub a: NodeId, pub a_if: String, pub b: NodeId, pub b_if: String,
                      pub cable: CableKind, pub up: bool, pub a_light: Light, pub b_light: Light,
                      pub bps_ab: u64, pub bps_ba: u64 }          // current simulated rate each way

pub struct NodeTables {           // all rows are plain structs with String/number fields, Serialize
    pub routes: Vec<RouteRow>,    // code ("C","S","O","O IA","R","S*"), prefix, ad, metric, next_hop, interface
    pub arp: Vec<ArpRow>,         // ip, mac, interface, age_s
    pub mac: Vec<MacRow>,         // vlan, mac, kind, port
    pub vlans: Vec<VlanRow>,      // id, name, status, ports
    pub stp: Vec<StpRow>,         // vlan, interface, role, state, cost, root_id, bridge_id, is_root
    pub neighbors: Vec<NeighborRow>, // protocol, local_if, device, remote_if, platform, mgmt_ip
    pub ospf_neighbors: Vec<OspfNeighborRow>,
    pub dhcp_bindings: Vec<DhcpRow>,
    pub nat: Vec<NatRow>,
}
pub struct NodeMetrics {
    pub hostname: String, pub uptime_s: u64, pub cpu_pct: f32,
    pub mem_used: u64, pub mem_total: u64, pub temperature_c: f32,
    pub interfaces: Vec<IfCounters>,
}
pub struct IfCounters {
    pub name: String, pub description: String, pub admin_up: bool, pub oper_up: bool,
    pub speed_bps: u64, pub mtu: u32,
    pub in_octets: u64, pub out_octets: u64, pub in_pkts: u64, pub out_pkts: u64,
    pub in_errors: u64, pub out_errors: u64, pub in_discards: u64, pub out_discards: u64,
    pub crc: u64, pub last_change: SimTime,
}

pub struct ProbeResult { pub ok: bool, pub rtt_us: u64, pub hops: Vec<ProbeHop>, pub reason: String }
pub struct ProbeHop { pub node: NodeId, pub name: String, pub in_if: String, pub out_if: String, pub note: String }
pub enum PduKind { Icmp, Tcp { port: u16 }, Udp { port: u16 } }

pub enum LabEvent {
    LinkState { link: LinkId, up: bool },
    Syslog { node: NodeId, time: SimTime, severity: u8, facility: String, mnemonic: String, message: String, line: String },
    Packet(PacketEvent),
    NodeChanged { node: NodeId },         // config or tables changed: host should refresh views
    TraceDone { trace: TraceId, ok: bool, summary: String },
}
pub struct PacketEvent {
    pub trace: Option<TraceId>, pub pdu: u64, pub seq: u32, pub time: SimTime,
    pub kind: PacketEventKind,            // Created | Forwarded | Received | Delivered | Dropped
    pub at: NodeId, pub from: Option<NodeId>, pub link: Option<LinkId>,
    pub protocol: String,                 // "ARP", "ICMP", "DHCP", "OSPF", "STP", "TCP", "UDP", "CDP"
    pub summary: String,                  // "ICMP echo request 10.0.0.2 → 10.0.1.5"
    pub layers: Vec<LayerInfo>,           // OSI-style breakdown for the inspector
    pub decision: String,                 // plain-English: what this device did and why
}
pub struct LayerInfo { pub layer: u8, pub title: String, pub lines: Vec<String> }

pub enum Fault {
    LinkDown { link: LinkId },
    InterfaceErrors { node: NodeId, interface: String, crc_per_s: u32 },
    PacketLoss { link: LinkId, percent: u8 },
    Flapping { link: LinkId, period_s: u32 },
    HighCpu { node: NodeId, percent: u8 },
    PowerOff { node: NodeId },
    DuplexMismatch { link: LinkId },
}
```

All public structs derive `Serialize, Deserialize, Clone, Debug` (enums as
`snake_case` strings / externally tagged with `snake_case`), because the app
forwards them straight to the UI as JSON.

## Model catalog (minimum)

| id | kind | interfaces | notes |
|---|---|---|---|
| `router` | Router | `GigabitEthernet0/0`–`0/3` | routed ports only |
| `router-8` | Router | `GigabitEthernet0/0`–`0/7` | |
| `switch` | Switch | `FastEthernet0/1`–`0/24`, `GigabitEthernet0/1`–`0/2` | L2 only, mgmt via `interface Vlan1` + `ip default-gateway` |
| `switch-l3` | L3Switch | `GigabitEthernet1/0/1`–`1/0/24`, `TenGigabitEthernet1/1/1`–`1/1/4` | `ip routing`, SVIs, `no switchport` |
| `firewall` | Firewall | `GigabitEthernet0/0`–`0/3` | router personality (ACL/NAT focus), different icon |
| `ap` | AccessPoint | `GigabitEthernet0` + `Dot11Radio0` | transparent bridge; no CLI |
| `pc`, `laptop`, `printer`, `phone` | hosts | `Ethernet0` | host shell (not printer/phone) |
| `server` | Server | `Ethernet0` | host shell + services |
| `cloud` | Cloud | `Ethernet0`–`Ethernet3` | "the Internet": answers for its `extra_ips`, routes between its ports |
| `nms` | Nms | `Ethernet0` | the Nocturne management station itself |

## Dataplane (what must really be simulated)

Frames are simulated hop by hop through an event queue (link latency ~ tens of
microseconds; devices process in ~1 µs steps). The *same forwarding functions*
must back both the timed simulation and the instant `probe_*` walkers, so the
two can never disagree.

**Layer 1/2**
* Link state: up only if both nodes powered, both interfaces `no shutdown`, no
  fault. Speed/duplex config; a `DuplexMismatch` fault produces late-collision
  style input errors under load.
* Ethernet switching with per-VLAN MAC learning and aging (300 s), flooding of
  unknown unicast/broadcast, access ports, 802.1Q trunks (`allowed vlan` list
  incl. `add`/`remove`/`except`/`all`/`none`, native VLAN, VLAN must exist and
  be active to forward), `switchport mode access|trunk|dynamic auto|desirable`
  with a simple DTP outcome, `switchport nonegotiate`.
* Spanning tree (PVST-style, per VLAN): root election by priority+MAC, path
  cost by speed, root/designated/alternate roles, blocking of redundant links,
  `spanning-tree vlan N priority|root primary|root secondary`, `portfast`,
  `bpduguard` (err-disable). New links go Amber (listening/learning) before
  Green; make the forward delay short by default (2 s total) with
  `Lab::set_realistic_timers(bool)` to opt into 30 s.
* EtherChannel: `channel-group N mode on|active|passive|desirable|auto` →
  `Port-channelN`; bundle forms only if both sides are compatible; STP and
  switchport config live on the Port-channel.
* Port security (max MACs, sticky, violation shutdown/restrict/protect) with
  `err-disabled` state and `errdisable recovery` not required (manual
  `shutdown`/`no shutdown` recovers).
* CDP and LLDP neighbor tables (CDP on by default, `lldp run` to enable LLDP;
  `no cdp run`, per-interface `no cdp enable`).

**Layer 3**
* IPv4 only (accept and store IPv6 config lines so configs round-trip, but do
  not forward IPv6).
* ARP with cache + timeout; the first packet to an unresolved next hop is
  dropped while ARP resolves (so the first `ping` shows `.!!!!`). Proxy ARP off.
* Routed interfaces, loopbacks, 802.1Q subinterfaces (`encapsulation dot1Q N
  [native]`), SVIs (`interface VlanN`, up only when the VLAN exists and has an
  up port in it), L3 switch `ip routing` / `no switchport`.
* Routing table with longest-prefix match, administrative distance, recursive
  static routes, `ip route … <next-hop|interface> [distance]`, default route,
  floating statics, `ip default-gateway` for L2 switches and hosts.
* OSPFv2: `router ospf <pid>`, `router-id`, `network … area`, `passive-interface
  [default]`, `default-information originate [always]`, `redistribute static|
  connected [subnets]`, interface `ip ospf cost|priority|hello-interval|
  dead-interval`, `ip ospf <pid> area <a>`, reference bandwidth; multi-area with
  inter-area (`O IA`) routes through ABRs. Adjacency requires L2 adjacency on
  the same subnet, matching area/timers/MTU and a non-passive interface.
  Computing adjacencies + SPF analytically on every topology/config change is
  fine (you do not need to exchange real LSAs), but neighbor state must move
  through timed states so `show ip ospf neighbor` and the `%OSPF-5-ADJCHG`
  syslog look right and dead-interval expiry takes time.
* RIP v2 (`router rip`, `version 2`, `network`, `no auto-summary`,
  `passive-interface`, `default-information originate`).
* Static + OSPF + RIP coexist with correct administrative distances.
* ACLs: numbered and named, standard and extended (`permit|deny
  ip|icmp|tcp|udp`, `host`, `any`, wildcard masks, `eq|gt|lt|range` ports,
  `established`, `log`), sequence numbers, hit counters, `ip access-group …
  in|out`, `access-class` on vty lines, implicit deny.
* NAT: `ip nat inside|outside`, static `ip nat inside source static`, dynamic
  pool, PAT `overload` on an interface, translation table with timeouts.
* DHCP: server pools (`network`, `default-router`, `dns-server`, `lease`,
  `ip dhcp excluded-address`), bindings; `ip helper-address` relay; DHCP client
  on hosts and on router interfaces (`ip address dhcp`).
* ICMP echo, unreachable (net/host/admin-prohibited), TTL exceeded; TTL
  decrement; traceroute works (UDP probes on network devices, ICMP on hosts).
* HSRP v1/v2 basics (`standby <g> ip|priority|preempt`): virtual IP/MAC,
  active/standby election, failover.
* TCP is modelled only as "can a SYN reach an open port and the SYN-ACK get
  back" (for `probe_tcp`, `telnet`/`ssh` from a host or device to a vty, ACL
  `established`).

**Flows.** A `FlowSpec` does not emit packets. Each tick the simulator resolves
(and caches until the topology/config changes) the forwarding path with the
instant walker and adds `rate × dt` to every interface counter on the path,
both directions (assume 10 % return traffic). Broken path ⇒ counters stop and
the dropping interface counts discards. `LinkView::bps_*` and
`show interfaces` 5-minute rates derive from this.

**Device health.** CPU = small seeded noise + a term proportional to forwarded
traffic + `HighCpu` faults; memory mostly static; `uptime` = time since boot.

## CLI: line discipline

The console is a raw byte stream, exactly like a serial console or an SSH shell:
* Echo printable characters; `\r` or `\n` executes; backspace (`0x08`/`0x7f`)
  erases; `Ctrl-A`/`Ctrl-E`/`Ctrl-U`/`Ctrl-W`; left/right arrows; up/down
  history (per mode, 20 entries); `Ctrl-C` aborts the line and long-running
  commands; `Ctrl-Z` = `end`; `Ctrl-Shift-6` (0x1e) aborts ping/traceroute.
* `?` (immediate, no Enter) prints context help: at a word boundary the list of
  valid next words with descriptions; mid-word the matching completions; then
  the prompt and the partial line are redrawn. `Tab` completes a unique prefix.
* Any unambiguous abbreviation is accepted (`sh ip int br`, `conf t`, `int
  g0/1`, `wr`, `no sh`). Ambiguous → `% Ambiguous command:  "…"`. Unknown →
  the classic caret marker line plus `% Invalid input detected at '^' marker.`
  Incomplete → `% Incomplete command.`
* Interface names accept every usual short form and optional space
  (`g0/1`, `gi0/1`, `gig 0/1`, `GigabitEthernet0/1`, `fa0/1`, `te1/1/1`,
  `po1`, `vl10`, `lo0`) and are normalised to the full name.
* Output lines end with `\r\n`. Paging: with `terminal length N` > 0 (default
  24), long output stops at ` --More-- `; space = next page, Enter = next line,
  `q` = quit, and the `--More--` text is erased the way IOS does it
  (`\x08` × 9, spaces, `\x08` × 9). `terminal length 0` disables paging.
* Output modifiers: `| include`, `| exclude`, `| begin`, `| section`,
  `| count` (regex, case-sensitive like IOS).
* Prompts: `Router>`, `Router#`, `Router(config)#`, `Router(config-if)#`,
  `(config-if-range)#`, `(config-subif)#`, `(config-vlan)#`, `(config-line)#`,
  `(config-router)#`, `(dhcp-config)#`, `(config-std-nacl)#`,
  `(config-ext-nacl)#`.
* Console and vty sessions honour `enable secret/password`, `line con 0` /
  `line vty` `password` + `login` / `login local`, `username … privilege …
  secret|password`, `exec-timeout`. A fresh device has no passwords (console
  drops straight to `>`; `enable` succeeds without a password).
* Unsolicited log messages interleave on the console the way IOS does
  (`logging synchronous` re-prints the prompt+line).

## CLI: command coverage (IOS-style personality)

EXEC: `enable`, `disable`, `exit`/`logout`, `configure terminal`, `show …`,
`ping <ip> [repeat N] [timeout S] [size N] [source IF|IP]` (also the bare
extended form is not required), `traceroute <ip>`, `telnet <ip>`, `ssh -l user
<ip>` (opens a nested session on the target's vty if reachable and permitted),
`copy running-config startup-config`, `write [memory|erase]`, `erase
startup-config`, `reload` (confirm prompt), `clear arp-cache | mac
address-table dynamic | ip nat translation * | counters | ip ospf process |
access-list counters | port-security`, `terminal length|width|monitor`,
`clock set`, `debug`/`undebug all` (accepted), `do <exec cmd>` from any config
mode, `delete`/`dir` (tiny fake flash: `dir` lists `vlan.dat`,
`config.text`).

SHOW (formats must match real IOS closely enough for the ntc-templates
`cisco_ios_*` TextFSM templates to parse them — see *Validation*):
`running-config [interface X | | section …]`, `startup-config`, `version`,
`inventory`, `clock`, `users`, `history`, `flash:`, `ip interface brief`,
`interfaces [X]`, `interfaces status`, `interfaces description`,
`interfaces trunk`, `interfaces X switchport`, `interfaces counters [errors]`,
`ip interface [X]`, `vlan [brief|id N]`, `mac address-table [dynamic|vlan N|
interface X|address M]`, `arp`, `ip arp`, `ip route [prefix|static|ospf|
connected|summary]`, `ip protocols`, `ip ospf`, `ip ospf neighbor [detail]`,
`ip ospf interface [brief]`, `ip ospf database` (summary form), `ip rip
database`, `cdp neighbors [detail]`, `lldp neighbors [detail]`,
`spanning-tree [vlan N|summary|root|interface X]`, `etherchannel summary`,
`port-security [interface X|address]`, `access-lists`, `ip access-lists`,
`ip nat translations`, `ip nat statistics`, `ip dhcp binding|pool|conflict`,
`standby [brief]`, `logging`, `ntp status|associations`, `snmp`, `ssh`,
`ip ssh`, `line`, `processes cpu [history|sorted]`, `memory statistics`,
`environment` (temperature/fans/psu, simple), `errdisable recovery`,
`vtp status`, `privilege`, `sessions`, `protocols`, `controllers` (minimal),
`tech-support` (concatenation of the main shows).

CONFIG: `hostname`, `enable secret|password`, `service password-encryption`
(type 7 obfuscation, real algorithm), `service timestamps`, `no ip
domain-lookup`, `ip domain-name`, `ip name-server`, `username`, `banner
motd|login` (delimiter handling, multi-line), `line con 0|vty 0 4|vty 5 15`
(`password`, `login [local]`, `transport input`, `exec-timeout`, `logging
synchronous`, `access-class`), `crypto key generate rsa [modulus N]`, `ip ssh
version 2`, `ip routing`, `ip route`, `ip default-gateway`, `ip
default-network` (accept), `vlan N` (+`name`, `state`, ranges `vlan 10,20,30-35`),
`no vlan N`, `vtp mode|domain` (mode transparent/server/client stored and
shown; no propagation), `spanning-tree mode pvst|rapid-pvst`, `spanning-tree
vlan N priority|root`, `spanning-tree portfast default`, `interface X`,
`interface range X-Y, Z`, `default interface X`, interface subcommands
(`description`, `shutdown`, `ip address A M [secondary]`, `ip address dhcp`,
`ipv6 address` (stored), `speed`, `duplex`, `mtu`, `bandwidth`, `delay`,
`switchport …` (mode, access vlan, trunk encapsulation dot1q, trunk allowed
vlan, trunk native vlan, nonegotiate, voice vlan, port-security …), `no
switchport`, `channel-group`, `spanning-tree portfast|bpduguard|cost|
port-priority`, `encapsulation dot1Q`, `ip helper-address`, `ip access-group`,
`ip nat inside|outside`, `ip ospf …`, `standby …`, `cdp enable`, `lldp
transmit|receive`, `storm-control` (stored), `power inline` (stored),
`negotiation auto` (stored)), `router ospf|rip` + subcommands, `ip
access-list standard|extended NAME` + numbered `access-list`, `ip nat …`, `ip
dhcp pool` + `ip dhcp excluded-address`, `ip dhcp snooping` (stored), `logging
host|buffered|console|trap`, `ntp server`, `clock timezone`, `snmp-server
community|location|contact|host|enable traps`, `lldp run`, `cdp run`, `ip
http server`/`secure-server` (stored), `aaa new-model` + `aaa …` (stored),
`archive`, `errdisable recovery` (stored), `end`, `exit`, `do`.

**Unknown but plausible configuration** must not be silently accepted — reject
it with the caret error like a real device. (The change pipeline relies on this
to catch typos.) Lines that are accepted-and-stored must appear in
`show running-config`.

**`show running-config` round-trip is a hard requirement:** booting a device
from the text of its own `show running-config` must reproduce exactly the same
running-config, in real IOS ordering (version/services, hostname, boot,
enable, usernames, aaa, clock, ip settings, vlans (only in vlan-transparent
style when `vtp mode transparent`; otherwise VLANs live in `vlan.dat` and still
persist with the lab), spanning-tree, interfaces in natural order, routers,
`ip route`s, ACLs, NAT, snmp, banner, lines, ntp, `end`), `!` separators and
single-space indentation.

**show version** uses Nocturne's own identity but IOS layout, e.g.

```
Nocturne Virtual Software (NVOS-L3-UNIVERSALK9), Version 15.9(3)M, RELEASE SOFTWARE (fc1)
Lab device simulated by Nocturne - not a vendor image
ROM: Bootstrap program is NVOS boot loader

R1 uptime is 2 hours, 14 minutes
System returned to ROM by power-on
System image file is "flash:nvos-l3-universalk9.15.9-3.M.bin"

nocturne NV-ROUTER-4GE (revision 1.0) with 524288K/65536K bytes of memory.
Processor board ID NV00000101
4 Gigabit Ethernet interfaces
255K bytes of non-volatile configuration memory.
250880K bytes of flash memory at flash:.

Configuration register is 0x2102
```

Serial numbers / base MACs derive deterministically from the lab seed + node id
(locally administered MAC range `02:4e:43:…`).

## Host shell (pc / laptop / server / nms)

Prompt `C:\>`. Commands: `ipconfig [/all|/release|/renew]`, `ping <ip|name>
[-n N] [-t]`, `tracert <ip>`, `arp -a`, `nslookup <name>` (if a DNS service is
reachable; servers with the `dns` service answer for lab hostnames),
`telnet <ip>`, `ssh -l <user> <ip>`, `netstat`, `hostname`, `cls`, `help`,
`ip <addr> <mask> [gateway]` (quick static config), `dhcp`. Windows-style
output (`Reply from 10.0.0.1: bytes=32 time<1ms TTL=255`, `Request timed
out.`, `Destination host unreachable.`).

## Syslog

Devices emit IOS-format messages as `LabEvent::Syslog` and on the console:
`%LINK-3-UPDOWN`, `%LINEPROTO-5-UPDOWN`, `%LINK-5-CHANGED` (administratively
down), `%SYS-5-CONFIG_I` (on leaving config mode: `Configured from console by
console`), `%SYS-5-RESTART`, `%OSPF-5-ADJCHG`, `%SPANTREE-2-BLOCK_BPDUGUARD`,
`%PM-4-ERR_DISABLE`, `%PORT_SECURITY-2-PSECURE_VIOLATION`, `%DHCPD-4-PING_CONFLICT`
(not required), `%HSRP-5-STATECHANGE`, `%SEC-6-IPACCESSLOGP` (ACL `log`),
`%CDP-4-NATIVE_VLAN_MISMATCH`, `%CDP-4-DUPLEX_MISMATCH`. `line` is the fully
formatted message (`*Mar  1 00:02:11.003: %LINK-3-UPDOWN: Interface
GigabitEthernet0/1, changed state to up`).

## Packet tracing (simulation mode)

With capture on, every hop of every frame yields a `PacketEvent` with an
OSI-style layer breakdown and a plain-English `decision`, e.g. *"The switch
looks up 0050.7966.6801 in the MAC table for VLAN 10: not found, so it floods
the frame out of every other port in VLAN 10 (Fa0/2, Gi0/1)."* or *"The
router's inbound ACL 101 on Gi0/0 matches line 10 `deny icmp any any`: the
packet is dropped."* These explanations are a product feature — make them
genuinely instructive. `start_trace` injects one PDU and its events carry the
`TraceId`; `TraceDone` reports success or the reason it died.

## Validation you must do

1. Unit + scenario tests (`cargo test -p nocturne-netsim`), at least:
   two hosts on a switch (first ping `.!!!!` then `!!!!!`); VLAN isolation and
   trunking incl. native VLAN and allowed-list pruning; router-on-a-stick;
   L3-switch SVIs; three-router static routing + traceroute hop list; OSPF
   triangle incl. link failure reconvergence and multi-area; RIP; STP triangle
   (one blocked port, root by priority, failover); EtherChannel; DHCP direct
   and via relay; NAT overload to the `cloud`; standard + extended ACLs with
   counters; HSRP failover; port security err-disable; CDP/LLDP tables;
   power-cycle keeps startup only; flows move counters and stop on a cut;
   every `Fault`; `to_spec`/`from_spec` round-trip; running-config round-trip
   for a config that touches every supported feature; determinism (two runs,
   identical transcripts and event logs).
2. **Fuzz-style robustness test:** feed several hundred thousand
   pseudo-random bytes and several thousand random-but-plausible command lines
   (random words from the grammar with random args) to consoles in every mode;
   no panics, no unbounded output, lab invariants still hold.
3. **Real-parser validation:** `pip install textfsm` and use the templates in
   `/home/user/.cache/vendor/ntc-templates/ntc_templates/templates/` to parse
   your output for: `show ip interface brief`, `show interfaces`, `show
   interfaces status`, `show interfaces switchport`, `show interfaces trunk`
   (if a template exists), `show vlan`, `show mac address-table`, `show ip
   arp`, `show ip route`, `show cdp neighbors`, `show cdp neighbors detail`,
   `show lldp neighbors`, `show lldp neighbors detail`, `show spanning-tree`,
   `show etherchannel summary`, `show ip ospf neighbor`, `show inventory`,
   `show access-lists`, `show ip dhcp binding` (where templates exist; they
   are named `cisco_ios_<command with underscores>.textfsm`). Every one must
   parse into sensible, complete records. Keep the harness as
   `crates/nocturne-netsim/tests/ntc_validate.py` plus a small Rust example
   binary (`examples/dump.rs`) that builds a reference lab and prints the
   command outputs for it; record the final results in your report. `show
   version` is exempt (own banner).
4. A reference lab builder `pub fn demo_campus() -> LabSpec` (in a `demo`
   module): one `cloud`, an edge router doing NAT + default route, two L3
   distribution switches running OSPF + HSRP for the user VLANs, three access
   switches with redundant uplinks (STP blocking visible), VLANs 10 (Staff),
   20 (Guest), 30 (Voice), 99 (Management), a server, ~6 hosts on DHCP, an
   `nms` in VLAN 99, every network device with a management SVI/loopback, vty
   `login local` + `username admin privilege 15 secret nocturne`, and three
   background flows. After `settle`, every host can reach `8.8.8.8` on the
   cloud and the `nms` can `probe_ping` every device's `mgmt_ip`. Also ship
   five small teaching labs as `pub fn examples() -> Vec<LabSpec>` (basic
   switching; VLANs + trunk; router-on-a-stick; OSPF triangle; DHCP + NAT).

## Suggested module layout (not binding)

`lib.rs` (public types) · `lab.rs` (arena, links, time, events) · `dataplane/`
(`ethernet.rs`, `switching.rs`, `stp.rs`, `ip.rs`, `arp.rs`, `routing.rs`,
`ospf.rs`, `rip.rs`, `acl.rs`, `nat.rs`, `dhcp.rs`, `hsrp.rs`, `walker.rs`) ·
`device/` (`model.rs` config model, `ios/` (`parser.rs`, `exec.rs`,
`config.rs`, `show/*.rs`, `runcfg.rs`, `help.rs`), `host.rs`, `line.rs` line
editor) · `flows.rs` · `faults.rs` · `demo.rs`.

Keep the device configuration model separate from the IOS command front end:
a RouterOS-style personality will be added on the same model later.
