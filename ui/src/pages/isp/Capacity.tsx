/**
 * Oversubscription: what a link has been sold, against what it can carry.
 *
 * Both figures, always, and the ratio beside them — never the ratio alone.
 * The engine's own note says why: 3:1 on a link that is never busy reads
 * differently from 3:1 on one that saturates nightly, which is what the
 * measured load is for. A link with no capacity written down has no ratio at
 * all, and that is a sentence, not 0:1.
 *
 * What was sold to **suspended** accounts is counted apart rather than
 * quietly folded in: it is still sold, and whether it is really free capacity
 * is a judgement, not arithmetic.
 *
 * The coverage sentence here is this screen's own (`coverage.ts:
 * linkCoverageNote`). `isp.capacity` sends the customer-worded one, which on
 * a tower feed reads "no serving device is recorded for this customer" — see
 * `NOTES.md`.
 */

import { Cable } from 'lucide-react';
import { useMemo, useState } from 'react';
import type { LinkCapacity } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { ProgressBar } from '@/design/ProgressBar';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Table, type TableColumn } from '@/design/Table';
import { formatCount, plural } from '@/lib/format';
import {
  capacityBasisText,
  isMeasured,
  linkCoverageNote,
  loadText,
  rateText,
  ratioTone,
  readingTime,
} from './coverage';
import { useCalendar, useCapacity } from './data';
import { Words } from './Figures';
import styles from './isp.module.css';

const HOUR = 3_600_000;

export function Capacity() {
  const [window, setWindow] = useState('latest');
  const span = useMemo(() => {
    if (window === 'latest') return {};
    const hours = window === '24h' ? 24 : 24 * 7;
    return { since: Date.now() - hours * HOUR };
  }, [window]);

  const report = useCapacity(span);
  const calendar = useCalendar();

  const columns: readonly TableColumn<LinkCapacity>[] = [
    {
      id: 'link',
      header: 'Link',
      required: true,
      width: 170,
      sortValue: (row) => row.link_name.toLowerCase(),
      cell: (row) => (
        <span>
          {row.link_name}
          <span className={styles.rowSub}>
            {row.tower ?? 'no tower label'} · {capacityBasisText(row.capacity_basis)}
          </span>
        </span>
      ),
    },
    {
      id: 'sold',
      header: 'Sold',
      width: 135,
      align: 'right',
      sortValue: (row) => row.sold_mbit,
      cell: (row) => (
        <span className={styles.amount}>
          {rateText(row.sold_mbit)}
          <span className={styles.rowSub}>{plural(row.customers, 'customer')}</span>
        </span>
      ),
    },
    {
      id: 'capacity',
      header: 'Carries',
      width: 115,
      align: 'right',
      sortValue: (row) => row.capacity_mbit,
      cell: (row) =>
        row.capacity_mbit > 0 ? (
          <span className={styles.amount}>{rateText(row.capacity_mbit)}</span>
        ) : (
          <Words>no capacity recorded</Words>
        ),
    },
    {
      id: 'ratio',
      header: 'Oversubscription',
      width: 170,
      sortValue: (row) => row.ratio,
      cell: (row) =>
        row.ratio === null ? (
          <Words title={row.note}>{row.ratio_text}</Words>
        ) : (
          <span className={styles.inlineGroup}>
            <Badge tone={ratioTone(row.ratio)} variant="outline">
              {row.ratio_text}
            </Badge>
            <ProgressBar
              kind="meter"
              // The bar is the sold megabits against the capacity, for the
              // eye: both figures are in the columns beside it.
              value={Math.min(100, (row.sold_mbit / Math.max(1, row.capacity_mbit)) * 100)}
              tone={ratioTone(row.ratio)}
              size="sm"
              valueText={row.ratio_text}
              aria-label={`${row.link_name}: ${row.ratio_text}`}
            />
          </span>
        ),
    },
    {
      id: 'suspended',
      header: 'Sold to suspended',
      width: 135,
      align: 'right',
      cell: (row) =>
        row.suspended_customers === 0 ? (
          <Words>none</Words>
        ) : (
          <span className={styles.amount}>
            {rateText(row.suspended_mbit)}
            <span className={styles.rowSub}>
              {plural(row.suspended_customers, 'suspended account')}, counted apart
            </span>
          </span>
        ),
    },
    {
      id: 'measured',
      header: 'Carrying',
      width: 190,
      align: 'right',
      sortValue: (row) => row.measured?.used_mbit ?? null,
      cell: (row) =>
        isMeasured(row.coverage) && row.measured ? (
          <span className={styles.amount}>
            {loadText(row)}
            {row.measured.peak_mbit !== null && (
              <span className={styles.rowSub}>peak {rateText(row.measured.peak_mbit)}</span>
            )}
            {readingTime(row.measured, calendar) !== null && (
              <span className={styles.rowSub}>{readingTime(row.measured, calendar)}</span>
            )}
          </span>
        ) : (
          <Words title={linkCoverageNote(row.coverage)}>{loadText(row)}</Words>
        ),
    },
  ];

  return (
    <Panel
      title="Oversubscription"
      padding="none"
      actions={
        <Select
          size="sm"
          aria-label="Load window"
          value={window}
          onChange={(event) => setWindow(event.target.value)}
          options={[
            { value: 'latest', label: 'Latest reading' },
            { value: '24h', label: 'Averaged over 24 hours' },
            { value: '7d', label: 'Averaged over 7 days' },
          ]}
        />
      }
    >
      <div className={styles.panelBody}>
        {report.isError && (
          <ErrorState
            size="compact"
            title="The links could not be read"
            error={report.error}
            onRetry={() => void report.refetch()}
          />
        )}
        {report.isPending && <Skeleton shape="block" height={140} />}
        {report.data && (
          <>
            <Table
              id="isp.capacity"
              aria-label="Links, what each was sold and what it carries"
              columns={columns}
              rows={report.data}
              rowKey={(row) => row.link_id}
              maxHeight={360}
              empty={
                <EmptyState
                  icon={Cable}
                  size="compact"
                  title="No links recorded"
                  description="A link is an upstream or a tower's feed. Write down what it carries, and what the customers behind it were sold can be set against it."
                />
              }
            />
            {report.data.length > 0 && (
              <p className={styles.quiet}>
                {formatCount(report.data.length)} link
                {report.data.length === 1 ? '' : 's'}. {report.data[0]?.note}
              </p>
            )}
          </>
        )}
      </div>
    </Panel>
  );
}
