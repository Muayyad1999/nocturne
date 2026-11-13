/**
 * IP address management (`nocturne-ipam`): prefixes, addresses and VLANs,
 * with the arithmetic done and the record checked against what the devices
 * actually report.
 *
 * These types mirror the backend's parameter and result structs exactly:
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, `?` marks a field the backend may leave out (or a
 * parameter it defaults) and `| null` a field it always sends and that may be
 * null. Backend source: `crates/nocturne-ipam`. Specification:
 * `docs/dev/features/ipam.md`.
 *
 * Three things about this surface are worth building the screens around.
 *
 * 1. **A record is not an observation.** A {@link Prefix}, an address with
 *    `documented: true` and a {@link VlanRow} with `documented: true` are
 *    what a person wrote down. An {@link Observation} is what a device said,
 *    and it always carries the device, the interface and the time its table
 *    was collected. An {@link Address} shows both halves side by side:
 *    `documented` and `observed` say which exist, `observations` is the
 *    evidence, and `disagreements` is where the two do not match. Nothing in
 *    the backend turns one into the other on its own.
 * 2. **`free` means "not seen", not "available".** `Status.free` is "no
 *    record and nothing has ever answered on it". How much was looked at is
 *    in the coverage of {@link Reconciliation} and {@link PrefixDetail}, and
 *    `ipam.next_address` says in `basis` what its answer rests on. A screen
 *    that calls it "available" is making a claim the backend did not.
 * 3. **`source: "discovered"` means nobody documented it.** A prefix a sync
 *    created because an address was seen inside it is not part of anybody's
 *    plan until somebody saves it (which adopts that record and makes it
 *    `manual`).
 */

// ---------------------------------------------------------------- vocabulary

export type Role =
  | 'lan'
  | 'wan'
  | 'loopback'
  | 'management'
  | 'p2p'
  | 'dmz'
  | 'guest'
  | 'voice'
  | 'other';

/** Where a *record* came from. `discovered` = nobody has confirmed it. */
export type Source = 'manual' | 'discovered';

/** What filled in an {@link Address} row. */
export type AddressSource = 'manual' | 'interface' | 'arp' | 'dns';

/**
 * The state of one address. `free` is the absence of both a record and a
 * sighting — never a promise that the address can be used.
 */
export type Status = 'active' | 'reserved' | 'dhcp' | 'free' | 'conflict';

/** Which collected table a sighting came out of. */
export type ObservationKind = 'interface' | 'arp';

export type ConflictKind =
  | 'duplicate_ip'
  | 'overlap'
  | 'outside_any_prefix'
  | 'gateway_mismatch';

/** Settings section `ipam` (`settings.get {section: "ipam"}`). */
export interface IpamSettings {
  /** An ARP sighting older than this stops counting and is forgotten. */
  arp_stale_days: number;
  /** Re-read collected data when `collect.updated` arrives (debounced). */
  sync_on_collect: boolean;
  /** How long that sync waits for the collections to stop arriving. */
  sync_delay_ms: number;
  /** The largest block listed address by address (the classic grid). */
  grid_max_addresses: number;
  /** Recognise HSRP/VRRP/GLBP virtual MACs, so a shared address is not a duplicate. */
  recognise_virtual_macs: boolean;
  /** Two MACs for one address within this window are a conflict. */
  mac_change_window_minutes: number;
}

// -------------------------------------------------------------------- model

export interface DhcpRange {
  start: string;
  end: string;
}

/** A device and interface that showed something, with the time it was read. */
export interface Sighting {
  device_id: string;
  device: string;
  interface: string;
  /** The address as configured, `10.1.10.1/24`. */
  cidr: string;
  collected_at: number;
}

/** A block of addresses somebody is keeping track of. */
export interface Prefix {
  id: string;
  /** Canonical `network/length`, IPv4 or IPv6. */
  cidr: string;
  name: string;
  description: string;
  /** A lab's blocks carry `lab:<lab id>` and never mix with production. */
  site?: string;
  vlan_id?: number;
  role?: Role;
  gateway?: string;
  dhcp?: DhcpRange;
  tags: string[];
  source: Source;
  /** For a `discovered` prefix: the interfaces it was inferred from. */
  discovered_from: Sighting[];
  created_at: number;
  updated_at: number;
}

