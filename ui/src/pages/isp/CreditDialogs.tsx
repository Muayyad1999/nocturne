/**
 * The three things that can be done with a credit note: issue one, set one
 * that is on the account against an invoice, and void one.
 *
 * A credit note writes off money a customer owes, so each of these says what
 * it will do to a figure before it does it — and none of them is the way to
 * correct an invoice. An invoice that was wrong is voided and reissued; a
 * credit note is for an amount that stopped being owed (a service that was
 * not delivered, a goodwill gesture), and it needs its reason, because an
 * amount that vanished with no reason recorded cannot be told from a mistake
 * six months later.
 *
 * What the dialogs refuse before the round trip is what the engine refuses
 * after it (`credit-notes.ts`): nothing or less, more than the invoice ever
 * charged, a note with no reason. The engine still checks, and its message
 * is shown if the two ever disagree. There is no currency to choose: a note
 * is written in the currency the customer is billed in, because no money
 * moves and so there is no rate anybody got.
 */

import { useEffect, useMemo, useState } from 'react';
import type { CreditInput, CreditNote, InvoiceView } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { Combobox } from '@/design/Combobox';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import { AmountField } from './AmountField';
import { dayStart, dayText, offsetName } from './calendar';
import {
  allocationChoices,
  CREDIT_NOTE_MEANING,
  creditEffect,
  creditRefusal,
  creditRoom,
} from './credit-notes';
import {
  useAllocateCredit,
  useCalendar,
  useCurrencies,
  useCustomers,
  useInvoices,
  useIssueCredit,
  useVoidCredit,
} from './data';
import { digitsOf, formatMoney, parseAmount } from './money';
import { useOnOpen } from './opening';
import styles from './isp.module.css';

/** How many customers the picker holds. Beyond this, use the customer's own page. */
const PICKER = 500;

export interface CreditDialogProps {
  open: boolean;
  onClose: () => void;
  /** Fixed when the dialog is opened from a customer or an invoice. */
  customerId?: string;
  /** The customer's currency, when it is known without asking. */
  currency?: string;
  /** Fixed when it is opened from an invoice. */
  invoice?: InvoiceView;
}

