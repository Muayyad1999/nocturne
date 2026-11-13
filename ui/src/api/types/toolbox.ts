/**
 * Toolbox (`nocturne-toolbox`): the calculators, decoders, converters and
 * reference tables, the probes that leave this machine, the probes a device
 * runs for us, and the two small servers.
 *
 * These types mirror the backend's parameter and result structs exactly —
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and a field the backend declares as optional is written
 * `T | null` because it is present in the JSON with a `null` value. Backend
 * source: `crates/nocturne-toolbox`. Specification:
 * `docs/dev/features/toolbox.md`.
 *
 * Four things run through the whole surface, and the screens should show them
 * rather than smooth them over:
 *
 * * **Offline or not.** `tools.*` is arithmetic: instant, deterministic, safe
 *   to call on every keystroke. `probe.*` and `tftp.*` put packets on the
 *   wire or bind a socket. `tools.list` says which is which (`offline`).
 * * **A refusal is not a result.** Bad input comes back as an error with the
 *   engine's own message (`code: 'invalid'`), never as a default answer. A
 *   probe that could not run comes back as an error too, and its history
 *   record has `ok: false`, `result: null` and an `error_code` that
 *   distinguishes "this machine may not do that" (`forbidden`) from "nothing
 *   answered" (`timeout`). 100 % packet loss means the network was silent; it
 *   never means the probe did not happen.
 * * **Where it ran.** Every probe result carries `from`: `this computer` in
 *   the desktop app, `the Nocturne server` behind a browser — and a ping also
 *   carries the `method` the operating system allowed. A device probe's
 *   `from` is the device itself.
 * * **The inventory knows.** A probe can be aimed at `device` (an id, a name
 *   or an address) instead of a `host`, and an address that comes back can
 *   carry the `DeviceHint` of the device it belongs to — exactly, by address,
 *   never by "it is in that subnet".
 */

// --------------------------------------------------------------- vocabulary

/** `tools.list` groups, in the order a tool drawer shows them. */
export type ToolGroup =
  | 'addressing'
  | 'identity'
  | 'secrets'
  | 'converters'
  | 'reference'
  | 'text'
  | 'probes'
  | 'device'
  | 'history'
  | 'servers';

/** One entry of the catalogue. `tools.list` — no params, result `Tool[]`. */
export interface Tool {
  /** The RPC method name, which is also the tool's identifier. */
  id: string;
  title: string;
  group: ToolGroup;
  description: string;
  /** Arithmetic: no socket, no device, no clock. */
  offline: boolean;
  /** The answer arrives as a job (`{job_id}`); follow the events. */
  job: boolean;
}

/** A device of the inventory that an address belongs to. */
export interface DeviceHint {
  id: string;
  name: string;
  /** The address that matched. */
  address: string;
  role: string;
  site: string;
  /** A simulated device: lab data is always labelled as lab data. */
  lab: boolean;
  /** `the device's address in the inventory` or `interface Vlan10`. */
  matched: string;
}

/** Where a probe ran from. */
export type ProbeFrom = 'this computer' | 'the Nocturne server' | string;

// -------------------------------------------------------------- calculators

/** Params of `tools.subnet`, `tools.wildcard`, `tools.mac`, `tools.oid`, `tools.interface`. */
export interface InputParams {
  input: string;
}

/** `tools.subnet` — params `InputParams`, result `SubnetInfo`. */
export interface SubnetInfo {
  /** 4 or 6. */
  version: number;
  /** The host address as it was given. */
  address: string;
  network: string;
  prefix: number;
  cidr: string;
  /** IPv4 only; `""` for IPv6. */
  netmask: string;
  wildcard: string;
  /** `""` for a /31, a /32 and for IPv6. */
  broadcast: string;
  first_host: string;
  last_host: string;
  /** A decimal string: an IPv6 count does not fit in a number. */
  usable_hosts: string;
  total_addresses: string;
  /** The classful letter A–E for IPv4, `""` for IPv6. */
  class: string;
  /** `private (RFC 1918)`, `public`, `loopback`, `CGNAT (RFC 6598)`, … */
  scope: string;
  binary_address: string;
  binary_netmask: string;
  hex: string;
  integer: string;
  reverse_dns: string;
  /** IPv6 fully expanded; the address itself for IPv4. */
  expanded: string;
  /** IPv6 in RFC 5952 form; the address itself for IPv4. */
  compressed: string;
  is_network_address: boolean;
  is_broadcast_address: boolean;
}

/** `tools.subnet.split` — params `SplitParams`, result `SplitResult`. */
export interface SplitParams {
  cidr: string;
  /** The prefix length of the pieces. One of this and `count` is required. */
  new_len?: number;
  /** How many pieces instead; rounded up to the next power of two. */
  count?: number;
  /** Pieces returned in full (default 256); the total is reported either way. */
  limit?: number;
}

export interface SplitResult {
  /** A decimal string: splitting IPv6 can produce more than a number holds. */
  total: string;
  subnets: SubnetInfo[];
  truncated: boolean;
}

/** `tools.vlsm` — params `VlsmParams`, result `VlsmPlan`. */
export interface VlsmParams {
  base: string;
  sizes: { name?: string; hosts: number }[];
  /** Give a two-host request a /31 (RFC 3021) instead of a /30. */
  p2p31?: boolean;
}

