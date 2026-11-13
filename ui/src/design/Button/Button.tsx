import type { ComponentProps, MouseEvent } from 'react';
import { cx } from '@/lib/cx';
import { Icon, type LucideIcon } from '../Icon';
import { Spinner } from '../Spinner';
import styles from './Button.module.css';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'ai';
export type ButtonSize = 'sm' | 'md';

export interface ButtonProps extends ComponentProps<'button'> {
  /**
   * `primary` is the one main action of a screen; `secondary` everything else;
   * `ghost` for toolbars and rows; `danger` for destructive actions; `ai` for
   * anything the assistant does or proposes.
   */
  variant?: ButtonVariant;
  size?: ButtonSize;
  /** Shows a spinner and ignores presses, keeping focus and width. */
  loading?: boolean;
  /** Icon before the label. */
  icon?: LucideIcon;
  /** Icon after the label, e.g. a chevron for a menu. */
  trailingIcon?: LucideIcon;
}

export function Button({
  variant = 'secondary',
  size = 'md',
  loading = false,
  icon,
  trailingIcon,
  type = 'button',
  className,
  children,
  onClick,
  ...rest
}: ButtonProps) {
  const iconSize = size === 'sm' ? 14 : 16;

  const handleClick = (event: MouseEvent<HTMLButtonElement>) => {
    if (loading) {
      event.preventDefault();
      return;
    }
    onClick?.(event);
  };

  return (
    <button
      {...rest}
      type={type}
      className={cx(styles.button, className)}
      data-variant={variant}
      data-size={size}
      data-loading={loading || undefined}
      aria-busy={loading || undefined}
      aria-disabled={loading || undefined}
      onClick={handleClick}
    >
      {loading && <Spinner className={styles.spinner} size={iconSize} label={null} />}
      <span className={styles.content}>
        {icon && <Icon icon={icon} size={iconSize} />}
        {children}
        {trailingIcon && <Icon icon={trailingIcon} size={iconSize} />}
      </span>
    </button>
  );
}
