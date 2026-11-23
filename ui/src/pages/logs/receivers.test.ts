import { describe, expect, it } from 'vitest';
import type { LogsStatus } from '@/api/types/logs';
import { SETTINGS, STATUS, STATUS_WITH_QUEUE_BYTES } from './fixtures';
import {
  counter,
  draftOf,
  enablePatch,
  parsePort,
  queueBytes,
  receiverCards,
  settingChange,
  unitFor,
  wouldBind,
  type ReceiverCard,
} from './receivers';

function card(status: LogsStatus, id: ReceiverCard['id']): ReceiverCard {
  const found = receiverCards(status).find((entry) => entry.id === id);
  if (!found) throw new Error(`no ${id} card`);
  return found;
}

function value(found: ReceiverCard, id: string): number | undefined {
  return [...found.counters, ...found.drops, ...found.memory].find((entry) => entry.id === id)?.value;
}

describe('a counter the engine does not give is not shown as zero', () => {
  it('yields nothing for a field the status does not have', () => {
    expect(counter({ received: 3 }, 'rejected', 'Rejected')).toBeNull();
    expect(counter({ received: null }, 'received', 'Received')).toBeNull();
    expect(counter(undefined, 'received', 'Received')).toBeNull();
  });

  it('yields the number that is there, zero included: a measured zero is a measurement', () => {
    expect(counter({ dropped: 0 }, 'dropped', 'Dropped')).toMatchObject({ value: 0 });
  });

  it('shows a syslog receiver from a build without `from_lab` with one row fewer, not a row of nought', () => {
    const { from_lab: _unsaid, ...older } = STATUS.syslog;
    const status = { ...STATUS, syslog: older } as unknown as LogsStatus;
    expect(card(STATUS, 'syslog').counters.map((entry) => entry.id)).toContain('from_lab');
    expect(card(status, 'syslog').counters.map((entry) => entry.id)).not.toContain('from_lab');
  });

  it('gives a receiver that is not in the build no counters at all', () => {
    // What `logs.status` said about traps before the receiver was built.
    const status = {
      ...STATUS,
      traps: { enabled: false, implemented: false, error: 'the SNMP trap receiver is not implemented in this build' },
    } as unknown as LogsStatus;
    const traps = card(status, 'traps');
    expect(traps.state).toBe('absent');
    expect(traps.stateLabel).toBe('Not in this build');
    expect(traps.counters).toEqual([]);
    expect(traps.error).toBe('the SNMP trap receiver is not implemented in this build');
    // Absent is not "off": nothing was measured, so nothing claims "none arrived".
    expect(traps.lastAt).toBeUndefined();
  });
});

