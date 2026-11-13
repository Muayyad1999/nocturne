/**
 * The topology feature (`nocturne-topology`): the network drawn from what the
 * devices themselves reported — who is connected to whom, which subnets they
 * share, where a host is plugged in, and the path between two points.
 *
 * These types mirror the backend's parameter and result structs exactly:
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and `?` marks a field the backend may leave out (or a
 * parameter it defaults). Backend source: `crates/nocturne-topology`.
 * Specification: `docs/dev/features/topology.md`.
 *
 * Nothing here reaches a device: the graph is computed from what
 * `collect.run` has already stored, so a "Refresh" button is a `collect.run`
 * followed by a `topology.get`.
 *
 * Three things the UI should show plainly, because the backend is careful
 * about them:
 *
 * * `Link.confidence` — `confirmed` means both ends reported the link,
 *   `one_sided` that only one did, `inferred` that it was derived from
 *   addresses or MAC tables. They are not the same claim.
 * * `Node.unmanaged` — seen by a neighbour protocol (or named in a manual
 *   link) but not in the inventory. The UI offers to add it.
 * * `Graph.warnings` and `Node.stale` — what could not be worked out, and
 *   what is being drawn from old data. Neither is hidden.
 */

// ------------------------------------------------------------------- model

/** Which picture of the network is wanted. */
export type Layer = 'physical' | 'l3';

/**
 * What a node is. Inferred from the inventory role, then from the
 * capabilities a neighbour protocol reported, then from the platform string.
 */
export type NodeKind =
  | 'router'
  | 'switch'
  | 'l3_switch'
  | 'firewall'
  | 'access_point'
  | 'server'
  | 'host'
  /** A WAN circuit, the internet, or anything else outside the network. */
  | 'cloud'
  | 'unknown';

/** How a link is known. */
export type LinkKind =
  | 'lldp'
  | 'cdp'
  | 'mndp'
  /** Two interfaces in one subnet. */
  | 'subnet'
  /** The lab feature says so. */
  | 'lab'
  /** A person drew it. */
  | 'manual'
  /** A MAC-table sighting on an access port. */
  | 'mac'
  | 'unknown';

/** How we came to believe a link exists. */
export type Confidence =
  /** Both ends reported it (or the lab, which owns its own wiring). */
  | 'confirmed'
  /** Only one end did: the other is unmanaged, silent, or never collected. */
  | 'one_sided'
  /** Derived from addresses or MAC tables, not reported as a neighbour. */
  | 'inferred';

/** Up, down, or nobody has looked. */
export type Status = 'up' | 'down' | 'unknown';

/** Nodes that belong together. */
export type GroupKind = 'site' | 'subnet' | 'lab';

/** Which way the nodes are arranged. */
export type Algorithm =
  /** Ranks by role (core at the top), barycentre ordering, sites as blocks. */
  | 'layered'
  /** Spring embedder, for meshes with no hierarchy. */
  | 'force'
  /** Rings around a root. */
  | 'radial';

/** One end of a link. */
export interface End {
  node: string;
  /** Canonical interface name (`GigabitEthernet1/0/1`), when it is known. */
  interface?: string;
  ip?: string;
}

/**
 * A device, a neighbour we have never logged in to, or a host found in a MAC
 * table. `x`/`y` are the centre of the node in this view's coordinates.
 */
export interface Node {
  id: string;
  kind: NodeKind;
  /** Absent for unmanaged neighbours, endpoints and manual-only nodes. */
  device_id?: string;
  name: string;
  address?: string;
  site?: string;
  role?: string;
  profile?: string;
  model?: string;
  lab: boolean;
  status: Status;
  /** Seen by a neighbour protocol but not in the inventory. */
  unmanaged: boolean;
  x: number;
  y: number;
  pinned: boolean;
  alerts?: number;
  /** When the data this node was built from was collected. */
  collected_at?: number;
  /** The device's data is older than the staleness setting. */
  stale?: boolean;
}

