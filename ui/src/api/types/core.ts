/**
 * The core RPC surface (`nocturne-app`): the device inventory, credentials,
 * device-type profiles, settings, the vault, known SSH host keys, jobs,
 * collected device data and the audit trail.
 *
 * These types mirror the backend's parameter and result structs exactly —
 * field names are the backend's (`snake_case`), optional fields are `?`, and
 * every timestamp is milliseconds since the Unix epoch. Backend source:
 * `crates/nocturne-app/src/{inventory,model,connect,collect,credentials,
 * profiles,settings,system,jobs}.rs`, plus the vendor-neutral model in
 * `crates/nocturne-drivers/src/{model,profile,collect}.rs` and
 * `crates/nocturne-transport/src/connection.rs`.
 *
 * Each RPC method is documented as `method` {@link Params} → {@link Result}.
 * A result written as `null` is a real `null` from the backend, which always
 * means "there is nothing", never "zero".
 */

// ------------------------------------------------------------------ devices

/** How the CLI of a device is reached. */
export type Transport = 'ssh' | 'telnet' | 'serial' | 'sim' | 'none';

/** The node of a lab a simulated device stands for. */
export interface LabRef {
  lab_id: string;
  /** Node name inside the lab. */
  node: string;
}

/** How a device is monitored. */
export interface MonitorSpec {
  enabled: boolean;
  /** Reachability and latency by ICMP echo. */
  icmp: boolean;
  /** Interface counters and health over SNMP. */
  snmp: boolean;
  /** Credential holding the SNMP community / v3 user; defaults to the device credential. */
  snmp_credential_id?: string | null;
  snmp_port?: number | null;
  /** Seconds between polls (5…86400). */
  interval_s: number;
}

/** A device under management. */
export interface Device {
  id: string;
  /** Display name; unique, case-insensitively. */
  name: string;
  /** Hostname or IP address (or the serial port for `serial`). */
  address: string;
  /** Device profile id (`cisco_ios`, …). Empty = detect on first connection. */
  profile: string;
  transport: Transport;
  /** TCP port (or baud rate for serial); `null` = the transport's default. */
  port?: number | null;
  credential_id?: string | null;
  /** Reach this device through another one (SSH jump host). */
  jump_device_id?: string | null;
  site: string;
  /** `core`, `distribution`, `access`, `edge`, `firewall`, … (free text, lower-cased). */
  role: string;
  tags: string[];
  notes: string;
  monitor: MonitorSpec;
  /** Set for simulated devices. */
  lab?: LabRef | null;
  /** Never changed by automation or the AI without an explicit human approval. */
  protected: boolean;
  created_at: number;
  updated_at: number;
}

/** Facts a device reported about itself (`nocturne_drivers::model::Facts`). */
export interface Facts {
  hostname: string;
  vendor: string;
  model: string;
  /** Operating system family: `IOS-XE`, `RouterOS`, `Junos`. */
  os: string;
  os_version: string;
  serial: string;
  uptime_s?: number | null;
  /** Anything else worth keeping (image file, config register, stack members…). */
  extra: Record<string, string>;
}

/** What Nocturne knows about a device, updated by monitoring and collection. */
export interface DeviceState {
  device_id: string;
  /** `null` until the first check — not "down". */
  reachable?: boolean | null;
  rtt_ms?: number | null;
  last_seen?: number | null;
  last_checked?: number | null;
  last_error?: string | null;
  facts?: Facts | null;
  facts_at?: number | null;
  /** Profile chosen by auto-detection when the device's own is empty. */
  detected_profile?: string | null;
}

/**
 * A device together with its state, as lists return it. The backend flattens
 * the device, so its fields sit at the top level next to `state`.
 */
export interface DeviceRow extends Device {
  state: DeviceState;
}

/** Fields the UI may set. Absent = leave unchanged (update) or default (create). */
export interface DeviceInput {
  name?: string;
  address?: string;
  profile?: string;
  transport?: Transport;
  /** `0` clears the port back to the transport's default. */
  port?: number;
  /** An empty string clears the credential. */
  credential_id?: string;
  jump_device_id?: string;
  site?: string;
  role?: string;
  tags?: string[];
  notes?: string;
  monitor?: MonitorSpec;
  lab?: LabRef;
  protected?: boolean;
}

export interface DeviceFilter {
  /** Free text over name, address, site, role, tags, profile, model and serial. */
  query?: string;
  site?: string;
  role?: string;
  tag?: string;
  profile?: string;
  /** `up`, `down` or `unknown`. */
  status?: string;
  /** `true` only lab devices, `false` only real ones. */
  lab?: boolean;
}

