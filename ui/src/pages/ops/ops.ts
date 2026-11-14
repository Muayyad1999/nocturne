/**
 * The Operations screen's reasoning, with no React in it.
 *
 * This screen is the record of what happened, so it is the one place in the
 * interface where being wrong is worse than being empty. Four decisions
 * therefore live here, in plain functions with tests, rather than inside
 * components:
 *
 * 1. **`null` is not `0`.** Every section of a handover and every part of a
 *    report that comes from another feature is independently nullable, and the
 *    reason is in the sibling `unavailable` list. {@link readSection} returns
 *    two *different shapes* for "here is the data" and "there is no data and
 *    here is why", so a component cannot render the second as a zero by
 *    accident. {@link emptyMeaning} then separates the third case — the
 *    section was read, and nothing happened — from both.
 * 2. **One timeline, in the right order.** An incident's timeline comes from
 *    the backend already merged and sorted, but the screen has a second
 *    source: `incidents.suggest_links`, the alerts, changes and configuration
 *    movement inside the incident's window that nobody has linked yet. Those
 *    belong *on the timeline, at their own times* — "this switch's
 *    configuration changed four minutes before the outage" is only useful
 *    where it can be seen next to the outage — so {@link mergeTimeline}
 *    interleaves them under one explicit order.
 * 3. **A generated summary is auditable or it is worthless.**
 *    {@link handoverSections} turns a handover into sections of items, and
 *    every item it produces carries a link to the record it came from. The
 *    unit test asserts that invariant for a handover with something in every
 *    section: a summary nobody can check would be trusted anyway.
 * 4. **A zero timestamp is not a time.** The backend sends `null` for "never"
 *    on the fields it can, but some records carry a `0`, and `TimeAgo` would
 *    render that as "56 years ago". {@link instant} is the only way this
 *    screen turns a number into a time.
 */

import type { StatusTone } from '@/design/tone';
import { formatCount, formatDuration, plural } from '@/lib/format';
import type {
  AlertRef,
  AuditRef,
  ChangeRef,
  DeviceRef,
  GeneratedReport,
  Handover,
  HandoverConfigChange,
  IncidentHead,
  IncidentSeverity,
  IncidentState,
  IncidentsSuggestResult,
  JobRef,
  Ref,
  SuggestedConfigChange,
  TimelineEntry,
  TimelineKind,
  Unavailable,
  UnavailableReason,
  WentDown,
} from '@/api/types/ops';

// --------------------------------------------------------------- null vs zero

/**
 * Either the data, or why there is none. Two shapes on purpose: a caller
 * cannot read `.value` off a section the backend did not answer.
 */
export type Reading<T> =
  | { present: true; value: T }
  | { present: false; reason: UnavailableReason; message: string };

/**
 * One nullable section of a result, with the reason beside it when it is
 * `null`.
 *
 * A `null` section with no entry in `unavailable` should not happen — the
 * backend writes one for every section it blanks — but inventing a zero would
 * be the one unforgivable answer, so it reports a failure with no reason
 * instead.
 */
export function readSection<T>(
  value: T | null | undefined,
  unavailable: readonly Unavailable[],
  section: string,
): Reading<NonNullable<T>> {
  if (value !== null && value !== undefined) return { present: true, value: value as NonNullable<T> };
  const why = unavailable.find((entry) => entry.section === section);
  if (why) return { present: false, reason: why.reason, message: why.message };
  return {
    present: false,
    reason: 'failed',
    message: `\`${section}\` was not reported, and no reason was given for it.`,
  };
}

/** The headline for a section that has no data: five reasons, five sentences. */
export const REASON_HEADLINE: Record<UnavailableReason, string> = {
  not_installed: 'Not in this build',
  failed: 'Could not be read',
  timeout: 'Did not answer in time',
  malformed: 'Answer not understood',
  no_data: 'Nothing recorded yet',
};

/**
 * How loud a missing section is. A feature left out of the build is a fact
 * about the build; one that failed, timed out or answered nonsense is
 * something to look into.
 */
export const REASON_TONE: Record<UnavailableReason, StatusTone> = {
  not_installed: 'idle',
  failed: 'warn',
  timeout: 'warn',
  malformed: 'warn',
  no_data: 'idle',
};

/** A count read from a capped list is a floor, and says so. */
export function floorText(count: number, truncated: boolean): string {
  return truncated ? `at least ${formatCount(count)}` : formatCount(count);
}

