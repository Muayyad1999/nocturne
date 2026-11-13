/**
 * The Overview's reasoning, with no React in it.
 *
 * Every number on this screen comes from one `overview.get` answer, and the
 * whole screen turns on one distinction the backend's author put at the top of
 * `api/types/ops.ts`: a part that is `null` is **not** a zero. A `null` part
 * means the feature behind it is not in this build, did not answer, answered
 * with something unreadable, or has nothing recorded yet — and the reason is in
 * the sibling `unavailable` list. A part that is present and `0` is a measured
 * zero, which is good news and must read as good news.
 *
 * So nothing here ever reaches into a part without going through
 * {@link readPart}, and the two outcomes produce different shapes
 * ({@link TileReading}), not the same shape with a different number. A missing
 * part cannot be formatted as "0" because it never becomes a number at all.
 *
 * The second rule that lives here: a count with `truncated: true` is a floor,
 * not a total, and says so ({@link floorText}).
 */

import type {
  ActivityLine,
  AlertRef,
  OverviewAlerts,
  ChangeRef,
  InsightRef,
  IncidentHead,
  JobRef,
  Overview,
  UnavailableReason,
} from '@/api/types/ops';
import type { DeviceRow } from '@/api/types/core';
import type { StatusTone } from '@/design/tone';
import { EMPTY, formatCount, formatPercent, plural } from '@/lib/format';

// --------------------------------------------------------------- the parts

/** The parts of `overview.get` that can be `null`. `devices` and `incidents` cannot. */
export const NULLABLE_PARTS = [
  'alerts',
  'changes',
  'backups',
  'compliance',
  'insights',
  'jobs',
] as const;

export type PartName = (typeof NULLABLE_PARTS)[number];

/**
 * Either the data, or why there is none. Deliberately two different shapes: a
 * caller cannot accidentally read `.value` off a part that was not answered.
 */
export type Reading<T> =
  | { present: true; value: T }
  | { present: false; reason: UnavailableReason; message: string };

/**
 * One part of the overview, with the reason beside it when it is `null`.
 *
 * A `null` part with no entry in `unavailable` should not happen — the backend
 * writes one for every part it blanks — but if it ever did, inventing a zero
 * would be the one unforgivable answer, so it reports a failure instead.
 */
export function readPart<K extends PartName>(
  overview: Overview,
  section: K,
): Reading<NonNullable<Overview[K]>> {
  const value = overview[section];
  if (value !== null && value !== undefined) {
    return { present: true, value: value as NonNullable<Overview[K]> };
  }
  const why = overview.unavailable.find((entry) => entry.section === section);
  if (why) return { present: false, reason: why.reason, message: why.message };
  return {
    present: false,
    reason: 'failed',
    // Not a zero, and not a dash: the screen says the part is missing and that
    // the backend did not say why.
    message: `\`${section}\` was not reported, and no reason was given for it.`,
  };
}

/**
 * Where the trouble is, from the per-site counts. Only the sites that have
 * something wrong are named — a wallboard for a hundred sites must not become
 * a list of a hundred sites — and the rest are counted.
 */
export function sitesLine(devices: Overview['devices']): string | null {
  if (devices.by_site.length <= 1) return null;
  const troubled = devices.by_site
    .filter((site) => site.down > 0)
    .sort((a, b) => b.down - a.down);
  // Nothing down is not a claim this line should make: with monitoring off,
  // or on a device that has never been polled, "no site is down" would read
  // as "everything is fine". The tile says up, down and never-polled; this
  // line says only where the trouble is, and says nothing when there is none.
  if (troubled.length === 0) return null;
  const named = troubled
    .slice(0, 4)
    .map((site) => `${site.site || 'no site'} (${formatCount(site.down)})`)
    .join(', ');
  const rest = troubled.length - Math.min(4, troubled.length);
  return `Down in ${named}${rest > 0 ? ` and ${formatCount(rest)} more` : ''}`;
}

/** The headline for an unavailable part: five reasons, five different sentences. */
export const REASON_HEADLINE: Record<UnavailableReason, string> = {
  not_installed: 'Not in this build',
  failed: 'Could not be read',
  timeout: 'Did not answer in time',
  malformed: 'Answer not understood',
  no_data: 'Nothing recorded yet',
};

/**
 * How loud an unavailable part is. A feature that was left out of the build is
 * a fact about the build, not a fault; a feature that failed, timed out or
 * answered nonsense is something to look into.
 */
