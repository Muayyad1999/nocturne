import { describe, expect, it } from 'vitest';
import type { InvoiceView, Payment, RunResult } from '@/api/types/isp';
import {
  dayStart,
  invoiceActions,
  lineSubtitle,
  monthWindow,
  overdueText,
  prorationText,
  recentMonths,
  runHasWork,
  runOutcome,
  statusWords,
  voidBlockedReason,
  windowOf,
} from './billing-rows';

function payment(over: Partial<Payment> = {}): Payment {
  return {
    id: 'p1',
    customer_id: 'c1',
    invoice_id: 'i1',
    amount: { currency: 'USD', minor: 2500, digits: 2, text: '25.00' },
    conversion: null,
    settled: { currency: 'USD', minor: 2500, digits: 2, text: '25.00' },
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

function invoice(over: Partial<InvoiceView> = {}): InvoiceView {
  const total = { currency: 'USD' as const, minor: 8000, digits: 2, text: '80.00' };
  return {
    id: 'i1',
    number: null,
    customer_id: 'c1',
    customer_name: 'Tigris Trading',
    currency: 'USD',
    state: 'draft',
    period_start: 1,
    period_end: 2,
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
    paid: { currency: 'USD', minor: 0, digits: 2, text: '0.00' },
    outstanding: total,
    days_overdue: null,
    payments: [],
    ...over,
  };
}

function run(over: Partial<RunResult> = {}): RunResult {
  return {
    period_start: Date.UTC(2026, 9, 1),
    period_end: Date.UTC(2026, 10, 1),
    dry_run: true,
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
      },
    ],
    skipped: [{ customer_id: 'c3', reason: 'nothing to bill for that period' }],
    totals: [
      { currency: 'IQD', minor: 150000, digits: 0, text: '150000' },
      { currency: 'USD', minor: 25000, digits: 2, text: '250.00' },
    ],
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
    });
    expect(invoiceActions(invoice({ state: 'issued', status: 'issued' }))).toEqual({
      edit: false,
      issue: false,
      void: true,
      reissue: true,
      pay: true,
    });
  });

  it('does not offer to void an invoice with money recorded against it', () => {
    const paid = invoice({ state: 'issued', status: 'part_paid', payments: [payment()] });
    expect(invoiceActions(paid).void).toBe(false);
    expect(invoiceActions(paid).reissue).toBe(false);
    expect(voidBlockedReason(paid)).toContain('hide money that was actually received');
    expect(voidBlockedReason(invoice())).toBeNull();
  });

  it('counts a voided payment as no payment, so the invoice can be voided again', () => {
    const withVoid = invoice({
      state: 'issued',
      status: 'issued',
      payments: [payment({ state: 'void', void_reason: 'entered twice' })],
    });
    expect(invoiceActions(withVoid).void).toBe(true);
    expect(voidBlockedReason(withVoid)).toBeNull();
  });

  it('offers nothing at all on a void invoice, which is kept and not deleted', () => {
    expect(invoiceActions(invoice({ state: 'void', status: 'void' }))).toEqual({
      edit: false,
      issue: false,
      void: false,
      reissue: false,
      pay: false,
    });
  });

  it('has a word and a meaning for every derived status, and no "mark as paid"', () => {
    for (const status of ['draft', 'issued', 'part_paid', 'paid', 'overdue', 'void'] as const) {
      expect(statusWords(status).label.length, status).toBeGreaterThan(3);
      expect(statusWords(status).meaning.length, status).toBeGreaterThan(20);
    }
    expect(statusWords('paid').meaning).toContain('Nothing else can mark an invoice paid');
  });
});

describe('how late an invoice is', () => {
  it('says it is not due yet rather than nought days overdue', () => {
    expect(overdueText(invoice({ state: 'issued', due_at: 5_000, days_overdue: null }))).toBe(
      'Not due yet',
    );
    expect(overdueText(invoice({ state: 'issued', due_at: 5_000, days_overdue: 0 }))).toBe(
      'Due today',
    );
    expect(overdueText(invoice({ state: 'issued', due_at: 5_000, days_overdue: 38 }))).toBe(
      '38 days overdue',
    );
    expect(overdueText(invoice())).toBe('Not issued');
    expect(overdueText(invoice({ state: 'void' }))).toBe('Void');
    expect(overdueText(invoice({ state: 'issued', due_at: null }))).toBe('No due date');
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

describe('choosing the period to bill', () => {
  it('is a whole month, in UTC, as `[start, end)`', () => {
    expect(monthWindow(2026, 9)).toEqual({ start: Date.UTC(2026, 9, 1), end: Date.UTC(2026, 10, 1) });
    // December rolls into the next year rather than into month 12.
    expect(monthWindow(2026, 11)).toEqual({ start: Date.UTC(2026, 11, 1), end: Date.UTC(2027, 0, 1) });
    expect(windowOf('2026-10')).toEqual(monthWindow(2026, 9));
    expect(windowOf('2026-13')).toBeNull();
    expect(windowOf('nonsense')).toBeNull();
  });

  it('reads a date field as that day in the reader’s own time zone', () => {
    const at = dayStart('2026-10-02');
    expect(at).not.toBeNull();
    const date = new Date(at ?? 0);
    expect([date.getFullYear(), date.getMonth(), date.getDate(), date.getHours()]).toEqual([
      2026, 9, 2, 0,
    ]);
    // An empty or half-typed field is no date, not today.
    expect(dayStart('')).toBeNull();
    expect(dayStart('2026-10')).toBeNull();
    expect(dayStart('tomorrow')).toBeNull();
  });

  it('offers recent months newest first, across a year boundary', () => {
    const months = recentMonths(Date.UTC(2026, 0, 15), 3);
    expect(months.map((month) => month.value)).toEqual(['2026-01', '2025-12', '2025-11']);
    expect(months[0]?.label).toBe('January 2026');
  });
});
