import { describe, expect, test } from 'vitest';
import type { Run } from '@/api/types/automation';
import { DEVICE_STATE, describeRun, debounceText, listWords, runElapsed, whenText } from './vocab';

const run = (over: Partial<Run> = {}): Run => ({
  id: 'r1',
  kind: 'commands',
  name: 'show version',
  origin: 'user',
  job_id: 'j1',
  commands: ['show version'],
  started_at: 1_000,
  finished_at: 4_000,
  devices: 3,
  ok: 3,
  failed: 0,
  unreached: 0,
  skipped: 0,
  cancelled: false,
  dry_run: false,
  change_ids: [],
  ...over,
});

describe('the four outcomes are four different things', () => {
  test('each has its own word, tone and icon', () => {
    const tones = new Set(Object.values(DEVICE_STATE).map((words) => words.tone));
    const labels = new Set(Object.values(DEVICE_STATE).map((words) => words.label));
    const icons = new Set(Object.values(DEVICE_STATE).map((words) => words.icon));
    expect(tones.size).toBe(4);
    expect(labels.size).toBe(4);
    expect(icons.size).toBe(4);
  });

  test('a device that was not reached does not read as a failure', () => {
    expect(DEVICE_STATE.unreached.label).toBe('Not reached');
    expect(DEVICE_STATE.unreached.tone).not.toBe(DEVICE_STATE.failed.tone);
    expect(DEVICE_STATE.unreached.explain).toContain('nothing was sent');
    expect(DEVICE_STATE.failed.explain).toContain('refused');
  });

  test('a skipped device says nothing failed', () => {
    expect(DEVICE_STATE.skipped.explain).toContain('nothing failed');
    expect(DEVICE_STATE.skipped.tone).toBe('idle');
  });
});

describe('a finished run in one sentence', () => {
  test('every device answered', () => {
    const outcome = describeRun(run());
    expect(outcome.title).toBe('3 devices answered');
    expect(outcome.tone).toBe('ok');
  });

  test('a failure leads, and names the count', () => {
    const outcome = describeRun(run({ ok: 2, failed: 1 }));
    expect(outcome.title).toBe('Failed on 1 device');
    expect(outcome.tone).toBe('crit');
    expect(outcome.detail).toBe('2 answered · 1 failed');
  });

  test('devices that could not be reached are their own sentence, not a failure', () => {
    const outcome = describeRun(run({ ok: 1, unreached: 2 }));
    expect(outcome.title).toBe('2 devices could not be reached');
    expect(outcome.tone).toBe('warn');
    expect(outcome.detail).toBe('1 answered · 2 not reached');
  });

  test('skipped devices are counted separately from everything else', () => {
    expect(describeRun(run({ ok: 2, skipped: 1 })).detail).toBe('2 answered · 1 skipped');
    expect(describeRun(run({ ok: 0, skipped: 3 })).title).toBe('Every device was skipped');
  });

  test('a stopped run says so instead of claiming a result', () => {
    expect(describeRun(run({ cancelled: true, ok: 1, skipped: 2 })).title).toBe('Stopped');
  });

  test('a run still going is not reported as a success', () => {
    const live = describeRun(run({ finished_at: undefined, ok: 0 }));
    expect(live.title).toBe('Running');
    expect(live.tone).toBe('info');
  });

  test('a dry run never claims a device answered', () => {
    expect(describeRun(run({ dry_run: true })).title).toBe('Dry run over 3 devices');
  });

  test('how long it took, and how long it has been going', () => {
    expect(runElapsed(run())).toBe('3s');
    expect(runElapsed(run({ finished_at: undefined }), 6_000)).toBe('5s');
  });
});

describe('when a schedule fires, in words', () => {
  test('an interval', () => {
    expect(whenText({ type: 'interval', every_s: 3600 })).toBe('Every 1h');
    expect(whenText({ type: 'interval', every_s: 90 })).toBe('Every 1m 30s');
  });

  test('daily, with and without days', () => {
    expect(whenText({ type: 'daily', at: '02:30' })).toBe('Every day at 02:30');
    expect(whenText({ type: 'daily', at: '02:30', days: [] })).toBe('Every day at 02:30');
    expect(whenText({ type: 'daily', at: '02:30', days: ['mon', 'wed', 'fri'] })).toBe(
      'Monday, Wednesday and Friday at 02:30',
    );
  });

  test('a cron expression is shown as written, never guessed at', () => {
    expect(whenText({ type: 'cron', expr: '*/15 * * * *' })).toBe('Cron: */15 * * * *');
  });

  test('a list of words', () => {
    expect(listWords([])).toBe('');
    expect(listWords(['Monday'])).toBe('Monday');
    expect(listWords(['Monday', 'Friday'])).toBe('Monday and Friday');
  });
});

describe('a trigger debounce', () => {
  test('zero is the next sweep, not "immediately"', () => {
    expect(debounceText(0)).toBe('Fires at the next sweep');
    expect(debounceText(30)).toBe('Collects events for 30s, then fires once');
  });
});
