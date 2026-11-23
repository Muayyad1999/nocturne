/**
 * The reference tables: ports, DSCP, protocol and EtherType numbers, ICMP
 * types, administrative distances, optics, syslog levels, encapsulations.
 *
 * The chooser is filled from `tools.reference.tables` rather than from a list
 * written here, so a table added to the engine appears with its own row count
 * and description. The rows come back as JSON with a `columns` list naming
 * their fields in the order a table should show them, which is what lets one
 * component render any of them without knowing which one it asked for.
 *
 * A table that is the input of a calculator leads to it: a row of the DSCP
 * table opens the marking calculator on that value, a codec or a link type
 * opens the voice calculator with it chosen (`leadOf` in `specs.ts`). The
 * table is for finding the row; what the row means in every other form is
 * worked out in one place, by the engine, and not repeated in columns here.
 * A build without that calculator has no such row action.
 */

import { useEffect } from 'react';
import { useNavigate } from 'react-router';
import type { ReferenceTable } from '@/api/types/toolbox';
import { Field } from '@/design/Field';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Table, type TableColumn } from '@/design/Table';
import { formatCount } from '@/lib/format';
import { Refusal } from './Answer';
import {
  prefill,
  TABLE_ROWS,
  useCatalogue,
  useDebounced,
  useReferenceTables,
  useToolResult,
  type Form,
} from './data';
import type { Row } from './facts';
import { leadOf, leadValues } from './specs';
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
  id: 'Id',
  // Eight columns of the codec table have to share one panel, so each
  // heading is the unit it is counted in: B for bytes.
  bit_rate_bps: 'Bit/s',
  sample_bytes: 'B/sample',
  sample_ms: 'ms/sample',
  packet_ms: 'ms/packet',
  payload_bytes: 'B/packet',
  mos: 'MOS',
  overhead_bytes: 'Bytes',
  parts: 'What they are',
  crtp: 'cRTP',
  note: 'Note',
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
  // A heading is cut 44 px short of its column (padding and the sort mark),
  // so a column is as wide as its heading needs, not as its two digits do.
  dscp: 96,
  tos: 84,
  precedence: 132,
  binary: 110,
  bytes: 90,
  level: 80,
  severity: 100,
  distance: 100,
  protocol: 110,
  // Kept short: the name already says which behaviour it is, the row leads
  // to the calculator that spells it out, and what the marking is *for* is
  // what somebody scanning this table needs the width for.
  phb: 110,
  keyword: 130,
  name: 150,
  // `frame-relay` is the longest of them, and an id that is cut cannot be
  // read off and typed.
  id: 112,
  bit_rate_bps: 88,
  sample_bytes: 106,
  sample_ms: 114,
  packet_ms: 114,
  payload_bytes: 106,
  mos: 80,
  overhead_bytes: 92,
  crtp: 84,
};

/** Columns of prose: everything else in a reference table is a figure or a keyword. */
const PROSE = new Set(['description', 'typical_use', 'name', 'phb', 'parts', 'source', 'note']);

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
    table ? { table, query: query.trim() || undefined, limit: TABLE_ROWS } : null,
  );
  const info = tables.data?.find((entry) => entry.table === table);

  // The calculator this table's rows lead to — only when this build has it:
  // the catalogue is the engine's own list of what there is.
  const navigate = useNavigate();
  const catalogue = useCatalogue();
  const lead = catalogue.data?.find((tool) => tool.id === leadOf(table));
  const follow = (row: KeyedRow) => {
    if (!lead) return;
    const values = leadValues(table, row);
    if (values === null || !prefill(lead.id, values)) return;
    navigate(`/tools/${lead.id}`);
  };

  const columns: TableColumn<KeyedRow>[] = (answer.data?.columns ?? []).map((field) => ({
    id: field,
    header: heading(field),
    mono: !PROSE.has(field),
    width: NARROW[field],
    align: typeof answer.data?.rows[0]?.[field] === 'number' ? ('right' as const) : undefined,
    cell: (row) => {
      const value = row[field];
      if (typeof value === 'boolean') return value ? 'yes' : 'no';
      if (value === null || value === undefined || value === '') return '—';
      // A table cell is one line and cuts what does not fit. A sentence that
      // was cut keeps its whole text on the cell, for whoever points at it;
      // the row's calculator, where there is one, shows it in full.
      return PROSE.has(field) ? <span title={String(value)}>{String(value)}</span> : String(value);
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
            onRowActivate={lead ? follow : undefined}
            rowActions={
              lead
                ? (row) =>
                    leadValues(table, row) === null
                      ? []
                      : [{ id: 'lead', label: `Open in ${lead.title}`, onSelect: () => follow(row) }]
                : undefined
            }
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
              {lead && ` Enter on a row, or the row’s menu, opens it in ${lead.title}.`}
            </p>
          )}
        </div>
      )}
    </>
  );
}