/** Issue a credit note: against an invoice, or on the account. */
export function CreditDialog({ open, onClose, customerId, currency, invoice }: CreditDialogProps) {
  const currencies = useCurrencies();
  const calendar = useCalendar();
  const issue = useIssueCredit();
  const reset = issue.reset;

  const [customer, setCustomer] = useState<string | null>(customerId ?? null);
  const [invoiceId, setInvoiceId] = useState(invoice?.id ?? '');
  const [amount, setAmount] = useState('');
  const [reason, setReason] = useState('');
  const [note, setNote] = useState('');
  const [at, setAt] = useState('');

  // The picker is only loaded when the dialog needs one.
  const picker = useCustomers(customerId ? { limit: 1 } : { limit: PICKER });
  const theirInvoices = useInvoices(
    { customer_id: customer ?? '', state: 'issued', limit: 200 },
    open && customer !== null && invoice === undefined,
  );

  // As it opens and not again (`opening.ts`): the invoice this was opened on
  // is read again while the note is being issued, and comes back changed
  // before the answer does.
  useOnOpen(open, () => {
    setCustomer(customerId ?? null);
    setInvoiceId(invoice?.id ?? '');
    setAmount('');
    setReason('');
    setNote('');
    setAt('');
    reset();
  });

  const chosenCustomer = useMemo(
    () => picker.data?.customers.find((row) => row.id === customer) ?? null,
    [picker.data, customer],
  );
  const chosenInvoice =
    invoice ?? theirInvoices.data?.find((row) => row.id === invoiceId) ?? null;
  // A note is in the customer's own currency, always: the invoice's when one
  // is chosen (the same thing), else the customer's.
  const code = chosenInvoice?.currency ?? currency ?? chosenCustomer?.currency ?? '';

  const digits = digitsOf(currencies.data ?? [], code);
  const parsed = digits === undefined ? null : parseAmount(amount, code, digits);
  const refusal =
    parsed?.ok === true ? creditRefusal(parsed.amount.minor, chosenInvoice, formatMoney) : null;
  const typedDate = at.trim() !== '';
  const agreedAt = dayStart(at, calendar);
  const ready =
    customer !== null &&
    parsed?.ok === true &&
    refusal === null &&
    reason.trim() !== '' &&
    (!typedDate || agreedAt !== null);

  // Only invoices that can still take a note, so the list cannot offer a refusal.
  const creditable = (theirInvoices.data ?? []).filter(
    (row) => (creditRoom(row)?.minor ?? 0) > 0,
  );

  const submit = () => {
    if (!parsed?.ok || customer === null) return;
    const input: CreditInput = {
      customer_id: customer,
      ...(chosenInvoice ? { invoice_id: chosenInvoice.id } : {}),
      amount: parsed.amount.input,
      reason: reason.trim(),
      ...(note.trim() === '' ? {} : { note: note.trim() }),
      ...(agreedAt === null ? {} : { at: agreedAt }),
    };
    issue.mutate(input, {
      onSuccess: (issued) => {
        toast.success(
          issued.invoice
            ? `${issued.credit.number}: ${formatMoney(issued.credit.amount)} credited on invoice ${
                issued.invoice.number ?? ''
              }`
            : `${issued.credit.number}: ${formatMoney(issued.credit.amount)} credited on the account`,
          {
            description:
              issued.warnings.length > 0
                ? issued.warnings.join(' ')
                : issued.invoice
                  ? `${formatMoney(issued.invoice.outstanding)} is outstanding on that invoice now.`
                  : undefined,
          },
        );
        onClose();
      },
    });
  };

  const room = chosenInvoice ? creditRoom(chosenInvoice) : null;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title="Issue a credit note"
      description={CREDIT_NOTE_MEANING}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={issue.isPending} disabled={!ready} onClick={submit}>
            Issue the credit note
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {issue.error && <p className={styles.error}>{issue.error.message}</p>}

        {customerId === undefined && (
          <Field
            label="Customer"
            required
            hint={
              (picker.data?.matching ?? 0) > PICKER
                ? `The first ${formatCount(PICKER)} customers by name. For anybody else, issue it from their own page.`
                : undefined
            }
          >
            <Combobox
              value={customer}
              onChange={(next) => {
                setCustomer(next);
                setInvoiceId('');
              }}
              placeholder="Search by name or code"
              options={(picker.data?.customers ?? []).map((row) => ({
                value: row.id,
                label: row.name,
                description: [row.code, row.currency].filter(Boolean).join(' · '),
              }))}
            />
          </Field>
        )}

        {invoice === undefined && (
          <Field
            label="Against"
            hint="On the account, a credit note reduces nothing until it is set against an invoice — which can be done later, once."
          >
            <Select
              value={invoiceId}
              disabled={customer === null}
              onChange={(event) => setInvoiceId(event.target.value)}
              options={[
                { value: '', label: 'The account (no invoice yet)' },
                ...creditable.map((row) => ({
                  value: row.id,
                  label: `${row.number ?? 'not numbered'} · ${formatMoney(row.total)} · ${formatMoney(
                    row.outstanding,
                  )} outstanding`,
                })),
              ]}
            />
          </Field>
        )}

        {chosenInvoice && room && (
          <p className={styles.note}>
            Invoice {chosenInvoice.number ?? '(not numbered)'} for {chosenInvoice.customer_name} is{' '}
            {formatMoney(chosenInvoice.total)}: {formatMoney(chosenInvoice.paid)} paid,{' '}
            {formatMoney(chosenInvoice.credited)} credited, {formatMoney(chosenInvoice.outstanding)}{' '}
            outstanding. Up to {formatMoney(room)} more can be credited — the cap is what was
            charged, not what is left.
          </p>
        )}

        <AmountField
          label="Amount to credit"
          required
          autoFocus={customerId !== undefined}
          value={amount}
          onChange={setAmount}
          currency={code}
          currencies={currencies.data ?? []}
          error={refusal ?? undefined}
          hint={
            code === ''
              ? 'Choose the customer first: a credit note is in the currency they are billed in.'
              : `In ${code}, the currency this customer is billed in: no money moves, so nothing is converted.`
          }
        />

        {chosenInvoice && parsed?.ok === true && refusal === null && (
          <p className={styles.quiet}>
            {creditEffect(parsed.amount.minor, chosenInvoice, formatMoney)}
          </p>
        )}

        <Field
          label="Reason"
          required
          hint="Why this stopped being owed. It is on the note for good, and it is what somebody will read in six months."
        >
          <Textarea
            rows={2}
            value={reason}
            onChange={(event) => setReason(event.target.value)}
            placeholder="Four days without service after the storm"
          />
        </Field>

        <div className={styles.pair}>
          <Field
            label="Agreed on"
            error={
              typedDate && agreedAt === null
                ? calendar === null
                  ? 'The business’s calendar is not known, so a date cannot be read. Leave it empty for now.'
                  : 'That is not a date.'
                : undefined
            }
            hint={
              calendar === null
                ? 'Leave it empty for now.'
                : `Leave it empty for now. A day of the business’s calendar (${offsetName(calendar)}); today is ${dayText(Date.now(), calendar)}.`
            }
          >
            <Input type="date" value={at} onChange={(event) => setAt(event.target.value)} />
          </Field>
          <Field label="Note">
            <Input value={note} onChange={(event) => setNote(event.target.value)} />
          </Field>
        </div>
      </div>
    </Dialog>
  );
}

