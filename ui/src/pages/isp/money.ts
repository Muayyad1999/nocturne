/**
 * Money, and the four things a screen is allowed to do with it. No React in
 * here.
 *
 * The backend's `Money` is a whole number of minor units with the currency
 * beside it (`api/types/isp.ts`), and the minor unit is **not** always a
 * hundredth: `{minor: 1000}` is ten dollars in `USD` (2 digits), a thousand
 * dinars in `IQD` (0 digits) and one dinar in `JOD` (3). So:
 *
 * * **Nothing here divides by a hundred, and nothing calls `toFixed`.**
 *   {@link amountText} places the decimal point by moving along the digits of
 *   `minor` as a string, using the `digits` the backend sent with it. A
 *   float never enters the arithmetic, so there is nothing to round.
 * * **An amount somebody types never becomes a float either.**
 *   {@link parseAmount} validates the text the way the backend does and
 *   returns whole minor units, worked out by padding the fraction and
 *   concatenating the digits. `25.5` in USD is `2550`, and `1.5` in IQD is a
 *   refusal rather than `1`, because a dinar has no halves. The backend
 *   refuses a JSON float as firmly, and its message says so — this exists so
 *   the person is told before the round trip, not instead of it.
 * * **Several currencies are several figures.** {@link formatTotals} returns
 *   one string per currency and there is deliberately no function here that
 *   adds them: the API returns `Totals` with no scalar on purpose, and a
 *   single mixed figure would be invented. A converted figure comes from the
 *   backend as {@link Converted}, which carries its rates and its date, and
 *   is labelled a conversion wherever it is shown.
 * * **A division is reported as one.** {@link divide} does integer division
 *   in `BigInt` and says whether it was exact, so "margin per megabit" can
 *   be shown without quietly losing minor units.
 * * **A column of money is ordered within a currency, never across them.**
 *   `3000` minor units of `IQD` is about two dollars and `2500` of `USD` is
 *   twenty-five, so comparing the bare numbers ranks every dinar invoice
 *   above every dollar one. {@link moneySortKey} orders by currency first
 *   and by amount inside it; the specification's rule that nothing converts
 *   silently names a sort alongside a total and a chart.
 */

import type {
  Conversion,
  Converted,
  CurrencyRow,
  Money,
  MoneyInput,
  Totals,
} from '@/api/types/isp';

/** Thin space: groups digits in threes, as `lib/format` does. */
const GROUP = ' ';
/** No-break space between a figure and its currency code. */
const NBSP = ' ';
/** A real minus sign, not a hyphen. */
const MINUS = '−';

/** What is shown where an amount exists but is nothing at all. */
export const NOTHING = 'nothing';

function groupDigits(whole: string): string {
  return whole.replace(/\B(?=(\d{3})+(?!\d))/g, GROUP);
}

/**
 * The amount as the backend writes it: `25.00`, `1000`, `-1.07`. Derived from
 * `minor` and `digits` alone, by string arithmetic.
 */
export function plainAmount(minor: number, digits: number): string {
  const places = Number.isFinite(digits) ? Math.max(0, Math.trunc(digits)) : 0;
  const magnitude = Math.abs(Math.trunc(minor)).toString().padStart(places + 1, '0');
  const whole = magnitude.slice(0, magnitude.length - places);
  const fraction = places === 0 ? '' : magnitude.slice(magnitude.length - places);
  const body = fraction === '' ? whole : `${whole}.${fraction}`;
  return minor < 0 ? `-${body}` : body;
}

/** The amount with its digits grouped and a real minus sign: `1 250.00`. */
export function amountText(money: Money): string {
  const plain = plainAmount(money.minor, money.digits);
  const negative = plain.startsWith('-');
  const [whole = '', fraction] = (negative ? plain.slice(1) : plain).split('.');
  const body = fraction === undefined ? groupDigits(whole) : `${groupDigits(whole)}.${fraction}`;
  return negative ? MINUS + body : body;
}

/** An amount and its currency: `1 250.00 USD`. The code always travels with it. */
export function formatMoney(money: Money): string {
  return `${amountText(money)}${NBSP}${money.currency}`;
}