describe('each receiver in its own units', () => {
  it('reads the trap receiver’s own counters, and every datagram ends in exactly one of them', () => {
    const traps = card(STATUS, 'traps');
    expect(traps.state).toBe('listening');
    expect(traps.listening).toEqual(['udp 127.0.0.1:38366']);
    expect(value(traps, 'received')).toBe(6);
    const ends = ['stored', 'dropped', 'rejected', 'unauthenticated', 'malformed', 'duplicates'];
    expect(ends.reduce((sum, id) => sum + (value(traps, id) ?? 0), 0)).toBe(6);
    expect(value(traps, 'rejected')).toBe(2);
    expect(value(traps, 'malformed')).toBe(1);
  });

  it('keeps datagrams and flow records apart for the flow collector', () => {
    const flows = card(STATUS, 'flows');
    const unit = (id: string) => flows.counters.find((entry) => entry.id === id)?.unit;
    expect(unit('received')).toBe('datagrams');
    expect(unit('malformed')).toBe('datagrams');
    expect(unit('records')).toBe('flow records');
    expect(unit('counted')).toBe('flow records');
    // It has no `stored`: no flow is.
    expect(flows.counters.map((entry) => entry.id)).not.toContain('stored');
  });

  it('breaks the flow collector’s drops down by reason and reports what its memory holds', () => {
    const flows = card(STATUS, 'flows');
    expect(flows.drops.map((entry) => entry.id)).toEqual(['queue_full', 'no_room', 'unclaimed', 'unreadable']);
    expect(flows.memory.map((entry) => entry.id)).toEqual(['minutes', 'keys', 'max_keys', 'evicted_minutes']);
    expect(value(flows, 'max_keys')).toBe(2_000_000);
    expect(flows.oldestAt).toBe(1795404480000);
    expect(value(flows, 'sets_without_template')).toBe(1);
  });

  it('counts a lab’s lines apart from what came off a socket', () => {
    const syslog = card(STATUS, 'syslog');
    expect(value(syslog, 'received')).toBe(0);
    expect(value(syslog, 'from_lab')).toBe(9);
    // Nothing came off the socket, and the card can say exactly that.
    expect(syslog.lastAt).toBeNull();
  });

  it('carries the engine’s sentence about labs for traps and flows, and none for syslog', () => {
    expect(card(STATUS, 'traps').lab).toBe(
      'A simulated device sends no traps, so nothing counted here came from a lab.',
    );
    expect(card(STATUS, 'flows').lab).toContain('exports no flows');
    expect(card(STATUS, 'syslog').lab).toBeNull();
  });

  it('shows what a queue takes up against its budget, where the engine says', () => {
    for (const id of ['syslog', 'traps', 'flows'] as const) {
      const queue = card(STATUS_WITH_QUEUE_BYTES, id).counters.find((entry) => entry.id === 'queued_bytes');
      expect(queue, id).toMatchObject({ value: 0, format: 'bytes', of: 67_108_864 });
    }
    // The budget is beside the size, not a counter of its own.
    expect(card(STATUS_WITH_QUEUE_BYTES, 'syslog').counters.map((entry) => entry.id)).not.toContain(
      'max_queued_bytes',
    );
  });

  it('shows no queue size at all for a build that bounds its queues in entries only', () => {
    for (const id of ['syslog', 'traps', 'flows'] as const) {
      expect(card(STATUS, id).counters.map((entry) => entry.id)).not.toContain('queued_bytes');
    }
    expect(queueBytes({ queued: 3 }, 'lines')).toBeNull();
    // A size without a budget is still a size.
    expect(queueBytes({ queued_bytes: 512 }, 'lines')).toMatchObject({ value: 512, format: 'bytes' });
    expect(queueBytes({ queued_bytes: 512 }, 'lines')?.of).toBeUndefined();
  });

  it('says one of a unit as one, and nothing of a unit a counter has none of', () => {
    expect(unitFor({ value: 1, unit: 'lines' })).toBe('line');
    expect(unitFor({ value: 1, unit: 'flow records' })).toBe('flow record');
    expect(unitFor({ value: 1, unit: 'data sets' })).toBe('data set');
    expect(unitFor({ value: 0, unit: 'datagrams' })).toBe('datagrams');
    expect(unitFor({ value: 2, unit: 'datagrams' })).toBe('datagrams');
    expect(unitFor({ value: 1 })).toBeNull();
  });
});

describe('off, failed and absent are three states', () => {
  it('tells a receiver that is switched off from one that could not bind', () => {
    const off = { ...STATUS, traps: { ...STATUS.traps, enabled: false, listening: [] } };
    expect(card(off, 'traps')).toMatchObject({ state: 'off', stateLabel: 'Switched off', tone: 'idle' });

    const failed = {
      ...STATUS,
      traps: {
        ...STATUS.traps,
        listening: [],
        error: '`not-an-address` is not an address this machine can listen on (try 0.0.0.0 for every interface)',
      },
    };
    const traps = card(failed, 'traps');
    expect(traps).toMatchObject({ state: 'failed', stateLabel: 'Not listening', tone: 'crit' });
    // The reason is the engine's sentence, whole.
    expect(traps.error).toContain('is not an address this machine can listen on');
  });

  it('says what anybody who can reach the port can do only where the receiver is switched on', () => {
    expect(card(STATUS, 'traps').caution).toContain('whoever can reach this port can forge one');
    expect(card(STATUS, 'flows').caution).toContain('report traffic that never was');
    const off = { ...STATUS, traps: { ...STATUS.traps, enabled: false, listening: [] } };
    expect(card(off, 'traps').caution).toBeNull();
    expect(card(STATUS, 'syslog').caution).toBeNull();
  });
});

