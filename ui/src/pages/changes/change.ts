/**
 * Words and tones for the change pipeline, and the small decisions a screen
 * makes from a change record. Everything here is pure: no React, no RPC, so
 * the rules that decide whether a change may be applied can be unit tested
 * on their own.
 */

import type {
  Change,
  ChangeOrigin,
  ChangeState,
  DiffLine,
  Plan,
  RiskLevel,
  Step,
  TargetResult,
  TargetState,
} from '@/api/types/changes';
import type { StatusTone } from '@/design/tone';

// --------------------------------------------------------------------- words

export const STATE_LABEL: Record<ChangeState, string> = {
  draft: 'Draft',
  planned: 'Waiting for approval',
  approved: 'Approved',
  applying: 'Applying',
  applied: 'Applied',
  failed: 'Failed',
  rolled_back: 'Rolled back',
  partially_applied: 'Partly applied',
  cancelled: 'Cancelled',
};

/**
 * What the state means for the person reading it: one sentence under the
 * title, so nobody has to know the pipeline to know where they are.
 */
export const STATE_SENTENCE: Record<ChangeState, string> = {
  draft: 'Nothing has been worked out yet. Plan it to see the exact commands.',
  planned: 'The commands below are what will be sent. Read them, then approve.',
  approved: 'Approved and bound to these exact plans. It has not been applied yet.',
  applying: 'Being applied now: backup, drift check, send, settle, verify.',
  applied: 'Applied and verified on every device.',
  failed: 'Nothing was left changed on the devices that failed.',
  rolled_back: 'Undone: every device is back to the configuration it had before.',
  partially_applied: 'Some devices were changed and some were not. Read each one.',
  cancelled: 'Cancelled before it reached a device.',
};

export const TARGET_STATE_LABEL: Record<TargetState, string> = {
  pending: 'Waiting',
  applying: 'Applying',
  applied: 'Applied',
  failed: 'Failed',
  rolled_back: 'Rolled back',
  rollback_failed: 'Rollback failed',
  skipped: 'Skipped',
  guard_fired: 'Dead-man switch will fire',
};

export const RISK_LABEL: Record<RiskLevel, string> = {
  low: 'Low risk',
  medium: 'Medium risk',
  high: 'High risk',
  critical: 'Critical risk',
};

export const ORIGIN_LABEL: Record<ChangeOrigin, string> = {
  user: 'A person',
  assistant: 'Assistant',
  template: 'Template',
  compliance: 'Compliance',
  restore: 'Restore',
  drift: 'Drift',
  playbook: 'Playbook',
};

/** The protected-resource rules, in words an approver can act on. */
const HIT_KIND_LABEL: Record<string, string> = {
  protected_device: 'Protected device',
  management_interface: 'Management interface',
  management_vlan: 'Management VLAN',
  default_route: 'Default route',
  default_gateway: 'Default gateway',
  ip_routing: 'IP routing',
  access_vty: 'Login lines',
  access_user: 'Login account',
  access_enable: 'Enable secret',
  access_aaa: 'AAA',
  access_ssh: 'SSH',
  access_crypto: 'Crypto keys',
  access_http: 'HTTP server',
  access_acl: 'Access list',
  access_service: 'Management service',
  access_firewall: 'Firewall',
  trunk_allowed_replace: 'Trunk allowed-VLAN list (replaced)',
  trunk_allowed: 'Trunk allowed-VLAN list',
  uplink: 'Uplink',
  vlan_removal: 'VLAN removal',
  spanning_tree: 'Spanning tree',
  routing_removal: 'Routing',
  hostname: 'Hostname',
};

/** A rule this build has no word for is shown as the backend named it. */
export function hitKindLabel(kind: string): string {
  return HIT_KIND_LABEL[kind] ?? kind.replace(/_/g, ' ');
}

/** `1 device` / `3 devices`. */
export function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

// --------------------------------------------------------------------- tones

export function stateTone(state: ChangeState): StatusTone {
  switch (state) {
    case 'applied':
      return 'ok';
    case 'failed':
      return 'crit';
    case 'partially_applied':
    case 'rolled_back':
    case 'planned':
      return 'warn';
    case 'applying':
    case 'approved':
      return 'info';
    case 'cancelled':
    case 'draft':
      return 'idle';
  }
}

export function riskTone(level: RiskLevel): StatusTone {
  switch (level) {
    case 'low':
      return 'ok';
    case 'medium':
      return 'warn';
    case 'high':
    case 'critical':
      return 'crit';
  }
}

export function targetStateTone(state: TargetState): StatusTone {
  switch (state) {
    case 'applied':
      return 'ok';
    case 'applying':
      return 'info';
    case 'failed':
    case 'rollback_failed':
    case 'guard_fired':
      return 'crit';
    case 'rolled_back':
      return 'warn';
    case 'pending':
    case 'skipped':
      return 'idle';
  }
}

// ------------------------------------------------------------------- the flow

/** A change that has reached its end: nothing more will happen by itself. */
export function isFinished(state: ChangeState): boolean {
  return (
    state === 'applied' ||
    state === 'failed' ||
    state === 'rolled_back' ||
    state === 'partially_applied' ||
    state === 'cancelled'
  );
}

