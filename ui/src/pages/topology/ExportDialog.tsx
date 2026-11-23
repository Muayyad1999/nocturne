/**
 * `topology.export` in all five formats, with the text on screen before
 * anything is downloaded — and, from the same drawing, a picture and a record.
 *
 * The backend writes the file; this dialog only shows it and saves it. That
 * means what is downloaded is exactly what was read, and a person can check a
 * drawing before pasting it into a change document.
 *
 * Two things are made from the engine's SVG rather than asked for as a
 * format of their own:
 *
 * - **PNG**, for a ticket or a mail. The SVG is decoded as an image and drawn
 *   onto a canvas (`lib/capture`); nothing is stored and nothing is sent.
 * - **Keep as evidence**, which sends no picture at all: it asks the engine
 *   to draw the same graph again and keep its own drawing, redacted, with a
 *   source and a hash (`evidence.capture_topology`). Offered only in a build
 *   that has it.
 *
 * Either way the picture is the engine's drawing of the same graph — the
 * same nodes, links and warnings — not a pixel copy of the screen, and the
 * dialog says so: the layout a person dragged into place, and a path or load
 * overlay, are not in it.
 */

import { Copy, Download, FileCheck2 } from 'lucide-react';
import { useState } from 'react';
import { useRpc } from '@/api/hooks';
import type { ExportParams, Exported, Format } from '@/api/types/topology';
import { useKeepAsEvidence } from '@/app/KeepAsEvidence';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { toast } from '@/design/Toast';
import { copyImageOrSave, savePicture, svgDataUrl, svgToPng, tellImageCopied } from '@/lib/capture';
import { copyText } from '@/lib/clipboard';
import { download, tellSaved } from '@/lib/download';
import { formatBytes } from '@/lib/format';
import styles from './ExportDialog.module.css';

/** What the dialog offers: the engine's formats, and a picture made from its SVG. */
type Choice = Format | 'png';

const FORMATS: readonly { value: Choice; label: string; about: string }[] = [
  { value: 'svg', label: 'SVG', about: 'A standalone drawing with its own legend.' },
  { value: 'png', label: 'PNG image', about: 'A picture of that drawing, for a ticket or a mail.' },
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
  const [format, setFormat] = useState<Choice>('svg');
  const picture = format === 'png';
  const exported = useRpc<Exported>(
    'topology.export',
    // A PNG is made here from the engine's SVG: there is no such format to ask for.
    { ...params, format: picture ? 'svg' : format },
    { enabled: open },
  );
  const keep = useKeepAsEvidence();
  const about = FORMATS.find((entry) => entry.value === format)?.about ?? '';
  const file = exported.data;
  const pngName = file ? file.filename.replace(/\.svg$/i, '') + '.png' : 'topology.png';

  return (
    <>
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
            {keep.can('topology') && (
              <Button
                variant="secondary"
                icon={FileCheck2}
                title="The engine draws this graph again and keeps its own drawing as a record, with a source and a hash."
                onClick={() => keep.keep({ kind: 'topology', ...params })}
              >
                Keep as evidence…
              </Button>
            )}
            {picture ? (
              <Button
                variant="secondary"
                icon={Copy}
                disabled={!file}
                onClick={() => {
                  if (file) void copyImageOrSave(svgToPng(file.text), pngName).then(tellImageCopied);
                }}
              >
                Copy image
              </Button>
            ) : (
              <Button
                variant="secondary"
                icon={Copy}
                disabled={!file}
                onClick={async () => {
                  if (!file) return;
                  // The clipboard can refuse — no secure context, a WebView that
                  // denies it — and "Copied" over an unchanged clipboard is how
                  // yesterday's text ends up pasted into a change document.
                  if (await copyText(file.text)) {
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
            )}
            <Button
              variant="primary"
              icon={Download}
              disabled={!file}
              onClick={() => {
                if (!file) return;
                // Says what became of it: a hand-over the window could not
                // make is an error, not "Saved".
                if (picture) void savePicture(svgToPng(file.text), pngName).then(tellSaved);
                else download(file.filename, file.text);
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
              onChange={(event) => setFormat(event.target.value as Choice)}
            />
          </Field>

          {(picture || format === 'svg') && (
            <p className={styles.note} data-testid="export-not-a-screenshot">
              This picture is the engine’s drawing of the same graph, not a pixel copy of the screen: where
              you dragged things, and a path or load overlay, are not in it.
            </p>
          )}

          {exported.isPending && (
            <div aria-busy="true">
              <Skeleton height={180} shape="block" />
            </div>
          )}
          {exported.isError && <p className={styles.error}>{exported.error.message}</p>}
          {file &&
            (picture ? (
              <>
                <p className={styles.file}>
                  <span className="mono">{pngName}</span> · made here from the engine’s SVG
                </p>
                {/* As an image, like every drawing: nothing in an SVG runs there. */}
                <div className={styles.picture}>
                  <img src={svgDataUrl(file.text)} alt="The drawing that will be saved" draggable={false} />
                </div>
              </>
            ) : (
              <>
                <p className={styles.file}>
                  <span className="mono">{file.filename}</span> · {formatBytes(new Blob([file.text]).size)}
                </p>
                <CodeBlock code={file.text} title={file.filename} maxHeight={320} copyable={false} />
              </>
            ))}
        </div>
      </Dialog>
      {keep.dialog}
    </>
  );
}
