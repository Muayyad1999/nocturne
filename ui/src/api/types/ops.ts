/**
 * The operations record (`nocturne-ops`): the human side of a NOC. The
 * overview a shift starts with, incidents with a timeline that gathers
 * itself, the handover note at the end of a shift, reports, runbook notes,
 * evidence (what a device printed, kept with its source), a search over all
 * of it, and the audit trail joined to what it mentions.
 *
 * These types mirror the backend's parameter and result structs exactly:
 * field names are the backend's (`snake_case`), timestamps are milliseconds
 * since the epoch, and `?` marks a field the backend may leave out (or a
 * parameter it defaults). Backend source: `crates/nocturne-ops`.
 * Specification: `docs/dev/features/ops.md`.
 *
 * Three things about this surface decide how the screens should be built:
 *
 * 1. **`null` and `0` are different answers.** Every part of the overview and
 *    every section of a handover that comes from another feature is
 *    independently nullable. `null` means the source is not in this build,
 *    did not answer, or has nothing recorded yet — and the reason is in the
 *    sibling `unavailable` / `partial` list ({@link Unavailable}). Render
 *    `null` as "not available" with that reason, never as a zero or a dash.
 *    A part that is present and `0` is a measured zero and should look like
 *    one.
 * 2. **A timeline entry is evidence.** Every {@link TimelineEntry} carries a
 *    {@link Provenance} saying which record and which of its fields it came
 *    from, and `ts` is when the thing happened, not when it was read. An
 *    entry of kind `gap` is not something that happened: it is a source that
 *    could not be read, placed at the moment of the attempt, and it means the
 *    entries around it may be incomplete. Show gaps in the timeline, not
 *    hidden behind a filter.
 * 3. **Counts that were capped say so.** A `truncated: true` beside a count
 *    means the backend read as far as it was willing to and the number is a
 *    floor.
 *
 * Nothing in this feature changes a device. There is no method here that
 * configures, reboots or connects to anything.
 */

// --------------------------------------------------------------------- shared

/**
 * A device as this feature points at one: enough to show it without a second
 * request, and able to say that it is gone.
 */
export interface DeviceRef {
  id: string;
  /** The device's name, or its id when the device no longer exists. */
  name: string;
  site?: string;
  /** A simulated device in a lab. */
  lab: boolean;
  /** The device has been deleted from the inventory since. */
  deleted?: boolean;
}

/** Why a section has no data. */
export type UnavailableReason =
  /** The feature that would answer is not part of this build. */
  | 'not_installed'
  /** It answered with an error. */
  | 'failed'
  /** It did not answer inside the budget. */
  | 'timeout'
  /** It answered with something this version of the backend cannot read. */
  | 'malformed'
  /** It works but has nothing recorded yet: no run, no backup, no samples. */
  | 'no_data';

/** A section that could not be filled, and why. Shown instead of a number. */
export interface Unavailable {
  /** The field of the result that is `null` (`alerts`, `changes`, …). */
  section: string;
  reason: UnavailableReason;
  /** A sentence for the person at the keyboard. */
  message: string;
}

/** A pointer to a record somewhere else. */
export interface Ref {
  /**
   * `alert`, `change`, `incident`, `note`, `report`, `handover`, `job`,
   * `config_version`, `audit`, `lab`, `policy`.
   */
  type: string;
  id: string;
}

/** A span of time, `since` inclusive and `until` exclusive. */
export interface Period {
  since: number;
  until: number;
}

/** An alert as this feature carries it. Its device name is the inventory's. */
export interface AlertRef {
  id: string;
  severity: 'info' | 'low' | 'medium' | 'high' | 'critical';
  title: string;
  /** `firing`, `acknowledged` or `resolved`. */
  state: string;
  started_at: number;
  acked_at?: number;
  resolved_at?: number;
  device_id?: string;
  device?: string;
  /** The name of the rule that raised it. */
  rule?: string;
  lab: boolean;
}

export interface ChangeRef {
  id: string;
  title: string;
  /** `draft`, `planned`, `approved`, `applying`, `applied`, `failed`, `rolled_back`, `partially_applied`, `cancelled`. */
  state: string;
  origin?: string;
  ticket?: string;
  created_by?: string;
  created_at: number;
  updated_at: number;
  /** Absent when the change list does not say which devices a change targets. */
  device_ids?: string[];
}

export interface InsightRef {
  id: string;
  title: string;
  severity: 'info' | 'low' | 'medium' | 'high' | 'critical';
  device_id?: string;
  device?: string;
  /** What on the device it is about (`Gi0/3`, `VLAN 20`). */
  subject?: string;
  check?: string;
  first_seen?: number;
  lab: boolean;
}

export interface JobRef {
  id: string;
  kind: string;
  title: string;
  state: 'running' | 'succeeded' | 'failed' | 'cancelled';
  /** `user`, `schedule`, `assistant` or `system`. */
  origin: string;
  done: number;
  /** `0` while the amount of work is unknown. */
  total: number;
  message: string;
  started_at: number;
  finished_at?: number;
  error?: string;
  device_ids: string[];
}