export const REASON_TONE: Record<UnavailableReason, StatusTone> = {
  not_installed: 'idle',
  failed: 'warn',
  timeout: 'warn',
  malformed: 'warn',
  no_data: 'idle',
};

/** A count that was read from a capped list is a floor: `at least 500`. */
export function floorText(count: number, truncated: boolean): string {
  return truncated ? `at least ${formatCount(count)}` : formatCount(count);
}

// ---------------------------------------------------------------- the tiles

export type TileId = 'devices' | 'alerts' | 'changes' | 'backups' | 'compliance';

/** A line under a tile's figure: what the number is made of. */
export interface TileNote {
  text: string;
  tone?: StatusTone;
  /** Rendered as a live relative time after `text`. */
  at?: number;
}

export type TileReading =
  | {
      kind: 'value';
      /** Already formatted; never a zero standing in for missing data. */
      value: string;
      unit?: string;
      /** The number is a floor read from a capped list. */
      floor: boolean;
      /** Where the number came from, when the backend says. Shown on hover. */
      provenance?: string;
      notes: TileNote[];
    }
  | { kind: 'unavailable'; reason: UnavailableReason; message: string };

export interface TileView {
  id: TileId;
  label: string;
  /** The screen that acts on this number. */
  to: string;
  /** What the link does, for its accessible name. */
  linkLabel: string;
  reading: TileReading;
}

/** The tile a part that could not be filled gets: a reason, and no figure. */
function noFigure(reading: Reading<unknown> & { present: false }): TileReading {
  return { kind: 'unavailable', reason: reading.reason, message: reading.message };
}

/** Devices up, out of the ones in the inventory. Never unavailable. */
function devicesTile(overview: Overview): TileView {
  const { total, up, down, unknown, lab } = overview.devices;
  const notes: TileNote[] = [];
  // Down is the only number on this tile that is ever loud, and it is loud
  // only when it is not zero.
  notes.push(
    down > 0
      ? { text: plural(down, 'device') + ' down', tone: 'crit' }
      : { text: 'None down', tone: 'ok' },
  );
  // "Never polled" is not "up": it is the third state, and it keeps its own
  // words wherever it appears.
  if (unknown > 0) notes.push({ text: `${formatCount(unknown)} never polled`, tone: 'idle' });
  if (lab > 0) notes.push({ text: `${formatCount(lab)} simulated`, tone: 'maint' });
  return {
    id: 'devices',
    label: 'Devices up',
    to: '/devices',
    linkLabel: 'Open Devices',
    reading: {
      kind: 'value',
      value: formatCount(up),
      unit: `of ${formatCount(total)}`,
      floor: false,
      notes,
    },
  };
}

const SEVERITY_ORDER = ['critical', 'high', 'medium', 'low', 'info'] as const;

export type Severity = (typeof SEVERITY_ORDER)[number];

export const SEVERITY_TONE: Record<Severity, StatusTone> = {
  critical: 'crit',
  high: 'crit',
  medium: 'warn',
  low: 'info',
  info: 'idle',
};

export const SEVERITY_WORD: Record<Severity, string> = {
  critical: 'Critical',
  high: 'High',
  medium: 'Medium',
  low: 'Low',
  info: 'Info',
};

export function severityRank(severity: string): number {
  const found = SEVERITY_ORDER.indexOf(severity as Severity);
  return found === -1 ? SEVERITY_ORDER.length : found;
}

/** The worst severity with at least one alert, or `null` when none is firing. */
export function worstSeverity(bySeverity: Record<string, number>): Severity | null {
  for (const severity of SEVERITY_ORDER) {
    if ((bySeverity[severity] ?? 0) > 0) return severity;
  }
  return null;
}

