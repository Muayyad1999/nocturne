/**
 * Virtual labs (`nocturne-lab`): a network you can break.
 *
 * These types mirror the backend's parameter and result structs exactly:
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, simulated time is **microseconds** since the lab was
 * created, and `?` marks a field the backend may leave out (or a parameter it
 * defaults). Backend source: `crates/nocturne-lab` and the simulator's public
 * types in `crates/nocturne-netsim`. Specification:
 * `docs/dev/features/lab.md`.
 *
 * How it hangs together:
 *
 * 1. A lab is a {@link LabSpec} stored with an id. `labs.list` / `labs.get`
 *    show it whether or not it is running.
 * 2. *Running* means a simulator in memory with a ticker advancing it:
 *    `labs.start`, `labs.speed`, `labs.stop`. Only a running lab answers
 *    probes, traces, tables, metrics, flows and faults.
 * 3. Every node with a command line is also an inventory device
 *    (`transport: "sim"`, `lab: {lab_id, node}`), so the terminal, the
 *    collectors, monitoring and the change pipeline work on it unchanged.
 *    {@link LabView.devices} maps node name → device id.
 * 4. What a console saves with `write memory` survives a stop;
 *    {@link LabView.unsaved_nodes} says what would be lost before it is.
 */

// --------------------------------------------------------------- the lab file

/** A lab as a file: what `labs.export` writes and `labs.import` takes. */
export interface LabSpec {
  name: string;
  description: string;
  nodes: NodeSpec[];
  links: LinkSpec[];
  notes: NoteSpec[];
  flows: FlowSpec[];
  /** Seed of the simulator's PRNG: the same seed replays identically. */
  seed: number;
}

export interface NodeSpec {
  /** Unique in the lab, and the device's initial hostname. */
  name: string;
  /** A {@link ModelInfo.id} from `labs.catalog`. */
  model: string;
  x: number;
  y: number;
  powered: boolean;
  /** Network devices: the configuration applied at boot. */
  startup_config?: string | null;
  /** End hosts: addressing and services. */
  host?: HostConfig | null;
  /** A switch's VLAN database, which lives outside the configuration. */
  vlan_dat?: string | null;
  /** Size of the generated RSA host key, if the device has one. */
  rsa_key_bits?: number | null;
}

export interface HostConfig {
  dhcp: boolean;
  /** `"192.168.1.10"`. */
  ip?: string | null;
  /** `"255.255.255.0"`. */
  mask?: string | null;
  gateway?: string | null;
  dns?: string | null;
  /** `"8.8.8.8/32"`, or `"203.0.113.1/30@Ethernet1"` on a multi-port node. */
  extra_ips: string[];
  /** `http`, `dns`, `dhcp`, `tftp`, `syslog`, `ntp`, `ssh`, `telnet`. */
  services: string[];
}

export interface LinkSpec {
  a: Endpoint;
  b: Endpoint;
  cable: CableKind;
}

/** One end of a cable, by node *name*. */
export interface Endpoint {
  node: string;
  interface: string;
}

/** `console` cables join the pseudo-ports `Console` and `RS232`. */
export type CableKind = 'auto' | 'copper' | 'fiber' | 'console';

/** A sticky note on the canvas. */
export interface NoteSpec {
  x: number;
  y: number;
  w: number;
  h: number;
  text: string;
  /** CSS colour, e.g. `"#f6e3b4"`. */
  color: string;
}

/** Background traffic: it moves counters without emitting packets. */
export interface FlowSpec {
  name: string;
  /** Source node name. */
  src: string;
  dst_ip: string;
  /** Average rate, bits per second. */
  bps: number;
  pattern: FlowPattern;
  enabled: boolean;
}

export type FlowPattern =
  | 'constant'
  | { sine: { period_s: number } }
  | { burst: { on_s: number; off_s: number } };

/** Live state of a flow (`labs.flow.list`). */
export interface FlowState {
  /** The path currently delivers the traffic. */
  active: boolean;
  current_bps: number;
  delivered_octets: number;
  /** Node names along the path, source first. */
  path: string[];
  /** Why it is not delivering; empty when `active`. */
  reason: string;
}

