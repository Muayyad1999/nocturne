/**
 * The reference tables: ports, DSCP, protocol and EtherType numbers, ICMP
 * types, administrative distances, optics, syslog levels, encapsulations.
 *
 * The chooser is filled from `tools.reference.tables` rather than from a list
 * written here, so a table added to the engine appears with its own row count
 * and description. The rows come back as JSON with a `columns` list naming
 * their fields in the order a table should show them, which is what lets one
 * component render any of them without knowing which one it asked for.
 */

import { useEffect } from 'react';
import type { ReferenceTable } from '@/api/types/toolbox';
import { Field } from '@/design/Field';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Table, type TableColumn } from '@/design/Table';
import { formatCount } from '@/lib/format';
import { Refusal } from './Answer';
import { useDebounced, useReferenceTables, useToolResult, type Form } from './data';
import type { Row } from './facts';
import styles from './Tools.module.css';
import resultStyles from './Result.module.css';

type KeyedRow = Row & { __key: string };

/** Field names the engine uses, in words a column heading can use. */
const HEADINGS: Record<string, string> = {
  port: 'Port',
  protocol: 'Protocol',
  name: 'Name',
  description: 'Description',
  dscp: 'DSCP',
  binary: 'Binary',
  tos: 'ToS',
  precedence: 'Precedence',
  phb: 'PHB',
  typical_use: 'Typically for',
  number: 'Number',
  bytes: 'Bytes',
  level: 'Level',
  keyword: 'Keyword',
  severity: 'Severity',
  distance: 'Distance',
  source: 'Source',
  wavelength_nm: 'Wavelength (nm)',
  reach: 'Reach',
  overhead: 'Overhead',
};

const heading = (field: string): string =>
  HEADINGS[field] ?? field.replace(/_/g, ' ').replace(/^./, (first) => first.toUpperCase());

/**
 * Narrow columns get their width, so the description beside them takes the
 * rest. Without this the table shares the width evenly and a port number
 * floats in the middle of a 200-pixel column.
 */
const NARROW: Record<string, number> = {
  port: 90,
  number: 90,
  dscp: 80,
  tos: 80,
  precedence: 110,
  binary: 110,
  bytes: 90,
  level: 80,
  severity: 100,
  distance: 100,
  protocol: 110,
  phb: 90,
  keyword: 130,
  name: 160,
};

export function Reference({ form }: { form: Form }) {
  const tables = useReferenceTables(true);
  const chosen = form.values['table'] ?? '';
  const query = useDebounced(form.values['query'] ?? '');

  // The first table in the engine's own order, until one is chosen: a chooser
  // with nothing in it and an empty area beside it says less than a table.
  const first = tables.data?.[0]?.table;
  useEffect(() => {
    if (!chosen && first) form.set('table', first);
  }, [chosen, first]);

  const table = chosen || first || '';
  const answer = useToolResult<ReferenceTable>(
    'tools.reference',
    table ? { table, query: query.trim() || undefined, limit: 2000 } : null,
  );
  const info = tables.data?.find((entry) => entry.table === table);

  const columns: TableColumn<KeyedRow>[] = (answer.data?.columns ?? []).map((field) => ({
    id: field,
    header: heading(field),
    mono: field !== 'description' && field !== 'typical_use' && field !== 'name',
    width: NARROW[field],
    align: typeof answer.data?.rows[0]?.[field] === 'number' ? ('right' as const) : undefined,
    cell: (row) => {
      const value = row[field];
      return value === null || value === undefined || value === '' ? '—' : String(value);
    },
    sortValue: (row) => {
      const value = row[field];
      return typeof value === 'boolean' ? String(value) : (value ?? null);
    },
  }));
  const rows: KeyedRow[] = (answer.data?.rows ?? []).map((row, index) => ({
    ...row,
    __key: String(index),
  }));

  return (
    <>
      <div className={styles.formRow}>
        <div className={styles.form}>
          <div className={styles.formField} data-span={1}>
            <Field label="Table" hint={info?.description}>
              <Select
                value={table}
                options={(tables.data ?? []).map((entry) => ({
                  value: entry.table,
                  label: `${entry.title} (${formatCount(entry.rows)})`,
                }))}
                disabled={tables.data === undefined}
                onChange={(event) => form.set('table', event.currentTarget.value)}
              />
            </Field>
          </div>
          <div className={styles.formField} data-span={1}>
            <Field label="Search" hint="A number or a name. Ports and DSCP search cleverly.">
              <SearchInput
                value={form.values['query'] ?? ''}
                placeholder="179, bgp, voice…"
                aria-label="Search this reference table"
                onChange={(value) => form.set('query', value)}
              />
            </Field>
          </div>
        </div>
      </div>

      {tables.error ? (
        <Refusal error={tables.error} />
      ) : answer.error ? (
        <Refusal error={answer.error} />
      ) : (
        <div className={styles.answer}>
          <Table
            columns={columns}
            rows={rows}
            rowKey={(row) => row.__key}
            aria-label={answer.data?.title ?? 'Reference table'}
            density="compact"
            loading={answer.isPending}
            maxHeight={520}
            empty={
              <p className={resultStyles.empty}>
                {query.trim()
                  ? `Nothing in this table matches “${query.trim()}”.`
                  : 'This table is empty.'}
              </p>
            }
          />
          {answer.data && (
            <p className={resultStyles.tableNote}>
              {answer.data.truncated
                ? `${formatCount(answer.data.total)} rows match; the table holds the first ${answer.data.rows.length}.`
                : `${formatCount(answer.data.total)} ${answer.data.total === 1 ? 'row' : 'rows'}.`}
            </p>
          )}
        </div>
      )}
    </>
  );
}
