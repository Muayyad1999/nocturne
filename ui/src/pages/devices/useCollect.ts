/**
 * Work on devices that takes longer than a click. `collect.run` and
 * `config.backup` both answer with a job id, so the screen follows the job
 * rather than waiting on a call: progress while it runs, and the truth when
 * it ends — including the kinds that could not be read and why.
 */

import { useEffect, useRef, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  CollectRunParams,
  CollectRunResult,
  Job,
  JobCancelled,
  JobStarted,
  Kind,
} from '@/api/types/core';
import { toast } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import { KIND_LABEL } from './device';

export interface Collection {
  /** The job that is running, or the one that finished last. */
  job: Job | undefined;
  running: boolean;
  /** Start a collection. `kinds` empty = everything the device type defines. */
  start: (deviceIds: readonly string[], kinds?: readonly Kind[]) => void;
  cancel: () => void;
  /** Forget the finished job, so its banner goes away. */
  dismiss: () => void;
  /** True while the request to start is in flight. */
  starting: boolean;
}

export interface DeviceJob {
  job: Job | undefined;
  running: boolean;
  starting: boolean;
  start: (deviceIds: readonly string[]) => void;
  cancel: () => void;
}

/** The outcome of a finished collection, in one sentence. */
export function describeOutcome(job: Job): { title: string; detail?: string; failed: boolean } {
  const result = job.result as CollectRunResult | undefined;
  const devices = result?.devices ?? [];
  const failed = result?.failed ?? 0;
  const problems = devices.flatMap((device) =>
    device.problems.map((problem) => ({ device: device.name, ...problem })),
  );
  const unreadable = problems.filter((problem) => problem.code !== 'unsupported');

  if (devices.length === 0) {
    return { title: job.error ?? 'Nothing was collected', failed: true };
  }
  if (failed === devices.length) {
    return {
      title: `Could not collect from ${failed === 1 ? devices[0]?.name : `${formatCount(failed)} devices`}`,
      detail: devices.find((device) => device.error)?.error ?? undefined,
      failed: true,
    };
  }
  const read = devices.length - failed;
  const title = `Collected from ${read === 1 ? devices.find((device) => !device.error)?.name : `${formatCount(read)} devices`}`;
  const notes: string[] = [];
  if (failed > 0) notes.push(`${formatCount(failed)} could not be reached`);
  if (unreadable.length > 0) {
    const kinds = [...new Set(unreadable.map((problem) => KIND_LABEL[problem.kind]))];
    notes.push(`${kinds.join(', ')} could not be read`);
  }
  return { title, detail: notes.join('; ') || undefined, failed: failed > 0 };
}

/**
 * Starts collections and watches the job it gets back. The job is refetched
 * on `jobs.changed`, so progress is live without polling.
 */
export function useCollect(): Collection {
  const [jobId, setJobId] = useState<string | null>(null);
  /** The job whose outcome has already been announced. */
  const reported = useRef<string | null>(null);

  const run = useRpcMutation<JobStarted, CollectRunParams>('collect.run', {
    errorToast: 'The collection could not be started',
  });
  const cancelJob = useRpcMutation<JobCancelled, { id: string }>('jobs.cancel');

  const job = useRpc<Job>(
    'jobs.get',
    { id: jobId },
    { enabled: jobId !== null, invalidateOn: ['jobs.changed'] },
  );

  const data = job.data;
  const running = data?.state === 'running';

  // Report the outcome once, when the job stops running.
  useEffect(() => {
    if (!data || data.state === 'running' || reported.current === data.id) return;
    reported.current = data.id;
    const outcome = describeOutcome(data);
    if (data.state === 'cancelled') toast.info('Collection cancelled');
    else if (outcome.failed) toast.warning(outcome.title, { description: outcome.detail });
    else toast.success(outcome.title, { description: outcome.detail });
  }, [data]);

  return {
    job: data,
    running,
    starting: run.isPending,
    start: (deviceIds, kinds) => {
      if (deviceIds.length === 0) return;
      run.mutate(
        { device_ids: [...deviceIds], ...(kinds?.length ? { kinds: [...kinds] } : {}) },
        { onSuccess: (started) => setJobId(started.job_id) },
      );
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

/**
 * Backing up the configuration of the selected devices (`config.backup`).
 * The method belongs to the configuration module, so the screen only offers
 * it when this build answers it.
 */
export function useBackup(): DeviceJob {
  const [jobId, setJobId] = useState<string | null>(null);
  const reported = useRef<string | null>(null);

  const run = useRpcMutation<JobStarted, { device_ids: string[] }>('config.backup', {
    errorToast: 'The backup could not be started',
  });
  const cancelJob = useRpcMutation<JobCancelled, { id: string }>('jobs.cancel');
  const job = useRpc<Job>(
    'jobs.get',
    { id: jobId },
    { enabled: jobId !== null, invalidateOn: ['jobs.changed'] },
  );

  const data = job.data;
  useEffect(() => {
    if (!data || data.state === 'running' || reported.current === data.id) return;
    reported.current = data.id;
    if (data.state === 'cancelled') toast.info('Backup cancelled');
    else if (data.state === 'failed') {
      toast.warning('The backup did not finish', { description: data.error ?? undefined });
    } else toast.success(data.title);
  }, [data]);

  return {
    job: data,
    running: data?.state === 'running',
    starting: run.isPending,
    start: (deviceIds) => {
      if (deviceIds.length === 0) return;
      run.mutate(
        { device_ids: [...deviceIds] },
        { onSuccess: (started) => setJobId(started.job_id) },
      );
    },
    cancel: () => {
      if (jobId) cancelJob.mutate({ id: jobId });
    },
  };
}