// ------------------------------------------------------------------- catalog

export interface ModelInfo {
  /** `router`, `switch`, `switch-l3`, `pc`, … */
  id: string;
  kind: NodeKind;
  /** `"Router (4 × GE)"`. */
  label: string;
  description: string;
  /** Full interface names, in display order. */
  interfaces: string[];
  cli: CliKind;
}

export type NodeKind =
  | 'router'
  | 'switch'
  | 'l3_switch'
  | 'firewall'
  | 'access_point'
  | 'pc'
  | 'laptop'
  | 'server'
  | 'printer'
  | 'phone'
  | 'cloud'
  | 'nms';

/** `none` means the node has no console, and so is not an inventory device. */
export type CliKind = 'ios' | 'host' | 'none';

// ----------------------------------------------------------------- the canvas

export interface LabSnapshot {
  name: string;
  description: string;
  /** Simulated time, microseconds. */
  now: number;
  nodes: NodeView[];
  links: LinkView[];
  notes: NoteSpec[];
}

export interface NodeView {
  /** Stable for the life of the lab, never reused. */
  id: number;
  name: string;
  /** The hostname the device's configuration gives it. */
  hostname: string;
  model: string;
  kind: NodeKind;
  x: number;
  y: number;
  powered: boolean;
  cli: CliKind;
  interfaces: IfView[];
  /** Best address to manage the device on, if it has one. */
  mgmt_ip?: string | null;
}

export interface IfView {
  /** `"GigabitEthernet0/1"`. */
  name: string;
  /** `"Gi0/1"`. */
  short: string;
  /** The link this port is cabled to. */
  link?: number | null;
  admin_up: boolean;
  oper_up: boolean;
  light: Light;
  /** `"10.0.0.1/24"`. */
  ip?: string | null;
  /** `"access 10"`, `"trunk"`, `"routed"`, `"host"` or empty. */
  mode: string;
  description: string;
}

/** `amber` is spanning tree listening, learning or blocking. */
export type Light = 'green' | 'amber' | 'red' | 'off';

export interface LinkView {
  id: number;
  /** Node ids. */
  a: number;
  a_if: string;
  b: number;
  b_if: string;
  cable: CableKind;
  up: boolean;
  a_light: Light;
  b_light: Light;
  /** Current simulated rate each way, bits per second. */
  bps_ab: number;
  bps_ba: number;
}

// ------------------------------------------------------------------- the labs

/** A row of `labs.list`. */
export interface LabSummary {
  id: string;
  name: string;
  description: string;
  nodes: number;
  links: number;
  running: boolean;
  updated_at: number;
}

/** Everything the lab screen needs about one lab. */
export interface LabView {
  id: string;
  name: string;
  description: string;
  running: boolean;
  /** Simulated seconds per real second; `0` while paused. */
  speed: number;
  /** Microseconds of simulated time since the lab was created; `0` when stopped. */
  sim_time: number;
  snapshot: LabSnapshot;
  /** Node name → inventory device id, for the nodes that have a console. */
  devices: Record<string, string>;
  /** Nodes whose running configuration a stop would lose. */
  unsaved_nodes: string[];
  capture: boolean;
}

/** One of the simulator's ready-made labs. */
export interface ExampleInfo {
  /** Pass this as `labs.create {example}`; `demo` is the campus. */
  id: string;
  name: string;
  description: string;
  nodes: number;
  /** The lab's own on-canvas notes: what it is there to show. */
  teaches: string[];
}

export interface LabIdParams {
  id: string;
}

/** `labs.list` — no parameters → {@link LabSummary}[] */

export interface LabsCreateParams {
  name?: string;
  /** An {@link ExampleInfo.id}; omit for an empty lab. */
  example?: string;
}

/** `labs.create` {@link LabsCreateParams} → {@link LabView} */

/** `labs.get` {@link LabIdParams} → {@link LabView} */

export interface LabsUpdateParams {
  id: string;
  name?: string;
  description?: string;
  /** Replaces the canvas notes. */
  notes?: NoteSpec[];
}

/** `labs.update` {@link LabsUpdateParams} → {@link LabView} */

