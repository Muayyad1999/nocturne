/**
 * The invoices: drafted, issued, voided and reissued.
 *
 * An invoice is a document, and what has happened to it as a document
 * (`state`) is not the same question as whether it has been settled
 * (`status`, which the engine derives from the recorded payments, the credit
 * notes and the business's calendar at read time). Both are shown, because
 * an issued invoice that is overdue and an issued invoice that is not due
 * yet are the same document in two different situations.
 *
 * Money columns sort within a currency, never across them: the list mixes
 * currencies unless one is chosen, and 3 000 dinars is not more than 25
 * dollars (`money.ts: moneySortKey`).
 *
 * The period filter is a month of the business's calendar, and an invoice
 * is in it when its own period — read in the calendar the invoice was
 * written in — is part of that month.
 *
 * `isp.invoices` is paged but, unlike the customers, carries no count of what
 * matches, so paging here is "a full page means there may be more" rather
 * than "1–50 of 3 412". The alternative would be to ask for everything to
 * count it, which is the thing the paging exists to avoid. Next continues
 * after the last invoice the engine sent, not after a count of them: the list
 * is newest period first, so the drafts of a run started meanwhile sort in
 * front of this page and would otherwise come round again (`paging.ts`).
 *
 * The columns shown at first fit the list at the smallest window the
 * application supports (1280 × 720, navigation open), so the billing state —
 * Status and Due — is never off to the side. What was paid is in the column
 * chooser, and the invoice itself shows all four figures.
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
import { dueText, invoicePeriod, pageText, statusWords } from './billing-rows';
import { dayText, monthQuery, periodInMonth, recentMonths } from './calendar';
import { useCalendar, useInvoices, useSummary } from './data';
import { Amount } from './Figures';
import { InvoiceDrawer } from './InvoiceDrawer';
import { formatMoney, moneySortKey } from './money';
import { cursorAfter, cursorParam, usePages } from './paging';
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
  const pages = usePages(PAGE);
  const [paying, setPaying] = useState(false);

  const calendar = useCalendar();
  const months = useMemo(() => recentMonths(Date.now(), calendar), [calendar]);
  const window = month === '' ? null : monthQuery(month, calendar);
  const params = useMemo(
    () => ({
      ...(state === '' ? {} : { state: state as InvoiceState }),
      ...(currency === '' ? {} : { currency }),
      // The month's own bounds, so that the page is this month's invoices.
      // A neighbouring month's, written under another offset, come with
      // them and are narrowed out below, each by its own calendar.
      ...(window ? { since: window.since, until: window.until } : {}),
      limit: PAGE,
      ...cursorParam(pages.walked),
    }),
    [state, currency, window?.since, window?.until, pages.walked],
  );

  const invoices = useInvoices(params);
  const summary = useSummary();
  // The page as the engine sent it decides whether there is another one;
  // what is shown is the part of it that belongs to the month.
  const page = invoices.data ?? [];
  const after = cursorAfter(page, PAGE);
  const rows = month === '' ? page : page.filter((row) => periodInMonth(row, month));
  const today = { now: Date.now(), offset: calendar };
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
      width: 125,
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
      width: 180,
      sortValue: (row) => row.customer_name.toLowerCase(),
      cell: (row) => <span>{row.customer_name}</span>,
    },
    {
      id: 'period',
      header: 'Period',
      width: 205,
      sortValue: (row) => row.period_start,
      cell: (row) => <span className={styles.amount}>{invoicePeriod(row)}</span>,
    },
    {
      id: 'total',
      header: 'Total',
      width: 125,
      align: 'right',
      sortValue: (row) => moneySortKey(row.total),
      cell: (row) => <Amount money={row.total} />,
    },
    {
      id: 'paid',
      header: 'Paid',
      width: 130,
      align: 'right',
      sortValue: (row) => moneySortKey(row.paid),
      // What was credited is under what was paid, as its own figure and in
      // its own words — never added to it, because a write-off is not money
      // that arrived — so that total, less both, visibly is what is
      // outstanding. An invoice nothing was written off on shows no line.
      cell: (row) => (
        <span className={styles.amount}>
          <Amount money={row.paid} muted />
          {row.credited.minor !== 0 && (
            // In a column as narrow as one figure, the word goes under it
            // rather than off the edge ("100.00 USD credite").
            <span className={`${styles.rowSub} ${styles.figureThenWord}`}>
              <span className={styles.figure}>{formatMoney(row.credited)}</span> credited
            </span>
          )}
        </span>
      ),
    },
    {
      id: 'outstanding',
      header: 'Outstanding',
      width: 125,
      align: 'right',
      sortValue: (row) => moneySortKey(row.outstanding),
      cell: (row) => <Amount money={row.outstanding} />,
    },
    {
      id: 'status',
      header: 'Status',
      width: 105,
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
      width: 125,
      cell: (row) => (
        <span className={styles.rowSub}>
          {dueText(row, today)}
          {row.due_at !== null && (
            <span className={styles.rowSub}>{dayText(row.due_at, row.utc_offset_minutes)}</span>
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
            pages.reset();
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
            pages.reset();
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
            pages.reset();
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
              title={
                page.length > 0
                  ? // Only after the calendar was changed: every invoice the
                    // engine sent for this page was written for another month.
                    'No invoice on this page is in that month'
                  : state === '' && month === ''
                    ? 'No invoices yet'
                    : 'No invoice matches these filters'
              }
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
            {pageText(pages.first - 1, page.length, rows.length)}
          </span>
          <ToolbarSpacer />
          <Button
            size="sm"
            disabled={!pages.hasPrevious || invoices.isFetching}
            onClick={pages.previous}
          >
            Previous
          </Button>
          <Button
            size="sm"
            disabled={after === null || invoices.isFetching}
            onClick={() => {
              if (after !== null) pages.next(after);
            }}
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
