import { describe, expect, it } from 'vitest';
import type { Confidence, Graph, Link, LinkKind, Node } from '@/api/types/topology';
import {
  NODE_H,
  NODE_W,
  bows,
  dataAge,
  drawableGroups,
  deviceIdsOf,
  edgeExit,
  evidence,
  facetsOf,
  fitName,
  groupBounds,
  labelPorts,
  linkGeometry,
  linkSource,
  nodeKindLabel,
  overlapping,
  positionsOf,
  sceneBounds,
  shortInterface,
} from './graph';

function node(id: string, extra: Partial<Node> = {}): Node {
  return {
    id,
    kind: 'switch',
    name: id.toUpperCase(),
    lab: false,
    status: 'up',
    unmanaged: false,
    x: 0,
    y: 0,
    pinned: false,
    ...extra,
  };
}

function link(id: string, a: string, b: string, extra: Partial<Link> = {}): Link {
  return {
    id,
    a: { node: a },
    b: { node: b },
    kind: 'lldp',
    confidence: 'confirmed',
    status: 'up',
    ...extra,
  };
}

describe('how a link is known', () => {
  it('says that both ends reported a confirmed link, and names the protocols', () => {
    const how = evidence(link('l1', 'a', 'b', { protocols: ['lldp', 'cdp'] }));
    expect(how.confidence).toBe('confirmed');
    expect(how.source).toBe('reported');
    expect(how.basis).toBe('LLDP and CDP');
    expect(how.claim).toContain('Both ends reported');
    expect(how.confidenceNote).toBe('');
    // Nothing to warn about, so nothing is written on the line.
    expect(how.chip).toBe('');
  });

  it('never says which end reported a one-sided link, because the backend does not say', () => {
    const how = evidence(link('l2', 'a', 'b', { confidence: 'one_sided', protocols: ['cdp'] }));
    expect(how.chip).toBe('one end only');
    expect(how.claim).toContain('Only one end reported');
    expect(how.claim).not.toMatch(/\bA\b|\bB\b|\ba reported\b/);
  });

  it('calls a subnet link derived, not reported', () => {
    const how = evidence(link('l3', 'a', 'b', { kind: 'subnet', confidence: 'inferred' }));
    expect(how.source).toBe('derived');
    expect(how.chip).toBe('inferred');
    expect(how.basis).toBe('two interfaces in one subnet');
    expect(how.claim).toContain('Neither end reported the other');
  });

  it('calls a MAC-table link derived from a MAC-table entry', () => {
    const how = evidence(link('l4', 'a', 'h:1', { kind: 'mac', confidence: 'inferred' }));
    expect(how.basis).toContain('MAC-table entry');
  });

  /**
   * The backend gives a drawn link `confidence: confirmed`, because the person
   * who drew it is sure. The screen must not let that read as "both devices
   * reported it".
   */
  it('does not let a link a person drew read as one the devices confirmed', () => {
    const how = evidence(link('l5', 'a', 'm:isp', { kind: 'manual', confidence: 'confirmed' }));
    expect(how.source).toBe('drawn');
    expect(how.chip).toBe('drawn by hand');
    expect(how.claim).toContain('A person drew this link');
    expect(how.claim).toContain('No device reported it');
    // The backend's word is `confirmed`; on its own it would read as the
    // devices having confirmed it, so it is never shown on its own.
    expect(how.confidence).toBe('confirmed');
    expect(how.confidenceNote).toBe('the person who drew it, not the devices');
  });

  it('marks lab wiring as the lab’s own, not as a neighbour report', () => {
    const how = evidence(link('l6', 'a', 'b', { kind: 'lab' }));
    expect(how.source).toBe('lab');
    expect(how.chip).toBe('lab wiring');
    expect(how.confidenceNote).toContain('the lab');
  });

  it('maps every link kind to a source', () => {
    const kinds: LinkKind[] = ['lldp', 'cdp', 'mndp', 'subnet', 'lab', 'manual', 'mac', 'unknown'];
    expect(kinds.map(linkSource)).toEqual([
      'reported',
      'reported',
      'reported',
      'derived',
      'lab',
      'drawn',
      'derived',
      'reported',
    ]);
  });

  it('gives every confidence a chip except the one that needs no caveat', () => {
    const confidences: Confidence[] = ['confirmed', 'one_sided', 'inferred'];
    const chips = confidences.map((confidence) => evidence(link('l', 'a', 'b', { confidence })).chip);
    expect(chips).toEqual(['', 'one end only', 'inferred']);
  });
});

