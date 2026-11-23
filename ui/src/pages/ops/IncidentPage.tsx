/**
 * One incident: everything known about it, around its timeline.
 *
 * The timeline is the centrepiece and takes the main column; the facts, the
 * links and the follow-ups sit beside it. The prose (what is happening, who it
 * affects, what caused it) is edited in place — an incident is written while it
 * is happening, and a modal for every sentence would mean nothing gets written.
 *
 * The state buttons are the backend's own verbs, with its own rules kept
 * visible rather than guessed at: resolving requires a cause in the same call,
 * reopening requires a reason, and acknowledging can only happen once. Each of
 * those refusals is a sentence the engine wrote, so the dialogs ask for what
 * it needs instead of discovering it as an error.
 */

import { ArrowLeft, Check, FileDown, RefreshCw, Siren, Trash2, TriangleAlert } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import { useHasMethod } from '@/api/hooks';
import type { FollowUp, Incident, IncidentSeverity } from '@/api/types/ops';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { confirm, Dialog } from '@/design/Dialog';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Textarea } from '@/design/Textarea';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { toast } from '@/design/Toast';
import { formatCount, plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import { AttachEvidenceDialog } from './AttachEvidence';
import { useAuthor } from './author';
import {
  useAcknowledgeIncident,
  useAddNote,
  useDeleteIncident,
  useDetachEvidence,
  useIncident,
  useLinkToIncident,
  useReopenIncident,
  useResolveIncident,
  useSuggestedLinks,
  useSyncIncident,
  useUpdateIncident,
} from './data';
import { attachedEvidenceId } from './evidenceReading';
import { ExportDialog } from './ExportDialog';
import styles from './Incident.module.css';
import opsStyles from './Ops.module.css';
import {
  SEVERITY_LABEL,
  SEVERITY_MEANING,
  SEVERITY_TONE,
  STATE_LABEL,
  STATE_MEANING,
  STATE_TONE,
  incidentDuration,
  instant,
} from './ops';
import { Timeline } from './Timeline';

const SEVERITIES = (['sev1', 'sev2', 'sev3', 'sev4'] as IncidentSeverity[]).map((severity) => ({
  value: severity,
  label: SEVERITY_LABEL[severity],
}));

export interface IncidentPageProps {
  id: string;
  onClose: () => void;
}

export function IncidentPage({ id, onClose }: IncidentPageProps) {
  const incident = useIncident(id);
  // Looking for what is not linked yet reaches two other features and the
  // snapshot table, so it is its own read and the timeline draws without it.
  const suggested = useSuggestedLinks(id, true);
  const [author, setAuthor] = useAuthor();

  const note = useAddNote();
  const link = useLinkToIncident();
  const sync = useSyncIncident();
  const acknowledge = useAcknowledgeIncident();
  const update = useUpdateIncident();
  const remove = useDeleteIncident();
  const resolve = useResolveIncident();
  const reopen = useReopenIncident();
  const [resolving, setResolving] = useState(false);
  const [reopening, setReopening] = useState(false);
  const [exporting, setExporting] = useState(false);
  // Evidence is attached and detached only where the engine has it: a build
  // without `evidence.*` shows neither the button nor a dead "Detach".
  const hasEvidence = useHasMethod('evidence.attach') === true;
  const detach = useDetachEvidence();
  const [attaching, setAttaching] = useState(false);

  if (incident.isError) {
    return (
      <ErrorState
        title="That incident could not be read"
        error={incident.error}
        onRetry={() => void incident.refetch()}
        actions={
          <Button variant="secondary" icon={ArrowLeft} onClick={onClose}>
            Back to the list
          </Button>
        }
      />
    );
  }

  if (incident.isPending || !incident.data) {
    return (
      <>
        <Skeleton shape="block" height={56} />
        <Skeleton shape="block" height={420} />
      </>
    );
  }

  const record = incident.data;
  const duration = incidentDuration(record, Date.now());
  const lab = record.devices.some((device) => device.lab);

  return (
    <>
      <Toolbar>
        <Button variant="ghost" icon={ArrowLeft} onClick={onClose}>
          Incidents
        </Button>
        <StatusPill tone={SEVERITY_TONE[record.severity]} title={SEVERITY_MEANING[record.severity]}>
          {SEVERITY_LABEL[record.severity]}
        </StatusPill>
        <StatusPill tone={STATE_TONE[record.state]} title={STATE_MEANING[record.state]}>
          {STATE_LABEL[record.state]}
        </StatusPill>
        {lab && <LabPill title="Every device on this incident is simulated: this is a lab." />}
        <ToolbarSpacer />
        <Button
          variant="ghost"
          icon={RefreshCw}
          loading={sync.isPending}
          title="Read every source again. Nothing new is linked; anything already on the timeline stays."
          onClick={() =>
            sync.mutate(
              { id },
              {
                onSuccess: (updated) =>
                  toast.success(
                    `The timeline now has ${plural(updated.timeline.length, 'entry', 'entries')}`,
                  ),
              },
            )
          }
        >
          Re-read sources
        </Button>
        <Button variant="secondary" icon={FileDown} onClick={() => setExporting(true)}>
          Export review
        </Button>
        {record.acked_at === null && (
          <Button
            variant="secondary"
            icon={Check}
            loading={acknowledge.isPending}
            onClick={() => acknowledge.mutate({ id, author })}
          >
            Acknowledge
          </Button>
        )}
        {record.state === 'open' && (
          <Button
            variant="secondary"
            loading={update.isPending}
            onClick={() => update.mutate({ id, state: 'mitigated', author })}
          >
            Mitigated
          </Button>
        )}
        {record.state === 'resolved' ? (
          <Button variant="secondary" icon={Siren} onClick={() => setReopening(true)}>
            Reopen…
          </Button>
        ) : (
          <Button variant="primary" icon={Check} onClick={() => setResolving(true)}>
            Resolve…
          </Button>
        )}
      </Toolbar>

      <Title record={record} author={author} onSave={(title) => update.mutate({ id, title, author })} />

      <div className={opsStyles.columns}>
        <div className={styles.main}>
          <Timeline
            incident={record}
            suggested={suggested.data}
            suggestionsPending={suggested.isPending}
            noting={note.isPending}
            onNote={(text, at) => note.mutate({ id, text, ts: at ?? undefined, author })}
            onAttachEvidence={hasEvidence ? () => setAttaching(true) : undefined}
            onDetachEvidence={
              hasEvidence
                ? (evidenceId) =>
                    detach.mutate(
                      { id: evidenceId, incident_id: id, ...(author.trim() === '' ? {} : { author: author.trim() }) },
                      { onSuccess: () => toast.success('Detached. The evidence itself is still kept.') },
                    )
                : undefined
            }
            detaching={detach.isPending ? (detach.variables?.id ?? null) : null}
            linking={link.isPending}
            onLink={(ids) =>
              link.mutate(
                { id, ...ids, author },
                {
                  onSuccess: (result) =>
                    toast.success(
                      result.timeline_entries_added > 0
                        ? `Linked, and ${plural(result.timeline_entries_added, 'entry', 'entries')} came with it`
                        : 'Linked. It brought no new timeline entries.',
                    ),
                },
              )
            }
          />

          <Prose
            record={record}
            saving={update.isPending}
            onSave={(fields) => update.mutate({ id, ...fields, author })}
          />
        </div>

        <aside className={styles.aside}>
          <Facts record={record} duration={duration} />
          <Severity
            record={record}
            author={author}
            onChange={(severity) => update.mutate({ id, severity, author })}
          />
          <Who
            record={record}
            author={author}
            onAuthor={setAuthor}
            onAssign={(assignee) => update.mutate({ id, assignee, author })}
          />
          <Links record={record} />
          <FollowUps
            record={record}
            saving={update.isPending}
            onSave={(follow_ups) => update.mutate({ id, follow_ups, author })}
          />
          <Panel title="This record">
            <div className={styles.dangerous}>
              <p className={opsStyles.sentence}>
                Deleting it removes the incident and its timeline. The alerts, changes and audit
                entries it gathered from are untouched — they belong to the features that wrote them.
              </p>
              <Button
                variant="danger"
                icon={Trash2}
                loading={remove.isPending}
                onClick={async () => {
                  const sure = await confirm({
                    title: 'Delete this incident?',
                    description: `“${record.title}” and its ${plural(record.timeline.length, 'timeline entry', 'timeline entries')} will be gone. This cannot be undone.`,
                    confirmLabel: 'Delete it',
                    tone: 'danger',
                  });
                  if (!sure) return;
                  remove.mutate({ id }, { onSuccess: () => onClose() });
                }}
              >
                Delete this incident
              </Button>
            </div>
          </Panel>
        </aside>
      </div>

      <ResolveDialog
        open={resolving}
        record={record}
        pending={resolve}
        author={author}
        onClose={() => setResolving(false)}
      />
      <ReopenDialog
        open={reopening}
        id={id}
        pending={reopen}
        author={author}
        onClose={() => setReopening(false)}
      />
      <ExportDialog
        open={exporting}
        onClose={() => setExporting(false)}
        method="incidents.export"
        params={{ id }}
        title="Post-incident review"
        description="Written by the engine from this record: the timeline with its evidence column, the gaps in it, the cause, the follow-ups, and an appendix with every piece of attached evidence."
      />
      {attaching && (
        <AttachEvidenceDialog incident={record} author={author} onClose={() => setAttaching(false)} />
      )}
    </>
  );
}

/** The title, edited where it is read. */
function Title({
  record,
  onSave,
}: {
  record: Incident;
  author: string;
  onSave: (title: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(record.title);
  if (!editing) {
    return (
      <h2 className={styles.title}>
        <button type="button" className={styles.titleEdit} onClick={() => { setText(record.title); setEditing(true); }}>
          {record.title}
        </button>
      </h2>
    );
  }
  return (
    <form
      className={styles.titleForm}
      onSubmit={(event) => {
        event.preventDefault();
        if (text.trim() !== '' && text !== record.title) onSave(text.trim());
        setEditing(false);
      }}
    >
      <Input
        aria-label="Incident title"
        autoFocus
        value={text}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === 'Escape') setEditing(false);
        }}
      />
      <Button type="submit" variant="secondary" disabled={text.trim() === ''}>
        Rename
      </Button>
      <Button variant="ghost" onClick={() => setEditing(false)}>
        Cancel
      </Button>
    </form>
  );
}

