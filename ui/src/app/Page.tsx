import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import styles from './Page.module.css';

export interface PageProps extends ComponentProps<'div'> {
  /**
   * Removes the page padding, for screens that own their whole area: a
   * terminal, a topology canvas, a split view.
   */
  flush?: boolean;
}

/**
 * The body of a screen: a column that starts with a Toolbar and continues
 * with content. It scrolls when the content is taller than the window; a
 * child that should fill the remaining height (a table, a canvas) takes
 * `flex: 1; min-height: 0` and scrolls inside itself instead.
 */
export function Page({ flush = false, className, ...rest }: PageProps) {
  return <div {...rest} className={cx(styles.page, className)} data-flush={flush || undefined} />;
}
