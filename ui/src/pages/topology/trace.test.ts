import { describe, expect, it } from 'vitest';
import type { Hop, Link, Node, Trace } from '@/api/types/topology';
import { traceOverlay, viaLabel } from './trace';

function node(id: string, extra: Partial<Node> = {}): Node {
  return {
    id,
    kind: 'switch',
    name: id.toUpperCase(),
    device_id: id,
    lab: false,
    status: 'up',
    unmanaged: false,
    x: 0,
    y: 0,
    pinned: false,
    ...extra,
  };
}

function hop(name: string, extra: Partial<Hop> = {}): Hop {
  return { node: name, via: 'route', detail: '', ...extra };
}

function trace(hops: Hop[], extra: Partial<Trace> = {}): Trace {
  return { from: 'a', to: 'b', hops, complete: true, notes: [], ...extra };
}

const nodes = [
  node('ds1', { name: 'DS1', x: 0, y: 0 }),
  node('edge', { name: 'EDGE', x: 0, y: 400 }),
  node('as1', { name: 'AS1', x: 600, y: 400 }),
];

const links: Link[] = [
  {
    id: 'l1',
    a: { node: 'ds1', interface: 'Gi1/0/1' },
    b: { node: 'edge', interface: 'Gi0/1' },
    kind: 'cdp',
    confidence: 'confirmed',
    status: 'up',
  },
  {
    id: 'l2',
    a: { node: 'ds1', interface: 'Gi1/0/2' },
    b: { node: 'edge', interface: 'Gi0/2' },
    kind: 'cdp',
    confidence: 'confirmed',
    status: 'up',
  },
];

describe('a path drawn on the graph', () => {
  it('has nothing to draw when nothing has been traced', () => {
    expect(traceOverlay(undefined, nodes, links)).toBeNull();
  });

  it('numbers the hops from one and matches them to the nodes by device id', () => {
    const overlay = traceOverlay(
      trace([hop('DS1', { device_id: 'ds1' }), hop('EDGE', { device_id: 'edge' })]),
      nodes,
      links,
    );
    expect(overlay?.steps.map((step) => [step.index, step.node?.id])).toEqual([
      [1, 'ds1'],
      [2, 'edge'],
    ]);
    expect([...(overlay?.onPath ?? [])]).toEqual(['ds1', 'edge']);
  });

  /**
   * Two cables join the same pair of switches. Highlighting the wrong one
   * would say the traffic went somewhere it did not.
   */
  it('highlights the cable the hops named, not just any cable between the two', () => {
    const overlay = traceOverlay(
      trace([
        hop('DS1', { device_id: 'ds1', out_interface: 'Gi1/0/2' }),
        hop('EDGE', { device_id: 'edge', in_interface: 'Gi0/2' }),
      ]),
      nodes,
      links,
    );
    expect([...(overlay?.usedLinks ?? [])]).toEqual(['l2']);
  });

  it('draws the step even when no drawn link joins the two hops', () => {
    const overlay = traceOverlay(
      trace([hop('EDGE', { device_id: 'edge' }), hop('AS1', { device_id: 'as1' })]),
      nodes,
      links,
    );
    expect(overlay?.segments).toHaveLength(1);
    expect(overlay?.segments[0]?.linkId).toBeUndefined();
    expect(overlay?.usedLinks.size).toBe(0);
  });

  it('does not invent a node for a hop the drawing does not contain', () => {
    const overlay = traceOverlay(
      trace([hop('DS1', { device_id: 'ds1' }), hop('203.0.113.1')]),
      nodes,
      links,
    );
    expect(overlay?.steps[1]?.node).toBeUndefined();
    // Nothing is drawn between a node and something that is not on the drawing.
    expect(overlay?.segments).toHaveLength(0);
  });

  it('ends an incomplete walk in a stub that carries the backend’s reason', () => {
    const overlay = traceOverlay(
      trace([hop('DS1', { device_id: 'ds1' }), hop('EDGE', { device_id: 'edge' })], {
        complete: false,
        stopped_because: 'the next hop 203.0.113.1 is not an address of any device Nocturne manages',
      }),
      nodes,
      links,
    );
    expect(overlay?.gap?.text).toContain('203.0.113.1');
    // The stub hangs off the last hop, pointing away from where the walk came from.
    expect(overlay?.gap?.to.y).toBeGreaterThan(overlay?.gap?.from.y ?? 0);
  });

  it('draws no stub for a walk that arrived', () => {
    const overlay = traceOverlay(
      trace([hop('DS1', { device_id: 'ds1' })], { complete: true }),
      nodes,
      links,
    );
    expect(overlay?.gap).toBeUndefined();
  });

  it('draws no stub when the walk stopped somewhere that is not on the drawing', () => {
    const overlay = traceOverlay(
      trace([hop('10.9.9.9')], { complete: false, stopped_because: 'no route' }),
      nodes,
      links,
    );
    expect(overlay?.gap).toBeUndefined();
  });

  it('matches a hop that is a host by the address it is drawn with', () => {
    const host = node('h:1', { name: '10.10.30.100', address: '10.10.30.100', device_id: undefined });
    const overlay = traceOverlay(trace([hop('10.10.30.100')]), [...nodes, host], links);
    expect(overlay?.steps[0]?.node?.id).toBe('h:1');
  });
});

describe('how a hop was worked out', () => {
  it('says it in words a person reads, for every way the backend has', () => {
    expect(['connected', 'route', 'arp', 'mac'].map((via) => viaLabel(via as Hop['via']))).toEqual([
      'directly connected',
      'routing table',
      'ARP table',
      'MAC table',
    ]);
  });
});
