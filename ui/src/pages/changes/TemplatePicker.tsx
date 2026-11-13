/**
 * Starting a change from a saved template.
 *
 * `templates.list` / `templates.render` are not part of the change engine's
 * own surface: a build either ships a template library or it does not. This
 * build does not, so the source is offered disabled (see `NewChange`) and
 * this component is never mounted there.
 *
 * Where the methods do exist, their shapes are not pinned down by any spec
 * this screen can read, so the reader below is deliberately tolerant: it
 * accepts the obvious spellings and shows exactly what came back, and when it
 * cannot make sense of the answer it says so rather than inventing a list.
 * Whatever a template renders to is dropped into the Lines box, where it can
 * be read and edited before anything is planned — a template is a starting
 * point, never a thing that is sent unseen.
 */

import { useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import { Button } from '@/design/Button';
import { Field } from '@/design/Field';
import { Select } from '@/design/Select';
import { Textarea } from '@/design/Textarea';
import styles from './NewChange.module.css';

export interface TemplateChoice {
  name: string;
  /** The rendered configuration, exactly as the backend returned it. */
  text: string;
}

interface TemplateRow {
  id: string;
  name: string;
  description?: string;
}

function readList(value: unknown): TemplateRow[] | null {
  const list = Array.isArray(value)
    ? value
    : typeof value === 'object' && value !== null && Array.isArray((value as { templates?: unknown }).templates)
      ? ((value as { templates: unknown[] }).templates)
      : null;
  if (!list) return null;
  const rows: TemplateRow[] = [];
  for (const entry of list) {
    if (typeof entry !== 'object' || entry === null) return null;
    const row = entry as Record<string, unknown>;
    const id = typeof row.id === 'string' ? row.id : typeof row.name === 'string' ? row.name : null;
    if (id === null) return null;
    rows.push({
      id,
      name: typeof row.name === 'string' ? row.name : id,
      description: typeof row.description === 'string' ? row.description : undefined,
    });
  }
  return rows;
}

function readRendered(value: unknown): string | null {
  if (typeof value === 'string') return value;
  if (typeof value !== 'object' || value === null) return null;
  const row = value as Record<string, unknown>;
  for (const key of ['text', 'config', 'rendered', 'output']) {
    const found = row[key];
    if (typeof found === 'string') return found;
  }
  if (Array.isArray(row.lines) && row.lines.every((line) => typeof line === 'string')) {
    return (row.lines as string[]).join('\n');
  }
  return null;
}

export interface TemplatePickerProps {
  onChoose: (choice: TemplateChoice) => void;
}

export function TemplatePicker({ onChoose }: TemplatePickerProps) {
  const [id, setId] = useState('');
  const [variables, setVariables] = useState('');
  const list = useRpc<unknown>('templates.list');
  const render = useRpcMutation<unknown, Record<string, unknown>>('templates.render', {
    errorToast: false,
  });

  if (list.isPending) return <p className={styles.lead}>Reading the template library…</p>;
  if (list.error) {
    return (
      <p className={styles.error} role="alert">
        {list.error.message}
      </p>
    );
  }

  const rows = readList(list.data);
  if (rows === null) {
    return (
      <p className={styles.lead}>
        This build answers `templates.list`, but not in a shape this screen knows how to read.
        Write the lines out instead.
      </p>
    );
  }
  if (rows.length === 0) {
    return <p className={styles.lead}>There are no templates saved yet.</p>;
  }

  const chosen = rows.find((row) => row.id === id);

  return (
    <div className={styles.step}>
      <Field label="Template" hint={chosen?.description}>
        <Select
          value={id}
          onChange={(event) => setId(event.target.value)}
          options={[
            { value: '', label: 'Choose a template…' },
            ...rows.map((row) => ({ value: row.id, label: row.name })),
          ]}
        />
      </Field>
      <Field label="Values" hint="One per line: name=value. Left to the template to use or ignore.">
        <Textarea
          mono
          rows={3}
          value={variables}
          onChange={(event) => setVariables(event.target.value)}
          placeholder={'vlan=30\nname=GUEST'}
        />
      </Field>
      {render.error && (
        <p className={styles.error} role="alert">
          {render.error.message}
        </p>
      )}
      <Button
        disabled={chosen === undefined}
        loading={render.isPending}
        onClick={() => {
          if (!chosen) return;
          const values: Record<string, string> = {};
          for (const line of variables.split('\n')) {
            const at = line.indexOf('=');
            if (at > 0) values[line.slice(0, at).trim()] = line.slice(at + 1).trim();
          }
          render.mutate(
            { id: chosen.id, variables: values },
            {
              onSuccess: (result) => {
                const text = readRendered(result);
                if (text !== null) onChoose({ name: chosen.name, text });
              },
            },
          );
        }}
      >
        Put it in the Lines box
      </Button>
    </div>
  );
}
