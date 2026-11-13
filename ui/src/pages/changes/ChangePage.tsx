/**
 * One change, from the commands nobody has read yet to the record of what
 * happened on every device.
 *
 * The page's whole job is that nothing is applied that a person has not seen
 * in its final form. Three rules it keeps, in order of importance:
 *
 * 1. A protected resource is confirmed by typing the device's name. Never a
 *    tick box (see `ApproveBar`).
 * 2. An approval is bound to the plans it was given (`approval.binding`). If
 *    the plans stored now are not the ones that were signed, this page
 *    refuses to apply and says so — it does not quietly plan again.
 * 3. Nothing is applied from the browser. The page calls `changes.apply`;
 *    the engine decides. When the engine refuses, its own sentence is what
 *    is shown, because only the engine knows why.
 */

import {
  CircleX,
  GitPullRequestArrow,
  Play,
  RefreshCw,
  ShieldAlert,
  Trash,
  TriangleAlert,
  Undo2,
} from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { RpcError } from '@/api/errors';
import type { Change, ChangeCancelled, ChangeDeleted, ChangeIdParams } from '@/api/types/changes';
import type { AuditList, DeviceRow, Job } from '@/api/types/core';
import { registerCommand } from '@/app/CommandPalette/commands';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { ProgressBar } from '@/design/ProgressBar';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { Tooltip } from '@/design/Tooltip';
import { formatDateTime } from '@/lib/time';
import { ApproveBar } from './ApproveBar';
import {
  approvalRequest,
  binding,
  namedTargets,
  pruneReview,
  type ReviewState,
} from './approval';
import {
  canApprove,
  canCancel,
  canPlan,
  isFinished,
  ORIGIN_LABEL,
  plural,
  RISK_LABEL,
  riskTone,
  rollbackCandidates,
  STATE_LABEL,
  STATE_SENTENCE,
  stateTone,
} from './change';
import { DeviceCard } from './DeviceCard';
import { reportOutcome, useChangeJob } from './useChangeJob';
import styles from './ChangePage.module.css';

const EMPTY_REVIEW: ReviewState = { seen: new Set<string>(), declared: false };

/** Worst-first ordering, to pick the risk the header shows. */
const RISK_ORDER = ['low', 'medium', 'high', 'critical'] as const;

export interface ChangePageProps {
  id: string;
}