/** `devices.list` {@link DeviceFilter} → {@link DeviceRow}[] (natural order by name) */

export interface DeviceIdParams {
  /** An id; the backend also accepts a device's name or address. */
  id: string;
}

/** `devices.get` {@link DeviceIdParams} → {@link DeviceRow} */

/** `devices.create` {@link DeviceInput} → {@link DeviceRow} */

export interface DeviceUpdateParams extends DeviceInput {
  id: string;
}

/** `devices.update` {@link DeviceUpdateParams} → {@link DeviceRow} */

export interface DeviceIdsParams {
  ids: string[];
}

/** How many records an operation touched. */
export interface CountResult {
  count: number;
}

/** `devices.delete` {@link DeviceIdsParams} → {@link CountResult} */

/** A change applied to every selected device at once. */
export interface BulkPatch {
  site?: string;
  role?: string;
  profile?: string;
  credential_id?: string;
  protected?: boolean;
  monitor_enabled?: boolean;
  add_tags?: string[];
  remove_tags?: string[];
}

export interface DeviceBulkUpdateParams extends BulkPatch {
  ids: string[];
}

/** `devices.bulk_update` {@link DeviceBulkUpdateParams} → {@link CountResult} */

/** A facet value and how many devices carry it. */
export type FacetCount = [value: string, count: number];

/** Counts per site, role, tag and profile, for the filter chips. */
export interface Facets {
  total: number;
  lab: number;
  sites: FacetCount[];
  roles: FacetCount[];
  tags: FacetCount[];
  profiles: FacetCount[];
}

/** `devices.facets` (no params) → {@link Facets} */

// ------------------------------------------------------------ import/export

/** What an import file is written in. `auto` (or absent) detects it. */
export type ImportFormat = 'auto' | 'csv' | 'ssh_config' | 'ansible';

/** What a row of an import will do when committed. */
export type ImportRowStatus = 'new' | 'update' | 'invalid';

/** One device parsed from an import, before it is committed. */
export interface ImportRow {
  input: DeviceInput;
  /** Name of the credential to attach, resolved at commit time. */
  credential_name?: string | null;
  status: ImportRowStatus;
  /** Why the row is invalid. */
  problem?: string | null;
}

export interface ImportPreview {
  /** The format that was used, detected when `auto` was asked for. */
  format: string;
  rows: ImportRow[];
  new: number;
  update: number;
  invalid: number;
}

export interface ImportParams {
  /** Empty or `auto` detects the format. */
  format?: ImportFormat | string;
  text: string;
  /** Apply rows whose device already exists; otherwise they are skipped. */
  update_existing?: boolean;
}

/** `devices.import_preview` {@link ImportParams} → {@link ImportPreview} (changes nothing) */

export interface ImportResult {
  created: number;
  updated: number;
  /** One sentence per row that was not imported, and why. */
  skipped: string[];
}

/** `devices.import` {@link ImportParams} → {@link ImportResult} */

export interface ExportParams {
  /** `csv` or `ansible`. Lab devices are never exported. */
  format: 'csv' | 'ansible' | string;
  /** Only these devices; absent exports all of them. */
  ids?: string[];
}

export interface ExportResult {
  text: string;
}

/** `devices.export` {@link ExportParams} → {@link ExportResult} */

// ---------------------------------------------------- connecting and testing

/** How a session reaches the far end (`nocturne_transport::TransportKind`). */
export type TransportKind = 'ssh' | 'telnet' | 'serial' | 'local' | 'sim';

/** What the transport negotiated. */
export interface ConnectionInfo {
  kind?: TransportKind | null;
  /** `10.0.0.1:22`, `COM3`, `lab:R1`. */
  peer: string;
  /** The SSH identification string, then the pre-authentication banner. */
  server_banner?: string | null;
  kex?: string | null;
  cipher?: string | null;
  /** `null` when the cipher authenticates itself (AEAD). */
  mac?: string | null;
  host_key_algorithm?: string | null;
  /** `SHA256:…` */
  host_key_fingerprint?: string | null;
  /** The session was negotiated only thanks to the legacy algorithm set. */
  legacy_crypto: boolean;
  /** Jump hosts traversed, first hop first. */
  via: string[];
}

/** How a device type was worked out. */
export type DetectMethod = 'snmp' | 'banner' | 'probe';

