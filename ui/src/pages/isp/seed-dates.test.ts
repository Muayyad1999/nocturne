import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { beforeAll, describe, expect, it } from 'vitest';
import { dueText, prorationText } from './billing-rows';
import {
  dayNumber,
  dayStart,
  dayText,
  monthOf,
  monthWindow,
  periodText,
  previousMonth,
  recentMonths,
} from './calendar';
import {
  creditNoteNumber,
  dayIn,
  invoiceNumber,
  monthIn,
  monthsBefore,
  seedDates,
  yearIn,
  type SeedDates,
} from './seed-dates';

const MINUTE = 60_000;
const DAY = 86_400_000;

const UTC = 0;
const BAGHDAD = 180;
const NEW_YORK_WINTER = -300;
const KATHMANDU = 345;
/** The two calendars furthest apart that the engine accepts (`calendar.rs`). */
const EASTERNMOST = 840;
const WESTERNMOST = -720;

/**
 * The calendars the seed might be written in, and the ones its invoices might
 * then be read in: the spec starts on UTC and moves the business to Baghdad
 * half way through, and the engine decides "overdue" by today in the calendar
 * that is set, against the due day in the calendar the invoice was written in.
 */
const CALENDARS = [UTC, BAGHDAD, NEW_YORK_WINTER, KATHMANDU, EASTERNMOST, WESTERNMOST];

/** `2026-12-02T09:00` on a calendar's wall clock, as the instant it is. */
function at(text: string, offset: number): number {
  return Date.parse(`${text}Z`) - offset * MINUTE;
}

/**
 * Every midnight from 2026 to the end of 2030 in each calendar, with the
 * instant before it and the middle of the day after it. The first and the
 * last instant of every month and of every year, 29 February 2028 and the
 * days either side of it are all among them.
 */
const EVERY_MIDNIGHT: readonly { now: number; offset: number }[] = CALENDARS.flatMap((offset) => {
  const moments: { now: number; offset: number }[] = [];
  for (let day = Date.UTC(2026, 0, 1); day <= Date.UTC(2031, 0, 1); day += DAY) {
    const midnight = day - offset * MINUTE;
    moments.push(
      { now: midnight - 1, offset },
      { now: midnight, offset },
      { now: midnight + DAY / 2, offset },
    );
  }
  return moments;
});

/** The first few moments at which a rule does not hold, named so a failure can be read. */
function broken(holds: (dates: SeedDates) => boolean): string[] {
  return EVERY_MIDNIGHT.filter(({ now, offset }) => !holds(seedDates(now, offset)))
    .slice(0, 5)
    .map(
      ({ now, offset }) =>
        `${new Date(now).toISOString()}, in a calendar ${offset} minutes ahead of UTC`,
    );
}

const monthsOf = (dates: SeedDates) => [
  dates.earliest,
  dates.earlier,
  dates.billed,
  dates.following,
];

/** The open invoice as the engine would describe it while it is not late. */
function openInvoice(dates: SeedDates) {
  return {
    state: 'issued' as const,
    status: 'part_paid' as const,
    due_at: dates.open.dueAt,
    days_overdue: null,
    outstanding: { currency: 'USD' as const, minor: 22_500, digits: 2, text: '225.00' },
    utc_offset_minutes: dates.offset,
  };
}

/**
 * As in `calendar.test.ts`: the process is in a zone that is none of the
 * calendars used here, so a helper that read the local zone would be caught.
 */
beforeAll(() => {
  process.env.TZ = 'Pacific/Auckland';
  expect(new Date(2025, 9, 1).getTimezoneOffset()).toBe(-780);
});

