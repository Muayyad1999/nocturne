import { describe, expect, it } from 'vitest';
import type { DetectionHit } from '@/api/types/detect';
import {
  allOff,
  detectionName,
  evidenceFacts,
  hitLinks,
  hitsPhrase,
  logEntryOf,
  severityTone,
  standingOf,
  withHit,
} from './detections';
import { CATALOGUE, HIT } from './fixtures';

function fact(evidence: unknown, path: string) {
  return evidenceFacts(evidence).find((entry) => entry.path === path);
}

describe('how often it fired', () => {
  it('counts a day of hits as the engine counted them', () => {
    expect(hitsPhrase(0)).toBe('0 hits in 24 h');
    expect(hitsPhrase(1)).toBe('1 hit in 24 h');
    expect(hitsPhrase(3)).toBe('3 hits in 24 h');
  });

  it('says a count at the engine’s ceiling is a floor, not the number of hits', () => {
    expect(hitsPhrase(100_000)).toMatch(/^at least 100.000 hits in 24 h$/);
  });
});

describe('switched on is not the same as able to run', () => {
  it('says a detection is watching only when it is both', () => {
    expect(standingOf({ enabled: true, available: true })).toMatchObject({ kind: 'watching', tone: 'ok' });
  });

  it('says a detection that is on and cannot run is silent, with the engine’s reason', () => {
    const reason = '`maintenance.list` is not in this build, so every protected change would look as if it were outside a window';
    expect(standingOf({ enabled: true, available: false, unavailable_reason: reason })).toEqual({
      kind: 'unavailable',
      tone: 'warn',
      label: 'Cannot run in this build',
      reason,
    });
  });

  it('keeps "switched off" apart from "switched off and could not run anyway"', () => {
    expect(standingOf({ enabled: false, available: true }).kind).toBe('off');
    expect(standingOf({ enabled: false, available: false, unavailable_reason: 'no source' })).toMatchObject({
      kind: 'off-unavailable',
      reason: 'no source',
    });
  });

  it('notices when every entry reads as switched off, which is what "off as a whole" looks like', () => {
    expect(allOff(CATALOGUE)).toBe(false);
    expect(allOff(CATALOGUE.map((entry) => ({ ...entry, enabled: false })))).toBe(true);
    expect(allOff([])).toBe(false);
  });
});

describe('evidence, field by field, as text', () => {
  it('lays out the hit a lab switch’s own log line produced', () => {
    expect(evidenceFacts(HIT.evidence)).toEqual([
      { path: 'decided_from', label: 'Decided from', value: 'log', mono: false },
      { path: 'log_entry_id', label: 'Log entry', value: '23', mono: true },
      { path: 'stream', label: 'Stream', value: 'syslog', mono: false },
      {
        path: 'via',
        label: 'How the line got here',
        value: 'lab — handed over by a simulated device, in process',
        mono: false,
      },
      { path: 'by', label: 'By (as the device said)', value: 'console', mono: true },
      { path: 'from', label: 'From (as the device said)', value: 'console', mono: true },
      { path: 'changes_checked', label: 'Changes compared', value: '0', mono: true },
      { path: 'grace_minutes', label: 'Grace either side', value: '15 min', mono: true },
    ]);
  });

  it('shows what a device said as the text it is, markup and all', () => {
    const by = fact({ decided_from: 'log', by: '<img src=x onerror=alert(1)>' }, 'by');
    expect(by?.value).toBe('<img src=x onerror=alert(1)>');
  });

  it('flattens the two sides of a changed host key and formats when the old one was first seen', () => {
    const evidence = {
      decided_from: 'connect.host_key_changed',
      host: '10.0.0.1',
      port: 22,
      known: { algorithm: 'ssh-ed25519', fingerprint: 'SHA256:kAp1b0mL8sQw3nRZ', first_seen: 1_700_000_000_000 },
      presented: { algorithm: 'ssh-ed25519', fingerprint: 'SHA256:Zz9yXw8vUt7sRq6p' },
      device_ids: ['a', 'b'],
      host_key_policy: 'tofu',
      connection: 'refused',
    };
    expect(fact(evidence, 'known.fingerprint')).toMatchObject({
      label: 'Remembered fingerprint',
      value: 'SHA256:kAp1b0mL8sQw3nRZ',
    });
    expect(fact(evidence, 'presented.fingerprint')?.label).toBe('Presented fingerprint');
    expect(fact(evidence, 'known.first_seen')?.value).toMatch(/^2023-11-1[45] /);
    expect(fact(evidence, 'port')?.value).toBe('22');
    expect(fact(evidence, 'device_ids')?.value).toBe('a, b');
    expect(fact(evidence, 'connection')?.value).toBe('refused — the connection was not made');
  });

  it('says a connection went ahead when the policy accepts a changed key', () => {
    expect(fact({ connection: 'went_ahead' }, 'connection')?.value).toContain('went ahead');
  });

  it('does not group the digits of an id as if it were a quantity', () => {
    const evidence = { audit_entry_ids: [12345, 12346], through_audit_id: 12346, count: 12345 };
    expect(fact(evidence, 'audit_entry_ids')?.value).toBe('12345, 12346');
    expect(fact(evidence, 'through_audit_id')?.value).toBe('12346');
    expect(fact(evidence, 'count')?.value).toBe('12 345');
  });

  it('lays out the devices of a protected change and says "none" for an empty list', () => {
    const evidence = {
      decided_from: 'changes.changed',
      change_id: 'c1',
      devices: [{ device_id: 'sw1', protected_hits: 2 }],
      devices_no_longer_in_inventory: [],
    };
    expect(fact(evidence, 'devices')?.value).toBe('device_id sw1, protected_hits 2');
    expect(fact(evidence, 'devices_no_longer_in_inventory')?.value).toBe('none');
  });

  it('keeps a field it has no name for, under the engine’s own key', () => {
    const facts = evidenceFacts({ decided_from: 'log', something_new: { deeper: 7 } });
    expect(facts.at(-1)).toMatchObject({ path: 'something_new.deeper', label: 'something_new.deeper', value: '7' });
  });

  it('has nothing to show for evidence that is not a record', () => {
    expect(evidenceFacts(null)).toEqual([]);
    expect(evidenceFacts('text')).toEqual([]);
    expect(evidenceFacts([1, 2])).toEqual([]);
  });

  it('finds the log entry a hit was decided from', () => {
    expect(logEntryOf(HIT.evidence)).toBe(23);
    expect(logEntryOf({ decided_from: 'audit' })).toBeNull();
  });
});

