/**
 * Writing an address record, or reserving one.
 *
 * A record is a statement by a person, so this form never fills itself in from
 * what the network reported: the sightings are shown beside the fields and
 * copying one across is a button the user presses. A reservation that lands on
 * an address something already holds is refused by the backend with the holder
 * named, and that refusal is shown as it came — it is the useful part.
 */

import { ArrowLeft } from 'lucide-react';
import { useEffect, useState } from 'react';
import type { Address, AddressInput, ReserveParams, SavedAddress, Status } from '@/api/types/ipam';
import type { RpcError } from '@/api/errors';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { TagInput } from '@/design/TagInput';
import { TimeAgo } from '@/design/TimeAgo';
import styles from './Dialogs.module.css';

const STATUSES: readonly { value: Status; label: string }[] = [
  { value: 'active', label: 'in use' },
  { value: 'reserved', label: 'reserved' },
  { value: 'dhcp', label: 'part of a DHCP pool' },
];

export type AddressDialogMode = 'record' | 'reserve';

export interface AddressDialogProps {
  open: boolean;
  mode: AddressDialogMode;
  /** The block the record belongs to. */
  prefixId: string | null;
  prefixCidr: string | null;
  /** The address being edited, when there is one. */
  address: Address | null;
  /** A pre-filled address (from the grid, a free-space answer or a report). */
  suggestedIp: string;
  scope: string;
  saving: boolean;
  error: RpcError | null;
  saved: SavedAddress | null;
  onClose: () => void;
  onSave: (input: AddressInput) => void;
  onReserve: (params: ReserveParams) => void;
}

interface Draft {
  ip: string;
  status: Status;
  hostname: string;
  description: string;
  interface: string;
  mac: string;
  tags: string[];
}

function draftOf(address: Address | null, suggestedIp: string): Draft {
  return {
    ip: address?.ip ?? suggestedIp,
    status: address?.documented && address.status !== 'free' && address.status !== 'conflict'
      ? address.status
      : 'active',
    hostname: address?.documented ? (address.hostname ?? '') : '',
    description: address?.documented ? (address.description ?? '') : '',
    interface: address?.documented ? (address.interface ?? '') : '',
    mac: address?.documented ? (address.mac ?? '') : '',
    tags: address?.tags ?? [],
  };
}

export function AddressDialog({
  open,
  mode,
  prefixId,
  prefixCidr,
  address,
  suggestedIp,
  scope,
  saving,
  error,
  saved,
  onClose,
  onSave,
  onReserve,
}: AddressDialogProps) {
  const [draft, setDraft] = useState<Draft>(() => draftOf(address, suggestedIp));

  useEffect(() => {
    if (open) setDraft(draftOf(address, suggestedIp));
  }, [open, address, suggestedIp]);

  const reserving = mode === 'reserve';
  const strongest = address?.observations[0];

  const submit = () => {
    if (reserving) {
      if (!prefixId) return;
      onReserve({
        prefix_id: prefixId,
        ...(draft.ip.trim() === '' ? {} : { ip: draft.ip.trim() }),
        hostname: draft.hostname.trim(),
        description: draft.description.trim(),
      });
      return;
    }
    onSave({
      ip: draft.ip.trim(),
      ...(prefixId ? { prefix_id: prefixId } : {}),
      status: draft.status,
      hostname: draft.hostname.trim(),
      description: draft.description.trim(),
      interface: draft.interface.trim(),
      mac: draft.mac.trim(),
      tags: draft.tags,
      ...(scope ? { scope } : {}),
    });
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={
        reserving
          ? `Reserve an address in ${prefixCidr ?? 'this block'}`
          : address?.documented
            ? `Edit the record for ${address.ip}`
            : `Write a record for ${draft.ip || 'an address'}`
      }
      description={
        reserving
          ? 'A reservation is a record that says this address is spoken for. An address something already holds is refused, and the holder is named.'
          : 'A record is what a person says about an address. It is never written from what the devices report; that stays separate evidence.'
      }
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            loading={saving}
            disabled={!reserving && draft.ip.trim() === ''}
            onClick={submit}
          >
            {reserving ? 'Reserve it' : 'Save the record'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field
          label="Address"
          hint={
            reserving
              ? 'Leave it empty to take the first address the engine finds free.'
              : undefined
          }
          error={error?.code === 'invalid' ? error.message : undefined}
        >
          <Input
            mono
            value={draft.ip}
            onChange={(event) => setDraft({ ...draft, ip: event.target.value })}
            placeholder={reserving ? 'the first free one' : '10.20.1.50'}
            disabled={address?.documented ?? false}
          />
        </Field>

        {!reserving && (
          <Field label="Status">
            <Select
              value={draft.status}
              onChange={(event) => setDraft({ ...draft, status: event.target.value as Status })}
              options={STATUSES}
            />
          </Field>
        )}

        <div className={styles.pair}>
          <Field label="Hostname">
            <Input
              value={draft.hostname}
              onChange={(event) => setDraft({ ...draft, hostname: event.target.value })}
              placeholder="printer-1"
            />
          </Field>
          <Field label="Description">
            <Input
              value={draft.description}
              onChange={(event) => setDraft({ ...draft, description: event.target.value })}
            />
          </Field>
        </div>

        {!reserving && (
          <>
            <div className={styles.pair}>
              <Field label="Interface" hint="Whose the address is, as the plan says.">
                <Input
                  mono
                  value={draft.interface}
                  onChange={(event) => setDraft({ ...draft, interface: event.target.value })}
                  placeholder="Vlan10"
                />
              </Field>
              <Field label="MAC">
                <Input
                  mono
                  value={draft.mac}
                  onChange={(event) => setDraft({ ...draft, mac: event.target.value })}
                  placeholder="02:4e:43:d0:fd:40"
                />
              </Field>
            </div>

            {strongest && (
              <div className={styles.observed}>
                <p className={styles.quiet}>
                  The network says: <Badge tone="warn" variant="outline">{strongest.kind}</Badge>{' '}
                  {strongest.device}
                  {strongest.interface ? ` (${strongest.interface})` : ''}
                  {strongest.mac ? ` · ${strongest.mac}` : ''} · read{' '}
                  <TimeAgo at={strongest.collected_at} />
                </p>
                <Button
                  size="sm"
                  variant="ghost"
                  icon={ArrowLeft}
                  onClick={() =>
                    setDraft({
                      ...draft,
                      interface: strongest.interface,
                      mac: strongest.mac ?? draft.mac,
                    })
                  }
                >
                  Copy that into the record
                </Button>
              </div>
            )}

            <Field label="Tags">
              <TagInput
                value={draft.tags}
                onChange={(tags) => setDraft({ ...draft, tags })}
                placeholder="printer, floor-3"
              />
            </Field>
          </>
        )}

        {error && error.code !== 'invalid' && (
          <div className={styles.error}>
            <p className={styles.errorTitle}>
              {error.code === 'conflict' ? 'That address is taken' : 'That record was refused'}
            </p>
            <p>{error.message}</p>
          </div>
        )}

        {saved && saved.warnings.length > 0 && (
          <div className={styles.warnings}>
            <p className={styles.warningsTitle}>Saved. The engine warned, rather than refused:</p>
            <ul>
              {saved.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </Dialog>
  );
}