describe('the dates the ISP seed writes', () => {
  it('are October’s when it runs on 22 November 2026: overdue since the 10th, and the open invoice due three weeks on', () => {
    const dates = seedDates(at('2026-11-22T12:00', UTC), UTC);
    expect(dates.billed).toEqual({
      name: '2026-10',
      start: Date.UTC(2026, 9, 1),
      end: Date.UTC(2026, 10, 1),
      first: '2026-10-01',
      last: '2026-10-31',
      days: 31,
    });
    expect(monthsOf(dates).map((month) => month.name)).toEqual([
      '2026-08',
      '2026-09',
      '2026-10',
      '2026-11',
    ]);
    expect(dates.plansStart).toBe(Date.UTC(2026, 7, 1));
    expect(dates.partPlanStart).toBe(Date.UTC(2026, 9, 16));
    expect(dates.partPeriod).toEqual({
      coveredDays: 16,
      periodDays: 31,
      words: '16 of the period’s 31 days',
    });
    expect(dates.overdue).toEqual({
      issuedAt: Date.UTC(2026, 9, 2),
      dueAt: Date.UTC(2026, 9, 10),
      number: '2026-0001',
    });
    // Three weeks from the day the seed runs, and not the first of a month:
    // a fixed day comes a day closer every day, and then the invoice is late.
    expect(dates.open).toEqual({
      issuedAt: Date.UTC(2026, 10, 1),
      dueAt: Date.UTC(2026, 11, 13),
      number: '2026-0002',
    });
  });

  it('move with the clock: on 2 December 2026 the billed month is November and the open invoice is still not due', () => {
    const now = at('2026-12-02T09:00', UTC);
    const dates = seedDates(now, UTC);
    expect(dates.billed.name).toBe('2026-11');
    expect(dates.following.name).toBe('2026-12');
    expect(dayIn(dates.overdue.dueAt, UTC)).toBe('2026-11-10');
    expect(dayIn(dates.open.dueAt, UTC)).toBe('2026-12-23');
    expect(dueText(openInvoice(dates), { now, offset: UTC })).toBe('Not due yet');
    expect(dueText(openInvoice(dates), { now, offset: BAGHDAD })).toBe('Not due yet');
  });

  it('put the overdue invoice at least two days past its due day in every calendar it could be read in', () => {
    expect(
      broken((dates) =>
        CALENDARS.every(
          (read) => dayNumber(dates.now, read) - dayNumber(dates.overdue.dueAt, dates.offset) >= 2,
        ),
      ),
    ).toEqual([]);
  });

  it('put the open invoice at least two days short of its due day in every calendar, a day after the seed ran too', () => {
    expect(
      broken((dates) =>
        CALENDARS.every((read) =>
          // A run takes minutes and may cross a midnight; a day covers it.
          [dates.now, dates.now + DAY].every(
            (later) =>
              dayNumber(dates.open.dueAt, dates.offset) - dayNumber(later, read) >= 2 &&
              dueText(openInvoice(dates), { now: later, offset: read }) === 'Not due yet',
          ),
        ),
      ),
    ).toEqual([]);
  });

  it('start the part-period plan on a day strictly inside the billed month, and say so in the screen’s words', () => {
    expect(
      broken((dates) => {
        const { billed, partPlanStart, partPeriod, offset } = dates;
        const covered = billed.end - partPlanStart;
        const line = {
          covered_ms: covered,
          period_ms: billed.end - billed.start,
          // The engine's own count: whole days, to the nearest day.
          covered_days: Math.round(covered / DAY),
          period_days: billed.days,
          exact: true,
          basis: '',
        };
        return (
          partPlanStart > billed.start &&
          partPlanStart < billed.end &&
          // The first moment of a day, as a plan sold "from the 16th" is.
          dayStart(dayText(partPlanStart, offset), offset) === partPlanStart &&
          partPeriod.coveredDays === line.covered_days &&
          partPeriod.coveredDays >= 1 &&
          partPeriod.coveredDays < billed.days &&
          partPeriod.periodDays === billed.days &&
          partPeriod.words === prorationText(line)
        );
      }),
    ).toEqual([]);
    // February is the short one, and a leap year's is a day longer.
    expect(seedDates(at('2027-03-15T12:00', UTC), UTC).partPeriod.words).toBe(
      '13 of the period’s 28 days',
    );
    expect(seedDates(at('2028-03-15T12:00', UTC), UTC).partPeriod.words).toBe(
      '14 of the period’s 29 days',
    );
    expect(seedDates(at('2028-02-29T12:00', UTC), UTC).billed).toMatchObject({
      name: '2028-01',
      days: 31,
    });
  });

  it('start the plans billed for whole months before the first month anything bills them for', () => {
    expect(
      broken(
        (dates) => dates.plansStart < dates.earlier.start && dates.plansStart < dates.billed.start,
      ),
    ).toEqual([]);
  });

  it('bill whole months of the business’s calendar that meet and never overlap', () => {
    expect(
      broken((dates) =>
        monthsOf(dates).every((month, index, months) => {
          const named = monthWindow(month.name, dates.offset);
          const previous = months[index - 1];
          return (
            // What the engine makes of the month's name (`Calendar::month`).
            named !== null &&
            named.start === month.start &&
            named.end === month.end &&
            // One whole monthly period, which is 28 to 31 days (`billing.rs`).
            month.end - month.start === month.days * DAY &&
            month.days >= 28 &&
            month.days <= 31 &&
            // Half-open windows: a period that overlaps one already billed is refused.
            (previous === undefined || previous.end === month.start)
          );
        }),
      ),
    ).toEqual([]);
  });

  it('issue no invoice in the future, before the plan it bills began, or to fall due before it is issued', () => {
    expect(
      broken((dates) =>
        [dates.overdue, dates.open].every(
          (invoice) =>
            invoice.issuedAt <= dates.now &&
            invoice.issuedAt > dates.plansStart &&
            dayNumber(invoice.dueAt, dates.offset) >= dayNumber(invoice.issuedAt, dates.offset),
        ),
      ),
    ).toEqual([]);
    // And nothing else it writes is still to come either.
    expect(
      broken((dates) => dates.plansStart < dates.now && dates.partPlanStart < dates.now),
    ).toEqual([]);
  });

  it('number the two invoices as the engine will: first and second of one year, except in January', () => {
    for (const offset of CALENDARS) {
      // The overdue one is issued in the billed month and the open one in the
      // month after it. On 1 January those are two different years, and each
      // year counts from one.
      const newYear = seedDates(at('2027-01-01T00:00', offset), offset);
      expect([newYear.overdue.number, newYear.open.number], String(offset)).toEqual([
        '2026-0001',
        '2027-0001',
      ]);
      const endOfJanuary = seedDates(at('2027-01-31T23:59:59.999', offset), offset);
      expect([endOfJanuary.overdue.number, endOfJanuary.open.number], String(offset)).toEqual([
        '2026-0001',
        '2027-0001',
      ]);
      // The instant before the year turns, both are still last year's…
      const newYearsEve = seedDates(at('2026-12-31T23:59:59.999', offset), offset);
      expect([newYearsEve.overdue.number, newYearsEve.open.number], String(offset)).toEqual([
        '2026-0001',
        '2026-0002',
      ]);
      // …and from February both are this year's.
      const february = seedDates(at('2027-02-01T00:00', offset), offset);
      expect([february.overdue.number, february.open.number], String(offset)).toEqual([
        '2027-0001',
        '2027-0002',
      ]);
    }
    // The year is the one the day of issue falls in, in the invoice's calendar.
    expect(
      broken((dates) => {
        const overdueYear = yearIn(dates.overdue.issuedAt, dates.offset);
        const openYear = yearIn(dates.open.issuedAt, dates.offset);
        return (
          dates.overdue.number === invoiceNumber(overdueYear, 1) &&
          dates.open.number === invoiceNumber(openYear, openYear === overdueYear ? 2 : 1)
        );
      }),
    ).toEqual([]);
  });

  it('take the business’s month on both sides of its midnight, east and west of UTC', () => {
    for (const offset of [BAGHDAD, NEW_YORK_WINTER, KATHMANDU, EASTERNMOST, WESTERNMOST]) {
      const midnight = at('2026-12-01T00:00', offset);
      expect(seedDates(midnight - 1, offset).billed.name, String(offset)).toBe('2026-10');
      expect(seedDates(midnight, offset).billed.name, String(offset)).toBe('2026-11');
      // The day the open invoice falls due is counted from the business's today.
      expect(dayIn(seedDates(midnight - 1, offset).open.dueAt, offset), String(offset)).toBe(
        '2026-12-21',
      );
      expect(dayIn(seedDates(midnight, offset).open.dueAt, offset), String(offset)).toBe(
        '2026-12-22',
      );
    }
    // One instant, three calendars: ten in the evening of 30 November in UTC
    // is already December in Baghdad and still the afternoon in New York.
    const instant = Date.UTC(2026, 10, 30, 22);
    expect(seedDates(instant, UTC).billed.name).toBe('2026-10');
    expect(seedDates(instant, BAGHDAD).billed.name).toBe('2026-11');
    expect(seedDates(instant, NEW_YORK_WINTER).billed.name).toBe('2026-10');
    expect(seedDates(instant, BAGHDAD).billed.start).toBe(Date.UTC(2026, 9, 31, 21));
  });

  it('name months the screens’ month controls offer, before and after the business moves its calendar', () => {
    expect(
      broken((dates) => {
        const offered = recentMonths(dates.now, dates.offset).map((month) => month.value);
        return (
          monthOf(dates.now, dates.offset) === dates.following.name &&
          offered[0] === dates.following.name &&
          offered[1] === dates.billed.name &&
          offered[2] === dates.earlier.name &&
          offered[3] === dates.earliest.name
        );
      }),
    ).toEqual([]);
    // The month after the billed one is picked while the calendar is still
    // the seed's own; a run that crossed into the next month is offered it too.
    expect(
      broken((dates) =>
        recentMonths(dates.now + DAY, dates.offset).some(
          (month) => month.value === dates.following.name,
        ),
      ),
    ).toEqual([]);
    // The months that are over are picked after the calendar has moved, and
    // are on offer in every calendar: a month over in one is at most the
    // current month in another.
    expect(
      broken((dates) =>
        CALENDARS.every((read) => {
          const offered = recentMonths(dates.now + DAY, read).map((month) => month.value);
          return [dates.billed, dates.earlier, dates.earliest].every((month) =>
            offered.includes(month.name),
          );
        }),
      ),
    ).toEqual([]);
  });
});

