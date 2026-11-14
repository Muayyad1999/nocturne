/**
 * Import and export, as CSV — the format a team's existing spreadsheet is in.
 *
 * An import is previewed line by line before anything is written: the engine
 * returns each row as `new`, `update` or `invalid` with the problem in its own
 * words (a prefix with host bits set comes back with the corrected network in
 * it), and the commit sends exactly the same text with `dry_run: false`.
 *
 * An export carries records only, unless "include what only the network knows"
 * is asked for — in which case each such row is labelled as a sighting, so a
 * spreadsheet full of them cannot be mistaken for a plan.
 */

import { Check, Copy, Download, Upload } from 'lucide-react';
import { useState } from 'react';
import type { ExportResult, ImportResult } from '@/api/types/ipam';
import type { RpcError } from '@/api/errors';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { Switch } from '@/design/Switch';
import { Tabs, type TabItem } from '@/design/Tabs';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { copyText } from '@/lib/clipboard';
import { formatCount, plural } from '@/lib/format';
import styles from './Dialogs.module.css';

export type TransferKind = 'prefixes' | 'addresses' | 'vlans';

const KINDS: readonly { value: TransferKind; label: string }[] = [
  { value: 'prefixes', label: 'blocks' },
  { value: 'addresses', label: 'address records' },
  { value: 'vlans', label: 'VLANs' },
];

const TABS: readonly TabItem[] = [
  { id: 'import', label: 'Import', icon: Upload },
  { id: 'export', label: 'Export', icon: Download },
];

export interface TransferDialogProps {
  open: boolean;
  tab: 'import' | 'export';
  onTab: (tab: 'import' | 'export') => void;
  kind: TransferKind;
  onKind: (kind: TransferKind) => void;
  text: string;
  onText: (text: string) => void;
  includeObserved: boolean;
  onIncludeObserved: (include: boolean) => void;
  preview: ImportResult | null;
  importing: boolean;
  importError: RpcError | null;
  exported: ExportResult | undefined;
  exportError: RpcError | null;
  onClose: () => void;
  onImport: (commit: boolean) => void;
}

export function TransferDialog({
  open,
  tab,
  onTab,
  kind,
  onKind,
  text,
  onText,
  includeObserved,
  onIncludeObserved,
  preview,
  importing,
  importError,
  exported,
  exportError,
  onClose,
  onImport,
}: TransferDialogProps) {
  const [copied, setCopied] = useState(false);
  const previewed = preview?.dry_run === true ? preview : null;
  const committed = preview?.dry_run === false ? preview : null;

  const copy = async () => {
    if (!exported) return;
    const ok = await copyText(exported.text);
    if (ok) {
      setCopied(true);
      setTimeout(() => setCopied(false), 1_500);
    } else {
      toast.error('This browser would not let Nocturne write to the clipboard');
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title="Import and export"
      description="CSV, both ways. An import is previewed before anything is written."
      footer={
        tab === 'import' ? (
          <>
            <Button variant="secondary" onClick={onClose}>
              Close
            </Button>
            <Button
              variant="secondary"
              loading={importing}
              disabled={text.trim() === ''}
              onClick={() => onImport(false)}
            >
              Preview
            </Button>
            <Button
              variant="primary"
              loading={importing}
              disabled={previewed === null || previewed.new + previewed.update === 0}
              onClick={() => onImport(true)}
            >
              {previewed ? `Write ${plural(previewed.new + previewed.update, 'row')}` : 'Write the rows'}
            </Button>
          </>
        ) : (
          <>
            <Button variant="secondary" onClick={onClose}>
              Close
            </Button>
            <Button
              variant="primary"
              icon={copied ? Check : Copy}
              disabled={!exported}
              onClick={() => void copy()}
            >
              {copied ? 'Copied' : 'Copy the CSV'}
            </Button>
          </>
        )
      }
    >
      <div className={styles.form}>
        <div className={styles.tabsRow}>
          <Tabs
            id="ipam-transfer"
            tabs={TABS}
            value={tab}
            onChange={(next) => onTab(next as 'import' | 'export')}
            aria-label="Import or export"
          />
        </div>

        <Field label="What">
          <Select
            value={kind}
            onChange={(event) => onKind(event.target.value as TransferKind)}
            options={KINDS}
          />
        </Field>

        {tab === 'import' ? (
          <>
            <Field
              label="CSV"
              hint="The first line is the header. An export of the same kind is a valid import."
            >
              <Textarea
                mono
                rows={8}
                value={text}
                onChange={(event) => onText(event.target.value)}
                placeholder={'cidr,name,site,vlan_id,role\n10.30.0.0/24,Voice,HQ,30,voice'}
              />
            </Field>

            {importError && <p className={styles.error}>{importError.message}</p>}

            {preview && (
              <>
                <div className={styles.counts}>
                  <Badge tone="ok" variant="outline">
                    {formatCount(preview.new)} new
                  </Badge>
                  <Badge tone="info" variant="outline">
                    {formatCount(preview.update)} to update
                  </Badge>
                  <Badge tone={preview.invalid > 0 ? 'crit' : 'idle'} variant="outline">
                    {formatCount(preview.invalid)} invalid
                  </Badge>
                  {committed && (
                    <span>
                      Written: {formatCount(committed.created)} new,{' '}
                      {formatCount(committed.updated)} updated
                    </span>
                  )}
                  {previewed && <span className={styles.quiet}>Nothing has been written yet.</span>}
                </div>

                <div className={styles.scroll}>
                  <table className={styles.table}>
                    <thead>
                      <tr>
                        <th scope="col">Line</th>
                        <th scope="col">Outcome</th>
                        <th scope="col">Row</th>
                        <th scope="col">Problem</th>
                      </tr>
                    </thead>
                    <tbody>
                      {preview.rows.map((row) => (
                        <tr key={row.line} data-invalid={row.status === 'invalid' || undefined}>
                          <td className={styles.mono}>{row.line}</td>
                          <td>{row.status}</td>
                          <td className={styles.mono}>{describe(row.value)}</td>
                          <td>{row.problem ?? ''}</td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </>
            )}
          </>
        ) : (
          <>
            <Switch
              checked={includeObserved}
              onChange={(event) => onIncludeObserved(event.target.checked)}
            >
              Include what only the network knows, each row labelled as a sighting
            </Switch>
            {exportError ? (
              <p className={styles.error}>{exportError.message}</p>
            ) : exported ? (
              <CodeBlock code={exported.text} maxHeight={320} copyable title={`${kind}.csv`} />
            ) : (
              <p className={styles.quiet}>Asking the engine for the CSV…</p>
            )}
          </>
        )}
      </div>
    </Dialog>
  );
}

/** The parsed row of an import preview, as a compact line. */
function describe(value: unknown): string {
  if (typeof value !== 'object' || value === null) return String(value ?? '');
  return Object.entries(value as Record<string, unknown>)
    .filter(([, entry]) => entry !== '' && entry !== null && entry !== undefined)
    .map(([key, entry]) => `${key}=${String(entry)}`)
    .join(' ');
}
