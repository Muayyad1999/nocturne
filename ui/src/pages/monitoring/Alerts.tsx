/**
 * The Alerts tab: everything that has fired and not resolved itself, newest
 * first, with the three things a person does about one.
 *
 * The rows are `alerts.list`'s own answer. Acknowledging, snoozing and
 * resolving go straight back to the backend, which publishes `alerts.changed`
 * — that is what refreshes this table, the tiles and the rail's badge, so they
 * cannot drift apart.
 */

import { BellOff, Check, ChevronDown, CircleCheck, Clock, X } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { Alert, AlertListParams, AlertNoteParams, AlertSnoozeParams, Severity } from '@/api/types/monitor';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { DropdownMenu } from '@/design/DropdownMenu';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import type { MenuEntry } from '@/design/Menu';
import { SearchInput } from '@/design/SearchInput';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { AlertDrawer } from './AlertDrawer';
import styles from './Monitoring.module.css';
import { SEVERITIES, severityLabel, severityRank, severityTone } from './vocab';

type StateFilter = NonNullable<AlertListParams['state']>;

const STATES: readonly { id: StateFilter; label: string }[] = [
  { id: 'open', label: 'Open' },
  { id: 'firing', label: 'Firing' },
  { id: 'acknowledged', label: 'Acknowledged' },
  { id: 'resolved', label: 'Resolved' },
];

const PAGE = 100;

const SNOOZE_CHOICES: readonly { label: string; minutes: number }[] = [
  { label: 'Snooze for 1 hour', minutes: 60 },
  { label: 'Snooze for 4 hours', minutes: 240 },
  { label: 'Snooze for a day', minutes: 1_440 },
];

/** Acknowledge, resolve and snooze, with the toast each one deserves. */
export function useAlertActions() {
  const ack = useRpcMutation<Alert, AlertNoteParams>('alerts.ack', {
    invalidates: ['alerts.'],
    successToast: 'Acknowledged',
  });
  const resolve = useRpcMutation<Alert, AlertNoteParams>('alerts.resolve', {
    invalidates: ['alerts.'],
    successToast: 'Resolved',
  });
  const snooze = useRpcMutation<Alert, AlertSnoozeParams>('alerts.snooze', { invalidates: ['alerts.'] });

  return {
    pending: ack.isPending || resolve.isPending || snooze.isPending,
    acknowledge: (alert: Alert, note?: string) => ack.mutate({ id: alert.id, note }),
    snooze: (alert: Alert, minutes: number) => snooze.mutate({ id: alert.id, minutes }),
    resolve: async (alert: Alert, note?: string) => {
      const ok = await confirm({
        title: `Resolve “${alert.title}”?`,
        description:
          'The alert closes now, and any notification channel the rule uses is told it is over. If the condition is still there the next poll raises it again as a new alert.',
        confirmLabel: 'Resolve',
      });
      if (ok) resolve.mutate({ id: alert.id, note });
    },
  };
}

/** The menu offered on a row and in the drawer. */
export function alertActions(
  alert: Alert,
  actions: ReturnType<typeof useAlertActions>,
  extra: readonly MenuEntry[] = [],
): MenuEntry[] {
  if (alert.state === 'resolved') {
    return [...extra, { type: 'label', label: 'This alert has resolved; there is nothing left to do.' }];
  }
  const snoozing: MenuEntry[] = alert.snoozed_until
    ? [{ label: 'Cancel the snooze', icon: X, onSelect: () => actions.snooze(alert, 0) }]
    : SNOOZE_CHOICES.map((choice) => ({
        label: choice.label,
        icon: Clock,
        onSelect: () => actions.snooze(alert, choice.minutes),
      }));
  return [
    ...extra,
    ...(alert.state === 'acknowledged'
      ? []
      : [{ label: 'Acknowledge', icon: Check, onSelect: () => actions.acknowledge(alert) } as MenuEntry]),
    { label: 'Resolve…', icon: CircleCheck, onSelect: () => void actions.resolve(alert) },
    { type: 'separator' },
    ...snoozing,
  ];
}

