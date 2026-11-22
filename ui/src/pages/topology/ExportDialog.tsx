/**
 * `topology.export` in all five formats, with the text on screen before
 * anything is downloaded.
 *
 * The backend writes the file; this dialog only shows it and saves it. That
 * means what is downloaded is exactly what was read, and a person can check a
 * drawing before pasting it into a change document.
 */

import { Copy, Download } from 'lucide-react';
import { useState } from 'react';
import { useRpc } from '@/api/hooks';
import type { ExportParams, Exported, Format } from '@/api/types/topology';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import { copyText } from '@/lib/clipboard';
import { formatBytes } from '@/lib/format';
import styles from './ExportDialog.module.css';

const FORMATS: readonly { value: Format; label: string; about: string }[] = [
  { value: 'svg', label: 'SVG', about: 'A standalone drawing with its own legend.' },
  { value: 'drawio', label: 'draw.io', about: 'Opens in diagrams.net with editable shapes.' },
  { value: 'mermaid', label: 'Mermaid', about: 'A graph for a Markdown document.' },
  { value: 'dot', label: 'Graphviz DOT', about: 'For anything that reads DOT.' },
  { value: 'json', label: 'JSON', about: 'The graph itself: nodes, links, groups, warnings.' },
];

export interface ExportDialogProps {
  open: boolean;
  onClose: () => void;
  /** Exactly what the screen is showing, so the file matches the drawing. */
  params: Omit<ExportParams, 'format'>;
}

export function ExportDialog({ open, onClose, params }: ExportDialogProps) {
  const [format, setFormat] = useState<Format>('svg');
  const exported = useRpc<Exported>(
    'topology.export',
    { ...params, format },
    { enabled: open },
  );
  const about = FORMATS.find((entry) => entry.value === format)?.about ?? '';

  const download = () => {
    const file = exported.data;
    if (!file) return;
    // A blob and a click: no backend round trip, and what is saved is what is shown.
    const blob = new Blob([file.text], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = file.filename;
    anchor.click();
    URL.revokeObjectURL(url);
    toast.success(`Saved ${file.filename}`);
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Export this drawing"
      description="The same nodes, links and warnings that are on screen."
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
          <Button
            variant="secondary"
            icon={Copy}
            disabled={!exported.data}
            onClick={async () => {
              if (!exported.data) return;
              // The clipboard can refuse — no secure context, a WebView that
              // denies it — and "Copied" over an unchanged clipboard is how
              // yesterday's text ends up pasted into a change document.
              if (await copyText(exported.data.text)) {
                toast.success('Copied');
              } else {
                toast.error('Could not copy to the clipboard', {
                  description:
                    'Nothing was copied. Download the file, or select the text below and copy it with the keyboard.',
                });
              }
            }}
          >
            Copy
          </Button>
          <Button variant="primary" icon={Download} disabled={!exported.data} onClick={download}>
            Download
          </Button>
        </>
      }
    >
      <div className={styles.body}>
        <Field label="Format" hint={about}>
          <Select
            value={format}
            options={FORMATS.map((entry) => ({ value: entry.value, label: entry.label }))}
            onChange={(event) => setFormat(event.target.value as Format)}
          />
        </Field>

        {exported.isPending && (
          <div aria-busy="true">
            <Skeleton height={180} shape="block" />
          </div>
        )}
        {exported.isError && <p className={styles.error}>{exported.error.message}</p>}
        {exported.data && (
          <>
            <p className={styles.file}>
              <span className="mono">{exported.data.filename}</span> ·{' '}
              {formatBytes(new Blob([exported.data.text]).size)}
            </p>
            <CodeBlock
              code={exported.data.text}
              title={exported.data.filename}
              maxHeight={320}
              copyable={false}
            />
          </>
        )}
      </div>
    </Dialog>
  );
}
