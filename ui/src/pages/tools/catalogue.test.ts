import { describe, expect, it } from 'vitest';
import type { Tool, ToolGroup } from '@/api/types/toolbox';
import {
  FAMILY,
  FOLDED,
  FORMED,
  groupPanels,
  kindOf,
  panels,
  selectPanel,
} from './catalogue';
import { SPECS } from './specs';

function tool(id: string, group: ToolGroup, over: Partial<Tool> = {}): Tool {
  return {
    id,
    title: id,
    group,
    description: `what ${id} does`,
    offline: group !== 'probes' && group !== 'device' && group !== 'servers' && group !== 'history',
    job: false,
    ...over,
  };
}

/** A slice of the real catalogue, in the order the backend lists it. */
const CATALOGUE: Tool[] = [
  tool('tools.subnet', 'addressing', { title: 'Subnet calculator' }),
  tool('tools.mtu', 'converters', { title: 'MTU and MSS', description: 'TCP MSS that follows' }),
  tool('tools.password', 'secrets', { title: 'Password generator' }),
  tool('tools.reference', 'reference', { title: 'Reference tables' }),
  tool('tools.reference.tables', 'reference', { title: 'Which tables there are' }),
  tool('probe.ping', 'probes', { title: 'Ping', job: true }),
  tool('probe.dns', 'probes', { title: 'DNS lookup' }),
  tool('probe.device.ping', 'device', { title: 'Ping from a device' }),
  tool('probe.history', 'history', { title: 'What was run' }),
  tool('probe.history.get', 'history', { title: 'One run' }),
  tool('probe.history.delete', 'history', { title: 'Forget runs' }),
  tool('tftp.start', 'servers', { title: 'TFTP server', job: true }),
  tool('tftp.stop', 'servers', { title: 'Stop the TFTP server' }),
  tool('tftp.status', 'servers', { title: 'TFTP status' }),
];

describe('what kind of thing a tool is', () => {
  it('calls a password generator offline but not a calculator, so it is not run on every keystroke', () => {
    expect(kindOf(tool('tools.subnet', 'addressing'))).toBe('calculator');
    expect(kindOf(tool('tools.password', 'secrets'))).toBe('generator');
    expect(kindOf(tool('tools.secret.encode', 'secrets'))).toBe('generator');
    expect(kindOf(tool('tools.keys.generate', 'secrets', { offline: false }))).toBe('generator');
  });

  it('separates a probe that streams from one that answers in the reply', () => {
    expect(kindOf(tool('probe.ping', 'probes', { job: true }))).toBe('job');
    expect(kindOf(tool('probe.dns', 'probes'))).toBe('probe');
  });

  it('puts the three families on the three different frames', () => {
    expect(FAMILY[kindOf(tool('tools.subnet', 'addressing'))]).toBe('offline');
    expect(FAMILY[kindOf(tool('probe.dns', 'probes'))]).toBe('network');
    expect(FAMILY[kindOf(tool('probe.device.ping', 'device'))]).toBe('standing');
    expect(FAMILY[kindOf(tool('tftp.start', 'servers'))]).toBe('standing');
  });

  it('marks a tool the engine has and this screen has no form for, rather than hiding it', () => {
    expect(kindOf(tool('tools.something.new', 'addressing'))).toBe('unformed');
  });
});

describe('folding the methods that are one panel', () => {
  it('leaves one row for a server and one for the history, with the methods they use', () => {
    const list = panels(CATALOGUE);
    expect(list.map((panel) => panel.tool.id)).toEqual([
      'tools.subnet',
      'tools.mtu',
      'tools.password',
      'tools.reference',
      'probe.ping',
      'probe.dns',
      'probe.device.ping',
      'probe.history',
      'tftp.start',
    ]);
    expect(list.find((panel) => panel.tool.id === 'tftp.start')?.uses).toEqual([
      'tftp.stop',
      'tftp.status',
    ]);
    expect(list.find((panel) => panel.tool.id === 'probe.history')?.uses).toEqual([
      'probe.history.get',
      'probe.history.delete',
    ]);
  });

  it('drops a folded method whose panel this build does not have, rather than promoting it', () => {
    const list = panels([tool('tftp.status', 'servers')]);
    expect(list).toEqual([]);
  });

  it('keeps the catalogue as the only list: a tool absent from it is absent from the screen', () => {
    const list = panels(CATALOGUE.filter((entry) => entry.id !== 'probe.ping'));
    expect(list.some((panel) => panel.tool.id === 'probe.ping')).toBe(false);
  });
});

