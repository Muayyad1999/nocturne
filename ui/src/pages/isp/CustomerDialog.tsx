/**
 * Adding a customer, and editing one.
 *
 * Two fields are not like the others.
 *
 * * **The currency is required when creating and cannot be changed once there
 *   is a ledger** (`isp.customer.save` refuses it with `conflict`): one
 *   customer's books holding two currencies with nothing saying which figure
 *   was which is exactly the mess this feature is built to avoid. So the
 *   control is a required select on a new customer and is disabled, with the
 *   reason, on an existing one.
 * * **The account state is not here at all.** It goes through
 *   `isp.customer.state`, so that the reason is recorded in the audit trail
 *   with the person and the moment.
 *
 * The device and interface are the join to the rest of Nocturne, and the only
 * reason a usage figure can exist. Saving an interface with no device is
 * allowed and comes back as a warning rather than a refusal, so the warnings
 * are shown.
 */

import { useState } from 'react';
import type { Customer, CustomerInput } from '@/api/types/isp';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { useCurrencies, useDeviceNames, useSaveCustomer } from './data';
import { useOnOpen } from './opening';
import styles from './isp.module.css';

export interface CustomerDialogProps {
  open: boolean;
  /** The customer being edited, or null to add one. */
  customer: Customer | null;
  onClose: () => void;
  /** Told the id of a customer that was just created. */
  onCreated?: (id: string) => void;
}

interface Draft {
  code: string;
  name: string;
  currency: string;
  phone: string;
  email: string;
  address: string;
  site: string;
  deviceId: string;
  interface: string;
  tower: string;
  location: string;
  tags: string[];
  notes: string;
}

function draftOf(customer: Customer | null): Draft {
  return {
    code: customer?.code ?? '',
    name: customer?.name ?? '',
    currency: customer?.currency ?? '',
    phone: customer?.contact.phone ?? '',
    email: customer?.contact.email ?? '',
    address: customer?.contact.address ?? '',
    site: customer?.site ?? '',
    deviceId: customer?.service.device_id ?? '',
    interface: customer?.service.interface ?? '',
    tower: customer?.service.tower ?? '',
    location: customer?.service.location ?? '',
    tags: customer?.tags ?? [],
    notes: customer?.notes ?? '',
  };
}

