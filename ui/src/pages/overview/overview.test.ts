import { describe, expect, it } from 'vitest';
import type { DeviceRow } from '@/api/types/core';
import type { Overview } from '@/api/types/ops';
import {
  ACTIVITY_WORD,
  sitesLine,
  activityLink,
  coveragePct,
  floorText,
  freshness,
  jobProgress,
  needsAttention,
  readPart,
  REASON_HEADLINE,
  severityRank,
  STALE_AFTER_MS,
  tiles,
  worstSeverity,
} from './overview';

/** An overview with every part present and quiet: the "nothing wrong" answer. */
function quiet(over: Partial<Overview> = {}): Overview {
  return {
    devices: { total: 4, up: 4, down: 0, unknown: 0, lab: 0, by_site: [] },
    alerts: { open: 0, by_severity: { critical: 0, high: 0, medium: 0, low: 0, info: 0 }, newest: [], truncated: false, from: 'alerts.summary' },
    changes: { awaiting_approval: 0, applying: 0, applied_24h: 0, failed_24h: 0, recent: [], truncated: false },
    backups: { devices_with_backup: 4, devices_without: 0, changed_24h: 0, oldest_backup_at: 1_000, from: 'store.configs' },
    compliance: { score_pct: 100, failing_devices: 0, failing_findings: 0, run_at: 2_000, truncated: false },
    insights: { open: 0, top: [], truncated: false },
    jobs: { running: [], failed_24h: 0 },
    incidents: { open: 0, mitigated: 0, unresolved: 0, by_severity: { sev1: 0, sev2: 0, sev3: 0, sev4: 0 }, newest: [] },
    activity: [],
    unavailable: [],
    generated_at: 10_000,
    ...over,
  };
}

function device(over: Partial<DeviceRow> = {}): DeviceRow {
  return {
    id: 'd1',
    name: 'core-sw1',
    address: '10.0.0.1',
    profile: 'cisco_ios',
    transport: 'ssh',
    site: 'HQ',
    role: 'core',
    tags: [],
    notes: '',
    monitor: { enabled: true, icmp: true, snmp: false, interval_s: 60 },
    protected: false,
    created_at: 0,
    updated_at: 0,
    state: { device_id: 'd1', reachable: false, last_seen: 5_000 },
    ...over,
  } as DeviceRow;
}

const tile = (overview: Overview, id: string) => {
  const found = tiles(overview).find((entry) => entry.id === id);
  if (!found) throw new Error(`no ${id} tile`);
  return found;
};

describe('readPart', () => {
  it('hands back the data when the part is there, even when every number in it is zero', () => {
    const part = readPart(quiet(), 'alerts');
    expect(part.present).toBe(true);
    if (part.present) expect(part.value.open).toBe(0);
  });

  it('hands back the reason, not a zero, when the part is null', () => {
    const part = readPart(
      quiet({
        alerts: null,
        unavailable: [{ section: 'alerts', reason: 'not_installed', message: '`alerts.list` is not part of this build' }],
      }),
      'alerts',
    );
    expect(part.present).toBe(false);
    if (!part.present) {
      expect(part.reason).toBe('not_installed');
      expect(part.message).toContain('not part of this build');
    }
  });

  it('reports a failure rather than inventing a number when a null part has no reason beside it', () => {
    const part = readPart(quiet({ compliance: null, unavailable: [] }), 'compliance');
    expect(part.present).toBe(false);
    if (!part.present) {
      expect(part.reason).toBe('failed');
      expect(part.message).toContain('no reason was given');
    }
  });

  it('gives each of the five reasons its own sentence', () => {
    const headlines = Object.values(REASON_HEADLINE);
    expect(new Set(headlines).size).toBe(headlines.length);
  });
});

