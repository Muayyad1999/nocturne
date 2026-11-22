/**
 * The business's calendar: where a day begins, for every date typed into
 * these screens and every date they show. No React in here.
 *
 * A billing period is a calendar month, an invoice falls due on a day, and a
 * plan "starts on the 1st". Each of those needs a midnight, and there are
 * three to choose from: UTC, the zone of whichever browser is open, and the
 * zone the ISP does business in. Only the last is the same for every
 * operator, and it is the day the customer was told. The engine keeps it as
 * one setting — `utc_offset_minutes` in the `isp` section, a fixed offset
 * from UTC with no daylight-saving rules (`docs/dev/features/isp.md`,
 * decision 13) — and this module is the interface's half of that decision:
 *
 * * a date somebody types, `Y-M-D`, is the instant
 *   `Date.UTC(Y, M-1, D) - offset * 60000` ({@link dayStart});
 * * an instant is shown by adding `offset * 60000` and reading the **UTC**
 *   fields ({@link dayText}, {@link dayTimeText}).
 *
 * **Nothing here calls a local-time method of `Date`** — no `getDate()`, no
 * `new Date(y, m, d)`, no `getTimezoneOffset()` — except
 * {@link computerOffsetMinutes}, whose whole job is to ask what this
 * computer's offset is so that it can be offered as the setting. With a
 * browser three hours east of a business on UTC, the old local-midnight
 * conversion put "starts on 1 October" three hours inside September, and the
 * September run billed 208 dinars for a month before the service began.
 *
 * Which offset to pass is the caller's decision and there are two: the
 * setting, for anything new (a plan's start, a payment's date, a month to
 * bill, a report's window), and **the invoice's own** `utc_offset_minutes`
 * for the dates on an existing invoice, which stay days of the calendar it
 * was written in when the setting later changes.
 *
 * An offset of `null` means the calendar is not known — the setting has not
 * been read yet, or holds something that is not a calendar. Then there is no
 * date to show and none to accept: every function here answers with nothing
 * rather than falling back to the browser's zone or to UTC.
 */

import { EMPTY } from '@/lib/format';

export const MINUTE_MS = 60_000;
export const DAY_MS = 86_400_000;

/** UTC−12:00, the westernmost offset anybody keeps (`calendar.rs`). */
export const MIN_OFFSET_MINUTES = -720;
/** UTC+14:00, the easternmost. */
export const MAX_OFFSET_MINUTES = 840;

/** An offset, or the fact that the calendar is not known. */
export type Offset = number | null;

/** Is this a calendar the engine accepts? Anything else is refused there too. */
export function isOffset(minutes: unknown): minutes is number {
  return (
    typeof minutes === 'number' &&
    Number.isInteger(minutes) &&
    minutes >= MIN_OFFSET_MINUTES &&
    minutes <= MAX_OFFSET_MINUTES
  );
}

const pad = (value: number, width = 2): string => String(value).padStart(width, '0');

/** `UTC+03:00`, as the engine names a calendar. */
export function offsetName(minutes: number): string {
  const magnitude = Math.abs(minutes);
  return `UTC${minutes < 0 ? '-' : '+'}${pad(Math.floor(magnitude / 60))}:${pad(magnitude % 60)}`;
}

/**
 * How far this computer's clock is ahead of UTC right now, in minutes — the
 * one place the browser's zone is read, and only to be offered as the
 * business's calendar. It is a reading of this moment: a zone with daylight
 * saving answers differently in summer, which is why the setting is a fixed
 * offset and says so.
 */
export function computerOffsetMinutes(now: number = Date.now()): number {
  return -Math.round(new Date(now).getTimezoneOffset());
}

/** The calendar's wall clock for an instant, as a `Date` to read UTC fields off. */
function wallClock(at: number, offset: number): Date | null {
  const shifted = new Date(at + offset * MINUTE_MS);
  return Number.isNaN(shifted.getTime()) ? null : shifted;
}

/**
 * A `YYYY-MM-DD` from a date field, as the first moment of that day in the
 * calendar. Text that is not a date — half typed, or a day that does not
 * exist — is no date, not today and not the nearest real one.
 */
export function dayStart(text: string, offset: Offset): number | null {
  if (offset === null) return null;
  const found = /^(\d{4})-(\d{2})-(\d{2})$/.exec(text.trim());
  if (!found) return null;
  const year = Number(found[1]);
  const month = Number(found[2]);
  const day = Number(found[3]);
  const utc = new Date(Date.UTC(year, month - 1, day));
  // `Date.UTC` rolls 31 February into March instead of refusing it.
  if (
    utc.getUTCFullYear() !== year ||
    utc.getUTCMonth() !== month - 1 ||
    utc.getUTCDate() !== day
  ) {
    return null;
  }
  return utc.getTime() - offset * MINUTE_MS;
}

/**
 * Which day an instant falls on, counted in whole days from 1 January 1970
 * in the calendar. Two instants are on the same day exactly when this is
 * equal — the same count the engine keeps (`Calendar::day`).
 */
export function dayNumber(at: number, offset: number): number {
  return Math.floor((at + offset * MINUTE_MS) / DAY_MS);
}

/** The date an instant falls on in the calendar: `2026-10-01`. */
export function dayText(at: number | null | undefined, offset: Offset): string {
  if (at == null || !Number.isFinite(at) || offset === null) return EMPTY;
  const clock = wallClock(at, offset);
  if (!clock) return EMPTY;
  return `${pad(clock.getUTCFullYear(), 4)}-${pad(clock.getUTCMonth() + 1)}-${pad(clock.getUTCDate())}`;
}