export function CustomerDialog({ open, customer, onClose, onCreated }: CustomerDialogProps) {
  const currencies = useCurrencies();
  const devices = useDeviceNames();
  const save = useSaveCustomer();
  const reset = save.reset;
  const [draft, setDraft] = useState<Draft>(() => draftOf(customer));

  // Filled when it opens, so a second customer never inherits the first
  // one's half-typed fields — or the refusal the first one got. As it opens
  // and not again (`opening.ts`): on a customer's own page the customer is
  // read again while the save is still reporting back.
  useOnOpen(open, () => {
    setDraft(draftOf(customer));
    reset();
  });

  const editing = customer !== null;
  const nameMissing = draft.name.trim() === '';
  const currencyMissing = !editing && draft.currency === '';

  const submit = () => {
    const input: CustomerInput = {
      ...(editing ? { id: customer.id } : {}),
      name: draft.name.trim(),
      code: draft.code.trim(),
      phone: draft.phone.trim(),
      email: draft.email.trim(),
      address: draft.address.trim(),
      site: draft.site.trim(),
      device_id: draft.deviceId.trim(),
      interface: draft.interface.trim(),
      tower: draft.tower.trim(),
      location: draft.location.trim(),
      tags: draft.tags,
      notes: draft.notes,
      // Sent only when creating: changing it afterwards is refused, and this
      // dialog does not ask for a refusal.
      ...(editing ? {} : { currency: draft.currency }),
    };
    save.mutate(input, {
      onSuccess: (saved) => {
        toast.success(
          saved.created
            ? `${saved.customer.name} is billed in ${saved.customer.currency}`
            : `${saved.customer.name} was updated`,
          saved.warnings.length > 0 ? { description: saved.warnings.join(' ') } : undefined,
        );
        if (saved.created) onCreated?.(saved.customer.id);
        onClose();
      },
    });
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={editing ? `Edit ${customer.name}` : 'Add a customer'}
      description={
        editing
          ? 'Their details and the service that reaches them. The account state is set from the list, so the reason is recorded with it.'
          : 'Who they are, the currency they are billed in, and the device and interface that serve them.'
      }
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            loading={save.isPending}
            disabled={nameMissing || currencyMissing}
            onClick={submit}
          >
            {editing ? 'Save' : 'Add the customer'}
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
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder="Zagros Net"
            />
          </Field>
          <Field label="Account number" hint="Your own, if you use one. It has to be unique.">
            <Input
              mono
              value={draft.code}
              onChange={(event) => setDraft({ ...draft, code: event.target.value })}
              placeholder="C-001"
            />
          </Field>
        </div>

        <Field
          label="Billed in"
          required={!editing}
          hint={
            editing
              ? 'The currency cannot be changed once a customer has a ledger: their books would then hold two currencies with nothing saying which figure was which.'
              : 'Every subscription and invoice of theirs is in this currency, and nothing is ever converted between them.'
          }
        >
          <Select
            value={draft.currency}
            disabled={editing}
            onChange={(event) => setDraft({ ...draft, currency: event.target.value })}
            options={[
              { value: '', label: 'Choose a currency', disabled: true },
              ...(currencies.data ?? []).map((row) => ({
                value: row.code,
                label: `${row.code} — ${row.name}`,
              })),
            ]}
          />
        </Field>

        <div className={styles.pair}>
          <Field label="Phone">
            <Input
              value={draft.phone}
              onChange={(event) => setDraft({ ...draft, phone: event.target.value })}
            />
          </Field>
          <Field label="Email">
            <Input
              type="email"
              value={draft.email}
              onChange={(event) => setDraft({ ...draft, email: event.target.value })}
            />
          </Field>
        </div>

        <Field label="Address">
          <Input
            value={draft.address}
            onChange={(event) => setDraft({ ...draft, address: event.target.value })}
          />
        </Field>

        <div className={styles.pair}>
          <Field label="Site" hint="The site in the inventory they belong to, when it matters.">
            <Input
              value={draft.site}
              onChange={(event) => setDraft({ ...draft, site: event.target.value })}
            />
          </Field>
          <Field
            label="Tower"
            hint="The label the link that feeds them carries, which is how oversubscription and margin know who is behind what."
          >
            <Input
              value={draft.tower}
              onChange={(event) => setDraft({ ...draft, tower: event.target.value })}
              placeholder="Tower A"
            />
          </Field>
        </div>

        <div className={styles.pair}>
          <Field
            label="Serving device"
            hint="Without a device and an interface they have no usage figure at all — not a usage of zero."
          >
            {devices.length > 0 ? (
              <Select
                value={draft.deviceId}
                onChange={(event) => setDraft({ ...draft, deviceId: event.target.value })}
                options={[
                  { value: '', label: 'None recorded' },
                  ...devices.map((device) => ({ value: device.id, label: device.name })),
                ]}
              />
            ) : (
              <Input
                mono
                value={draft.deviceId}
                onChange={(event) => setDraft({ ...draft, deviceId: event.target.value })}
                placeholder="an inventory device id"
              />
            )}
          </Field>
          <Field label="Serving interface" hint="As the device reports it: ether3, Gi0/2.">
            <Input
              mono
              value={draft.interface}
              onChange={(event) => setDraft({ ...draft, interface: event.target.value })}
              placeholder="ether3"
            />
          </Field>
        </div>

        <Field label="Where the service is" hint="In the customer's own words.">
          <Input
            value={draft.location}
            onChange={(event) => setDraft({ ...draft, location: event.target.value })}
          />
        </Field>

        <Field label="Tags">
          <TagInput value={draft.tags} onChange={(tags) => setDraft({ ...draft, tags })} />
        </Field>

        <Field label="Notes">
          <Textarea
            rows={3}
            value={draft.notes}
            onChange={(event) => setDraft({ ...draft, notes: event.target.value })}
          />
        </Field>
      </div>
    </Dialog>
  );
}
