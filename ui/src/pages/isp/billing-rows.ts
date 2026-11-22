/**
 * Invoices, what may be done to one, and what a billing run has actually
 * done. No React in here — the file is `billing-rows.ts` and not `Billing.ts`
 * so that an import of `./Billing` cannot resolve to it on a case-insensitive
 * filesystem (`scripts/check-case-collisions.sh`).
 *
 * Four facts from the API decide everything in this module.
 *
 * 1. **An issued invoice is frozen.** Every edit is refused with `conflict`,
 *    so {@link invoiceActions} works out from the invoice itself which
 *    controls may exist. A button that can only produce a refusal is not
 *    offered — and the case where the refusal is the *useful* answer
 *    (voiding an invoice a payment or a credit note stands against) is
 *    explained in words instead ({@link voidBlockedReason}).
 * 2. **Settlement is derived, and it is the engine's.** `status`, `paid`,
 *    `credited`, `outstanding` and `days_overdue` come from the payments,
 *    the credit notes and the business's calendar at read time. Two
 *    documents move them — a recorded payment and a credit note — and there
 *    is no "mark as paid" anywhere, by design. Nothing here works out again
 *    whether an invoice is late: an invoice is overdue when its `status`
 *    says so, because the engine knows where the business's midnight is and
 *    a browser does not ({@link dueText}).
 * 3. **The figures add up, and are shown so that they can be seen to.**
 *    `total − paid − credited = outstanding`. {@link settlement} puts all
 *    four on screen for every invoice: a figure left out is how 100.00
 *    credited once went missing between a total of 600.00 and an outstanding
 *    of 300.00.
 * 4. **A run previews unless it is told not to.** `isp.invoice.run` defaults
 *    to `dry_run: true` and then writes nothing: `invoice_id` is `null` on
 *    every row and the backend says so in `note`. {@link runOutcome} keeps
 *    that distinction as the first thing the screen shows, because the
 *    difference between "this is what would be billed" and "these invoices
 *    exist now" is the difference the person is deciding about.
 */

import type {
  InvoiceLine,
  Money,
  InvoiceStatus,
  InvoiceView,
  Proration,
  RunResult,
  Totals,
} from '@/api/types/isp';
import type { StatusTone } from '@/design/tone';
import { formatCount, plural } from '@/lib/format';
import { dayNumber, periodText, type Offset } from './calendar';
import { moneyLike } from './money';

export interface StatusWords {
  label: string;
  tone: StatusTone;
  /** What that word means here, for a tooltip or a legend. */
  meaning: string;
}

export const STATUS_WORDS: Record<InvoiceStatus, StatusWords> = {
  draft: {
    label: 'Draft',
    tone: 'idle',
    meaning: 'Not issued. It can still be changed, and the customer has not been asked for it.',
  },
  issued: {
    label: 'Issued',
    tone: 'info',
    meaning: 'Issued and frozen. No payment is recorded against it, and its due day is not over.',
  },
  part_paid: {
    label: 'Part paid',
    tone: 'warn',
    meaning: 'Some of it has been received. The figure is what the recorded payments come to.',
  },
  paid: {
    label: 'Paid',
    tone: 'ok',
    meaning: 'Settled in full by recorded payments. Nothing else can mark an invoice paid.',
  },
  credited: {
    label: 'Credited',
    tone: 'ok',
    meaning:
      'Nothing more is owed, and it took a credit note to get there: part of it was written off, with a reason, rather than received. That is why it does not say paid.',
  },
  overdue: {
    label: 'Overdue',
    tone: 'crit',
    meaning: 'Issued, still short, and its due day is over in the business’s calendar.',
  },
  void: {
    label: 'Void',
    tone: 'maint',
    meaning: 'Withdrawn, with a reason, and kept: a void invoice is never deleted.',
  },
};

export function statusWords(status: InvoiceStatus): StatusWords {
  return STATUS_WORDS[status] ?? { label: status, tone: 'idle', meaning: '' };
}

