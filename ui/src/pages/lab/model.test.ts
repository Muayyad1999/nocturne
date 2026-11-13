import { describe, expect, it } from 'vitest';
import type { IfView, LinkView, NodeView, NoteSpec } from '@/api/types/lab';
import {
  NODE_H,
  NODE_W,
  addressableNodes,
  cableGeometry,
  edgePoint,
  freeInterfaces,
  freeSpot,
  nodeAddress,
  nodeIndex,
  portLabel,
  sceneBounds,
  simClock,
  withLights,
} from './model';

function port(name: string, extra: Partial<IfView> = {}): IfView {
  return {
    name,
    short: name.replace('GigabitEthernet', 'Gi').replace('Ethernet', 'Et'),
    link: null,
    admin_up: true,
    oper_up: false,
    light: 'off',
    ip: null,
    mode: 'routed',
    description: '',
    ...extra,
  };
}

function node(id: number, name: string, extra: Partial<NodeView> = {}): NodeView {
  return {
    id,
    name,
    hostname: name,
    model: 'router',
    kind: 'router',
    x: 0,
    y: 0,
    powered: true,
    cli: 'ios',
    interfaces: [port('GigabitEthernet0/0'), port('GigabitEthernet0/1')],
    mgmt_ip: null,
    ...extra,
  };
}

describe('sceneBounds', () => {
  it('has no bounds for an empty lab, so the viewport does not zoom into nothing', () => {
    expect(sceneBounds([], [])).toBeNull();
  });

  it('covers every node body and its label', () => {
    const bounds = sceneBounds([node(1, 'R1', { x: 100, y: 100 })], []);
    expect(bounds).toEqual({
      x: 100 - NODE_W / 2,
      y: 100 - NODE_H / 2,
      width: NODE_W,
      height: NODE_H + 30,
    });
  });

  it('covers the notes as well, which are wider than the devices', () => {
    const note: NoteSpec = { x: -40, y: -20, w: 600, h: 120, text: 'Access layer', color: '#fff' };
    const bounds = sceneBounds([node(1, 'R1', { x: 100, y: 100 })], [note]);
    expect(bounds?.x).toBe(-40);
    expect(bounds?.y).toBe(-20);
    expect(bounds && bounds.x + bounds.width).toBe(560);
  });
});

describe('edgePoint', () => {
  it('leaves the body at its side when the other node is beside it', () => {
    expect(edgePoint({ x: 0, y: 0 }, { x: 200, y: 0 })).toEqual({ x: NODE_W / 2, y: 0 });
  });

  it('leaves the body at its top when the other node is above it', () => {
    const point = edgePoint({ x: 0, y: 0 }, { x: 0, y: -200 });
    expect(point.x).toBe(0);
    expect(point.y).toBeCloseTo(-NODE_H / 2, 6);
  });

  it('does not divide by zero when two nodes are in the same place', () => {
    const point = edgePoint({ x: 10, y: 10 }, { x: 10, y: 10 });
    expect(Number.isFinite(point.x)).toBe(true);
    expect(Number.isFinite(point.y)).toBe(true);
  });
});

describe('cableGeometry', () => {
  it('puts the lights and the port labels inside the cable, each near its own end', () => {
    const geometry = cableGeometry({ x: 0, y: 0 }, { x: 400, y: 0 });
    expect(geometry.a.x).toBeLessThan(geometry.aLight.x);
    expect(geometry.aLight.x).toBeLessThan(geometry.aLabel.x);
    expect(geometry.bLabel.x).toBeLessThan(geometry.bLight.x);
    expect(geometry.bLight.x).toBeLessThan(geometry.b.x);
  });
});

describe('interfaces', () => {
  it('counts a port as free only when no link is on it', () => {
    const subject = node(1, 'R1', {
      interfaces: [port('GigabitEthernet0/0', { link: 3 }), port('GigabitEthernet0/1')],
    });
    expect(freeInterfaces(subject).map((item) => item.name)).toEqual(['GigabitEthernet0/1']);
  });

  it('labels a port by the short name the device uses', () => {
    const subject = node(1, 'R1');
    expect(portLabel(subject, 'GigabitEthernet0/1')).toBe('Gi0/1');
  });

  it('falls back to the full name when the node is not drawn yet', () => {
    expect(portLabel(undefined, 'GigabitEthernet0/1')).toBe('GigabitEthernet0/1');
  });
});