/**
 * A timestamp that can be shown as a time, or `null`.
 *
 * `0`, a negative number and anything non-finite are not instants: a record
 * that never happened carries one of those, and rendering it would put
 * "56 years ago" on screen next to real times.
 */
export function instant(at: number | null | undefined): number | null {
  if (at == null || !Number.isFinite(at) || at <= 0) return null;
  return at;
}

// ------------------------------------------------------------------ vocabulary

export const SEVERITY_LABEL: Record<IncidentSeverity, string> = {
  sev1: 'Sev 1',
  sev2: 'Sev 2',
  sev3: 'Sev 3',
  sev4: 'Sev 4',
};

/** What each severity means, in the words the backend's author used. */
export const SEVERITY_MEANING: Record<IncidentSeverity, string> = {
  sev1: 'The campus is down.',
  sev2: 'A site or a service is badly affected.',
  sev3: 'Something is broken but contained.',
  sev4: 'Somebody should look at this eventually.',
};

export const SEVERITY_TONE: Record<IncidentSeverity, StatusTone> = {
  sev1: 'crit',
  sev2: 'crit',
  sev3: 'warn',
  sev4: 'info',
};

export const STATE_LABEL: Record<IncidentState, string> = {
  open: 'Open',
  mitigated: 'Mitigated',
  resolved: 'Resolved',
};

export const STATE_MEANING: Record<IncidentState, string> = {
  open: 'Happening, and nobody has stopped the bleeding yet.',
  mitigated: 'The users are served again; the cause is still there.',
  resolved: 'Done, with a cause written down.',
};

export const STATE_TONE: Record<IncidentState, StatusTone> = {
  open: 'crit',
  mitigated: 'warn',
  resolved: 'ok',
};

/** What a timeline entry is, as a word. */
export const ENTRY_WORD: Record<TimelineKind, string> = {
  note: 'Note',
  alert: 'Alert',
  change: 'Change',
  command: 'Command',
  status: 'Status',
  log: 'Log',
  gap: 'Gap',
};

export const ENTRY_TONE: Record<TimelineKind, StatusTone> = {
  note: 'info',
  alert: 'crit',
  change: 'maint',
  command: 'idle',
  status: 'ok',
  log: 'idle',
  // A hole in the evidence is a warning, not a neutral line.
  gap: 'warn',
};

/** How long an incident lasted, or has been running. */
export function incidentDuration(
  incident: { started_at: number; resolved_at?: number | null },
  now: number,
): { label: string; running: boolean } | null {
  const started = instant(incident.started_at);
  if (started === null) return null;
  const ended = instant(incident.resolved_at);
  const until = ended ?? now;
  if (until < started) return null;
  return { label: formatDuration(until - started), running: ended === null };
}

// -------------------------------------------------------------------- linking

/** Where a reference goes, or `null` when this build has nowhere to send it. */
export function refLink(reference: Ref | AuditRef, deviceId?: string | null): string | null {
  const id = encodeURIComponent(reference.id);
  switch (reference.type) {
    case 'incident':
      return `/ops/incidents?incident=${id}`;
    case 'change':
      return `/changes/${id}`;
    case 'note':
      return `/ops/notes?note=${id}`;
    case 'report':
      return `/ops/reports?report=${id}`;
    case 'handover':
      return `/ops/handover?saved=${id}`;
    case 'job':
      return `/ops/jobs?job=${id}`;
    case 'device':
      return `/devices/${id}`;
    case 'alert':
      // Alerts have no page of their own; the device's health page is where
      // this one can be seen in context.
      return deviceId ? `/monitoring/health?device=${encodeURIComponent(deviceId)}` : '/monitoring/alerts';
    case 'config_version':
      // The version id cannot be addressed from the Configuration screen, so
      // the device's configuration history is as close as this can point.
      return deviceId ? `/config?device=${encodeURIComponent(deviceId)}` : null;
    case 'lab':
      return `/lab/${id}`;
    default:
      return null;
  }
}

/** Where a timeline entry's own reference goes. */
export function entryLink(entry: TimelineEntry, deviceIds: readonly string[]): string | null {
  if (!entry.ref) return null;
  // An entry about a device-shaped thing has no device of its own, so the
  // incident's first device is the best context available.
  return refLink(entry.ref, deviceIds[0] ?? null);
}

