/**
 * Who is writing this down.
 *
 * Nocturne has no user directory and the core's audit trail has nowhere to put
 * an actor (see `crates/nocturne-ops/NOTES.md`), so every incident method
 * takes an `author` as free text instead. Asking for it on every note would be
 * absurd at three in the morning, so this browser remembers what was typed
 * last and every write sends it.
 *
 * It is remembered, not authenticated: the field is editable wherever it is
 * used and the timeline shows exactly what was sent, so nothing here pretends
 * to be an identity.
 */

import { useState } from 'react';
import { readStored, storageKey, writeStored } from '@/lib/storage';

const KEY = storageKey('ops.author');

function isString(value: unknown): value is string {
  return typeof value === 'string';
}

export function storedAuthor(): string {
  return readStored(KEY, isString) ?? '';
}

/** The remembered name, and a setter that remembers the new one. */
export function useAuthor(): [string, (name: string) => void] {
  const [author, setAuthor] = useState(storedAuthor);
  return [
    author,
    (name: string) => {
      setAuthor(name);
      writeStored(KEY, name);
    },
  ];
}