describe('a day, a month and a number in the spec', () => {
  it('are written the way the screens write them', () => {
    expect(
      broken((dates) => {
        const { now, offset } = dates;
        return monthsOf(dates).every(
          (month) =>
            dayIn(now, offset) === dayText(now, offset) &&
            month.first === dayText(month.start, offset) &&
            `${month.first} → ${month.last}` === periodText(month.start, month.end, offset) &&
            monthsBefore(month.name, 1) === previousMonth(month.name),
        );
      }),
    ).toEqual([]);
  });

  it('count months back across a year’s end, and a month’s days in the calendar asked for', () => {
    expect(monthsBefore('2026-10', 1)).toBe('2026-09');
    expect(monthsBefore('2027-01', 1)).toBe('2026-12');
    expect(monthsBefore('2027-03', 8)).toBe('2026-07');
    expect(monthsBefore('2026-11', 0)).toBe('2026-11');
    expect(monthIn('2026-12', BAGHDAD)).toEqual({
      start: Date.UTC(2026, 10, 30, 21),
      end: Date.UTC(2026, 11, 31, 21),
      first: '2026-12-01',
      last: '2026-12-31',
    });
    expect(monthIn('2028-02', NEW_YORK_WINTER)).toEqual({
      start: Date.UTC(2028, 1, 1, 5),
      end: Date.UTC(2028, 2, 1, 5),
      first: '2028-02-01',
      last: '2028-02-29',
    });
  });

  it('read a day and a year off the business’s wall clock, not UTC’s and not this computer’s', () => {
    // Nine in the evening of 31 December in UTC is already next year in Baghdad.
    const instant = Date.UTC(2026, 11, 31, 21);
    expect(dayIn(instant, UTC)).toBe('2026-12-31');
    expect(dayIn(instant, BAGHDAD)).toBe('2027-01-01');
    expect(dayIn(instant - 1, BAGHDAD)).toBe('2026-12-31');
    expect(yearIn(instant, UTC)).toBe(2026);
    expect(yearIn(instant, BAGHDAD)).toBe(2027);
    expect(yearIn(instant - 1, BAGHDAD)).toBe(2026);
    expect(yearIn(Date.UTC(2027, 0, 1, 11, 59), WESTERNMOST)).toBe(2026);
  });

  it('number an invoice and a credit note as the engine does: the year, then four digits', () => {
    expect(invoiceNumber(2026, 1)).toBe('2026-0001');
    expect(invoiceNumber(2027, 12)).toBe('2027-0012');
    expect(creditNoteNumber(2026, 3)).toBe('CN-2026-0003');
    expect(creditNoteNumber(2027, 1)).toBe('CN-2027-0001');
  });
});