/** Is this amount exactly nothing? Not the same question as "is there a figure". */
export function isZero(money: Money): boolean {
  return money.minor === 0;
}

/**
 * One string per currency, in the order the backend sent them. An empty
 * `Totals` is an empty list, not a zero: the caller decides what to write,
 * because "no invoices in this window" and "a total of zero" are different
 * facts.
 */
export function formatTotals(totals: Totals): string[] {
  return totals.map(formatMoney);
}

/**
 * The per-currency figures on one line, for a cell that has no room for a
 * column each. Still several figures: they are separated, never summed.
 */
export function totalsLine(totals: Totals, empty = NOTHING): string {
  return totals.length === 0 ? empty : formatTotals(totals).join(' · ');
}

/** The currencies in a `Totals`, for deciding whether a figure is mixed. */
export function totalsCurrencies(totals: Totals): string[] {
  return totals.map((money) => money.currency);
}

/** An amount derived from another: the currency and its minor digits are kept. */
export function moneyLike(like: Money, minor: number): Money {
  return {
    currency: like.currency,
    minor,
    digits: like.digits,
    text: plainAmount(minor, like.digits),
  };
}

export interface Divided {
  amount: Money;
  /** False when the division lost minor units: the figure is rounded down. */
  exact: boolean;
}

/**
 * `money ÷ by`, in whole minor units, truncated towards zero — and whether
 * anything was lost. `BigInt`, so a large ledger cannot drift. `null` when
 * there is nothing to divide by, which is not a division by zero to be shown
 * as `0`.
 */
export function divide(money: Money, by: number): Divided | null {
  if (!Number.isFinite(by) || Math.trunc(by) === 0) return null;
  const divisor = BigInt(Math.trunc(by));
  const quotient = BigInt(money.minor) / divisor;
  return {
    amount: moneyLike(money, Number(quotient)),
    exact: quotient * divisor === BigInt(money.minor),
  };
}

// ------------------------------------------------------------ entering money

export interface Amount {
  /** Whole minor units: an integer, and what goes on the wire. */
  minor: number;
  /** The parameter to send. Never a JSON number with a point in it. */
  input: MoneyInput;
  /** The amount written back out, for a confirmation line. */
  text: string;
}

export type AmountResult = { ok: true; amount: Amount } | { ok: false; error: string };

/**
 * What somebody typed, as whole minor units of `currency` — or why it is not
 * an amount.
 *
 * The rules are the backend's (`money::Money::parse`): digits, at most one
 * decimal point, an optional leading minus, no thousands separator and no
 * currency symbol, and never more decimals than the currency has. They are
 * repeated here so the field can say so while it is being typed in; the
 * backend still checks, and its refusal is shown if the two ever disagree.
 */
export function parseAmount(raw: string, currency: string, digits: number): AmountResult {
  const places = Number.isFinite(digits) ? Math.max(0, Math.trunc(digits)) : 0;
  const text = raw.trim();
  if (text === '') return { ok: false, error: 'Enter an amount.' };

  const negative = text.startsWith('-');
  const body = negative ? text.slice(1) : text;
  if (!/^\d*\.?\d*$/.test(body)) {
    return {
      ok: false,
      error:
        'Only digits, one decimal point and a leading minus sign: no spaces, no thousands ' +
        'separator and no currency symbol.',
    };
  }
  const [whole = '', fraction = ''] = body.split('.');
  if (whole === '' && fraction === '') return { ok: false, error: 'Enter an amount.' };
  if (fraction.length > places) {
    return {
      ok: false,
      error:
        places === 0
          ? `${currency} has no minor unit, so it is written in whole units: 1500, not 1.5.`
          : `${currency} has ${places} decimal place${places === 1 ? '' : 's'}, so ${
              fraction.length
            } of them cannot be represented without inventing a rounding.`,
    };
  }

  const padded = (whole === '' ? '0' : whole) + fraction.padEnd(places, '0');
  // Concatenated digits, so nothing is ever divided or multiplied by a scale.
  const magnitude = Number(padded);
  if (!Number.isSafeInteger(magnitude)) {
    return { ok: false, error: 'That amount is too large to record.' };
  }
  const minor = negative ? -magnitude : magnitude;
  return {
    ok: true,
    amount: { minor, input: { currency, minor }, text: plainAmount(minor, places) },
  };
}