function Facts({
  record,
  duration,
}: {
  record: Incident;
  duration: { label: string; running: boolean } | null;
}) {
  const when = (at: number | null | undefined, never: string) => {
    const usable = instant(at);
    return usable === null ? (
      <span className={opsStyles.muted}>{never}</span>
    ) : (
      <span title={formatTimestamp(usable)}>
        <TimeAgo at={usable} />
      </span>
    );
  };

  const items: KeyValueItem[] = [
    { label: 'Started', value: when(record.started_at, 'not recorded') },
    {
      label: 'Acknowledged',
      value:
        record.acked_at === null ? (
          <span className={styles.unacked}>nobody yet</span>
        ) : (
          <>
            {when(record.acked_at, 'not recorded')}
            {record.acked_by && <span className={opsStyles.muted}> by {record.acked_by}</span>}
          </>
        ),
    },
    { label: 'Mitigated', value: when(record.mitigated_at, 'not yet') },
    { label: 'Resolved', value: when(record.resolved_at, 'not yet') },
    {
      label: duration?.running ? 'Open for' : 'Lasted',
      value: duration ? duration.label : <span className={opsStyles.muted}>unknown</span>,
    },
    {
      label: 'Opened by',
      value: record.opened_by || <span className={opsStyles.muted}>not recorded</span>,
    },
    { label: 'Record id', value: <span className={opsStyles.mono}>{record.id}</span> },
  ];
  return (
    <Panel title="Times">
      <KeyValue items={items} layout="rows" />
    </Panel>
  );
}

