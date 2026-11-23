import { describe, expect, it } from 'vitest';
import type { CreditNote, InvoiceView, Payment, RunResult } from '@/api/types/isp';
import {
  dueText,
  invoiceActions,
  invoicePeriod,
  lineSubtitle,
  pageText,
  prorationText,
  runCountsText,
  runHasWork,
  runOutcome,
  settlement,
  statusWords,
  voidBlockedReason,
} from './billing-rows';
import { dayStart, monthWindow } from './calendar';

const usd = (minor: number, text: string) => ({ currency: 'USD' as const, minor, digits: 2, text });

function payment(over: Partial<Payment> = {}): Payment {
  return {
    id: 'p1',
    customer_id: 'c1',
    invoice_id: 'i1',
    amount: usd(2500, '25.00'),
    conversion: null,
    settled: usd(2500, '25.00'),
    settled_exact: true,
    method: 'cash',
    reference: '',
    note: '',
    at: 1_000,
    state: 'recorded',
    void_reason: '',
    voided_at: null,
    recorded_at: 1_000,
    ...over,
  };
}

function credit(over: Partial<CreditNote> = {}): CreditNote {
  return {
    id: 'cn1',
    number: 'CN-2026-0001',
    customer_id: 'c1',
    invoice_id: 'i1',
    amount: usd(10000, '100.00'),
    reason: 'Four days without service',
    note: '',
    at: 1_000,
    allocated_at: null,
    state: 'issued',
    void_reason: '',
    voided_at: null,
    recorded_at: 1_000,
    ...over,
  };
}

function invoice(over: Partial<InvoiceView> = {}): InvoiceView {
  const total = usd(8000, '80.00');
  return {
    id: 'i1',
    number: null,
    customer_id: 'c1',
    customer_name: 'Tigris Trading',
    currency: 'USD',
    state: 'draft',
    period_start: 1,
    period_end: 2,
    utc_offset_minutes: 0,
    lines: [],
    total,
    issued_at: null,
    due_at: null,
    voided_at: null,
    void_reason: '',
    replaced_by: null,
    replaces: null,
    note: '',
    created_at: 1,
    updated_at: 1,
    status: 'draft',
    paid: usd(0, '0.00'),
    credited: usd(0, '0.00'),
    outstanding: total,
    days_overdue: null,
    payments: [],
    credits: [],
    ...over,
  };
}

/** The finding's own invoice: 600.00, of which 200.00 was paid and 100.00 credited. */
function partCredited(): InvoiceView {
  return invoice({
    state: 'issued',
    status: 'part_paid',
    number: '2026-0007',
    total: usd(60000, '600.00'),
    paid: usd(20000, '200.00'),
    credited: usd(10000, '100.00'),
    outstanding: usd(30000, '300.00'),
    payments: [payment({ amount: usd(20000, '200.00'), settled: usd(20000, '200.00') })],
    credits: [credit()],
  });
}

function run(over: Partial<RunResult> = {}): RunResult {
  return {
    period_start: Date.UTC(2026, 9, 1),
    period_end: Date.UTC(2026, 10, 1),
    utc_offset_minutes: 0,
    dry_run: true,
    considered: 3,
    not_reached: 0,
    drafted: 2,
    issued: 0,
    invoices: [
      {
        customer_id: 'c1',
        invoice_id: null,
        number: null,
        total: { currency: 'USD', minor: 25000, digits: 2, text: '250.00' },
        lines: 1,
        skipped_subscriptions: [],
        not_issued: null,
      },
      {
        customer_id: 'c2',
        invoice_id: null,
        number: null,
        total: { currency: 'IQD', minor: 150000, digits: 0, text: '150000' },
        lines: 1,
        skipped_subscriptions: [
          { subscription_id: 's9', tariff_name: 'Home 5', reason: 'agreed monthly, 7 days asked' },
        ],
        not_issued: null,
      },
    ],
    skipped: [
      { customer_id: 'c3', reason: 'nothing to bill for that period', skipped_subscriptions: [] },
    ],
    totals: [
      { currency: 'IQD', minor: 150000, digits: 0, text: '150000' },
      { currency: 'USD', minor: 25000, digits: 2, text: '250.00' },
    ],
    warnings: [],
    note: 'nothing was written: this is what would be billed',
    ...over,
  };
}