/** The output formats a document can be rendered in. */
export type Format = 'markdown' | 'csv' | 'html' | 'json';

/** A rendered document, as the export and report methods return it. */
export interface Rendered {
  title: string;
  format: Format;
  /** A file name to offer, with the right extension. */
  filename: string;
  /** The document itself. HTML is self-contained: inline CSS, no scripts. */
  content: string;
  /** Data rows in it — not lines of text. */
  rows: number;
  generated_at: number;
}

// ------------------------------------------------------------------- overview

/** `overview.get` parameters. */
export interface OverviewParams {
  /** Read everything again instead of reusing the copy cached for a few seconds. */
  refresh?: boolean;
}

export interface DeviceCounts {
  total: number;
  up: number;
  down: number;
  /** Never polled, or monitoring switched off. Not "up". */
  unknown: number;
  lab: number;
  by_site: { site: string; total: number; down: number; unknown: number }[];
}

export interface OverviewAlerts {
  open: number;
  /** Every severity, with a real `0` where there are none. */
  by_severity: Record<string, number>;
  newest: AlertRef[];
  /** The count came from a capped list and is a floor. */
  truncated: boolean;
  /** `alerts.summary` (an exact count) or `alerts.list` (counted here). */
  from: string;
}

export interface OverviewChanges {
  awaiting_approval: number;
  applying: number;
  applied_24h: number;
  failed_24h: number;
  recent: ChangeRef[];
  truncated: boolean;
}

export interface OverviewBackups {
  devices_with_backup: number;
  devices_without: number;
  changed_24h: number;
  /** `null` when nothing has ever been backed up. */
  oldest_backup_at: number | null;
  /** Always `store.configs`: counted from the stored snapshots themselves. */
  from: string;
}

export interface OverviewCompliance {
  /** `null` when the compliance feature did not give a score. */
  score_pct: number | null;
  failing_devices: number;
  failing_findings: number;
  /** `null` when the compliance feature did not say when it ran. */
  run_at: number | null;
  truncated: boolean;
}

export interface OverviewInsights {
  open: number;
  top: InsightRef[];
  truncated: boolean;
}

export interface OverviewIncidents {
  open: number;
  mitigated: number;
  unresolved: number;
  by_severity: Record<IncidentSeverity, number>;
  newest: IncidentHead[];
}

/** One line of the activity feed: merged audit trail, alerts and changes. */
export interface ActivityLine {
  ts: number;
  /** `audit`, `alert` or `change`. */
  kind: string;
  text: string;
  device_id?: string;
  ref?: Ref;
}

/**
 * `overview.get` result. Cheap enough to poll every 30 s; the backend caches
 * it for a few seconds, so several panes asking at once cost one read.
 */
export interface Overview {
  devices: DeviceCounts;
  alerts: OverviewAlerts | null;
  changes: OverviewChanges | null;
  backups: OverviewBackups | null;
  compliance: OverviewCompliance | null;
  insights: OverviewInsights | null;
  jobs: { running: JobRef[]; failed_24h: number } | null;
  /** This feature's own records: never `null`. */
  incidents: OverviewIncidents;
  /** Newest first, at most 20. */
  activity: ActivityLine[];
  /** One entry per `null` part above, with the reason. */
  unavailable: Unavailable[];
  generated_at: number;
}

// ------------------------------------------------------------------ incidents

export type IncidentSeverity = 'sev1' | 'sev2' | 'sev3' | 'sev4';

export type IncidentState =
  /** Happening, nobody has stopped the bleeding yet. */
  | 'open'
  /** The users are served again; the cause is still there. */
  | 'mitigated'
  /** Done, with a cause written down. */
  | 'resolved';

export type TimelineKind =
  | 'note'
  | 'alert'
  | 'change'
  | 'command'
  | 'status'
  | 'log'
  /**
   * Not something that happened: a source that could not be read. The
   * timeline around it may be missing entries. Not in the specification's
   * list of kinds; added because a timeline with a hole in it has to be able
   * to say where the hole is.
   */
  | 'gap'
  /**
   * An attached evidence item, at the time it was captured. `id` is
   * `evidence:<evidence id>`, `text` is the item's title and `ref` is
   * `{ type: 'evidence', id }`: a reference, never the item's text. An entry
   * of this kind **without** a `ref` is a tombstone: the item was deleted,
   * `text` says when, `source.from` is `evidence.delete` and the SHA-256 of
   * what was there stays in `source.evidence`.
   */
  | 'evidence';

/** Where a timeline entry came from. Every entry has one. */
export interface Provenance {
  /**
   * `operator` (a person typed it), the RPC method it was read from
   * (`alerts.get`, `changes.get`), or the core table it was read from
   * (`store.configs`, `store.logs`).
   */
  from: string;
  /** Which record and which of its fields, e.g. ``alert `a1`: started_at``. */
  evidence: string;
  /**
   * When the backend read it. Far from `ts` means the entry was added to the
   * timeline long after the thing itself happened.
   */
  gathered_at: number;
}

