/**
 * Forwarding to a log platform.
 *
 * Nocturne is a network event collector for the people who run the network:
 * thirty days on one SQLite file. It is not a SIEM and does not pretend to
 * be. What it does instead is hand everything that is stored — already
 * redacted — to one, and this pane is where that is set up.
 *
 * The list is two sources joined by id (`forward.ts`): the forwarders
 * themselves (`logs.forward.list`, which never holds a URL or a token) and
 * what each has done (`logs.status`, `forwarders`): sent, dropped, the last
 * error and when. Those counters are since the application started, and a
 * forwarder the status does not list is shown as not reported — never as
 * zero.
 *
 * A test (`logs.forward.test`) sends one event that says it is a test, on a
 * connection of its own. The engine's answer is the whole verdict: `sent`,
 * or an error whose message says why not. It is shown as given.
 */

import { Pencil, Plus, Send, Trash, Waypoints } from 'lucide-react';
import { useState } from 'react';
import type { Forwarder, LogsStatus } from '@/api/types/logs';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { EMPTY, formatBytes, formatCount, plural } from '@/lib/format';
import { useDeleteForwarder, useForwarders, useTestForwarder } from './data';
import { ForwarderDrawer } from './ForwarderDrawer';
import {
  destinationOf,
  floorPhrase,
  kindLabel,
  rowsOf,
  sendsAudit,
  sentMeaning,
  stateView,
  streamLabel,
  waitingOf,
  type ForwarderRow,
} from './forward';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';

export interface ForwardingPaneProps {
  capabilities: Capabilities;
  status: LogsStatus | undefined;
}

/** The outcome of the last test, kept so it can be read, not just glimpsed. */
interface TestOutcome {
  id: string;
  name: string;
  ok: boolean;
  /** The engine's words on failure; ours on success, built from `{sent: true}`. */
  message: string;
}

/** The engine allows this many. */
const MAX_FORWARDERS = 16;

