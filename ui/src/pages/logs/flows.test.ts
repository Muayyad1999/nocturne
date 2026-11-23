import { describe, expect, it } from 'vitest';
import type { FlowTop } from '@/api/types/logs';
import { EXPORTERS, TOP_BY_INTERFACE, TOP_BY_SOURCE, TOP_ONE_EXPORTER } from './fixtures';
import {
  DEFAULT_PERIOD,
  coverageLines,
  emptyKind,
  hasOther,
  isFlowBy,
  parseScope,
  periodOf,
  ratesPhrase,
  samplingNote,
  scopeChoices,
  scopeValue,
  shareOf,
  topParams,
  versionLabel,
} from './flows';

const EMPTY: FlowTop = {
  rows: [],
  total: { bytes: 0, packets: 0, flows: 0 },
  other: { bytes: 0, packets: 0, flows: 0 },
  coverage: {
    collecting: false,
    window_start: 1_795_403_580_000,
    until: 1_795_404_526_478,
    since: null,
    records: 0,
    sets_without_template: 0,
    sampling: [],
    sampled_records: 0,
    exporters: 0,
  },
};

function line(top: FlowTop, id: string) {
  return coverageLines(top.coverage).find((entry) => entry.id === id);
}

describe('an empty answer is one of three things', () => {
  it('is "not collecting", not "no traffic", while the collector is not listening', () => {
    expect(emptyKind(EMPTY)).toBe('not-collecting');
    expect(line(EMPTY, 'collecting')).toMatchObject({ value: 'Not listening', tone: 'warn' });
    expect(line(EMPTY, 'collecting')?.note).toContain('not “no traffic”');
  });

  it('is a quiet period when the collector is listening and holds nothing', () => {
    const quiet = { ...EMPTY, coverage: { ...EMPTY.coverage, collecting: true } };
    expect(emptyKind(quiet)).toBe('nothing-in-window');
    expect(line(quiet, 'since')?.value).toBe('Nothing in this window');
  });

  it('is "no row of this kind" when the period holds records and none under such a key', () => {
    const noRows = { ...TOP_BY_SOURCE, rows: [] };
    expect(emptyKind(noRows)).toBe('no-rows');
    expect(emptyKind(TOP_BY_SOURCE)).toBeNull();
  });
});

describe('sampled figures are labelled, not multiplied', () => {
  it('states the rate the exporter said beside the table', () => {
    const note = samplingNote(TOP_BY_SOURCE.coverage);
    expect(note?.label).toBe('Sampled 1 in 100');
    expect(note?.sentence).toContain('3 of the 4 records in this answer');
    expect(note?.sentence).toContain('they are not multiplied here');
    expect(note?.all).toBe(false);
  });

  it('leaves the bytes exactly as the exporter sent them', () => {
    // 49 940 bytes were exported for 10.0.0.5, 25 060 of them by an exporter
    // sampling 1 in 100. Nothing in this module produces a larger figure.
    const row = TOP_BY_SOURCE.rows[0];
    expect(row?.bytes).toBe(49_940);
    expect(shareOf(row ?? TOP_BY_SOURCE.total, TOP_BY_SOURCE.total, 'source')).toBeCloseTo(49_940 / 59_340);
    expect(JSON.stringify(coverageLines(TOP_BY_SOURCE.coverage))).not.toContain('4 994 000');
  });

  it('says nothing about sampling when no exporter said it samples', () => {
    expect(samplingNote(TOP_ONE_EXPORTER.coverage)).toBeNull();
    expect(line(TOP_ONE_EXPORTER, 'sampling')).toBeUndefined();
  });

  it('says when every record in the answer was sampled', () => {
    const all = samplingNote({ ...TOP_BY_SOURCE.coverage, sampled_records: 4 });
    expect(all?.all).toBe(true);
    expect(all?.sentence).toContain('Every record in this answer');
  });

  it('spells several rates out', () => {
    expect(ratesPhrase([100])).toBe('1 in 100');
    expect(ratesPhrase([100, 1000])).toBe('1 in 100 and 1 in 1 000');
    expect(ratesPhrase([10, 100, 1000])).toBe('1 in 10, 1 in 100, 1 in 1 000');
  });
});

