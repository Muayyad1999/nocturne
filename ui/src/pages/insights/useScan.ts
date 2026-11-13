/**
 * Following a scan to its end.
 *
 * `insights.scan` answers with a job id, so the screen watches the job
 * (`jobs.changed`) instead of waiting on the call: a scan over a few hundred
 * devices reads everything that was ever collected from them. When it
 * finishes, its result is a `ScanSummary` and the toast says what changed —
 * including "nothing", which is the answer an engineer most wants to trust.
 */

import { useEffect, useRef, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { Job, JobCancelled, JobStarted } from '@/api/types/core';
import type { InsightsScanParams, ScanSummary } from '@/api/types/insights';
import { toast } from '@/design/Toast';
import { formatCount, formatDuration, plural } from '@/lib/format';
import { word } from './insight';

export interface Scan {
  /** The scan that is running, or the one that finished last. */
  job: Job | undefined;
  summary: ScanSummary | undefined;
  running: boolean;
  /** True while the request to start one is in flight. */
  starting: boolean;
  /**
   * Starts a scan: everything by default, or the named devices and checks.
   * Naming a check runs it even when it is switched off.
   */
  start: (params?: InsightsScanParams) => void;
  cancel: () => void;
  /** Forget the finished job, so its banner goes away. */
  dismiss: () => void;
}

export function scanSummary(job: Job | undefined): ScanSummary | undefined {
  if (!job || job.state !== 'succeeded') return undefined;
  const result = job.result as ScanSummary | undefined;
  return result && typeof result.devices === 'number' ? result : undefined;
}

/** What a finished scan amounts to, in one sentence. */
export function describeScan(summary: ScanSummary): { title: string; detail: string } {
  const devices = plural(summary.devices, 'device');
  const title =
    summary.opened === 0 && summary.resolved === 0
      ? `Nothing new on ${devices}`
      : [
          summary.opened > 0 ? `${formatCount(summary.opened)} new` : '',
          summary.resolved > 0 ? `${formatCount(summary.resolved)} resolved` : '',
        ]
          .filter(Boolean)
          .join(', ');
  return {
    title,
    detail:
      `${plural(summary.checks_run, 'check')} over ${devices}, ` +
      `${formatCount(summary.open)} open ${word(summary.open, 'finding')} in all ` +
      `(${formatDuration(summary.took_ms)})`,
  };
}

export function useScan(): Scan {
  const [jobId, setJobId] = useState<string | null>(null);
  /** The job whose outcome has already been announced. */
  const reported = useRef<string | null>(null);

  const run = useRpcMutation<JobStarted, InsightsScanParams>('insights.scan', {
    errorToast: 'The scan could not be started',
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
    if (data.state === 'cancelled') {
      toast.info('Scan cancelled', { description: 'Nothing was changed.' });
      return;
    }
    if (data.state === 'failed') {
      toast.warning('The scan did not finish', { description: data.error ?? undefined });
      return;
    }
    const summary = scanSummary(data);
    if (!summary) {
      toast.info(data.title);
      return;
    }
    const { title, detail } = describeScan(summary);
    toast.success(title, { description: detail });
  }, [data]);

  return {
    job: data,
    summary: scanSummary(data),
    running: data?.state === 'running' || run.isPending,
    starting: run.isPending,
    start: (params) => {
      run.mutate(params ?? {}, { onSuccess: (started) => setJobId(started.job_id) });
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
