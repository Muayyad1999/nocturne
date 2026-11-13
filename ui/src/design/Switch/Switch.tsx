import type { ComponentProps, ReactNode } from 'react';
import { cx } from '@/lib/cx';
import styles from './Switch.module.css';

export interface SwitchProps extends Omit<ComponentProps<'input'>, 'type' | 'role' | 'children'> {
  /** The label. Without one, pass `aria-label`. */
  children?: ReactNode;
}

/**
 * An on/off setting that takes effect at once. For a choice that is submitted
 * with a form, use Checkbox. `ref` reaches the `<input>`.
 */
export function Switch({ children, className, ...rest }: SwitchProps) {
  return (
    <label className={cx(styles.switch, className)} data-disabled={rest.disabled || undefined}>
      <input {...rest} type="checkbox" role="switch" className={styles.input} />
      <span className={styles.track} aria-hidden="true">
        <span className={styles.thumb} />
      </span>
      {children && <span className={styles.label}>{children}</span>}
    </label>
  );
}
