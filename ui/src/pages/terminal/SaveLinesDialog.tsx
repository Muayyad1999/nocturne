/**
 * "Save the last lines…": the end of a session as a text file.
 *
 * What is written is the engine's redacted read of the session
 * (`terminal.snapshot`), not what xterm holds: escape sequences resolved and
 * device secrets replaced. The dialog shows that text first and saves
 * exactly what it shows — it does not read again when Save is pressed, so
 * the file cannot differ from what was looked at.
 */

import { Download } from 'lucide-react';
import { useEffect, useState } from 'react';
import { rpc } from '@/api/client';
import type { TerminalSnapshotResult } from '@/api/types/terminal';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { download } from '@/lib/download';
import { plural } from '@/lib/format';
import { asFile, DEFAULT_LINES, readsAsAsked, snapshotFilename, wholeLines } from './capture';
import styles from './SaveLinesDialog.module.css';

/** How long the number has to stand still before the session is read. */
const READ_AFTER_MS = 250;

export interface SaveLinesDialogProps {
  /** The session to save from, or null when the dialog is closed. */
  session: { id: string; title: string } | null;
  onClose: () => void;
}

type Read =
  | { state: 'reading' }
  | { state: 'read'; lines: number; snapshot: TerminalSnapshotResult }
  | { state: 'failed'; why: string };

export function SaveLinesDialog({ session, onClose }: SaveLinesDialogProps) {
  if (!session) return null;
  // Keyed by the session, so a second one starts from the default again.
  return <Form key={session.id} session={session} onClose={onClose} />;
}

function Form({ session, onClose }: { session: { id: string; title: string }; onClose: () => void }) {
  const [linesText, setLinesText] = useState(String(DEFAULT_LINES));
  const [read, setRead] = useState<Read>({ state: 'reading' });
  const typed = wholeLines(linesText);
  // Only a number the engine reads as asked is asked for: see `readsAsAsked`.
  const lines = typed !== null && readsAsAsked(typed) ? typed : null;

  useEffect(() => {
    if (lines === null) return;
    let current = true;
    setRead({ state: 'reading' });
    const timer = setTimeout(() => {
      rpc<TerminalSnapshotResult>('terminal.snapshot', { session_id: session.id, lines }).then(
        (snapshot) => {
          if (current) setRead({ state: 'read', lines, snapshot });
        },
        (error: unknown) => {
          if (current) {
            setRead({ state: 'failed', why: error instanceof Error ? error.message : 'The session could not be read.' });
          }
        },
      );
    }, READ_AFTER_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [session.id, lines]);

  const ready = lines !== null && read.state === 'read' && read.snapshot.text !== '' ? read : null;

  return (
    <Dialog
      open
      onClose={onClose}
      size="lg"
      title={`Save the last lines of ${session.title}`}
      description="A text file of what the session printed, as the engine reads it back: secrets replaced, escape sequences resolved."
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            icon={Download}
            disabled={ready === null}
            onClick={() => {
              if (!ready) return;
              // What is on screen, not a second read: the file is what was looked at.
              if (download(snapshotFilename(session.title, ready.snapshot.lines), asFile(ready.snapshot.text))) onClose();
            }}
          >
            Save as a file
          </Button>
        </>
      }
    >
      <div className={styles.body}>
        <Field
          label="Lines from the end"
          hint="1 to 10,000. A session that was logged can be read in full from its log."
          error={typed === null ? 'A whole number of lines.' : lines === null ? 'Between 1 and 10,000 lines.' : undefined}
        >
          <Input
            data-autofocus
            size="sm"
            inputMode="numeric"
            value={linesText}
            onChange={(event) => setLinesText(event.target.value)}
          />
        </Field>

        <section className={styles.preview} aria-label="What will be written" data-testid="save-preview">
          <h3 className={styles.previewTitle}>What will be written</h3>
          {lines === null ? null : read.state === 'reading' ? (
            <p className={styles.note} role="status">
              Reading the session…
            </p>
          ) : read.state === 'failed' ? (
            <p className={styles.refused} role="alert">
              <b>The session could not be read.</b> {read.why}
            </p>
          ) : read.snapshot.text === '' ? (
            <p className={styles.note}>The session has printed nothing yet.</p>
          ) : (
            <>
              <CodeBlock code={read.snapshot.text} copyable={false} maxHeight={320} />
              <p className={styles.note}>
                {plural(read.snapshot.lines, 'line')}
                {read.snapshot.lines < read.lines ? ` — all the session holds, though ${read.lines} were asked for` : ''}.{' '}
                {read.snapshot.redacted
                  ? 'Secrets in them are replaced.'
                  : 'The engine found nothing to replace in them.'}
              </p>
            </>
          )}
        </section>
      </div>
    </Dialog>
  );
}