export interface VlsmAllocation {
  name: string;
  hosts_requested: number;
  cidr: string;
  netmask: string;
  first_host: string;
  last_host: string;
  broadcast: string;
  usable_hosts: number;
  /** Usable addresses beyond what was asked for. */
  spare: number;
}

export interface VlsmPlan {
  allocations: VlsmAllocation[];
  /** What is left of the base network after the allocations. */
  unallocated: string[];
  used_addresses: number;
  total_addresses: number;
}

/** `tools.supernet` / `tools.overlaps` — params `CidrsParams`. */
export interface CidrsParams {
  cidrs: string[];
}

/** Result of `tools.supernet`. */
export interface SummaryResult {
  /** The smallest set of blocks covering exactly the input. */
  summarized: string[];
  /** The single smallest block containing all of it, which may cover more. */
  supernet: string;
}

/** `tools.contains` — params `PairParams`, result `ContainsResult`. */
export interface PairParams {
  /** The network. */
  a: string;
  /** The address or network to test against it. */
  b: string;
}

export interface ContainsResult {
  contains: boolean;
  overlaps: boolean;
}

/** Result of `tools.overlaps`. */
export interface OverlapResult {
  any: boolean;
  pairs: { a: string; b: string }[];
}

/** `tools.ip.range` — params `RangeParams`, result `{cidrs}`. */
export interface RangeParams {
  first: string;
  last: string;
}

export interface RangeResult {
  cidrs: string[];
}

/** `tools.ip.convert` — params `InputParams`, result `ConvertResult`. */
export interface ConvertResult {
  input: string;
  /** Present when the input is a number in any base, or an IPv4 address. */
  number: NumberForms | null;
  /** Present when the input is, or denotes, an address. */
  address: AddressForms | null;
}

export interface NumberForms {
  decimal: string;
  hex: string;
  binary: string;
  octal: string;
  /** The same 32-bit value as an IPv4 address, when it is one. */
  ipv4: string | null;
}

export interface AddressForms {
  version: number;
  address: string;
  integer: string;
  hex: string;
  binary: string;
  expanded: string;
  compressed: string;
  reverse_dns: string;
  scope: string;
}

/** `tools.wildcard` — params `InputParams`, result `MaskInfo`. */
export interface MaskInfo {
  /** `null` for a non-contiguous wildcard, which has no prefix length. */
  prefix: number | null;
  netmask: string;
  wildcard: string;
  contiguous: boolean;
  /** How many addresses the mask covers, as a decimal string. */
  addresses: string;
}

/** `tools.wildcard.acl` — params `AclParams`, result `AclResult`. */
export interface AclParams {
  address: string;
  wildcard: string;
  /** An address to test against the pair. */
  candidate?: string;
}

export interface AclResult {
  /** What the pair matches, in words. */
  explanation: string;
  prefix: number | null;
  netmask: string;
  contiguous: boolean;
  addresses: string;
  /** Only when a `candidate` was given. */
  matches: boolean | null;
}

/** `tools.mac` — params `InputParams`, result `MacInfo`. */
export interface MacInfo {
  /** `aa:bb:cc:dd:ee:ff`. */
  colon: string;
  /** `AA-BB-CC-DD-EE-FF`. */
  hyphen: string;
  /** `aabb.ccdd.eeff`. */
  dotted: string;
  bare: string;
  /** The first three octets, `AA:BB:CC`. */
  oui: string;
  /** `null` when the block is not in the bundled IEEE registry. */
  vendor: string | null;
  is_multicast: boolean;
  is_broadcast: boolean;
  /** Assigned by an administrator or by software, not by a manufacturer. */
  is_locally_administered: boolean;
  /** The modified EUI-64 interface identifier. */
  eui64: string;
  /** The IPv6 link-local address SLAAC derives from the MAC. */
  link_local_ipv6: string;
}

/**
 * `tools.oui` — params `OuiParams`, result `OuiResult`. Both directions: a
 * vendor from a prefix, or the prefixes registered to a vendor.
 *
 * A name search covers the 24-bit MA-L assignments; the smaller MA-M, MA-S
 * and IAB blocks can only be looked up by prefix, which `note` says.
 */
export interface OuiParams {
  /** A MAC address, an OUI prefix (`00:1b:21`), or part of a vendor's name. */
  query: string;
  /** Default 100, at most 1000. */
  limit?: number;
}

export interface OuiResult {
  query: string;
  /** How the query was read. */
  read_as: 'prefix' | 'vendor';
  assignments: { prefix: string; vendor: string }[];
  /** Matches found, before `limit`. */
  total: number;
  registry: { assignments: number; date: string };
  /** What this answer does not cover, when that is worth saying. */
  note: string | null;
}

/** `tools.oid` — params `InputParams`, result `OidResult`. */
export interface OidResult {
  input: string;
  oid: string | null;
  /** `ifInOctets.3` — the object and the instance. */
  name: string | null;
  /** The short name of the notification, when the OID is one. */
  notification: string | null;
  /** `false` when nothing in the built-in dictionary matches. */
  known: boolean;
}

/** `tools.interface` — params `InputParams`, result `InterfaceResult`. */
export interface InterfaceResult {
  long: string;
  short: string;
}

