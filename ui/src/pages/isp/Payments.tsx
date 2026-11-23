/**
 * The payments: what came in, in the currency it came in, and what each one
 * settled.
 *
 * A payment is never deleted. One recorded in error is **voided** with its
 * own reason, and both records stay — which is also why an invoice with a
 * recorded payment cannot be voided until the payment is. A credit note is
 * not a payment and is not in this list: nothing was received.
 *
 * Three figures are shown and they are three different facts: the amount that
 * was handed over, the rate it was taken at (when the currencies differed,
 * exactly as it was quoted), and what it settled in the invoice's currency —
 * worked out once, when it was taken, and never recomputed.
 *
 * `isp.payments` carries the customer's id and not their name, so the names
 * come from a page of customers and a row shows the id when the name is not
 * in it (see `NOTES.md`).
 *
 * The list mixes currencies, so "Received" sorts within a currency and never
 * across them, and the day a payment is listed under is a day of the
 * business's calendar.
 *
 * The list is newest first, and Next continues after the last payment on
 * screen: payments recorded while somebody reads it sort in front, and a page
 * asked for by a count of rows would show some of this one again
 * (`paging.ts`). Its columns fit the smallest window the application
 * supports, State included.
 */

import { HandCoins } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Link } from 'react-router';
import type { Payment, PaymentState } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { Table, type TableColumn } from '@/design/Table';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { dayText } from './calendar';
import { useCalendar, useCurrencies, useCustomers, usePayments, useVoidPayment } from './data';
import { Amount, Words } from './Figures';
import { moneySortKey, rateText } from './money';
import { cursorAfter, cursorParam, usePages } from './paging';
import { PaymentDialog } from './PaymentDialog';
import styles from './isp.module.css';

const PAGE = 50;
/** How many customers are read to put names on the rows. */
const NAMES = 2_000;

