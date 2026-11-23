/**
 * One evidence item: what it is, where it came from, and what is in it.
 *
 * The content is in the main column (`EvidenceBody.tsx`); beside it are the
 * facts that make it evidence rather than a paste — the method the engine
 * read it with, when, from which session or device, its size and the SHA-256
 * of what is stored — and the incidents it is attached to.
 *
 * Three things can be done to an item, and each says what it will do first:
 * renaming changes the title on every timeline that points at it; attaching
 * puts a reference (never the content) on an incident's timeline at the time
 * of capture; deleting removes the content for good and leaves a tombstone on
 * every incident it was attached to, which the confirmation names.
 */

import { ArrowLeft, Copy, FileCheck2, Link2, Pencil, Trash2, Unlink, UserRound } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import { useHasMethod } from '@/api/hooks';
import type { Evidence, EvidenceWithBody, IncidentHead } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button, IconButton } from '@/design/Button';
import { confirm, Dialog } from '@/design/Dialog';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { LabPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { copyText } from '@/lib/clipboard';
import { formatBytes, plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { useAuthor } from './author';
import {
  useAttachEvidence,
  useDeleteEvidence,
  useDetachEvidence,
  useEvidenceItem,
  useIncidents,
  useRenameEvidence,
} from './data';
import { EvidenceBodyView } from './EvidenceBody';
import styles from './Evidence.module.css';
import { KIND_LABEL, KIND_MEANING, deleteWarning, failure, isOperatorSupplied, sourceSentence } from './evidenceReading';
import incidentStyles from './Incident.module.css';
import opsStyles from './Ops.module.css';
import { instant } from './ops';

export interface EvidenceItemProps {
  id: string;
  onClose: () => void;
}

export function EvidenceItem({ id, onClose }: EvidenceItemProps) {
  const item = useEvidenceItem(id);
  // The names of the incidents an item is attached to. Without the incident
  // feature's list they are shown by id, which is still true.
  const incidents = useIncidents({ state: 'any', limit: 500 });
  const canAttach = useHasMethod('evidence.attach') === true;
  const remove = useDeleteEvidence();
  const [author, setAuthor] = useAuthor();
  const [renaming, setRenaming] = useState(false);
  const [attaching, setAttaching] = useState(false);

  if (item.isError) {
    const why = failure(item.error);
    return (
      <ErrorState
        title={item.error.code === 'not_found' ? 'That evidence is not there' : 'That evidence could not be read'}
        error={
          item.error.code === 'not_found'
            ? `${why.message}. It may have been deleted: an incident it was attached to keeps a line saying so.`
            : item.error
        }
        onRetry={() => void item.refetch()}
        actions={
          <Button variant="secondary" icon={ArrowLeft} onClick={onClose}>
            Back to the list
          </Button>
        }
      />
    );
  }
  if (item.isPending || !item.data) {
    return (
      <>
        <Skeleton shape="block" height={40} />
        <Skeleton shape="block" height={420} />
      </>
    );
  }

  const record = item.data;
  const known = incidents.data?.incidents ?? [];

  const destroy = async () => {
    const sure = await confirm({
      title: 'Delete this evidence?',
      description: deleteWarning(record, known),
      confirmLabel: 'Delete it',
      tone: 'danger',
    });
    if (!sure) return;
    remove.mutate(
      { id: record.id, ...(author.trim() === '' ? {} : { author: author.trim() }) },
      {
        onSuccess: (result) => {
          toast.success(
            result.tombstones.length > 0
              ? `Deleted. ${plural(result.tombstones.length, 'incident')} ${result.tombstones.length === 1 ? 'keeps' : 'keep'} a line saying it was there.`
              : 'Deleted.',
          );
          onClose();
        },
      },
    );
  };

  return (
    <>
      <Toolbar>
        <Button variant="ghost" icon={ArrowLeft} onClick={onClose}>
          Evidence
        </Button>
        <Badge tone="info" title={KIND_MEANING[record.kind]}>
          {KIND_LABEL[record.kind] ?? record.kind}
        </Badge>
        {record.source.lab && <LabPill title="It comes from, or shows, a simulated device: this is lab evidence." />}
        {isOperatorSupplied(record.source) && <OperatorBadge />}
        <ToolbarSpacer />
        <Button variant="secondary" icon={Pencil} onClick={() => setRenaming(true)}>
          Rename…
        </Button>
        {canAttach && (
          <Button variant="secondary" icon={Link2} onClick={() => setAttaching(true)}>
            Attach to an incident…
          </Button>
        )}
        <Button variant="danger" icon={Trash2} loading={remove.isPending} onClick={() => void destroy()}>
          Delete…
        </Button>
      </Toolbar>

      <h2 className={incidentStyles.title} data-testid="evidence-title">
        {record.title}
      </h2>

      <div className={opsStyles.columns}>
        <div className={styles.main}>
          <EvidenceBodyView item={record} />
        </div>
        <aside className={styles.aside}>
          <Facts record={record} />
          <Attached
            record={record}
            incidents={known}
            namesKnown={incidents.data !== undefined}
            author={author}
            canAttach={canAttach}
            onAttach={() => setAttaching(true)}
          />
        </aside>
      </div>

      {renaming && <RenameDialog record={record} onClose={() => setRenaming(false)} />}
      {attaching && (
        <AttachDialog
          record={record}
          incidents={known}
          failed={incidents.isError ? incidents.error.message : null}
          author={author}
          onAuthor={setAuthor}
          onClose={() => setAttaching(false)}
        />
      )}
    </>
  );
}

/** Text a person handed in says so wherever it is shown. */
export function OperatorBadge() {
  return (
    <Badge
      tone="warn"
      variant="outline"
      title="This text was sent by a person. The engine redacted it on arrival but did not read it from a device, so it cannot vouch for where it came from."
    >
      <Icon icon={UserRound} size={11} />
      Supplied by a person
    </Badge>
  );
}

function deviceFact(record: EvidenceWithBody) {
  const { source } = record;
  if (source.device_id) {
    return (
      <Link to={`/devices/${encodeURIComponent(source.device_id)}`} className={styles.factLink}>
        {source.device ?? source.device_id}
        {source.lab && <LabPill />}
      </Link>
    );
  }
  // No device is not "unknown device": each kind says why there is none.
  if (record.kind === 'topology') return <span className={opsStyles.muted}>The drawing as a whole, not one device</span>;
  if (record.body.type === 'chart') {
    const devices = new Set(record.body.coverage.series.map((entry) => entry.device_id)).size;
    return <span className={opsStyles.muted}>{plural(devices, 'device')}: see the coverage</span>;
  }
  return <span className={opsStyles.muted}>None was recorded with it</span>;
}

function Facts({ record }: { record: EvidenceWithBody }) {
  const { source } = record;
  const captured = instant(record.captured_at);
  const facts: KeyValueItem[] = [
    { label: 'Source', value: <span data-testid="evidence-source">{sourceSentence(source)}</span> },
    { label: 'Read with', value: source.from, mono: true },
    {
      label: 'Captured',
      value: (
        <span title={formatTimestamp(record.captured_at)}>
          {formatTimestamp(record.captured_at)} · <TimeAgo at={captured} />
        </span>
      ),
    },
    { label: 'Device', value: deviceFact(record) },
  ];
  if (source.session_title || source.session_id) {
    facts.push({
      label: 'Session',
      value: (
        <>
          {source.session_title ?? <span className={opsStyles.muted}>untitled</span>}
          {source.session_id && <span className={styles.factId}> {source.session_id}</span>}
        </>
      ),
    });
  }
  if (source.note) facts.push({ label: 'Note', value: source.note });
  facts.push(
    {
      label: 'Size',
      value:
        record.lines === undefined
          ? formatBytes(record.bytes)
          : `${plural(record.lines, 'line')} · ${formatBytes(record.bytes)}`,
    },
    {
      label: 'SHA-256',
      value: (
        <span className={styles.hash}>
          <span className={styles.hashText} data-testid="evidence-sha256">
            {record.sha256}
          </span>
          <IconButton
            icon={Copy}
            size="sm"
            aria-label="Copy the SHA-256"
            onClick={async () => {
              if (await copyText(record.sha256)) toast.success('Hash copied');
              else toast.error('Could not copy to the clipboard', { description: 'Select the hash and copy it with the keyboard.' });
            }}
          />
        </span>
      ),
    },
    {
      label: 'Redacted',
      value: record.redacted
        ? 'Yes. Secrets were replaced before it was stored; what was replaced cannot be recovered from it.'
        : 'The item does not say it was redacted.',
    },
    { label: 'Id', value: record.id, mono: true },
  );

  return (
    <Panel title="Where it came from">
      <KeyValue items={facts} layout="stacked" />
      <p className={styles.hashNote}>
        The hash is of the content as stored{record.kind === 'chart' ? ' (for a chart, of its JSON)' : ''}. A
        text or a drawing downloaded here is those bytes.
      </p>
      {source.params !== undefined && source.params !== null && (
        <details className={styles.params}>
          <summary>What was asked for</summary>
          <pre className={styles.paramsBody}>{JSON.stringify(source.params, null, 2)}</pre>
        </details>
      )}
    </Panel>
  );
}

function Attached({
  record,
  incidents,
  namesKnown,
  author,
  canAttach,
  onAttach,
}: {
  record: Evidence;
  incidents: readonly IncidentHead[];
  namesKnown: boolean;
  author: string;
  canAttach: boolean;
  onAttach: () => void;
}) {
  const detach = useDetachEvidence();
  return (
    <Panel
      title="Attached to"
      actions={
        canAttach ? (
          <Button size="sm" variant="ghost" icon={Link2} onClick={onAttach}>
            Attach…
          </Button>
        ) : undefined
      }
    >
      {record.incident_ids.length === 0 ? (
        <p className={opsStyles.sentence}>
          No incident. It is kept on its own, and can be attached to one at any time: the timeline entry
          goes where it was captured, not where it was attached.
        </p>
      ) : (
        <ul className={styles.attached} aria-label="Incidents this is attached to">
          {record.incident_ids.map((incidentId) => {
            const incident = incidents.find((entry) => entry.id === incidentId);
            return (
              <li key={incidentId} data-testid="evidence-incident">
                <Link to={`/ops/incidents?incident=${encodeURIComponent(incidentId)}`} className={styles.factLink}>
                  {incident?.title ?? (
                    <>
                      incident <span className={styles.factId}>{incidentId}</span>
                      {namesKnown && <span className={opsStyles.muted}> (not in the list)</span>}
                    </>
                  )}
                </Link>
                {canAttach && (
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={Unlink}
                    loading={detach.isPending && detach.variables?.incident_id === incidentId}
                    title="Takes the entry off that incident's timeline. The evidence itself stays."
                    onClick={() =>
                      detach.mutate(
                        {
                          id: record.id,
                          incident_id: incidentId,
                          ...(author.trim() === '' ? {} : { author: author.trim() }),
                        },
                        { onSuccess: () => toast.success('Detached. The evidence itself is still kept.') },
                      )
                    }
                  >
                    Detach
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
}

function RenameDialog({ record, onClose }: { record: Evidence; onClose: () => void }) {
  const rename = useRenameEvidence();
  const [title, setTitle] = useState(record.title);
  const ready = title.trim() !== '' && title.trim() !== record.title;
  const send = () => {
    if (!ready) return;
    rename.mutate({ id: record.id, title: title.trim() }, { onSuccess: onClose });
  };
  return (
    <Dialog
      open
      onClose={onClose}
      title="Rename this evidence"
      description={
        record.incident_ids.length > 0
          ? `The ${plural(record.incident_ids.length, 'timeline entry', 'timeline entries')} that ${record.incident_ids.length === 1 ? 'points' : 'point'} at it will follow. The content and its hash do not change.`
          : 'Only the title changes. The content and its hash stay as they are.'
      }
      size="sm"
      dismissible={!rename.isPending}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={rename.isPending}>
            Cancel
          </Button>
          <Button variant="primary" loading={rename.isPending} disabled={!ready} onClick={send}>
            Rename
          </Button>
        </>
      }
    >
      <Field label="Title" hint="At most 200 characters. It is redacted like the first one was.">
        <Input
          data-autofocus
          value={title}
          maxLength={200}
          onChange={(event) => setTitle(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') send();
          }}
        />
      </Field>
    </Dialog>
  );
}

function AttachDialog({
  record,
  incidents,
  failed,
  author,
  onAuthor,
  onClose,
}: {
  record: Evidence;
  incidents: readonly IncidentHead[];
  /** Why the incidents could not be listed, when they could not. */
  failed: string | null;
  author: string;
  onAuthor: (name: string) => void;
  onClose: () => void;
}) {
  const attach = useAttachEvidence();
  const free = incidents.filter((incident) => !record.incident_ids.includes(incident.id));
  const [incidentId, setIncidentId] = useState('');
  return (
    <Dialog
      open
      onClose={onClose}
      title="Attach to an incident"
      description="A reference goes on that incident’s timeline at the time this was captured. The content stays here; the incident never holds a copy."
      size="sm"
      dismissible={!attach.isPending}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={attach.isPending}>
            Cancel
          </Button>
          <Button
            variant="primary"
            icon={FileCheck2}
            loading={attach.isPending}
            disabled={incidentId === ''}
            onClick={() =>
              attach.mutate(
                { id: record.id, incident_id: incidentId, ...(author.trim() === '' ? {} : { author: author.trim() }) },
                {
                  onSuccess: () => {
                    toast.success('Attached');
                    onClose();
                  },
                },
              )
            }
          >
            Attach
          </Button>
        </>
      }
    >
      <div className={opsStyles.form}>
        <Field
          label="Incident"
          error={failed ? `The incidents could not be listed: ${failed}` : undefined}
          hint={
            free.length === 0 && !failed
              ? incidents.length === 0
                ? 'There is no incident to attach it to yet.'
                : 'It is already attached to every incident there is.'
              : undefined
          }
        >
          <Select data-autofocus value={incidentId} onChange={(event) => setIncidentId(event.target.value)}>
            <option value="">Choose an incident</option>
            {free.map((incident) => (
              <option key={incident.id} value={incident.id}>
                {incident.title} ({incident.state})
              </option>
            ))}
          </Select>
        </Field>
        <Field label="Your name" hint="Shown on the timeline entry. Remembered in this browser.">
          <Input size="sm" value={author} placeholder="who is attaching this" onChange={(event) => onAuthor(event.target.value)} />
        </Field>
      </div>
    </Dialog>
  );
}