/** An interface configured inside a prefix. An observation, so it is timed. */
export interface PrefixDevice {
  device_id: string;
  device: string;
  interface: string;
  ip: string;
  /** The mask the interface carries, which is not always the prefix's own. */
  prefix_len: number | null;
  collected_at: number;
  lab: boolean;
}

/** A prefix with everything computed from what is inside it. */
export interface PrefixRow extends Prefix {
  parent_id: string | null;
  depth: number;
  /** Direct children. */
  children: number;
  /** Addresses a host can have (IPv4 less network and broadcast above a /31). */
  usable: number;
  /** The same number exactly, for IPv6 blocks that outgrow 64 bits. */
  usable_text: string;
  /** In use: documented as in use, or seen on the network. */
  used: number;
  reserved: number;
  free: number;
  /** `0` when `util_known` is false. */
  util_pct: number;
  /** False for a block too large for a percentage to mean anything (IPv6 /64 and up). */
  util_known: boolean;
  /** How many addresses inside have a record. */
  documented: number;
  /** How many have been seen on the network. */
  observed: number;
  /** Share of the block covered by child prefixes. */
  child_coverage_pct: number;
  devices: PrefixDevice[];
}

/** One sighting of one address. Never merged with a record. */
export interface Observation {
  ip: string;
  kind: ObservationKind;
  device_id: string;
  device: string;
  interface: string;
  /** The mask the interface carried; absent for an ARP sighting. */
  prefix_len?: number;
  cidr?: string;
  mac?: string;
  vendor?: string;
  /** `""` (production) or `lab:<lab id>`. */
  scope: string;
  first_seen: number;
  last_seen: number;
  /** When the table behind this sighting was read from the device. */
  collected_at: number;
  /** `cli`, `lab`, `snmp`. */
  collector: string;
  /** Still in the device's latest table. */
  current: boolean;
}

/** A record and the network disagreeing about one field of one address. */
export interface Disagreement {
  /** `device`, `interface` or `mac`. */
  field: string;
  documented: string;
  observed: string;
  device_id: string;
  device: string;
  interface: string;
  collected_at: number;
}

/** One address, with the record and the evidence kept apart. */
export interface Address {
  ip: string;
  prefix_id: string | null;
  status: Status;
  hostname: string | null;
  description: string | null;
  /** From the record when there is one, else from the strongest sighting. */
  device_id: string | null;
  interface: string | null;
  mac: string | null;
  vendor: string | null;
  source: AddressSource;
  /** First time the network showed it; null = it never has. */
  first_seen: number | null;
  last_seen: number | null;
  /** When the record was last written, if there is one. */
  documented_at: number | null;
  tags: string[];
  /** A person wrote a record for this address. */
  documented: boolean;
  /** The network has shown it at least once. */
  observed: boolean;
  observations: Observation[];
  /** Reported, never resolved by the backend. */
  disagreements: Disagreement[];
}

/** A run of addresses with neither a record nor a sighting. */
export interface FreeRange {
  start: string;
  end: string;
  /** Saturated at 2^64-1 for IPv6; `count_text` is exact. */
  count: number;
  count_text: string;
}

export interface VlanRecord {
  /** The document id; `""` for a VLAN that is only observed. */
  id: string;
  vid: number;
  name: string;
  site?: string;
  description: string;
  prefix_ids: string[];
  source: Source;
  created_at: number;
  updated_at: number;
}

/** A VLAN as one device reports it. */
export interface VlanDevice {
  device_id: string;
  device: string;
  /** The name on that device, which is not always the documented one. */
  name: string;
  status: string;
  ports: number;
  collected_at: number;
  lab: boolean;
}

export interface VlanPrefix {
  id: string;
  cidr: string;
}

/** A VLAN record, the collected VLANs that match it, or both. */
export interface VlanRow extends VlanRecord {
  devices: VlanDevice[];
  prefixes: VlanPrefix[];
  documented: boolean;
  observed: boolean;
}

/** One side of a conflict, and whether it is a record or a sighting. */
export interface Involved {
  /** `record` or `observation`. */
  kind: string;
  device_id?: string;
  device?: string;
  interface?: string;
  ip?: string;
  cidr?: string;
  prefix_id?: string;
  mac?: string;
  collected_at?: number;
}

