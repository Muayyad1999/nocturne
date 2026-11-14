/**
 * The Operations screen's reasoning. These tests are about the four things
 * that decide whether the screen is honest rather than merely populated:
 * the order of a merged timeline, the difference between "nothing happened"
 * and "nobody could tell", the auditability of a generated summary, and a
 * timestamp that is not a time.
 */

import { describe, expect, it } from 'vitest';
import type {
  AlertRef,
  ChangeRef,
  DeviceRef,
  GeneratedReport,
  Handover,
  IncidentHead,
  JobRef,
  TimelineEntry,
  Unavailable,
} from '@/api/types/ops';
import {
  actionGroups,
  actionTone,
  floorText,
  gapCount,
  handoverSections,
  incidentDuration,
  instant,
  jobProgress,
  mergeTimeline,
  periodChoice,
  readSection,
  refLink,
  reportCoverage,
  suggestions,
  timelineDays,
} from './ops';

// ------------------------------------------------------------------ fixtures

function entry(id: string, ts: number, over: Partial<TimelineEntry> = {}): TimelineEntry {
  return {
    id,
    ts,
    kind: 'note',
    text: `text of ${id}`,
    author: 'mo',
    source: { from: 'operator', evidence: 'typed', gathered_at: ts },
    ...over,
  };
}

function alert(id: string, startedAt: number, over: Partial<AlertRef> = {}): AlertRef {
  return {
    id,
    severity: 'critical',
    title: `${id} fired`,
    state: 'firing',
    started_at: startedAt,
    device_id: 'd1',
    device: 'core-sw1',
    rule: 'reachability',
    lab: false,
    ...over,
  };
}

function change(id: string, updatedAt: number, over: Partial<ChangeRef> = {}): ChangeRef {
  return {
    id,
    title: `change ${id}`,
    state: 'applied',
    origin: 'user',
    ticket: 'NET-1',
    created_at: updatedAt - 1_000,
    updated_at: updatedAt,
    device_ids: ['d1'],
    ...over,
  };
}

function device(id: string, name: string, over: Partial<DeviceRef> = {}): DeviceRef {
  return { id, name, site: 'HQ', lab: false, ...over };
}

function incidentHead(id: string, over: Partial<IncidentHead> = {}): IncidentHead {
  return {
    id,
    title: `incident ${id}`,
    severity: 'sev2',
    state: 'open',
    started_at: 1_000,
    assignee: '',
    device_ids: ['d1'],
    devices: [device('d1', 'core-sw1')],
    alert_ids: [],
    change_ids: [],
    summary: '',
    entries: 2,
    open_follow_ups: 0,
    gaps: 0,
    updated_at: 2_000,
    ...over,
  };
}

function job(id: string, over: Partial<JobRef> = {}): JobRef {
  return {
    id,
    kind: 'config.backup',
    title: 'Back up 4 devices',
    state: 'failed',
    origin: 'user',
    done: 3,
    total: 4,
    message: 'three of four',
    started_at: 1_000,
    finished_at: 2_000,
    error: 'core-sw1 refused the connection',
    device_ids: ['d1'],
    ...over,
  };
}

/** A handover with something real in every section. */
function fullHandover(): Handover {
  return {
    period: { since: 1_000, until: 9_000 },
    alerts: { fired: 3, resolved: 1, still_open: [alert('a1', 1_100)], truncated: false },
    incidents: {
      opened: [incidentHead('i1')],
      resolved: [incidentHead('i2', { state: 'resolved', resolved_at: 5_000 })],
      open: [incidentHead('i1')],
    },
    changes: {
      applied: [change('c1', 2_000)],
      failed_or_rolled_back: [change('c2', 3_000, { state: 'failed' })],
      awaiting_approval: [change('c3', 4_000, { state: 'planned' })],
      truncated: false,
    },
    config_changes: [
      {
        device_id: 'd1',
        device: 'core-sw1',
        version_id: 7,
        previous_version_id: 6,
        at: 2_500,
        lines_added: 4,
        lines_removed: 1,
        lines: 300,
        source: 'manual',
        note: '',
        first: false,
        explained_by_change: false,
        unplanned: true,
        checked: true,
      },
    ],
    devices: {
      added: [device('d2', 'access-sw2')],
      removed: [{ device_id: 'd9', name: 'acc-sw9', at: 4_000, from: 'audit entry 12' }],
      went_down: [
        { ...device('d1', 'core-sw1'), at: 1_200, down_ms: 90_000, ongoing: true, from: '`up` samples' },
      ],
      unreachable_now: [device('d1', 'core-sw1')],
      without_reachability_samples: 2,
      not_measured: 0,
    },
    backups: {
      failed: [{ device_id: 'd1', device: 'core-sw1', error: 'no credential', last_backup_at: null }],
    },
    jobs_failed: [job('j1')],
    notes: '',
    unavailable: [],
    generated_at: 9_000,
  };
}

