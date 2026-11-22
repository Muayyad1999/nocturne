/**
 * One customer: who they are, what they were sold, what they owe, what they
 * have paid, and what they are actually using against what they pay for.
 *
 * `isp.customer.get` answers with all of it in one read — the customer, their
 * subscriptions with a monthly-equivalent figure, their last invoices with
 * the payments and credit notes against each, their balance per currency,
 * and their used-against-sold row. Three more reads sit beside it: their
 * payments (including money on account, which belongs to no invoice), their
 * credit notes (including the ones on the account) and their own aged
 * arrears.
 *
 * The balance is four figures and none is netted against another: what is
 * owed, money received on account, credit notes on the account (which is
 * not money and reduces nothing until a note is set against an invoice),
 * and what stands in their favour on invoices settled beyond their total.
 *
 * Every date here is a day of the business's calendar (`calendar.ts`), and a
 * date on an invoice is a day of the calendar that invoice was written in.
 *
 * The account state is changed from here, and what that does is said plainly
 * before it is done: it records a decision and sends nothing to a device.
 * There is no control on this page that offers to suspend the service itself,
 * because there is no such thing in Nocturne
 * (`crates/nocturne-isp/tests/boundaries.rs`).
 */

import { ArrowLeft, FilePlus2, HandCoins, Pencil, Plus, ReceiptText } from 'lucide-react';
import { useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router';
import type { CreditNote, Payment, SubscriptionRow } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { KeyValue } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { formatCount, plural } from '@/lib/format';
import { Arrears } from './Arrears';
import { dueText, invoicePeriod, statusWords } from './billing-rows';
import { dayText, dayTimeText, monthOf, previousMonth } from './calendar';
import {
  basisText,
  coverageWords,
  isMeasured,
  ofPlanText,
  ofPlanTone,
  rateText,
  soldText,
  usageText,
} from './coverage';
import { AllocateCreditDialog, CreditDialog, VoidCreditDialog } from './CreditDialogs';
import { CreditNoteList } from './CreditNoteList';
import { CustomerDialog } from './CustomerDialog';
import {
  useCalendar,
  useCredits,
  useCustomer,
  useDeviceNames,
  useDraftInvoice,
  useEndSubscription,
  useIspInstalled,
  usePayments,
  useSetCustomerState,
  useSettingsWatch,
} from './data';
import { Amount, Figures, Words } from './Figures';
import { InvoiceDrawer } from './InvoiceDrawer';
import { formatMoney } from './money';
import { PaymentDialog } from './PaymentDialog';
import { VoidPaymentDialog } from './Payments';
import { AGREED_NOTE, PERIOD_WORDS, STATE_WORDS, termsText } from './records';
import { SubscriptionDialog } from './SubscriptionDialog';
import styles from './isp.module.css';

const { label: areaLabel, path: areaPath } = destination('isp');

export default function CustomerRoute() {
  const installed = useIspInstalled();
  const { id } = useParams();
  if (installed === false) {
    return (
      <Page>
        <EmptyState
          icon={destination('isp').icon}
          title="The ISP edition is not in this build"
          description="This copy of Nocturne was assembled without the subscriber and billing feature."
        />
      </Page>
    );
  }
  return <Customer id={id ?? ''} />;
}

function Customer({ id }: { id: string }) {
  const navigate = useNavigate();
  const detail = useCustomer(id === '' ? null : id, 50);
  const payments = usePayments(id === '' ? { limit: 1 } : { customer_id: id, limit: 200 });
  const credits = useCredits({ customer_id: id, limit: 200 }, id !== '');
  const calendar = useCalendar();
  useSettingsWatch();
  const devices = useDeviceNames();
  const setState = useSetCustomerState();
  const draft = useDraftInvoice();
  const endPlan = useEndSubscription();

  const [editing, setEditing] = useState(false);
  const [selling, setSelling] = useState<{ open: boolean; row: SubscriptionRow | null }>({
    open: false,
    row: null,
  });
  const [paying, setPaying] = useState(false);
  const [crediting, setCrediting] = useState(false);
  const [voiding, setVoiding] = useState<Payment | null>(null);
  const [voidingCredit, setVoidingCredit] = useState<CreditNote | null>(null);
  const [allocating, setAllocating] = useState<CreditNote | null>(null);
  const [openInvoice, setOpenInvoice] = useState<string | null>(null);

  const customer = detail.data?.customer;
  usePageHeader({
    title: customer?.name ?? 'Customer',
    breadcrumbs: [{ label: areaLabel, to: `${areaPath}/customers` }],
  });

  if (detail.isError) {
    return (
      <Page>
        <ErrorState
          title="That customer could not be read"
          error={detail.error}
          onRetry={() => void detail.refetch()}
          actions={
            <Button icon={ArrowLeft} onClick={() => void navigate(`${areaPath}/customers`)}>
              Back to the customers
            </Button>
          }
        />
      </Page>
    );
  }

  if (!detail.data || !customer) {
    return (
      <Page>
        <Skeleton shape="block" height={40} width={360} />
        <Skeleton shape="block" height={200} />
        <Skeleton shape="block" height={280} />
      </Page>
    );
  }

  const { balance, subscriptions, invoices, usage } = detail.data;
  const state = STATE_WORDS[customer.state];

  // Last whole month of the business's calendar: a window has to be one
  // whole period of a subscription's kind or nothing is charged for it. The
  // month is named and the engine works out where it begins and ends.
  const lastMonth = calendar === null ? null : previousMonth(monthOf(Date.now(), calendar));
  const today = { now: Date.now(), offset: calendar };

  const draftLastMonth = () => {
    if (lastMonth === null) return;
    draft.mutate(
      { customer_id: customer.id, month: lastMonth },
      {
        onSuccess: (drafted) => {
          setOpenInvoice(drafted.invoice.id);
          toast.success(`A draft for ${formatMoney(drafted.invoice.total)} is ready`, {
            description:
              drafted.skipped.length > 0
                ? `${plural(drafted.skipped.length, 'subscription')} not charged; the draft says why.`
                : undefined,
          });
        },
      },
    );
  };

  return (
    <Page flush>
      <Toolbar className={styles.toolbar}>
        <Button icon={ArrowLeft} onClick={() => void navigate(`${areaPath}/customers`)}>
          Customers
        </Button>
        <ToolbarSpacer />
        <Button icon={Pencil} onClick={() => setEditing(true)}>
          Edit details…
        </Button>
        <Button icon={Plus} onClick={() => setSelling({ open: true, row: null })}>
          Sell a plan…
        </Button>
        <Button
          icon={FilePlus2}
          loading={draft.isPending}
          disabled={lastMonth === null}
          onClick={draftLastMonth}
        >
          Draft last month's invoice
        </Button>
        <Button icon={ReceiptText} onClick={() => setCrediting(true)}>
          Issue a credit note…
        </Button>
        <Button variant="primary" icon={HandCoins} onClick={() => setPaying(true)}>
          Record a payment…
        </Button>
      </Toolbar>

      <div className={styles.detail}>
        {/* The header already carries the name, with the breadcrumb back to
            the list, so this line is what the name does not say: the account
            number, the state and the currency everything of theirs is in. */}
        <div className={styles.identity}>
          {customer.code !== '' && <span className={styles.identityCode}>{customer.code}</span>}
          <Badge tone={state.tone} variant="outline">
            {state.label}
          </Badge>
          <span className={styles.identityCode}>
            billed in {customer.currency}
            {customer.service.tower !== null && ` · ${customer.service.tower}`}
            {customer.site !== '' && ` · ${customer.site}`}
          </span>
        </div>
        <p className={styles.quiet}>{state.meaning}</p>

        <div className={styles.columns}>
          <Panel title="Details" padding="none">
            <div className={styles.panelBody}>
              <KeyValue
                items={[
                  { label: 'Phone', value: customer.contact.phone || <Words>not recorded</Words> },
                  { label: 'Email', value: customer.contact.email || <Words>not recorded</Words> },
                  {
                    label: 'Address',
                    value: customer.contact.address || <Words>not recorded</Words>,
                  },
                  { label: 'Site', value: customer.site || <Words>not recorded</Words> },
                  {
                    label: 'Tower',
                    value: customer.service.tower ?? <Words>not recorded</Words>,
                  },
                  {
                    label: 'Served by',
                    value:
                      customer.service.device_id === null ? (
                        <Words title="Without a device and an interface they have no usage figure at all — not a usage of zero.">
                          no device recorded
                        </Words>
                      ) : (
                        // The inventory's name for it, when this build has the
                        // inventory; the id is what the record holds.
                        <Link to={`/devices/${customer.service.device_id}`}>
                          {devices.find((device) => device.id === customer.service.device_id)
                            ?.name ?? customer.service.device_id}
                        </Link>
                      ),
                    mono: true,
                  },
                  {
                    label: 'Interface',
                    value: customer.service.interface ?? <Words>no interface recorded</Words>,
                    mono: true,
                  },
                  {
                    label: 'Where',
                    value: customer.service.location || <Words>not recorded</Words>,
                  },
                  {
                    label: 'Tags',
                    value:
                      customer.tags.length === 0 ? (
                        <Words>none</Words>
                      ) : (
                        <ul className={styles.chips}>
                          {customer.tags.map((tag) => (
                            <li key={tag}>
                              <Badge>{tag}</Badge>
                            </li>
                          ))}
                        </ul>
                      ),
                  },
                  { label: 'Added', value: dayTimeText(customer.created_at, calendar) },
                ]}
              />
              {customer.notes !== '' && <p className={styles.note}>{customer.notes}</p>}
              <div className={styles.inlineGroup}>
                {(['active', 'suspended', 'cancelled'] as const)
                  .filter((next) => next !== customer.state)
                  .map((next) => (
                    <Button
                      key={next}
                      size="sm"
                      loading={setState.isPending && setState.variables?.state === next}
                      onClick={() => {
                        void (async () => {
                          const ok = await confirm({
                            title: `Mark ${customer.name} ${STATE_WORDS[next].label.toLowerCase()}?`,
                            description: STATE_WORDS[next].consequence,
                            confirmLabel: `Mark ${STATE_WORDS[next].label.toLowerCase()}`,
                          });
                          if (!ok) return;
                          setState.mutate(
                            { id: customer.id, state: next },
                            { onSuccess: (change) => toast.success(change.note) },
                          );
                        })();
                      }}
                    >
                      Mark {STATE_WORDS[next].label.toLowerCase()}
                    </Button>
                  ))}
              </div>
            </div>
          </Panel>

          <Panel title="What they owe" padding="none">
            <div className={styles.panelBody}>
              <div className={styles.stats}>
                <div className={styles.statGroup}>
                  <span className={styles.statLabel}>Outstanding</span>
                  <Figures totals={balance.outstanding} empty="nothing is owed" />
                  <span className={styles.quiet}>
                    {plural(balance.invoices_outstanding, 'issued invoice')} still short
                  </span>
                </div>
                <div className={styles.statGroup}>
                  <span className={styles.statLabel}>On account</span>
                  <Figures totals={balance.on_account} empty="nothing held on account" />
                  <span className={styles.quiet}>received, not yet against an invoice</span>
                </div>
                <div className={styles.statGroup}>
                  <span className={styles.statLabel}>Credit on account</span>
                  <Figures totals={balance.credit_on_account} empty="no credit note on the account" />
                  <span className={styles.quiet}>
                    credit notes, not money: it reduces nothing until a note is set against an
                    invoice
                  </span>
                </div>
                <div className={styles.statGroup}>
                  <span className={styles.statLabel}>In their favour</span>
                  <Figures totals={balance.overpaid} empty="nothing overpaid" />
                  {balance.invoices_overpaid > 0 && (
                    <span className={styles.quiet}>
                      {plural(balance.invoices_overpaid, 'invoice')} settled beyond{' '}
                      {balance.invoices_overpaid === 1 ? 'its total' : 'their totals'}; not taken
                      off what they owe
                    </span>
                  )}
                </div>
                <div className={styles.statGroup}>
                  <span className={styles.statLabel}>Oldest due</span>
                  {balance.oldest_due_at === null ? (
                    <Words>nothing owed has a due date</Words>
                  ) : (
                    <span className={styles.statValue}>
                      {dayText(balance.oldest_due_at, calendar)}
                    </span>
                  )}
                </div>
              </div>
              <p className={styles.quiet}>{balance.note}</p>
              <Arrears customerId={customer.id} compact />
            </div>
          </Panel>
        </div>

        <Panel
          title="What they were sold"
          padding="none"
          actions={
            <Button size="sm" icon={Plus} onClick={() => setSelling({ open: true, row: null })}>
              Sell a plan
            </Button>
          }
        >
          <div className={styles.panelBody}>
            <table className={styles.lines}>
              <thead>
                <tr>
                  <th>Plan</th>
                  <th>Agreed</th>
                  <th>As a month</th>
                  <th>Running</th>
                  <th aria-label="Actions" />
                </tr>
              </thead>
              <tbody>
                {subscriptions.length === 0 && (
                  <tr>
                    <td colSpan={5}>
                      <Words>
                        no plan has been sold to them — which is not a plan of zero megabits
                      </Words>
                    </td>
                  </tr>
                )}
                {subscriptions.map((row) => (
                  <tr key={row.id}>
                    <td>
                      <span className={styles.lineDescription}>{row.tariff_name}</span>
                      <span className={styles.lineSubtitle}>{termsText(row)}</span>
                      {row.amends !== null && (
                        <span className={styles.lineSubtitle}>amends an earlier agreement</span>
                      )}
                      {row.note !== '' && <span className={styles.lineSubtitle}>{row.note}</span>}
                    </td>
                    <td>
                      <Amount money={row.price} />
                      <span className={styles.lineSubtitle}>
                        {PERIOD_WORDS[row.period]}
                        {/* The unit price is the same figure as the agreed one
                            on a flat plan, and worth stating on a per-megabit
                            one, where the sum is price × megabits. */}
                        {row.unit === 'per_mbit' &&
                          ` · ${formatCount(row.mbit)} × ${formatMoney(row.unit_price)} per megabit`}
                      </span>
                    </td>
                    <td>
                      <Amount money={row.monthly_price} />
                      {!row.monthly_exact && (
                        <span className={styles.lineSubtitle}>
                          the division into months rounded
                        </span>
                      )}
                    </td>
                    <td>
                      {row.state === 'active' ? (
                        <Badge tone="ok" variant="outline">
                          since {dayText(row.started_at, calendar)}
                        </Badge>
                      ) : (
                        <Badge tone="idle" variant="outline">
                          ended {dayText(row.ended_at, calendar)}
                        </Badge>
                      )}
                    </td>
                    <td>
                      {row.state === 'active' && (
                        <span className={styles.inlineGroup}>
                          <Button size="sm" onClick={() => setSelling({ open: true, row })}>
                            Amend…
                          </Button>
                          <Button
                            size="sm"
                            onClick={() => {
                              void (async () => {
                                const ok = await confirm({
                                  title: `End the ${row.tariff_name} plan?`,
                                  description:
                                    'The subscription stops being billed from the date it ends. Invoices already issued on it are untouched, and nothing is sent to a device.',
                                  confirmLabel: 'End it',
                                });
                                if (!ok) return;
                                endPlan.mutate(
                                  { id: row.id },
                                  {
                                    onSuccess: () =>
                                      toast.success(`The ${row.tariff_name} plan has ended`),
                                  },
                                );
                              })();
                            }}
                          >
                            End…
                          </Button>
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <p className={styles.quiet}>{AGREED_NOTE}</p>
          </div>
        </Panel>

        <Panel title="What they are using against what they pay for" padding="none">
          <div className={styles.panelBody}>
            <div className={styles.stats}>
              <div className={styles.statGroup}>
                <span className={styles.statLabel}>Sold</span>
                {usage.sold_mbit === null ? (
                  <Words>{soldText(null)}</Words>
                ) : (
                  <span className={styles.statValue}>{soldText(usage.sold_mbit)}</span>
                )}
                <span className={styles.quiet}>
                  {plural(usage.subscriptions, 'active plan')}
                </span>
              </div>
              <div className={styles.statGroup}>
                <span className={styles.statLabel}>Using</span>
                {isMeasured(usage.coverage) && usage.used ? (
                  <>
                    <span className={styles.statValue}>{usageText(usage)}</span>
                    <span className={styles.quiet}>{basisText(usage.used)}</span>
                  </>
                ) : (
                  <>
                    <Words>{coverageWords(usage.coverage).label}</Words>
                    <span className={styles.quiet}>{usage.coverage_note}</span>
                  </>
                )}
              </div>
              <div className={styles.statGroup}>
                <span className={styles.statLabel}>Of the plan</span>
                {usage.of_plan_pct === null ? (
                  <Words>{ofPlanText(null)}</Words>
                ) : (
                  <Badge tone={ofPlanTone(usage.of_plan_pct)} variant="outline">
                    {ofPlanText(usage.of_plan_pct)}
                  </Badge>
                )}
                {usage.used?.peak_mbit != null && (
                  <span className={styles.quiet}>peak {rateText(usage.used.peak_mbit)}</span>
                )}
              </div>
            </div>
          </div>
        </Panel>

        <Panel title="Invoices" padding="none">
          <div className={styles.panelBody}>
            <table className={styles.lines}>
              <thead>
                <tr>
                  <th>Invoice</th>
                  <th>Period</th>
                  <th>Total</th>
                  <th>Outstanding</th>
                  <th>Status</th>
                </tr>
              </thead>
              <tbody>
                {invoices.length === 0 && (
                  <tr>
                    <td colSpan={5}>
                      <Words>nothing has been invoiced to them yet</Words>
                    </td>
                  </tr>
                )}
                {invoices.map((invoice) => (
                  <tr key={invoice.id}>
                    <td>
                      <button
                        type="button"
                        className={styles.rowLink}
                        onClick={() => setOpenInvoice(invoice.id)}
                      >
                        {invoice.number ?? 'a draft'}
                      </button>
                      <span className={styles.lineSubtitle}>
                        {plural(invoice.lines.length, 'line')} ·{' '}
                        {plural(invoice.payments.length, 'payment')}
                        {invoice.credits.length > 0 &&
                          ` · ${plural(invoice.credits.length, 'credit note')}`}
                      </span>
                    </td>
                    <td>
                      <span className={styles.amount}>{invoicePeriod(invoice)}</span>
                    </td>
                    <td>
                      <Amount money={invoice.total} />
                    </td>
                    <td>
                      <Amount money={invoice.outstanding} />
                    </td>
                    <td>
                      <Badge tone={statusWords(invoice.status).tone} variant="outline">
                        {statusWords(invoice.status).label}
                      </Badge>
                      <span className={styles.lineSubtitle}>{dueText(invoice, today)}</span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <p className={styles.quiet}>
              The newest {formatCount(invoices.length)} of theirs. A status is worked out from the
              recorded payments, the credit notes and the business’s calendar: a payment or a
              credit note moves it, and nothing else does.
            </p>
          </div>
        </Panel>

        <Panel
          title="Credit notes"
          padding="none"
          actions={
            <Button size="sm" icon={Plus} onClick={() => setCrediting(true)}>
              Issue a credit note
            </Button>
          }
        >
          <div className={styles.panelBody}>
            {credits.isError ? (
              <ErrorState
                size="compact"
                title="Their credit notes could not be read"
                error={credits.error}
                onRetry={() => void credits.refetch()}
              />
            ) : credits.isPending ? (
              <Skeleton shape="block" height={60} />
            ) : (
              <CreditNoteList
                credits={credits.data}
                calendar={calendar}
                standing
                invoiceNumber={(invoiceId) =>
                  invoices.find((invoice) => invoice.id === invoiceId)?.number
                }
                onOpenInvoice={setOpenInvoice}
                onVoid={setVoidingCredit}
                onAllocate={setAllocating}
                empty="nothing has been credited to them — no amount they owed has been written off"
              />
            )}
            <p className={styles.quiet}>
              A credit note is an amount they no longer owe, with its reason: a document, not
              money. Nothing here records a refund.
            </p>
          </div>
        </Panel>

        <Panel title="Payments" padding="none">
          <div className={styles.panelBody}>
            <table className={styles.lines}>
              <thead>
                <tr>
                  <th>Received</th>
                  <th>Settles</th>
                  <th aria-label="Actions" />
                </tr>
              </thead>
              <tbody>
                {(payments.data ?? []).length === 0 && (
                  <tr>
                    <td colSpan={3}>
                      <Words>nothing has been received from them yet</Words>
                    </td>
                  </tr>
                )}
                {(payments.data ?? []).map((payment) => (
                  <tr key={payment.id}>
                    <td>
                      <span className={styles.lineDescription}>
                        {formatMoney(payment.amount)} by {payment.method}
                      </span>
                      <span className={styles.lineSubtitle}>
                        {dayText(payment.at, calendar)}
                        {payment.reference === '' ? '' : ` · ${payment.reference}`}
                        {payment.state === 'void' ? ` · void: ${payment.void_reason}` : ''}
                      </span>
                    </td>
                    <td>
                      {payment.settled ? (
                        <Amount money={payment.settled} muted={payment.state === 'void'} />
                      ) : (
                        <Words title="Real money, not yet against an invoice.">on account</Words>
                      )}
                      {payment.conversion && (
                        <span className={styles.lineSubtitle}>
                          at 1 {payment.conversion.from} = {payment.conversion.rate} minor units of{' '}
                          {payment.conversion.to}, as it was quoted
                        </span>
                      )}
                    </td>
                    <td>
                      {payment.state === 'recorded' && (
                        <Button size="sm" onClick={() => setVoiding(payment)}>
                          Void…
                        </Button>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </Panel>
      </div>

      <CustomerDialog open={editing} customer={customer} onClose={() => setEditing(false)} />
      <SubscriptionDialog
        open={selling.open}
        subscription={selling.row}
        customerId={customer.id}
        currency={customer.currency}
        onClose={() => setSelling({ open: false, row: null })}
      />
      <PaymentDialog
        open={paying}
        onClose={() => setPaying(false)}
        customerId={customer.id}
        defaultCurrency={customer.currency}
      />
      <VoidPaymentDialog payment={voiding} onClose={() => setVoiding(null)} />
      <CreditDialog
        open={crediting}
        onClose={() => setCrediting(false)}
        customerId={customer.id}
        currency={customer.currency}
      />
      <VoidCreditDialog credit={voidingCredit} onClose={() => setVoidingCredit(null)} />
      <AllocateCreditDialog credit={allocating} onClose={() => setAllocating(null)} />
      <InvoiceDrawer invoiceId={openInvoice} onClose={() => setOpenInvoice(null)} />
    </Page>
  );
}