function Severity({
  record,
  onChange,
}: {
  record: Incident;
  author: string;
  onChange: (severity: IncidentSeverity) => void;
}) {
  return (
    <Panel title="How bad">
      <Field label="Severity" hint={SEVERITY_MEANING[record.severity]}>
        <Select
          size="sm"
          value={record.severity}
          options={SEVERITIES}
          onChange={(event) => onChange(event.target.value as IncidentSeverity)}
        />
      </Field>
    </Panel>
  );
}

function Who({
  record,
  author,
  onAuthor,
  onAssign,
}: {
  record: Incident;
  author: string;
  onAuthor: (name: string) => void;
  onAssign: (assignee: string) => void;
}) {
  const [assignee, setAssignee] = useState(record.assignee);
  return (
    <Panel title="Who">
      <div className={styles.stack}>
        <Field
          label="On it"
          hint="Free text: Nocturne has no user directory. Empty unassigns."
        >
          <Input
            size="sm"
            value={assignee}
            placeholder="unassigned"
            onChange={(event) => setAssignee(event.target.value)}
            onBlur={() => {
              if (assignee !== record.assignee) onAssign(assignee);
            }}
          />
        </Field>
        <Field label="You" hint="Goes on the timeline with everything you write here.">
          <Input size="sm" value={author} placeholder="who is writing this" onChange={(event) => onAuthor(event.target.value)} />
        </Field>
      </div>
    </Panel>
  );
}