/** A connection between two nodes. `a.node === b.node` is a stack cable. */
export interface Link {
  id: string;
  a: End;
  b: End;
  kind: LinkKind;
  confidence: Confidence;
  status: Status;
  speed_bps?: number;
  util_pct?: number;
  /** VLANs carried, as a compressed list (`1,10-12,99`). */
  vlans?: string;
  /** Port-channel this link stands for, with its member count. */
  aggregate?: string;
  label?: string;
  /** Every protocol that reported it (`lldp`, `cdp`), in order. */
  protocols?: string[];
  /** Member interface pairs when this link is a folded port-channel. */
  members?: string[];
}

/** Nodes that belong together: a site, a subnet, a lab. */
export interface Group {
  id: string;
  kind: GroupKind;
  label: string;
  nodes: string[];
}

export interface Stats {
  devices: number;
  links: number;
  unmanaged: number;
  stale_devices: number;
  endpoints: number;
}

/** How old the data the graph was drawn from is. */
export interface DataAge {
  oldest_at?: number;
  newest_at?: number;
}

/** The network, drawn. */
export interface Graph {
  nodes: Node[];
  links: Link[];
  groups: Group[];
  /** What could not be worked out, in sentences meant for a person. */
  warnings: string[];
  stats: Stats;
  generated_at: number;
  data_age: DataAge;
  layer: Layer;
  view: string;
}

/** Which devices a view, a graph or an export covers. */
export interface Filter {
  site?: string | null;
  tag?: string | null;
  role?: string | null;
  lab_id?: string | null;
  device_ids: string[];
}

/** A saved picture: which layer, which devices, whether hosts are drawn. */
export interface View {
  id: string;
  name: string;
  layer: Layer;
  filter: Filter;
  include_endpoints: boolean;
  created_at: number;
  updated_at: number;
}

/** The view every client gets when it asks for none. */
export const DEFAULT_VIEW = 'default';

/** A node's place in a view. */
export interface Position {
  x: number;
  y: number;
  /** A person put it here: no automatic layout may move it. */
  pinned: boolean;
}

/** The `topology` settings section (`settings.get {section: 'topology'}`). */
export interface TopologySettings {
  /**
   * Neighbour data older than this contributes links, but the device is
   * counted in `stats.stale_devices` and named in `warnings`.
   */
  stale_after_hours: number;
  /** A port with more MAC addresses than this is not an access port. */
  endpoint_max_macs: number;
  /** Fold the members of a port-channel into one link when both ends agree. */
  fold_port_channels: boolean;
  /** How many physical-graph snapshots `topology.changes` keeps. */
  max_snapshots: number;
  default_algorithm: Algorithm;
  /** Ask the lab feature for the wiring of lab devices (`labs.links`). */
  use_lab_links: boolean;
  /** Ask the monitor feature for link utilisation (`monitor.interfaces`). */
  use_monitor_utilisation: boolean;
}

// ------------------------------------------------------------------- graph

/** `topology.get` params. A saved view supplies what is left out. */
export interface GetParams {
  /** Defaults to `default`. */
  view?: string;
  layer?: Layer;
  filter?: Filter;
  include_endpoints?: boolean;
}

/** `topology.get` {@link GetParams} → {@link Graph} */

// ------------------------------------------------------------------- views

/** `topology.views` — no params → {@link View}[] (the default view first) */

/** `topology.view_save` params. Without an `id` a new view is created. */
export interface ViewSaveParams {
  id?: string;
  name: string;
  layer?: Layer;
  filter?: Filter;
  include_endpoints?: boolean;
}

/** `topology.view_save` {@link ViewSaveParams} → {@link View} */

/** Params of every method that takes one id. */
export interface IdParams {
  id: string;
}

/** Whether something was there to delete. */
export interface Deleted {
  deleted: boolean;
}

