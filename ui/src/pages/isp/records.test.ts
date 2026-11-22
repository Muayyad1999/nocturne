import { describe, expect, it } from 'vitest';
import type { Link } from '@/api/types/isp';
import {
  AGREED_NOTE,
  AMEND_NOTE,
  linkInput,
  parseBands,
  parseWholeNumber,
  PERIOD_WINDOW,
  PERIOD_WORDS,
  STATE_WORDS,
  termsText,
  UNIT_WORDS,
  type LinkForm,
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

describe('saving a link', () => {
  const stored: Link = {
    id: 'l1',
    name: 'Uplink A',
    description: 'The feed to Tower A',
    device_id: null,
    interface: null,
    tower: 'Tower A',
    capacity_mbit: 100,
    cost_per_mbit: { currency: 'USD', minor: 400, digits: 2, text: '4.00' },
    cost_period: 'monthly',
    created_at: 1,
    updated_at: 1,
  };
  const form = (over: Partial<LinkForm> = {}): LinkForm => ({
    name: 'Uplink A',
    description: 'The feed to Tower A',
    tower: 'Tower A',
    deviceId: '',
    iface: '',
    capacity: '100',
    cost: { currency: 'USD', minor: 400 },
    costPeriod: 'monthly',
    ...over,
  });

  it('says outright that an emptied cost is to be taken off the link', () => {
    // Leaving `cost_per_mbit` out keeps the stored 4.00 — the dialog said
    // "updated", reopened with the old figure, and the margin went on
    // subtracting a cost that had been removed.
    const input = linkInput(form({ cost: null }), stored);
    expect(input.clear_cost).toBe(true);
    expect('cost_per_mbit' in input).toBe(false);
  });

  it('says outright that an emptied capacity is to be taken off the link', () => {
    const input = linkInput(form({ capacity: '  ' }), stored);
    expect(input.capacity_mbit).toBe(0);
  });

  it('sends a figure that is still there as that figure, and never both a cost and its removal', () => {
    const input = linkInput(form({ capacity: '250', cost: { currency: 'USD', minor: 350 } }), stored);
    expect(input.capacity_mbit).toBe(250);
    expect(input.cost_per_mbit).toEqual({ currency: 'USD', minor: 350 });
    expect('clear_cost' in input).toBe(false);
    expect(input.id).toBe('l1');
  });

  it('clears nothing on a new link, or on one that never had the figure', () => {
    const fresh = linkInput(form({ capacity: '', cost: null }), null);
    expect('clear_cost' in fresh).toBe(false);
    expect('capacity_mbit' in fresh).toBe(false);
    expect('id' in fresh).toBe(false);
    const bare = linkInput(form({ capacity: '', cost: null }), {
      ...stored,
      capacity_mbit: 0,
      cost_per_mbit: null,
    });
    expect('clear_cost' in bare).toBe(false);
    expect('capacity_mbit' in bare).toBe(false);
  });
});

describe('a setting typed as text', () => {
  it('is a whole number in range, or it is not saved as something else', () => {
    expect(parseWholeNumber('14', 0, 3650)).toBe(14);
    expect(parseWholeNumber(' 0 ', 0, 3650)).toBe(0);
    for (const bad of ['', '-1', '1.5', '14 days', '1,000', '3651', '1e3']) {
      expect(parseWholeNumber(bad, 0, 3650), bad).toBeNull();
    }
  });

  it('reads the arrears bands as rising whole days, and refuses a list the engine would rearrange', () => {
    expect(parseBands('30, 60, 90')).toEqual([30, 60, 90]);
    expect(parseBands('7,14')).toEqual([7, 14]);
    expect(parseBands('45')).toEqual([45]);
    for (const bad of ['', '0, 30', '60, 30', '30, 30', '30, x', '30; 60', '1.5']) {
      expect(parseBands(bad), bad).toBeNull();
    }
  });
});
