import { useEffect, useRef, type ComponentProps, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { useMergedRefs } from '@/lib/refs';
import styles from './Checkbox.module.css';

export interface CheckboxProps extends Omit<ComponentProps<'input'>, 'type' | 'children'> {
  /** The label. Without one, pass `aria-label`. */
  children?: ReactNode;
  /** Neither on nor off: some, but not all, of what it stands for is selected. */
  indeterminate?: boolean;
}

/** A native checkbox with a label. `ref` reaches the `<input>`. */
export function Checkbox({ children, indeterminate = false, className, ref, ...rest }: CheckboxProps) {
  const inputRef = useRef<HTMLInputElement>(null);
  const mergedRef = useMergedRefs(inputRef, ref);

  // `indeterminate` exists only as a DOM property.
  useEffect(() => {
    if (inputRef.current) inputRef.current.indeterminate = indeterminate;
  }, [indeterminate]);

  return (
    <label className={cx(styles.checkbox, className)} data-disabled={rest.disabled || undefined}>
      <input {...rest} ref={mergedRef} type="checkbox" className={styles.input} />
      <span className={styles.box} aria-hidden="true">
        <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
          <path className={styles.check} d="M2.5 6.2 5 8.6l4.5-5" />
          <path className={styles.dash} d="M3 6h6" />
        </svg>
      </span>
      {children && <span className={styles.label}>{children}</span>}
    </label>
  );
}
