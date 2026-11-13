import { X } from 'lucide-react';
import { useId, useRef, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { IconButton } from '../Button';
import { useFocusScope } from '../overlay/focus';
import { useLayer } from '../overlay/layers';
import { Portal } from '../overlay/Portal';
import styles from './Drawer.module.css';

export interface DrawerProps {
  open: boolean;
  /** Called by Escape, the close button and a press outside. */
  onClose: () => void;
  title: string;
  /** A line under the title: the object the drawer is about. */
  description?: ReactNode;
  children?: ReactNode;
  /** Buttons pinned to the bottom, right-aligned. */
  footer?: ReactNode;
  size?: 'md' | 'lg';
  className?: string;
}

/**
 * A panel that slides in from the right for looking at or editing one thing
 * without leaving the list behind it. Modal, like Dialog: focus moves in,
 * stays in, and returns on close.
 */
export function Drawer({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  size = 'md',
  className,
}: DrawerProps) {
  const drawerRef = useRef<HTMLDivElement>(null);
  const id = useId();

  useLayer(open, drawerRef, { modal: true, onEscape: onClose, onPressOutside: onClose });
  useFocusScope(open, drawerRef);

  if (!open) return null;
  return (
    <Portal>
      <div className={styles.scrim}>
        <div
          ref={drawerRef}
          role="dialog"
          aria-modal="true"
          aria-labelledby={`${id}-title`}
          aria-describedby={description ? `${id}-description` : undefined}
          tabIndex={-1}
          className={cx(styles.drawer, className)}
          data-size={size}
        >
          <header className={styles.header}>
            <div className={styles.heading}>
              <h2 id={`${id}-title`} className={styles.title}>
                {title}
              </h2>
              {description && (
                <p id={`${id}-description`} className={styles.description}>
                  {description}
                </p>
              )}
            </div>
            <IconButton icon={X} aria-label="Close" tooltip={false} onClick={onClose} />
          </header>
          <div className={styles.body}>{children}</div>
          {footer && <footer className={styles.footer}>{footer}</footer>}
        </div>
      </div>
    </Portal>
  );
}