/** Which device type a live session turned out to be, and on what evidence. */
export interface Detection {
  profile_id: string;
  method: DetectMethod;
  /** The text that matched, trimmed to about 200 characters. */
  evidence: string;
}

/**
 * An error as the backend reports it inside a result (not as a failed call).
 *
 * For `code: "host_key"` the `details` are a {@link HostKeyEvent}: both
 * fingerprints, so the interface can show them and offer to trust the new key
 * without having listened for the event.
 */
export interface AppErrorBody {
  /** Stable machine string: `not_found`, `invalid`, `auth`, `host_key`, `unreachable`, … */
  code: string;
  message: string;
  details?: unknown;
}

/** How far "Test connection" got before it failed. */
export type TestStage = 'connect' | 'login';

/** The outcome of "Test connection". A failed test is a successful call. */
export interface ConnectionTest {
  ok: boolean;
  /** `connect` or `login`; absent when the test succeeded. */
  failed_at?: TestStage | null;
  error?: AppErrorBody | null;
  elapsed_ms: number;
  info?: ConnectionInfo | null;
  /** The name the device calls itself at its prompt. */
  hostname?: string | null;
  /** The device type the session ended up bound to. */
  profile?: string | null;
  /** The device type was worked out during this test rather than configured. */
  profile_detected: boolean;
  detection?: Detection | null;
  /** Plain sentences: legacy crypto, a profile that only half fits, … */
  warnings: string[];
}

/** `devices.test` {@link DeviceIdParams} → {@link ConnectionTest} */

/** One command and what the device printed back. */
export interface CommandOutput {
  command: string;
  output: string;
  /** The line that matched one of the platform's error patterns, if any. */
  error?: string | null;
  elapsed_ms: number;
}

export interface DeviceExecParams {
  id: string;
  /** Read-only commands only; at most 50. The whole batch is refused up front. */
  commands?: string[];
  /** Convenience for a single command. */
  command?: string;
}

export interface DeviceExecResult {
  outputs: CommandOutput[];
}

/** `devices.exec` {@link DeviceExecParams} → {@link DeviceExecResult} */

// ---------------------------------------------------------------- host keys

/** An SSH host key Nocturne has accepted. */
export interface KnownHost {
  host: string;
  port: number;
  algorithm: string;
  /** `SHA256:…` */
  fingerprint: string;
  /** The public key in OpenSSH format. */
  openssh: string;
  first_seen: number;
  last_seen: number;
}

/** `hostkeys.list` (no params) → {@link KnownHost}[] (by host, then port) */

export interface HostKeyRef {
  host: string;
  /** 22 when left out. */
  port?: number;
}

export interface HostKeyForgetResult {
  removed: boolean;
}

/** `hostkeys.forget` {@link HostKeyRef} → {@link HostKeyForgetResult} */

export interface HostKeyTrustParams extends HostKeyRef {
  algorithm: string;
  /** Must be in the `SHA256:…` form. */
  fingerprint: string;
  openssh?: string;
}

/** `hostkeys.trust` {@link HostKeyTrustParams} → {@link KnownHost} */

/** Payload of the `connect.host_key_changed` and `connect.host_key_unknown` events. */
export interface HostKeyEvent {
  host: string;
  port: number;
  /** Absent on `connect.host_key_unknown`: there was nothing to compare with. */
  known?: {
    algorithm: string;
    fingerprint: string;
    first_seen: number;
  };
  presented: {
    algorithm: string;
    fingerprint: string;
    openssh: string;
  };
}

// -------------------------------------------------------- collected data

/** The kinds of structured data a profile can collect. */
export type Kind =
  | 'facts'
  | 'interfaces'
  | 'neighbors'
  | 'vlans'
  | 'mac_table'
  | 'arp'
  | 'routes'
  | 'health'
  | 'inventory';

/** Every kind, in the backend's order. */
export const KINDS: readonly Kind[] = [
  'facts',
  'interfaces',
  'neighbors',
  'vlans',
  'mac_table',
  'arp',
  'routes',
  'health',
  'inventory',
];

/** Interface counters, exactly as the device reports them. */
export interface Counters {
  in_octets?: number | null;
  out_octets?: number | null;
  in_packets?: number | null;
  out_packets?: number | null;
  in_errors?: number | null;
  out_errors?: number | null;
  in_discards?: number | null;
  out_discards?: number | null;
  crc?: number | null;
  /** Rates as the device computes them (often a 5-minute average). */
  in_rate_bps?: number | null;
  out_rate_bps?: number | null;
}

