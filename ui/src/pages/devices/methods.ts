/**
 * Which RPC methods this build answers.
 *
 * Nocturne is assembled from feature crates, so a method a screen would like
 * to offer may simply not be there. Asking `app.methods` once (the answer is
 * cached by the query client) lets a button be left out rather than shown and
 * then failing.
 */

import { useRpc } from '@/api/hooks';

export function useHasMethod(method: string): boolean {
  const methods = useRpc<string[]>('app.methods');
  return methods.data?.includes(method) ?? false;
}
