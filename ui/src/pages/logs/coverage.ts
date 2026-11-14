/**
 * The two judgements this screen must not get wrong.
 *
 * **An empty bucket is not a zero unless it can be proved to be one.**
 * `logs.histogram` answers with every bucket of the range, including the ones
 * with nothing in them, and it has no way to say *why* a bucket is empty. The
 * difference matters: "the network was quiet between three and four" and "we
 * were not listening between three and four" look identical in a bar chart and
 * mean opposite things. {@link bucketReach} decides, from `logs.status` alone,
 * which of them a bucket is, and the chart draws the unprovable ones as gaps.
 *
 * **"Nothing here" is not one state but four.** The receiver can be switched
 * off, enabled but unable to bind, listening with nothing ever arriving, or
 * listening and fed — and the fifth case, a filter that matches nothing, is not
 * a setup problem at all. {@link receiverVerdict} names which one it is, so the
 * screen can offer the device command instead of a shrug.
 *
 * Both are pure functions over the backend's own answers, which is why they are
 * here and tested rather than spread through the components.
 */

import type { Bucket, LogsStatus } from '@/api/types/logs';
import { SEVERITY_BANDS, bandKept, type BandId } from './query';

// ------------------------------------------------------------- the histogram

/** Why a bucket holds no bar. */
export type Reach =
  /** Counted: the range is stored and the receiver was listening. A zero is a zero. */
  | 'counted'
  /** The bucket has not happened yet. */
  | 'future'
  /** Older than the oldest line in the store: pruned, or never received. */
  | 'before-store'
  /** The receiver is not listening, so nothing could have arrived. */
  | 'not-listening';

export interface Window {
  now: number;
  /** `store.oldest_at`: the oldest line kept, or null for an empty store. */
  oldestAt: number | null;
  rows: number;
  /** Whether the syslog receiver has a socket open right now. */
  listening: boolean;
}

export function windowOf(status: LogsStatus | undefined, now: number): Window {
  return {
    now,
    oldestAt: status?.store.oldest_at ?? null,
    rows: status?.store.rows ?? 0,
    listening: (status?.syslog.listening.length ?? 0) > 0,
  };
}

/**
 * What an empty bucket means. A bucket with lines in it is always counted —
 * whatever the receiver is doing now, those lines are facts.
 */
export function bucketReach(bucket: Bucket, width: number, window: Window): Reach {
  if (bucket.count > 0) return 'counted';
  if (bucket.t > window.now) return 'future';
  // Nothing is stored at or before this bucket's end, so its emptiness is not
  // a reading: retention may have taken it, or nobody was sending yet.
  if (window.oldestAt === null || bucket.t + width <= window.oldestAt) return 'before-store';
  if (!window.listening) return 'not-listening';
  return 'counted';
}

export interface Segment {
  band: BandId;
  /** Every line of this band in the bucket. */
  count: number;
  /** Of those, the ones the severity filter keeps (drawn solid). */
  kept: number;
  /** The rest, which the filter hides (drawn as an outline, not left out). */
  hidden: number;
}

export interface Bar {
  t: number;
  /** The end of the bucket, for the tooltip's range. */
  until: number;
  reach: Reach;
  /** Every line in the bucket, whatever the severity filter keeps. */
  total: number;
  /** The lines the severity filter keeps. */
  shown: number;
  /** Tallest band first is wrong for a stack: these are in band order. */
  segments: Segment[];
}

export interface Hidden {
  /** Lines in the range, before the severity filter. */
  total: number;
  /** Lines the filter keeps. */
  shown: number;
  /** Hidden because they are less severe than the filter asks for. */
  lessSevere: number;
  /** Hidden because the line never said how severe it was. */
  noSeverity: number;
}

export interface HistogramView {
  bars: Bar[];
  /** Bucket width in milliseconds, as the backend chose it. */
  width: number;
  /** The tallest bucket, so every bar is drawn to the same scale. */
  peak: number;
  /** Buckets whose emptiness could not be proved, grouped into runs. */
  gaps: { from: number; until: number; reach: Reach; buckets: number }[];
  counts: Hidden;
}

/**
 * Turns the backend's buckets into bars.
 *
 * The buckets are asked for **without** the severity filter, so that the chart
 * can show what the filter is hiding instead of hiding it twice, and so the
 * counts below the chart are the honest denominator.
 */
