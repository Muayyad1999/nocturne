/**
 * Two stored configurations, compared.
 *
 * The diff is the engine's (`config.diff`), rendered as it arrived — line
 * numbers, the lines it chose to keep as context, the `…` where it skipped
 * unchanged lines, and its own counts. The browser does not re-diff
 * anything, for the same reason the Changes screen does not: the engine's
 * hierarchy-aware diff is what the rest of Nocturne acts on, and a second
 * opinion computed here could differ from it and be believed.
 *
 * Side by side is therefore a *projection* of the same unified diff into two
 * columns (`sideBySide` in `vocab.ts`), not a second diff.
 */

import { Columns2, Rows3 } from 'lucide-react';
import { useState } from 'react';
import type { DiffResult } from '@/api/types/config';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { formatCount } from '@/lib/format';
import { RedactedLine } from './Redacted';
import styles from './VersionDiff.module.css';
import { diffTagLabel, sectionLabel, sideBySide } from './vocab';

export interface VersionDiffProps {
  diff: DiffResult;
  /** Names the comparison for assistive technology. */
  label: string;
}

type Layout = 'unified' | 'split';

function kindOf(tag: string): 'add' | 'remove' | 'gap' | 'same' {
  if (tag === '+') return 'add';
  if (tag === '-') return 'remove';
  if (tag === '@') return 'gap';
  return 'same';
}

export function VersionDiff({ diff, label }: VersionDiffProps) {
  const [layout, setLayout] = useState<Layout>('unified');
  const { added, removed, lines } = diff.diff;

  return (
    <div className={styles.diff}>
      <div className={styles.header}>
        <span className={styles.counts}>
          <span className={styles.stat} data-kind="add">
            +{formatCount(added)}
          </span>
          <span className={styles.stat} data-kind="remove">
            −{formatCount(removed)}
          </span>
        </span>
        <div className={styles.sections}>
          {diff.sections.length === 0
            ? null
            : diff.sections.map((section) => (
              <Badge key={sectionLabel(section.path)} variant="outline" title={sectionLabel(section.path)}>
                <span className={styles.sectionName}>{sectionLabel(section.path)}</span>
                <span className={styles.sectionCounts}>
                  {section.added > 0 && <span data-kind="add">+{section.added}</span>}
                  {section.removed > 0 && <span data-kind="remove">−{section.removed}</span>}
                </span>
              </Badge>
              ))}
        </div>
        <Button
          size="sm"
          variant="ghost"
          icon={layout === 'unified' ? Columns2 : Rows3}
          onClick={() => setLayout(layout === 'unified' ? 'split' : 'unified')}
        >
          {layout === 'unified' ? 'Side by side' : 'One column'}
        </Button>
      </div>

      {lines.length === 0 ? (
        <p className={styles.nothing}>
          The engine finds no difference between these two versions: the stored text is identical.
        </p>
      ) : layout === 'unified' ? (
        <div className={styles.scroll} tabIndex={0} role="group" aria-label={label}>
          <table className={styles.table}>
            <tbody>
              {lines.map((line, index) => {
                const kind = kindOf(line.tag);
                return (
                  <tr key={index} className={styles.row} data-kind={kind} data-testid="diff-line">
                    <td className={styles.number}>{line.old_line ?? ''}</td>
                    <td className={styles.number}>{line.new_line ?? ''}</td>
                    <td className={styles.sign} aria-hidden>
                      {kind === 'add' ? '+' : kind === 'remove' ? '−' : kind === 'gap' ? '⋯' : ' '}
                    </td>
                    <td className={styles.text}>
                      <span className="visually-hidden">{diffTagLabel(line.tag)}: </span>
                      {kind === 'gap' ? (
                        <span className={styles.gap}>unchanged lines</span>
                      ) : (
                        <RedactedLine line={line.text || ' '} />
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      ) : (
        <div className={styles.scroll} tabIndex={0} role="group" aria-label={`${label}, side by side`}>
          <table className={styles.table} data-split>
            <tbody>
              {sideBySide(lines).map((row, index) => (
                <tr key={index} className={styles.row} data-kind={row.kind} data-testid="diff-row">
                  <td className={styles.number}>{row.left?.no ?? ''}</td>
                  <td className={styles.text} data-side="left" data-empty={!row.left || undefined}>
                    {row.kind === 'gap' ? (
                      <span className={styles.gap}>unchanged lines</span>
                    ) : row.left ? (
                      <RedactedLine line={row.left.text || ' '} />
                    ) : (
                      ''
                    )}
                  </td>
                  <td className={styles.number}>{row.right?.no ?? ''}</td>
                  <td className={styles.text} data-side="right" data-empty={!row.right || undefined}>
                    {row.kind === 'gap' ? (
                      <span className={styles.gap}>unchanged lines</span>
                    ) : row.right ? (
                      <RedactedLine line={row.right.text || ' '} />
                    ) : (
                      ''
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
