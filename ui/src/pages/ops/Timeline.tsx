/**
 * The incident timeline: the centre of this screen.
 *
 * Four sources end up in one column, each line at the time the thing itself
 * happened rather than the time anybody read it:
 *
 * - notes a person typed;
 * - the linked alerts' and changes' own events, gathered by the backend;
 * - the incident's own state changes;
 * - and, as dashed lines nobody has adopted yet, the alerts, changes and
 *   configuration movement that `incidents.suggest_links` found inside the
 *   window. A configuration change four minutes before the outage is only
 *   useful where it can be seen four minutes before the outage, so it is
 *   shown there, with the button that links it.
 *
 * Two kinds of line are not events and are drawn so they cannot be mistaken
 * for one: a **gap**, which is a source that could not be read and means the
 * lines around it may be incomplete, and a **suggestion**, which is an offer.
 *
 * **Evidence** sits on the timeline too, at the time it was captured: an
 * entry that is a reference to the item (it opens it), never its content.
 * When an item has been deleted the entry stays as a **tombstone** — the
 * engine's own sentence saying that evidence was here and when it went, with
 * the hash of what it was in the provenance. It is drawn as what it is: not
 * a link, not an event, and not a hole.
 *
 * Every line carries its evidence — which record and which of its fields it
 * came from — behind a disclosure, because this timeline ends up in a review
 * document that somebody will be asked to believe.
 *
 * The composer is not behind a button. It sits under the scroll area, always
 * on screen, and takes a note on Ctrl/⌘+Enter as well as from the button:
 * an engineer typing into this while something is broken should not have to
 * aim at anything.
 */

import { CornerDownLeft, FileCheck2, FileX2, Link2, Plus, Search, TriangleAlert, Unlink } from 'lucide-react';
import { useLayoutEffect, useRef, useState } from 'react';
import { Link } from 'react-router';
import type { Incident, IncidentsSuggestResult, TimelineEntry } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { LabPill } from '@/design/StatusPill';
import { Textarea } from '@/design/Textarea';
import { TimeAgo } from '@/design/TimeAgo';
import { Tooltip } from '@/design/Tooltip';
import { formatCount, formatDuration, plural } from '@/lib/format';
import { formatDate, formatTime, formatTimestamp } from '@/lib/time';
import { UnavailableNote } from './Unavailable';
import { attachedEvidenceId, isTombstone } from './evidenceReading';
import {
  ENTRY_TONE,
  ENTRY_WORD,
  entryLink,
  instant,
  mergeTimeline,
  suggestions,
  timelineDays,
  type Suggestion,
  type TimelineRow,
} from './ops';
import styles from './Timeline.module.css';

export interface TimelineProps {
  incident: Incident;
  /** What nobody has linked yet. `undefined` while it is still being looked for. */
  suggested: IncidentsSuggestResult | undefined;
  suggestionsPending: boolean;
  /** Adds a note. `at` is when the thing being noted happened. */
  onNote: (text: string, at: number | null) => void;
  noting: boolean;
  /** Adopts a suggestion: the ids go to `incidents.link`. */
  onLink: (link: { alert_ids?: string[]; change_ids?: string[] }) => void;
  linking: boolean;
  /**
   * Opens "Attach evidence…". Absent in a build without `evidence.*`: the
   * button is then not shown at all.
   */
  onAttachEvidence?: () => void;
  /** Takes an attached item off this timeline. The item itself stays. */
  onDetachEvidence?: (evidenceId: string) => void;
  /** The evidence id being detached right now, for the button's spinner. */
  detaching?: string | null;
}