/** What `labs.delete` answers. */
export interface LabDeleted {
  deleted: boolean;
  /** Simulated devices removed from the inventory with the lab. */
  devices_removed: number;
}

/** `labs.delete` {@link LabIdParams} → {@link LabDeleted} (and open consoles close) */

export interface LabsDuplicateParams {
  id: string;
  /** Made unique if a lab already has it. */
  name: string;
}

/** `labs.duplicate` {@link LabsDuplicateParams} → {@link LabView} (saved state only) */

export interface LabsImportParams {
  /** Overrides the name in the file. */
  name?: string;
  /** A {@link LabSpec}, or the text of one (JSON or YAML). */
  spec: LabSpec | string;
}

/** `labs.import` {@link LabsImportParams} → {@link LabView}; errors name the node or link */

export interface LabsExportParams {
  id: string;
  /** `json` (default) or `yaml`. */
  format?: 'json' | 'yaml';
}

/** What `labs.export` answers: a {@link LabSpec} as text. */
export interface LabExport {
  text: string;
}

/** `labs.export` {@link LabsExportParams} → {@link LabExport} */

/** `labs.start` {@link LabIdParams} → {@link LabView}; refused past the running limit */

/** What `labs.stop` answers. */
export interface LabStopped {
  id: string;
  running: false;
  /** Nodes whose running configuration has just been lost. */
  unsaved_nodes: string[];
}

/** `labs.stop` {@link LabIdParams} → {@link LabStopped} */

export interface LabsSpeedParams {
  id: string;
  /** `0` pauses; otherwise 0.25 to 16 times real time. */
  speed: number;
}

/** What `labs.speed` answers. */
export interface LabSpeed {
  id: string;
  speed: number;
}

/** `labs.speed` {@link LabsSpeedParams} → {@link LabSpeed} */

/** `labs.reset` {@link LabIdParams} → {@link LabView}; every node reboots from its startup-config */

/** `labs.catalog` — no parameters → {@link ModelInfo}[] */

/** `labs.examples` — no parameters → {@link ExampleInfo}[] */

// --------------------------------------------------------------------- nodes

export interface LabsNodeAddParams {
  lab_id: string;
  /** A {@link ModelInfo.id}. */
  model: string;
  /** Chosen by the simulator (`R1`, `SW2`, …) when left out. */
  name?: string;
  x: number;
  y: number;
}

/** `labs.node.add` {@link LabsNodeAddParams} → {@link NodeView} (an inventory device appears) */

export interface LabsNodeUpdateParams {
  lab_id: string;
  /** The node's current name. */
  node: string;
  /** Renames the node and its device. */
  name?: string;
  x?: number;
  y?: number;
  /** Powering off loses the running configuration, as on real hardware. */
  powered?: boolean;
  /** End hosts only. */
  host?: HostConfig;
}

/** `labs.node.update` {@link LabsNodeUpdateParams} → {@link NodeView} */

export interface LabsNodeMoveParams {
  lab_id: string;
  /** Node name → position. */
  positions: Record<string, { x: number; y: number }>;
}

/** What `labs.node.move` answers. */
export interface NodesMoved {
  moved: number;
}

/** `labs.node.move` {@link LabsNodeMoveParams} → {@link NodesMoved}; publishes no event (drag) */

export interface LabsNodeParams {
  lab_id: string;
  node: string;
}

/** What `labs.node.remove` answers. */
export interface NodeRemoved {
  removed: boolean;
}

/** `labs.node.remove` {@link LabsNodeParams} → {@link NodeRemoved} (its device goes too) */

export interface LabsNodeConfigParams {
  lab_id: string;
  node: string;
  /** `running` (default) or `startup`. */
  kind?: 'running' | 'startup';
}

/** What `labs.node.config` answers. */
export interface NodeConfig {
  text: string;
}

/** `labs.node.config` {@link LabsNodeConfigParams} → {@link NodeConfig} */

export interface LabsNodeConfigSetParams {
  lab_id: string;
  node: string;
  /** The configuration to boot from; everything up to `end` is applied. */
  startup: string;
}

/** A line the simulated device would not take, and what it said. */
export interface RejectedLine {
  line: string;
  error: string;
}

