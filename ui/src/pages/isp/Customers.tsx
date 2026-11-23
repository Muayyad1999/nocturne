/**
 * The customer list, built to be useful at a few thousand rows.
 *
 * `isp.customers` is paged and filtered in the engine (`ORDER BY name`, a
 * `LIKE` over the searchable text, `matching` for the count), so this screen
 * asks for one page at a time and never pulls the whole book down to count
 * it. The filters go into the request; Next sends the engine's own
 * `next_cursor`, which continues after the last customer on screen, so a
 * customer deleted from this page does not make the next one skip a row
 * (`paging.ts`).
 *
 * Two columns come from elsewhere, because a customer record does not carry
 * them:
 *
 * * **what they are using against what they pay for** — `isp.usage`, asked
 *   for with the same filters and the same page, so the two lists are the
 *   same rows in the same order (both are ordered by name in the engine) and
 *   are joined by id rather than by position;
 * * **what they owe** — `isp.arrears`, which is per customer and per
 *   currency; a customer who owes in two currencies has two figures and they
 *   are shown as two.
 *
 * While either is still in flight the cell is a skeleton, not a dash and not
 * a zero: "we have not asked yet" is not "nothing". And when either could not
 * be read the cell says "not read" — never "nothing owed", which is a fact
 * about the ledger that a failed read has not established.
 */

import { Plus, UserPlus } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router';
import type { Customer, CustomerState, UsedAgainstSold } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { Table, type TableColumn } from '@/design/Table';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { CustomerDialog } from './CustomerDialog';
import {
  useArrears,
  useCalendar,
  useCustomers,
  useDeleteCustomer,
  useLinks,
  useSetCustomerState,
  useSummary,
  useUsage,
} from './data';
import { Figures, Words } from './Figures';
import {
  coverageWords,
  isMeasured,
  ofPlanText,
  readingTime,
  soldText,
  usageText,
} from './coverage';
import { owedById, owedCell } from './ageing';
import { cursorParam, usePages } from './paging';
import { STATE_WORDS } from './records';
import styles from './isp.module.css';

const PAGE = 50;
const DEBOUNCE_MS = 250;

interface Filters {
  query: string;
  state: string;
  currency: string;
  tower: string;
}