export function AlertsTab() {
  const [state, setState] = useState<StateFilter>('open');
  const [severity, setSeverity] = useState<Severity | null>(null);
  const [text, setText] = useState('');
  const [limit, setLimit] = useState(PAGE);
  const [open, setOpen] = useState<Alert | null>(null);
  const actions = useAlertActions();

  const params: AlertListParams = { state, limit, ...(severity ? { severity } : {}) };
  const alerts = useRpc<Alert[]>('alerts.list', params, { invalidateOn: ['alerts.changed'] });

  const all = alerts.data ?? [];
  // The text box narrows what has already been fetched: `alerts.list` has no
  // free-text filter, and saying so beats a search that quietly misses rows.
  const needle = text.trim().toLowerCase();
  const rows = needle
    ? all.filter((alert) =>
        [alert.title, alert.device, alert.rule_name, alert.instance, alert.detail]
          .filter(Boolean)
          .some((field) => field?.toLowerCase().includes(needle)),
      )
    : all;

  const columns: TableColumn<Alert>[] = [
    {
      id: 'severity',
      header: 'Severity',
      width: 112,
      required: true,
      sortValue: (alert) => severityRank(alert.severity),
      cell: (alert) => <StatusPill tone={severityTone(alert.severity)}>{severityLabel(alert.severity)}</StatusPill>,
    },
    {
      id: 'state',
      header: 'State',
      width: 128,
      sortValue: (alert) => alert.state,
      cell: (alert) => <AlertStateCell alert={alert} />,
    },
    {
      id: 'device',
      header: 'Device',
      width: 180,
      sortValue: (alert) => alert.device ?? '',
      cell: (alert) =>
        alert.device_id ? (
          <span className={styles.cell}>
            <Link className={styles.cellText} to={`/monitoring/health?device=${encodeURIComponent(alert.device_id)}`}>
              {alert.device}
            </Link>
            {alert.lab && <LabPill />}
          </span>
        ) : (
          <span className={styles.missing}>Not about a device</span>
        ),
    },
    {
      id: 'title',
      header: 'What fired',
      required: true,
      sortValue: (alert) => alert.title,
      cell: (alert) => (
        <span className={styles.cell} title={`${alert.title}\n${alert.detail}`}>
          <span className={styles.cellText}>{alert.title}</span>
          {alert.instance && <Badge variant="outline">{alert.instance}</Badge>}
        </span>
      ),
    },
    {
      id: 'rule',
      header: 'Rule',
      width: 180,
      // What fired is the headline; which rule caught it is one click away in
      // the drawer, and the room is better spent on the title at 1280.
      defaultHidden: true,
      sortValue: (alert) => alert.rule_name,
      cell: (alert) => <span className={styles.cellText}>{alert.rule_name}</span>,
    },
    {
      id: 'count',
      header: 'Seen',
      width: 88,
      align: 'right',
      sortValue: (alert) => alert.count,
      cell: (alert) => <span title={`Seen ${formatCount(alert.count)} times since it was raised`}>{formatCount(alert.count)}×</span>,
    },
    {
      id: 'started',
      header: 'Since',
      width: 116,
      sortValue: (alert) => alert.started_at,
      cell: (alert) => <TimeAgo at={alert.started_at} />,
    },
    {
      id: 'last_seen',
      header: 'Last seen',
      width: 116,
      defaultHidden: true,
      sortValue: (alert) => alert.last_seen_at,
      cell: (alert) => <TimeAgo at={alert.last_seen_at} />,
    },
  ];

  if (alerts.isError) {
    return (
      <ErrorState
        title="The alerts could not be loaded"
        error={alerts.error}
        onRetry={() => void alerts.refetch()}
      />
    );
  }

  return (
    <>
      <Toolbar className={styles.filters}>
        <div className={styles.segmented} role="group" aria-label="Alert state">
          {STATES.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className={styles.segment}
              aria-pressed={state === entry.id}
              onClick={() => {
                setState(entry.id);
                setLimit(PAGE);
              }}
            >
              {entry.label}
            </button>
          ))}
        </div>

        <DropdownMenu
          aria-label="Filter by severity"
          items={[
            ...(severity
              ? ([
                  { label: 'Any severity', icon: X, onSelect: () => setSeverity(null) },
                  { type: 'separator' },
                ] as MenuEntry[])
              : []),
            ...SEVERITIES.map((entry) => ({
              label: `${severityLabel(entry)} and above`,
              checked: severity === entry,
              onSelect: () => setSeverity(severity === entry ? null : entry),
            })),
          ]}
          trigger={
            <Button size="sm" variant={severity ? 'secondary' : 'ghost'} trailingIcon={ChevronDown}>
              {severity ? `${severityLabel(severity)} and above` : 'Severity'}
            </Button>
          }
        />

        <SearchInput
          className={styles.search}
          size="sm"
          aria-label="Search alerts"
          placeholder="Search what was fetched…"
          value={text}
          onChange={setText}
        />

        <span className={styles.count}>
          {alerts.isPending ? '' : `${formatCount(rows.length)} ${rows.length === 1 ? 'alert' : 'alerts'}`}
        </span>
        <ToolbarSpacer />
      </Toolbar>

      <Table
        className={styles.table}
        id="monitoring.alerts"
        aria-label="Alerts"
        fill
        columns={columns}
        rows={rows}
        rowKey={(alert) => alert.id}
        loading={alerts.isPending}
        defaultSort={{ columnId: 'started', direction: 'desc' }}
        onRowActivate={setOpen}
        rowActions={(alert) =>
          alertActions(alert, actions, [{ label: 'Open', onSelect: () => setOpen(alert) }, { type: 'separator' }])
        }
        empty={
          needle ? (
            <EmptyState
              size="compact"
              icon={BellOff}
              title="Nothing matches"
              description="No alert in this page matches what you typed."
              action={
                <Button size="sm" onClick={() => setText('')}>
                  Clear the search
                </Button>
              }
            />
          ) : state === 'open' ? (
            <EmptyState
              size="compact"
              icon={CircleCheck}
              title="Nothing is open"
              description="No rule is firing right now. Alerts appear here as soon as one does; the Rules tab says what is being watched."
            />
          ) : (
            <EmptyState
              size="compact"
              icon={BellOff}
              title="No alerts in this state"
              description="Nothing has been recorded here yet."
            />
          )
        }
      />

      {rows.length >= limit && (
        <div className={styles.more}>
          <Button size="sm" onClick={() => setLimit(limit + PAGE)} loading={alerts.isFetching}>
            Show more
          </Button>
          <span className={styles.count}>Showing the {formatCount(limit)} most recent.</span>
        </div>
      )}

      {open && <AlertDrawer alert={open} onClose={() => setOpen(null)} />}
    </>
  );
}

/** Firing, acknowledged or resolved — and whether anything is muting it. */
export function AlertStateCell({ alert }: { alert: Alert }) {
  const snoozed = alert.snoozed_until != null && alert.snoozed_until > Date.now();
  return (
    <span className={styles.cell}>
      <Badge tone={alert.state === 'resolved' ? 'ok' : alert.state === 'acknowledged' ? 'idle' : 'crit'}>
        {alert.state === 'resolved' ? 'Resolved' : alert.state === 'acknowledged' ? 'Acknowledged' : 'Firing'}
      </Badge>
      {snoozed && (
        <Badge tone="maint" variant="outline" title="Notifications are muted until the snooze ends">
          Snoozed
        </Badge>
      )}
      {alert.muted_by_maintenance && (
        <Badge tone="maint" variant="outline" title="A maintenance window covers this: recorded, not notified">
          Maintenance
        </Badge>
      )}
    </span>
  );
}
