import { describe, expect, it } from 'vitest';
import { formatReading, holeText, recordedAt } from './readings';

describe('a value read out by a chart without a format of its own', () => {
  it('gives each line of an axis from 0 to 0.1 its own label', () => {
    const ticks = [0, 0.02, 0.04, 0.06, 0.08, 0.1].map(formatReading);
    expect(ticks).toEqual(['0', '0.02', '0.04', '0.06', '0.08', '0.1']);
  });

  it('keeps the tenths of an axis from 0 to 1 (an "up" that is one or zero)', () => {
    expect([0, 0.2, 0.4, 0.6000000000000001, 0.8, 1].map(formatReading)).toEqual([
      '0',
      '0.2',
      '0.4',
      '0.6',
      '0.8',
      '1',
    ]);
  });

  it('reads a value under a thousand to three significant digits', () => {
    expect(formatReading(0.3)).toBe('0.3');
    expect(formatReading(37.6)).toBe('37.6');
    expect(formatReading(45.5)).toBe('45.5');
    expect(formatReading(99.5)).toBe('99.5');
    expect(formatReading(123.456)).toBe('123');
    expect(formatReading(0.000123456)).toBe('0.000123');
  });

  it('shortens thousands as the compact format does, including what rounds up to one', () => {
    expect(formatReading(999.5)).toBe('1k');
    expect(formatReading(1234)).toBe('1.2k');
    expect(formatReading(2_500_000)).toBe('2.5M');
  });

  it('writes a negative value with a minus sign, and nothing that rounds to zero with one', () => {
    expect(formatReading(-0.5)).toBe('−0.5');
    expect(formatReading(-1234)).toBe('−1.2k');
    expect(formatReading(-0)).toBe('0');
  });

  it('never writes a gap as a number', () => {
    expect(formatReading(null)).toBe('—');
    expect(formatReading(undefined)).toBe('—');
    expect(formatReading(Number.NaN)).toBe('—');
    expect(formatReading(Number.POSITIVE_INFINITY)).toBe('—');
  });

  it('writes a value below a millionth in exponent form rather than as zero', () => {
    expect(formatReading(1.23e-7)).toBe('1.23e-7');
    expect(formatReading(1e-7)).toBe('1e-7');
  });
});

describe('a time at which nothing was recorded', () => {
  const series = [
    { values: [1, null, null, 3] },
    { values: [null, 2, null, null] },
  ];

  it('is one where no series has a value, not one where some series has none', () => {
    expect([0, 1, 2, 3].map((index) => recordedAt(series, index))).toEqual([true, true, false, true]);
  });

  it('says for how long there were no samples, between the ones either side', () => {
    expect(holeText([0, 1_000, 2_000, 61_000], series, 2)).toBe('1m, no samples');
  });

  it('says only that there are no samples where there is none on one side', () => {
    expect(holeText([0, 1_000], [{ values: [null, 4] }], 0)).toBe('No samples');
  });
});