export function Timeline({
  incident,
  suggested,
  suggestionsPending,
  onNote,
  noting,
  onLink,
  linking,
  onAttachEvidence,
  onDetachEvidence,
  detaching = null,
}: TimelineProps) {
  const offered = suggestions(suggested);
  const rows = mergeTimeline(incident.timeline, offered);
  const days = timelineDays(rows, (at) => formatDate(at));
  const gaps = incident.timeline.filter((entry) => entry.kind === 'gap');
  const tombstones = incident.timeline.filter(isTombstone).length;

  return (
    <section className={styles.timeline} aria-label="Incident timeline">
      <header className={styles.head}>
        <h3 className={styles.heading}>Timeline</h3>
        <span className={styles.count}>
          {plural(incident.timeline.length, 'entry', 'entries')}
          {offered.length > 0 && ` · ${formatCount(offered.length)} offered`}
        </span>
        {gaps.length > 0 && (
          <Badge tone="warn" variant="outline" title={gaps.map((gap) => gap.text).join('\n')}>
            <Icon icon={TriangleAlert} size={12} />
            {plural(gaps.length, 'gap')}
          </Badge>
        )}
        {tombstones > 0 && (
          <Badge
            tone="idle"
            variant="outline"
            title="Evidence that was attached here and has since been deleted. Each line says when, and keeps the hash of what it was."
          >
            <Icon icon={FileX2} size={12} />
            {plural(tombstones, 'deleted item')}
          </Badge>
        )}
        {suggestionsPending && <span className={styles.count}>looking for what else happened…</span>}
        {onAttachEvidence && (
          <Button size="sm" variant="secondary" icon={FileCheck2} className={styles.attach} onClick={onAttachEvidence}>
            Attach evidence…
          </Button>
        )}
      </header>

      {gaps.length > 0 && (
        <p className={styles.gapWarning} role="status">
          {plural(gaps.length, 'source')} could not be read while this timeline was gathered, so it
          may be missing entries. Each one is marked below at the moment of the failed read.
        </p>
      )}

      <Scroller
        rows={rows}
        days={days}
        incident={incident}
        onLink={onLink}
        linking={linking}
        onDetachEvidence={onDetachEvidence}
        detaching={detaching}
      />

      {suggested && suggested.unavailable.length > 0 && (
        <div className={styles.suggestGaps}>
          {suggested.unavailable.map((entry) => (
            <UnavailableNote
              key={entry.section}
              size="inline"
              about={`Looking for unlinked ${entry.section.replace('_', ' ')}`}
              reason={entry.reason}
              message={entry.message}
            />
          ))}
        </div>
      )}

      <Composer onNote={onNote} noting={noting} startedAt={incident.started_at} />
    </section>
  );
}

interface ScrollerProps {
  rows: readonly TimelineRow[];
  days: readonly { day: string; rows: TimelineRow[] }[];
  incident: Incident;
  onLink: (link: { alert_ids?: string[]; change_ids?: string[] }) => void;
  linking: boolean;
  onDetachEvidence?: (evidenceId: string) => void;
  detaching: string | null;
}

function Scroller({ rows, days, incident, onLink, linking, onDetachEvidence, detaching }: ScrollerProps) {
  const box = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const count = rows.length;

  // Oldest first, so the newest line is at the bottom next to the composer —
  // and the view follows it, but only while the reader is already there: an
  // engineer reading the start of an outage must not be yanked away by a note
  // somebody else just added.
  useLayoutEffect(() => {
    const element = box.current;
    if (element && atBottom.current) element.scrollTop = element.scrollHeight;
  }, [count]);

  if (count === 0) {
    return (
      <div className={styles.scroll} ref={box}>
        <p className={styles.empty}>
          Nothing is on the timeline yet. A note you write below goes on it; linking an alert or a
          change brings that record’s own events with it, at their own times.
        </p>
      </div>
    );
  }

  return (
    <div
      className={styles.scroll}
      ref={box}
      onScroll={(event) => {
        const element = event.currentTarget;
        atBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
      }}
    >
      <ol className={styles.list}>
        {days.map((day) => (
          <li key={day.day}>
            <p className={styles.day}>{day.day}</p>
            <ol className={styles.list}>
              {day.rows.map((row) =>
                row.kind === 'entry' ? (
                  <EntryRow
                    key={row.key}
                    entry={row.entry}
                    deviceIds={incident.device_ids}
                    onDetachEvidence={onDetachEvidence}
                    detaching={detaching}
                  />
                ) : (
                  <SuggestionRow
                    key={row.key}
                    suggestion={row.suggestion}
                    onLink={onLink}
                    linking={linking}
                  />
                ),
              )}
            </ol>
          </li>
        ))}
      </ol>
    </div>
  );
}

