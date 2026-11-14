/**
 * One renderer for every answer on this screen.
 *
 * `facts.ts` turns a result into {@link Section}s and this draws them, so a
 * subnet, a certificate chain and a port scan all read the same way: the
 * facts that matter first, in a two-column list with copy buttons on anything
 * worth pasting, then the tables, then the raw text a device or a server
 * actually sent.
 *
 * Numbers are monospace and right-aligned where they are compared down a
 * column, which is the only way a column of round-trip times can be read at
 * a glance.
 */

import { Check, Copy } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { CodeBlock } from '@/design/CodeBlock';
import { Table, type TableColumn } from '@/design/Table';
import { toast } from '@/design/Toast';
import { copyText } from '@/lib/clipboard';
import { cx } from '@/lib/cx';
import { EMPTY } from '@/lib/format';
import type { Fact, Row, Section } from './facts';
import styles from './Result.module.css';

const COPIED_MS = 1400;

/** Copies one value, and says so where the value is rather than in a toast. */
export function CopyButton({ value, label }: { value: string; label: string }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = async () => {
    if (!(await copyText(value))) {
      toast.error('Could not copy to the clipboard', {
        description: 'Select the text and copy it with the keyboard instead.',
      });
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), COPIED_MS);
  };

  const Glyph = copied ? Check : Copy;
  return (
    <button
      type="button"
      className={styles.copy}
      aria-label={copied ? `${label} copied` : `Copy ${label}`}
      onClick={() => void copy()}
    >
      <Glyph size={13} aria-hidden="true" />
    </button>
  );
}

export function Facts({ facts, title }: { facts: readonly Fact[]; title?: string }) {
  if (facts.length === 0) return null;
  return (
    <section className={styles.block}>
      {title && <h4 className={styles.blockTitle}>{title}</h4>}
      <dl className={styles.facts}>
        {facts.map((item) => (
          <div key={item.label} className={styles.fact}>
            <dt className={styles.label}>{item.label}</dt>
            <dd className={cx(styles.value, item.mono && styles.mono)} data-tone={item.tone}>
              <span className={styles.text}>{item.value}</span>
              {item.copy && <CopyButton value={item.value} label={item.label} />}
            </dd>
          </div>
        ))}
      </dl>
    </section>
  );
}

type KeyedRow = Row & { __key: string };

function cell(value: Row[string] | undefined) {
  if (value === null || value === undefined || value === '') return EMPTY;
  if (typeof value === 'boolean') return value ? 'yes' : 'no';
  return String(value);
}

export function SectionView({ section }: { section: Section }) {
  if (section.kind === 'facts') return <Facts facts={section.facts} title={section.title} />;

  if (section.kind === 'note') {
    return (
      <p className={styles.note} data-tone={section.tone}>
        {section.text}
      </p>
    );
  }

  if (section.kind === 'code') {
    return (
      <section className={styles.block}>
        <h4 className={styles.blockTitle}>{section.title}</h4>
        <CodeBlock code={section.text} wrap={section.wrap} maxHeight={320} />
      </section>
    );
  }

  const columns: TableColumn<KeyedRow>[] = section.columns.map((column) => ({
    id: column.id,
    header: column.header,
    align: column.align,
    mono: column.mono,
    width: column.width,
    cell: (row) => cell(row[column.id]),
    sortValue: (row) => {
      const value = row[column.id];
      return typeof value === 'boolean' ? String(value) : (value ?? null);
    },
  }));
  const rows: KeyedRow[] = section.rows.map((row, index) => ({ ...row, __key: String(index) }));

  return (
    <section className={styles.block}>
      {section.title && <h4 className={styles.blockTitle}>{section.title}</h4>}
      <Table
        columns={columns}
        rows={rows}
        rowKey={(row) => row.__key}
        aria-label={section.title ?? 'Result'}
        density="compact"
        maxHeight={420}
        empty={<p className={styles.empty}>{section.empty}</p>}
      />
      {section.note && <p className={styles.tableNote}>{section.note}</p>}
    </section>
  );
}

export function Sections({ sections }: { sections: readonly Section[] }) {
  return (
    <div className={styles.sections}>
      {sections.map((section, index) => (
        <SectionView key={index} section={section} />
      ))}
    </div>
  );
}
