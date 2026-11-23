/**
 * "Attach evidence…" on an incident's timeline.
 *
 * Three ways to put evidence on an incident, in the order of how much the
 * engine can vouch for:
 *
 * - **pick** something that is already kept;
 * - **capture** the last lines of a terminal session — the engine asks the
 *   terminal for them itself (`evidence.capture_terminal`), so the text is
 *   what the session holds, redacted, with the session as its source. This
 *   dialog never sends the text;
 * - **text you hold** — a line from a ticket, something read out over the
 *   phone. It is redacted on arrival and labelled as supplied by a person
 *   wherever it is shown, because the engine did not read it from anything.
 *
 * Whichever it is, what goes on the timeline is a reference at the time the
 * evidence was captured. The incident never holds the content.
 *
 * A capture the engine cannot make — no terminal feature in this build, a
 * session that is gone, a limit — is answered with its own sentence, shown
 * here as it came, and nothing is stored.
 */

import { FileCheck2, Link2, SquareTerminal, Type } from 'lucide-react';
import { useState } from 'react';
import { useHasMethod } from '@/api/hooks';
import type { Evidence, Incident } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { RadioGroup } from '@/design/RadioGroup';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { LabPill } from '@/design/StatusPill';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { Textarea } from '@/design/Textarea';
import { toast } from '@/design/Toast';
import { formatBytes, plural } from '@/lib/format';
import { formatDateTime } from '@/lib/time';
import {
  useAddEvidenceText,
  useAttachEvidence,
  useCaptureTerminal,
  useEvidenceList,
  useTerminalSessions,
} from './data';
import styles from './Evidence.module.css';
import { KIND_LABEL, failure, isOperatorSupplied, listCoverage } from './evidenceReading';
import opsStyles from './Ops.module.css';

type Mode = 'pick' | 'terminal' | 'text';

export interface AttachEvidenceDialogProps {
  incident: Incident;
  /** Who is attaching it, as the incident page already knows. */
  author: string;
  onClose: () => void;
}

interface Refused {
  headline: string;
  message: string;
}

export function AttachEvidenceDialog({ incident, author, onClose }: AttachEvidenceDialogProps) {
  const canCapture = useHasMethod('evidence.capture_terminal') === true;
  const hasSessions = useHasMethod('terminal.list') === true;
  const canAddText = useHasMethod('evidence.add_text') === true;
  const [mode, setMode] = useState<Mode>('pick');
  const attach = useAttachEvidence();
  const [refused, setRefused] = useState<Refused | null>(null);
  /** Captured here, but the attachment that followed did not happen. */
  const [kept, setKept] = useState<Evidence | null>(null);

  const tabs: TabItem[] = [
    { id: 'pick', label: 'Already kept', icon: Link2 },
    // Capturing needs the session list to choose from; a build without the
    // terminal feature has neither, and the tab is not offered.
    ...(canCapture && hasSessions ? [{ id: 'terminal', label: 'From a terminal session', icon: SquareTerminal }] : []),
    ...(canAddText ? [{ id: 'text', label: 'Text I hold', icon: Type }] : []),
  ];

  /** Attaches, and says whether it did. */
  const attachItem = async (item: Pick<Evidence, 'id' | 'title'>): Promise<boolean> => {
    try {
      await attach.mutateAsync({
        id: item.id,
        incident_id: incident.id,
        ...(author.trim() === '' ? {} : { author: author.trim() }),
      });
      toast.success('Attached to the timeline', { description: item.title });
      onClose();
      return true;
    } catch (error) {
      setRefused({ headline: 'It could not be attached', message: failure(error).message });
      return false;
    }
  };

  /** A capture, then its attachment. A capture that worked is not lost when the attachment fails. */
  const keepAndAttach = async (capture: () => Promise<Evidence>) => {
    setRefused(null);
    let item: Evidence;
    try {
      item = await capture();
    } catch (error) {
      const why = failure(error);
      // The engine says so itself for a limit; for the rest it is said here.
      const stored = why.message.includes('Nothing was stored') ? '' : ' Nothing was stored.';
      setRefused({ headline: why.headline, message: `${why.message}${stored}` });
      return;
    }
    // Only a capture whose attachment failed is "kept, but not attached".
    if (!(await attachItem(item))) setKept(item);
  };

  return (
    <Dialog
      open
      onClose={onClose}
      title="Attach evidence"
      description={`To “${incident.title}”. A reference goes on the timeline at the time the evidence was captured; the content is kept under Evidence.`}
      size="lg"
    >
      <div className={styles.attachBody}>
        {kept ? (
          <>
            <p className={styles.refused} role="alert">
              <b>Kept, but not attached.</b> “{kept.title}” is stored under Evidence.{' '}
              {refused?.message ?? 'Attaching it to this incident did not happen.'}
            </p>
            <div className={opsStyles.row}>
              <Button variant="primary" loading={attach.isPending} onClick={() => void attachItem(kept)}>
                Try attaching again
              </Button>
              <Button variant="ghost" onClick={onClose}>
                Leave it unattached
              </Button>
            </div>
          </>
        ) : (
          <>
            <Tabs
              id="attach-evidence"
              aria-label="Where the evidence comes from"
              tabs={tabs}
              value={mode}
              onChange={(id) => {
                setMode(id as Mode);
                setRefused(null);
              }}
            />
            <TabPanel tabs="attach-evidence" value={mode}>
              {mode === 'pick' && (
                <Pick incident={incident} attaching={attach.isPending} onAttach={(item) => void attachItem(item)} />
              )}
              {mode === 'terminal' && <FromSession onKeep={keepAndAttach} attaching={attach.isPending} />}
              {mode === 'text' && <TextIHold incident={incident} onKeep={keepAndAttach} attaching={attach.isPending} />}
            </TabPanel>
            {refused && (
              <p className={styles.refused} role="alert" data-testid="attach-refused">
                <b>{refused.headline}.</b> {refused.message}
              </p>
            )}
          </>
        )}
      </div>
    </Dialog>
  );
}