function alertsTile(overview: Overview): TileView {
  const part = readPart(overview, 'alerts');
  const base = {
    id: 'alerts' as const,
    label: 'Open alerts',
    to: '/monitoring/alerts',
    linkLabel: 'Open Monitoring',
  };
  if (!part.present) return { ...base, reading: noFigure(part) };

  const alerts = part.value;
  const worst = worstSeverity(alerts.by_severity);
  const notes: TileNote[] = [];
  if (alerts.open === 0) {
    // A measured zero. "Nothing is firing" is the whole point of the screen
    // and must not look like "there is no alerting here".
    notes.push({ text: 'Nothing firing', tone: 'ok' });
  } else if (worst) {
    notes.push({
      // When the list was capped, the severities were counted from the part
      // of it that was read: the worst alert of all may not be in it, and the
      // tile may not claim otherwise.
      text: alerts.truncated
        ? `Worst seen: ${SEVERITY_WORD[worst].toLowerCase()} (${floorText(alerts.by_severity[worst] ?? 0, true)})`
        : `Worst: ${SEVERITY_WORD[worst].toLowerCase()} (${formatCount(alerts.by_severity[worst] ?? 0)})`,
      tone: SEVERITY_TONE[worst],
    });
  }
  if (alerts.truncated) {
    notes.push({ text: 'Counted from a capped list; there may be worse', tone: 'warn' });
  }
  return {
    ...base,
    reading: {
      kind: 'value',
      value: formatCount(alerts.open),
      floor: alerts.truncated,
      // Which method the backend counted with, for anyone comparing screens.
      provenance: `Counted from \`${alerts.from}\``,
      notes,
    },
  };
}

function changesTile(overview: Overview): TileView {
  const part = readPart(overview, 'changes');
  const base = {
    id: 'changes' as const,
    label: 'Awaiting approval',
    to: '/changes',
    linkLabel: 'Open Changes',
  };
  if (!part.present) return { ...base, reading: noFigure(part) };

  const changes = part.value;
  const notes: TileNote[] = [];
  if (changes.awaiting_approval === 0) notes.push({ text: 'Nothing waiting', tone: 'ok' });
  if (changes.applying > 0)
    notes.push({ text: `${formatCount(changes.applying)} applying now`, tone: 'info' });
  notes.push({
    text: `${formatCount(changes.applied_24h)} applied, ${formatCount(changes.failed_24h)} failed in 24 h`,
    tone: changes.failed_24h > 0 ? 'warn' : undefined,
  });
  return {
    ...base,
    reading: {
      kind: 'value',
      value: formatCount(changes.awaiting_approval),
      floor: changes.truncated,
      notes,
    },
  };
}

/**
 * Backup coverage as a percentage, or `null` when there is nothing to cover.
 * `0 / 0` is not 0 %: an estate with no devices has no coverage to report.
 */
export function coveragePct(withBackup: number, without: number): number | null {
  const total = withBackup + without;
  return total === 0 ? null : (withBackup / total) * 100;
}

function backupsTile(overview: Overview): TileView {
  const part = readPart(overview, 'backups');
  const base = {
    id: 'backups' as const,
    label: 'Backup coverage',
    to: '/config/backups',
    linkLabel: 'Open Configuration backups',
  };
  if (!part.present) return { ...base, reading: noFigure(part) };

  const backups = part.value;
  const pct = coveragePct(backups.devices_with_backup, backups.devices_without);
  const notes: TileNote[] = [
    backups.devices_without > 0
      ? { text: `${plural(backups.devices_without, 'device')} never backed up`, tone: 'warn' }
      : { text: 'Every device has a backup', tone: 'ok' },
  ];
  if (backups.oldest_backup_at === null) {
    notes.push({ text: 'No configuration has ever been stored', tone: 'idle' });
  } else {
    notes.push({ text: 'Oldest backup', at: backups.oldest_backup_at });
  }
  // Only when something moved: a zero here is not news, and the tile has
  // three lines to spend.
  if (backups.changed_24h > 0) {
    notes.push({ text: `${plural(backups.changed_24h, 'device')} changed in 24 h`, tone: 'info' });
  }
  return {
    ...base,
    reading: {
      kind: 'value',
      // A percentage of nothing would be a fabrication; the note says what is there.
      value: pct === null ? EMPTY : formatPercent(pct, { digits: 0 }),
      unit: pct === null ? 'no devices to cover' : `${formatCount(backups.devices_with_backup)} of ${formatCount(backups.devices_with_backup + backups.devices_without)}`,
      floor: false,
      notes,
    },
  };
}

