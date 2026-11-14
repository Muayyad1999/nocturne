import { describe, expect, it } from 'vitest';
import type { LogEntry } from '@/api/types/core';
import type { LogsAppended } from '@/api/types/logs';
import { bodyOf, clockOf, interfaceOf, mnemonicOf, rawOf, tagOf, viaOf } from './entry';
import { adopt, append, entryCount, fromPage, prepend } from './stream';

function entry(id: number, over: Partial<LogEntry> = {}): LogEntry {
  return {
    id,
    ts: 1_700_000_000_000 + id * 1_000,
    stream: 'syslog',
    device_id: 'dev-1',
    severity: 5,
    source: '10.0.0.1',
    message: `line ${id}`,
    body: { raw: `<189>line ${id}`, via: 'syslog' },
    ...over,
  };
}

function batch(entries: LogEntry[], over: Partial<LogsAppended> = {}): LogsAppended {
  return { entries, dropped: 0, sampled: false, received: entries.length, ...over };
}

describe('the list the screen scrolls', () => {
  it('turns a newest-first page into oldest-first rows, so the newest is at the bottom', () => {
    const stream = fromPage([entry(3), entry(2), entry(1)]);
    expect(stream.rows.map((row) => row.key)).toEqual(['e1', 'e2', 'e3']);
    expect([stream.oldest, stream.newest]).toEqual([1, 3]);
  });

  it('puts an older page in front without letting a line in twice', () => {
    const stream = prepend(fromPage([entry(3), entry(2)]), [entry(2), entry(1)]);
    expect(stream.rows.map((row) => row.key)).toEqual(['e1', 'e2', 'e3']);
    expect(stream.oldest).toBe(1);
  });

  it('keeps what the tail delivered while a page was being read', () => {
    // The page is a snapshot of the store when the query ran; a line stored a
    // moment later is in neither the page nor a view that merely replaced
    // itself with it. That is a hole in a live tail, once per refresh.
    const live = append(fromPage([entry(2), entry(1)]), batch([entry(3)]));
    const merged = adopt(live, [entry(2), entry(1)]);
    expect(merged.rows.map((row) => row.key)).toEqual(['e1', 'e2', 'e3']);
    expect(merged.newest).toBe(3);
    // A page that reaches past them replaces them, without duplicating one.
    expect(adopt(live, [entry(3), entry(2), entry(1)]).rows.map((row) => row.key)).toEqual([
      'e1',
      'e2',
      'e3',
    ]);
    expect(adopt(live, []).rows).toEqual([]);
  });

  it('appends a tail batch and ignores the lines it already has', () => {
    const stream = append(fromPage([entry(2), entry(1)]), batch([entry(2), entry(3)]));
    expect(stream.rows.map((row) => row.key)).toEqual(['e1', 'e2', 'e3']);
    expect(stream.newest).toBe(3);
  });

  it('is unchanged by a batch that adds nothing, so the view does not re-render', () => {
    // Every open screen gets `logs.appended`, including batches that belong to
    // somebody else's filter.
    const before = fromPage([entry(2), entry(1)]);
    expect(append(before, batch([entry(1)]))).toBe(before);
    expect(append(before, batch([]))).toBe(before);
  });

  it('keeps a row for the lines the tail sampled away', () => {
    // 18,000 lines in a quarter of a second arrive as 200 plus a number. The
    // number is the shape of the storm; it is not allowed to vanish.
    const stream = append(
      fromPage([entry(1)]),
      batch([entry(2), entry(3)], { dropped: 17_800, sampled: true, received: 17_802 }),
    );
    expect(stream.rows.map((row) => row.kind)).toEqual(['entry', 'missing', 'entry', 'entry']);
    const missing = stream.rows[1];
    expect(missing).toMatchObject({ kind: 'missing', count: 17_800, reason: 'sampled' });
    expect(stream.sampled).toBe(17_800);
  });

  it('drops the oldest rows when the view is full, and says how many it dropped', () => {
    let stream = fromPage([entry(3), entry(2), entry(1)]);
    stream = append(stream, batch([entry(4), entry(5)]), 3);
    expect(entryCount(stream)).toBe(3);
    expect(stream.rows[0]).toMatchObject({ kind: 'missing', count: 2, reason: 'trimmed' });
    expect(stream.oldest).toBe(3);

    // The marker counts everything that has left the window, and there is
    // only ever one of it.
    stream = append(stream, batch([entry(6)]), 3);
    expect(stream.rows.filter((row) => row.kind === 'missing')).toHaveLength(1);
    expect(stream.rows[0]).toMatchObject({ count: 3 });
  });
});

describe('one line, read defensively', () => {
  it('keeps the line exactly as the device wrote it when the body has it', () => {
    expect(rawOf(entry(1))).toEqual({ text: '<189>line 1', exact: true });
  });

  it('falls back to the parsed message and says it is not the raw line', () => {
    // A lab device's lines go into the same table with a different body, and
    // the `audit` and `event` streams have no raw line at all.
    expect(rawOf(entry(1, { body: null }))).toEqual({ text: 'line 1', exact: false });
    const lab = entry(1, { body: { line: '%LINK-3-UPDOWN: Gi0/1 down', lab_id: 'lab-1' } });
    expect(rawOf(lab)).toEqual({ text: '%LINK-3-UPDOWN: Gi0/1 down', exact: true });
    expect(viaOf(lab)).toBe('lab');
    expect(bodyOf(entry(1, { body: 'not an object' }))).toEqual({});
  });

  it('names the clock a row’s time came from, and never mixes the two', () => {
    const received = clockOf(entry(1, { body: { via: 'syslog', timestamp: 'Nov 13 09:13:01' } }));
    expect(received.shown).toBe('received');
    expect(received.deviceText).toBe('Nov 13 09:13:01');
    expect(received.note).toContain('when Nocturne received');
    expect(received.note).toContain('Nov 13 09:13:01');

    const pulled = clockOf(
      entry(1, { body: { via: 'pull', ts_source: 'device', timestamp: 'Nov 13 09:13:01' } }),
    );
    expect(pulled.shown).toBe('device');
    expect(pulled.note).toContain('device’s own');
    expect(viaOf(entry(1, { body: { via: 'pull' } }))).toBe('pull');

    const silent = clockOf(entry(1, { body: { via: 'syslog' } }));
    expect(silent.deviceText).toBeNull();
    expect(silent.note).toContain('carried no timestamp');
  });

  it('labels a row with the mnemonic, then the application, then the address', () => {
    expect(tagOf(entry(1, { body: { mnemonic: 'LINK-3-UPDOWN', app: 'ios' } }))).toBe(
      'LINK-3-UPDOWN',
    );
    expect(tagOf(entry(1, { body: { app: 'sshd' } }))).toBe('sshd');
    expect(tagOf(entry(1, { body: {} }))).toBe('10.0.0.1');
    expect(mnemonicOf(entry(1, { body: {} }))).toBeNull();
  });

  it('shows an interface the way the device spelled it', () => {
    const row = entry(1, { body: { interface: 'GigabitEthernet1/0/3', interface_text: 'Gi1/0/3' } });
    expect(interfaceOf(row)).toBe('Gi1/0/3');
    expect(interfaceOf(entry(1, { body: { interface: 'GigabitEthernet1/0/3' } }))).toBe(
      'GigabitEthernet1/0/3',
    );
    expect(interfaceOf(entry(1))).toBeNull();
  });
});