describe('the seed, the spec and the screenshots', () => {
  // The helper above can be right and a date still be typed in beside it. A
  // typed date is right on the day it is typed, so no run of anything on
  // that day can tell: the end-to-end spec notices only once the month has
  // turned, which is the failure this module exists to end. What can be
  // seen today is the typing, so that is what is looked for.
  const here = dirname(fileURLToPath(import.meta.url));
  const FILES = [
    '../../../tests/support/isp.ts',
    '../../../tests/isp.spec.ts',
    '../../../tests/isp.shots.ts',
  ];
  /** Code only: a comment may show what a date looks like. */
  const code = (source: string) =>
    source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
  const TYPED_IN = [
    // `2026-11`, `2026-11-01`, a document number (`2026-0001`, `CN-2026-0001`)
    // and a pattern for one (`2026-\d{4}`).
    /\b(?:19|20)\d{2}-(?:\d{2}|\\d)/,
    /Date\.UTC\(\s*\d/,
    /new Date\(\s*['"`\d]/,
    // What a month control shows: `November 2026`.
    /\b(?:January|February|March|April|May|June|July|August|September|October|November|December)\s+(?:19|20)\d{2}\b/,
  ];

  it.each(FILES)(
    '%s types in no year, month, day or document number: each is counted from the clock',
    (path) => {
      const lines = code(readFileSync(resolve(here, path), 'utf8')).split('\n');
      expect(lines.length).toBeGreaterThan(50);
      expect(lines.filter((line) => TYPED_IN.some((typed) => typed.test(line)))).toEqual([]);
    },
  );

  it('would be caught typing any of the dates the old seed had', () => {
    const typed = (line: string) => TYPED_IN.some((pattern) => pattern.test(code(line)));
    for (const line of [
      'due_at: Date.UTC(2026, 11, 1),',
      "await dialog.getByLabel('Period').selectOption('2026-11');",
      "const drawer = page.getByRole('dialog', { name: '2026-0002' });",
      "await expect(notes).toContainText('CN-2026-0001');",
      'page.getByText(/Invoice 2026-\\d{4} is issued to Zagros Net/)',
      "await period.selectOption({ label: 'August 2026' });",
      "started_at: new Date('2026-10-16').getTime(),",
      'started_at: +new Date(2026, 9, 16),',
    ]) {
      expect(typed(line), line).toBe(true);
    }
    // And what these files do write is left alone: dates worked out from
    // the clock, amounts, and a date in a comment.
    for (const line of [
      'expect(plans[0]?.started_at).toBe(Date.UTC(next.year, next.month - 1, 1) - offset * 60_000);',
      'expect(paid[0]?.at).toBe(Date.parse(`${today}T00:00:00Z`) - offset * 60_000);',
      'new RegExp(`CN-${noteYear()}-\\d{4}: 1\\s*000\\s*IQD credited on invoice ${overdue}`)',
      '`${process.env.NOCTURNE_SERVER_BIN ?? \'unset\'}.\\n${backend.log().slice(-2000)}`,',
      "phone: '+964 750 000 0001',",
      '  // `2026-11-22`: the day an instant falls on',
    ]) {
      expect(typed(line), line).toBe(false);
    }
  });
});
