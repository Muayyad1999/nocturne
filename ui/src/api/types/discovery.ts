/**
 * Discovery (`nocturne-discovery`): sweeping address ranges, crawling the
 * neighbour tables of devices already managed, identifying what answers, and
 * proposing devices to add.
 *
 * These types mirror the backend's parameter and result structs exactly —
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and a field the backend declares as optional is written
 * `T | null` because it is present in the JSON with a `null` value. Backend
 * source: `crates/nocturne-discovery`. Specification:
 * `docs/dev/features/discovery.md`.
 *
 * Three things run through the whole surface, and the screens should show
 * them rather than smooth them over:
 *
 * * **A find is what answered, not what is there.** A host that did not
 *   answer is not in the list at all, and `alive: false` happens only for a
 *   crawl candidate another device told us about that then ignored every
 *   probe. "Nothing answered" never means "nothing is there".
 * * **Every claim carries its evidence.** `Found.evidence` says, field by
 *   field, which probe produced the value (`icmp`, `tcp/22`, `arp`, `dns`,
 *   `snmp`, `oui`, `profile`, `neighbor`, `inventory`, `user`). A field with
 *   no evidence line was not measured. `profile: null` means no device type's
 *   detect rules matched — it is never guessed from open ports, from the MAC
 *   vendor or from a name.
 * * **Nothing is added until someone asks.** `discovery.preview` says exactly
 *   what `discovery.add` would create, with a reason for every row it would
 *   skip. Neither logs in anywhere: the first login is the user's own act.
 */

// --------------------------------------------------------------- vocabulary

/** What proved a host alive, or how it is known at all. */
export type FoundVia =
  /** It answered an echo request. */
  | 'icmp'
  /** It accepted a TCP connection on that port (`tcp/22`). */
  | `tcp/${number}`
  /** It answered neighbour discovery on a local segment, nothing else. */
  | 'arp'
  /** Only an SNMP agent answered. */
  | 'snmp'
  /** A device Nocturne manages has it in a table; it answered nothing itself. */
  | 'neighbor';

/** Which of the profile registry's detect rules matched. */
export type DetectMethod = 'snmp' | 'banner' | 'probe';

/** What a scan is: a sweep of addresses, or a crawl out from known devices. */
export type ScanKind = 'sweep' | 'crawl';

/** `running` until it ends; then one of the other three. */
export type ScanState = 'running' | 'finished' | 'cancelled' | 'failed';

/** The transport `discovery.add` would give a device. */
export type SuggestedTransport = 'ssh' | 'telnet' | 'none';

// -------------------------------------------------------------------- finds

/** An open TCP port and what, if anything, it said first. */
export interface OpenPort {
  port: number;
  /**
   * The service usually found on that port number (`ssh`, `https`,
   * `winbox`), from the probe engine's table. It says what the *port* means,
   * never what the host is.
   */
  service: string;
  /** What the server volunteered, when banners were read. */
  banner: string | null;
}

/** What an SNMP agent said about itself. */
export interface SnmpFind {
  /** The credential that answered — never the community or passphrase. */
  credential_id: string;
  credential_name: string;
  sys_name: string | null;
  sys_descr: string | null;
  sys_object_id: string | null;
  uptime_s: number | null;
}

/** The device type the profile registry matched, and what matched it. */
export interface ProfileFind {
  id: string;
  name: string;
  method: DetectMethod;
  /** The text that matched, as the registry clipped it. */
  evidence: string;
}

/** Why one field of a find says what it says. */
export interface Evidence {
  /** A field of `Found`, or one of `suggested.name`, `suggested.transport`, `suggested.role`. */
  field: string;
  /** The probe or record it came from: `icmp`, `snmp`, `dns`, `oui`, … */
  source: string;
  /** One sentence a person can check. */
  detail: string;
}

/** The device `discovery.add` would create. A proposal, not a fact. */
export interface Suggested {
  name: string;
  transport: SuggestedTransport;
  /** Empty unless a device Nocturne manages reported what this is. */
  role: string;
  /** The matched profile id; empty when nothing matched. */
  profile: string;
  site: string;
  /** The SNMP credential that answered, so monitoring can use it. */
  snmp_credential_id: string | null;
}

/** Which managed device told us about a crawled find, and out of which table. */
export interface ToldBy {
  device_id: string;
  device_name: string;
  /** `cdp`, `lldp`, `arp`, `route`, … */
  protocol: string;
  local_interface: string;
  remote_interface: string;
  /** The name that device gave it in the table. */
  reported_name: string;
  /** The capabilities it reported (`Router Switch IGMP`) — the only source of a role. */
  capabilities: string;
}