/** What `labs.node.config_set` answers. */
export interface NodeConfigSet {
  /** The startup configuration the device now holds. */
  text: string;
  accepted: number;
  rejected: RejectedLine[];
}

/** `labs.node.config_set` {@link LabsNodeConfigSetParams} → {@link NodeConfigSet}; the node reboots */

/** The device's live tables, as its `show` commands print them. */
export interface NodeTables {
  routes: RouteRow[];
  arp: ArpRow[];
  mac: MacRow[];
  vlans: VlanRow[];
  stp: StpRow[];
  neighbors: NeighborRow[];
  ospf_neighbors: OspfNeighborRow[];
  dhcp_bindings: DhcpRow[];
  nat: NatRow[];
}

export interface RouteRow {
  /** `C`, `S`, `S*`, `O`, `O IA`, `O E2`, `R`. */
  code: string;
  prefix: string;
  ad: number;
  metric: number;
  /** Empty for a connected route. */
  next_hop: string;
  interface: string;
}

export interface ArpRow {
  ip: string;
  /** `"0050.7966.6801"`. */
  mac: string;
  interface: string;
  /** Zero for the device's own addresses. */
  age_s: number;
}

export interface MacRow {
  vlan: number;
  mac: string;
  /** `DYNAMIC` or `STATIC`. */
  kind: string;
  port: string;
}

export interface VlanRow {
  id: number;
  name: string;
  /** `active` or `suspended`. */
  status: string;
  /** Short names of the access ports in the VLAN. */
  ports: string[];
}

export interface StpRow {
  vlan: number;
  interface: string;
  /** `Root`, `Desg`, `Altn`, `Back`. */
  role: string;
  /** `FWD`, `BLK`, `LIS`, `LRN`. */
  state: string;
  cost: number;
  /** `"32778 0050.7966.6801"`. */
  root_id: string;
  bridge_id: string;
  is_root: boolean;
}

export interface NeighborRow {
  /** `CDP` or `LLDP`. */
  protocol: string;
  local_if: string;
  device: string;
  remote_if: string;
  platform: string;
  mgmt_ip: string;
}

export interface OspfNeighborRow {
  neighbor_id: string;
  priority: number;
  /** `FULL/DR`, `2WAY/DROTHER`, … */
  state: string;
  dead_time_s: number;
  address: string;
  interface: string;
  area: string;
}

export interface DhcpRow {
  ip: string;
  mac: string;
  pool: string;
  expires_s: number;
  /** `Automatic`. */
  kind: string;
}

export interface NatRow {
  /** `icmp`, `tcp`, `udp`, or `---` for a static entry. */
  protocol: string;
  inside_global: string;
  inside_local: string;
  outside_local: string;
  outside_global: string;
}

/** `labs.node.tables` {@link LabsNodeParams} → {@link NodeTables} (running labs only) */

export interface NodeMetrics {
  hostname: string;
  uptime_s: number;
  cpu_pct: number;
  mem_used: number;
  mem_total: number;
  temperature_c: number;
  interfaces: IfCounters[];
}

export interface IfCounters {
  name: string;
  description: string;
  admin_up: boolean;
  oper_up: boolean;
  speed_bps: number;
  mtu: number;
  in_octets: number;
  out_octets: number;
  in_pkts: number;
  out_pkts: number;
  in_errors: number;
  out_errors: number;
  in_discards: number;
  out_discards: number;
  crc: number;
  /** Simulated time of the last state change, microseconds. */
  last_change: number;
}

/** `labs.node.metrics` {@link LabsNodeParams} → {@link NodeMetrics} (running labs only) */

// --------------------------------------------------------------------- cables

export interface LabsLinkAddParams {
  lab_id: string;
  /** Without an interface the first free one that suits the cable is used. */
  a: { node: string; interface?: string };
  b: { node: string; interface?: string };
  cable?: CableKind;
}

/** `labs.link.add` {@link LabsLinkAddParams} → {@link LinkView}; refusals say why */

export interface LabsLinkRemoveParams {
  lab_id: string;
  /** {@link LinkView.id}. */
  link: number;
}

