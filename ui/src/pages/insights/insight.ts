/**
 * The words the Insights screen uses, and the arithmetic behind its coverage.
 *
 * One rule governs this file. A check can say three different things about a
 * device and they must never be confused:
 *
 *   * it found something        → a finding, with its evidence;
 *   * it looked and found nothing → "clear";
 *   * it could not look        → "not evaluated", with the reason;
 *   * and a check that is switched off said nothing at all.
 *
 * `checkVerdict` is the one place that decides which of those a coverage row
 * is, and `verdict()` the one place that gives it a word and a colour.
 */

import type {
  CheckCoverage,
  Category,
  Coverage,
  Insight,
  InsightState,
  Severity,
} from '@/api/types/insights';
import type { StatusTone } from '@/design/tone';
import { formatCount, plural } from '@/lib/format';

// ---------------------------------------------------------------- severity

/** Worst first: the order findings are listed and grouped in. */
export const SEVERITIES: readonly Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

const SEVERITY_LABEL: Record<Severity, string> = {
  critical: 'Critical',
  high: 'High',
  medium: 'Medium',
  low: 'Low',
  info: 'Info',
};

/**
 * The same mapping the Monitoring screen uses (`pages/monitoring/vocab.ts`),
 * so one severity is one colour across the application. Colour carries the
 * level, the word carries the meaning: critical and high share a hue and are
 * told apart by their label, never by hue alone.
 */
const SEVERITY_TONE: Record<Severity, StatusTone> = {
  critical: 'crit',
  high: 'crit',
  medium: 'warn',
  low: 'info',
  info: 'info',
};

export function severityLabel(severity: Severity): string {
  return SEVERITY_LABEL[severity] ?? severity;
}

export function severityTone(severity: Severity): StatusTone {
  return SEVERITY_TONE[severity] ?? 'info';
}

/** 0 is the worst. An unknown severity sorts last. */
export function severityRank(severity: Severity): number {
  const rank = SEVERITIES.indexOf(severity);
  return rank === -1 ? SEVERITIES.length : rank;
}

/** The worst severity with a count above zero, or null when nothing matches. */
export function worstSeverity(counts: Record<string, number> | undefined): Severity | null {
  if (!counts) return null;
  return SEVERITIES.find((severity) => (counts[severity] ?? 0) > 0) ?? null;
}

// ---------------------------------------------------------------- category

export const CATEGORIES: readonly Category[] = [
  'physical',
  'l2',
  'l3',
  'config',
  'security',
  'capacity',
  'hygiene',
];

const CATEGORY_LABEL: Record<Category, string> = {
  physical: 'Physical',
  l2: 'Layer 2',
  l3: 'Layer 3',
  config: 'Configuration',
  security: 'Security',
  capacity: 'Capacity',
  hygiene: 'Hygiene',
};

export function categoryLabel(category: Category): string {
  return CATEGORY_LABEL[category] ?? category;
}

// ------------------------------------------------------------------- state

const STATE_LABEL: Record<InsightState, string> = {
  open: 'Open',
  dismissed: 'Dismissed',
  resolved: 'Resolved',
};

export function stateLabel(state: InsightState): string {
  return STATE_LABEL[state] ?? state;
}

export function stateTone(state: InsightState): StatusTone {
  return state === 'open' ? 'warn' : state === 'resolved' ? 'ok' : 'idle';
}

// ---------------------------------------------------------------- verdicts

/** What one check managed to say about the devices in the last scan. */
export type Verdict = 'findings' | 'clear' | 'partial' | 'not_evaluated' | 'off';

/**
 * A coverage row's verdict. `evaluated + skipped` is every device the scan
 * covered, so a check with findings on some devices and no verdict on others
 * is `findings` — the findings are the headline — while a check with nothing
 * to report is `clear` only where it reached every device.
 */
