/**
 * Recording a payment: what was actually received.
 *
 * Nothing else in this feature can make an invoice look paid, so this is the
 * only way money ever enters the ledger — and the figures here are what the
 * person taking the money says they took, not a calculation.
 *
 * The part that needs care is a payment in a currency the invoice is not in.
 * There is no rate feed and there never will be, so whoever takes the payment
 * enters the rate they were given, and it is stored **on that payment** and
 * never looked up again: a receipt from March still reconciles in November
 * because the figure is not recomputed. A rate is a whole number of minor
 * units of the second currency for one unit of the first, in the direction it
 * was quoted (`1 USD = 1320 IQD`), because the other direction does not fit
 * in an integer — 0.00076 dollars per dinar is zero, and the payment would
 * settle nothing. The field says so, and the direction can be swapped for the
 * cases where the quote goes the other way.
 *
 * A payment with no invoice is money on account, kept in the currency it
 * arrived in; a rate on one of those is refused, because there is nothing to
 * convert it towards yet.
 */

import { useEffect, useMemo, useState } from 'react';
import type { InvoiceView, PaymentInput } from '@/api/types/isp';
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
import { useCalendar, useCurrencies, useCustomers, useInvoices, useRecordPayment } from './data';
import { digitsOf, formatMoney, parseAmount, quotedRate } from './money';
import { useOnOpen } from './opening';
import styles from './isp.module.css';

/** How many customers the picker holds. Beyond this, use the customer's own page. */
const PICKER = 500;

export interface PaymentDialogProps {
  open: boolean;
  onClose: () => void;
  /** Fixed when the dialog is opened from a customer or an invoice. */
  customerId?: string;
  /** Fixed when it is opened from an invoice. */
  invoice?: InvoiceView;
  /** The customer's own currency, when it is known without asking. */
  defaultCurrency?: string;
}

