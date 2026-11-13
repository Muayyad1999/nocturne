/**
 * The approval gate.
 *
 * An approval is a signature on *specific* plans: `changes.approve` names the
 * `plan_hash` of every target, and the engine refuses one that is not the
 * plan it holds. This module is the screen's half of that contract, kept pure
 * so it can be tested without a backend:
 *
 * * which devices need their name typed out, and whether what was typed is
 *   that name (a tick box is never enough — `typedConfirmationOk`);
 * * whether the person has read what they are signing (`review`);
 * * whether the approval on a change still covers the plans that are stored
 *   now (`binding`) — if it does not, the screen refuses to apply and says
 *   why rather than quietly planning again.
 */

import type { Change, Plan, Target } from '@/api/types/changes';
import { plural } from './change';

/** A target with its plan and the device's display name. */
export interface NamedTarget {
  target: Target;
  deviceId: string;
  /** The device's name as the inventory has it: what must be typed. */
  name: string;
  plan: Plan | undefined;
}

/** Pairs every target with the name the engine will expect to be typed. */
export function namedTargets(change: Change, names: ReadonlyMap<string, string>): NamedTarget[] {
  return change.targets.map((target) => ({
    target,
    deviceId: target.device_id,
    name: names.get(target.device_id) ?? target.device_id,
    plan: target.plan ?? undefined,
  }));
}

/**
 * Is what the user typed the device's name? The engine compares
 * case-insensitively after trimming, and this must agree with it exactly: a
 * screen that accepted less would send an approval the engine then refuses,
 * and one that accepted more would be a lie.
 */
export function typedConfirmationOk(typed: string | undefined, name: string): boolean {
  return typed !== undefined && typed.trim().toLowerCase() === name.trim().toLowerCase();
}

/** Devices whose plan touches a protected resource, so the name must be typed. */
export function needsConfirmation(targets: readonly NamedTarget[]): NamedTarget[] {
  return targets.filter((entry) => (entry.plan?.protected_hits.length ?? 0) > 0);
}

export interface ApprovalRequest {
  /** The hashes of the plans on screen, in target order. */
  plan_hashes: string[];
  /** Device name → the name as it was typed, for every protected device. */
  confirm: Record<string, string>;
}

/**
 * What to send to `changes.approve`: the hashes of exactly the plans that are
 * on screen. If a plan changed while the person was reading, the hash they
 * send is the one they read, and the engine refuses it — which is the point.
 */
export function approvalRequest(
  targets: readonly NamedTarget[],
  typed: ReadonlyMap<string, string>,
): ApprovalRequest {
  const confirm: Record<string, string> = {};
  for (const entry of needsConfirmation(targets)) {
    confirm[entry.name] = typed.get(entry.deviceId) ?? '';
  }
  return {
    plan_hashes: targets.flatMap((entry) => (entry.plan ? [entry.plan.plan_hash] : [])),
    confirm,
  };
}

/** One thing standing between the user and approval. */
export interface Blocker {
  /** What kind it is, so the bar can leave out what it already says itself. */
  kind: 'empty' | 'unplanned' | 'confirm' | 'review';
  text: string;
}

export interface ApprovalReadiness {
  ready: boolean;
  /** Everything standing between the user and approval, in reading order. */
  blockers: Blocker[];
  /** Devices still waiting for their name to be typed. */
  awaiting: NamedTarget[];
}

export interface ReviewState {
  /** `plan_hash` of every plan the person has opened and looked at. */
  seen: ReadonlySet<string>;
  /** They ticked "I have reviewed the plans" instead of opening each one. */
  declared: boolean;
}

/**
 * May this change be approved from the screen? The rules are the spec's:
 * every target planned, every protected device's name typed out, and every
 * plan either expanded once or explicitly declared as read.
 */
export function readiness(
  targets: readonly NamedTarget[],
  typed: ReadonlyMap<string, string>,
  review: ReviewState,
): ApprovalReadiness {
  const blockers: Blocker[] = [];
  const unplanned = targets.filter((entry) => entry.plan === undefined);
  if (targets.length === 0) blockers.push({ kind: 'empty', text: 'This change has no devices.' });
  if (unplanned.length > 0) {
    blockers.push({
      kind: 'unplanned',
      text: `${unplanned.map((entry) => entry.name).join(', ')} ${unplanned.length === 1 ? 'has' : 'have'} no plan yet. Plan the change first.`,
    });
  }

  const awaiting = needsConfirmation(targets).filter(
    (entry) => !typedConfirmationOk(typed.get(entry.deviceId), entry.name),
  );
  if (awaiting.length > 0) {
    blockers.push({
      kind: 'confirm',
      text: `Type the name of ${awaiting.map((entry) => entry.name).join(' and ')} to confirm the protected resources this change touches.`,
    });
  }

  const unseen = targets.filter((entry) => entry.plan && !review.seen.has(entry.plan.plan_hash));
  if (!review.declared && unseen.length > 0) {
    blockers.push({
      kind: 'review',
      text: `Open ${unseen.length === targets.length ? (targets.length === 1 ? 'the plan' : 'every plan') : plural(unseen.length, 'plan')} to read the commands, or tick "I have reviewed the plans".`,
    });
  }

  return { ready: blockers.length === 0 && unplanned.length === 0 && targets.length > 0, blockers, awaiting };
}

export interface Binding {
  /** The approval covers exactly the plans stored now. */
  bound: boolean;
  /** Devices whose stored plan is not the one that was approved. */
  stale: NamedTarget[];
  /** Why the screen will not apply this, in one sentence for the approver. */
  reason?: string;
}

/**
 * Does the approval on this change still cover the plans it holds?
 *
 * The engine checks this too (`changes.apply` answers `conflict`), and this is
 * the screen's own guard in front of it: an approved change whose plan has
 * been replaced is never applied from here, and never quietly planned again
 * either. The person is told which device's plan changed and asked to read it.
 */
export function binding(change: Change, names: ReadonlyMap<string, string>): Binding {
  const targets = namedTargets(change, names);
  const approval = change.approval;
  if (!approval) {
    return {
      bound: false,
      stale: [],
      reason: 'This change carries no approval, so it cannot be applied.',
    };
  }
  const missing = targets.filter((entry) => entry.plan === undefined);
  if (missing.length > 0) {
    return {
      bound: false,
      stale: missing,
      reason: `${missing.map((entry) => entry.name).join(', ')} has no plan any more. Plan the change again and approve what you see.`,
    };
  }
  const stale = targets.filter(
    (entry) => entry.plan !== undefined && !approval.plan_hashes.includes(entry.plan.plan_hash),
  );
  if (stale.length > 0) {
    return {
      bound: false,
      stale,
      reason: `The plan for ${stale.map((entry) => entry.name).join(', ')} is not the one that was approved — it was planned again after it was signed off. Nocturne will not apply a plan nobody has read: look at the commands and approve them again.`,
    };
  }
  return { bound: true, stale: [] };
}

/**
 * The review is void when the plans are not the ones that were read. Called
 * whenever a change arrives from the backend, so a re-plan behind the
 * screen's back cannot inherit a tick from the plan it replaced.
 */
export function pruneReview(review: ReviewState, targets: readonly NamedTarget[]): ReviewState {
  const current = new Set(targets.flatMap((entry) => (entry.plan ? [entry.plan.plan_hash] : [])));
  const seen = new Set([...review.seen].filter((hash) => current.has(hash)));
  // "I have reviewed the plans" was said about the plans that were on screen.
  const declared = review.declared && seen.size === current.size;
  return { seen, declared };
}