/** How many minor digits a currency has, from `isp.currencies`. */
export function digitsOf(currencies: readonly CurrencyRow[], code: string): number | undefined {
  return currencies.find((row) => row.code === code)?.digits;
}

/**
 * What to tell somebody about the currency they are typing in, in its own
 * terms rather than in hundredths: the backend sends what 1,000 minor units
 * of it actually is.
 */
export function currencyHint(row: CurrencyRow | undefined): string {
  if (!row) return '';
  // For a currency with no minor unit the two figures are the same, and
  // "1000 minor units is 1000" reads as a riddle rather than a hint.
  return row.digits === 0
    ? `${row.name}: whole units, no decimals.`
    : `${row.name}: ${row.digits} decimal place${row.digits === 1 ? '' : 's'}, so 1000 minor units is ${
        row.thousand_minor_units
      }.`;
}

/**
 * A rate as a person quotes it. `rate` is minor units of `to` for one major
 * unit of `from`, so it is only readable as an amount once the minor digits
 * of `to` are known; without them it is stated as what it is, in minor units,
 * rather than written as if it were whole dinars.
 */
export function rateText(
  rate: Pick<Conversion, 'from' | 'to' | 'rate'>,
  currencies: readonly CurrencyRow[] = [],
): string {
  const digits = digitsOf(currencies, rate.to);
  return digits === undefined
    ? `1 ${rate.from} = ${plainAmount(rate.rate, 0)} minor units of ${rate.to}`
    : `1 ${rate.from} = ${amountText({
        currency: rate.to,
        minor: rate.rate,
        digits,
        text: plainAmount(rate.rate, digits),
      })}${NBSP}${rate.to}`;
}

/**
 * A rate being typed, read back the way {@link rateText} writes a stored
 * one — or `null` while what is in the field is not a rate.
 *
 * The field takes minor units of the second currency, because that is what
 * fits in an integer; echoing the bare number beside the currency code
 * ("1 EUR = 108 USD" for a rate of 1.08) states the rate a hundredfold. So
 * the echo puts the point where the second currency puts it, and says
 * "minor units of" only when the table of currencies has not answered and
 * the point cannot be placed.
 */
export function quotedRate(
  from: string,
  to: string,
  typed: string,
  currencies: readonly CurrencyRow[] = [],
): string | null {
  const text = typed.trim();
  if (!/^[1-9]\d*$/.test(text)) return null;
  const rate = Number(text);
  if (!Number.isSafeInteger(rate)) return null;
  return rateText({ from: from as Conversion['from'], to: to as Conversion['to'], rate }, currencies);
}

/** Wide enough for any amount a JavaScript number can hold exactly. */
const SORT_WIDTH = 17;
const SORT_OFFSET = 10n ** 16n;

/**
 * What a money column sorts by: the currency, then the amount within it.
 *
 * A string, so that the table's own rule for a missing value still applies
 * (a link with no cost recorded goes last in both directions). The amount is
 * shifted to be positive and padded to one width, so that the order of the
 * strings is the order of the amounts, negatives included; amounts in
 * different currencies are never compared with each other at all, because
 * the code in front of them already differs.
 */
export function moneySortKey(money: Money | null | undefined): string | null {
  if (money == null) return null;
  const shifted = BigInt(Math.trunc(money.minor)) + SORT_OFFSET;
  return `${money.currency} ${shifted.toString().padStart(SORT_WIDTH, '0')}`;
}

/**
 * The sentence that goes beside a converted figure: that it is a conversion,
 * at which rates, and that the ledger is the per-currency figures beside it.
 * A reader must never be able to take this for the books.
 */
export function conversionCaption(
  converted: Converted,
  currencies: readonly CurrencyRow[] = [],
): string {
  const rates = converted.rates.map((rate) => rateText(rate, currencies)).join(', ');
  return rates === ''
    ? 'A conversion, not the ledger.'
    : `A conversion at rates you supplied (${rates}), not the ledger.`;
}