export function PaymentDialog({
  open,
  onClose,
  customerId,
  invoice,
  defaultCurrency,
}: PaymentDialogProps) {
  const currencies = useCurrencies();
  const calendar = useCalendar();
  const record = useRecordPayment();
  const reset = record.reset;

  const [customer, setCustomer] = useState<string | null>(customerId ?? null);
  const [invoiceId, setInvoiceId] = useState(invoice?.id ?? '');
  const [amount, setAmount] = useState('');
  const [currency, setCurrency] = useState(invoice?.currency ?? defaultCurrency ?? '');
  const [method, setMethod] = useState('');
  const [reference, setReference] = useState('');
  const [note, setNote] = useState('');
  const [at, setAt] = useState('');
  const [rate, setRate] = useState('');
  const [swapped, setSwapped] = useState(false);

  // The picker is only loaded when the dialog needs one.
  const picker = useCustomers(customerId ? { limit: 1 } : { limit: PICKER });
  // The customer's own open invoices, to settle one of them.
  const theirInvoices = useInvoices(
    customer && !invoice ? { customer_id: customer, state: 'issued', limit: 200 } : { limit: 1 },
  );

  const chosenCustomer = useMemo(
    () => picker.data?.customers.find((row) => row.id === customer) ?? null,
    [picker.data, customer],
  );

  const chosenInvoice =
    invoice ?? theirInvoices.data?.find((row) => row.id === invoiceId) ?? null;

  // As it opens and not again (`opening.ts`): the invoice this was opened on
  // is read again while the payment is being recorded, and comes back
  // changed before the answer does.
  useOnOpen(open, () => {
    setCustomer(customerId ?? null);
    setInvoiceId(invoice?.id ?? '');
    setAmount('');
    setCurrency(invoice?.currency ?? defaultCurrency ?? '');
    setMethod('');
    setReference('');
    setNote('');
    setAt('');
    setRate('');
    setSwapped(false);
    reset();
  });

  // Without an explicit choice, a payment is in the currency of the invoice
  // it settles, or of the customer's account.
  useEffect(() => {
    if (!open || currency !== '') return;
    const guess = chosenInvoice?.currency ?? chosenCustomer?.currency;
    if (guess) setCurrency(guess);
  }, [open, currency, chosenInvoice?.currency, chosenCustomer?.currency]);

  const digits = digitsOf(currencies.data ?? [], currency);
  const parsed = digits === undefined ? null : parseAmount(amount, currency, digits);
  const needsRate = chosenInvoice !== null && currency !== '' && currency !== chosenInvoice.currency;
  const rateValid = /^[1-9]\d*$/.test(rate.trim());
  // The day the money changed hands, in the business's calendar. A date that
  // was typed and cannot be read is not "now".
  const paidAt = dayStart(at, calendar);
  const dateUnread = at.trim() !== '' && paidAt === null;
  const ready =
    customer !== null &&
    parsed?.ok === true &&
    parsed.amount.minor > 0 &&
    method.trim() !== '' &&
    !dateUnread &&
    (!needsRate || rateValid);

  const submit = () => {
    if (!parsed?.ok || customer === null) return;
    const from = needsRate && chosenInvoice ? (swapped ? currency : chosenInvoice.currency) : '';
    const to = needsRate && chosenInvoice ? (swapped ? chosenInvoice.currency : currency) : '';
    const input: PaymentInput = {
      customer_id: customer,
      ...(chosenInvoice ? { invoice_id: chosenInvoice.id } : {}),
      amount: parsed.amount.input,
      method: method.trim(),
      ...(reference.trim() === '' ? {} : { reference: reference.trim() }),
      ...(note.trim() === '' ? {} : { note: note.trim() }),
      ...(paidAt === null ? {} : { at: paidAt }),
      ...(needsRate ? { conversion: { from, to, rate: Number(rate.trim()) } } : {}),
    };
    record.mutate(input, {
      onSuccess: (recorded) => {
        const settled = recorded.payment.settled;
        toast.success(
          settled
            ? `${formatMoney(recorded.payment.amount)} received, settling ${formatMoney(settled)}`
            : `${formatMoney(recorded.payment.amount)} received, on account`,
          {
            description:
              recorded.warnings.length > 0
                ? recorded.warnings.join(' ')
                : recorded.invoice
                  ? `${recorded.invoice.customer_name}: ${formatMoney(
                      recorded.invoice.outstanding,
                    )} outstanding on that invoice.`
                  : 'It is not against an invoice yet, and is reported as money on account.',
          },
        );
        onClose();
      },
    });
  };

  const openInvoices = (theirInvoices.data ?? []).filter(
    (row) => row.state === 'issued' && row.outstanding.minor > 0,
  );

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title="Record a payment"
      description="What was actually received: the amount, when, how, and which invoice it settles. Nothing else can mark an invoice paid."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={record.isPending} disabled={!ready} onClick={submit}>
            Record it
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {record.error && <p className={styles.error}>{record.error.message}</p>}

        {customerId === undefined && (
          <Field
            label="Customer"
            required
            hint={
              (picker.data?.matching ?? 0) > PICKER
                ? `The first ${formatCount(PICKER)} customers by name. For anybody else, record it from their own page.`
                : undefined
            }
          >
            <Combobox
              value={customer}
              onChange={setCustomer}
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
            hint="A payment with no invoice is money on account: real money, kept in the currency it arrived in, not yet against anything."
          >
            <Select
              value={invoiceId}
              disabled={customer === null}
              onChange={(event) => setInvoiceId(event.target.value)}
              options={[
                { value: '', label: 'On account (no invoice)' },
                ...openInvoices.map((row) => ({
                  value: row.id,
                  label: `${row.number ?? 'draft'} · ${formatMoney(row.outstanding)} outstanding · due ${
                    row.due_at === null ? 'unset' : dayText(row.due_at, row.utc_offset_minutes)
                  }`,
                })),
              ]}
            />
          </Field>
        )}

        {invoice && (
          <p className={styles.note}>
            Against invoice {invoice.number ?? '(a draft)'} for {invoice.customer_name}:{' '}
            {formatMoney(invoice.outstanding)} outstanding of {formatMoney(invoice.total)}.
          </p>
        )}

        <AmountField
          label="Amount received"
          required
          autoFocus
          value={amount}
          onChange={setAmount}
          currency={currency}
          onCurrencyChange={setCurrency}
          currencies={currencies.data ?? []}
          hint="What was handed over, in the currency it was handed over in."
        />

        {needsRate && chosenInvoice && (
          <Field
            label="Rate it was taken at"
            required
            error={rate.trim() !== '' && !rateValid ? 'A rate is a whole number, greater than zero.' : undefined}
            hint={
              <>
                The invoice is in {chosenInvoice.currency} and this payment is in {currency}, so the
                rate you were given is recorded on the payment and never looked up again.{' '}
                {rateValid
                  ? `As quoted: ${
                      quotedRate(
                        swapped ? currency : chosenInvoice.currency,
                        swapped ? chosenInvoice.currency : currency,
                        rate,
                        currencies.data ?? [],
                      ) ?? ''
                    }, entered in minor units of ${swapped ? chosenInvoice.currency : currency}.`
                  : `Enter it the way it was quoted to you, in minor units of ${
                      swapped ? chosenInvoice.currency : currency
                    }.`}
              </>
            }
          >
            <div className={styles.rateRow}>
              <Input
                mono
                inputMode="numeric"
                value={rate}
                placeholder="1320"
                invalid={rate.trim() !== '' && !rateValid}
                onChange={(event) => setRate(event.target.value)}
              />
              <Button size="sm" onClick={() => setSwapped(!swapped)}>
                {`1 ${swapped ? currency : chosenInvoice.currency} → ${
                  swapped ? chosenInvoice.currency : currency
                }`}
              </Button>
            </div>
          </Field>
        )}

        <div className={styles.pair}>
          <Field label="How it came in" required hint="Cash, bank transfer, a wallet — your own words.">
            <Input
              value={method}
              onChange={(event) => setMethod(event.target.value)}
              placeholder="cash"
            />
          </Field>
          <Field label="Reference">
            <Input
              mono
              value={reference}
              onChange={(event) => setReference(event.target.value)}
              placeholder="receipt or transfer number"
            />
          </Field>
        </div>

        <Field
          label="When the money changed hands"
          error={
            dateUnread
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
          <Textarea rows={2} value={note} onChange={(event) => setNote(event.target.value)} />
        </Field>
      </div>
    </Dialog>
  );
}
