/**
 * Revenue: what was invoiced in a window, what was received in it, and what
 * is owed right now.
 *
 * Those are three different facts and they are not netted off against each
 * other — an invoice issued in October and paid in November belongs to
 * October's `issued` and November's `received`, and the engine says so. Each
 * is one figure per currency.
 *
 * One number across currencies only exists if the person asks for it and
 * supplies the rates: `isp.revenue {convert}` answers with a `Converted`,
 * which carries the rates it used, the moment they are from, and whether
 * minor units were rounded away. It is shown underneath the ledger, labelled
 * a conversion, never instead of it.
 *
 * A rate is typed in minor units of the second currency, because that is
 * what fits in an integer, and read back with the point where that currency
 * puts it (`money.ts: quotedRate`): 108 is "1 EUR = 1.08 USD", not 108
 * dollars to the euro.
 *
 * A month in the window control is a month of the business's calendar.
 */

import { ArrowLeftRight, Calculator } from 'lucide-react';
import { useMemo, useState } from 'react';
import type { ConversionInput, Converted, RevenueParams } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { plural } from '@/lib/format';
import { dayTimeText, monthWindow, periodText, recentMonths } from './calendar';
import { useCalendar, useCurrencies, useRevenue } from './data';
import { Figures, Words } from './Figures';
import { conversionCaption, formatMoney, quotedRate } from './money';
import styles from './isp.module.css';

const DAY = 86_400_000;

interface Rate {
  /** The other currency, which the rate must relate to the target. */
  currency: string;
  /** Minor units of the second currency for one unit of the first. */
  rate: string;
  /** False: `1 target = rate × other`. True: the other way round. */
  swapped: boolean;
}

