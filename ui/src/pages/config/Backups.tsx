/**
 * Who is backed up, who is not, and what the last backup did.
 *
 * The table is the coverage report (`config.devices`): one row per device,
 * with four states that are never blurred into one another — captured, never
 * captured, could not be read, and deliberately not backed up. A device with
 * no stored configuration says so in words; it does not show a zero.
 */

import { AlertTriangle, Camera, FileClock, HardDriveDownload, X } from 'lucide-react';
import type { ConfigKind, Coverage, UnsavedReport } from '@/api/types/config';
import type { Job } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Icon } from '@/design/Icon';
import { Panel } from '@/design/Panel';
import { ProgressBar } from '@/design/ProgressBar';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, plural } from '@/lib/format';
import styles from './Backups.module.css';
import {
  BACKUP_STATE_TONE,
  BACKUP_STATE_WORD,
  backupRows,
  CAPTURE_TONE,
  CAPTURE_WORD,
  captureSentence,
  captureState,
} from './vocab';

export interface BackupsTabProps {
  coverage: Coverage[] | undefined;
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  unsaved: UnsavedReport | undefined;
  selected: ReadonlySet<string>;
  onSelected: (ids: Set<string>) => void;
  /** Opens a device's history. */
  onOpen: (deviceId: string) => void;
  onBackUp: (deviceIds: readonly string[], kinds?: readonly ConfigKind[]) => void;
  job: Job | undefined;
  running: boolean;
  onCancel: () => void;
  onDismiss: () => void;
}

export function BackupsTab({
  coverage,
  loading,
  error,
  onRetry,
  unsaved,
  selected,
  onSelected,
  onOpen,
  onBackUp,
  job,
  running,
  onCancel,
  onDismiss,
}: BackupsTabProps) {
  if (error) {
    return <ErrorState title="The backup coverage could not be read" error={error} onRetry={onRetry} />;
  }

  const rows = coverage ?? [];

  const columns: readonly TableColumn<Coverage>[] = [
    {
      id: 'name',
      header: 'Device',
      required: true,
      cell: (row) => (
        <span className={styles.name}>
          <span className={styles.nameText} title={row.name}>
            {row.name}
          </span>
          {row.lab && <LabPill />}
        </span>
      ),
      sortValue: (row) => row.name.toLowerCase(),
    },
    {
      id: 'state',
      header: 'Backup',
      width: 150,
      cell: (row) => {
        const state = captureState(row);
        return (
          <Tooltip content={captureSentence(row)}>
            <StatusPill tone={CAPTURE_TONE[state]}>{CAPTURE_WORD[state]}</StatusPill>
          </Tooltip>
        );
      },
      sortValue: (row) => captureState(row),
    },
    {
      id: 'last',
      header: 'Last capture',
      width: 130,
      cell: (row) =>
        row.running ? (
          <TimeAgo at={row.running.fetched_at} />
        ) : (
          <span className={styles.never}>never</span>
        ),
      sortValue: (row) => row.running?.fetched_at ?? 0,
    },
    {
      id: 'lines',
      header: 'Lines',
      align: 'right',
      width: 90,
      mono: true,
      cell: (row) => (row.running ? formatCount(row.running.lines) : <span className={styles.never}>—</span>),
      sortValue: (row) => row.running?.lines ?? -1,
    },
    {
      id: 'versions',
      header: 'Versions',
      align: 'right',
      width: 90,
      mono: true,
      cell: (row) => (row.versions === 0 ? <span className={styles.never}>none</span> : formatCount(row.versions)),
      sortValue: (row) => row.versions,
    },
    {
      id: 'changed',
      header: 'Last change',
      width: 130,
      cell: (row) =>
        row.last_change_at ? <TimeAgo at={row.last_change_at} /> : <span className={styles.never}>—</span>,
      sortValue: (row) => row.last_change_at ?? 0,
    },
    {
      id: 'startup',
      header: 'Startup',
      width: 130,
      defaultHidden: true,
      cell: (row) =>
        row.startup ? (
          <TimeAgo at={row.startup.fetched_at} />
        ) : (
          <Tooltip content="No startup configuration has been stored, so unsaved changes cannot be told from here.">
            <span className={styles.never}>not read</span>
          </Tooltip>
        ),
      sortValue: (row) => row.startup?.fetched_at ?? 0,
    },
    {
      id: 'profile',
      header: 'Device type',
      width: 140,
      defaultHidden: true,
      cell: (row) => row.profile || <span className={styles.never}>unknown</span>,
      sortValue: (row) => row.profile,
    },
    {
      id: 'problem',
      header: 'Last problem',
      cell: (row) =>
        row.last_error ? (
          <span className={styles.problem} title={row.last_error}>
            <Icon icon={AlertTriangle} size={12} />
            {row.last_error}
          </span>
        ) : row.skipped ? (
          <span className={styles.skipped} title={row.skipped}>
            {row.skipped}
          </span>
        ) : (
          ''
        ),
      sortValue: (row) => row.last_error ?? row.skipped ?? '',
    },
  ];

  return (
    <div className={styles.backups}>
      {job && <JobReport job={job} running={running} onCancel={onCancel} onDismiss={onDismiss} coverage={rows} />}

      {unsaved && <Unsaved report={unsaved} onOpen={onOpen} />}

      <Table
        id="config.coverage"
        aria-label="Backup coverage"
        columns={columns}
        rows={rows}
        rowKey={(row) => row.device_id}
        loading={loading}
        density="compact"
        defaultSort={{ columnId: 'state', direction: 'asc' }}
        selection={{ selected, onChange: onSelected }}
        onRowActivate={(row) => onOpen(row.device_id)}
        rowActions={(row) => [
          { label: 'Open its history', icon: FileClock, onSelect: () => onOpen(row.device_id) },
          { label: 'Back up now', icon: Camera, onSelect: () => onBackUp([row.device_id]) },
          {
            label: 'Back up the running configuration only',
            onSelect: () => onBackUp([row.device_id], ['running']),
          },
        ]}
        maxHeight="62vh"
        empty={
          <EmptyState
            icon={HardDriveDownload}
            title="No devices in the inventory"
            description="Add a device, or create a lab, and Nocturne can start keeping its configuration."
            size="compact"
          />
        }
      />
    </div>
  );
}

