import { useLocation, useNavigate, useRouteError } from 'react-router';
import { Button } from '@/design/Button';
import { ErrorState } from '@/design/ErrorState';
import { Page } from './Page';

/**
 * The error boundary of a route. A screen that throws while rendering, or
 * whose code fails to load, is replaced by this; the rest of the shell keeps
 * working, and moving to another screen clears it.
 */
export function RouteError() {
  const error = useRouteError();
  const location = useLocation();
  const navigate = useNavigate();

  return (
    <Page>
      <ErrorState
        title="This screen ran into a problem"
        error={error instanceof Error ? error : 'The screen could not be displayed.'}
        // Navigating to the same place gives the router a fresh location, which resets the boundary.
        onRetry={() => void navigate(`${location.pathname}${location.search}`, { replace: true })}
        actions={
          <Button variant="ghost" onClick={() => window.location.reload()}>
            Reload Nocturne
          </Button>
        }
      />
    </Page>
  );
}
