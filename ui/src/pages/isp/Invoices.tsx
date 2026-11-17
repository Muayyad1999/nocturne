/**
 * The invoices: drafted, issued, voided and reissued.
 *
 * An invoice is a document, and what has happened to it as a document
 * (`state`) is not the same question as whether it has been paid (`status`,
 * which the engine derives from the recorded payments and the clock at read
 * time). Both are shown, because an issued invoice that is overdue and an
 * issued invoice that is not due yet are the same document in two different
 * situations.
 *
 * `isp.invoices` is paged but, unlike the customers, carries no count of what
 * matches, so paging here is "a full page means there may be more" rather
 * than "1–50 of 3 412". The alternative would be to ask for everything to
 * count it, which is the thing the paging exists to avoid.
 *
 * The run and the drawer are driven by the address (`?run=1`, `?invoice=…`)
 * so that both can be reached from the command palette and linked to.
 */

import { CreditCard, HandCoins, Play } from 'lucide-react';
import { useMemo, useState } from 'react';
import type { SetURLSearchParams } from 'react-router';
import type { InvoiceState, InvoiceView } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Select } from '@/design/Select';
import { Table, type TableColumn } from '@/design/Table';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { formatDate } from '@/lib/time';
import { overdueText, recentMonths, statusWords, windowOf } from './billing-rows';
import { useInvoices, useSummary } from './data';
import { Amount } from './Figures';
import { InvoiceDrawer } from './InvoiceDrawer';
import { PaymentDialog } from './PaymentDialog';
import { RunDialog } from './RunDialog';
import styles from './isp.module.css';

const PAGE = 50;

export interface InvoicesProps {
  search: URLSearchParams;
  setSearch: SetURLSearchParams;
}