describe('what may be done to an invoice', () => {
  it('offers an edit only on a draft, because an issued invoice is frozen', () => {
    expect(invoiceActions(invoice())).toEqual({
      edit: true,
      issue: true,
      void: true,
      reissue: false,
      pay: false,
      credit: false,
    });
    expect(invoiceActions(invoice({ state: 'issued', status: 'issued' }))).toEqual({
      edit: false,
      issue: false,
      void: true,
      reissue: true,
      pay: true,
      credit: true,
    });
  });

  it('does not offer to void an invoice with money recorded against it', () => {
    const paid = invoice({ state: 'issued', status: 'part_paid', payments: [payment()] });
    expect(invoiceActions(paid).void).toBe(false);
    expect(invoiceActions(paid).reissue).toBe(false);
    expect(voidBlockedReason(paid)).toContain('hide money that was actually received');
    expect(voidBlockedReason(invoice())).toBeNull();
  });

  it('does not offer to void or reissue an invoice a credit note stands against, and says which note', () => {
    // The engine refuses both with `conflict`; a button for either could
    // only produce that refusal.
    const credited = invoice({
      state: 'issued',
      status: 'issued',
      credited: usd(1000, '10.00'),
      outstanding: usd(7000, '70.00'),
      credits: [credit({ amount: usd(1000, '10.00') })],
    });
    expect(invoiceActions(credited).void).toBe(false);
    expect(invoiceActions(credited).reissue).toBe(false);
    const reason = voidBlockedReason(credited) ?? '';
    expect(reason).toContain('CN-2026-0001');
    expect(reason).toContain('void the note first');
    expect(reason).not.toContain('money that was actually received');
    // Both kinds standing: both are said.
    const both = voidBlockedReason({ ...credited, payments: [payment()] }) ?? '';
    expect(both).toContain('money that was actually received');
    expect(both).toContain('CN-2026-0001');
  });

  it('counts a voided payment or a void credit note as nothing standing, so the invoice can be voided again', () => {
    const withVoid = invoice({
      state: 'issued',
      status: 'issued',
      payments: [payment({ state: 'void', void_reason: 'entered twice' })],
      credits: [credit({ state: 'void', void_reason: 'wrong customer' })],
    });
    expect(invoiceActions(withVoid).void).toBe(true);
    expect(invoiceActions(withVoid).reissue).toBe(true);
    expect(voidBlockedReason(withVoid)).toBeNull();
  });

  it('offers no payment on an invoice with nothing left to pay', () => {
    // Credited in full: the finding's second invoice. A payment here could
    // only overpay it.
    const cleared = invoice({
      state: 'issued',
      status: 'credited',
      credited: usd(8000, '80.00'),
      outstanding: usd(0, '0.00'),
      credits: [credit({ amount: usd(8000, '80.00') })],
    });
    expect(invoiceActions(cleared)).toEqual({
      edit: false,
      issue: false,
      void: false,
      reissue: false,
      pay: false,
      // Its whole total has been credited: there is no room for another note.
      credit: false,
    });
    // Paid in full: nothing to pay, but a goodwill credit is still allowed —
    // the cap is what was charged, not what is left.
    const paid = invoice({
      state: 'issued',
      status: 'paid',
      paid: usd(8000, '80.00'),
      outstanding: usd(0, '0.00'),
      payments: [payment({ amount: usd(8000, '80.00'), settled: usd(8000, '80.00') })],
    });
    expect(invoiceActions(paid).pay).toBe(false);
    expect(invoiceActions(paid).credit).toBe(true);
    // Part settled: both are still on offer.
    expect(invoiceActions(partCredited())).toMatchObject({ pay: true, credit: true });
  });

  it('offers nothing at all on a void invoice, which is kept and not deleted', () => {
    expect(invoiceActions(invoice({ state: 'void', status: 'void' }))).toEqual({
      edit: false,
      issue: false,
      void: false,
      reissue: false,
      pay: false,
      credit: false,
    });
  });

  it('has a word and a meaning for every derived status, and no "mark as paid"', () => {
    for (const status of [
      'draft',
      'issued',
      'part_paid',
      'paid',
      'credited',
      'overdue',
      'void',
    ] as const) {
      expect(statusWords(status).label.length, status).toBeGreaterThan(3);
      // A word for a person: capitalised, and not the wire value.
      expect(statusWords(status).label, status).toMatch(/^[A-Z]/);
      expect(statusWords(status).label, status).not.toContain('_');
      expect(statusWords(status).meaning.length, status).toBeGreaterThan(20);
    }
    expect(statusWords('paid').meaning).toContain('Nothing else can mark an invoice paid');
  });

  it('says credited is a write-off and not money received', () => {
    const words = statusWords('credited');
    expect(words.label).toBe('Credited');
    expect(words.meaning).toContain('credit note');
    expect(words.meaning).toContain('written off');
    expect(words.meaning).toContain('does not say paid');
  });
});

