/**
 * Used against sold, and the nine different nothings. No React in here.
 *
 * `isp.usage` gives a row per customer in which `used` is `null` unless
 * `coverage === 'measured'`, and {@link Coverage} says which of nine reasons
 * it is. The distinction is the point of the report: a customer whose
 * interface has never been collected is not a customer using nothing, and in
 * a billing dispute that is the difference between a fact and a guess
 * (`docs/dev/features/isp.md`).
 *
 * So everything in here obeys one rule: **where there is no figure, the
 * answer is words.** {@link usageText} returns a sentence with no digit in it
 * rather than a `0`, a `—` or an empty bar, and
 * {@link coverageWords} gives each reason its own short label so a table can
 * be read down a column. The backend's own sentence (`coverage_note`) is
 * shown beside it, never paraphrased.
 *
 * One deliberate departure: `isp.capacity` carries the same `coverage_note`
 * as a customer row, and that sentence is written about a customer ("no
 * serving device is recorded for this customer"), which is wrong on a link.
 * {@link linkCoverageNote} writes the link's version. See `NOTES.md`.
 */

import type { Coverage, LinkCapacity, UsageFigure, UsedAgainstSold } from '@/api/types/isp';
import type { StatusTone } from '@/design/tone';
import { formatPercent, formatRate } from '@/lib/format';

export interface CoverageWords {
  /** Two or three words for a table cell. Never a number, never a dash. */
  label: string;
  /** The same thing as a sentence, for a tooltip or a legend. */
  meaning: string;
  /** Whether this is a figure, something nobody looked at, or a failure. */
  tone: StatusTone;
}

/**
 * What to call each reason. Nothing here is `0`, `—`, `n/a` or `unknown`:
 * each of these is a different thing to have found out, and the words say
 * which. The label is short enough for a column; the meaning is the sentence,
 * and the row's own `coverage_note` from the engine is shown beside it.
 */
export const COVERAGE_WORDS: Record<Coverage, CoverageWords> = {
  measured: {
    label: 'Measured',
    meaning: 'There is a figure, and it was measured.',
    tone: 'ok',
  },
  no_serving_device: {
    label: 'No serving device',
    meaning:
      'Nobody has recorded which device serves them, so there is nothing to measure their usage on — this is not a usage of zero.',
    tone: 'idle',
  },
  no_serving_interface: {
    label: 'No serving interface',
    meaning:
      'A device is recorded but not which interface, so there is no figure — this is not a usage of zero.',
    tone: 'idle',
  },
  device_missing: {
    label: 'Device is gone',
    meaning: 'The serving device is not in the inventory any more, so nothing can be measured.',
    tone: 'warn',
  },
  monitor_unavailable: {
    label: 'No monitoring here',
    meaning: 'This build has no monitoring, so no interface rate exists to compare a plan against.',
    tone: 'idle',
  },
  interface_never_collected: {
    label: 'Never collected',
    meaning:
      'Monitoring has never recorded that interface: nobody has looked, which is not the same as nothing passing.',
    tone: 'idle',
  },
  no_rate_yet: {
    label: 'No rate yet',
    meaning:
      'The interface is known and its rate could not be measured — a first poll, a reboot or a failed poll: a gap, not a zero.',
    tone: 'warn',
  },
  no_samples_in_window: {
    label: 'No samples in that window',
    meaning: 'Nothing was recorded for that interface inside the window asked for.',
    tone: 'idle',
  },
  monitor_failed: {
    label: 'Monitoring refused',
    meaning: 'Monitoring was asked for the rate and refused.',
    tone: 'crit',
  },
};

export function coverageWords(coverage: Coverage): CoverageWords {
  return COVERAGE_WORDS[coverage] ?? { label: coverage, meaning: '', tone: 'idle' };
}

/** Is there a figure at all? The only question `used` may be read against. */
export function isMeasured(coverage: Coverage): boolean {
  return coverage === 'measured';
}

/**
 * What a customer is using, or why that is not known — in words.
 *
 * There is no branch of this that returns a number for a row with no figure,
 * and `usage-has-no-digits` in the tests holds it to that.
 */
export function usageText(row: Pick<UsedAgainstSold, 'coverage' | 'used'>): string {
  if (!isMeasured(row.coverage) || !row.used) return coverageWords(row.coverage).label;
  return rateText(row.used.used_mbit);
}

/** A rate in megabits per second, through the app's own rate formatter. */
export function rateText(mbit: number): string {
  return formatRate(mbit * 1_000_000);
}

/**
 * What a customer was sold. `null` means they are on no plan — which is not a
 * plan of zero megabits, so it is a sentence and not `0 Mb/s`.
 */
