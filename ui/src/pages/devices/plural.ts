/**
 * Counted things, written the way a person would: `1 device`, `3 devices`.
 * An interface that writes "1 device(s)" has stopped paying attention.
 *
 * Settings has its own copy; this belongs in `lib/format.ts` once a third
 * screen wants it.
 */

import { formatCount } from '@/lib/format';

export function plural(count: number, one: string, many = `${one}s`): string {
  return `${formatCount(count)} ${count === 1 ? one : many}`;
}