// ------------------------------------------------------------------ secrets

/**
 * `tools.secret.decode` — params `DecodeParams`, result `Decoded`.
 *
 * Only the reversible encodings: Cisco type 7 and Junos `$9$`, which are
 * obfuscation, not encryption. Anything else is refused with an `invalid`
 * error whose message names what it is looking at, and whose `details`
 * carry the `SecretKind` it identified.
 */
export interface DecodeParams {
  /** `cisco7` or `junos9`; left out, the encoding is recognised from the text. */
  kind?: 'cisco7' | 'junos9';
  /** The encoded value, or the whole configuration line it sits in. */
  text: string;
}

export interface Decoded {
  kind: 'cisco7' | 'junos9';
  name: string;
  secret: string;
  /** Why this was possible at all, in words a ticket can quote. */
  note: string;
}

/** `tools.secret.encode` — params `EncodeParams`, result `Encoded`. */
export interface EncodeParams {
  kind: 'cisco7' | 'junos9';
  text: string;
  /** Cisco type 7 only: 0 to 15. Random when left out. */
  salt?: number;
}

export interface Encoded {
  kind: 'cisco7' | 'junos9';
  encoded: string;
  /** The configuration line this value belongs in. */
  example: string;
  note: string;
}

/**
 * `tools.secret.identify` — params `{text}`, result `Identified`.
 *
 * `reversible` is about the format (plain text, obfuscation, or encryption
 * under a key the device holds); `decodable` is whether *this* toolbox can
 * recover the secret. Cisco type 6 is reversible and not decodable: the key
 * is the device's.
 */
export interface Identified {
  /** `cisco_type7`, `md5crypt`, `junos_9`, …, or `unknown`. */
  kind: string;
  name: string;
  reversible: boolean;
  /** `none`, `weak`, `moderate`, `strong` or `unknown`. */
  strength: string;
  advice: string;
  decodable: boolean;
  /** The `kind` to pass to `tools.secret.decode`, when it can. */
  decoder: 'cisco7' | 'junos9' | null;
}

/** `tools.password` — params `PasswordParams`, result `Passwords`. */
export interface PasswordParams {
  /** Default 20. */
  length?: number;
  upper?: boolean;
  lower?: boolean;
  digits?: boolean;
  symbols?: boolean;
  /** Leave out `0 O o 1 l I . ,`. */
  avoid_ambiguous?: boolean;
  /** 1 to 50. */
  count?: number;
}

export interface Passwords {
  passwords: string[];
  /** The symbols the generator may use, so a device's rules can be checked. */
  symbols: string;
}

/**
 * `tools.keys.generate` — params `KeyParams`, result `GeneratedKey`.
 *
 * The pair is returned once and kept nowhere. `fingerprint` is `null` in a
 * build without the terminal feature, which is what computes it — never a
 * made-up value.
 */
export interface KeyParams {
  kind?: 'ed25519' | 'rsa3072' | 'rsa4096';
  comment?: string;
}

export interface GeneratedKey {
  /** OpenSSH format, unencrypted. */
  private_key: string;
  /** The `authorized_keys` line. */
  public_key: string;
  kind: string;
  fingerprint: string | null;
}

// --------------------------------------------------------------- converters

/** `tools.convert` — params `UnitConvertParams`, result `Conversion`. */
export interface UnitConvertParams {
  /** The amount, with or without its unit: `700 MB`, `4.7GiB`, `-12`. */
  value: string;
  /** The unit of `value`, when the value does not carry it. */
  from?: string;
  /** The unit to answer in. Left out, every unit of the family comes back. */
  to?: string;
}

export interface Conversion {
  /** What was parsed, as it was handed to the engine. */
  input: string;
  family: 'size' | 'rate' | 'duration' | 'power';
  /** The amount in the family's own unit (bytes, bit/s, seconds, mW). */
  canonical: number;
  canonical_unit: string;
  /** The engine's own rendering: `667.57 MiB`, `1.25 Gbps`, `2w 3d`. */
  human: string;
  /** The amount in `to`, when a unit was asked for. */
  value: number | null;
  unit: string | null;
  forms: { unit: string; value: number }[];
}

/** `tools.transfer_time` — params `TransferParams`, result `TransferEstimate`. */
export interface TransferParams {
  /** `700 MB`, `4.7GiB`, a number of bytes. */
  size: string;
  /** `100 Mbps`, `1.5G`, a number of bits per second. */
  rate: string;
  /** Protocol overhead as a percentage of the link rate, 0 to under 100. */
  overhead_pct?: number;
}

export interface TransferEstimate {
  seconds: number;
  human: string;
  effective_bps: number;
}

/** `tools.mtu` — params `MtuParams`, result `MtuResult`. */
export interface MtuParams {
  link_mtu: number;
  /** Stacked outermost first: `["ipsec-esp-tunnel", "gre"]`. */
  encapsulations: string[];
}

export interface MtuResult {
  payload_mtu: number;
  tcp_mss_v4: number;
  tcp_mss_v6: number;
  overhead: number;
  /** `[name, bytes]` per encapsulation. */
  breakdown: [string, number][];
}

// ---------------------------------------------------------------- reference

