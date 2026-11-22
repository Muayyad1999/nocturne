/**
 * Credit notes: what one is, where it stands, and what may be done to it.
 * No React in here.
 *
 * A credit note is an amount a customer no longer owes, as a document with
 * its own number and a required reason (`docs/dev/features/isp.md`, "Tax,
 * refunds and credit notes"). Four things about it decide what these screens
 * may say and offer:
 *
 * * **It is not money.** Nothing was received and nothing was refunded. It
 *   is never added to what was paid, and an invoice it cleared reads
 *   `credited`, not `paid`.
 * * **It stands against one invoice, or against the account.** On the
 *   account it reduces nothing until it is set against an invoice, once
 *   ({@link allocationChoices}); the words for that never suggest the
 *   customer's debt already went down ({@link standingText}).
 * * **The cap is what was charged, not what is left.** The notes on an
 *   invoice may come to its total and no more ({@link creditRoom}). An
 *   invoice that is already paid can still be credited — that is a real
 *   case — and it then stands in the customer's favour, which is said
 *   before it is done ({@link creditEffect}).
 * * **It is never deleted.** A note issued in error is voided with its own
 *   reason, and the debt comes back.
 *
 * The engine checks every one of these again and its refusal is shown; this
 * module exists so that the person is told before the round trip, and so
 * that no button can only produce a refusal.
 */

import type { CreditNote, InvoiceView, Money } from '@/api/types/isp';
import type { StatusTone } from '@/design/tone';
import { moneyLike } from './money';

/** What a credit note is, in the words every dialog about one starts with. */
export const CREDIT_NOTE_MEANING =
  'A credit note is an amount the customer no longer owes, with the reason. It is a document ' +
  'beside the invoice — the invoice’s own figures do not change — and it is not money: nothing ' +
  'is received, and nothing here says anything was refunded.';

export interface CreditWords {
  label: string;
  tone: StatusTone;
}

/** A note's state as a document. Its settlement effect is in {@link standingText}. */
export function creditStateWords(note: Pick<CreditNote, 'state'>): CreditWords {
  return note.state === 'void'
    ? { label: 'Void', tone: 'maint' }
    : { label: 'Issued', tone: 'ok' };
}

/**
 * Where a note stands, and what that does. A void note does nothing at all,
 * wherever it stood; a note on the account reduces nothing yet, and the
 * sentence says so rather than leaving "on the account" to be read as a
 * balance somebody can spend.
 */
export function standingText(
  note: Pick<CreditNote, 'state' | 'invoice_id'>,
  invoiceNumber?: string | null,
): string {
  if (note.state === 'void') {
    return note.invoice_id === null
      ? 'void: it credits nothing'
      : 'void: what it wrote off is owed again';
  }
  if (note.invoice_id === null) {
    return 'on the account: it reduces nothing until it is set against an invoice';
  }
  // The note carries the invoice's id, not its number; a caller that has
  // not got the number says so rather than printing an id nobody quotes.
  return invoiceNumber ? `against invoice ${invoiceNumber}` : 'against an invoice';
}

export interface CreditActions {
  /** An issued note can be voided, with a reason. */
  void: boolean;
  /** An issued note on the account can be set against an invoice, once. */
  allocate: boolean;
}

export function creditActions(note: Pick<CreditNote, 'state' | 'invoice_id'>): CreditActions {
  const issued = note.state === 'issued';
  return { void: issued, allocate: issued && note.invoice_id === null };
}

/**
 * How much more can be credited on an invoice: its total less what issued
 * notes have already written off. `null` for an invoice that cannot take a
 * note at all — a draft nobody owes yet, or a void one.
 */
export function creditRoom(
  invoice: Pick<InvoiceView, 'state' | 'total' | 'credited'>,
): Money | null {
  if (invoice.state !== 'issued') return null;
  return moneyLike(invoice.total, Math.max(0, invoice.total.minor - invoice.credited.minor));
}

/**
 * Why an amount cannot be credited on this invoice, or `null` when it can.
 * `minor` is whole minor units of the invoice's currency.
 */
export function creditRefusal(
  minor: number,
  invoice: Pick<InvoiceView, 'state' | 'total' | 'credited'> | null,
  amount: (money: Money) => string,
): string | null {
  if (minor <= 0) {
    return 'A credit note is more than nothing. To charge for something, raise an invoice; to undo a credit note, void it.';
  }
  if (invoice === null) return null;
  const room = creditRoom(invoice);
  if (room === null) return 'Only an issued invoice can be credited.';
  if (minor > room.minor) {
    return `This invoice is ${amount(invoice.total)} and ${amount(
      invoice.credited,
    )} of it has been credited already, so at most ${amount(
      room,
    )} more can be: a credit note cannot write off more than was ever charged.`;
  }
  return null;
}

/**
 * What a note of `minor` would leave on this invoice, said before it is
 * issued: what is still owed, that it is cleared, or — on an invoice already
 * settled — that the amount will stand in the customer's favour rather than
 * reduce a debt.
 */
export function creditEffect(
  minor: number,
  invoice: Pick<InvoiceView, 'outstanding'>,
  amount: (money: Money) => string,
): string {
  const left = invoice.outstanding.minor - minor;
  if (left > 0) return `${amount(moneyLike(invoice.outstanding, left))} would still be outstanding.`;
  if (left === 0) return 'Nothing would be left outstanding.';
  return `The invoice would be settled ${amount(
    moneyLike(invoice.outstanding, -left),
  )} beyond its total. That stands in the customer’s favour; it is not taken off anything else they owe.`;
}

/**
 * The invoices a note on the account can be set against: issued, in the
 * note's currency, the same customer's, and with room for the **whole**
 * note — a note is not split, so 150.00 cannot go against an invoice with
 * 100.00 left to credit.
 */
export function allocationChoices<T extends Pick<InvoiceView, 'state' | 'total' | 'credited' | 'customer_id'>>(
  note: Pick<CreditNote, 'amount' | 'customer_id'>,
  invoices: readonly T[],
): T[] {
  return invoices.filter((invoice) => {
    const room = creditRoom(invoice);
    return (
      invoice.customer_id === note.customer_id &&
      room !== null &&
      room.currency === note.amount.currency &&
      room.minor >= note.amount.minor
    );
  });
}