/** One line of an incident's timeline. */
export interface TimelineEntry {
  /** Derived from what the entry is about, so re-reading a source adds nothing. */
  id: string;
  /** When the thing happened — not when it was read. */
  ts: number;
  kind: TimelineKind;
  text: string;
  ref?: Ref;
  /**
   * Who did it, as the record says: a person, `nocturne` for something the
   * platform did by itself, empty where the source does not say. The core's
   * audit trail records no actor, so entries gathered from it are empty.
   */
  author: string;
  source: Provenance;
}

export interface FollowUp {
  text: string;
  done: boolean;
}

export interface Incident {
  id: string;
  title: string;
  severity: IncidentSeverity;
  state: IncidentState;
  /** When the trouble started — the alert's time when it was opened from one. */
  started_at: number;
  mitigated_at: number | null;
  resolved_at: number | null;
  acked_at: number | null;
  acked_by: string;
  /** Free text: the backend has no user directory. */
  assignee: string;
  device_ids: string[];
  alert_ids: string[];
  change_ids: string[];
  summary: string;
  impact: string;
  root_cause: string;
  follow_ups: FollowUp[];
  /** In time order, oldest first. */
  timeline: TimelineEntry[];
  opened_by: string;
  created_at: number;
  updated_at: number;
  /** Added by `incidents.get` and friends: the devices as the inventory has them now. */
  devices: DeviceRef[];
  /** How many timeline entries are a source that could not be read. */
  gaps: number;
}

/** An incident as a list shows it, without its timeline or its prose. */
export interface IncidentHead {
  id: string;
  title: string;
  severity: IncidentSeverity;
  state: IncidentState;
  started_at: number;
  mitigated_at?: number;
  resolved_at?: number;
  acked_at?: number;
  assignee: string;
  device_ids: string[];
  /** Resolved against the inventory when the list is read. */
  devices: DeviceRef[];
  alert_ids: string[];
  change_ids: string[];
  summary: string;
  entries: number;
  open_follow_ups: number;
  gaps: number;
  updated_at: number;
}

export interface IncidentsListParams {
  /** `open`, `mitigated`, `resolved`, `unresolved` (the first two) or `any`. */
  state?: string;
  severity?: IncidentSeverity;
  device_id?: string;
  /** Free text over the title, the prose and the timeline. */
  query?: string;
  /** Default 50, at most 500. */
  limit?: number;
  cursor?: string;
}

export interface IncidentsListResult {
  incidents: IncidentHead[];
  counts: {
    total: number;
    by_state: Record<IncidentState, number>;
    by_severity: Record<IncidentSeverity, number>;
  };
  next_cursor?: string;
}

export interface IncidentsCreateParams {
  /** Required unless `from_alert` is given, in which case the alert's title is used. */
  title?: string;
  /** Default `sev3`, or mapped from the alert's severity. */
  severity?: IncidentSeverity;
  /** Default now, or the alert's `started_at`. */
  started_at?: number;
  device_ids?: string[];
  alert_ids?: string[];
  change_ids?: string[];
  summary?: string;
  impact?: string;
  /**
   * Open it from an alert: its title, time, severity and device become the
   * incident's and the alert is linked. `unavailable` without an alerting
   * feature, `not_found` when there is no such alert — never a silent miss.
   */
  from_alert?: string;
  /** Who is opening it; the backend has no identity of its own. */
  author?: string;
  assignee?: string;
}

export interface IncidentsUpdateParams {
  id: string;
  title?: string;
  severity?: IncidentSeverity;
  /** Changing it stamps the times and leaves a `status` entry. */
  state?: IncidentState;
  summary?: string;
  impact?: string;
  root_cause?: string;
  /** Replaces the whole list. */
  follow_ups?: FollowUp[];
  /** An empty string unassigns. */
  assignee?: string;
  device_ids?: string[];
  started_at?: number;
  /** Why, for a state change that wants a reason. */
  note?: string;
  author?: string;
}

/** `incidents.get` / `.delete` / `.sync` / `.acknowledge` / `.suggest_links`. */
export interface IncidentsIdParams {
  id: string;
  author?: string;
  /** For `incidents.acknowledge`: what to say on the timeline. */
  note?: string;
}

export interface IncidentsNoteParams {
  id: string;
  text: string;
  /** When what is being noted happened; defaults to now. May be in the past. */
  ts?: number;
  author?: string;
}

export interface IncidentsLinkParams {
  id: string;
  alert_ids?: string[];
  change_ids?: string[];
  device_ids?: string[];
  author?: string;
}

/** `incidents.link` returns the incident with what the call did. */
export interface IncidentsLinkResult extends Incident {
  /** What was newly linked, as sentences. */
  linked: string[];
  timeline_entries_added: number;
}

export interface IncidentsResolveParams {
  id: string;
  /** Required and not blank: `invalid` otherwise. */
  root_cause: string;
  follow_ups?: FollowUp[];
  author?: string;
}

export interface IncidentsReopenParams {
  id: string;
  /** Required and not blank; it goes on the timeline. */
  why: string;
  author?: string;
}

export interface IncidentsAssignParams {
  id: string;
  /** An empty string unassigns. */
  to: string;
  author?: string;
}

