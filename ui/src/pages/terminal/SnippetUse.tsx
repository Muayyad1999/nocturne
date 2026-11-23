/**
 * One snippet, about to be sent: its variables as a form, the exact text
 * that will be typed, and where it goes.
 *
 * The text on screen is never put together here. It is
 * `terminal.snippets.render`'s answer for the values in the form, and when
 * the engine refuses a value its sentence goes beside the field it is about
 * — a line break in a value, a control character, a value that is too long.
 * None of that is checked on this side: a second copy of the rule would
 * sooner or later disagree with the first.
 *
 * For the same reason a field does not tidy what is put into it. A browser's
 * single-line input turns every line break of a pasted text into a space
 * before anyone sees it, U+2028 included; the fields here keep the text as it
 * arrived, so a value with a second line in it is refused by the engine, in
 * the engine's words, instead of being quietly joined into one.
 *
 * Sending is `sendSnippet` (snippetSend.ts): the paste sheet's question when
 * the text is worth one, then `terminal.snippets.send` held to the hash of
 * what is on screen.
 */

import { ArrowLeft, Send } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { rpc } from '@/api/client';
import type { Snippet, SnippetRendered } from '@/api/types/terminal';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { CodeBlock } from '@/design/CodeBlock';
import { Field } from '@/design/Field';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { plural } from '@/lib/format';
import styles from './SnippetsDrawer.module.css';
import { snippetRoad } from './snippetRoad';
import { sendSnippet } from './snippetSend';
import {
  audience,
  changedHash,
  initialFields,
  isRequired,
  notChosenBecause,
  refusedSessions,
  renderRefusal,
  sendReport,
  unfilled,
  valuesFor,
  type RenderRefusal,
  type SendReport,
  type SendTarget,
} from './snippets';
import { useFocusWhenShown } from './viewFocus';

/** How long the form waits after a key before asking the engine again. */
const RENDER_AFTER_MS = 150;

/** The rows a field needs to show what it holds: one, unless a line break was pasted into it. */
function fieldRows(value: string): number {
  return value.split(/\r\n?|\n|\u2028|\u2029/).length;
}

export interface SnippetUseProps {
  snippet: Snippet;
  /** Every snippet the list holds, to name the variant that stands in for this one. */
  all: readonly Snippet[];
  /** The sessions that are open right now. */
  targets: readonly SendTarget[];
  /** The sessions ticked when the form opens. */
  initial: readonly string[];
  onBack: () => void;
  /** Every session that was asked for took the text. */
  onSent: (sessionId: string) => void;
}

type Shown =
  | { state: 'asking' }
  | { state: 'shown'; rendered: SnippetRendered; values: Record<string, string> }
  | { state: 'refused'; refusal: RenderRefusal };