/** `topology.view_delete` {@link IdParams} → {@link Deleted} (the default view cannot be deleted) */

// ------------------------------------------------------------------ layout

/** `topology.layout.save` params: the positions that changed. */
export interface LayoutSaveParams {
  /** Defaults to `default`. */
  view?: string;
  /** Node id → where it now is. Merged with what is already stored. */
  positions: Record<string, Position>;
}

/** Every position stored for a view. */
export interface LayoutResult {
  view: string;
  positions: Record<string, Position>;
}

/** `topology.layout.save` {@link LayoutSaveParams} → {@link LayoutResult} */

/** `topology.autolayout` params. */
export interface AutoLayoutParams {
  /** Defaults to `default`. */
  view?: string;
  /** Defaults to the `default_algorithm` setting. */
  algorithm?: Algorithm;
  layer?: Layer;
  filter?: Filter;
  include_endpoints?: boolean;
  /** For `radial`: the node, device id or name to put in the middle. */
  root?: string;
  /** Store the result as the view's positions. Defaults to `true`. */
  save?: boolean;
}

export interface AutoLayoutResult {
  view: string;
  algorithm: Algorithm;
  positions: Record<string, Position>;
}

/**
 * `topology.autolayout` {@link AutoLayoutParams} → {@link AutoLayoutResult}
 *
 * Deterministic: the same graph gives the same picture. Pinned nodes are left
 * exactly where they are and everything else is laid out around them.
 */

// --------------------------------------------------------------- neighbors

/** One row of a device's "connected to" table. */
export interface NeighborRow {
  link_id: string;
  local_interface?: string;
  local_ip?: string;
  peer: Node;
  peer_interface?: string;
  peer_ip?: string;
  kind: LinkKind;
  confidence: Confidence;
  status: Status;
  speed_bps?: number;
  util_pct?: number;
  vlans?: string;
  aggregate?: string;
  label?: string;
  protocols: string[];
  members: string[];
}

export interface Neighbors {
  device_id: string;
  device: string;
  /** Ordered by local interface, the way a port list reads. */
  rows: NeighborRow[];
  /** The warnings from building the graph that mention this device. */
  warnings: string[];
}

/** `topology.neighbors` params. `id` is accepted as an alias of `device_id`. */
export interface NeighborsParams {
  device_id: string;
  /** Also list the hosts learnt on this device's edge ports. */
  include_endpoints?: boolean;
}

/** `topology.neighbors` {@link NeighborsParams} → {@link Neighbors} */

// -------------------------------------------------------------------- path

/** `topology.path` params: device ids, names or IP addresses. */
export interface PathParams {
  from: string;
  to: string;
}

/** One step of a path. */
export interface Hop {
  /** The name of the device, or the address of the host at the end. */
  node: string;
  device_id?: string;
  in_interface?: string;
  out_interface?: string;
  /** How this step was worked out. */
  via: 'connected' | 'route' | 'arp' | 'mac';
  /** The route that matched, the ARP entry, or what was missing. */
  detail: string;
}

export interface Trace {
  from: string;
  to: string;
  /** The address the walk was aiming at. */
  destination?: string;
  hops: Hop[];
  /** Whether the walk reached the destination. */
  complete: boolean;
  /**
   * Why it stopped, when it did not — "no route data collected for R3" and
   * the like. The walk never guesses a next hop.
   */
  stopped_because?: string;
  /** Equal-cost choices, VRFs and anything else worth saying. */
  notes: string[];
}

/** `topology.path` {@link PathParams} → {@link Trace} */

// ------------------------------------------------------------------ locate

/** `locate.host` params: an IP address, a MAC address, or a device name. */
export interface LocateParams {
  query: string;
}