/**
 * `tools.reference` — params `ReferenceParams`, result `ReferenceTable`.
 *
 * `rows` are the engine's own rows as JSON and `columns` names their fields
 * in the order a table should show them, so a screen can render any table
 * without knowing which one it asked for. `tools.reference.tables` (no
 * params, result `TableInfo[]`) lists what there is.
 */
export interface ReferenceParams {
  /**
   * `ports`, `dscp`, `protocols`, `ethertypes`, `icmp_types`,
   * `icmpv6_types`, `admin_distance`, `ethernet`, `syslog_levels`,
   * `encapsulations`. Common aliases work too (`qos`, `optics`, `mtu`).
   */
  table: string;
  /** Free text, a number or a name; `ports` and `dscp` search cleverly. */
  query?: string;
  /** Default 500, at most 5000. */
  limit?: number;
}

export interface ReferenceTable {
  table: string;
  title: string;
  columns: string[];
  rows: ReferenceRow[];
  /** Rows matching the query, before `limit`. */
  total: number;
  truncated: boolean;
}

export type ReferenceRow = Record<string, string | number | boolean | null>;

export interface TableInfo {
  table: string;
  title: string;
  description: string;
  rows: number;
}

/** The row of the `ports` table. */
export interface PortEntry extends ReferenceRow {
  port: number;
  protocol: string;
  name: string;
  description: string;
}

/** The row of the `dscp` table. */
export interface DscpEntry extends ReferenceRow {
  name: string;
  dscp: number;
  binary: string;
  tos: number;
  precedence: number;
  phb: string;
  typical_use: string;
}

/** The row of `protocols`, `ethertypes`, `icmp_types` and `icmpv6_types`. */
export interface NamedNumber extends ReferenceRow {
  number: number;
  name: string;
  description: string;
}

// --------------------------------------------------------------------- text

/**
 * `tools.regex` — params `RegexParams`, result `RegexResult`.
 *
 * A pattern that does not compile comes back in `error`, not as a failed
 * call: this is called on every keystroke while somebody types one.
 */
export interface RegexParams {
  pattern: string;
  text: string;
  /** `i` case-insensitive, `m` multi-line — or the two booleans. */
  flags?: string;
  case_insensitive?: boolean;
  multiline?: boolean;
}

export interface RegexResult {
  matches: RegexMatch[];
  error: string | null;
}

export interface RegexMatch {
  /** Byte offsets into the text. */
  start: number;
  end: number;
  text: string;
  /** Numbered groups; `null` for one that did not participate. */
  groups: (string | null)[];
  /** `[name, value]` per named group. */
  named: [string, string | null][];
}

/** `tools.diff` — params `DiffParams`, result `TextDiff`. */
export interface DiffParams {
  a: string;
  b: string;
  /** Unchanged lines kept around each change; left out, everything is kept. */
  context?: number;
}

export interface TextDiff {
  lines: DiffLine[];
  added: number;
  removed: number;
}

export interface DiffLine {
  /** `" "`, `"+"`, `"-"`, or `"@"` for a gap where context was dropped. */
  tag: string;
  text: string;
  old_line: number | null;
  new_line: number | null;
}

/** `tools.hash` — params `BytesParams`, result `Digests`. */
export interface BytesParams {
  /** The text to work on. One of `text` and `base64`. */
  text?: string;
  /** The bytes to work on, for anything that is not text. */
  base64?: string;
}

export interface Digests {
  md5: string;
  sha1: string;
  sha256: string;
  sha512: string;
  bytes: number;
  /** The same bytes as Base64. */
  base64: string;
}

/** `tools.base64` — params `Base64Params`, result `Base64Result`. */
export interface Base64Params {
  mode: 'encode' | 'decode';
  text: string;
}

export interface Base64Result {
  base64: string;
  bytes: number;
  /** The decoded bytes as text, when they are valid UTF-8. */
  text: string | null;
  /** Said when the bytes are not text, so an empty box is not a mystery. */
  note: string | null;
}

// ------------------------------------------------------------------- probes

/** What every probe aimed at a host takes: one of `host` and `device`. */
export interface TargetParams {
  /** An address or a host name. */
  host?: string;
  /** Or a device in the inventory, by id, name or address. */
  device?: string;
}

/** What a method that starts a job returns. */
export interface StartedProbe {
  /** Follow it through `jobs.changed` / `jobs.log`; stop it with `jobs.cancel`. */
  job_id: string;
  /** The history record it is writing: `probe.history.get`. */
  run_id: string;
}

/** `probe.ping` — params `PingParams`, result `StartedProbe`, job result `PingResult`. */
export interface PingParams extends TargetParams {
  /** `0` pings until the job is cancelled (within `probe.max_run_s`). */
  count?: number;
  interval_ms?: number;
  timeout_ms?: number;
  /** Data bytes per echo (56 is what `ping` sends). */
  payload?: number;
}

/** How the echo requests were actually sent. */
export type PingMethod = 'icmp_datagram' | 'icmp_raw' | 'system_command';

export interface PingEcho {
  /** 1 for the first echo. */
  seq: number;
  rtt_ms: number | null;
  ttl: number | null;
  /** Why there was no reply. */
  error: string | null;
}

