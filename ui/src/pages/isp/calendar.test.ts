import { beforeAll, describe, expect, it } from 'vitest';
import {
  computerOffsetMinutes,
  dayNumber,
  dayStart,
  dayText,
  dayTimeText,
  isOffset,
  monthOf,
  monthQuery,
  monthWindow,
  offsetChoices,
  offsetName,
  periodInMonth,
  periodText,
  previousMonth,
  recentMonths,
  revenueWindow,
  timeText,
} from './calendar';

/** The business's calendars these tests use. */
const BAGHDAD = 180;
const UTC = 0;
const NEW_YORK_WINTER = -300;
const KATHMANDU = 345;

/**
 * Every test here runs with the process in a zone that is **not** the
 * business's: that is the situation the module exists for, and under UTC a
 * helper that quietly read the browser's zone would pass every assertion.
 * Auckland is thirteen hours ahead of UTC in October — further east than any
 * calendar used below — so a local-midnight conversion lands on the day
 * before in every one of them.
 */
beforeAll(() => {
  process.env.TZ = 'Pacific/Auckland';
  // If the zone did not take, these tests would prove nothing: say so.
  expect(new Date(2025, 9, 1).getTimezoneOffset()).toBe(-780);
});

describe('a date somebody types', () => {
  it('is the first moment of that day in the business calendar, whatever zone the browser is in', () => {
    // The finding's own case: Baghdad, a plan that "starts on 1 October".
    // Midnight there is 21:00 UTC the evening before — not Auckland's
    // midnight (11:00 UTC on the 30th) and not UTC's.
    expect(dayStart('2025-10-01', BAGHDAD)).toBe(Date.UTC(2025, 8, 30, 21));
    expect(dayStart('2025-10-01', UTC)).toBe(Date.UTC(2025, 9, 1));
    expect(dayStart('2025-10-01', NEW_YORK_WINTER)).toBe(Date.UTC(2025, 9, 1, 5));
    expect(dayStart('2025-10-01', KATHMANDU)).toBe(Date.UTC(2025, 8, 30, 18, 15));
  });

  it('lands exactly where that month begins, so a plan from the 1st is in none of the month before', () => {
    for (const offset of [BAGHDAD, UTC, NEW_YORK_WINTER, KATHMANDU, -720, 840]) {
      const october = monthWindow('2025-10', offset);
      const september = monthWindow('2025-09', offset);
      const typed = dayStart('2025-10-01', offset);
      expect(typed, String(offset)).toBe(october?.start);
      // Half-open windows that meet: nothing of October is in September.
      expect(september?.end, String(offset)).toBe(typed);
    }
  });

  it('falls on the day that was typed, so a due date of today is today and not yesterday', () => {
    for (const offset of [BAGHDAD, UTC, NEW_YORK_WINTER, KATHMANDU, -720, 840]) {
      const typed = dayStart('2025-12-01', offset);
      expect(typed).not.toBeNull();
      expect(dayText(typed, offset), String(offset)).toBe('2025-12-01');
      // The last moment of that day is still that day; the next is not.
      expect(dayNumber((typed ?? 0) + 86_399_999, offset)).toBe(dayNumber(typed ?? 0, offset));
      expect(dayNumber((typed ?? 0) + 86_400_000, offset)).toBe(dayNumber(typed ?? 0, offset) + 1);
      expect(dayNumber((typed ?? 0) - 1, offset)).toBe(dayNumber(typed ?? 0, offset) - 1);
    }
  });

  it('is no date when it is half typed, impossible, or the calendar is not known', () => {
    expect(dayStart('', BAGHDAD)).toBeNull();
    expect(dayStart('2026-10', BAGHDAD)).toBeNull();
    expect(dayStart('tomorrow', BAGHDAD)).toBeNull();
    // 31 February is not rolled into March.
    expect(dayStart('2026-02-31', BAGHDAD)).toBeNull();
    expect(dayStart('2026-13-01', BAGHDAD)).toBeNull();
    // Without the business's calendar there is no instant to give: not the
    // browser's midnight, and not UTC's.
    expect(dayStart('2026-10-02', null)).toBeNull();
  });
});