describe('the figures of an invoice', () => {
  it('shows all four, and they add up: total, less paid, less credited, is outstanding', () => {
    const figures = settlement(partCredited());
    expect(figures.map((figure) => [figure.label, figure.money.text])).toEqual([
      ['Total', '600.00'],
      ['Paid', '200.00'],
      ['Credited', '100.00'],
      ['Outstanding', '300.00'],
    ]);
    const [total, paid, credited, outstanding] = figures.map((figure) => figure.money.minor);
    expect((total ?? 0) - (paid ?? 0) - (credited ?? 0)).toBe(outstanding);
  });

  it('keeps what was paid and what was credited as two figures, never one', () => {
    const figures = settlement(partCredited());
    // 200.00 + 100.00 = 300.00 "settled" is not a figure anybody received.
    expect(figures.filter((figure) => figure.label === 'Paid')).toHaveLength(1);
    expect(figures.filter((figure) => figure.label === 'Credited')).toHaveLength(1);
    expect(figures.find((figure) => figure.label === 'Credited')?.note).toContain('not received');
    expect(figures.find((figure) => figure.label === 'Paid')?.note).toBe('1 payment received');
  });

  it('shows the credited figure even when it is nothing, so the sum is always whole', () => {
    const figures = settlement(invoice({ state: 'issued', status: 'issued' }));
    expect(figures.map((figure) => figure.label)).toEqual([
      'Total',
      'Paid',
      'Credited',
      'Outstanding',
    ]);
    expect(figures[2]?.note).toBe('no credit note');
    expect(figures[1]?.note).toBe('no payment recorded');
  });

  it('counts only what still stands: a void payment or note is in no figure’s caption', () => {
    const figures = settlement(
      invoice({
        state: 'issued',
        status: 'issued',
        payments: [payment({ state: 'void' })],
        credits: [credit({ state: 'void' })],
      }),
    );
    expect(figures[1]?.note).toBe('no payment recorded');
    expect(figures[2]?.note).toBe('no credit note');
  });

  it('says a settlement beyond the total is in the customer’s favour, not a negative debt', () => {
    // Paid in full, then a 5.00 goodwill credit.
    const figures = settlement(
      invoice({
        state: 'issued',
        status: 'paid',
        paid: usd(8000, '80.00'),
        credited: usd(500, '5.00'),
        outstanding: usd(-500, '-5.00'),
        payments: [payment()],
        credits: [credit({ amount: usd(500, '5.00') })],
      }),
    );
    const last = figures.at(-1);
    expect(last?.label).toBe('In their favour');
    expect(last?.money).toEqual(usd(500, '5.00'));
    expect(last?.note).toContain('not taken off anything else they owe');
    expect(figures.some((figure) => figure.label === 'Outstanding')).toBe(false);
  });
});

