import { CircleExclamationPoint } from 'lucide-react';
import { createContext, use, useId, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { Icon } from '../Icon';
import styles from './Field.module.css';

interface FieldContextValue {
  /** The id the control must carry so the label points at it. */
  controlId: string;
  labelId: string;
  /** Id of the hint or error text, when there is one. */
  descriptionId?: string;
  invalid: boolean;
  required: boolean;
}

const FieldContext = createContext<FieldContextValue | null>(null);

export interface FieldProps {
  label: ReactNode;
  /** Help that is always visible: the expected format, a consequence. */
  hint?: ReactNode;
  /** What is wrong and how to fix it. Replaces the hint and marks the control invalid. */
  error?: ReactNode;
  required?: boolean;
  /** One control: Input, Textarea, Select, Combobox, TagInput, Slider or RadioGroup. */
  children: ReactNode;
  className?: string;
}

/**
 * Wraps a control with its label, hint and error and wires them together for
 * assistive technology. The control picks the wiring up by itself.
 */
export function Field({ label, hint, error, required = false, children, className }: FieldProps) {
  const id = useId();
  const invalid = Boolean(error);
  const description = error ?? hint;
  const context: FieldContextValue = {
    controlId: `${id}-control`,
    labelId: `${id}-label`,
    descriptionId: description ? `${id}-description` : undefined,
    invalid,
    required,
  };

  return (
    <div className={cx(styles.field, className)}>
      <label id={context.labelId} htmlFor={context.controlId} className={styles.label}>
        {label}
        {required && (
          <span className={styles.required} aria-hidden="true">
            required
          </span>
        )}
      </label>
      <FieldContext value={context}>{children}</FieldContext>
      {description && (
        <p id={context.descriptionId} className={styles.description} data-invalid={invalid || undefined}>
          {invalid && <Icon icon={CircleExclamationPoint} size={12} />}
          <span>{description}</span>
        </p>
      )}
    </div>
  );
}

export interface FieldControlProps {
  id?: string;
  required?: boolean;
  'aria-describedby'?: string;
  'aria-invalid'?: boolean | 'true' | 'false' | 'grammar' | 'spelling';
}

/**
 * For control components: merges the enclosing Field's wiring into the
 * control's own props. Outside a Field the props pass through unchanged.
 */
export function useFieldControl<P extends FieldControlProps>(props: P): P {
  const field = use(FieldContext);
  if (!field) return props;
  return {
    ...props,
    id: props.id ?? field.controlId,
    required: props.required ?? (field.required || undefined),
    'aria-describedby':
      [props['aria-describedby'], field.descriptionId].filter(Boolean).join(' ') || undefined,
    'aria-invalid': props['aria-invalid'] ?? (field.invalid || undefined),
  };
}

/** For group controls (RadioGroup): the id of the Field's label, to name the group. */
export function useFieldLabelId(): string | undefined {
  return use(FieldContext)?.labelId;
}

/** Whether the enclosing Field is showing an error. */
export function useFieldInvalid(): boolean {
  return use(FieldContext)?.invalid ?? false;
}
