import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import styles from './Toolbar.module.css';

export type ToolbarProps = ComponentProps<'div'>;

/**
 * The row at the top of a page or panel: filters on the left, then a
 * ToolbarSpacer, then the primary action on the right.
 */
export function Toolbar({ className, ...rest }: ToolbarProps) {
  return <div {...rest} className={cx(styles.toolbar, className)} />;
}

/** Pushes what follows it in a Toolbar to the right edge. */
export function ToolbarSpacer() {
  return <div className={styles.spacer} />;
}