function complianceTile(overview: Overview): TileView {
  const part = readPart(overview, 'compliance');
  const base = {
    id: 'compliance' as const,
    label: 'Compliance score',
    to: '/config/compliance',
    linkLabel: 'Open Compliance',
  };
  if (!part.present) return { ...base, reading: noFigure(part) };

  const compliance = part.value;
  const notes: TileNote[] = [];
  // What is failing, and what to do about it, is a row in the list below;
  // the tile carries the score and its age, so neither says the other twice.
  if (compliance.failing_devices === 0 && compliance.failing_findings === 0) {
    notes.push({ text: 'Nothing failing', tone: 'ok' });
  }
  if (compliance.run_at === null) {
    // The part is here, so a run happened; the feature just did not say when.
    notes.push({ text: 'The feature did not say when it ran', tone: 'idle' });
  } else {
    notes.push({ text: 'Ran ', at: compliance.run_at });
  }
  return {
    ...base,
    reading: {
      kind: 'value',
      // A present part with no score is a third case again: the run happened,
      // the score did not come with it.
      value: compliance.score_pct === null ? 'No score' : formatPercent(compliance.score_pct, { digits: 0 }),
      floor: false,
      notes,
    },
  };
}

/** The five tiles of the top row, in order, each a reading or a reason. */
export function tiles(overview: Overview): TileView[] {
  return [
    devicesTile(overview),
    alertsTile(overview),
    changesTile(overview),
    backupsTile(overview),
    complianceTile(overview),
  ];
}

// ------------------------------------------------------------ what needs you

export type AttentionKind =
  | 'incident'
  | 'device'
  | 'alert'
  | 'change'
  | 'compliance'
  | 'insight'
  | 'more';

/** One row of the merged, ranked "Needs attention" list. */
export interface AttentionRow {
  /** Stable across refetches, so React keeps the row and its focus. */
  id: string;
  kind: AttentionKind;
  /** The word for the kind, shown as a pill beside the row. */
  word: string;
  tone: StatusTone;
  /** What it is. */
  title: string;
  /** Where it is: a device, a site, or the scope of the number. */
  where?: string;
  /** When it started, for a live relative time. */
  since?: number;
  /** A simulated device: never mistaken for production. */
  lab?: boolean;
  /** The one obvious thing to do about it. */
  action?: { label: string; to: string };
  /** An open alert this row can acknowledge without leaving the screen. */
  ackAlertId?: string;
}

/** Rank by kind: an incident outranks a dead device, which outranks an alert. */
const KIND_RANK: Record<AttentionKind, number> = {
  incident: 0,
  device: 1,
  alert: 2,
  change: 3,
  compliance: 4,
  insight: 5,
  more: 6,
};

const INCIDENT_SEVERITY_TONE: Record<string, StatusTone> = {
  sev1: 'crit',
  sev2: 'crit',
  sev3: 'warn',
  sev4: 'info',
};

const FAILED_CHANGE_STATES = new Set(['failed', 'partially_applied', 'rolled_back']);

const CHANGE_STATE_WORD: Record<string, string> = {
  failed: 'Change failed',
  partially_applied: 'Change half-applied',
  rolled_back: 'Change rolled back',
};

function incidentRows(incidents: IncidentHead[]): AttentionRow[] {
  return incidents.map((incident) => ({
    id: `incident:${incident.id}`,
    kind: 'incident' as const,
    word: incident.severity.toUpperCase(),
    tone: INCIDENT_SEVERITY_TONE[incident.severity] ?? 'warn',
    title: incident.title,
    where:
      incident.devices.length === 0
        ? (incident.assignee || undefined)
        : incident.devices.map((device) => device.name).join(', '),
    since: incident.started_at,
    lab: incident.devices.some((device) => device.lab),
    action: { label: 'Review', to: '/ops/incidents' },
  }));
}

/**
 * The devices that are not answering, by name. The overview itself carries
 * only counts, so the names come from the inventory; when that read is not
 * there, {@link downSummaryRow} states the count instead of naming nothing.
 */
function downDeviceRows(
  devices: readonly DeviceRow[],
  alreadyAlerted: ReadonlySet<string>,
): AttentionRow[] {
  return devices
    .filter((device) => device.state.reachable === false)
    // A device that is down and has an alert saying so is one problem, not
    // two: the alert row carries the same fact plus its own words, its time
    // and an Acknowledge, so the device row would only repeat it. A device
    // that is down with nothing firing about it is listed here — which is
    // also how "monitoring never looked" shows up on this list.
    .filter((device) => !alreadyAlerted.has(device.id))
    .sort((a, b) => (b.state.last_seen ?? 0) - (a.state.last_seen ?? 0))
    .map((device) => ({
      id: `device:${device.id}`,
      kind: 'device' as const,
      word: 'Down',
      tone: 'crit' as const,
      title: `${device.name} is not answering`,
      // Where it is, and — when the device has never once answered — that
      // fact, because a row with no time at all reads as a missing time.
      where: [device.site || device.address, device.state.last_seen == null ? 'never seen' : null]
        .filter(Boolean)
        .join(' · '),
      // When it was last seen is when the trouble started.
      since: device.state.last_seen ?? undefined,
      lab: Boolean(device.lab),
      action: { label: 'Open', to: `/devices/${encodeURIComponent(device.id)}` },
    }));
}

