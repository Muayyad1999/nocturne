import { useState } from 'react';

/**
 * State that the parent may control. With `value` given the parent owns it;
 * without, the component keeps its own, starting from `defaultValue`.
 * `onChange` is called either way.
 */
export function useControllableState<T>(
  value: T | undefined,
  defaultValue: T,
  onChange?: (value: T) => void,
): [T, (next: T) => void] {
  const [internal, setInternal] = useState(defaultValue);
  const controlled = value !== undefined;
  const set = (next: T) => {
    if (!controlled) setInternal(next);
    onChange?.(next);
  };
  return [controlled ? value : internal, set];
}