/** A row of `interfaces`. */
export interface Interface {
  name: string;
  description: string;
  admin_up?: boolean | null;
  oper_up?: boolean | null;
  speed_bps?: number | null;
  duplex: string;
  mtu?: number | null;
  mac: string;
  /** Addresses in CIDR form, e.g. `10.0.0.1/24`. */
  ipv4: string[];
  ipv6: string[];
  /** `access`, `trunk`, `routed`, or empty when unknown. */
  mode: string;
  /** Access VLAN, or the native VLAN on a trunk. */
  vlan?: number | null;
  /** Hardware or media type as the device reports it. */
  kind: string;
  counters: Counters;
}

/** A row of `neighbors`. */
export interface Neighbor {
  local_interface: string;
  device: string;
  remote_interface: string;
  platform: string;
  address: string;
  /** `cdp`, `lldp`, `mndp`, … */
  protocol: string;
  capabilities: string;
}

/** A row of `vlans`. */
export interface Vlan {
  id?: number | null;
  name: string;
  status: string;
  interfaces: string[];
}

/** A row of `mac_table`. */
export interface MacEntry {
  vlan?: number | null;
  mac: string;
  interface: string;
  /** `dynamic`, `static`, … */
  kind: string;
}

/** A row of `arp`. */
export interface ArpEntry {
  ip: string;
  mac: string;
  interface: string;
  age: string;
}

/** A row of `routes`. */
export interface Route {
  /** Destination in CIDR form. */
  prefix: string;
  /** `connected`, `static`, `ospf`, `bgp`, … as the device labels it. */
  protocol: string;
  next_hop: string;
  interface: string;
  distance?: number | null;
  metric?: number | null;
  vrf: string;
}

/** The single row of `health`. */
export interface Health {
  cpu_pct?: number | null;
  memory_used?: number | null;
  memory_total?: number | null;
  temperature_c?: number | null;
  uptime_s?: number | null;
}

/** A row of `inventory`: one physical part. */
export interface InventoryItem {
  name: string;
  description: string;
  part_id: string;
  serial: string;
}

/** The row type each kind stores. */
export interface KindRow {
  facts: Facts;
  interfaces: Interface;
  neighbors: Neighbor;
  vlans: Vlan;
  mac_table: MacEntry;
  arp: ArpEntry;
  routes: Route;
  health: Health;
  inventory: InventoryItem;
}

/** One kind of data as last read from one device. */
export interface Stored<K extends Kind = Kind> {
  device_id: string;
  kind: K;
  collected_at: number;
  /** Where it came from: `cli`, `snmp`, `lab`, … */
  source: string;
  /** `facts` and `health` hold exactly one row; the others hold a table. */
  rows: KindRow[K][];
}

export interface CollectGetParams {
  id: string;
  kind: Kind;
  /** The generation before the latest, for "what changed since last time". */
  previous?: boolean;
}

/**
 * `collect.get` {@link CollectGetParams} → {@link Stored} | `null`
 *
 * `null` means this kind was never collected from this device. It is never an
 * empty table: a kind that could not be read is not stored at all.
 */

/** What has been collected from a device, without the rows. */
export interface KindStatus {
  kind: Kind;
  collected_at: number;
  source: string;
  rows: number;
}

/** `collect.status` {@link DeviceIdParams} → {@link KindStatus}[] (only kinds that exist) */

export interface CollectRunParams {
  device_ids?: string[];
  /** Convenience for a single device. */
  id?: string;
  /** Empty or absent = everything each device's profile defines. */
  kinds?: Kind[];
  /** Who asked: `user` (default), `schedule`, `assistant`. */
  origin?: string;
}

/** A job that has been started. Follow it with `jobs.get` and `jobs.changed`. */
export interface JobStarted {
  job_id: string;
}

/** `collect.run` {@link CollectRunParams} → {@link JobStarted} */

/** Why a kind is missing from a collection. */
export type ProblemCode =
  /** The profile defines no steps for this kind: the device type cannot collect it. */
  | 'unsupported'
  /** No step that creates rows succeeded. */
  | 'failed'
  /** Some steps, rows or fields could not be collected or mapped. */
  | 'partial';

export interface Problem {
  kind: Kind;
  code: ProblemCode;
  message: string;
}

/** One command a collection ran. */
export interface StepReport {
  kind: Kind;
  command: string;
  ok: boolean;
  /** Rows this step contributed after filtering. */
  rows: number;
  error?: string | null;
  elapsed_ms: number;
  /** An optional step that failed, or one that had nothing to add. */
  skipped: boolean;
}