// ------------------------------------------------------------ null versus zero

describe('a section that was not read', () => {
  const gaps: Unavailable[] = [
    { section: 'alerts', reason: 'not_installed', message: 'no alerting in this build' },
  ];

  it('is a different shape from a section that was read and was empty', () => {
    const absent = readSection(null, gaps, 'alerts');
    expect(absent.present).toBe(false);
    if (absent.present) throw new Error('unreachable');
    expect(absent.reason).toBe('not_installed');
    expect(absent.message).toBe('no alerting in this build');

    // A measured zero stays a value, and keeps its zero.
    const measured = readSection({ fired: 0 }, gaps, 'changes');
    expect(measured.present).toBe(true);
    if (!measured.present) throw new Error('unreachable');
    expect(measured.value.fired).toBe(0);
  });

  it('reports a failure rather than a zero when the backend gave no reason', () => {
    const absent = readSection(null, [], 'backups');
    if (absent.present) throw new Error('unreachable');
    expect(absent.reason).toBe('failed');
    expect(absent.message).toContain('backups');
    expect(absent.message).toContain('no reason');
  });

  it('keeps a zero that is present and a count that was capped apart', () => {
    expect(floorText(0, false)).toBe('0');
    expect(floorText(500, true)).toBe('at least 500');
  });
});

describe('a timestamp', () => {
  it('is only a time when it could be one', () => {
    expect(instant(1_700_000_000_000)).toBe(1_700_000_000_000);
    // The cases that would otherwise be rendered as "56 years ago".
    expect(instant(0)).toBeNull();
    expect(instant(-1)).toBeNull();
    expect(instant(null)).toBeNull();
    expect(instant(undefined)).toBeNull();
    expect(instant(Number.NaN)).toBeNull();
  });

  it('decides whether an incident has a duration at all', () => {
    expect(incidentDuration({ started_at: 1_000, resolved_at: 61_000 }, 9_000)).toEqual({
      label: '1m',
      running: false,
    });
    const running = incidentDuration({ started_at: 1_000, resolved_at: null }, 61_000);
    expect(running?.running).toBe(true);
    // A record with no start has no duration, rather than one since 1970.
    expect(incidentDuration({ started_at: 0 }, 9_000)).toBeNull();
    // A clock that went backwards is not a negative duration.
    expect(incidentDuration({ started_at: 9_000, resolved_at: 1_000 }, 9_000)).toBeNull();
  });
});

// ------------------------------------------------------------------- timeline

