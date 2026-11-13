import { describe, expect, test } from 'vitest';
import {
  describeSelection,
  EMPTY_SELECTION,
  filterIsOpen,
  fromSelector,
  toSelector,
  type SelectionDraft,
} from './selector';

const draft = (over: Partial<SelectionDraft> = {}): SelectionDraft => ({ ...EMPTY_SELECTION, ...over });

describe('a form becomes a selector', () => {
  test('an empty form asks for every device, which is a real answer', () => {
    expect(filterIsOpen(draft())).toBe(true);
    expect(toSelector(draft())).toEqual({ filter: {} });
  });

  test('only what was set is sent', () => {
    expect(toSelector(draft({ site: 'HQ', role: 'access', query: '  sw ' }))).toEqual({
      filter: { query: 'sw', site: 'HQ', role: 'access' },
    });
  });

  test('the lab choice is three states, not a boolean', () => {
    expect(toSelector(draft({ lab: 'any' })).filter?.lab).toBeUndefined();
    expect(toSelector(draft({ lab: 'only' })).filter?.lab).toBe(true);
    expect(toSelector(draft({ lab: 'none' })).filter?.lab).toBe(false);
  });

  test('named devices alone do not drag the whole inventory in with them', () => {
    // An empty filter means "every device", so it must not be sent beside
    // hand-picked ids: the two combine as a union.
    expect(toSelector(draft({ deviceIds: ['a', 'b'] }))).toEqual({ device_ids: ['a', 'b'] });
    expect(toSelector(draft({ deviceIds: ['a'], site: 'HQ' }))).toEqual({
      device_ids: ['a'],
      filter: { site: 'HQ' },
    });
  });

  test('a cap is only sent when it is a positive number', () => {
    expect(toSelector(draft({ limit: '10' })).filter?.limit).toBe(10);
    expect(toSelector(draft({ limit: 'ten' })).filter?.limit).toBeUndefined();
    expect(toSelector(draft({ limit: '0' })).filter?.limit).toBeUndefined();
  });

  test('exclusions survive the round trip', () => {
    const start = draft({ deviceIds: ['a'], excludeIds: ['b'], profile: 'cisco_ios', status: 'down' });
    expect(fromSelector(toSelector(start))).toEqual(start);
  });

  test('a selector written by hand becomes a form', () => {
    expect(fromSelector({ filter: { lab: false, status: 'up', limit: 5 } })).toEqual(
      draft({ lab: 'none', status: 'up', limit: '5' }),
    );
    expect(fromSelector(undefined)).toEqual(draft());
  });

  test('a status the form does not know is dropped rather than shown wrongly', () => {
    expect(fromSelector({ filter: { status: 'sideways' as 'up' } }).status).toBe('');
  });
});

describe('a selection in words', () => {
  test('describes the filter, not the devices it matches today', () => {
    expect(describeSelection({ filter: { site: 'HQ', role: 'access' } })).toBe(
      'Every device in HQ, with the role access',
    );
    expect(describeSelection({ filter: { lab: true } })).toBe('Every lab device');
    expect(describeSelection({ filter: { lab: false, profile: 'cisco_ios' } })).toBe(
      'Real devices of type cisco_ios',
    );
  });

  test('named devices and a filter are both said', () => {
    expect(describeSelection({ device_ids: ['a', 'b'], filter: { tag: 'critical' } })).toBe(
      '2 named devices, plus every device tagged critical',
    );
    expect(describeSelection({ device_ids: ['a'] })).toBe('1 named device');
  });

  test('a cap and exclusions are said, because they shrink the run', () => {
    expect(describeSelection({ filter: { limit: 5 }, exclude_device_ids: ['x', 'y'] })).toBe(
      'Every device, at most 5, less 2 excluded',
    );
  });

  test('nothing selected says nothing, not "all"', () => {
    expect(describeSelection({})).toBe('Nothing');
    expect(describeSelection(undefined)).toBe('Nothing');
  });
});