/** Date and time on the calendar's wall clock: `2026-10-01 14:03:22`. */
export function dayTimeText(at: number | null | undefined, offset: Offset): string {
  if (at == null || !Number.isFinite(at) || offset === null) return EMPTY;
  const clock = wallClock(at, offset);
  if (!clock) return EMPTY;
  return `${dayText(at, offset)} ${pad(clock.getUTCHours())}:${pad(clock.getUTCMinutes())}:${pad(
    clock.getUTCSeconds(),
  )}`;
}

/**
 * A period `[start, end)` as the days it covers, first to last: October is
 * `2026-10-01 → 2026-10-31`. The end is the first moment *after* the period,
 * so the last day is the one the millisecond before it falls on; writing the
 * end itself would show every month running into the first of the next.
 */
export function periodText(start: number, end: number, offset: Offset): string {
  return `${dayText(start, offset)} → ${dayText(end - 1, offset)}`;
}

// ------------------------------------------------------------------- months

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

function parseMonth(value: string): { year: number; month: number } | null {
  const found = /^(\d{4})-(\d{2})$/.exec(value.trim());
  if (!found) return null;
  const year = Number(found[1]);
  const month = Number(found[2]);
  return month >= 1 && month <= 12 ? { year, month } : null;
}

const monthName = (year: number, month: number): string => `${pad(year, 4)}-${pad(month)}`;

/** The month an instant falls in, in the calendar: `2026-10`. */
export function monthOf(at: number, offset: number): string {
  const clock = wallClock(at, offset);
  return clock ? monthName(clock.getUTCFullYear(), clock.getUTCMonth() + 1) : '';
}

/** The month before `2026-01` is `2025-12`. Text that is not a month has none. */
export function previousMonth(value: string): string | null {
  const parsed = parseMonth(value);
  if (!parsed) return null;
  return parsed.month === 1
    ? monthName(parsed.year - 1, 12)
    : monthName(parsed.year, parsed.month - 1);
}

/**
 * `2026-10` as `[start, end)` in the calendar: from the first moment of its
 * first day to the first moment of the next month's (`Calendar::month`).
 *
 * This is for *reading* — a list's filter, a report's window, the dates a
 * hint shows. A billing run and a draft do not send it: they name the month
 * (`month: "2026-10"`) and the engine resolves it, so no browser's
 * arithmetic stands between an operator and the period that is billed.
 */
export function monthWindow(value: string, offset: Offset): { start: number; end: number } | null {
  const parsed = parseMonth(value);
  if (!parsed || offset === null) return null;
  const { year, month } = parsed;
  return {
    start: Date.UTC(year, month - 1, 1) - offset * MINUTE_MS,
    end: Date.UTC(month === 12 ? year + 1 : year, month === 12 ? 0 : month, 1) - offset * MINUTE_MS,
  };
}

/** The months to offer, newest first, counted in the calendar: `2026-10`, `2026-09`, … */
export function recentMonths(
  now: number,
  offset: Offset,
  count = 14,
): { value: string; label: string }[] {
  if (offset === null) return [];
  const months: { value: string; label: string }[] = [];
  let value: string | null = monthOf(now, offset);
  for (let index = 0; index < count && value; index += 1) {
    const parsed = parseMonth(value);
    if (!parsed) break;
    months.push({ value, label: `${MONTH_NAMES[parsed.month - 1] ?? ''} ${parsed.year}` });
    value = previousMonth(value);
  }
  return months;
}

/**
 * Is this invoice's period part of the month named — asked in the calendar
 * the invoice was **written in**, not the one set now.
 *
 * `isp.invoices {since, until}` selects by instants, and after the setting
 * has been changed an October written under the old offset overlaps the new
 * calendar's November by the hours the two disagree about. Read in its own
 * calendar it is October and nothing else, which is what a list filtered to
 * November should say.
 */
export function periodInMonth(
  invoice: { period_start: number; period_end: number; utc_offset_minutes: number },
  month: string,
): boolean {
  const window = monthWindow(month, invoice.utc_offset_minutes);
  return window !== null && invoice.period_start < window.end && invoice.period_end > window.start;
}

/**
 * What `isp.invoices` is asked for, to list one month: that month's own
 * bounds and nothing wider.
 *
 * The engine selects by instants (a period that ends after `since` and
 * starts before `until`), newest period first, a page at a time. Months
 * meet exactly, so a window widened at all takes in every invoice of the
 * month after — which fills the page ahead of this month's, to be taken off
 * the screen again by {@link periodInMonth}; with fifty of them, a list
 * filtered to October showed nothing. The exact window still reaches every
 * invoice written for the month under another offset, because a month and
 * the same month a few hours along overlap; what it also brings is a
 * neighbouring month's invoices that were written under another offset, and
 * {@link periodInMonth} is for those.
 */
export function monthQuery(value: string, offset: Offset): { since: number; until: number } | null {
  const window = monthWindow(value, offset);
  return window ? { since: window.start, until: window.end } : null;
}

// --------------------------------------------------------------- the offsets

/**
 * Every offset the setting's control offers: the quarter hours from
 * UTC−12:00 to UTC+14:00, which covers every offset in use (Nepal is
 * +05:45, the Chatham Islands +12:45).
 */
export function offsetChoices(): { value: number; label: string }[] {
  const choices: { value: number; label: string }[] = [];
  for (let minutes = MIN_OFFSET_MINUTES; minutes <= MAX_OFFSET_MINUTES; minutes += 15) {
    choices.push({ value: minutes, label: offsetName(minutes) });
  }
  return choices;
}