/** What differs between a device's running and startup configurations. */
function Unsaved({ report, onOpen }: { report: UnsavedReport; onOpen: (deviceId: string) => void }) {
  if (report.devices.length === 0 && report.not_comparable.length === 0) return null;

  return (
    <Panel
      title="Unsaved changes"
      storageId="config.unsaved"
      collapsible
      defaultCollapsed={report.devices.length === 0}
      actions={
        report.devices.length > 0 ? (
          <Badge tone="warn">{plural(report.devices.length, 'device')}</Badge>
        ) : (
          <Badge tone="ok">{plural(report.saved, 'device')} saved</Badge>
        )
      }
    >
      {report.devices.length > 0 ? (
        <ul className={styles.unsaved}>
          {report.devices.map((device) => (
            <li key={device.device_id}>
              <button type="button" className={styles.linkish} onClick={() => onOpen(device.device_id)}>
                {device.name}
              </button>
              {device.lab && <LabPill />}
              <span className={styles.unsavedDiff}>
                <span data-kind="add">+{device.added}</span>
                <span data-kind="remove">−{device.removed}</span>
              </span>
              <span className={styles.quiet}>
                running and startup last read <TimeAgo at={device.fetched_at} />
              </span>
            </li>
          ))}
        </ul>
      ) : (
        <p className={styles.quiet}>
          Every device whose two configurations could be read has them in step.
        </p>
      )}
      {report.not_comparable.length > 0 && (
        <p className={styles.quiet} data-testid="not-comparable">
          {plural(report.not_comparable.length, 'device')} cannot be judged, because only one of the two
          configurations could be read: {report.not_comparable[0]?.name} —{' '}
          {report.not_comparable[0]?.reason}
          {report.not_comparable.length > 1 && ` (and ${report.not_comparable.length - 1} more)`}
        </p>
      )}
    </Panel>
  );
}

/** The job, device by device: what was stored, what was skipped, what failed. */
function JobReport({
  job,
  running,
  onCancel,
  onDismiss,
  coverage,
}: {
  job: Job;
  running: boolean;
  onCancel: () => void;
  onDismiss: () => void;
  coverage: readonly Coverage[];
}) {
  const nameOf = (deviceId: string) => coverage.find((row) => row.device_id === deviceId)?.name;
  const rows = backupRows(job, nameOf);
  const failed = rows.filter((row) => row.state === 'failed').length;

  return (
    <Panel
      title={job.title}
      actions={
        <div className={styles.jobActions}>
          {running ? (
            <>
              <span className={styles.quiet}>
                {job.message || `${formatCount(job.done)} of ${formatCount(job.total)}`}
              </span>
              <Button size="sm" variant="ghost" icon={X} onClick={onCancel}>
                Cancel
              </Button>
            </>
          ) : (
            <>
              <StatusPill tone={failed > 0 ? 'warn' : job.state === 'succeeded' ? 'ok' : 'crit'}>
                {failed > 0 ? `${plural(failed, 'device')} failed` : job.state === 'succeeded' ? 'Done' : job.state}
              </StatusPill>
              <Button size="sm" variant="ghost" onClick={onDismiss}>
                Dismiss
              </Button>
            </>
          )}
        </div>
      }
    >
      {running && <ProgressBar
          value={job.done}
          max={Math.max(job.total, 1)}
          aria-label="Backup progress"
          valueText={`${job.done} of ${job.total} devices`}
        />}
      <ul className={styles.jobRows} data-testid="backup-outcomes">
        {rows.map((row) => (
          <li key={row.device_id} data-testid="backup-outcome">
            <StatusPill tone={BACKUP_STATE_TONE[row.state]}>{BACKUP_STATE_WORD[row.state]}</StatusPill>
            <span className={styles.jobDevice}>{row.name ?? row.device_id}</span>
            <span className={styles.jobDetail}>
              {row.detail}
              {row.notes.map((note) => (
                <span key={note} className={styles.jobNote}>
                  <Icon icon={AlertTriangle} size={11} />
                  {note}
                </span>
              ))}
            </span>
          </li>
        ))}
      </ul>
      {rows.length === 0 && (
        <p className={styles.quiet}>
          <Icon icon={job.state === 'running' ? Camera : FileClock} size={12} /> Nothing has been reported
          per device yet.
        </p>
      )}
    </Panel>
  );
}