describe('what is said beside a due date', () => {
  // The business is in Baghdad; the invoice was written in that calendar.
  const BAGHDAD = 180;
  const dueDay = dayStart('2026-03-15', BAGHDAD) ?? 0;
  const at = (text: string, hour: number) => (dayStart(text, BAGHDAD) ?? 0) + hour * 3_600_000;
  const issued = (over: Partial<InvoiceView> = {}) =>
    invoice({
      state: 'issued',
      status: 'issued',
      utc_offset_minutes: BAGHDAD,
      due_at: dueDay,
      ...over,
    });

  it('takes whether an invoice is late from the engine, and how late', () => {
    const late = issued({ status: 'overdue', days_overdue: 38 });
    expect(dueText(late, { now: at('2026-04-22', 9), offset: BAGHDAD })).toBe('38 days overdue');
    expect(dueText(issued({ status: 'overdue', days_overdue: 1 }), { now: 0, offset: BAGHDAD })).toBe(
      '1 day overdue',
    );
    // The browser's clock is not consulted: a "now" years before the due
    // date changes nothing about an invoice the engine says is overdue.
    expect(dueText(late, { now: at('2020-01-01', 0), offset: BAGHDAD })).toBe('38 days overdue');
    expect(dueText(late, { now: at('2020-01-01', 0), offset: null })).toBe('38 days overdue');
  });

  it('never calls an invoice overdue that the engine does not, whatever the clock here says', () => {
    // The due day is long past by this browser's clock, and the engine says
    // `issued`: the worst this can say is that it is due today.
    const text = dueText(issued(), { now: at('2026-09-01', 12), offset: BAGHDAD });
    expect(text).not.toMatch(/overdue/i);
    expect(text).toBe('Due today');
  });

  it('says a settled invoice long past its due date is settled, not that it is not due yet', () => {
    // Due 15 March, paid on the 10th, looked at in November.
    const now = { now: at('2026-11-22', 9), offset: BAGHDAD };
    const paid = issued({ status: 'paid', paid: usd(8000, '80.00'), outstanding: usd(0, '0.00') });
    expect(dueText(paid, now)).toBe('Settled');
    const credited = issued({
      status: 'credited',
      credited: usd(8000, '80.00'),
      outstanding: usd(0, '0.00'),
    });
    expect(dueText(credited, now)).toBe('Settled');
    // Settled beyond its total is settled too.
    expect(dueText(issued({ status: 'paid', outstanding: usd(-500, '-5.00') }), now)).toBe('Settled');
    // An invoice that came to nothing reads `issued` for ever and is never late.
    const nothing = issued({ total: usd(0, '0.00'), outstanding: usd(0, '0.00') });
    expect(dueText(nothing, now)).toBe('Nothing owed');
  });

  it('says due today throughout the due day, and not due yet before it', () => {
    const open = issued({ status: 'part_paid', outstanding: usd(3000, '30.00') });
    expect(dueText(open, { now: at('2026-03-14', 23), offset: BAGHDAD })).toBe('Not due yet');
    // The first and the last hour of the 15th in Baghdad: neither is late.
    expect(dueText(open, { now: at('2026-03-15', 0), offset: BAGHDAD })).toBe('Due today');
    expect(dueText(open, { now: at('2026-03-15', 23), offset: BAGHDAD })).toBe('Due today');
    // Without the calendar there is no "today" to compare with.
    expect(dueText(open, { now: at('2026-03-15', 9), offset: null })).toBe('Not overdue');
  });

  it('reads the due day in the calendar the invoice was written in', () => {
    // Written on UTC and due on the 15th there; the business has since
    // moved to Baghdad, where it is already the 15th three hours earlier.
    const written = issued({ utc_offset_minutes: 0, due_at: Date.UTC(2026, 2, 15) });
    const lateOn14thUtc = Date.UTC(2026, 2, 14, 22); // 01:00 on the 15th in Baghdad
    expect(dueText(written, { now: lateOn14thUtc, offset: BAGHDAD })).toBe('Due today');
    expect(dueText(written, { now: lateOn14thUtc, offset: 0 })).toBe('Not due yet');
  });

  it('has its own words for a draft, a void invoice and one with no due date', () => {
    const now = { now: 0, offset: BAGHDAD };
    expect(dueText(invoice(), now)).toBe('Not issued');
    expect(dueText(invoice({ state: 'void', status: 'void' }), now)).toBe('Void');
    expect(dueText(issued({ due_at: null }), now)).toBe('No due date');
  });

  it('shows an invoice’s period in the calendar it was written in, first day to last', () => {
    const october = monthWindow('2026-10', BAGHDAD);
    expect(
      invoicePeriod(
        invoice({
          period_start: october?.start ?? 0,
          period_end: october?.end ?? 0,
          utc_offset_minutes: BAGHDAD,
        }),
      ),
    ).toBe('2026-10-01 → 2026-10-31');
  });
});

describe('where a page of the list is', () => {
  it('is the range the engine sent, counted from one', () => {
    expect(pageText(0, 50, 50)).toBe('1–50');
    expect(pageText(50, 2, 2)).toBe('51–52');
    expect(pageText(0, 0, 0)).toBe('nothing on this page');
    expect(pageText(100, 0, 0)).toBe('nothing on this page');
  });

  it('says how many invoices on the page are not listed, and why, instead of counting them as shown', () => {
    // A month filter after the calendar was changed: the engine sent three,
    // one of them written for the month before under the old offset.
    expect(pageText(0, 3, 2)).toBe(
      '1–3, 1 not shown: its period is another month in the calendar it was written in',
    );
    expect(pageText(50, 50, 0)).toBe(
      '51–100, 50 not shown: their periods are another month in the calendar they were written in',
    );
  });
});

