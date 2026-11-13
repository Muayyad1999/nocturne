/**
 * Run something now, and the runs that came before.
 *
 * The composer sends read-only commands across a *filter*, not a list: the
 * same selection a task would save. Two things it refuses to hide:
 *
 * * a command that is not read-only on one of the targets is refused by the
 *   backend before anything is sent, and the refusal — which names the
 *   command, the device type and the device — is shown here as it arrived,
 *   because only the engine knows why it said no;
 * * a run is never a configuration change. The sentence above the button says
 *   so, and the confirmation says it again with the device count.
 */

import { ClipboardList, History, Play, Terminal, Trash } from 'lucide-react';
import { useState } from 'react';
import { useNavigate } from 'react-router';
import { useRpcMutation } from '@/api/hooks';
import type { DeleteRunsParams, Removed, Run, RunnerRunParams } from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { formatCount, formatDuration, plural } from '@/lib/format';
import styles from './Automation.module.css';
import { CodeEditor } from './CodeEditor';
import { useRuns } from './data';
import { DeviceSelect } from './DeviceSelect';
import { given } from './given';
import { EMPTY_SELECTION, toSelector, type SelectionDraft } from './selector';
import { TaskDrawer } from './TaskDrawer';
import { useRunJob } from './useRun';
import { kindLabel, originLabel, runElapsed } from './vocab';

export interface RunTabProps {
  onOpenRun: (runId: string) => void;
}

const PLACEHOLDER = 'show version\nshow ip interface brief';

export function RunTab({ onOpenRun }: RunTabProps) {
  const [selection, setSelection] = useState<SelectionDraft>(EMPTY_SELECTION);
  const [commands, setCommands] = useState('');
  const [name, setName] = useState('');
  const [parse, setParse] = useState(false);
  const [dryRun, setDryRun] = useState(false);
  const [saving, setSaving] = useState(false);

  const lines = commands
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '');

  const run = useRunJob('runner.run', {
    onStarted: (result) => {
      if (typeof result.run_id === 'string') onOpenRun(result.run_id);
    },
  });

  const start = async () => {
    const selector = toSelector(selection);
    const ok = await confirm({
      title: `Send ${plural(lines.length, 'command')} to the selected devices?`,
      description: dryRun
        ? 'A dry run reports what would be sent and sends nothing.'
        : 'Only read-only commands are sent. Nothing is configured: the run is refused before anything is sent if a command is not read-only on one of these device types.',
      confirmLabel: dryRun ? 'Dry run' : 'Run',
    });
    if (!ok) return;
    const params: RunnerRunParams = {
      ...selector,
      commands: lines,
      ...(name.trim() === '' ? {} : { name: name.trim() }),
      ...(parse ? { parse: true } : {}),
      ...(dryRun ? { dry_run: true } : {}),
    };
    run.start(params as unknown as Record<string, unknown>);
  };

  return (
    <div className={styles.stack}>
      <Panel
        title="Run read-only commands now"
        actions={
          <div className={styles.panelActions}>
            <Button icon={ClipboardList} disabled={lines.length === 0} onClick={() => setSaving(true)}>
              Save as a task…
            </Button>
            <Button
              variant="primary"
              icon={Play}
              loading={run.starting}
              disabled={lines.length === 0}
              onClick={() => void start()}
            >
              {dryRun ? 'Dry run' : 'Run'}
              {lines.length > 0 ? ` ${plural(lines.length, 'command')}` : ''}…
            </Button>
          </div>
        }
      >
        <div className={styles.composer}>
          <div className={styles.composerWhere}>
            <h4 className={styles.subheading}>Where</h4>
            <DeviceSelect id="run" value={selection} onChange={setSelection} />
          </div>
          <div className={styles.composerWhat}>
            <h4 className={styles.subheading}>What</h4>
            <Field
              label="Commands, one per line"
              hint="Read-only commands only — at most 50. Everything that changes a device goes through a change."
            >
              <CodeEditor
                label="Commands, one per line"
                value={commands}
                onChange={setCommands}
                placeholder={PLACEHOLDER}
                minRows={8}
              />
            </Field>
            <Field label="Name this run" hint="Shown in the history. Defaults to the commands themselves.">
              <Input
                value={name}
                placeholder={lines[0] ?? 'show version'}
                onChange={(event) => setName(event.target.value)}
              />
            </Field>
            <div className={styles.switchRow}>
              <Switch checked={parse} onChange={(event) => setParse(event.target.checked)}>
                Parse the output
              </Switch>
              <span className={styles.hint}>
                Adds a table view where a parser covers the command. Absent is “no parser”, never “nothing matched”.
              </span>
            </div>
            <div className={styles.switchRow}>
              <Switch checked={dryRun} onChange={(event) => setDryRun(event.target.checked)}>
                Dry run
              </Switch>
              <span className={styles.hint}>Says what would be sent, and sends nothing.</span>
            </div>
            {run.error !== null && (
              <div className={styles.refusal} role="alert">
                <h5>
                  <Icon icon={Terminal} size={13} /> Nothing was sent
                </h5>
                <p>{run.error.message}</p>
              </div>
            )}
          </div>
        </div>
      </Panel>

      <RunHistory onOpenRun={onOpenRun} />

      {saving && (
        <TaskDrawer
          open
          task={{
            name: name.trim() || lines[0] || 'New task',
            select: toSelector(selection),
            action: { type: 'commands', commands: lines, ...(parse ? { parse: true } : {}) },
          }}
          onClose={() => setSaving(false)}
        />
      )}
    </div>
  );
}

// ----------------------------------------------------------------- history

