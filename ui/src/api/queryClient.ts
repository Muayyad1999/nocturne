import { QueryClient } from '@tanstack/react-query';
import { subscribe } from './client';
import { useConnection } from './connection';

/**
 * The query client for RPC data.
 *
 * - `networkMode: 'always'`: the engine is local (or on the LAN); whether the
 *   machine has internet access says nothing about whether it can be reached.
 * - No automatic retries: a failed call is shown with a Retry button rather
 *   than silently repeated against a device that may be struggling.
 * - Data stays fresh for a few seconds; backend events (`invalidateOn`) are
 *   what normally trigger a refetch.
 */
export function createQueryClient(): QueryClient {
  return new QueryClient({
    defaultOptions: {
      queries: {
        networkMode: 'always',
        retry: false,
        staleTime: 10_000,
      },
      mutations: {
        networkMode: 'always',
        retry: false,
      },
    },
  });
}

/**
 * Refetches everything whenever events may have been missed: the server says
 * it dropped some (`events.lagged`), or the event stream came back after a
 * break. Returns a function that undoes the binding.
 */
export function bindQueryClient(queryClient: QueryClient): () => void {
  const refetchAll = () => {
    void queryClient.invalidateQueries();
  };

  const stopLagged = subscribe('events.lagged', refetchAll);

  let hasBeenOnline = false;
  const stopConnection = useConnection.subscribe((connection, previous) => {
    if (connection.state !== 'online' || previous.state === 'online') return;
    if (hasBeenOnline) refetchAll();
    hasBeenOnline = true;
  });

  return () => {
    stopLagged();
    stopConnection();
  };
}
