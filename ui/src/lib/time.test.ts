import { describe, expect, it } from 'vitest';
import { EMPTY } from './format';
import {
  formatDate,
  formatDateTime,
  formatRelative,
  formatTime,
  formatTimestamp,
  formatUtcOffset,
  relativeRefreshInterval,
} from './time';

const NOW = Date.UTC(2026, 10, 13, 2, 47, 5);
const SECOND = 1000;
const MINUTE = 60 * SECOND;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

describe('formatRelative', () => {
  it('describes the past in the largest whole unit', () => {
    expect(formatRelative(NOW - 2 * SECOND, NOW)).toBe('just now');
    expect(formatRelative(NOW - 42 * SECOND, NOW)).toBe('42s ago');
    expect(formatRelative(NOW - 5 * MINUTE - 30 * SECOND, NOW)).toBe('5m ago');
    expect(formatRelative(NOW - 3 * HOUR, NOW)).toBe('3h ago');
    expect(formatRelative(NOW - 12 * DAY, NOW)).toBe('12d ago');
    expect(formatRelative(NOW - 65 * DAY, NOW)).toBe('2mo ago');
    expect(formatRelative(NOW - 800 * DAY, NOW)).toBe('2y ago');
  });

  it('describes the future', () => {
    expect(formatRelative(NOW + 5 * MINUTE, NOW)).toBe('in 5m');
    expect(formatRelative(NOW + 2 * SECOND, NOW)).toBe('just now');
  });

  it('renders a dash for a missing timestamp', () => {
    expect(formatRelative(null, NOW)).toBe(EMPTY);
    expect(formatRelative(Number.NaN, NOW)).toBe(EMPTY);
  });
});

describe('relativeRefreshInterval', () => {
  it('refreshes as often as the label can change', () => {
    expect(relativeRefreshInterval(NOW - 10 * SECOND, NOW)).toBe(SECOND);
    expect(relativeRefreshInterval(NOW - 10 * MINUTE, NOW)).toBe(15 * SECOND);
    expect(relativeRefreshInterval(NOW - 10 * HOUR, NOW)).toBe(MINUTE);
  });
});

describe('absolute formatting', () => {
  it('formats UTC date and time ISO-style', () => {
    expect(formatDate(NOW, { utc: true })).toBe('2026-11-13');
    expect(formatTime(NOW, { utc: true })).toBe('02:47:05');
    expect(formatDateTime(NOW, { utc: true })).toBe('2026-11-13 02:47:05');
  });

  it('formats local time from local components', () => {
    const local = new Date(2026, 0, 5, 9, 3, 7).getTime();
    expect(formatDateTime(local)).toBe('2026-01-05 09:03:07');
  });

  it('renders a dash for a missing timestamp', () => {
    expect(formatDateTime(undefined)).toBe(EMPTY);
    expect(formatTimestamp(null)).toBe(EMPTY);
  });

  it('names the local offset', () => {
    const offset = formatUtcOffset(NOW);
    expect(offset).toMatch(/^UTC([+-]\d{2}:\d{2})?$/);
    expect(formatTimestamp(NOW).startsWith(`${formatDateTime(NOW)} ${offset}`)).toBe(true);
  });
});
