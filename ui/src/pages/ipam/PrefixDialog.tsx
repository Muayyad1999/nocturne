/**
 * Writing a block down — which is also how a discovered block becomes part of
 * the plan ("adoption": the same record is kept, with its provenance, and its
 * source becomes `manual`).
 *
 * Two of the backend's answers are shown rather than swallowed:
 *
 * * a CIDR with host bits set is refused with the corrected network in
 *   `details.corrected`, and that correction is offered as a button;
 * * an overlap that is not containment is a **warning**, not a refusal, so the
 *   save succeeds and the warnings are shown afterwards.
 */

import { useEffect, useState } from 'react';
import type { PrefixInput, PrefixRow, Role, SavedPrefix } from '@/api/types/ipam';
import type { RpcError } from '@/api/errors';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import styles from './Dialogs.module.css';

const ROLES: readonly Role[] = [
  'lan',
  'wan',
  'loopback',
  'management',
  'p2p',
  'dmz',
  'guest',
  'voice',
  'other',
];

export interface PrefixDialogProps {
  open: boolean;
  /** The block being edited, or null to create one. */
  prefix: PrefixRow | null;
  /** Pre-filled CIDR when it comes from a split or a free-space answer. */
  suggestedCidr?: string;
  /** The site a new block inherits from the current filter. */
  defaultSite?: string;
  saving: boolean;
  error: RpcError | null;
  saved: SavedPrefix | null;
  onClose: () => void;
  onSave: (input: PrefixInput) => void;
}

interface Draft {
  cidr: string;
  name: string;
  description: string;
  site: string;
  role: string;
  vlan: string;
  gateway: string;
  dhcpStart: string;
  dhcpEnd: string;
  tags: string[];
}

function draftOf(prefix: PrefixRow | null, suggestedCidr = '', defaultSite = ''): Draft {
  return {
    cidr: prefix?.cidr ?? suggestedCidr,
    name: prefix?.name ?? '',
    description: prefix?.description ?? '',
    site: prefix?.site ?? defaultSite,
    role: prefix?.role ?? '',
    vlan: prefix?.vlan_id === undefined ? '' : String(prefix.vlan_id),
    gateway: prefix?.gateway ?? '',
    dhcpStart: prefix?.dhcp?.start ?? '',
    dhcpEnd: prefix?.dhcp?.end ?? '',
    tags: prefix?.tags ?? [],
  };
}

/** The corrected network a `host bits set` refusal carries. */
function correctedCidr(error: RpcError | null): string | null {
  if (!error || error.code !== 'invalid') return null;
  const details = error.details;
  if (typeof details !== 'object' || details === null) return null;
  const corrected = (details as { corrected?: unknown }).corrected;
  return typeof corrected === 'string' ? corrected : null;
}