/** Something that cannot be true at the same time, with its evidence. */
export interface Conflict {
  kind: ConflictKind;
  /** The address, prefix or VLAN it is about. */
  subject: string;
  /** One sentence a person can act on. */
  detail: string;
  involved: Involved[];
}

/** What one device contributed — including when it is nothing, and why. */
export interface DeviceCoverage {
  device_id: string;
  device: string;
  site: string;
  lab: boolean;
  /** When the interface table was read; null = it never was. */
  interfaces_at: number | null;
  arp_at: number | null;
  addresses_configured: number;
  arp_entries: number;
  /** Why this device says nothing about addresses. */
  reason: string | null;
}

// -------------------------------------------------------- prefixes: methods

/** `ipam.prefixes` params. Result: {@link PrefixRow}[] — a containment tree in address order. */
export interface PrefixesParams {
  site?: string;
  query?: string;
  /** Only the prefixes inside this one. */
  parent_id?: string;
  /** `true`: only the matches. Default keeps the ancestors of a match. */
  flat?: boolean;
}

/** `ipam.prefix.get` params. Result: {@link PrefixDetail}. */
export interface GetPrefixParams {
  id: string;
  /** Default `true`. */
  addresses?: boolean;
}

export interface PrefixDetail {
  prefix: PrefixRow;
  addresses: Address[];
  free_ranges: FreeRange[];
  /** Child blocks no child prefix covers. */
  free_child_prefixes: string[];
  /** True when `addresses` is every address of the block. */
  grid: boolean;
  /** What the free space does and does not mean. */
  note: string;
  /** The devices that could have seen anything in this block. */
  coverage: DeviceCoverage[];
}

/**
 * `ipam.prefix.save` params. Result: {@link SavedPrefix}. Every field but
 * `cidr` is optional; one left out keeps the value it had.
 */
export interface PrefixInput {
  /** Absent creates; present updates (and makes a discovered prefix manual). */
  id?: string;
  cidr?: string;
  name?: string;
  description?: string;
  site?: string;
  vlan_id?: number;
  role?: Role;
  /** `""` clears it. */
  gateway?: string;
  dhcp?: DhcpRange;
  tags?: string[];
}

export interface SavedPrefix {
  prefix: PrefixRow;
  /** An overlap that is not containment, and the like: said, never refused. */
  warnings: string[];
  created: boolean;
  /** The block was there as `discovered` and this save adopted it. */
  adopted: boolean;
}

/** `ipam.prefix.delete` params: `{id}`. Result: {@link DeletedPrefix}. */
export interface DeletedPrefix {
  deleted: boolean;
  cidr: string;
  /** Children that now hang off this prefix's parent. */
  children_reparented: number;
  /** Address records that claimed it and now claim its parent, or nothing. */
  addresses_repointed: number;
}

/** `ipam.prefix.split` params. Result: {@link SplitResult}. */
export interface SplitParams {
  id: string;
  /** Equal blocks of this length. Mutually exclusive with `sizes`. */
  new_len?: number;
  /** Host counts (VLSM), as numbers or `{name, hosts}`. */
  sizes?: Array<number | { name?: string; hosts: number }>;
  /** Point-to-point links get a /31 rather than a /30. */
  p2p31?: boolean;
  /** Default `false`: the children are created. */
  dry_run?: boolean;
}

export interface SplitChild {
  cidr: string;
  name: string;
  /** VLSM only. */
  hosts_requested: number | null;
  usable: number;
  /** VLSM only. */
  spare: number | null;
  /** The id it was created as; null in a dry run. */
  id: string | null;
  /** Why it was not created. */
  skipped: string | null;
}

export interface SplitResult {
  parent: string;
  parent_id: string;
  dry_run: boolean;
  created: number;
  children: SplitChild[];
}

/** `ipam.next_prefix` params. Result: {@link NextPrefix}. Nothing is created. */
export interface NextPrefixParams {
  parent_id: string;
  len: number;
}

export interface NextPrefix {
  parent: string;
  parent_id: string;
  len: number;
  /** null when nothing of that size fits. */
  cidr: string | null;
  /** Every free child block, as CIDRs. */
  free_space: string[];
  note: string;
}

