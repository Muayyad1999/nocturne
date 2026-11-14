/**
 * The tools that are a form and one answer: every calculator, the generators,
 * the probes that reply in the call, and the two device probes.
 *
 * One file, two behaviours, and the difference is the point:
 *
 * * **A calculator is a query.** The form's values become the parameters,
 *   debounced, and React Query caches the answer under them — so typing is the
 *   question, going back to an earlier input costs nothing, and no button
 *   stands between a subnet and its mask. A refusal from the engine
 *   (`10.0.300.1`) replaces the answer with the engine's own sentence rather
 *   than leaving the previous answer on screen pretending to describe the new
 *   input.
 * * **A generator or a probe is a mutation.** Each press is a new call: a new
 *   password, a new key pair, a new probe recorded in the history. A query
 *   would hand back the cached previous answer, which for a password generator
 *   is exactly wrong and for a probe is a lie about when it ran.
 *
 * A probe's failure goes through {@link ProbeFailure}: no chart, no zero, and
 * the engine's distinction between "this machine may not" and "nothing
 * answered" left intact.
 */

import { Play } from 'lucide-react';
import type { ReactNode } from 'react';
import type { RpcError } from '@/api/errors';
import { useRpcMutation } from '@/api/hooks';
import type { DeviceRow } from '@/api/types/core';
import { Button } from '@/design/Button';
import { Skeleton } from '@/design/Skeleton';
import { useDebounced, useToolResult, type Form } from './data';
import { sectionsFor } from './facts';
import { Fields } from './Fields';
import { Sections } from './Sections';
import type { Spec } from './specs';
import styles from './Tools.module.css';
import { Absence, ProbeFailure } from './Unavailable';

export interface AnswerProps {
  id: string;
  spec: Spec;
  form: Form;
  devices?: readonly DeviceRow[];
  /** A probe's failure is shown as a refusal with its code, never as a result. */
  probe?: boolean;
  /** What a failure sentence calls this run: "the ping of 10.0.0.1". */
  what?: string;
  /** Anything to show under the answer: a secret's note, for instance. */
  footer?: (result: unknown) => ReactNode;
}

/** A calculator: the answer follows the keystrokes. */
export function LiveAnswer({ id, spec, form, footer }: AnswerProps) {
  const settled = useDebounced(form.values);
  const params = spec.params(settled);
  const answer = useToolResult<unknown>(id, params);
  // While a new input is in flight the previous answer is still on screen;
  // it describes the previous input, so it is dimmed rather than left to look
  // like the answer to what is being typed now.
  const stale = answer.isFetching && !answer.isPending;

  return (
    <>
      <div className={styles.formRow}>
        <Fields fields={spec.fields} values={form.values} onChange={form.set} />
      </div>
      {params === null ? (
        <Absence
          reason="nothing_yet"
          message="Fill the fields above and the answer appears here as you type. Nothing leaves this machine: this is arithmetic."
        />
      ) : answer.isPending ? (
        <AnswerSkeleton />
      ) : answer.error ? (
        <Refusal error={answer.error} />
      ) : (
        <div className={styles.answer} data-stale={stale || undefined}>
          <Sections sections={sectionsFor(id, answer.data)} />
          {footer?.(answer.data)}
        </div>
      )}
    </>
  );
}

/** A generator or a probe: the answer follows the button. */
export function AskedAnswer({ id, spec, form, devices, probe, what, footer }: AnswerProps) {
  const call = useRpcMutation<unknown, Record<string, unknown>>(id, {
    // The screen shows the error where the answer would be; a toast as well
    // would say the same thing twice.
    errorToast: false,
    invalidates: probe ? ['probe.history'] : undefined,
  });
  const params = spec.params(form.values);
  const run = () => {
    if (params !== null) call.mutate(params);
  };

  return (
    <>
      <div className={styles.formRow}>
        <Fields
          fields={spec.fields}
          values={form.values}
          devices={devices}
          onChange={form.set}
          onSubmit={run}
          disabled={call.isPending}
        />
      </div>
      <div className={styles.actions}>
        <Button variant="primary" icon={Play} loading={call.isPending} disabled={params === null} onClick={run}>
          {spec.action ?? 'Run'}
        </Button>
        {params === null && <span className={styles.actionHint}>Fill the fields above to ask for this.</span>}
      </div>
      {call.isPending ? (
        <AnswerSkeleton />
      ) : call.error ? (
        probe ? (
          <ProbeFailure failure={{ code: call.error.code, message: call.error.message }} what={what ?? 'this probe'} />
        ) : (
          <Refusal error={call.error} />
        )
      ) : call.data === undefined ? (
        <Absence
          reason="nothing_yet"
          message={
            probe
              ? 'Nothing has been probed yet. Every run is recorded in the probe history, with whether it could run at all.'
              : 'Nothing has been made yet.'
          }
        />
      ) : (
        <div className={styles.answer}>
          <Sections sections={sectionsFor(id, call.data)} />
          {footer?.(call.data)}
        </div>
      )}
    </>
  );
}

/** The engine's refusal, in its own words, where the answer would have been. */
export function Refusal({ error }: { error: RpcError }) {
  return (
    <div className={styles.refusal} role="status">
      <strong className={styles.refusalHead}>That input will not do</strong>
      <p className={styles.refusalWhy}>{error.message}</p>
    </div>
  );
}

export function AnswerSkeleton() {
  return (
    <div className={styles.answer} aria-hidden="true">
      <Skeleton shape="block" height={18} width="40%" />
      <Skeleton shape="block" height={18} width="70%" />
      <Skeleton shape="block" height={18} width="55%" />
    </div>
  );
}
