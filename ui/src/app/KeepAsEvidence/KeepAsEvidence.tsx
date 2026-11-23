/**
 * "Keep as evidence…" for anything the engine can capture by itself.
 *
 * A screen that shows a chart or the map can offer to keep what is on it. The
 * dialog here does not send the picture or the numbers: it sends *what to
 * read* (`evidence.capture_chart`, `evidence.capture_topology`) and the
 * engine reads it again, redacts it and stores it with its source and a
 * hash. That is why the dialog can say where the item will come from before
 * it exists, and why what is kept is checkable afterwards.
 *
 * How a screen uses it:
 *
 *   const keep = useKeepAsEvidence();
 *   …
 *   <TimeSeriesChart
 *     …
 *     exportMenu={{ extra: keep.chartEntries(() => ({ kind: 'chart', series, since, until: Date.now(), step_s })) }}
 *   />
 *   {keep.dialog}
 *
 * The three kinds that are text — the last lines of a terminal session
 * (`evidence.capture_terminal`), a page of a session log
 * (`evidence.capture_log`) and text a person holds (`evidence.add_text`) —
 * show the text in the dialog before anything is stored. For the first two
 * it is the engine's own redacted read of what it will read again
 * (`terminal.snapshot`, `terminal.logs.read`). For the third there is
 * nothing redacted to show beforehand — the engine has no method that
 * redacts without storing — so the dialog shows the text as it will be
 * sent, says that the item will be labelled as supplied by the operator,
 * and afterwards shows what was stored, with a way to delete it.
 *
 * `keep.can('chart')` is false in a build without `evidence.capture_chart`
 * (and while `app.methods` is still on its way), and `chartEntries` is then
 * empty, so a build without evidence shows no entry at all. For the map,
 * show a button when `keep.can('topology')` and call `keep.keep({ kind:
 * 'topology', view, layer, filter, include_endpoints })`.
 *
 * It reads with `useRpc` and writes with a plain `rpc` call rather than a
 * mutation hook: the lists that show evidence refresh on the engine's own
 * `evidence.changed` and `incidents.changed`, so there is nothing to
 * invalidate by hand.
 */

import { FileCheck2, Trash } from 'lucide-react';
import { useEffect, useState, type ReactNode } from 'react';
import { rpc } from '@/api/client';
import { useRpc } from '@/api/hooks';
import type { Evidence, EvidenceWithBody, IncidentsListResult } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { KeyValue } from '@/design/KeyValue';
import type { MenuAction } from '@/design/Menu';
import { Select } from '@/design/Select';
import { toast } from '@/design/Toast';
import { formatCount, plural } from '@/lib/format';
import { useAuthor } from '@/pages/ops/author';
import styles from './KeepAsEvidence.module.css';
import {
  CAPTURE_METHOD,
  captureCall,
  describeRequest,
  evidenceLink,
  refusal,
  type ChartEvidenceRequest,
  type KeepRequest,
  type LogEvidenceRequest,
  type Refusal,
  type TerminalEvidenceRequest,
  type TextEvidenceRequest,
  type TopologyEvidenceRequest,
} from './keep';

export type {
  ChartEvidenceRequest,
  KeepRequest,
  LogEvidenceRequest,
  TerminalEvidenceRequest,
  TextEvidenceRequest,
  TopologyEvidenceRequest,
};

/**
 * The methods this build answers, or an empty set while that is not known.
 * One read for the four names this module asks about.
 */
function useMethods(): ReadonlySet<string> {
  const methods = useRpc<string[]>('app.methods');
  // Anything but a list of names is "not known": nothing is offered on a guess.
  return new Set(Array.isArray(methods.data) ? methods.data : []);
}

const ABOUT: Record<KeepRequest['kind'], { what: string; sentence: string }> = {
  chart: {
    what: 'these samples',
    sentence:
      'The engine reads these samples again from its own store and keeps the points it finds — with their gaps left as gaps — as a record with a source, a time and a hash. Nothing from this screen is uploaded.',
  },
  topology: {
    what: 'this drawing',
    sentence:
      'The engine draws this graph again and keeps its own drawing as a record with a source, a time and a hash. It is the engine’s drawing of the same graph, not a pixel copy of the screen.',
  },
  terminal: {
    what: 'these lines',
    sentence:
      'The engine reads the last lines of this session again itself and keeps what it reads, with secrets replaced, as a record with a source, a time and a hash. Nothing from this screen is uploaded.',
  },
  log: {
    what: 'these lines',
    sentence:
      'The engine reads these bytes of the session log again itself and keeps what it reads, with secrets replaced, as a record with a source, a time and a hash. Nothing from this screen is uploaded.',
  },
  text: {
    what: 'this text',
    sentence:
      'This text is sent as you hold it. The engine did not read it from a device itself, so it keeps it as supplied by the operator and says so wherever the item is shown.',
  },
};

