import { RotateCw, TriangleExclamationPoint } from 'lucide-react';
import type { ComponentProps, ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { Button } from '../Button';
import { Icon } from '../Icon';
import styles from './ErrorState.module.css';

export interface ErrorStateProps extends Omit<ComponentProps<'div'>, 'title'> {
  /** What failed, in the user's terms: "Devices could not be loaded". */
  title: string;
  /**
   * What happened. Pass the caught error (an `RpcError` shows its message and
   * code) or a sentence of your own.
   */
  error?: unknown;
  /** Runs the failed operation again. Omit it only when retrying cannot help. */
  onRetry?: () => void;
  retryLabel?: string;
  /** Further actions next to Retry. */
  actions?: ReactNode;
  /** `page` centres in a whole page; `compact` fits a card, a panel or a table body. */
  size?: 'page' | 'compact';
}

function describe(error: unknown): { message?: string; code?: string } {
  if (typeof error === 'string') return { message: error };
  if (error instanceof Error) {
    const code = (error as { code?: unknown }).code;
    return { message: error.message, code: typeof code === 'string' ? code : undefined };
  }
  return {};
}

/** Says what happened and what to do about it. */
export function ErrorState({
  title,
  error,
  onRetry,
  retryLabel = 'Try again',
  actions,
  size = 'page',
  className,
  ...rest
}: ErrorStateProps) {
  const { message, code } = describe(error);
  const Heading = size === 'page' ? 'h2' : 'h3';
  return (
    <div {...rest} className={cx(styles.error, className)} data-size={size} role="alert">
      <span className={styles.icon}>
        <Icon icon={TriangleExclamationPoint} size={size === 'page' ? 20 : 16} />
      </span>
      <Heading className={styles.title}>{title}</Heading>
      {message && <p className={styles.message}>{message}</p>}
      {code && <code className={styles.code}>{code}</code>}
      {(onRetry || actions) && (
        <div className={styles.actions}>
          {onRetry && (
            <Button icon={RotateCw} onClick={onRetry}>
              {retryLabel}
            </Button>
          )}
          {actions}
        </div>
      )}
    </div>
  );
}
