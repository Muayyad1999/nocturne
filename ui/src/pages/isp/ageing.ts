/**
 * Arrears: what is owed, for how long, and in which currency. No React in
 * here.
 *
 * `isp.arrears` returns one aged report **per currency** and no total across
 * them, because there is no such number. {@link byCurrency} therefore hands
 * the screen a list of reports to put side by side, each with its own
 * customers, and there is no function in this module that merges two of them.
 *
 * The other thing this module exists for is the difference between *owed* and
 * *overdue*. An invoice with no due date yet, or one whose date has not
 * passed, is money owed and nobody is late: `oldest_days` is `null`, which is
 * neither `0` nor a dash. {@link oldestText} writes that as a sentence.
 *
 * None of this disconnects anybody. Arrears are a thing to know, and
 * `nocturne-isp` has no write path to a device at all
 * (`docs/dev/features/isp.md`); the words here are chosen so that no label
 * can be read as a threat to cut a customer off.
 */

import type { Arrears, Bucket, CurrencyArrears, CustomerArrears } from '@/api/types/isp';
import type { StatusTone } from '@/design/tone';
import { plural } from '@/lib/format';

export interface AgedReport {
  currency: string;
  report: CurrencyArrears;
  /** That currency's customers only, worst first, as the backend ordered them. */
  customers: CustomerArrears[];
}

/**
 * One report per currency, each carrying only its own customers. A customer
 * who owes in two currencies appears in both, once, as the backend counts
 * them.
 */
export function byCurrency(arrears: Arrears): AgedReport[] {
  return arrears.by_currency.map((report) => ({
    currency: report.currency,
    report,
    customers: arrears.customers.filter((row) => row.outstanding.currency === report.currency),
  }));
}

/**
 * How to colour an age band. The first band is not late at all, so it is
 * never a warning; past ninety days is the one worth a colour that stops
 * somebody.
 */
export function bucketTone(bucket: Pick<Bucket, 'from_days' | 'to_days'>): StatusTone {
  if (bucket.from_days < 0) return 'info';
  if (bucket.to_days === null || bucket.to_days > 60) return 'crit';
  return 'warn';
}

/** The band's own label, as the backend wrote it, with what it means. */
export function bucketMeaning(bucket: Pick<Bucket, 'from_days' | 'to_days'>): string {
  if (bucket.from_days < 0) return 'Owed, and not late: the due date has not passed yet.';
  if (bucket.to_days === null) return `More than ${bucket.from_days - 1} days past the due date.`;
  return `Between ${Math.max(1, bucket.from_days)} and ${bucket.to_days} days past the due date.`;
}

/**
 * How late a customer's oldest unpaid invoice is. `null` is not zero: nothing
 * of theirs is due yet, which is a different sentence.
 */
export function oldestText(oldest_days: number | null): string {
  if (oldest_days === null) return 'Nothing due yet';
  if (oldest_days === 0) return 'Due today';
  return `${plural(oldest_days, 'day')} overdue`;
}

export function oldestTone(oldest_days: number | null): StatusTone {
  if (oldest_days === null) return 'info';
  if (oldest_days > 60) return 'crit';
  if (oldest_days > 0) return 'warn';
  return 'info';
}

/**
 * Whether any of these reports has anything overdue, for deciding what the
 * screen leads with. Asked per currency and never across them.
 */
export function anythingOverdue(reports: readonly AgedReport[]): boolean {
  return reports.some((entry) => entry.report.overdue.minor !== 0);
}

/**
 * What the ageing screen says about itself. Kept here because it is a claim
 * about what Nocturne does, and it must be the same claim everywhere.
 */
export const BOOKKEEPING_NOTE =
  'Arrears are a record. Nocturne does not suspend a service or change a device: ' +
  'cutting a customer off is a change somebody makes on the router.';
