import { describe, expect, it } from 'vitest';
import type { LogEntry } from '@/api/types/core';
import { severityCell, tagOf, viaOf } from './entry';
import { LAB_LINE, TRAP_ENTRIES } from './fixtures';
import { bindingName, isTrap, omittedNote, trapFacts, trapOf, trapTag } from './trap';

const [V1, V2C_UP] = TRAP_ENTRIES as [LogEntry, LogEntry, LogEntry];

function trap(entry: LogEntry) {
  const view = trapOf(entry);
  if (!view) throw new Error('not a trap');
  return view;
}

function fact(entry: LogEntry, label: string) {
  return trapFacts(trap(entry), entry.source).find((item) => item.label === label);
}

describe('a trap row', () => {
  it('has no severity — which is not "info" and not the dash of a line that gave none', () => {
    expect(V2C_UP.severity).toBeNull();
    expect(severityCell(V2C_UP)).toEqual({ kind: 'none' });
    // A syslog line without a priority is a different thing, shown differently.
    expect(severityCell({ ...LAB_LINE, severity: null })).toEqual({ kind: 'unsaid' });
    expect(severityCell(LAB_LINE)).toEqual({ kind: 'given', severity: 5 });
  });

  it('is tagged with the notification’s name, like a mnemonic', () => {
    expect(tagOf(V2C_UP)).toBe('linkUp');
    expect(tagOf(V1)).toBe('linkDown');
  });

  it('is tagged with its OID when the dictionary has no name for it', () => {
    const unnamed = { ...V2C_UP, body: { ...(V2C_UP.body as object), trap: null, oid: '1.3.6.1.4.1.99999.0.7' } };
    expect(tagOf(unnamed)).toBe('1.3.6.1.4.1.99999.0.7');
    expect(trapTag(trap(unnamed))).toBe('1.3.6.1.4.1.99999.0.7');
  });

  it('is only read as a trap when it is one', () => {
    expect(isTrap(V1)).toBe(true);
    expect(trapOf(LAB_LINE)).toBeNull();
  });
});

describe('a trap’s body, field by field', () => {
  it('reads the notification, the version and the bindings in the order they were sent', () => {
    const view = trap(V2C_UP);
    expect(view).toMatchObject({ version: 'v2c', name: 'linkUp', oid: '1.3.6.1.6.3.1.1.5.4', ifIndex: 3 });
    expect(view.bindings.map((binding) => [binding.name, binding.value])).toEqual([
      ['ifIndex.3', '3'],
      ['ifAdminStatus.3', 'up(1)'],
      ['ifOperStatus.3', 'up(1)'],
    ]);
  });

  it('shows a v1 agent’s address as a claim, beside where the datagram really came from', () => {
    const claimed = fact(V1, 'Agent address (claimed)');
    expect(claimed?.value).toBe('192.0.2.1');
    expect(claimed?.note).toContain('never decides the device');
    expect(fact(V1, 'Sent from')?.value).toBe('127.0.0.1');
    // A v2c trap carries no such claim, and none is shown.
    expect(fact(V2C_UP, 'Agent address (claimed)')?.value).toBeNull();
  });

  it('leaves out what the datagram did not carry rather than filling it in', () => {
    expect(fact(V2C_UP, 'Enterprise')?.value).toBeNull();
    expect(fact(V2C_UP, 'Generic / specific trap')?.value).toBeNull();
    expect(fact(V1, 'Enterprise')?.value).toBe('1.3.6.1.4.1.8072.3.2.10');
    expect(fact(V1, 'Generic / specific trap')?.value).toBe('2 / 0');
  });

  it('numbers an interface the monitor has no name for, and says no name was made up', () => {
    const numbered = fact(V2C_UP, 'Interface');
    expect(numbered?.value).toBe('ifIndex 3');
    expect(numbered?.note).toContain('None is made up');

    const named = {
      ...V2C_UP,
      body: { ...(V2C_UP.body as object), interface: 'GigabitEthernet0/3' },
    };
    expect(fact(named, 'Interface')).toMatchObject({ value: 'GigabitEthernet0/3 (ifIndex 3)', note: undefined });
  });

  it('says an inform is one', () => {
    const inform = { ...V2C_UP, body: { ...(V2C_UP.body as object), inform: true } };
    expect(fact(inform, 'Version')?.value).toBe('SNMPv2c, an inform (acknowledged once stored)');
    expect(fact(V2C_UP, 'Version')?.value).toBe('SNMPv2c');
  });

  it('shows a binding the dictionary cannot name by its OID, and says it has no name', () => {
    expect(bindingName({ oid: '1.3.6.1.4.1.99999.1.9.0', name: null, value: 'x' })).toEqual({
      text: '1.3.6.1.4.1.99999.1.9.0',
      named: false,
    });
    expect(bindingName({ oid: '1.3.6.1.2.1.2.2.1.1.3', name: 'ifIndex.3', value: '3' })).toEqual({
      text: 'ifIndex.3',
      named: true,
    });
  });

  it('keeps a withheld community as the engine wrote it: a placeholder, as text', () => {
    const forwarded = {
      ...V2C_UP,
      body: {
        ...(V2C_UP.body as object),
        varbinds: [{ oid: '1.3.6.1.6.3.18.1.4.0', name: 'snmpTrapCommunity.0', value: '<community>' }],
      },
    };
    expect(trap(forwarded).bindings[0]?.value).toBe('<community>');
  });

  it('counts the bindings that did not fit instead of pretending the row is whole', () => {
    expect(omittedNote(trap(V2C_UP))).toBeNull();
    const huge = { ...V2C_UP, body: { ...(V2C_UP.body as object), varbinds_omitted: 5936 } };
    expect(omittedNote(trap(huge))).toBe(
      '5936 more bindings were in the datagram and did not fit in the row (3 are shown).',
    );
  });

  it('reads a body that is not the shape the receiver writes without throwing or inventing', () => {
    const broken = { ...V2C_UP, body: { varbinds: [{ value: 'no oid' }, 'text', { oid: '1.3', value: 7 }] } };
    const view = trap(broken);
    expect(view.name).toBeNull();
    expect(view.version).toBeNull();
    // Only what is a binding is kept; a value that is not text is not made into one.
    expect(view.bindings).toEqual([{ oid: '1.3', name: null, value: '' }]);
    expect(trap({ ...V2C_UP, body: null }).bindings).toEqual([]);
  });
});

describe('a line a lab handed over', () => {
  it('says it came from a lab, and has no sending address', () => {
    expect(viaOf(LAB_LINE)).toBe('lab');
    expect(LAB_LINE.source).toBe('');
  });

  it('still recognises a lab’s line from a build where the lab wrote it itself', () => {
    const older = { ...LAB_LINE, body: { line: '%SYS-5-RESTART: System restarted', lab_id: 'lab1' } };
    expect(viaOf(older)).toBe('lab');
  });
});
