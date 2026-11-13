/**
 * The Rules tab: what is watched, when alerts are muted, and where they go.
 *
 * Three things that are edited rather than read, so each is a panel with its
 * own table and its own drawer. Saving any of them publishes
 * `monitor.status_changed`, which is what refreshes the others.
 */

import { BellRing, CalendarClock, Plus, Send, SlidersHorizontal, Trash } from 'lucide-react';
import { useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  AlertRule,
  AlertRuleSaveParams,
  Channel,
  ChannelTestResult,
  Deleted,
  IdParams,
  MaintenanceWindow,
} from '@/api/types/monitor';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Panel } from '@/design/Panel';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { formatCount, formatDuration } from '@/lib/format';
import { formatDateTime } from '@/lib/time';
import { ChannelDrawer } from './ChannelDrawer';
import { useChannels, useMaintenance } from './data';
import { windowState } from './datetime';
import { MaintenanceDrawer } from './MaintenanceDrawer';
import styles from './Monitoring.module.css';
import { RuleDrawer } from './RuleDrawer';
import { conditionText, scopeText, severityLabel, severityTone } from './vocab';

export function RulesTab() {
  return (
    <div className={styles.stack}>
      <AlertRules />
      <MaintenanceWindows />
      <Channels />
    </div>
  );
}

// ------------------------------------------------------------------- rules

function AlertRules() {
  const [editing, setEditing] = useState<AlertRule | 'new' | null>(null);
  const rules = useRpc<AlertRule[]>('alerts.rules.list', undefined, {
    invalidateOn: ['monitor.status_changed'],
  });
  const save = useRpcMutation<AlertRule, AlertRuleSaveParams>('alerts.rules.save', {
    invalidates: ['alerts.rules.'],
  });
  const remove = useRpcMutation<Deleted, IdParams>('alerts.rules.delete', {
    invalidates: ['alerts.rules.'],
  });

  const columns: TableColumn<AlertRule>[] = [
    {
      id: 'enabled',
      header: 'On',
      width: 72,
      sortValue: (rule) => (rule.enabled ? 0 : 1),
      cell: (rule) => (
        <Switch
          checked={rule.enabled}
          aria-label={`${rule.name} is ${rule.enabled ? 'on' : 'off'}`}
          onChange={(event) =>
            save.mutate({ id: rule.id, name: rule.name, enabled: event.target.checked })
          }
        />
      ),
    },
    {
      id: 'name',
      header: 'Rule',
      required: true,
      sortValue: (rule) => rule.name,
      cell: (rule) => (
        <span className={styles.cell} title={rule.name}>
          <span className={styles.cellText}>{rule.name}</span>
          {rule.builtin && (
            <Badge variant="outline" title="Shipped with Nocturne: it can be changed or switched off, but not deleted">
              Built in
            </Badge>
          )}
        </span>
      ),
    },
    {
      id: 'severity',
      header: 'Severity',
      width: 112,
      sortValue: (rule) => rule.severity,
      cell: (rule) => <StatusPill tone={severityTone(rule.severity)}>{severityLabel(rule.severity)}</StatusPill>,
    },
    {
      id: 'condition',
      header: 'Fires when',
      sortValue: (rule) => rule.condition.type,
      cell: (rule) => (
        <span className={styles.cellText} title={conditionText(rule.condition)}>
          {conditionText(rule.condition)}
        </span>
      ),
    },
    {
      id: 'for',
      header: 'After',
      width: 92,
      sortValue: (rule) => rule.for_s,
      cell: (rule) => (rule.for_s === 0 ? <span className={styles.muted}>At once</span> : formatDuration(rule.for_s * 1000)),
    },
    {
      id: 'scope',
      header: 'Where',
      width: 200,
      sortValue: (rule) => scopeText(rule.scope),
      cell: (rule) => <span className={styles.cellText}>{scopeText(rule.scope)}</span>,
    },
  ];

  return (
    <Panel
      title="Alert rules"
      actions={
        <Button size="sm" variant="primary" icon={Plus} onClick={() => setEditing('new')}>
          New rule
        </Button>
      }
      padding="none"
    >
      {rules.isError ? (
        <ErrorState
          size="compact"
          title="The rules could not be loaded"
          error={rules.error}
          onRetry={() => void rules.refetch()}
        />
      ) : (
        <Table
          id="monitoring.rules"
          aria-label="Alert rules"
          columns={columns}
          rows={rules.data ?? []}
          rowKey={(rule) => rule.id}
          loading={rules.isPending}
          maxHeight={420}
          onRowActivate={setEditing}
          rowActions={(rule) => [
            { label: 'Edit…', onSelect: () => setEditing(rule) },
            { type: 'separator' },
            rule.builtin
              ? {
                  label: 'Built-in rules cannot be deleted',
                  disabled: true,
                  onSelect: () => undefined,
                }
              : {
                  label: 'Delete…',
                  icon: Trash,
                  tone: 'danger',
                  onSelect: async () => {
                    const ok = await confirm({
                      title: `Delete the rule “${rule.name}”?`,
                      description: 'Nothing it has already raised is removed; it simply stops being evaluated.',
                      confirmLabel: 'Delete',
                      tone: 'danger',
                    });
                    if (ok) remove.mutate({ id: rule.id });
                  },
                },
          ]}
          empty={
            <EmptyState
              size="compact"
              icon={SlidersHorizontal}
              title="No rules"
              description="Nocturne ships with ten rules; if none is here, something is wrong with the store."
            />
          }
        />
      )}

      {editing && (
        <RuleDrawer rule={editing === 'new' ? null : editing} onClose={() => setEditing(null)} />
      )}
    </Panel>
  );
}