/** What `labs.link.remove` answers. */
export interface LinkRemoved {
  removed: boolean;
}

/** `labs.link.remove` {@link LabsLinkRemoveParams} → {@link LinkRemoved} */

export interface LabsLinksParams {
  /** Every lab when left out. */
  lab_id?: string;
}

/** One cable, as the topology feature sees it. */
export interface LabLinkRow {
  lab_id: string;
  /** The lab's name. */
  lab: string;
  /** The simulator's link id while the lab runs, its position in the file otherwise. */
  link: number;
  a: LabLinkEnd;
  b: LabLinkEnd;
  /** Always false while the lab is stopped. */
  up: boolean;
  running: boolean;
}

export interface LabLinkEnd {
  /** Absent for a node with no console (a cloud, an access point). */
  device_id?: string | null;
  node: string;
  interface: string;
}

/** `labs.links` {@link LabsLinksParams} → {@link LabLinkRow}[] */

// --------------------------------------------------------- probes and packets

export interface LabsProbeParams {
  lab_id: string;
  /** Node name. */
  from: string;
  /** Node name or IPv4 address. */
  to: string;
  /** `ping` (default) or `tcp`. */
  kind?: 'ping' | 'tcp';
  /** Required for `tcp`. */
  port?: number;
}

/** Instant, with no simulated time passing and no packet events. */
export interface ProbeResult {
  ok: boolean;
  /** Round-trip time of the successful exchange, microseconds. */
  rtt_us: number;
  /** The forward path, source first; on failure, as far as it got. */
  hops: ProbeHop[];
  /** Why it failed; empty on success. */
  reason: string;
}

export interface ProbeHop {
  /** Node id. */
  node: number;
  name: string;
  /** Empty on the source. */
  in_if: string;
  /** Empty on the destination. */
  out_if: string;
  /** What this device did with the packet. */
  note: string;
}

/** `labs.probe` {@link LabsProbeParams} → {@link ProbeResult} (running labs only) */

export interface LabsConnectivityParams {
  lab_id: string;
  /** Every powered node with an address when left out. */
  nodes?: string[];
}

/** Every node to every other: the before and after of a rehearsal. */
export interface Connectivity {
  /** The nodes tested, in the order of the matrix. */
  nodes: string[];
  /** `matrix[from][to]`; the diagonal is always `ok`. */
  matrix: ConnectivityCell[][];
  /** Share of the ordered pairs that worked, 0–100. */
  reachable_pct: number;
}

export interface ConnectivityCell {
  ok: boolean;
  /** Why not; empty when `ok`. */
  reason: string;
}

/** `labs.connectivity` {@link LabsConnectivityParams} → {@link Connectivity} */

export interface LabsCaptureParams {
  lab_id: string;
  on: boolean;
}

/** What `labs.capture` answers. */
export interface CaptureState {
  capture: boolean;
}

/** `labs.capture` {@link LabsCaptureParams} → {@link CaptureState}; off empties the buffer */

export interface LabsTraceParams {
  lab_id: string;
  from: string;
  /** Node name or IPv4 address. */
  to: string;
  /** `icmp` (default), `tcp` or `udp`. */
  pdu?: 'icmp' | 'tcp' | 'udp';
  /** Required for `tcp` and `udp`. */
  port?: number;
}

/** What `labs.trace` answers. */
export interface TraceStarted {
  trace_id: number;
}

/** `labs.trace` {@link LabsTraceParams} → {@link TraceStarted}; hops arrive as packet events */

export interface LabsPacketsParams {
  lab_id: string;
  /** Only events after this sequence number. */
  after_seq?: number;
  /** Default 500, at most 5000. */
  limit?: number;
}

/** What `labs.packets` answers. */
export interface PacketPage {
  events: PacketEvent[];
  /** Pass as `after_seq` next time. */
  next_seq: number;
}