function Pick({
  incident,
  attaching,
  onAttach,
}: {
  incident: Incident;
  attaching: boolean;
  onAttach: (item: Evidence) => void;
}) {
  const [query, setQuery] = useState('');
  const [picked, setPicked] = useState<string | null>(null);
  const list = useEvidenceList({ limit: 50, ...(query.trim() === '' ? {} : { query: query.trim() }) });
  const items = list.data?.items ?? [];
  const chosen = items.find((item) => item.id === picked);

  return (
    <div className={styles.attachBody}>
      <SearchInput
        // Where the dialog starts: the first thing a person does here is look
        // for the item, not close the dialog (its first control otherwise).
        data-autofocus
        aria-label="Search evidence"
        value={query}
        onChange={(value) => {
          setQuery(value);
          setPicked(null);
        }}
        placeholder="Search titles, devices, sessions and notes"
      />
      {list.isPending && <Skeleton shape="block" height={160} />}
      {list.isError && (
        <p className={styles.refused} role="alert">
          <b>The evidence could not be read.</b> {list.error.message}
        </p>
      )}
      {list.data && items.length > 0 && (
        <div className={styles.pick}>
          <RadioGroup
            aria-label="Evidence to attach"
            value={picked}
            onChange={setPicked}
            options={items.map((item) => {
              const already = item.incident_ids.includes(incident.id);
              return {
                value: item.id,
                disabled: already,
                label: (
                  <span className={styles.pickTitle}>
                    {item.title}
                    <Badge tone="info" variant="outline">
                      {KIND_LABEL[item.kind] ?? item.kind}
                    </Badge>
                    {item.source.lab && <LabPill />}
                    {isOperatorSupplied(item.source) && (
                      <Badge tone="warn" variant="outline">
                        supplied by a person
                      </Badge>
                    )}
                  </span>
                ),
                description: [
                  already ? 'already on this timeline' : null,
                  item.source.device,
                  `captured ${formatDateTime(item.captured_at)}`,
                  item.lines === undefined ? formatBytes(item.bytes) : plural(item.lines, 'line'),
                ]
                  .filter(Boolean)
                  .join(' · '),
              };
            })}
          />
        </div>
      )}
      {list.data && (
        <p className={opsStyles.sentence}>
          {listCoverage(items.length, Boolean(list.data.next_cursor), query.trim() !== '', true)}
          {list.data.next_cursor && ' Search to find an older one.'}
        </p>
      )}
      <div className={opsStyles.row}>
        <Button
          variant="primary"
          icon={FileCheck2}
          loading={attaching}
          disabled={!chosen}
          onClick={() => chosen && onAttach(chosen)}
        >
          Attach
        </Button>
      </div>
    </div>
  );
}