describe('geometry', () => {
  it('leaves a card at its edge, never from its centre', () => {
    const exit = edgeExit({ x: 0, y: 0 }, { x: 1000, y: 0 });
    expect(exit.x).toBeCloseTo(NODE_W / 2 + 2);
    expect(exit.y).toBe(0);
    const up = edgeExit({ x: 0, y: 0 }, { x: 0, y: -1000 });
    expect(up.y).toBeCloseTo(-(NODE_H / 2 + 2));
  });

  it('draws a straight line when nothing is in the way', () => {
    const geometry = linkGeometry({ x: 0, y: 0 }, { x: 0, y: 400 });
    expect(geometry.path).toMatch(/^M .* L /);
    expect(geometry.bow).toBe(0);
    expect(geometry.mid.y).toBeCloseTo(200);
  });

  it('bows a curved line away from the straight one and puts its label on the curve', () => {
    const geometry = linkGeometry({ x: 0, y: 0 }, { x: 0, y: 400 }, 40);
    expect(geometry.path).toMatch(/Q /);
    // The midpoint of a quadratic is half way to the control point.
    expect(Math.abs(geometry.mid.x)).toBeCloseTo(40);
  });

  it('draws a stack cable as a loop rather than a line of zero length', () => {
    const geometry = linkGeometry({ x: 100, y: 100 }, { x: 100, y: 100 });
    expect(geometry.path).toMatch(/^M .* C /);
    expect(geometry.a).not.toEqual(geometry.b);
  });

  it('fans parallel links apart so two cables do not look like one', () => {
    const positions = new Map([
      ['a', { x: 0, y: 0 }],
      ['b', { x: 0, y: 400 }],
    ]);
    const result = bows([link('l1', 'a', 'b'), link('l2', 'a', 'b')], positions);
    expect(result.get('l1')).toBe(-10);
    expect(result.get('l2')).toBe(10);
    expect(result.get('l1')).not.toBe(result.get('l2'));
  });

  it('bows a line that would otherwise pass straight through a card it is not connected to', () => {
    const positions = new Map([
      ['a', { x: 0, y: 0 }],
      ['b', { x: 0, y: 800 }],
      // Directly between the two, which is exactly the lie to avoid.
      ['c', { x: 0, y: 400 }],
    ]);
    const result = bows([link('l1', 'a', 'b')], positions);
    expect(Math.abs(result.get('l1') ?? 0)).toBeGreaterThan(NODE_H / 2);
  });

  it('leaves a line alone when the other cards are nowhere near it', () => {
    const positions = new Map([
      ['a', { x: 0, y: 0 }],
      ['b', { x: 0, y: 800 }],
      ['c', { x: 900, y: 400 }],
    ]);
    expect(bows([link('l1', 'a', 'b')], positions).get('l1')).toBe(0);
  });

  it('measures the scene from the cards, not from their centres', () => {
    const box = sceneBounds([node('a'), node('b', { x: 400, y: 200 })]);
    expect(box).not.toBeNull();
    expect(box?.x).toBeLessThan(-NODE_W / 2);
    expect(box?.width).toBeGreaterThan(400 + NODE_W);
  });

  it('has no scene at all when there is nothing to draw', () => {
    expect(sceneBounds([])).toBeNull();
  });

  it('draws a group outline around every one of its members', () => {
    const box = groupBounds([node('a'), node('b', { x: 400, y: 0 })]);
    expect(box?.x).toBeLessThan(-NODE_W / 2);
    expect((box?.x ?? 0) + (box?.width ?? 0)).toBeGreaterThan(400 + NODE_W / 2);
  });
});

describe('port labels', () => {
  it('writes an interface the short way a network engineer does', () => {
    expect(shortInterface('GigabitEthernet1/0/11')).toBe('Gi1/0/11');
    expect(shortInterface('TenGigabitEthernet1/1/1')).toBe('Te1/1/1');
    expect(shortInterface('FastEthernet0/3')).toBe('Fa0/3');
    expect(shortInterface('Port-channel1')).toBe('Po1');
    expect(shortInterface('Vlan30')).toBe('Vl30');
  });

  it('leaves a name it does not know alone rather than mangling it', () => {
    expect(shortInterface('sfp-sfpplus1')).toBe('sfp-sfpplus1');
    expect(shortInterface('ether1')).toBe('ether1');
    expect(shortInterface(undefined)).toBe('');
  });
});

describe('a name that does not fit its card', () => {
  /**
   * The card clips what overflows, which turned `Campus/Guest-Laptop1` and
   * `Campus/Guest-Laptop2` into two cards both reading `Campus/Guest-Laptop`:
   * two different devices with the same name on them.
   */
  it('cuts in the middle, so two devices that differ at the end still differ', () => {
    const one = fitName('Campus/Guest-Laptop1');
    const other = fitName('Campus/Guest-Laptop2');
    expect(one).toContain('…');
    expect(one).not.toBe(other);
    expect(one.endsWith('1')).toBe(true);
    expect(other.endsWith('2')).toBe(true);
    expect(one.length).toBeLessThan('Campus/Guest-Laptop1'.length);
  });

  it('leaves a name that fits as it is, and cuts one that does not', () => {
    expect(fitName('Campus/DS1')).toBe('Campus/DS1');
    expect(fitName('core-sw1.hq.ex')).toBe('core-sw1.hq.ex');
    expect(fitName('core-sw1.hq.example')).toBe('core-sw…xample');
    expect(fitName('core-sw1.hq.example.net')).toBe('core-sw…le.net');
  });

  it('cuts to what the card can really show', () => {
    // The budget is in characters but the card is in pixels, and the name
    // sits between an icon and a badge. At 19 the cut still overflowed and
    // the card clipped the tail, which put `Campus/Guest…aptop` on both
    // guest laptops — the very thing the middle cut exists to prevent.
    // Checked by looking at the drawing, not by arithmetic.
    for (const name of ['Campus/Guest-Laptop1', 'Campus/Staff-PC1', 'a'.repeat(40)]) {
      expect(fitName(name).length).toBeLessThanOrEqual(14);
    }
  });
});

