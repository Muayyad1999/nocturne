import { describe, expect, it } from 'vitest';
import type { CliKind, IfView, NodeView } from '@/api/types/lab';
import { CABLE_OFF, cableHint, cancelEnd, clickNode, consoleRefusal, defaultInterface } from './cabling';

function port(name: string, link: number | null = null): IfView {
  return {
    name,
    short: name,
    link,
    admin_up: true,
    oper_up: false,
    light: 'off',
    ip: null,
    mode: 'routed',
    description: '',
  };
}

function node(name: string, interfaces: IfView[], cli: CliKind = 'ios'): NodeView {
  return {
    id: 1,
    name,
    hostname: name,
    model: 'router',
    kind: 'router',
    x: 0,
    y: 0,
    powered: true,
    cli,
    interfaces,
    mgmt_ip: null,
  };
}

const R1 = node('R1', [port('GigabitEthernet0/0'), port('GigabitEthernet0/1')]);
const R2 = node('R2', [port('GigabitEthernet0/0')]);
const FULL = node('FULL', [port('Ethernet0', 4)], 'host');

describe('the cable tool', () => {
  it('does nothing while it is off', () => {
    expect(clickNode(CABLE_OFF, R1)).toEqual({ tool: CABLE_OFF });
  });

  it('takes the first end, then asks for the cable on the second', () => {
    const first = clickNode({ mode: 'armed' }, R1);
    expect(first.tool).toEqual({ mode: 'from', node: 'R1' });
    expect(first.connect).toBeUndefined();

    const second = clickNode(first.tool, R2);
    expect(second.connect).toEqual({ a: 'R1', b: 'R2' });
    // The tool stays on, so a second cable is two more clicks.
    expect(second.tool).toEqual({ mode: 'armed' });
  });

  it('refuses a node whose every port is cabled, and says which', () => {
    const step = clickNode({ mode: 'armed' }, FULL);
    expect(step.connect).toBeUndefined();
    expect(step.refused).toContain('FULL has no free interface');
    expect(step.refused).toContain('1 of its ports');
    // The first end is not lost because of a click on a full node.
    expect(step.tool).toEqual({ mode: 'armed' });
  });

  it('refuses a cable from a node to itself', () => {
    const step = clickNode({ mode: 'from', node: 'R1' }, R1);
    expect(step.refused).toBe('A cable needs two different nodes.');
    expect(step.connect).toBeUndefined();
    expect(step.tool).toEqual({ mode: 'from', node: 'R1' });
  });

  it('forgets the first end on a click in open space, but keeps cabling', () => {
    expect(cancelEnd({ mode: 'from', node: 'R1' })).toEqual({ mode: 'armed' });
    expect(cancelEnd({ mode: 'armed' })).toEqual({ mode: 'armed' });
    expect(cancelEnd(CABLE_OFF)).toEqual(CABLE_OFF);
  });

  it('says what it is waiting for', () => {
    expect(cableHint(CABLE_OFF)).toBeNull();
    expect(cableHint({ mode: 'armed' })).toContain('Click the first device');
    expect(cableHint({ mode: 'from', node: 'SW1' })).toContain('Cabling from SW1');
  });
});

describe('the port a cable would take', () => {
  it('is the first free one', () => {
    const subject = node('R1', [port('GigabitEthernet0/0', 2), port('GigabitEthernet0/1')]);
    expect(defaultInterface(subject, 'auto')).toBe('GigabitEthernet0/1');
  });

  it('prefers a Gigabit port for fibre, as the backend does', () => {
    const subject = node('SW1', [port('FastEthernet0/1'), port('GigabitEthernet0/1')]);
    expect(defaultInterface(subject, 'fiber')).toBe('GigabitEthernet0/1');
    expect(defaultInterface(subject, 'copper')).toBe('FastEthernet0/1');
  });

  it('is the console pseudo-port for a console cable, by the kind of command line', () => {
    expect(defaultInterface(R1, 'console')).toBe('Console');
    expect(defaultInterface(node('PC1', [port('Ethernet0')], 'host'), 'console')).toBe('RS232');
    expect(defaultInterface(node('AP1', [port('GigabitEthernet0')], 'none'), 'console')).toBeNull();
  });

  it('is nothing when every port is taken', () => {
    expect(defaultInterface(FULL, 'auto')).toBeNull();
  });
});

describe('consoleRefusal', () => {
  it('explains that a node without a command line has no console port', () => {
    expect(consoleRefusal(node('AP1', [port('GigabitEthernet0')], 'none'))).toContain('no console port');
  });

  it('has nothing to say about a node that has one', () => {
    expect(consoleRefusal(R1)).toBeNull();
  });
});
