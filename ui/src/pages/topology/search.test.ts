import { describe, expect, it } from 'vitest';
import type { Node } from '@/api/types/topology';
import { classify, matches, pathReference } from './search';

function node(name: string, extra: Partial<Node> = {}): Node {
  return {
    id: name,
    kind: 'switch',
    name,
    lab: false,
    status: 'up',
    unmanaged: false,
    x: 0,
    y: 0,
    pinned: false,
    ...extra,
  };
}

describe('what the search box does with what is typed', () => {
  it('sends an IPv4 address to locate.host', () => {
    expect(classify('10.10.30.100')).toEqual({
      kind: 'address',
      text: '10.10.30.100',
      address: 'ip',
    });
  });

  it('sends an IPv6 address to locate.host', () => {
    expect(classify('2001:db8::1').kind).toBe('address');
  });

  it('sends a MAC address to locate.host, in any notation', () => {
    for (const text of ['02:4e:43:d1:09:01', '024e.43d1.0901', '024e43d10901']) {
      expect(classify(text), text).toEqual({ kind: 'address', text, address: 'mac' });
    }
  });

  it('treats anything that is not an address as a name, and never guesses', () => {
    expect(classify('core-sw1')).toEqual({ kind: 'name', text: 'core-sw1' });
    expect(classify('10.10.30.')).toEqual({ kind: 'name', text: '10.10.30.' });
    expect(classify('   ')).toEqual({ kind: 'empty' });
  });
});

describe('finding a node by name', () => {
  const nodes = [
    node('AS1', { address: '10.0.0.11' }),
    node('AS10'),
    node('DS1'),
    node('Campus/AS1'),
  ];

  it('prefers the exact name over the one that merely contains it', () => {
    expect(matches(nodes, 'AS1').map((found) => found.name)).toEqual(['AS1', 'AS10', 'Campus/AS1']);
  });

  it('ignores case and surrounding space', () => {
    expect(matches(nodes, '  ds1 ')[0]?.name).toBe('DS1');
  });

  it('finds a node by an address it is drawn with', () => {
    expect(matches(nodes, '10.0.0.11')[0]?.name).toBe('AS1');
  });

  it('finds nothing rather than the first thing when nothing matches', () => {
    expect(matches(nodes, 'nothing-like-this')).toEqual([]);
    expect(matches(nodes, '')).toEqual([]);
  });
});

describe('what a node is called when asking for a path', () => {
  it('uses the device id when there is one, so a renamed device still works', () => {
    expect(pathReference(node('AS1', { device_id: 'abc123' }))).toBe('abc123');
  });

  it('falls back to the name for a node that is not a device', () => {
    expect(pathReference(node('ISP uplink'))).toBe('ISP uplink');
  });
});
