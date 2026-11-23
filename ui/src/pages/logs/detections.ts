/**
 * The reasoning behind the Detections pane.
 *
 * A detection only tells. Each entry of the catalogue says what it is
 * decided from and what it cannot see; each hit carries the records it was
 * decided from, by id. This module keeps three distinctions that the engine
 * makes and a screen could easily flatten:
 *
 * * **Switched on is not the same as able to run.** `enabled` is the switch,
 *   `available` is whether this build has the detection's source. A
 *   detection that is on and unavailable is silent, and says why
 *   ({@link standingOf}).
 * * **A check that could not be made found nothing — and proved nothing.**
 *   `last_error` is shown as a failed check, never as a quiet detection.
 * * **Evidence is data, not prose.** It is ids, counts and two words a
 *   device said. {@link evidenceFacts} lays it out field by field as text;
 *   a field this screen has no name for is still shown, under the engine's
 *   own key, rather than dropped.
 */

import type { Detection, DetectionHit } from '@/api/types/detect';
import type { StatusTone } from '@/design/tone';
import { formatCount } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';

export function severityTone(severity: string): StatusTone {
  switch (severity) {
    case 'critical':
    case 'high':
      return 'crit';
    case 'medium':
      return 'warn';
    case 'low':
    case 'info':
      return 'info';
    default:
      return 'idle';
  }
}

export type StandingKind =
  /** On, and this build can run it. */
  | 'watching'
  /** On, and this build cannot run it: silent, with the reason. */
  | 'unavailable'
  /** Switched off, and it could run. */
  | 'off'
  /** Switched off, and it could not run anyway. */
  | 'off-unavailable';

export interface Standing {
  kind: StandingKind;
  tone: StatusTone;
  label: string;
  /** The engine's reason it cannot run, when it cannot. */
  reason: string | null;
}

/** Whether a detection is watching, and if not, which of the two reasons it is. */
export function standingOf(detection: Pick<Detection, 'enabled' | 'available' | 'unavailable_reason'>): Standing {
  const reason = detection.unavailable_reason ?? null;
  if (detection.enabled && detection.available) {
    return { kind: 'watching', tone: 'ok', label: 'Watching', reason: null };
  }
  if (detection.enabled) {
    return { kind: 'unavailable', tone: 'warn', label: 'Cannot run in this build', reason };
  }
  if (detection.available) {
    return { kind: 'off', tone: 'idle', label: 'Switched off', reason: null };
  }
  return { kind: 'off-unavailable', tone: 'idle', label: 'Switched off · cannot run in this build', reason };
}

/** The engine counts a day's hits from the event stream and stops at this many. */
export const HITS_COUNTED_UP_TO = 100_000;

/** `3 hits in 24 h`; at the engine's ceiling, a floor and said to be one. */
export function hitsPhrase(hits24h: number): string {
  if (hits24h >= HITS_COUNTED_UP_TO) return `at least ${formatCount(HITS_COUNTED_UP_TO)} hits in 24 h`;
  return `${formatCount(hits24h)} ${hits24h === 1 ? 'hit' : 'hits'} in 24 h`;
}

/**
 * Whether the whole feature looks switched off: every entry reads
 * `enabled: false`. The catalogue cannot tell that apart from seven switches
 * moved by hand, so this only decides whether to *mention* the setting; the
 * engine's own refusal is what says it for certain.
 */
export function allOff(detections: readonly Pick<Detection, 'enabled'>[]): boolean {
  return detections.length > 0 && detections.every((detection) => !detection.enabled);
}

// ------------------------------------------------------------------ evidence

export interface EvidenceFact {
  /** The dotted path in the evidence, for a stable key. */
  path: string;
  label: string;
  /** Always text. */
  value: string;
  mono?: boolean;
}

/** Names for the engine's evidence keys. A key that is not here keeps its own name. */
const LABELS: Record<string, string> = {
  decided_from: 'Decided from',
  log_entry_id: 'Log entry',
  rule_id: 'Log rule',
  stream: 'Stream',
  via: 'How the line got here',
  by: 'By (as the device said)',
  from: 'From (as the device said)',
  changes_checked: 'Changes compared',
  jobs_checked: 'Jobs compared',
  windows_checked: 'Maintenance windows compared',
  grace_minutes: 'Grace either side',
  caution: 'Caution',
  'nearest_change.change_id': 'Nearest change',
  'nearest_change.state': 'Its state',
  'nearest_change.updated_at': 'It reached that state',
  kind: 'Configuration',
  version_id: 'Version',
  previous_id: 'Compared with version',
  'between.from': 'Earlier version last seen unchanged',
  'between.to': 'Later version first seen',
  lines_added: 'Lines added',
  lines_removed: 'Lines removed',
  host: 'Host',
  port: 'Port',
  'known.algorithm': 'Remembered key',
  'known.fingerprint': 'Remembered fingerprint',
  'known.first_seen': 'Remembered since',
  'presented.algorithm': 'Presented key',
  'presented.fingerprint': 'Presented fingerprint',
  device_ids: 'Devices',
  host_key_policy: 'Host key policy',
  connection: 'The connection',
  action: 'Audit action',
  count: 'Count',
  window_min: 'Within',
  audit_entry_id: 'Audit entry',
  audit_entry_ids: 'Audit entries',
  through_audit_id: 'Counted up to audit entry',
  who: 'Who',
  what: 'What was guessed at',
  change_id: 'Change',
  state: 'State',
  finished_at: 'Finished',
  devices: 'Devices and protected hits',
  devices_no_longer_in_inventory: 'Devices no longer in the inventory',
};

