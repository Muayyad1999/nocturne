import { ChevronRight } from 'lucide-react';
import { useId, useState, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { readStored, storageKey, writeStored } from '@/lib/storage';
import { Icon } from '../Icon';
import styles from './Panel.module.css';

export interface PanelProps {
  title: string;
  children: ReactNode;
  /** Shown at the right of the header: a count, a status, buttons. */
  actions?: ReactNode;
  /** Lets the user fold the panel away. */
  collapsible?: boolean;
  defaultCollapsed?: boolean;
  /** Remembers whether it is collapsed under this id (`localStorage`). */
  storageId?: string;
  /** Inner spacing of the body. `none` lets a table run edge to edge. */
  padding?: 'none' | 'md';
  className?: string;
}

function isBoolean(value: unknown): value is boolean {
  return typeof value === 'boolean';
}

/** A titled section of a screen, optionally collapsible. */
export function Panel({
  title,
  children,
  actions,
  collapsible = false,
  defaultCollapsed = false,
  storageId,
  padding = 'md',
  className,
}: PanelProps) {
  const key = storageId ? storageKey(`panel.${storageId}`) : null;
  const [collapsed, setCollapsed] = useState(
    () => collapsible && ((key ? readStored(key, isBoolean) : undefined) ?? defaultCollapsed),
  );
  const id = useId();
  const titleId = `${id}-title`;
  const bodyId = `${id}-body`;

  const toggle = () => {
    setCollapsed(!collapsed);
    if (key) writeStored(key, !collapsed);
  };

  return (
    <section className={cx(styles.panel, className)} aria-labelledby={titleId} data-collapsed={collapsed || undefined}>
      <header className={styles.header}>
        <h3 id={titleId} className={styles.title}>
          {collapsible ? (
            <button
              type="button"
              className={styles.toggle}
              aria-expanded={!collapsed}
              aria-controls={bodyId}
              onClick={toggle}
            >
              <Icon icon={ChevronRight} size={14} className={styles.chevron} />
              {title}
            </button>
          ) : (
            title
          )}
        </h3>
        {actions && <div className={styles.actions}>{actions}</div>}
      </header>
      <div id={bodyId} className={styles.body} data-padding={padding} hidden={collapsed}>
        {children}
      </div>
    </section>
  );
}
