/**
 * Selling a plan, and changing one.
 *
 * A subscription takes its figures from a tariff and then **keeps its own
 * copy** of them. That is why this dialog offers the tariff's megabits and
 * price as the starting point and lets either be overridden for this
 * customer: a negotiated figure is a property of the agreement, not of the
 * tariff.
 *
 * Changing what somebody pays is an **amendment**, not an edit: the old
 * subscription ends on the date given and a new one starts, so invoices
 * already issued on the old terms stay exactly as they were. The dialog says
 * so before it is done, because "edit" would be the wrong word and the wrong
 * thing.
 *
 * The price must be in the customer's own currency — the engine refuses
 * anything else, since every invoice of theirs is in it.
 *
 * The date a plan starts, or new terms take over, is a day of the business's
 * calendar and is sent as that day's first moment there (`calendar.ts`). A
 * billing month begins at the same midnight, so a plan that "starts on the
 * 1st" is in force for the whole of that month and for none of the one
 * before — which it was not while the date was read in the browser's zone.
 */

import { useEffect, useState } from 'react';
import type { Period, SubscriptionRow, TariffRow, Unit } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { AmountField } from './AmountField';
import { dayStart, dayText, offsetName } from './calendar';
import {
  useAmendSubscription,
  useCalendar,
  useCreateSubscription,
  useCurrencies,
  useTariffs,
} from './data';
import { digitsOf, formatMoney, parseAmount } from './money';
import { AGREED_NOTE, AMEND_NOTE, PERIOD_WORDS } from './records';
import styles from './isp.module.css';

export interface SubscriptionDialogProps {
  open: boolean;
  /** The subscription being amended, or null to sell a new plan. */
  subscription: SubscriptionRow | null;
  customerId: string;
  /** The customer's currency: every figure of theirs is in it. */
  currency: string;
  onClose: () => void;
}