describe('a date that is shown', () => {
  it('is the day in the calendar it is asked in, not the browser’s', () => {
    // 22:30 UTC on 30 September: already 1 October in Baghdad and in
    // Auckland, still 30 September in UTC and New York.
    const at = Date.UTC(2025, 8, 30, 22, 30, 5);
    expect(dayText(at, BAGHDAD)).toBe('2025-10-01');
    expect(dayText(at, UTC)).toBe('2025-09-30');
    expect(dayText(at, NEW_YORK_WINTER)).toBe('2025-09-30');
    expect(dayTimeText(at, BAGHDAD)).toBe('2025-10-01 01:30:05');
    expect(dayTimeText(at, UTC)).toBe('2025-09-30 22:30:05');
  });

  it('shows a period as the days it covers, first to last', () => {
    const october = monthWindow('2025-10', BAGHDAD);
    expect(periodText(october?.start ?? 0, october?.end ?? 0, BAGHDAD)).toBe(
      '2025-10-01 → 2025-10-31',
    );
    // The same instants read in another calendar are other days — which is
    // why an invoice's dates are read in the calendar it was written in.
    expect(periodText(october?.start ?? 0, october?.end ?? 0, UTC)).toBe('2025-09-30 → 2025-10-31');
  });

  it('shows nothing rather than a guess when there is no instant or no calendar', () => {
    expect(dayText(null, BAGHDAD)).toBe('—');
    expect(dayText(undefined, BAGHDAD)).toBe('—');
    expect(dayText(Date.UTC(2025, 9, 1), null)).toBe('—');
    expect(dayTimeText(Date.UTC(2025, 9, 1), null)).toBe('—');
    expect(dayText(Number.NaN, BAGHDAD)).toBe('—');
  });
});

describe('a month', () => {
  it('begins and ends at the business’s midnight, as the engine’s does', () => {
    // `calendar.rs`: month bounds at +180, -240 and +345.
    expect(monthWindow('2025-10', BAGHDAD)).toEqual({
      start: Date.UTC(2025, 8, 30, 21),
      end: Date.UTC(2025, 9, 31, 21),
    });
    expect(monthWindow('2025-10', -240)).toEqual({
      start: Date.UTC(2025, 9, 1, 4),
      end: Date.UTC(2025, 10, 1, 4),
    });
    // December runs into the next year, not into a thirteenth month.
    expect(monthWindow('2025-12', KATHMANDU)).toEqual({
      start: Date.UTC(2025, 10, 30, 18, 15),
      end: Date.UTC(2025, 11, 31, 18, 15),
    });
    expect(monthWindow('2025-13', BAGHDAD)).toBeNull();
    expect(monthWindow('nonsense', BAGHDAD)).toBeNull();
    expect(monthWindow('2025-10', null)).toBeNull();
  });

  it('is the business’s month at this moment, which is not always the browser’s', () => {
    // 22:00 UTC on 31 December: January already in Baghdad, December in UTC.
    const at = Date.UTC(2025, 11, 31, 22);
    expect(monthOf(at, BAGHDAD)).toBe('2026-01');
    expect(monthOf(at, UTC)).toBe('2025-12');
    expect(previousMonth('2026-01')).toBe('2025-12');
    expect(previousMonth('2026-03')).toBe('2026-02');
    expect(previousMonth('soon')).toBeNull();
  });

  it('offers recent months newest first, across a year boundary, in the business calendar', () => {
    const months = recentMonths(Date.UTC(2025, 11, 31, 22), BAGHDAD, 3);
    expect(months.map((month) => month.value)).toEqual(['2026-01', '2025-12', '2025-11']);
    expect(months[0]?.label).toBe('January 2026');
    expect(recentMonths(Date.UTC(2025, 11, 31, 22), UTC, 2).map((month) => month.value)).toEqual([
      '2025-12',
      '2025-11',
    ]);
    // No calendar, no months to offer: not the browser's.
    expect(recentMonths(Date.now(), null)).toEqual([]);
  });

  it('is listed by asking for that month alone, so a full page of the next month cannot hide it', () => {
    // `isp.invoices` selects a period that ends after `since` and starts
    // before `until`, newest period first, fifty to a page.
    const selects = (
      query: { since: number; until: number } | null,
      period: { start: number; end: number } | null,
    ) => query !== null && period !== null && period.end > query.since && period.start < query.until;
    for (const offset of [BAGHDAD, UTC, NEW_YORK_WINTER, KATHMANDU]) {
      const october = monthQuery('2025-10', offset);
      expect(selects(october, monthWindow('2025-10', offset)), String(offset)).toBe(true);
      // The months either side meet it exactly and are not part of it. Asked
      // for any wider, every November invoice is on the page too — ahead of
      // October's, and then taken off the screen again, which left a list
      // filtered to October showing nothing while there were fifty or more
      // November invoices.
      expect(selects(october, monthWindow('2025-11', offset)), String(offset)).toBe(false);
      expect(selects(october, monthWindow('2025-09', offset)), String(offset)).toBe(false);
    }
    expect(monthQuery('2025-10', null)).toBeNull();
    expect(monthQuery('soon', BAGHDAD)).toBeNull();
  });

  it('holds an invoice by the calendar the invoice was written in, after the setting has moved', () => {
    // October, billed while the business was on UTC.
    const written = monthWindow('2025-10', UTC);
    const invoice = {
      period_start: written?.start ?? 0,
      period_end: written?.end ?? 0,
      utc_offset_minutes: UTC,
    };
    expect(periodInMonth(invoice, '2025-10')).toBe(true);
    // The setting is now Baghdad, whose November starts three hours before
    // that invoice ends — in milliseconds the two overlap. It is still not a
    // November invoice.
    const november = monthWindow('2025-11', BAGHDAD);
    expect(invoice.period_end).toBeGreaterThan(november?.start ?? 0);
    expect(periodInMonth(invoice, '2025-11')).toBe(false);
    expect(periodInMonth(invoice, '2025-09')).toBe(false);
  });
});

