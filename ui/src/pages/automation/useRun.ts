/**
 * Starting a run and following it to the end.
 *
 * `runner.run`, `playbooks.run`, `tasks.run` and `ansible.run` all answer with
 * a job id and then get on with it, so the screen follows the job: live
 * progress and per-device lines from `jobs.changed` / `jobs.log`, and the
 * outcome reported once, when it stops — taken from the *run record*, because
 * that is where the truth about the devices is. (The same pattern as
 * `pages/changes/useChangeJob.ts` and `pages/devices/useCollect.ts`.)
 *
 * One wrinkle the backend decides for us: a device's output is written when
 * the whole run finishes, not as each device answers. So while a run is in
 * flight the live view shows what the job says per device — which is the
 * truth as it is known — and the outputs appear when the run record does.
 */

import { useEffect, useRef, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { Applied, Run, Started } from '@/api/types/automation';
import type { Job, JobCancelled, JobLine } from '@/api/types/core';
import { toast } from '@/design/Toast';
import { given } from './given';
import { describeRun } from './vocab';

/** What a start returned: a run with a job, or a change for a template task. */
export type StartResult = Partial<Started> & Partial<Applied> & Record<string, unknown>;

export interface RunJob {
  /** The job that is running, or the one that finished last. */
  job: Job | undefined;
  /** The run the job is about, once one is known. */
  runId: string | null;
  running: boolean;
  /** True while the request to start it is in flight. */
  starting: boolean;
  /** The last refusal from starting it; the screen shows it itself. */
  error: Error | null;
  start: (params: Record<string, unknown>) => void;
  cancel: () => void;
  forget: () => void;
}

export interface UseRunOptions {
  /** Called once with whatever the start returned (a run id, or a change id). */
  onStarted?: (result: StartResult) => void;
  /** Called once when the job has stopped. */
  onFinished?: (job: Job) => void;
}

/**
 * Starts one of the automation jobs. `method` is the RPC to call; the params
 * are passed through untouched, so the caller owns the contract.
 */
export function useRunJob(method: string, options: UseRunOptions = {}): RunJob {
  const [jobId, setJobId] = useState<string | null>(null);
  const [runId, setRunId] = useState<string | null>(null);
  const reported = useRef<string | null>(null);
  const callbacks = useRef(options);
  callbacks.current = options;

  const start = useRpcMutation<StartResult, Record<string, unknown>>(method, { errorToast: false });
  const cancelJob = useRpcMutation<JobCancelled, { id: string }>('jobs.cancel');

  const job = useRpc<Job>(
    'jobs.get',
    { id: jobId },
    { enabled: jobId !== null, invalidateOn: ['jobs.changed', 'jobs.log'] },
  );

  const data = job.data;
  const running = data?.state === 'running';

  useEffect(() => {
    if (!data || data.state === 'running' || reported.current === data.id) return;
    reported.current = data.id;
    callbacks.current.onFinished?.(data);
  }, [data]);

  return {
    job: data,
    runId,
    running,
    starting: start.isPending,
    error: start.error,
    start: (params) => {
      reported.current = null;
      start.mutate(params, {
        onSuccess: (result) => {
          if (typeof result.run_id === 'string') setRunId(result.run_id);
          if (typeof result.job_id === 'string') setJobId(result.job_id);
          callbacks.current.onStarted?.(result);
        },
      });
    },
    cancel: () => {
      if (jobId) cancelJob.mutate({ id: jobId });
    },
    forget: () => {
      setJobId(null);
      setRunId(null);
      reported.current = null;
    },
  };
}

/** Reports a finished run as a toast, once, in the words of `describeRun`. */
export function reportRun(run: Run): void {
  const outcome = describeRun(run);
  const options = { description: outcome.detail };
  if (outcome.tone === 'ok') toast.success(`${run.name}: ${outcome.title.toLowerCase()}`, options);
  else if (outcome.tone === 'crit') toast.error(`${run.name}: ${outcome.title.toLowerCase()}`, options);
  else toast.warning(`${run.name}: ${outcome.title.toLowerCase()}`, options);
}

export type LiveLevel = 'info' | 'warn' | 'error';

/** What the job has said about one device so far. */
export interface LiveDevice {
  deviceId: string;
  level: LiveLevel;
  message: string;
  at: number;
}

/**
 * The job's own per-device lines, newest per device. The level is the
 * backend's: `error` for a device it could not reach or that failed, `warn`
 * for one it skipped, `info` for one that answered. Nothing is inferred from
 * the text, which is shown as it arrived.
 */
export function liveDevices(log: readonly JobLine[] | undefined): Map<string, LiveDevice> {
  const byDevice = new Map<string, LiveDevice>();
  for (const line of log ?? []) {
    if (!given(line.device_id) || line.device_id === '') continue;
    const level: LiveLevel = line.level === 'error' ? 'error' : line.level === 'warn' ? 'warn' : 'info';
    byDevice.set(line.device_id, {
      deviceId: line.device_id,
      level,
      message: line.message,
      at: line.ts,
    });
  }
  return byDevice;
}

/** The job's lines that are not about one device: the caveat, the tally. */
export function generalLines(log: readonly JobLine[] | undefined): JobLine[] {
  return (log ?? []).filter((line) => !given(line.device_id) || line.device_id === '');
}
