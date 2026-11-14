import { describe, expect, it } from 'vitest';
import type { Bucket, LogsStatus } from '@/api/types/logs';
import {
  bucketReach,
  emptyReason,
  gapNote,
  histogramView,
  receiverVerdict,
  windowOf,
  type Window,
} from './coverage';

const WIDTH = 60_000;

function bucket(t: number, counts: Partial<Record<number, number>> = {}, noSeverity = 0): Bucket {
  const by = [0, 0, 0, 0, 0, 0, 0, 0];
  for (const [severity, count] of Object.entries(counts)) by[Number(severity)] = count ?? 0;
  return {
    t,
    count: by.reduce((sum, value) => sum + value, 0) + noSeverity,
    by_severity: by,
    no_severity: noSeverity,
  };
}

const LISTENING: Window = { now: 10 * WIDTH, oldestAt: 2 * WIDTH, rows: 400, listening: true };

function status(over: {
  enabled?: boolean;
  listening?: string[];
  error?: string | null;
  received?: number;
  rows?: number;
} = {}): LogsStatus {
  return {
    syslog: {
      enabled: over.enabled ?? true,
      listening: over.listening ?? ['udp 0.0.0.0:5514'],
      error: over.error ?? null,
      received: over.received ?? 12,
      stored: 12,
      dropped: 0,
      queued: 0,
      rate_per_minute: 3,
      last_at: 1_000,
    },
    traps: { enabled: false, implemented: false, error: 'the SNMP trap receiver is not implemented in this build' },
    flows: { enabled: false, implemented: false, error: 'the flow collector is not implemented in this build' },
    store: {
      rows: over.rows ?? 400,
      oldest_at: 2 * WIDTH,
      retention_days: 30,
      max_rows: 2_000_000,
      last_prune_at: null,
      pruned_by_age: 0,
      pruned_by_rows: 0,
    },
    unattributed_sources: 0,
  };
}

describe('an empty bucket, which is only a zero when it can be proved to be one', () => {
  it('counts a bucket that has lines in it, whatever the receiver is doing now', () => {
    const off: Window = { ...LISTENING, listening: false };
    expect(bucketReach(bucket(3 * WIDTH, { 3: 2 }), WIDTH, off)).toBe('counted');
  });

  it('calls a bucket inside the stored range with a listening receiver a real zero', () => {
    expect(bucketReach(bucket(5 * WIDTH), WIDTH, LISTENING)).toBe('counted');
  });

  it('will not call a bucket older than the oldest stored line a zero', () => {
    // Retention may have taken it, or nothing was being sent yet. Either way
    // nobody can say the network was quiet then.
    expect(bucketReach(bucket(0), WIDTH, LISTENING)).toBe('before-store');
    expect(bucketReach(bucket(WIDTH), WIDTH, LISTENING)).toBe('before-store');
    // The bucket that contains the oldest line reaches far enough.
    expect(bucketReach(bucket(2 * WIDTH), WIDTH, LISTENING)).toBe('counted');
  });

  it('treats an empty store as covering nothing at all', () => {
    const empty: Window = { ...LISTENING, oldestAt: null, rows: 0 };
    expect(bucketReach(bucket(5 * WIDTH), WIDTH, empty)).toBe('before-store');
  });

  it('will not call a bucket a zero while nothing is listening', () => {
    const off: Window = { ...LISTENING, listening: false };
    expect(bucketReach(bucket(5 * WIDTH), WIDTH, off)).toBe('not-listening');
  });

  it('does not pretend a bucket in the future is quiet', () => {
    expect(bucketReach(bucket(11 * WIDTH), WIDTH, LISTENING)).toBe('future');
  });
});

