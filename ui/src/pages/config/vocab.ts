/**
 * Words, tones and the small pure decisions the Configuration screen makes
 * from what the backend returns. No React and no RPC in here, so the rules
 * that decide what a row *says* — never captured, captured and failed since,
 * skipped on purpose — can be unit tested on their own.
 */

import type {
  ConfigKind,
  ConfigVersion,
  Coverage,
  DiffLine,
  FindingStatus,
  Severity,
} from '@/api/types/config';
import type { Job } from '@/api/types/core';
import type { StatusTone } from '@/design/tone';

// --------------------------------------------------------------------- words

export const KIND_LABEL: Record<ConfigKind, string> = {
  running: 'Running configuration',
  startup: 'Startup configuration',
};

export const KIND_SHORT: Record<ConfigKind, string> = {
  running: 'running',
  startup: 'startup',
};

/**
 * How a stored version came to be stored. The backend writes these strings;
 * anything it adds later falls through to the string itself rather than being
 * swallowed.
 */
const ORIGIN_SENTENCE: Record<string, string> = {
  manual: 'asked for by hand',
  scheduled: 'taken by the backup schedule',
  assistant: 'asked for by the assistant',
  event: 'taken because the device reported its configuration had changed',
  'pre-change': 'taken by the change pipeline before a change was applied',
  'post-change': 'taken by the change pipeline after a change was applied',
};

const ORIGIN_LABEL: Record<string, string> = {
  manual: 'By hand',
  scheduled: 'Scheduled',
  assistant: 'Assistant',
  event: 'Device said so',
  'pre-change': 'Before a change',
  'post-change': 'After a change',
};

/** A short label for a version's origin, for a timeline row or a chip. */
export function originLabel(origin: string): string {
  return ORIGIN_LABEL[origin] ?? origin;
}

/** The same thing as a sentence: "…, taken by the backup schedule." */
export function originSentence(origin: string): string {
  return ORIGIN_SENTENCE[origin] ?? `stored with origin \`${origin}\``;
}

/** The assistant's own captures are tinted, like everything else it did. */
export function originTone(origin: string): StatusTone | 'ai' | 'neutral' {
  if (origin === 'assistant') return 'ai';
  if (origin === 'pre-change' || origin === 'post-change') return 'info';
  return 'neutral';
}

// ------------------------------------------------------------ coverage state

/**
 * What a device's backup coverage amounts to. These are four different
 * things and the screen must never blur them:
 *
 * - `never`    nothing has ever been captured. Not an empty configuration.
 * - `captured` there is a stored configuration.
 * - `failed`   nothing is stored and the last attempt failed, with a reason.
 * - `skipped`  the device is deliberately not backed up (monitor-only, or its
 *              device type defines no configuration command).
 */
export type CaptureState = 'never' | 'captured' | 'failed' | 'skipped';

export function captureState(row: Coverage): CaptureState {
  if (row.running || row.startup) return 'captured';
  if (row.skipped) return 'skipped';
  if (row.last_error) return 'failed';
  return 'never';
}

export const CAPTURE_WORD: Record<CaptureState, string> = {
  never: 'Never captured',
  captured: 'Captured',
  failed: 'Could not be read',
  skipped: 'Not backed up',
};

export const CAPTURE_TONE: Record<CaptureState, StatusTone> = {
  never: 'idle',
  captured: 'ok',
  failed: 'crit',
  skipped: 'idle',
};

/** One honest sentence about a device's coverage, for a row's detail line. */
export function captureSentence(row: Coverage): string {
  switch (captureState(row)) {
    case 'never':
      return 'Nothing has ever been captured from this device. That is not the same as an empty configuration — nobody has looked yet.';
    case 'failed':
      return row.last_error ?? 'The last attempt to read this device failed.';
    case 'skipped':
      return row.skipped ?? 'This device is not backed up.';
    case 'captured':
      return row.last_error
        ? `There is a stored configuration, but the most recent attempt failed: ${row.last_error}`
        : 'A configuration is stored for this device.';
  }
}

/** A capture that really is empty: the device answered, with nothing in it. */
export function isEmptyCapture(version: ConfigVersion | undefined): boolean {
  return version !== undefined && version.lines === 0;
}

// ---------------------------------------------------------------- compliance

export const SEVERITY_LABEL: Record<Severity, string> = {
  critical: 'Critical',
  high: 'High',
  medium: 'Medium',
  low: 'Low',
  info: 'Info',
};

/** Worst first: the order findings and severity filters are listed in. */
export const SEVERITY_ORDER: readonly Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

export function severityTone(severity: Severity): StatusTone {
  switch (severity) {
    case 'critical':
      return 'crit';
    case 'high':
      return 'crit';
    case 'medium':
      return 'warn';
    case 'low':
      return 'warn';
    case 'info':
      return 'info';
  }
}