/** Keys whose number is an instant, shown as one. */
const INSTANTS = new Set([
  'nearest_change.updated_at',
  'between.from',
  'between.to',
  'known.first_seen',
  'finished_at',
]);

/** Keys whose number is a count of minutes. */
const MINUTES = new Set(['grace_minutes', 'window_min']);

/** Keys whose text is the engine's or a device's own words rather than an identifier. */
const PROSE = new Set(['caution', 'who', 'decided_from', 'connection', 'what', 'kind', 'state', 'nearest_change.state', 'via', 'stream', 'host_key_policy']);

const VIA_WORD: Record<string, string> = {
  syslog: 'syslog — sent to the receiver, unauthenticated',
  lab: 'lab — handed over by a simulated device, in process',
  pull: 'pull — read back over Nocturne’s own session',
};

const CONNECTION_WORD: Record<string, string> = {
  refused: 'refused — the connection was not made',
  went_ahead: 'went ahead — the host key policy accepts a changed key',
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function scalar(path: string, value: unknown): string {
  if (value === null) return 'none';
  if (typeof value === 'number') {
    if (INSTANTS.has(path)) return formatTimestamp(value);
    if (MINUTES.has(path)) return `${formatCount(value)} min`;
    // An id is a name, not a quantity: no digit grouping.
    return path.endsWith('_id') || path.endsWith('_ids') || path === 'port' ? String(value) : formatCount(value);
  }
  if (typeof value === 'boolean') return value ? 'yes' : 'no';
  if (typeof value === 'string') {
    if (path === 'via') return VIA_WORD[value] ?? value;
    if (path === 'connection') return CONNECTION_WORD[value] ?? value;
    return value === '' ? '(empty)' : value;
  }
  return JSON.stringify(value);
}

function labelOf(path: string): string {
  return LABELS[path] ?? path;
}

function push(facts: EvidenceFact[], path: string, value: unknown): void {
  if (value === undefined) return;
  if (Array.isArray(value)) {
    if (value.length === 0) {
      facts.push({ path, label: labelOf(path), value: 'none' });
      return;
    }
    const simple = value.every((item) => !isRecord(item) && !Array.isArray(item));
    facts.push({
      path,
      label: labelOf(path),
      value: simple
        ? value.map((item) => scalar(path, item)).join(', ')
        : value
            .map((item) =>
              isRecord(item)
                ? Object.entries(item)
                    .map(([key, part]) => `${key} ${scalar(`${path}.${key}`, part)}`)
                    .join(', ')
                : scalar(path, item),
            )
            .join(' · '),
      mono: true,
    });
    return;
  }
  if (isRecord(value)) {
    for (const [key, part] of Object.entries(value)) push(facts, `${path}.${key}`, part);
    return;
  }
  facts.push({
    path,
    label: labelOf(path),
    value: scalar(path, value),
    mono: !PROSE.has(path) && typeof value !== 'boolean',
  });
}

/**
 * A hit's evidence, field by field, as text. Nested records are flattened to
 * a dotted path; lists are joined. Nothing is interpreted beyond naming the
 * field and formatting an instant — the evidence is the engine's judgement
 * and is shown as it was given.
 */
export function evidenceFacts(evidence: unknown): EvidenceFact[] {
  if (!isRecord(evidence)) return [];
  const facts: EvidenceFact[] = [];
  for (const [key, value] of Object.entries(evidence)) push(facts, key, value);
  return facts;
}

/** The log entry a hit was decided from, when it names one. */
export function logEntryOf(evidence: unknown): number | null {
  if (!isRecord(evidence)) return null;
  return typeof evidence.log_entry_id === 'number' ? evidence.log_entry_id : null;
}

// --------------------------------------------------------------------- links

export interface HitLinks {
  /** Where the hit's alert is: the one alert list. `null` when no alert was raised. */
  alert: string | null;
  /** The device's own page. `null` when the hit is about no one device. */
  device: string | null;
}

/**
 * Where a hit points. There is no second inbox: the alert is in Monitoring's
 * alert list like every other, so that is where its link goes.
 */
export function hitLinks(hit: Pick<DetectionHit, 'alert_id' | 'device_id'>): HitLinks {
  return {
    alert: hit.alert_id ? '/monitoring/alerts' : null,
    device: hit.device_id ? `/devices/${encodeURIComponent(hit.device_id)}` : null,
  };
}

/** The detection's name for a hit, from the catalogue; its id when the catalogue has no such entry. */
export function detectionName(id: string, catalogue: readonly Pick<Detection, 'id' | 'name'>[]): string {
  return catalogue.find((entry) => entry.id === id)?.name ?? id;
}

/**
 * A hit that just arrived, in front of the list it belongs in. `filter` is
 * the detection the list is narrowed to, if any: a hit of another detection
 * is not added. A hit already in the list is not added twice.
 */
export function withHit(
  hits: readonly DetectionHit[],
  hit: DetectionHit,
  filter: string | null,
  limit: number,
): DetectionHit[] {
  if (filter !== null && filter !== '' && hit.detection !== filter) return [...hits];
  if (hits.some((known) => known.id === hit.id)) return [...hits];
  return [hit, ...hits].sort((a, b) => b.at - a.at).slice(0, limit);
}