export interface PingResult {
  /** The host as it was given. */
  target: string;
  /** The address that was pinged. */
  address: string;
  method: PingMethod;
  sent: number;
  received: number;
  loss_pct: number;
  min_ms: number | null;
  avg_ms: number | null;
  max_ms: number | null;
  stddev_ms: number | null;
  echoes: PingEcho[];
  from: ProbeFrom;
  run_id: string;
  device?: DeviceHint;
  /** Echoes left out of `echoes` because the run was a long one. */
  echoes_dropped?: number;
}

/** `probe.traceroute` — params `TraceParams`, result `StartedProbe`, job result `TraceResult`. */
export interface TraceParams extends TargetParams {
  max_hops?: number;
  probes?: number;
  timeout_ms?: number;
  resolve_names?: boolean;
}

export interface Hop {
  ttl: number;
  /** `null` when no router answered for that hop. */
  address: string | null;
  hostname: string | null;
  /** One entry per probe; `null` for a probe without an answer. */
  rtts_ms: (number | null)[];
  /** `no reply`, an ICMP error, or the other routers that answered. */
  note: string;
}

export interface TraceResult {
  target: string;
  address: string;
  hops: Hop[];
  reached: boolean;
  /** `native-udp`, `system-traceroute` or `system-tracert`. */
  method: string;
  from: ProbeFrom;
  run_id: string;
  device?: DeviceHint;
  /** The hops that are devices Nocturne manages. */
  hop_devices: ({ ttl: number } & DeviceHint)[];
}

/** `probe.ports` — params `PortsParams`, result `StartedProbe`, job result `PortsResult`. */
export interface PortsParams extends TargetParams {
  /** `22,80,443`, `1-1024`, `top100`, `common`, or a mix. */
  ports?: string;
  timeout_ms?: number;
  /** Read the banner a server volunteers (on by default). */
  banners?: boolean;
}

export type PortState = 'open' | 'closed' | 'filtered';

export interface PortResult {
  port: number;
  state: PortState;
  rtt_ms: number | null;
  /** What usually listens on that port number. It says nothing about the host. */
  service: string;
  /** What the server volunteered, when banners were read. */
  banner: string | null;
}

export interface PortsResult {
  /** Every port that was tried: `closed` and "not tried" are different. */
  ports: PortResult[];
  open: number[];
  scanned: number;
  host: string;
  from: ProbeFrom;
  run_id: string;
  device?: DeviceHint;
}

/**
 * `probe.sweep` — params `SweepParams`, result `StartedProbe`, job result
 * `SweepResult`.
 *
 * Held to the same leash as discovery: private address space unless the
 * request *and* the settings allow more, and a host cap. A range larger than
 * the cap is refused rather than truncated.
 */
export interface SweepParams {
  /** `10.0.0.0/24`, `10.0.1.5-10.0.1.40`, addresses, comma separated. */
  targets: string;
  timeout_ms?: number;
  concurrency?: number;
  resolve_names?: boolean;
  /** TCP ports to try on an address that ignores echo requests. */
  ports?: string;
  /** Refused unless the settings allow it too. */
  allow_public?: boolean;
}

export interface SweptHost {
  address: string;
  alive: boolean;
  rtt_ms: number | null;
  /** `icmp`, `tcp/22` or `arp`. */
  via: string;
  hostname: string | null;
  mac: string | null;
  open_ports: number[];
  device?: DeviceHint;
}

export interface SweepResult {
  targets: string;
  hosts: SweptHost[];
  /** How many addresses were probed. */
  planned: number;
  alive: number;
  from: ProbeFrom;
  run_id: string;
}

/** `probe.dns` — params `DnsParams`, result `DnsResult`. */
export interface DnsParams {
  /** The name to look up, or an address for a `PTR` query. */
  name: string;
  /** Default `A`. */
  kind?: string;
  /** `10.0.0.53`, `9.9.9.9:53`; left out, the system resolvers. */
  server?: string;
  timeout_ms?: number;
}

export interface DnsRecord {
  name: string;
  kind: string;
  ttl: number;
  data: string;
}

export interface DnsResult {
  query: string;
  kind: string;
  /** The server that answered. */
  server: string;
  elapsed_ms: number;
  records: DnsRecord[];
  /** `NOERROR`, `NXDOMAIN`, … A negative answer is still an answer. */
  rcode: string;
  from: ProbeFrom;
  run_id: string;
  /** The addresses in the answer that belong to devices Nocturne manages. */
  devices: DeviceHint[];
}

/** `probe.whois` — params `WhoisParams`, result `WhoisResult`. */
export interface WhoisParams {
  /** An address, a CIDR block, an AS number (`AS13335`) or a domain. */
  query: string;
  /** Ask this server instead of starting at IANA. */
  server?: string;
  timeout_ms?: number;
}

export interface WhoisResult {
  query: string;
  /** The servers that answered, in the order they were asked. */
  servers: string[];
  /** The answer of the last (most specific) server. */
  text: string;
  /** `[field, value]`: netname, org, country, cidr, abuse, registrar, … */
  summary: [string, string][];
  from: ProbeFrom;
  run_id: string;
}