export function Payments() {
  const [state, setState] = useState('');
  const pages = usePages(PAGE);
  const [recording, setRecording] = useState(false);
  const [voiding, setVoiding] = useState<Payment | null>(null);

  const params = useMemo(
    () => ({
      ...(state === '' ? {} : { state: state as PaymentState }),
      limit: PAGE,
      ...cursorParam(pages.walked),
    }),
    [state, pages.walked],
  );

  const payments = usePayments(params);
  const customers = useCustomers({ limit: NAMES });
  const currencies = useCurrencies();
  const calendar = useCalendar();
  const rows = payments.data ?? [];
  const after = cursorAfter(rows, PAGE);

  const names = useMemo(() => {
    const index = new Map<string, string>();
    for (const customer of customers.data?.customers ?? []) index.set(customer.id, customer.name);
    return index;
  }, [customers.data]);

  const columns: readonly TableColumn<Payment>[] = [
    {
      id: 'at',
      header: 'When',
      required: true,
      width: 110,
      sortValue: (row) => row.at,
      cell: (row) => <span className={styles.amount}>{dayText(row.at, calendar)}</span>,
    },
    {
      id: 'customer',
      header: 'Customer',
      width: 160,
      sortValue: (row) => names.get(row.customer_id) ?? row.customer_id,
      cell: (row) => (
        <Link to={`/isp/customer/${row.customer_id}`}>
          {names.get(row.customer_id) ?? row.customer_id}
        </Link>
      ),
    },
    {
      id: 'amount',
      header: 'Received',
      width: 125,
      align: 'right',
      sortValue: (row) => moneySortKey(row.amount),
      cell: (row) => <Amount money={row.amount} muted={row.state === 'void'} />,
    },
    {
      id: 'settled',
      header: 'Settled',
      width: 135,
      align: 'right',
      cell: (row) =>
        row.settled ? (
          <span className={styles.amount}>
            <Amount money={row.settled} muted={row.state === 'void'} />
            {!row.settled_exact && (
              <span className={styles.rowSub}>the conversion rounded minor units away</span>
            )}
          </span>
        ) : (
          <Words title="Real money, not yet against an invoice.">on account</Words>
        ),
    },
    {
      id: 'rate',
      header: 'Rate',
      width: 160,
      cell: (row) =>
        row.conversion ? (
          <span className={styles.rowSub}>
            {rateText(row.conversion, currencies.data ?? [])}
          </span>
        ) : (
          <Words>same currency</Words>
        ),
    },
    {
      id: 'method',
      header: 'How',
      width: 130,
      sortValue: (row) => row.method,
      cell: (row) => (
        <span>
          {row.method}
          {row.reference !== '' && <span className={styles.rowSub}>{row.reference}</span>}
        </span>
      ),
    },
    {
      id: 'state',
      header: 'State',
      width: 140,
      cell: (row) =>
        row.state === 'void' ? (
          <Badge tone="maint" variant="outline">
            Void: {row.void_reason}
          </Badge>
        ) : (
          <Badge tone="ok" variant="outline">
            Recorded
          </Badge>
        ),
    },
  ];

  if (payments.isError) {
    return (
      <div className={styles.scrollPane}>
        <ErrorState
          title="The payments could not be read"
          error={payments.error}
          onRetry={() => void payments.refetch()}
        />
      </div>
    );
  }

  return (
    <div className={styles.pane}>
      <Toolbar className={styles.toolbar}>
        <Select
          className={styles.filter}
          aria-label="Payment state"
          value={state}
          onChange={(event) => {
            setState(event.target.value);
            pages.reset();
          }}
          options={[
            { value: '', label: 'Recorded and void' },
            { value: 'recorded', label: 'Recorded' },
            { value: 'void', label: 'Void' },
          ]}
        />
        <ToolbarSpacer />
        <Button variant="primary" icon={HandCoins} onClick={() => setRecording(true)}>
          Record a payment…
        </Button>
      </Toolbar>

      <div className={styles.tableWrap}>
        <Table
          id="isp.payments"
          aria-label="Payments"
          columns={columns}
          rows={rows}
          rowKey={(row) => row.id}
          loading={payments.isPending}
          fill
          defaultSort={{ columnId: 'at', direction: 'desc' }}
          rowActions={(row) =>
            row.state === 'recorded'
              ? [{ label: 'Void…', tone: 'danger', onSelect: () => setVoiding(row) }]
              : []
          }
          empty={
            <EmptyState
              icon={HandCoins}
              size="compact"
              title="No payments recorded"
              description="A payment is what was actually received, and nothing else marks an invoice paid. An amount that was written off instead is a credit note, which is not listed here: it is not money."
              action={
                <Button variant="primary" icon={HandCoins} onClick={() => setRecording(true)}>
                  Record a payment…
                </Button>
              }
            />
          }
        />
        <div className={styles.paging}>
          <span className={styles.count}>
            {rows.length === 0
              ? 'nothing on this page'
              : `${formatCount(pages.first)}–${formatCount(pages.first - 1 + rows.length)}`}
          </span>
          <ToolbarSpacer />
          <Button
            size="sm"
            disabled={!pages.hasPrevious || payments.isFetching}
            onClick={pages.previous}
          >
            Previous
          </Button>
          <Button
            size="sm"
            disabled={after === null || payments.isFetching}
            onClick={() => {
              if (after !== null) pages.next(after);
            }}
          >
            Next
          </Button>
        </div>
      </div>

      <PaymentDialog open={recording} onClose={() => setRecording(false)} />
      <VoidPaymentDialog payment={voiding} onClose={() => setVoiding(null)} />
    </div>
  );
}

export function VoidPaymentDialog({
  payment,
  onClose,
}: {
  payment: Payment | null;
  onClose: () => void;
}) {
  const voidIt = useVoidPayment();
  const [reason, setReason] = useState('');

  return (
    <Dialog
      open={payment !== null}
      onClose={onClose}
      size="sm"
      title="Void this payment"
      description="A payment is never deleted. Voiding it keeps the record, with the reason, and puts what it settled back on the invoice."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="danger"
            loading={voidIt.isPending}
            disabled={reason.trim() === '' || payment === null}
            onClick={() => {
              if (!payment) return;
              voidIt.mutate(
                { id: payment.id, reason: reason.trim() },
                {
                  onSuccess: () => {
                    toast.success('The payment is void and the record is kept');
                    setReason('');
                    onClose();
                  },
                },
              );
            }}
          >
            Void it
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {voidIt.error && <p className={styles.error}>{voidIt.error.message}</p>}
        <Field label="Reason" required hint="This is what the trail will say happened.">
          <Textarea
            autoFocus
            rows={3}
            value={reason}
            onChange={(event) => setReason(event.target.value)}
            placeholder="Recorded twice"
          />
        </Field>
      </div>
    </Dialog>
  );
}