// ------------------------------------------------------------- maintenance

function MaintenanceWindows() {
  const [editing, setEditing] = useState<MaintenanceWindow | 'new' | null>(null);
  const windows = useMaintenance();
  const remove = useRpcMutation<Deleted, IdParams>('maintenance.delete', { invalidates: ['maintenance.'] });

  const columns: TableColumn<MaintenanceWindow>[] = [
    {
      id: 'state',
      header: 'State',
      width: 132,
      sortValue: (window) => windowState(window),
      cell: (window) => {
        const state = windowState(window);
        return (
          <StatusPill tone={state === 'open' ? 'maint' : state === 'upcoming' ? 'info' : 'idle'}>
            {state === 'open' ? 'Open now' : state === 'upcoming' ? 'Upcoming' : 'Over'}
          </StatusPill>
        );
      },
    },
    {
      id: 'name',
      header: 'Window',
      required: true,
      sortValue: (window) => window.name,
      cell: (window) => (
        <span className={styles.cell}>
          <span className={styles.cellText}>{window.name}</span>
          {window.recurring === 'weekly' && <Badge variant="outline">Weekly</Badge>}
        </span>
      ),
    },
    {
      id: 'starts',
      header: 'From',
      width: 180,
      sortValue: (window) => window.starts_at,
      cell: (window) => formatDateTime(window.starts_at),
    },
    {
      id: 'length',
      header: 'For',
      width: 100,
      sortValue: (window) => window.ends_at - window.starts_at,
      cell: (window) => formatDuration(window.ends_at - window.starts_at),
    },
    {
      id: 'scope',
      header: 'Covering',
      width: 220,
      sortValue: (window) => scopeText(window.scope),
      cell: (window) => <span className={styles.cellText}>{scopeText(window.scope)}</span>,
    },
  ];

  return (
    <Panel
      title="Maintenance windows"
      actions={
        <Button size="sm" icon={Plus} onClick={() => setEditing('new')}>
          New window
        </Button>
      }
      padding="none"
    >
      <Table
        id="monitoring.maintenance"
        aria-label="Maintenance windows"
        columns={columns}
        rows={windows.data ?? []}
        rowKey={(window) => window.id}
        loading={windows.isPending}
        maxHeight={320}
        onRowActivate={setEditing}
        rowActions={(window) => [
          { label: 'Edit…', onSelect: () => setEditing(window) },
          {
            label: 'Delete…',
            icon: Trash,
            tone: 'danger',
            onSelect: async () => {
              const ok = await confirm({
                title: `Delete the window “${window.name}”?`,
                description: 'Alerts it would have muted will notify again.',
                confirmLabel: 'Delete',
                tone: 'danger',
              });
              if (ok) remove.mutate({ id: window.id });
            },
          },
        ]}
        empty={
          <EmptyState
            size="compact"
            icon={CalendarClock}
            title="No maintenance windows"
            description="Alerts raised inside a window are recorded but not sent to a channel, so planned work does not wake anybody."
          />
        }
      />

      {editing && (
        <MaintenanceDrawer window={editing === 'new' ? null : editing} onClose={() => setEditing(null)} />
      )}
    </Panel>
  );
}

