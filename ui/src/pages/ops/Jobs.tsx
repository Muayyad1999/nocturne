/**
 * The full history of the work the engine has done.
 *
 * Jobs belong to the core (`jobs.*`), not to the operations feature, and the
 * status bar already shows the ones running now; this is the record of all of
 * them — what ran, on what, how long it took, and the log of the one you pick.
 *
 * A job that is still running can be cancelled from here, and that is the only
 * thing on this whole screen that changes anything. It says what it will stop
 * before it stops it.
 */

import { ListTree, X } from 'lucide-react';
import { useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import type { Job } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { ProgressBar } from '@/design/ProgressBar';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount, formatDuration, plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { useCancelJob, useDevices, useJob, useJobs } from './data';
import styles from './Jobs.module.css';
import opsStyles from './Ops.module.css';
import { instant, jobProgress, jobTone } from './ops';

const STATES = [
  { value: '', label: 'Every state' },
  { value: 'running', label: 'Running' },
  { value: 'finished', label: 'Finished' },
  { value: 'succeeded', label: 'Succeeded' },
  { value: 'failed', label: 'Failed' },
  { value: 'cancelled', label: 'Cancelled' },
];

/** How long a job took, or has been running. */
function took(job: Job, now: number): string | null {
  const started = instant(job.started_at);
  if (started === null) return null;
  const finished = instant(job.finished_at);
  return formatDuration((finished ?? now) - started);
}

export function JobsTab() {
  const [search, setSearch] = useSearchParams();
  const [state, setState] = useState('');
  const [text, setText] = useState('');
  const jobs = useJobs({ limit: 500, ...(state === '' ? {} : { state }) });
  const devices = useDevices();
  const openId = search.get('job');

  const openJob = (id: string | null) => {
    const next = new URLSearchParams(search);
    if (id) next.set('job', id);
    else next.delete('job');
    setSearch(next, { replace: id === null });
  };

  const filter = text.trim().toLowerCase();
  const rows = (jobs.data ?? []).filter(
    (job) =>
      filter === '' ||
      job.title.toLowerCase().includes(filter) ||
      job.kind.toLowerCase().includes(filter) ||
      job.message.toLowerCase().includes(filter),
  );
  const now = Date.now();

  const columns: readonly TableColumn<Job>[] = [
    {
      id: 'state',
      header: 'State',
      // Wide enough for the longest state pill ("succeeded") not to be cut.
      width: 132,
      required: true,
      sortValue: (job) => job.state,
      cell: (job) => <StatusPill tone={jobTone(job.state)}>{job.state}</StatusPill>,
    },
    {
      id: 'title',
      header: 'Job',
      required: true,
      sortValue: (job) => job.title,
      cell: (job) => (
        <span className={opsStyles.clip} title={job.title}>
          {job.title || job.kind}
        </span>
      ),
    },
    {
      id: 'kind',
      header: 'Kind',
      width: 140,
      mono: true,
      sortValue: (job) => job.kind,
      cell: (job) => (
        <span className={opsStyles.clip} title={job.kind}>
          {job.kind}
        </span>
      ),
    },
    {
      id: 'origin',
      header: 'Started by',
      width: 100,
      sortValue: (job) => job.origin,
      cell: (job) => <Badge tone={job.origin === 'assistant' ? 'ai' : 'neutral'} variant="outline">{job.origin}</Badge>,
    },
    {
      id: 'devices',
      header: 'Devices',
      width: 90,
      align: 'right',
      sortValue: (job) => job.device_ids.length,
      cell: (job) =>
        job.device_ids.length === 0 ? (
          <span className={opsStyles.muted}>none</span>
        ) : (
          formatCount(job.device_ids.length)
        ),
    },
    {
      id: 'progress',
      header: 'Progress',
      width: 130,
      cell: (job) => <Progress job={job} />,
    },
    {
      id: 'started',
      header: 'Started',
      width: 100,
      align: 'right',
      sortValue: (job) => job.started_at,
      cell: (job) => <TimeAgo at={instant(job.started_at)} />,
    },
    {
      id: 'took',
      header: 'Took',
      width: 90,
      align: 'right',
      sortValue: (job) => (instant(job.finished_at) ?? now) - job.started_at,
      cell: (job) => took(job, now) ?? <span className={opsStyles.muted}>unknown</span>,
    },
  ];

  return (
    <>
      <Toolbar>
        <SearchInput
          aria-label="Search jobs"
          value={text}
          onChange={setText}
          placeholder="Search titles, kinds and messages"
          className={opsStyles.filter}
        />
        <Select
          aria-label="State"
          size="sm"
          className={opsStyles.narrow}
          value={state}
          options={STATES}
          onChange={(event) => setState(event.target.value)}
        />
        <ToolbarSpacer />
        <span className={opsStyles.muted}>
          {jobs.data ? plural(jobs.data.length, 'job') : ''}
        </span>
      </Toolbar>

      {jobs.isError ? (
        <ErrorState
          title="The job history could not be read"
          error={jobs.error}
          onRetry={() => void jobs.refetch()}
        />
      ) : (
        <Table
          id="ops.jobs"
          aria-label="Jobs"
          columns={columns}
          rows={rows}
          rowKey={(job) => job.id}
          density="compact"
          loading={jobs.isPending}
          defaultSort={{ columnId: 'started', direction: 'desc' }}
          onRowActivate={(job) => openJob(job.id)}
          empty={
            <EmptyState
              size="compact"
              icon={ListTree}
              title={text.trim() !== '' || state !== '' ? 'No job matches that' : 'Nothing has run yet'}
              description={
                text.trim() !== '' || state !== ''
                  ? 'Widen the filters, or look at every state.'
                  : 'Backups, collections, discovery sweeps, compliance runs and applied changes all run as jobs, and every one of them is kept here with its log.'
              }
            />
          }
        />
      )}

      <JobDrawer
        id={openId}
        onClose={() => openJob(null)}
        deviceName={(id) => devices.data?.find((device) => device.id === id)?.name ?? id}
      />
    </>
  );
}

function Progress({ job }: { job: Job }) {
  const percent = jobProgress(job);
  if (job.state !== 'running') {
    return job.total > 0 ? (
      <span className={opsStyles.muted}>
        {formatCount(job.done)} of {formatCount(job.total)}
      </span>
    ) : (
      <span className={opsStyles.muted}>—</span>
    );
  }
  return percent === null ? (
    <span className={opsStyles.muted} title="The engine does not know how much work there is yet.">
      working
    </span>
  ) : (
    <ProgressBar value={percent} aria-label="Progress" valueText={`${percent}%`} />
  );
}

function JobDrawer({
  id,
  onClose,
  deviceName,
}: {
  id: string | null;
  onClose: () => void;
  deviceName: (id: string) => string;
}) {
  const job = useJob(id);
  const cancel = useCancelJob();
  const record = job.data;

  const items: KeyValueItem[] = record
    ? [
        { label: 'Kind', value: record.kind, mono: true },
        { label: 'State', value: <StatusPill tone={jobTone(record.state)}>{record.state}</StatusPill> },
        { label: 'Started by', value: record.origin },
        {
          label: 'Started',
          value: <span title={formatTimestamp(record.started_at)}>
            <TimeAgo at={instant(record.started_at)} />
          </span>,
        },
        {
          label: 'Finished',
          value:
            instant(record.finished_at) === null ? (
              <span className={opsStyles.muted}>still running</span>
            ) : (
              <span title={formatTimestamp(record.finished_at ?? undefined)}>
                <TimeAgo at={instant(record.finished_at)} />
              </span>
            ),
        },
        { label: 'Took', value: took(record, Date.now()) ?? '—' },
        {
          label: 'Progress',
          value:
            record.total > 0
              ? `${formatCount(record.done)} of ${formatCount(record.total)}`
              : `${formatCount(record.done)} done, total unknown`,
        },
        { label: 'Now', value: record.message || <span className={opsStyles.muted}>nothing said</span> },
        {
          label: 'Devices',
          value:
            record.device_ids.length === 0 ? (
              <span className={opsStyles.muted}>none</span>
            ) : (
              <span className={styles.deviceList}>
                {record.device_ids.map((deviceId) => (
                  <Link key={deviceId} to={`/devices/${encodeURIComponent(deviceId)}`}>
                    {deviceName(deviceId)}
                  </Link>
                ))}
              </span>
            ),
        },
        { label: 'Job id', value: <span className={opsStyles.mono}>{record.id}</span> },
      ]
    : [];

  return (
    <Drawer
      open={id !== null}
      onClose={onClose}
      title={record?.title || 'Job'}
      description={record ? record.kind : undefined}
      size="lg"
      footer={
        record?.state === 'running' ? (
          <Button
            variant="danger"
            icon={X}
            loading={cancel.isPending}
            onClick={async () => {
              const sure = await confirm({
                title: 'Cancel this job?',
                description: `“${record.title || record.kind}” will stop as soon as it can. Work already done is not undone${record.device_ids.length > 0 ? `, and the ${plural(record.device_ids.length, 'device')} it has already reached stay as they are` : ''}.`,
                confirmLabel: 'Cancel the job',
                tone: 'danger',
              });
              if (sure) cancel.mutate({ id: record.id });
            }}
          >
            Cancel this job
          </Button>
        ) : undefined
      }
    >
      {job.isPending && <Skeleton shape="block" height={220} />}
      {job.isError && (
        <ErrorState size="compact" title="That job could not be read" error={job.error} />
      )}
      {record && (
        <div className={styles.drawer}>
          <KeyValue items={items} layout="rows" />
          {record.error && (
            <div className={styles.failure}>
              <p className={styles.failureHead}>It failed</p>
              <p className={styles.failureText}>{record.error}</p>
            </div>
          )}
          <div>
            <p className={styles.logHead}>
              Log
              {record.log_dropped > 0 && (
                <Badge tone="warn" variant="outline" title="The middle of a very long log was dropped by the engine.">
                  {formatCount(record.log_dropped)} lines dropped
                </Badge>
              )}
            </p>
            {record.log.length === 0 ? (
              <p className={opsStyles.sentence}>This job wrote no log.</p>
            ) : (
              <ol className={styles.log}>
                {record.log.map((line, index) => (
                  <li key={`${line.ts}:${index}`} data-level={line.level}>
                    <span className={styles.logWhen}>
                      <TimeAgo at={instant(line.ts)} />
                    </span>
                    <span className={styles.logText}>{line.message}</span>
                  </li>
                ))}
              </ol>
            )}
          </div>
          {record.result !== undefined && record.result !== null && (
            <div>
              <p className={styles.logHead}>What it returned</p>
              <pre className={styles.result}>{JSON.stringify(record.result, null, 2)}</pre>
            </div>
          )}
        </div>
      )}
    </Drawer>
  );
}
