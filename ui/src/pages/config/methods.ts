/**
 * Which RPC methods this build answers.
 *
 * Nocturne is assembled from feature crates, so a method a screen would like
 * to use may simply not be there. This screen needs `config.*`; the parts
 * that reach out of the feature — handing drift to the change pipeline — ask
 * before they offer themselves, rather than offering a button that fails.
 *
 * (`pages/devices` and `pages/changes` each keep their own copy of this for
 * the same reason; it belongs in `api/`.)
 */

import { useRpc } from '@/api/hooks';

/** The method names this build answers; `undefined` while the answer is on its way. */
export function useMethods(): string[] | undefined {
  return useRpc<string[]>('app.methods').data;
}