// ------------------------------------------------------------------- timeline

/** Something inside the incident's window that nobody has linked to it yet. */
export interface Suggestion {
  /** Stable, and the tie-breaker when two things share a millisecond. */
  key: string;
  kind: 'alert' | 'change' | 'config';
  at: number;
  /** One line: what it was. */
  text: string;
  /** A second line: where, and how big. */
  detail: string;
  to: string | null;
  /** What `incidents.link` needs to adopt it, or `null` when it cannot be linked. */
  link: { alert_ids?: string[]; change_ids?: string[] } | null;
  lab: boolean;
  /**
   * For configuration movement: an applied change covers this device at this
   * time, so somebody meant it. `false` is the interesting case.
   */
  explained: boolean;
  /** How long before the incident started it happened, when it did. */
  beforeStartMs: number | null;
}

function configSuggestion(change: SuggestedConfigChange): Suggestion {
  const added = change.lines_added;
  const removed = change.lines_removed;
  const size =
    added === undefined && removed === undefined
      ? 'the difference was not computed'
      : `+${formatCount(added ?? 0)} −${formatCount(removed ?? 0)} lines`;
  return {
    key: `config:${change.device_id}:${change.version_id}`,
    kind: 'config',
    at: change.at,
    text: `${change.device} configuration changed`,
    detail: `${size} · ${change.source}`,
    to: `/config?device=${encodeURIComponent(change.device_id)}`,
    // Configuration movement is evidence, not a record this feature can link:
    // the device is already linked, and there is no id for a snapshot.
    link: null,
    lab: false,
    explained: change.explained_by_change,
    beforeStartMs: change.before_start_ms ?? null,
  };
}

/** The suggestions `incidents.suggest_links` offered, as timeline material. */
export function suggestions(result: IncidentsSuggestResult | undefined): Suggestion[] {
  if (!result) return [];
  const out: Suggestion[] = [];
  for (const alert of result.alerts) {
    out.push({
      key: `alert:${alert.id}`,
      kind: 'alert',
      at: alert.started_at,
      text: alert.title,
      detail: [alert.severity, alert.device, alert.rule].filter(Boolean).join(' · '),
      to: refLink({ type: 'alert', id: alert.id }, alert.device_id ?? null),
      link: { alert_ids: [alert.id] },
      lab: alert.lab,
      explained: false,
      beforeStartMs: null,
    });
  }
  for (const change of result.changes) {
    out.push({
      key: `change:${change.id}`,
      kind: 'change',
      at: change.updated_at,
      text: change.title,
      detail: [change.state, change.ticket, change.origin].filter(Boolean).join(' · '),
      to: `/changes/${encodeURIComponent(change.id)}`,
      link: { change_ids: [change.id] },
      lab: false,
      explained: false,
      beforeStartMs: null,
    });
  }
  for (const change of result.config_changes) out.push(configSuggestion(change));
  return out;
}

/** One line of the timeline: something that happened, or something offered. */
export type TimelineRow =
  | { kind: 'entry'; key: string; at: number; entry: TimelineEntry }
  | { kind: 'suggestion'; key: string; at: number; suggestion: Suggestion };

/**
 * The incident's recorded timeline and the unlinked suggestions, in one order.
 *
 * The order is `ts`, then recorded facts before suggestions, then the key.
 * The middle rule is the only one that needed a decision: at the same
 * instant, what is on the record is shown above what is merely offered, so
 * reading down the list never puts a suggestion between a fact and its
 * consequence.
 */
export function mergeTimeline(
  entries: readonly TimelineEntry[],
  offered: readonly Suggestion[],
): TimelineRow[] {
  const rows: TimelineRow[] = [
    ...entries.map(
      (entry): TimelineRow => ({ kind: 'entry', key: entry.id, at: entry.ts, entry }),
    ),
    ...offered.map(
      (suggestion): TimelineRow => ({
        kind: 'suggestion',
        key: suggestion.key,
        at: suggestion.at,
        suggestion,
      }),
    ),
  ];
  const rank = (row: TimelineRow) => (row.kind === 'entry' ? 0 : 1);
  rows.sort((a, b) => a.at - b.at || rank(a) - rank(b) || (a.key < b.key ? -1 : a.key > b.key ? 1 : 0));
  // A suggestion the backend offers for something already on the timeline
  // would be a duplicate line; the entry wins.
  const seen = new Set(rows.filter((row) => row.kind === 'entry').map((row) => row.key));
  return rows.filter((row) => row.kind === 'entry' || !seen.has(row.key));
}