describe('addresses', () => {
  it('prefers the management address', () => {
    expect(nodeAddress(node(1, 'R1', { mgmt_ip: '10.255.0.1' }))).toBe('10.255.0.1');
  });

  it('falls back to the first addressed interface, without its prefix length', () => {
    const subject = node(1, 'R1', {
      interfaces: [port('GigabitEthernet0/0'), port('GigabitEthernet0/1', { ip: '10.0.0.1/30' })],
    });
    expect(nodeAddress(subject)).toBe('10.0.0.1');
  });

  it('has nothing to say about a node with no address', () => {
    expect(nodeAddress(node(1, 'R1'))).toBeNull();
  });

  it('only offers powered, addressed nodes for a probe, as the backend does', () => {
    const nodes = [
      node(1, 'R1', { mgmt_ip: '10.0.0.1' }),
      node(2, 'R2', { mgmt_ip: '10.0.0.2', powered: false }),
      node(3, 'R3'),
    ];
    expect(addressableNodes(nodes).map((item) => item.name)).toEqual(['R1']);
  });
});

describe('freeSpot', () => {
  it('uses the start when nothing is there', () => {
    expect(freeSpot([])).toEqual({ x: 160, y: 140 });
  });

  it('steps aside when a node already holds the place', () => {
    const spot = freeSpot([node(1, 'R1', { x: 160, y: 140 })]);
    expect(spot).not.toEqual({ x: 160, y: 140 });
  });

  it('wraps onto a second row when the visible area is only two wide', () => {
    const taken = [
      node(1, 'R1', { x: 0, y: 0 }),
      node(2, 'R2', { x: NODE_W + 60, y: 0 }),
    ];
    const spot = freeSpot(taken, { x: 0, y: 0 }, 2);
    expect(spot.x).toBe(0);
    expect(spot.y).toBeGreaterThan(0);
  });

  it('never packs into fewer than two columns, whatever it is given', () => {
    const spot = freeSpot([node(1, 'R1', { x: 0, y: 0 })], { x: 0, y: 0 }, -4);
    expect(spot).toEqual({ x: NODE_W + 60, y: 0 });
  });
});

describe('withLights', () => {
  const link: LinkView = {
    id: 7,
    a: 1,
    a_if: 'GigabitEthernet0/0',
    b: 2,
    b_if: 'Ethernet0',
    cable: 'auto',
    up: false,
    a_light: 'red',
    b_light: 'red',
    bps_ab: 0,
    bps_ba: 0,
  };

  it('keeps the snapshot when no tick has been heard', () => {
    expect(withLights(link, new Map())).toBe(link);
  });

  it('lays the latest tick over the snapshot', () => {
    const lights = new Map([
      [7, { up: true, a_light: 'green' as const, b_light: 'amber' as const, bps_ab: 1200, bps_ba: 0 }],
    ]);
    const live = withLights(link, lights);
    expect(live.up).toBe(true);
    expect(live.a_light).toBe('green');
    expect(live.bps_ab).toBe(1200);
    // Everything the tick does not carry is still the snapshot's.
    expect(live.a_if).toBe('GigabitEthernet0/0');
  });
});

describe('nodeIndex', () => {
  it('finds a node by the id a link refers to', () => {
    const index = nodeIndex([node(1, 'R1'), node(2, 'R2')]);
    expect(index.get(2)?.name).toBe('R2');
  });
});

describe('simClock', () => {
  it('reads simulated microseconds as a clock', () => {
    expect(simClock(0)).toBe('0:00:00');
    expect(simClock(21_250_000)).toBe('0:00:21');
    expect(simClock(3_750_000_000)).toBe('1:02:30');
  });

  it('never shows a negative clock', () => {
    expect(simClock(-5)).toBe('0:00:00');
  });
});