export function PrefixDialog({
  open,
  prefix,
  suggestedCidr,
  defaultSite,
  saving,
  error,
  saved,
  onClose,
  onSave,
}: PrefixDialogProps) {
  const [draft, setDraft] = useState<Draft>(() => draftOf(prefix, suggestedCidr, defaultSite));

  // The dialog is mounted once and filled when it opens, so a second block
  // never inherits the first one's half-typed fields.
  useEffect(() => {
    if (open) setDraft(draftOf(prefix, suggestedCidr, defaultSite));
  }, [open, prefix, suggestedCidr, defaultSite]);

  const editing = prefix !== null;
  const correction = correctedCidr(error);
  const vlanInvalid = draft.vlan !== '' && !/^\d{1,4}$/.test(draft.vlan.trim());

  const submit = () => {
    const input: PrefixInput = {
      ...(editing ? { id: prefix.id } : {}),
      cidr: draft.cidr.trim(),
      name: draft.name.trim(),
      description: draft.description.trim(),
      site: draft.site.trim(),
      gateway: draft.gateway.trim(),
      tags: draft.tags,
      ...(draft.role === '' ? {} : { role: draft.role as Role }),
      ...(draft.vlan.trim() === '' ? {} : { vlan_id: Number(draft.vlan.trim()) }),
      ...(draft.dhcpStart.trim() !== '' && draft.dhcpEnd.trim() !== ''
        ? { dhcp: { start: draft.dhcpStart.trim(), end: draft.dhcpEnd.trim() } }
        : {}),
    };
    onSave(input);
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="md"
      title={
        editing
          ? prefix.source === 'discovered'
            ? `Document ${prefix.cidr}`
            : `Edit ${prefix.cidr}`
          : 'Add a block'
      }
      description={
        editing && prefix.source === 'discovered'
          ? 'This block was discovered from an address seen on a device. Saving it keeps that provenance and makes the record yours: from then on the network is checked against it.'
          : 'A block is the plan. Nothing is sent to any device.'
      }
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" loading={saving} onClick={submit} disabled={vlanInvalid}>
            {editing ? 'Save' : 'Add the block'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field
          label="Block"
          hint="A network and a length, such as 10.20.1.0/24 or 2001:db8:10::/64."
          required
          error={error?.code === 'invalid' || error?.code === 'conflict' ? error.message : undefined}
        >
          <Input
            mono
            value={draft.cidr}
            onChange={(event) => setDraft({ ...draft, cidr: event.target.value })}
            placeholder="10.20.1.0/24"
            disabled={editing}
          />
        </Field>

        {correction && (
          <p className={styles.correction}>
            <Button
              size="sm"
              variant="secondary"
              onClick={() => setDraft({ ...draft, cidr: correction })}
            >
              Use {correction}
            </Button>
            <span>The address you typed has host bits set; this is the network it is in.</span>
          </p>
        )}

        <div className={styles.pair}>
          <Field label="Name">
            <Input
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder="Staff wired"
            />
          </Field>
          <Field label="Site" hint="A label in production. Lab blocks carry their lab’s namespace.">
            <Input
              value={draft.site}
              onChange={(event) => setDraft({ ...draft, site: event.target.value })}
              placeholder="HQ"
              disabled={editing && (prefix.site?.startsWith('lab:') ?? false)}
            />
          </Field>
        </div>

        <div className={styles.pair}>
          <Field label="Role">
            <Select
              value={draft.role}
              onChange={(event) => setDraft({ ...draft, role: event.target.value })}
              options={[
                { value: '', label: 'none' },
                ...ROLES.map((role) => ({ value: role, label: role })),
              ]}
            />
          </Field>
          <Field
            label="VLAN id"
            error={vlanInvalid ? 'A VLAN id is a number from 1 to 4094.' : undefined}
          >
            <Input
              mono
              value={draft.vlan}
              onChange={(event) => setDraft({ ...draft, vlan: event.target.value })}
              placeholder="10"
            />
          </Field>
        </div>

        <div className={styles.pair}>
          <Field
            label="Gateway"
            hint="Checked against what the devices in the block really have."
          >
            <Input
              mono
              value={draft.gateway}
              onChange={(event) => setDraft({ ...draft, gateway: event.target.value })}
              placeholder="10.20.1.1"
            />
          </Field>
          <Field label="DHCP range" hint="Left out of “next free address”.">
            <div className={styles.range}>
              <Input
                mono
                aria-label="DHCP range start"
                value={draft.dhcpStart}
                onChange={(event) => setDraft({ ...draft, dhcpStart: event.target.value })}
                placeholder="10.20.1.100"
              />
              <Input
                mono
                aria-label="DHCP range end"
                value={draft.dhcpEnd}
                onChange={(event) => setDraft({ ...draft, dhcpEnd: event.target.value })}
                placeholder="10.20.1.200"
              />
            </div>
          </Field>
        </div>

        <Field label="Description">
          <Textarea
            rows={2}
            value={draft.description}
            onChange={(event) => setDraft({ ...draft, description: event.target.value })}
          />
        </Field>

        <Field label="Tags">
          <TagInput
            value={draft.tags}
            onChange={(tags) => setDraft({ ...draft, tags })}
            placeholder="users, wired"
          />
        </Field>

        {error && error.code !== 'invalid' && error.code !== 'conflict' && (
          <p className={styles.error}>{error.message}</p>
        )}

        {saved && saved.warnings.length > 0 && (
          <div className={styles.warnings}>
            <p className={styles.warningsTitle}>
              Saved{saved.adopted ? ', and the discovered block was adopted' : ''}. The engine
              warned, rather than refused:
            </p>
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
