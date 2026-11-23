/**
 * Export the inventory as a CSV sheet or an Ansible inventory, with the text
 * on screen before it leaves: no credential ever goes into it, and lab
 * devices are left out, which is easier to believe when you can read it.
 */

import { Download } from 'lucide-react';
import { useState } from 'react';
import { useRpc } from '@/api/hooks';
import type { ExportParams, ExportResult } from '@/api/types/core';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Dialog } from '@/design/Dialog';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { RadioGroup } from '@/design/RadioGroup';
import { Skeleton } from '@/design/Skeleton';
import { download as saveAs } from '@/lib/download';
import { formatCount } from '@/lib/format';
import styles from './ImportDialog.module.css';

const FORMATS = [
  {
    value: 'csv',
    label: 'CSV',
    description: 'One row per device. Can be imported again as it is.',
  },
  {
    value: 'ansible',
    label: 'Ansible inventory',
    description: 'YAML, with a group per site and per role. Passwords are never written.',
  },
];

const FILENAME: Record<string, string> = { csv: 'devices.csv', ansible: 'inventory.yml' };

export interface ExportDialogProps {
  open: boolean;
  onClose: () => void;
  /** Export only these devices; empty exports the whole inventory. */
  selection: readonly string[];
}

export function ExportDialog({ open, onClose, selection }: ExportDialogProps) {
  const [format, setFormat] = useState('csv');
  const [scope, setScope] = useState<'all' | 'selected'>(selection.length > 0 ? 'selected' : 'all');

  // The list mounts this dialog once, before anything is ticked, and only
  // opens and closes it: the default is the selection as it is when it opens.
  const [wasOpen, setWasOpen] = useState(open);
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setScope(selection.length > 0 ? 'selected' : 'all');
  }

  const ids = scope === 'selected' && selection.length > 0 ? [...selection] : undefined;
  const params: ExportParams = { format, ...(ids ? { ids } : {}) };
  const exported = useRpc<ExportResult>('devices.export', params, { enabled: open });

  const text = exported.data?.text ?? '';

  // Says what became of it: a download the window could not hand over is an
  // error, not "Saved".
  const download = () => saveAs(FILENAME[format] ?? 'devices.txt', text);

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title="Export devices"
      description="Lab devices and every secret are left out."
      footer={
        <>
          <Button onClick={onClose}>Close</Button>
          <Button variant="primary" icon={Download} disabled={text === ''} onClick={download}>
            Download
          </Button>
        </>
      }
    >
      <div className={styles.body}>
        <Field label="Format">
          <RadioGroup
            name="export-format"
            value={format}
            options={FORMATS}
            onChange={setFormat}
          />
        </Field>

        {selection.length > 0 && (
          <Field label="Which devices">
            <RadioGroup
              name="export-scope"
              value={scope}
              orientation="horizontal"
              options={[
                { value: 'selected', label: `The ${formatCount(selection.length)} selected` },
                { value: 'all', label: 'Every device' },
              ]}
              onChange={(value) => setScope(value as 'all' | 'selected')}
            />
          </Field>
        )}

        {exported.isError ? (
          <ErrorState
            size="compact"
            title="The export could not be made"
            error={exported.error}
            onRetry={() => void exported.refetch()}
          />
        ) : exported.isPending ? (
          <Skeleton height={220} shape="block" />
        ) : (
          <CodeBlock
            code={text || '# No device matched: nothing to export.'}
            title={FILENAME[format]}
            maxHeight={320}
            copyable
          />
        )}
      </div>
    </Dialog>
  );
}
