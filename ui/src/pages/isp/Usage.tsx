/**
 * Used against sold: what a customer pays for, and what is actually passing.
 *
 * The report leads with its coverage, not with its rows, because the rows
 * cannot be read without it: a customer with no serving interface has **no**
 * usage figure, and a screen that drew that as 0 Mbit/s would be the easiest
 * possible way to make somebody argue with a customer about a number that
 * does not exist. The ten reasons are counted, in their own words, above the
 * table; in the table itself a row with no figure says which of them it is.
 *
 * A figure is either the latest reading or an average over a window, and it
 * says which, and which direction it took (the `usage_basis` setting), and
 * over what resolution the monitor answered. A latest reading says when it
 * was taken, under the figure: the engine only calls it a figure inside the
 * usage window, and inside it the moment is still part of the fact.
 *
 * The report is of one page of customers, by name, as `isp.customers` pages
 * them. When the filters match more, it says which of them these are and
 * that every count is of this page, and it pages like the customer list —
 * a report of the first two hundred is not a report of the estate.
 */

import { Gauge } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Link } from 'react-router';
import type { CustomerState, UsedAgainstSold } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Table, type TableColumn } from '@/design/Table';
import { Button } from '@/design/Button';
import { ToolbarSpacer } from '@/design/Toolbar';
import { formatCount, plural } from '@/lib/format';
import { dayTimeText } from './calendar';
import {
  basisText,
  coverageTally,
  coverageWords,
  isMeasured,
  ofPlanText,
  ofPlanTone,
  readingTime,
  soldText,
  usageScopeText,
  usageText,
} from './coverage';
import { useCalendar, useLinks, useUsage } from './data';
import { Words } from './Figures';
import { cursorParam, usePages } from './paging';
import { STATE_WORDS } from './records';
import styles from './isp.module.css';

const HOUR = 3_600_000;
const PAGE = 200;