describe('what an answer covers', () => {
  it('says how many data sets could not be read, and that they are in no row and not in the total', () => {
    const templates = line(TOP_BY_SOURCE, 'templates');
    expect(templates).toMatchObject({ value: '1', tone: 'warn' });
    expect(templates?.note).toContain('in no row and not in the total');
    expect(line(TOP_ONE_EXPORTER, 'templates')).toBeUndefined();
  });

  it('says that two exporters are added up', () => {
    expect(line(TOP_BY_SOURCE, 'exporters')).toMatchObject({ value: '2' });
    expect(line(TOP_BY_SOURCE, 'exporters')?.note).toContain('reported by both');
    expect(line(TOP_ONE_EXPORTER, 'exporters')?.note).toBeUndefined();
  });

  it('says when the period reaches back further than what is held', () => {
    // Asked about 15 minutes; the first minute that holds anything is the last one.
    expect(line(TOP_BY_SOURCE, 'since')?.note).toContain('covers less than was asked for');
    const whole = {
      ...TOP_BY_SOURCE,
      coverage: { ...TOP_BY_SOURCE.coverage, since: TOP_BY_SOURCE.coverage.window_start },
    };
    expect(line(whole, 'since')?.note).toBeUndefined();
  });

  it('gives the record count as the engine counted it', () => {
    expect(line(TOP_BY_SOURCE, 'records')?.value).toBe('4');
  });
});

describe('other and total, as the engine gives them', () => {
  it('works out a share only where a row is a part of the total', () => {
    const row = TOP_BY_INTERFACE.rows[0];
    // By interface every flow is counted in and out: the rows are twice the total.
    const sum = TOP_BY_INTERFACE.rows.reduce((bytes, entry) => bytes + entry.bytes, 0);
    expect(sum).toBe(2 * TOP_BY_INTERFACE.total.bytes);
    expect(shareOf(row ?? TOP_BY_INTERFACE.total, TOP_BY_INTERFACE.total, 'interface')).toBeNull();
    expect(shareOf({ bytes: 5, packets: 1, flows: 1 }, EMPTY.total, 'source')).toBeNull();
  });

  it('tells an "other" that holds nothing from one that holds something', () => {
    expect(hasOther(TOP_BY_SOURCE.other)).toBe(false);
    expect(hasOther({ bytes: 0, packets: 0, flows: 3 })).toBe(true);
  });
});

describe('whose traffic', () => {
  it('asks for one device, one exporter by its address, or everything', () => {
    expect(topParams('source', 15, { kind: 'all' })).toEqual({ by: 'source', minutes: 15, limit: 20 });
    expect(topParams('dscp', 60, { kind: 'device', id: 'kq4kwzyscmgnnvdt' })).toEqual({
      by: 'dscp',
      minutes: 60,
      limit: 20,
      device_id: 'kq4kwzyscmgnnvdt',
    });
    expect(topParams('source', 5, { kind: 'exporter', address: '127.0.0.2' })).toMatchObject({
      exporter: '127.0.0.2',
    });
  });

  it('offers an exporter no device claims by its address, which is the only way to look at it', () => {
    expect(scopeChoices(EXPORTERS)).toEqual([
      { value: '', label: 'Every exporter' },
      { value: 'exporter:127.0.0.2', label: '127.0.0.2 (no device claims it)' },
      { value: 'device:kq4kwzyscmgnnvdt', label: 'CORE1' },
    ]);
  });

  it('offers a device once however many addresses it exports from', () => {
    const [, core] = EXPORTERS;
    if (!core) throw new Error('fixture');
    const twice = scopeChoices([core, { ...core, address: '127.0.0.9' }]);
    expect(twice.filter((choice) => choice.value.startsWith('device:'))).toHaveLength(1);
  });

  it('round-trips a scope through the address', () => {
    for (const value of ['', 'device:abc', 'exporter:10.0.0.1', 'exporter:2001:db8::1']) {
      expect(scopeValue(parseScope(value))).toBe(value);
    }
    expect(parseScope('device:')).toEqual({ kind: 'all' });
    expect(parseScope('nonsense')).toEqual({ kind: 'all' });
    expect(parseScope(null)).toEqual({ kind: 'all' });
  });

  it('reads the way of looking and the period from the address, and falls back when they are not ones', () => {
    expect(isFlowBy('conversation')).toBe(true);
    expect(isFlowBy('port')).toBe(false);
    expect(isFlowBy(null)).toBe(false);
    expect(periodOf('60')).toBe(60);
    expect(periodOf('2000')).toBe(DEFAULT_PERIOD);
    expect(periodOf(null)).toBe(DEFAULT_PERIOD);
  });

  it('names what an exporter speaks, and shows a number it does not know as the number', () => {
    expect(versionLabel(5)).toBe('NetFlow v5');
    expect(versionLabel(9)).toBe('NetFlow v9');
    expect(versionLabel(10)).toBe('IPFIX');
    expect(versionLabel(7)).toBe('version 7');
  });
});
