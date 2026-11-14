/**
 * Conflicts: things that cannot be true at the same time.
 *
 * `ipam.conflicts` is a list of findings. Nothing in it is resolved by the
 * backend — it never picks a side — so there is no "fix this" button here
 * either. Each finding shows the backend's own sentence and then every piece
 * of evidence behind it, labelled as a record or as a sighting, because which
 * of the two it is decides what a person should do about it.
 */

import { CircleCheck, ShieldAlert } from 'lucide-react';
import type { Conflict, ConflictKind, Involved } from '@/api/types/ipam';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Icon } from '@/design/Icon';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount } from '@/lib/format';
import styles from './Conflicts.module.css';

const KIND_WORDS: Record<ConflictKind, { label: string; about: string }> = {
  duplicate_ip: {
    label: 'Duplicate address',
    about:
      'One address configured on two devices, or answering with two different MAC addresses inside the window. Virtual router MACs (HSRP, VRRP, GLBP) are recognised and are not counted.',
  },
  overlap: {
    label: 'Overlapping blocks',
    about: 'Two blocks that overlap without either containing the other.',
  },
  outside_any_prefix: {
    label: 'Outside its block',
    about: 'An address record that sits outside the block it claims, or outside every block on record.',
  },
  gateway_mismatch: {
    label: 'Gateway mismatch',
    about:
      'A block documents a gateway address that nothing inside it is configured with. Only raised where something in the block really was read.',
  },
};

export interface ConflictsProps {
  conflicts: readonly Conflict[];
  onOpenSubject: (conflict: Conflict) => void;
}

export function Conflicts({ conflicts, onOpenSubject }: ConflictsProps) {
  if (conflicts.length === 0) {
    return (
      <EmptyState
        icon={CircleCheck}
        title="Nothing contradicts itself"
        description="No duplicate address, no overlapping block, no record outside the block it claims and no documented gateway that is missing from the network — in what has been collected so far."
      />
    );
  }

  return (
    <div className={styles.list}>
      <p className={styles.about}>
        {formatCount(conflicts.length)} finding{conflicts.length === 1 ? '' : 's'}. Each one is
        reported, never resolved: which side is wrong is a decision, and the engine does not make
        it for you.
      </p>
      {conflicts.map((conflict, index) => {
        const words = KIND_WORDS[conflict.kind];
        return (
          <article key={`${conflict.kind}-${conflict.subject}-${index}`} className={styles.card}>
            <header className={styles.head}>
              <Icon icon={ShieldAlert} size={14} />
              <Badge tone="crit" variant="soft">
                {words.label}
              </Badge>
              <span className={styles.subject}>{conflict.subject}</span>
              <Button
                size="sm"
                variant="ghost"
                className={styles.open}
                onClick={() => onOpenSubject(conflict)}
              >
                Look at it
              </Button>
            </header>
            {/* The backend's sentence, quoted. */}
            <p className={styles.detail}>{conflict.detail}</p>
            <p className={styles.about}>{words.about}</p>
            <ul className={styles.involved}>
              {conflict.involved.map((side, position) => (
                <li key={`${side.kind}-${side.device_id ?? side.prefix_id ?? position}`}>
                  <Evidence side={side} />
                </li>
              ))}
            </ul>
          </article>
        );
      })}
    </div>
  );
}

function Evidence({ side }: { side: Involved }) {
  const isRecord = side.kind === 'record';
  return (
    <>
      <Badge tone={isRecord ? 'info' : 'warn'} variant="outline">
        {isRecord ? 'written down' : 'seen on the network'}
      </Badge>
      {side.ip && <span className={styles.mono}>{side.ip}</span>}
      {side.cidr && side.cidr !== side.ip && <span className={styles.mono}>{side.cidr}</span>}
      {side.device && (
        <span className={styles.where}>
          {side.device}
          {side.interface ? ` (${side.interface})` : ''}
        </span>
      )}
      {side.mac && <span className={styles.mono}>{side.mac}</span>}
      {side.collected_at !== undefined && (
        <span className={styles.when}>
          read <TimeAgo at={side.collected_at} />
        </span>
      )}
    </>
  );
}
