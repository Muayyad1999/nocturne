/**
 * What this drawing cannot tell you.
 *
 * `topology.get` returns sentences meant for a person: a device that reported
 * no neighbours, a name two devices answer to and so was not guessed at, data
 * older than the staleness setting. A drawing that hides its own caveats is
 * worse than no drawing, so they sit above the canvas, counted, and they are
 * the backend's words — not a paraphrase, not a count on its own.
 *
 * The age of the data is in the same band, because "this is six hours old" is
 * the same kind of fact and has the same remedy: collect again.
 */

import { AlertTriangle, ChevronDown, ChevronUp, Clock, RefreshCw } from 'lucide-react';
import { useState } from 'react';
import type { Graph } from '@/api/types/topology';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { plural } from '@/lib/format';
import { formatRelative, formatTimestamp } from '@/lib/time';
import styles from './Warnings.module.css';

export interface WarningsProps {
  graph: Graph;
  /** How old the data may be before it is called stale (hours, from settings). */
  staleAfterHours: number | undefined;
  onRefresh: () => void;
  refreshing: boolean;
}

export function Warnings({ graph, staleAfterHours, onRefresh, refreshing }: WarningsProps) {
  const [open, setOpen] = useState(false);
  const warnings = graph.warnings;
  const oldest = graph.data_age.oldest_at;
  const limit = (staleAfterHours ?? 24) * 3_600_000;
  const old = oldest !== undefined && Date.now() - oldest > limit;

  if (warnings.length === 0 && !old) return null;

  return (
    <section className={styles.band} aria-label="What this drawing cannot tell you">
      <div className={styles.row}>
        {warnings.length > 0 && (
          <button
            type="button"
            className={styles.summary}
            onClick={() => setOpen((was) => !was)}
            aria-expanded={open}
          >
            <Icon icon={AlertTriangle} size={14} />
            <span>{plural(warnings.length, 'thing')} this drawing cannot tell you</span>
            <Icon icon={open ? ChevronUp : ChevronDown} size={14} />
          </button>
        )}

        {old && oldest !== undefined && (
          <p className={styles.age} title={formatTimestamp(oldest)}>
            <Icon icon={Clock} size={14} />
            The oldest data here was collected {formatRelative(oldest)}
            {graph.stats.stale_devices > 0 && (
              <>
                {' '}
                — {plural(graph.stats.stale_devices, 'device')} past the {staleAfterHours ?? 24}-hour
                limit
              </>
            )}
            .
          </p>
        )}

        {/*
          Only offered where it is the answer to what the band says: the
          toolbar's own "Refresh data" is a few pixels above, and two of them
          side by side read as two different things.
        */}
        {old && (
          <Button
            size="sm"
            variant="secondary"
            icon={RefreshCw}
            loading={refreshing}
            onClick={onRefresh}
          >
            Collect again
          </Button>
        )}
      </div>

      {open && warnings.length > 0 && (
        <ul className={styles.list}>
          {warnings.map((warning) => (
            <li key={warning}>{warning}</li>
          ))}
        </ul>
      )}
    </section>
  );
}