describe('the tiles', () => {
  it('reads a measured zero as a zero and says nothing is firing', () => {
    const reading = tile(quiet(), 'alerts').reading;
    expect(reading.kind).toBe('value');
    if (reading.kind !== 'value') return;
    expect(reading.value).toBe('0');
    expect(reading.notes[0]).toEqual({ text: 'Nothing firing', tone: 'ok' });
  });

  it('never turns an absent part into a number', () => {
    const reading = tile(
      quiet({
        alerts: null,
        unavailable: [{ section: 'alerts', reason: 'not_installed', message: 'no alerting here' }],
      }),
      'alerts',
    ).reading;
    expect(reading.kind).toBe('unavailable');
    if (reading.kind !== 'unavailable') return;
    expect(reading.reason).toBe('not_installed');
    // The two cases are different shapes, so a zero cannot be rendered by mistake.
    expect('value' in reading).toBe(false);
  });

  it('a quiet feature and an absent feature produce different tiles for the same screen', () => {
    const silent = tile(quiet(), 'alerts').reading;
    const absent = tile(
      quiet({ alerts: null, unavailable: [{ section: 'alerts', reason: 'not_installed', message: 'x' }] }),
      'alerts',
    ).reading;
    expect(silent.kind).not.toBe(absent.kind);
  });

  it('names the worst severity when something is firing', () => {
    const reading = tile(
      quiet({
        alerts: {
          open: 7,
          by_severity: { critical: 0, high: 2, medium: 5, low: 0, info: 0 },
          newest: [],
          truncated: false,
          from: 'alerts.summary',
        },
      }),
      'alerts',
    ).reading;
    if (reading.kind !== 'value') throw new Error('expected a reading');
    expect(reading.value).toBe('7');
    expect(reading.notes[0]?.text).toBe('Worst: high (2)');
    expect(reading.notes[0]?.tone).toBe('crit');
  });

  it('marks a capped count as a floor, and will not call its worst the worst', () => {
    const reading = tile(
      quiet({
        alerts: { open: 500, by_severity: { medium: 500 }, newest: [], truncated: true, from: 'alerts.list' },
      }),
      'alerts',
    ).reading;
    if (reading.kind !== 'value') throw new Error('expected a reading');
    expect(reading.floor).toBe(true);
    expect(reading.notes[0]?.text).toBe('Worst seen: medium (at least 500)');
    expect(reading.notes[1]?.text).toContain('there may be worse');
    expect(reading.provenance).toBe('Counted from `alerts.list`');
  });

  it('says how many open alerts the attention list does not show', () => {
    const rows = needsAttention({
      overview: quiet({
        alerts: { open: 503, by_severity: { medium: 500 }, newest: [], truncated: true, from: 'alerts.list' },
      }),
      devices: [],
    });
    expect(rows.at(-1)?.title).toBe('at least 503 further open alerts');
  });

  it('counts down devices loudly and never-polled ones separately', () => {
    const reading = tile(
      quiet({ devices: { total: 10, up: 7, down: 2, unknown: 1, lab: 0, by_site: [] } }),
      'devices',
    ).reading;
    if (reading.kind !== 'value') throw new Error('expected a reading');
    expect(reading.value).toBe('7');
    expect(reading.unit).toBe('of 10');
    expect(reading.notes[0]).toEqual({ text: '2 devices down', tone: 'crit' });
    expect(reading.notes[1]?.text).toBe('1 never polled');
  });

  it('says "no score" for a compliance run that produced none, which is not 0 %', () => {
    const reading = tile(
      quiet({ compliance: { score_pct: null, failing_devices: 0, failing_findings: 0, run_at: 1, truncated: false } }),
      'compliance',
    ).reading;
    if (reading.kind !== 'value') throw new Error('expected a reading');
    expect(reading.value).toBe('No score');
  });

  it('every tile links to the screen that acts on it', () => {
    for (const entry of tiles(quiet())) {
      expect(entry.to.startsWith('/')).toBe(true);
      expect(entry.linkLabel.length).toBeGreaterThan(0);
    }
  });
});

describe('coveragePct', () => {
  it('is a measured zero when devices exist and none is backed up', () => {
    expect(coveragePct(0, 3)).toBe(0);
  });

  it('is nothing at all when there are no devices: 0 / 0 is not 0 %', () => {
    expect(coveragePct(0, 0)).toBe(null);
  });

  it('counts both halves', () => {
    expect(coveragePct(3, 1)).toBe(75);
  });
});