export const FINDING_LABEL: Record<FindingStatus, string> = {
  pass: 'Pass',
  fail: 'Fail',
  not_applicable: 'Not applicable',
  not_evaluated: 'Not evaluated',
};

export function findingTone(status: FindingStatus): StatusTone {
  switch (status) {
    case 'pass':
      return 'ok';
    case 'fail':
      return 'crit';
    case 'not_applicable':
      return 'idle';
    case 'not_evaluated':
      return 'idle';
  }
}

/**
 * Why a finding is not a verdict. `not_evaluated` is the one that matters:
 * it means there was no configuration to judge, which a reader must not
 * mistake for a pass.
 */
export const FINDING_SENTENCE: Record<FindingStatus, string> = {
  pass: 'The configuration satisfies this rule.',
  fail: 'The configuration does not satisfy this rule.',
  not_applicable: 'This rule has nothing to judge on this device.',
  not_evaluated: 'There is no stored configuration for this device, so nothing was judged. This is not a pass.',
};

export function scoreTone(score: number | null): StatusTone {
  if (score === null) return 'idle';
  if (score >= 90) return 'ok';
  if (score >= 70) return 'warn';
  return 'crit';
}

/** What a rule's check looks for, in a sentence a person can read. */
export function checkSentence(check: { type: string } & Record<string, unknown>): string {
  const str = (key: string) => String(check[key] ?? '');
  const list = (key: string) => (Array.isArray(check[key]) ? (check[key] as string[]) : []);
  switch (check.type) {
    case 'present':
      return `The configuration must contain the line \`${str('line')}\`.`;
    case 'absent':
      return `The configuration must not contain the line \`${str('line')}\`.`;
    case 'matches':
      return `Some line must match \`${str('pattern')}\`.`;
    case 'not_matches':
      return `No line may match \`${str('pattern')}\`.`;
    case 'section': {
      const each = check['each'] === true;
      const must = list('must');
      const mustNot = list('must_not');
      const parts: string[] = [];
      if (must.length > 0) parts.push(`have ${must.map((line) => `\`${line}\``).join(', ')}`);
      if (mustNot.length > 0) parts.push(`not have ${mustNot.map((line) => `\`${line}\``).join(', ')}`);
      return `${each ? 'Every section' : 'At least one section'} whose header matches \`${str('parent')}\` must ${
        parts.join(' and ') || 'exist'
      }.`;
    }
    case 'section_when': {
      const must = list('must');
      const mustNot = list('must_not');
      const parts: string[] = [];
      if (must.length > 0) parts.push(`have ${must.map((line) => `\`${line}\``).join(', ')}`);
      if (mustNot.length > 0) parts.push(`not have ${mustNot.map((line) => `\`${line}\``).join(', ')}`);
      return `Every section matching \`${str('parent')}\` that contains a line matching \`${str('when')}\` must ${
        parts.join(' and ') || 'exist'
      }.`;
    }
    case 'snippet':
      return 'Every line of a golden block, in its block structure, must be on the device.';
    default:
      return `A \`${check.type}\` check.`;
  }
}

// ---------------------------------------------------------------- diff shape

export interface DiffSide {
  /** The line number in that version, when it has one. */
  no: number | null;
  text: string;
}

export interface SideRow {
  kind: 'same' | 'add' | 'remove' | 'change' | 'gap';
  left?: DiffSide;
  right?: DiffSide;
}

/**
 * The engine's unified diff, projected into two columns.
 *
 * This is deliberately a projection and not a diff: the lines, their
 * numbers and what counts as added or removed are all the engine's, exactly
 * as they are in the unified view. A browser-side re-diff could disagree
 * with the engine about what changed, and then neither view would be the
 * truth. A run of removals is paired with the run of additions that follows
 * it, which is what makes an edited line read as one row.
 */
export function sideBySide(lines: readonly DiffLine[]): SideRow[] {
  const rows: SideRow[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    if (!line) break;
    if (line.tag === '@') {
      rows.push({ kind: 'gap' });
      index += 1;
      continue;
    }
    if (line.tag !== '+' && line.tag !== '-') {
      rows.push({
        kind: 'same',
        left: { no: line.old_line ?? null, text: line.text },
        right: { no: line.new_line ?? null, text: line.text },
      });
      index += 1;
      continue;
    }
    const removed: DiffLine[] = [];
    while (lines[index]?.tag === '-') {
      removed.push(lines[index] as DiffLine);
      index += 1;
    }
    const added: DiffLine[] = [];
    while (lines[index]?.tag === '+') {
      added.push(lines[index] as DiffLine);
      index += 1;
    }
    for (let step = 0; step < Math.max(removed.length, added.length); step += 1) {
      const left = removed[step];
      const right = added[step];
      rows.push({
        kind: left && right ? 'change' : left ? 'remove' : 'add',
        left: left ? { no: left.old_line ?? null, text: left.text } : undefined,
        right: right ? { no: right.new_line ?? null, text: right.text } : undefined,
      });
    }
  }
  return rows;
}