/** A configuration change `incidents.suggest_links` offers. */
export interface SuggestedConfigChange {
  device_id: string;
  device: string;
  version_id: number;
  previous_version_id?: number;
  at: number;
  /** Absent for a device's first backup, which is a baseline, not a change. */
  lines_added?: number;
  lines_removed?: number;
  lines: number;
  /** `manual`, `scheduled`, `pre-change`, `post-change`, `import`. */
  source: string;
  note: string;
  first: boolean;
  /** How long before the incident started. Absent when it happened after. */
  before_start_ms?: number;
  /**
   * An applied change covers this device at this time, so somebody meant it.
   * `false` is the interesting case — and the reason this method exists.
   */
  explained_by_change: boolean;
}

export interface IncidentsSuggestResult {
  /** The incident's own window, extended backwards by the lead setting. */
  window: Period;
  alerts: AlertRef[];
  changes: ChangeRef[];
  config_changes: SuggestedConfigChange[];
  /** A source that could not be looked at. An empty list with a reason here is not "nothing to suggest". */
  unavailable: Unavailable[];
}

export interface IncidentsExportParams {
  id: string;
  /** Default `markdown`. */
  format?: Format;
  /** Minutes east of UTC for every time in the document; default the server's. */
  utc_offset_minutes?: number;
}

// ------------------------------------------------------------------- handover

export interface HandoverGenerateParams {
  since: number;
  /** Default now. The period is `[since, until)`. */
  until?: number;
}

/** A configuration change as the handover reports it. */
export interface HandoverConfigChange extends SuggestedConfigChange {
  /** No applied change covers this device at this time. */
  unplanned: boolean;
  /** `false` when there was no change list to check against, so `unplanned` is not claimed. */
  checked: boolean;
}

export interface WentDown extends DeviceRef {
  at: number;
  down_ms: number;
  /** Still down when the period ended. */
  ongoing: boolean;
  /** Which samples this was read from, and at what resolution. */
  from: string;
}

export interface HandoverDevices {
  added: DeviceRef[];
  /** Recovered from the audit trail: a deleted device leaves no other record. */
  removed: { device_id: string | null; name: string; at: number; from: string }[];
  went_down: WentDown[];
  unreachable_now: DeviceRef[];
  /** Devices with no reachability samples at all: whether they went down is unknown. */
  without_reachability_samples: number;
  /** Devices past the measurement limit, which were not looked at. */
  not_measured: number;
}

/** `handover.generate` result. Every item is a real record with its id. */
export interface Handover {
  period: Period;
  alerts: {
    fired: number;
    resolved: number;
    still_open: AlertRef[];
    truncated: boolean;
  } | null;
  /** This feature's own records: never `null`. */
  incidents: {
    opened: IncidentHead[];
    resolved: IncidentHead[];
    open: IncidentHead[];
  };
  changes: {
    applied: ChangeRef[];
    failed_or_rolled_back: ChangeRef[];
    /** Waiting now, whenever it was planned: what the next shift inherits. */
    awaiting_approval: ChangeRef[];
    truncated: boolean;
  } | null;
  config_changes: HandoverConfigChange[] | null;
  devices: HandoverDevices;
  /** Only the configuration feature knows a backup was attempted and failed. */
  backups: {
    failed: {
      device_id: string;
      device: string;
      error: string | null;
      last_backup_at: number | null;
    }[];
  } | null;
  jobs_failed: JobRef[];
  /**
   * What was captured in the period, oldest first: real records with ids and
   * hashes, never their content. `null` when the table could not be read (the
   * reason is in `unavailable`), and absent altogether from a handover saved
   * before evidence existed: neither of those is "nothing was captured".
   */
  evidence?: { captured: Evidence[] } | null;
  /** Empty from `generate`; the human part is written through `handover.save`. */
  notes: string;
  unavailable: Unavailable[];
  generated_at: number;
}

export interface HandoverSaveParams {
  /**
   * The generated document. Only its `period` is read: the data sections are
   * generated again on the server, so nothing a client — or a model asked to
   * tidy the prose — writes can end up in them.
   */
  handover?: Handover;
  /** The period, when the whole document is not being sent back. */
  since?: number;
  until?: number;
  /** The human part. The only field stored as sent. */
  notes?: string;
  author?: string;
  /** Update one saved earlier instead of making a new one. */
  id?: string;
}

export interface SavedHandover {
  id: string;
  period: Period;
  /** The server's own generated copy. */
  data: Handover;
  notes: string;
  author: string;
  created_at: number;
  updated_at: number;
}

/** A saved handover as the list shows it. A `null` count had no source. */
export interface HandoverHead {
  id: string;
  period: Period;
  author: string;
  incidents_opened?: number | null;
  incidents_open?: number | null;
  alerts_fired?: number | null;
  changes_applied?: number | null;
  unavailable: Unavailable[];
  has_notes: boolean;
  created_at: number;
  updated_at: number;
}

export interface HandoverListParams {
  /** Free text over the prose. */
  query?: string;
  limit?: number;
  cursor?: string;
}

export interface HandoverExportParams {
  /** A saved handover, or `since`/`until` to export one without saving it. */
  id?: string;
  since?: number;
  until?: number;
  format?: Format;
  utc_offset_minutes?: number;
  /** Prose for an unsaved export. */
  notes?: string;
}