describe('where a hit points', () => {
  it('links to the one alert list and to the device, when the hit names them', () => {
    expect(hitLinks(HIT)).toEqual({ alert: '/monitoring/alerts', device: '/devices/fvkvbsh2tveiuow2' });
  });

  it('links to neither when the hit names neither', () => {
    // `device_id` and `alert_id` are absent, not null, when there is none.
    expect(hitLinks({})).toEqual({ alert: null, device: null });
  });

  it('names a hit’s detection from the catalogue, and by its id when the catalogue has no such entry', () => {
    expect(detectionName('unplanned_config_change', CATALOGUE)).toBe('Unplanned configuration change');
    expect(detectionName('something_newer', CATALOGUE)).toBe('something_newer');
  });

  it('gives a severity its tone, and an unknown one none', () => {
    expect(severityTone('critical')).toBe('crit');
    expect(severityTone('medium')).toBe('warn');
    expect(severityTone('info')).toBe('info');
    expect(severityTone('urgent')).toBe('idle');
  });
});

describe('a hit as it happens', () => {
  const later: DetectionHit = { ...HIT, id: 'later', at: HIT.at + 1_000, detection: 'failed_unlocks' };

  it('goes in front of the list', () => {
    expect(withHit([HIT], later, null, 100).map((hit) => hit.id)).toEqual(['later', HIT.id]);
  });

  it('is not added twice when the event and the list both carry it', () => {
    expect(withHit([HIT], HIT, null, 100)).toEqual([HIT]);
  });

  it('is not added to a list narrowed to another detection', () => {
    expect(withHit([HIT], later, 'unplanned_config_change', 100)).toEqual([HIT]);
    expect(withHit([HIT], later, 'failed_unlocks', 100).map((hit) => hit.id)).toEqual(['later', HIT.id]);
  });

  it('keeps the list in order of when things happened, not of when they were judged', () => {
    // `at` is when what it was decided from happened; a backup's sighting
    // can be judged after a later log line's.
    const earlier = { ...HIT, id: 'earlier', at: HIT.at - 60_000 };
    expect(withHit([HIT], earlier, null, 100).map((hit) => hit.id)).toEqual([HIT.id, 'earlier']);
  });

  it('holds no more than the list is asked to', () => {
    expect(withHit([HIT], later, null, 1).map((hit) => hit.id)).toEqual(['later']);
  });
});