/** One place the address was seen. */
export interface Location {
  device_id: string;
  device: string;
  interface: string;
  vlan?: number;
  /** What kind of port it is; only `access` is where something is plugged in. */
  kind: 'access' | 'trunk' | 'uplink';
  /** When the MAC table it came from was collected. */
  seen_at: number;
  /** How many MACs that port has learnt. */
  macs_on_port: number;
  /** `dynamic`, `static`, … as the device labelled the entry. */
  entry_kind: string;
  why: string;
}

export interface Located {
  mac?: string;
  ip?: string;
  vendor?: string;
  /** Every sighting, in device then interface order. */
  locations: Location[];
  /** The index in `locations` of the port the host is most likely on. */
  best?: number;
  /** The reasoning, step by step — show it next to the answer. */
  how: string[];
}

/** `locate.host` {@link LocateParams} → {@link Located} */

// ----------------------------------------------------------------- changes

export interface SnapshotNode {
  id: string;
  name: string;
  device_id?: string;
  kind: NodeKind;
  unmanaged: boolean;
}

export interface SnapshotLink {
  /** The link's identity, independent of which end is `a`. */
  signature: string;
  a_node: string;
  a_name: string;
  a_interface?: string;
  b_node: string;
  b_name: string;
  b_interface?: string;
  kind: LinkKind;
  confidence: Confidence;
}

/** `topology.changes` params. */
export interface ChangesParams {
  /** Compare the snapshot at or before this time with the newest one. */
  since: number;
}

export interface Changes {
  since: number;
  /** The snapshot the comparison started from. */
  from_at?: number;
  to_at?: number;
  /** How many snapshots are kept at all. */
  snapshots: number;
  nodes_added: SnapshotNode[];
  nodes_removed: SnapshotNode[];
  links_added: SnapshotLink[];
  links_removed: SnapshotLink[];
  /** What the comparison could and could not say. */
  notes: string[];
}

/** `topology.changes` {@link ChangesParams} → {@link Changes} */

// ------------------------------------------------------------------ export

export type Format = 'svg' | 'drawio' | 'mermaid' | 'dot' | 'json';

/** `topology.export` params: a graph to draw, and what to draw it as. */
export interface ExportParams {
  view?: string;
  layer?: Layer;
  filter?: Filter;
  include_endpoints?: boolean;
  format: Format;
}

export interface Exported {
  format: Format;
  /** What a download should be called. */
  filename: string;
  text: string;
}

/**
 * `topology.export` {@link ExportParams} → {@link Exported}
 *
 * `svg` is a standalone drawing with a legend, `drawio` opens in
 * diagrams.net with editable shapes, `json` is the {@link Graph} itself.
 * Every format escapes device names, which are attacker-controlled text.
 */

// ------------------------------------------------------------ manual links

/** One end of a drawn link: a device, or a name for something that is not one. */
export interface ManualEnd {
  /** A device id, name or address; resolved to an id when it is saved. */
  device?: string | null;
  /** For an end that is not a device: what to call it. */
  name?: string | null;
  interface?: string | null;
  /** What to draw a non-device end as; `cloud` by default. */
  kind?: NodeKind | null;
}

/** A link a person drew: an unmanaged switch, a WAN circuit, dark fibre. */
export interface ManualLink {
  /** Empty or absent on save creates a new link. */
  id?: string;
  a: ManualEnd;
  b: ManualEnd;
  label?: string | null;
  created_at?: number;
  updated_at?: number;
}

/** `topology.manual.list` — no params → {@link ManualLink}[] */

/** `topology.manual.save` {@link ManualLink} → {@link ManualLink} */

/** `topology.manual.delete` {@link IdParams} → {@link Deleted} */

// ------------------------------------------------------------------- event

/**
 * `topology.changed` — the graph may have changed and should be refetched.
 * Debounced to one a second; `reason` says what moved.
 */
export interface TopologyChanged {
  reason: 'collect' | 'inventory' | 'lab' | 'manual' | 'view' | 'layout' | 'events';
  /** Set when only one view's positions changed. */
  view?: string;
}
