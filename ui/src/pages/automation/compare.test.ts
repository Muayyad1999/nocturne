import { describe, expect, test } from 'vitest';
import type { Comparison, CompareGroup, DiffLine } from '@/api/types/automation';
import { comparableCommands, diffCount, diffSummary, headline, interestingLines } from './compare';

const group = (names: string[], diff: DiffLine[] = []): CompareGroup => ({
  devices: names.map((name) => ({ device_id: name, name })),
  output: 'output',
  diff,
});

const comparison = (over: Partial<Comparison> = {}): Comparison => ({
  command: 'show version',
  devices: 3,
  missing: [],
  groups: [group(['a', 'b', 'c'])],
  ignored_patterns: ['(?i)uptime is'],
  ...over,
});

describe('the headline of a comparison', () => {
  test('agreement is stated but not celebrated', () => {
    const result = headline(comparison());
    expect(result.title).toBe('All 3 agree');
    expect(result.unanimous).toBe(true);
    // Fifty devices can agree on the wrong answer: no green tick.
    expect(result.tone).toBe('info');
  });

  test('the odd ones out are counted', () => {
    const result = headline(
      comparison({
        devices: 50,
        groups: [group(Array.from({ length: 47 }, (_v, i) => `sw${i}`)), group(['x', 'y', 'z'], [{ op: '+', text: 'a' }])],
      }),
    );
    expect(result.title).toBe('47 say one thing, 3 differ');
    expect(result.detail).toContain('Two different answers across 50 devices');
    expect(result.tone).toBe('warn');
  });

  test('every device saying something different is its own sentence', () => {
    const result = headline(
      comparison({ devices: 3, groups: [group(['a']), group(['b']), group(['c'])] }),
    );
    expect(result.title).toBe('All 3 answers differ');
    expect(result.detail).toContain('No two devices printed the same thing');
  });

  test('a majority of one still reads as English', () => {
    const result = headline(
      comparison({ devices: 4, groups: [group(['a']), group(['b']), group(['c', 'd'])] }),
    );
    // The backend sorts the largest first; a hand-made answer that does not
    // must still read properly.
    expect(result.title).toBe('1 says one thing, 3 differ');
  });

  test('one odd device is singular', () => {
    expect(headline(comparison({ devices: 4, groups: [group(['a', 'b', 'c']), group(['d'])] })).title).toBe(
      '3 say one thing, 1 differs',
    );
  });

  test('three answers say how many there are', () => {
    expect(
      headline(comparison({ devices: 6, groups: [group(['a', 'b', 'c']), group(['d', 'e']), group(['f'])] })).detail,
    ).toContain('3 different answers');
  });

  test('devices with no output are mentioned, never silently dropped', () => {
    const result = headline(
      comparison({
        devices: 2,
        groups: [group(['a', 'b'])],
        missing: [{ device_id: 'c', name: 'c', state: 'unreached', reason: 'no route' }],
      }),
    );
    expect(result.detail).toContain('1 device in this run had no output to compare');
  });

  test('a command nothing answered says so', () => {
    const result = headline(comparison({ devices: 0, groups: [] }));
    expect(result.title).toBe('Nothing to compare');
    expect(result.tone).toBe('idle');
  });
});

describe('how much differs', () => {
  const diff: DiffLine[] = [
    { op: '=', text: 'same 1' },
    { op: '-', text: 'theirs' },
    { op: '+', text: 'ours' },
    { op: '=', text: 'same 2' },
  ];

  test('counted by operation', () => {
    expect(diffCount(diff)).toEqual({ added: 1, removed: 1, same: 2 });
  });

  test('summarised in words', () => {
    expect(diffSummary(diff)).toBe('2 lines differ: 1 only here, 1 only in the majority');
    expect(diffSummary([{ op: '+', text: 'x' }])).toBe('Every line differs');
    expect(diffSummary([{ op: '=', text: 'x' }])).toBe('No difference outside the ignored lines');
    expect(diffSummary([])).toBe('No difference outside the ignored lines');
  });

  test('only the lines that matter, with a little context', () => {
    const long: DiffLine[] = [
      ...Array.from({ length: 10 }, (_v, i): DiffLine => ({ op: '=', text: `line ${i}` })),
      { op: '+', text: 'here' },
      ...Array.from({ length: 10 }, (_v, i): DiffLine => ({ op: '=', text: `line ${i + 11}` })),
    ];
    const { lines, hidden } = interestingLines(long, 2);
    expect(lines.map((line) => line.text)).toEqual(['line 8', 'line 9', 'here', 'line 11', 'line 12']);
    expect(hidden).toBe(16);
  });

  test('an identical answer has nothing interesting in it', () => {
    expect(interestingLines([{ op: '=', text: 'x' }])).toEqual({ lines: [], hidden: 1 });
  });
});

describe('which commands can be compared', () => {
  test('the ones the run sent, once each, trimmed', () => {
    expect(comparableCommands(['show version', ' show version ', '', 'show clock'])).toEqual([
      'show version',
      'show clock',
    ]);
  });
});
