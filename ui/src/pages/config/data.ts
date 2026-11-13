/**
 * Everything this screen asks the backend, in one place.
 *
 * Reads are `useRpc` queries kept fresh by the configuration feature's own
 * events rather than by polling: a backup that lands anywhere in the
 * application refreshes the coverage table, the version list and the diff
 * on screen. Writes are `useRpcMutation`. Nothing here renders.
 */

import { useEffect, useRef, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  ComplianceResults,
  ComplianceResultsParams,
  ComplianceRunParams,
  ComplianceRunResult,
  ComplianceTestParams,
  ComplianceTestResult,
  ConfigBackupParams,
  ConfigKind,
  ConfigText,
  ConfigVersion,
  Coverage,
  DiffResult,
  Drift,
  Handover,
  IntendedView,
  JobStarted,
  MissingResult,
  Policy,
  RemediateResult,
  RunSummary,
  SearchResult,
  UnsavedReport,
} from '@/api/types/config';
import type { Device, Job, JobCancelled } from '@/api/types/core';
import { toast } from '@/design/Toast';
import { formatCount, plural } from '@/lib/format';

/** Events that mean a stored configuration somewhere has changed. */
const STORED = ['config.changed', 'config.first_backup', 'config.deleted', 'config.pinned'];

// ------------------------------------------------------------------- reading

export function useCoverage() {
  return useRpc<Coverage[]>('config.devices', {}, { invalidateOn: [...STORED, 'devices.changed'] });
}

export function useUnsaved() {
  return useRpc<UnsavedReport>('config.unsaved', {}, { invalidateOn: STORED });
}

export function useDevices() {
  return useRpc<Device[]>('devices.list', {}, { invalidateOn: ['devices.changed'] });
}

export function useVersions(deviceId: string | null, kind: ConfigKind) {
  return useRpc<ConfigVersion[]>(
    'config.versions',
    { device_id: deviceId, kind, limit: 200 },
    { enabled: deviceId !== null, invalidateOn: STORED },
  );
}

/** One stored version's text. `redact: false` is audited by the backend. */
export function useConfigText(versionId: number | null, redact: boolean) {
  return useRpc<ConfigText>(
    'config.get',
    { version_id: versionId, redact },
    { enabled: versionId !== null },
  );
}

/** The diff between two versions, as the engine computes it. */
export function useDiff(a: number | null, b: number | null, context: number) {
  return useRpc<DiffResult>(
    'config.diff',
    { a, b, context },
    { enabled: a !== null && b !== null && a !== b },
  );
}

export function useSearch(
  params: { query: string; regex: boolean; case: boolean; context: number; kind: ConfigKind },
  enabled: boolean,
) {
  return useRpc<SearchResult>(
    'config.search',
    { ...params, limit: 500 },
    { enabled, invalidateOn: STORED },
  );
}

export function useMissing(
  params: { query: string; regex: boolean; case: boolean; kind: ConfigKind },
  enabled: boolean,
) {
  return useRpc<MissingResult>('config.missing', params, { enabled, invalidateOn: STORED });
}

export function usePolicies() {
  return useRpc<Policy[]>(
    'compliance.policies.list',
    {},
    { invalidateOn: ['compliance.policies_changed'] },
  );
}

export function useResults(params: ComplianceResultsParams, enabled = true) {
  return useRpc<ComplianceResults>('compliance.results', params, {
    enabled,
    invalidateOn: ['compliance.finished'],
  });
}

export function useRuns() {
  return useRpc<RunSummary[]>('compliance.runs', { limit: 30 }, { invalidateOn: ['compliance.finished'] });
}

export function useIntended(deviceId: string | null) {
  return useRpc<IntendedView | null>(
    'config.intended.get',
    { device_id: deviceId },
    { enabled: deviceId !== null, invalidateOn: ['config.intended_changed'] },
  );
}

export function useDrift(deviceId: string | null, enabled: boolean) {
  return useRpc<Drift>(
    'config.drift',
    { device_id: deviceId },
    { enabled: deviceId !== null && enabled, invalidateOn: ['config.intended_changed', ...STORED] },
  );
}

// ------------------------------------------------------------------ writing

export function usePin() {
  return useRpcMutation<ConfigVersion, { version_id: number; reason: string }>('config.pin', {
    invalidates: ['config.'],
  });
}

export function useUnpin() {
  return useRpcMutation<ConfigVersion, { version_id: number }>('config.unpin', {
    invalidates: ['config.'],
  });
}

export function useSetIntended() {
  return useRpcMutation<IntendedView, { device_id: string; text: string; exact: boolean }>(
    'config.intended.set',
    { invalidates: ['config.'], successToast: 'Intended configuration saved' },
  );
}