export function SubscriptionDialog({
  open,
  subscription,
  customerId,
  currency,
  onClose,
}: SubscriptionDialogProps) {
  const tariffs = useTariffs();
  const currencies = useCurrencies();
  const calendar = useCalendar();
  const create = useCreateSubscription();
  const amend = useAmendSubscription();
  const amending = subscription !== null;
  const mutation = amending ? amend : create;
  const reset = mutation.reset;

  const [tariffId, setTariffId] = useState('');
  const [mbit, setMbit] = useState('');
  const [price, setPrice] = useState('');
  const [unit, setUnit] = useState<Unit | ''>('');
  const [period, setPeriod] = useState<Period | ''>('');
  const [from, setFrom] = useState('');
  const [note, setNote] = useState('');

  useEffect(() => {
    if (!open) return;
    setTariffId(subscription?.tariff_id ?? '');
    setMbit(subscription === null ? '' : String(subscription.mbit));
    setPrice(subscription?.unit_price.text ?? '');
    setUnit('');
    setPeriod('');
    setFrom('');
    setNote('');
    reset();
  }, [open, subscription, reset]);

  const chosen: TariffRow | undefined = (tariffs.data ?? []).find((row) => row.id === tariffId);
  const digits = digitsOf(currencies.data ?? [], currency);
  const parsedPrice = price.trim() === '' || digits === undefined ? null : parseAmount(price, currency, digits);
  const mbitValid = mbit.trim() === '' || /^[1-9]\d*$/.test(mbit.trim());
  const at = dayStart(from, calendar);
  // A date that was typed and cannot be read is not "now": without the
  // business's calendar there is no telling which instant the day begins at.
  const dateUnread = from.trim() !== '' && at === null;
  const ready =
    mbitValid &&
    parsedPrice?.ok !== false &&
    !dateUnread &&
    (amending ? mbit.trim() !== '' || price.trim() !== '' : tariffId !== '');

  const submit = () => {
    if (dateUnread) return;
    if (amending) {
      amend.mutate(
        {
          id: subscription.id,
          ...(mbit.trim() === '' ? {} : { mbit: Number(mbit.trim()) }),
          ...(parsedPrice?.ok ? { price: parsedPrice.amount.input } : {}),
          ...(at === null ? {} : { from_at: at }),
          ...(note.trim() === '' ? {} : { note: note.trim() }),
        },
        {
          onSuccess: (saved) => {
            toast.success(
              `Now ${formatMoney(saved.subscription.price)} ${PERIOD_WORDS[saved.subscription.period]}`,
              { description: saved.note },
            );
            onClose();
          },
        },
      );
      return;
    }
    create.mutate(
      {
        customer_id: customerId,
        tariff_id: tariffId,
        ...(mbit.trim() === '' ? {} : { mbit: Number(mbit.trim()) }),
        ...(parsedPrice?.ok ? { price: parsedPrice.amount.input } : {}),
        ...(unit === '' ? {} : { unit }),
        ...(period === '' ? {} : { period }),
        ...(at === null ? {} : { started_at: at }),
        ...(note.trim() === '' ? {} : { note: note.trim() }),
      },
      {
        onSuccess: (saved) => {
          toast.success(
            `${saved.subscription.tariff_name}: ${formatMoney(saved.subscription.price)} ${
              PERIOD_WORDS[saved.subscription.period]
            }`,
            { description: saved.note },
          );
          onClose();
        },
      },
    );
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={amending ? `Amend the ${subscription.tariff_name} plan` : 'Sell a plan'}
      description={amending ? AMEND_NOTE : AGREED_NOTE}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={mutation.isPending} disabled={!ready} onClick={submit}>
            {amending ? 'Amend the terms' : 'Sell it'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {mutation.error && <p className={styles.error}>{mutation.error.message}</p>}

        {!amending && (
          <Field
            label="Tariff"
            required
            hint={
              chosen
                ? `${formatMoney(chosen.period_price)} ${PERIOD_WORDS[chosen.period]} at the tariff's own figures.`
                : 'Where the terms come from. The figures are copied into the subscription.'
            }
          >
            <Select
              value={tariffId}
              onChange={(event) => {
                setTariffId(event.target.value);
                setMbit('');
                setPrice('');
              }}
              options={[
                { value: '', label: 'Choose a tariff', disabled: true },
                ...(tariffs.data ?? []).map((row) => ({
                  value: row.id,
                  label: `${row.name} — ${row.mbit} Mbit/s, ${formatMoney(row.period_price)} ${
                    PERIOD_WORDS[row.period]
                  }`,
                  // A tariff priced in another currency cannot be sold to
                  // this customer: every figure of theirs is in theirs.
                  disabled: row.price.currency !== currency,
                })),
              ]}
            />
          </Field>
        )}

        <div className={styles.pair}>
          <Field
            label="Megabits per second"
            hint={
              amending
                ? 'Leave it empty to keep the agreed megabits.'
                : "Leave it empty to take the tariff's."
            }
            error={mbitValid ? undefined : 'A whole number, greater than zero.'}
          >
            <Input
              mono
              inputMode="numeric"
              value={mbit}
              invalid={!mbitValid}
              placeholder={chosen ? String(chosen.mbit) : subscription ? String(subscription.mbit) : ''}
              onChange={(event) => setMbit(event.target.value)}
            />
          </Field>
          <AmountField
            label="Agreed unit price"
            value={price}
            onChange={setPrice}
            currency={currency}
            currencies={currencies.data ?? []}
            hint={
              amending
                ? 'Leave it empty to keep the agreed price.'
                : "Leave it empty to take the tariff's. A negotiated figure belongs here, not on the tariff."
            }
          />
        </div>

        {!amending && (
          <div className={styles.pair}>
            <Field label="Priced" hint="Leave it as the tariff's unless this customer differs.">
              <Select
                value={unit}
                onChange={(event) => setUnit(event.target.value as Unit | '')}
                options={[
                  { value: '', label: `As the tariff (${chosen ? chosen.unit : 'per megabit or flat'})` },
                  { value: 'per_mbit', label: 'Per megabit' },
                  { value: 'flat', label: 'Flat, for the whole plan' },
                ]}
              />
            </Field>
            <Field label="Charged">
              <Select
                value={period}
                onChange={(event) => setPeriod(event.target.value as Period | '')}
                options={[
                  { value: '', label: `As the tariff (${chosen ? chosen.period : 'monthly'})` },
                  { value: 'monthly', label: 'Monthly' },
                  { value: 'quarterly', label: 'Quarterly' },
                  { value: 'yearly', label: 'Yearly' },
                ]}
              />
            </Field>
          </div>
        )}

        <Field
          label={amending ? 'The new terms start' : 'Starts'}
          error={
            dateUnread
              ? calendar === null
                ? 'The business’s calendar is not known, so a date cannot be read. Leave it empty for now.'
                : 'That is not a date.'
              : undefined
          }
          hint={`${
            amending
              ? 'The old terms end on this date and the new ones start. Leave it empty for now.'
              : 'Leave it empty for now. A plan that started mid-month is pro-rated on the invoice, with the arithmetic on the line.'
          }${
            calendar === null
              ? ''
              : ` A day of the business’s calendar (${offsetName(calendar)}), from its first moment; today is ${dayText(Date.now(), calendar)}.`
          }`}
        >
          <Input type="date" value={from} onChange={(event) => setFrom(event.target.value)} />
        </Field>

        <Field label="Note">
          <Textarea rows={2} value={note} onChange={(event) => setNote(event.target.value)} />
        </Field>
      </div>
    </Dialog>
  );
}
