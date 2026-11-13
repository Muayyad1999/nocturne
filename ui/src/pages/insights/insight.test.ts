import { describe, expect, it } from 'vitest';
import type { CheckCoverage, Coverage, Insight } from '@/api/types/insights';
import {
  AGEING_MS,
  checkVerdict,
  coverageTotals,
  dismissal,
  freshness,
  groupBySeverity,
  isRepeat,
  reasonTotals,
  severityRank,
  STALE_MS,
  verdict,
  word,
  worstSeverity,
} from './insight';

function row(over: Partial<CheckCoverage> = {}): CheckCoverage {
  return {
    check: 'duplex_mismatch',
    title: 'Duplex mismatch',
    category: 'physical',
    enabled: true,
    evaluated: 3,
    skipped: 0,
    findings: 0,
    reasons: [],
    ...over,
  };
}

function coverage(checks: CheckCoverage[], over: Partial<Coverage> = {}): Coverage {
  return {
    scanned_at: 1_000,
    devices: 3,
    link_source: 'topology',
    links: 2,
    checks,
    missing_sources: [],
    took_ms: 12,
    ...over,
  };
}

function insight(over: Partial<Insight> = {}): Insight {
  return {
    id: 'abc',
    check: 'duplex_mismatch',
    category: 'physical',
    severity: 'high',
    device_id: 'd1',
    device: 'ACC1',
    subject: 'Gi0/1',
    title: 'half duplex against full',
    detail: 'why it matters',
    evidence: [{ label: 'ACC1 Gi0/1', value: 'half duplex' }],
    related: [],
    state: 'open',
    first_seen: 1,
    last_seen: 2,
    lab: false,
    occurrences: 1,
    ...over,
  };
}

describe('the verdict of a check', () => {
  it('tells nothing found, nothing evaluated and switched off apart', () => {
    const clear = row({ evaluated: 3, skipped: 0, findings: 0 });
    const blind = row({ evaluated: 0, skipped: 3, findings: 0 });
    const off = row({ enabled: false, evaluated: 0, skipped: 3 });

    expect(checkVerdict(clear)).toBe('clear');
    expect(checkVerdict(blind)).toBe('not_evaluated');
    expect(checkVerdict(off)).toBe('off');

    // The whole point: three answers, three words, three colours.
    const words = [clear, blind, off].map((entry) => verdict(entry));
    expect(new Set(words.map((entry) => entry.label)).size).toBe(3);
    expect(words.map((entry) => entry.tone)).toEqual(['ok', 'idle', 'idle']);
    expect(words[0]?.label).toBe('Clear');
    expect(words[1]?.label).toBe('Not evaluated');
    expect(words[2]?.label).toBe('Off');
  });

  it('does not call a check clear when it only reached some of the devices', () => {
    const partial = row({ evaluated: 1, skipped: 2, findings: 0 });
    expect(checkVerdict(partial)).toBe('partial');
    expect(verdict(partial).label).toBe('Clear on 1 of 3');
    expect(verdict(partial).tone).toBe('warn');
  });

  it('leads with the findings when it found something, even where it could not look everywhere', () => {
    const found = row({ evaluated: 2, skipped: 1, findings: 1 });
    expect(checkVerdict(found)).toBe('findings');
    expect(verdict(found).label).toBe('1 finding');
    expect(verdict(found).detail).toContain('could not decide for 1 device');
  });
});

describe('coverage totals', () => {
  it('counts answers, not devices, and leaves a switched-off check out of them', () => {
    const totals = coverageTotals(
      coverage([
        row({ check: 'a', evaluated: 3, skipped: 0, findings: 1 }),
        row({ check: 'b', evaluated: 1, skipped: 2 }),
        row({ check: 'c', evaluated: 0, skipped: 3 }),
        row({ check: 'd', enabled: false, evaluated: 0, skipped: 3 }),
      ]),
    );
    expect(totals).toMatchObject({
      checks: 4,
      checksOn: 3,
      checksOff: 1,
      evaluated: 4,
      skipped: 5,
      considered: 9,
      findings: 1,
      checksWithFindings: 1,
      checksClear: 0,
      checksPartly: 1,
      checksNotEvaluated: 1,
    });
  });
});