function downSummaryRow(down: number): AttentionRow | null {
  if (down === 0) return null;
  return {
    id: 'device:count',
    kind: 'device',
    word: 'Down',
    tone: 'crit',
    title: `${plural(down, 'device')} not answering`,
    where: 'the inventory could not be read, so they cannot be named here',
    action: { label: 'Open', to: '/devices' },
  };
}

function alertRows(alerts: AlertRef[]): AttentionRow[] {
  return alerts
    .filter((alert) => alert.severity === 'critical' || alert.severity === 'high')
    .sort((a, b) => severityRank(a.severity) - severityRank(b.severity) || b.started_at - a.started_at)
    .map((alert) => ({
      id: `alert:${alert.id}`,
      kind: 'alert' as const,
      word: SEVERITY_WORD[alert.severity as Severity] ?? alert.severity,
      tone: SEVERITY_TONE[alert.severity as Severity] ?? 'warn',
      title: alert.title,
      where: alert.device ?? alert.rule,
      since: alert.started_at,
      lab: alert.lab,
      action: {
        label: 'Open',
        to: alert.device_id
          ? `/monitoring/health?device=${encodeURIComponent(alert.device_id)}`
          : '/monitoring/alerts',
      },
      // Already acknowledged alerts keep their row but lose the button.
      ackAlertId: alert.state === 'firing' ? alert.id : undefined,
    }));
}

/**
 * `newest` holds at most five alerts, newest first — not the worst five — and
 * only the loud ones become rows, so most of what is firing can be missing
 * from this list. One row says how much, rather than letting a short list
 * suggest a quiet network.
 */
function moreAlertsRow(alerts: OverviewAlerts, listed: number): AttentionRow | null {
  const rest = alerts.open - listed;
  if (rest <= 0) return null;
  return {
    id: 'alert:more',
    kind: 'more',
    word: 'More',
    tone: 'warn',
    title: `${floorText(rest, alerts.truncated)} further open ${rest === 1 ? 'alert' : 'alerts'}`,
    where: 'below critical and high, or outside the five newest',
    action: { label: 'Open', to: '/monitoring/alerts' },
  };
}

function changeRows(changes: ChangeRef[]): AttentionRow[] {
  return changes
    .filter((change) => FAILED_CHANGE_STATES.has(change.state))
    .map((change) => ({
      id: `change:${change.id}`,
      kind: 'change' as const,
      word: CHANGE_STATE_WORD[change.state] ?? change.state,
      tone: change.state === 'rolled_back' ? ('warn' as const) : ('crit' as const),
      title: change.title,
      where: change.ticket ?? change.created_by,
      since: change.updated_at,
      action: { label: 'Review', to: `/changes/${change.id}` },
    }));
}

function insightRows(insights: InsightRef[]): AttentionRow[] {
  return insights.map((insight) => ({
    id: `insight:${insight.id}`,
    kind: 'insight' as const,
    word: SEVERITY_WORD[insight.severity as Severity] ?? insight.severity,
    tone: SEVERITY_TONE[insight.severity as Severity] ?? 'info',
    title: insight.title,
    where: [insight.device, insight.subject].filter(Boolean).join(' · ') || insight.check,
    since: insight.first_seen,
    lab: insight.lab,
    action: insight.device_id
      ? { label: 'Open', to: `/devices/${encodeURIComponent(insight.device_id)}` }
      : undefined,
  }));
}

export interface AttentionInput {
  overview: Overview;
  /** The inventory, when it could be read: for the names of the dead devices. */
  devices: readonly DeviceRow[] | undefined;
}

/**
 * Everything that wants a person, merged and ranked: incidents, then dead
 * devices, then the loud alerts, failed changes, compliance, missing backups
 * and finally the insights.
 *
 * Nothing is counted twice and nothing comes from anywhere but the one
 * `overview.get` answer (plus the inventory, for names) — so a row can never
 * disagree with the tile above it.
 */