// ---------------------------------------------------------------- channels

function Channels() {
  const [editing, setEditing] = useState<Channel | 'new' | null>(null);
  const channels = useChannels();
  const remove = useRpcMutation<Deleted, IdParams>('notify.channels.delete', { invalidates: ['notify.'] });
  const test = useRpcMutation<ChannelTestResult, IdParams>('notify.channels.test', {
    invalidates: ['notify.'],
    errorToast: 'The test message could not be delivered',
  });

  const columns: TableColumn<Channel>[] = [
    {
      id: 'name',
      header: 'Channel',
      required: true,
      sortValue: (channel) => channel.name,
      cell: (channel) => (
        <span className={styles.cell}>
          <span className={styles.cellText}>{channel.name}</span>
          <Badge variant="outline">{channel.kind}</Badge>
          {!channel.enabled && <Badge tone="idle">Off</Badge>}
        </span>
      ),
    },
    {
      id: 'secret',
      header: 'Secret',
      width: 120,
      sortValue: (channel) => (channel.has_secret ? 0 : 1),
      cell: (channel) =>
        channel.has_secret ? (
          <Badge tone="ok" title="Held in the vault; it is never shown again">
            Set
          </Badge>
        ) : (
          <Badge tone="warn" title="Without it this channel cannot send">
            Not set
          </Badge>
        ),
    },
    {
      id: 'min',
      header: 'From',
      width: 112,
      sortValue: (channel) => channel.min_severity,
      cell: (channel) => `${severityLabel(channel.min_severity)} up`,
    },
    {
      id: 'last',
      header: 'Last sent',
      width: 140,
      sortValue: (channel) => channel.last_sent_at ?? null,
      cell: (channel) =>
        channel.last_sent_at == null ? <span className={styles.missing}>Never</span> : <TimeAgo at={channel.last_sent_at} />,
    },
    {
      id: 'error',
      header: 'Last error',
      sortValue: (channel) => channel.last_error ?? '',
      cell: (channel) =>
        channel.last_error ? (
          <span className={styles.cellText} title={channel.last_error}>
            {channel.last_error}
          </span>
        ) : (
          <span className={styles.muted}>None</span>
        ),
    },
  ];

  return (
    <Panel
      title="Notification channels"
      actions={
        <Button size="sm" icon={Plus} onClick={() => setEditing('new')}>
          New channel
        </Button>
      }
      padding="none"
    >
      <Table
        id="monitoring.channels"
        aria-label="Notification channels"
        columns={columns}
        rows={channels.data ?? []}
        rowKey={(channel) => channel.id}
        loading={channels.isPending}
        maxHeight={320}
        onRowActivate={setEditing}
        rowActions={(channel) => [
          { label: 'Edit…', onSelect: () => setEditing(channel) },
          {
            label: 'Send a test message',
            icon: Send,
            onSelect: () =>
              test.mutate(
                { id: channel.id },
                {
                  onSuccess: (result) =>
                    toast.success(
                      `Test message sent to ${channel.name}`,
                      result.attempts > 1
                        ? { description: `It took ${formatCount(result.attempts)} attempts.` }
                        : undefined,
                    ),
                },
              ),
          },
          { type: 'separator' },
          {
            label: 'Delete…',
            icon: Trash,
            tone: 'danger',
            onSelect: async () => {
              const ok = await confirm({
                title: `Delete the channel “${channel.name}”?`,
                description: 'Its URL or password is removed from the vault as well. Rules that name it stop notifying.',
                confirmLabel: 'Delete',
                tone: 'danger',
              });
              if (ok) remove.mutate({ id: channel.id });
            },
          },
        ]}
        empty={
          <EmptyState
            size="compact"
            icon={BellRing}
            title="No channels"
            description="Without a channel alerts are still raised and recorded — they just do not leave the application."
          />
        }
      />

      {editing && <ChannelDrawer channel={editing === 'new' ? null : editing} onClose={() => setEditing(null)} />}
    </Panel>
  );
}