describe('floorText', () => {
  it('says "at least" for a count read from a capped list', () => {
    expect(floorText(500, true)).toBe('at least 500');
  });

  it('says the number plainly when the whole list was read', () => {
    expect(floorText(12, false)).toBe('12');
  });
});

describe('worstSeverity and severityRank', () => {
  it('finds the worst severity that has anything in it', () => {
    expect(worstSeverity({ critical: 0, high: 0, medium: 3 })).toBe('medium');
  });

  it('is null when nothing is firing, which is not the same as "no alerting"', () => {
    expect(worstSeverity({ critical: 0, high: 0, medium: 0, low: 0, info: 0 })).toBe(null);
  });

  it('ranks critical above high above medium', () => {
    expect(severityRank('critical')).toBeLessThan(severityRank('high'));
    expect(severityRank('high')).toBeLessThan(severityRank('medium'));
    expect(severityRank('nonsense')).toBeGreaterThan(severityRank('info'));
  });
});

describe('sitesLine', () => {
  const counts = (sites: { site: string; total: number; down: number; unknown: number }[]) =>
    quiet({ devices: { total: 9, up: 6, down: 3, unknown: 0, lab: 0, by_site: sites } }).devices;

  it('says nothing when there is only one site to say it about', () => {
    expect(sitesLine(counts([{ site: 'HQ', total: 3, down: 1, unknown: 0 }]))).toBe(null);
  });

  it('names the sites with something down, worst first', () => {
    expect(
      sitesLine(
        counts([
          { site: 'HQ', total: 3, down: 1, unknown: 0 },
          { site: 'DC1', total: 4, down: 2, unknown: 0 },
          { site: 'Branch', total: 2, down: 0, unknown: 0 },
        ]),
      ),
    ).toBe('Down in DC1 (2), HQ (1)');
  });

  it('counts the sites it does not name', () => {
    const many = Array.from({ length: 7 }, (_value, index) => ({
      site: `S${index}`,
      total: 2,
      down: 1,
      unknown: 0,
    }));
    expect(sitesLine(counts(many))).toContain('and 3 more');
  });

  it('claims nothing when no site has a device down, because nobody may be looking', () => {
    expect(
      sitesLine(
        counts([
          { site: 'HQ', total: 3, down: 0, unknown: 3 },
          { site: 'DC1', total: 4, down: 0, unknown: 4 },
        ]),
      ),
    ).toBe(null);
  });
});

