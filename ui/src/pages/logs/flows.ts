/**
 * The reasoning behind the Flows pane.
 *
 * `flows.top` answers "who is talking" over a window, from one-minute
 * buckets the collector keeps in memory for a day. Nothing of a flow is
 * stored, so every answer comes with what it covers, and this module's job
 * is to make sure the table is never read as more than that:
 *
 * * **An empty answer is one of three things.** The collector is not
 *   listening (so nothing could arrive), or it is and the window holds
 *   nothing, or it holds something and this way of looking has no row.
 *   {@link emptyKind} says which.
 * * **Sampled figures are labelled, not multiplied.** Byte and packet counts
 *   are what the exporter sent. When an exporter said it samples 1 in N, the
 *   rate is stated beside the table ({@link samplingNote}) and the numbers
 *   stay as they came: multiplying here would turn a measurement into an
 *   estimate and call it a measurement.
 * * **`other` and `total` are the engine's.** `total` is exact; a row is a
 *   floor; `other` is what is under no key. They are shown as given, and a
 *   share is only worked out where it means something — by interface every
 *   flow is counted twice (in and out), so those rows add up to twice the
 *   total and get no percentage.
 */

import type { FlowBy, FlowCounts, FlowCoverage, FlowExporter, FlowTop, FlowTopParams } from '@/api/types/logs';
import type { StatusTone } from '@/design/tone';
import { formatCount, plural } from '@/lib/format';
import { formatTime } from '@/lib/time';

export const FLOW_BYS: readonly { value: FlowBy; label: string; column: string; help: string }[] = [
  { value: 'source', label: 'Source', column: 'Source address', help: 'Who sent the traffic.' },
  { value: 'destination', label: 'Destination', column: 'Destination address', help: 'Who it was sent to.' },
  {
    value: 'conversation',
    label: 'Conversation',
    column: 'Address pair',
    help: 'Both directions of an address pair are one row.',
  },
  {
    value: 'application',
    label: 'Application',
    column: 'Protocol and service port',
    help: 'The IP protocol and the port the service is on. A name is derived from the number: flow export says port 443, never HTTPS.',
  },
  {
    value: 'interface',
    label: 'Interface',
    column: 'Interface and direction',
    help: 'Each flow is counted under the interface it came in through and the one it left by, so these rows add up to twice the total.',
  },
  {
    value: 'dscp',
    label: 'DSCP',
    column: 'DSCP',
    help: 'The upper six bits of the ToS / Traffic Class byte, as the exporter saw it.',
  },
];

/** The periods offered, in minutes. A day is all that is kept. */
export const FLOW_PERIODS: readonly { minutes: number; label: string }[] = [
  { minutes: 5, label: 'Last 5 minutes' },
  { minutes: 15, label: 'Last 15 minutes' },
  { minutes: 60, label: 'Last hour' },
  { minutes: 360, label: 'Last 6 hours' },
  { minutes: 1_440, label: 'Last 24 hours' },
];

export const DEFAULT_PERIOD = 15;

export function isFlowBy(value: string | null): value is FlowBy {
  return FLOW_BYS.some((entry) => entry.value === value);
}

export function periodOf(value: string | null): number {
  const minutes = Number(value);
  return FLOW_PERIODS.some((period) => period.minutes === minutes) ? minutes : DEFAULT_PERIOD;
}

/**
 * Whose traffic: every exporter, the exporters of one device, or one
 * exporter by its address — which is how one no device claims is looked at.
 * Encoded as one string so it fits a `<select>` and a search parameter.
 */
export type FlowScope =
  | { kind: 'all' }
  | { kind: 'device'; id: string }
  | { kind: 'exporter'; address: string };

export function parseScope(value: string | null): FlowScope {
  if (value?.startsWith('device:') && value.length > 7) return { kind: 'device', id: value.slice(7) };
  if (value?.startsWith('exporter:') && value.length > 9) return { kind: 'exporter', address: value.slice(9) };
  return { kind: 'all' };
}