/** One address that answered something, or that a managed device knows of. */
export interface Found {
  address: string;
  alive: boolean;
  rtt_ms: number | null;
  via: FoundVia;
  /** Reverse DNS. Not the same claim as `snmp.sys_name`. */
  hostname: string | null;
  /** Only hosts on a local segment, or ones a managed device's table knew. */
  mac: string | null;
  /** The MAC's registered organisation, from the OUI registry. */
  vendor: string | null;
  open_ports: OpenPort[];
  snmp: SnmpFind | null;
  ssh_banner: string | null;
  /** `null` = no device type matched. Show "unknown device", not a guess. */
  profile: ProfileFind | null;
  suggested: Suggested;
  /** Set when this address is already in the inventory. */
  device_id: string | null;
  device_name: string | null;
  ignored: boolean;
  /** Managed devices from the seed to the one that told us (crawl only). */
  path: string[];
  told_by: ToldBy | null;
  found_at: number;
  evidence: Evidence[];
}

// -------------------------------------------------------------------- scans

/** A scan as the list shows it. */
export interface Scan {
  id: string;
  kind: ScanKind;
  /** The target expression as typed, or the seed devices of a crawl. */
  targets: string;
  note: string;
  job_id: string;
  started_at: number;
  finished_at: number | null;
  state: ScanState;
  error: string | null;
  /** Addresses the scan set out to probe. */
  planned: number;
  probed: number;
  found: number;
  /** Finds the profile registry could put a device type to. */
  identified: number;
  /** Finds that are neither in the inventory nor ignored. */
  new: number;
  /** Packets actually sent: echo requests, TCP connections, SNMP GETs. */
  probes_sent: number;
  /** Whether it covered public address space (and said so in the job log). */
  allow_public: boolean;
}

/**
 * `discovery.scan` — start a sweep. Params `ScanRequest`, result `StartedScan`.
 *
 * Every field is optional; what is left out comes from the `discovery`
 * settings, which also bound what may be asked for (a request can only ask
 * for *less*). A target expression that is too large, that includes
 * multicast, broadcast or the unspecified address, or that reaches outside
 * private space without `allow_public`, is refused before anything is sent.
 */
export interface ScanRequest {
  /** `"10.0.0.0/24, 10.0.1.5-10.0.1.40, core-sw.example"`. */
  targets: string;
  /** Send an echo request to every address. Default true. */
  icmp?: boolean;
  /**
   * TCP ports to try on every address. Left out = the ports in the settings
   * (`22,23,80,443,161,8291` by default); `""` = no port probing at all.
   */
  ports?: string;
  /**
   * Credentials holding a community or a v3 user, tried in this order.
   * Nothing else is ever offered to anything.
   */
  snmp_credential_ids?: string[];
  /** Read the SSH identification string port 22 volunteers. Never a login. */
  identify_ssh?: boolean;
  /** Reverse-resolve the addresses that answered. */
  resolve_names?: boolean;
  /** Lower than the settings' `max_hosts`; higher is ignored. */
  max_hosts?: number;
  /** Probes per second, bounded by the settings' `rate`. */
  rate?: number;
  /** Addresses in flight at once, bounded by the settings' `concurrency`. */
  concurrency?: number;
  /** Per-probe timeout, bounded by the settings. */
  timeout_ms?: number;
  /** The UDP port SNMP is asked on. Default 161. */
  snmp_port?: number;
  /**
   * Sweep addresses outside private, link-local and loopback space. Refused
   * with `forbidden` unless `allow_public` is on in the settings too.
   */
  allow_public?: boolean;
  /** A label for the scan, for whoever reads the list later. */
  note?: string;
}

/**
 * `discovery.crawl` — discover from what is already known. Params
 * `CrawlRequest`, result `StartedScan`.
 *
 * Reads the neighbour, ARP and (on request) routing tables of devices in the
 * inventory, through the core's read-only command gate and with each
 * device's own credential. A candidate that is not in the inventory is
 * probed exactly like a scan host and never logged into. Without `add` the
 * crawl can only read devices the user already manages, so that is as far as
 * it goes.
 */
