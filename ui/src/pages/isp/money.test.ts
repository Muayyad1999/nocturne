import { describe, expect, it } from 'vitest';
import type { Converted, CurrencyRow, Money } from '@/api/types/isp';
import {
  amountText,
  conversionCaption,
  currencyHint,
  digitsOf,
  divide,
  formatMoney,
  formatTotals,
  moneyLike,
  parseAmount,
  plainAmount,
  rateText,
  totalsLine,
} from './money';

/** Amounts exactly as the backend serialises them (verified against a running engine). */
const USD_250: Money = { currency: 'USD', minor: 25000, digits: 2, text: '250.00' };
const IQD_150K: Money = { currency: 'IQD', minor: 150000, digits: 0, text: '150000' };
const USD_42_67: Money = { currency: 'USD', minor: 4267, digits: 2, text: '42.67' };
const USD_MINUS_50: Money = { currency: 'USD', minor: -5000, digits: 2, text: '-50.00' };
const JOD_ONE: Money = { currency: 'JOD', minor: 1000, digits: 3, text: '1.000' };

const CURRENCIES: CurrencyRow[] = [
  { code: 'USD', digits: 2, scale: 100, name: 'US dollar', thousand_minor_units: '10.00' },
  { code: 'IQD', digits: 0, scale: 1, name: 'Iraqi dinar', thousand_minor_units: '1000' },
  { code: 'JOD', digits: 3, scale: 1000, name: 'Jordanian dinar', thousand_minor_units: '1.000' },
];

describe('an amount is minor units and a currency', () => {
  it('writes the same figure the backend wrote, from `minor` and `digits` alone', () => {
    // If this ever disagrees the screen is inventing a figure, so every
    // sample here is real output from `nocturne-server --features isp`.
    for (const money of [USD_250, IQD_150K, USD_42_67, USD_MINUS_50, JOD_ONE]) {
      expect(plainAmount(money.minor, money.digits)).toBe(money.text);
    }
  });

  it('puts 1000 minor units where the currency puts it, not where a hundredth would be', () => {
    // The whole reason this module exists: `1000` is ten dollars, a thousand
    // dinars, or one Jordanian dinar.
    expect(plainAmount(1000, 2)).toBe('10.00');
    expect(plainAmount(1000, 0)).toBe('1000');
    expect(plainAmount(1000, 3)).toBe('1.000');
  });

  it('does not lose the units below the point of a currency that has them', () => {
    expect(plainAmount(7, 2)).toBe('0.07');
    expect(plainAmount(-7, 2)).toBe('-0.07');
    expect(plainAmount(0, 2)).toBe('0.00');
    expect(plainAmount(0, 0)).toBe('0');
  });

  it('groups the digits and uses a real minus sign', () => {
    expect(amountText(IQD_150K)).toBe('150 000');
    expect(amountText(USD_250)).toBe('250.00');
    expect(amountText({ currency: 'USD', minor: 123456789, digits: 2, text: '1234567.89' })).toBe(
      '1 234 567.89',
    );
    expect(amountText(USD_MINUS_50)).toBe('−50.00');
  });

  it('keeps the currency code beside the figure', () => {
    expect(formatMoney(IQD_150K)).toBe('150 000 IQD');
    expect(formatMoney(USD_250)).toBe('250.00 USD');
  });
});

describe('a total across currencies', () => {
  it('is several figures, each with its own code', () => {
    expect(formatTotals([IQD_150K, USD_250])).toEqual(['150 000 IQD', '250.00 USD']);
    expect(totalsLine([IQD_150K, USD_250])).toBe('150 000 IQD · 250.00 USD');
  });

  it('never adds the figures into a third one', () => {
    const line = totalsLine([IQD_150K, USD_250]);
    // 150000 + 25000 = 175000, and 150000 + 250 = 150250: neither may appear.
    expect(line).not.toContain('175');
    expect(line).not.toContain('150250');
    expect(line.match(/IQD|USD/g)).toEqual(['IQD', 'USD']);
  });

  it('says nothing came in rather than showing a zero when the list is empty', () => {
    // An empty `Totals` means no money at all in the window, and a `0` with
    // no currency beside it would be a figure in no currency.
    expect(totalsLine([])).toBe('nothing');
    expect(totalsLine([], 'No invoice was issued in this window')).toBe(
      'No invoice was issued in this window',
    );
    expect(formatTotals([])).toEqual([]);
  });
});