// ------------------------------------------------------- addresses: methods

/** `ipam.next_address` params. Result: {@link NextAddresses}. */
export interface NextAddressParams {
  prefix_id: string;
  /** Default 1, at most 1024. */
  count?: number;
  /** Skip this many host addresses at the bottom of the block. */
  skip_first?: number;
}

/** Why addresses were passed over. */
export interface NextAddressSkipped {
  network: number;
  broadcast: number;
  /** The Subnet-Router anycast address of an IPv6 subnet. */
  anycast: number;
  gateway: number;
  dhcp_range: number;
  documented: number;
  seen_on_an_interface: number;
  seen_in_arp: number;
  asked_to_skip: number;
}

export interface NextAddresses {
  prefix_id: string;
  cidr: string;
  addresses: string[];
  /** Fewer than asked for, because the block ran out. */
  exhausted: boolean;
  skipped: NextAddressSkipped;
  /** What the answer rests on. Show it: "not seen" is not "free". */
  basis: string;
}

/** `ipam.address.save` params. Result: {@link SavedAddress}. */
export interface AddressInput {
  ip: string;
  /** Taken from the containing prefix when left out. */
  prefix_id?: string;
  /** `active`, `reserved` or `dhcp`; `free` and `conflict` are refused. */
  status?: Status;
  hostname?: string;
  description?: string;
  /** Whom the person says the address belongs to. */
  device_id?: string;
  interface?: string;
  mac?: string;
  tags?: string[];
  /** `""` (production) or `lab:<lab id>`; from the prefix when left out. */
  scope?: string;
}

export interface SavedAddress {
  address: Address;
  created: boolean;
  warnings: string[];
}

/** `ipam.address.delete` and `ipam.address.get` params. */
export interface AddressKeyParams {
  ip: string;
  /** Default `""` (production). */
  scope?: string;
}

/** Result of `ipam.address.delete`. The sightings stay. */
export interface DeletedAddress {
  deleted: boolean;
  ip: string;
  /** Sightings left behind, which is why the address may still be listed. */
  observations_left: number;
}

/** Result of `ipam.address.get`. */
export interface AddressDetail {
  address: Address;
  prefix: PrefixRow | null;
}

/**
 * `ipam.address.reserve` params. Result: {@link SavedAddress}. Without `ip`
 * the first free address is taken; an address already recorded or in use is a
 * `conflict` error naming who has it.
 */
export interface ReserveParams {
  prefix_id: string;
  ip?: string;
  hostname?: string;
  description?: string;
}

/** `ipam.search` params. Result: {@link SearchResult}. */
export interface SearchParams {
  /** An address, part of one, a MAC in any notation, a name, a tag. */
  query: string;
  /** Default 200. */
  limit?: number;
}

export interface SearchResult {
  /** Prefixes that match, plus the one containing a typed address. */
  prefixes: PrefixRow[];
  addresses: Address[];
  vlans: VlanRow[];
}

// --------------------------------------------- reconciliation: the methods

/** `ipam.sync` params. Result: {@link SyncResult}. Refreshes evidence only. */
export interface SyncParams {
  /** Empty = every device. */
  device_ids?: string[];
}

export interface SyncResult {
  at: number;
  /** Prefixes created because an address was seen inside them (`discovered`). */
  prefixes_added: number;
  prefixes_refreshed: number;
  /** Observations written. */
  addresses_updated: number;
  /** ARP sightings forgotten because nothing refreshed them in time. */
  addresses_aged_out: number;
  /** Interface addresses that are no longer configured anywhere. */
  addresses_withdrawn: number;
  /** Distinct VLANs seen. A sync writes no VLAN records. */
  vlans_updated: number;
  /** Addresses left out on purpose: link-local, loopback, unspecified. */
  skipped_addresses: number;
  devices: DeviceCoverage[];
  conflicts: Conflict[];
}

/** `ipam.reconcile` params. Result: {@link Reconciliation}. Writes nothing. */
export interface ReconcileParams {
  site?: string;
}