// -------------------------------------------------------------------- reports

export type ReportKind =
  | 'inventory'
  | 'software'
  | 'availability'
  | 'changes'
  | 'backups'
  | 'compliance'
  | 'alerts';

export interface ReportKindInfo {
  id: ReportKind;
  title: string;
  description: string;
  /** It covers a period rather than the present, so `since`/`until` matter. */
  period: boolean;
  /** The RPC method it cannot be made without, if any. */
  needs: string | null;
  /** `false` when `needs` is not in this build. Generating it still works and says so. */
  available: boolean;
  unavailable_reason: string | null;
}

export interface ReportsKindsResult {
  kinds: ReportKindInfo[];
  formats: Format[];
}

export interface ReportsGenerateParams {
  kind: ReportKind;
  /** Default `until` minus seven days, for a report that covers a period. */
  since?: number;
  /** Default now. */
  until?: number;
  /** The core's device filter: `{site?, role?, tag?, profile?, status?, lab?, query?}`. */
  filter?: Record<string, unknown>;
  /** Default `markdown`. */
  format?: Format;
  utc_offset_minutes?: number;
}

export interface GeneratedReport extends Rendered {
  kind: ReportKind;
  /** The sections that had no source. Empty for a report that needs only the core. */
  unavailable: Unavailable[];
  /** Present for a report that covers a period. */
  period: Period | null;
}

export interface ReportsSaveParams extends ReportsGenerateParams {
  /** A title of your own; the report's own otherwise. */
  title?: string;
  notes?: string;
  author?: string;
}

export interface SavedReport {
  id: string;
  kind: string;
  title: string;
  format: string;
  /** The text the server generated. A client cannot hand in content of its own. */
  content: string;
  rows: number;
  period: Period | null;
  filter: Record<string, unknown> | null;
  notes: string;
  author: string;
  unavailable: Unavailable[];
  generated_at: number;
  created_at: number;
}

/** A saved report as the list shows it: the size, not the text. */
export interface ReportHead {
  id: string;
  kind: string;
  title: string;
  format: string;
  rows: number;
  period?: Period;
  bytes: number;
  notes: string;
  author: string;
  unavailable: Unavailable[];
  generated_at: number;
}

export interface ReportsListParams {
  kind?: ReportKind;
  query?: string;
  limit?: number;
  cursor?: string;
}

// ---------------------------------------------------------------------- notes

/** A `[[device name]]` in a note's text, resolved when it was saved. */
export interface NoteLink {
  text: string;
  /** Absent when the inventory has no such device: the link is shown unresolved, not dropped. */
  device_id?: string | null;
  /** Added by `notes.get`. */
  device?: DeviceRef | null;
  resolved?: boolean;
}

export interface Note {
  id: string;
  title: string;
  /** Markdown, as written. The backend never renders it. */
  body: string;
  tags: string[];
  /** Including the devices named in the body. */
  device_ids: string[];
  sites: string[];
  incident_ids: string[];
  links: NoteLink[];
  author: string;
  created_at: number;
  updated_at: number;
  /** Added by `notes.get`. */
  devices?: DeviceRef[];
}

export interface NoteHead {
  id: string;
  title: string;
  tags: string[];
  device_ids: string[];
  sites: string[];
  incident_ids: string[];
  author: string;
  /** How long the note is, so a list can say "12 lines" without the body. */
  lines: number;
  updated_at: number;
  /** Present when the list was filtered by text: the piece the words are in. */
  excerpt?: string;
}

export interface NotesListParams {
  device_id?: string;
  site?: string;
  tag?: string;
  incident_id?: string;
  query?: string;
  limit?: number;
  cursor?: string;
}

export interface NotesSaveParams {
  /** Left out for a new note. */
  id?: string;
  title: string;
  body?: string;
  tags?: string[];
  device_ids?: string[];
  sites?: string[];
  incident_ids?: string[];
  author?: string;
}

// ------------------------------------------------------------------- evidence

/**
 * Evidence: a record of something Nocturne itself read, at a stated time,
 * from a stated source. Text or data, never pixels.
 *
 * Contract: `docs/dev/spec-capture.md` ("Evidence") and
 * `docs/dev/features/ops.md` ("Evidence (contract)"). Every shape below was
 * typed from a real answer of `nocturne-server` and from the recording in the
 * backend's own test run, not from the specification's prose.
 *
 * Two rules decide how a screen may show an item, and both are about what is
 * *in* it rather than about this type:
 *
 * - **A drawing is hostile input.** `body.svg` is stored as `topology.export`
 *   returned it; it is not sanitised and may hold a `<script>`, an `onload`
 *   or an address to fetch. Show it only as an `<img>` whose `src` is a
 *   `data:` or `blob:` address made from the string, where a browser runs no
 *   script and fetches nothing. Never as markup, never in a frame or an
 *   `<object>`, never a window opened on the address.
 * - **Text is stored as it was printed**, escape sequences included. Show it
 *   as text; never write it into a terminal emulator.
 */