/** What a read of the engine came to: nothing yet, its answer, or its own refusal. */
type Read<T> = { state: 'reading' } | { state: 'read'; data: T } | { state: 'failed'; why: string };

/**
 * Asks the engine once per `params` and holds the answer in this component
 * alone. Not `useRpc`: what comes back is device output, read to be looked
 * at once, and a cached copy would be shown again minutes later as if it
 * were what the session holds now.
 */
function useRead<T>(method: string, params: Record<string, unknown> | null): Read<T> {
  const key = params === null ? null : JSON.stringify(params);
  const [read, setRead] = useState<Read<T>>({ state: 'reading' });
  useEffect(() => {
    if (key === null) return;
    let current = true;
    setRead({ state: 'reading' });
    rpc<T>(method, JSON.parse(key)).then(
      (data) => {
        if (current) setRead({ state: 'read', data });
      },
      (error: unknown) => {
        if (current) setRead({ state: 'failed', why: refusal(error).message });
      },
    );
    return () => {
      current = false;
    };
  }, [method, key]);
  return read;
}

/** How long the number of lines has to stand still before the session is read again. */
const READ_AFTER_MS = 250;

/** The text of a read, or what stands in for it while there is none. */
function ReadText({ read, empty }: { read: Read<{ text: string; redacted: boolean }>; empty: string }) {
  if (read.state === 'reading') {
    return (
      <p className={styles.note} role="status">
        Reading it from the engine…
      </p>
    );
  }
  if (read.state === 'failed') {
    return (
      <p className={styles.refused} role="alert">
        <b>It could not be read to show it here.</b> {read.why}
      </p>
    );
  }
  if (typeof read.data?.text !== 'string') {
    return <p className={styles.note}>The engine answered with something this dialog cannot show as text.</p>;
  }
  if (read.data.text === '') return <p className={styles.note}>{empty}</p>;
  return (
    <>
      <CodeBlock code={read.data.text} copyable={false} maxHeight={160} />
      <p className={styles.note}>
        {read.data.redacted
          ? 'Secrets in it are already replaced. '
          : 'The engine found nothing to replace in it. '}
        When it is kept the engine reads it again and the evidence store’s own rules run on top, so the item can
        hide more than this and never less.
      </p>
    </>
  );
}

/** The last lines of a session, as the engine reads them now. */
function TerminalPreview({ sessionId, lines }: { sessionId: string; lines: number | null }) {
  const [settled, setSettled] = useState(lines);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(lines), READ_AFTER_MS);
    return () => clearTimeout(timer);
  }, [lines]);
  const read = useRead<{ text: string; redacted: boolean }>(
    'terminal.snapshot',
    settled === null ? null : { session_id: sessionId, lines: settled },
  );
  if (lines === null) return null;
  return (
    <section className={styles.preview} aria-label="What the engine reads now" data-testid="keep-preview">
      <h3 className={styles.previewTitle}>What the engine reads now</h3>
      <ReadText read={read} empty="The session has printed nothing yet." />
      <p className={styles.note}>
        If the session prints more before you press Keep, the item holds its last lines as they are then.
      </p>
    </section>
  );
}

/** A page of a session log, as the engine reads it. */
function LogPreview({ request }: { request: LogEvidenceRequest }) {
  const read = useRead<{ text: string; redacted: boolean }>('terminal.logs.read', {
    name: request.name,
    offset: request.offset,
    limit: request.limit,
  });
  return (
    <section className={styles.preview} aria-label="What the engine reads" data-testid="keep-preview">
      <h3 className={styles.previewTitle}>What the engine reads</h3>
      <ReadText read={read} empty="There is nothing at that place in the log." />
    </section>
  );
}

