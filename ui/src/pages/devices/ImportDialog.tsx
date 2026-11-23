/**
 * Import devices from what a team already has: a CSV sheet, an OpenSSH client
 * config, or an Ansible inventory. Paste it or drop the file in, see exactly
 * what will happen to each row, then commit.
 *
 * Nothing is written until **Import** is pressed: `devices.import_preview`
 * parses and classifies, `devices.import` applies.
 */

import { FileUp, Upload } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { ImportParams, ImportPreview, ImportResult, ImportRow } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { Dialog } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { Table, type TableColumn } from '@/design/Table';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import { plural } from '@/lib/format';
import styles from './ImportDialog.module.css';

const FORMATS = [
  { value: 'auto', label: 'Detect automatically' },
  { value: 'csv', label: 'CSV' },
  { value: 'ssh_config', label: 'OpenSSH client config' },
  { value: 'ansible', label: 'Ansible inventory' },
];

const FORMAT_NAME: Record<string, string> = {
  csv: 'CSV',
  ssh_config: 'an OpenSSH client config',
  ansible: 'an Ansible inventory',
};

/** How long the text must be still before it is sent for a preview. */
const DEBOUNCE_MS = 300;

const STATUS_TONE = { new: 'ok', update: 'info', invalid: 'crit' } as const;
const STATUS_WORD = { new: 'New', update: 'Update', invalid: 'Skipped' } as const;

const rowName = (row: ImportRow) => row.input.name ?? row.input.address ?? '';

/** The preview rows need a stable key, and an import may repeat a name. */
interface PreviewRow extends ImportRow {
  key: string;
}

const COLUMNS: TableColumn<PreviewRow>[] = [
  {
    id: 'status',
    header: 'Will',
    width: 86,
    required: true,
    sortValue: (row) => row.status,
    cell: (row) => (
      <Badge tone={STATUS_TONE[row.status] ?? 'neutral'} variant="outline">
        {STATUS_WORD[row.status] ?? row.status}
      </Badge>
    ),
  },
  {
    id: 'name',
    header: 'Name',
    required: true,
    sortValue: rowName,
    cell: (row) => <span className={styles.cell}>{rowName(row) || '—'}</span>,
  },
  {
    id: 'address',
    header: 'Address',
    width: 140,
    mono: true,
    sortValue: (row) => row.input.address ?? '',
    cell: (row) => row.input.address ?? '—',
  },
  {
    id: 'profile',
    header: 'Device type',
    width: 140,
    mono: true,
    sortValue: (row) => row.input.profile ?? '',
    cell: (row) => row.input.profile || <span className={styles.muted}>detect</span>,
  },
  {
    id: 'problem',
    header: 'Why not',
    width: 210,
    cell: (row) => <span className={styles.cell}>{row.problem ?? ''}</span>,
  },
];

export interface ImportDialogProps {
  open: boolean;
  onClose: () => void;
}

