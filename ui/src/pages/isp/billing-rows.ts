/**
 * Invoices, what may be done to one, and what a billing run has actually
 * done. No React in here — the file is `billing-rows.ts` and not `Billing.ts`
 * so that an import of `./Billing` cannot resolve to it on a case-insensitive
 * filesystem (`scripts/check-case-collisions.sh`).
 *
 * Three facts from the API decide everything in this module.
 *
 * 1. **An issued invoice is frozen.** Every edit is refused with `conflict`,
 *    so {@link invoiceActions} works out from the invoice itself which
 *    controls may exist. A button that can only produce a refusal is not
 *    offered — and the one case where the refusal is the *useful* answer
 *    (voiding an invoice a payment is recorded against) is explained in
 *    words instead (@link voidBlockedReason).
 * 2. **Settlement is derived.** `status` comes from the payments and the
 *    clock at read time, and nothing but a recorded payment moves it; there
 *    is no "mark as paid" anywhere, by design.
 * 3. **A run previews unless it is told not to.** `isp.invoice.run` defaults
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
    meaning: 'Issued and frozen. Nothing has been paid against it yet, and it is not due yet.',
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
  overdue: {
    label: 'Overdue',
    tone: 'crit',
    meaning: 'Issued, still short, and the due date has passed.',
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

/** How late an invoice is. `null` is not zero: it is not due yet. */
export function overdueText(invoice: Pick<InvoiceView, 'days_overdue' | 'state' | 'due_at'>): string {
  if (invoice.state === 'draft') return 'Not issued';
  if (invoice.state === 'void') return 'Void';
  if (invoice.due_at === null) return 'No due date';
  if (invoice.days_overdue === null) return 'Not due yet';
  if (invoice.days_overdue === 0) return 'Due today';
  return `${plural(invoice.days_overdue, 'day')} overdue`;
}

export interface InvoiceActions {
  /** Lines may be added or removed: a draft only. */
  edit: boolean;
  issue: boolean;
  /** Void is offered for a draft or an issued invoice with no payment. */
  void: boolean;
  reissue: boolean;
  /** A payment can only be recorded against an issued invoice. */
  pay: boolean;
}

/**
 * Which controls an invoice may have. Derived from the document's own state
 * so that nothing on screen can only produce a `conflict`.
 */
export function invoiceActions(
  invoice: Pick<InvoiceView, 'state' | 'payments' | 'outstanding'>,
): InvoiceActions {
  const recorded = invoice.payments.filter((payment) => payment.state === 'recorded').length;
  switch (invoice.state) {
    case 'draft':
      return { edit: true, issue: true, void: true, reissue: false, pay: false };
    case 'issued':
      return {
        edit: false,
        issue: false,
        void: recorded === 0,
        reissue: recorded === 0,
        pay: true,
      };
    default:
      return { edit: false, issue: false, void: false, reissue: false, pay: false };
  }
}

/**
 * Why void is not offered, when the reason is money rather than state. The
 * backend refuses this for a good reason and says so; this is the same
 * sentence before the attempt.
 */
export function voidBlockedReason(
  invoice: Pick<InvoiceView, 'state' | 'payments'>,
): string | null {
  if (invoice.state !== 'issued') return null;
  const recorded = invoice.payments.filter((payment) => payment.state === 'recorded').length;
  if (recorded === 0) return null;
  return `${plural(
    recorded,
    'payment',
  )} recorded against this invoice. Voiding it would hide money that was actually received — void the payment first, with its own reason, if it was recorded in error.`;
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
  /** What the run comes to, per currency. Never one figure. */
  totals: Totals;
  invoices: number;
  issued: number;
  skipped: number;
  /** Subscriptions inside a billed customer that were deliberately not charged. */
  skippedSubscriptions: number;
}

export function runOutcome(result: RunResult): RunOutcome {
  const skippedSubscriptions = result.invoices.reduce(
    (total, row) => total + row.skipped_subscriptions.length,
    0,
  );
  return {
    preview: result.dry_run,
    headline: result.dry_run
      ? 'Preview — nothing has been written'
      : result.issued > 0
        ? `${plural(result.issued, 'invoice')} issued`
        : `${plural(result.drafted, 'draft')} created`,
    note: result.note,
    totals: result.totals,
    invoices: result.invoices.length,
    issued: result.issued,
    skipped: result.skipped.length,
    skippedSubscriptions,
  };
}

/**
 * Whether a preview is worth acting on. A run that would bill nobody is not
 * an error — every customer it left out says why — but it has nothing to
 * confirm either.
 */
export function runHasWork(result: RunResult | null): boolean {
  return result !== null && result.invoices.length > 0;
}

/** A whole month in milliseconds, as `[start, end)` — what a run wants. */
export function monthWindow(year: number, month: number): { start: number; end: number } {
  return {
    start: Date.UTC(year, month, 1),
    end: Date.UTC(month === 11 ? year + 1 : year, (month + 1) % 12, 1),
  };
}

/** The months to offer for a billing run, newest first: `2026-10`, `2026-09`, … */
export function recentMonths(now: number, count = 14): { value: string; label: string }[] {
  const months: { value: string; label: string }[] = [];
  const at = new Date(now);
  let year = at.getUTCFullYear();
  let month = at.getUTCMonth();
  for (let index = 0; index < count; index += 1) {
    months.push({
      value: `${year}-${String(month + 1).padStart(2, '0')}`,
      label: `${MONTH_NAMES[month] ?? ''} ${year}`,
    });
    month -= 1;
    if (month < 0) {
      month = 11;
      year -= 1;
    }
  }
  return months;
}

const MONTH_NAMES = [
  'January',
  'February',
  'March',
  'April',
  'May',
  'June',
  'July',
  'August',
  'September',
  'October',
  'November',
  'December',
];

/**
 * A `YYYY-MM-DD` from a date field, as the first moment of that day in the
 * reader's own time zone — the same zone `lib/time` writes dates in, so what
 * is typed and what is shown are the same day. Invalid text is no date, not
 * today.
 */
export function dayStart(text: string): number | null {
  const found = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text.trim());
  if (!found?.[1] || !found[2] || !found[3]) return null;
  const at = new Date(Number(found[1]), Number(found[2]) - 1, Number(found[3]));
  return Number.isNaN(at.getTime()) ? null : at.getTime();
}

/** `2026-10` back to the window it means. Invalid text has no window. */
export function windowOf(value: string): { start: number; end: number } | null {
  const found = /^(\d{4})-(\d{2})$/.exec(value.trim());
  if (!found?.[1] || !found[2]) return null;
  const year = Number(found[1]);
  const month = Number(found[2]) - 1;
  if (month < 0 || month > 11) return null;
  return monthWindow(year, month);
}
