import { describe, expect, test } from 'vitest';
import { fromLocalInput, toLocalInput, windowState } from './datetime';

describe('typing a time for a maintenance window', () => {
  test('a time survives the round trip through the input', () => {
    const at = new Date(2026, 4, 17, 23, 30, 0, 0).getTime();
    expect(toLocalInput(at)).toBe('2026-05-17T23:30');
    expect(fromLocalInput(toLocalInput(at))).toBe(at);
  });

  test('the input is read in the browser time zone, not as UTC', () => {
    const at = fromLocalInput('2026-01-02T03:04');
    expect(at).not.toBeNull();
    const when = new Date(at ?? 0);
    expect(when.getHours()).toBe(3);
    expect(when.getMinutes()).toBe(4);
  });

  test('anything that is not a time is refused rather than guessed', () => {
    expect(fromLocalInput('')).toBeNull();
    expect(fromLocalInput('tomorrow')).toBeNull();
    expect(fromLocalInput('2026-13-45T99:99')).not.toBe(0);
  });
});

describe('where a window stands', () => {
  const base = { starts_at: 1_000, ends_at: 2_000, recurring: null };

  test('open, still to come, and over are three different things', () => {
    expect(windowState(base, 1_500)).toBe('open');
    expect(windowState(base, 500)).toBe('upcoming');
    expect(windowState(base, 5_000)).toBe('past');
  });

  test('a weekly window that has passed is still coming round again', () => {
    expect(windowState({ ...base, recurring: 'weekly' }, 5_000)).toBe('upcoming');
  });
});
