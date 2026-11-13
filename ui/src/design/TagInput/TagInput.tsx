import { X } from 'lucide-react';
import { useId, useRef, useState, type ClipboardEvent, type KeyboardEvent } from 'react';
import { cx } from '@/lib/cx';
import { useFieldControl, useFieldInvalid } from '../Field';
import { Icon } from '../Icon';
import styles from './TagInput.module.css';

export interface TagInputProps {
  value: readonly string[];
  onChange: (tags: string[]) => void;
  /**
   * Checks a tag before it is added. Return a sentence to reject it (shown
   * under the field), or null to accept. Duplicates are always ignored.
   */
  validate?: (tag: string) => string | null;
  placeholder?: string;
  disabled?: boolean;
  /** At least one tag is needed. Inside a required Field this is set for you. */
  required?: boolean;
  /** Marks the value as wrong. Inside a Field with an error this is set for you. */
  invalid?: boolean;
  /** Monospace, for addresses, VLAN ids, interface names. */
  mono?: boolean;
  id?: string;
  'aria-label'?: string;
  'aria-describedby'?: string;
  className?: string;
}

const SEPARATORS = /[\s,;]+/;

/**
 * A list of short values entered one at a time: Enter or a comma adds what was
 * typed, Backspace in the empty field removes the last one, and pasting a
 * list adds every entry.
 */
export function TagInput({
  value,
  onChange,
  validate,
  placeholder,
  disabled = false,
  invalid,
  mono = false,
  className,
  ...rest
}: TagInputProps) {
  const [text, setText] = useState('');
  const [rejection, setRejection] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const rejectionId = useId();
  const fieldInvalid = useFieldInvalid();
  const control = useFieldControl(rest);
  const isInvalid = invalid ?? (fieldInvalid || rejection !== null);

  /** Adds every acceptable entry; returns false if one was rejected. */
  const add = (entries: string[]): boolean => {
    const next = [...value];
    let problem: string | null = null;
    for (const entry of entries) {
      const tag = entry.trim();
      if (!tag || next.includes(tag)) continue;
      const message = validate?.(tag) ?? null;
      if (message) problem ??= message;
      else next.push(tag);
    }
    if (next.length !== value.length) onChange(next);
    setRejection(problem);
    return problem === null;
  };

  const commit = () => {
    if (!text.trim()) return;
    if (add([text])) setText('');
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Enter' || event.key === ',') {
      event.preventDefault();
      commit();
    } else if (event.key === 'Backspace' && text === '' && value.length > 0) {
      onChange(value.slice(0, -1));
    }
  };

  const onPaste = (event: ClipboardEvent<HTMLInputElement>) => {
    const pasted = event.clipboardData.getData('text');
    if (!SEPARATORS.test(pasted.trim())) return;
    event.preventDefault();
    add(pasted.split(SEPARATORS));
  };

  return (
    <div className={cx(styles.wrapper, className)}>
      <div
        className={styles.control}
        data-control=""
        data-invalid={isInvalid || undefined}
        data-disabled={disabled || undefined}
        data-mono={mono || undefined}
        // A press on the padding lands in the field, as it would in an input.
        onPointerDown={(event) => {
          if (event.target === event.currentTarget) {
            event.preventDefault();
            inputRef.current?.focus();
          }
        }}
      >
        {value.map((tag) => (
          <span key={tag} className={styles.tag}>
            {tag}
            <button
              type="button"
              className={styles.remove}
              aria-label={`Remove ${tag}`}
              disabled={disabled}
              onClick={() => {
                onChange(value.filter((entry) => entry !== tag));
                inputRef.current?.focus();
              }}
            >
              <Icon icon={X} size={12} />
            </button>
          </span>
        ))}
        <input
          {...control}
          ref={inputRef}
          className={styles.input}
          data-control-native=""
          value={text}
          disabled={disabled}
          // The field is satisfied by its tags, not by the text being typed.
          required={Boolean(control.required) && value.length === 0}
          placeholder={value.length === 0 ? placeholder : undefined}
          autoComplete="off"
          spellCheck={false}
          aria-invalid={isInvalid || undefined}
          aria-describedby={
            [control['aria-describedby'], rejection ? rejectionId : undefined].filter(Boolean).join(' ') ||
            undefined
          }
          onChange={(event) => {
            setText(event.target.value);
            setRejection(null);
          }}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onBlur={commit}
        />
      </div>
      {rejection && (
        <p id={rejectionId} className={styles.rejection} role="alert">
          {rejection}
        </p>
      )}
    </div>
  );
}
