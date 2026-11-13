import { Check, ChevronsUpDown, X } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { cx } from '@/lib/cx';
import { fuzzyFilter } from '@/lib/fuzzy';
import { useFieldControl, useFieldInvalid } from '../Field';
import { Highlight } from '../Highlight';
import { Icon } from '../Icon';
import { useLayer } from '../overlay/layers';
import { Portal } from '../overlay/Portal';
import { useFloating } from '../overlay/useFloating';
import styles from './Combobox.module.css';

export interface ComboboxOption {
  value: string;
  label: string;
  /** Secondary text at the right: a site, a platform, an address. Also searched. */
  description?: string;
  disabled?: boolean;
}

export interface ComboboxProps {
  options: readonly ComboboxOption[];
  /** The selected option's value, or null. */
  value: string | null;
  onChange: (value: string | null) => void;
  placeholder?: string;
  /** Shown in the list when nothing matches what was typed. */
  emptyText?: string;
  /** Offers a button that clears the selection. */
  clearable?: boolean;
  disabled?: boolean;
  /** Marks the value as wrong. Inside a Field with an error this is set for you. */
  invalid?: boolean;
  size?: 'sm' | 'md';
  id?: string;
  'aria-label'?: string;
  'aria-describedby'?: string;
  className?: string;
}

/**
 * A select for long lists: type to filter, arrows to move, Enter to choose.
 * Focus stays in the field the whole time.
 */
export function Combobox({
  options,
  value,
  onChange,
  placeholder,
  emptyText = 'No matches',
  clearable = false,
  disabled = false,
  invalid,
  size = 'md',
  className,
  ...rest
}: ComboboxProps) {
  const [open, setOpen] = useState(false);
  /** Null while the field shows the selection; a string once the user types. */
  const [query, setQuery] = useState<string | null>(null);
  const [activeIndex, setActiveIndex] = useState(0);
  const controlRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const fieldInvalid = useFieldInvalid();
  const control = useFieldControl(rest);
  const isInvalid = invalid ?? fieldInvalid;

  const selected = options.find((option) => option.value === value) ?? null;
  const results = useMemo(
    () =>
      fuzzyFilter(options, query ?? '', {
        text: (option) => option.label,
        keywords: (option) => (option.description ? [option.description] : undefined),
      }),
    [options, query],
  );
  const active = results[Math.min(activeIndex, results.length - 1)];
  const optionId = (option: ComboboxOption) => `${listId}-${option.value}`;

  const close = () => {
    setOpen(false);
    setQuery(null);
  };
  const { style } = useFloating(open, controlRef, listRef, { matchWidth: true, offset: 4 });
  useLayer(open, listRef, { onEscape: close, onPressOutside: close, trigger: controlRef });

  useEffect(() => {
    if (open) listRef.current?.querySelector('[data-active]')?.scrollIntoView({ block: 'nearest' });
  }, [open, active]);

  const show = () => {
    if (disabled || open) return;
    setOpen(true);
    // Start on the current selection so arrows continue from it.
    setActiveIndex(Math.max(0, options.findIndex((option) => option.value === value)));
  };

  const choose = (option: ComboboxOption) => {
    if (option.disabled) return;
    onChange(option.value);
    close();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    const move = (delta: number) => {
      if (!open) {
        show();
        return;
      }
      if (results.length === 0) return;
      const current = Math.min(activeIndex, results.length - 1);
      setActiveIndex((current + delta + results.length) % results.length);
    };
    if (event.key === 'ArrowDown') move(1);
    else if (event.key === 'ArrowUp') move(-1);
    else if (event.key === 'Enter' && open && !event.nativeEvent.isComposing) {
      if (active) choose(active.item);
    } else if (event.key === 'Tab') {
      close();
      return;
    } else return;
    event.preventDefault();
  };

  return (
    <>
      <div
        ref={controlRef}
        className={cx(styles.combobox, className)}
        data-control=""
        data-size={size}
        data-invalid={isInvalid || undefined}
        data-disabled={disabled || undefined}
      >
        <input
          {...control}
          ref={inputRef}
          className={styles.input}
          data-control-native=""
          role="combobox"
          aria-expanded={open}
          aria-controls={open ? listId : undefined}
          aria-activedescendant={open && active ? optionId(active.item) : undefined}
          aria-autocomplete="list"
          aria-invalid={isInvalid || undefined}
          autoComplete="off"
          spellCheck={false}
          disabled={disabled}
          placeholder={placeholder}
          value={query ?? selected?.label ?? ''}
          onChange={(event) => {
            setQuery(event.target.value);
            setActiveIndex(0);
            setOpen(true);
          }}
          onPointerDown={show}
          onKeyDown={onKeyDown}
          onBlur={close}
        />
        {clearable && selected && !disabled && (
          <button
            type="button"
            className={styles.clear}
            aria-label="Clear selection"
            onClick={() => {
              onChange(null);
              close();
              inputRef.current?.focus();
            }}
          >
            <Icon icon={X} size={12} />
          </button>
        )}
        <Icon icon={ChevronsUpDown} size={14} className={styles.chevron} />
      </div>

      {open && (
        <Portal>
          <div
            ref={listRef}
            id={listId}
            role="listbox"
            aria-label={rest['aria-label'] ?? 'Options'}
            className={styles.list}
            style={style}
            // Keep focus in the field while the list is pressed.
            onPointerDown={(event) => event.preventDefault()}
          >
            {results.length === 0 && <div className={styles.empty}>{emptyText}</div>}
            {results.map(({ item, indices }, index) => (
              <div
                key={item.value}
                id={optionId(item)}
                role="option"
                aria-selected={item.value === value}
                aria-disabled={item.disabled || undefined}
                data-active={item === active?.item || undefined}
                className={styles.option}
                onClick={() => choose(item)}
                onPointerMove={() => {
                  if (index !== activeIndex) setActiveIndex(index);
                }}
              >
                <span className={styles.check}>{item.value === value && <Icon icon={Check} size={14} />}</span>
                <span className={styles.optionLabel}>
                  <Highlight text={item.label} indices={indices} />
                </span>
                {item.description && <span className={styles.optionDescription}>{item.description}</span>}
              </div>
            ))}
          </div>
        </Portal>
      )}
    </>
  );
}
