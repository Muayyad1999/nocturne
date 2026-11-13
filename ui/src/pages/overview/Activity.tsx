/**
 * What has just happened: the audit trail, the alerts and the changes, merged
 * by the backend into one feed, newest first, at most twenty lines.
 *
 * `ts` is when the thing happened, not when it was read, so the times here can
 * be older than the reading at the top of the screen — which is correct, and
 * is why every line carries its own time rather than a position in a list.
 *
 * The feed is never `null`, but the backend can report that part of it was
 * unreadable (an `activity` entry in `unavailable`); that reason is shown with
 * the other gaps in "Needs attention", because a feed with a hole in it is
 * exactly the thing a person must not mistake for a quiet night.
 */

import { History } from 'lucide-react';
import { Link } from 'react-router';
import type { ActivityLine } from '@/api/types/ops';
import { EmptyState } from '@/design/EmptyState';
import { Panel } from '@/design/Panel';
import { Skeleton } from '@/design/Skeleton';
import { TimeAgo } from '@/design/TimeAgo';
import { ACTIVITY_TONE, ACTIVITY_WORD, activityLink } from './overview';
import styles from './Activity.module.css';

export interface ActivityProps {
  lines: readonly ActivityLine[];
}

export function Activity({ lines }: ActivityProps) {
  return (
    <Panel title="Activity" padding={lines.length === 0 ? 'md' : 'none'}>
      {lines.length === 0 ? (
        <EmptyState
          size="compact"
          icon={History}
          title="Nothing has happened yet"
          description="Every alert, change and recorded action appears here as it happens."
        />
      ) : (
        <ol className={styles.feed} aria-label="What has just happened">
          {lines.map((line, index) => (
            <Line key={`${line.ts}:${line.kind}:${line.ref?.id ?? index}`} line={line} />
          ))}
        </ol>
      )}
    </Panel>
  );
}

function Line({ line }: { line: ActivityLine }) {
  const to = activityLink(line);
  return (
    <li className={styles.line}>
      <span className={styles.when}>
        <TimeAgo at={line.ts} />
      </span>
      <span className={styles.kind} data-tone={ACTIVITY_TONE[line.kind] ?? 'idle'}>
        {ACTIVITY_WORD[line.kind] ?? line.kind}
      </span>
      {to ? (
        <Link className={styles.text} to={to} title={line.text}>
          {line.text}
        </Link>
      ) : (
        <span className={styles.text} title={line.text}>
          {line.text}
        </span>
      )}
    </li>
  );
}

export function ActivitySkeleton() {
  return (
    <Panel title="Activity" padding="none">
      <ol className={styles.feed} aria-busy="true">
        {[0, 1, 2, 3, 4, 5].map((index) => (
          <li key={index} className={styles.line}>
            <Skeleton width={48} height={12} />
            <Skeleton width={44} height={12} />
            <Skeleton width={`${70 - index * 6}%`} height={12} />
          </li>
        ))}
      </ol>
    </Panel>
  );
}