describe('group outlines', () => {
  const site = { id: 'site:HQ', kind: 'site' as const, label: 'HQ', nodes: ['a', 'b'] };
  const lab = { id: 'lab:1', kind: 'lab' as const, label: 'Lab 1', nodes: ['b', 'a'] };
  const subnet = { id: 'net', kind: 'subnet' as const, label: '10.0.0.0/24', nodes: ['a'] };

  /**
   * The demo campus is both a site and a lab, with the same devices in both.
   * Two outlines on the same rectangle drew their labels on top of each other.
   */
  it('draws one outline for two groups that cover the same nodes, named for both', () => {
    const drawn = drawableGroups([site, lab]);
    expect(drawn).toHaveLength(1);
    expect(drawn[0]?.label).toBe('HQ · Lab 1');
  });

  it('keeps groups that cover different nodes, biggest first', () => {
    const drawn = drawableGroups([subnet, site]);
    expect(drawn.map((group) => group.id)).toEqual(['site:HQ', 'net']);
  });
});

describe('the no-overlap guarantee', () => {
  it('accepts the separation the backend’s layout keeps', () => {
    // The backend lays out in 190 x 64 boxes with a 48px gap.
    expect(overlapping([node('a'), node('b', { x: 238, y: 0 })])).toBeNull();
    expect(overlapping([node('a'), node('b', { x: 0, y: 150 })])).toBeNull();
  });

  it('reports the pair when two cards would be drawn on top of each other', () => {
    const clash = overlapping([node('a'), node('b', { x: 20, y: 10 })]);
    expect(clash?.map((entry) => entry.id)).toEqual(['a', 'b']);
  });
});

describe('saving positions', () => {
  it('sends every node, so moving one does not let the rest be rearranged', () => {
    const nodes = [node('a'), node('b', { x: 400, y: 0 }), node('c', { x: 800, y: 0 })];
    const positions = positionsOf(nodes, { id: 'b', at: { x: 410.4, y: 12.7 } });
    expect(Object.keys(positions)).toEqual(['a', 'b', 'c']);
    expect(positions['b']).toEqual({ x: 410, y: 13, pinned: true });
    expect(positions['a']).toEqual({ x: 0, y: 0, pinned: false });
  });

  it('keeps a node that was already pinned pinned', () => {
    const positions = positionsOf([node('a', { pinned: true })]);
    expect(positions['a']?.pinned).toBe(true);
  });
});

describe('what the drawing says about itself', () => {
  const graph = {
    nodes: [
      node('a', { site: 'HQ', role: 'core', device_id: 'a' }),
      node('b', { site: 'Branch', role: 'access', device_id: 'b' }),
      node('u:switch', { unmanaged: true }),
    ],
    links: [],
    groups: [],
    warnings: [],
    stats: { devices: 2, links: 0, unmanaged: 1, stale_devices: 0, endpoints: 0 },
    generated_at: 0,
    data_age: { oldest_at: 1_000, newest_at: 2_000 },
    layer: 'physical',
    view: 'default',
  } satisfies Graph;

  it('offers only the sites and roles that are in the drawing', () => {
    expect(facetsOf(graph)).toEqual({ sites: ['Branch', 'HQ'], roles: ['access', 'core'] });
  });

  it('collects from the devices it has, and asks nothing of the ones it does not', () => {
    expect(deviceIdsOf(graph)).toEqual(['a', 'b']);
  });

  it('has no age at all when nothing has been collected', () => {
    expect(dataAge({ ...graph, data_age: {} }, 5_000)).toBeNull();
    expect(dataAge(graph, 5_000)).toBe(4_000);
  });

  it('labels ports only while they can be read', () => {
    expect(labelPorts(6)).toBe(true);
    expect(labelPorts(7)).toBe(false);
    expect(labelPorts(undefined)).toBe(true);
  });

  it('names a layer-3 switch the way the backend’s own drawing does', () => {
    expect(nodeKindLabel('l3_switch')).toBe('Layer-3 switch');
    expect(nodeKindLabel('cloud')).toBe('External');
  });
});