/** Text the person holds: shown as it will be sent, and said to be theirs. */
function TextPreview({ request }: { request: TextEvidenceRequest }) {
  return (
    <section className={styles.preview} aria-label="What will be sent" data-testid="keep-preview">
      <h3 className={styles.previewTitle}>
        What will be sent <Badge tone="warn">Supplied by the operator</Badge>
      </h3>
      <CodeBlock code={request.text} copyable={false} maxHeight={160} />
      <p className={styles.note}>
        This is the text as you hold it, not yet redacted: the engine replaces the secrets it recognises when the
        text arrives, before anything is stored, and you are then shown exactly what it kept.
      </p>
    </section>
  );
}

/** What the last-lines field holds, as a number of lines — or null when it is not one. */
function wholeLines(text: string): number | null {
  if (!/^\d+$/.test(text.trim())) return null;
  return Number(text.trim());
}

export interface KeepAsEvidenceDialogProps {
  /** What to keep. `null` renders nothing. */
  request: KeepRequest | null;
  onClose: () => void;
  /** Called once the item is stored, with the item as the engine returned it (without its body). */
  onKept?: (item: Evidence) => void;
  /** An incident to offer first in "Attach to", e.g. the one the screen is about. */
  incidentId?: string;
}

/** The dialog alone, for a screen that keeps its own state. Most screens want {@link useKeepAsEvidence}. */
export function KeepAsEvidenceDialog({ request, onClose, onKept, incidentId }: KeepAsEvidenceDialogProps) {
  if (request === null) return null;
  // Keyed by what is asked for, so a second request starts with an empty form.
  return (
    <Form key={JSON.stringify(request)} request={request} onClose={onClose} onKept={onKept} incidentId={incidentId} />
  );
}

interface FormProps extends KeepAsEvidenceDialogProps {
  request: KeepRequest;
}

