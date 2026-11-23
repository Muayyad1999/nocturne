/**
 * Issue identification (`nocturne-insights`): the things an experienced
 * engineer would spot walking the network — a duplex mismatch, a port that
 * flaps, a trunk whose two ends disagree, a switch that was never saved, two
 * devices with one address — each with the evidence it was decided from.
 *
 * These types mirror the backend's parameter and result structs exactly:
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and `?` marks a field the backend may leave out (or a
 * parameter it defaults). Backend source: `crates/nocturne-insights`.
 * Specification: `docs/dev/features/insights.md`.
 *
 * Two things about this surface are worth building the screens around:
 *
 * 1. **"Nothing found" and "not checked" are different answers.**
 *    `insights.coverage` says, per check, how many devices it reached a
 *    verdict for and how many it could not, with the reasons. A check with no
 *    findings and no coverage has told you nothing.
 * 2. **A finding keeps its `id` across scans.** That is what makes its
 *    history, its dismissal and its alert stick to it. It resolves itself
 *    after two consecutive scans that evaluated its device and did not find
 *    it, and `occurrences` counts how many separate times it has come back.
 */

// --------------------------------------------------------------------- model

export type Severity = 'info' | 'low' | 'medium' | 'high' | 'critical';

export type Category =
  | 'physical'
  | 'l2'
  | 'l3'
  | 'config'
  | 'security'
  | 'capacity'
  | 'hygiene';

export type InsightState = 'open' | 'dismissed' | 'resolved';

/** One fact a finding was decided from: a counter, a configuration line, a time. */
export interface Evidence {
  label: string;
  value: string;
}

/** What to do about a finding. */
export interface Suggestion {
  text: string;
  /**
   * Configuration-mode lines for the insight's device, exactly as they would
   * be typed. Present only where the fix is exact and safe; `insights.fix`
   * hands these to the change pipeline. Absent means the suggestion is advice,
   * not something to apply — show `text` and `commands` instead.
   */
  config_lines?: string[];
  /** Read-only commands that confirm the finding on the device. */
  commands?: string[];
}

/** Another place the same problem shows: the far end of a link, the other holder of an address. */
export interface Related {
  device_id: string;
  /** What on that device it is about: `Gi1/0/1`, `VLAN 20`, `10.0.0.5`. */
  subject: string;
  /** That device's name, so a list can show it without a lookup. */
  device: string;
}

/** A problem found on the network, with what it was decided from. */
export interface Insight {
  /** Stable across scans: derived from the check, the device and the subject. */
  id: string;
  /** The check that found it, e.g. `duplex_mismatch`; see {@link CheckInfo}. */
  check: string;
  category: Category;
  severity: Severity;
  device_id: string;
  device: string;
  /** What on the device it is about: `Gi0/3`, `VLAN 20`, `10.0.0.5`. */
  subject: string;
  /** One sentence a person can act on. */
  title: string;
  /** Why it matters, in two or three sentences. */
  detail: string;
  /** The actual lines, counters and timestamps it was decided from. */
  evidence: Evidence[];
  suggestion?: Suggestion;
  related: Related[];
  state: InsightState;
  first_seen: number;
  last_seen: number;
  resolved_at?: number;
  /** When a dismissal ends; absent while `dismissed` means for good. */
  dismissed_until?: number;
  dismissed_at?: number;
  dismiss_note?: string;
  /** The device is simulated (part of a lab). */
  lab: boolean;
  /** How many separate times this problem has appeared (1 = the first time). */
  occurrences: number;
}

// ----------------------------------------------------------------- catalogue

/** One entry of the catalogue: what a check does, and what it cannot see. */
export interface CheckInfo {
  id: string;
  title: string;
  category: Category;
  default_severity: Severity;
  /** The collected kinds and other sources it reads. */
  needs: string[];
  enabled: boolean;
  /** What it looks for, and what it deliberately stays quiet about. */
  description: string;
  /** When it reports "not evaluated" instead of "nothing found". */
  not_evaluated: string;
}

// ------------------------------------------------------------------ coverage

/** Why a check could not decide, and for how many devices. */
export interface CoverageReason {
  reason: string;
  devices: number;
}

/** What one check could and could not decide in the last scan. */
export interface CheckCoverage {
  check: string;
  title: string;
  category: Category;
  /** False when the check is switched off; `reasons` then says so. */
  enabled: boolean;
  /** Devices the check reached a verdict for. */
  evaluated: number;
  /** Devices it could not decide for. `evaluated + skipped` is every device scanned. */
  skipped: number;
  findings: number;
  /** Worst first. */
  reasons: CoverageReason[];
}

/** The last scan: what ran, over how much, and what it could not see. */
export interface Coverage {
  scanned_at: number;
  devices: number;
  /** How the links the link-based checks used were worked out: `topology` or `neighbors`. */
  link_source: string;
  links: number;
  checks: CheckCoverage[];
  /** Features that were not installed, and the checks that wanted them. */
  missing_sources: string[];
  took_ms: number;
}

// ------------------------------------------------------------------ requests

export interface InsightsScanParams {
  /** Empty or absent = every device. Ids, names or addresses. */
  device_ids?: string[];
  /** Convenience for a single device. */
  id?: string;
  /** Empty or absent = every enabled check. Running a named check runs it even if it is switched off. */
  checks?: string[];
}

export interface InsightsListParams {
  /** `open` (the default), `dismissed`, `resolved`, or `any`. */
  state?: InsightState | 'any';
  /** Only findings at least this severe. */
  severity?: Severity;
  category?: Category;
  check?: string;
  /** Matches the finding's own device *and* the device at the other end of it. */
  device_id?: string;
  /** Free text over the device name, subject, title and check id. */
  query?: string;
  /** Default 50, at most 500. */
  limit?: number;
  /** From a previous result's `next_cursor`. */
  cursor?: string;
  /** Overrides the `include_lab` setting for one list. */
  include_lab?: boolean;
}