export function useClearIntended() {
  return useRpcMutation<{ cleared: boolean }, { device_id: string }>('config.intended.clear', {
    invalidates: ['config.'],
  });
}

/** Hands the drift to the change pipeline. Nothing is sent to a device here. */
export function useFixDrift() {
  return useRpcMutation<Handover, { device_id: string }>('config.drift.fix');
}

export function useRestore() {
  return useRpcMutation<Handover, { version_id: number }>('config.restore');
}

export function useRemediate() {
  return useRpcMutation<
    RemediateResult,
    { run_id?: string; findings: { device_id: string; rule_id: string; policy_id?: string }[] }
  >('compliance.remediate');
}

export function useSavePolicy() {
  return useRpcMutation<Policy, { policy: Policy }>('compliance.policies.save', {
    invalidates: ['compliance.'],
  });
}

export function useTestRule() {
  return useRpcMutation<ComplianceTestResult, ComplianceTestParams>('compliance.test', {
    errorToast: false,
  });
}

/**
 * A compliance run. It reads stored configurations only — no device is
 * touched — so the backend answers with the finished summary rather than a
 * job id, and this is over in a moment even for a large network.
 */
export function useComplianceRun() {
  const run = useRpcMutation<ComplianceRunResult, ComplianceRunParams>('compliance.run', {
    invalidates: ['compliance.'],
    errorToast: 'The check could not be run',
  });
  return {
    pending: run.isPending,
    runId: run.data?.run_id,
    start: (params: ComplianceRunParams, what: string) =>
      run.mutate(params, {
        onSuccess: (result) => {
          const { fail, devices, not_evaluated } = result.summary;
          const detail =
            not_evaluated > 0
              ? `${plural(not_evaluated, 'rule')} could not be judged: a device had no stored configuration.`
              : undefined;
          if (fail === 0) {
            toast.success(`${what}: nothing failed on ${plural(devices, 'device')}`, { description: detail });
          } else {
            toast.warning(`${what}: ${plural(fail, 'finding')} on ${plural(devices, 'device')}`, {
              description: detail,
            });
          }
        },
      }),
  };
}

// --------------------------------------------------------------- backup job

export interface BackupRun {
  /** The job that is running, or the one that finished last. */
  job: Job | undefined;
  running: boolean;
  starting: boolean;
  /** Back up these devices. Empty does nothing. */
  start: (deviceIds: readonly string[], kinds?: readonly ConfigKind[]) => void;
  cancel: () => void;
  /** Forget the finished job, so its report goes away. */
  dismiss: () => void;
}

/**
 * `config.backup` answers with a job id and gets on with it, so the screen
 * follows the job: live progress from `jobs.changed`, and the outcome read
 * from the job's own result and per-device log when it stops.
 */
export function useBackup(): BackupRun {
  const [jobId, setJobId] = useState<string | null>(null);
  /** The job whose outcome has already been announced. */
  const reported = useRef<string | null>(null);

  const run = useRpcMutation<JobStarted, ConfigBackupParams>('config.backup', {
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
    const result = (data.result ?? {}) as {
      backed_up?: number;
      changed?: number;
      first?: number;
      skipped?: unknown[];
      failed?: unknown[];
    };
    const failed = result.failed?.length ?? 0;
    const skipped = result.skipped?.length ?? 0;
    const stored = result.backed_up ?? 0;
    const notes: string[] = [];
    if (failed > 0) notes.push(`${formatCount(failed)} failed`);
    if (skipped > 0) notes.push(`${formatCount(skipped)} skipped`);
    const detail = notes.length > 0 ? notes.join(', ') : undefined;

    if (data.state === 'cancelled') toast.info('Backup cancelled', { description: detail });
    else if (data.state === 'failed') {
      toast.error('The backup did not finish', { description: data.error ?? detail });
    } else if (failed > 0) {
      toast.warning(`Backed up ${plural(stored, 'device')}`, { description: detail });
    } else {
      const fresh = (result.changed ?? 0) + (result.first ?? 0);
      toast.success(`Backed up ${plural(stored, 'device')}`, {
        description:
          fresh === 0
            ? 'Nothing had changed since the last capture.'
            : `${plural(fresh, 'new version')} stored.${skipped > 0 ? ` ${formatCount(skipped)} skipped.` : ''}`,
      });
    }
  }, [data]);

  return {
    job: data,
    running: data?.state === 'running',
    starting: run.isPending,
    start: (deviceIds, kinds) => {
      if (deviceIds.length === 0) return;
      reported.current = null;
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