/**
 * `probe.tls` — params `TlsParams`, result `TlsResult`.
 *
 * The chain is described **even when it does not verify** — expired,
 * self-signed, wrong name — because that is what the tool is for. `trusted`
 * and `trust_error` carry the verdict separately.
 */
export interface TlsParams extends TargetParams {
  /** Default 443. */
  port?: number;
  /** The server name to send and check instead of `host`. */
  sni?: string;
  timeout_ms?: number;
}

export interface CertInfo {
  subject: string;
  issuer: string;
  common_name: string;
  sans: string[];
  not_before: string;
  not_after: string;
  days_left: number;
  serial: string;
  signature_algorithm: string;
  /** `RSA 2048`, `ECDSA P-256`. */
  public_key: string;
  is_ca: boolean;
  self_signed: boolean;
  sha256_fingerprint: string;
}

export interface TlsResult {
  host: string;
  address: string;
  port: number;
  /** `TLSv1.3`. */
  protocol: string;
  cipher: string;
  alpn: string | null;
  chain: CertInfo[];
  trusted: boolean;
  trust_error: string | null;
  hostname_matches: boolean;
  handshake_ms: number;
  expires_in_days: number;
  from: ProbeFrom;
  run_id: string;
  device?: DeviceHint;
}

/**
 * `probe.http` — params `HttpParams`, result `HttpResult`.
 *
 * An HTTP error status is a successful check and is reported in `status`.
 * Request headers are sent and then forgotten: one of them may be a token,
 * so no part of the request is recorded.
 */
export interface HttpParams {
  url: string;
  /** Default `GET`. */
  method?: string;
  /** `[name, value]` pairs. */
  headers?: [string, string][];
  /** Default 5, at most 10. With 0 a redirect is itself the result. */
  follow_redirects?: number;
  /** Default true. When off, `tls` still says whether it would have been. */
  verify_tls?: boolean;
  timeout_ms?: number;
}

export interface HttpResult {
  url: string;
  final_url: string;
  status: number;
  reason: string;
  version: string;
  headers: [string, string][];
  /** `[status, location]` per redirect followed. */
  redirects: [number, string][];
  dns_ms: number;
  connect_ms: number;
  tls_ms: number | null;
  first_byte_ms: number;
  total_ms: number;
  body_bytes: number;
  body_preview: string;
  tls: TlsResult | null;
  from: ProbeFrom;
  run_id: string;
  device?: DeviceHint;
}

/** `probe.ntp` — params `NtpParams`, result `NtpResult`. */
export interface NtpParams {
  /** `pool.ntp.org`, `10.0.0.1`, `10.0.0.1:123`. */
  server: string;
  timeout_ms?: number;
}

export interface NtpResult {
  server: string;
  address: string;
  stratum: number;
  /** A primary server's clock (`GPS`) or a secondary's upstream address. */
  reference: string;
  /** `none`, `insert_second`, `delete_second`, `not_synchronised`. */
  leap: string;
  version: number;
  /** How far the server's clock is from this machine's. */
  offset_ms: number;
  delay_ms: number;
  root_delay_ms: number;
  root_dispersion_ms: number;
  server_time_unix: number;
  from: ProbeFrom;
  run_id: string;
}

/** `probe.local` — no params, result `LocalReport`. */
export interface LocalReport {
  /** Which machine this is about. */
  from: ProbeFrom;
  interfaces: LocalInterface[];
  gateways: LocalRoute[];
  neighbours: KnownNeighbour[];
}

export interface LocalInterface {
  name: string;
  /** In CIDR form. */
  addresses: string[];
  mac: string | null;
  is_loopback: boolean;
  is_up: boolean;
}

export interface LocalRoute {
  destination: string;
  gateway: string;
  interface: string;
}

export interface KnownNeighbour {
  address: string;
  mac: string;
  interface: string;
  state: string;
  /** The vendor of the MAC address, from the bundled IEEE registry. */
  vendor?: string;
  /** The device of the inventory at that address. */
  device?: DeviceHint;
}

/** `probe.wol` — params `WolParams`, result `WolResult`. */
export interface WolParams {
  /** In any notation. */
  mac: string;
  /** Left out, every broadcast address of this machine. */
  broadcast?: string;
}

export interface WolResult {
  /** The address as it was understood, normalised. */
  mac: string;
  /** The broadcast addresses the magic packet was sent to. */
  sent_to: string[];
  from: ProbeFrom;
  run_id: string;
}

// ------------------------------------------------------------ device probes

/**
 * `probe.device.ping` and `probe.device.traceroute` — params
 * `DeviceProbeParams`, result `DeviceProbeResult`.
 *
 * The platform's own command is built, run through the core's read-only gate
 * (so the device's profile still decides whether it may be sent) and parsed
 * when the output is in a format this crate reads. `summary: null` means the
 * output was not recognised — the device's own words are in `output` and
 * nothing is invented. A platform whose ping command is not known is
 * `unsupported`, not guessed at.
 */
export interface DeviceProbeParams {
  /** The device to probe from: its id, name or address. */
  device_id: string;
  /** An address or a host name. Anything else is refused. */
  target: string;
  count?: number;
  /** The source interface or address the device should use. */
  source?: string;
  /** The VRF / routing instance. */
  vrf?: string;
}

