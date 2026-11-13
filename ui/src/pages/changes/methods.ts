/**
 * Which RPC methods this build answers.
 *
 * Nocturne is assembled from feature crates, so a method a screen would like
 * to offer may simply not be there: `templates.list` is one the change
 * pipeline is happy without. Asking `app.methods` once (the query client
 * caches the answer) lets a source be left out rather than offered and then
 * failing with `unknown_method`.
 */

import { useRpc } from '@/api/hooks';

export function useHasMethod(method: string): boolean {
  const methods = useRpc<string[]>('app.methods');
  return methods.data?.includes(method) ?? false;
}
