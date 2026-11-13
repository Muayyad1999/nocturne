/**
 * The scan while it runs, and only if it went wrong afterwards: a finished
 * scan announces itself in a toast, and its numbers end up in the coverage
 * band, so a second banner saying the same thing would just be noise.
 */

import { ProgressBar } from '@/design/ProgressBar';
import { Button } from '@/design/Button';
import { ErrorState } from '@/design/ErrorState';
import { plural } from '@/lib/format';
import styles from './Insights.module.css';
import type { Scan } from './useScan';

export interface ScanBannerProps {
  scan: Scan;
}

export function ScanBanner({ scan }: ScanBannerProps) {
  const job = scan.job;

  if (scan.running) {
    const devices = job?.device_ids.length ?? 0;
    return (
      <div className={styles.banner} role="status" aria-live="polite">
        <div className={styles.bannerText}>
          <strong>{job?.title ?? 'Scanning for issues'}</strong>
          <span className={styles.bannerDetail}>
            {job?.message !== undefined && job.message !== ''
              ? job.message
              : devices > 0
                ? `Reading what has been collected from ${plural(devices, 'device')}. Nothing is being connected to.`
                : 'Reading what has already been collected. Nothing is being connected to.'}
          </span>
        </div>
        <ProgressBar
          className={styles.bannerBar}
          value={job && job.total > 0 && job.done > 0 ? job.done : undefined}
          max={job && job.total > 0 ? job.total : 100}
          aria-label="Scan progress"
          valueText={job && job.total > 0 ? `${job.done} of ${job.total}` : 'Working'}
        />
        <Button size="sm" variant="ghost" onClick={scan.cancel}>
          Cancel
        </Button>
      </div>
    );
  }

  if (job?.state === 'failed') {
    return (
      <ErrorState
        size="compact"
        title="The scan did not finish"
        error={job.error ?? 'The engine did not say why.'}
        onRetry={() => scan.start()}
        retryLabel="Scan again"
        actions={
          <Button size="sm" variant="ghost" onClick={scan.dismiss}>
            Dismiss
          </Button>
        }
      />
    );
  }

  return null;
}