/** One hop of one frame, with the device's reasoning in words. */
export interface PacketEvent {
  /** Set when this hop belongs to a `labs.trace`. */
  trace?: number | null;
  /** Identity of the PDU: the same while it is forwarded, routed or translated. */
  pdu: number;
  /** Lab-wide and strictly increasing. */
  seq: number;
  /** Simulated time, microseconds. */
  time: number;
  kind: PacketEventKind;
  /** Node id. */
  at: number;
  /** `received` only: where it came from. */
  from?: number | null;
  /** `received` only: the link it crossed. */
  link?: number | null;
  /** `ARP`, `ICMP`, `DHCP`, `OSPF`, `STP`, `TCP`, `UDP`, `CDP`, … */
  protocol: string;
  /** `"ICMP echo request 10.0.0.2 → 10.0.1.5"`. */
  summary: string;
  layers: LayerInfo[];
  /** Plain English: what this device did and why. */
  decision: string;
  /** The interface involved at `at`, or empty. */
  interface: string;
  /** `seq` of the event this one follows from; `0` for none. */
  parent: number;
}

export type PacketEventKind =
  | 'created'
  | 'forwarded'
  | 'received'
  | 'delivered'
  | 'dropped';

export interface LayerInfo {
  /** OSI layer: 2, 3, 4 or 7. */
  layer: number;
  title: string;
  lines: string[];
}

/** `labs.packets` {@link LabsPacketsParams} → {@link PacketPage} */

// ---------------------------------------------------------- traffic and faults

export interface LabsFlowAddParams {
  lab_id: string;
  flow: FlowSpec;
}

/** What `labs.flow.add` answers. */
export interface FlowAdded {
  flow_id: number;
}

/** `labs.flow.add` {@link LabsFlowAddParams} → {@link FlowAdded} */

export interface LabsFlowRemoveParams {
  lab_id: string;
  flow_id: number;
}

/** What `labs.flow.remove` answers. */
export interface FlowRemoved {
  removed: boolean;
}

/** `labs.flow.remove` {@link LabsFlowRemoveParams} → {@link FlowRemoved} */

export interface LabsLabParams {
  lab_id: string;
}

/** A flow with what it is doing right now. */
export interface FlowRow {
  id: number;
  flow: FlowSpec;
  state: FlowState;
}

/** `labs.flow.list` {@link LabsLabParams} → {@link FlowRow}[] */

/** What can be made to go wrong, exactly as the simulator defines it. */
export type Fault =
  | { link_down: { link: number } }
  | { interface_errors: { node: number; interface: string; crc_per_s: number } }
  | { packet_loss: { link: number; percent: number } }
  | { flapping: { link: number; period_s: number } }
  | { high_cpu: { node: number; percent: number } }
  | { power_off: { node: number } }
  | { duplex_mismatch: { link: number } };

export interface LabsFaultInjectParams {
  lab_id: string;
  fault: Fault;
}

/** What `labs.fault.inject` answers. */
export interface FaultInjected {
  fault_id: number;
}

/** `labs.fault.inject` {@link LabsFaultInjectParams} → {@link FaultInjected} */

export interface LabsFaultClearParams {
  lab_id: string;
  fault_id: number;
}

/** What `labs.fault.clear` answers. */
export interface FaultCleared {
  cleared: boolean;
}

/** `labs.fault.clear` {@link LabsFaultClearParams} → {@link FaultCleared} */

/** A fault that is in force. */
export interface FaultRow {
  id: number;
  fault: Fault;
}

/** `labs.fault.list` {@link LabsLabParams} → {@link FaultRow}[] */

// -------------------------------------------------------------- exposed ports

export interface LabsExposeParams {
  lab_id: string;
  node: string;
  protocol: 'ssh' | 'telnet';
  /** `0` or absent: a free port. */
  port?: number;
  /** Defaults to the `expose_bind` setting; anything but loopback needs the desktop app. */
  bind?: string;
}

/** A console being served on a real TCP port. */
export interface ExposedRow {
  id: string;
  lab_id: string;
  /** The lab's name. */
  lab: string;
  node: string;
  protocol: 'ssh' | 'telnet';
  /** `"127.0.0.1:34567"`: what a client connects to. */
  address: string;
  opened_at: number;
}

/** `labs.expose` {@link LabsExposeParams} → {@link ExposedRow} */

/** `labs.exposed` — no parameters → {@link ExposedRow}[] */

export interface LabsUnexposeParams {
  /** {@link ExposedRow.id}. */
  id: string;
}