function Form({ request, onClose, onKept, incidentId }: FormProps) {
  const methods = useMethods();
  const canAttach = methods.has('evidence.attach') && methods.has('incidents.list');
  const incidents = useRpc<IncidentsListResult>(
    'incidents.list',
    { state: 'unresolved', limit: 200 },
    { enabled: canAttach, invalidateOn: ['incidents.changed'] },
  );

  const [title, setTitle] = useState(request.title ?? '');
  /** How many lines of a session to keep, as typed. The engine is the judge of the range. */
  const [linesText, setLinesText] = useState(request.kind === 'terminal' ? String(request.lines) : '');
  const lines = request.kind === 'terminal' ? wholeLines(linesText) : null;
  /** Text a person handed in, once it is stored: shown so they can see what the redaction left. */
  const [stored, setStored] = useState<{ item: EvidenceWithBody; attached: boolean } | null>(null);
  const [attachTo, setAttachTo] = useState(incidentId ?? '');
  const [author, setAuthor] = useAuthor();
  const [pending, setPending] = useState(false);
  const [refused, setRefused] = useState<Refusal | null>(null);
  /** Stored, but the attachment that was asked for did not happen. */
  const [unattached, setUnattached] = useState<{ item: Evidence; why: string } | null>(null);

  const about = ABOUT[request.kind];
  const open = Array.isArray(incidents.data?.incidents) ? incidents.data.incidents : [];

  const finish = (item: Evidence, attached: boolean) => {
    toast.success(attached ? 'Kept as evidence and attached' : 'Kept as evidence', {
      description: `${item.title} · sha256 ${item.sha256.slice(0, 12)}…`,
      action: {
        label: 'Open',
        // A hash route, set directly: this dialog is also used where there is
        // no router above it to ask.
        onSelect: () => {
          window.location.hash = evidenceLink(item.id);
        },
      },
    });
    onKept?.(item);
    onClose();
  };

  const attach = async (item: Evidence): Promise<Evidence | null> => {
    try {
      return await rpc<Evidence>('evidence.attach', {
        id: item.id,
        incident_id: attachTo,
        ...(author.trim() === '' ? {} : { author: author.trim() }),
      });
    } catch (error) {
      setUnattached({ item, why: refusal(error).message });
      return null;
    }
  };

  const keep = async () => {
    setPending(true);
    setRefused(null);
    try {
      const call = captureCall(request.kind === 'terminal' && lines !== null ? { ...request, lines } : request, title);
      const answer = await rpc<EvidenceWithBody>(call.method, call.params);
      // The body comes back with the item; it is not kept here.
      const { body, ...item } = answer;
      const attached = attachTo === '' ? null : await attach(item);
      if (attachTo !== '' && attached === null) return;
      if (request.kind === 'text' && body?.type === 'text') {
        // Text a person handed in could not be shown redacted beforehand:
        // the engine redacts on arrival. So it is shown now, as stored.
        setStored({ item: { ...(attached ?? item), body }, attached: attached !== null });
        onKept?.(attached ?? item);
        return;
      }
      finish(attached ?? item, attached !== null);
    } catch (error) {
      setRefused(refusal(error));
    } finally {
      setPending(false);
    }
  };

  const discard = async () => {
    if (!stored) return;
    setPending(true);
    try {
      await rpc('evidence.delete', { id: stored.item.id });
      toast.success('Deleted', { description: `“${stored.item.title}” is no longer kept.` });
      onClose();
    } catch (error) {
      setRefused(refusal(error));
    } finally {
      setPending(false);
    }
  };

  if (stored) {
    const { item } = stored;
    return (
      <Dialog
        open
        onClose={onClose}
        title={stored.attached ? 'Kept and attached' : 'Kept as evidence'}
        description="This is what was stored: the text as the engine redacted it on arrival. It is labelled as supplied by the operator."
        size="md"
        dismissible={!pending}
        footer={
          <>
            {methods.has('evidence.delete') && !stored.attached && (
              <Button variant="ghost" icon={Trash} loading={pending} onClick={() => void discard()}>
                Delete it
              </Button>
            )}
            <Button
              variant="ghost"
              disabled={pending}
              onClick={() => {
                window.location.hash = evidenceLink(item.id);
                onClose();
              }}
            >
              Open in Evidence
            </Button>
            <Button variant="primary" data-autofocus disabled={pending} onClick={onClose}>
              Done
            </Button>
          </>
        }
      >
        <div className={styles.body} data-testid="keep-stored">
          <KeyValue
            items={[
              { label: 'Title', value: item.title },
              {
                label: 'Stored',
                value: `${plural(item.lines ?? 0, 'line')}, ${formatCount(item.bytes)} bytes, sha256 ${item.sha256.slice(0, 12)}…`,
              },
              { label: 'Source', value: 'supplied by the operator, redacted on arrival' },
            ]}
          />
          <CodeBlock code={item.body.type === 'text' ? item.body.text : ''} copyable={false} maxHeight={260} />
          <p className={styles.note}>
            If something that should be hidden is still readable above, delete the item: what the redaction did not
            recognise is stored as it is.
            {stored.attached ? ' It is on the incident’s timeline; it can be deleted under Operations → Evidence.' : ''}
          </p>
          {refused && (
            <p className={styles.refused} role="alert" data-testid="keep-refused">
              <b>{refused.headline}.</b> {refused.message}
            </p>
          )}
        </div>
      </Dialog>
    );
  }

  if (unattached) {
    return (
      <Dialog
        open
        onClose={onClose}
        title="Kept, but not attached"
        description="The evidence is stored. Attaching it to the incident is what failed."
        size="md"
        footer={
          <>
            <Button variant="ghost" onClick={onClose}>
              Leave it unattached
            </Button>
            <Button
              variant="primary"
              loading={pending}
              onClick={async () => {
                setPending(true);
                const attached = await attach(unattached.item);
                if (attached) finish(attached, true);
                setPending(false);
              }}
            >
              Try attaching again
            </Button>
          </>
        }
      >
        <div className={styles.body}>
          <p className={styles.refused} role="alert">
            <b>It could not be attached.</b> {unattached.why}
          </p>
          <p className={styles.note}>
            “{unattached.item.title}” is under Operations → Evidence, where it can be attached later.
          </p>
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog
      open
      onClose={onClose}
      title="Keep as evidence"
      description={about.sentence}
      size="md"
      dismissible={!pending}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={pending}>
            Cancel
          </Button>
          <Button
            variant="primary"
            icon={FileCheck2}
            loading={pending}
            // The engine requires a title for text a person hands in, and a
            // number of lines is needed to ask for any.
            disabled={(request.kind === 'text' && title.trim() === '') || (request.kind === 'terminal' && lines === null)}
            onClick={() => void keep()}
          >
            {attachTo === '' ? 'Keep' : 'Keep and attach'}
          </Button>
        </>
      }
    >
      <div className={styles.body}>
        <KeyValue
          items={describeRequest(request.kind === 'terminal' && lines !== null ? { ...request, lines } : request).map(
            (fact) => ({ label: fact.label, value: fact.value }),
          )}
        />
        {request.kind === 'terminal' && (
          <Field
            label="Lines from the end"
            hint="1 to 10,000. For more of a session, keep pages of its log."
            error={lines === null ? 'A whole number of lines.' : undefined}
          >
            <Input
              size="sm"
              inputMode="numeric"
              value={linesText}
              onChange={(event) => setLinesText(event.target.value)}
            />
          </Field>
        )}
        {request.kind === 'terminal' && <TerminalPreview sessionId={request.session_id} lines={lines} />}
        {request.kind === 'log' && <LogPreview request={request} />}
        {request.kind === 'text' && <TextPreview request={request} />}
        <Field
          label="Title"
          required={request.kind === 'text'}
          hint={
            request.kind === 'text'
              ? 'Say what it is: the engine did not read it, so it cannot name it.'
              : 'Left empty, the engine names it after what it read.'
          }
        >
          <Input
            data-autofocus
            value={title}
            maxLength={200}
            placeholder="What this shows, in a few words"
            onChange={(event) => setTitle(event.target.value)}
          />
        </Field>
        {canAttach && (
          <Field
            label="Attach to an incident"
            hint={
              incidents.isError
                ? `The incidents could not be read (${incidents.error.message}); it can be attached later, under Evidence.`
                : 'It goes on that incident’s timeline at the time it was captured, as a reference.'
            }
          >
            <Select value={attachTo} onChange={(event) => setAttachTo(event.target.value)}>
              <option value="">Do not attach</option>
              {open.map((incident) => (
                <option key={incident.id} value={incident.id}>
                  {incident.title}
                </option>
              ))}
              {/* An incident the screen named that is not among the unresolved ones is still a choice. */}
              {incidentId && !open.some((incident) => incident.id === incidentId) && (
                <option value={incidentId}>This incident</option>
              )}
            </Select>
          </Field>
        )}
        {attachTo !== '' && (
          <Field label="Your name" hint="Shown on the timeline entry. Remembered in this browser.">
            <Input
              size="sm"
              value={author}
              placeholder="who is attaching this"
              onChange={(event) => setAuthor(event.target.value)}
            />
          </Field>
        )}
        <p className={styles.note}>
          Secrets are replaced before it is stored, always. What was replaced cannot be recovered from the item.
        </p>
        {refused && (
          <p className={styles.refused} role="alert" data-testid="keep-refused">
            <b>{refused.headline}.</b> {refused.message}
          </p>
        )}
      </div>
    </Dialog>
  );
}