export function checkVerdict(row: CheckCoverage): Verdict {
  if (!row.enabled) return 'off';
  if (row.findings > 0) return 'findings';
  if (row.evaluated === 0) return 'not_evaluated';
  if (row.skipped > 0) return 'partial';
  return 'clear';
}

export interface VerdictWords {
  label: string;
  tone: StatusTone;
  /** One sentence, in devices, for the row's tooltip and for assistive technology. */
  detail: string;
}

/** The word, the colour and the sentence for a coverage row. */
export function verdict(row: CheckCoverage): VerdictWords {
  const devices = (count: number) => plural(count, 'device');
  switch (checkVerdict(row)) {
    case 'off':
      return {
        label: 'Off',
        tone: 'idle',
        detail: 'This check is switched off, so it said nothing about any device.',
      };
    case 'findings':
      return {
        label: plural(row.findings, 'finding'),
        tone: 'crit',
        detail:
          row.skipped > 0
            ? `Found something on ${devices(row.findings)}; reached a verdict for ${devices(row.evaluated)} and could not decide for ${devices(row.skipped)}.`
            : `Found something on ${devices(row.findings)} of ${devices(row.evaluated)}.`,
      };
    case 'not_evaluated':
      return {
        label: 'Not evaluated',
        tone: 'idle',
        detail: `This check reached no verdict at all: ${devices(row.skipped)} could not be decided.`,
      };
    case 'partial':
      return {
        label: `Clear on ${formatCount(row.evaluated)} of ${formatCount(row.evaluated + row.skipped)}`,
        tone: 'warn',
        detail: `Nothing found on ${devices(row.evaluated)}; ${devices(row.skipped)} could not be decided.`,
      };
    case 'clear':
      return {
        label: 'Clear',
        tone: 'ok',
        detail: `Nothing found, and every one of ${devices(row.evaluated)} was evaluated.`,
      };
  }
}

// ------------------------------------------------------------ coverage sums

export interface CoverageTotals {
  /** Devices the last scan covered. */
  devices: number;
  /** Checks in the catalogue, as the last scan saw it. */
  checks: number;
  checksOn: number;
  checksOff: number;
  /** Device-by-check pairs with a verdict. */
  evaluated: number;
  /** Device-by-check pairs the scan could not decide. */
  skipped: number;
  /** `evaluated + skipped`: every pair an enabled check was asked about. */
  considered: number;
  findings: number;
  checksWithFindings: number;
  checksClear: number;
  checksPartly: number;
  checksNotEvaluated: number;
}

/**
 * The coverage report in numbers. Pairs, not devices: a check that could not
 * decide for three devices is three missing answers, and summing them is the
 * only honest way to say how much of the network was really examined.
 */
export function coverageTotals(coverage: Coverage): CoverageTotals {
  const totals: CoverageTotals = {
    devices: coverage.devices,
    checks: coverage.checks.length,
    checksOn: 0,
    checksOff: 0,
    evaluated: 0,
    skipped: 0,
    considered: 0,
    findings: 0,
    checksWithFindings: 0,
    checksClear: 0,
    checksPartly: 0,
    checksNotEvaluated: 0,
  };
  for (const row of coverage.checks) {
    totals.findings += row.findings;
    if (!row.enabled) {
      totals.checksOff += 1;
      continue;
    }
    totals.checksOn += 1;
    totals.evaluated += row.evaluated;
    totals.skipped += row.skipped;
    totals.considered += row.evaluated + row.skipped;
    switch (checkVerdict(row)) {
      case 'findings':
        totals.checksWithFindings += 1;
        break;
      case 'clear':
        totals.checksClear += 1;
        break;
      case 'partial':
        totals.checksPartly += 1;
        break;
      case 'not_evaluated':
        totals.checksNotEvaluated += 1;
        break;
      case 'off':
        break;
    }
  }
  return totals;
}