describe('entering an amount', () => {
  it('turns what was typed into whole minor units of that currency', () => {
    expect(parseAmount('250.00', 'USD', 2)).toEqual({
      ok: true,
      amount: { minor: 25000, input: { currency: 'USD', minor: 25000 }, text: '250.00' },
    });
    expect(parseAmount('25.5', 'USD', 2)).toMatchObject({ ok: true, amount: { minor: 2550 } });
    expect(parseAmount('150000', 'IQD', 0)).toMatchObject({ ok: true, amount: { minor: 150000 } });
    expect(parseAmount('1.000', 'JOD', 3)).toMatchObject({ ok: true, amount: { minor: 1000 } });
    expect(parseAmount('.07', 'USD', 2)).toMatchObject({ ok: true, amount: { minor: 7 } });
    expect(parseAmount(' -50 ', 'USD', 2)).toMatchObject({ ok: true, amount: { minor: -5000 } });
  });

  it('sends an integer, so no request can carry a float the backend would refuse', () => {
    const parsed = parseAmount('1000.00', 'USD', 2);
    expect(parsed.ok).toBe(true);
    if (!parsed.ok) return;
    expect(Number.isInteger(parsed.amount.minor)).toBe(true);
    // `1000.0` is what the backend refuses by name; the wire form must be `100000`.
    expect(JSON.stringify(parsed.amount.input)).toBe('{"currency":"USD","minor":100000}');
  });

  it('refuses more decimals than the currency has, rather than rounding them away', () => {
    expect(parseAmount('1.555', 'USD', 2)).toMatchObject({ ok: false });
    expect(parseAmount('1.5', 'IQD', 0)).toMatchObject({ ok: false });
    const dinar = parseAmount('1.5', 'IQD', 0);
    expect(dinar.ok ? '' : dinar.error).toContain('whole units');
  });

  it('refuses what is not digits, naming what is not allowed', () => {
    for (const bad of ['', '  ', 'abc', '1,000', '$10', '1.2.3', '10 00', '1-0']) {
      expect(parseAmount(bad, 'USD', 2).ok, bad).toBe(false);
    }
  });

  it('refuses an amount too large to hold exactly', () => {
    expect(parseAmount('99999999999999999', 'USD', 2).ok).toBe(false);
  });
});

describe('dividing an amount', () => {
  it('says whether the division was exact', () => {
    expect(divide(USD_MINUS_50, 100)).toEqual({
      amount: { currency: 'USD', minor: -50, digits: 2, text: '-0.50' },
      exact: true,
    });
    expect(divide(USD_250, 3)).toEqual({
      amount: { currency: 'USD', minor: 8333, digits: 2, text: '83.33' },
      exact: false,
    });
  });

  it('has no figure to give when there is nothing to divide by', () => {
    // Not a zero: a margin per megabit over no megabits is not 0.00.
    expect(divide(USD_250, 0)).toBeNull();
    expect(divide(USD_250, Number.NaN)).toBeNull();
  });

  it('keeps the currency and its minor digits', () => {
    expect(moneyLike(IQD_150K, 7)).toEqual({
      currency: 'IQD',
      minor: 7,
      digits: 0,
      text: '7',
    });
  });
});

describe('what a screen says about a currency and a rate', () => {
  it('describes a currency in its own terms, not in hundredths', () => {
    expect(digitsOf(CURRENCIES, 'IQD')).toBe(0);
    expect(digitsOf(CURRENCIES, 'XXX')).toBeUndefined();
    expect(currencyHint(CURRENCIES[1])).toBe('Iraqi dinar: whole units, no decimals.');
    expect(currencyHint(CURRENCIES[0])).toBe(
      'US dollar: 2 decimal places, so 1000 minor units is 10.00.',
    );
    expect(currencyHint(undefined)).toBe('');
  });

  it('writes a rate in the direction it was quoted in', () => {
    const rate = { from: 'USD', to: 'IQD', rate: 1320, at: 1 } as const;
    expect(rateText(rate, CURRENCIES)).toBe('1 USD = 1 320 IQD');
    // Without the minor digits of the target it is stated as minor units
    // rather than written as if it were whole dinars.
    expect(rateText(rate)).toBe('1 USD = 1320 minor units of IQD');
  });

  it('labels a converted figure as a conversion and names its rates', () => {
    const converted: Converted = {
      basis: 'conversion',
      amount: { currency: 'USD', minor: 19898, digits: 2, text: '198.98' },
      of: [IQD_150K, { currency: 'USD', minor: 8534, digits: 2, text: '85.34' }],
      rates: [{ from: 'USD', to: 'IQD', rate: 1320, at: 1 }],
      at: 2,
      exact: false,
      note: 'a conversion into USD at the rates given, not the ledger',
    };
    const caption = conversionCaption(converted, CURRENCIES);
    expect(caption).toContain('conversion');
    expect(caption).toContain('1 USD = 1 320 IQD');
    expect(caption).toContain('not the ledger');
  });
});
