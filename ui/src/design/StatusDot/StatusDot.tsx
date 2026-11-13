import type { ComponentProps, ReactNode } from 'react';
import { cx } from '@/lib/cx';
import type { StatusTone } from '../tone';
import styles from './StatusDot.module.css';

export interface StatusDotProps extends Omit<ComponentProps<'span'>, 'children'> {
  tone: StatusTone;
  /**
   * What the dot means, for assistive technology. Leave it out only when text
   * right next to the dot already says it.
   */
  label?: string;
  /** Diameter in pixels. */
  size?: number;
  /** A slow halo for something happening right now (an active alarm, a live session). */
  pulse?: boolean;
}

/**
 * Each status has its own silhouette, so a dot never depends on colour alone:
 * circle, triangle, square, diamond, ring and half-disc.
 */
const SHAPES: Record<StatusTone, ReactNode> = {
  ok: <circle cx="5" cy="5" r="4" fill="currentColor" />,
  warn: <path d="M5 1.1 9.4 8.7H.6Z" fill="currentColor" stroke="currentColor" strokeWidth="0.6" strokeLinejoin="round" />,
  crit: <rect x="1.4" y="1.4" width="7.2" height="7.2" rx="1.4" fill="currentColor" />,
  info: <rect x="1.9" y="1.9" width="6.2" height="6.2" rx="1" fill="currentColor" transform="rotate(45 5 5)" />,
  idle: <circle cx="5" cy="5" r="3.25" stroke="currentColor" strokeWidth="1.5" />,
  maint: (
    <>
      <circle cx="5" cy="5" r="3.25" stroke="currentColor" strokeWidth="1.5" />
      <path d="M5 1.75a3.25 3.25 0 0 0 0 6.5Z" fill="currentColor" />
    </>
  ),
};

/** The smallest status indicator: a coloured, shape-coded dot. */
export function StatusDot({ tone, label, size = 10, pulse = false, className, ...rest }: StatusDotProps) {
  return (
    <span
      {...rest}
      className={cx(styles.dot, className)}
      data-tone={tone}
      data-pulse={pulse || undefined}
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <svg width={size} height={size} viewBox="0 0 10 10" fill="none" aria-hidden="true">
        {SHAPES[tone]}
      </svg>
    </span>
  );
}