export function soldText(sold_mbit: number | null): string {
  return sold_mbit === null ? 'No active subscription' : rateText(sold_mbit);
}

/**
 * Used as a share of the plan. `null` means one of the two numbers does not
 * exist, so there is no share — not 0% and not 100%.
 */
export function ofPlanText(of_plan_pct: number | null): string {
  return of_plan_pct === null ? 'no figure' : formatPercent(of_plan_pct);
}

/**
 * How to colour a share of the plan. Over the plan is worth a second look and
 * so is nearly all of it; a quarter of it is worth a different conversation,
 * and neither is an alarm, so nothing here is `crit`.
 */
export function ofPlanTone(of_plan_pct: number | null): StatusTone {
  if (of_plan_pct === null) return 'idle';
  if (of_plan_pct >= 100) return 'warn';
  if (of_plan_pct >= 80) return 'info';
  return 'ok';
}

/** Which direction the figure is of, and over what — straight from the figure. */
export function basisText(used: UsageFigure): string {
  const window = used.window;
  if (!window) return `${used.basis}, latest reading`;
  return `${used.basis}, averaged over the window (${window.rollup})`;
}

export interface CoverageTally {
  coverage: Coverage;
  customers: number;
  label: string;
  meaning: string;
  /** The engine's own sentence for this reason. */
  note: string;
  tone: StatusTone;
}

/**
 * The report's coverage, worst first, with `measured` last — a reader should
 * meet the reasons there is no figure before the figures.
 */
export function coverageTally(
  counts: readonly { coverage: Coverage; customers: number; note: string }[],
): CoverageTally[] {
  const order: Record<StatusTone, number> = { crit: 0, warn: 1, maint: 2, idle: 3, info: 4, ok: 5 };
  return counts
    .map((count) => ({ ...count, ...coverageWords(count.coverage) }))
    .sort((a, b) => order[a.tone] - order[b.tone] || b.customers - a.customers);
}

// ------------------------------------------------------------------- links

/**
 * The link's version of the coverage sentence.
 *
 * `isp.capacity` reuses the customer-worded note, which on a link reads "no
 * serving device is recorded for this customer" about a tower feed. These say
 * the same thing about the right subject.
 */
export function linkCoverageNote(coverage: Coverage): string {
  switch (coverage) {
    case 'measured':
      return 'measured on the link’s own interface';
    case 'no_serving_device':
      return 'no device is recorded for this link, so there is nothing to measure it on — this is not a load of zero';
    case 'no_serving_interface':
      return 'a device is recorded for this link but not which interface, so there is no load figure — this is not a load of zero';
    case 'device_missing':
      return 'the link’s device is not in the inventory any more, so nothing can be measured';
    case 'monitor_unavailable':
      return 'this build has no monitoring, so no interface rate exists for this link';
    case 'interface_never_collected':
      return 'monitoring has never recorded that interface on that device: nobody has looked, which is not the same as nothing passing';
    case 'no_rate_yet':
      return 'the interface is known but its rate could not be measured (a first poll, a reboot or a failed poll): a gap, not a zero';
    case 'no_samples_in_window':
      return 'nothing was recorded for that interface inside the window asked for';
    case 'monitor_failed':
      return 'monitoring was asked for the link’s rate and refused';
    default:
      return 'there is no load figure for this link';
  }
}

/**
 * What a link is carrying, or why that is not known. The sold figure is
 * beside it elsewhere; this is only the measurement.
 */
export function loadText(row: Pick<LinkCapacity, 'coverage' | 'measured'>): string {
  if (!isMeasured(row.coverage) || !row.measured) return coverageWords(row.coverage).label;
  return rateText(row.measured.used_mbit);
}

/**
 * How to colour an oversubscription ratio. `null` means no capacity was ever
 * written down, which is not 1:1 and not 0:1.
 */
export function ratioTone(ratio: number | null): StatusTone {
  if (ratio === null) return 'idle';
  if (ratio > 4) return 'crit';
  if (ratio > 2) return 'warn';
  if (ratio > 1) return 'info';
  return 'ok';
}

/**
 * Sold against capacity, as a sentence that always carries both figures — a
 * ratio on its own is the thing the specification refuses to show.
 */
export function capacityText(row: LinkCapacity): string {
  if (row.capacity_mbit <= 0) return `${rateText(row.sold_mbit)} sold, no capacity recorded`;
  return `${rateText(row.sold_mbit)} sold of ${rateText(row.capacity_mbit)} (${row.ratio_text})`;
}

/** Where the capacity figure came from, in words. */
export function capacityBasisText(basis: LinkCapacity['capacity_basis']): string {
  return basis === 'declared'
    ? 'capacity as somebody wrote it down'
    : 'capacity as the interface itself reports it';
}