describe('needsAttention', () => {
  it('is empty when nothing is wrong', () => {
    expect(needsAttention({ overview: quiet(), devices: [] })).toEqual([]);
  });

  it('ranks an incident above a dead device above an alert', () => {
    const rows = needsAttention({
      overview: quiet({
        devices: { total: 2, up: 1, down: 1, unknown: 0, lab: 0, by_site: [] },
        alerts: {
          open: 1,
          by_severity: { critical: 1 },
          newest: [
            { id: 'a1', severity: 'critical', title: 'Link down', state: 'firing', started_at: 9_000, lab: false },
          ],
          truncated: false,
          from: 'alerts.summary',
        },
        incidents: {
          open: 1,
          mitigated: 0,
          unresolved: 1,
          by_severity: { sev1: 1, sev2: 0, sev3: 0, sev4: 0 },
          newest: [
            {
              id: 'i1',
              title: 'HQ is dark',
              severity: 'sev1',
              state: 'open',
              started_at: 8_000,
              assignee: '',
              device_ids: [],
              devices: [],
              alert_ids: [],
              change_ids: [],
              summary: '',
              entries: 2,
              open_follow_ups: 0,
              gaps: 0,
              updated_at: 8_000,
            },
          ],
        },
      }),
      devices: [device()],
    });
    expect(rows.map((row) => row.kind)).toEqual(['incident', 'device', 'alert']);
  });

  it('does not list a dead device twice when an alert already says it is dead', () => {
    const rows = needsAttention({
      overview: quiet({
        devices: { total: 1, up: 0, down: 1, unknown: 0, lab: 0, by_site: [] },
        alerts: {
          open: 1,
          by_severity: { critical: 1 },
          newest: [
            {
              id: 'a1',
              severity: 'critical',
              title: 'Device down — core-sw1',
              state: 'firing',
              started_at: 9_000,
              device_id: 'd1',
              device: 'core-sw1',
              lab: false,
            },
          ],
          truncated: false,
          from: 'alerts.summary',
        },
      }),
      devices: [device()],
    });
    expect(rows.map((row) => row.kind)).toEqual(['alert']);
    expect(rows[0]?.ackAlertId).toBe('a1');
  });

  it('lists a dead device that nothing is firing about, which is how "nobody is watching" looks', () => {
    const rows = needsAttention({
      overview: quiet({ devices: { total: 1, up: 0, down: 1, unknown: 0, lab: 0, by_site: [] } }),
      devices: [device()],
    });
    expect(rows.map((row) => row.kind)).toEqual(['device']);
  });

  it('names the dead devices from the inventory', () => {
    const rows = needsAttention({
      overview: quiet({ devices: { total: 1, up: 0, down: 1, unknown: 0, lab: 0, by_site: [] } }),
      devices: [device({ name: 'dist-sw9', site: 'Branch North' })],
    });
    expect(rows[0]?.title).toBe('dist-sw9 is not answering');
    expect(rows[0]?.where).toBe('Branch North');
    expect(rows[0]?.action?.to).toBe('/devices/d1');
  });

  it('says a device has never been seen rather than leaving the time blank', () => {
    const rows = needsAttention({
      overview: quiet({ devices: { total: 1, up: 0, down: 1, unknown: 0, lab: 0, by_site: [] } }),
      devices: [device({ state: { device_id: 'd1', reachable: false, last_seen: null } })],
    });
    expect(rows[0]?.where).toBe('HQ · never seen');
    expect(rows[0]?.since).toBeUndefined();
  });

  it('states the count instead of naming nothing when the inventory could not be read', () => {
    const rows = needsAttention({
      overview: quiet({ devices: { total: 9, up: 7, down: 2, unknown: 0, lab: 0, by_site: [] } }),
      devices: undefined,
    });
    expect(rows).toHaveLength(1);
    expect(rows[0]?.title).toBe('2 devices not answering');
    expect(rows[0]?.where).toContain('cannot be named');
  });

  it('says how many loud alerts are not in the five it was given', () => {
    const rows = needsAttention({
      overview: quiet({
        alerts: {
          open: 30,
          by_severity: { critical: 4, high: 0, medium: 26 },
          newest: [
            { id: 'a1', severity: 'critical', title: 'One', state: 'firing', started_at: 3, lab: false },
          ],
          truncated: false,
          from: 'alerts.summary',
        },
      }),
      devices: [],
    });
    const more = rows.find((row) => row.kind === 'more');
    expect(more?.title).toBe('29 further open alerts');
  });

  it('offers an acknowledge only for an alert that is still firing', () => {
    const rows = needsAttention({
      overview: quiet({
        alerts: {
          open: 2,
          by_severity: { critical: 2 },
          newest: [
            { id: 'a1', severity: 'critical', title: 'One', state: 'firing', started_at: 3, lab: false },
            { id: 'a2', severity: 'critical', title: 'Two', state: 'acknowledged', started_at: 2, lab: false },
          ],
          truncated: false,
          from: 'alerts.summary',
        },
      }),
      devices: [],
    });
    expect(rows.find((row) => row.id === 'alert:a1')?.ackAlertId).toBe('a1');
    expect(rows.find((row) => row.id === 'alert:a2')?.ackAlertId).toBeUndefined();
  });

  it('carries the lab mark through, so a simulated problem is never mistaken for a real one', () => {
    const rows = needsAttention({
      overview: quiet({
        insights: {
          open: 1,
          top: [{ id: 'n1', title: 'No time source', severity: 'low', device: 'ovlab/SW1', device_id: 'x', lab: true }],
          truncated: false,
        },
      }),
      devices: [],
    });
    expect(rows[0]?.lab).toBe(true);
  });

  it('leaves "never backed up" to the tile, which says it in the same words', () => {
    const rows = needsAttention({
      overview: quiet({
        backups: { devices_with_backup: 1, devices_without: 3, changed_24h: 0, oldest_backup_at: 5, from: 'store.configs' },
      }),
      devices: [],
    });
    expect(rows).toEqual([]);
    const reading = tile(
      quiet({
        backups: { devices_with_backup: 1, devices_without: 3, changed_24h: 0, oldest_backup_at: 5, from: 'store.configs' },
      }),
      'backups',
    ).reading;
    if (reading.kind !== 'value') throw new Error('expected a reading');
    expect(reading.value).toBe('25%');
    expect(reading.notes[0]).toEqual({ text: '3 devices never backed up', tone: 'warn' });
  });

  it('says nothing about compliance or backups when those parts are absent', () => {
    const rows = needsAttention({
      overview: quiet({
        compliance: null,
        backups: null,
        unavailable: [
          { section: 'compliance', reason: 'no_data', message: 'no compliance run has been made yet' },
          { section: 'backups', reason: 'failed', message: 'database error' },
        ],
      }),
      devices: [],
    });
    expect(rows).toEqual([]);
  });

  it('counts a half-applied change as needing a person', () => {
    const rows = needsAttention({
      overview: quiet({
        changes: {
          awaiting_approval: 0,
          applying: 0,
          applied_24h: 0,
          failed_24h: 1,
          recent: [
            { id: 'c1', title: 'Trunk edit', state: 'partially_applied', created_at: 1, updated_at: 2 },
            { id: 'c2', title: 'Fine', state: 'applied', created_at: 1, updated_at: 3 },
          ],
          truncated: false,
        },
      }),
      devices: [],
    });
    expect(rows).toHaveLength(1);
    expect(rows[0]?.word).toBe('Change half-applied');
    expect(rows[0]?.action?.to).toBe('/changes/c1');
  });
});

