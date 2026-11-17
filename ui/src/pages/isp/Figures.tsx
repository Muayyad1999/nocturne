/**
 * How an amount, a set of per-currency amounts, and a figure that does not
 * exist are put on screen. Every money cell on these screens goes through
 * one of these, so that none of them can quietly grow a `toFixed(2)`.
 *
 * {@link Amount} renders one `Money` in tabular figures with its code beside
 * it. {@link Figures} renders a `Totals` as one line per currency — there is
 * no prop that collapses it, because the API has no scalar to collapse it to.
 * {@link Words} renders a sentence where a figure would be, in prose type, so
 * "nobody has looked" never looks like a quantity.
 */

import type { ReactNode } from 'react';
import type { Money, Totals } from '@/api/types/isp';
import { Tooltip } from '@/design/Tooltip';
import { cx } from '@/lib/cx';
import { formatMoney, NOTHING } from './money';
import styles from './isp.module.css';

export interface AmountProps {
  money: Money;
  /** Muted: a figure that is beside the point of the row (what is already paid). */
  muted?: boolean;
  className?: string;
}

export function Amount({ money, muted = false, className }: AmountProps) {
  return (
    <span className={cx(styles.amount, muted && styles.amountMuted, className)}>
      {formatMoney(money)}
    </span>
  );
}

export interface FiguresProps {
  totals: Totals;
  /** What to write when there is no money at all. Never a zero. */
  empty?: ReactNode;
  /** `inline` puts them on one row; the default stacks them. */
  layout?: 'stacked' | 'inline';
}

/**
 * One figure per currency. Two dinars and a dollar are three facts, so they
 * get three lines and no sum.
 */
export function Figures({ totals, empty = NOTHING, layout = 'stacked' }: FiguresProps) {
  if (totals.length === 0) return <Words>{empty}</Words>;
  return (
    <ul className={layout === 'inline' ? styles.figuresInline : styles.figures}>
      {totals.map((money) => (
        <li key={money.currency}>{formatMoney(money)}</li>
      ))}
    </ul>
  );
}

export interface WordsProps {
  children: ReactNode;
  /** The backend's own sentence about why there is no figure. */
  title?: string;
  className?: string;
}

/** A sentence where a figure would be. */
export function Words({ children, title, className }: WordsProps) {
  const words = <span className={cx(styles.words, className)}>{children}</span>;
  return title ? (
    <Tooltip content={title} placement="top">
      {words}
    </Tooltip>
  ) : (
    words
  );
}