export function scopeValue(scope: FlowScope): string {
  if (scope.kind === 'device') return `device:${scope.id}`;
  if (scope.kind === 'exporter') return `exporter:${scope.address}`;
  return '';
}

/** The parameters of one `flows.top` call. */
export function topParams(by: FlowBy, minutes: number, scope: FlowScope, limit = 20): FlowTopParams {
  return {
    by,
    minutes,
    limit,
    ...(scope.kind === 'device' ? { device_id: scope.id } : {}),
    ...(scope.kind === 'exporter' ? { exporter: scope.address } : {}),
  };
}

export interface ScopeChoice {
  value: string;
  label: string;
}

/**
 * What the scope picker offers: everything, then each exporter heard from —
 * by its device when one claims it, by its address when none does.
 */
export function scopeChoices(exporters: readonly FlowExporter[]): ScopeChoice[] {
  const choices: ScopeChoice[] = [{ value: '', label: 'Every exporter' }];
  const devices = new Set<string>();
  for (const exporter of exporters) {
    if (exporter.device_id != null && exporter.device_id !== '') {
      if (devices.has(exporter.device_id)) continue;
      devices.add(exporter.device_id);
      choices.push({
        value: `device:${exporter.device_id}`,
        label: exporter.device ?? exporter.device_id,
      });
    } else {
      choices.push({
        value: `exporter:${exporter.address}`,
        label: `${exporter.address} (no device claims it)`,
      });
    }
  }
  return choices;
}

// ------------------------------------------------------------------ coverage

export type EmptyKind =
  /** The collector is not listening: nothing could arrive. */
  | 'not-collecting'
  /** Listening, and the window holds no flow record. */
  | 'nothing-in-window'
  /** The window holds records, but none under a key of this way of looking. */
  | 'no-rows'
  /** There are rows. */
  | null;

/** Why the table is empty, which is never only "no traffic". */
export function emptyKind(top: FlowTop): EmptyKind {
  if (top.rows.length > 0) return null;
  if (top.coverage.records > 0) return 'no-rows';
  return top.coverage.collecting ? 'nothing-in-window' : 'not-collecting';
}

/** The sampling rates as the exporters said them: `1 in 100`, `1 in 100 and 1 in 1 000`. */
export function ratesPhrase(rates: readonly number[]): string {
  return rates.map((rate) => `1 in ${formatCount(rate)}`).join(rates.length === 2 ? ' and ' : ', ');
}

export interface SamplingNote {
  /** A short label for beside the heading: `Sampled 1 in 100`. */
  label: string;
  /** What it means for the numbers, in a sentence. */
  sentence: string;
  /** Whether every record in the answer came with a rate. */
  all: boolean;
}

/**
 * What to say about sampling, or `null` when no exporter said it samples.
 * The figures are never multiplied: this is the label that says so.
 */
export function samplingNote(coverage: FlowCoverage): SamplingNote | null {
  if (coverage.sampling.length === 0 && coverage.sampled_records === 0) return null;
  const rates = coverage.sampling.length === 0 ? 'a rate that is not reported' : ratesPhrase(coverage.sampling);
  const all = coverage.sampled_records >= coverage.records;
  const share = all
    ? 'Every record in this answer'
    : `${formatCount(coverage.sampled_records)} of the ${plural(coverage.records, 'record')} in this answer`;
  return {
    label: `Sampled ${rates}`,
    sentence: `${share} came from an exporter that samples ${rates}. Bytes and packets are as the exporter sent them — they are not multiplied here, so the real traffic is larger than these figures.`,
    all,
  };
}

export interface CoverageLine {
  id: 'window' | 'collecting' | 'since' | 'records' | 'templates' | 'sampling' | 'exporters';
  label: string;
  value: string;
  tone?: StatusTone;
  /** A sentence under the value, where the value alone would mislead. */
  note?: string;
}

