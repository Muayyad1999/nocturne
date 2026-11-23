/**
 * Margin: what a link earns after what the bandwidth cost.
 *
 * The engine works the margin out **in the currency the wholesale cost is in,
 * and in no other**. Revenue behind a tower can be in several currencies, and
 * the ones the cost is not in come back untouched in `unconverted`, with a
 * note saying so, rather than converted at a rate nobody entered. Both are
 * shown: the figure, and what is not in it.
 *
 * The per-megabit column is this screen's own arithmetic — the margin divided
 * by the megabits the cost was worked out on — and it is integer division, so
 * it says when it was not exact rather than presenting a rounded figure as
 * the ledger (`money.ts: divide`).
 *
 * `isp.margin` answers per link (or for one customer, which is on their own
 * page). With no link recorded at all it answers `not_found` with a sentence
 * about what to do, so that is shown as an empty state and not as a failure —
 * see `NOTES.md`.
 */

import { Coins } from 'lucide-react';
import { useMemo, useState } from 'react';
import type { Margin as MarginRow } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Table, type TableColumn } from '@/design/Table';
import { formatCount } from '@/lib/format';
import { rateText } from './coverage';
import { useLinks, useMargin } from './data';
import { Amount, Figures, Words } from './Figures';
import { divide, moneySortKey } from './money';
import styles from './isp.module.css';

export const MARGIN_COLUMNS: readonly TableColumn<MarginRow>[] = [
  {
    id: 'name',
    header: 'Of',
    required: true,
    width: 160,
    sortValue: (row) => row.name.toLowerCase(),
    cell: (row) => (
      <span>
        {row.name}
        <span className={styles.rowSub}>{row.of}</span>
      </span>
    ),
  },
  {
    id: 'revenue',
    header: 'Revenue a month',
    width: 155,
    align: 'right',
    cell: (row) => <Figures totals={row.revenue} empty="nothing is sold on it" />,
  },
  {
    id: 'cost',
    header: 'Bandwidth a month',
    width: 145,
    align: 'right',
    sortValue: (row) => moneySortKey(row.cost),
    cell: (row) =>
      row.cost === null ? (
        <Words title="No wholesale cost per megabit is recorded, so there is no margin to work out — not a margin of zero.">
          no cost recorded
        </Words>
      ) : (
        <span className={styles.amount}>
          <Amount money={row.cost} />
          {row.cost_mbit !== null && (
            <span className={styles.rowSub}>on {rateText(row.cost_mbit)}</span>
          )}
        </span>
      ),
  },
  {
    id: 'margin',
    header: 'Margin',
    width: 155,
    align: 'right',
    sortValue: (row) => moneySortKey(row.margin),
    cell: (row) =>
      row.margin === null ? (
        <Words>no figure</Words>
      ) : (
        <span className={styles.amount}>
          <Amount money={row.margin} />
          <span className={styles.rowSub}>
            in {row.margin.currency} only{row.exact ? '' : ' · a division rounded'}
          </span>
        </span>
      ),
  },
  {
    id: 'per_mbit',
    header: 'Per megabit',
    width: 135,
    align: 'right',
    cell: (row) => {
      if (row.margin === null || row.cost_mbit === null) return <Words>no figure</Words>;
      const each = divide(row.margin, row.cost_mbit);
      if (each === null) return <Words>no megabits to divide by</Words>;
      return (
        <span className={styles.amount}>
          <Amount money={each.amount} />
          <span className={styles.rowSub}>
            {each.exact ? 'exactly' : 'rounded towards zero'}
          </span>
        </span>
      );
    },
  },
  {
    id: 'unconverted',
    header: 'Not in the margin',
    width: 175,
    align: 'right',
    cell: (row) =>
      row.unconverted.length === 0 ? (
        <Words>nothing left out</Words>
      ) : (
        <span>
          <Figures totals={row.unconverted} />
          <span className={styles.rowSub}>left as it is: no rate was entered for it</span>
        </span>
      ),
  },
];

export function Margin() {
  const [tower, setTower] = useState('');
  const links = useLinks();
  const report = useMargin(tower === '' ? {} : { tower });

  const towers = useMemo(() => {
    const names = new Set<string>();
    for (const link of links.data ?? []) if (link.tower) names.add(link.tower);
    return [...names].sort((a, b) => a.localeCompare(b));
  }, [links.data]);

  const rows = report.data ?? [];
  // No link at all is answered as `not_found` with a sentence about what to
  // do, which is an empty report and not a failure.
  const nothingRecorded = report.isError && report.error.code === 'not_found';

  return (
    <Panel
      title="Margin per megabit"
      padding="none"
      actions={
        towers.length > 0 ? (
          <Select
            size="sm"
            aria-label="Tower"
            value={tower}
            onChange={(event) => setTower(event.target.value)}
            options={[
              { value: '', label: 'Every link' },
              ...towers.map((name) => ({ value: name, label: name })),
            ]}
          />
        ) : undefined
      }
    >
      <div className={styles.panelBody}>
        {report.isError && !nothingRecorded && (
          <ErrorState
            size="compact"
            title="The margin could not be worked out"
            error={report.error}
            onRetry={() => void report.refetch()}
          />
        )}
        {report.isPending && <Skeleton shape="block" height={140} />}
        {nothingRecorded && (
          <EmptyState
            icon={Coins}
            size="compact"
            title="No link has a wholesale cost yet"
            description={report.error.message}
          />
        )}
        {report.data && (
          <>
            <Table
              id="isp.margin"
              aria-label="Margin per link"
              columns={MARGIN_COLUMNS}
              rows={rows}
              rowKey={(row) => `${row.of}:${row.id}`}
              maxHeight={360}
              empty={
                <EmptyState
                  icon={Coins}
                  size="compact"
                  title="Nothing to work a margin out from"
                  description="Record what a megabit costs you on a link, and what the customers behind it pay can be set against it. Per currency, because revenue behind one tower can be in several."
                />
              }
            />
            {rows.length > 0 && (
              <>
                <p className={styles.quiet}>{rows[0]?.note}</p>
                {rows.some((row) => !row.exact) && (
                  <ul className={styles.chips}>
                    <li>
                      <Badge tone="warn" variant="outline">
                        {formatCount(rows.filter((row) => !row.exact).length)} of these came out of
                        a division that rounded
                      </Badge>
                    </li>
                  </ul>
                )}
              </>
            )}
          </>
        )}
      </div>
    </Panel>
  );
}
