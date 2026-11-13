import { useRouteError } from 'react-router';
import { ErrorState } from '@/design/ErrorState';
import styles from './AppError.module.css';

/** The last line of defence: the shell itself failed to render. */
export function AppError() {
  const error = useRouteError();
  return (
    <div className={styles.fatal}>
      <ErrorState
        title="Nocturne could not be displayed"
        error={error instanceof Error ? error : 'The interface ran into a problem it could not recover from.'}
        onRetry={() => window.location.reload()}
        retryLabel="Reload Nocturne"
      />
    </div>
  );
}