export type EvidenceKind =
  /** The last lines of a terminal session. */
  | 'terminal'
  /** A piece of a session log. */
  | 'log'
  /** Text a person handed in. Its source is `operator`, and it says so wherever it is shown. */
  | 'text'
  /** The topology's own drawing (SVG). */
  | 'topology'
  /** Samples of one or more series over a period, with their coverage. */
  | 'chart';

/** Where an item came from. Recorded by the engine, not typed by a person. */
export interface EvidenceSource {
  /**
   * The method the content was read with — `terminal.snapshot`,
   * `terminal.logs.read`, `topology.export`, `metrics.query_many` — or
   * `operator` for text a person handed in.
   */
  from: string;
  session_id?: string;
  session_title?: string;
  device_id?: string;
  /** The device's name when the item was captured. */
  device?: string;
  /** The far end of the session (`lab:Ops/SW1`, `admin@10.0.0.1`). */
  peer?: string;
  /** It comes from, or shows, a simulated device. */
  lab: boolean;
  /** What was asked for: lines, a view and filter, the series and period. Redacted like the rest. */
  params?: unknown;
  /**
   * The engine's own remark, with the person's note after it when one was
   * given: `text as sent by the client, redacted on arrival; the operator's
   * note: ticket NET-431`, or which bytes of a log were read.
   */
  note?: string;
}

/** An evidence item without its body: what a list, an event and a handover carry. */
export interface Evidence {
  id: string;
  kind: EvidenceKind;
  title: string;
  captured_at: number;
  source: EvidenceSource;
  /** Always `true`: there is no way to store an item otherwise. */
  redacted: boolean;
  /** Lines of text. Absent for a drawing and for a chart. */
  lines?: number;
  /** The size of the body as stored (for a chart, of its JSON). */
  bytes: number;
  /** Of the body as stored, in hexadecimal. */
  sha256: string;
  /** The incidents it is attached to. */
  incident_ids: string[];
}

/**
 * One series of a stored chart, exactly as `metrics.query_many` returned it,
 * holes included. The monitor sends these fields; they are all optional here
 * because the item stores whatever was answered, string by string redacted,
 * and a screen must not assume more than it checks.
 */
export interface EvidenceChartSeries {
  device_id?: string;
  metric?: string;
  instance?: string;
  /** `[time in ms, value]`, only the buckets that had data: a gap is a missing point. */
  points?: unknown;
  step_s?: number;
  rollup?: string;
  [other: string]: unknown;
}

/** How much of the period one series has samples for, as it was read. */
export interface EvidenceSeriesCoverage {
  device_id: string;
  /** The device's name when the item was captured. */
  device: string;
  lab: boolean;
  metric: string;
  /** Empty for a device-level metric. */
  instance: string;
  points: number;
  /** Absent for a series without points. */
  first_at?: number;
  last_at?: number;
  /** The longest stretch between two neighbouring points. Absent with fewer than two. */
  longest_gap_ms?: number;
  step_s?: number;
  /** `raw`, `5m`, `1h`: where the points came from. */
  rollup?: string;
}

export interface EvidenceCoverage {
  since: number;
  until: number;
  /** Points over all the series. */
  points: number;
  /** One entry per series, in the order they were asked for. */
  series: EvidenceSeriesCoverage[];
}

/** The content of an item. Only `evidence.get` and the capture methods return it. */
export type EvidenceBody =
  /** kind `terminal` | `log` | `text` */
  | { type: 'text'; text: string }
  /** kind `topology`. Hostile input: see {@link EvidenceKind}. */
  | { type: 'svg'; svg: string }
  /** kind `chart` */
  | { type: 'chart'; series: EvidenceChartSeries[]; coverage: EvidenceCoverage };

export interface EvidenceWithBody extends Evidence {
  body: EvidenceBody;
}

export interface EvidenceCaptureTerminalParams {
  session_id: string;
  /** 1 to 10,000. Default 200. */
  lines?: number;
  title?: string;
}

export interface EvidenceCaptureLogParams {
  /** The log's name, as `terminal.logs.list` gives it. */
  name: string;
  /** Default 0. */
  offset?: number;
  /** At most 262,144 bytes, which is also the default. */
  limit?: number;
  title?: string;
}

/** The same graph `topology.export` is asked for, without the format: the engine stores its SVG. */
export interface EvidenceCaptureTopologyParams {
  view?: string;
  layer?: unknown;
  filter?: unknown;
  include_endpoints?: boolean;
  title?: string;
}

export interface EvidenceSeriesRef {
  device_id: string;
  metric: string;
  /** Empty or absent for a device-level metric. */
  instance?: string;
}

export interface EvidenceCaptureChartParams {
  /** Every device must be in the inventory. */
  series: EvidenceSeriesRef[];
  since: number;
  /** Must be after `since`. */
  until: number;
  step_s?: number;
  title?: string;
}

export interface EvidenceAddTextParams {
  text: string;
  title: string;
  /** Recorded whether or not the session can still be found, so it must be shaped like an id. */
  session_id?: string;
  device_id?: string;
  /** At most 1,000 characters. */
  note?: string;
}