/** What was looked at. Without it, "nothing answered" is an opinion. */
export interface Coverage {
  devices_total: number;
  devices_with_addresses: number;
  devices_never_collected: number;
  oldest_collection: number | null;
  newest_collection: number | null;
  devices: DeviceCoverage[];
}

/** An address the network shows and nobody wrote down. */
export interface Undocumented {
  ip: string;
  scope: string;
  prefix_id: string | null;
  prefix: string | null;
  /** False when even the prefix it falls in is only `discovered`. */
  prefix_documented: boolean;
  observations: Observation[];
  summary: string;
}

/** An address somebody wrote down that the network has never shown. */
export interface Unobserved {
  ip: string;
  scope: string;
  prefix_id: string | null;
  prefix: string | null;
  hostname: string;
  status: Status;
  documented_at: number;
  /** null = never seen at all. */
  last_seen: number | null;
  /** Whether anything that could have seen it was ever collected. */
  had_coverage: boolean;
  summary: string;
}

/** A record and the network disagreeing about one address. */
export interface Mismatch {
  ip: string;
  scope: string;
  field: string;
  documented: string;
  observed: string;
  device_id: string;
  device: string;
  interface: string;
  collected_at: number;
}

/** An interface whose mask is not the length of the prefix it sits in. */
export interface MaskMismatch {
  ip: string;
  prefix_id: string;
  prefix: string;
  prefix_len: number;
  configured_len: number;
  device_id: string;
  device: string;
  interface: string;
  collected_at: number;
}

export interface PrefixNote {
  id: string;
  cidr: string;
  site: string | null;
  detail: string;
}

export interface Reconciliation {
  at: number;
  coverage: Coverage;
  /** Configured on a device, missing from the plan. */
  undocumented: Undocumented[];
  /** In the plan, never seen on the network. */
  unobserved: Unobserved[];
  mismatches: Mismatch[];
  mask_mismatches: MaskMismatch[];
  /** Prefixes that exist only because something was seen inside them. */
  prefixes_not_documented: PrefixNote[];
  /** Documented prefixes nothing has ever been seen in. */
  prefixes_without_evidence: PrefixNote[];
  conflicts: Conflict[];
  /** Any list that hit its cap (500 rows). */
  truncated: string[];
  /** What this report will not decide, and why. Worth showing. */
  refused: string[];
}

/**
 * `ipam.learn` params. Result: {@link LearnPlan}. The one call that turns
 * evidence into records — and it previews unless `dry_run` is `false`.
 */
export interface LearnParams {
  device_ids?: string[];
  /** Default `true`: nothing is written. */
  dry_run?: boolean;
  /** Also learn the hosts that answered in ARP. Default `false`. */
  include_arp?: boolean;
}

export interface LearnPrefix {
  cidr: string;
  site: string | null;
  from: Sighting[];
  /** `create` or `exists`. */
  action: string;
  id: string | null;
}

export interface LearnAddress {
  ip: string;
  scope: string;
  prefix: string | null;
  device_id: string;
  device: string;
  interface: string;
  kind: ObservationKind;
  mac: string | null;
  collected_at: number;
  /** `create`, `exists` or `skip`. */
  action: string;
  reason: string | null;
}

export interface LearnVlan {
  vid: number;
  name: string;
  site: string | null;
  devices: string[];
  action: string;
}

export interface LearnPlan {
  /** True when nothing was written, which is the default. */
  dry_run: boolean;
  prefixes: LearnPrefix[];
  addresses: LearnAddress[];
  vlans: LearnVlan[];
  created_prefixes: number;
  created_addresses: number;
  created_vlans: number;
  coverage: Coverage;
  notes: string[];
}

// ------------------------------------------------- VLANs, import, summary

/** `ipam.vlans` params. Result: {@link VlanRow}[]. */
export interface VlansParams {
  site?: string;
  query?: string;
}

/** `ipam.vlan.save` params. Result: {@link VlanRow}. */
export interface VlanInput {
  /** Absent creates. */
  id?: string;
  vid?: number;
  name?: string;
  site?: string;
  description?: string;
  prefix_ids?: string[];
}

/** Result of `ipam.vlan.delete` (`{id}`). */
export interface DeletedVlan {
  deleted: boolean;
  vid: number;
}

