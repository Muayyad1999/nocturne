import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import type { StatusTone } from '../tone';
import styles from './ProgressBar.module.css';

export interface ProgressBarProps extends Omit<ComponentProps<'div'>, 'children' | 'role'> {
  /** How far along, from 0 to `max`. Leave it out while the amount of work is unknown. */
  value?: number;
  max?: number;
  /**
   * `progress`: a task moving towards completion. `meter`: a level within a
   * known range (CPU, disk, a licence count), where the tone carries the
   * severity of that level.
   */
  kind?: 'progress' | 'meter';
  tone?: 'accent' | 'ai' | StatusTone;
  size?: 'sm' | 'md';
  /** The value as words, for assistive technology: "62%", "3 of 12 devices". */
  valueText?: string;
}

/**
 * A horizontal bar: progress of a task, or a level against a limit. Name it
 * with `aria-label` or `aria-labelledby`.
 */
export function ProgressBar({
  value,
  max = 100,
  kind = 'progress',
  tone = 'accent',
  size = 'md',
  valueText,
  className,
  ...rest
}: ProgressBarProps) {
  const known = value !== undefined && Number.isFinite(value) && max > 0;
  const ratio = known ? Math.min(1, Math.max(0, value / max)) : 0;

  return (
    <div
      {...rest}
      className={cx(styles.track, className)}
      role={kind === 'meter' ? 'meter' : 'progressbar'}
      aria-valuemin={0}
      aria-valuemax={max}
      aria-valuenow={known ? value : undefined}
      aria-valuetext={valueText}
      data-tone={tone}
      data-size={size}
      data-indeterminate={!known || undefined}
    >
      <div className={styles.fill} style={known ? { width: `${ratio * 100}%` } : undefined} />
    </div>
  );
}
