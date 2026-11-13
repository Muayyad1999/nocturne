import { ArrowDownRight, ArrowRight, ArrowUpRight } from 'lucide-react';
import type { ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { Icon } from '../Icon';
import { Skeleton } from '../Skeleton';
import styles from './Stat.module.css';

export interface StatDelta {
  /** The change, already formatted with its sign: `formatDelta(4.2, formatPercent)`. */
  text: string;
  direction: 'up' | 'down' | 'flat';
  /**
   * Whether this change is good or bad news. Up is good for throughput and
   * bad for errors, so the screen has to say; without it the delta is neutral.
   */
  sentiment?: 'good' | 'bad';
  /** What it is compared with: "vs last week", "since 09:00". */
  period?: string;
}

export interface StatProps {
  /** What is measured, in sentence case. */
  label: string;
  /** The value, formatted with `lib/format`. Missing data is that module's dash, never a zero. */
  value: string;
  unit?: string;
  delta?: StatDelta;
  /** A Sparkline of the same measure, drawn at the right. */
  trend?: ReactNode;
  size?: 'sm' | 'lg';
  /** Shows the outline of the figure while the value is on its way. */
  loading?: boolean;
  className?: string;
}

const DIRECTION_ICON = { up: ArrowUpRight, down: ArrowDownRight, flat: ArrowRight } as const;
const DIRECTION_WORD = { up: 'Up', down: 'Down', flat: 'No change:' } as const;

/**
 * One number that matters: label, value, unit, and optionally how it changed
 * and where it has been. The number is the chart.
 */
export function Stat({ label, value, unit, delta, trend, size = 'lg', loading = false, className }: StatProps) {
  return (
    <div className={cx(styles.stat, className)} data-size={size} aria-busy={loading || undefined}>
      <div className={styles.figure}>
        <div className={styles.label}>{label}</div>
        {loading ? (
          <Skeleton width={96} height={size === 'lg' ? 28 : 20} className={styles.placeholder} />
        ) : (
          <div className={styles.reading}>
            <span className={styles.value}>{value}</span>
            {unit && <span className={styles.unit}>{unit}</span>}
          </div>
        )}
        {delta && !loading && (
          <div className={styles.delta} data-sentiment={delta.sentiment}>
            <Icon icon={DIRECTION_ICON[delta.direction]} size={14} />
            <span className="visually-hidden">{DIRECTION_WORD[delta.direction]}</span>
            <span className={styles.deltaText}>{delta.text}</span>
            {delta.period && <span className={styles.period}>{delta.period}</span>}
          </div>
        )}
      </div>
      {trend && !loading && <div className={styles.trend}>{trend}</div>}
    </div>
  );
}
