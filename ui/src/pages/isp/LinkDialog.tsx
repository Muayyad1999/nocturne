/**
 * Writing a link down: an upstream, or the feed to a tower.
 *
 * Three fields earn their place. The **tower** label is the join to the
 * customers behind it — it is the same string as their `service.tower`, which
 * is how oversubscription and margin know who is on what. The **capacity** is
 * what somebody wrote down, and without it there is no ratio at all (not a
 * ratio of zero). The **wholesale cost of a megabit** is what makes a margin
 * possible, and the margin is only ever worked out in that cost's currency.
 *
 * Either figure can be taken off again by emptying its field: a capacity or
 * a cost written down in error must not go on feeding a ratio or a margin
 * (`records.ts: linkInput`).
 */

import { useEffect, useState } from 'react';
import type { Link, Period } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { AmountField } from './AmountField';
import { useCurrencies, useDeviceNames, useSaveLink } from './data';
import { digitsOf, parseAmount } from './money';
import { linkInput } from './records';
import styles from './isp.module.css';

export interface LinkDialogProps {
  open: boolean;
  link: Link | null;
  onClose: () => void;
}

export function LinkDialog({ open, link, onClose }: LinkDialogProps) {
  const currencies = useCurrencies();
  const devices = useDeviceNames();
  const save = useSaveLink();
  const reset = save.reset;

  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [tower, setTower] = useState('');
  const [deviceId, setDeviceId] = useState('');
  const [iface, setIface] = useState('');
  const [capacity, setCapacity] = useState('');
  const [cost, setCost] = useState('');
  const [currency, setCurrency] = useState('USD');
  const [costPeriod, setCostPeriod] = useState<Period>('monthly');

  useEffect(() => {
    if (!open) return;
    setName(link?.name ?? '');
    setDescription(link?.description ?? '');
    setTower(link?.tower ?? '');
    setDeviceId(link?.device_id ?? '');
    setIface(link?.interface ?? '');
    setCapacity(link === null || link.capacity_mbit === 0 ? '' : String(link.capacity_mbit));
    setCost(link?.cost_per_mbit?.text ?? '');
    setCurrency(link?.cost_per_mbit?.currency ?? 'USD');
    setCostPeriod(link?.cost_period ?? 'monthly');
    reset();
  }, [open, link, reset]);

  const digits = digitsOf(currencies.data ?? [], currency);
  const costTyped = cost.trim() !== '';
  const parsedCost = !costTyped || digits === undefined ? null : parseAmount(cost, currency, digits);
  const capacityValid = capacity.trim() === '' || /^[1-9]\d*$/.test(capacity.trim());
  // A typed cost has to have been read before the link is saved: an emptied
  // field takes the cost off the link, and a figure that merely could not be
  // read yet (the currencies still loading) must not be taken for one.
  const ready =
    name.trim() !== '' && capacityValid && (!costTyped || parsedCost?.ok === true);

  const submit = () => {
    const input = linkInput(
      {
        name,
        description,
        tower,
        deviceId,
        iface,
        capacity,
        cost: parsedCost?.ok ? parsedCost.amount.input : null,
        costPeriod,
      },
      link,
    );
    save.mutate(input, {
      onSuccess: (saved) => {
        toast.success(saved.created ? `${saved.link.name} was added` : `${saved.link.name} was updated`);
        onClose();
      },
    });
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={link ? `Edit ${link.name}` : 'Add a link'}
      description="An upstream or a tower's feed: what it can carry, what a megabit on it costs you, and where it can be measured."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" loading={save.isPending} disabled={!ready} onClick={submit}>
            {link ? 'Save' : 'Add the link'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {save.error && <p className={styles.error}>{save.error.message}</p>}

        <div className={styles.pair}>
          <Field label="Name" required>
            <Input
              autoFocus
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Uplink A"
            />
          </Field>
          <Field
            label="Tower"
            hint="The same label the customers behind it carry, which is how the reports know who is on it."
          >
            <Input
              value={tower}
              onChange={(event) => setTower(event.target.value)}
              placeholder="Tower A"
            />
          </Field>
        </div>

        <Field label="Description">
          <Textarea
            rows={2}
            value={description}
            onChange={(event) => setDescription(event.target.value)}
          />
        </Field>

        <div className={styles.pair}>
          <Field
            label="Device"
            hint="Without a device and an interface the link has no measured load — which is not a load of zero."
          >
            {devices.length > 0 ? (
              <Select
                value={deviceId}
                onChange={(event) => setDeviceId(event.target.value)}
                options={[
                  { value: '', label: 'None recorded' },
                  ...devices.map((device) => ({ value: device.id, label: device.name })),
                ]}
              />
            ) : (
              <Input
                mono
                value={deviceId}
                onChange={(event) => setDeviceId(event.target.value)}
                placeholder="an inventory device id"
              />
            )}
          </Field>
          <Field label="Interface" hint="As the device reports it.">
            <Input
              mono
              value={iface}
              onChange={(event) => setIface(event.target.value)}
              placeholder="ether1"
            />
          </Field>
        </div>

        <Field
          label="Capacity in Mbit/s"
          hint="What it can carry, as you know it. Empty, there is no ratio to show — not a ratio of zero — unless the interface’s own speed is known."
          error={capacityValid ? undefined : 'A whole number, greater than zero.'}
        >
          <Input
            mono
            inputMode="numeric"
            value={capacity}
            invalid={!capacityValid}
            onChange={(event) => setCapacity(event.target.value)}
            placeholder="100"
          />
        </Field>

        <AmountField
          label="Wholesale cost of one megabit"
          value={cost}
          onChange={setCost}
          currency={currency}
          onCurrencyChange={setCurrency}
          currencies={currencies.data ?? []}
          hint="Empty, the link has no wholesale cost and there is no margin to work out — not a margin against a cost of nothing. The margin is only ever in this currency."
        />

        <Field label="That cost is per">
          <Select
            value={costPeriod}
            onChange={(event) => setCostPeriod(event.target.value as Period)}
            options={[
              { value: 'monthly', label: 'Month' },
              { value: 'quarterly', label: 'Quarter' },
              { value: 'yearly', label: 'Year' },
            ]}
          />
        </Field>
      </div>
    </Dialog>
  );
}