describe('a line that covers part of a period', () => {
  it('carries the arithmetic, so the figure can be rechecked', () => {
    const prorated = {
      covered_ms: 1_382_400_000,
      period_ms: 2_592_000_000,
      covered_days: 16,
      period_days: 30,
      exact: false,
      basis: '1382400000 of the period’s 2592000000 milliseconds',
    };
    expect(prorationText(prorated)).toBe('16 of the period’s 30 days, rounded to the minor unit');
    expect(prorationText({ ...prorated, exact: true })).toBe('16 of the period’s 30 days');
    // The description already says the plan and the megabits, so the line
    // under it is the arithmetic: the sum, and what part of the period.
    expect(
      lineSubtitle(
        {
          id: 'l1',
          kind: 'subscription',
          description: 'Business 50 — 20 Mbit/s (part period)',
          subscription_id: 's1',
          tariff_name: 'Business 50',
          mbit: 20,
          unit: 'per_mbit',
          unit_price: { currency: 'USD', minor: 400, digits: 2, text: '4.00' },
          full_period_price: { currency: 'USD', minor: 8000, digits: 2, text: '80.00' },
          period_start: 1,
          period_end: 2,
          prorated,
          amount: { currency: 'USD', minor: 4267, digits: 2, text: '42.67' },
        },
        (money) => `${money.text} ${money.currency}`,
      ),
    ).toBe('20 Mbit/s × 4.00 USD per megabit · 16 of the period’s 30 days, rounded to the minor unit');

    // A flat line with no proration has nothing to add to its description.
    expect(
      lineSubtitle(
        {
          id: 'l2',
          kind: 'subscription',
          description: 'Business 50 — 50 Mbit/s',
          subscription_id: 's2',
          tariff_name: 'Business 50',
          mbit: 50,
          unit: 'flat',
          unit_price: { currency: 'USD', minor: 25000, digits: 2, text: '250.00' },
          full_period_price: { currency: 'USD', minor: 25000, digits: 2, text: '250.00' },
          period_start: 1,
          period_end: 2,
          prorated: null,
          amount: { currency: 'USD', minor: 25000, digits: 2, text: '250.00' },
        },
        (money) => `${money.text} ${money.currency}`,
      ),
    ).toBe('');
  });
});

describe('a billing run', () => {
  it('says first of all that a preview wrote nothing', () => {
    const outcome = runOutcome(run());
    expect(outcome.preview).toBe(true);
    expect(outcome.headline).toBe('Preview — nothing has been written');
    expect(outcome.note).toContain('nothing was written');
    expect(outcome.invoices).toBe(2);
    expect(outcome.skipped).toBe(1);
    expect(outcome.skippedSubscriptions).toBe(1);
  });

  it('says what it did once it has done it', () => {
    expect(runOutcome(run({ dry_run: false, drafted: 2, issued: 0 })).headline).toBe(
      '2 drafts created',
    );
    expect(runOutcome(run({ dry_run: false, drafted: 2, issued: 2 })).headline).toBe(
      '2 invoices issued',
    );
    expect(runOutcome(run({ dry_run: false })).preview).toBe(false);
  });

  it('says which drafts it was asked to issue and could not, rather than counting them as issued', () => {
    const written = run({ dry_run: false, drafted: 2, issued: 1 });
    const second = written.invoices[1];
    if (second) second.not_issued = 'this invoice comes to -5.00, which is less than nothing';
    const outcome = runOutcome({
      ...written,
      warnings: ['1 invoice(s) were drafted and could not be issued'],
    });
    expect(outcome.headline).toBe('1 invoice issued, 1 draft left a draft');
    expect(outcome.leftAsDrafts).toBe(1);
    expect(outcome.warnings).toEqual(['1 invoice(s) were drafted and could not be issued']);
  });

  it('accounts for everybody it looked at, and says which days it billed', () => {
    const outcome = runOutcome(run());
    expect(outcome.considered).toBe(outcome.invoices + outcome.skipped);
    expect(runCountsText(outcome)).toBe(
      '2 customers would be billed, 1 left out, and 1 subscription not charged on an invoice that was still drafted — 3 looked at.',
    );
    expect(runCountsText(runOutcome(run({ dry_run: false })))).toContain('2 customers billed');
    // The period is the engine's, read in the calendar the engine says it used.
    expect(outcome.period).toBe('2026-10-01 → 2026-10-31');
    const baghdad = monthWindow('2026-10', 180);
    expect(
      runOutcome(
        run({
          period_start: baghdad?.start ?? 0,
          period_end: baghdad?.end ?? 0,
          utc_offset_minutes: 180,
        }),
      ).period,
    ).toBe('2026-10-01 → 2026-10-31');
  });

  it('keeps the two currencies of the run apart', () => {
    const outcome = runOutcome(run());
    expect(outcome.totals.map((money) => money.currency)).toEqual(['IQD', 'USD']);
    // There is no scalar on the outcome to put a mixed figure in.
    expect(Object.values(outcome).some((value) => value === 175000)).toBe(false);
  });

  it('has nothing to confirm when it would bill nobody', () => {
    expect(runHasWork(null)).toBe(false);
    expect(runHasWork(run({ invoices: [] }))).toBe(false);
    expect(runHasWork(run())).toBe(true);
  });
});
