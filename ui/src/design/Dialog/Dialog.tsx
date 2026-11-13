import { X } from 'lucide-react';
import { useId, useRef, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { IconButton } from '../Button';
import { useFocusScope } from '../overlay/focus';
import { useLayer } from '../overlay/layers';
import { Portal } from '../overlay/Portal';
import styles from './Dialog.module.css';

export interface DialogProps {
  open: boolean;
  /** Called by Escape, the close button and a press outside. */
  onClose: () => void;
  title: string;
  /** A sentence under the title: what this dialog is for, or what will happen. */
  description?: ReactNode;
  children?: ReactNode;
  /** Buttons, right-aligned: the secondary action first, the main one last. */
  footer?: ReactNode;
  size?: 'sm' | 'md' | 'lg';
  /**
   * When false, Escape and a press outside do nothing and there is no close
   * button: for a step that must be answered, or while work is in flight.
   */
  dismissible?: boolean;
  className?: string;
}

/**
 * A modal dialog. Focus moves in when it opens (to the element marked
 * `data-autofocus`, else the first control), Tab stays inside, and focus
 * returns to where it was on close. Everything behind it is inert.
 */
export function Dialog({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  size = 'md',
  dismissible = true,
  className,
}: DialogProps) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const id = useId();
  const close = dismissible ? onClose : undefined;

  useLayer(open, dialogRef, { modal: true, onEscape: close, onPressOutside: close });
  useFocusScope(open, dialogRef);

  if (!open) return null;
  return (
    <Portal>
      <div className={styles.scrim}>
        <div
          ref={dialogRef}
          role="dialog"
          aria-modal="true"
          aria-labelledby={`${id}-title`}
          aria-describedby={description ? `${id}-description` : undefined}
          tabIndex={-1}
          className={cx(styles.dialog, className)}
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
            {dismissible && <IconButton icon={X} aria-label="Close" tooltip={false} onClick={onClose} />}
          </header>
          {children && <div className={styles.body}>{children}</div>}
          {footer && <footer className={styles.footer}>{footer}</footer>}
        </div>
      </div>
    </Portal>
  );
}
