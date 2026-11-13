import { describe, expect, it } from 'vitest';
import type { Change, Plan, Target } from '@/api/types/changes';
import {
  approvalRequest,
  binding,
  namedTargets,
  needsConfirmation,
  pruneReview,
  readiness,
  typedConfirmationOk,
} from './approval';

function plan(hash: string, hits = 0): Plan {
  return {
    commands: ['interface Gi0/1', 'description x', 'exit'],
    rollback: ['interface Gi0/1', 'no description', 'exit'],
    diff: { lines: [], added: 1, removed: 0 },
    risk: { level: hits > 0 ? 'critical' : 'low', reasons: ['because'] },
    protected_hits: Array.from({ length: hits }, (_value, index) => ({
      kind: 'access_vty',
      subject: `line vty ${index}`,
      command: 'login local',
      why: 'that is how Nocturne logs in',
    })),
    warnings: [],
    base_version_id: 1,
    base_sha256: 'base',
    predicted_sha256: 'predicted',
    plan_hash: hash,
    planned_at: 1,
    profile: 'cisco_ios',
    maturity: 'verified',
  };
}

function change(targets: Target[], approval?: Change['approval']): Change {
  return {
    id: 'c1',
    title: 'A change',
    description: '',
    origin: 'user',
    state: approval ? 'approved' : 'planned',
    targets,
    options: {},
    approval,
    created_at: 1,
    updated_at: 1,
    created_by: 'user',
  };
}

const NAMES = new Map([
  ['d1', 'sw-east-1'],
  ['d2', 'sw-east-2'],
]);

function target(deviceId: string, p?: Plan): Target {
  return { device_id: deviceId, intent: { type: 'lines', lines: ['x'] }, plan: p };
}

describe('typedConfirmationOk', () => {
  it('accepts the device name, trimmed and in any case, as the engine does', () => {
    expect(typedConfirmationOk('sw-east-1', 'sw-east-1')).toBe(true);
    expect(typedConfirmationOk('  SW-East-1 ', 'sw-east-1')).toBe(true);
  });

  it('accepts nothing else: not "yes", not a tick, not a near miss', () => {
    expect(typedConfirmationOk('yes', 'sw-east-1')).toBe(false);
    expect(typedConfirmationOk('true', 'sw-east-1')).toBe(false);
    expect(typedConfirmationOk('sw-east', 'sw-east-1')).toBe(false);
    expect(typedConfirmationOk('sw-east-11', 'sw-east-1')).toBe(false);
    expect(typedConfirmationOk('', 'sw-east-1')).toBe(false);
    expect(typedConfirmationOk(undefined, 'sw-east-1')).toBe(false);
  });
});

describe('readiness', () => {
  const safe = namedTargets(change([target('d1', plan('h1'))]), NAMES);
  const risky = namedTargets(change([target('d1', plan('h1', 2))]), NAMES);

  it('will not approve a plan nobody has opened', () => {
    const gate = readiness(safe, new Map(), { seen: new Set(), declared: false });
    expect(gate.ready).toBe(false);
    expect(gate.blockers.map((blocker) => blocker.text).join(' ')).toContain('read the commands');
  });

  it('is satisfied by opening the plan', () => {
    const gate = readiness(safe, new Map(), { seen: new Set(['h1']), declared: false });
    expect(gate.ready).toBe(true);
  });

  it('is satisfied by saying outright that the plans were read', () => {
    expect(readiness(safe, new Map(), { seen: new Set(), declared: true }).ready).toBe(true);
  });

  it('holds a protected device back until its name is typed', () => {
    const read = { seen: new Set(['h1']), declared: false };
    const gate = readiness(risky, new Map(), read);
    expect(gate.ready).toBe(false);
    expect(gate.awaiting).toHaveLength(1);
    expect(gate.blockers.map((blocker) => blocker.text).join(' ')).toContain('sw-east-1');

    expect(gate.blockers.some((blocker) => blocker.kind === 'confirm')).toBe(true);
    expect(readiness(risky, new Map([['d1', 'nope']]), read).ready).toBe(false);
    expect(readiness(risky, new Map([['d1', 'sw-east-1']]), read).ready).toBe(true);
  });

  it('refuses a target that has no plan', () => {
    const unplanned = namedTargets(change([target('d1')]), NAMES);
    const gate = readiness(unplanned, new Map(), { seen: new Set(), declared: true });
    expect(gate.ready).toBe(false);
    expect(gate.blockers.map((blocker) => blocker.text).join(' ')).toContain('no plan');
  });
});

