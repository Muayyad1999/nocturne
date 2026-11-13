import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import styles from './Card.module.css';

export interface CardProps extends ComponentProps<'div'> {
  /** Inner spacing. `none` lets content such as a table run edge to edge. */
  padding?: 'none' | 'sm' | 'md';
  /**
   * Tints the card with the assistant's colour: something the AI proposes
   * that a human has not accepted yet.
   */
  proposed?: boolean;
}

/** A plain surface that groups related content. For a titled section, use Panel. */
export function Card({ padding = 'md', proposed = false, className, ...rest }: CardProps) {
  return (
    <div
      {...rest}
      className={cx(styles.card, className)}
      data-padding={padding}
      data-proposed={proposed || undefined}
    />
  );
}
