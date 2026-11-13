/**
 * What the engine is doing to one device, step by step, while it does it.
 *
 * The rows come from the target's own `steps` (the engine appends one per
 * stage and publishes `changes.progress` at the same moment), followed by the
 * stages still to come while the device is working. A step nobody has
 * reported is "waiting" — never a tick.
 */

import { Check, Circle, CircleDashed, X } from 'lucide-react';
import type { Check as VerifyCheck, TargetResult } from '@/api/types/changes';
import { Spinner } from '@/design/Spinner';
import { formatTime } from '@/lib/time';
import { stepper } from './change';
import styles from './ApplyStepper.module.css';

export interface ApplyStepperProps {
  result: TargetResult | undefined;
  /** The plan arms a dead-man switch, so its two steps are expected. */
  guard: boolean;
  label: string;
}

export function ApplyStepper({ result, guard, label }: ApplyStepperProps) {
  const rows = stepper(result, { guard });
  if (rows.length === 0) return null;
  return (
    <ol className={styles.steps} aria-label={label}>
      {rows.map((row, index) => (
        <li
          key={`${index}-${row.name}`}
          className={styles.step}
          data-status={row.status}
          data-testid="apply-step"
        >
          <span className={styles.icon} aria-hidden>
            {row.status === 'done' ? (
              <Check size={13} />
            ) : row.status === 'failed' ? (
              <X size={13} />
            ) : row.status === 'running' ? (
              <Spinner size={13} label={null} />
            ) : (
              <CircleDashed size={13} />
            )}
          </span>
          <span className={styles.name}>{row.label}</span>
          <span className={styles.detail}>
            {row.detail ?? (row.status === 'running' ? 'in progress…' : 'waiting')}
          </span>
          <span className={styles.at}>{row.at ? formatTime(row.at) : ''}</span>
        </li>
      ))}
    </ol>
  );
}

export interface ChecksProps {
  checks: readonly VerifyCheck[];
  label: string;
}

/** The post-change checks, pass or fail, with the engine's own words. */
export function Checks({ checks, label }: ChecksProps) {
  if (checks.length === 0) return null;
  return (
    <ul className={styles.checks} aria-label={label}>
      {checks.map((check) => (
        <li key={check.name} className={styles.check} data-ok={check.ok} data-testid="verify-check">
          <span className={styles.icon} aria-hidden>
            {check.ok ? <Check size={13} /> : <Circle size={13} />}
          </span>
          <span className={styles.name}>{check.name}</span>
          <span className={styles.detail}>{check.detail}</span>
        </li>
      ))}
    </ul>
  );
}