describe('approvalRequest', () => {
  it('sends the hashes of the plans on screen, and the names as they were typed', () => {
    const targets = namedTargets(
      change([target('d1', plan('h1', 1)), target('d2', plan('h2'))]),
      NAMES,
    );
    const request = approvalRequest(targets, new Map([['d1', ' SW-East-1 ']]));
    expect(request.plan_hashes).toEqual(['h1', 'h2']);
    // Keyed by name, which is what the engine looks for.
    expect(request.confirm).toEqual({ 'sw-east-1': ' SW-East-1 ' });
  });

  it('asks for a confirmation only where there are protected hits', () => {
    const targets = namedTargets(change([target('d1', plan('h1')), target('d2', plan('h2'))]), NAMES);
    expect(needsConfirmation(targets)).toHaveLength(0);
    expect(approvalRequest(targets, new Map()).confirm).toEqual({});
  });
});

describe('binding — an approval covers the plans it was given, and no others', () => {
  it('holds while the stored plans are the approved ones', () => {
    const approved = change([target('d1', plan('h1')), target('d2', plan('h2'))], {
      by: 'user',
      at: 1,
      plan_hashes: ['h1', 'h2'],
      note: '',
      confirmed_devices: [],
    });
    expect(binding(approved, NAMES).bound).toBe(true);
  });

  it('breaks when a plan was worked out again after it was signed', () => {
    const replanned = change([target('d1', plan('h1-NEW')), target('d2', plan('h2'))], {
      by: 'user',
      at: 1,
      plan_hashes: ['h1', 'h2'],
      note: '',
      confirmed_devices: [],
    });
    const result = binding(replanned, NAMES);
    expect(result.bound).toBe(false);
    expect(result.stale.map((entry) => entry.name)).toEqual(['sw-east-1']);
    expect(result.reason).toContain('sw-east-1');
    expect(result.reason).toContain('approve them again');
  });

  it('breaks when a plan disappeared altogether', () => {
    const gone = change([target('d1')], {
      by: 'user',
      at: 1,
      plan_hashes: ['h1'],
      note: '',
      confirmed_devices: [],
    });
    expect(binding(gone, NAMES).bound).toBe(false);
    expect(binding(gone, NAMES).reason).toContain('no plan');
  });

  it('refuses a change with no approval at all', () => {
    expect(binding(change([target('d1', plan('h1'))]), NAMES).bound).toBe(false);
  });
});

describe('pruneReview', () => {
  it('forgets a plan that has been replaced', () => {
    const targets = namedTargets(change([target('d1', plan('h1-NEW'))]), NAMES);
    const pruned = pruneReview({ seen: new Set(['h1']), declared: false }, targets);
    expect([...pruned.seen]).toEqual([]);
  });

  it('withdraws "I have reviewed the plans" when the plans changed', () => {
    const targets = namedTargets(change([target('d1', plan('h1-NEW'))]), NAMES);
    expect(pruneReview({ seen: new Set(['h1']), declared: true }, targets).declared).toBe(false);
  });

  it('keeps a review that still matches', () => {
    const targets = namedTargets(change([target('d1', plan('h1'))]), NAMES);
    const kept = pruneReview({ seen: new Set(['h1']), declared: true }, targets);
    expect([...kept.seen]).toEqual(['h1']);
    expect(kept.declared).toBe(true);
  });
});
