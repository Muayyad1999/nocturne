import { describe, expect, it } from 'vitest';
import type { ConfigVersion, Coverage, DiffLine } from '@/api/types/config';
import type { Job } from '@/api/types/core';
import {
  backupRows,
  captureSentence,
  captureState,
  checkSentence,
  isEmptyCapture,
  originSentence,
  scoreTone,
  sectionLabel,
  severityTone,
  sideBySide,
} from './vocab';

function coverage(partial: Partial<Coverage> = {}): Coverage {
  return {
    device_id: 'd1',
    name: 'core-sw-1',
    profile: 'cisco_ios',
    lab: false,
    versions: 0,
    ...partial,
  };
}

function version(partial: Partial<ConfigVersion> = {}): ConfigVersion {
  return {
    id: 7,
    device_id: 'd1',
    kind: 'running',
    sha256: 'abc',
    lines: 120,
    bytes: 4096,
    first_seen: 1,
    last_seen: 2,
    origin: 'manual',
    pinned: false,
    ...partial,
  };
}

describe('captureState', () => {
  it('tells a device nobody has looked at from one that could not be read', () => {
    expect(captureState(coverage())).toBe('never');
    expect(captureState(coverage({ last_error: 'authentication failed' }))).toBe('failed');
    expect(captureState(coverage({ skipped: 'monitor-only device' }))).toBe('skipped');
  });

  it('is captured as soon as anything is stored, even with a later failure', () => {
    const row = coverage({
      versions: 2,
      running: { version_id: 3, fetched_at: 10, lines: 90, sha256: 'x' },
      last_error: 'timed out',
    });
    expect(captureState(row)).toBe('captured');
    expect(captureSentence(row)).toContain('timed out');
  });

  it('says in words that nothing captured is not an empty configuration', () => {
    expect(captureSentence(coverage())).toContain('not the same as an empty configuration');
  });

  it('an empty capture is a stored version with no lines, not a missing one', () => {
    expect(isEmptyCapture(undefined)).toBe(false);
    expect(isEmptyCapture(version({ lines: 0 }))).toBe(true);
    expect(isEmptyCapture(version())).toBe(false);
  });
});

describe('originSentence', () => {
  it('says how a version came to be stored', () => {
    expect(originSentence('scheduled')).toContain('schedule');
    expect(originSentence('pre-change')).toContain('before a change');
  });

  it('quotes an origin it has never seen rather than inventing one', () => {
    expect(originSentence('from-mars')).toContain('from-mars');
  });
});

describe('sideBySide', () => {
  const lines: DiffLine[] = [
    { tag: ' ', text: 'hostname SW1', old_line: 1, new_line: 1 },
    { tag: '-', text: ' description old', old_line: 2, new_line: null },
    { tag: '+', text: ' description new', old_line: null, new_line: 2 },
    { tag: '+', text: ' switchport mode access', old_line: null, new_line: 3 },
    { tag: '@', text: '…', old_line: null, new_line: null },
    { tag: ' ', text: 'end', old_line: 40, new_line: 41 },
  ];

  it('pairs a run of removals with the additions that replace them', () => {
    const rows = sideBySide(lines);
    expect(rows.map((row) => row.kind)).toEqual(['same', 'change', 'add', 'gap', 'same']);
    expect(rows[1]?.left?.text).toBe(' description old');
    expect(rows[1]?.right?.text).toBe(' description new');
    // An addition with nothing to pair with has no left side at all.
    expect(rows[2]?.left).toBeUndefined();
    expect(rows[2]?.right?.no).toBe(3);
  });

  it('keeps the engine’s line numbers on both sides', () => {
    const rows = sideBySide(lines);
    expect(rows[0]).toMatchObject({ left: { no: 1 }, right: { no: 1 } });
    expect(rows[4]).toMatchObject({ left: { no: 40 }, right: { no: 41 } });
  });

  it('terminates on a tag it does not know', () => {
    const rows = sideBySide([{ tag: '?', text: 'odd', old_line: 1, new_line: 1 }]);
    expect(rows).toHaveLength(1);
    expect(rows[0]?.kind).toBe('same');
  });

  it('projects and never re-diffs: every row comes from a given line', () => {
    const rows = sideBySide(lines);
    const texts = rows.flatMap((row) => [row.left?.text, row.right?.text]).filter(Boolean);
    for (const text of texts) {
      expect(lines.some((line) => line.text === text)).toBe(true);
    }
  });
});