export interface KeepAsEvidence {
  /**
   * Whether this build can keep that kind. False while `app.methods` has not
   * answered: an entry that appears and then vanishes is worse than one that
   * arrives a moment late.
   */
  can: (kind: KeepRequest['kind']) => boolean;
  /** Opens the dialog on a request. Does nothing in a build that cannot keep it. */
  keep: (request: KeepRequest) => void;
  /**
   * The "Keep as evidence…" entry for a chart's export menu, or nothing in a
   * build without `evidence.capture_chart`. Give it a function when part of
   * the request is "now" (a chart whose range runs to the present): it is
   * called when the entry is chosen, not when the menu is built.
   */
  chartEntries: (request: ChartEvidenceRequest | (() => ChartEvidenceRequest)) => MenuAction[];
  /** Render once, anywhere in the screen. */
  dialog: ReactNode;
}

/** State and dialog for "Keep as evidence…", for one screen. */
export function useKeepAsEvidence(options: { incidentId?: string; onKept?: (item: Evidence) => void } = {}): KeepAsEvidence {
  const methods = useMethods();
  const [request, setRequest] = useState<KeepRequest | null>(null);
  const can = (kind: KeepRequest['kind']) => methods.has(CAPTURE_METHOD[kind]);
  const keep = (next: KeepRequest) => {
    if (can(next.kind)) setRequest(next);
  };

  return {
    can,
    keep,
    chartEntries: (wanted) =>
      can('chart')
        ? [
            {
              label: 'Keep as evidence…',
              icon: FileCheck2,
              onSelect: () => keep(typeof wanted === 'function' ? wanted() : wanted),
            },
          ]
        : [],
    dialog: (
      <KeepAsEvidenceDialog
        request={request}
        onClose={() => setRequest(null)}
        onKept={options.onKept}
        incidentId={options.incidentId}
      />
    ),
  };
}