describe('activityLink', () => {
  it('points a change at the change', () => {
    expect(activityLink({ ts: 1, kind: 'change', text: 'x', ref: { type: 'change', id: 'c1' } })).toBe('/changes/c1');
  });

  it('points an alert at the device it is about', () => {
    expect(
      activityLink({ ts: 1, kind: 'alert', text: 'x', device_id: 'd1', ref: { type: 'alert', id: 'a1' } }),
    ).toBe('/monitoring/health?device=d1');
  });

  it('leads nowhere rather than somewhere wrong when the line has no reference', () => {
    expect(activityLink({ ts: 1, kind: 'audit', text: 'Started a backup' })).toBe(null);
  });

  it('has a word for each kind of line the backend merges', () => {
    expect(Object.keys(ACTIVITY_WORD).sort()).toEqual(['alert', 'audit', 'change']);
  });
});

describe('jobProgress', () => {
  const job = { id: 'j1', kind: 'discovery.scan', title: 'Scanning', state: 'running' as const, origin: 'user', done: 6, total: 254, message: '', started_at: 1, device_ids: [] };

  it('shows how far along a job with a known amount of work is', () => {
    expect(jobProgress(job)).toEqual({ value: 6, max: 254, text: '6 of 254' });
  });

  it('does not pretend to know the total when the backend sent zero', () => {
    const unknown = jobProgress({ ...job, total: 0 });
    expect(unknown.value).toBeUndefined();
    expect(unknown.text).toBe('6 done, total unknown');
  });
});

describe('freshness', () => {
  it('is fresh while the reading is recent', () => {
    expect(freshness(1_000_000, 1_000_000 + 30_000)).toBe('fresh');
  });

  it('turns stale when two refetches have been missed', () => {
    expect(freshness(1_000_000, 1_000_000 + STALE_AFTER_MS + 1)).toBe('stale');
  });
});