/** What "now" is, for the words beside a due date: an instant and the business's calendar. */
export interface Today {
  now: number;
  /** The setting. `null` while the calendar is not known. */
  offset: Offset;
}

/**
 * What to say beside an invoice's due date.
 *
 * Whether it is **late** is the engine's answer and is only read here:
 * `status: "overdue"` with `days_overdue`, counted in the business's
 * calendar — an invoice due on the 15th is not late at any hour of the 15th
 * and is one day late throughout the 16th. `days_overdue` is null for every
 * other status, so it cannot be read as "not due yet": an invoice paid in
 * March and looked at in November is settled, not waiting.
 *
 * The one thing worked out here is a choice of words for an invoice the
 * engine says is *not* late — "due today" on its due day, "not due yet"
 * before it — from the due day in the invoice's own calendar and today in
 * the business's. A wrong clock in the browser can at worst pick the other
 * of those two phrases; it cannot call anything overdue.
 */
export function dueText(
  invoice: Pick<
    InvoiceView,
    'state' | 'status' | 'due_at' | 'days_overdue' | 'outstanding' | 'utc_offset_minutes'
  >,
  today: Today,
): string {
  if (invoice.state === 'draft') return 'Not issued';
  if (invoice.state === 'void') return 'Void';
  if (invoice.status === 'overdue') {
    return invoice.days_overdue === null ? 'Overdue' : `${plural(invoice.days_overdue, 'day')} overdue`;
  }
  // Nothing is left to fall due. An invoice that came to nothing reads
  // `issued` for ever — nobody paid it, and nobody owed it.
  if (invoice.outstanding.minor <= 0) return invoice.status === 'issued' ? 'Nothing owed' : 'Settled';
  if (invoice.due_at === null) return 'No due date';
  if (today.offset === null) return 'Not overdue';
  return dayNumber(invoice.due_at, invoice.utc_offset_minutes) <= dayNumber(today.now, today.offset)
    ? 'Due today'
    : 'Not due yet';
}

/** An invoice's period as the days it covers, in the calendar it was written in. */
export function invoicePeriod(
  invoice: Pick<InvoiceView, 'period_start' | 'period_end' | 'utc_offset_minutes'>,
): string {
  return periodText(invoice.period_start, invoice.period_end, invoice.utc_offset_minutes);
}

export interface SettlementFigure {
  label: 'Total' | 'Paid' | 'Credited' | 'Outstanding' | 'In their favour';
  money: Money;
  /** What this figure is made of, for the line under it. */
  note: string;
  /** Beside the point of the row: what has already been dealt with. */
  muted: boolean;
}

/**
 * The four figures of an invoice, in the order the sum is done:
 * `total − paid − credited = outstanding`. All four, always — with the
 * credited figure left out, an invoice of 600.00 with 200.00 paid showed
 * 300.00 outstanding and 100.00 accounted for nowhere.
 *
 * `paid` and `credited` are never added together: one is money that
 * arrived, the other an amount written off, and a figure that merged them
 * could not say how much came in. An invoice settled beyond its total has
 * no negative "outstanding": the difference stands in the customer's
 * favour, and is said that way.
 */
export function settlement(
  invoice: Pick<InvoiceView, 'total' | 'paid' | 'credited' | 'outstanding' | 'payments' | 'credits'>,
): SettlementFigure[] {
  const payments = invoice.payments.filter((payment) => payment.state === 'recorded').length;
  const credits = invoice.credits.filter((credit) => credit.state === 'issued').length;
  const inFavour = invoice.outstanding.minor < 0;
  return [
    { label: 'Total', money: invoice.total, note: 'what was charged', muted: false },
    {
      label: 'Paid',
      money: invoice.paid,
      note: payments === 0 ? 'no payment recorded' : `${plural(payments, 'payment')} received`,
      muted: true,
    },
    {
      label: 'Credited',
      money: invoice.credited,
      note:
        credits === 0
          ? 'no credit note'
          : `written off by ${plural(credits, 'credit note')}, not received`,
      muted: true,
    },
    inFavour
      ? {
          label: 'In their favour',
          money: moneyLike(invoice.outstanding, -invoice.outstanding.minor),
          note: 'settled beyond its total; it is not taken off anything else they owe',
          muted: false,
        }
      : {
          label: 'Outstanding',
          money: invoice.outstanding,
          note: 'total, less what was paid, less what was credited',
          muted: false,
        },
  ];
}

