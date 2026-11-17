/**
 * Writing a tariff down.
 *
 * The price is either per megabit or for the whole plan, and the figure for
 * one period follows from that: `price × mbit`, or the flat price. The
 * backend works it out and returns it, so nothing is multiplied here.
 *
 * Saving an existing tariff answers with how many running subscriptions it
 * left alone, and that is shown, because it is the thing somebody raising a
 * price will want to know: a subscription holds its own copy of the figures
 * that were agreed, and raising a tariff does not raise what anybody already
 * on it pays.
 */

import { useEffect, useState } from 'react';
import type { Period, TariffInput, TariffRow, Unit } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { plural } from '@/lib/format';
import { AmountField } from './AmountField';
import { useCurrencies, useSaveTariff } from './data';
import { digitsOf, formatMoney, parseAmount } from './money';
import { AGREED_NOTE, PERIOD_WORDS, UNIT_WORDS } from './records';
import styles from './isp.module.css';

export interface TariffDialogProps {
  open: boolean;
  tariff: TariffRow | null;
  onClose: () => void;
}

export function TariffDialog({ open, tariff, onClose }: TariffDialogProps) {
  const currencies = useCurrencies();
  const save = useSaveTariff();
  const reset = save.reset;

  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [mbit, setMbit] = useState('');
  const [unit, setUnit] = useState<Unit>('flat');
  const [period, setPeriod] = useState<Period>('monthly');
  const [price, setPrice] = useState('');
  const [currency, setCurrency] = useState('USD');

  useEffect(() => {
    if (!open) return;
    setName(tariff?.name ?? '');
    setDescription(tariff?.description ?? '');
    setMbit(tariff === null ? '' : String(tariff.mbit));
    setUnit(tariff?.unit ?? 'flat');
    setPeriod(tariff?.period ?? 'monthly');
    setPrice(tariff?.price.text ?? '');
    setCurrency(tariff?.price.currency ?? 'USD');
    reset();
  }, [open, tariff, reset]);

  const digits = digitsOf(currencies.data ?? [], currency);
  const parsed = digits === undefined ? null : parseAmount(price, currency, digits);
  const mbitValid = /^[1-9]\d*$/.test(mbit.trim());
  const ready = name.trim() !== '' && mbitValid && parsed?.ok === true;

  const submit = () => {
    if (!parsed?.ok) return;
    const input: TariffInput = {
      ...(tariff ? { id: tariff.id } : {}),
      name: name.trim(),
      description: description.trim(),
      mbit: Number(mbit.trim()),
      unit,
      period,
      price: parsed.amount.input,
    };
    save.mutate(input, {
      onSuccess: (saved) => {
        toast.success(
          `${saved.tariff.name} is ${formatMoney(saved.period_price)} ${PERIOD_WORDS[saved.tariff.period]}`,
          {
            description:
              saved.subscriptions_unaffected > 0
                ? `${plural(saved.subscriptions_unaffected, 'running subscription')} keep their own copy of the figures that were agreed and are untouched.`
                : undefined,
          },
        );
        onClose();
      },
    });
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={tariff ? `Edit ${tariff.name}` : 'Add a tariff'}
      description={tariff ? AGREED_NOTE : 'A named plan: the megabits, the price and the period.'}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={save.isPending} disabled={!ready} onClick={submit}>
            {tariff ? 'Save' : 'Add the tariff'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {save.error && <p className={styles.error}>{save.error.message}</p>}

        <Field label="Name" required>
          <Input
            autoFocus
            value={name}
            onChange={(event) => setName(event.target.value)}
            placeholder="Home 10"
          />
        </Field>

        <Field label="Description">
          <Textarea
            rows={2}
            value={description}
            onChange={(event) => setDescription(event.target.value)}
          />
        </Field>

        <div className={styles.pair}>
          <Field
            label="Megabits per second"
            required
            error={mbit.trim() !== '' && !mbitValid ? 'A whole number, greater than zero.' : undefined}
          >
            <Input
              mono
              inputMode="numeric"
              value={mbit}
              invalid={mbit.trim() !== '' && !mbitValid}
              onChange={(event) => setMbit(event.target.value)}
              placeholder="10"
            />
          </Field>
          <Field label="Priced" hint={`A ${UNIT_WORDS[unit]} price.`}>
            <Select
              value={unit}
              onChange={(event) => setUnit(event.target.value as Unit)}
              options={[
                { value: 'per_mbit', label: 'Per megabit' },
                { value: 'flat', label: 'Flat, for the whole plan' },
              ]}
            />
          </Field>
        </div>

        <AmountField
          label={unit === 'per_mbit' ? 'Price for one megabit' : 'Price for the plan'}
          required
          value={price}
          onChange={setPrice}
          currency={currency}
          onCurrencyChange={setCurrency}
          currencies={currencies.data ?? []}
          hint={
            unit === 'per_mbit'
              ? 'One period comes to this times the megabits; the engine works that out.'
              : 'This is what one whole period costs, whatever the megabits.'
          }
        />

        <Field label="Charged" hint="A subscription can override this for one customer.">
          <Select
            value={period}
            onChange={(event) => setPeriod(event.target.value as Period)}
            options={[
              { value: 'monthly', label: 'Monthly' },
              { value: 'quarterly', label: 'Quarterly' },
              { value: 'yearly', label: 'Yearly' },
            ]}
          />
        </Field>
      </div>
    </Dialog>
  );
}
