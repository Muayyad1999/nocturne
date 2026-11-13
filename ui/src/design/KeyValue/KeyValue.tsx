import { Fragment, type ReactNode } from 'react';
import { cx } from '@/lib/cx';
import { EMPTY } from '@/lib/format';
import styles from './KeyValue.module.css';

export interface KeyValueItem {
  label: string;
  /** The value. Null or undefined renders as a dash: a missing fact is shown as missing. */
  value: ReactNode;
  /** Monospace, for anything a device said: addresses, serial numbers, versions. */
  mono?: boolean;
}

export interface KeyValueProps {
  items: readonly KeyValueItem[];
  /** `rows`: label and value side by side. `stacked`: label above value, for narrow panels. */
  layout?: 'rows' | 'stacked';
  className?: string;
}

/** A list of facts about one thing: a device's properties, a job's parameters. */
export function KeyValue({ items, layout = 'rows', className }: KeyValueProps) {
  return (
    <dl className={cx(styles.list, className)} data-layout={layout}>
      {items.map((item) => (
        <Fragment key={item.label}>
          <dt className={styles.label}>{item.label}</dt>
          <dd className={cx(styles.value, item.mono && 'mono')}>{item.value ?? EMPTY}</dd>
        </Fragment>
      ))}
    </dl>
  );
}