export function Revenue() {
  const calendar = useCalendar();
  const months = useMemo(() => recentMonths(Date.now(), calendar, 12), [calendar]);
  const [window, setWindow] = useState('30d');
  const [target, setTarget] = useState('');
  const [rates, setRates] = useState<Rate[]>([]);
  const [convert, setConvert] = useState<RevenueParams['convert']>(undefined);

  const span = useMemo(() => {
    const now = Date.now();
    if (window === '30d') return { since: now - 30 * DAY, until: now + 1 };
    if (window === '12m') return { since: now - 365 * DAY, until: now + 1 };
    const month = monthWindow(window, calendar);
    return month ? { since: month.start, until: month.end } : { since: now - 30 * DAY, until: now + 1 };
  }, [window, calendar]);

  const currencies = useCurrencies();
  const revenue = useRevenue({ ...span, ...(convert ? { convert } : {}) });

  const present = useMemo(() => {
    const codes = new Set<string>();
    for (const money of revenue.data?.issued ?? []) codes.add(money.currency);
    for (const money of revenue.data?.received ?? []) codes.add(money.currency);
    return [...codes];
  }, [revenue.data]);

  const others = present.filter((code) => code !== target);
  const rateOf = (code: string) => rates.find((entry) => entry.currency === code);
  const setRate = (code: string, patch: Partial<Rate>) =>
    setRates((current) => {
      const found = current.find((entry) => entry.currency === code);
      if (!found) return [...current, { currency: code, rate: '', swapped: false, ...patch }];
      return current.map((entry) => (entry.currency === code ? { ...entry, ...patch } : entry));
    });

  const ready =
    target !== '' &&
    others.every((code) => /^[1-9]\d*$/.test((rateOf(code)?.rate ?? '').trim()));

  const ask = () => {
    if (!ready) return;
    const supplied: ConversionInput[] = others.map((code) => {
      const entry = rateOf(code);
      const rate = Number((entry?.rate ?? '').trim());
      return entry?.swapped
        ? { from: code, to: target, rate }
        : { from: target, to: code, rate };
    });
    setConvert({ to: target, rates: supplied, at: Date.now() });
  };

  return (
    <Panel
      title="Revenue"
      padding="none"
      actions={
        <Select
          size="sm"
          aria-label="Window"
          value={window}
          onChange={(event) => {
            setWindow(event.target.value);
            setConvert(undefined);
          }}
          options={[
            { value: '30d', label: 'Last 30 days' },
            { value: '12m', label: 'Last 12 months' },
            ...months,
          ]}
        />
      }
    >
      <div className={styles.panelBody}>
        {revenue.isError && (
          <ErrorState
            size="compact"
            title="The revenue could not be read"
            error={revenue.error}
            onRetry={() => void revenue.refetch()}
          />
        )}
        {revenue.isPending && <Skeleton shape="block" height={140} />}
        {revenue.data && (
          <>
            <p className={styles.quiet}>
              {periodText(revenue.data.since, revenue.data.until, calendar)}, days of the
              business’s calendar
            </p>
            <div className={styles.stats}>
              <div className={styles.statGroup}>
                <span className={styles.statLabel}>Invoiced in the window</span>
                <Figures totals={revenue.data.issued} empty="no invoice was issued" />
                <span className={styles.quiet}>
                  {plural(revenue.data.invoices_issued, 'invoice')}
                </span>
              </div>
              <div className={styles.statGroup}>
                <span className={styles.statLabel}>Received in the window</span>
                <Figures
                  totals={revenue.data.received}
                  empty="no payment was received"
                />
                <span className={styles.quiet}>in the currency it was received in</span>
              </div>
              <div className={styles.statGroup}>
                <span className={styles.statLabel}>Owed right now</span>
                <Figures totals={revenue.data.outstanding} empty="nothing is owed" />
                <span className={styles.quiet}>not limited to this window</span>
              </div>
            </div>
            <p className={styles.quiet}>{revenue.data.note}</p>

            {present.length > 1 && (
              <div className={styles.form}>
                <div className={styles.inlineFields}>
                  <Field label="One figure, at rates you supply">
                    <Select
                      size="sm"
                      value={target}
                      onChange={(event) => setTarget(event.target.value)}
                      options={[
                        { value: '', label: 'Choose a currency', disabled: true },
                        ...present.map((code) => ({ value: code, label: code })),
                      ]}
                    />
                  </Field>
                  {target !== '' &&
                    others.map((code) => {
                      const entry = rateOf(code);
                      const first = entry?.swapped ? code : target;
                      const second = entry?.swapped ? target : code;
                      return (
                        <Field
                          key={code}
                          label={`1 ${first} = ? minor units of ${second}`}
                          hint={
                            quotedRate(first, second, entry?.rate ?? '', currencies.data ?? []) ===
                            null
                              ? 'a whole number, as it was quoted to you'
                              : `as quoted: ${quotedRate(first, second, entry?.rate ?? '', currencies.data ?? [])}`
                          }
                        >
                          <div className={styles.rateRow}>
                            <Input
                              mono
                              inputMode="numeric"
                              size="sm"
                              value={entry?.rate ?? ''}
                              placeholder="1320"
                              onChange={(event) => setRate(code, { rate: event.target.value })}
                            />
                            <Button
                              size="sm"
                              icon={ArrowLeftRight}
                              aria-label={`Swap the direction of the ${code} rate`}
                              onClick={() => setRate(code, { swapped: !entry?.swapped })}
                            />
                          </div>
                        </Field>
                      );
                    })}
                  <Button size="sm" icon={Calculator} disabled={!ready} onClick={ask}>
                    Convert
                  </Button>
                  {convert && (
                    <Button size="sm" onClick={() => setConvert(undefined)}>
                      Clear the conversion
                    </Button>
                  )}
                </div>

                {revenue.data.converted ? (
                  <>
                    <ConvertedFigure
                      label="Invoiced, converted"
                      converted={revenue.data.converted.issued}
                      currencies={currencies.data ?? []}
                    />
                    <ConvertedFigure
                      label="Received, converted"
                      converted={revenue.data.converted.received}
                      currencies={currencies.data ?? []}
                    />
                  </>
                ) : (
                  <p className={styles.quiet}>
                    <Words>
                      the figures above are the ledger: there is no single number across
                      currencies until you supply the rates for one
                    </Words>
                  </p>
                )}
              </div>
            )}
          </>
        )}
      </div>
    </Panel>
  );
}

function ConvertedFigure({
  label,
  converted,
  currencies,
}: {
  label: string;
  converted: Converted;
  currencies: Parameters<typeof conversionCaption>[1];
}) {
  const calendar = useCalendar();
  return (
    <div className={styles.card} data-basis="conversion">
      <p className={styles.cardTitle}>
        <span>{label}</span>
        <span className={styles.statValue}>{formatMoney(converted.amount)}</span>
      </p>
      <p className={styles.quiet}>
        {conversionCaption(converted, currencies)} Of{' '}
        {converted.of.map((money) => formatMoney(money)).join(' and ')}, at{' '}
        {dayTimeText(converted.at, calendar)}.
        {converted.exact ? '' : ' Minor units were rounded away in the conversion.'}
      </p>
    </div>
  );
}