export interface CrawlRequest {
  /** Devices to start from. Empty = every device in the inventory. */
  seed_device_ids?: string[];
  /** Levels out from the seeds. Default 2, bounded by `max_crawl_depth`. */
  depth?: number;
  /** SNMP credentials to try on candidates. Nothing else is tried. */
  credential_ids?: string[];
  /**
   * Add what the profile registry positively identifies, with the seed's
   * credential and site, and carry on through it. Default false.
   */
  add?: boolean;
  /** Walk ARP caches too (hosts the seed has spoken to). Default true. */
  use_arp?: boolean;
  /** Walk routing tables too (next hops). Default false. */
  use_routes?: boolean;
  /** TCP ports to try on candidates; empty = the settings' ports. */
  ports?: string;
  note?: string;
}

/** What `discovery.scan` and `discovery.crawl` return. */
export interface StartedScan {
  /** Follow it through `jobs.changed` / `jobs.log`; cancel with `jobs.cancel`. */
  job_id: string;
  scan_id: string;
}

/** `discovery.scans` — the scans kept, newest first. Result `Scan[]`. */
export interface ScansParams {
  /** Default 50, at most 500. */
  limit?: number;
}

/**
 * `discovery.scan_get` — a scan and its finds, while it runs as well as
 * after. Params `ScanGetParams`, result `ScanGetResult`.
 *
 * `device_id` and `ignored` are re-evaluated on every read, so a scan from
 * last week reflects the inventory and the ignore list as they are now.
 */
export interface ScanGetParams {
  scan_id: string;
  /** Leave out finds that are already in the inventory or ignored. */
  only_new?: boolean;
  /** Free text over address, names, vendor, profile, banners and ports. */
  query?: string;
  /** Finds per page. Default 1000, at most 5000. */
  limit?: number;
  /**
   * Paging is by offset: the number of finds already received, as a decimal
   * string (`"1000"`). A page shorter than `limit` is the last one.
   */
  cursor?: string;
}

export interface ScanGetResult {
  scan: Scan;
  hosts: Found[];
  /** Finds matching the filters, before paging. */
  total: number;
}

/** `discovery.scan_delete` — forget scans and their finds. */
export interface ScanDeleteParams {
  scan_ids: string[];
}

export interface ScanDeleteResult {
  deleted: number;
}

// ------------------------------------------------------------------- adding

/** What the user wants every added device to have. */
export interface AddDefaults {
  site?: string;
  role?: string;
  tags?: string[];
  /** A CLI credential to attach. Discovery does not test it — that is a login. */
  credential_id?: string;
  /** Override the transport the finds suggest. */
  transport?: SuggestedTransport;
  monitor?: boolean;
}

/**
 * `discovery.preview` — what `discovery.add` would create (result
 * `AddPreview`), and `discovery.add` — do it (result `AddResult`). Same
 * params for both, so the preview cannot disagree with the import.
 */
export interface AddParams {
  scan_id: string;
  /** Addresses from that scan. Empty = every find that is new. */
  addresses?: string[];
  defaults?: AddDefaults;
}

/** One device that would be created. */
export interface PreviewRow {
  address: string;
  /** Unique in the inventory: a clashing name gets the address appended. */
  name: string;
  profile: string;
  transport: SuggestedTransport;
  site: string;
  role: string;
  tags: string[];
  /** Whether SNMP monitoring would be switched on (an agent answered). */
  monitor_snmp: boolean;
  snmp_credential_id: string | null;
  credential_id: string | null;
  /** `new`, or `skip` — then `reason` says why. */
  status: 'new' | 'skip';
  reason: string | null;
  /** The find's evidence, plus anything the import itself decided. */
  evidence: Evidence[];
}

export interface AddPreview {
  scan_id: string;
  rows: PreviewRow[];
  new: number;
  skipped: number;
}

export interface SkippedAdd {
  address: string;
  reason: string;
}

export interface AddResult {
  /** Inventory ids, in the order they were created. */
  created: string[];
  skipped: SkippedAdd[];
}

// ------------------------------------------------------------------- ignore

/** An address the user never wants offered again. */
export interface Ignored {
  address: string;
  note: string;
  at: number;
}

/** `discovery.ignore` → `{ignored}` / `discovery.unignore` → `{unignored}`. */
export interface IgnoreParams {
  /** IP addresses, not names. */
  addresses: string[];
  note?: string;
}

export interface IgnoreResult {
  ignored: string[];
}

export interface UnignoreResult {
  unignored: string[];
}

/** `discovery.ignored` — the whole list. Result `Ignored[]`. */