describe('the incident timeline', () => {
  it('interleaves recorded entries and unlinked suggestions at their real times', () => {
    const entries = [
      entry('alert:a1:fired', 2_000, { kind: 'alert' }),
      entry('note:n1', 3_000),
      entry('alert:a1:resolved', 6_000, { kind: 'alert' }),
    ];
    const offered = suggestions({
      window: { since: 0, until: 9_000 },
      alerts: [alert('a2', 2_500)],
      changes: [change('c1', 4_000)],
      config_changes: [
        {
          device_id: 'd1',
          device: 'core-sw1',
          version_id: 7,
          at: 1_500,
          lines_added: 2,
          lines_removed: 0,
          lines: 300,
          source: 'manual',
          note: '',
          first: false,
          before_start_ms: 500,
          explained_by_change: false,
        },
      ],
      unavailable: [],
    });

    const rows = mergeTimeline(entries, offered);
    expect(rows.map((row) => `${row.kind === 'entry' ? 'E' : 'S'}:${row.at}`)).toEqual([
      // The configuration change four minutes before the alert is the whole
      // point of suggesting anything, and it has to come first.
      'S:1500',
      'E:2000',
      'S:2500',
      'E:3000',
      'S:4000',
      'E:6000',
    ]);
  });

  it('puts a recorded fact above a mere suggestion stamped the same millisecond', () => {
    const rows = mergeTimeline([entry('note:n1', 5_000)], suggestions({
      window: { since: 0, until: 9_000 },
      alerts: [alert('a2', 5_000)],
      changes: [],
      config_changes: [],
      unavailable: [],
    }));
    expect(rows.map((row) => row.kind)).toEqual(['entry', 'suggestion']);
  });

  it('breaks a tie between two facts by their ids, so the order never jitters', () => {
    const rows = mergeTimeline(
      [entry('change:c2:applied', 400), entry('change:c1:applied', 400)],
      [],
    );
    expect(rows.map((row) => row.key)).toEqual(['change:c1:applied', 'change:c2:applied']);
  });

  it('drops a suggestion for something already on the timeline', () => {
    // `incidents.link` stores the alert under this entry id; the suggestion
    // keyed `alert:a1` must not appear a second time beside it.
    const rows = mergeTimeline([entry('alert:a1', 2_000, { kind: 'alert' })], suggestions({
      window: { since: 0, until: 9_000 },
      alerts: [alert('a1', 2_000)],
      changes: [],
      config_changes: [],
      unavailable: [],
    }));
    expect(rows).toHaveLength(1);
    expect(rows[0]?.kind).toBe('entry');
  });

  it('keeps gaps in place and counts them', () => {
    const entries = [
      entry('alert:a1:fired', 1_000, { kind: 'alert' }),
      entry('gap:changes.list:timeout', 1_500, { kind: 'gap', text: 'The change pipeline timed out' }),
      entry('note:n1', 2_000),
    ];
    const rows = mergeTimeline(entries, []);
    expect(rows[1]?.kind === 'entry' && rows[1].entry.kind).toBe('gap');
    expect(gapCount(entries)).toBe(1);
  });

  it('says what a suggested configuration change is, including when nothing explains it', () => {
    const [suggestion] = suggestions({
      window: { since: 0, until: 9_000 },
      alerts: [],
      changes: [],
      config_changes: [
        {
          device_id: 'd1',
          device: 'core-sw1',
          version_id: 7,
          at: 1_500,
          lines: 300,
          source: 'manual',
          note: '',
          first: false,
          explained_by_change: false,
        },
      ],
      unavailable: [],
    });
    expect(suggestion?.explained).toBe(false);
    // No diff was computed: it says so rather than claiming zero lines moved.
    expect(suggestion?.detail).toContain('not computed');
    expect(suggestion?.to).toBe('/config?device=d1');
    // There is no id for a snapshot, so it is evidence, not something to link.
    expect(suggestion?.link).toBeNull();
  });

  it('groups into days without reordering anything', () => {
    const rows = mergeTimeline(
      [entry('a', 1_000), entry('b', 2_000), entry('c', 3_000)],
      [],
    );
    const days = timelineDays(rows, (at) => (at < 2_500 ? 'day one' : 'day two'));
    expect(days.map((day) => [day.day, day.rows.length])).toEqual([
      ['day one', 2],
      ['day two', 1],
    ]);
  });

  it('has nothing to show for an incident with no timeline and no suggestions', () => {
    expect(mergeTimeline([], [])).toEqual([]);
    expect(suggestions(undefined)).toEqual([]);
  });
});

// ------------------------------------------------------------------- handover

