/**
 * Splitting a block: equal children, or VLSM from a list of host counts.
 *
 * The plan is always previewed first (`dry_run: true`) and the preview is the
 * backend's own arithmetic — including the spare addresses each VLSM child
 * ends up with, and the reason any child cannot be created. Only then is the
 * same request sent for real.
 */

import { useEffect, useState } from 'react';
import type { PrefixRow, SplitParams, SplitResult } from '@/api/types/ipam';
import type { RpcError } from '@/api/errors';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { RadioGroup } from '@/design/RadioGroup';
import { Switch } from '@/design/Switch';
import { formatCount } from '@/lib/format';
import styles from './Dialogs.module.css';

export interface SplitDialogProps {
  open: boolean;
  prefix: PrefixRow | null;
  /** The preview, or the result of the real run. */
  result: SplitResult | null;
  pending: boolean;
  error: RpcError | null;
  onClose: () => void;
  onRun: (params: SplitParams) => void;
}

export function SplitDialog({
  open,
  prefix,
  result,
  pending,
  error,
  onClose,
  onRun,
}: SplitDialogProps) {
  const [mode, setMode] = useState<'equal' | 'vlsm'>('equal');
  const [newLen, setNewLen] = useState('');
  const [sizes, setSizes] = useState('');
  const [p2p31, setP2p31] = useState(false);

  useEffect(() => {
    if (!open || !prefix) return;
    const length = Number(prefix.cidr.split('/')[1] ?? '0');
    setMode('equal');
    setNewLen(String(Math.min(length + 1, prefix.cidr.includes(':') ? 128 : 32)));
    setSizes('');
    setP2p31(false);
  }, [open, prefix]);

  const parsedSizes = sizes
    .split(/[,\n]/)
    .map((entry) => entry.trim())
    .filter((entry) => entry !== '')
    .map((entry) => {
      const [name, hosts] = entry.includes(':') ? entry.split(':') : ['', entry];
      return { name: (name ?? '').trim(), hosts: Number((hosts ?? '').trim()) };
    });
  const sizesValid = parsedSizes.length > 0 && parsedSizes.every((size) => size.hosts > 0);
  const lenValid = /^\d{1,3}$/.test(newLen.trim());
  const ready = mode === 'equal' ? lenValid : sizesValid;

  const params = (dryRun: boolean): SplitParams | null => {
    if (!prefix) return null;
    if (mode === 'equal') {
      return { id: prefix.id, new_len: Number(newLen.trim()), dry_run: dryRun };
    }
    return {
      id: prefix.id,
      sizes: parsedSizes.map((size) => (size.name ? { name: size.name, hosts: size.hosts } : size.hosts)),
      p2p31: p2p31,
      dry_run: dryRun,
    };
  };

  const run = (dryRun: boolean) => {
    const next = params(dryRun);
    if (next) onRun(next);
  };

  const preview = result?.dry_run === true ? result : null;
  const created = result?.dry_run === false ? result : null;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title={prefix ? `Split ${prefix.cidr}` : 'Split a block'}
      description="Children are blocks in the plan. Nothing is sent to a device, and nothing is created until the preview is accepted."
      footer={
        <>
          <Button variant="secondary" onClick={onClose}>
            Close
          </Button>
          <Button variant="secondary" loading={pending} disabled={!ready} onClick={() => run(true)}>
            Preview
          </Button>
          <Button
            variant="primary"
            loading={pending}
            disabled={!ready || preview === null}
            onClick={() => run(false)}
          >
            Create {preview ? formatCount(preview.children.length) : ''} blocks
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <RadioGroup
          name="split-mode"
          value={mode}
          onChange={(value) => setMode(value as 'equal' | 'vlsm')}
          aria-label="How to split"
          options={[
            { value: 'equal', label: 'Equal blocks of one length' },
            { value: 'vlsm', label: 'VLSM: a list of host counts' },
          ]}
        />

        {mode === 'equal' ? (
          <Field
            label="New length"
            hint={prefix ? `${prefix.cidr} split into blocks of this prefix length.` : undefined}
            error={lenValid ? undefined : 'A prefix length is a number.'}
          >
            <Input mono value={newLen} onChange={(event) => setNewLen(event.target.value)} />
          </Field>
        ) : (
          <>
            <Field
              label="Host counts"
              hint="One per line, or comma separated. A name is optional: “voice: 100”."
              error={sizesValid ? undefined : 'At least one host count, each greater than zero.'}
            >
              <Input
                mono
                value={sizes}
                onChange={(event) => setSizes(event.target.value)}
                placeholder="staff: 500, voice: 100, 50"
              />
            </Field>
            <Switch checked={p2p31} onChange={(event) => setP2p31(event.target.checked)}>
              Point-to-point links get a /31 rather than a /30
            </Switch>
          </>
        )}

        {error && <p className={styles.error}>{error.message}</p>}

        {result && (
          <>
            <p className={styles.note}>
              {created
                ? `${formatCount(created.created)} of ${formatCount(created.children.length)} blocks were created inside ${created.parent}.`
                : `This is a preview of ${formatCount(result.children.length)} blocks inside ${result.parent}. Nothing has been created.`}
            </p>
            <div className={styles.scroll}>
              <table className={styles.table}>
                <thead>
                  <tr>
                    <th scope="col">Block</th>
                    <th scope="col">Name</th>
                    <th scope="col">Asked for</th>
                    <th scope="col">Usable</th>
                    <th scope="col">Spare</th>
                    <th scope="col">Outcome</th>
                  </tr>
                </thead>
                <tbody>
                  {result.children.map((child) => (
                    <tr key={child.cidr} data-invalid={child.skipped !== null || undefined}>
                      <td className={styles.mono}>{child.cidr}</td>
                      <td>{child.name}</td>
                      <td className={styles.mono}>
                        {child.hosts_requested === null ? '—' : formatCount(child.hosts_requested)}
                      </td>
                      <td className={styles.mono}>{formatCount(child.usable)}</td>
                      <td className={styles.mono}>
                        {child.spare === null ? '—' : formatCount(child.spare)}
                      </td>
                      <td>
                        {child.skipped ??
                          (child.id === null ? 'would be created' : 'created')}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </>
        )}
      </div>
    </Dialog>
  );
}