/** The timeline split into days, in the order {@link mergeTimeline} gave. */
export function timelineDays(
  rows: readonly TimelineRow[],
  dayOf: (at: number) => string,
): { day: string; rows: TimelineRow[] }[] {
  const days: { day: string; rows: TimelineRow[] }[] = [];
  for (const row of rows) {
    const day = dayOf(row.at);
    const last = days[days.length - 1];
    if (last && last.day === day) last.rows.push(row);
    else days.push({ day, rows: [row] });
  }
  return days;
}

/** How many of a timeline's entries are a source that could not be read. */
export function gapCount(entries: readonly TimelineEntry[]): number {
  return entries.filter((entry) => entry.kind === 'gap').length;
}

// ------------------------------------------------------------------- handover

/** One item of a generated handover: a real record, and the way to check it. */
export interface HandoverItem {
  id: string;
  /** What it was. */
  text: string;
  /** Where and when, in a second line. */
  detail?: string;
  /** The record this item was made from. Never `null`: see the module note. */
  to: string;
  tone?: StatusTone;
  lab?: boolean;
  at?: number | null;
}

/**
 * Why a section has no items. The distinction the whole screen turns on:
 * `nothing` was read and was empty; `unavailable` was not read at all.
 */
export type HandoverBody =
  | { kind: 'items'; items: HandoverItem[] }
  | { kind: 'nothing'; sentence: string }
  | { kind: 'unavailable'; reason: UnavailableReason; message: string };

export interface HandoverSection {
  id: string;
  title: string;
  /** How the section was counted, or what it deliberately leaves out. */
  note?: string;
  /** A short summary of the figures, when the section has any. */
  summary?: string;
  body: HandoverBody;
}

function deviceItem(device: DeviceRef, extra?: Partial<HandoverItem>): HandoverItem {
  return {
    id: device.id,
    text: device.name,
    detail: device.site || undefined,
    to: `/devices/${encodeURIComponent(device.id)}`,
    lab: device.lab,
    ...extra,
  };
}

function alertItem(alert: AlertRef): HandoverItem {
  return {
    id: alert.id,
    text: alert.title,
    detail: [alert.severity, alert.device, alert.state].filter(Boolean).join(' · '),
    to: refLink({ type: 'alert', id: alert.id }, alert.device_id ?? null) ?? '/monitoring/alerts',
    tone: alert.state === 'resolved' ? 'ok' : 'crit',
    lab: alert.lab,
    at: instant(alert.started_at),
  };
}

function changeItem(change: ChangeRef, tone: StatusTone): HandoverItem {
  return {
    id: change.id,
    text: change.title,
    detail: [change.state, change.ticket, change.origin].filter(Boolean).join(' · '),
    to: `/changes/${encodeURIComponent(change.id)}`,
    tone,
    at: instant(change.updated_at),
  };
}

function incidentItem(incident: IncidentHead): HandoverItem {
  return {
    id: incident.id,
    text: incident.title,
    detail: [
      SEVERITY_LABEL[incident.severity],
      STATE_LABEL[incident.state],
      incident.assignee || undefined,
      incident.gaps > 0 ? `${plural(incident.gaps, 'gap')} in its evidence` : undefined,
    ]
      .filter(Boolean)
      .join(' · '),
    to: `/ops/incidents?incident=${encodeURIComponent(incident.id)}`,
    tone: SEVERITY_TONE[incident.severity],
    lab: incident.devices.some((device) => device.lab),
    at: instant(incident.started_at),
  };
}

function wentDownItem(down: WentDown): HandoverItem {
  return {
    id: `${down.id}:${down.at}`,
    text: down.name,
    detail: `${down.ongoing ? 'down since' : 'down for'} ${formatDuration(down.down_ms)} · read from ${down.from}`,
    to: `/monitoring/health?device=${encodeURIComponent(down.id)}`,
    tone: down.ongoing ? 'crit' : 'warn',
    lab: down.lab,
    at: instant(down.at),
  };
}