describe('a generated handover', () => {
  it('links every item to the record it was made from', () => {
    const sections = handoverSections(fullHandover());
    const all = sections.flatMap((section) =>
      section.body.kind === 'items' ? section.body.items : [],
    );
    expect(all.length).toBeGreaterThan(8);
    for (const item of all) {
      expect(item.to, `"${item.text}" has nowhere to go`).toBeTruthy();
      expect(item.to.startsWith('/'), `"${item.text}" → ${item.to}`).toBe(true);
    }
    // Every section is accounted for, in a fixed order.
    expect(sections.map((section) => section.id)).toEqual([
      'incidents',
      'alerts',
      'changes',
      'config_changes',
      'devices',
      'backups',
      'jobs_failed',
    ]);
  });

  it('names an incident once even though it is opened and still open', () => {
    const sections = handoverSections(fullHandover());
    const incidents = sections.find((section) => section.id === 'incidents');
    if (incidents?.body.kind !== 'items') throw new Error('expected items');
    expect(incidents.body.items.map((item) => item.id)).toEqual(['i1', 'i2']);
    expect(incidents.summary).toBe('1 opened, 1 resolved, 1 still open');
  });

  it('says "nothing happened" for a section it read and found empty', () => {
    const handover = fullHandover();
    handover.jobs_failed = [];
    handover.alerts = { fired: 7, resolved: 7, still_open: [], truncated: false };
    const sections = handoverSections(handover);
    const jobs = sections.find((section) => section.id === 'jobs_failed');
    expect(jobs?.body).toEqual({ kind: 'nothing', sentence: 'No job failed in this period.' });
    const alerts = sections.find((section) => section.id === 'alerts');
    expect(alerts?.body.kind).toBe('nothing');
    if (alerts?.body.kind !== 'nothing') throw new Error('unreachable');
    // The alerts did fire; none is still open. That is not "no alerts".
    expect(alerts.body.sentence).toContain('7 fired');
  });

  it('says why a section is missing instead of showing it as empty', () => {
    const handover = fullHandover();
    handover.alerts = null;
    handover.changes = null;
    handover.config_changes = null;
    handover.backups = null;
    handover.unavailable = [
      { section: 'alerts', reason: 'not_installed', message: 'no alerting in this build' },
      { section: 'changes', reason: 'timeout', message: '`changes.list` did not answer in time' },
      { section: 'config_changes', reason: 'failed', message: 'the snapshot table could not be read' },
      { section: 'backups', reason: 'not_installed', message: 'no configuration feature in this build' },
    ];
    const sections = handoverSections(handover);
    const body = (id: string) => sections.find((section) => section.id === id)?.body;
    expect(body('alerts')).toEqual({
      kind: 'unavailable',
      reason: 'not_installed',
      message: 'no alerting in this build',
    });
    expect(body('changes')?.kind).toBe('unavailable');
    expect(body('config_changes')?.kind).toBe('unavailable');
    expect(body('backups')?.kind).toBe('unavailable');
    // Our own records are never null, so they are still there.
    expect(body('incidents')?.kind).toBe('items');
    expect(body('devices')?.kind).toBe('items');
    // And no summary line pretends to a figure it does not have.
    expect(sections.find((section) => section.id === 'alerts')?.summary).toBeUndefined();
  });

  it('puts the measurement limits of the devices section in words', () => {
    const sections = handoverSections(fullHandover());
    const devices = sections.find((section) => section.id === 'devices');
    expect(devices?.note).toContain('2 devices');
    expect(devices?.note).toContain('unknown rather than no');
  });

  it('marks a configuration change nobody can account for, and admits when it cannot tell', () => {
    const handover = fullHandover();
    const sections = handoverSections(handover);
    const configs = sections.find((section) => section.id === 'config_changes');
    if (configs?.body.kind !== 'items') throw new Error('expected items');
    expect(configs.body.items[0]?.detail).toContain('no change of ours covers it');
    expect(configs.summary).toContain('1 of them unplanned');

    // Without a change list to check against, nothing is called unplanned.
    const first = handover.config_changes?.[0];
    if (!first) throw new Error('expected a configuration change');
    first.checked = false;
    first.unplanned = false;
    const unchecked = handoverSections(handover).find((section) => section.id === 'config_changes');
    if (unchecked?.body.kind !== 'items') throw new Error('expected items');
    expect(unchecked.body.items[0]?.detail).toContain('nothing to check it against');
    expect(unchecked.summary).toContain('0 of them unplanned');
  });

  it('sends a removed device to the only record left of it', () => {
    const sections = handoverSections(fullHandover());
    const devices = sections.find((section) => section.id === 'devices');
    if (devices?.body.kind !== 'items') throw new Error('expected items');
    const removed = devices.body.items.find((item) => item.id.startsWith('removed:'));
    expect(removed?.to).toBe('/ops/audit?device=d9');
    expect(removed?.detail).toContain('audit entry 12');
  });

  it('carries the LAB mark through from a simulated device', () => {
    const handover = fullHandover();
    handover.devices.added = [device('d3', 'Campus/SW1', { lab: true })];
    const sections = handoverSections(handover);
    const devices = sections.find((section) => section.id === 'devices');
    if (devices?.body.kind !== 'items') throw new Error('expected items');
    expect(devices.body.items.find((item) => item.id === 'added:d3')?.lab).toBe(true);
  });
});

