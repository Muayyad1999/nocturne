import { describe, expect, it } from 'vitest';
import {
  AGREED_NOTE,
  AMEND_NOTE,
  PERIOD_WINDOW,
  PERIOD_WORDS,
  STATE_WORDS,
  termsText,
  UNIT_WORDS,
} from './records';

describe('what an account state is called', () => {
  it('has a word, a meaning and a consequence for each of the three', () => {
    for (const state of ['active', 'suspended', 'cancelled'] as const) {
      expect(STATE_WORDS[state].label.length, state).toBeGreaterThan(3);
      expect(STATE_WORDS[state].meaning.length, state).toBeGreaterThan(20);
      expect(STATE_WORDS[state].consequence.length, state).toBeGreaterThan(20);
    }
  });

  it('never says that marking an account will stop the service', () => {
    // `nocturne-isp` has no write path to a device at all, and the interface
    // must not imply one: `tests/boundaries.rs` holds the backend to this and
    // this test holds the words to it.
    const sentences = Object.values(STATE_WORDS).flatMap((words) => [
      words.meaning,
      words.consequence,
    ]);
    for (const sentence of sentences) {
      expect(sentence.toLowerCase(), sentence).not.toMatch(
        /disconnect|cut off|cuts them off|will stop the service|shuts? (them|it) (off|down)|block(s|ed)? (their|the) (traffic|service)/,
      );
    }
  });

  it('says what suspending really does, which is to write it down', () => {
    expect(STATE_WORDS.suspended.consequence).toContain('records a decision');
    expect(STATE_WORDS.suspended.consequence).toContain('somebody changes the router');
    expect(STATE_WORDS.active.consequence).toContain('Nothing is sent to a device');
    expect(STATE_WORDS.cancelled.meaning).toContain('invoices and payments stay');
  });
});

describe('what a plan says', () => {
  it('writes the terms the subscription itself holds', () => {
    expect(termsText({ mbit: 20, unit: 'per_mbit', period: 'monthly' })).toBe(
      '20 Mbit/s, per megabit, a month',
    );
    expect(termsText({ mbit: 50, unit: 'flat', period: 'yearly' })).toBe('50 Mbit/s, flat, a year');
  });

  it('knows what a whole period of each kind is, as the engine requires', () => {
    expect(PERIOD_WINDOW.monthly).toContain('28 to 31');
    expect(PERIOD_WINDOW.quarterly).toContain('89 to 92');
    expect(PERIOD_WINDOW.yearly).toContain('365 or 366');
    expect(Object.keys(PERIOD_WORDS)).toEqual(['monthly', 'quarterly', 'yearly']);
    expect(Object.keys(UNIT_WORDS)).toEqual(['per_mbit', 'flat']);
  });

  it('says why a tariff change does not move what somebody pays', () => {
    expect(AGREED_NOTE).toContain('own copy');
    expect(AMEND_NOTE).toContain('already issued');
  });
});