/** `ipam.import` params. Result: {@link ImportResult}. */
export interface ImportParams {
  /** `csv`. */
  format?: string;
  kind: 'prefixes' | 'addresses' | 'vlans';
  text: string;
  /** Default `true`: a preview, nothing written. */
  dry_run?: boolean;
}

export interface ImportRow {
  /** The line of the CSV, counting the header. */
  line: number;
  /** `new`, `update` or `invalid`. */
  status: string;
  problem: string | null;
  /** The parsed row, for the preview table. */
  value: unknown;
}

export interface ImportResult {
  kind: string;
  dry_run: boolean;
  rows: ImportRow[];
  new: number;
  update: number;
  invalid: number;
  created: number;
  updated: number;
}

/** `ipam.export` params. Result: `{kind, format, text}`. */
export interface ExportParams {
  kind: 'prefixes' | 'addresses' | 'vlans';
  /** `csv`. */
  format?: string;
  /** Add the addresses only the network knows about, labelled as such. */
  include_observed?: boolean;
}

export interface ExportResult {
  kind: string;
  format: string;
  text: string;
}

export interface Utilisation {
  prefix: string;
  prefix_id: string;
  util_pct: number;
  util_known: boolean;
  used: number;
  usable_text: string;
}

/** Result of `ipam.summary` (no params). */
export interface Summary {
  prefixes: number;
  /** Of those, the ones nobody has documented. */
  prefixes_discovered: number;
  /** Address *records*. */
  addresses: number;
  /** Sightings: a different number and a different thing. */
  observations: number;
  vlans: number;
  /** The ten fullest prefixes. */
  utilisation: Utilisation[];
  conflicts: number;
  /** Everything above is only as complete as this. */
  devices_never_collected: number;
  devices_total: number;
}

// ---------------------------------------------------------------- methods

/**
 * Every method of this feature, with its parameter and result types.
 *
 * | Method | Params | Result |
 * |---|---|---|
 * | `ipam.prefixes` | {@link PrefixesParams} | {@link PrefixRow}[] |
 * | `ipam.prefix.get` | {@link GetPrefixParams} | {@link PrefixDetail} |
 * | `ipam.prefix.save` | {@link PrefixInput} | {@link SavedPrefix} |
 * | `ipam.prefix.delete` | `{id: string}` | {@link DeletedPrefix} |
 * | `ipam.prefix.split` | {@link SplitParams} | {@link SplitResult} |
 * | `ipam.next_prefix` | {@link NextPrefixParams} | {@link NextPrefix} |
 * | `ipam.next_address` | {@link NextAddressParams} | {@link NextAddresses} |
 * | `ipam.address.save` | {@link AddressInput} | {@link SavedAddress} |
 * | `ipam.address.delete` | {@link AddressKeyParams} | {@link DeletedAddress} |
 * | `ipam.address.get` | {@link AddressKeyParams} | {@link AddressDetail} |
 * | `ipam.address.reserve` | {@link ReserveParams} | {@link SavedAddress} |
 * | `ipam.search` | {@link SearchParams} | {@link SearchResult} |
 * | `ipam.sync` | {@link SyncParams} | {@link SyncResult} |
 * | `ipam.reconcile` | {@link ReconcileParams} | {@link Reconciliation} |
 * | `ipam.learn` | {@link LearnParams} | {@link LearnPlan} |
 * | `ipam.conflicts` | — | {@link Conflict}[] |
 * | `ipam.vlans` | {@link VlansParams} | {@link VlanRow}[] |
 * | `ipam.vlan.save` | {@link VlanInput} | {@link VlanRow} |
 * | `ipam.vlan.delete` | `{id: string}` | {@link DeletedVlan} |
 * | `ipam.import` | {@link ImportParams} | {@link ImportResult} |
 * | `ipam.export` | {@link ExportParams} | {@link ExportResult} |
 * | `ipam.summary` | — | {@link Summary} |
 *
 * The one event: `ipam.changed` with {@link IpamChanged}. It arrives after a
 * save, a delete, an import, a learn and after the debounced sync that
 * follows `collect.updated`.
 */
export interface IpamChanged {
  /** Some of `prefixes`, `addresses`, `vlans`, `conflicts`. */
  kinds: string[];
}