export function needsAttention({ overview, devices }: AttentionInput): AttentionRow[] {
  const rows: AttentionRow[] = [];

  rows.push(...incidentRows(overview.incidents.newest));

  const alerts = readPart(overview, 'alerts');
  const loud = alerts.present ? alertRows(alerts.value.newest) : [];
  const alerted = new Set(
    alerts.present
      ? alerts.value.newest
          .filter((alert) => alert.severity === 'critical' || alert.severity === 'high')
          .map((alert) => alert.device_id)
          .filter((id): id is string => id !== undefined)
      : [],
  );

  if (devices) rows.push(...downDeviceRows(devices, alerted));
  else {
    const summary = downSummaryRow(overview.devices.down);
    if (summary) rows.push(summary);
  }

  rows.push(...loud);
  if (alerts.present) {
    const more = moreAlertsRow(alerts.value, loud.length);
    if (more) rows.push(more);
  }

  const changes = readPart(overview, 'changes');
  if (changes.present) rows.push(...changeRows(changes.value.recent));

  const compliance = readPart(overview, 'compliance');
  if (compliance.present && compliance.value.failing_devices > 0) {
    rows.push({
      id: 'compliance:failing',
      kind: 'compliance',
      word: 'Drift',
      tone: 'warn',
      title: `${plural(compliance.value.failing_devices, 'device')} ${compliance.value.failing_devices === 1 ? 'fails' : 'fail'} a compliance check`,
      where: `${floorText(compliance.value.failing_findings, compliance.value.truncated)} failing ${compliance.value.failing_findings === 1 ? 'finding' : 'findings'} in the last run`,
      since: compliance.value.run_at ?? undefined,
      action: { label: 'Review', to: '/config/compliance' },
    });
  }

  const insights = readPart(overview, 'insights');
  if (insights.present) rows.push(...insightRows(insights.value.top));

  return rows.sort(
    (a, b) => KIND_RANK[a.kind] - KIND_RANK[b.kind] || (b.since ?? 0) - (a.since ?? 0),
  );
}

/** How many rows of the list are shown before it is cut off with a count. */
export const ATTENTION_LIMIT = 12;

// ------------------------------------------------------------------- the feed

export const ACTIVITY_WORD: Record<string, string> = {
  audit: 'Action',
  alert: 'Alert',
  change: 'Change',
};

export const ACTIVITY_TONE: Record<string, StatusTone> = {
  audit: 'idle',
  alert: 'warn',
  change: 'info',
};

/** Where an activity line's reference goes, or `null` when it goes nowhere. */
export function activityLink(line: ActivityLine): string | null {
  const reference = line.ref;
  if (!reference) return null;
  if (reference.type === 'change') return `/changes/${reference.id}`;
  if (reference.type === 'incident') return '/ops/incidents';
  if (reference.type === 'alert') {
    return line.device_id
      ? `/monitoring/health?device=${encodeURIComponent(line.device_id)}`
      : '/monitoring/alerts';
  }
  // An audit entry has a screen of its own (`/ops/audit`) but no way to point
  // at one row of it, so the feed links to the device when it names one.
  if (reference.type === 'audit')
    return line.device_id ? `/devices/${encodeURIComponent(line.device_id)}` : null;
  return null;
}

// -------------------------------------------------------------------- jobs

/** What a running job's progress bar should say, when the total is known. */
export function jobProgress(job: JobRef): { value?: number; max: number; text: string } {
  if (job.total <= 0) {
    // `0` means the amount of work is not known yet — not "none done".
    return { max: 1, text: `${formatCount(job.done)} done, total unknown` };
  }
  return {
    value: job.done,
    max: job.total,
    text: `${formatCount(job.done)} of ${formatCount(job.total)}`,
  };
}

// --------------------------------------------------------------- freshness

/**
 * How old the reading on screen is allowed to get before the screen says so.
 * Two missed refetches: by then the engine, the event stream or the browser's
 * timers have stopped, and a wallboard showing a frozen picture is the one way
 * this screen can really mislead.
 */
export const STALE_AFTER_MS = 150_000;

export type Freshness = 'fresh' | 'stale';

export function freshness(generatedAt: number, now: number): Freshness {
  return now - generatedAt > STALE_AFTER_MS ? 'stale' : 'fresh';
}
