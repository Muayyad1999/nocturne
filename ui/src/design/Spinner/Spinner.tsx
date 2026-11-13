import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import styles from './Spinner.module.css';

export interface SpinnerProps extends Omit<ComponentProps<'span'>, 'children'> {
  /** Diameter in pixels. */
  size?: number;
  /**
   * What is loading, announced to assistive technology. Pass `null` when the
   * spinner sits inside something that already says so (a busy button).
   */
  label?: string | null;
}

/**
 * Activity indicator for a small region: a button, a row, a panel header.
 * A whole page that is loading shows Skeletons shaped like its content instead.
 */
export function Spinner({ size = 16, label = 'Loading', className, ...rest }: SpinnerProps) {
  return (
    <span
      {...rest}
      className={cx(styles.spinner, className)}
      role={label === null ? undefined : 'status'}
      aria-hidden={label === null ? true : undefined}
    >
      <svg width={size} height={size} viewBox="0 0 16 16" fill="none" aria-hidden="true">
        <circle className={styles.track} cx="8" cy="8" r="6" strokeWidth="2" />
        <path className={styles.arc} d="M8 2a6 6 0 0 1 6 6" strokeWidth="2" strokeLinecap="round" />
      </svg>
      {label !== null && <span className="visually-hidden">{label}</span>}
    </span>
  );
}