function configItem(change: HandoverConfigChange): HandoverItem {
  const unplanned = change.checked
    ? change.unplanned
      ? 'no change of ours covers it'
      : 'covered by an applied change'
    : 'nothing to check it against';
  const added = change.lines_added;
  const removed = change.lines_removed;
  const size =
    added === undefined && removed === undefined
      ? 'difference not computed'
      : `+${formatCount(added ?? 0)} −${formatCount(removed ?? 0)}`;
  return {
    id: `${change.device_id}:${change.version_id}`,
    text: change.device,
    detail: `${size} · ${change.source} · ${unplanned}`,
    to: `/config?device=${encodeURIComponent(change.device_id)}`,
    tone: change.checked && change.unplanned ? 'warn' : 'idle',
    at: instant(change.at),
  };
}

function jobItem(job: JobRef): HandoverItem {
  return {
    id: job.id,
    text: job.title || job.kind,
    detail: job.error ?? job.message,
    to: `/ops/jobs?job=${encodeURIComponent(job.id)}`,
    tone: 'crit',
    at: instant(job.finished_at ?? job.started_at),
  };
}

function items(list: HandoverItem[], sentence: string): HandoverBody {
  return list.length > 0 ? { kind: 'items', items: list } : { kind: 'nothing', sentence };
}

function unavailable(reading: Reading<unknown> & { present: false }): HandoverBody {
  return { kind: 'unavailable', reason: reading.reason, message: reading.message };
}

/**
 * A generated handover as the screen shows it: sections of items, each item
 * carrying the record it came from.
 *
 * Nothing in here is written by this screen; every string is assembled from
 * fields of the answer. A section the backend could not fill keeps its
 * reason, and a section it filled with nothing says that nothing happened —
 * the two sentences are different because the two facts are.
 */
