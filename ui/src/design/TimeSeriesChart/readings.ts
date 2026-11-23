/**
 * How a chart reads its numbers out: on the axis, in the tooltip and in the
 * table view.
 */

import { EMPTY, formatCompact, formatDuration, type Numeric } from '@/lib/format';

const MINUS = '−';

/** A value that was recorded: a finite number. `null` is a gap, and so is anything else. */
export function isReading(value: number | null | undefined): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/**
 * A value for a chart that was given no formatter of its own: three
 * significant digits below a thousand, k, M, B and T above. A whole-number
 * format reads 0.3 as 0, 45.5 as 46, and puts "0" on every line of the axis
 * of a series that stays at zero, whose range is 0 to 0.1.
 */
export function formatReading(value: Numeric): string {
  if (!isReading(value)) return EMPTY;
  const magnitude = Math.abs(value);
  if (magnitude >= 1000) return formatCompact(value);
  const rounded = Number(magnitude.toPrecision(3));
  if (rounded >= 1000) return formatCompact(value);
  if (rounded === 0) return '0';
  // `String` would switch to exponent notation below a millionth.
  const text = rounded < 1e-6 ? rounded.toExponential(2).replace(/\.?0+e/, 'e') : String(rounded);
  return value < 0 ? MINUS + text : text;
}

interface Readable {
  values: readonly (number | null | undefined)[];
}

/**
 * Whether anything was recorded at `index`. A time at which no series has a
 * value is one the chart put there only to lift its pen across a hole: no
 * sample was taken at it, so it is not read out as one.
 */
export function recordedAt(series: readonly Readable[], index: number): boolean {
  return series.some((entry) => isReading(entry.values[index]));
}

/**
 * What the table and the tooltip say at such a time, in place of the time
 * itself: how long nothing was recorded for, between the samples on either
 * side of it. Short, and the length first: a table cell is as wide as a date
 * and a time, and cuts off what does not fit.
 */
export function holeText(timestamps: readonly number[], series: readonly Readable[], index: number): string {
  let before: number | undefined;
  for (let at = index - 1; at >= 0 && before === undefined; at -= 1) {
    if (recordedAt(series, at)) before = timestamps[at];
  }
  let after: number | undefined;
  for (let at = index + 1; at < timestamps.length && after === undefined; at += 1) {
    if (recordedAt(series, at)) after = timestamps[at];
  }
  return before !== undefined && after !== undefined && after > before
    ? `${formatDuration(after - before)}, no samples`
    : 'No samples';
}