export interface DeviceProbeSummary {
  sent: number;
  received: number;
  loss_pct: number;
  min_ms: number | null;
  avg_ms: number | null;
  max_ms: number | null;
}

export interface DeviceProbeResult {
  /** The command that was sent, exactly as it was sent. */
  command: string;
  /** What the device printed, unchanged. */
  output: string;
  summary?: DeviceProbeSummary;
  /** The platform family the command was built for. */
  family: string;
  /** `SW1 itself`. */
  from: string;
  device_id: string;
  device_name: string;
  run_id: string;
  /** The device's own error line, when it rejected the command. */
  error?: string;
}

// ------------------------------------------------------------------ history

/**
 * `probe.history` — params `HistoryParams`, result `History`. The records
 * carry no `result`; `probe.history.get` ({id}) returns one whole.
 *
 * `ok: null` means it is still running. `ok: false` with `error_code:
 * 'forbidden'` means this machine may not do that; `timeout` means nothing
 * answered. A failed run has `result: null` — there is no plausible-looking
 * substitute for an answer.
 */
export interface HistoryParams {
  /** One kind, or several separated by commas. */
  kind?: string;
  /** Only runs aimed at this device. */
  device_id?: string;
  /** Free text over the title, target and summary. */
  query?: string;
  /** Default 50, at most 500. */
  limit?: number;
  /** Rows already received, as a decimal string. */
  cursor?: string;
}

export type ProbeKind =
  | 'ping'
  | 'traceroute'
  | 'ports'
  | 'sweep'
  | 'dns'
  | 'whois'
  | 'tls'
  | 'http'
  | 'ntp'
  | 'wol'
  | 'device.ping'
  | 'device.traceroute'
  | 'tftp'
  | 'syslog';

export interface ProbeRun {
  id: string;
  kind: ProbeKind | string;
  /** One line for a list: `Ping 10.0.0.2 (SW1)`. */
  title: string;
  /** The target as the caller gave it. */
  target: string;
  /** The address it turned out to be, once that is known. */
  address: string | null;
  device: DeviceHint | null;
  from: ProbeFrom;
  /** How it was sent: `icmp_datagram`, `native-udp`, the device command, … */
  method: string | null;
  job_id: string | null;
  started_at: number;
  finished_at: number | null;
  /** `null` while it is still running. */
  ok: boolean | null;
  error: string | null;
  /** The machine-stable code of the failure. */
  error_code: string | null;
  /** One line a person can read without opening the result. */
  summary: string;
  /** The engine's own report; absent from a list, `null` on a failure. */
  result: unknown | null;
}

export interface History {
  runs: ProbeRun[];
  /** Runs matching the filters, before paging. */
  total: number;
  /** The cursor for the next page, or `null` at the end. */
  cursor: string | null;
}

/** `probe.history.delete` — params `DeleteParams`, result `{removed}`. */
export interface DeleteParams {
  ids?: string[];
  /** Delete every finished run instead of named ones. */
  all?: boolean;
}

// ------------------------------------------------------------------ servers

/**
 * `tftp.start` — params `TftpStartParams`, result `TftpStarted`.
 *
 * Local access only: refused (`forbidden`) in server mode. A port that
 * cannot be bound is an error saying why — in use, or needing privileges —
 * never a server that looks as though it started. The server is a job:
 * `tftp.stop` cancels it, and it ends when the application shuts down.
 */
export interface TftpStartParams {
  /** The directory to serve. Left out, the one in the settings. */
  root?: string;
  /** Left out, the port in the settings (6969). `0` takes any free port. */
  port?: number;
  bind?: string;
  /** Accept uploads from devices. Off by default. */
  allow_write?: boolean;
  /** Let an upload replace a file that is already there. Off by default. */
  allow_overwrite?: boolean;
}

export interface TftpStarted {
  /** Where it is listening, with the port it actually got. */
  address: string;
  root: string;
  allow_write: boolean;
  allow_overwrite: boolean;
  job_id: string;
  run_id: string;
}

/** `tftp.status` — no params, result `TftpStatus`. */
export interface TftpStatus {
  running: boolean;
  address?: string;
  root?: string;
  allow_write?: boolean;
  allow_overwrite?: boolean;
  job_id?: string;
  run_id?: string;
  started_at?: number;
  /** What has happened on it, newest last. */
  transfers?: TftpEvent[];
  /** Transfers dropped from the front of `transfers`. */
  dropped?: number;
  /** What a `tftp.start` without parameters would use. */
  defaults: { bind: string; port: number; root: string };
}

/** `tftp.stop` — no params, result `{stopped}`: false when none was running. */
export interface Stopped {
  stopped: boolean;
}

/** One thing that happened on the TFTP server, as `tftp.transfer` carries it. */
export type TftpEvent =
  | { started: { id: number; peer: string; file: string; write: boolean; size: number | null } }
  | { progress: { id: number; bytes: number } }
  | { finished: { id: number; bytes: number; elapsed_ms: number } }
  | { failed: { id: number; error: string } }
  | { denied: { peer: string; file: string; reason: string } };

/**
 * `probe.syslog.start` — params `SyslogStartParams`, result `SyslogStarted`.
 *
 * A **staging** listener: it keeps messages in memory while its job runs and
 * stores nothing. Nocturne's log collection is the logs feature, and the
 * default port (5515) deliberately avoids the one its receiver uses.
 */
