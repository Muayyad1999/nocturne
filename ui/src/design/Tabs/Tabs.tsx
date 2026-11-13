import type { ComponentProps, KeyboardEvent, ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { Icon, type LucideIcon } from '../Icon';
import styles from './Tabs.module.css';

export interface TabItem {
  id: string;
  label: string;
  icon?: LucideIcon;
  /** Shown after the label, typically a Badge with a count. */
  badge?: ReactNode;
  disabled?: boolean;
}

export interface TabsProps {
  /** Links the tabs to their TabPanel. Unique on the page. */
  id: string;
  tabs: readonly TabItem[];
  /** The selected tab's id. */
  value: string;
  onChange: (id: string) => void;
  /** Names the set of tabs for assistive technology. */
  'aria-label': string;
  className?: string;
}

const tabId = (tabsId: string, id: string) => `${tabsId}-tab-${id}`;
const panelId = (tabsId: string, id: string) => `${tabsId}-panel-${id}`;

/**
 * A row of tabs. Arrow keys move between tabs and select as they go; Home and
 * End jump to the ends. For tabs that are part of a route, keep `value` in the
 * URL and navigate in `onChange`.
 */
export function Tabs({ id, tabs, value, onChange, className, ...rest }: TabsProps) {
  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const enabled = tabs.filter((tab) => !tab.disabled);
    const index = enabled.findIndex((tab) => tab.id === value);
    let next: TabItem | undefined;
    if (event.key === 'ArrowRight') next = enabled[(index + 1) % enabled.length];
    else if (event.key === 'ArrowLeft') next = enabled[(index - 1 + enabled.length) % enabled.length];
    else if (event.key === 'Home') next = enabled[0];
    else if (event.key === 'End') next = enabled[enabled.length - 1];
    if (!next) return;
    event.preventDefault();
    onChange(next.id);
    document.getElementById(tabId(id, next.id))?.focus();
  };

  return (
    <div
      role="tablist"
      aria-label={rest['aria-label']}
      className={cx(styles.tabs, className)}
      onKeyDown={onKeyDown}
    >
      {tabs.map((tab) => {
        const selected = tab.id === value;
        return (
          <button
            key={tab.id}
            type="button"
            role="tab"
            id={tabId(id, tab.id)}
            aria-selected={selected}
            aria-controls={selected ? panelId(id, tab.id) : undefined}
            tabIndex={selected ? 0 : -1}
            disabled={tab.disabled}
            className={styles.tab}
            onClick={() => onChange(tab.id)}
          >
            {tab.icon && <Icon icon={tab.icon} size={14} />}
            {tab.label}
            {tab.badge}
          </button>
        );
      })}
    </div>
  );
}

export interface TabPanelProps extends ComponentProps<'div'> {
  /** The `id` of the Tabs this panel belongs to. */
  tabs: string;
  /** The id of the tab whose content this is. */
  value: string;
}

/** The content of the selected tab, labelled by that tab. */
export function TabPanel({ tabs, value, className, ...rest }: TabPanelProps) {
  return (
    <div
      {...rest}
      role="tabpanel"
      id={panelId(tabs, value)}
      aria-labelledby={tabId(tabs, value)}
      className={cx(styles.panel, className)}
    />
  );
}