export function RunHistory({ onOpenRun, taskId }: { onOpenRun: (runId: string) => void; taskId?: string }) {
  const runs = useRuns();
  const remove = useRpcMutation<Removed, DeleteRunsParams>('runner.delete', {
    invalidates: ['runner.'],
    successToast: 'The run was deleted',
  });
  const navigate = useNavigate();

  const rows = (runs.data ?? []).filter((entry) => taskId === undefined || entry.task_id === taskId);

  const columns: TableColumn<Run>[] = [
    {
      id: 'name',
      header: 'Run',
      required: true,
      sortValue: (entry) => entry.name,
      cell: (entry) => (
        <span className={styles.cell}>
          <span className={styles.cellText} title={entry.name}>
            {entry.name}
          </span>
          {entry.kind !== 'commands' && <Badge variant="outline">{kindLabel(entry.kind)}</Badge>}
          {entry.dry_run && (
            <Badge tone="info" title="Nothing was sent: this was a dry run">
              Dry run
            </Badge>
          )}
          {given(entry.caveat) && (
            <Badge tone="warn" title={entry.caveat}>
              Outside the pipeline
            </Badge>
          )}
          {entry.change_ids.length > 0 && (
            <Badge tone="accent" title="This run drafted a change; nothing has been applied">
              {plural(entry.change_ids.length, 'change')} drafted
            </Badge>
          )}
        </span>
      ),
    },
    {
      id: 'outcome',
      header: 'Devices',
      width: 210,
      sortValue: (entry) => entry.devices,
      cell: (entry) => <Tally run={entry} />,
    },
    {
      id: 'origin',
      header: 'Started by',
      width: 130,
      sortValue: (entry) => entry.origin,
      cell: (entry) => originLabel(entry.origin),
    },
    {
      id: 'started',
      header: 'Started',
      width: 120,
      sortValue: (entry) => entry.started_at,
      cell: (entry) => <TimeAgo at={entry.started_at} />,
    },
    {
      id: 'took',
      header: 'Took',
      width: 90,
      align: 'right',
      sortValue: (entry) => (entry.finished_at ?? Date.now()) - entry.started_at,
      cell: (entry) =>
        given(entry.finished_at) ? runElapsed(entry) : <span className={styles.muted}>running</span>,
    },
  ];

  if (runs.error) {
    return <ErrorState title="The run history could not be read" error={runs.error} onRetry={() => void runs.refetch()} />;
  }

  return (
    <Panel
      title={taskId === undefined ? 'Runs' : 'Runs of this task'}
      actions={<span className={styles.muted}>{formatCount(rows.length)} kept</span>}
      padding="none"
    >
      <Table
        id="automation.runs"
        aria-label="Runs"
        columns={columns}
        rows={rows}
        rowKey={(entry) => entry.id}
        loading={runs.isLoading}
        defaultSort={{ columnId: 'started', direction: 'desc' }}
        onRowActivate={(entry) => onOpenRun(entry.id)}
        rowActions={(entry) => [
          { label: 'Open this run', icon: History, onSelect: () => onOpenRun(entry.id) },
          ...(given(entry.change_ids[0])
            ? [
                {
                  label: 'Open the change it drafted',
                  onSelect: () => navigate(`/changes/${entry.change_ids[0]}`),
                },
              ]
            : []),
          {
            label: 'Delete this run',
            icon: Trash,
            tone: 'danger' as const,
            onSelect: async () => {
              const ok = await confirm({
                title: `Delete the run “${entry.name}”?`,
                description: 'Its per-device output goes with it. The devices are not touched.',
                confirmLabel: 'Delete',
                tone: 'danger',
              });
              if (ok) remove.mutate({ run_ids: [entry.id] });
            },
          },
        ]}
        empty={
          <EmptyState
            size="compact"
            icon={History}
            title="No run yet"
            description="Every run is kept here with what it sent, what each device answered and which devices it never reached."
          />
        }
      />
    </Panel>
  );
}

/** The four outcomes as four separate numbers: they are four different facts. */
export function Tally({ run, size = 'sm' }: { run: Run; size?: 'sm' | 'md' }) {
  const entries: { key: string; count: number; tone: string; title: string }[] = [
    { key: 'ok', count: run.ok, tone: 'ok', title: 'answered' },
    { key: 'failed', count: run.failed, tone: 'crit', title: 'failed' },
    { key: 'unreached', count: run.unreached, tone: 'warn', title: 'could not be reached' },
    { key: 'skipped', count: run.skipped, tone: 'idle', title: 'skipped' },
  ];
  const running = !given(run.finished_at);
  return (
    <span className={styles.tally} data-size={size}>
      {entries
        .filter((entry) => entry.count > 0)
        .map((entry) => (
          <span key={entry.key} className={styles.tallyItem} data-tone={entry.tone} title={`${entry.count} ${entry.title}`}>
            {formatCount(entry.count)} {entry.title === 'could not be reached' ? 'not reached' : entry.title}
          </span>
        ))}
      {/* A run's per-device outcome is written when the run ends, so while it
          is going there is nothing to count — and saying "0 answered" would be
          a different claim from "not finished yet". */}
      {running && <span className={styles.muted}>{plural(run.devices, 'device')}, still running</span>}
      {!running && run.ok + run.failed + run.unreached + run.skipped === 0 && (
        <span className={styles.muted}>no devices</span>
      )}
    </span>
  );
}

/** A run's commands, exactly as they were sent. */
export function SentCommands({ commands }: { commands: readonly string[] }) {
  if (commands.length === 0) return null;
  return (
    <CodeBlock
      title={`${plural(commands.length, 'command')} sent, in this order`}
      code={commands.join('\n')}
      lineNumbers
    />
  );
}

/** How long one device took, for a result row. */
export function Elapsed({ ms }: { ms: number }) {
  return <span className={styles.muted}>{formatDuration(ms)}</span>;
}
