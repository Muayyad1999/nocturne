/**
 * React hooks over the API client. Screens read with `useRpc`, write with
 * `useRpcMutation` and react to the backend with `useEvent`.
 */

import {
  hashKey,
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
  type QueryKey,
  type UseMutationResult,
  type UseQueryResult,
} from '@tanstack/react-query';
import { useEffect, useEffectEvent } from 'react';
import { toast } from '@/design/Toast';
import { rpc, subscribe, type AppEvent } from './client';
import type { RpcError } from './errors';

/** The cache key of an RPC query: `[method, params]`. */
export function rpcQueryKey(method: string, params?: unknown): QueryKey {
  return [method, params];
}

/**
 * Events often arrive in bursts (a switch reboots and forty interfaces change
 * state). Invalidations inside this window collapse into one refetch per query.
 */
export const INVALIDATION_WINDOW_MS = 50;

const pendingInvalidations = new WeakMap<QueryClient, Map<string, QueryKey>>();

function scheduleInvalidation(queryClient: QueryClient, queryKey: QueryKey): void {
  let pending = pendingInvalidations.get(queryClient);
  if (!pending) {
    pending = new Map();
    pendingInvalidations.set(queryClient, pending);
  }
  const queue = pending;
  if (queue.size === 0) {
    setTimeout(() => {
      const keys = [...queue.values()];
      queue.clear();
      for (const key of keys) void queryClient.invalidateQueries({ queryKey: key, exact: true });
    }, INVALIDATION_WINDOW_MS);
  }
  queue.set(hashKey(queryKey), queryKey);
}

/** Marks every cached query whose method starts with one of `prefixes` as stale and refetches the active ones. */
export function invalidateMethods(queryClient: QueryClient, prefixes: readonly string[]): Promise<void> {
  return queryClient.invalidateQueries({
    predicate: ({ queryKey }) => {
      const method = queryKey[0];
      return typeof method === 'string' && prefixes.some((prefix) => method.startsWith(prefix));
    },
  });
}

export interface UseRpcOptions {
  /** Set to false to hold the query back, e.g. until an id is known. */
  enabled?: boolean;
  /** Poll every this many milliseconds. Prefer `invalidateOn` when the backend emits events. */
  refetchInterval?: number;
  /** Event topics that refetch this query. A topic may end in `*` (`['devices.*']`). */
  invalidateOn?: string[];
}

/**
 * Reads from the backend. The result is cached under `[method, params]`, shared
 * by every component that asks for the same thing, and refetched when one of
 * the `invalidateOn` topics fires.
 */
export function useRpc<T>(
  method: string,
  params?: unknown,
  { enabled, refetchInterval, invalidateOn }: UseRpcOptions = {},
): UseQueryResult<T, RpcError> {
  const queryClient = useQueryClient();
  const queryKey = rpcQueryKey(method, params);
  const keyHash = hashKey(queryKey);
  const topics = invalidateOn?.join('\n') ?? '';

  const invalidate = useEffectEvent(() => scheduleInvalidation(queryClient, queryKey));

  useEffect(() => {
    if (!topics) return;
    const unsubscribes = topics.split('\n').map((topic) => subscribe(topic, () => invalidate()));
    return () => {
      for (const unsubscribe of unsubscribes) unsubscribe();
    };
  }, [topics, keyHash]);

  return useQuery<T, RpcError>({
    queryKey,
    queryFn: () => rpc<T>(method, params),
    enabled,
    refetchInterval,
  });
}

export interface UseRpcMutationOptions {
  /** Method prefixes whose cached queries are refetched after success (`['devices.']`). */
  invalidates?: string[];
  /** Shown as a success toast when the call succeeds. */
  successToast?: string;
  /**
   * A failed call always tells the user: by default the error's message is
   * shown as a toast. Pass a title to put in front of it, or `false` when the
   * screen shows `mutation.error` itself.
   */
  errorToast?: string | false;
}

/**
 * Writes to the backend. Call `mutation.mutate(params)`; while it runs
 * `mutation.isPending` is true, which is what a Button's `loading` wants.
 */
export function useRpcMutation<T, P = unknown>(
  method: string,
  { invalidates, successToast, errorToast }: UseRpcMutationOptions = {},
): UseMutationResult<T, RpcError, P> {
  const queryClient = useQueryClient();
  return useMutation<T, RpcError, P>({
    mutationFn: (params) => rpc<T>(method, params),
    onSuccess: async () => {
      if (successToast) toast.success(successToast);
      // Awaited so the mutation stays pending until the lists show the change.
      if (invalidates?.length) await invalidateMethods(queryClient, invalidates);
    },
    onError: (error) => {
      if (errorToast === false) return;
      if (errorToast) toast.error(errorToast, { description: error.message });
      else toast.error(error.message);
    },
  });
}

/** Runs `handler` for every backend event on `topic` while the component is mounted. */
export function useEvent<P>(topic: string, handler: (payload: P, event: AppEvent<P>) => void): void {
  const onEvent = useEffectEvent(handler);
  useEffect(
    () => subscribe<P>(topic, (event) => onEvent(event.payload, event)),
    [topic],
  );
}

/**
 * Is `method` registered on this backend?
 *
 * `undefined` while `app.methods` has not answered yet, and that third value
 * is the point: a build without a feature crate and a build whose method list
 * is still in flight look identical if they are both `false`, so a control
 * that should be there flickers, or a screen says "that feature is not
 * installed" about a feature that is. Decide what to do with `undefined` at
 * the call site — `=== true` to wait for proof, `!== false` to assume it is
 * there until told otherwise.
 *
 * The list is fetched once and cached, so asking per component is free.
 */
export function useHasMethod(method: string): boolean | undefined {
  const methods = useRpc<string[]>('app.methods');
  return methods.data === undefined ? undefined : methods.data.includes(method);
}
