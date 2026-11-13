import { Search, X } from 'lucide-react';
import type { ComponentProps, KeyboardEvent } from 'react';
import { Icon } from '../Icon';
import { Input } from '../Input';
import styles from './SearchInput.module.css';

export interface SearchInputProps
  extends Omit<ComponentProps<'input'>, 'type' | 'value' | 'onChange' | 'size' | 'aria-label'> {
  value: string;
  /** Called with the new text on every change, and with `''` when cleared. */
  onChange: (value: string) => void;
  /** What is being searched: "Search devices". */
  'aria-label': string;
  size?: 'sm' | 'md';
}

/**
 * A filter field for a list or table. Escape or the clear button empties it.
 * `ref` reaches the `<input>`.
 */
export function SearchInput({ value, onChange, placeholder, onKeyDown, ...rest }: SearchInputProps) {
  const handleKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    onKeyDown?.(event);
    if (event.key === 'Escape' && value !== '' && !event.defaultPrevented) {
      // Handled here: an enclosing dialog must not close on this Escape.
      event.preventDefault();
      onChange('');
    }
  };

  return (
    <Input
      {...rest}
      type="search"
      value={value}
      placeholder={placeholder ?? rest['aria-label']}
      autoComplete="off"
      spellCheck={false}
      onChange={(event) => onChange(event.target.value)}
      onKeyDown={handleKeyDown}
      leading={<Icon icon={Search} size={14} />}
      trailing={
        value !== '' && (
          <button type="button" className={styles.clear} aria-label="Clear search" onClick={() => onChange('')}>
            <Icon icon={X} size={12} />
          </button>
        )
      }
    />
  );
}
