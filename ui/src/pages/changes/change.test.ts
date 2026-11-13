import { describe, expect, it } from 'vitest';
import type { Change, Plan, Step, TargetResult } from '@/api/types/changes';
import {
  canApprove,
  canPlan,
  diffTagLabel,
  hitKindLabel,
  isFinished,
  plural,
  predictedLines,
  rollbackCandidates,
  stateTone,
  stepper,
} from './change';

function steps(...entries: [string, boolean][]): Step[] {
  return entries.map(([name, ok], index) => ({ name, ok, detail: `${name} detail`, at: index }));
}

function result(state: TargetResult['state'], ...entries: [string, boolean][]): TargetResult {
  return {
    state,
    started_at: 0,
    steps: steps(...entries),
    sent: [],
    verification: [],
    transcript: '',
  };
}

describe('stepper', () => {
  it('shows what is still to come while a device is being configured', () => {
    const rows = stepper(result('applying', ['connect', true], ['drift check', true]), {});
    expect(rows.slice(0, 2).map((row) => row.status)).toEqual(['done', 'done']);
    expect(rows[2]).toMatchObject({ name: 'snapshot', status: 'running' });
    expect(rows.slice(3).every((row) => row.status === 'waiting')).toBe(true);
    // Nothing invents a guard the plan does not arm.
    expect(rows.some((row) => row.name === 'arm guard')).toBe(false);
  });

  it('includes the dead-man switch when the plan arms one', () => {
    const rows = stepper(result('applying', ['connect', true]), { guard: true });
    expect(rows.some((row) => row.name === 'arm guard')).toBe(true);
    expect(rows.some((row) => row.name === 'disarm guard')).toBe(true);
  });

  it('turns to rolling back once a step has failed', () => {
    const rows = stepper(result('applying', ['connect', true], ['apply', false]), {});
    expect(rows[1]).toMatchObject({ name: 'apply', status: 'failed' });
    expect(rows[2]).toMatchObject({ name: 'rollback', status: 'running' });
    expect(rows).toHaveLength(3);
  });

  it('shows only what happened once the device has stopped', () => {
    const rows = stepper(result('applied', ['connect', true], ['apply', true], ['verify', true]), {});
    expect(rows.map((row) => row.name)).toEqual(['connect', 'apply', 'verify']);
    expect(rows.every((row) => row.status === 'done')).toBe(true);
  });

  it('has nothing to show for a device that has not started', () => {
    expect(stepper(undefined, {}).every((row) => row.status !== 'done')).toBe(true);
  });
});

describe('the states of a change', () => {
  it('knows which are finished', () => {
    expect(isFinished('applied')).toBe(true);
    expect(isFinished('partially_applied')).toBe(true);
    expect(isFinished('applying')).toBe(false);
    expect(isFinished('planned')).toBe(false);
  });

  it('only approves a planned change, as the engine does', () => {
    expect(canApprove('planned')).toBe(true);
    expect(canApprove('draft')).toBe(false);
    expect(canApprove('approved')).toBe(false);
    expect(canApprove('applied')).toBe(false);
  });

  it('does not offer to plan again what was actually applied', () => {
    expect(canPlan('failed')).toBe(true);
    expect(canPlan('rolled_back')).toBe(true);
    expect(canPlan('applied')).toBe(false);
    expect(canPlan('partially_applied')).toBe(false);
    expect(canPlan('applying')).toBe(false);
  });

  it('pairs each state with a tone that is paired with a word on screen', () => {
    expect(stateTone('applied')).toBe('ok');
    expect(stateTone('failed')).toBe('crit');
    expect(stateTone('planned')).toBe('warn');
  });
});

describe('rollbackCandidates', () => {
  const change = (...results: (TargetResult | undefined)[]): Change => ({
    id: 'c',
    title: 't',
    description: '',
    origin: 'user',
    state: 'applied',
    targets: results.map((entry, index) => ({
      device_id: `d${index}`,
      intent: { type: 'lines', lines: ['x'] },
      result: entry,
    })),
    options: {},
    created_at: 0,
    updated_at: 0,
    created_by: 'user',
  });

  it('only offers devices that were applied and have a backup of before', () => {
    const applied = { ...result('applied', ['apply', true]), before_version_id: 7 };
    const noBackup = result('applied', ['apply', true]);
    const rolled = { ...result('rolled_back', ['rollback', true]), before_version_id: 8 };
    expect(rollbackCandidates(change(applied, noBackup, rolled, undefined))).toEqual(['d0']);
  });
});

describe('predictedLines', () => {
  it('reads the predicted side out of the engine’s diff, gaps marked', () => {
    const plan = {
      diff: {
        lines: [
          { tag: ' ', text: 'hostname sw1' },
          { tag: '-', text: 'no ip routing' },
          { tag: '+', text: 'ip routing' },
          { tag: '@', text: '…' },
        ],
        added: 1,
        removed: 1,
      },
    } as Plan;
    expect(predictedLines(plan)).toEqual(['hostname sw1', 'ip routing', '…']);
  });
});

describe('words', () => {
  it('counts', () => {
    expect(plural(1, 'device')).toBe('1 device');
    expect(plural(3, 'device')).toBe('3 devices');
    expect(plural(2, 'protected resource')).toBe('2 protected resources');
  });

  it('turns a protected-resource rule into something an approver can act on', () => {
    expect(hitKindLabel('access_vty')).toBe('Login lines');
    expect(hitKindLabel('trunk_allowed_replace')).toContain('replaced');
    // A rule this build has no word for is still shown, not swallowed.
    expect(hitKindLabel('some_new_rule')).toBe('some new rule');
  });

  it('names a diff line for a screen reader', () => {
    expect(diffTagLabel({ tag: '+', text: 'x' })).toBe('added');
    expect(diffTagLabel({ tag: '-', text: 'x' })).toBe('removed');
    expect(diffTagLabel({ tag: ' ', text: 'x' })).toBe('unchanged');
  });
});
