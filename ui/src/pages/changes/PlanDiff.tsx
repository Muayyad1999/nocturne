/**
 * The predicted configuration, as a diff against what is on the device now.
 *
 * The engine computes this diff when it plans (it is what verification
 * compares the device against afterwards), so the screen renders the lines it
 * was given rather than diffing anything itself: what is shown here is
 * exactly what Nocturne predicted, down to the line numbers.
 */

import { useState } from 'react';
import type { TextDiff } from '@/api/types/changes';
import { Button } from '@/design/Button';
import { EMPTY } from '@/lib/format';
import { diffTagLabel } from './change';
import styles from './PlanDiff.module.css';

export interface PlanDiffProps {
  diff: TextDiff;
  /** Shown in the header: `Predicted configuration of sw-1`. */
  title: string;
  /** Lines to show before "Show the rest"; 0 shows everything. */
  limit?: number;
}

function kindOf(tag: string): 'add' | 'remove' | 'gap' | 'same' {
  if (tag === '+') return 'add';
  if (tag === '-') return 'remove';
  if (tag === '@') return 'gap';
  return 'same';
}

export function PlanDiff({ diff, title, limit = 120 }: PlanDiffProps) {
  const [all, setAll] = useState(false);
  const lines = all || limit === 0 ? diff.lines : diff.lines.slice(0, limit);
  const hidden = diff.lines.length - lines.length;

  if (diff.lines.length === 0) {
    return (
      <div className={styles.diff}>
        <div className={styles.header}>
          <span className={styles.title}>{title}</span>
        </div>
        <p className={styles.nothing}>
          The engine predicts no difference: the device already has what this change asks for.
        </p>
      </div>
    );
  }

  return (
    <div className={styles.diff}>
      <div className={styles.header}>
        <span className={styles.title}>{title}</span>
        <span className={styles.stat} data-kind="add">
          +{diff.added}
        </span>
        <span className={styles.stat} data-kind="remove">
          −{diff.removed}
        </span>
      </div>
      <div className={styles.scroll} tabIndex={0} role="group" aria-label={title}>
        <table className={styles.table}>
          <tbody>
            {lines.map((line, index) => {
              const kind = kindOf(line.tag);
              return (
                <tr
                  key={`${index}-${line.text}`}
                  className={styles.row}
                  data-kind={kind}
                  data-testid="diff-line"
                >
                  <td className={styles.number}>{line.old_line ?? ''}</td>
                  <td className={styles.number}>{line.new_line ?? ''}</td>
                  <td className={styles.sign} aria-hidden>
                    {kind === 'add' ? '+' : kind === 'remove' ? '−' : kind === 'gap' ? '⋯' : ' '}
                  </td>
                  <td className={styles.text}>
                    <span className={styles.srOnly}>{diffTagLabel(line)}: </span>
                    {kind === 'gap' ? EMPTY : line.text || ' '}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {hidden > 0 && (
        <div className={styles.more}>
          <Button size="sm" variant="ghost" onClick={() => setAll(true)}>
            Show the remaining {hidden} lines
          </Button>
        </div>
      )}
    </div>
  );
}