export function handoverSections(handover: Handover): HandoverSection[] {
  const gaps = handover.unavailable;
  const sections: HandoverSection[] = [];

  // Our own records: never null, so there is no "unavailable" case.
  const { opened, resolved, open } = handover.incidents;
  sections.push({
    id: 'incidents',
    title: 'Incidents',
    summary: `${formatCount(opened.length)} opened, ${formatCount(resolved.length)} resolved, ${formatCount(open.length)} still open`,
    note: 'Opened in the period, resolved in it, or open at the end of it. An incident can appear in more than one of those.',
    body: items(
      [
        ...open.map((incident) => incidentItem(incident)),
        ...opened
          .filter((incident) => !open.some((other) => other.id === incident.id))
          .map((incident) => incidentItem(incident)),
        ...resolved
          .filter(
            (incident) =>
              !open.some((other) => other.id === incident.id) &&
              !opened.some((other) => other.id === incident.id),
          )
          .map((incident) => incidentItem(incident)),
      ],
      'No incident was opened, resolved or left open in this period.',
    ),
  });

  const alerts = readSection(handover.alerts, gaps, 'alerts');
  sections.push({
    id: 'alerts',
    title: 'Alerts',
    summary: alerts.present
      ? `${floorText(alerts.value.fired, alerts.value.truncated)} fired, ${formatCount(alerts.value.resolved)} resolved, ${formatCount(alerts.value.still_open.length)} still firing`
      : undefined,
    note: alerts.present
      ? 'The list is the alerts that are still open now, whenever they fired.'
      : undefined,
    body: alerts.present
      ? items(
          alerts.value.still_open.map(alertItem),
          `${floorText(alerts.value.fired, alerts.value.truncated)} fired in this period and none is still open.`,
        )
      : unavailable(alerts),
  });

  const changes = readSection(handover.changes, gaps, 'changes');
  sections.push({
    id: 'changes',
    title: 'Changes',
    summary: changes.present
      ? `${formatCount(changes.value.applied.length)} applied, ${formatCount(changes.value.failed_or_rolled_back.length)} failed or rolled back, ${formatCount(changes.value.awaiting_approval.length)} awaiting approval`
      : undefined,
    note: changes.present
      ? 'Applied and failed are from this period; awaiting approval is what is waiting now, whenever it was planned — that is what the next shift inherits.'
      : undefined,
    body: changes.present
      ? items(
          [
            ...changes.value.failed_or_rolled_back.map((change) => changeItem(change, 'crit')),
            ...changes.value.awaiting_approval.map((change) => changeItem(change, 'warn')),
            ...changes.value.applied.map((change) => changeItem(change, 'ok')),
          ],
          'Nothing was applied, nothing failed, and nothing is waiting for approval.',
        )
      : unavailable(changes),
  });

  const configs = readSection(handover.config_changes, gaps, 'config_changes');
  const unplanned = configs.present
    ? configs.value.filter((change) => change.checked && change.unplanned).length
    : 0;
  sections.push({
    id: 'config_changes',
    title: 'Configuration movement',
    summary: configs.present
      ? `${plural(configs.value.length, 'device configuration')} changed, ${formatCount(unplanned)} of them unplanned`
      : undefined,
    note: configs.present
      ? 'Read from the stored snapshots themselves. "Unplanned" means no applied change of ours covers that device at that time — somebody was on the console.'
      : undefined,
    body: configs.present
      ? items(
          configs.value.map(configItem),
          'No stored configuration changed in this period.',
        )
      : unavailable(configs),
  });

  const devices = handover.devices;
  const measurement: string[] = [];
  if (devices.without_reachability_samples > 0) {
    measurement.push(
      `there are no reachability samples at all for ${plural(devices.without_reachability_samples, 'device')}, so whether those went down is unknown rather than no`,
    );
  }
  if (devices.not_measured > 0) {
    measurement.push(`${formatCount(devices.not_measured)} past the measurement limit were not looked at`);
  }
  sections.push({
    id: 'devices',
    title: 'Devices',
    summary: `${formatCount(devices.added.length)} added, ${formatCount(devices.removed.length)} removed, ${formatCount(devices.went_down.length)} went down, ${formatCount(devices.unreachable_now.length)} unreachable now`,
    note: measurement.length > 0 ? `${measurement.join('; ')}.` : undefined,
    body: items(
      [
        ...devices.went_down.map(wentDownItem),
        ...devices.unreachable_now.map((device) =>
          deviceItem(device, {
            id: `now:${device.id}`,
            detail: [device.site || undefined, 'not answering now'].filter(Boolean).join(' · '),
            tone: 'crit',
          }),
        ),
        ...devices.added.map((device) =>
          deviceItem(device, {
            id: `added:${device.id}`,
            detail: [device.site || undefined, 'added in this period'].filter(Boolean).join(' · '),
            tone: 'info',
          }),
        ),
        ...devices.removed.map((device) => ({
          id: `removed:${device.device_id ?? device.name}:${device.at}`,
          text: device.name,
          // The device is gone, so the audit trail is the only thing left to
          // check this against — and it is what the name came from.
          detail: `removed · recovered from the ${device.from}`,
          to: device.device_id
            ? `/ops/audit?device=${encodeURIComponent(device.device_id)}`
            : '/ops/audit?action=device.delete',
          tone: 'idle' as StatusTone,
          at: instant(device.at),
        })),
      ],
      'No device was added or removed, and none was seen to go down.',
    ),
  });

  const backups = readSection(handover.backups, gaps, 'backups');
  sections.push({
    id: 'backups',
    title: 'Backups that failed',
    note: backups.present
      ? 'Only the configuration feature knows that a backup was attempted and failed; coverage itself is in the backups report.'
      : undefined,
    body: backups.present
      ? items(
          backups.value.failed.map((failure) => ({
            id: failure.device_id,
            text: failure.device,
            detail: failure.error ?? 'the reason was not recorded',
            to: `/config?device=${encodeURIComponent(failure.device_id)}`,
            tone: 'warn' as StatusTone,
            at: instant(failure.last_backup_at),
          })),
          'No backup failed.',
        )
      : unavailable(backups),
  });

  sections.push({
    id: 'jobs_failed',
    title: 'Jobs that failed',
    body: items(handover.jobs_failed.map(jobItem), 'No job failed in this period.'),
  });

  return sections;
}

/** The sections of a handover that have items, for "is there anything to hand over?". */
export function handoverHasContent(sections: readonly HandoverSection[]): boolean {
  return sections.some((section) => section.body.kind === 'items');
}

// -------------------------------------------------------------------- periods

export interface PeriodChoice {
  id: string;
  label: string;
  /** What it means, since "my shift" is not a thing the backend knows. */
  hint: string;
  since: (now: number) => number;
}

const HOUR = 3_600_000;

/**
 * The periods a handover or a report is usually asked for. None of them is a
 * shift pattern Nocturne pretends to know: each says plainly how far back it
 * reaches, and the exact period is always on screen.
 */
