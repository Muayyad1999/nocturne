import { describe, expect, it } from 'vitest';
import {
  EMPTY,
  bytesQuantity,
  formatBytes,
  formatCompact,
  formatCount,
  formatDelta,
  formatDuration,
  formatNumber,
  formatPercent,
  formatRate,
  rateQuantity,
} from './format';

const NBSP = '\u00a0';
const GROUP = '\u2009';
const MINUS = '\u2212';

describe('missing values', () => {
  it('renders an em dash instead of inventing a number', () => {
    for (const format of [
      formatNumber,
      formatCount,
      formatCompact,
      formatBytes,
      formatRate,
      formatPercent,
      formatDuration,
      formatDelta,
    ]) {
      expect(format(null)).toBe(EMPTY);
      expect(format(undefined)).toBe(EMPTY);
      expect(format(Number.NaN)).toBe(EMPTY);
      expect(format(Number.POSITIVE_INFINITY)).toBe(EMPTY);
    }
    expect(bytesQuantity(null)).toEqual({ value: EMPTY, unit: '' });
  });
});

describe('formatCount / formatNumber', () => {
  it('groups digits with a thin space', () => {
    expect(formatCount(0)).toBe('0');
    expect(formatCount(999)).toBe('999');
    expect(formatCount(1204)).toBe(`1${GROUP}204`);
    expect(formatCount(12_345_678)).toBe(`12${GROUP}345${GROUP}678`);
  });

  it('rounds fractions and keeps the fraction ungrouped', () => {
    expect(formatCount(1234.6)).toBe(`1${GROUP}235`);
    expect(formatNumber(12345.678, { digits: 2 })).toBe(`12${GROUP}345.68`);
  });

  it('uses a real minus sign and never prints negative zero', () => {
    expect(formatCount(-42)).toBe(`${MINUS}42`);
    expect(formatCount(-0.2)).toBe('0');
    expect(formatNumber(-1234.5, { digits: 1 })).toBe(`${MINUS}1${GROUP}234.5`);
  });
});

describe('formatCompact', () => {
  it('shortens large counts', () => {
    expect(formatCompact(950)).toBe('950');
    expect(formatCompact(1200)).toBe('1.2k');
    expect(formatCompact(34_000)).toBe('34k');
    expect(formatCompact(5_600_000)).toBe('5.6M');
    expect(formatCompact(2_000_000_000)).toBe('2B');
  });

  it('promotes when rounding reaches the next unit', () => {
    expect(formatCompact(999.6)).toBe('1k');
    expect(formatCompact(999_960)).toBe('1M');
  });
});

describe('formatBytes', () => {
  it('uses binary units', () => {
    expect(formatBytes(0)).toBe(`0${NBSP}B`);
    expect(formatBytes(512)).toBe(`512${NBSP}B`);
    expect(formatBytes(1024)).toBe(`1${NBSP}KiB`);
    expect(formatBytes(1.2 * 1024 ** 3)).toBe(`1.2${NBSP}GiB`);
    expect(formatBytes(153 * 1024 ** 2)).toBe(`153${NBSP}MiB`);
    expect(formatBytes(16 * 1024 ** 3)).toBe(`16${NBSP}GiB`);
  });

  it('never shows 1024 of a unit', () => {
    expect(formatBytes(1023.6)).toBe(`1${NBSP}KiB`);
    expect(formatBytes(1024 ** 2 - 1)).toBe(`1${NBSP}MiB`);
  });

  it('stops at the largest unit', () => {
    expect(formatBytes(5000 * 1024 ** 5)).toBe(`5000${NBSP}PiB`);
  });

  it('exposes value and unit separately', () => {
    expect(bytesQuantity(1536)).toEqual({ value: '1.5', unit: 'KiB' });
  });
});

describe('formatRate', () => {
  it('uses decimal bit units', () => {
    expect(formatRate(0)).toBe(`0${NBSP}b/s`);
    expect(formatRate(56_000)).toBe(`56${NBSP}kb/s`);
    expect(formatRate(940_000_000)).toBe(`940${NBSP}Mb/s`);
    expect(formatRate(1_200_000_000)).toBe(`1.2${NBSP}Gb/s`);
    expect(formatRate(10_000_000_000)).toBe(`10${NBSP}Gb/s`);
    expect(rateQuantity(2_500_000)).toEqual({ value: '2.5', unit: 'Mb/s' });
  });
});

describe('formatPercent', () => {
  it('keeps one decimal only where it matters', () => {
    expect(formatPercent(42.4)).toBe('42%');
    expect(formatPercent(0.34)).toBe('0.3%');
    expect(formatPercent(5)).toBe('5%');
    expect(formatPercent(9.96)).toBe('10%');
    expect(formatPercent(100)).toBe('100%');
  });

  it('accepts ratios and fixed digits', () => {
    expect(formatPercent(0.425, { fromRatio: true })).toBe('43%');
    expect(formatPercent(12.345, { digits: 2 })).toBe('12.35%');
  });

  it('never prints negative zero', () => {
    expect(formatPercent(-0.01)).toBe('0%');
    expect(formatPercent(-3.2)).toBe(`${MINUS}3.2%`);
  });
});

describe('formatDuration', () => {
  it('shows the two most significant units', () => {
    expect(formatDuration(320)).toBe(`320${NBSP}ms`);
    expect(formatDuration(1500)).toBe('1.5s');
    expect(formatDuration(45_000)).toBe('45s');
    expect(formatDuration(12 * 60_000 + 5000)).toBe('12m 5s');
    expect(formatDuration(4 * 3_600_000 + 12 * 60_000)).toBe('4h 12m');
    expect(formatDuration(3 * 86_400_000 + 4 * 3_600_000 + 59 * 60_000)).toBe('3d 4h');
    expect(formatDuration(400 * 86_400_000)).toBe('1y 35d');
  });

  it('drops a zero second unit rather than skipping to a smaller one', () => {
    expect(formatDuration(3 * 86_400_000 + 12 * 60_000)).toBe('3d');
    expect(formatDuration(3_600_000)).toBe('1h');
  });

  it('rounds across unit boundaries', () => {
    expect(formatDuration(59_600)).toBe('1m');
    expect(formatDuration(9960)).toBe('10s');
    expect(formatDuration(999.6)).toBe('1s');
  });

  it('marks negative durations', () => {
    expect(formatDuration(-90_000)).toBe(`${MINUS}1m 30s`);
  });
});

describe('formatDelta', () => {
  it('adds an explicit plus to increases', () => {
    expect(formatDelta(12)).toBe('+12');
    expect(formatDelta(-12)).toBe(`${MINUS}12`);
    expect(formatDelta(0)).toBe('0');
    expect(formatDelta(4.2, formatPercent)).toBe('+4.2%');
  });

  it('does not sign a change that rounds to zero', () => {
    expect(formatDelta(0.01, formatPercent)).toBe('0%');
    expect(formatDelta(0.2)).toBe('0');
  });
});