// ------------------------------------------------------------------ suggest

/** One network this machine is attached to, offered as a starting point. */
export interface SuggestedNetwork {
  cidr: string;
  interface: string;
  /** Host addresses in the block: what a sweep of it would cover. */
  hosts: number;
  /** This machine's own address on that network. */
  address: string;
  /** Larger than the settings allow one scan to cover. */
  too_large: boolean;
  /** Outside private space: a sweep of it needs `allow_public`. */
  needs_allow_public: boolean;
}

export interface SuggestedGateway {
  address: string;
  interface: string;
  /** `0.0.0.0/0` or `::/0`. */
  destination: string;
}

/**
 * `discovery.suggest` — where to start. No params, result `Suggestions`.
 *
 * These are the networks of the machine the backend runs on. In the desktop
 * app that is the operator's workstation; in server mode it is the server,
 * which `local: false` says plainly rather than pretending otherwise.
 */
export interface Suggestions {
  networks: SuggestedNetwork[];
  gateways: SuggestedGateway[];
  local: boolean;
  /** The most addresses one scan may cover under the current settings. */
  max_hosts: number;
}

// ----------------------------------------------------------------- settings

/**
 * The `discovery` settings section (`settings.get` / `settings.set`). These
 * are the bounds on what a sweep may touch: a request can ask for less, never
 * more, and every value is clamped when it is read.
 */
export interface DiscoverySettings {
  /** The most addresses one scan may cover. Default 4096, at most 65536. */
  max_hosts: number;
  /** Probes per second across a whole scan. Default 500, at most 20000. */
  rate: number;
  /** Addresses probed at once. Default 64, at most 512. */
  concurrency: number;
  /** Per-probe timeout. Default 800, clamped to 50…30000. */
  timeout_ms: number;
  /** Ports tried when a scan does not name its own. */
  ports: string;
  /** Whether a scan may ask for public address space at all. Default false. */
  allow_public: boolean;
  /** Blocks never probed, whatever a request says (`10.9.0.0/16`). */
  never_scan: string[];
  /** Finished scans kept. Default 20. */
  keep_scans: number;
  /** The deepest a crawl may go. Default 3. */
  max_crawl_depth: number;
}

// ------------------------------------------------------------------- events

/**
 * `discovery.found` — one host answered. Throttled: a scan streams at most
 * 200 of these and then says so in its job log, after which the finds are in
 * `discovery.scan_get` and `discovery.progress` carries the counters.
 */
export interface DiscoveryFoundEvent {
  scan_id: string;
  host: Found;
}

/** `discovery.progress` — counters for a scan that has stopped streaming finds. */
export interface DiscoveryProgressEvent {
  scan_id: string;
  probed: number;
  found: number;
  /** False once the per-scan cap on `discovery.found` has been reached. */
  streaming: boolean;
}

/** `discovery.finished` — the scan ended; the summary is also the job's result. */
export interface DiscoveryFinishedEvent {
  scan_id: string;
  summary: {
    scan_id: string;
    state: ScanState;
    probed: number;
    planned?: number;
    found: number;
    identified: number;
    new: number;
    probes_sent: number;
    /** Crawls only: devices whose tables were read. */
    seeds_read?: number;
  };
}

/** `discovery.added` — devices were created from a scan's finds. */
export interface DiscoveryAddedEvent {
  scan_id: string;
  device_ids: string[];
}

/** `discovery.ignored_changed` — the ignore list moved. */
export interface DiscoveryIgnoredChangedEvent {
  ignored?: string[];
  unignored?: string[];
}

/** `discovery.scans_changed` — scans were deleted. */
export interface DiscoveryScansChangedEvent {
  deleted: number;
}

/**
 * The events this feature publishes. A scan also drives the core's
 * `jobs.changed` and `jobs.log`, which is where progress and the log of what
 * it did come from.
 */
export type DiscoveryEvent =
  | { topic: 'discovery.found'; payload: DiscoveryFoundEvent }
  | { topic: 'discovery.progress'; payload: DiscoveryProgressEvent }
  | { topic: 'discovery.finished'; payload: DiscoveryFinishedEvent }
  | { topic: 'discovery.added'; payload: DiscoveryAddedEvent }
  | { topic: 'discovery.ignored_changed'; payload: DiscoveryIgnoredChangedEvent }
  | { topic: 'discovery.scans_changed'; payload: DiscoveryScansChangedEvent };