export function Customers() {
  const navigate = useNavigate();
  const [filters, setFilters] = useState<Filters>({ query: '', state: '', currency: '', tower: '' });
  const [typed, setTyped] = useState('');
  const pages = usePages(PAGE);
  const calendar = useCalendar();
  const [editing, setEditing] = useState<{ open: boolean; customer: Customer | null }>({
    open: false,
    customer: null,
  });

  // The search runs a quarter of a second after the typing stops, so a long
  // name is one query and not twelve. The guard matters: without it the timer
  // armed on mount fires a moment later and sends the reader back to the
  // first page, whatever they did in between.
  useEffect(() => {
    if (typed === filters.query) return;
    const timer = setTimeout(() => {
      setFilters((current) => ({ ...current, query: typed }));
      pages.reset();
    }, DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [typed, filters.query]);

  const params = useMemo(
    () => ({
      ...(filters.query.trim() === '' ? {} : { query: filters.query.trim() }),
      ...(filters.state === '' ? {} : { state: filters.state as CustomerState }),
      ...(filters.currency === '' ? {} : { currency: filters.currency }),
      ...(filters.tower === '' ? {} : { tower: filters.tower }),
      limit: PAGE,
      ...cursorParam(pages.walked),
    }),
    [filters, pages.walked],
  );

  const page = useCustomers(params);
  const usage = useUsage(params);
  const arrears = useArrears();
  const summary = useSummary();
  const links = useLinks();
  const setState = useSetCustomerState();
  const remove = useDeleteCustomer();

  const usageById = useMemo(() => {
    const index = new Map<string, UsedAgainstSold>();
    for (const row of usage.data?.rows ?? []) index.set(row.customer_id, row);
    return index;
  }, [usage.data]);

  const owed = useMemo(() => owedById(arrears.data), [arrears.data]);

  const towers = useMemo(() => {
    const names = new Set<string>();
    for (const link of links.data ?? []) if (link.tower) names.add(link.tower);
    for (const customer of page.data?.customers ?? []) {
      if (customer.service.tower) names.add(customer.service.tower);
    }
    return [...names].sort((a, b) => a.localeCompare(b));
  }, [links.data, page.data]);

  const currencies = summary.data?.by_currency.map((entry) => entry.currency) ?? [];
  const matching = page.data?.matching ?? 0;
  const rows = page.data?.customers ?? [];
  const from = rows.length === 0 ? 0 : pages.first;
  const to = pages.first - 1 + rows.length;

  const columns: readonly TableColumn<Customer>[] = [
    {
      id: 'name',
      header: 'Customer',
      required: true,
      width: 220,
      sortValue: (row) => row.name.toLowerCase(),
      cell: (row) => (
        <button
          type="button"
          className={styles.rowLink}
          onClick={(event) => {
            event.stopPropagation();
            void navigate(`/isp/customer/${row.id}`);
          }}
        >
          {row.name}
          <span className={styles.rowSub}>
            {[row.code, row.site, row.service.tower].filter(Boolean).join(' · ') || 'no code'}
          </span>
        </button>
      ),
    },
    {
      id: 'state',
      header: 'Account',
      width: 110,
      sortValue: (row) => row.state,
      cell: (row) => (
        <Badge tone={STATE_WORDS[row.state].tone} variant="outline">
          {STATE_WORDS[row.state].label}
        </Badge>
      ),
    },
    {
      id: 'currency',
      header: 'Billed in',
      width: 90,
      sortValue: (row) => row.currency,
      cell: (row) => <span className={styles.amount}>{row.currency}</span>,
    },
    {
      id: 'sold',
      header: 'Sold',
      width: 130,
      align: 'right',
      cell: (row) => {
        const found = usageById.get(row.id);
        if (!found) return usage.isPending ? <Skeleton width={70} /> : <Words>not read</Words>;
        return found.sold_mbit === null ? (
          <Words>{soldText(null)}</Words>
        ) : (
          <span className={styles.amount}>{soldText(found.sold_mbit)}</span>
        );
      },
    },
    {
      id: 'using',
      header: 'Using',
      width: 190,
      cell: (row) => {
        const found = usageById.get(row.id);
        if (!found) return usage.isPending ? <Skeleton width={110} /> : <Words>not read</Words>;
        if (!isMeasured(found.coverage)) {
          return <Words title={found.coverage_note}>{coverageWords(found.coverage).label}</Words>;
        }
        const when = found.used ? readingTime(found.used, calendar) : null;
        return (
          <span className={styles.amount}>
            {usageText(found)}
            <span className={styles.rowSub}>
              {ofPlanText(found.of_plan_pct)} of the plan{when === null ? '' : `, ${when}`}
            </span>
          </span>
        );
      },
    },
    {
      id: 'owed',
      header: 'Owed',
      width: 150,
      align: 'right',
      cell: (row) => {
        const cell = owedCell(arrears, owed, row.id);
        switch (cell.kind) {
          case 'pending':
            return <Skeleton width={90} />;
          case 'not_read':
            return <Words title={arrears.error?.message}>not read</Words>;
          case 'nothing':
            return <Words>nothing owed</Words>;
          case 'owes':
            return <Figures totals={cell.amounts} />;
        }
      },
    },
    {
      id: 'contact',
      header: 'Contact',
      width: 180,
      defaultHidden: true,
      cell: (row) => (
        <span className={styles.rowSub}>
          {[row.contact.phone, row.contact.email].filter(Boolean).join(' · ') || '—'}
        </span>
      ),
    },
  ];

  if (page.isError) {
    return (
      <div className={styles.scrollPane}>
        <ErrorState
          title="The customers could not be read"
          error={page.error}
          onRetry={() => void page.refetch()}
        />
      </div>
    );
  }

  return (
    <div className={styles.pane}>
      <Toolbar className={styles.toolbar}>
        <SearchInput
          className={styles.search}
          value={typed}
          onChange={setTyped}
          placeholder="Name, code, phone, site or tower"
          aria-label="Search the customers"
        />
        <Select
          className={styles.filter}
          aria-label="Account state"
          value={filters.state}
          onChange={(event) => {
            setFilters({ ...filters, state: event.target.value });
            pages.reset();
          }}
          options={[
            { value: '', label: 'Any state' },
            { value: 'active', label: 'Active' },
            { value: 'suspended', label: 'Suspended' },
            { value: 'cancelled', label: 'Cancelled' },
          ]}
        />
        <Select
          className={styles.filter}
          aria-label="Billed in"
          value={filters.currency}
          onChange={(event) => {
            setFilters({ ...filters, currency: event.target.value });
            pages.reset();
          }}
          options={[
            { value: '', label: 'Any currency' },
            ...currencies.map((code) => ({ value: code, label: code })),
          ]}
        />
        {towers.length > 0 && (
          <Select
            className={styles.filter}
            aria-label="Tower"
            value={filters.tower}
            onChange={(event) => {
              setFilters({ ...filters, tower: event.target.value });
              pages.reset();
            }}
            options={[
              { value: '', label: 'Any tower' },
              ...towers.map((tower) => ({ value: tower, label: tower })),
            ]}
          />
        )}
        <ToolbarSpacer />
        <Button
          variant="primary"
          icon={Plus}
          onClick={() => setEditing({ open: true, customer: null })}
        >
          Add a customer
        </Button>
      </Toolbar>

      <div className={styles.tableWrap}>
        <Table
          id="isp.customers"
          aria-label="Customers"
          columns={columns}
          rows={rows}
          rowKey={(row) => row.id}
          loading={page.isPending}
          fill
          onRowActivate={(row) => void navigate(`/isp/customer/${row.id}`)}
          rowActions={(row) => [
            { label: 'Open', onSelect: () => void navigate(`/isp/customer/${row.id}`) },
            { label: 'Edit details…', onSelect: () => setEditing({ open: true, customer: row }) },
            { type: 'separator' },
            ...(['active', 'suspended', 'cancelled'] as CustomerState[])
              .filter((state) => state !== row.state)
              .map((state) => ({
                label: `Mark ${STATE_WORDS[state].label.toLowerCase()}`,
                onSelect: () => {
                  void (async () => {
                    const ok = await confirm({
                      title: `Mark ${row.name} ${STATE_WORDS[state].label.toLowerCase()}?`,
                      description: STATE_WORDS[state].consequence,
                      confirmLabel: `Mark ${STATE_WORDS[state].label.toLowerCase()}`,
                    });
                    if (!ok) return;
                    setState.mutate(
                      { id: row.id, state },
                      {
                        onSuccess: (change) => toast.success(change.note),
                      },
                    );
                  })();
                },
              })),
            { type: 'separator' },
            {
              label: 'Delete…',
              tone: 'danger',
              onSelect: () => {
                void (async () => {
                  const ok = await confirm({
                    title: `Delete ${row.name}?`,
                    description:
                      'Only a customer with no invoice and no payment can be deleted; their ledger is never deleted with them. A customer who has been billed is marked cancelled instead, and their invoices and payments stay.',
                    confirmLabel: 'Delete',
                    tone: 'danger',
                  });
                  if (!ok) return;
                  remove.mutate(
                    { id: row.id },
                    { onSuccess: (deleted) => toast.success(`${deleted.name} was deleted`) },
                  );
                })();
              },
            },
          ]}
          empty={
            <EmptyState
              icon={UserPlus}
              size="compact"
              title={
                matching === 0 && filters.query === '' && filters.state === ''
                  ? 'No customers yet'
                  : 'No customer matches these filters'
              }
              description={
                matching === 0 && filters.query === ''
                  ? 'A customer is who you bill: their details, the currency they are billed in, and the device and interface that serve them.'
                  : 'Clear the search or the filters to see the rest.'
              }
              action={
                <Button variant="primary" icon={Plus} onClick={() => setEditing({ open: true, customer: null })}>
                  Add a customer
                </Button>
              }
            />
          }
        />
        <div className={styles.paging}>
          <span className={styles.count}>
            {rows.length === 0
              ? `${formatCount(matching)} matching`
              : `${formatCount(from)}–${formatCount(to)} of ${formatCount(matching)}`}
          </span>
          <ToolbarSpacer />
          <Button
            size="sm"
            disabled={!pages.hasPrevious || page.isFetching}
            onClick={pages.previous}
          >
            Previous
          </Button>
          <Button
            size="sm"
            disabled={page.data?.next_cursor == null || page.isFetching}
            onClick={() => {
              const cursor = page.data?.next_cursor;
              if (cursor != null) pages.next(cursor);
            }}
          >
            Next
          </Button>
        </div>
      </div>

      <CustomerDialog
        open={editing.open}
        customer={editing.customer}
        onClose={() => setEditing({ open: false, customer: null })}
      />
    </div>
  );
}
