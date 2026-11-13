/**
 * Links a person drew: an unmanaged switch in a cupboard, a WAN circuit, dark
 * fibre between two buildings. `topology.manual.list` / `.save` / `.delete`.
 *
 * These are the only lines on the drawing that no device reported, so they are
 * drawn in their own colour, labelled "drawn by hand" on the canvas, and
 * listed here with who they join — never mixed in with what was discovered.
 */

import { Pencil, Plus, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { Graph, ManualLink, NodeKind } from '@/api/types/topology';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import styles from './ManualLinks.module.css';

const NON_DEVICE_KINDS: readonly { value: NodeKind; label: string }[] = [
  { value: 'cloud', label: 'External (a circuit, the internet)' },
  { value: 'switch', label: 'Switch' },
  { value: 'router', label: 'Router' },
  { value: 'firewall', label: 'Firewall' },
  { value: 'access_point', label: 'Access point' },
  { value: 'server', label: 'Server' },
  { value: 'host', label: 'Host' },
  { value: 'unknown', label: 'Something else' },
];

interface Draft {
  id?: string;
  aDevice: string;
  aInterface: string;
  bDevice: string;
  bName: string;
  bKind: NodeKind;
  label: string;
}

const EMPTY_DRAFT: Draft = {
  aDevice: '',
  aInterface: '',
  bDevice: '',
  bName: '',
  bKind: 'cloud',
  label: '',
};

function draftOf(link: ManualLink): Draft {
  return {
    ...(link.id ? { id: link.id } : {}),
    aDevice: link.a.device ?? link.a.name ?? '',
    aInterface: link.a.interface ?? '',
    bDevice: link.b.device ?? '',
    bName: link.b.name ?? '',
    bKind: link.b.kind ?? 'cloud',
    label: link.label ?? '',
  };
}

export interface ManualLinksProps {
  open: boolean;
  onClose: () => void;
  graph: Graph | undefined;
}

const SUGGESTIONS = 'topology-manual-devices';

export function ManualLinks({ open, onClose, graph }: ManualLinksProps) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const links = useRpc<ManualLink[]>('topology.manual.list', undefined, {
    enabled: open,
    invalidateOn: ['topology.changed'],
  });
  const save = useRpcMutation<ManualLink, ManualLink>('topology.manual.save', {
    invalidates: ['topology.'],
    successToast: 'The link was saved and is drawn as one a person added',
  });
  const remove = useRpcMutation<{ deleted: boolean }, { id: string }>('topology.manual.delete', {
    invalidates: ['topology.'],
  });

  const names = [...new Set((graph?.nodes ?? []).map((node) => node.name))].sort((one, other) =>
    one.localeCompare(other),
  );

  const nameOf = (end: { device?: string | null; name?: string | null }): string => {
    if (end.name) return end.name;
    if (!end.device) return 'unnamed';
    const node = graph?.nodes.find(
      (candidate) => candidate.device_id === end.device || candidate.id === end.device,
    );
    return node?.name ?? end.device;
  };

  const submit = () => {
    if (!draft) return;
    const bIsDevice = draft.bDevice.trim() !== '';
    save.mutate(
      {
        ...(draft.id ? { id: draft.id } : {}),
        a: {
          device: draft.aDevice.trim(),
          interface: draft.aInterface.trim() === '' ? null : draft.aInterface.trim(),
        },
        b: bIsDevice
          ? { device: draft.bDevice.trim() }
          : { name: draft.bName.trim(), kind: draft.bKind },
        label: draft.label.trim() === '' ? null : draft.label.trim(),
      },
      { onSuccess: () => setDraft(null) },
    );
  };

  return (
    <Drawer
      open={open}
      onClose={onClose}
      title="Links drawn by hand"
      description="What discovery cannot see. These are drawn in their own colour and labelled on the canvas."
      size="md"
    >
      {!draft && (
        <>
          <div className={styles.top}>
            <Button size="sm" variant="primary" icon={Plus} onClick={() => setDraft(EMPTY_DRAFT)}>
              Draw a link
            </Button>
          </div>

          {links.isPending && (
            <div aria-busy="true" className={styles.loading}>
              <Skeleton />
              <Skeleton width="70%" />
            </div>
          )}
          {links.isError && <p className={styles.error}>{links.error.message}</p>}
          {links.data?.length === 0 && (
            <EmptyState
              icon={Pencil}
              title="Nothing has been drawn by hand"
              description="Add a link for something no device reports: an unmanaged switch, a WAN circuit, a fibre between two buildings."
              action={
                <Button size="sm" icon={Plus} onClick={() => setDraft(EMPTY_DRAFT)}>
                  Draw a link
                </Button>
              }
            />
          )}

          <ul className={styles.list}>
            {(links.data ?? []).map((link) => (
              <li key={link.id ?? `${nameOf(link.a)}-${nameOf(link.b)}`}>
                <div className={styles.ends}>
                  <b>{nameOf(link.a)}</b>
                  {link.a.interface && <span className="mono">{link.a.interface}</span>}
                  <span className={styles.joiner}>↔</span>
                  <b>{nameOf(link.b)}</b>
                  {link.b.interface && <span className="mono">{link.b.interface}</span>}
                </div>
                {link.label && <p className={styles.label}>{link.label}</p>}
                <div className={styles.rowActions}>
                  <Button size="sm" variant="ghost" icon={Pencil} onClick={() => setDraft(draftOf(link))}>
                    Edit
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={Trash2}
                    onClick={async () => {
                      if (!link.id) return;
                      const sure = await confirm({
                        title: 'Delete this drawn link?',
                        description: `The line between ${nameOf(link.a)} and ${nameOf(
                          link.b,
                        )} will no longer be drawn. No device is touched.`,
                        confirmLabel: 'Delete',
                        tone: 'danger',
                      });
                      if (sure) remove.mutate({ id: link.id });
                    }}
                  >
                    Delete
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        </>
      )}

      {draft && (
        <form
          className={styles.form}
          onSubmit={(event) => {
            event.preventDefault();
            submit();
          }}
        >
          <datalist id={SUGGESTIONS}>
            {names.map((name) => (
              <option key={name} value={name} />
            ))}
          </datalist>

          <h3>This end</h3>
          <Field label="Device" hint="A device id, name or address. It must be in the inventory.">
            <Input
              value={draft.aDevice}
              list={SUGGESTIONS}
              onChange={(event) => setDraft({ ...draft, aDevice: event.target.value })}
            />
          </Field>
          <Field label="Interface" hint="Optional: the port the cable is in.">
            <Input
              value={draft.aInterface}
              onChange={(event) => setDraft({ ...draft, aInterface: event.target.value })}
            />
          </Field>

          <h3>The other end</h3>
          <Field
            label="Device"
            hint="Leave empty if the other end is not a device Nocturne manages."
          >
            <Input
              value={draft.bDevice}
              list={SUGGESTIONS}
              onChange={(event) => setDraft({ ...draft, bDevice: event.target.value })}
            />
          </Field>
          {draft.bDevice.trim() === '' && (
            <>
              <Field label="Call it" hint="What to write on the node.">
                <Input
                  value={draft.bName}
                  onChange={(event) => setDraft({ ...draft, bName: event.target.value })}
                />
              </Field>
              <Field label="Draw it as">
                <Select
                  value={draft.bKind}
                  options={NON_DEVICE_KINDS}
                  onChange={(event) => setDraft({ ...draft, bKind: event.target.value as NodeKind })}
                />
              </Field>
            </>
          )}

          <Field label="Label" hint="Optional: what this link is — “100 Mb/s fibre to Annex”.">
            <Input
              value={draft.label}
              onChange={(event) => setDraft({ ...draft, label: event.target.value })}
            />
          </Field>

          {save.isError && <p className={styles.error}>{save.error.message}</p>}

          <div className={styles.formActions}>
            <Button variant="ghost" onClick={() => setDraft(null)}>
              Cancel
            </Button>
            <Button
              type="submit"
              variant="primary"
              loading={save.isPending}
              disabled={
                draft.aDevice.trim() === '' ||
                (draft.bDevice.trim() === '' && draft.bName.trim() === '')
              }
            >
              {draft.id ? 'Save changes' : 'Draw it'}
            </Button>
          </div>
        </form>
      )}
    </Drawer>
  );
}