export function Invoices({ search, setSearch }: InvoicesProps) {
  const [state, setState] = useState('');
  const [currency, setCurrency] = useState('');
  const [month, setMonth] = useState('');
  const [offset, setOffset] = useState(0);
  const [paying, setPaying] = useState(false);

  const months = useMemo(() => recentMonths(Date.now()), []);
  const window = month === '' ? null : windowOf(month);
  const params = useMemo(
    () => ({
      ...(state === '' ? {} : { state: state as InvoiceState }),
      ...(currency === '' ? {} : { currency }),
      ...(window ? { since: window.start, until: window.end } : {}),
      limit: PAGE,
      ...(offset === 0 ? {} : { cursor: String(offset) }),
    }),
    [state, currency, window?.start, window?.end, offset],
  );

  const invoices = useInvoices(params);
  const summary = useSummary();
  const rows = invoices.data ?? [];
  const openId = search.get('invoice');
  const runOpen = search.get('run') !== null;

  const setParam = (key: string, value: string | null) => {
    const next = new URLSearchParams(search);
    if (value === null) next.delete(key);
    else next.set(key, value);
    setSearch(next, { replace: value === null });
  };

  const columns: readonly TableColumn<InvoiceView>[] = [
    {
      id: 'number',
      header: 'Invoice',
      required: true,
      width: 140,
      sortValue: (row) => row.number ?? '',
      // The number is the way in, as a button: a row that only answers a
      // double-click is a row nobody finds.
      cell: (row) => (
        <button
          type="button"
          className={styles.rowLink}
          onClick={(event) => {
            event.stopPropagation();
            setParam('invoice', row.id);
          }}
        >
          {row.number === null ? (
            // A number is allocated when it is issued, never before.
            <span className={styles.words}>not issued</span>
          ) : (
            <span className={styles.amount}>{row.number}</span>
          )}
        </button>
      ),
    },
    {
      id: 'customer',
      // The name the invoice itself carries, as it was when it was written:
      // said once, in the column's name, rather than under every row.
      header: 'Customer (as written)',
      width: 200,
      sortValue: (row) => row.customer_name.toLowerCase(),
      cell: (row) => <span>{row.customer_name}</span>,
    },
    {
      id: 'period',
      header: 'Period',
      width: 230,
      sortValue: (row) => row.period_start,
      cell: (row) => (
        <span className={styles.amount}>
          {formatDate(row.period_start)} → {formatDate(row.period_end)}
        </span>
      ),
    },
    {
      id: 'total',
      header: 'Total',
      width: 130,
      align: 'right',
      sortValue: (row) => row.total.minor,
      cell: (row) => <Amount money={row.total} />,
    },
    {
      id: 'paid',
      header: 'Paid',
      width: 130,
      align: 'right',
      cell: (row) => <Amount money={row.paid} muted />,
    },
    {
      id: 'outstanding',
      header: 'Outstanding',
      width: 130,
      align: 'right',
      sortValue: (row) => row.outstanding.minor,
      cell: (row) => <Amount money={row.outstanding} />,
    },
    {
      id: 'status',
      header: 'Status',
      width: 110,
      sortValue: (row) => row.status,
      cell: (row) => (
        <Badge tone={statusWords(row.status).tone} variant="outline">
          {statusWords(row.status).label}
        </Badge>
      ),
    },
    {
      id: 'due',
      header: 'Due',
      width: 150,
      cell: (row) => (
        <span className={styles.rowSub}>
          {overdueText(row)}
          {row.due_at !== null && (
            <span className={styles.rowSub}>{formatDate(row.due_at)}</span>
          )}
        </span>
      ),
    },
  ];

  if (invoices.isError) {
    return (
      <div className={styles.scrollPane}>
        <ErrorState
          title="The invoices could not be read"
          error={invoices.error}
          onRetry={() => void invoices.refetch()}
        />
      </div>
    );
  }

  return (
    <div className={styles.pane}>
      <Toolbar className={styles.toolbar}>
        <Select
          className={styles.filter}
          aria-label="Invoice state"
          value={state}
          onChange={(event) => {
            setState(event.target.value);
            setOffset(0);
          }}
          options={[
            { value: '', label: 'Any state' },
            { value: 'draft', label: 'Drafts' },
            { value: 'issued', label: 'Issued' },
            { value: 'void', label: 'Void' },
          ]}
        />
        <Select
          className={styles.filter}
          aria-label="Currency"
          value={currency}
          onChange={(event) => {
            setCurrency(event.target.value);
            setOffset(0);
          }}
          options={[
            { value: '', label: 'Any currency' },
            ...(summary.data?.by_currency ?? []).map((entry) => ({
              value: entry.currency,
              label: entry.currency,
            })),
          ]}
        />
        <Select
          className={styles.filter}
          aria-label="Period"
          value={month}
          onChange={(event) => {
            setMonth(event.target.value);
            setOffset(0);
          }}
          options={[{ value: '', label: 'Any period' }, ...months]}
        />
        <ToolbarSpacer />
        <Button icon={HandCoins} onClick={() => setPaying(true)}>
          Record a payment…
        </Button>
        <Button variant="primary" icon={Play} onClick={() => setParam('run', '1')}>
          Bill a period…
        </Button>
      </Toolbar>

      <div className={styles.tableWrap}>
        <Table
          id="isp.invoices"
          aria-label="Invoices"
          columns={columns}
          rows={rows}
          rowKey={(row) => row.id}
          loading={invoices.isPending}
          fill
          defaultSort={{ columnId: 'period', direction: 'desc' }}
          onRowActivate={(row) => setParam('invoice', row.id)}
          empty={
            <EmptyState
              icon={CreditCard}
              size="compact"
              title={state === '' && month === '' ? 'No invoices yet' : 'No invoice matches these filters'}
              description="Billing a period drafts one invoice per customer out of what their subscriptions say they agreed to. It previews first, and nothing is written until you say so."
              action={
                <Button variant="primary" icon={Play} onClick={() => setParam('run', '1')}>
                  Bill a period…
                </Button>
              }
            />
          }
        />
        <div className={styles.paging}>
          <span className={styles.count}>
            {rows.length === 0
              ? 'nothing on this page'
              : `${formatCount(offset + 1)}–${formatCount(offset + rows.length)}`}
          </span>
          <ToolbarSpacer />
          <Button
            size="sm"
            disabled={offset === 0 || invoices.isFetching}
            onClick={() => setOffset(Math.max(0, offset - PAGE))}
          >
            Previous
          </Button>
          <Button
            size="sm"
            disabled={rows.length < PAGE || invoices.isFetching}
            onClick={() => setOffset(offset + PAGE)}
          >
            Next
          </Button>
        </div>
      </div>

      <RunDialog open={runOpen} onClose={() => setParam('run', null)} />
      <PaymentDialog open={paying} onClose={() => setPaying(false)} />
      <InvoiceDrawer invoiceId={openId} onClose={() => setParam('invoice', null)} />
    </div>
  );
}
