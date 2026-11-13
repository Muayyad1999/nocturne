import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import { formatShortcut } from '@/lib/keyboard';
import styles from './Kbd.module.css';

export interface KbdProps extends ComponentProps<'kbd'> {
  /**
   * A shortcut in `lib/keyboard` notation (`mod+k`), rendered as the key caps
   * of the current platform. Without it, the children are one key cap.
   */
  shortcut?: string;
}

/** A key cap, or a whole shortcut. */
export function Kbd({ shortcut, className, children, ...rest }: KbdProps) {
  if (!shortcut) {
    return (
      <kbd {...rest} className={cx(styles.key, className)}>
        {children}
      </kbd>
    );
  }
  return (
    <kbd {...rest} className={cx(styles.shortcut, className)}>
      {formatShortcut(shortcut).map((cap) => (
        <kbd key={cap} className={styles.key}>
          {cap}
        </kbd>
      ))}
    </kbd>
  );
}
