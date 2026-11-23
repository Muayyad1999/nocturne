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
  /** Whether whatever fills the stream being looked at can receive right now. */
  listening: boolean;
}

/**
 * Whether anything can arrive in a stream right now. Each stream is filled
 * by something different: `syslog` by the syslog receiver, `trap` by the
 * trap receiver, and `audit` and `event` by the application itself, which is
 * always there. Every stream at once can receive when either receiver can.
 */
export function canReceive(status: LogsStatus | undefined, stream: string): boolean {
  if (stream === 'audit' || stream === 'event') return true;
  const syslog = (status?.syslog?.listening?.length ?? 0) > 0;
  const traps = (status?.traps?.listening?.length ?? 0) > 0;
  if (stream === 'trap') return traps;
  if (stream === 'syslog') return syslog;
  return syslog || traps;
}

export function windowOf(status: LogsStatus | undefined, now: number, stream = 'syslog'): Window {
  return {
    now,
    oldestAt: status?.store.oldest_at ?? null,
    rows: status?.store.rows ?? 0,
    listening: canReceive(status, stream),
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
    why.push('the receiver is not listening, so nothing a device sent could arrive');
  }
  return `${buckets} ${plural} drawn as a gap, not as a zero: ${why.join('; and ')}.`;
}

// -------------------------------------------------------------- the receiver

export type VerdictKind =
  /** This build has no such receiver. */
  | 'absent'
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
  /** Which receiver this is about. */
  receiver: 'syslog' | 'traps';
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
export function receiverVerdict(status: LogsStatus, stream = 'syslog'): Verdict {
  if (stream === 'trap') return trapVerdict(status);
  const { syslog, store } = status;
  if (!syslog.enabled) {
    return {
      receiver: 'syslog',
      kind: 'off',
      setup: true,
      title: 'The syslog receiver is switched off',
      detail:
        'Nothing is listening, so no device can send its logs here. Turning it on opens a listening port on this machine.',
    };
  }
  if (syslog.listening.length === 0) {
    return {
      receiver: 'syslog',
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
      receiver: 'syslog',
      kind: 'silent',
      setup: true,
      title: `Listening on ${syslog.listening.join(', ')} — nothing has arrived yet`,
      detail:
        'The port is open and no device has sent a line to it. Devices do not send syslog until they are told where to.',
    };
  }
  if (syslog.received === 0) {
    return {
      receiver: 'syslog',
      kind: 'stale',
      setup: true,
      title: 'Nothing has arrived since Nocturne started',
      detail:
        'What is stored came from an earlier run or from a device’s own buffer. The port is open; no device has sent to it since.',
    };
  }
  return {
    receiver: 'syslog',
    kind: 'receiving',
    setup: false,
    title: `Listening on ${syslog.listening.join(', ')}`,
    detail: `${syslog.received} lines received since Nocturne started.`,
  };
}

/**
 * The same judgement for the trap receiver, which is what fills the `trap`
 * stream. Two things differ. A build may have no trap receiver at all, and
 * says so. And the store's row count is syslog lines and traps together, so
 * "never been sent a trap" cannot be told from "none since this process
 * started" — the wording claims only the second.
 */
function trapVerdict(status: LogsStatus): Verdict {
  const traps = status.traps;
  if (traps.implemented === false) {
    return {
      receiver: 'traps',
      kind: 'absent',
      setup: false,
      title: 'SNMP traps are not in this build',
      detail:
        'This copy of Nocturne has no trap receiver, so nothing is listening for them. It is not switched off: it is absent.',
      ...(traps.error ? { error: traps.error } : {}),
    };
  }
  if (!traps.enabled) {
    return {
      receiver: 'traps',
      kind: 'off',
      setup: true,
      title: 'The SNMP trap receiver is switched off',
      detail:
        'Nothing is listening, so no device can send its notifications here. Turning it on opens a listening port on this machine.',
    };
  }
  if (traps.listening.length === 0) {
    return {
      receiver: 'traps',
      kind: 'failed',
      setup: true,
      title: 'The SNMP trap receiver is on but not listening',
      detail:
        'It is enabled, and the socket could not be opened — until it can, nothing a device sends arrives.',
      ...(traps.error ? { error: traps.error } : {}),
    };
  }
  if (traps.received === 0) {
    return {
      receiver: 'traps',
      kind: 'silent',
      setup: true,
      title: `Listening on ${traps.listening.join(', ')} — no trap has arrived since Nocturne started`,
      detail:
        'The port is open and no datagram has reached it. Devices do not send notifications until they are told where to.',
    };
  }
  return {
    receiver: 'traps',
    kind: 'receiving',
    setup: false,
    title: `Listening on ${traps.listening.join(', ')}`,
    detail: `${traps.received} datagrams received since Nocturne started, ${traps.stored} stored.`,
  };
}

/**
 * Whether a stream is filled by a receiver at all. The `audit` trail and the
 * `event` stream are written by the application, so an empty list there is
 * never a setup problem and gets no "point a device here".
 */
export function hasReceiver(stream: string): boolean {
  return stream !== 'audit' && stream !== 'event';
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
  verdict: Pick<Verdict, 'setup'> | null,
  { narrowed, rangeTotal }: { narrowed: boolean; rangeTotal: number },
): 'setup' | 'filter' | 'range' {
  if (rangeTotal > 0) return 'filter';
  if (verdict?.setup) return 'setup';
  return narrowed ? 'filter' : 'range';
}
