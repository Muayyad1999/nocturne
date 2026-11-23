/**
 * The dates the ISP end-to-end seed writes (`tests/support/isp.ts`), and the
 * days, months and document numbers the spec and the screenshots then expect
 * to read — every one worked out from the moment the seed runs. No React in
 * here, and nothing the application imports: it sits beside the screens
 * because unit tests are collected from `src/` only (`vite.config.ts`), and
 * what it decides is worth a unit test.
 *
 * The spec reads the seeded invoices against the real clock — "overdue",
 * "not due yet", "nothing overdue" — so a date typed in here is right for a
 * few weeks and then fails every run, for a reason no change had anything to
 * do with. {@link seedDates} takes them all from `now` instead:
 *
 * * the **billed** month is the last whole month before `now`, in the
 *   calendar the seed is written in; the plans billed for it whole began two
 *   months before that, so the month before it can be billed too;
 * * the **overdue** invoice is issued on the 2nd of the billed month and due
 *   on its 10th, which is at least nineteen days before `now`;
 * * the **open** invoice is issued on the first day of the month `now` is in
 *   and falls due three weeks after the day the seed runs;
 * * the **part-period** plan starts on the 16th of the billed month.
 *
 * Whether an invoice is late is the engine's answer: today in the calendar
 * the business has *now*, against the due day in the calendar the invoice
 * was *written in* (`calendar.rs: days_overdue`). The spec moves the business
 * from UTC to UTC+03:00 half way through, and two calendars can be as much
 * as twenty-six hours apart, so both margins above are weeks, not days.
 *
 * An invoice's number carries the year it was issued in, in its own
 * calendar, and each year counts from one. Seeded in January, the overdue
 * invoice is last year's first and the open one is this year's first; the
 * numbers are worked out here rather than typed into the spec for that
 * reason.
 *
 * The arithmetic is this module's own and does not go through `calendar.ts`:
 * a spec that formatted its expectations with the screen's formatter would
 * agree with it when it is wrong. `seed-dates.test.ts` is where the two are
 * held to each other, over every midnight of five years in six calendars.
 */

const MINUTE_MS = 60_000;
const DAY_MS = 86_400_000;

/** `2026-11-22`: the day an instant falls on, `offset` minutes ahead of UTC. */
export function dayIn(at: number, offset: number): string {
  return new Date(at + offset * MINUTE_MS).toISOString().slice(0, 10);
}

/** The year an instant falls in, `offset` minutes ahead of UTC. */
export function yearIn(at: number, offset: number): number {
  return new Date(at + offset * MINUTE_MS).getUTCFullYear();
}

/** `2026-10`, as days and instants `offset` minutes ahead of UTC. */
export function monthIn(
  month: string,
  offset: number,
): { start: number; end: number; first: string; last: string } {
  const [year, number] = month.split('-').map(Number) as [number, number];
  const start = Date.UTC(year, number - 1, 1) - offset * MINUTE_MS;
  // `Date.UTC` carries a thirteenth month into the next year.
  const end = Date.UTC(year, number, 1) - offset * MINUTE_MS;
  return { start, end, first: dayIn(start, offset), last: dayIn(end - 1, offset) };
}

/** The month `count` months before `2026-10`: `2026-09` for one. */
export function monthsBefore(month: string, count: number): string {
  const [year, number] = month.split('-').map(Number) as [number, number];
  const index = year * 12 + (number - 1) - count;
  return `${Math.floor(index / 12)}-${String((index % 12) + 1).padStart(2, '0')}`;
}

/** `2026-0001`: the `sequence`th invoice issued in a year, with no prefix set. */
export function invoiceNumber(year: number, sequence: number): string {
  return `${year}-${String(sequence).padStart(4, '0')}`;
}

/** `CN-2026-0001`: the `sequence`th credit note issued in a year. */
export function creditNoteNumber(year: number, sequence: number): string {
  return `CN-${invoiceNumber(year, sequence)}`;
}

