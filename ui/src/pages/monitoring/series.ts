/**
 * Turning `metrics.query` answers into something a chart can draw.
 *
 * The backend leaves a bucket out when nothing was recorded in it — a device
 * that did not answer, a counter with nothing to subtract from, the poll after
 * a reboot. Plotting those points as they come would draw a straight line
 * across the hole and claim a measurement nobody made, so this module puts the
 * series on one shared time axis and marks what is missing:
 *
 * * several series are aligned on the union of their timestamps, and a series
 *   with no sample at one of them gets `null` there;
 * * where the axis itself jumps — a stretch with no sample in any series — one
 *   extra timestamp is inserted that no series has a value for, which is what
 *   makes uPlot lift the pen instead of joining the ends;
 * * the stretches are returned as `gaps`, so the chart can also say in words
 *   how much of the range is missing.
 *
 * "How far apart samples normally are" is taken from the data itself (the
 * lower median distance between consecutive samples), never shorter than the
 * bucket width the backend answered with. A distance more than `GAP_FACTOR`
 * times that is a hole rather than ordinary jitter.
 */

import type { MetricSeries } from '@/api/types/monitor';

/** How much further apart than usual two samples must be to count as a gap. */
export const GAP_FACTOR = 1.5;

/** A stretch of the range with no sample in any of the aligned series. */
export interface Gap {
  /** The last sample before the hole. */
  from: number;
  /** The first sample after it. */
  to: number;
}

export interface AlignedSeries {
  /** Sample times, ascending, including the fillers that break the lines. */
  timestamps: number[];
  /** One row per input series, in the order given; `null` is a gap. */
  values: (number | null)[][];
  gaps: Gap[];
  /** How far apart samples are in this answer, in milliseconds. */
  spacing: number;
}

/**
 * The lower median of a sorted list. Lower, not the average of the two middle
 * values, so that a handful of holes in a short series cannot stretch what
 * counts as a normal distance between samples and hide themselves.
 */
function lowerMedian(sorted: readonly number[]): number {
  if (sorted.length === 0) return 0;
  return sorted[Math.floor((sorted.length - 1) / 2)] ?? 0;
}

/**
 * Puts several answers for the same time range on one axis.
 *
 * Every series must come from the same `since`/`until`/`step_s`, which is what
 * `metrics.query_many` guarantees: the store buckets by absolute time, so the
 * same request puts samples of different devices on the same grid.
 */
export function alignSeries(answers: readonly MetricSeries[]): AlignedSeries {
  const times = new Set<number>();
  for (const answer of answers) {
    for (const [at] of answer.points) times.add(at);
  }
  const axis = [...times].sort((a, b) => a - b);

  // The bucket width is the floor: samples can never be closer than that.
  const step = Math.max(1, ...answers.map((answer) => answer.step_s)) * 1000;
  const distances: number[] = [];
  for (let index = 1; index < axis.length; index += 1) {
    distances.push((axis[index] ?? 0) - (axis[index - 1] ?? 0));
  }
  const spacing = Math.max(step, lowerMedian([...distances].sort((a, b) => a - b)));

  const gaps: Gap[] = [];
  const timestamps: number[] = [];
  for (const [index, at] of axis.entries()) {
    const previous = index === 0 ? undefined : axis[index - 1];
    if (previous !== undefined && at - previous > spacing * GAP_FACTOR) {
      gaps.push({ from: previous, to: at });
      // One timestamp no series has a value for: the line ends here.
      timestamps.push(previous + spacing);
    }
    timestamps.push(at);
  }

  const values = answers.map((answer) => {
    const byTime = new Map(answer.points);
    return timestamps.map((at) => byTime.get(at) ?? null);
  });

  return { timestamps, values, gaps, spacing };
}

/** The time ranges the charts offer. */
export interface Range {
  id: string;
  label: string;
  /** How far back it reaches, in milliseconds. */
  span: number;
  /**
   * Seconds per point to ask for. Chosen well below the shortest sensible
   * polling interval for the short ranges, so a missed poll shows as a hole
   * rather than being averaged away, and at the store's own resolution for
   * the long ones (five minutes beyond two days, an hour beyond thirty).
   */
  step_s: number;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** The default, and what an unknown range id falls back to. */
const LAST_HOUR: Range = { id: '1h', label: 'Last hour', span: HOUR, step_s: 10 };

export const RANGES: readonly Range[] = [
  { id: '5m', label: 'Last 5 minutes', span: 5 * MINUTE, step_s: 1 },
  LAST_HOUR,
  { id: '6h', label: 'Last 6 hours', span: 6 * HOUR, step_s: 60 },
  { id: '24h', label: 'Last 24 hours', span: DAY, step_s: 300 },
  { id: '7d', label: 'Last 7 days', span: 7 * DAY, step_s: 1_800 },
  { id: '30d', label: 'Last 30 days', span: 30 * DAY, step_s: 3_600 },
];

export const DEFAULT_RANGE = LAST_HOUR.id;

export function range(id: string): Range {
  return RANGES.find((entry) => entry.id === id) ?? LAST_HOUR;
}

/**
 * Seconds per point for a range the user did not pick from the list (the
 * window around an alert, say): about three hundred points, never finer than
 * a second.
 */
export function stepFor(span: number): number {
  return Math.max(1, Math.round(span / 300 / 1000));
}