export interface SyslogStartParams {
  bind?: string;
  /** Left out, the port in the settings (5515). `0` takes any free port. */
  port?: number;
  /** Also listen on TCP (RFC 6587), on the same port. */
  tcp?: boolean;
}

export interface SyslogStarted {
  udp_address: string;
  tcp_address: string | null;
  job_id: string;
  run_id: string;
}

/** `probe.syslog.status` — no params, result `SyslogStatus`. */
export interface SyslogStatus {
  listening: boolean;
  udp_address?: string;
  tcp_address?: string | null;
  job_id?: string;
  run_id?: string;
  started_at?: number;
  received?: number;
  senders?: { address: string; messages: number; device?: string }[];
  defaults: { bind: string; port: number; tcp: boolean; keep_messages: number };
  /** Said once, so nobody mistakes this for log collection. */
  note: string;
}

/** `probe.syslog.messages` — params `MessagesParams`, result `Messages`. */
export interface MessagesParams {
  /** Default 200, at most 10000. */
  limit?: number;
  /** Only the messages of this sender address. */
  source?: string;
}

export interface Messages {
  messages: SyslogMessage[];
  /** Messages in memory, before `limit`. */
  total: number;
  listening: boolean;
}

export interface SyslogMessage {
  /** The sender's address. */
  source: string;
  facility: number | null;
  severity: number | null;
  /** As the device wrote it, not reinterpreted. */
  timestamp: string | null;
  hostname: string | null;
  app: string | null;
  procid: string | null;
  msgid: string | null;
  /** The human part. */
  message: string;
  /** Cisco `%LINK-3-UPDOWN`, a Junos tag, a RouterOS topic. */
  mnemonic: { facility: string; severity: number | null; name: string } | null;
  sequence: number | null;
  raw: string;
}

// ----------------------------------------------------------------- settings

/**
 * The `toolbox` settings section (`settings.get` / `settings.set`). Every
 * number is clamped when it is read, so a value out of range here cannot
 * make a probe run for a week or open ten thousand sockets.
 */
export interface ToolboxSettings {
  probe: {
    ping_count: number;
    ping_interval_ms: number;
    ping_timeout_ms: number;
    ping_payload: number;
    /** How long a ping with `count: 0` may run. At most 3600. */
    max_run_s: number;
    trace_max_hops: number;
    trace_probes: number;
    trace_timeout_ms: number;
    /** Ports a port check tries when the request names none. */
    ports: string;
    port_timeout_ms: number;
    port_concurrency: number;
    max_ports: number;
    dns_timeout_ms: number;
    whois_timeout_ms: number;
    tls_timeout_ms: number;
    http_timeout_ms: number;
    ntp_timeout_ms: number;
    resolve_names: boolean;
  };
  sweep: {
    max_hosts: number;
    concurrency: number;
    timeout_ms: number;
    /** Whether a sweep may be asked for public address space at all. */
    allow_public: boolean;
    tcp_fallback_ports: string;
  };
  tftp: { bind: string; port: number; root: string; max_file_mb: number };
  syslog: { bind: string; port: number; tcp: boolean; keep_messages: number };
  /** How many finished probe runs are kept. */
  keep_runs: number;
}

// ------------------------------------------------------------------- events

/** `probe.ping.echo` — one echo, as it came back. */
export interface PingEchoEvent {
  job_id: string;
  run_id: string;
  echo: PingEcho;
}

/** `probe.trace.hop` — one hop, as it was found. */
export interface TraceHopEvent {
  job_id: string;
  run_id: string;
  hop: Hop;
  /** The device that hop is, when Nocturne knows. */
  device: DeviceHint | null;
}

/** `probe.port` — an open port. Closed ones are only in the result. */
export interface PortEvent {
  job_id: string;
  run_id: string;
  port: PortResult;
}

/** `probe.sweep.host` — an address that answered (the first 200 of them). */
export interface SweepHostEvent {
  job_id: string;
  run_id: string;
  host: SweptHost;
  device: DeviceHint | null;
}

/** `probe.ran` — a run started, finished, or runs were deleted. */
export type ProbeRanEvent = ProbeRun | { removed: number };

/** `tftp.transfer` — the event's own fields, with the job that is serving. */
export type TftpTransferEvent = { job_id: string } & TftpEvent;

/** `probe.syslog.message` — one message, parsed, with the job that heard it. */
export type SyslogMessageEvent = { job_id: string } & SyslogMessage;

/**
 * The events this feature publishes. Every job-shaped probe also drives the
 * core's `jobs.changed` and `jobs.log`, which is where progress and the log
 * of what it did come from.
 */
export type ToolboxEvent =
  | { topic: 'probe.ping.echo'; payload: PingEchoEvent }
  | { topic: 'probe.trace.hop'; payload: TraceHopEvent }
  | { topic: 'probe.port'; payload: PortEvent }
  | { topic: 'probe.sweep.host'; payload: SweepHostEvent }
  | { topic: 'probe.ran'; payload: ProbeRanEvent }
  | { topic: 'tftp.transfer'; payload: TftpTransferEvent }
  | { topic: 'probe.syslog.message'; payload: SyslogMessageEvent };
