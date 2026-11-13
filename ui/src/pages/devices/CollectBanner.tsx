/**
 * A collection in flight. Long work shows as the job it started: what it is
 * doing, how far it has got, and a way to stop it.
 */

import { CircleStop, Loader } from 'lucide-react';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { ProgressBar } from '@/design/ProgressBar';
import { formatCount } from '@/lib/format';
import type { Collection } from './useCollect';
import styles from './CollectBanner.module.css';

export function CollectBanner({ collection }: { collection: Collection }) {
  const job = collection.job;
  if (!collection.running || !job) return null;

  return (
    <div className={styles.banner} role="status">
      <Icon icon={Loader} size={14} className={styles.spin} />
      <span className={styles.title}>{job.title}</span>
      <span className={styles.message}>{job.message || 'Connecting…'}</span>
      {job.total > 0 && (
        <>
          <ProgressBar
            className={styles.bar}
            value={job.done}
            max={job.total}
            size="sm"
            aria-label="Collection progress"
            valueText={`${formatCount(job.done)} of ${formatCount(job.total)} devices`}
          />
          <span className={styles.count}>
            {formatCount(job.done)}/{formatCount(job.total)}
          </span>
        </>
      )}
      <Button size="sm" variant="ghost" icon={CircleStop} onClick={collection.cancel}>
        Stop
      </Button>
    </div>
  );
}
