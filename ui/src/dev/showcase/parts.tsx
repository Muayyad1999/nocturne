import type { ReactNode } from 'react';
import { cx } from '@/lib/cx';
import styles from './Showcase.module.css';

/** One component (or one idea) in the showcase: a heading, a sentence, and examples. */
export function Section({ title, description, children }: { title: string; description?: ReactNode; children: ReactNode }) {
  return (
    <section className={styles.section}>
      <header className={styles.sectionHeader}>
        <h2 className={styles.sectionTitle}>{title}</h2>
        {description && <p className={styles.sectionDescription}>{description}</p>}
      </header>
      <div className={styles.examples}>{children}</div>
    </section>
  );
}

export interface ExampleProps {
  /** The state or variant being shown. */
  title: string;
  children: ReactNode;
  /** How the examples inside are laid out. */
  layout?: 'row' | 'column' | 'fill';
  /** How much of the row the example takes in the section's grid. */
  span?: 1 | 2 | 3;
  className?: string;
}

/** A labelled frame around one state of a component. */
export function Example({ title, children, layout = 'row', span = 1, className }: ExampleProps) {
  return (
    <div className={cx(styles.example, className)} data-span={span}>
      <div className={styles.exampleTitle}>{title}</div>
      <div className={styles.exampleBody} data-layout={layout}>
        {children}
      </div>
    </div>
  );
}
