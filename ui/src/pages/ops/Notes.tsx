/**
 * Notes and runbooks: what the last engineer learned at 3am.
 *
 * Markdown as written, never rendered by the backend and not rendered here
 * either — a runbook is read and copied from, and a half-rendered one hides
 * the difference between what it says and what it means. It is shown in the
 * monospace it was typed in.
 *
 * A `[[device name]]` in the body is resolved against the inventory when the
 * note is saved, and the links are shown as the backend reports them: a name
 * that matches nothing keeps its text and is marked unresolved, because a note
 * that names a device nobody has is worth knowing about.
 */

import { Notebook, Plus, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import type { NoteHead } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { SearchInput } from '@/design/SearchInput';
import { Skeleton } from '@/design/Skeleton';
import { LabPill } from '@/design/StatusPill';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { plural } from '@/lib/format';
import { useAuthor } from './author';
import { useDeleteNote, useNote, useNotes, useSaveNote } from './data';
import styles from './Notes.module.css';
import opsStyles from './Ops.module.css';
import { instant } from './ops';

export function NotesTab() {
  const [query, setQuery] = useState('');
  const [openId, setOpenId] = useState<string | null>(null);
  const [writing, setWriting] = useState(false);
  const notes = useNotes({ query: query.trim() === '' ? undefined : query.trim() });

  return (
    <>
      <Toolbar>
        <SearchInput
          aria-label="Search notes"
          value={query}
          onChange={setQuery}
          placeholder="Search titles, bodies and tags"
          className={opsStyles.filter}
        />
        <ToolbarSpacer />
        <Button
          variant="primary"
          icon={Plus}
          onClick={() => {
            setOpenId(null);
            setWriting(true);
          }}
        >
          Write a note
        </Button>
      </Toolbar>

      <div className={opsStyles.columns}>
        <div className={styles.main}>
          {writing ? (
            <Editor key="new" id={null} onDone={(id) => { setWriting(false); setOpenId(id); }} onCancel={() => setWriting(false)} />
          ) : openId !== null ? (
            <Editor key={openId} id={openId} onDone={(id) => setOpenId(id)} onCancel={() => setOpenId(null)} />
          ) : (
            <EmptyState
              icon={Notebook}
              title="Nothing is open"
              description="Notes are the runbooks and the hard-won details: how the WAN fails over, which optic is spare, what the vendor said. A note that names a device in double brackets turns up on that device's page."
              action={
                <Button variant="primary" icon={Plus} onClick={() => setWriting(true)}>
                  Write a note
                </Button>
              }
            />
          )}
        </div>

        <aside className={styles.aside}>
          <Panel title="Notes" padding={notes.data?.notes.length ? 'none' : 'md'}>
            {notes.isPending && <Skeleton shape="block" height={160} />}
            {notes.isError && (
              <ErrorState
                size="compact"
                title="The notes could not be read"
                error={notes.error}
                onRetry={() => void notes.refetch()}
              />
            )}
            {notes.data && notes.data.notes.length === 0 && (
              <EmptyState
                size="compact"
                icon={Notebook}
                title={query.trim() === '' ? 'No note yet' : 'Nothing matches that'}
                description={
                  query.trim() === ''
                    ? 'The first one is usually the thing you had to ask somebody.'
                    : 'The search covers titles, bodies, tags and sites.'
                }
              />
            )}
            {notes.data && notes.data.notes.length > 0 && (
              <ul className={styles.list}>
                {notes.data.notes.map((head) => (
                  <Row
                    key={head.id}
                    head={head}
                    current={head.id === openId && !writing}
                    onOpen={() => {
                      setWriting(false);
                      setOpenId(head.id);
                    }}
                  />
                ))}
              </ul>
            )}
          </Panel>
        </aside>
      </div>
    </>
  );
}

function Row({ head, current, onOpen }: { head: NoteHead; current: boolean; onOpen: () => void }) {
  return (
    <li className={styles.row} data-current={current || undefined}>
      <button type="button" className={styles.rowButton} onClick={onOpen}>
        <span className={styles.rowTitle}>{head.title}</span>
        <span className={styles.rowMeta}>
          {plural(head.lines, 'line')} · <TimeAgo at={instant(head.updated_at)} />
          {head.author && ` · ${head.author}`}
        </span>
        {head.excerpt && <span className={styles.excerpt}>{head.excerpt}</span>}
        {(head.tags.length > 0 || head.sites.length > 0) && (
          <span className={styles.rowTags}>
            {head.tags.map((tag) => (
              <Badge key={tag} tone="neutral" variant="outline">
                #{tag}
              </Badge>
            ))}
            {head.sites.map((site) => (
              <Badge key={site} tone="info" variant="outline">
                {site}
              </Badge>
            ))}
          </span>
        )}
      </button>
    </li>
  );
}

function Editor({
  id,
  onDone,
  onCancel,
}: {
  id: string | null;
  onDone: (id: string) => void;
  onCancel: () => void;
}) {
  const existing = useNote(id);
  const save = useSaveNote();
  const remove = useDeleteNote();
  const [author, setAuthor] = useAuthor();

  const note = existing.data;
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [tags, setTags] = useState<string[]>([]);
  const [sites, setSites] = useState<string[]>([]);
  // One load into the form, then it is the person's: a `notes.changed` event
  // from somebody else's save must not overwrite what is being typed.
  const [loaded, setLoaded] = useState<string | null>(null);
  if (note && loaded !== note.id) {
    setLoaded(note.id);
    setTitle(note.title);
    setBody(note.body);
    setTags(note.tags);
    setSites(note.sites);
  }

  if (id !== null && existing.isError) {
    return (
      <ErrorState
        size="compact"
        title="That note could not be read"
        error={existing.error}
        onRetry={() => void existing.refetch()}
      />
    );
  }
  if (id !== null && existing.isPending) return <Skeleton shape="block" height={360} />;

  const dirty =
    note === undefined
      ? title.trim() !== '' || body.trim() !== ''
      : title !== note.title ||
        body !== note.body ||
        tags.join() !== note.tags.join() ||
        sites.join() !== note.sites.join();

  return (
    <Panel
      title={id === null ? 'A new note' : 'Note'}
      actions={
        <>
          {id !== null && (
            <Button
              size="sm"
              variant="ghost"
              icon={Trash2}
              loading={remove.isPending}
              aria-label="Delete this note"
              onClick={async () => {
                const sure = await confirm({
                  title: 'Delete this note?',
                  description: `“${note?.title ?? ''}” and its body will be gone. This cannot be undone.`,
                  confirmLabel: 'Delete it',
                  tone: 'danger',
                });
                if (sure) remove.mutate({ id }, { onSuccess: () => onCancel() });
              }}
            />
          )}
          <Button size="sm" variant="ghost" onClick={onCancel}>
            Close
          </Button>
          <Button
            size="sm"
            variant={dirty ? 'primary' : 'secondary'}
            loading={save.isPending}
            disabled={title.trim() === '' || !dirty}
            onClick={() =>
              save.mutate(
                {
                  ...(id === null ? {} : { id }),
                  title: title.trim(),
                  body,
                  tags,
                  sites,
                  author,
                },
                { onSuccess: (stored) => onDone(stored.id) },
              )
            }
          >
            {dirty ? 'Save' : 'Saved'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field label="Title" required>
          <Input
            value={title}
            autoFocus={id === null}
            placeholder="What this note is about"
            onChange={(event) => setTitle(event.target.value)}
          />
        </Field>
        <Field
          label="Body"
          hint="Markdown, kept exactly as you type it. Put a device in double square brackets to link it: [[core-sw1]]."
        >
          <Textarea
            mono
            rows={16}
            value={body}
            placeholder="Markdown. [[device name]] links a device."
            onChange={(event) => setBody(event.target.value)}
          />
        </Field>
        <div className={opsStyles.formRow}>
          <Field label="Tags" hint="Lower-cased and sorted when saved.">
            <TagInput aria-label="Tags" value={tags} onChange={setTags} placeholder="add a tag" />
          </Field>
          <Field label="Sites" hint="The note turns up for these sites.">
            <TagInput aria-label="Sites" value={sites} onChange={setSites} placeholder="add a site" />
          </Field>
        </div>
        <Field label="Your name" hint="Remembered in this browser.">
          <Input size="sm" value={author} placeholder="who is writing this" onChange={(event) => setAuthor(event.target.value)} />
        </Field>

        {note && note.links.length > 0 && (
          <div>
            <p className={styles.linksLabel}>Devices named in the body, as saved</p>
            <ul className={styles.links}>
              {note.links.map((link) => (
                <li key={link.text}>
                  {link.device ? (
                    <Link to={`/devices/${encodeURIComponent(link.device.id)}`} className={styles.link}>
                      {link.device.name}
                      {link.device.lab && <LabPill />}
                      {link.device.deleted && (
                        <Badge tone="idle" variant="outline">
                          gone
                        </Badge>
                      )}
                    </Link>
                  ) : (
                    <span
                      className={styles.unresolved}
                      title="The inventory has no device of that name, so the link does not resolve. The text is kept as written."
                    >
                      {link.text} — not in the inventory
                    </span>
                  )}
                </li>
              ))}
            </ul>
            {dirty && (
              <p className={styles.linksHint}>
                These are the links of the saved version; they are worked out again when you save.
              </p>
            )}
          </div>
        )}

        {note && (
          <p className={styles.meta}>
            Last saved <TimeAgo at={instant(note.updated_at)} />
            {note.author && ` by ${note.author}`} · created{' '}
            <TimeAgo at={instant(note.created_at)} />
          </p>
        )}
      </div>
    </Panel>
  );
}