/** One month of the calendar the seed is written in. */
export interface SeedMonth {
  /** `2026-10`: what the engine is told, and the value a month control holds. */
  name: string;
  /** Its first moment, and the first moment after it. */
  start: number;
  end: number;
  /** Its first and last day: `2026-10-01` and `2026-10-31`. */
  first: string;
  last: string;
  /** How many days long it is: what a part period is counted against. */
  days: number;
}

/** An invoice the seed issues with dates of its own. */
export interface SeedInvoice {
  issuedAt: number;
  dueAt: number;
  /** The number the engine gives it on a ledger nothing else was issued in. */
  number: string;
}

export interface SeedDates {
  /** The moment these were worked out for, and the calendar they are days of. */
  now: number;
  offset: number;
  /** The month every seeded invoice bills. */
  billed: SeedMonth;
  /** The month after it. Nothing is billed for it, so a run of it has something to preview. */
  following: SeedMonth;
  /** The month before the billed one, and the one before that: left for the spec to bill. */
  earlier: SeedMonth;
  earliest: SeedMonth;
  /** When the two plans that are billed for whole months began. */
  plansStart: number;
  /** When the third plan began, part of the way through the billed month. */
  partPlanStart: number;
  /** What its invoice line then says: how much of the month the plan ran for. */
  partPeriod: { coveredDays: number; periodDays: number; words: string };
  /** Issued and long past its due day. */
  overdue: SeedInvoice;
  /** Issued, and not due for weeks. */
  open: SeedInvoice;
}

function seedMonth(name: string, offset: number): SeedMonth {
  const window = monthIn(name, offset);
  return { name, ...window, days: Math.round((window.end - window.start) / DAY_MS) };
}

/** How long after the day the seed runs the open invoice falls due. */
const OPEN_DUE_IN_DAYS = 21;

/**
 * What the seed writes when it runs at `now`, in a business whose calendar
 * is `offset` minutes ahead of UTC — the `utc_offset_minutes` the engine has
 * at that moment, which is the calendar it writes each invoice in.
 */
export function seedDates(now: number, offset: number): SeedDates {
  const thisMonth = dayIn(now, offset).slice(0, 7);
  const month = (count: number) => seedMonth(monthsBefore(thisMonth, count), offset);
  const following = month(0);
  const billed = month(1);
  const earlier = month(2);
  const earliest = month(3);
  /** The first moment of a day of the billed month, counted from 1. */
  const dayOfBilled = (day: number) => billed.start + (day - 1) * DAY_MS;

  const partPlanStart = dayOfBilled(16);
  const coveredDays = Math.round((billed.end - partPlanStart) / DAY_MS);

  const overdueIssuedAt = dayOfBilled(2);
  // The first moment of the month `now` is in, so never in the future —
  // which the engine refuses for an issue date.
  const openIssuedAt = following.start;
  const overdueYear = yearIn(overdueIssuedAt, offset);
  const openYear = yearIn(openIssuedAt, offset);
  const startOfToday = Date.parse(`${dayIn(now, offset)}T00:00:00Z`) - offset * MINUTE_MS;

  return {
    now,
    offset,
    billed,
    following,
    earlier,
    earliest,
    plansStart: earliest.start,
    partPlanStart,
    partPeriod: {
      coveredDays,
      periodDays: billed.days,
      words: `${coveredDays} of the period’s ${billed.days} days`,
    },
    overdue: {
      issuedAt: overdueIssuedAt,
      dueAt: dayOfBilled(10),
      number: invoiceNumber(overdueYear, 1),
    },
    open: {
      issuedAt: openIssuedAt,
      dueAt: startOfToday + OPEN_DUE_IN_DAYS * DAY_MS,
      // Issued second. In January the first was last year's, and this is
      // the first of its own year.
      number: invoiceNumber(openYear, openYear === overdueYear ? 2 : 1),
    },
  };
}
