/**
 * The credit notes: every amount a customer stopped owing, with its reason.
 *
 * A credit note is a document, not a correction and not money — it has its
 * own `CN-` number, it is never deleted, and it appears in no figure of what
 * was received. This list is where somebody answers "why does this customer
 * owe less than they were invoiced": each row says how much, why, when it
 * was agreed, and where it stands — against one invoice, or on the account,
 * where it reduces nothing until it is set against one.
 *
 * `isp.credits` carries the customer's and the invoice's ids and not their
 * name or number. The names come from a page of customers, as on the
 * payments list; an invoice's number is read for the rows on screen
 * ({@link InvoiceRef}), which is at most one page of them (see `NOTES.md`).
 *
 * The list mixes currencies, so the amount sorts within a currency and never
 * across them.
 */

import { ReceiptText } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Link } from 'react-router';
import type { CreditNote, CreditState } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Select } from '@/design/Select';
import { Table, type TableColumn } from '@/design/Table';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { dayText } from './calendar';
import { AllocateCreditDialog, CreditDialog, VoidCreditDialog } from './CreditDialogs';
import { CREDIT_NOTE_MEANING, creditActions, creditStateWords, standingText } from './credit-notes';
import { useCalendar, useCredits, useCustomers, useInvoice } from './data';
import { Amount, Words } from './Figures';
import { InvoiceDrawer } from './InvoiceDrawer';
import { moneySortKey } from './money';
import styles from './isp.module.css';

const PAGE = 50;
/** How many customers are read to put names on the rows. */
const NAMES = 2_000;

