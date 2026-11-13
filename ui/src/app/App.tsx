import { QueryClientProvider, type QueryClient } from '@tanstack/react-query';
import { RouterProvider } from 'react-router/dom';
import type { AppRouter } from './routes';

export interface AppProps {
  router: AppRouter;
  queryClient: QueryClient;
}

/** The application: server state and routing around the shell. */
export function App({ router, queryClient }: AppProps) {
  return (
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}