function FromSession({
  onKeep,
  attaching,
}: {
  onKeep: (capture: () => Promise<Evidence>) => Promise<void>;
  attaching: boolean;
}) {
  const sessions = useTerminalSessions(true);
  const capture = useCaptureTerminal();
  const [sessionId, setSessionId] = useState('');
  const [lines, setLines] = useState('200');
  const [title, setTitle] = useState('');
  const count = Number(lines);
  const badLines = !Number.isInteger(count) || count < 1 || count > 10_000;
  const list = sessions.data ?? [];

  if (sessions.isPending) return <Skeleton shape="block" height={160} />;
  if (sessions.isError) {
    return (
      <p className={styles.refused} role="alert">
        <b>The terminal sessions could not be listed.</b> {sessions.error.message}
      </p>
    );
  }
  if (list.length === 0) {
    return (
      <p className={opsStyles.sentence}>
        The engine holds no terminal session right now. Open one under Terminal and come back: the last
        lines are read from the session by the engine, so there has to be a session to read.
      </p>
    );
  }

  return (
    <div className={opsStyles.form}>
      <Field
        label="Session"
        hint="The engine reads the session’s last lines itself and redacts them. This dialog sends no text."
      >
        <Select value={sessionId} onChange={(event) => setSessionId(event.target.value)}>
          <option value="">Choose a session</option>
          {list.map((session) => (
            <option key={session.id} value={session.id}>
              {session.title}
              {session.lab ? ' (lab)' : ''}
              {session.state === 'open' ? '' : ` — ${session.state}`}
            </option>
          ))}
        </Select>
      </Field>
      <div className={opsStyles.formRow}>
        <Field
          label="Last lines"
          error={badLines ? 'Between 1 and 10,000.' : undefined}
          hint="For more of a session, capture from its log."
        >
          <Input
            type="number"
            min={1}
            max={10_000}
            inputMode="numeric"
            value={lines}
            onChange={(event) => setLines(event.target.value)}
          />
        </Field>
        <Field label="Title" hint="Left empty, the engine names it after the session.">
          <Input value={title} maxLength={200} onChange={(event) => setTitle(event.target.value)} />
        </Field>
      </div>
      <div className={opsStyles.row}>
        <Button
          variant="primary"
          icon={FileCheck2}
          loading={capture.isPending || attaching}
          disabled={sessionId === '' || badLines}
          onClick={() =>
            void onKeep(async () => {
              const { body: _body, ...item } = await capture.mutateAsync({
                session_id: sessionId,
                lines: count,
                ...(title.trim() === '' ? {} : { title: title.trim() }),
              });
              return item;
            })
          }
        >
          Capture and attach
        </Button>
      </div>
    </div>
  );
}

function TextIHold({
  incident,
  onKeep,
  attaching,
}: {
  incident: Incident;
  onKeep: (capture: () => Promise<Evidence>) => Promise<void>;
  attaching: boolean;
}) {
  const add = useAddEvidenceText();
  const [title, setTitle] = useState('');
  const [text, setText] = useState('');
  const [deviceId, setDeviceId] = useState('');
  const [note, setNote] = useState('');
  const ready = title.trim() !== '' && text !== '';

  return (
    <div className={opsStyles.form}>
      <p className={styles.operatorNote}>
        This is kept as <b>supplied by a person</b> and says so wherever it is shown. Secrets in it are
        replaced on arrival, but the engine did not read it from a device, so it cannot vouch for where it
        came from. For what a session printed, capture from the session instead.
      </p>
      <Field label="Title" required>
        <Input
          value={title}
          maxLength={200}
          placeholder="What this is, in a few words"
          onChange={(event) => setTitle(event.target.value)}
        />
      </Field>
      <Field
        label="Text"
        required
        hint={`${formatBytes(new Blob([text]).size)} of the 256 KiB an item holds, measured after redaction. Past that it is refused, not cut.`}
      >
        <Textarea mono rows={8} value={text} spellCheck={false} onChange={(event) => setText(event.target.value)} />
      </Field>
      <div className={opsStyles.formRow}>
        <Field label="About device" hint="Optional. Only the devices on this incident are offered.">
          <Select value={deviceId} onChange={(event) => setDeviceId(event.target.value)}>
            <option value="">No device</option>
            {incident.devices.map((device) => (
              <option key={device.id} value={device.id}>
                {device.name}
                {device.lab ? ' (lab)' : ''}
              </option>
            ))}
          </Select>
        </Field>
        <Field label="Where it is from" hint="Optional: a ticket, a phone call, a vendor’s mail.">
          <Input value={note} maxLength={1000} onChange={(event) => setNote(event.target.value)} />
        </Field>
      </div>
      <div className={opsStyles.row}>
        <Button
          variant="primary"
          icon={FileCheck2}
          loading={add.isPending || attaching}
          disabled={!ready}
          onClick={() =>
            void onKeep(async () => {
              const { body: _body, ...item } = await add.mutateAsync({
                text,
                title: title.trim(),
                ...(deviceId === '' ? {} : { device_id: deviceId }),
                ...(note.trim() === '' ? {} : { note: note.trim() }),
              });
              return item;
            })
          }
        >
          Keep and attach
        </Button>
      </div>
    </div>
  );
}
