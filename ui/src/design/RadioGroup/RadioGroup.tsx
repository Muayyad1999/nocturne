import { useId, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { useFieldLabelId } from '../Field';
import styles from './RadioGroup.module.css';

export interface RadioOption {
  value: string;
  label: ReactNode;
  /** A line of explanation under the label. */
  description?: ReactNode;
  disabled?: boolean;
}

export interface RadioGroupProps {
  options: readonly RadioOption[];
  /** The chosen value, or null when nothing is chosen yet. */
  value: string | null;
  onChange: (value: string) => void;
  orientation?: 'vertical' | 'horizontal';
  disabled?: boolean;
  /** Names the group. Inside a Field the Field's label is used. */
  'aria-label'?: string;
  /** Form field name. Defaults to a generated one, which is all the grouping needs. */
  name?: string;
  className?: string;
}

/** One choice out of a few, all visible. Arrow keys move between the options. */
export function RadioGroup({
  options,
  value,
  onChange,
  orientation = 'vertical',
  disabled = false,
  name,
  className,
  ...rest
}: RadioGroupProps) {
  const generatedName = useId();
  const labelId = useFieldLabelId();

  return (
    <div
      role="radiogroup"
      aria-label={rest['aria-label']}
      aria-labelledby={rest['aria-label'] ? undefined : labelId}
      className={cx(styles.group, className)}
      data-orientation={orientation}
    >
      {options.map((option) => {
        const optionDisabled = disabled || option.disabled;
        return (
          <label key={option.value} className={styles.option} data-disabled={optionDisabled || undefined}>
            <input
              type="radio"
              className={styles.input}
              name={name ?? generatedName}
              value={option.value}
              checked={option.value === value}
              disabled={optionDisabled}
              onChange={() => onChange(option.value)}
            />
            <span className={styles.dot} aria-hidden="true" />
            <span className={styles.text}>
              <span>{option.label}</span>
              {option.description && <span className={styles.description}>{option.description}</span>}
            </span>
          </label>
        );
      })}
    </div>
  );
}
