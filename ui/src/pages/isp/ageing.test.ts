import { describe, expect, it } from 'vitest';
import type { Arrears, CurrencyArrears, Money } from '@/api/types/isp';
import {
  anythingOverdue,
  BOOKKEEPING_NOTE,
  bucketMeaning,
  bucketTone,
  byCurrency,
  oldestText,
  oldestTone,
} from './ageing';

function money(currency: 'USD' | 'IQD', minor: number): Money {
  const digits = currency === 'USD' ? 2 : 0;
  return {
    currency,
    minor,
    digits,
    text: digits === 0 ? String(minor) : (minor / 100).toFixed(2),
  };
}

function report(currency: 'USD' | 'IQD', total: number, overdue: number): CurrencyArrears {
  return {
    currency,
    total: money(currency, total),
    not_yet_due: money(currency, total - overdue),
    overdue: money(currency, overdue),
    buckets: [
      {
        label: 'not yet due',
        from_days: -1,
        to_days: -1,
        amount: money(currency, total - overdue),
        invoices: 1,
      },
      { label: '1–30 days', from_days: 0, to_days: 30, amount: money(currency, overdue), invoices: 1 },
      { label: '31–60 days', from_days: 31, to_days: 60, amount: money(currency, 0), invoices: 0 },
      { label: '61–90 days', from_days: 61, to_days: 90, amount: money(currency, 0), invoices: 0 },
      { label: 'over 90 days', from_days: 91, to_days: null, amount: money(currency, 0), invoices: 0 },
    ],
    customers: 1,
    invoices: 2,
  };
}

const ARREARS: Arrears = {
  as_of: 5_000,
  by_currency: [report('IQD', 149_000, 0), report('USD', 3_534, 3_534)],
  customers: [
    {
      customer_id: 'c1',
      customer_name: 'Zagros Net',
      state: 'active',
      outstanding: money('IQD', 149_000),
      oldest_days: null,
      invoices: 1,
    },
    {
      customer_id: 'c2',
      customer_name: 'Measured Mahmoud',
      state: 'suspended',
      outstanding: money('USD', 3_534),
      oldest_days: 38,
      invoices: 1,
    },
  ],
  note: 'one aged report per currency, and no total across them',
};

describe('ageing what is owed', () => {
  it('is one report per currency, each with only its own customers', () => {
    const reports = byCurrency(ARREARS);
    expect(reports.map((entry) => entry.currency)).toEqual(['IQD', 'USD']);
    expect(reports[0]?.customers.map((row) => row.customer_name)).toEqual(['Zagros Net']);
    expect(reports[1]?.customers.map((row) => row.customer_name)).toEqual(['Measured Mahmoud']);
  });

  it('has nowhere to put a figure across the currencies', () => {
    const reports = byCurrency(ARREARS);
    // 149000 + 3534 is not a number anybody is owed.
    expect(JSON.stringify(reports)).not.toContain('152534');
    expect(reports).toHaveLength(2);
  });

  it('says nothing is due yet rather than nought days late', () => {
    expect(oldestText(null)).toBe('Nothing due yet');
    expect(oldestTone(null)).toBe('info');
    expect(oldestText(0)).toBe('Due today');
    expect(oldestText(1)).toBe('1 day overdue');
    expect(oldestText(38)).toBe('38 days overdue');
    expect(oldestTone(38)).toBe('warn');
    expect(oldestTone(91)).toBe('crit');
  });

  it('does not colour the first band as late, because it is not', () => {
    expect(bucketTone({ from_days: -1, to_days: -1 })).toBe('info');
    expect(bucketMeaning({ from_days: -1, to_days: -1 })).toContain('not late');
    expect(bucketTone({ from_days: 0, to_days: 30 })).toBe('warn');
    expect(bucketTone({ from_days: 61, to_days: 90 })).toBe('crit');
    expect(bucketTone({ from_days: 91, to_days: null })).toBe('crit');
    expect(bucketMeaning({ from_days: 91, to_days: null })).toBe(
      'More than 90 days past the due date.',
    );
    expect(bucketMeaning({ from_days: 0, to_days: 30 })).toBe(
      'Between 1 and 30 days past the due date.',
    );
  });

  it('asks whether anything is overdue per currency, never across them', () => {
    expect(anythingOverdue(byCurrency(ARREARS))).toBe(true);
    expect(anythingOverdue(byCurrency({ ...ARREARS, by_currency: [report('IQD', 149_000, 0)] }))).toBe(
      false,
    );
  });

  it('says what Nocturne does about arrears, which is to write them down', () => {
    expect(BOOKKEEPING_NOTE).toContain('does not suspend');
    expect(BOOKKEEPING_NOTE).toContain('on the router');
  });
});