function EntryRow({
  entry,
  deviceIds,
  onDetachEvidence,
  detaching,
}: {
  entry: TimelineEntry;
  deviceIds: readonly string[];
  onDetachEvidence?: (evidenceId: string) => void;
  detaching: string | null;
}) {
  const at = instant(entry.ts);
  const gap = entry.kind === 'gap';
  // An evidence entry without a reference is what is left of a deleted item.
  // It has nowhere to go, and it must not look like a line that does.
  const tombstone = isTombstone(entry);
  const evidenceId = attachedEvidenceId(entry);
  const to = tombstone ? null : entryLink(entry, deviceIds);
  return (
    <li
      className={styles.entry}
      data-testid="timeline-row"
      data-row={entry.id}
      data-kind={entry.kind}
      data-tone={tombstone ? 'idle' : ENTRY_TONE[entry.kind]}
      data-tombstone={tombstone ? '' : undefined}
    >
      <Tooltip content={formatTimestamp(entry.ts)} describe={false}>
        <time className={styles.when} dateTime={at ? new Date(at).toISOString() : undefined}>
          {at === null ? '—' : formatTime(at)}
        </time>
      </Tooltip>
      <span className={styles.kind}>{tombstone ? 'Deleted' : (ENTRY_WORD[entry.kind] ?? entry.kind)}</span>
      <div className={styles.body}>
        <p className={styles.text}>
          {tombstone && <Icon icon={FileX2} size={13} className={styles.tombstoneIcon} />}
          {to ? (
            <Link to={to} className={styles.textLink}>
              {entry.text}
            </Link>
          ) : (
            entry.text
          )}
        </p>
        <p className={styles.meta}>
          {entry.author ? (
            <span>{entry.author}</span>
          ) : (
            <span
              className={styles.muted}
              title="The record this came from does not say who did it; the core's audit trail keeps no actor."
            >
              who is not recorded
            </span>
          )}
          <Evidence entry={entry} />
          {gap && <span className={styles.gapMark}>the lines around this may be incomplete</span>}
          {tombstone && (
            <span className={styles.tombstoneMark}>
              the item is gone; its hash is in where this came from
            </span>
          )}
          {evidenceId !== null && onDetachEvidence && (
            <Button
              size="sm"
              variant="ghost"
              icon={Unlink}
              loading={detaching === evidenceId}
              title="Takes this entry off the timeline. The evidence itself stays under Evidence."
              onClick={() => onDetachEvidence(evidenceId)}
            >
              Detach
            </Button>
          )}
        </p>
      </div>
    </li>
  );
}

/**
 * Where a line came from. Collapsed, because nobody reads it until they doubt
 * a line — and then it is the only thing they want.
 */
function Evidence({ entry }: { entry: TimelineEntry }) {
  const gathered = instant(entry.source.gathered_at);
  const late = gathered !== null && instant(entry.ts) !== null && gathered - entry.ts > 60_000;
  return (
    <details className={styles.evidence}>
      <summary>
        <Icon icon={Search} size={11} />
        {entry.source.from}
      </summary>
      <div className={styles.evidenceBody}>
        <p>
          <span className={styles.muted}>Read from</span> <code>{entry.source.from}</code>
        </p>
        <p>
          <span className={styles.muted}>Evidence</span> <code>{entry.source.evidence}</code>
        </p>
        <p>
          <span className={styles.muted}>Gathered</span> <TimeAgo at={gathered} />
          {late && (
            <span className={styles.muted}>
              {' '}
              — {formatDuration(gathered - entry.ts)} after it happened
            </span>
          )}
        </p>
        <p className={styles.muted}>Entry id {entry.id}</p>
      </div>
    </details>
  );
}