/** The outcome of collecting from one device, inside a `collect.run` job result. */
export interface CollectDeviceOutcome {
  device_id: string;
  name: string;
  collected: Kind[];
  problems: Problem[];
  /** Set when the device could not be reached or logged in to at all. */
  error?: string | null;
}

/** The `result` of a finished `collect.run` job. */
export interface CollectRunResult {
  devices: CollectDeviceOutcome[];
  failed: number;
}

/** Payload of the `collect.updated` event. */
export interface CollectUpdatedEvent {
  device_id: string;
  kinds: Kind[];
  at: number;
  source: string;
}

// -------------------------------------------------------------- credentials

/** The non-secret half of SNMP access. */
export interface SnmpAccess {
  /** `""` (none), `v1`, `v2c` or `v3`. */
  version: string;
  has_community: boolean;
  /** SNMPv3 user name. */
  user: string;
  /** `md5`, `sha1`, `sha224`, `sha256`, `sha384`, `sha512`, or empty. */
  auth_protocol: string;
  has_auth_password: boolean;
  /** `des`, `aes128`, `aes192`, `aes256`, or empty. */
  priv_protocol: string;
  has_priv_password: boolean;
}

/**
 * A stored login. Secrets live in the vault and are never part of this record:
 * the `has_*` flags are all the UI ever learns about them.
 */
export interface Credential {
  id: string;
  name: string;
  username: string;
  has_password: boolean;
  has_enable_secret: boolean;
  has_private_key: boolean;
  has_passphrase: boolean;
  snmp: SnmpAccess;
  notes: string;
  created_at: number;
  updated_at: number;
}

/**
 * What the UI sends to create or update a credential. For each secret:
 * absent = leave as it is, `''` = remove it, anything else = set it.
 */
export interface CredentialInput {
  name?: string;
  username?: string;
  notes?: string;
  password?: string;
  enable_secret?: string;
  private_key?: string;
  passphrase?: string;
  /** `v1`, `v2c`, `v3`, or `''` / `none` for no SNMP. */
  snmp_version?: string;
  snmp_community?: string;
  snmp_user?: string;
  snmp_auth_protocol?: string;
  snmp_auth_password?: string;
  snmp_priv_protocol?: string;
  snmp_priv_password?: string;
}

/** `credentials.list` (no params) → {@link Credential}[] (by name) */

/** `credentials.create` {@link CredentialInput} → {@link Credential} */

export interface CredentialUpdateParams extends CredentialInput {
  id: string;
}

/** `credentials.update` {@link CredentialUpdateParams} → {@link Credential} */

export interface DeletedResult {
  deleted: boolean;
}

/**
 * `credentials.delete` {@link DeviceIdParams} → {@link DeletedResult}
 *
 * Refused with `conflict` while a device still uses the credential; the
 * message names the devices.
 */

// ------------------------------------------------------------- device types

/** Where a profile comes from. A user profile of the same id replaces a built-in. */
export type Origin = 'builtin' | 'user';

/** How far a profile has been checked against the real platform. */
export type Maturity =
  /** Checked against recorded output of the real platform by the repository's tests. */
  | 'verified'
  /** Written from documentation and other tools; not yet checked against a device. */
  | 'community'
  /** A draft. */
  | 'experimental';

/** The grammar a configuration is written in. */
export type Syntax = 'ios' | 'vrp' | 'set' | 'routeros' | 'flat';

/** How collected interface names are spelled. */
export type InterfaceNames = 'cisco' | 'verbatim';

/** A device type as the list shows it. */
export interface ProfileSummary {
  id: string;
  name: string;
  vendor: string;
  os: string;
  description: string;
  origin: Origin;
  maturity: Maturity;
  extends?: string | null;
  /** What the profile can collect. */
  collects: Kind[];
  can_configure: boolean;
  syntax: Syntax;
}

/** A row of `profiles.list`. */
export interface ProfileRow extends ProfileSummary {
  /** A user profile that replaces a built-in of the same id. */
  overrides_builtin: boolean;
  /** Devices currently using this profile. */
  devices: number;
}

/** A profile that could not be loaded, and why. */
export interface ProfileProblem {
  id: string;
  origin: Origin;
  problems: string[];
}

export interface ProfileList {
  profiles: ProfileRow[];
  problems: ProfileProblem[];
}

/** `profiles.list` (no params) → {@link ProfileList} */