/**
 * Whether the backend will plan this again. It refuses for a change whose
 * devices were actually changed: those results are the record of what
 * happened and the rollback still needs them.
 */
export function canPlan(state: ChangeState): boolean {
  return state === 'draft' || state === 'planned' || state === 'approved' || state === 'failed' || state === 'rolled_back';
}

/** Only a planned change can be approved (`changes.approve`). */
export function canApprove(state: ChangeState): boolean {
  return state === 'planned';
}

/** Title, description and ticket are editable; targets only while editable. */
export function canEdit(state: ChangeState): boolean {
  return state === 'draft' || state === 'planned';
}

export function canCancel(state: ChangeState): boolean {
  return !isFinished(state);
}

/** On-demand rollback needs a device that was applied with a backup of before. */
export function rollbackCandidates(change: Change): string[] {
  return change.targets
    .filter(
      (target) =>
        target.result?.before_version_id != null &&
        (target.result.state === 'applied' || target.result.state === 'rollback_failed'),
    )
    .map((target) => target.device_id);
}

// ----------------------------------------------------------------- the stepper

/**
 * The pipeline in the order it runs, so a device that has only reached
 * "backup" still shows what is coming. `reconnect`, `guard`, `rollback` and
 * `disarm guard` only appear when the engine reports them.
 */
export const APPLY_STEPS = [
  'connect',
  'drift check',
  'snapshot',
  'backup',
  'arm guard',
  'apply',
  'verify',
  'disarm guard',
  'save',
  'final backup',
] as const;

export const STEP_LABEL: Record<string, string> = {
  connect: 'Connect',
  'drift check': 'Drift check',
  snapshot: 'Snapshot',
  backup: 'Backup',
  'arm guard': 'Arm dead-man switch',
  apply: 'Send',
  verify: 'Verify',
  rollback: 'Roll back',
  reconnect: 'Reconnect',
  guard: 'Dead-man switch',
  'disarm guard': 'Disarm dead-man switch',
  save: 'Save',
  'final backup': 'Backup afterwards',
};

export function stepLabel(name: string): string {
  return STEP_LABEL[name] ?? name;
}

export type StepStatus = 'done' | 'failed' | 'running' | 'waiting';

export interface StepperRow {
  name: string;
  label: string;
  status: StepStatus;
  detail?: string;
  at?: number;
}

/**
 * The live stepper for one device: every step the engine has reported, in the
 * order it reported them, followed by the ones still to come while the device
 * is working. Nothing is invented — a step that has not happened is
 * "waiting", never "done".
 */
export function stepper(result: TargetResult | undefined, options: { guard?: boolean } = {}): StepperRow[] {
  const steps: Step[] = result?.steps ?? [];
  const running = result?.state === 'applying' || result === undefined;
  const rows: StepperRow[] = steps.map((step) => ({
    name: step.name,
    label: stepLabel(step.name),
    status: step.ok ? 'done' : 'failed',
    detail: step.detail,
    at: step.at,
  }));

  // While a device is still working, show what is still to come. Once it has
  // stopped, the steps that happened are the whole truth.
  if (!running) return rows;

  const reported = new Set(steps.map((step) => step.name));
  const failed = steps.some((step) => !step.ok);
  const upcoming = APPLY_STEPS.filter((name) => {
    if (reported.has(name)) return false;
    if (name === 'arm guard' || name === 'disarm guard') return options.guard === true;
    return true;
  });
  // After a failure the pipeline rolls back instead of carrying on.
  if (failed) return [...rows, { name: 'rollback', label: stepLabel('rollback'), status: 'running' }];

  const [next, ...rest] = upcoming;
  return [
    ...rows,
    ...(next === undefined ? [] : [{ name: next, label: stepLabel(next), status: 'running' as StepStatus }]),
    ...rest.map((name) => ({ name, label: stepLabel(name), status: 'waiting' as StepStatus })),
  ];
}

// -------------------------------------------------------------------- the diff

export interface DiffCounts {
  added: number;
  removed: number;
}

/** `+5 −2`, or nothing to show when the prediction is no different. */
export function diffCounts(plan: Plan | undefined): DiffCounts {
  return { added: plan?.diff.added ?? 0, removed: plan?.diff.removed ?? 0 };
}

/** A diff line's accessible prefix: screen readers must not rely on colour. */
export function diffTagLabel(line: DiffLine): string {
  switch (line.tag) {
    case '+':
      return 'added';
    case '-':
      return 'removed';
    case '@':
      return 'unchanged lines skipped';
    default:
      return 'unchanged';
  }
}

/**
 * The predicted configuration, as far as the diff shows it: the context and
 * added lines, with a marker where unchanged lines were left out. This is the
 * engine's own prediction re-read, never a re-computation of it.
 */
export function predictedLines(plan: Plan): string[] {
  return plan.diff.lines
    .filter((line) => line.tag !== '-')
    .map((line) => (line.tag === '@' ? '…' : line.text));
}