function SuggestionRow({
  suggestion,
  onLink,
  linking,
}: {
  suggestion: Suggestion;
  onLink: (link: { alert_ids?: string[]; change_ids?: string[] }) => void;
  linking: boolean;
}) {
  const at = instant(suggestion.at);
  return (
    <li
      className={styles.entry}
      data-testid="timeline-row"
      data-row={suggestion.key}
      data-offered=""
      data-kind={suggestion.kind}
    >
      <Tooltip content={formatTimestamp(suggestion.at)} describe={false}>
        <time className={styles.when} dateTime={at ? new Date(at).toISOString() : undefined}>
          {at === null ? '—' : formatTime(at)}
        </time>
      </Tooltip>
      <span className={styles.kind}>{suggestion.kind === 'config' ? 'Config' : ENTRY_WORD[suggestion.kind]}</span>
      <div className={styles.body}>
        <p className={styles.text}>
          {suggestion.to ? (
            <Link to={suggestion.to} className={styles.textLink}>
              {suggestion.text}
            </Link>
          ) : (
            suggestion.text
          )}
          {suggestion.lab && <LabPill />}
        </p>
        <p className={styles.meta}>
          <span className={styles.offeredMark}>not linked</span>
          <span className={styles.muted}>{suggestion.detail}</span>
          {suggestion.beforeStartMs !== null && (
            <span className={styles.before}>
              {formatDuration(suggestion.beforeStartMs)} before this incident started
            </span>
          )}
          {suggestion.kind === 'config' &&
            (suggestion.explained ? (
              <span className={styles.muted}>an applied change covers it</span>
            ) : (
              <span className={styles.unexplained}>no change of ours covers it</span>
            ))}
          {suggestion.link && (
            <Button
              size="sm"
              variant="secondary"
              icon={Link2}
              loading={linking}
              onClick={() => suggestion.link && onLink(suggestion.link)}
            >
              Link it
            </Button>
          )}
        </p>
      </div>
    </li>
  );
}

/** `<input type="datetime-local">` wants local wall-clock text. */
function toLocalInput(at: number): string {
  const date = new Date(at - new Date(at).getTimezoneOffset() * 60_000);
  return date.toISOString().slice(0, 16);
}

function Composer({
  onNote,
  noting,
  startedAt,
}: {
  onNote: (text: string, at: number | null) => void;
  noting: boolean;
  startedAt: number;
}) {
  const [text, setText] = useState('');
  const [backdating, setBackdating] = useState(false);
  const [at, setAt] = useState(() => toLocalInput(Date.now()));
  const parsed = backdating ? new Date(at).getTime() : null;
  const badTime = backdating && !Number.isFinite(parsed);
  const ready = text.trim() !== '' && !badTime;

  const send = () => {
    if (!ready) return;
    onNote(text.trim(), parsed !== null && Number.isFinite(parsed) ? parsed : null);
    setText('');
    setBackdating(false);
  };

  return (
    <form
      className={styles.composer}
      onSubmit={(event) => {
        event.preventDefault();
        send();
      }}
    >
      <Textarea
        aria-label="Add a note to the timeline"
        className={styles.note}
        rows={2}
        value={text}
        placeholder="What you just did, saw or tried…"
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
            event.preventDefault();
            send();
          }
        }}
      />
      <div className={styles.composerSide}>
        <Button type="submit" variant="primary" icon={Plus} loading={noting} disabled={!ready}>
          Add note
        </Button>
        <span className={styles.hint}>
          <Icon icon={CornerDownLeft} size={11} />
          Ctrl/⌘ + Enter
        </span>
        <button
          type="button"
          className={styles.backdate}
          onClick={() => {
            setBackdating(!backdating);
            if (!backdating) setAt(toLocalInput(Math.max(startedAt, Date.now() - 600_000)));
          }}
        >
          {backdating ? 'Note it as now' : 'It happened earlier…'}
        </button>
        {backdating && (
          <Input
            size="sm"
            type="datetime-local"
            aria-label="When it happened"
            value={at}
            invalid={badTime}
            onChange={(event) => setAt(event.target.value)}
          />
        )}
      </div>
    </form>
  );
}