export function Usage() {
  const [window, setWindow] = useState('latest');
  const [state, setState] = useState('');
  const [tower, setTower] = useState('');
  const pages = usePages(PAGE);
  const calendar = useCalendar();
  const links = useLinks();

  const span = useMemo(() => {
    if (window === 'latest') return {};
    const hours = window === '24h' ? 24 : 24 * 7;
    return { since: Date.now() - hours * HOUR };
  }, [window]);

  const report = useUsage({
    ...(state === '' ? {} : { state: state as CustomerState }),
    ...(tower === '' ? {} : { tower }),
    ...span,
    limit: PAGE,
    ...cursorParam(pages.walked),
  });
  const scope = report.data ? usageScopeText(report.data, pages.first) : null;

  // A usage row carries no tower, so the links are where the filter's options
  // come from: they are the records that say which towers exist.
  const towers = useMemo(() => {
    const names = new Set<string>();
    for (const link of links.data ?? []) if (link.tower) names.add(link.tower);
    return [...names].sort((a, b) => a.localeCompare(b));
  }, [links.data]);

  const columns: readonly TableColumn<UsedAgainstSold>[] = [
    {
      id: 'customer',
      header: 'Customer',
      required: true,
      width: 180,
      sortValue: (row) => row.customer_name.toLowerCase(),
      cell: (row) => (
        <Link to={`/isp/customer/${row.customer_id}`}>{row.customer_name}</Link>
      ),
    },
    {
      id: 'state',
      header: 'Account',
      width: 100,
      cell: (row) => (
        <Badge tone={STATE_WORDS[row.state].tone} variant="outline">
          {STATE_WORDS[row.state].label}
        </Badge>
      ),
    },
    {
      id: 'sold',
      header: 'Sold',
      width: 130,
      align: 'right',
      sortValue: (row) => row.sold_mbit,
      cell: (row) =>
        row.sold_mbit === null ? (
          <Words>{soldText(null)}</Words>
        ) : (
          <span className={styles.amount}>
            {soldText(row.sold_mbit)}
            <span className={styles.rowSub}>{plural(row.subscriptions, 'plan')}</span>
          </span>
        ),
    },
    {
      id: 'using',
      header: 'Using',
      width: 160,
      align: 'right',
      sortValue: (row) => row.used?.used_mbit ?? null,
      cell: (row) =>
        isMeasured(row.coverage) && row.used ? (
          <span className={styles.amount}>
            {usageText(row)}
            {readingTime(row.used, calendar) !== null && (
              <span className={styles.rowSub}>{readingTime(row.used, calendar)}</span>
            )}
          </span>
        ) : (
          <Words title={row.coverage_note}>{coverageWords(row.coverage).label}</Words>
        ),
    },
    {
      id: 'share',
      header: 'Of the plan',
      width: 105,
      align: 'right',
      sortValue: (row) => row.of_plan_pct,
      cell: (row) =>
        row.of_plan_pct === null ? (
          <Words
            title={
              row.sold_mbit === null
                ? 'They are on no plan, so there is nothing to be a share of.'
                : row.coverage_note
            }
          >
            {ofPlanText(null)}
          </Words>
        ) : (
          <Badge tone={ofPlanTone(row.of_plan_pct)} variant="outline">
            {ofPlanText(row.of_plan_pct)}
          </Badge>
        ),
    },
    {
      // Which interface a figure would come from — not a second copy of the
      // Using column: a customer with no device and one with a device but no
      // interface are different rows here.
      id: 'where',
      header: 'Measured on',
      width: 230,
      cell: (row) =>
        row.interface === null ? (
          <Words title={row.coverage_note}>
            {row.device_id === null ? 'nothing to measure on' : 'no interface recorded'}
          </Words>
        ) : (
          <span className={styles.rowSub}>
            {row.interface}
            {row.used && <span className={styles.rowSub}>{basisText(row.used)}</span>}
          </span>
        ),
    },
  ];

  return (
    <Panel
      title="Used against sold"
      padding="none"
      actions={
        <>
          <Select
            size="sm"
            aria-label="Usage window"
            value={window}
            onChange={(event) => {
              setWindow(event.target.value);
              pages.reset();
            }}
            options={[
              { value: 'latest', label: 'Latest reading' },
              { value: '24h', label: 'Averaged over 24 hours' },
              { value: '7d', label: 'Averaged over 7 days' },
            ]}
          />
          <Select
            size="sm"
            aria-label="Account state"
            value={state}
            onChange={(event) => {
              setState(event.target.value);
              pages.reset();
            }}
            options={[
              { value: '', label: 'Any state' },
              { value: 'active', label: 'Active' },
              { value: 'suspended', label: 'Suspended' },
              { value: 'cancelled', label: 'Cancelled' },
            ]}
          />
          {towers.length > 0 && (
            <Select
              size="sm"
              aria-label="Tower"
              value={tower}
              onChange={(event) => {
                setTower(event.target.value);
                pages.reset();
              }}
              options={[
                { value: '', label: 'Any tower' },
                ...towers.map((name) => ({ value: name, label: name })),
              ]}
            />
          )}
        </>
      }
    >
      <div className={styles.panelBody}>
        {report.isError && (
          <ErrorState
            size="compact"
            title="The usage could not be read"
            error={report.error}
            onRetry={() => void report.refetch()}
          />
        )}
        {report.isPending && <Skeleton shape="block" height={160} />}
        {report.data && (
          <>
            {scope !== null && <p className={styles.note}>{scope}</p>}
            <p className={styles.note}>
              {formatCount(report.data.measured)} of {plural(report.data.customers, 'customer')} have
              a usage figure.{' '}
              {report.data.without_usage > 0
                ? `${formatCount(report.data.without_usage)} have none — which is not a usage of zero:`
                : ''}
            </p>
            {report.data.coverage.length > 0 && (
              <ul className={styles.chips}>
                {coverageTally(report.data.coverage).map((entry) => (
                  <li key={entry.coverage}>
                    <Badge tone={entry.tone} variant="outline" title={entry.meaning}>
                      {formatCount(entry.customers)} · {entry.label}
                    </Badge>
                  </li>
                ))}
              </ul>
            )}
            {report.data.window && (
              <p className={styles.quiet}>
                Averaged from {dayTimeText(report.data.window.since, calendar)} to{' '}
                {dayTimeText(report.data.window.until, calendar)}, on the business’s calendar,
                at the monitor's {report.data.window.rollup} resolution.
              </p>
            )}
            <Table
              id="isp.usage"
              aria-label="Customers, what each was sold and what each is using"
              columns={columns}
              rows={report.data.rows}
              rowKey={(row) => row.customer_id}
              maxHeight={420}
              empty={
                <EmptyState
                  icon={Gauge}
                  size="compact"
                  title="No customers to compare"
                  description="A customer needs a plan to be compared against, and a serving device and interface for there to be anything to measure."
                />
              }
            />
            {(pages.hasPrevious || report.data.next_cursor !== null) && (
              <div className={styles.paging}>
                <span className={styles.count}>
                  {report.data.customers === 0
                    ? `${formatCount(report.data.matching)} matching`
                    : `${formatCount(pages.first)}–${formatCount(
                        pages.first - 1 + report.data.customers,
                      )} of ${formatCount(report.data.matching)}`}
                </span>
                <ToolbarSpacer />
                <Button
                  size="sm"
                  disabled={!pages.hasPrevious || report.isFetching}
                  onClick={pages.previous}
                >
                  Previous
                </Button>
                <Button
                  size="sm"
                  disabled={report.data.next_cursor === null || report.isFetching}
                  onClick={() => {
                    const cursor = report.data?.next_cursor;
                    if (cursor != null) pages.next(cursor);
                  }}
                >
                  Next
                </Button>
              </div>
            )}
            <p className={styles.quiet}>{report.data.note}</p>
          </>
        )}
      </div>
    </Panel>
  );
}