export function histogramView(
  buckets: readonly Bucket[],
  maxSeverity: number | undefined,
  window: Window,
): HistogramView {
  const width =
    buckets.length >= 2 ? (buckets[1]?.t ?? 0) - (buckets[0]?.t ?? 0) : 60_000;
  const counts: Hidden = { total: 0, shown: 0, lessSevere: 0, noSeverity: 0 };
  const bars: Bar[] = buckets.map((bucket) => {
    const segments: Segment[] = [];
    let shown = 0;
    for (const band of SEVERITY_BANDS) {
      // A band can be partly hidden: "critical and worse" keeps three of the
      // four severities of the error band, so the split is per severity.
      const count =
        band.severities === null
          ? bucket.no_severity
          : band.severities.reduce((sum, severity) => sum + (bucket.by_severity[severity] ?? 0), 0);
      if (count === 0) continue;
      const kept =
        band.severities === null || maxSeverity === undefined
          ? bandKept(band, maxSeverity)
            ? count
            : 0
          : band.severities
              .filter((severity) => severity <= maxSeverity)
              .reduce((sum, severity) => sum + (bucket.by_severity[severity] ?? 0), 0);
      shown += kept;
      segments.push({ band: band.id, count, kept, hidden: count - kept });
    }
    counts.total += bucket.count;
    counts.shown += shown;
    counts.noSeverity += maxSeverity === undefined ? 0 : bucket.no_severity;
    return {
      t: bucket.t,
      until: bucket.t + width,
      reach: bucketReach(bucket, width, window),
      total: bucket.count,
      shown,
      segments,
    };
  });
  counts.lessSevere = counts.total - counts.shown - counts.noSeverity;

  const gaps: HistogramView['gaps'] = [];
  for (const bar of bars) {
    if (bar.reach === 'counted') continue;
    const last = gaps.at(-1);
    if (last && last.reach === bar.reach && last.until === bar.t) {
      last.until = bar.until;
      last.buckets += 1;
    } else {
      gaps.push({ from: bar.t, until: bar.until, reach: bar.reach, buckets: 1 });
    }
  }

  return {
    bars,
    width,
    peak: bars.reduce((peak, bar) => Math.max(peak, bar.total), 0),
    gaps,
    counts,
  };
}

/** One sentence about the holes, or null when there are none. */
export function gapNote(view: HistogramView, window: Window): string | null {
  const real = view.gaps.filter((gap) => gap.reach !== 'future');
  if (real.length === 0) return null;
  const buckets = real.reduce((sum, gap) => sum + gap.buckets, 0);
  const plural = buckets === 1 ? 'bucket is' : 'buckets are';
  const reasons = new Set(real.map((gap) => gap.reach));
  const why: string[] = [];
  if (reasons.has('before-store')) {
    why.push(
      window.rows === 0
        ? 'no log line is stored at all yet'
        : 'they are older than the oldest line still stored, so retention or a late start took them',
    );
  }
  if (reasons.has('not-listening')) {
    why.push('the receiver is not listening, so nothing could arrive');
  }
  return `${buckets} ${plural} drawn as a gap, not as a zero: ${why.join('; and ')}.`;
}

// -------------------------------------------------------------- the receiver

export type VerdictKind =
  /** The receiver is switched off. Nothing is listening. */
  | 'off'
  /** Enabled, but the socket could not be opened. The reason is the backend's. */
  | 'failed'
  /** Listening, and not one line has ever arrived. */
  | 'silent'
  /** Lines are stored, but none has arrived since this process started. */
  | 'stale'
  /** Listening and fed. */
  | 'receiving';

export interface Verdict {
  kind: VerdictKind;
  /** Whether a screen should offer the "point a device here" instructions. */
  setup: boolean;
  title: string;
  detail: string;
  /** The backend's own words, when it has some: a bind error. */
  error?: string;
}

/**
 * What the receiver is doing, in the words the screen says it in.
 *
 * `received`, `stored` and `dropped` are counted since the process started and
 * are deliberately not persisted, so "nothing received" after a restart is not
 * the same as "nothing has ever arrived" — `store.rows` is what says that.
 */
export function receiverVerdict(status: LogsStatus): Verdict {
  const { syslog, store } = status;
  if (!syslog.enabled) {
    return {
      kind: 'off',
      setup: true,
      title: 'The syslog receiver is switched off',
      detail:
        'Nothing is listening, so no device can send its logs here. Turning it on opens a listening port on this machine.',
    };
  }
  if (syslog.listening.length === 0) {
    return {
      kind: 'failed',
      setup: true,
      title: 'The syslog receiver is on but not listening',
      detail:
        'It is enabled, and the socket could not be opened — until it can, nothing a device sends arrives.',
      ...(syslog.error ? { error: syslog.error } : {}),
    };
  }
  if (store.rows === 0 && syslog.received === 0) {
    return {
      kind: 'silent',
      setup: true,
      title: `Listening on ${syslog.listening.join(', ')} — nothing has arrived yet`,
      detail:
        'The port is open and no device has sent a line to it. Devices do not send syslog until they are told where to.',
    };
  }
  if (syslog.received === 0) {
    return {
      kind: 'stale',
      setup: true,
      title: 'Nothing has arrived since Nocturne started',
      detail:
        'What is stored came from an earlier run or from a device’s own buffer. The port is open; no device has sent to it since.',
    };
  }
  return {
    kind: 'receiving',
    setup: false,
    title: `Listening on ${syslog.listening.join(', ')}`,
    detail: `${syslog.received} lines received since Nocturne started.`,
  };
}

/**
 * Why the list is empty, which is never only "no results".
 *
 * `narrowed` says whether anything but the time range is filtering, and
 * `rangeTotal` is how many lines the range holds before the severity filter —
 * so "your filter hides all of them" can be told apart from "this range is
 * empty".
 */
export function emptyReason(
  verdict: Verdict,
  { narrowed, rangeTotal }: { narrowed: boolean; rangeTotal: number },
): 'setup' | 'filter' | 'range' {
  if (rangeTotal > 0) return 'filter';
  if (verdict.setup) return 'setup';
  return narrowed ? 'filter' : 'range';
}