describe('the filter', () => {
  it('finds a tool by a word in its description, not only in its title', () => {
    const found = groupPanels(panels(CATALOGUE), 'mss');
    expect(found.flatMap((group) => group.entries.map((entry) => entry.panel.tool.id))).toEqual([
      'tools.mtu',
    ]);
  });

  it('matches a description as text, not as a subsequence, so a filter narrows', () => {
    // `dns` appears in no description here; as a subsequence it would match
    // "Which tables there are" (D-N-S scattered through it) and half the list.
    const found = groupPanels(panels(CATALOGUE), 'dns');
    expect(found.flatMap((group) => group.entries.map((entry) => entry.panel.tool.id))).toEqual([
      'probe.dns',
    ]);
  });

  it('still matches a title loosely, so an abbreviation finds the tool', () => {
    const found = groupPanels(panels(CATALOGUE), 'subcalc');
    expect(found[0]?.entries[0]?.panel.tool.id).toBe('tools.subnet');
  });

  it('keeps the groups in their fixed order while nothing is typed', () => {
    const groups = groupPanels(panels(CATALOGUE), '').map((group) => group.group);
    expect(groups).toEqual([...groups].sort(byDrawerOrder));
  });

  it('puts the group with the best match first once something is typed', () => {
    // "Converters" sorts after "Addressing" in the drawer, but somebody who
    // types `mss` is looking for one tool, not browsing a shelf.
    const found = groupPanels(panels(CATALOGUE), 'mss');
    expect(found[0]?.group).toBe('converters');
    expect(found[0]?.entries[0]?.panel.tool.id).toBe('tools.mtu');
  });

  it('returns everything, in catalogue order, for an empty query', () => {
    const found = groupPanels(panels(CATALOGUE), '   ');
    expect(found.flatMap((group) => group.entries.length > 0)).not.toContain(false);
    expect(found.find((group) => group.group === 'addressing')?.label).toBe('Addressing');
  });

  it('marks the characters it matched, for the highlight', () => {
    const [first] = groupPanels(panels(CATALOGUE), 'subnet').flatMap((group) => group.entries);
    expect(first?.indices.length).toBeGreaterThan(0);
  });
});

const DRAWER = [
  'addressing',
  'identity',
  'secrets',
  'converters',
  'reference',
  'text',
  'probes',
  'device',
  'servers',
  'history',
];
const byDrawerOrder = (a: string, b: string) => DRAWER.indexOf(a) - DRAWER.indexOf(b);

describe('which panel the route names', () => {
  it('opens the tool in the address, so a link to one is a link to that one', () => {
    expect(selectPanel(panels(CATALOGUE), 'probe.dns')?.tool.id).toBe('probe.dns');
  });

  it('falls back to the first tool when the address names one this build does not have', () => {
    expect(selectPanel(panels(CATALOGUE), 'probe.nothing')?.tool.id).toBe('tools.subnet');
    expect(selectPanel(panels(CATALOGUE), undefined)?.tool.id).toBe('tools.subnet');
  });
});

describe('the screen covers the whole drawer', () => {
  it('has a form for every tool it claims to have a form for', () => {
    const missing = [...FORMED].filter((id) => SPECS[id] === undefined);
    expect(missing).toEqual([]);
  });

  it('claims no form for a method that is only ever folded into another panel', () => {
    const folded = Object.keys(FOLDED).filter((id) => FORMED.has(id));
    expect(folded).toEqual([]);
  });

  it('has a form for every tool the real backend catalogue offers', () => {
    // The ids of `crates/nocturne-toolbox/src/catalogue.rs`, which a Rust test
    // keeps in step with the methods that crate registers. A tool added there
    // without one here shows as "no form yet" on screen; this test is what
    // makes that a decision rather than an oversight.
    const backend = [
      'tools.subnet',
      'tools.subnet.split',
      'tools.vlsm',
      'tools.supernet',
      'tools.contains',
      'tools.overlaps',
      'tools.ip.range',
      'tools.ip.convert',
      'tools.wildcard',
      'tools.wildcard.acl',
      'tools.mac',
      'tools.oui',
      'tools.oid',
      'tools.interface',
      'tools.secret.decode',
      'tools.secret.encode',
      'tools.secret.identify',
      'tools.password',
      'tools.keys.generate',
      'tools.convert',
      'tools.transfer_time',
      'tools.mtu',
      'tools.reference',
      'tools.reference.tables',
      'tools.regex',
      'tools.diff',
      'tools.hash',
      'tools.base64',
      'probe.ping',
      'probe.traceroute',
      'probe.ports',
      'probe.sweep',
      'probe.dns',
      'probe.whois',
      'probe.tls',
      'probe.http',
      'probe.ntp',
      'probe.local',
      'probe.wol',
      'probe.device.ping',
      'probe.device.traceroute',
      'probe.history',
      'probe.history.get',
      'probe.history.delete',
      'tftp.start',
      'tftp.stop',
      'tftp.status',
      'probe.syslog.start',
      'probe.syslog.stop',
      'probe.syslog.status',
      'probe.syslog.messages',
    ];
    const unformed = backend.filter((id) => !FORMED.has(id) && FOLDED[id] === undefined);
    expect(unformed).toEqual([]);
  });
});