describe('the calendar itself', () => {
  it('is named the way the engine names it', () => {
    expect(offsetName(180)).toBe('UTC+03:00');
    expect(offsetName(0)).toBe('UTC+00:00');
    expect(offsetName(-240)).toBe('UTC-04:00');
    expect(offsetName(345)).toBe('UTC+05:45');
    expect(offsetName(-570)).toBe('UTC-09:30');
  });

  it('accepts what the engine accepts and nothing else', () => {
    for (const good of [-720, -1, 0, 180, 345, 840]) expect(isOffset(good), String(good)).toBe(true);
    for (const bad of [-721, 841, 9999, 1.5, Number.NaN, '180', null, undefined]) {
      expect(isOffset(bad), String(bad)).toBe(false);
    }
  });

  it('offers every quarter hour from UTC−12:00 to UTC+14:00', () => {
    const choices = offsetChoices();
    expect(choices[0]).toEqual({ value: -720, label: 'UTC-12:00' });
    expect(choices.at(-1)).toEqual({ value: 840, label: 'UTC+14:00' });
    expect(choices).toHaveLength(105);
    expect(choices.some((choice) => choice.value === 345)).toBe(true);
    expect(choices.every((choice) => isOffset(choice.value))).toBe(true);
  });

  it('reads this computer’s offset only to offer it as the setting', () => {
    // Auckland in October (daylight time) and in June (standard time): the
    // reading is of the moment asked about, which is why the setting is a
    // fixed offset and says so.
    expect(computerOffsetMinutes(Date.UTC(2025, 9, 1))).toBe(780);
    expect(computerOffsetMinutes(Date.UTC(2025, 5, 1))).toBe(720);
  });
});

describe('the window a revenue report asks for', () => {
  const now = Date.UTC(2026, 10, 23, 9, 30);

  it('leaves the end of a rolling window to the engine, so what is written later is in it', () => {
    // An end fixed when the window was chosen left a payment recorded a
    // minute later out of every refetch that followed.
    expect(revenueWindow('30d', now, BAGHDAD)).toEqual({ since: now - 30 * 86_400_000 });
    expect(revenueWindow('12m', now, BAGHDAD)).toEqual({ since: now - 365 * 86_400_000 });
    expect(revenueWindow('30d', now, BAGHDAD)).not.toHaveProperty('until');
  });

  it('gives a month both of its bounds, in the business’s calendar', () => {
    expect(revenueWindow('2026-10', now, BAGHDAD)).toEqual({
      since: Date.UTC(2026, 9, 1) - BAGHDAD * 60_000,
      until: Date.UTC(2026, 10, 1) - BAGHDAD * 60_000,
    });
  });

  it('falls back to the last thirty days for a month it cannot place', () => {
    expect(revenueWindow('2026-10', now, null)).toEqual({ since: now - 30 * 86_400_000 });
    expect(revenueWindow('soon', now, UTC)).toEqual({ since: now - 30 * 86_400_000 });
  });
});

describe('the time of day on the business’s clock', () => {
  it('reads the wall clock of the calendar, not of the browser', () => {
    const at = Date.UTC(2026, 10, 22, 21, 5, 9);
    expect(timeText(at, UTC)).toBe('21:05:09');
    expect(timeText(at, BAGHDAD)).toBe('00:05:09');
    expect(timeText(at, null)).toBe('—');
    expect(dayTimeText(at, BAGHDAD)).toBe('2026-11-23 00:05:09');
  });
});
