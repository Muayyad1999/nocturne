import type { ComponentProps, ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { Icon, type LucideIcon } from '../Icon';
import styles from './EmptyState.module.css';

export interface EmptyStateProps extends Omit<ComponentProps<'div'>, 'title'> {
  icon: LucideIcon;
  /** What this place is, or what is missing. */
  title: string;
  /** One sentence: what the screen is for, or how it gets filled. */
  description?: ReactNode;
  /** The one action that fills it, usually a primary Button. */
  action?: ReactNode;
  /** `page` centres in a whole page; `compact` fits a card, a panel or a table body. */
  size?: 'page' | 'compact';
}

/**
 * Never a dead end: says what the screen is for and offers the one action
 * that fills it.
 */
export function EmptyState({
  icon,
  title,
  description,
  action,
  size = 'page',
  className,
  ...rest
}: EmptyStateProps) {
  const Heading = size === 'page' ? 'h2' : 'h3';
  return (
    <div {...rest} className={cx(styles.empty, className)} data-size={size}>
      <span className={styles.icon}>
        <Icon icon={icon} size={size === 'page' ? 20 : 16} />
      </span>
      <Heading className={styles.title}>{title}</Heading>
      {description && <p className={styles.description}>{description}</p>}
      {action && <div className={styles.action}>{action}</div>}
    </div>
  );
}