export interface InvoiceActions {
  /** Lines may be added or removed: a draft only. */
  edit: boolean;
  issue: boolean;
  /** A draft, or an issued invoice nothing stands against. */
  void: boolean;
  reissue: boolean;
  /** Only while something is still owed on an issued invoice. */
  pay: boolean;
  /** Only while less than its total has been credited on an issued invoice. */
  credit: boolean;
}

/** The payments and credit notes that still stand against an invoice. */
function standing(invoice: Pick<InvoiceView, 'payments' | 'credits'>) {
  return {
    payments: invoice.payments.filter((payment) => payment.state === 'recorded'),
    credits: invoice.credits.filter((credit) => credit.state === 'issued'),
  };
}

/**
 * Which controls an invoice may have. Derived from the document's own state
 * and from what stands against it, so that nothing on screen can only
 * produce a refusal: the engine refuses to void or reissue an invoice while
 * a recorded payment or an issued credit note stands against it, a payment
 * on an invoice with nothing outstanding would only overpay it, and the
 * credit notes on an invoice may not come to more than it charged.
 */
export function invoiceActions(
  invoice: Pick<
    InvoiceView,
    'state' | 'payments' | 'credits' | 'outstanding' | 'credited' | 'total'
  >,
): InvoiceActions {
  switch (invoice.state) {
    case 'draft':
      return { edit: true, issue: true, void: true, reissue: false, pay: false, credit: false };
    case 'issued': {
      const { payments, credits } = standing(invoice);
      const clear = payments.length === 0 && credits.length === 0;
      return {
        edit: false,
        issue: false,
        void: clear,
        reissue: clear,
        pay: invoice.outstanding.minor > 0,
        credit: invoice.credited.minor < invoice.total.minor,
      };
    }
    default:
      return { edit: false, issue: false, void: false, reissue: false, pay: false, credit: false };
  }
}

/**
 * Why void is not offered, when the reason is what stands against the
 * invoice rather than its state. The backend refuses this for a good reason
 * and says so; this is the same explanation before the attempt.
 */
export function voidBlockedReason(
  invoice: Pick<InvoiceView, 'state' | 'payments' | 'credits'>,
): string | null {
  if (invoice.state !== 'issued') return null;
  const { payments, credits } = standing(invoice);
  const reasons: string[] = [];
  if (payments.length > 0) {
    reasons.push(
      `${plural(
        payments.length,
        'payment',
      )} recorded against this invoice. Voiding it would hide money that was actually received — void the payment first, with its own reason, if it was recorded in error.`,
    );
  }
  if (credits.length > 0) {
    reasons.push(
      `Credit note ${credits
        .map((credit) => credit.number)
        .join(', ')} stands against this invoice. Voiding it would leave a numbered document the customer was given writing off nothing — void the note first, with its own reason, and credit the replacement once it is issued.`,
    );
  }
  return reasons.length === 0 ? null : reasons.join(' ');
}

/**
 * Where a page of the invoice list is, and what of it is not on screen.
 *
 * The engine pages by instants and the month filter is asked in each
 * invoice's own calendar, so after the business's calendar has been changed
 * a page can hold an invoice of the month before or after — one written
 * under the old offset, which overlaps this month by the hours the two
 * calendars disagree about. Those are not listed, and the count says so
 * rather than showing "1–50" above forty-eight rows.
 */
export function pageText(offset: number, onPage: number, shown: number): string {
  if (onPage === 0) return 'nothing on this page';
  const range = `${formatCount(offset + 1)}–${formatCount(offset + onPage)}`;
  const hidden = onPage - shown;
  if (hidden <= 0) return range;
  return `${range}, ${formatCount(hidden)} not shown: ${
    hidden === 1 ? 'its period is' : 'their periods are'
  } another month in the calendar ${hidden === 1 ? 'it was' : 'they were'} written in`;
}