/** One command a profile runs and how its output becomes model rows. */
export interface CollectStep {
  command: string;
  /** `kv`, `colon`, `raw`, or a one-key map: `{ntc}`, `{textfsm}`, `{regex}`, `{json}`. */
  parser: string | Record<string, string>;
  /** Model field → expression over the parsed row. */
  map: Record<string, string>;
  when?: string | null;
  /** A failing optional step is skipped instead of failing the collection. */
  optional: boolean;
  merge_on?: string | null;
}

/** A dead-man switch the platform offers. */
export interface Guard {
  /** `reload timer`, `commit confirmed`, … */
  name: string;
  /** Exec commands that arm it; `{minutes}` is substituted. */
  arm: string[];
  /** Exec commands that cancel it once the change is verified. */
  disarm: string[];
  /** What happens if it fires, in words. */
  effect: string;
}

/** How configuration is read and written on a platform. */
export interface ConfigSpec {
  syntax: Syntax;
  /** Commands that enter configuration mode. Empty = the platform has no config mode. */
  enter: string[];
  exit: string[];
  commit: string[];
  abort: string[];
  save: string[];
  /** The command that prints the running configuration. */
  running: string;
  startup?: string | null;
  ignore: string[];
  errors: string[];
  answers: { prompt: string; answer: string }[];
  guard?: Guard | null;
  secrets: string[];
}

/** Which commands the platform counts as read-only, and which as dangerous. */
export interface PolicySpec {
  read_only: string[];
  dangerous: string[];
  /** The CLI is a POSIX shell, so chained commands are each classified. */
  shell: boolean;
}

/** How to recognise the platform. */
export interface Detect {
  banner: string[];
  sysdescr: string[];
  /** `sysObjectID` prefixes, e.g. `1.3.6.1.4.1.9.`. */
  sysobjectid: string[];
  probes: { command: string; matches: string }[];
  /** Higher wins when several profiles match. */
  priority: number;
}

/** Interactive session behaviour. */
export interface CliSpec {
  prompt: string;
  user_prompt?: string | null;
  config_prompt?: string | null;
  enable?: { command: string; password_prompt: string } | null;
  login: {
    username_prompt: string;
    password_prompt: string;
    failure: string;
    username_suffix: string;
  };
  init: string[];
  pager?: { pattern: string; key: string } | null;
  newline: string;
  exit: string;
  errors: string[];
  answers: { prompt: string; answer: string }[];
  unsolicited: string[];
  timeout_s: number;
}

/**
 * A test on a sibling column of an {@link SnmpEntry}, one row at a time:
 * exactly one of `is`, `in` and `matches`. Values are text.
 */
export interface SnmpTest {
  oid: string;
  is?: string;
  in?: string[];
  matches?: string;
}

/**
 * Where one health reading is taken from, when a bare OID would not say
 * enough. Only the keys an entry uses are present. See docs/profiles.md,
 * section `snmp`.
 */
export interface SnmpEntry {
  /** An object (ending in `.0`) or a column. */
  oid?: string;
  /** Exactly this OID, fetched as written. */
  instance?: string;
  /** The agent sends a signed 32-bit object in an unsigned type. Present only when `true`. */
  signed?: boolean;
  where?: SnmpTest[];
  /** Raw values that mean "no reading". */
  absent?: number[];
  si_scale?: string;
  precision?: string;
  scale?: number;
  complement?: number;
  /** `cpu` only. */
  combine?: 'mean' | 'max';
  /** `temperature` only. */
  label?: string;
  /** `temperature` only: read only where the entries above gave no sensor. Present only when `true`. */
  fallback?: boolean;
}

/** Where to find health data over SNMP: each entry a bare OID or an {@link SnmpEntry}. */
export interface SnmpSpec {
  cpu: (string | SnmpEntry)[];
  memory_used: (string | SnmpEntry)[];
  memory_free: (string | SnmpEntry)[];
  memory_total: (string | SnmpEntry)[];
  /** An object that is already "percent of memory in use". */
  memory_percent: (string | SnmpEntry)[];
  temperature: (string | SnmpEntry)[];
}

/** A device type with inheritance applied. */
export interface Profile {
  id: string;
  name: string;
  vendor: string;
  os: string;
  description: string;
  extends?: string | null;
  ntc_platform?: string | null;
  maturity: Maturity;
  interface_names: InterfaceNames;
  detect: Detect;
  cli: CliSpec;
  config: ConfigSpec;
  policy: PolicySpec;
  /** Kind → the steps that collect it. A kind that is absent cannot be collected. */
  collect: Partial<Record<Kind, CollectStep[]>>;
  snmp: SnmpSpec;
}