describe('the histogram the chart draws', () => {
  const buckets = [
    bucket(0, { 3: 1 }),
    bucket(WIDTH),
    bucket(2 * WIDTH, { 3: 4, 4: 2, 6: 10 }, 3),
    bucket(3 * WIDTH),
    bucket(4 * WIDTH, { 7: 40 }),
  ];

  it('takes the bucket width from the answer, not from the request', () => {
    const view = histogramView(buckets, undefined, LISTENING);
    expect(view.width).toBe(WIDTH);
    expect(view.peak).toBe(40);
  });

  it('stacks the severities in band order and keeps the lines that said nothing', () => {
    const view = histogramView(buckets, undefined, LISTENING);
    const bar = view.bars[2];
    expect(bar?.segments).toEqual([
      { band: 'error', count: 4, kept: 4, hidden: 0 },
      { band: 'warning', count: 2, kept: 2, hidden: 0 },
      { band: 'notice', count: 10, kept: 10, hidden: 0 },
      { band: 'unsaid', count: 3, kept: 3, hidden: 0 },
    ]);
    expect(view.counts).toEqual({ total: 60, shown: 60, lessSevere: 0, noSeverity: 0 });
  });

  it('accounts for every line a severity filter hides, in the two ways it hides them', () => {
    const view = histogramView(buckets, 3, LISTENING);
    // 5 error lines kept; 2 warnings + 10 info + 40 debug are less severe; 3
    // lines never said how severe they were, and SQL drops those too.
    expect(view.counts).toEqual({ total: 60, shown: 5, lessSevere: 52, noSeverity: 3 });
  });

  it('splits a band the filter only half keeps', () => {
    // "Critical and worse" keeps 0…2 and drops the error lines of the same
    // band, so the band cannot be shown as wholly kept or wholly hidden.
    const view = histogramView([bucket(2 * WIDTH, { 2: 1, 3: 6 })], 2, LISTENING);
    expect(view.bars[0]?.segments).toEqual([{ band: 'error', count: 7, kept: 1, hidden: 6 }]);
    expect(view.counts.shown).toBe(1);
    expect(view.counts.lessSevere).toBe(6);
  });

  it('groups neighbouring unprovable buckets into one gap, and leaves the zeros alone', () => {
    const view = histogramView(buckets, undefined, LISTENING);
    // Bucket 1 is before the oldest stored line; bucket 3 is a real zero.
    expect(view.gaps).toEqual([{ from: WIDTH, until: 2 * WIDTH, reach: 'before-store', buckets: 1 }]);
    expect(view.bars[3]?.reach).toBe('counted');
    expect(gapNote(view, LISTENING)).toContain('1 bucket is drawn as a gap, not as a zero');
    expect(gapNote(view, LISTENING)).toContain('older than the oldest line still stored');
  });

  it('says nothing about gaps when every bucket is accounted for', () => {
    const view = histogramView([bucket(2 * WIDTH), bucket(3 * WIDTH, { 5: 1 })], undefined, LISTENING);
    expect(view.gaps).toEqual([]);
    expect(gapNote(view, LISTENING)).toBeNull();
  });

  it('does not count a bucket that has not happened yet as a hole in the data', () => {
    const view = histogramView([bucket(20 * WIDTH)], undefined, LISTENING);
    expect(view.gaps[0]?.reach).toBe('future');
    expect(gapNote(view, LISTENING)).toBeNull();
  });

  it('reads the window out of logs.status, so one answer decides every bucket', () => {
    expect(windowOf(status(), 99)).toEqual({
      now: 99,
      oldestAt: 2 * WIDTH,
      rows: 400,
      listening: true,
    });
    expect(windowOf(undefined, 99).listening).toBe(false);
  });
});

describe('what "nothing here" means', () => {
  it('knows the receiver is switched off, and says a port will be opened', () => {
    const verdict = receiverVerdict(status({ enabled: false, listening: [], received: 0, rows: 0 }));
    expect(verdict.kind).toBe('off');
    expect(verdict.setup).toBe(true);
    expect(verdict.detail).toContain('opens a listening port');
  });

  it('passes on the backend’s own reason when the socket could not be opened', () => {
    const verdict = receiverVerdict(
      status({ listening: [], error: 'udp 0.0.0.0:514: permission denied', received: 0, rows: 0 }),
    );
    expect(verdict.kind).toBe('failed');
    expect(verdict.error).toBe('udp 0.0.0.0:514: permission denied');
  });

  it('tells a quiet network from one that was never told where to send', () => {
    const silent = receiverVerdict(status({ received: 0, rows: 0 }));
    expect(silent.kind).toBe('silent');
    expect(silent.setup).toBe(true);
    expect(silent.title).toContain('udp 0.0.0.0:5514');

    const fed = receiverVerdict(status({ received: 12, rows: 400 }));
    expect(fed.kind).toBe('receiving');
    expect(fed.setup).toBe(false);
  });

  it('separates "nothing has ever arrived" from "nothing since the restart"', () => {
    // The counters start at zero with the process; what is stored does not.
    const verdict = receiverVerdict(status({ received: 0, rows: 400 }));
    expect(verdict.kind).toBe('stale');
    expect(verdict.detail).toContain('earlier run');
  });

  it('blames the filter, not the setup, when the range holds lines the filter hides', () => {
    const fed = receiverVerdict(status());
    expect(emptyReason(fed, { narrowed: true, rangeTotal: 120 })).toBe('filter');
    expect(emptyReason(fed, { narrowed: false, rangeTotal: 0 })).toBe('range');
    const off = receiverVerdict(status({ enabled: false, listening: [], received: 0, rows: 0 }));
    expect(emptyReason(off, { narrowed: false, rangeTotal: 0 })).toBe('setup');
    // A filtered-out line is still a line: the setup is not the problem.
    expect(emptyReason(off, { narrowed: true, rangeTotal: 9 })).toBe('filter');
  });
});