export function Credits() {
  const [state, setState] = useState('');
  const [offset, setOffset] = useState(0);
  const [issuing, setIssuing] = useState(false);
  const [voiding, setVoiding] = useState<CreditNote | null>(null);
  const [allocating, setAllocating] = useState<CreditNote | null>(null);
  const [openInvoice, setOpenInvoice] = useState<string | null>(null);

  const params = useMemo(
    () => ({
      ...(state === '' ? {} : { state: state as CreditState }),
      limit: PAGE,
      ...(offset === 0 ? {} : { cursor: String(offset) }),
    }),
    [state, offset],
  );

  const credits = useCredits(params);
  const customers = useCustomers({ limit: NAMES });
  const calendar = useCalendar();
  const rows = credits.data ?? [];

  const names = useMemo(() => {
    const index = new Map<string, string>();
    for (const customer of customers.data?.customers ?? []) index.set(customer.id, customer.name);
    return index;
  }, [customers.data]);

  const columns: readonly TableColumn<CreditNote>[] = [
    {
      id: 'number',
      header: 'Credit note',
      required: true,
      width: 150,
      sortValue: (row) => row.number,
      cell: (row) => <span className={styles.amount}>{row.number}</span>,
    },
    {
      id: 'at',
      header: 'Agreed',
      width: 120,
      sortValue: (row) => row.at,
      cell: (row) => <span className={styles.amount}>{dayText(row.at, calendar)}</span>,
    },
    {
      id: 'customer',
      header: 'Customer',
      width: 180,
      sortValue: (row) => names.get(row.customer_id) ?? row.customer_id,
      cell: (row) => (
        <Link to={`/isp/customer/${row.customer_id}`}>
          {names.get(row.customer_id) ?? row.customer_id}
        </Link>
      ),
    },
    {
      id: 'amount',
      header: 'Writes off',
      width: 140,
      align: 'right',
      sortValue: (row) => moneySortKey(row.amount),
      cell: (row) => <Amount money={row.amount} muted={row.state === 'void'} />,
    },
    {
      id: 'standing',
      header: 'Stands',
      // Wide enough for the whole sentence about a note on the account:
      // cut short, "on the account" reads as a balance somebody can spend.
      width: 400,
      cell: (row) =>
        row.invoice_id === null ? (
          <Words title="Not money, and not yet a reduction of anything: set it against an invoice to reduce what that invoice says is outstanding.">
            {standingText(row)}
          </Words>
        ) : (
          <InvoiceRef credit={row} onOpen={setOpenInvoice} />
        ),
    },
    {
      id: 'reason',
      header: 'Reason',
      width: 220,
      cell: (row) => (
        <span>
          {row.reason}
          {row.note !== '' && <span className={styles.rowSub}>{row.note}</span>}
        </span>
      ),
    },
    {
      id: 'state',
      header: 'State',
      width: 170,
      sortValue: (row) => row.state,
      cell: (row) => (
        <Badge tone={creditStateWords(row).tone} variant="outline">
          {row.state === 'void' ? `Void: ${row.void_reason}` : creditStateWords(row).label}
        </Badge>
      ),
    },
  ];

  if (credits.isError) {
    return (
      <div className={styles.scrollPane}>
        <ErrorState
          title="The credit notes could not be read"
          error={credits.error}
          onRetry={() => void credits.refetch()}
        />
      </div>
    );
  }

  return (
    <div className={styles.pane}>
      <Toolbar className={styles.toolbar}>
        <Select
          className={styles.filter}
          aria-label="Credit note state"
          value={state}
          onChange={(event) => {
            setState(event.target.value);
            setOffset(0);
          }}
          options={[
            { value: '', label: 'Issued and void' },
            { value: 'issued', label: 'Issued' },
            { value: 'void', label: 'Void' },
          ]}
        />
        <ToolbarSpacer />
        <Button variant="primary" icon={ReceiptText} onClick={() => setIssuing(true)}>
          Issue a credit note…
        </Button>
      </Toolbar>

      <div className={styles.tableWrap}>
        <Table
          id="isp.credits"
          aria-label="Credit notes"
          columns={columns}
          rows={rows}
          rowKey={(row) => row.id}
          loading={credits.isPending}
          fill
          defaultSort={{ columnId: 'at', direction: 'desc' }}
          rowActions={(row) => {
            const actions = creditActions(row);
            return [
              ...(actions.allocate
                ? [{ label: 'Set against an invoice…', onSelect: () => setAllocating(row) }]
                : []),
              ...(actions.void
                ? [{ label: 'Void…', tone: 'danger' as const, onSelect: () => setVoiding(row) }]
                : []),
            ];
          }}
          empty={
            <EmptyState
              icon={ReceiptText}
              size="compact"
              title={state === '' ? 'No credit notes' : 'No credit note is in that state'}
              description={CREDIT_NOTE_MEANING}
              action={
                <Button variant="primary" icon={ReceiptText} onClick={() => setIssuing(true)}>
                  Issue a credit note…
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
            disabled={offset === 0 || credits.isFetching}
            onClick={() => setOffset(Math.max(0, offset - PAGE))}
          >
            Previous
          </Button>
          <Button
            size="sm"
            disabled={rows.length < PAGE || credits.isFetching}
            onClick={() => setOffset(offset + PAGE)}
          >
            Next
          </Button>
        </div>
      </div>

      <CreditDialog open={issuing} onClose={() => setIssuing(false)} />
      <VoidCreditDialog credit={voiding} onClose={() => setVoiding(null)} />
      <AllocateCreditDialog credit={allocating} onClose={() => setAllocating(null)} />
      <InvoiceDrawer invoiceId={openInvoice} onClose={() => setOpenInvoice(null)} />
    </div>
  );
}

/**
 * Where a note that stands against an invoice stands, by the invoice's
 * number — which the note does not carry, so it is read. Until it has been,
 * the cell says "an invoice" rather than showing an id nobody quotes.
 */
function InvoiceRef({ credit, onOpen }: { credit: CreditNote; onOpen: (id: string) => void }) {
  const invoice = useInvoice(credit.invoice_id);
  return (
    <button
      type="button"
      className={styles.rowLink}
      onClick={(event) => {
        event.stopPropagation();
        if (credit.invoice_id) onOpen(credit.invoice_id);
      }}
    >
      {standingText(credit, invoice.data?.number)}
    </button>
  );
}
