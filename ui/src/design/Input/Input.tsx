import type { ComponentProps, ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { useFieldControl, useFieldInvalid } from '../Field';
import styles from './Input.module.css';

export interface InputProps extends Omit<ComponentProps<'input'>, 'size'> {
  size?: 'sm' | 'md';
  /** Marks the value as wrong. Inside a Field with an error this is set for you. */
  invalid?: boolean;
  /** Monospace, for anything a device said or will be told: addresses, commands, OIDs. */
  mono?: boolean;
  /** Shown inside the field before the text: an icon, a prefix. */
  leading?: ReactNode;
  /** Shown inside the field after the text: a unit, a button. */
  trailing?: ReactNode;
}

/** A single-line text field. `ref` reaches the `<input>`; `className` styles the box around it. */
export function Input({
  size = 'md',
  invalid,
  mono = false,
  leading,
  trailing,
  className,
  ...rest
}: InputProps) {
  const fieldInvalid = useFieldInvalid();
  const props = useFieldControl(rest);
  const isInvalid = invalid ?? fieldInvalid;

  return (
    <div
      className={cx(styles.input, className)}
      data-size={size}
      data-invalid={isInvalid || undefined}
      data-disabled={props.disabled || undefined}
      data-mono={mono || undefined}
    >
      {leading && <span className={styles.adornment}>{leading}</span>}
      <input {...props} className={styles.native} aria-invalid={props['aria-invalid'] ?? (isInvalid || undefined)} />
      {trailing && <span className={styles.adornment}>{trailing}</span>}
    </div>
  );
}