export interface EvidenceListParams {
  kind?: EvidenceKind;
  device_id?: string;
  incident_id?: string;
  /** Words that must all occur in the title, the device, the session or the note. The content is not searched. */
  query?: string;
  /** Default 50, at most 200. */
  limit?: number;
  cursor?: string;
}

export interface EvidenceListResult {
  /** Newest first. */
  items: Evidence[];
  /** Present when older items remain. */
  next_cursor?: string;
}

export interface EvidenceRenameParams {
  id: string;
  /** At most 200 characters. */
  title: string;
}

export interface EvidenceAttachParams {
  id: string;
  incident_id: string;
  author?: string;
}

export interface EvidenceDeleteParams {
  id: string;
  author?: string;
}

export interface EvidenceDeleteResult {
  deleted: true;
  /** The incidents that keep a tombstone where the item was. */
  tombstones: string[];
}

/**
 * `details` of an `invalid` refusal past a stated limit. Nothing was stored
 * and nothing was cut; `message` says what to do instead.
 */
export interface EvidenceLimitDetails {
  limit: 'text_bytes' | 'svg_bytes' | 'points' | 'items' | 'params_bytes';
  max: number;
  actual: number;
}

// --------------------------------------------------------------------- search

export interface SearchParams {
  q: string;
  /** Per kind. Default 8. */
  limit?: number;
}

/**
 * `search.query` result. The first three are the backend's own records and
 * are always there; the rest come from other features with a short budget
 * each, and a source that was slow, broken or absent is `null` with its
 * reason in `partial`.
 */
export interface SearchResult {
  query: string;
  devices: DeviceRef[];
  notes: NoteHead[];
  incidents: IncidentHead[];
  /** Matched over the newest page of changes only; see `changes_truncated`. */
  changes: ChangeRef[] | null;
  changes_truncated?: boolean;
  /** `config.search`'s matches, passed through. */
  config: unknown[] | null;
  /** `ipam.search`'s answer, passed through. */
  addresses: { prefixes?: unknown[]; addresses?: unknown[]; vlans?: unknown[] } | null;
  /** `logs.query`'s entries, passed through. */
  logs: unknown[] | null;
  /**
   * Only for a query that looks like an IP or MAC address: `locate.host`'s
   * answer ("which switch port is this plugged into"). `null` when the query
   * is not an address, when the host was not found, or — with a reason in
   * `partial` — when nothing in this build can answer it.
   */
  location: unknown | null;
  partial: Unavailable[];
  generated_at: number;
}

// ---------------------------------------------------------------------- audit

export interface AuditSearchParams {
  since?: number;
  until?: number;
  device_id?: string;
  /** An exact action (`incidents.create`) or a prefix ending in `.` (`incidents.`). */
  action?: string;
  /** Free text in the message or the action. */
  text?: string;
  /** Only entries whose own detail names this record. */
  reference?: Ref;
  /** Default 100, at most 1000. */
  limit?: number;
  /** Page backwards: only entries older than this one. */
  before_id?: number;
}

/** A reference in an audit entry, joined to the record it names. */
export interface AuditRef extends Ref {
  /** `null` when the record is gone, or belongs to a feature this view cannot read. */
  title: string | null;
  resolved: boolean;
}

export interface AuditEntry {
  id: number;
  ts: number;
  /** What was done, as the feature that did it named it (`incidents.create`). */
  action: string;
  /** A sentence in the past tense. */
  message: string;
  device_id: string | null;
  /** The device as the inventory has it now, `deleted: true` when it is gone. */
  device: DeviceRef | null;
  /** Only the records the entry's own detail names. Nothing is inferred. */
  refs: AuditRef[];
  /** The structured detail the feature recorded, whatever shape it has. */
  detail: unknown | null;
}

export interface AuditSearchResult {
  entries: AuditEntry[];
  has_more: boolean;
  next_before_id: number | null;
  read_at: number;
}

export interface AuditActionsResult {
  actions: { action: string; count: number }[];
  /** The trail is longer than what was counted. */
  truncated: boolean;
}

// -------------------------------------------------------------------- methods