export function SnippetUse({ snippet, all, targets, initial, onBack, onSent }: SnippetUseProps) {
  const [fields, setFields] = useState<Record<string, string>>(() => initialFields(snippet.variables));
  const [ticked, setTicked] = useState<readonly string[]>(initial);
  const [shown, setShown] = useState<Shown>({ state: 'asking' });
  const [sending, setSending] = useState(false);
  const [report, setReport] = useState<SendReport | null>(null);
  /** Why the last send was refused: the engine's sentence, and the sessions it named. */
  const [refused, setRefused] = useState<{ message: string; sessions: Record<string, string> } | null>(null);
  /** Counts up to have the engine asked again for the same values. */
  const [again, setAgain] = useState(0);
  /**
   * Whether this form is still on screen. The question can stay open for as
   * long as the person likes, and when the drawer is closed or the snippet
   * is edited meanwhile (the drawer keys the form by the edit), an answer
   * arrives for a form that is no longer there to show it.
   */
  const onScreen = useRef(true);
  useEffect(() => {
    onScreen.current = true;
    return () => {
      onScreen.current = false;
    };
  }, []);
  /**
   * A send is on its way. Held in a ref, not read from `sending`: a second
   * Enter can arrive before the first one's render, and the spinner on the
   * button does not stop a key pressed in a field. One read-only line into
   * one session asks nothing, so without this a key held a moment too long
   * types the snippet twice.
   */
  const inFlight = useRef(false);
  const rootRef = useRef<HTMLDivElement>(null);
  useFocusWhenShown(rootRef);

  const values = useMemo(() => valuesFor(snippet.variables, fields), [snippet.variables, fields]);
  const valuesKey = JSON.stringify(values);

  // The preview is the engine's. Values are held in this component and sent
  // straight to it: not through the query cache, where a value typed for a
  // password variable would outlive the form.
  useEffect(() => {
    let current = true;
    setShown({ state: 'asking' });
    const asked = JSON.parse(valuesKey) as Record<string, string>;
    const timer = setTimeout(() => {
      rpc<SnippetRendered>('terminal.snippets.render', { id: snippet.id, values: asked }).then(
        (rendered) => {
          if (current) setShown({ state: 'shown', rendered, values: asked });
        },
        (error: unknown) => {
          if (current) setShown({ state: 'refused', refusal: renderRefusal(error, snippet.variables) });
        },
      );
    }, RENDER_AFTER_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [snippet.id, snippet.updated_at, snippet.variables, valuesKey, again]);

  // A session that closed while the form was open cannot stay ticked.
  const open = useMemo(() => new Set(targets.map((target) => target.sessionId)), [targets]);
  const chosen = targets.filter((target) => ticked.includes(target.sessionId) && open.has(target.sessionId));

  const refusal = shown.state === 'refused' ? shown.refusal : null;
  const waitingFor = unfilled(snippet.variables, fields);
  const first = chosen[0];
  const where = chosen.length === 1 && first ? first.title : plural(chosen.length, 'session');

  const send = async () => {
    if (inFlight.current || shown.state !== 'shown' || chosen.length === 0) return;
    inFlight.current = true;
    setSending(true);
    setReport(null);
    setRefused(null);
    try {
      const outcome = await sendSnippet(snippetRoad, {
        id: snippet.id,
        values: shown.values,
        shown: shown.rendered,
        targets: chosen,
      });
      if (outcome.kind !== 'sent') return;
      const told = sendReport(outcome.result, chosen);
      if (told.complete && first) {
        toast.success(told.headline, { description: 'Nothing pressed Enter for you: the last line is at the prompt.' });
        onSent(first.sessionId);
        return;
      }
      // Some took it and some did not: that cannot be taken back, so it stays on screen.
      setReport(told);
      if (!onScreen.current) {
        toast.error(told.headline, {
          description: told.sessions
            .filter((entry) => !entry.ok)
            .map((entry) => `${entry.title}: ${entry.says}`)
            .join(' '),
        });
      }
    } catch (error) {
      const sessions: Record<string, string> = {};
      for (const entry of refusedSessions(error)) sessions[entry.session_id] = entry.message;
      const message = error instanceof Error ? error.message : 'The snippet was not sent.';
      const changed = changedHash(error) !== null;
      // A snippet that was edited under the question gets a new form — it
      // may be on screen already, or a moment after this — and the line
      // below would go with the old one. A refusal has to outlast that.
      if (changed || !onScreen.current) toast.error(`“${snippet.name}” was not sent`, { description: message });
      setRefused({ message, sessions });
      // The snippet or a value no longer gives the text on screen: show what it gives now.
      if (changed) setAgain((count) => count + 1);
    } finally {
      inFlight.current = false;
      setSending(false);
    }
  };

  return (
    <div ref={rootRef} className={styles.use} data-testid="snippet-use">
      <div className={styles.useHead}>
        <Button size="sm" variant="ghost" icon={ArrowLeft} onClick={onBack}>
          All snippets
        </Button>
      </div>
      <h3 className={styles.useName}>
        {snippet.name}
        <Badge variant="outline">{audience(snippet)}</Badge>
      </h3>
      {snippet.chosen === false && <p className={styles.note}>{notChosenBecause(snippet, all)}</p>}

      {snippet.variables.length > 0 && (
        <form className={styles.variables} aria-label="Variables" onSubmit={(event) => event.preventDefault()}>
          {snippet.variables.map((variable, index) => {
            const fault = refusal?.fields[variable.name];
            // "It has no value yet" is where every form starts, not a mistake.
            const waiting = fault?.problem === 'missing';
            const fallback = isRequired(variable)
              ? 'Required: it has no default.'
              : variable.default === ''
                ? 'May be left empty.'
                : `Default: ${variable.default}`;
            return (
              <Field
                key={variable.name}
                label={variable.name}
                required={isRequired(variable)}
                hint={fault && waiting ? fault.sentence : fallback}
                error={fault && !waiting ? fault.sentence : undefined}
              >
                <Textarea
                  mono
                  // One line, as a value is; it grows only to show a pasted
                  // text that has more, which the engine then refuses.
                  rows={Math.min(4, fieldRows(fields[variable.name] ?? ''))}
                  className={styles.value}
                  data-autofocus={index === 0 || undefined}
                  autoComplete="off"
                  spellCheck={false}
                  value={fields[variable.name] ?? ''}
                  onChange={(event) => setFields({ ...fields, [variable.name]: event.target.value })}
                  onKeyDown={(event) => {
                    // Enter sends, as in any form of one-line fields, and is
                    // never a line break in a value. Not while an input
                    // method is composing: that Enter picks a candidate.
                    if (event.key !== 'Enter' || event.nativeEvent.isComposing) return;
                    event.preventDefault();
                    void send();
                  }}
                />
              </Field>
            );
          })}
        </form>
      )}

      <section className={styles.exact} aria-label="What will be typed">
        <h4 className={styles.exactTitle}>What will be typed</h4>
        {shown.state === 'shown' ? (
          <>
            <CodeBlock code={shown.rendered.text} lineNumbers copyable={false} wrap maxHeight={240} />
            <p className={styles.note}>
              {plural(shown.rendered.lines, 'line')}, exactly as shown. Nothing presses Enter after the last one.
            </p>
          </>
        ) : shown.state === 'asking' ? (
          <p className={styles.note} role="status">
            Asking the engine what this types…
          </p>
        ) : (
          <p className={styles.note} role="status" data-testid="snippet-nothing-yet">
            {waitingFor.length > 0 && refusal?.general === null
              ? `Nothing yet: ${waitingFor.join(', ')} ${waitingFor.length === 1 ? 'needs' : 'need'} a value.`
              : 'Nothing: the engine will not type this as it is filled in.'}
            {refusal?.general ? ` ${refusal.general}` : ''}
          </p>
        )}
      </section>

      <fieldset className={styles.targets}>
        <legend className={styles.exactTitle}>Send to</legend>
        {targets.length === 0 ? (
          <p className={styles.note}>No session is open. Open one to send this.</p>
        ) : (
          targets.map((target) => (
            <div key={target.sessionId} className={styles.target}>
              <Checkbox
                checked={ticked.includes(target.sessionId)}
                onChange={(event) =>
                  setTicked(
                    event.target.checked
                      ? [...ticked, target.sessionId]
                      : ticked.filter((id) => id !== target.sessionId),
                  )
                }
              >
                {target.title}
              </Checkbox>
              {refused?.sessions[target.sessionId] && (
                <span className={styles.targetSays} data-ok="false">
                  {refused.sessions[target.sessionId]}
                </span>
              )}
              {report?.sessions
                .filter((entry) => entry.sessionId === target.sessionId)
                .map((entry) => (
                  <span key={entry.sessionId} className={styles.targetSays} data-ok={entry.ok}>
                    {entry.says}
                  </span>
                ))}
            </div>
          ))
        )}
        {chosen.length > 1 && (
          <p className={styles.note}>
            The same lines go to all {chosen.length}, and you are asked first. Either every session takes them or
            none is sent anything.
          </p>
        )}
      </fieldset>

      {refused && (
        <p className={styles.refused} role="alert" data-testid="snippet-refused">
          <b>Nothing was sent.</b> {refused.message}
        </p>
      )}
      {report && (
        <p className={styles.refused} role="alert" data-testid="snippet-partial">
          <b>{report.headline}.</b> What a session took cannot be taken back; the ones that did not are named above.
        </p>
      )}

      <div className={styles.formActions}>
        <Button
          variant="primary"
          icon={Send}
          loading={sending}
          disabled={shown.state !== 'shown' || shown.rendered.text === '' || chosen.length === 0}
          onClick={() => void send()}
        >
          {chosen.length === 0 ? 'Type it in' : `Type into ${where}`}
        </Button>
      </div>
    </div>
  );
}