describe('backupRows', () => {
  function job(partial: Partial<Job> = {}): Job {
    return {
      id: 'j1',
      kind: 'config.backup',
      title: 'Back up 3 devices',
      state: 'succeeded',
      origin: 'user',
      device_ids: ['a', 'b', 'c'],
      done: 3,
      total: 3,
      message: '',
      started_at: 0,
      log: [],
      log_dropped: 0,
      ...partial,
    };
  }

  it('reads each device’s outcome from the lines the job wrote about it', () => {
    const rows = backupRows(
      job({
        result: { backed_up: 2, unchanged: 1, changed: 0, first: 1, skipped: [], failed: [] },
        log: [
          { ts: 1, level: 'info', message: 'first backup stored', device_id: 'a' },
          { ts: 2, level: 'info', message: 'configuration unchanged', device_id: 'b' },
          { ts: 3, level: 'error', message: 'could not enter privileged mode', device_id: 'c' },
        ],
      }),
      (id) => ({ a: 'sw-a', b: 'sw-b', c: 'sw-c' })[id],
    );
    expect(rows.map((row) => [row.name, row.state])).toEqual([
      ['sw-c', 'failed'],
      ['sw-a', 'first'],
      ['sw-b', 'unchanged'],
    ]);
    expect(rows[0]?.detail).toBe('could not enter privileged mode');
  });

  it('keeps a warning about a device as a note, not as its outcome', () => {
    const rows = backupRows(
      job({
        device_ids: ['a'],
        log: [
          { ts: 1, level: 'warn', message: 'could not read the startup-config: not present', device_id: 'a' },
          { ts: 2, level: 'info', message: 'first backup stored', device_id: 'a' },
        ],
      }),
      () => 'sw-a',
    );
    expect(rows[0]?.state).toBe('first');
    expect(rows[0]?.notes).toEqual(['could not read the startup-config: not present']);
  });

  it('shows a failure recorded only in the result', () => {
    const rows = backupRows(
      job({
        device_ids: ['a'],
        result: { failed: [{ device_id: 'a', name: 'sw-a', error: 'no route to host' }] },
      }),
      () => undefined,
    );
    expect(rows[0]).toMatchObject({ state: 'failed', name: 'sw-a', detail: 'no route to host' });
  });

  it('leaves a device the job has not reported on as still working', () => {
    const rows = backupRows(job({ device_ids: ['a'], state: 'running' }), () => 'sw-a');
    expect(rows[0]?.state).toBe('running');
  });
});

describe('words and tones', () => {
  it('scores nothing as unknown rather than as zero', () => {
    expect(scoreTone(null)).toBe('idle');
    expect(scoreTone(95)).toBe('ok');
    expect(scoreTone(80)).toBe('warn');
    expect(scoreTone(35)).toBe('crit');
  });

  it('puts high and critical in the same place: both are red', () => {
    expect(severityTone('critical')).toBe('crit');
    expect(severityTone('high')).toBe('crit');
    expect(severityTone('info')).toBe('info');
  });

  it('names a section path outermost first, and the top level by name', () => {
    expect(sectionLabel([])).toBe('Top level');
    expect(sectionLabel(['router bgp 65001', 'address-family ipv4'])).toBe(
      'router bgp 65001 › address-family ipv4',
    );
  });

  it('says what each kind of check looks for', () => {
    expect(checkSentence({ type: 'present', line: 'ip ssh version 2' })).toContain('must contain');
    expect(checkSentence({ type: 'absent', line: 'ip http server' })).toContain('must not contain');
    expect(checkSentence({ type: 'matches', pattern: '^ntp ' })).toContain('^ntp ');
    expect(checkSentence({ type: 'section', parent: '^line vty', each: true, must: ['transport input ssh'] })).toContain(
      'Every section',
    );
    expect(checkSentence({ type: 'section', parent: '^line vty', must: ['x'] })).toContain(
      'At least one section',
    );
    expect(checkSentence({ type: 'snippet', config: 'x' })).toContain('golden block');
    // Something the backend adds later is still described, not dropped.
    expect(checkSentence({ type: 'future_check' })).toContain('future_check');
  });
});
