/**
 * One ranked list of everything that wants a person: incidents, devices that
 * stopped answering, the loud alerts, changes that went wrong, drift, devices
 * with no backup, findings. Each row says what, where, since when, and offers
 * the one obvious thing to do.
 *
 * Every row is built from the single `overview.get` answer (plus the inventory
 * for the names of dead devices), so a row here can never disagree with the
 * tile above it. The parts the backend could not fill are listed underneath,
 * each with its reason: what this list does *not* know is part of what it says.
 */

import { CircleCheck } from 'lucide-react';
import { Link } from 'react-router';
import type { Unavailable } from '@/api/types/ops';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Icon } from '@/design/Icon';
import { Panel } from '@/design/Panel';
import { Skeleton } from '@/design/Skeleton';
import { LabPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount, plural } from '@/lib/format';
import styles from './NeedsAttention.module.css';
import { ATTENTION_LIMIT, type AttentionRow } from './overview';
import { UnavailableNote } from './Unavailable';

export interface NeedsAttentionProps {
  rows: readonly AttentionRow[];
  /** The parts of the overview that could not be filled, with their reasons. */
  unavailable: readonly Unavailable[];
  /** When the reading on screen was taken, for the "nothing needs you" state. */
  generatedAt: number;
  /** True while a name could not be put to the devices that are down. */
  namesMissing: boolean;
  onAcknowledge: (alertId: string) => void;
  acknowledging: string | null;
}

export function NeedsAttention({
  rows,
  unavailable,
  generatedAt,
  namesMissing,
  onAcknowledge,
  acknowledging,
}: NeedsAttentionProps) {
  const shown = rows.slice(0, ATTENTION_LIMIT);
  const rest = rows.length - shown.length;

  return (
    <Panel
      title="Needs attention"
      padding="none"
      actions={
        rows.length > 0 ? (
          <span className={styles.count}>{plural(rows.length, 'thing')}</span>
        ) : undefined
      }
    >
      {rows.length === 0 ? (
        <div className={styles.calm}>
          <EmptyState
            size="compact"
            icon={CircleCheck}
            title="Nothing needs you right now"
            description={
              <>
                No incident, no device down, no loud alert, no failed change. Read{' '}
                <TimeAgo at={generatedAt} />.
              </>
            }
          />
        </div>
      ) : (
        <ul className={styles.list} aria-label="Things that need attention">
          {shown.map((row) => (
            <Row
              key={row.id}
              row={row}
              onAcknowledge={onAcknowledge}
              acknowledging={acknowledging === row.ackAlertId}
            />
          ))}
          {rest > 0 && (
            <li className={styles.rest}>
              {formatCount(rest)} more, ranked below these. The screens they link to have them all.
            </li>
          )}
        </ul>
      )}

      {(unavailable.length > 0 || namesMissing) && (
        <div className={styles.gaps}>
          <h4 className={styles.gapsTitle}>What this list could not look at</h4>
          {namesMissing && (
            <UnavailableNote
              reason="failed"
              message="The inventory could not be read, so the devices that are down are counted but not named."
            />
          )}
          {unavailable.map((entry) => (
            <UnavailableNote
              key={entry.section}
              reason={entry.reason}
              message={`${SECTION_WORD[entry.section] ?? entry.section}: ${entry.message}`}
            />
          ))}
        </div>
      )}
    </Panel>
  );
}

/** What each part of the overview is, in the words the screen uses for it. */
const SECTION_WORD: Record<string, string> = {
  alerts: 'Alerts',
  changes: 'Changes',
  backups: 'Backup coverage',
  compliance: 'Compliance',
  insights: 'Findings',
  jobs: 'Jobs',
  activity: 'Activity',
};

function Row({
  row,
  onAcknowledge,
  acknowledging,
}: {
  row: AttentionRow;
  onAcknowledge: (alertId: string) => void;
  acknowledging: boolean;
}) {
  return (
    <li className={styles.row} data-kind={row.kind}>
      <span className={styles.word} data-tone={row.tone}>
        {row.word}
      </span>
      <span className={styles.what}>
        <span className={styles.title}>
          {row.title}
          {row.lab && <LabPill />}
        </span>
        <span className={styles.where}>
          {row.where}
          {row.since !== undefined && (
            <>
              {row.where && <span className={styles.dot}>·</span>}
              <TimeAgo at={row.since} />
            </>
          )}
        </span>
      </span>
      <span className={styles.actions}>
        {row.ackAlertId && (
          <Button
            size="sm"
            variant="ghost"
            loading={acknowledging}
            onClick={() => onAcknowledge(row.ackAlertId as string)}
          >
            Acknowledge
          </Button>
        )}
        {row.action && (
          // A real link, not a Button with a handler: a NOC engineer opens
          // three of these in new tabs before touching the first. The name
          // is on `aria-label` rather than in hidden text, so the row's title
          // appears on screen exactly once.
          <Link
            className={styles.open}
            to={row.action.to}
            aria-label={`${row.action.label}: ${row.title}`}
          >
            {row.action.label}
          </Link>
        )}
      </span>
    </li>
  );
}

/** The list while the first read is in flight. */
export function NeedsAttentionSkeleton() {
  return (
    <Panel title="Needs attention" padding="none">
      <ul className={styles.list} aria-busy="true">
        {[0, 1, 2, 3].map((index) => (
          <li key={index} className={styles.row}>
            <Skeleton width={56} height={18} shape="block" />
            <span className={styles.what}>
              <Skeleton width={`${60 - index * 8}%`} height={13} />
              <Skeleton width={140} height={12} />
            </span>
            <Icon icon={CircleCheck} size={1} className={styles.hidden} />
          </li>
        ))}
      </ul>
    </Panel>
  );
}