function Links({ record }: { record: Incident }) {
  const hasChanges = useHasMethod('changes.list');
  const hasEvidence = useHasMethod('evidence.list') === true;
  // Counted from the timeline, which is where an attachment is recorded.
  const attached = record.timeline.filter((entry) => attachedEvidenceId(entry) !== null).length;
  return (
    <Panel title="What it touches">
      <div className={styles.stack}>
        <div>
          <p className={styles.linkLabel}>Devices</p>
          {record.devices.length === 0 ? (
            <p className={opsStyles.sentence}>None is recorded, so the timeline gathers nothing from a device.</p>
          ) : (
            <ul className={styles.chips}>
              {record.devices.map((device) => (
                <li key={device.id}>
                  <Link to={`/devices/${encodeURIComponent(device.id)}`} className={styles.chip}>
                    {device.name}
                    {device.lab && <LabPill />}
                    {device.deleted && (
                      <Badge tone="idle" variant="outline" title="Deleted from the inventory since">
                        gone
                      </Badge>
                    )}
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div>
          <p className={styles.linkLabel}>Alerts</p>
          {record.alert_ids.length === 0 ? (
            <p className={opsStyles.sentence}>None linked.</p>
          ) : (
            <ul className={styles.chips}>
              {record.alert_ids.map((alertId) => (
                <li key={alertId}>
                  <Link
                    to={
                      record.device_ids[0]
                        ? `/monitoring/health?device=${encodeURIComponent(record.device_ids[0])}`
                        : '/monitoring/alerts'
                    }
                    className={styles.chip}
                  >
                    <span className={opsStyles.mono}>{alertId}</span>
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </div>
        <div>
          <p className={styles.linkLabel}>Changes</p>
          {record.change_ids.length === 0 ? (
            <p className={opsStyles.sentence}>
              {hasChanges === false
                ? 'This build has no change pipeline, so nothing can be linked.'
                : 'None linked.'}
            </p>
          ) : (
            <ul className={styles.chips}>
              {record.change_ids.map((changeId) => (
                <li key={changeId}>
                  <Link to={`/changes/${encodeURIComponent(changeId)}`} className={styles.chip}>
                    <span className={opsStyles.mono}>{changeId}</span>
                  </Link>
                </li>
              ))}
            </ul>
          )}
        </div>
        {hasEvidence && (
          <div>
            <p className={styles.linkLabel}>Evidence</p>
            <p className={opsStyles.sentence}>
              {attached === 0 ? (
                'None attached. “Attach evidence…” on the timeline puts some here.'
              ) : (
                <Link to={`/ops/evidence?attached=${encodeURIComponent(record.id)}`}>
                  {plural(attached, 'item')} attached
                </Link>
              )}
            </p>
          </div>
        )}
        <p className={opsStyles.sentence}>
          <Link to={`/ops/audit?reference=incident:${encodeURIComponent(record.id)}`}>
            Every action recorded about this incident
          </Link>
        </p>
      </div>
    </Panel>
  );
}

/** The prose: written while it is happening, so saved where it is typed. */
function Prose({
  record,
  saving,
  onSave,
}: {
  record: Incident;
  saving: boolean;
  onSave: (fields: { summary?: string; impact?: string; root_cause?: string }) => void;
}) {
  const [summary, setSummary] = useState(record.summary);
  const [impact, setImpact] = useState(record.impact);
  const [cause, setCause] = useState(record.root_cause);
  const dirty =
    summary !== record.summary || impact !== record.impact || cause !== record.root_cause;

  return (
    <Panel
      title="What happened"
      actions={
        <Button
          size="sm"
          variant={dirty ? 'primary' : 'secondary'}
          loading={saving}
          disabled={!dirty}
          onClick={() => onSave({ summary, impact, root_cause: cause })}
        >
          {dirty ? 'Save' : 'Saved'}
        </Button>
      }
    >
      <div className={styles.stack}>
        <Field label="Summary" hint="What is going on, in the words you would use on the phone.">
          <Textarea rows={3} value={summary} onChange={(event) => setSummary(event.target.value)} />
        </Field>
        <Field label="Impact" hint="Who could not do what, and for how long.">
          <Textarea rows={2} value={impact} onChange={(event) => setImpact(event.target.value)} />
        </Field>
        <Field
          label="Root cause"
          hint="The engine will not let the incident be resolved without this; it is what the next person reads."
        >
          <Textarea rows={3} value={cause} onChange={(event) => setCause(event.target.value)} />
        </Field>
      </div>
    </Panel>
  );
}

function FollowUps({
  record,
  saving,
  onSave,
}: {
  record: Incident;
  saving: boolean;
  onSave: (followUps: FollowUp[]) => void;
}) {
  const [text, setText] = useState('');
  const open = record.follow_ups.filter((followUp) => !followUp.done).length;
  return (
    <Panel
      title="Follow-ups"
      actions={
        open > 0 ? (
          <Badge tone="warn" variant="outline">
            {formatCount(open)} open
          </Badge>
        ) : undefined
      }
    >
      <div className={styles.stack}>
        {record.follow_ups.length === 0 ? (
          <p className={opsStyles.sentence}>
            Nothing is outstanding. What you add here goes into the review document.
          </p>
        ) : (
          <ul className={styles.followUps}>
            {record.follow_ups.map((followUp, index) => (
              <li key={`${index}:${followUp.text}`}>
                <Checkbox
                  checked={followUp.done}
                  disabled={saving}
                  onChange={(event) =>
                    onSave(
                      record.follow_ups.map((other, position) =>
                        position === index ? { ...other, done: event.target.checked } : other,
                      ),
                    )
                  }
                >
                  <span data-done={followUp.done || undefined} className={styles.followUpText}>
                    {followUp.text}
                  </span>
                </Checkbox>
              </li>
            ))}
          </ul>
        )}
        <form
          className={styles.followUpForm}
          onSubmit={(event) => {
            event.preventDefault();
            if (text.trim() === '') return;
            onSave([...record.follow_ups, { text: text.trim(), done: false }]);
            setText('');
          }}
        >
          <Input
            size="sm"
            aria-label="Add a follow-up"
            value={text}
            placeholder="Something still to be done"
            onChange={(event) => setText(event.target.value)}
          />
          <Button size="sm" type="submit" disabled={text.trim() === ''} loading={saving}>
            Add
          </Button>
        </form>
      </div>
    </Panel>
  );
}

type Resolve = ReturnType<typeof useResolveIncident>;
type Reopen = ReturnType<typeof useReopenIncident>;

function ResolveDialog({
  open,
  record,
  pending,
  author,
  onClose,
}: {
  open: boolean;
  record: Incident;
  pending: Resolve;
  author: string;
  onClose: () => void;
}) {
  const [cause, setCause] = useState(record.root_cause);
  const ready = cause.trim() !== '';
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Resolve this incident"
      description="The engine refuses an incident resolved without a cause, and it is right to: it is what the next person reads."
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            icon={Check}
            disabled={!ready}
            loading={pending.isPending}
            onClick={() =>
              pending.mutate(
                { id: record.id, root_cause: cause.trim(), author },
                { onSuccess: () => onClose() },
              )
            }
          >
            Resolve it
          </Button>
        </>
      }
    >
      <Field label="Root cause" required hint="Plain sentences. It goes into the review document.">
        <Textarea
          rows={4}
          autoFocus
          data-autofocus
          value={cause}
          placeholder="What actually caused it"
          onChange={(event) => setCause(event.target.value)}
        />
      </Field>
      {pending.isError && <p className={styles.dialogError}>{pending.error.message}</p>}
    </Dialog>
  );
}

function ReopenDialog({
  open,
  id,
  pending,
  author,
  onClose,
}: {
  open: boolean;
  id: string;
  pending: Reopen;
  author: string;
  onClose: () => void;
}) {
  const [why, setWhy] = useState('');
  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Reopen this incident"
      description="The times it was mitigated and resolved are cleared, because neither holds any more."
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            icon={TriangleAlert}
            disabled={why.trim() === ''}
            loading={pending.isPending}
            onClick={() =>
              pending.mutate({ id, why: why.trim(), author }, { onSuccess: () => onClose() })
            }
          >
            Reopen it
          </Button>
        </>
      }
    >
      <Field
        label="Why"
        required
        hint="Required by the engine, and it goes on the timeline: a reopened incident nobody explained is the one nobody can explain a week later."
      >
        <Textarea
          rows={3}
          autoFocus
          data-autofocus
          value={why}
          placeholder="What is happening again"
          onChange={(event) => setWhy(event.target.value)}
        />
      </Field>
      {pending.isError && <p className={styles.dialogError}>{pending.error.message}</p>}
    </Dialog>
  );
}
