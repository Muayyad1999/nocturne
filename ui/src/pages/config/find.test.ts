import { describe, expect, it } from 'vitest';
import { findInText, matchAt, splitMatches } from './find';

const TEXT = 'interface Vlan30\n description Guest\n no ip address\ninterface Vlan40\n';

describe('findInText', () => {
  it('finds every occurrence, ignoring case by default', () => {
    const { ranges } = findInText(TEXT, 'vlan');
    expect(ranges).toHaveLength(2);
    expect(TEXT.slice(ranges[0]!.from, ranges[0]!.to)).toBe('Vlan');
  });

  it('matches case when asked', () => {
    expect(findInText(TEXT, 'vlan', { caseSensitive: true }).ranges).toHaveLength(0);
    expect(findInText(TEXT, 'Vlan', { caseSensitive: true }).ranges).toHaveLength(2);
  });

  it('treats the query as text unless regex is asked for', () => {
    expect(findInText('a.b axb', 'a.b').ranges).toHaveLength(1);
    expect(findInText('a.b axb', 'a.b', { regex: true }).ranges).toHaveLength(2);
  });

  it('reports a broken pattern instead of throwing', () => {
    const result = findInText(TEXT, 'inter(face', { regex: true });
    expect(result.ranges).toHaveLength(0);
    expect(result.error).toBeTruthy();
  });

  it('matches nothing for an empty query', () => {
    expect(findInText(TEXT, '').ranges).toHaveLength(0);
  });

  it('cannot be made to loop by a pattern that matches nothing at all', () => {
    const result = findInText('abc', 'x*', { regex: true });
    expect(result.ranges).toHaveLength(0);
  });

  it('advances past a zero-width alternative', () => {
    const result = findInText('aaa', 'a*', { regex: true });
    expect(result.ranges).toEqual([{ from: 0, to: 3 }]);
  });
});

describe('matchAt', () => {
  it('wraps around in both directions', () => {
    const ranges = [
      { from: 0, to: 1 },
      { from: 5, to: 6 },
    ];
    expect(matchAt(ranges, 0)).toEqual(ranges[0]);
    expect(matchAt(ranges, 2)).toEqual(ranges[0]);
    expect(matchAt(ranges, -1)).toEqual(ranges[1]);
    expect(matchAt([], 3)).toBeUndefined();
  });
});

describe('splitMatches', () => {
  it('splits a line into matched and unmatched parts in order', () => {
    expect(splitMatches('vlan 30 vlan 40', 'vlan')).toEqual([
      { text: 'vlan', match: true },
      { text: ' 30 ', match: false },
      { text: 'vlan', match: true },
      { text: ' 40', match: false },
    ]);
  });

  it('returns the whole line when nothing matches', () => {
    expect(splitMatches('hostname SW1', 'vlan')).toEqual([{ text: 'hostname SW1', match: false }]);
  });

  it('never loses a character', () => {
    const line = 'snmp-server community <secret:2:weak> RO';
    const parts = splitMatches(line, 'secret');
    expect(parts.map((part) => part.text).join('')).toBe(line);
  });
});
