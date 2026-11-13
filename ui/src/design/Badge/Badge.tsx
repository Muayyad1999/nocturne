import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import type { Tone } from '../tone';
import styles from './Badge.module.css';

export interface BadgeProps extends ComponentProps<'span'> {
  /** What the colour means. `neutral` carries no meaning. */
  tone?: Tone;
  /** `soft` is a quiet fill; `outline` is quieter still, for dense rows. */
  variant?: 'soft' | 'outline';
}

/**
 * A small label or count: a version, a role, a number of items. For a state
 * (up, degraded, down) use StatusPill, which adds the icon.
 */
export function Badge({ tone = 'neutral', variant = 'soft', className, ...rest }: BadgeProps) {
  return (
    <span {...rest} className={cx(styles.badge, className)} data-tone={tone} data-variant={variant} />
  );
}
