import { describe, expect, it } from 'vitest';
import type { Coverage, LinkCapacity, UsageFigure, UsedAgainstSold } from '@/api/types/isp';
import {
  basisText,
  capacityBasisText,
  capacityText,
  coverageTally,
  coverageWords,
  isMeasured,
  linkCoverageNote,
  loadText,
  ofPlanText,
  ofPlanTone,
  ratioTone,
  readingTime,
  soldText,
  usageScopeText,
  usageText,
} from './coverage';

const ALL: Coverage[] = [
  'measured',
  'no_serving_device',
  'no_serving_interface',
  'device_missing',
  'monitor_unavailable',
  'interface_never_collected',
  'no_rate_yet',
  'no_recent_reading',
  'no_samples_in_window',
  'monitor_failed',
];

function figure(over: Partial<UsageFigure> = {}): UsageFigure {
  return {
    in_mbit: 3.4,
    out_mbit: 1.2,
    used_mbit: 3.4,
    basis: 'the greater of the two directions',
    at: 1_000,
    window: null,
    peak_mbit: 4.1,
    samples: null,
    speed_mbit: 1000,
    ...over,
  };
}

function row(over: Partial<UsedAgainstSold> = {}): UsedAgainstSold {
  return {
    customer_id: 'c1',
    customer_name: 'Zagros Net',
    state: 'active',
    sold_mbit: 10,
    subscriptions: 1,
    device_id: 'd1',
    interface: 'ether5',
    used: null,
    coverage: 'interface_never_collected',
    coverage_note: 'monitoring has never recorded that interface on that device',
    of_plan_pct: null,
    ...over,
  };
}

function link(over: Partial<LinkCapacity> = {}): LinkCapacity {
  return {
    link_id: 'l1',
    link_name: 'Uplink A',
    tower: 'Tower A',
    device_id: null,
    interface: null,
    capacity_mbit: 100,
    capacity_basis: 'declared',
    sold_mbit: 300,
    suspended_mbit: 0,
    customers: 12,
    suspended_customers: 0,
    ratio: 3,
    ratio_text: '3.0:1',
    measured: null,
    coverage: 'no_serving_device',
    coverage_note: 'no serving device is recorded for this customer',
    note: 'both figures, not a verdict',
    ...over,
  };
}

describe('a customer with no usage figure', () => {
  it('is told about in words, with no digit that could read as a usage of zero', () => {
    for (const coverage of ALL.filter((entry) => entry !== 'measured')) {
      const text = usageText(row({ coverage, used: null }));
      expect(text, coverage).not.toMatch(/\d/);
      expect(text, coverage).not.toBe('—');
      expect(text.length, coverage).toBeGreaterThan(8);
    }
  });

  it('keeps the ten reasons apart instead of calling them all unknown', () => {
    const labels = new Set(ALL.map((coverage) => coverageWords(coverage).label));
    expect(labels.size).toBe(ALL.length);
    // The label fits a column; the sentence beside it says what it means, and
    // the two that are most often confused say so in words.
    expect(coverageWords('interface_never_collected').meaning).toContain('nobody has looked');
    expect(coverageWords('no_rate_yet').meaning).toContain('a gap, not a zero');
    // A reading from before the usage window is what monitoring last saw,
    // which is neither a figure nor a zero.
    expect(coverageWords('no_recent_reading').meaning).toContain('not what is passing now');
    expect(coverageWords('no_recent_reading').meaning).toContain('not a usage of zero');
    for (const coverage of ALL) {
      expect(coverageWords(coverage).meaning.length, coverage).toBeGreaterThan(20);
    }
  });

  it('shows the figure, and only the figure, when there is one', () => {
    expect(isMeasured('measured')).toBe(true);
    expect(isMeasured('no_rate_yet')).toBe(false);
    expect(usageText(row({ coverage: 'measured', used: figure() }))).toBe('3.4 Mb/s');
    // A measured row that somehow has no figure still must not read as zero.
    expect(usageText(row({ coverage: 'measured', used: null }))).toBe('Measured');
  });

  it('leads with the reasons there is no figure, not with the figures', () => {
    const tally = coverageTally([
      { coverage: 'measured', customers: 40, note: 'measured' },
      { coverage: 'no_serving_device', customers: 2, note: 'no device' },
      { coverage: 'monitor_failed', customers: 1, note: 'refused' },
    ]);
    expect(tally.map((entry) => entry.coverage)).toEqual([
      'monitor_failed',
      'no_serving_device',
      'measured',
    ]);
    expect(tally[0]?.label).toBe('Monitoring refused');
    expect(tally[0]?.meaning).toContain('refused');
  });
});