/** Void a credit note. The debt it cancelled comes back. */
export function VoidCreditDialog({
  credit,
  onClose,
}: {
  credit: CreditNote | null;
  onClose: () => void;
}) {
  const voidIt = useVoidCredit();
  const reset = voidIt.reset;
  const [reason, setReason] = useState('');

  useEffect(() => {
    if (credit !== null) {
      setReason('');
      reset();
    }
  }, [credit, reset]);

  return (
    <Dialog
      open={credit !== null}
      onClose={onClose}
      size="sm"
      title={credit ? `Void credit note ${credit.number}` : 'Void this credit note'}
      description={
        credit?.invoice_id
          ? `A credit note is never deleted. Voiding it keeps the record, with the reason, and puts back the ${formatMoney(
              credit.amount,
            )} it wrote off: the invoice it stands against is owed that much again.`
          : 'A credit note is never deleted. Voiding it keeps the record, with the reason, and it then credits nothing.'
      }
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="danger"
            loading={voidIt.isPending}
            disabled={reason.trim() === '' || credit === null}
            onClick={() => {
              if (!credit) return;
              voidIt.mutate(
                { id: credit.id, reason: reason.trim() },
                {
                  onSuccess: (voided) => {
                    toast.success(`Credit note ${voided.number} is void and the record is kept`);
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
        <Field
          label="Reason"
          required
          hint="It puts back money the customer had been told they no longer owed, so say why."
        >
          <Textarea
            autoFocus
            rows={3}
            value={reason}
            onChange={(event) => setReason(event.target.value)}
            placeholder="Issued against the wrong customer"
          />
        </Field>
      </div>
    </Dialog>
  );
}

/** Set a credit note that is on the account against one of the customer's invoices. */
export function AllocateCreditDialog({
  credit,
  onClose,
}: {
  credit: CreditNote | null;
  onClose: () => void;
}) {
  const allocate = useAllocateCredit();
  const reset = allocate.reset;
  const [invoiceId, setInvoiceId] = useState('');
  const invoices = useInvoices(
    { customer_id: credit?.customer_id ?? '', state: 'issued', limit: 200 },
    credit !== null,
  );

  useEffect(() => {
    if (credit !== null) {
      setInvoiceId('');
      reset();
    }
  }, [credit, reset]);

  const choices = credit ? allocationChoices(credit, invoices.data ?? []) : [];
  const chosen = choices.find((row) => row.id === invoiceId) ?? null;

  return (
    <Dialog
      open={credit !== null}
      onClose={onClose}
      size="sm"
      title={credit ? `Set ${credit.number} against an invoice` : 'Set against an invoice'}
      description="The same credit note — its number, amount, reason and date do not change — now reducing one invoice. It happens once: to move it afterwards, void it with a reason and issue another, so the trail shows both."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            loading={allocate.isPending}
            disabled={chosen === null || credit === null}
            onClick={() => {
              if (!credit || !chosen) return;
              allocate.mutate(
                { id: credit.id, invoice_id: chosen.id },
                {
                  onSuccess: (allocated) => {
                    toast.success(
                      `${allocated.credit.number} now stands against invoice ${
                        allocated.invoice?.number ?? ''
                      }`,
                      {
                        description:
                          allocated.warnings.length > 0
                            ? allocated.warnings.join(' ')
                            : allocated.invoice
                              ? `${formatMoney(allocated.invoice.outstanding)} is outstanding on it now.`
                              : undefined,
                      },
                    );
                    onClose();
                  },
                },
              );
            }}
          >
            Set it against the invoice
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {allocate.error && <p className={styles.error}>{allocate.error.message}</p>}
        <Field
          label="Invoice"
          required
          hint={
            credit && invoices.data && choices.length === 0
              ? `None of their issued invoices has ${formatMoney(
                  credit.amount,
                )} left to credit, and a note is not split. Void this note and issue smaller ones instead.`
              : 'Their issued invoices with room for the whole note: the notes on an invoice cannot come to more than it charged.'
          }
        >
          <Select
            value={invoiceId}
            onChange={(event) => setInvoiceId(event.target.value)}
            options={[
              { value: '', label: 'Choose an invoice', disabled: true },
              ...choices.map((row) => ({
                value: row.id,
                label: `${row.number ?? 'not numbered'} · ${formatMoney(row.total)} · ${formatMoney(
                  row.outstanding,
                )} outstanding`,
              })),
            ]}
          />
        </Field>
        {credit && chosen && (
          <p className={styles.quiet}>
            {creditEffect(credit.amount.minor, chosen, formatMoney)}
          </p>
        )}
      </div>
    </Dialog>
  );
}
