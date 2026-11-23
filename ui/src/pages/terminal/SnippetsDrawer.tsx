/**
 * Saved commands. Clicking one types it into the session and stops there —
 * the Enter is always the user's, never ours.
 */

import { Pencil, Plus, ScrollText, Send, Trash } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { Snippet, SnippetDeleteResult, SnippetSaveParams } from '@/api/types/terminal';
import { Button, IconButton } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import { fuzzyFilter } from '@/lib/fuzzy';
import {
  draftFor,
  newDraft,
  profileChoices,
  saveParams,
  withProfileChoice,
  type SnippetDraft,
} from './snippetDraft';
import styles from './SnippetsDrawer.module.css';

export interface SnippetsDrawerProps {
  open: boolean;
  onClose: () => void;
  /** The active device's profile, so its own snippets come first. */
  profile: string | null;
  /** Where a snippet would go. Empty when no session is open. */
  destination: string | null;
  onSend: (snippet: Snippet) => void;
}

export function SnippetsDrawer({ open, onClose, profile, destination, onSend }: SnippetsDrawerProps) {
  const [query, setQuery] = useState('');
  const [draft, setDraft] = useState<SnippetDraft | null>(null);

  const snippets = useRpc<Snippet[]>(
    'terminal.snippets.list',
    { profile },
    { invalidateOn: ['terminal.snippets.changed'], enabled: open },
  );
  const save = useRpcMutation<Snippet, SnippetSaveParams>('terminal.snippets.save', {
    invalidates: ['terminal.snippets.'],
    successToast: 'Snippet saved',
    errorToast: false,
  });
  const remove = useRpcMutation<SnippetDeleteResult, { id: string }>('terminal.snippets.delete', {
    invalidates: ['terminal.snippets.'],
    successToast: 'Snippet deleted',
  });

  const rows = useMemo(() => {
    const all = snippets.data ?? [];
    if (query.trim() === '') return all;
    return fuzzyFilter(all, query, {
      text: (snippet) => snippet.name,
      keywords: (snippet) => [snippet.text, ...snippet.tags],
    }).map((result) => result.item);
  }, [snippets.data, query]);

  const close = () => {
    setDraft(null);
    save.reset();
    onClose();
  };

  return (
    <Drawer
      open={open}
      onClose={close}
      title="Snippets"
      description={
        destination
          ? `Clicking one types it into ${destination}. It is never sent on its own: the Enter is yours.`
          : 'Saved commands. Open a session to send one.'
      }
      footer={
        draft ? null : (
          <Button icon={Plus} onClick={() => setDraft(newDraft(profile))}>
            New snippet
          </Button>
        )
      }
    >
      {draft ? (
        <form
          className={styles.form}
          onSubmit={(event) => {
            event.preventDefault();
            save.mutate(saveParams(draft), { onSuccess: () => setDraft(null) });
          }}
        >
          <Field label="Name" required>
            <Input
              data-autofocus
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
            />
          </Field>
          <Field
            label="Commands"
            hint="Exactly as they will be typed. Several lines are allowed; each one is an Enter on the device."
            error={save.error?.message}
          >
            <Textarea
              mono
              rows={8}
              value={draft.text}
              onChange={(event) => setDraft({ ...draft, text: event.target.value })}
            />
          </Field>
          <Field
            label="Device type"
            hint="A snippet for one device type is offered only in sessions to devices of that type."
          >
            <Select
              options={profileChoices(draft, profile)}
              value={draft.profile ?? ''}
              onChange={(event) => setDraft(withProfileChoice(draft, event.target.value))}
            />
          </Field>
          <Field label="Tags" hint="Words to find it by. Enter or a comma adds one.">
            <TagInput value={draft.tags} onChange={(tags) => setDraft({ ...draft, tags })} />
          </Field>
          <div className={styles.formActions}>
            <Button onClick={() => setDraft(null)} disabled={save.isPending}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" loading={save.isPending}>
              Save snippet
            </Button>
          </div>
        </form>
      ) : (
        <div className={styles.body}>
          <SearchInput aria-label="Search snippets" value={query} onChange={setQuery} />

          {snippets.isError ? (
            <ErrorState
              size="compact"
              title="Snippets could not be loaded"
              error={snippets.error}
              onRetry={() => void snippets.refetch()}
            />
          ) : snippets.isPending ? (
            <>
              <Skeleton height={44} />
              <Skeleton height={44} />
            </>
          ) : rows.length === 0 ? (
            <EmptyState
              size="compact"
              icon={ScrollText}
              title={query ? 'No snippet matches' : 'No snippets yet'}
              description={
                query
                  ? `Nothing matches “${query}”.`
                  : 'Save the commands you type most and send them to any device in one click.'
              }
              action={
                <Button size="sm" icon={Plus} onClick={() => setDraft(newDraft(profile))}>
                  New snippet
                </Button>
              }
            />
          ) : (
            <ul className={styles.rows}>
              {rows.map((snippet) => (
                <li key={snippet.id} className={styles.row}>
                  <button
                    type="button"
                    className={styles.send}
                    disabled={destination === null}
                    onClick={() => onSend(snippet)}
                    title={destination ? `Type into ${destination}` : 'Open a session first'}
                  >
                    <span className={styles.name}>
                      {snippet.name}
                      {snippet.profile && <span className={styles.profile}>{snippet.profile}</span>}
                    </span>
                    <code className={styles.preview}>{snippet.text.split('\n')[0]}</code>
                    {snippet.text.includes('\n') && (
                      <span className={styles.more}>{snippet.text.trimEnd().split('\n').length} lines</span>
                    )}
                    {snippet.tags.length > 0 && (
                      <span className={styles.tags}>
                        {snippet.tags.map((tag) => (
                          <span key={tag} className={styles.tag}>
                            {tag}
                          </span>
                        ))}
                      </span>
                    )}
                  </button>
                  <span className={styles.rowActions}>
                    <IconButton
                      icon={Send}
                      size="sm"
                      aria-label={`Type ${snippet.name}`}
                      disabled={destination === null}
                      onClick={() => onSend(snippet)}
                    />
                    <IconButton
                      icon={Pencil}
                      size="sm"
                      aria-label={`Edit ${snippet.name}`}
                      onClick={() => setDraft(draftFor(snippet))}
                    />
                    <IconButton
                      icon={Trash}
                      size="sm"
                      aria-label={`Delete ${snippet.name}`}
                      onClick={async () => {
                        const confirmed = await confirm({
                          title: `Delete “${snippet.name}”?`,
                          description: 'The snippet is removed. Commands already sent are not affected.',
                          confirmLabel: 'Delete',
                          tone: 'danger',
                        });
                        if (confirmed) remove.mutate({ id: snippet.id });
                      }}
                    />
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Drawer>
  );
}
