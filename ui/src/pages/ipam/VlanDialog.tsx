/**
 * Writing a VLAN down, or editing the record of one.
 *
 * A VLAN that only the devices report has no record at all (`id: ""`), so this
 * form creates one for it rather than editing something that does not exist —
 * and it says what each device currently calls that id, because the documented
 * name is a decision about a disagreement, not a default.
 */

import { useEffect, useState } from 'react';
import type { PrefixRow, VlanInput, VlanRow } from '@/api/types/ipam';
import type { RpcError } from '@/api/errors';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import styles from './Dialogs.module.css';

export interface VlanDialogProps {
  open: boolean;
  /** The row being documented or edited; null to create one from scratch. */
  row: VlanRow | null;
  defaultSite: string;
  /** Every block, so a VLAN can be linked to the ones that carry it. */
  prefixes: readonly PrefixRow[];
  saving: boolean;
  error: RpcError | null;
  onClose: () => void;
  onSave: (input: VlanInput) => void;
}

export function VlanDialog({
  open,
  row,
  defaultSite,
  prefixes,
  saving,
  error,
  onClose,
  onSave,
}: VlanDialogProps) {
  const [vid, setVid] = useState('');
  const [name, setName] = useState('');
  const [site, setSite] = useState('');
  const [description, setDescription] = useState('');
  const [cidrs, setCidrs] = useState<string[]>([]);

  useEffect(() => {
    if (!open) return;
    setVid(row ? String(row.vid) : '');
    setName(row?.name ?? '');
    setSite(row?.site ?? defaultSite);
    setDescription(row?.description ?? '');
    setCidrs(row?.prefixes.map((prefix) => prefix.cidr) ?? []);
  }, [open, row, defaultSite]);

  const vidValid = /^\d{1,4}$/.test(vid.trim()) && Number(vid.trim()) >= 1 && Number(vid.trim()) <= 4094;
  const byCidr = new Map(prefixes.map((prefix) => [prefix.cidr, prefix.id]));
  const unknown = cidrs.filter((cidr) => !byCidr.has(cidr));

  const submit = () => {
    onSave({
      ...(row && row.id !== '' ? { id: row.id } : {}),
      vid: Number(vid.trim()),
      name: name.trim(),
      site: site.trim(),
      description: description.trim(),
      prefix_ids: cidrs.map((cidr) => byCidr.get(cidr)).filter((id): id is string => Boolean(id)),
    });
  };

  const reported = row?.devices ?? [];

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={
        row === null
          ? 'Add a VLAN'
          : row.documented
            ? `Edit VLAN ${row.vid}`
            : `Write a record for VLAN ${row.vid}`
      }
      description={
        row !== null && !row.documented
          ? 'The devices already report this VLAN. Writing a record says what it is for, and gives the name the devices are then checked against.'
          : 'A VLAN record is part of the plan. Nothing is sent to a switch.'
      }
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={saving} disabled={!vidValid} onClick={submit}>
            Save
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <div className={styles.pair}>
          <Field
            label="VLAN id"
            required
            error={vid !== '' && !vidValid ? 'A VLAN id is a number from 1 to 4094.' : undefined}
          >
            <Input
              mono
              value={vid}
              onChange={(event) => setVid(event.target.value)}
              disabled={row !== null}
              placeholder="10"
            />
          </Field>
          <Field label="Name">
            <Input
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Staff"
            />
          </Field>
        </div>

        {reported.length > 0 && (
          <div className={styles.observed}>
            <p className={styles.quiet}>What the devices call it:</p>
            {reported.slice(0, 6).map((device) => (
              <Badge key={device.device_id} tone="warn" variant="outline">
                {device.device}: {device.name || 'unnamed'}
              </Badge>
            ))}
            {reported[0] && reported[0].name !== name && (
              <Button size="sm" variant="ghost" onClick={() => setName(reported[0]?.name ?? '')}>
                Use “{reported[0].name}”
              </Button>
            )}
          </div>
        )}

        <div className={styles.pair}>
          <Field label="Site" hint="A lab’s VLANs carry their lab’s namespace.">
            <Input
              value={site}
              onChange={(event) => setSite(event.target.value)}
              disabled={row !== null}
              placeholder="HQ"
            />
          </Field>
          <Field
            label="Blocks"
            hint="The blocks this VLAN carries, by CIDR."
            error={unknown.length > 0 ? `Not a block on record: ${unknown.join(', ')}` : undefined}
          >
            <TagInput mono value={cidrs} onChange={setCidrs} placeholder="10.20.1.0/24" />
          </Field>
        </div>

        <Field label="Description">
          <Textarea
            rows={2}
            value={description}
            onChange={(event) => setDescription(event.target.value)}
          />
        </Field>

        {error && <p className={styles.error}>{error.message}</p>}
      </div>
    </Dialog>
  );
}