export interface ProfileDetail {
  id: string;
  origin: Origin;
  /** The YAML as written, before `extends` is applied. */
  yaml: string;
  /** The profile with inheritance applied. */
  resolved: Profile;
  /** When the user profile was last saved; absent for a built-in. */
  updated_at?: number | null;
}

/** `profiles.get` {@link DeviceIdParams} → {@link ProfileDetail} */

export interface ProfileYamlParams {
  yaml: string;
}

export interface ProfileValidation {
  ok: boolean;
  /** The id the YAML declares, even when it is wrong. */
  id: string;
  /** One sentence per problem, each prefixed with the field it is about. */
  problems: string[];
  /** The resolved profile, when it is valid. */
  resolved?: Profile | null;
}

/** `profiles.validate` {@link ProfileYamlParams} → {@link ProfileValidation} (saves nothing) */

/** `profiles.save` {@link ProfileYamlParams} → {@link ProfileDetail} */

export interface ProfileDeleted {
  /** The built-in profile of the same id is active again. */
  restored_builtin: boolean;
  /** Devices that were using the profile and now have none. */
  devices_affected: number;
}

/** `profiles.delete` {@link DeviceIdParams} → {@link ProfileDeleted} (built-ins cannot be deleted) */

// ----------------------------------------------------------------- settings

/** Preferences of the application as a whole (`settings.get {section: 'general'}`). */
export interface GeneralSettings {
  /** `night`, `day` or `system`. */
  theme: string;
  /** `compact` or `comfortable`. */
  density: string;
  /** `24h` or `12h`. */
  time_format: string;
  /** Desktop app: closing the window hides it to the tray. */
  keep_running_in_tray: boolean;
}

/** How devices are reached (`settings.get {section: 'connections'}`). */
export interface ConnectionSettings {
  connect_timeout_s: number;
  /** Seconds between SSH keepalives; `0` turns them off. */
  keepalive_s: number;
  /** Seconds allowed from "connected" to the first usable prompt. */
  login_timeout_s: number;
  /** `auto` (fall back and flag the session), `never` or `always`. */
  legacy_crypto: string;
  /**
   * `tofu` (remember the key on first contact, refuse a changed one),
   * `strict` (refuse unknown keys) or `accept` (no checking — lab use only).
   */
  host_key_policy: string;
  /** Automated sessions Nocturne may hold to one device at a time. */
  max_sessions_per_device: number;
}

export interface SettingsGetParams {
  section: string;
}

/**
 * `settings.get` {@link SettingsGetParams} → the section as an object
 * (stored values over the defaults). An unknown section is `not_found`.
 */

export interface SettingsSetParams {
  section: string;
  /** The keys to change; objects merge key by key, anything else is replaced. */
  value: Record<string, unknown>;
}

/** `settings.set` {@link SettingsSetParams} → the whole section after the merge */

/** `settings.all` (no params) → `Record<string, object>`, one entry per declared section */

// ------------------------------------------------- application, vault, audit

/** Whether the credential vault can be used. */
export type VaultState =
  /** Usable; the key file is not password-protected. */
  | 'open'
  /** Usable; a master password protects the key file. */
  | 'unlocked'
  /** A master password is needed before any secret can be read or written. */
  | 'locked';

export interface AppInfo {
  name: string;
  version: string;
  started_at: number;
  /** Where the database and key file live; `null` for an in-memory instance. */
  data_dir?: string | null;
  vault: VaultState;
  os: string;
  arch: string;
  /** How many RPC methods this build registers. */
  methods: number;
  /**
   * May this instance act on the machine it runs on?
   *
   * False in server mode, where the person is at the other end of a browser
   * and the machine is not theirs: a TFTP or syslog listener bound there
   * would be the server's, not the user's, so both are refused. A screen that
   * offers one has to ask before it offers it.
   */
  local_access: boolean;
}

/** `app.info` (no params) → {@link AppInfo} */

/** `app.methods` (no params) → `string[]`: every method this build answers */

export interface VaultStatus {
  state: VaultState;
}

/** `vault.state` (no params) → {@link VaultStatus} */

export interface VaultUnlockParams {
  password: string;
}

/** `vault.unlock` {@link VaultUnlockParams} → {@link VaultStatus} */

/** `vault.lock` (no params) → {@link VaultStatus}; refused while no master password is set */

export interface VaultSetPasswordParams {
  /** Required when a master password is already set. */
  current?: string;
  /** At least 8 characters. Absent or empty removes the master password. */
  new?: string;
}

