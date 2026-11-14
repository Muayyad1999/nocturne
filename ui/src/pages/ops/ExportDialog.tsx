/**
 * A document the backend rendered, on screen before it is saved.
 *
 * The incident review and the shift handover are both produced by the engine
 * (`incidents.export`, `handover.export`) in Markdown, CSV, HTML or JSON, so
 * this dialog renders nothing itself: it shows exactly the text that will be
 * written to the file, which is the only way a person can check a document
 * before pasting it into a ticket.
 *
 * HTML is shown in a sandboxed frame rather than as source, because the point
 * of the HTML form is that it prints — and a self-contained document with no
 * scripts is exactly what is safe to put in a frame with scripting off.
 */

import { Copy, Download, Printer } from 'lucide-react';
import { useRef, useState } from 'react';
import type { Format } from '@/api/types/ops';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import { copyText } from '@/lib/clipboard';
import { formatBytes, plural } from '@/lib/format';
import { download, useRendered, type ExportMethod } from './data';
import styles from './Export.module.css';

const FORMATS: readonly { value: Format; label: string; about: string }[] = [
  { value: 'markdown', label: 'Markdown', about: 'For a ticket, a wiki or a pull request.' },
  { value: 'html', label: 'HTML', about: 'Self-contained, with its own styling. Prints well.' },
  { value: 'csv', label: 'CSV', about: 'The tables only, for a spreadsheet.' },
  { value: 'json', label: 'JSON', about: 'The document’s structure, with raw timestamps.' },
];

export interface ExportDialogProps {
  open: boolean;
  onClose: () => void;
  title: string;
  description: string;
  method: ExportMethod;
  /** Everything but the format: the id, or the period. */
  params: Record<string, unknown>;
}

export function ExportDialog({ open, onClose, title, description, method, params }: ExportDialogProps) {
  const [format, setFormat] = useState<Format>('markdown');
  const frame = useRef<HTMLIFrameElement>(null);
  const rendered = useRendered(method, open ? { ...params, format } : null);
  const about = FORMATS.find((entry) => entry.value === format)?.about ?? '';
  const document_ = rendered.data;

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={title}
      description={description}
      size="lg"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
          {format === 'html' && (
            <Button
              variant="secondary"
              icon={Printer}
              disabled={!document_}
              onClick={() => frame.current?.contentWindow?.print()}
            >
              Print
            </Button>
          )}
          <Button
            variant="secondary"
            icon={Copy}
            disabled={!document_}
            onClick={async () => {
              if (!document_) return;
              if (await copyText(document_.content)) toast.success('Copied');
              else toast.error('This browser refused the clipboard');
            }}
          >
            Copy
          </Button>
          <Button
            variant="primary"
            icon={Download}
            disabled={!document_}
            onClick={() => {
              if (!document_) return;
              download(document_.filename, document_.content);
              toast.success(`Saved ${document_.filename}`);
            }}
          >
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

        {rendered.isPending && (
          <div aria-busy="true">
            <Skeleton height={220} shape="block" />
          </div>
        )}
        {rendered.isError && <p className={styles.error}>{rendered.error.message}</p>}
        {document_ && (
          <>
            <p className={styles.file}>
              <span className="mono">{document_.filename}</span> ·{' '}
              {formatBytes(new Blob([document_.content]).size)} · {plural(document_.rows, 'data row')}
            </p>
            {format === 'html' ? (
              <iframe
                ref={frame}
                className={styles.frame}
                title={`${document_.title} (preview)`}
                sandbox=""
                srcDoc={document_.content}
              />
            ) : (
              <CodeBlock
                code={document_.content}
                title={document_.filename}
                maxHeight={340}
                copyable={false}
              />
            )}
          </>
        )}
      </div>
    </Dialog>
  );
}