/** What a diff line is, spoken: screen readers get this in front of the text. */
export function diffTagLabel(tag: string): string {
  if (tag === '+') return 'added';
  if (tag === '-') return 'removed';
  if (tag === '@') return 'unchanged lines skipped';
  return 'unchanged';
}

/** A section path as one string: `router bgp 65001 › address-family ipv4`. */
export function sectionLabel(path: readonly string[]): string {
  return path.length === 0 ? 'Top level' : path.join(' › ');
}

// ------------------------------------------------------------- backup job

export type BackupDeviceState = 'first' | 'changed' | 'unchanged' | 'skipped' | 'failed' | 'running';

export interface BackupDeviceRow {
  device_id: string;
  /** The name the job's own result gave, when it gave one. */
  name?: string;
  state: BackupDeviceState;
  /** The engine's words: a skip reason, an error, or what it stored. */
  detail: string;
  /** Warnings about this device that are not its outcome (a startup-config that could not be read). */
  notes: string[];
}

const DEVICE_STATE: { prefix: string; state: BackupDeviceState }[] = [
  { prefix: 'first backup stored', state: 'first' },
  { prefix: 'configuration changed', state: 'changed' },
  { prefix: 'configuration unchanged', state: 'unchanged' },
  { prefix: 'skipped: ', state: 'skipped' },
];

/**
 * Per-device outcomes of a backup job.
 *
 * The job's `result` counts outcomes but names only the devices it skipped
 * or failed on, so which device was unchanged and which was stored anew is
 * read from the log lines the job wrote per device. Every line the engine
 * wrote is kept as it was written; nothing here paraphrases it.
 */
export function backupRows(job: Job, nameOf: (deviceId: string) => string | undefined): BackupDeviceRow[] {
  const result = (job.result ?? {}) as {
    skipped?: { device_id: string; name?: string; reason?: string }[];
    failed?: { device_id: string; name?: string; error?: string }[];
  };
  const named = new Map<string, string>();
  for (const entry of [...(result.skipped ?? []), ...(result.failed ?? [])]) {
    if (entry.name) named.set(entry.device_id, entry.name);
  }

  const rows = new Map<string, BackupDeviceRow>();
  for (const deviceId of job.device_ids) {
    rows.set(deviceId, {
      device_id: deviceId,
      name: named.get(deviceId) ?? nameOf(deviceId),
      state: 'running',
      detail: '',
      notes: [],
    });
  }

  for (const line of job.log) {
    const deviceId = line.device_id;
    if (!deviceId) continue;
    const row = rows.get(deviceId) ?? {
      device_id: deviceId,
      name: named.get(deviceId) ?? nameOf(deviceId),
      state: 'running' as BackupDeviceState,
      detail: '',
      notes: [],
    };
    rows.set(deviceId, row);
    if (line.level === 'error') {
      row.state = 'failed';
      row.detail = line.message;
      continue;
    }
    const match = DEVICE_STATE.find((entry) => line.message.startsWith(entry.prefix));
    if (match) {
      row.state = match.state;
      row.detail = line.message;
    } else {
      row.notes.push(line.message);
    }
  }

  // A failure recorded in the result but not in the log still has to show.
  for (const entry of result.failed ?? []) {
    const row = rows.get(entry.device_id);
    if (row && row.state !== 'failed') {
      row.state = 'failed';
      row.detail = entry.error ?? row.detail;
    }
  }

  const order: Record<BackupDeviceState, number> = {
    failed: 0,
    skipped: 1,
    running: 2,
    changed: 3,
    first: 4,
    unchanged: 5,
  };
  return [...rows.values()].sort(
    (a, b) => order[a.state] - order[b.state] || (a.name ?? '').localeCompare(b.name ?? ''),
  );
}

export const BACKUP_STATE_WORD: Record<BackupDeviceState, string> = {
  first: 'First capture',
  changed: 'Changed',
  unchanged: 'Unchanged',
  skipped: 'Skipped',
  failed: 'Failed',
  running: 'Working…',
};

export const BACKUP_STATE_TONE: Record<BackupDeviceState, StatusTone> = {
  first: 'ok',
  changed: 'info',
  unchanged: 'ok',
  skipped: 'idle',
  failed: 'crit',
  running: 'idle',
};
