/**
 * Saved commands. Clicking one types it into the session and stops there —
 * the Enter is always the user's, never ours.
 *
 * With an engine that renders and sends snippets itself
 * (`terminal.snippets.render` / `.send`) a snippet is never typed from this
 * window: the engine says what the text is, the person sees it, and the
 * engine types exactly that and records where it went. A snippet with
 * variables opens a form first; one without is sent on a click, through the
 * same question a paste gets. With an older engine the click goes down the
 * paste funnel as it always did, and nothing that needs the new methods is
 * on screen.
 *
 * Which variant of a snippet a session gets is the engine's decision
 * (`terminal.snippets.list {session_id}`); this drawer shows its choice and
 * keeps the others one click away.
 */

import {
  ChevronDown,
  ChevronRight,
  Download,
  FileUp,
  MoreHorizontal,
  Pencil,
  Plus,
  ScrollText,
  Send,
  Trash,
  TriangleAlert,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { Link } from 'react-router';
import { rpc } from '@/api/client';
import { useHasMethod, useRpc, useRpcMutation } from '@/api/hooks';
import type {
  SessionInfo,
  Snippet,
  SnippetDeleteResult,
  SnippetRendered,
  SnippetSaveParams,
  SnippetsDocument,
} from '@/api/types/terminal';
import { Button, IconButton } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { DropdownMenu, type MenuEntry } from '@/design/DropdownMenu';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { download } from '@/lib/download';
import { plural } from '@/lib/format';
import { fuzzyFilter } from '@/lib/fuzzy';
import {
  draftFor,
  newDraft,
  profileChoices,
  saveParams,
  withProfileChoice,
  type SnippetDraft,
} from './snippetDraft';
import { SnippetImport } from './SnippetImport';
import styles from './SnippetsDrawer.module.css';
import { snippetRoad } from './snippetRoad';
import { sendSnippet } from './snippetSend';
import {
  exportFilename,
  initialTargets,
  lineCount,
  notChosenBecause,
  openTargets,
  sendReport,
  splitByChoice,
  tagCounts,
  toggleTag,
  withTags,
} from './snippets';
import { SnippetUse } from './SnippetUse';
import { useFocusWhenShown } from './viewFocus';

export interface SnippetsDrawerProps {
  open: boolean;
  onClose: () => void;
  /** The active device's profile: what a new snippet starts out being for. */
  profile: string | null;
  /** Every session the engine holds, to offer the open ones as destinations. */
  sessions: readonly SessionInfo[];
  /** The session in front, or null when none is. */
  focusedId: string | null;
  /** The sessions typing is being broadcast to; empty when it is not. */
  broadcast: readonly string[];
  /**
   * The old road, for an engine that cannot render or send a snippet
   * itself: the snippet's own text, down the paste funnel.
   */
  onPaste: (snippet: Snippet) => void;
  /** A snippet reached every session it was meant for; the first of them is given. */
  onSent: (sessionId: string) => void;
  /**
   * Text to start a new snippet from (a clip being saved as one). The
   * number tells one request from the next for the same text.
   */
  seed?: { text: string; nonce: number } | null;
}

type View = { kind: 'list' } | { kind: 'form'; draft: SnippetDraft } | { kind: 'use'; id: string } | { kind: 'import' };

export function SnippetsDrawer({
  open,
  onClose,
  profile,
  sessions,
  focusedId,
  broadcast,
  onPaste,
  onSent,
  seed,
}: SnippetsDrawerProps) {
  const [query, setQuery] = useState('');
  const [tags, setTags] = useState<string[]>([]);
  const [view, setView] = useState<View>({ kind: 'list' });
  const [showOthers, setShowOthers] = useState(false);
  /** The snippet being sent straight from its row, so it cannot be sent twice. */
  const [quick, setQuick] = useState<string | null>(null);
  /** The same, for the next event before the render that `quick` waits for. */
  const quickNow = useRef<string | null>(null);
  /** The snippet last opened to send or edit: the list gives it the focus when it is shown again. */
  const [lastOpened, setLastOpened] = useState<string | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const formRef = useRef<HTMLFormElement>(null);
  useFocusWhenShown(listRef, view.kind);
  useFocusWhenShown(formRef, view.kind);
  // The row to come back to belongs to one visit. A send closes the drawer
  // from outside, past `close`, and the next visit must not open on it.
  useEffect(() => {
    if (!open) setLastOpened(null);
  }, [open]);

  // Nothing is offered on a guess: until `app.methods` has answered these
  // are false, and the drawer is the one it was before the engine could do
  // any of it.
  const canRender = useHasMethod('terminal.snippets.render') === true;
  const canSend = useHasMethod('terminal.snippets.send') === true;
  const engineSends = canRender && canSend;
  const canExport = useHasMethod('terminal.snippets.export') === true;
  const canImport = useHasMethod('terminal.snippets.import') === true;
  const hasAutomation = useHasMethod('runner.run') === true;

  const focused = focusedId ? sessions.find((session) => session.id === focusedId) : undefined;
  const targets = useMemo(() => openTargets(sessions), [sessions]);
  // Where a click on a row types: the session in front — a closed one takes
  // nothing — or, while typing is broadcast from it, every session the
  // keyboard goes to. Said as such, so the row does not name one session
  // and the question then name three.
  const clickGoesTo = initialTargets(targets, focusedId, broadcast);
  const destination =
    clickGoesTo.length > 1
      ? `${clickGoesTo.length} sessions`
      : focused && focused.state === 'open'
        ? focused.title
        : null;

  // With a session in front the engine is asked for *its* snippets and marks
  // the variant it chose; without one, or with an engine that cannot say,
  // the list is the plain one filtered by device type as before.
  const listParams = engineSends && focused ? { session_id: focused.id } : { profile };
  const snippets = useRpc<Snippet[]>('terminal.snippets.list', listParams, {
    invalidateOn: ['terminal.snippets.changed'],
    enabled: open,
  });
  const save = useRpcMutation<Snippet, SnippetSaveParams>('terminal.snippets.save', {
    invalidates: ['terminal.snippets.'],
    successToast: 'Snippet saved',
    errorToast: false,
  });
  const remove = useRpcMutation<SnippetDeleteResult, { id: string }>('terminal.snippets.delete', {
    invalidates: ['terminal.snippets.'],
    successToast: 'Snippet deleted',
  });

  // A clip being saved as a snippet opens the form with its text in it.
  const seedNonce = seed?.nonce;
  useEffect(() => {
    if (!open || !seed) return;
    setView({ kind: 'form', draft: { ...newDraft(profile), text: seed.text } });
    // Keyed on the request, not on what it carries: only a new request
    // re-opens the form, and the profile in front may change under it.
  }, [open, seedNonce]); // eslint-disable-line react-hooks/exhaustive-deps

  const all = useMemo(() => snippets.data ?? [], [snippets.data]);
  const allTags = useMemo(() => tagCounts(all), [all]);
  const rows = useMemo(() => {
    const tagged = withTags(all, tags);
    const matching =
      query.trim() === ''
        ? tagged
        : fuzzyFilter(tagged, query, {
            text: (snippet) => snippet.name,
            keywords: (snippet) => [snippet.text, ...snippet.tags],
          }).map((result) => result.item);
    return splitByChoice(matching);
  }, [all, tags, query]);

  const close = () => {
    setView({ kind: 'list' });
    // The next session it opens for may have another device type, and other others.
    setShowOthers(false);
    setLastOpened(null);
    save.reset();
    onClose();
  };

  const openUse = (snippet: Snippet) => {
    setLastOpened(snippet.id);
    setView({ kind: 'use', id: snippet.id });
  };

  const using = view.kind === 'use' ? all.find((snippet) => snippet.id === view.id) : undefined;

  /**
   * A snippet without variables, sent from its row. There is nothing to fill
   * in, so the engine's text for it goes straight to the question a paste
   * would get — every line on the sheet when it has several or one of them
   * changes something — and one line that only reads is typed at once.
   */
  const sendNow = async (snippet: Snippet) => {
    const chosen = initialTargets(targets, focusedId, broadcast);
    const where = targets.filter((target) => chosen.includes(target.sessionId));
    const first = where[0];
    if (!first || quickNow.current !== null) return;
    quickNow.current = snippet.id;
    setQuick(snippet.id);
    try {
      const rendered = await rpc<SnippetRendered>('terminal.snippets.render', { id: snippet.id, values: {} });
      const outcome = await sendSnippet(snippetRoad, { id: snippet.id, values: {}, shown: rendered, targets: where });
      if (outcome.kind !== 'sent') return;
      const told = sendReport(outcome.result, where);
      if (told.complete) {
        setView({ kind: 'list' });
        onSent(first.sessionId);
        return;
      }
      toast.error(told.headline, {
        description: told.sessions
          .filter((entry) => !entry.ok)
          .map((entry) => `${entry.title}: ${entry.says}`)
          .join(' '),
      });
    } catch (error) {
      toast.error(`“${snippet.name}” was not sent`, {
        description: error instanceof Error ? error.message : undefined,
      });
    } finally {
      quickNow.current = null;
      setQuick(null);
    }
  };

  const choose = (snippet: Snippet) => {
    if (snippet.problem) return;
    if (!engineSends) {
      onPaste(snippet);
      return;
    }
    if (snippet.variables.length > 0) openUse(snippet);
    else void sendNow(snippet);
  };

  const exportAll = async () => {
    try {
      const document_ = await rpc<SnippetsDocument>('terminal.snippets.export');
      download(exportFilename(), `${JSON.stringify(document_, null, 2)}\n`, 'application/json');
    } catch (error) {
      toast.error('The snippets could not be exported', {
        description: error instanceof Error ? error.message : undefined,
      });
    }
  };

  const more: MenuEntry[] = [
    ...(canImport ? [{ label: 'Import…', icon: FileUp, onSelect: () => setView({ kind: 'import' }) }] : []),
    ...(canExport ? [{ label: 'Export all', icon: Download, onSelect: () => void exportAll() }] : []),
  ];

  const row = (snippet: Snippet) => {
    const lines = lineCount(snippet.text);
    const sendable = !snippet.problem && destination !== null;
    return (
      <li key={snippet.id} className={styles.row} data-problem={snippet.problem ? true : undefined}>
        <button
          type="button"
          className={styles.send}
          // Busy, not disabled, while a send from a row is on its way: a
          // disabled button lets go of the focus, and the question that
          // follows would then have nothing to hand it back to.
          disabled={!sendable}
          aria-disabled={quick !== null || undefined}
          aria-busy={quick === snippet.id || undefined}
          data-autofocus={snippet.id === lastOpened || undefined}
          onClick={() => choose(snippet)}
          title={
            snippet.problem
              ? 'It cannot be sent until it is corrected'
              : destination
                ? engineSends && snippet.variables.length > 0
                  ? `Fill in and type into ${destination}`
                  : `Type into ${destination}`
                : 'Open a session first'
          }
        >
          <span className={styles.name}>
            {snippet.name}
            {snippet.profile && <span className={styles.profile}>{snippet.profile}</span>}
            {engineSends && snippet.variables.length > 0 && (
              <span className={styles.profile}>{plural(snippet.variables.length, 'variable')}</span>
            )}
          </span>
          {/* Text a person typed for a device, and for an old snippet it can hold anything: as text. */}
          <code className={styles.preview}>{snippet.text.split('\n')[0]}</code>
          {lines > 1 && <span className={styles.more}>{lines} lines</span>}
          {snippet.tags.length > 0 && (
            <span className={styles.tags}>
              {snippet.tags.map((tag) => (
                <span key={tag} className={styles.tag}>
                  {tag}
                </span>
              ))}
            </span>
          )}
          {snippet.problem && (
            <span className={styles.problemLine}>
              <Icon icon={TriangleAlert} size={12} />
              Cannot be sent as it is — {snippet.problem}. Edit it and save it again.
            </span>
          )}
          {snippet.chosen === false && <span className={styles.more}>{notChosenBecause(snippet, all)}</span>}
        </button>
        <span className={styles.rowActions}>
          {engineSends && !snippet.problem && (
            <IconButton
              icon={Send}
              size="sm"
              aria-label={`Choose where to send ${snippet.name}`}
              tooltip="Choose the sessions it goes to"
              onClick={() => openUse(snippet)}
            />
          )}
          {!engineSends && (
            <IconButton
              icon={Send}
              size="sm"
              aria-label={`Type ${snippet.name}`}
              disabled={!sendable}
              onClick={() => choose(snippet)}
            />
          )}
          <IconButton
            icon={Pencil}
            size="sm"
            aria-label={`Edit ${snippet.name}`}
            onClick={() => {
              setLastOpened(snippet.id);
              setView({ kind: 'form', draft: draftFor(snippet) });
            }}
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
    );
  };

  const draft = view.kind === 'form' ? view.draft : null;
  const setDraft = (next: SnippetDraft) => setView({ kind: 'form', draft: next });

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
        view.kind === 'list' ? (
          <>
            {more.length > 0 && (
              <DropdownMenu
                aria-label="Import and export"
                trigger={
                  <Button icon={MoreHorizontal} aria-label="Import and export snippets">
                    Import and export
                  </Button>
                }
                items={more}
              />
            )}
            <Button icon={Plus} onClick={() => setView({ kind: 'form', draft: newDraft(profile) })}>
              New snippet
            </Button>
          </>
        ) : null
      }
    >
      {draft ? (
        <form
          ref={formRef}
          className={styles.form}
          onSubmit={(event) => {
            event.preventDefault();
            save.mutate(saveParams(draft), { onSuccess: () => setView({ kind: 'list' }) });
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
            hint={
              engineSends
                ? 'Exactly as they will be typed; each line is an Enter on the device. {{port}} is asked for when it is sent, {{port=Gi0/1}} has a default, and \\{{ types two braces as they are.'
                : 'Exactly as they will be typed. Several lines are allowed; each one is an Enter on the device.'
            }
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
            hint="A snippet for one device type is offered only in sessions to devices of that type. Two snippets may share a name when their device types differ."
          >
            <Select
              options={profileChoices(draft, profile)}
              value={draft.profile ?? ''}
              onChange={(event) => setDraft(withProfileChoice(draft, event.target.value))}
            />
          </Field>
          <Field label="Tags" hint="Words to find it by. Enter or a comma adds one.">
            <TagInput value={draft.tags} onChange={(next) => setDraft({ ...draft, tags: next })} />
          </Field>
          <div className={styles.formActions}>
            <Button onClick={() => setView({ kind: 'list' })} disabled={save.isPending}>
              Cancel
            </Button>
            <Button type="submit" variant="primary" loading={save.isPending}>
              Save snippet
            </Button>
          </div>
        </form>
      ) : view.kind === 'import' ? (
        <SnippetImport onBack={() => setView({ kind: 'list' })} onImported={() => void snippets.refetch()} />
      ) : view.kind === 'use' && !using ? (
        <div className={styles.body}>
          <p className={styles.note}>That snippet is no longer in the list.</p>
          <div>
            <Button size="sm" onClick={() => setView({ kind: 'list' })}>
              All snippets
            </Button>
          </div>
        </div>
      ) : view.kind === 'use' && using ? (
        <SnippetUse
          // A snippet that was edited meanwhile is another text: the form starts over.
          key={`${using.id}:${using.updated_at}`}
          snippet={using}
          all={all}
          targets={targets}
          initial={initialTargets(targets, focusedId, broadcast)}
          onBack={() => setView({ kind: 'list' })}
          onSent={(sessionId) => {
            setView({ kind: 'list' });
            onSent(sessionId);
          }}
        />
      ) : (
        <div ref={listRef} className={styles.body}>
          <SearchInput aria-label="Search snippets" value={query} onChange={setQuery} />

          {allTags.length > 0 && (
            <div className={styles.filter} role="group" aria-label="Filter by tag">
              {allTags.map(({ tag, count }) => (
                <Button
                  key={tag}
                  size="sm"
                  variant={tags.includes(tag) ? 'secondary' : 'ghost'}
                  aria-pressed={tags.includes(tag)}
                  onClick={() => setTags(toggleTag(tags, tag))}
                >
                  {tag} <span className={styles.count}>{count}</span>
                </Button>
              ))}
              {tags.length > 0 && (
                <Button size="sm" variant="ghost" onClick={() => setTags([])}>
                  Clear
                </Button>
              )}
            </div>
          )}

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
          ) : rows.offered.length === 0 && rows.others.length === 0 ? (
            <EmptyState
              size="compact"
              icon={ScrollText}
              title={query || tags.length > 0 ? 'No snippet matches' : 'No snippets yet'}
              description={
                query || tags.length > 0
                  ? 'Nothing matches the search and the tags that are picked.'
                  : 'Save the commands you type most and send them to any device in one click.'
              }
              action={
                <Button size="sm" icon={Plus} onClick={() => setView({ kind: 'form', draft: newDraft(profile) })}>
                  New snippet
                </Button>
              }
            />
          ) : (
            <>
              {focused && engineSends && (
                <p className={styles.note} data-testid="snippet-choice">
                  Shown for {focused.title}: where a snippet has a variant for this session’s device type, the engine
                  has picked that one.
                </p>
              )}
              <ul className={styles.rows} aria-label="Snippets">
                {rows.offered.map(row)}
              </ul>
              {rows.others.length > 0 && (
                <section className={styles.others}>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={showOthers ? ChevronDown : ChevronRight}
                    aria-expanded={showOthers}
                    onClick={() => setShowOthers(!showOthers)}
                  >
                    {showOthers ? 'Hide' : 'Show'} {plural(rows.others.length, 'other variant')}
                  </Button>
                  {showOthers && (
                    <ul className={styles.rows} aria-label="Other variants">
                      {rows.others.map(row)}
                    </ul>
                  )}
                </section>
              )}
            </>
          )}

          {hasAutomation && (
            <p className={styles.note}>
              The same on many devices? <Link to="/automation/run">Automation</Link> runs read-only commands on a set
              of devices and keeps what each one answered. Snippets are typed into the sessions in front of you, one
              confirmation at a time.
            </p>
          )}
        </div>
      )}
    </Drawer>
  );
}
