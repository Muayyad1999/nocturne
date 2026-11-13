import { ChevronDown } from 'lucide-react';
import type { ComponentProps } from 'react';
import { cx } from '@/lib/cx';
import { useFieldControl, useFieldInvalid } from '../Field';
import { Icon } from '../Icon';
import styles from './Select.module.css';

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface SelectProps extends Omit<ComponentProps<'select'>, 'size'> {
  /** The choices. For groups, pass `<optgroup>` / `<option>` children instead. */
  options?: readonly SelectOption[];
  size?: 'sm' | 'md';
  /** Marks the value as wrong. Inside a Field with an error this is set for you. */
  invalid?: boolean;
}

/**
 * A native `<select>` in the system's clothes: the platform's own list,
 * keyboard behaviour and accessibility. For long lists that need filtering,
 * use Combobox.
 */
export function Select({ options, size = 'md', invalid, className, children, ...rest }: SelectProps) {
  const fieldInvalid = useFieldInvalid();
  const props = useFieldControl(rest);
  const isInvalid = invalid ?? fieldInvalid;

  return (
    <div
      className={cx(styles.select, className)}
      data-size={size}
      data-invalid={isInvalid || undefined}
      data-disabled={props.disabled || undefined}
    >
      <select {...props} className={styles.native} aria-invalid={props['aria-invalid'] ?? (isInvalid || undefined)}>
        {options?.map((option) => (
          <option key={option.value} value={option.value} disabled={option.disabled}>
            {option.label}
          </option>
        ))}
        {children}
      </select>
      <Icon icon={ChevronDown} size={14} className={styles.chevron} />
    </div>
  );
}
