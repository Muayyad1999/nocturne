/**
 * Following a change through a job.
 *
 * `changes.plan` and `changes.apply` answer with a job id and then get on
 * with it, so the screen follows the job rather than waiting on the call (an
 * undo is applied by `changes.apply` too: `changes.rollback` only creates
 * it, and the screen opens it): live progress from `jobs.changed` and
 * `changes.progress`, and the outcome reported once, when it stops — taken
 * from the change record, because that is where the truth about the devices
 * is. (`ui/src/pages/devices/useCollect.ts` is the same pattern for
 * collections.)
 */

import { useEffect, useRef, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { Change, ChangeIdParams, JobStarted } from '@/api/types/changes';
import type { Job, JobCancelled } from '@/api/types/core';
import { toast } from '@/design/Toast';
import { plural } from '@/lib/format';

export interface ChangeJob {
  /** The job that is running, or the one that finished last. */
  job: Job | undefined;
  running: boolean;
  /** True while the request to start it is in flight. */
  starting: boolean;
  start: (params?: Record<string, unknown>) => void;
  cancel: () => void;
  /** Forget the finished job, so its banner goes away. */
  dismiss: () => void;
  /** The last error from starting it, when the screen shows it itself. */
  error: Error | null;
}

export interface ChangeJobOptions {
  /** Called once with the change as it was when the job stopped. */
  onFinished?: (change: Change) => void;
  /** Shown in front of the message when the call itself is refused. */
  errorToast?: string | false;
}

/**
 * Starts one of the change pipeline's jobs and watches it. `method` is the
 * RPC to call; `id` is the change it is about.
 */
export function useChangeJob(method: string, id: string, options: ChangeJobOptions = {}): ChangeJob {
  const [jobId, setJobId] = useState<string | null>(null);
  /** The job whose outcome has already been announced. */
  const reported = useRef<string | null>(null);
  const onFinished = useRef(options.onFinished);
  onFinished.current = options.onFinished;

  const run = useRpcMutation<JobStarted, ChangeIdParams & Record<string, unknown>>(method, {
    errorToast: options.errorToast ?? false,
  });
  const cancelJob = useRpcMutation<JobCancelled, { id: string }>('jobs.cancel');

  const job = useRpc<Job>(
    'jobs.get',
    { id: jobId },
    { enabled: jobId !== null, invalidateOn: ['jobs.changed', 'changes.progress'] },
  );
  // The change record is the outcome; the job only says the work has stopped.
  const change = useRpc<Change>('changes.get', { id }, { invalidateOn: ['changes.changed'] });

  const data = job.data;
  const running = data?.state === 'running';

  useEffect(() => {
    if (!data || data.state === 'running' || reported.current === data.id) return;
    if (!change.data) return;
    reported.current = data.id;
    onFinished.current?.(change.data);
  }, [data, change.data]);

  return {
    job: data,
    running,
    starting: run.isPending,
    error: run.error,
    start: (params) => {
      reported.current = null;
      run.mutate({ id, ...params }, { onSuccess: (started) => setJobId(started.job_id) });
    },
    cancel: () => {
      if (jobId) cancelJob.mutate({ id: jobId });
    },
    dismiss: () => {
      setJobId(null);
      reported.current = null;
    },
  };
}

/** What an apply or a rollback did, in one sentence, from the change record. */
export function describeOutcome(change: Change): { title: string; detail?: string; tone: 'ok' | 'warn' | 'crit' } {
  const results = change.targets.map((target) => target.result);
  const count = (...states: string[]) =>
    results.filter((result) => result && states.includes(result.state)).length;
  const applied = count('applied');
  const rolled = count('rolled_back');
  const bad = count('failed', 'rollback_failed', 'guard_fired');
  const first = change.targets.find((target) =>
    ['failed', 'rollback_failed', 'guard_fired'].includes(target.result?.state ?? ''),
  );

  if (change.state === 'applied') {
    return { title: `Applied and verified on ${plural(applied, 'device')}`, tone: 'ok' };
  }
  if (change.state === 'rolled_back') {
    return {
      title: `Rolled back on ${plural(rolled, 'device')}`,
      detail: first?.result?.error ?? results.find((result) => result?.error)?.error ?? undefined,
      tone: 'warn',
    };
  }
  if (change.state === 'partially_applied') {
    return {
      title: `${plural(applied, 'device')} changed, ${bad + rolled} not`,
      detail: first?.result?.error ?? undefined,
      tone: 'warn',
    };
  }
  if (change.state === 'cancelled') return { title: 'Cancelled', tone: 'warn' };
  return {
    title: bad === 0 ? 'The change did not finish' : `Failed on ${plural(bad, 'device')}`,
    detail: first?.result?.error ?? undefined,
    tone: 'crit',
  };
}

/** Reports a finished plan or apply (an undo's included) as a toast. */
export function reportOutcome(change: Change): void {
  const outcome = describeOutcome(change);
  if (outcome.tone === 'ok') toast.success(outcome.title, { description: outcome.detail });
  else if (outcome.tone === 'warn') toast.warning(outcome.title, { description: outcome.detail });
  else toast.error(outcome.title, { description: outcome.detail });
}