export default function ChangePage({ id }: ChangePageProps) {
  const navigate = useNavigate();
  const [typed, setTyped] = useState<ReadonlyMap<string, string>>(new Map());
  const [review, setReview] = useState<ReviewState>(EMPTY_REVIEW);
  const [note, setNote] = useState('');
  /** A refusal this page made itself, not one the engine sent. */
  const [refusal, setRefusal] = useState<string | null>(null);

  const change = useRpc<Change>(
    'changes.get',
    { id },
    { invalidateOn: ['changes.changed', 'changes.progress'] },
  );
  const devices = useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed'],
  });
  const audit = useRpc<AuditList>(
    'audit.list',
    { text: 'changes.', limit: 300 },
    { invalidateOn: ['audit.appended'] },
  );

  const data = change.data;
  const names = useMemo(
    () => new Map((devices.data ?? []).map((device) => [device.id, device.name])),
    [devices.data],
  );
  const deviceById = useMemo(
    () => new Map((devices.data ?? []).map((device) => [device.id, device])),
    [devices.data],
  );
  const targets = useMemo(
    () => (data ? namedTargets(data, names) : []),
    [data, names],
  );

  // A plan that was replaced takes its review with it: nothing the person
  // read about the old plan says anything about the new one.
  useEffect(() => {
    setReview((was) => {
      const next = pruneReview(was, targets);
      return next.seen.size === was.seen.size && next.declared === was.declared ? was : next;
    });
  }, [targets]);

  // …and the page says so out loud, because a plan that changes while
  // somebody is reading it is the one thing this screen must never let past
  // quietly.
  const [replaced, setReplaced] = useState(false);
  const planned = targets.length > 0 && targets.every((entry) => entry.plan !== undefined);
  const hashes = targets.map((entry) => entry.plan?.plan_hash ?? '').join('|');
  const firstHashes = useRef<string | null>(null);
  useEffect(() => {
    if (!planned) return;
    if (firstHashes.current === null) {
      firstHashes.current = hashes;
      return;
    }
    if (firstHashes.current !== hashes) {
      firstHashes.current = hashes;
      setReplaced(true);
    }
  }, [hashes, planned]);

  const planning = useChangeJob('changes.plan', id, {
    onFinished: (finished) => {
      const failed = finished.targets.filter((target) => target.plan == null);
      if (failed.length > 0) {
        toast.warning(`${plural(failed.length, 'device')} could not be planned`);
      } else {
        toast.success('Planned', { description: 'Read the commands below before approving.' });
      }
    },
  });
  const applying = useChangeJob('changes.apply', id, { onFinished: reportOutcome });
  const undoing = useChangeJob('changes.rollback', id, { onFinished: reportOutcome });

  const approve = useRpcMutation<Change, Record<string, unknown>>('changes.approve', {
    invalidates: ['changes.'],
    errorToast: false,
  });
  const unapprove = useRpcMutation<Change, ChangeIdParams>('changes.unapprove', {
    invalidates: ['changes.'],
  });
  const cancel = useRpcMutation<ChangeCancelled, ChangeIdParams>('changes.cancel', {
    invalidates: ['changes.'],
  });
  const remove = useRpcMutation<ChangeDeleted, ChangeIdParams>('changes.delete', {
    invalidates: ['changes.'],
  });

  const applyJob = useRpc<Job>(
    'jobs.get',
    { id: data?.job_id },
    { enabled: Boolean(data?.job_id), invalidateOn: ['jobs.changed', 'changes.progress'] },
  );

  usePageHeader({
    title: data?.title ?? 'Change',
    breadcrumbs: [{ label: 'Changes', to: '/changes' }],
  });

  useEffect(() => {
    if (!data || !canPlan(data.state)) return;
    return registerCommand({
      id: 'changes.plan-again',
      title: 'Plan this change again',
      group: 'Changes',
      icon: RefreshCw,
      run: () => planning.start(),
    });
  }, [data?.state]);

  if (change.error) {
    return (
      <Page>
        <ErrorState
          title="This change could not be read"
          error={change.error}
          onRetry={() => void change.refetch()}
          actions={<Button onClick={() => void navigate('/changes')}>Back to the list</Button>}
        />
      </Page>
    );
  }

  if (!data) {
    return (
      <Page>
        <div className={styles.loading} aria-label="Loading the change">
          <Skeleton width={320} height={24} />
          <Skeleton width="100%" height={72} shape="block" />
          <Skeleton width="100%" height={160} shape="block" />
        </div>
      </Page>
    );
  }

  const bound = binding(data, names);
  const approvedButStale = data.state === 'approved' && !bound.bound;
  const undoable = rollbackCandidates(data);
  const driftFailed = data.targets.some((target) =>
    target.result?.steps.some((step) => step.name === 'drift check' && !step.ok),
  );
  const running = data.state === 'applying';
  const applyError = (applying.error ?? null) as RpcError | null;
  const approveError = (approve.error ?? null) as RpcError | null;

  const history = (audit.data?.entries ?? []).filter(
    (entry) => (entry.body as { change_id?: string } | undefined)?.change_id === id,
  );

  const doApply = () => {
    // The screen's own guard in front of the engine's. An approval that no
    // longer covers the stored plans is never turned into an apply here.
    const check = binding(data, names);
    if (!check.bound) {
      setRefusal(check.reason ?? 'This approval does not cover the plans that are stored now.');
      return;
    }
    setRefusal(null);
    applying.start();
  };

  const doApprove = (thenApply: boolean) => {
    const request = approvalRequest(targets, typed);
    approve.mutate(
      { id, ...request, note: note.trim() || undefined },
      {
        onSuccess: (approved) => {
          toast.success(`Approved on ${plural(approved.targets.length, 'device')}`);
          if (thenApply) applying.start();
        },
      },
    );
  };

  const risk = targets
    .map((entry) => entry.plan?.risk.level)
    .reduce<(typeof RISK_ORDER)[number] | undefined>(
      (worst, level) =>
        level && (!worst || RISK_ORDER.indexOf(level) > RISK_ORDER.indexOf(worst)) ? level : worst,
      undefined,
    );
  const hits = targets.reduce((total, entry) => total + (entry.plan?.protected_hits.length ?? 0), 0);

  return (
    <Page>
      <Toolbar>
        <ToolbarSpacer />
        {canPlan(data.state) && (
          <Button
            icon={RefreshCw}
            loading={planning.running || planning.starting}
            onClick={() => planning.start()}
          >
            {data.state === 'draft' ? 'Plan' : 'Plan again'}
          </Button>
        )}
        {undoable.length > 0 && !running && (
          <Button
            icon={Undo2}
            loading={undoing.running || undoing.starting}
            onClick={async () => {
              const where = undoable.map((deviceId) => names.get(deviceId) ?? deviceId);
              const ok = await confirm({
                title: `Roll back on ${plural(undoable.length, 'device')}?`,
                description: `Nocturne will put ${where.join(', ')} back to the configuration each of them had before this change, and check that it got there.`,
                confirmLabel: 'Roll back',
                tone: 'danger',
              });
              if (ok) undoing.start();
            }}
          >
            Roll back…
          </Button>
        )}
        {canCancel(data.state) && (
          <Button
            icon={CircleX}
            loading={cancel.isPending}
            onClick={async () => {
              const ok = await confirm({
                title: running ? 'Stop applying this change?' : 'Cancel this change?',
                description: running
                  ? 'The device being configured right now finishes or is rolled back; the ones after it are skipped. No device is left half-configured.'
                  : 'The change is kept as a record, but it will not be applied.',
                confirmLabel: running ? 'Stop' : 'Cancel the change',
                cancelLabel: 'Keep going',
                tone: 'danger',
              });
              if (ok) cancel.mutate({ id });
            }}
          >
            {running ? 'Stop' : 'Cancel'}
          </Button>
        )}
        {isFinished(data.state) && (
          <Button
            icon={Trash}
            variant="ghost"
            loading={remove.isPending}
            onClick={async () => {
              const ok = await confirm({
                title: `Delete “${data.title}”?`,
                description:
                  'The record of this change goes, including the commands that were sent. The audit trail keeps the fact that it existed.',
                confirmLabel: 'Delete',
                tone: 'danger',
                confirmText: data.title,
              });
              if (ok) remove.mutate({ id }, { onSuccess: () => void navigate('/changes') });
            }}
          >
            Delete
          </Button>
        )}
      </Toolbar>

      <div className={styles.page}>
        <header className={styles.intro}>
          <div className={styles.headline}>
            <StatusPill tone={stateTone(data.state)}>{STATE_LABEL[data.state]}</StatusPill>
            {risk && <Badge tone={riskTone(risk)}>{RISK_LABEL[risk]}</Badge>}
            {hits > 0 && (
              <Tooltip content="Management or access resources: every one of these devices has to be named to approve the change.">
                <Badge tone="crit">{plural(hits, 'protected resource')}</Badge>
              </Tooltip>
            )}
            {data.origin === 'assistant' && (
              <Tooltip content="Drafted by the assistant. It is applied only once a person approves it here.">
                <Badge tone="ai">Assistant</Badge>
              </Tooltip>
            )}
            {data.ticket && <span className={styles.ticket}>{data.ticket}</span>}
          </div>
          <p className={styles.sentence}>{STATE_SENTENCE[data.state]}</p>
          {data.description !== '' && <p className={styles.description}>{data.description}</p>}
          <p className={styles.meta}>
            {data.origin !== 'user' && <>{ORIGIN_LABEL[data.origin]} · </>}
            Created <TimeAgo at={data.created_at} /> by {data.created_by}
            {data.approval && (
              <>
                {' '}
                · approved by {data.approval.by} at {formatDateTime(data.approval.at)}
                {data.approval.confirmed_devices.length > 0 && (
                  <> · {data.approval.confirmed_devices.join(', ')} confirmed by name</>
                )}
              </>
            )}
          </p>
        </header>

        {(planning.running || planning.starting) && (
          <section className={styles.banner} data-tone="info" aria-label="Planning">
            <div className={styles.bannerText}>
              <strong>Working out the commands</strong>
              <span>
                Reading the running configuration of {plural(data.targets.length, 'device')}.
                Nothing is changed.
              </span>
            </div>
            <ProgressBar
              className={styles.bannerBar}
              value={planning.job?.done}
              max={planning.job?.total || data.targets.length}
            />
            <Button size="sm" onClick={planning.cancel}>
              Cancel
            </Button>
          </section>
        )}

        {planning.error && (
          <section className={styles.banner} data-tone="crit" aria-label="Planning failed">
            <div className={styles.bannerText}>
              <strong>The change could not be planned</strong>
              <span data-testid="plan-error">{planning.error.message}</span>
            </div>
          </section>
        )}

        {running && (
          <section className={styles.banner} data-tone="info" aria-label="Applying">
            <div className={styles.bannerText}>
              <strong>Applying</strong>
              <span>
                {applyJob.data?.message ||
                  `Backup, drift check, send, settle, verify — on ${plural(data.targets.length, 'device')}.`}
              </span>
            </div>
            <ProgressBar
              className={styles.bannerBar}
              value={applyJob.data?.done}
              max={applyJob.data?.total || data.targets.length}
            />
            <Button
              size="sm"
              onClick={async () => {
                const ok = await confirm({
                  title: 'Stop applying this change?',
                  description:
                    'The device being configured right now finishes or is rolled back; the ones after it are skipped.',
                  confirmLabel: 'Stop',
                  cancelLabel: 'Keep going',
                  tone: 'danger',
                });
                if (ok) cancel.mutate({ id });
              }}
            >
              Stop
            </Button>
          </section>
        )}

        {driftFailed && (
          <section className={styles.banner} data-tone="warn" aria-label="Stale plan">
            <div className={styles.bannerText}>
              <strong>The plan was out of date</strong>
              <span>
                The configuration of a device changed after this was planned, so nothing was sent to
                it. Plan it again, read what is different, and approve the new plan.
              </span>
            </div>
            <Button size="sm" icon={RefreshCw} onClick={() => planning.start()}>
              Plan again
            </Button>
          </section>
        )}

        {replaced && canApprove(data.state) && (
          <section
            className={styles.banner}
            data-tone="warn"
            aria-label="The plans were replaced"
            data-testid="plans-replaced"
          >
            <div className={styles.bannerText}>
              <strong>These plans were worked out again while this page was open</strong>
              <span>
                The commands below are not the ones that were here a moment ago, so whatever was
                read or approved before no longer counts. Read them again.
              </span>
            </div>
            <Button size="sm" onClick={() => setReplaced(false)}>
              Understood
            </Button>
          </section>
        )}

        {approvedButStale && (
          <section
            className={styles.banner}
            data-tone="crit"
            aria-label="The approval no longer matches"
            data-testid="stale-approval"
          >
            <div className={styles.bannerText}>
              <strong>This approval is not for the plans stored now</strong>
              <span>{bound.reason}</span>
            </div>
            <Button size="sm" icon={RefreshCw} onClick={() => planning.start()}>
              Plan again
            </Button>
          </section>
        )}

        {refusal && (
          <p className={styles.refusal} role="alert" data-testid="apply-refused">
            <ShieldAlert size={14} aria-hidden /> {refusal}
          </p>
        )}

        <div className={styles.cards}>
          {data.targets.map((target) => {
            const device = deviceById.get(target.device_id);
            return (
              <DeviceCard
                key={target.device_id}
                change={data}
                target={target}
                name={names.get(target.device_id) ?? target.device_id}
                lab={Boolean(device?.lab)}
                protectedDevice={Boolean(device?.protected)}
                defaultOpen={data.targets.length === 1 && data.state !== 'draft'}
                onSeen={(hash) =>
                  setReview((was) =>
                    was.seen.has(hash) ? was : { ...was, seen: new Set([...was.seen, hash]) },
                  )
                }
              />
            );
          })}
        </div>

        {canApprove(data.state) && (
          <ApproveBar
            targets={targets}
            risk={risk}
            typed={typed}
            onTyped={(deviceId, value) =>
              setTyped((was) => new Map(was).set(deviceId, value))
            }
            review={review}
            onDeclare={(declared) => setReview((was) => ({ ...was, declared }))}
            note={note}
            onNote={setNote}
            onApprove={doApprove}
            pending={approve.isPending || applying.starting}
            error={approveError}
          />
        )}

        {data.state === 'approved' && (
          <section className={styles.applyBar} aria-label="Apply">
            <div className={styles.applyText}>
              <strong>Approved by {data.approval?.by ?? 'somebody'}</strong>
              <span>
                {bound.bound
                  ? `Bound to ${plural(data.approval?.plan_hashes.length ?? 0, 'plan')}. Applying connects to each device, backs it up, checks it has not drifted, sends the commands and verifies — and rolls back if it does not verify.`
                  : bound.reason}
              </span>
            </div>
            <ToolbarSpacer />
            <Button onClick={() => unapprove.mutate({ id })} loading={unapprove.isPending}>
              Withdraw the approval
            </Button>
            <Button
              variant="primary"
              icon={Play}
              disabled={!bound.bound}
              loading={applying.starting || applying.running}
              onClick={doApply}
            >
              Apply to {plural(data.targets.length, 'device')}
            </Button>
          </section>
        )}

        {applyError && (
          <p className={styles.refusal} role="alert" data-testid="apply-error">
            <TriangleAlert size={14} aria-hidden /> {applyError.message}
          </p>
        )}

        {history.length > 0 && (
          <Panel title="History" collapsible defaultCollapsed storageId="changes.history">
            <ol className={styles.history} aria-label="What happened to this change">
              {history.map((entry) => (
                <li key={entry.id} className={styles.historyItem}>
                  <span className={styles.historyAt}>{formatDateTime(entry.ts)}</span>
                  <span className={styles.historySource}>{entry.source}</span>
                  <span className={styles.historyMessage}>{entry.message}</span>
                </li>
              ))}
            </ol>
          </Panel>
        )}

        {data.targets.length === 0 && (
          <p className={styles.sentence}>
            <GitPullRequestArrow size={14} aria-hidden /> This change has no devices.
          </p>
        )}
      </div>
    </Page>
  );
}
