import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import { useFieldControl, useFieldInvalid } from '../Field';
import styles from './Textarea.module.css';

export interface TextareaProps extends ComponentProps<'textarea'> {
  /** Marks the value as wrong. Inside a Field with an error this is set for you. */
  invalid?: boolean;
  /** Monospace, for config snippets and commands. */
  mono?: boolean;
}

/** A multi-line text field. It can be resized vertically. */
export function Textarea({ invalid, mono = false, rows = 3, className, ...rest }: TextareaProps) {
  const fieldInvalid = useFieldInvalid();
  const props = useFieldControl(rest);
  const isInvalid = invalid ?? fieldInvalid;

  return (
    <textarea
      {...props}
      rows={rows}
      className={cx(styles.textarea, className)}
      data-control=""
      data-invalid={isInvalid || undefined}
      data-disabled={props.disabled || undefined}
      data-mono={mono || undefined}
      aria-invalid={props['aria-invalid'] ?? (isInvalid || undefined)}
    />
  );
}