/** What `labs.unexpose` answers. */
export interface ExposeClosed {
  closed: boolean;
}

/** `labs.unexpose` {@link LabsUnexposeParams} → {@link ExposeClosed} */

// ---------------------------------------------------------------------- twins

export interface LabsTwinParams {
  /** Device ids, names or addresses. */
  device_ids: string[];
  name?: string;
}

/** What one real device became, and how faithfully. */
export interface TwinNode {
  device_id: string;
  /** The real device's name. */
  device: string;
  /** Its name in the lab. */
  node: string;
  /** The {@link ModelInfo.id} it was put on. */
  model: string;
  /** Which backup the configuration came from: `running` or `startup`. */
  config_source: string;
  /** Configuration lines tried (blank lines and comments do not count). */
  config_lines: number;
  accepted: number;
  /** Every line the simulated device refused, with what it said. */
  rejected: RejectedLine[];
  /** Real interfaces the chosen model has. */
  interfaces_mapped: string[];
  /** Real interfaces no model has: anything configured on them is lost. */
  interfaces_unmapped: string[];
}

/** A device that could not be twinned, and why. */
export interface TwinRefusal {
  device_id: string;
  device: string;
  reason: string;
}

/** A cable the neighbour tables implied. */
export interface TwinLink {
  a: string;
  a_interface: string;
  b: string;
  b_interface: string;
  /** Which neighbour table it came from: `cdp` or `lldp`. */
  protocol: string;
}

/** The job result of `labs.twin`. */
export interface TwinResult {
  lab_id: string;
  name: string;
  nodes: TwinNode[];
  links: TwinLink[];
  refused: TwinRefusal[];
}

/**
 * `labs.twin` {@link LabsTwinParams} → `{job_id}`; the job's result is a
 * {@link TwinResult}. Devices with no collected data or no backup are refused
 * with the reason, never guessed at.
 */

// --------------------------------------------------------------------- events

/** `labs.changed` — the lab, its topology, a node or its configuration moved. */
export interface LabsChangedEvent {
  lab_id: string;
  reason:
    | 'created'
    | 'updated'
    | 'deleted'
    | 'started'
    | 'stopped'
    | 'topology'
    | 'node'
    | 'config';
}

/**
 * `labs.tick` — at most four a second per running lab, and only while
 * something about the cables changed.
 */
export interface LabsTickEvent {
  lab_id: string;
  /** Simulated time, microseconds. */
  sim_time: number;
  links: LabsTickLink[];
}

export interface LabsTickLink {
  id: number;
  up: boolean;
  a_light: Light;
  b_light: Light;
  bps_ab: number;
  bps_ba: number;
}

/** `labs.packets` — batches while capture is on, at most ten a second. */
export interface LabsPacketsEvent {
  lab_id: string;
  events: PacketEvent[];
  next_seq: number;
}

/** `labs.trace_done` — one traced PDU reached its end, or died on the way. */
export interface LabsTraceDoneEvent {
  lab_id: string;
  trace_id: number;
  ok: boolean;
  /** What happened, in words. */
  summary: string;
}

// ------------------------------------------------------------------- settings

/** The `labs` settings section (`settings.get` / `settings.set`). */
export interface LabSettings {
  /** Labs that may run at the same time. Default 8. */
  max_running: number;
  /** Real milliseconds between simulation steps. Default 50. */
  tick_ms: number;
  /** Simulated time per step, as a multiple of `tick_ms`. Default 1. */
  default_speed: number;
  /** Simulated seconds a freshly loaded lab may take to converge. Default 60. */
  settle_s: number;
  /** Real protocol timers (30 s spanning-tree forward delay) instead of short ones. Default false. */
  realistic_timers: boolean;
  /** Packet events kept per lab for the inspector. Default 20000. */
  capture_events: number;
  /** Milliseconds of quiet before an edited lab is written back. Default 1000. */
  autosave_ms: number;
  /** Address `labs.expose` binds to by default. Default `127.0.0.1`. */
  expose_bind: string;
  /** Nodes `labs.connectivity` tests without an explicit list. Default 40. */
  connectivity_max_nodes: number;
}