export function ForwardingPane({ capabilities, status }: ForwardingPaneProps) {
  const forwarders = useForwarders();
  const test = useTestForwarder();
  const remove = useDeleteForwarder();
  /** `null`: closed; `forwarder: null`: a new one. */
  const [open, setOpen] = useState<{ forwarder: Forwarder | null } | null>(null);
  const [outcome, setOutcome] = useState<TestOutcome | null>(null);

  const rows = rowsOf(forwarders.data ?? [], status?.forwarders);

  const runTest = (forwarder: Forwarder) => {
    setOutcome(null);
    test.mutate(
      { id: forwarder.id },
      {
        onSuccess: () =>
          setOutcome({
            id: forwarder.id,
            name: forwarder.name,
            ok: true,
            message: `Sent: one test event was handed to “${forwarder.name}”. ${sentMeaning(forwarder.kind)} A test is not stored and not counted below.`,
          }),
        onError: (error) =>
          setOutcome({ id: forwarder.id, name: forwarder.name, ok: false, message: error.message }),
      },
    );
  };

  const destroy = async (forwarder: Forwarder) => {
    const confirmed = await confirm({
      title: `Delete the forwarder “${forwarder.name}”?`,
      description:
        forwarder.kind === 'https_json'
          ? 'It stops sending, what waits in its queue is dropped, and its URL and token are removed from the vault.'
          : 'It stops sending, and what waits in its queue is dropped.',
      confirmLabel: 'Delete the forwarder',
      tone: 'danger',
    });
    if (!confirmed) return;
    remove.mutate({ id: forwarder.id }, { onSuccess: () => setOpen(null) });
  };

  const columns: readonly TableColumn<ForwarderRow>[] = [
    {
      id: 'name',
      header: 'Forwarder',
      required: true,
      cell: ({ forwarder }) => forwarder.name,
      sortValue: ({ forwarder }) => forwarder.name,
    },
    {
      // Beside the name: in a narrow window the table scrolls sideways, and
      // whether a forwarder is failing is the column that must not be out of view.
      id: 'state',
      header: 'State',
      width: 150,
      required: true,
      cell: ({ status: reported }) => {
        const view = stateView(reported);
        return (
          <StatusPill tone={view.tone} title={view.meaning}>
            {view.label}
          </StatusPill>
        );
      },
    },
    { id: 'kind', header: 'Kind', width: 132, cell: ({ forwarder }) => kindLabel(forwarder.kind) },
    {
      id: 'destination',
      header: 'Destination',
      width: 190,
      cell: ({ forwarder }) =>
        forwarder.kind === 'https_json' ? (
          <span className={styles.muted} title={destinationOf(forwarder)}>
            {destinationOf(forwarder)}
          </span>
        ) : (
          <span className={styles.monoText}>{destinationOf(forwarder)}</span>
        ),
    },
    {
      id: 'streams',
      header: 'Sends',
      width: 210,
      cell: ({ forwarder }) => {
        const sends = `${forwarder.streams.map(streamLabel).join(', ')}${forwarder.lab ? ', and labs' : ''}`;
        // The audit trail leaving the machine is the one thing in this column
        // that must not be lost to an ellipsis, so it is said first.
        const audit = sendsAudit(forwarder.streams);
        const rest = forwarder.streams.filter((stream) => stream !== 'audit').map(streamLabel);
        return (
          <span title={sends} data-audit={audit || undefined}>
            {audit && (
              <Badge tone="warn" variant="outline">
                Audit trail
              </Badge>
            )}
            {audit && rest.length > 0 ? ' ' : ''}
            {`${rest.join(', ')}${forwarder.lab ? ', and labs' : ''}`}
          </span>
        );
      },
    },
    {
      id: 'floor',
      header: 'Holds back below',
      width: 170,
      defaultHidden: true,
      cell: ({ forwarder }) => floorPhrase(forwarder.min_severity),
    },
    {
      id: 'sent',
      header: 'Sent',
      width: 88,
      align: 'right',
      cell: ({ status: reported }) => (reported ? formatCount(reported.sent) : EMPTY),
      sortValue: ({ status: reported }) => reported?.sent,
    },
    {
      id: 'dropped',
      header: 'Dropped',
      width: 108,
      align: 'right',
      cell: ({ status: reported }) =>
        reported ? (
          <span data-attention={reported.dropped > 0 || undefined} className={styles.cellCount}>
            {formatCount(reported.dropped)}
          </span>
        ) : (
          EMPTY
        ),
      sortValue: ({ status: reported }) => reported?.dropped,
    },
    {
      id: 'last',
      header: 'Last sent',
      width: 118,
      cell: ({ status: reported }) =>
        reported?.last_at == null ? (
          <span className={styles.muted}>{reported ? 'Never' : EMPTY}</span>
        ) : (
          <TimeAgo at={reported.last_at} />
        ),
      sortValue: ({ status: reported }) => reported?.last_at,
    },
  ];

  // The last error is a sentence and is read whole, so it is under the
  // table rather than cut to the width of a column.
  const failures = rows.filter((row) => row.status?.last_error);
  // What waits in a queue, where the engine reports it. A column of it would
  // be noughts nearly always, so it is said where there is something to say.
  const waiting = rows
    .map((row) => ({ forwarder: row.forwarder, queue: waitingOf(row.status) }))
    .filter((row) => row.queue !== null && row.queue.entries > 0);
  const budget = rows.map((row) => waitingOf(row.status)?.budget).find((value) => value != null) ?? null;

  const count = forwarders.data?.length ?? 0;

  return (
    <div className={styles.paneBody}>
      <div className={styles.paneHead}>
        <p className={styles.paneLead}>
          Nocturne keeps thirty days of what the network says on this machine. It is not a SIEM — no
          years of retention, no threat intelligence, no case management — so it hands everything that
          is stored, already redacted, to one. A slow or dead collector never slows reception: what
          does not fit in a forwarder’s queue is dropped and counted here.
        </p>
        {capabilities.forwardSave && (
          <Button
            variant="primary"
            icon={Plus}
            disabled={count >= MAX_FORWARDERS}
            title={count >= MAX_FORWARDERS ? `There can be at most ${MAX_FORWARDERS} forwarders.` : undefined}
            onClick={() => setOpen({ forwarder: null })}
          >
            Add a forwarder
          </Button>
        )}
      </div>

      {outcome !== null && (
        <p
          className={outcome.ok ? styles.testOk : styles.fieldError}
          role={outcome.ok ? 'status' : 'alert'}
          data-testid="forward-test-outcome"
        >
          {outcome.ok ? outcome.message : `“${outcome.name}” was not sent the test event: ${outcome.message}`}
        </p>
      )}

      {forwarders.isError ? (
        <ErrorState
          title="The forwarders could not be read"
          error={forwarders.error}
          onRetry={() => void forwarders.refetch()}
        />
      ) : (
        <Table
          id="logs.forwarders"
          columns={columns}
          rows={rows}
          rowKey={({ forwarder }) => forwarder.id}
          loading={forwarders.isPending}
          density="comfortable"
          onRowActivate={capabilities.forwardSave ? ({ forwarder }) => setOpen({ forwarder }) : undefined}
          rowActions={({ forwarder }) => [
            ...(capabilities.forwardSave
              ? [{ label: 'Edit', icon: Pencil, onSelect: () => setOpen({ forwarder }) }]
              : []),
            ...(capabilities.forwardTest
              ? [{ label: 'Send a test event', icon: Send, onSelect: () => runTest(forwarder) }]
              : []),
            ...(capabilities.forwardDelete
              ? [
                  { type: 'separator' as const },
                  { label: 'Delete…', icon: Trash, tone: 'danger' as const, onSelect: () => void destroy(forwarder) },
                ]
              : []),
          ]}
          empty={
            <EmptyState
              size="compact"
              icon={Waypoints}
              title="Nothing is forwarded"
              description="Everything stays on this machine until a forwarder is added. A forwarder sends what is stored from the moment it is saved; nothing older."
              action={
                capabilities.forwardSave ? (
                  <Button icon={Plus} onClick={() => setOpen({ forwarder: null })}>
                    Add a forwarder
                  </Button>
                ) : undefined
              }
            />
          }
          aria-label="Log forwarders"
        />
      )}

      {failures.length > 0 && (
        <ul className={styles.failures} aria-label="Last errors">
          {failures.map(({ forwarder, status: reported }) => (
            <li key={forwarder.id}>
              <strong>{forwarder.name}</strong> — last error: {reported?.last_error}
            </li>
          ))}
        </ul>
      )}

      {waiting.length > 0 && (
        <ul className={styles.waiting} aria-label="Waiting to be sent">
          {waiting.map(({ forwarder, queue }) => (
            <li key={forwarder.id}>
              <strong>{forwarder.name}</strong> — {plural(queue?.entries ?? 0, 'entry', 'entries')} waiting to be sent
              {queue?.bytes != null ? `, ${formatBytes(queue.bytes)} with the batch on its way` : ''}
            </li>
          ))}
        </ul>
      )}

      {count > 0 && (
        <p className={styles.muted}>
          Sent and dropped are counted since Nocturne started. For the syslog kinds “sent” is what was
          handed to the network — no syslog transport acknowledges anything — and for HTTPS it is what
          the collector answered 2xx to. “Dropped” never left: the queue was full, every attempt
          failed, or the forwarder was saved again while entries waited.
          {budget !== null ? ` A forwarder’s queue holds at most ${formatBytes(budget)} of entries.` : ''}
        </p>
      )}

      {open !== null && (
        <ForwarderDrawer
          key={open.forwarder?.id ?? 'new'}
          forwarder={open.forwarder}
          capabilities={capabilities}
          onClose={() => setOpen(null)}
          onTest={runTest}
          onDelete={(forwarder) => void destroy(forwarder)}
          testing={test.isPending}
          outcome={outcome !== null && outcome.id === open.forwarder?.id ? outcome : null}
        />
      )}
    </div>
  );
}