/** An invoice line that is part of a period: the arithmetic, in words. */
export function prorationText(prorated: Proration): string {
  return `${formatCount(prorated.covered_days)} of the period’s ${formatCount(
    prorated.period_days,
  )} days${prorated.exact ? '' : ', rounded to the minor unit'}`;
}

/**
 * What is under a line's figure — the arithmetic, not the plan's name, which
 * the description already carries. A per-megabit line shows the sum somebody
 * can check (`20 Mbit/s × 4.00 USD`), and a part period shows what part.
 */
export function lineSubtitle(line: InvoiceLine, amount: (money: Money) => string): string {
  const parts: string[] = [];
  if (line.unit === 'per_mbit' && line.unit_price && line.mbit !== null) {
    parts.push(`${formatCount(line.mbit)} Mbit/s × ${amount(line.unit_price)} per megabit`);
  }
  if (line.prorated) parts.push(prorationText(line.prorated));
  return parts.join(' · ');
}

// -------------------------------------------------------------- the run

export interface RunOutcome {
  /** True while nothing has been written. The screen leads with this. */
  preview: boolean;
  /** The heading: what this is, in one phrase. */
  headline: string;
  /** The backend's own sentence about what it did or did not do. */
  note: string;
  /** The days that were billed, in the calendar the engine read the period in. */
  period: string;
  /** What the run comes to, per currency. Never one figure. */
  totals: Totals;
  /** How many customers the run looked at: `invoices + skipped`, always. */
  considered: number;
  invoices: number;
  issued: number;
  /** Drafts the run was asked to issue and could not; each row says why. */
  leftAsDrafts: number;
  skipped: number;
  /** Subscriptions inside a billed customer that were deliberately not charged. */
  skippedSubscriptions: number;
  /** Anything about the run as a whole that did not go as asked. */
  warnings: string[];
}

export function runOutcome(result: RunResult): RunOutcome {
  const skippedSubscriptions = result.invoices.reduce(
    (total, row) => total + row.skipped_subscriptions.length,
    0,
  );
  const leftAsDrafts = result.invoices.filter((row) => row.not_issued !== null).length;
  // `drafted` counts every invoice the run made, the issued ones included.
  const drafts = result.drafted - result.issued;
  return {
    preview: result.dry_run,
    headline: result.dry_run
      ? 'Preview — nothing has been written'
      : result.issued > 0
        ? `${plural(result.issued, 'invoice')} issued${
            drafts > 0 ? `, ${plural(drafts, 'draft')} left a draft` : ''
          }`
        : `${plural(result.drafted, 'draft')} created`,
    note: result.note,
    period: periodText(result.period_start, result.period_end, result.utc_offset_minutes),
    totals: result.totals,
    considered: result.considered,
    invoices: result.invoices.length,
    issued: result.issued,
    leftAsDrafts,
    skipped: result.skipped.length,
    skippedSubscriptions,
    warnings: result.warnings,
  };
}

/** Who a run billed and whom it left out, as one sentence that adds up. */
export function runCountsText(outcome: RunOutcome): string {
  const billed = outcome.preview
    ? `${plural(outcome.invoices, 'customer')} would be billed`
    : `${plural(outcome.invoices, 'customer')} billed`;
  return `${billed}${outcome.skipped > 0 ? `, ${formatCount(outcome.skipped)} left out` : ''}${
    outcome.skippedSubscriptions > 0
      ? `, and ${plural(outcome.skippedSubscriptions, 'subscription')} not charged on an invoice that was still drafted`
      : ''
  } — ${formatCount(outcome.considered)} looked at.`;
}

/**
 * Whether a preview is worth acting on. A run that would bill nobody is not
 * an error — every customer it left out says why — but it has nothing to
 * confirm either.
 */
export function runHasWork(result: RunResult | null): boolean {
  return result !== null && result.invoices.length > 0;
}