export interface InsightsIdParams {
  id: string;
}

export interface InsightsDismissParams {
  id: string;
  /** Absent = for good. */
  days?: number;
  note?: string;
}

// ------------------------------------------------------------------- results

export interface InsightsCounts {
  /** Severity name → how many findings match the filter (every severity is present). */
  by_severity: Record<string, number>;
  /** Category name → how many findings match the filter (every category is present). */
  by_category: Record<string, number>;
}

export interface InsightsListResult {
  insights: Insight[];
  /** Of everything that matches the filter, not of this page. */
  counts: InsightsCounts;
  /** Absent on the last page. */
  next_cursor?: string;
}

/** The result of `insights.scan`'s job, not of the call. */
export interface ScanSummary {
  devices: number;
  opened: number;
  resolved: number;
  open: number;
  findings: number;
  checks_run: number;
  took_ms: number;
}

export interface InsightsFixResult {
  /** The draft change the suggestion became; nothing has been applied. */
  change_id: string;
}

// ------------------------------------------------------------------- methods

/** `insights.scan` {@link InsightsScanParams} → `{ job_id: string }` (follow `jobs.changed`; the job's result is a {@link ScanSummary}) */
/** `insights.list` {@link InsightsListParams} → {@link InsightsListResult} */
/** `insights.get` {@link InsightsIdParams} → {@link Insight} */
/** `insights.checks` `{}` → {@link CheckInfo}[] — the catalogue, with what each check needs and when it declines to answer */
/** `insights.coverage` `{}` → {@link Coverage} or `null` before the first scan */
/** `insights.dismiss` {@link InsightsDismissParams} → {@link Insight} */
/** `insights.reopen` {@link InsightsIdParams} → {@link Insight} */
/** `insights.fix` {@link InsightsIdParams} → {@link InsightsFixResult} — `changes.create` with the suggestion's lines; `invalid` when the insight has no configuration fix, `unavailable` without the change pipeline */

// -------------------------------------------------------------------- events

/** `insights.changed` — after every scan, and after a dismissal or a reopen. */
export interface InsightsChangedEvent {
  /** Present when the scan was about one device. */
  device_id?: string | null;
  opened: number;
  resolved: number;
}

// ------------------------------------------------------------------ settings

/** The `insights` settings section (`settings.get` / `settings.set`). */
export interface InsightsSettings {
  /** Check id → whether it runs. Every check in the catalogue is a key. */
  enabled_checks: Record<string, boolean>;
  thresholds: InsightsThresholds;
  /** Include simulated (lab) devices in scans and lists. Default true. */
  include_lab: boolean;
  /** Open findings at or above this severity call `alerts.raise`. Default `high`. */
  raise_alerts_from: Severity;
  /** Scan automatically, debounced, after `collect.updated` / `config.changed`. Default true. */
  auto_scan: boolean;
  /** How long to wait for more events before an automatic scan runs. Default 5000. */
  auto_scan_delay_ms: number;
}

/**
 * A setting of the section that cannot be used as it is: refused on write, or
 * read back wrong (stored before writes were checked, or by hand).
 */
export interface SettingsProblem {
  /** Dotted, from the section: `thresholds.backup_age_days`. */
  key: string;
  /** What was sent, or what is stored. */
  value: unknown;
  /** "`thresholds.backup_age_days` must be a whole number of days (0 or more), not 7.5" */
  message: string;
}

/**
 * The `details` of `settings.set {section: 'insights'}` when it refuses a
 * write (`invalid`): one problem per setting, sorted by key. Nothing of a
 * refused write is stored.
 */
export interface InsightsSettingsRefusal {
  section: 'insights';
  problems: SettingsProblem[];
}

/**
 * Every number a check compares against. The engine counts some in whole
 * units (`u32`; `interface_error_min_delta` is a `u64`) and refuses a
 * fraction for them; the ratios, rates and temperatures are fractions.
 */
export interface InsightsThresholds {
  /** Up/down transitions within an hour that count as flapping. Default 4. */
  flap_transitions_per_hour: number;
  /** Input errors per million packets, between the last two collections. Default 100. */
  interface_error_ppm: number;
  /** …and at least this many new errors. Default 10. */
  interface_error_min_delta: number;
  /** Interface utilisation that counts as congested. Default 80. */
  utilisation_pct: number;
  /** Default 85. */
  cpu_pct: number;
  /** Default 90. */
  memory_pct: number;
  /** Default 65. */
  temperature_c: number;
  /** How long a metric must stay over its threshold to count as sustained. Default 15. */
  sustained_minutes: number;
  /** How far back metric and log history is read. Default 24. */
  history_hours: number;
  /** A backup older than this is "no recent backup". Default 7. */
  backup_age_days: number;
  /** Collected data older than this is not used for cross-device checks. Default 168. */
  stale_data_hours: number;
  /** Packets a second a QoS class must drop, sustained, before it is reported. Default 10. */
  qos_drop_ps: number;
  /** Addresses that must move between the same two ports before it counts as a flap. Default 3. */
  mac_flap_min_macs: number;
  /** Devices of one model needed before a version majority means anything. Default 3. */
  software_min_group: number;
  /** How close together running and startup must have been captured. Default 60. */
  saved_pair_window_minutes: number;
  /** How close together two ARP tables must have been read to be compared. Default 60. */
  arp_window_minutes: number;
  /** How far from a configuration change an applied change may be and still account for it. Default 120. */
  change_window_minutes: number;
}