/** One reason a check could not decide, and how much of the scan it accounts for. */
export interface ReasonTotal {
  /** The backend's own sentence, never paraphrased. */
  reason: string;
  /** Device-by-check pairs this reason accounts for. */
  devices: number;
  /** The checks that gave this reason, worst first by how many devices each lost to it. */
  checks: { check: string; title: string; devices: number }[];
}

/**
 * Every reason the last scan could not decide something, with the checks it
 * came from — the backend's words, grouped by how many devices they cover.
 * Switched-off checks are left out: "the check is switched off" is not a gap
 * in the data, it is a decision, and the catalogue is where it belongs.
 */
export function reasonTotals(coverage: Coverage): ReasonTotal[] {
  const totals = new Map<string, ReasonTotal>();
  for (const row of coverage.checks) {
    if (!row.enabled) continue;
    for (const { reason, devices } of row.reasons) {
      const found = totals.get(reason) ?? { reason, devices: 0, checks: [] };
      found.devices += devices;
      found.checks.push({ check: row.check, title: row.title, devices });
      totals.set(reason, found);
    }
  }
  const list = [...totals.values()];
  for (const entry of list) entry.checks.sort((a, b) => b.devices - a.devices);
  list.sort((a, b) => b.devices - a.devices || a.reason.localeCompare(b.reason));
  return list;
}

// --------------------------------------------------------------- freshness

/**
 * How much a scan can still be believed. A finding is only as true as the
 * data it was decided from, so the screen says the age of the scan everywhere
 * it shows findings, and says it louder as it ages.
 */
export type Freshness = 'fresh' | 'ageing' | 'stale';

/** An hour: long enough for a collection and a scan to have happened. */
export const AGEING_MS = 60 * 60 * 1000;
/** A day: by now the network has almost certainly moved on. */
export const STALE_MS = 24 * 60 * 60 * 1000;

export function freshness(scannedAt: number, now: number = Date.now()): Freshness {
  const age = now - scannedAt;
  if (age >= STALE_MS) return 'stale';
  if (age >= AGEING_MS) return 'ageing';
  return 'fresh';
}

export function freshnessTone(value: Freshness): StatusTone {
  return value === 'stale' ? 'crit' : value === 'ageing' ? 'warn' : 'ok';
}

// ----------------------------------------------------------------- history

/** A problem that was found, went away and came back is not a new discovery. */
export function isRepeat(insight: Insight): boolean {
  return insight.occurrences > 1;
}

/**
 * What a dismissal amounts to right now. A dismissal that has run out is
 * still `dismissed` in the ledger until the next scan, and saying so is the
 * only way the list and the backend can agree.
 */
export function dismissal(
  insight: Insight,
  now: number = Date.now(),
): { label: string; expired: boolean } | null {
  if (insight.state !== 'dismissed') return null;
  if (insight.dismissed_until == null) return { label: 'Dismissed for good', expired: false };
  const expired = insight.dismissed_until <= now;
  return { label: expired ? 'Dismissal has run out' : 'Dismissed', expired };
}

// ---------------------------------------------------------------- grouping

export interface SeverityGroup {
  severity: Severity;
  items: Insight[];
}

/** The findings in severity order, each level a group with its own heading. */
export function groupBySeverity(insights: readonly Insight[]): SeverityGroup[] {
  const groups = SEVERITIES.map((severity) => ({ severity, items: [] as Insight[] }));
  for (const insight of insights) {
    const group = groups[severityRank(insight.severity)];
    if (group) group.items.push(insight);
  }
  return groups.filter((group) => group.items.length > 0);
}

/** How a finding reads in one line, for the card heading and for a test. */
export function subjectLine(insight: Insight): string {
  return `${insight.device} ${insight.subject}`;
}

/**
 * Just the word, in the right number. `lib/format`'s `plural` writes the
 * count as well, which is usually what a sentence wants; this is for the
 * places where the count is already on screen ("3 devices · were").
 */
export function word(count: number, one: string, many = `${one}s`): string {
  return count === 1 ? one : many;
}
