import { describe, expect, it } from 'vitest';
import type { Arrears, CurrencyArrears, Money } from '@/api/types/isp';
import {
  anythingOverdue,
  BOOKKEEPING_NOTE,
  bucketMeaning,
  bucketTone,
  byCurrency,
  inFavour,
  oldestText,
  oldestTone,
  owedById,
  owedCell,
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
  overpaid: [money('IQD', 5_000), money('USD', 2_000)],
  customers_overpaid: [
    {
      customer_id: 'c1',
      customer_name: 'Zagros Net',
      state: 'active',
      overpaid: money('IQD', 5_000),
      invoices: 1,
    },
    {
      customer_id: 'c3',
      customer_name: 'Goodwill Gulnar',
      state: 'active',
      overpaid: money('USD', 2_000),
      invoices: 2,
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

  it('says nothing is overdue rather than nought days late, and does not claim nothing is due', () => {
    // `null` is the engine's "nothing of theirs is overdue" — which includes
    // an invoice that falls due today, so "nothing due yet" would be false.
    expect(oldestText(null)).toBe('Nothing overdue');
    expect(oldestTone(null)).toBe('info');
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
      'More than 90 days past the due day.',
    );
    expect(bucketMeaning({ from_days: 0, to_days: 30 })).toBe(
      'Between 1 and 30 days past the due day.',
    );
  });

  it('asks whether anything is overdue per currency, never across them', () => {
    expect(anythingOverdue(byCurrency(ARREARS))).toBe(true);
    expect(anythingOverdue(byCurrency({ ...ARREARS, by_currency: [report('IQD', 149_000, 0)] }))).toBe(
      false,
    );
  });

  it('keeps what is in customers’ favour beside what is owed, per currency, and nets nothing', () => {
    const favour = inFavour(ARREARS);
    expect(favour.map((entry) => [entry.currency, entry.total.minor])).toEqual([
      ['IQD', 5_000],
      ['USD', 2_000],
    ]);
    expect(favour[0]?.customers.map((row) => row.customer_name)).toEqual(['Zagros Net']);
    expect(favour[1]?.customers.map((row) => row.customer_name)).toEqual(['Goodwill Gulnar']);
    // Zagros owes 149 000 and is 5 000 in credit: both stand. 144 000 is a
    // figure nobody allocated, and it is nowhere.
    const everything = JSON.stringify([byCurrency(ARREARS), favour]);
    expect(everything).not.toContain('144000');
    expect(byCurrency(ARREARS)[0]?.report.total.minor).toBe(149_000);
    // Nothing overpaid is no report, not a report of zero.
    expect(inFavour({ overpaid: [], customers_overpaid: [] })).toEqual([]);
  });

  it('says what Nocturne does about arrears, which is to write them down', () => {
    expect(BOOKKEEPING_NOTE).toContain('does not suspend');
    expect(BOOKKEEPING_NOTE).toContain('on the router');
  });
});

describe('what a customer list says each customer owes', () => {
  const arrears = {
    customers: [
      { customer_id: 'zagros', outstanding: money('IQD', 149_000) },
      { customer_id: 'tigris', outstanding: money('USD', 22_500) },
      { customer_id: 'tigris', outstanding: money('IQD', 3_000) },
    ],
  } as unknown as Pick<Arrears, 'customers'>;
  const owed = owedById(arrears);
  const read = { isPending: false, isError: false };

  it('gives each customer what they owe in each currency, apart', () => {
    expect(owedCell(read, owed, 'zagros')).toEqual({
      kind: 'owes',
      amounts: [money('IQD', 149_000)],
    });
    expect(owedCell(read, owed, 'tigris')).toEqual({
      kind: 'owes',
      amounts: [money('USD', 22_500), money('IQD', 3_000)],
    });
    expect(owedCell(read, owed, 'dohuk')).toEqual({ kind: 'nothing' });
  });

  it('does not say nothing is owed when the arrears could not be read', () => {
    // A failed read has no customers in it, which is not the same as a
    // ledger in which nobody owes anything.
    const failed = { isPending: false, isError: true };
    const empty = owedById(undefined);
    for (const customer of ['zagros', 'tigris', 'dohuk']) {
      expect(owedCell(failed, empty, customer)).toEqual({ kind: 'not_read' });
      expect(owedCell(failed, owed, customer)).toEqual({ kind: 'not_read' });
    }
    expect(owedCell({ isPending: true, isError: false }, empty, 'zagros')).toEqual({
      kind: 'pending',
    });
  });
});
