/**
 * One invoice: its lines, its settlement, its payments, and the four things
 * that may be done to it.
 *
 * The lifecycle is the backend's and this drawer only offers what the
 * document's state allows (`billing-rows.ts: invoiceActions`), because every
 * other button could only produce a `conflict`:
 *
 * * a **draft** can have lines added and removed, and can be issued;
 * * an **issued** invoice is frozen — no line, no total, no currency ever
 *   changes again — and can take payments;
 * * a wrong invoice is **voided with a reason** and **reissued**, which
 *   leaves the void original readable, keeps its number, and hands back a
 *   fresh draft that `replaces` it, so the trail shows what happened;
 * * an invoice with a payment recorded against it cannot be voided at all,
 *   and the reason is said here rather than found out by trying.
 *
 * Nothing marks an invoice paid. `status`, `paid` and `outstanding` are
 * derived from the recorded payments at read time, so the only way to move
 * them is to record a payment.
 */

import { Ban, Plus, RefreshCw, Send, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import type { InvoiceView } from '@/api/types/isp';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { KeyValue } from '@/design/KeyValue';
import { Skeleton } from '@/design/Skeleton';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { formatDate, formatDateTime } from '@/lib/time';
import { AmountField } from './AmountField';
import {
  dayStart,
  invoiceActions,
  lineSubtitle,
  overdueText,
  statusWords,
  voidBlockedReason,
} from './billing-rows';
import {
  useAddLine,
  useCurrencies,
  useInvoice,
  useIssueInvoice,
  useRemoveLine,
  useReissueInvoice,
  useVoidInvoice,
} from './data';
import { Amount, Words } from './Figures';
import { digitsOf, formatMoney, parseAmount, rateText } from './money';
import { PaymentDialog } from './PaymentDialog';
import styles from './isp.module.css';

export interface InvoiceDrawerProps {
  invoiceId: string | null;
  onClose: () => void;
}

export function InvoiceDrawer({ invoiceId, onClose }: InvoiceDrawerProps) {
  const invoice = useInvoice(invoiceId);
  const [paying, setPaying] = useState(false);

  return (
    <Drawer
      open={invoiceId !== null}
      onClose={onClose}
      size="lg"
      title={invoice.data?.number ?? (invoice.isPending ? 'Invoice' : 'Draft invoice')}
      description={
        invoice.data
          ? `${invoice.data.customer_name} · ${formatDate(invoice.data.period_start)} to ${formatDate(
              invoice.data.period_end,
            )}`
          : undefined
      }
    >
      {invoice.isError && (
        <ErrorState
          size="compact"
          title="That invoice could not be read"
          error={invoice.error}
          onRetry={() => void invoice.refetch()}
        />
      )}
      {invoice.isPending && <Skeleton shape="block" height={280} />}
      {invoice.data && (
        <Body invoice={invoice.data} onClose={onClose} onPay={() => setPaying(true)} />
      )}
      {invoice.data && (
        <PaymentDialog
          open={paying}
          onClose={() => setPaying(false)}
          customerId={invoice.data.customer_id}
          invoice={invoice.data}
        />
      )}
    </Drawer>
  );
}

type Asking = 'none' | 'issue' | 'void' | 'reissue' | 'line';

function Body({
  invoice,
  onClose,
  onPay,
}: {
  invoice: InvoiceView;
  onClose: () => void;
  onPay: () => void;
}) {
  const [asking, setAsking] = useState<Asking>('none');
  const actions = invoiceActions(invoice);
  const blocked = voidBlockedReason(invoice);
  const removeLine = useRemoveLine();
  const words = statusWords(invoice.status);

  return (
    <div className={styles.sections}>
      <div className={styles.inlineGroup}>
        <Badge tone={words.tone} variant="outline">
          {words.label}
        </Badge>
        <span className={styles.quiet}>{words.meaning}</span>
      </div>

      <KeyValue
        items={[
          {
            label: 'Customer',
            value: (
              <Link to={`/isp/customer/${invoice.customer_id}`} onClick={onClose}>
                {invoice.customer_name}
              </Link>
            ),
          },
          {
            label: 'Period',
            value: `${formatDate(invoice.period_start)} → ${formatDate(invoice.period_end)}`,
            mono: true,
          },
          {
            label: 'Issued',
            value:
              invoice.issued_at === null ? (
                <Words>not issued yet</Words>
              ) : (
                formatDateTime(invoice.issued_at)
              ),
            mono: invoice.issued_at !== null,
          },
          {
            label: 'Due',
            value:
              invoice.due_at === null ? (
                <Words>no due date</Words>
              ) : (
                `${formatDate(invoice.due_at)} · ${overdueText(invoice)}`
              ),
          },
          { label: 'Total', value: <Amount money={invoice.total} /> },
          { label: 'Paid', value: <Amount money={invoice.paid} muted /> },
          { label: 'Outstanding', value: <Amount money={invoice.outstanding} /> },
          ...(invoice.state === 'void'
            ? [
                {
                  label: 'Voided',
                  value: `${formatDateTime(invoice.voided_at)} — ${invoice.void_reason}`,
                },
              ]
            : []),
          ...(invoice.replaces
            ? [
                {
                  label: 'Replaces',
                  value: <code>{invoice.replaces}</code>,
                },
              ]
            : []),
          ...(invoice.replaced_by
            ? [
                {
                  label: 'Replaced by',
                  value: <code>{invoice.replaced_by}</code>,
                },
              ]
            : []),
        ]}
      />

      <table className={styles.lines}>
        <thead>
          <tr>
            <th>Line</th>
            <th>Amount</th>
            {actions.edit && <th aria-label="Remove" />}
          </tr>
        </thead>
        <tbody>
          {invoice.lines.length === 0 && (
            <tr>
              <td colSpan={actions.edit ? 3 : 2}>
                <Words>
                  no lines — nothing is charged for on this invoice yet
                </Words>
              </td>
            </tr>
          )}
          {invoice.lines.map((line) => (
            <tr key={line.id}>
              <td>
                <span className={styles.lineDescription}>{line.description}</span>
                {lineSubtitle(line, formatMoney) !== '' && (
                  <span className={styles.lineSubtitle}>{lineSubtitle(line, formatMoney)}</span>
                )}
                {line.full_period_price && line.prorated && (
                  <span className={styles.lineSubtitle}>
                    a whole period at these terms is {formatMoney(line.full_period_price)}
                  </span>
                )}
              </td>
              <td>
                <Amount money={line.amount} />
              </td>
              {actions.edit && (
                <td>
                  <Button
                    size="sm"
                    icon={Trash2}
                    aria-label={`Remove ${line.description}`}
                    loading={removeLine.isPending}
                    onClick={() =>
                      removeLine.mutate({ invoice_id: invoice.id, line_id: line.id })
                    }
                  />
                </td>
              )}
            </tr>
          ))}
        </tbody>
        <tfoot>
          <tr>
            <td>Total</td>
            <td>
              <Amount money={invoice.total} />
            </td>
            {actions.edit && <td />}
          </tr>
        </tfoot>
      </table>

      <Payments invoice={invoice} />

      {blocked && <p className={styles.warning}>{blocked}</p>}

      <div className={styles.inlineGroup}>
        {actions.edit && (
          <Button icon={Plus} onClick={() => setAsking('line')}>
            Add a line…
          </Button>
        )}
        {actions.issue && (
          <Button variant="primary" icon={Send} onClick={() => setAsking('issue')}>
            Issue…
          </Button>
        )}
        {actions.pay && (
          <Button variant="primary" onClick={onPay}>
            Record a payment…
          </Button>
        )}
        {actions.reissue && (
          <Button icon={RefreshCw} onClick={() => setAsking('reissue')}>
            Void and reissue…
          </Button>
        )}
        {actions.void && (
          <Button variant="danger" icon={Ban} onClick={() => setAsking('void')}>
            Void…
          </Button>
        )}
      </div>

      <IssueDialog
        open={asking === 'issue'}
        invoice={invoice}
        onClose={() => setAsking('none')}
      />
      <VoidDialog
        open={asking === 'void' || asking === 'reissue'}
        reissue={asking === 'reissue'}
        invoice={invoice}
        onClose={() => setAsking('none')}
      />
      <LineDialog open={asking === 'line'} invoice={invoice} onClose={() => setAsking('none')} />
    </div>
  );
}

function Payments({ invoice }: { invoice: InvoiceView }) {
  const currencies = useCurrencies();
  if (invoice.payments.length === 0) {
    return (
      <p className={styles.note}>
        <Words>
          no payment is recorded against this invoice — nothing but a recorded payment can move
          what it says is outstanding
        </Words>
      </p>
    );
  }
  return (
    <table className={styles.lines}>
      <thead>
        <tr>
          <th>Payment</th>
          <th>Settles</th>
        </tr>
      </thead>
      <tbody>
        {invoice.payments.map((payment) => (
          <tr key={payment.id}>
            <td>
              <span className={styles.lineDescription}>
                {formatMoney(payment.amount)} by {payment.method}
              </span>
              <span className={styles.lineSubtitle}>
                {formatDateTime(payment.at)}
                {payment.reference === '' ? '' : ` · ${payment.reference}`}
                {payment.state === 'void' ? ` · void: ${payment.void_reason}` : ''}
              </span>
              {payment.conversion && (
                <span className={styles.lineSubtitle}>
                  taken at {rateText(payment.conversion, currencies.data ?? [])}, as it was quoted
                  {payment.settled_exact ? '' : ' (the conversion rounded minor units away)'}
                </span>
              )}
            </td>
            <td>
              {payment.settled ? (
                <Amount money={payment.settled} muted={payment.state === 'void'} />
              ) : (
                <Words>on account</Words>
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function IssueDialog({
  open,
  invoice,
  onClose,
}: {
  open: boolean;
  invoice: InvoiceView;
  onClose: () => void;
}) {
  const issue = useIssueInvoice();
  const reset = issue.reset;
  const [due, setDue] = useState('');
  const [issued, setIssued] = useState('');

  useEffect(() => {
    if (open) {
      setDue('');
      setIssued('');
      reset();
    }
  }, [open, reset]);

  const submit = () => {
    const dueAt = dayStart(due);
    const issuedAt = dayStart(issued);
    issue.mutate(
      {
        id: invoice.id,
        ...(dueAt === null ? {} : { due_at: dueAt }),
        ...(issuedAt === null ? {} : { issued_at: issuedAt }),
      },
      {
        onSuccess: (issuedInvoice) => {
          toast.success(
            `Invoice ${issuedInvoice.number ?? ''} is issued to ${issuedInvoice.customer_name}`,
            { description: 'It is frozen now: a wrong invoice is voided and reissued.' },
          );
          onClose();
        },
      },
    );
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="sm"
      title="Issue this invoice"
      description="Issuing allocates its number and freezes it: nothing recalculates an issued invoice, ever. A wrong one is voided with a reason and reissued."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={issue.isPending} onClick={submit}>
            Issue it
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {issue.error && <p className={styles.error}>{issue.error.message}</p>}
        <Field
          label="Due date"
          hint="Leave it empty for your payment terms (Settings → ISP). It may not fall before the invoice is issued."
        >
          <Input type="date" value={due} onChange={(event) => setDue(event.target.value)} />
        </Field>
        <Field
          label="Issued on"
          hint="For entering an invoice that already exists on paper. It may not be in the future; leave it empty for now."
        >
          <Input type="date" value={issued} onChange={(event) => setIssued(event.target.value)} />
        </Field>
      </div>
    </Dialog>
  );
}

function VoidDialog({
  open,
  reissue,
  invoice,
  onClose,
}: {
  open: boolean;
  reissue: boolean;
  invoice: InvoiceView;
  onClose: () => void;
}) {
  const voidIt = useVoidInvoice();
  const reissueIt = useReissueInvoice();
  const mutation = reissue ? reissueIt : voidIt;
  const reset = mutation.reset;
  const [reason, setReason] = useState('');

  useEffect(() => {
    if (open) {
      setReason('');
      reset();
    }
  }, [open, reset]);

  const submit = () => {
    const params = { id: invoice.id, reason: reason.trim() };
    if (reissue) {
      reissueIt.mutate(params, {
        onSuccess: (result) => {
          toast.success('Voided, and a draft replacement is ready', { description: result.note });
          onClose();
        },
      });
      return;
    }
    voidIt.mutate(params, {
      onSuccess: (voided) => {
        toast.success(`${voided.number ?? 'The draft'} is void`, {
          description: 'It is kept, and still readable: a void invoice is never deleted.',
        });
        onClose();
      },
    });
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="sm"
      title={reissue ? 'Void this invoice and draft its replacement' : 'Void this invoice'}
      description={
        reissue
          ? 'The original is voided, keeps its number and stays readable; its lines are copied into a new draft that replaces it, so the correction can be made before anything is issued.'
          : 'A void invoice is never deleted: it stays, with the reason, because somebody will ask about it.'
      }
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="danger"
            loading={mutation.isPending}
            disabled={reason.trim() === ''}
            onClick={submit}
          >
            {reissue ? 'Void and reissue' : 'Void it'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {mutation.error && <p className={styles.error}>{mutation.error.message}</p>}
        <Field label="Reason" required hint="This is what the trail will say happened.">
          <Textarea
            autoFocus
            rows={3}
            value={reason}
            onChange={(event) => setReason(event.target.value)}
            placeholder="The megabits were wrong"
          />
        </Field>
      </div>
    </Dialog>
  );
}

function LineDialog({
  open,
  invoice,
  onClose,
}: {
  open: boolean;
  invoice: InvoiceView;
  onClose: () => void;
}) {
  const currencies = useCurrencies();
  const add = useAddLine();
  const reset = add.reset;
  const [description, setDescription] = useState('');
  const [amount, setAmount] = useState('');

  useEffect(() => {
    if (open) {
      setDescription('');
      setAmount('');
      reset();
    }
  }, [open, reset]);

  const digits = digitsOf(currencies.data ?? [], invoice.currency);
  const parsed = digits === undefined ? null : parseAmount(amount, invoice.currency, digits);

  const submit = () => {
    if (!parsed?.ok) return;
    add.mutate(
      {
        invoice_id: invoice.id,
        description: description.trim(),
        amount: parsed.amount.input,
      },
      {
        onSuccess: () => {
          toast.success('The line was added');
          onClose();
        },
      },
    );
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="sm"
      title="Add a line by hand"
      description={`An installation, a discount, a correction. It is in ${invoice.currency}, like the rest of this invoice, and a discount is a negative amount.`}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            loading={add.isPending}
            disabled={description.trim() === '' || parsed?.ok !== true}
            onClick={submit}
          >
            Add the line
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {add.error && <p className={styles.error}>{add.error.message}</p>}
        <Field label="Description" required>
          <Input
            autoFocus
            value={description}
            onChange={(event) => setDescription(event.target.value)}
            placeholder="Installation"
          />
        </Field>
        <AmountField
          label="Amount"
          required
          value={amount}
          onChange={setAmount}
          currency={invoice.currency}
          currencies={currencies.data ?? []}
        />
      </div>
    </Dialog>
  );
}