// -------------------------------------------------------------------- reports

describe('a report', () => {
  const base: GeneratedReport = {
    kind: 'availability',
    title: 'Availability',
    format: 'markdown',
    filename: 'availability.md',
    content: '# Availability',
    rows: 12,
    unavailable: [],
    period: { since: 1_000, until: 9_000 },
    generated_at: 9_000,
  };

  it('states its coverage, and counts its rows', () => {
    const lines = reportCoverage(base);
    expect(lines[0]).toContain('covers the period');
    expect(lines.join(' ')).toContain('12 rows');
  });

  it('keeps "nothing to report" and "could not be read" apart', () => {
    const empty = reportCoverage({ ...base, rows: 0 });
    expect(empty.join(' ')).toContain('there was nothing to report');

    const blind = reportCoverage({
      ...base,
      rows: 0,
      unavailable: [{ section: 'findings', reason: 'timeout', message: 'compliance timed out' }],
    });
    expect(blind.join(' ')).toContain('cannot be read as "nothing happened"');
    expect(blind.join(' ')).toContain('1 section');
    expect(blind.join(' ')).not.toContain('there was nothing to report');
  });

  it('says when it describes the present rather than a period', () => {
    expect(reportCoverage({ ...base, period: null })[0]).toContain('describes the present');
  });
});

// ---------------------------------------------------------------------- links

describe('links out of the record', () => {
  it('sends each kind of reference somewhere it can be checked', () => {
    expect(refLink({ type: 'change', id: 'c1' })).toBe('/changes/c1');
    expect(refLink({ type: 'incident', id: 'i1' })).toBe('/ops/incidents?incident=i1');
    expect(refLink({ type: 'job', id: 'j1' })).toBe('/ops/jobs?job=j1');
    expect(refLink({ type: 'device', id: 'd 1' })).toBe('/devices/d%201');
    expect(refLink({ type: 'alert', id: 'a1' }, 'd1')).toBe('/monitoring/health?device=d1');
    expect(refLink({ type: 'alert', id: 'a1' })).toBe('/monitoring/alerts');
    // A configuration version cannot be addressed without its device.
    expect(refLink({ type: 'config_version', id: '7' })).toBeNull();
    expect(refLink({ type: 'config_version', id: '7' }, 'd1')).toBe('/config?device=d1');
    // Something this build has no screen for goes nowhere, rather than to a
    // route that does not exist.
    expect(refLink({ type: 'policy', id: 'p1' })).toBeNull();
  });
});

// ---------------------------------------------------------------------- audit

describe('the audit trail', () => {
  it('groups actions by the feature that recorded them', () => {
    const groups = actionGroups([
      { action: 'incidents.create', count: 4 },
      { action: 'incidents.note', count: 9 },
      { action: 'device.create', count: 2 },
      { action: 'vault.unlock', count: 1 },
    ]);
    expect(groups.map((group) => [group.group, group.count])).toEqual([
      ['device.', 2],
      ['incidents.', 13],
      ['vault.', 1],
    ]);
  });

  it('does not make a deletion look like a read', () => {
    expect(actionTone('incidents.delete')).toBe('warn');
    expect(actionTone('changes.apply')).toBe('crit');
    expect(actionTone('incidents.note')).toBe('idle');
  });
});

describe('jobs', () => {
  it('has no percentage while the amount of work is unknown', () => {
    expect(jobProgress({ done: 3, total: 4 })).toBe(75);
    expect(jobProgress({ done: 3, total: 0 })).toBeNull();
  });
});

describe('periods', () => {
  it('falls back to the first choice rather than throwing on an unknown id', () => {
    expect(periodChoice('12h').label).toBe('Last 12 hours');
    expect(periodChoice('nonsense').id).toBe('8h');
  });

  it('measures "since midnight" from local midnight', () => {
    const now = new Date(2026, 10, 14, 7, 30).getTime();
    const since = periodChoice('midnight').since(now);
    expect(new Date(since).getHours()).toBe(0);
    expect(new Date(since).getDate()).toBe(14);
  });
});