describe('the port and the address', () => {
  it('reads a port as a whole number inside what a port can be', () => {
    expect(parsePort('5514')).toEqual({ ok: true, value: 5514 });
    expect(parsePort(' 162 ')).toEqual({ ok: true, value: 162 });
    expect(parsePort('65536').ok).toBe(false);
    expect(parsePort('51.4').ok).toBe(false);
    expect(parsePort('-1').ok).toBe(false);
    expect(parsePort('').ok).toBe(false);
    expect(parsePort('syslog').ok).toBe(false);
  });

  it('takes 0 only where 0 means something', () => {
    expect(parsePort('0').ok).toBe(false);
    expect(parsePort('0', { zero: true })).toEqual({ ok: true, value: 0 });
  });

  it('sends nothing when nothing changed', () => {
    const { patch, errors } = settingChange('traps', draftOf('traps', SETTINGS), SETTINGS);
    expect(patch).toBeNull();
    expect(errors).toEqual({});
  });

  it('sends only the key that changed, so a patch of one port leaves the rest alone', () => {
    const draft = { ...draftOf('syslog', SETTINGS), udp_port: '514' };
    expect(settingChange('syslog', draft, SETTINGS).patch).toEqual({ syslog: { udp_port: 514 } });
  });

  it('lets the syslog TCP listener be switched off with 0 and nothing else', () => {
    const tcpOff = { ...draftOf('syslog', SETTINGS), tcp_port: '0' };
    expect(settingChange('syslog', tcpOff, SETTINGS).patch).toEqual({ syslog: { tcp_port: 0 } });
    const udpOff = { ...draftOf('syslog', SETTINGS), udp_port: '0' };
    expect(settingChange('syslog', udpOff, SETTINGS)).toMatchObject({
      patch: null,
      errors: { udp_port: 'A port is between 1 and 65535.' },
    });
  });

  it('will not send a port that would make the whole section unreadable', () => {
    // The engine's field is sixteen bits wide. A section that no longer
    // parses falls back to its defaults, and every receiver switches off.
    const draft = { ...draftOf('flows', SETTINGS), port: '70000' };
    const { patch, errors } = settingChange('flows', draft, SETTINGS);
    expect(patch).toBeNull();
    expect(errors.port).toBe('A port is at most 65535.');
  });

  it('does not object to what is stored, only to what was typed over it', () => {
    // A port of 0 is "any free port": the engine takes it and says which it
    // got. The form does not offer it, and must not greet it with an error.
    const stored = { ...SETTINGS, syslog: { ...SETTINGS.syslog, udp_port: 0 } };
    expect(settingChange('syslog', draftOf('syslog', stored), stored)).toEqual({ patch: null, errors: {} });
    const other = { ...draftOf('syslog', stored), tcp_port: '601' };
    expect(settingChange('syslog', other, stored)).toEqual({ patch: { syslog: { tcp_port: 601 } }, errors: {} });
  });

  it('leaves it to the engine to say whether an address is one this machine has', () => {
    const draft = { ...draftOf('traps', SETTINGS), bind: 'not-an-address' };
    expect(settingChange('traps', draft, SETTINGS).patch).toEqual({ traps: { bind: 'not-an-address' } });
    expect(settingChange('traps', { ...draft, bind: ' ' }, SETTINGS).errors.bind).toBeDefined();
  });

  it('switches one receiver and touches nothing else', () => {
    expect(enablePatch('flows', true)).toEqual({ flows: { enabled: true } });
    expect(enablePatch('syslog', false)).toEqual({ syslog: { enabled: false } });
  });

  it('names what switching a receiver on would open, from the settings and not from a guess', () => {
    expect(wouldBind('syslog', SETTINGS)).toBe('UDP 5514 and TCP 5514 on 0.0.0.0');
    expect(wouldBind('syslog', { ...SETTINGS, syslog: { ...SETTINGS.syslog, tcp_port: 0 } })).toBe(
      'UDP 5514 on 0.0.0.0',
    );
    expect(wouldBind('traps', SETTINGS)).toBe('UDP 1162 on 0.0.0.0');
    expect(wouldBind('flows', SETTINGS)).toBe('UDP 2055 on 0.0.0.0');
    expect(wouldBind('flows', undefined)).toBeNull();
  });
});
