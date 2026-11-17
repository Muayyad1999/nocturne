/**
 * Typing an amount.
 *
 * The field is text, never a number input: a browser's number input will
 * happily hand JavaScript `1000.0`, and the backend refuses a JSON float
 * firmly enough that it says so in two sentences. What leaves this screen is
 * always whole minor units, worked out by {@link parseAmount} from the digits
 * that were typed.
 *
 * It also says what the currency's minor unit *is*, because the whole trap is
 * that it is not always a hundredth: with `IQD` selected the hint says the
 * dinar is written in whole units, and `1.5` is refused here rather than
 * rounded on the way in. Below the field, what was typed is echoed as the
 * number of minor units that will be sent, so an amount in a currency the
 * person is not used to can be checked before it is a ledger entry.
 */

import { useId } from 'react';
import type { CurrencyRow } from '@/api/types/isp';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { formatCount } from '@/lib/format';
import { currencyHint, parseAmount } from './money';
import styles from './isp.module.css';

export interface AmountFieldProps {
  label: string;
  value: string;
  onChange: (value: string) => void;
  /** The currency the amount is in. */
  currency: string;
  /** Let the currency be changed here (a payment in another currency). */
  onCurrencyChange?: (currency: string) => void;
  /** `isp.currencies`: the authority on minor digits. */
  currencies: readonly CurrencyRow[];
  hint?: string;
  /** An error from the backend, shown in place of this field's own. */
  error?: string;
  required?: boolean;
  autoFocus?: boolean;
}

export function AmountField({
  label,
  value,
  onChange,
  currency,
  onCurrencyChange,
  currencies,
  hint,
  error,
  required = false,
  autoFocus = false,
}: AmountFieldProps) {
  // The currency select carries its own id: inside a Field every control
  // that does not have one takes the Field's, and two controls with one id
  // are two controls the label cannot point at.
  const currencyId = useId();
  const row = currencies.find((entry) => entry.code === currency);
  const digits = row?.digits;
  // Until `isp.currencies` has answered, the minor digits are not known, so
  // nothing is validated here and the backend decides. Guessing two would be
  // the exact mistake this feature is built against.
  const parsed = digits === undefined || value.trim() === '' ? null : parseAmount(value, currency, digits);
  const own = parsed && !parsed.ok ? parsed.error : undefined;
  const minor = parsed?.ok === true ? parsed.amount.minor : null;

  return (
    <Field
      label={label}
      required={required}
      error={error ?? own}
      hint={
        <>
          {hint ? `${hint} ` : ''}
          {currencyHint(row)}
          {minor !== null && (
            <>
              {' '}
              Sent as {formatCount(minor)} minor unit{minor === 1 || minor === -1 ? '' : 's'} of{' '}
              {currency}.
            </>
          )}
        </>
      }
    >
      <div className={onCurrencyChange ? styles.amountRow : undefined}>
        <Input
          mono
          inputMode="decimal"
          autoComplete="off"
          spellCheck={false}
          autoFocus={autoFocus}
          invalid={Boolean(error ?? own)}
          value={value}
          placeholder={digits === 0 ? '150000' : '250.00'}
          onChange={(event) => onChange(event.target.value)}
        />
        {onCurrencyChange && (
          <Select
            id={currencyId}
            // Named "Currency" and not "<label> currency": a name that
            // contains the field's own label makes the two controls
            // indistinguishable to anything that looks a control up by it.
            aria-label="Currency"
            value={currency}
            options={currencies.map((entry) => ({ value: entry.code, label: entry.code }))}
            onChange={(event) => onCurrencyChange(event.target.value)}
          />
        )}
      </div>
    </Field>
  );
}