/** `overview.get` {@link OverviewParams} → {@link Overview} */
/** `incidents.list` {@link IncidentsListParams} → {@link IncidentsListResult} */
/** `incidents.get` {@link IncidentsIdParams} → {@link Incident} */
/** `incidents.create` {@link IncidentsCreateParams} → {@link Incident} */
/** `incidents.update` {@link IncidentsUpdateParams} → {@link Incident} */
/** `incidents.delete` {@link IncidentsIdParams} → `{ deleted: boolean }` */
/** `incidents.note` {@link IncidentsNoteParams} → {@link Incident} */
/** `incidents.link` {@link IncidentsLinkParams} → {@link IncidentsLinkResult} */
/** `incidents.sync` {@link IncidentsIdParams} → {@link IncidentsLinkResult} — read every source again without linking anything new */
/** `incidents.acknowledge` {@link IncidentsIdParams} → {@link Incident}; `conflict` when it was already acknowledged */
/** `incidents.assign` {@link IncidentsAssignParams} → {@link Incident} */
/** `incidents.resolve` {@link IncidentsResolveParams} → {@link Incident}; `invalid` without a cause */
/** `incidents.reopen` {@link IncidentsReopenParams} → {@link Incident}; `invalid` without a reason */
/** `incidents.suggest_links` {@link IncidentsIdParams} → {@link IncidentsSuggestResult} */
/** `incidents.export` {@link IncidentsExportParams} → {@link Rendered} — the post-incident review */
/** `handover.generate` {@link HandoverGenerateParams} → {@link Handover} */
/** `handover.save` {@link HandoverSaveParams} → {@link SavedHandover} — the data sections are regenerated here */
/** `handover.list` {@link HandoverListParams} → `{ handovers: `{@link HandoverHead}`[], next_cursor: string | null }` */
/** `handover.get` `{ id }` → {@link SavedHandover} */
/** `handover.delete` `{ id }` → `{ deleted: boolean }` */
/** `handover.export` {@link HandoverExportParams} → {@link Rendered} */
/** `reports.kinds` `{}` → {@link ReportsKindsResult} — what this build can report on, and why not the rest */
/** `reports.generate` {@link ReportsGenerateParams} → {@link GeneratedReport} */
/** `reports.save` {@link ReportsSaveParams} → {@link SavedReport} — generated here and stored */
/** `reports.list` {@link ReportsListParams} → `{ reports: `{@link ReportHead}`[], next_cursor: string | null }` */
/** `reports.get` `{ id }` → {@link SavedReport} */
/** `reports.delete` `{ id }` → `{ deleted: boolean }` */
/** `notes.list` {@link NotesListParams} → `{ notes: `{@link NoteHead}`[], next_cursor: string | null }` */
/** `notes.get` `{ id }` → {@link Note} */
/** `notes.save` {@link NotesSaveParams} → {@link Note} */
/** `notes.delete` `{ id }` → `{ deleted: boolean }` */
/** `evidence.capture_terminal` {@link EvidenceCaptureTerminalParams} → {@link EvidenceWithBody} — the engine asks `terminal.snapshot` itself; `unavailable` without the terminal feature */
/** `evidence.capture_log` {@link EvidenceCaptureLogParams} → {@link EvidenceWithBody} */
/** `evidence.capture_topology` {@link EvidenceCaptureTopologyParams} → {@link EvidenceWithBody} — the SVG `topology.export` returns */
/** `evidence.capture_chart` {@link EvidenceCaptureChartParams} → {@link EvidenceWithBody} — the points `metrics.query_many` returns: gaps stay gaps */
/** `evidence.add_text` {@link EvidenceAddTextParams} → {@link EvidenceWithBody} — `source.from` is `operator` */
/** `evidence.list` {@link EvidenceListParams} → {@link EvidenceListResult} */
/** `evidence.get` `{ id }` → {@link EvidenceWithBody} */
/** `evidence.rename` {@link EvidenceRenameParams} → {@link Evidence} */
/** `evidence.attach` / `evidence.detach` {@link EvidenceAttachParams} → {@link Evidence}, with its `incident_ids` as they now are */
/** `evidence.delete` {@link EvidenceDeleteParams} → {@link EvidenceDeleteResult} */
/** `search.query` {@link SearchParams} → {@link SearchResult} */
/** `audit.search` {@link AuditSearchParams} → {@link AuditSearchResult} — the core's audit trail, filtered and joined */
/** `audit.actions` `{ since? }` → {@link AuditActionsResult} — for a filter menu */

// --------------------------------------------------------------------- events

/** `incidents.changed` — an {@link IncidentHead}, or `{ id, deleted: true }`. */
export type IncidentsChangedEvent = IncidentHead | { id: string; deleted: true };

/** `handover.changed` — a {@link HandoverHead}, or `{ id, deleted: true }`. */
export type HandoverChangedEvent = HandoverHead | { id: string; deleted: true };

/** `notes.changed` — a {@link NoteHead}, or `{ id, deleted: true }`. */
export type NotesChangedEvent = NoteHead | { id: string; deleted: true };

/** `reports.changed` — a {@link ReportHead}, or `{ id, deleted: true }`. */
export type ReportsChangedEvent = ReportHead | { id: string; deleted: true };

/**
 * `evidence.changed` — the {@link Evidence} without its body, or
 * `{ id, deleted: true }`. Attach, detach, rename and delete also publish
 * `incidents.changed` for each incident whose timeline moved.
 */
export type EvidenceChangedEvent = Evidence | { id: string; deleted: true };

// ------------------------------------------------------------------- settings

/** The `ops` settings section (`settings.get` / `settings.set`). */
export interface OpsSettings {
  /** How long an overview may be reused before it is made again. Default 5000. */
  overview_cache_ms: number;
  /** How long `search.query` waits for one of the features it fans out to. Default 300. */
  search_budget_ms: number;
  /**
   * How far before an incident started `incidents.suggest_links` looks for
   * something that might have caused it. Default 30.
   */
  suggest_lead_minutes: number;
  /**
   * How far a configuration change may sit outside an applied change's own
   * window and still count as that change's work. Beyond it the handover
   * calls it `unplanned`. Default 10.
   */
  unplanned_grace_minutes: number;
  /** How many devices an availability report measures. Default 2000. */
  availability_max_devices: number;
}