export const PERIODS: readonly PeriodChoice[] = [
  { id: '8h', label: 'Last 8 hours', hint: 'A shift of eight hours ending now.', since: (now) => now - 8 * HOUR },
  { id: '12h', label: 'Last 12 hours', hint: 'A shift of twelve hours ending now.', since: (now) => now - 12 * HOUR },
  { id: '24h', label: 'Last 24 hours', hint: 'A full day ending now.', since: (now) => now - 24 * HOUR },
  {
    id: 'midnight',
    label: 'Since midnight',
    hint: 'From local midnight today until now.',
    since: (now) => {
      const date = new Date(now);
      date.setHours(0, 0, 0, 0);
      return date.getTime();
    },
  },
  { id: '7d', label: 'Last 7 days', hint: 'A week ending now.', since: (now) => now - 7 * 24 * HOUR },
  { id: '30d', label: 'Last 30 days', hint: 'Thirty days ending now.', since: (now) => now - 30 * 24 * HOUR },
];

export function periodChoice(id: string): PeriodChoice {
  return PERIODS.find((period) => period.id === id) ?? (PERIODS[0] as PeriodChoice);
}

// --------------------------------------------------------------------- reports

/**
 * What a report covers, in sentences — including the ones that say what it
 * does not cover.
 *
 * A report with no rows and nothing missing is a measured nothing. A report
 * with no rows *and* a section that could not be read is not: those two must
 * never produce the same line, which is the whole reason this is a function
 * with a test and not a ternary in a component.
 */
export function reportCoverage(report: GeneratedReport): string[] {
  const lines: string[] = [];
  lines.push(
    report.period === null
      ? 'This report describes the present, not a period: every figure in it is as things are now.'
      : 'This report covers the period in its header, and nothing outside it.',
  );
  if (report.unavailable.length > 0) {
    lines.push(
      `${plural(report.unavailable.length, 'section')} of it could not be read, so what is missing from the figures below is not necessarily zero.`,
    );
  }
  if (report.rows === 0) {
    lines.push(
      report.unavailable.length > 0
        ? 'There are no rows at all, and because of the above this cannot be read as "nothing happened".'
        : 'There are no rows: everything it looked at was read, and there was nothing to report.',
    );
  } else {
    lines.push(`${plural(report.rows, 'row')} of data.`);
  }
  return lines;
}

// ----------------------------------------------------------------------- audit

/** The groups the action filter offers, so a long trail can be narrowed fast. */
export function actionGroups(
  actions: readonly { action: string; count: number }[],
): { group: string; count: number; actions: { action: string; count: number }[] }[] {
  const groups = new Map<string, { group: string; count: number; actions: { action: string; count: number }[] }>();
  for (const entry of actions) {
    const group = entry.action.includes('.') ? `${entry.action.split('.')[0] ?? ''}.` : entry.action;
    const found = groups.get(group) ?? { group, count: 0, actions: [] };
    found.count += entry.count;
    found.actions.push(entry);
    groups.set(group, found);
  }
  return [...groups.values()].sort((a, b) => a.group.localeCompare(b.group));
}

/**
 * The kinds of record this view can look up a title for (they are the
 * operations feature's own). For any other kind `resolved: false` means only
 * "this view cannot read that feature's records", not "the record is gone",
 * and the screen must not imply the second.
 */
export const RESOLVABLE_KINDS: ReadonlySet<string> = new Set([
  'incident',
  'note',
  'handover',
  'report',
]);

/** The tone of an audit action: destructive actions are not quiet. */
export function actionTone(action: string): StatusTone {
  if (/\.(delete|clear|forget|remove|lock)$/.test(action)) return 'warn';
  if (/\.(apply|rollback|restore|fix|remediate)$/.test(action)) return 'crit';
  return 'idle';
}

// ------------------------------------------------------------------------ jobs

export function jobTone(state: JobRef['state']): StatusTone {
  switch (state) {
    case 'running':
      return 'info';
    case 'succeeded':
      return 'ok';
    case 'failed':
      return 'crit';
    default:
      return 'idle';
  }
}

/** A job's progress as a percentage, or `null` while the work is unknown. */
export function jobProgress(job: { done: number; total: number }): number | null {
  if (job.total <= 0) return null;
  return Math.min(100, Math.round((job.done / job.total) * 100));
}
