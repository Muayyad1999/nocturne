/**
 * Bringing a document of snippets in.
 *
 * Always in two steps, and the first writes nothing: `dry_run` answers
 * exactly what the import would do — how many would be added, replaced,
 * brought in under a new name or left alone, and every entry that cannot be
 * imported with the engine's reason — and only then can it be applied. An
 * import is the whole document or none of it, so a document with one bad
 * entry is shown with that entry named and no button to press.
 *
 * The document is hostile input as far as this screen is concerned: it is
 * handed to the engine as the text that was read, never parsed or shown as
 * anything but text here.
 */

import { ArrowLeft, FileUp } from 'lucide-react';
import { useRef, useState } from 'react';
import { rpc } from '@/api/client';
import type { SnippetImportConflict, SnippetImportResult } from '@/api/types/terminal';
import { Button } from '@/design/Button';
import { Field } from '@/design/Field';
import { KeyValue } from '@/design/KeyValue';
import { RadioGroup } from '@/design/RadioGroup';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import styles from './SnippetsDrawer.module.css';
import { importSummary, mayApply } from './snippets';
import { useFocusWhenShown } from './viewFocus';

const MODES: { value: SnippetImportConflict; label: string; description: string }[] = [
  { value: 'skip', label: 'Keep the one here', description: 'An entry whose name and device type a snippet here already has is left out.' },
  { value: 'replace', label: 'Replace the one here', description: 'The snippet here takes the entry’s text and tags.' },
  { value: 'rename', label: 'Keep both', description: 'The entry comes in beside it as “name (2)”.' },
];

export interface SnippetImportProps {
  onBack: () => void;
  /** The import was applied: the list has changed. */
  onImported: () => void;
}

export function SnippetImport({ onBack, onImported }: SnippetImportProps) {
  const fileRef = useRef<HTMLInputElement>(null);
  const [text, setText] = useState('');
  const [fileName, setFileName] = useState<string | null>(null);
  const [mode, setMode] = useState<SnippetImportConflict>('skip');
  /** The last answer, and what it was an answer about. */
  const [answer, setAnswer] = useState<{ result: SnippetImportResult; text: string; mode: SnippetImportConflict } | null>(
    null,
  );
  const [failure, setFailure] = useState<string | null>(null);
  const [pending, setPending] = useState<'check' | 'apply' | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  useFocusWhenShown(rootRef);

  // A preview is about one document and one mode. Change either and it is
  // about something else: it goes, and so does the button it allowed.
  const current = answer && answer.text === text && answer.mode === mode ? answer.result : null;
  const preview = current?.dry_run ? current : null;
  const applied = current && !current.dry_run ? current : null;

  const run = async (dryRun: boolean) => {
    setPending(dryRun ? 'check' : 'apply');
    setFailure(null);
    try {
      const result = await rpc<SnippetImportResult>('terminal.snippets.import', {
        document: text,
        on_conflict: mode,
        dry_run: dryRun,
      });
      setAnswer({ result, text, mode });
      if (!dryRun && result.applied) {
        toast.success('Snippets imported', { description: importSummary(result) });
        onImported();
      }
    } catch (error) {
      setAnswer(null);
      setFailure(error instanceof Error ? error.message : 'The document could not be read.');
    } finally {
      setPending(null);
    }
  };

  const shown = applied ?? preview;

  return (
    <div ref={rootRef} className={styles.use} data-testid="snippet-import">
      <div className={styles.useHead}>
        <Button size="sm" variant="ghost" icon={ArrowLeft} onClick={onBack}>
          All snippets
        </Button>
      </div>
      <h3 className={styles.useName}>Import snippets</h3>
      <p className={styles.note}>
        A document that Export made here or in another Nocturne. It is checked first, and nothing is written until
        you have seen what would change.
      </p>

      <div className={styles.fileRow}>
        <Button size="sm" icon={FileUp} data-autofocus onClick={() => fileRef.current?.click()}>
          Choose a file
        </Button>
        <span className={styles.note}>{fileName ?? 'or paste the document below'}</span>
        <input
          ref={fileRef}
          className={styles.file}
          type="file"
          accept=".json,application/json,text/plain"
          tabIndex={-1}
          aria-hidden="true"
          title="A snippets document to import"
          onChange={async (event) => {
            const file = event.target.files?.[0];
            event.target.value = '';
            if (!file) return;
            try {
              setText(await file.text());
              setFileName(file.name);
            } catch {
              setFailure('That file could not be read.');
            }
          }}
        />
      </div>

      <Field label="Document" hint="JSON, as Export wrote it.">
        <Textarea
          mono
          rows={6}
          spellCheck={false}
          value={text}
          onChange={(event) => {
            setText(event.target.value);
            setFileName(null);
          }}
        />
      </Field>

      <Field label="When a snippet here has the same name and device type">
        <RadioGroup
          options={MODES}
          value={mode}
          onChange={(value) => setMode(value as SnippetImportConflict)}
        />
      </Field>

      {failure && (
        <p className={styles.refused} role="alert" data-testid="import-refused">
          <b>Nothing was imported.</b> {failure}
        </p>
      )}

      {shown && (
        <section className={styles.exact} aria-label={applied ? 'What was imported' : 'What the import would do'}>
          <h4 className={styles.exactTitle}>{applied ? 'What was imported' : 'What the import would do'}</h4>
          <p className={styles.importSummary} data-testid="import-summary">
            {importSummary(shown)}
          </p>
          {shown.problems.length === 0 ? (
            <KeyValue
              items={[
                { label: applied ? 'Added' : 'Would be added', value: formatCount(shown.created) },
                { label: applied ? 'Of them renamed' : 'Of them under a new name', value: formatCount(shown.renamed) },
                { label: applied ? 'Replaced' : 'Would be replaced', value: formatCount(shown.replaced) },
                { label: applied ? 'Left as they are' : 'Would be left as they are', value: formatCount(shown.skipped) },
              ]}
            />
          ) : (
            <ul className={styles.problems} aria-label="Entries that cannot be imported">
              {shown.problems.map((problem) => (
                <li key={problem.index} className={styles.problem}>
                  <span className={styles.problemWhere}>
                    Entry {problem.index + 1}
                    {problem.name ? ` · ${problem.name}` : ''}
                  </span>
                  <span>{problem.problem}</span>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      <div className={styles.formActions}>
        <Button loading={pending === 'check'} disabled={text.trim() === '' || pending !== null} onClick={() => void run(true)}>
          Check the document
        </Button>
        <Button
          variant="primary"
          loading={pending === 'apply'}
          disabled={!mayApply(preview) || pending !== null}
          onClick={() => void run(false)}
        >
          Import
        </Button>
      </div>
    </div>
  );
}
