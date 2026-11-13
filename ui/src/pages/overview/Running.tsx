/**
 * What the engine is doing right now, with a way to stop it.
 *
 * The jobs come from the overview's own `jobs` part, which is nullable like
 * every other: a build whose job list could not be read says so rather than
 * showing an empty, reassuring panel. "Nothing running" and "the job list
 * could not be read" are different sentences here too.
 *
 * `total: 0` means the amount of work is not yet known, not "none of it done",
 * so the bar is indeterminate rather than empty (see `jobProgress`).
 */

import { Hourglass } from 'lucide-react';
import type { JobRef } from '@/api/types/ops';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { Icon } from '@/design/Icon';
import { Panel } from '@/design/Panel';
import { ProgressBar } from '@/design/ProgressBar';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount, plural } from '@/lib/format';
import { jobProgress, type Reading } from './overview';
import styles from './Running.module.css';
import { UnavailableNote } from './Unavailable';

export interface RunningProps {
  jobs: Reading<{ running: JobRef[]; failed_24h: number }>;
  onCancel: (id: string) => void;
  cancelling: string | null;
}

export function Running({ jobs, onCancel, cancelling }: RunningProps) {
  if (!jobs.present) {
    return (
      <Panel title="Running">
        <UnavailableNote reason={jobs.reason} message={jobs.message} size="block" />
      </Panel>
    );
  }

  const { running, failed_24h: failed } = jobs.value;

  return (
    <Panel
      title="Running"
      padding={running.length === 0 ? 'md' : 'none'}
      actions={
        <span className={styles.failed} data-tone={failed > 0 ? 'warn' : undefined}>
          {failed === 0
            ? 'none failed in 24 h'
            : `${plural(failed, 'job')} failed in 24 h`}
        </span>
      }
    >
      {running.length === 0 ? (
        // One quiet line rather than a block: an idle engine is the normal
        // state, and this panel should not take the room of a problem.
        <p className={styles.idle}>
          <Icon icon={Hourglass} size={14} />
          <span>
            <strong>Nothing running.</strong> Collections, backups, scans and configuration
            changes appear here while they run, with a way to stop them.
          </span>
        </p>
      ) : (
        <ul className={styles.list} aria-label="Jobs running now">
          {running.map((job) => (
            <Job
              key={job.id}
              job={job}
              onCancel={onCancel}
              cancelling={cancelling === job.id}
            />
          ))}
        </ul>
      )}
    </Panel>
  );
}

function Job({
  job,
  onCancel,
  cancelling,
}: {
  job: JobRef;
  onCancel: (id: string) => void;
  cancelling: boolean;
}) {
  const progress = jobProgress(job);

  const ask = async () => {
    const devices =
      job.device_ids.length === 0
        ? 'It is not working on a particular device.'
        : `It is working on ${plural(job.device_ids.length, 'device')}.`;
    if (
      await confirm({
        title: `Stop "${job.title}"?`,
        description: `${devices} Work already done is not undone — a configuration change that has been sent stays sent, and a job that is stopped half way leaves the estate half done. ${progress.text} so far.`,
        confirmLabel: 'Stop the job',
        tone: 'danger',
      })
    ) {
      onCancel(job.id);
    }
  };

  return (
    <li className={styles.job}>
      <div className={styles.head}>
        <span className={styles.title} title={job.title}>
          {job.title}
        </span>
        <Button size="sm" variant="ghost" loading={cancelling} onClick={() => void ask()}>
          Cancel
        </Button>
      </div>
      <ProgressBar
        value={progress.value}
        max={progress.max}
        valueText={progress.text}
        aria-label={`Progress of ${job.title}`}
        size="sm"
      />
      <div className={styles.foot}>
        <span className={styles.counts}>{progress.text}</span>
        <span className={styles.dot}>·</span>
        <span className={styles.origin}>
          {job.kind} by {job.origin}
        </span>
        <span className={styles.dot}>·</span>
        <span>
          started <TimeAgo at={job.started_at} />
        </span>
      </div>
      {job.message && (
        <p className={styles.message} title={job.message}>
          {job.message}
        </p>
      )}
      {job.device_ids.length > 0 && (
        <p className={styles.devices}>on {formatCount(job.device_ids.length)} devices</p>
      )}
    </li>
  );
}