describe('what was sold, and the share of it', () => {
  it('says a customer is on no plan rather than selling them nothing', () => {
    expect(soldText(null)).toBe('No active subscription');
    expect(soldText(10)).toBe('10 Mb/s');
  });

  it('has no percentage when one of the two numbers does not exist', () => {
    expect(ofPlanText(null)).toBe('no figure');
    expect(ofPlanTone(null)).toBe('idle');
    // 140% and 4% are both worth knowing, and neither is an alarm.
    expect(ofPlanText(140)).toBe('140%');
    expect(ofPlanTone(140)).toBe('warn');
    expect(ofPlanText(4)).toBe('4%');
    expect(ofPlanTone(4)).toBe('ok');
    expect(ofPlanTone(85)).toBe('info');
  });

  it('says which direction a figure is of and over what', () => {
    expect(basisText(figure())).toContain('latest reading');
    expect(
      basisText(figure({ window: { since: 1, until: 2, rollup: '5m' } })),
    ).toContain('averaged over the window (5m)');
  });

  it('says when a latest reading was taken, on the business’s calendar', () => {
    const at = Date.UTC(2026, 10, 22, 21, 5, 9);
    const reading = figure({ at });
    // Ten in the evening on the 22nd in UTC is one in the morning of the
    // 23rd in Baghdad, which is the day the business is having.
    const sameDay = Date.UTC(2026, 10, 23, 8, 0, 0);
    expect(readingTime(reading, 180, sameDay)).toBe('at 00:05:09');
    expect(readingTime(reading, 0, sameDay)).toBe('at 2026-11-22 21:05:09');
    // Another day's reading carries its date, whatever day the test runs on.
    expect(basisText(figure({ at: Date.UTC(2020, 0, 2, 3, 4, 5) }), 0)).toContain(
      'latest reading at 2020-01-02 03:04:05',
    );
    // An average says its window instead, and an unknown calendar gives no
    // moment rather than the browser's.
    expect(readingTime(figure({ at, window: { since: 1, until: 2, rollup: '5m' } }), 0)).toBeNull();
    expect(readingTime(reading, null)).toBeNull();
    expect(basisText(reading, null)).toBe(`${reading.basis}, latest reading`);
  });
});

describe('a usage report of one page of the customers', () => {
  it('says which of them it is, and that every count is of those', () => {
    const text = usageScopeText({ customers: 200, matching: 900 }, 201);
    expect(text).toContain('customers 201–400 of 900');
    expect(text).toContain('every count here is of these 200');
  });

  it('says nothing more when it is all of the customers the filters match', () => {
    expect(usageScopeText({ customers: 63, matching: 63 }, 1)).toBeNull();
    expect(usageScopeText({ customers: 0, matching: 0 }, 1)).toBeNull();
  });
});

describe('a link that may be oversubscribed', () => {
  it('states both figures, never the ratio alone', () => {
    expect(capacityText(link())).toBe('300 Mb/s sold of 100 Mb/s (3.0:1)');
    expect(capacityText(link({ capacity_mbit: 0, ratio: null, ratio_text: 'no capacity' }))).toBe(
      '300 Mb/s sold, no capacity recorded',
    );
  });

  it('has no ratio colour when no capacity was written down', () => {
    expect(ratioTone(null)).toBe('idle');
    expect(ratioTone(0.6)).toBe('ok');
    expect(ratioTone(1.5)).toBe('info');
    expect(ratioTone(3)).toBe('warn');
    expect(ratioTone(5)).toBe('crit');
  });

  it('says where the capacity figure came from', () => {
    expect(capacityBasisText('declared')).toContain('wrote it down');
    expect(capacityBasisText('interface_speed')).toContain('interface itself');
  });

  it('writes the coverage sentence about the link, not about a customer', () => {
    // `isp.capacity` sends the customer-worded note; on a tower feed it reads
    // "no serving device is recorded for this customer", which is wrong.
    expect(link().coverage_note).toContain('this customer');
    for (const coverage of ALL) {
      expect(linkCoverageNote(coverage), coverage).not.toContain('customer');
    }
    expect(linkCoverageNote('no_serving_device')).toContain('this link');
    expect(linkCoverageNote('no_serving_device')).toContain('not a load of zero');
  });

  it('says in words when a link is not measured, and shows the rate when it is', () => {
    expect(loadText(link())).toBe('No serving device');
    expect(loadText(link({ coverage: 'measured', measured: figure({ used_mbit: 61 }) }))).toBe(
      '61 Mb/s',
    );
  });
});
