/**
 * Configuration management (`nocturne-config`): versioned backups, diff and
 * search across the network, compliance policies, and drift against the
 * configuration a device is meant to have.
 *
 * These types mirror the backend's parameter and result structs exactly —
 * field names are the backend's (`snake_case`), optional fields are `?`, and
 * every timestamp is milliseconds since the epoch. Backend source:
 * `crates/nocturne-config`. Specification: `docs/dev/features/config.md`.
 *
 * Configuration text is **redacted by default** everywhere it appears: each
 * secret becomes a stable placeholder such as `<secret:3>` (`<secret:3:weak>`
 * for a well-known default). Asking for `redact: false` is recorded in the
 * audit trail. Redacted text has exactly as many lines as the original, so
 * line numbers mean the same line either way.
 */

/** A device filter, as the inventory defines it (`devices.list`). */
export interface DeviceFilter {
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

/** Which of a device's configurations a version is. */
export type ConfigKind = 'running' | 'startup';

/** The grammar a configuration is written in (`nocturne_drivers::hier::Syntax`). */
export type ConfigSyntax = 'ios' | 'vrp' | 'set' | 'routeros' | 'flat';

/** One stored version of a device's configuration, without its text. */
export interface ConfigVersion {
  id: number;
  device_id: string;
  kind: ConfigKind;
  sha256: string;
  lines: number;
  bytes: number;
  /** When this content was first fetched. */
  first_seen: number;
  /** When a fetch last returned exactly this content. */
  last_seen: number;
  /** What stored it: `manual`, `scheduled`, `assistant`, `event`, `pre-change`, `post-change`, … */
  origin: string;
  /** Pinned versions are never pruned or deleted. */
  pinned: boolean;
  pin_reasons?: string[];
  note?: string;
}

/** One line of a unified diff (`nocturne_drivers::hier::DiffLine`). */
export interface DiffLine {
  /** `" "`, `"+"` or `"-"`. */
  tag: string;
  text: string;
  old_line?: number | null;
  new_line?: number | null;
}

export interface TextDiff {
  lines: DiffLine[];
  added: number;
  removed: number;
}

/** How much of a diff falls into one configuration section. */
export interface SectionChange {
  /** Outermost line first (`["router bgp 65001", "address-family ipv4"]`); empty at the top level. */
  path: string[];
  added: number;
  removed: number;
}

// ---------------------------------------------------------------- backups

/** `config.backup` params. */
export interface ConfigBackupParams {
  device_ids?: string[];
  filter?: DeviceFilter;
  /** Defaults to running, plus startup when the `backup_startup` setting is on. */
  kinds?: ConfigKind[];
  /** Who asked: `user` (default), `schedule`, `assistant` or `system`. */
  origin?: 'user' | 'schedule' | 'assistant' | 'system';
}

/** A device a job could not do anything with, and why. */
export interface DeviceNote {
  device_id: string;
  name: string;
  reason?: string;
  error?: string;
}

/** The `result` of a finished `config.backup` job. */
export interface BackupResult {
  /** Devices whose configuration was read and stored: `unchanged + changed + first`. */
  backed_up: number;
  unchanged: number;
  changed: number;
  /** Devices backed up for the first time. */
  first: number;
  skipped: DeviceNote[];
  failed: DeviceNote[];
}

/** What a job-starting method returns; follow it with `jobs.changed` / `jobs.log`. */
export interface JobStarted {
  job_id: string;
}

/** `config.backup` {@link ConfigBackupParams} → {@link JobStarted} (result: {@link BackupResult}). */

// ------------------------------------------------------ coverage, unsaved

/** The latest stored version of one kind, as the coverage table shows it. */
export interface CoverageSnapshot {
  version_id: number;
  /** When a fetch last returned this content. */
  fetched_at: number;
  lines: number;
  sha256: string;
}

/** One row of the backup coverage table. */
export interface Coverage {
  device_id: string;
  name: string;
  /** The device type in use; empty when it is not known yet. */
  profile: string;
  lab: boolean;
  running?: CoverageSnapshot;
  startup?: CoverageSnapshot;
  /** Stored versions of both kinds. */
  versions: number;
  /** When the running configuration was last seen to differ from the version before it. */
  last_change_at?: number;
  /** Why the most recent backup attempt failed. */
  last_error?: string;
  last_attempt_at?: number;
  /** Why this device is not backed up at all (monitor-only, no configuration command). */
  skipped?: string;
}

/** Params of the methods that take a set of devices. */
export interface ConfigScopeParams {
  device_ids?: string[];
  filter?: DeviceFilter;
}

/** `config.devices` {@link ConfigScopeParams} → {@link Coverage}[] */

/** A device whose running configuration differs from its startup configuration. */
export interface Unsaved {
  device_id: string;
  name: string;
  lab: boolean;
  running_version_id: number;
  startup_version_id: number;
  /** Lines in the running configuration that the startup configuration lacks. */
  added: number;
  /** Lines in the startup configuration that the running configuration lacks. */
  removed: number;
  /** When the two were read, in one session. */
  fetched_at: number;
}

/** A device for which the question cannot be answered, and why. */
export interface NotComparable {
  device_id: string;
  name: string;
  lab: boolean;
  reason: string;
}

export interface UnsavedReport {
  devices: Unsaved[];
  /** Devices whose running and startup configurations agree. */
  saved: number;
  not_comparable: NotComparable[];
}

/** `config.unsaved` {@link ConfigScopeParams} → {@link UnsavedReport} */

// ----------------------------------------------------------- reading text

export interface ConfigVersionsParams {
  device_id: string;
  /** Defaults to `running`. */
  kind?: ConfigKind;
  /** 1–500, default 50. */
  limit?: number;
  /** Page backwards: only versions older than this one. */
  before_id?: number;
}

/** `config.versions` {@link ConfigVersionsParams} → {@link ConfigVersion}[] (newest first) */

/** A version together with its text. */
export interface ConfigText {
  version: ConfigVersion;
  text: string;
  redacted: boolean;
}

export interface ConfigGetParams {
  version_id: number;
  /** Default `true`; `false` returns the stored text and is audited. */
  redact?: boolean;
}

/** `config.get` {@link ConfigGetParams} → {@link ConfigText} */

export interface ConfigLatestParams {
  /** Id, name or address. */
  device_id: string;
  kind?: ConfigKind;
  redact?: boolean;
}

/** `config.latest` {@link ConfigLatestParams} → {@link ConfigText} (`not_found` when there is no backup yet) */

export interface ConfigDiffParams {
  /** The older (left) version. */
  a?: number;
  /** The newer (right) version. */
  b?: number;
  /** Instead of `a`/`b`: this device's latest configuration against the one before it. */
  device_id?: string;
  kind?: ConfigKind;
  /** Unchanged lines kept around each change; default 3. */
  context?: number;
  redact?: boolean;
}

export interface DiffResult {
  a: ConfigVersion;
  b: ConfigVersion;
  diff: TextDiff;
  /** The change grouped by the configuration section it sits in. */
  sections: SectionChange[];
  redacted: boolean;
}

/** `config.diff` {@link ConfigDiffParams} → {@link DiffResult} */

/** One side of a comparison: a device (its latest) or one particular version. */
export interface CompareSide {
  device_id?: string;
  version_id?: number;
  kind?: ConfigKind;
}

export interface ConfigCompareParams {
  a: CompareSide;
  b: CompareSide;
  kind?: ConfigKind;
  context?: number;
  redact?: boolean;
}

/** `config.compare` {@link ConfigCompareParams} → {@link DiffResult} */

// ---------------------------------------------------------------- search

export interface ConfigSearchParams {
  query: string;
  /** Treat `query` as a regular expression; default false. */
  regex?: boolean;
  /** Match case; default false. */
  case?: boolean;
  filter?: DeviceFilter;
  device_ids?: string[];
  /** Defaults to `running`. */
  kind?: ConfigKind;
  /** Lines of context around each hit; default 1, at most 20. */
  context?: number;
  /** Matches to return; default 500, at most 5000. */
  limit?: number;
}

export interface SearchMatch {
  device_id: string;
  name: string;
  lab: boolean;
  version_id: number;
  /** Line number (from 1) in the version's text. */
  line_no: number;
  /** The sections the line sits in, outermost first; empty at the top level. */
  section: string[];
  line: string;
  before: string[];
  after: string[];
}

export interface SearchResult {
  matches: SearchMatch[];
  /** Devices whose latest configuration was looked at; fewer than there are when `truncated`. */
  devices_searched: number;
  devices_matched: number;
  /** Devices in scope with no stored configuration of this kind. */
  devices_without_backup: number;
  /** There are more matches than `limit`; the search stopped there. */
  truncated: boolean;
  /** Said when the query looks like a secret value, which can never match. */
  note?: string;
}

/** `config.search` {@link ConfigSearchParams} → {@link SearchResult} (always redacted) */

export interface ConfigMissingParams {
  query: string;
  regex?: boolean;
  case?: boolean;
  filter?: DeviceFilter;
  device_ids?: string[];
  kind?: ConfigKind;
}

/** A device that does not have what was asked for. */
export interface DeviceRef {
  device_id: string;
  name: string;
  lab: boolean;
  /** The version that was looked at. */
  version_id?: number;
  /** When that version was last fetched. */
  fetched_at?: number;
}

export interface MissingResult {
  /** Devices whose latest configuration does not contain the line or pattern. */
  devices: DeviceRef[];
  devices_searched: number;
  /** Devices with no stored configuration: whether they have the line is unknown. */
  without_backup: DeviceRef[];
  note?: string;
}

/** `config.missing` {@link ConfigMissingParams} → {@link MissingResult} */

// --------------------------------------------- export, delete, pin, unpin

export interface ConfigExportParams {
  /** The full path of the folder to write into; it is created if it does not exist. */
  dir: string;
  filter?: DeviceFilter;
  device_ids?: string[];
  /** Default `false`: an export is unredacted unless asked otherwise. Either way it is audited. */
  redact?: boolean;
}

export interface ExportResult {
  dir: string;
  /** Files written. */
  files: number;
  /** Devices that have at least one file. */
  devices: number;
  skipped: DeviceNote[];
  /** Files of the previous export removed because nothing produces them any more. */
  removed: number;
  redacted: boolean;
}

/** `config.export` {@link ConfigExportParams} → {@link ExportResult} (local access only) */

export interface ConfigDeleteParams {
  /** Every stored version of this device… */
  device_id?: string;
  /** …or exactly these versions. One of the two. */
  version_ids?: number[];
}

export interface DeleteResult {
  deleted: number;
  /** Versions that were asked for but stay because they are pinned. */
  kept_pinned: number[];
}

/** `config.delete` {@link ConfigDeleteParams} → {@link DeleteResult} */

export interface ConfigPinParams {
  version_id: number;
  /** Why it is pinned; shown next to it, and what a later unpin refers to. */
  reason: string;
}

/** `config.pin` {@link ConfigPinParams} → {@link ConfigVersion} */

export interface ConfigUnpinParams {
  version_id: number;
  /** One pin's reason, or every pin of the version when left out. */
  reason?: string;
}

/** `config.unpin` {@link ConfigUnpinParams} → {@link ConfigVersion} */

// ------------------------------------------------- intended state, drift

export interface IntendedView {
  device_id: string;
  text: string;
  /** `true`: the device must equal the text. `false`: the text must be present on it. */
  exact: boolean;
  updated_at: number;
  lines: number;
  redacted: boolean;
}

export interface ConfigIntendedGetParams {
  device_id: string;
  redact?: boolean;
}

/** `config.intended.get` {@link ConfigIntendedGetParams} → {@link IntendedView} | null */

export interface ConfigIntendedSetParams {
  device_id: string;
  text: string;
  /** Keeps the meaning the stored text already had when left out; a new one is a fragment. */
  exact?: boolean;
}

/** `config.intended.set` {@link ConfigIntendedSetParams} → {@link IntendedView} */

export interface DeviceIdParams {
  device_id: string;
}

/** `config.intended.clear` {@link DeviceIdParams} → `{ cleared: boolean }` */

export interface ConfigDriftParams {
  device_id: string;
  context?: number;
  redact?: boolean;
}

export interface Drift {
  device_id: string;
  name: string;
  /** False when the device type's grammar is not modelled: the difference is shown, the commands cannot be derived. */
  supported: boolean;
  /** Nothing would have to be sent and nothing is left unexplained. */
  in_sync: boolean;
  exact: boolean;
  /** Configuration-mode lines that would make the device match, in order. */
  commands: string[];
  warnings: string[];
  /** `exact`: running → intended. Otherwise: the intended lines the device has → all of them. */
  diff: TextDiff;
  /** The running-config version the device was judged by. */
  version_id: number;
  fetched_at: number;
  redacted: boolean;
}

/** `config.drift` {@link ConfigDriftParams} → {@link Drift} */

/** What a hand-over to the change pipeline returns. */
export interface Handover {
  change_id: string;
}

/** `config.drift.fix` {@link DeviceIdParams} → {@link Handover} (creates a change; applies nothing) */

export interface ConfigRestoreParams {
  version_id: number;
}

/** `config.restore` {@link ConfigRestoreParams} → {@link Handover} (creates a change; applies nothing) */

// ------------------------------------------------------------ compliance

export type Severity = 'info' | 'low' | 'medium' | 'high' | 'critical';

export type FindingStatus = 'pass' | 'fail' | 'not_applicable' | 'not_evaluated';

/**
 * One question asked of a configuration. Lines are compared with runs of
 * blanks collapsed. A `must` / `must_not` entry is matched both as a literal
 * line and as a regular expression — whichever hits first.
 */
export type Check =
  /** A line at the top level of the configuration is exactly this. */
  | { type: 'present'; line: string }
  /** No top-level line is exactly this. */
  | { type: 'absent'; line: string }
  /** Some line matches. Lines are offered indented one space per level, so `^ntp ` is a global line. */
  | { type: 'matches'; pattern: string }
  /** No line matches. */
  | { type: 'not_matches'; pattern: string }
  /** Sections whose header matches `parent`: with `each` every one must satisfy it, otherwise at least one. */
  | { type: 'section'; parent: string; each?: boolean; must?: string[]; must_not?: string[] }
  /** Only the matching sections that contain a line matching `when`; every one of them must satisfy it. */
  | { type: 'section_when'; parent: string; when: string; must?: string[]; must_not?: string[] }
  /** A golden block: every line of it, in its block structure, must be on the device. */
  | { type: 'snippet'; config: string };

export interface Rule {
  id: string;
  title: string;
  severity: Severity;
  /** Why this matters, and what the rule can and cannot tell from the text. */
  rationale: string;
  check: Check;
  /**
   * Configuration lines that would fix it. `{{ device.name }}`,
   * `{{ device.address }}`, `{{ device.site }}`, `{{ device.role }}` and —
   * for a section rule — `{{ section }}` are substituted per device. Lines
   * that mention `{{ section }}` are rendered once per offending section.
   */
  remediation?: string[];
}

/** Which devices a policy is about. An empty list means "any". */
export interface Scope {
  /** Device type ids; a device also matches a profile its own extends. */
  profiles: string[];
  /** Operating system families as device types report them (`ios`, `routeros`), ignoring case. */
  os: string[];
  roles: string[];
  sites: string[];
  tags: string[];
}

export interface Policy {
  id: string;
  name: string;
  description: string;
  enabled: boolean;
  /** Shipped with Nocturne: read-only, but it can be switched off or duplicated. */
  builtin: boolean;
  scope: Scope;
  rules: Rule[];
  /** When a user policy was last saved; 0 for a built-in. */
  updated_at: number;
}

/** `compliance.policies.list` `{}` → {@link Policy}[] (built-ins first, then by name) */

export interface PolicyIdParams {
  policy_id: string;
}

/** `compliance.policies.get` {@link PolicyIdParams} → {@link Policy} */

export interface PolicyParams {
  policy: Policy;
}

/** `compliance.policies.save` {@link PolicyParams} → {@link Policy} (a built-in accepts only a change of `enabled`) */

/** `compliance.policies.delete` {@link PolicyIdParams} → `{ deleted: boolean }` (a built-in cannot be deleted) */

export interface PolicyDuplicateParams {
  policy_id: string;
  /** Defaults to "<name> (copy)". */
  name?: string;
}

/** `compliance.policies.duplicate` {@link PolicyDuplicateParams} → {@link Policy} */

export interface Problem {
  /** The rule it is about; empty for a problem with the policy itself. */
  rule_id: string;
  /** Which part of the rule, e.g. `check.pattern`. */
  field: string;
  level: 'error' | 'warning';
  message: string;
}

export interface ValidationResult {
  /** False when something must be fixed before the policy can be saved. */
  ok: boolean;
  problems: Problem[];
}

/** `compliance.policies.validate` {@link PolicyParams} → {@link ValidationResult} */

/** A configuration line quoted as evidence; always redacted. */
export interface EvidenceLine {
  line_no: number;
  line: string;
}

/** What one rule concluded about one device. */
export interface Finding {
  policy_id: string;
  policy_name: string;
  rule_id: string;
  title: string;
  severity: Severity;
  device_id: string;
  device_name: string;
  status: FindingStatus;
  evidence: EvidenceLine[];
  /** What the device should have and has not. */
  missing: string[];
  /** One sentence for the person reading the result. */
  detail: string;
  /** The configuration the verdict was reached from. */
  version_id?: number;
  fetched_at?: number;
  /** The rule carries remediation lines, so `compliance.remediate` can act on it. */
  remediable: boolean;
  /** Section headers a section rule found wanting. */
  sections?: string[];
}

export interface SeverityCount {
  pass: number;
  fail: number;
}

export interface RunSummary {
  run_id: string;
  at: number;
  /** Devices at least one policy covered. */
  devices: number;
  /** Devices the request asked about, whether a policy covered them or not. */
  devices_requested: number;
  /** Devices in scope with no stored running configuration. */
  devices_without_backup: number;
  policies: number;
  /** Rules evaluated, counted once per policy. */
  rules: number;
  pass: number;
  fail: number;
  not_applicable: number;
  not_evaluated: number;
  /** Share of the judged findings that passed; `null` when nothing was judged. */
  score_pct: number | null;
  /** One entry per severity, whether it occurred or not. */
  by_severity: Record<Severity, SeverityCount>;
  /** Older runs keep only this summary; their findings are gone. */
  has_findings: boolean;
}

export interface ComplianceRunParams {
  /** Defaults to every enabled policy; naming one runs it even when it is off. */
  policy_ids?: string[];
  device_ids?: string[];
  filter?: DeviceFilter;
}

export interface ComplianceRunResult {
  run_id: string;
  summary: RunSummary;
}

/** `compliance.run` {@link ComplianceRunParams} → {@link ComplianceRunResult} (no device is touched) */

export interface ComplianceResultsParams {
  /** Defaults to the most recent run. */
  run_id?: string;
  status?: FindingStatus;
  severity?: Severity;
  device_id?: string;
  policy_id?: string;
  rule_id?: string;
  /** 1–2000, default 200. */
  limit?: number;
  /** The `next_cursor` of the page before. */
  cursor?: string;
}

export interface ComplianceResults {
  findings: Finding[];
  summary: RunSummary;
  next_cursor?: string;
}

/** `compliance.results` {@link ComplianceResultsParams} → {@link ComplianceResults} */

export interface ComplianceRunsParams {
  /** 1–500, default 50. */
  limit?: number;
  /** Page backwards: only runs older than this one. */
  before_id?: string;
}

/** `compliance.runs` {@link ComplianceRunsParams} → {@link RunSummary}[] (newest first) */

export interface FindingRef {
  device_id: string;
  rule_id: string;
  /** Needed only when two policies share a rule id. */
  policy_id?: string;
}

export interface ComplianceRemediateParams {
  /** Defaults to the most recent run. */
  run_id?: string;
  findings: FindingRef[];
}

export interface RemediateResult {
  change_id: string;
  devices: number;
  /** Configuration lines the change will send in all. */
  commands: number;
}

/** `compliance.remediate` {@link ComplianceRemediateParams} → {@link RemediateResult} (creates a change; applies nothing) */

export interface ComplianceTestParams {
  rule: Rule;
  /** A pasted configuration… */
  text?: string;
  /** …or a device, whose latest backup is used. One of the two. */
  device_id?: string;
  /** The grammar `text` is written in; the device's own when `device_id` is given, `ios` otherwise. */
  syntax?: ConfigSyntax;
}

export interface ComplianceTestResult {
  finding: Finding;
  /** The grammar the text was read with. */
  syntax: ConfigSyntax;
  /** What would be sent to fix it, when the rule has a remediation and a device was named. */
  fix: string[];
}

/** `compliance.test` {@link ComplianceTestParams} → {@link ComplianceTestResult} */

// ---------------------------------------------------------------- events

/** `config.first_backup` — a device's configuration was stored for the first time. */
export interface ConfigFirstBackupEvent {
  device_id: string;
  name: string;
  kind: ConfigKind;
  version_id: number;
  lines: number;
}

/** `config.changed` — a fetch found a configuration different from the stored one. */
export interface ConfigChangedEvent {
  device_id: string;
  name: string;
  kind: ConfigKind;
  version_id: number;
  previous_id: number;
  added: number;
  removed: number;
}

/** `config.pinned` — a version was pinned or unpinned. */
export interface ConfigPinnedEvent {
  version_id: number;
  device_id: string;
  pinned: boolean;
}

/** `config.deleted` — stored versions were removed. */
export interface ConfigDeletedEvent {
  device_ids: string[];
  version_ids: number[];
}

/** `config.intended_changed` — a device's intended configuration was set or cleared. */
export interface ConfigIntendedChangedEvent {
  device_id: string;
  set: boolean;
}

/** `compliance.finished` — a run ended and its findings are readable. */
export interface ComplianceFinishedEvent {
  run_id: string;
  summary: RunSummary;
}

/** `compliance.policies_changed` — a policy was saved, switched on or off, or deleted. */
export interface CompliancePoliciesChangedEvent {
  policy_id: string;
  change: 'saved' | 'deleted';
}

// -------------------------------------------------------------- settings

/** The `config` settings section (`settings.get` / `settings.set`). */
export interface ConfigSettings {
  /** Versions kept per device and kind; older ones are pruned unless pinned. Default 200. */
  keep_versions: number;
  /** Fetch the startup configuration too, so unsaved changes can be told. Default true. */
  backup_startup: boolean;
  /** Back a device up when the log feature reports that its configuration changed. Default true. */
  backup_on_change_event: boolean;
  /** Devices backed up at the same time. Default 6, clamped to 1–32. */
  concurrency: number;
}