/** `vault.set_password` {@link VaultSetPasswordParams} → {@link VaultStatus} */

/** One entry of a log stream; the audit trail is the `audit` stream. */
export interface LogEntry {
  id: number;
  ts: number;
  /** `syslog`, `trap`, `audit`, `event`, … */
  stream: string;
  device_id?: string | null;
  /** Syslog-style severity, 0 (emergency) … 7 (debug). */
  severity?: number | null;
  source: string;
  message: string;
  /** Structured payload, stream-specific. */
  body?: unknown;
}

export interface LogQuery {
  stream?: string;
  device_id?: string;
  /** Keep entries at least this severe (numerically ≤). */
  max_severity?: number;
  /** Case-insensitive substring of the message or source. */
  text?: string;
  since?: number;
  until?: number;
  /** Page backwards: only entries with a smaller id. */
  before_id?: number;
  limit?: number;
}

export interface AuditList {
  entries: LogEntry[];
}

/** `audit.list` {@link LogQuery} → {@link AuditList} (the `audit` stream, newest first, ≤ 1000) */

// --------------------------------------------------------------------- jobs

export type JobState = 'running' | 'succeeded' | 'failed' | 'cancelled';

export interface JobLine {
  ts: number;
  /** `info`, `warn` or `error`. */
  level: string;
  message: string;
  device_id?: string;
}

/** Anything that takes longer than a click. */
export interface Job {
  id: string;
  /** Machine name of the operation: `collect.run`, `config.backup`, … */
  kind: string;
  title: string;
  state: JobState;
  /** Who started it: `user`, `schedule`, `assistant` or `system`. */
  origin: string;
  device_ids: string[];
  done: number;
  /** `0` while the amount of work is unknown. */
  total: number;
  /** What it is doing right now. */
  message: string;
  started_at: number;
  finished_at?: number | null;
  error?: string | null;
  /** Whatever the operation returned, once it has finished. */
  result?: unknown;
  /** Empty in `jobs.list`; `jobs.get` carries the log. */
  log: JobLine[];
  /** Log lines dropped from the middle of a very long log. */
  log_dropped: number;
}

export interface JobFilter {
  /** `running`, `finished`, or a single state name. */
  state?: string;
  /** An exact kind, or a prefix ending in `.` or `*` (`config.`). */
  kind?: string;
  device_id?: string;
  /** 1…1000; 100 by default. */
  limit?: number;
}

/** `jobs.list` {@link JobFilter} → {@link Job}[] (newest first, without logs and results) */

/** `jobs.get` {@link DeviceIdParams} → {@link Job} (with its log and result) */

export interface JobCancelled {
  /** False when the job had already finished. */
  cancelled: boolean;
}

/** `jobs.cancel` {@link DeviceIdParams} → {@link JobCancelled} */

export interface JobsCleared {
  removed: number;
}

/** `jobs.clear` (no params) → {@link JobsCleared}: forgets every finished job */

// ------------------------------------------------------------------- events

/** Payload of `devices.changed`. */
export interface DevicesChangedEvent {
  ids: string[];
  /** `created`, `updated`, `deleted` or `imported`. */
  change: string;
}

/** Payload of `credentials.changed`. */
export interface CredentialsChangedEvent {
  id: string;
  deleted?: boolean;
}

/** Payload of `profiles.changed`. */
export interface ProfilesChangedEvent {
  id: string;
  /** `created`, `updated` or `deleted`. */
  change: string;
}

/** Payload of `settings.changed`. */
export interface SettingsChangedEvent {
  section: string;
  value: Record<string, unknown>;
}

/**
 * The topics this surface publishes:
 *
 * | Topic | Payload |
 * |---|---|
 * | `devices.changed` | {@link DevicesChangedEvent} |
 * | `devices.state` | {@link DeviceState} |
 * | `collect.updated` | {@link CollectUpdatedEvent} |
 * | `credentials.changed` | {@link CredentialsChangedEvent} |
 * | `profiles.changed` | {@link ProfilesChangedEvent} |
 * | `settings.changed` | {@link SettingsChangedEvent} |
 * | `vault.changed` | {@link VaultStatus} |
 * | `jobs.changed` | {@link Job} (a summary, without the log) |
 * | `connect.host_key_changed` | {@link HostKeyEvent} |
 * | `connect.host_key_unknown` | {@link HostKeyEvent} |
 * | `audit.appended` | {@link LogEntry} |
 */