/**
 * What an answer covers, line by line, in the order a reader needs it:
 * whether anything is coming in, what was asked about, since when there is
 * anything, how much, and what is known to be missing from it.
 */
export function coverageLines(coverage: FlowCoverage): CoverageLine[] {
  const lines: CoverageLine[] = [
    coverage.collecting
      ? { id: 'collecting', label: 'Collector', value: 'Listening', tone: 'ok' }
      : {
          id: 'collecting',
          label: 'Collector',
          value: 'Not listening',
          tone: 'warn',
          note: 'Nothing new is coming in. What was counted before is still here; an empty answer means “not collecting”, not “no traffic”.',
        },
    {
      id: 'window',
      label: 'Asked about',
      value: `${formatTime(coverage.window_start)} – ${formatTime(coverage.until)}`,
    },
    coverage.since == null
      ? {
          id: 'since',
          label: 'Holds traffic since',
          value: 'Nothing in this window',
          tone: 'idle',
        }
      : {
          id: 'since',
          label: 'Holds traffic since',
          value: formatTime(coverage.since),
          // Buckets are whole minutes; anything later than the start of the
          // window means the answer covers less than was asked for.
          ...(coverage.since - coverage.window_start >= 60_000
            ? { note: 'Later than the start of the period: the answer covers less than was asked for.' }
            : {}),
        },
    { id: 'records', label: 'Flow records', value: formatCount(coverage.records) },
    {
      id: 'exporters',
      label: 'Exporters',
      value: formatCount(coverage.exporters),
      ...(coverage.exporters > 1
        ? {
            note: 'A conversation that crosses two exporters is reported by both, and this answer adds both up.',
          }
        : {}),
    },
  ];
  if (coverage.sets_without_template > 0) {
    lines.push({
      id: 'templates',
      label: 'Sets without a template',
      value: formatCount(coverage.sets_without_template),
      tone: 'warn',
      note: 'Data that arrived before its template could not be read. That traffic was reported by the exporters and is in no row and not in the total.',
    });
  }
  const sampling = samplingNote(coverage);
  if (sampling) {
    lines.push({
      id: 'sampling',
      label: 'Sampling',
      value: ratesPhrase(coverage.sampling) || 'rate not reported',
      tone: 'warn',
      note: sampling.sentence,
    });
  }
  return lines;
}

/**
 * A row's part of the total by bytes, as a ratio — or `null` where that
 * would not be a share of anything: by interface each flow is counted twice,
 * and an empty total has no parts.
 */
export function shareOf(row: FlowCounts, total: FlowCounts, by: FlowBy): number | null {
  if (by === 'interface' || total.bytes <= 0) return null;
  return Math.min(1, row.bytes / total.bytes);
}

/** Whether `other` holds anything: an all-zero `other` is "nothing was folded", and reads so. */
export function hasOther(other: FlowCounts): boolean {
  return other.bytes > 0 || other.packets > 0 || other.flows > 0;
}

/** `5` → `NetFlow v5`, `10` → `IPFIX`. An unknown number is shown as the number it is. */
export function versionLabel(version: number): string {
  if (version === 5) return 'NetFlow v5';
  if (version === 9) return 'NetFlow v9';
  if (version === 10) return 'IPFIX';
  return `version ${version}`;
}

/**
 * What this collector has and has not met. The specification's own
 * statement (`docs/dev/features/logs.md`, "Not verified"), kept as one
 * sentence so the pane cannot quietly outgrow it.
 */
export const NOT_VERIFIED =
  'Not verified against a real exporter: this collector has only met two software exporters (softflowd 1.1.0 and nfdump 1.7.3’s nfreplay) reporting traffic written by hand as a capture file. It has never met a router or a switch — Cisco Flexible NetFlow, MikroTik Traffic Flow, Juniper inline J-Flow and the fields and option templates they send are untested, and so is an exporter that reports interface numbers.';