export function ImportDialog({ open, onClose }: ImportDialogProps) {
  const [text, setText] = useState('');
  const [debounced, setDebounced] = useState('');
  const [format, setFormat] = useState('auto');
  const [updateExisting, setUpdateExisting] = useState(true);
  const [over, setOver] = useState(false);
  const [result, setResult] = useState<ImportResult | null>(null);
  const fileRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setText('');
    setDebounced('');
    setFormat('auto');
    setUpdateExisting(true);
    setResult(null);
  }, [open]);

  useEffect(() => {
    const timer = setTimeout(() => setDebounced(text), DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [text]);

  const params: ImportParams = { text: debounced, format };
  const preview = useRpc<ImportPreview>('devices.import_preview', params, {
    enabled: open && debounced.trim() !== '',
  });

  const commit = useRpcMutation<ImportResult, ImportParams>('devices.import', {
    invalidates: ['devices.'],
    errorToast: 'The import failed',
  });

  const readFile = async (file: File) => {
    setText(await file.text());
  };

  const onImport = async () => {
    const outcome = await commit.mutateAsync({ text, format, update_existing: updateExisting });
    setResult(outcome);
    const total = outcome.created + outcome.updated;
    if (total > 0) {
      toast.success(
        `Imported ${plural(total, 'device')}`,
        outcome.skipped.length > 0
          ? { description: `${plural(outcome.skipped.length, 'row')} skipped.` }
          : undefined,
      );
    }
    if (outcome.skipped.length === 0) onClose();
  };

  const data = preview.data;
  const willImport = data ? data.new + (updateExisting ? data.update : 0) : 0;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      size="lg"
      title="Import devices"
      description="Paste a CSV sheet, an OpenSSH client config or an Ansible inventory — or drop the file here. Nothing is written until you import."
      dismissible={!commit.isPending}
      footer={
        <>
          <Button onClick={onClose} disabled={commit.isPending}>
            {result ? 'Close' : 'Cancel'}
          </Button>
          <Button
            variant="primary"
            icon={Upload}
            loading={commit.isPending}
            disabled={willImport === 0}
            onClick={() => void onImport()}
          >
            {willImport === 0
              ? 'Import'
              : `Import ${plural(willImport, 'device')}`}
          </Button>
        </>
      }
    >
      <div className={styles.body}>
        {/*
          While something is dragged over the zone the file input lies over
          all of it (`.drop[data-over='true'] .file`), so the drop lands on
          the input and the browser does with it what choosing a file does.
          That is not decoration: WebKitGTK — the Linux application — never
          lets a page read a dropped file from the drop itself, only through
          a file input it was dropped on.
        */}
        <div
          className={styles.drop}
          data-over={over}
          onDragOver={(event) => {
            event.preventDefault();
            setOver(true);
          }}
          onDragLeave={(event) => {
            // Under the input there is nothing else to be over, so leaving it
            // is leaving the zone — or the drag being given up where it was,
            // which says nothing else. Leaving anything else is leaving the
            // zone only when the pointer says so: the input coming up under
            // the pointer is itself a "leave" of what was there before.
            const box = event.currentTarget.getBoundingClientRect();
            const outside =
              event.clientX <= box.left ||
              event.clientX >= box.right ||
              event.clientY <= box.top ||
              event.clientY >= box.bottom;
            if (event.target === fileRef.current || outside) setOver(false);
          }}
          onDrop={(event) => {
            setOver(false);
            // Left to the browser, which gives the input the file; the
            // input's own `change` reads it.
            if (event.target === fileRef.current) return;
            event.preventDefault();
            const file = event.dataTransfer.files[0];
            if (file) void readFile(file);
          }}
          // No mouse event reaches a page during a drag, so one that does
          // means the drag is over, however it ended. Without this a drag
          // that ended in a way nobody was told about would leave an input
          // nobody can see lying over the text box, taking its clicks.
          onMouseMove={() => {
            if (over) setOver(false);
          }}
        >
          <div className={styles.dropHint}>
            <span>Paste here, or drop a file.</span>
            <span className={styles.spacer} />
            <Button size="sm" icon={FileUp} onClick={() => fileRef.current?.click()}>
              Choose a file
            </Button>
            <input
              ref={fileRef}
              className={styles.file}
              type="file"
              accept=".csv,.txt,.yml,.yaml,.ini,.cfg,text/*"
              tabIndex={-1}
              aria-hidden="true"
              // The pointer rests on the input when a drop ends, and
              // WebKitGTK then shows the input's tooltip until it moves:
              // without a title that is the browser's own "no file
              // selected", right after one was. An empty title does not
              // stop it, so it is given something true to say.
              title="A file to import devices from"
              onChange={(event) => {
                const file = event.target.files?.[0];
                if (file) void readFile(file);
                event.target.value = '';
              }}
            />
          </div>
          <Textarea
            mono
            rows={6}
            aria-label="Devices to import"
            placeholder={'name,address,profile,site,tags\ncore-sw1,10.0.0.1,cisco_ios,HQ,core;l3'}
            value={text}
            onChange={(event) => setText(event.target.value)}
          />
        </div>

        <Field label="Format" hint="Detected from the text unless you choose one.">
          <Select
            size="sm"
            value={format}
            options={FORMATS}
            onChange={(event) => setFormat(event.target.value)}
          />
        </Field>

        {preview.isError && (
          <ErrorState
            size="compact"
            title="That could not be read"
            error={preview.error}
            onRetry={() => void preview.refetch()}
          />
        )}

        {data && (
          <>
            <div className={styles.summary}>
              <Badge tone="ok">{formatCount(data.new)} new</Badge>
              <Badge tone="info">{formatCount(data.update)} already here</Badge>
              {data.invalid > 0 && <Badge tone="crit">{formatCount(data.invalid)} unusable</Badge>}
              <span className={styles.summaryText}>
                Read as {FORMAT_NAME[data.format] ?? data.format}.
              </span>
              <span className={styles.spacer} />
              <Checkbox
                checked={updateExisting}
                disabled={data.update === 0}
                onChange={(event) => setUpdateExisting(event.target.checked)}
              >
                Update the devices that already exist
              </Checkbox>
            </div>

            <Table
              className={styles.preview}
              aria-label="Devices to import"
              columns={COLUMNS}
              rows={data.rows.map((row, index) => ({ ...row, key: `${index}` }))}
              rowKey={(row) => row.key}
              maxHeight={320}
              empty={
                <EmptyState
                  size="compact"
                  icon={FileUp}
                  title="Nothing to import"
                  description="The text was read, but it holds no devices."
                />
              }
            />
          </>
        )}

        {result && result.skipped.length > 0 && (
          <ul className={styles.skipped}>
            <li>
              <strong>
                {formatCount(result.created)} added, {formatCount(result.updated)} updated,{' '}
                {formatCount(result.skipped.length)} skipped:
              </strong>
            </li>
            {result.skipped.map((reason) => (
              <li key={reason}>{reason}</li>
            ))}
          </ul>
        )}
      </div>
    </Dialog>
  );
}
