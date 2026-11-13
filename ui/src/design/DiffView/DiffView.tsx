import { Equal, UnfoldVertical } from 'lucide-react';
import { useMemo, useState } from 'react';
import { cx } from '@/lib/cx';
import { diffLines, diffStats, groupDiff, type DiffLine } from '@/lib/diff';
import { formatCount } from '@/lib/format';
import { EmptyState } from '../EmptyState';
import { Icon } from '../Icon';
import styles from './DiffView.module.css';

export interface DiffViewProps {
  /** The text as it is. */
  before: string;
  /** The text as it will be. */
  after: string;
  /** Names the comparison in the header: "core-sw-01 running-config". */
  title?: string;
  /** Unchanged lines kept visible around each change. */
  context?: number;
  /** Caps the height (a number is pixels); the diff then scrolls inside. */
  maxHeight?: number | string;
  className?: string;
}

/** A true minus sign: the same width as the plus it is paired with. */
const MINUS = '\u2212';
const SIGN = { same: ' ', add: '+', remove: MINUS } as const;
const SPOKEN = { same: 'unchanged', add: 'added', remove: 'removed' } as const;

function Row({ line }: { line: DiffLine }) {
  return (
    <tr className={styles.row} data-kind={line.kind}>
      <td className={styles.number}>{line.oldNumber}</td>
      <td className={styles.number}>{line.newNumber}</td>
      <td className={styles.sign}>
        <span aria-hidden="true">{SIGN[line.kind]}</span>
        <span className="visually-hidden">{SPOKEN[line.kind]}</span>
      </td>
      <td className={styles.text}>{line.text}</td>
    </tr>
  );
}

/**
 * A unified line diff: removed and added lines marked with − and + (never by
 * colour alone), line numbers of both sides, and long unchanged stretches
 * folded away until asked for.
 */
export function DiffView({ before, after, title, context = 3, maxHeight, className }: DiffViewProps) {
  const lines = useMemo(() => diffLines(before, after), [before, after]);
  const blocks = useMemo(() => groupDiff(lines, context), [lines, context]);
  const { added, removed } = diffStats(lines);
  /** Indices of the folded blocks the reader has opened. */
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(new Set());
  const unchanged = added === 0 && removed === 0;

  return (
    <div className={cx(styles.diff, className)}>
      <div className={styles.header}>
        <span className={styles.title}>{title ?? 'Changes'}</span>
        <span className={styles.stat} data-kind="add">
          +{formatCount(added)}
          <span className="visually-hidden"> lines added</span>
        </span>
        <span className={styles.stat} data-kind="remove">
          {MINUS}
          {formatCount(removed)}
          <span className="visually-hidden"> lines removed</span>
        </span>
      </div>

      {unchanged ? (
        <EmptyState
          size="compact"
          icon={Equal}
          title="No differences"
          description="The two versions are identical."
        />
      ) : (
        // Focusable so a keyboard user can scroll it.
        <div className={styles.scroll} style={{ maxHeight }} tabIndex={0}>
          <table className={styles.table}>
            <caption className="visually-hidden">
              {title ?? 'Changes'}: old line number, new line number, change, text
            </caption>
            {blocks.map((block, index) =>
              block.kind === 'collapsed' && !expanded.has(index) ? (
                <tbody key={index}>
                  <tr>
                    <td colSpan={4} className={styles.foldCell}>
                      <button
                        type="button"
                        className={styles.fold}
                        onClick={() => setExpanded(new Set(expanded).add(index))}
                      >
                        <Icon icon={UnfoldVertical} size={14} />
                        Show {formatCount(block.lines.length)} unchanged lines
                      </button>
                    </td>
                  </tr>
                </tbody>
              ) : (
                <tbody key={index}>
                  {block.lines.map((line) => (
                    <Row key={`${line.oldNumber ?? ''}:${line.newNumber ?? ''}`} line={line} />
                  ))}
                </tbody>
              ),
            )}
          </table>
        </div>
      )}
    </div>
  );
}