describe('the reasons an answer is missing', () => {
  it('groups the engine’s own words by how many devices they account for', () => {
    const reasons = reasonTotals(
      coverage([
        row({
          check: 'high_memory',
          skipped: 3,
          evaluated: 0,
          reasons: [{ reason: 'this metric is not polled for the device', devices: 3 }],
        }),
        row({
          check: 'high_temperature',
          skipped: 3,
          evaluated: 0,
          reasons: [{ reason: 'this metric is not polled for the device', devices: 3 }],
        }),
        row({
          check: 'mac_flapping',
          skipped: 2,
          evaluated: 1,
          reasons: [{ reason: 'only one MAC-table collection', devices: 2 }],
        }),
        // Off is a decision, not a gap in the data.
        row({
          check: 'telnet_enabled',
          enabled: false,
          skipped: 3,
          reasons: [{ reason: 'the check is switched off', devices: 3 }],
        }),
      ]),
    );
    expect(reasons.map((entry) => [entry.reason, entry.devices])).toEqual([
      ['this metric is not polled for the device', 6],
      ['only one MAC-table collection', 2],
    ]);
    expect(reasons[0]?.checks.map((check) => check.check)).toEqual([
      'high_memory',
      'high_temperature',
    ]);
  });
});

describe('how much a scan can still be believed', () => {
  it('ages and then goes stale', () => {
    const now = 10 * STALE_MS;
    expect(freshness(now - 1_000, now)).toBe('fresh');
    expect(freshness(now - AGEING_MS, now)).toBe('ageing');
    expect(freshness(now - STALE_MS, now)).toBe('stale');
  });
});

describe('a finding’s history', () => {
  it('marks a problem that came back as a repeat rather than a discovery', () => {
    expect(isRepeat(insight({ occurrences: 1 }))).toBe(false);
    expect(isRepeat(insight({ occurrences: 3 }))).toBe(true);
  });

  it('says when a dismissal has run out, which the ledger still calls dismissed', () => {
    expect(dismissal(insight())).toBeNull();
    expect(dismissal(insight({ state: 'dismissed' }))).toEqual({
      label: 'Dismissed for good',
      expired: false,
    });
    expect(
      dismissal(insight({ state: 'dismissed', dismissed_until: 500 }), 1_000),
    ).toEqual({ label: 'Dismissal has run out', expired: true });
    expect(
      dismissal(insight({ state: 'dismissed', dismissed_until: 2_000 }), 1_000),
    ).toEqual({ label: 'Dismissed', expired: false });
  });
});

describe('ordering', () => {
  it('groups worst first and leaves empty levels out', () => {
    const groups = groupBySeverity([
      insight({ id: '1', severity: 'low' }),
      insight({ id: '2', severity: 'critical' }),
      insight({ id: '3', severity: 'low' }),
    ]);
    expect(groups.map((group) => [group.severity, group.items.length])).toEqual([
      ['critical', 1],
      ['low', 2],
    ]);
  });

  it('ranks critical worst and an unknown severity last', () => {
    expect(severityRank('critical')).toBe(0);
    expect(severityRank('info')).toBe(4);
    expect(severityRank('nonsense' as never)).toBe(5);
  });

  it('takes the worst open severity from the backend’s own counts', () => {
    expect(worstSeverity({ critical: 0, high: 0, medium: 2, low: 1, info: 9 })).toBe('medium');
    expect(worstSeverity({ critical: 0, high: 0, medium: 0, low: 0, info: 0 })).toBeNull();
    expect(worstSeverity(undefined)).toBeNull();
  });
});

describe('words', () => {
  it('gives the bare plural, for sentences that already carry the count', () => {
    expect(word(1, 'device')).toBe('device');
    expect(word(2, 'device')).toBe('devices');
    expect(word(1, 'was', 'were')).toBe('was');
    expect(word(0, 'was', 'were')).toBe('were');
  });
});
