/**
 * Small, safe wrapper around localStorage for remembered UI state (layout,
 * table sort, splitter sizes). Storage can be unavailable or hold stale shapes,
 * so every read is validated and every failure degrades to "not remembered".
 */

const PREFIX = 'nocturne.';

/** Namespaced storage key, e.g. `storageKey('table.devices')` → `nocturne.table.devices`. */
export function storageKey(name: string): string {
  return `${PREFIX}${name}`;
}

function storage(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

/** Reads and validates a stored JSON value; `undefined` when missing or invalid. */
export function readStored<T>(key: string, isValid: (value: unknown) => value is T): T | undefined {
  try {
    const raw = storage()?.getItem(key);
    if (raw == null) return undefined;
    const value: unknown = JSON.parse(raw);
    return isValid(value) ? value : undefined;
  } catch {
    return undefined;
  }
}

export function writeStored(key: string, value: unknown): void {
  try {
    storage()?.setItem(key, JSON.stringify(value));
  } catch {
    // Quota exceeded or storage disabled: the state is simply not remembered.
  }
}

export function removeStored(key: string): void {
  try {
    storage()?.removeItem(key);
  } catch {
    // Nothing to clean up when storage is unavailable.
  }
}
