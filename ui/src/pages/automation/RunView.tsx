/**
 * One run: live while it happens, and the whole truth afterwards.
 *
 * While the job is going the screen shows what the job itself says — progress
 * and one line per device, as the backend worded it — because that is all
 * that is known: a device's output is written to the run record when the run
 * finishes, not as each device answers. Afterwards the record is the truth and
 * the comparison is the first thing on screen, which is what this feature is
 * for: one command, fifty devices, and the odd one out in one glance.
 */

import {
  CircleStop,
  Download,
  Play,
  Search,
  SplitSquareHorizontal,
  Trash,
  Server,
} from 'lucide-react';
import { useState } from 'react';
import { useNavigate } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  DeleteRunsParams,
  Exported,
  ExportParams,
  Removed,
  RunDetail,
  RunnerRunParams,
  SearchMatch,
  SearchParams,
} from '@/api/types/automation';
import type { Job, JobCancelled } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { confirm } from '@/design/Dialog';
import { Dialog } from '@/design/Dialog';
import { DropdownMenu } from '@/design/DropdownMenu';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { ProgressBar } from '@/design/ProgressBar';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { formatCount, plural } from '@/lib/format';
import { formatTimestamp } from '@/lib/time';
import styles from './Automation.module.css';
import { CaveatLine } from './Caveat';
import { CompareView } from './Compare';
import { useDevices, useLabDevices, useRun } from './data';
import { given } from './given';
import { RunDevices } from './RunDevices';
import { SentCommands, Tally } from './RunTab';
import { generalLines, liveDevices } from './useRun';
import { describeRun, kindLabel, originLabel, runElapsed } from './vocab';

export interface RunViewProps {
  runId: string;
}

const TABS: readonly TabItem[] = [
  { id: 'compare', label: 'Compare', icon: SplitSquareHorizontal },
  { id: 'devices', label: 'Devices', icon: Server },
  { id: 'search', label: 'Search output', icon: Search },
];

export function RunView({ runId }: RunViewProps) {
  const detail = useRun(runId);
  const labDevices = useLabDevices();
  const [tab, setTab] = useState('compare');
  const [exported, setExported] = useState<{ format: string; text: string } | null>(null);
  const navigate = useNavigate();

  const exportRun = useRpcMutation<Exported, ExportParams>('runner.export');
  const remove = useRpcMutation<Removed, DeleteRunsParams>('runner.delete', { invalidates: ['runner.'] });
  const again = useRpcMutation<{ run_id?: string }, RunnerRunParams>('runner.run', {
    errorToast: 'The run could not be started',
  });
  // While a run is in flight its record carries the job id, so the run can be
  // stopped from here; the backend marks the devices it never got to as
  // skipped rather than failed.
  const stop = useRpcMutation<JobCancelled, { id: string }>('jobs.cancel');

  if (detail.isLoading) return <Skeleton shape="block" height={320} />;
  if (detail.error || detail.data === undefined) {
    return (
      <ErrorState
        title="This run could not be read"
        error={detail.error ?? 'The backend returned nothing'}
        onRetry={() => void detail.refetch()}
      />
    );
  }

  const { run, results } = detail.data;
  const outcome = describeRun(run);
  const running = !given(run.finished_at);

  return (
    <div className={styles.stack}>
      <section className={styles.runHead} aria-label="This run">
        <div className={styles.runHeadTop}>
          <StatusPill tone={outcome.tone}>{outcome.title}</StatusPill>
          <h2 className={styles.runName}>{run.name}</h2>
          <Badge variant="outline">{kindLabel(run.kind)}</Badge>
          {run.dry_run && <Badge tone="info">Dry run — nothing was sent</Badge>}
          <span className={styles.spacer} />
          {running && run.job_id !== '' && (
            <Button
              icon={CircleStop}
              loading={stop.isPending}
              onClick={async () => {
                const ok = await confirm({
                  title: 'Stop this run?',
                  description:
                    'The devices already being worked on finish what they are doing. The ones it has not reached yet are recorded as skipped, not failed.',
                  confirmLabel: 'Stop the run',
                });
                if (ok) stop.mutate({ id: run.job_id });
              }}
            >
              Stop the run
            </Button>
          )}
          {!running && (
            <>
              <DropdownMenu
                aria-label="Export this run"
                trigger={<Button icon={Download}>Export…</Button>}
                items={(['text', 'csv', 'json'] as const).map((format) => ({
                  label: `as ${format.toUpperCase()}`,
                  onSelect: () =>
                    exportRun.mutate(
                      { run_id: run.id, format },
                      { onSuccess: (result) => setExported({ format, text: result.text }) },
                    ),
                }))}
              />
              <Button
                icon={Play}
                loading={again.isPending}
                disabled={run.kind !== 'commands' || results.length === 0}
                title={
                  run.kind === 'commands'
                    ? 'Send the same commands to the same devices again'
                    : 'Only a command run can be repeated from here'
                }
                onClick={async () => {
                  const ok = await confirm({
                    title: `Send these commands to the same ${plural(results.length, 'device')} again?`,
                    description:
                      'The same read-only commands, to the devices this run selected — as a list, not as the filter it came from.',
                    confirmLabel: 'Run again',
                  });
                  if (!ok) return;
                  again.mutate(
                    {
                      device_ids: results.map((result) => result.device_id),
                      commands: run.commands,
                      name: run.name,
                    },
                    {
                      onSuccess: (result) => {
                        if (typeof result.run_id === 'string') navigate(`/automation/run?run=${result.run_id}`);
                      },
                    },
                  );
                }}
              >
                Run again…
              </Button>
              <Button
                icon={Trash}
                onClick={async () => {
                  const ok = await confirm({
                    title: `Delete the run “${run.name}”?`,
                    description: 'Its per-device output goes with it. The devices are not touched.',
                    confirmLabel: 'Delete',
                    tone: 'danger',
                  });
                  if (ok) {
                    remove.mutate({ run_ids: [run.id] }, { onSuccess: () => navigate('/automation/run') });
                  }
                }}
              >
                Delete
              </Button>
            </>
          )}
        </div>

        {/* One line rather than a column of rows: at 1280 the comparison
            below it has to stay above the fold. */}
        <dl className={styles.runMeta}>
          <div>
            <dt>Devices</dt>
            <dd>
              <Tally run={run} size="md" />
            </dd>
          </div>
          <div>
            <dt>Started</dt>
            <dd title={formatTimestamp(run.started_at)}>
              <TimeAgo at={run.started_at} />
            </dd>
          </div>
          <div>
            <dt>{running ? 'Running for' : 'Took'}</dt>
            <dd>{runElapsed(run)}</dd>
          </div>
          <div>
            <dt>Started by</dt>
            <dd>{originLabel(run.origin)}</dd>
          </div>
          {given(run.task_id) && (
            <div>
              <dt>From the task</dt>
              <dd className={styles.mono}>{run.task_id}</dd>
            </div>
          )}
        </dl>

        {given(run.caveat) && <CaveatLine caveat={run.caveat} />}

        {run.change_ids.length > 0 && (
          <p className={styles.drafted} role="note">
            This run drafted {plural(run.change_ids.length, 'change')} and applied nothing. Each one still has to be
            planned, approved and applied in Changes.{' '}
            {run.change_ids.map((changeId) => (
              <Button key={changeId} size="sm" onClick={() => navigate(`/changes/${changeId}`)}>
                Open the change
              </Button>
            ))}
          </p>
        )}
      </section>

      {running && <Live run={detail.data} />}
      <SentCommands commands={run.commands} />

      {!running && (
        <>
          <Tabs id="run" aria-label="This run" tabs={TABS} value={tab} onChange={setTab} />
          <TabPanel tabs="run" value={tab}>
            {tab === 'compare' && <CompareView run={run} labDevices={labDevices} />}
            {tab === 'devices' && <RunDevices run={run} results={results} labDevices={labDevices} />}
            {tab === 'search' && <SearchOutput runId={run.id} />}
          </TabPanel>
        </>
      )}
      {/* While it runs, the live panel above is what is known; the empty
          per-device view would only repeat it. */}
      {running && results.length > 0 && <RunDevices run={run} results={results} labDevices={labDevices} />}

      {exported !== null && (
        <Dialog
          open
          onClose={() => setExported(null)}
          title={`This run as ${exported.format.toUpperCase()}`}
          description="Copy it, or save it to a file."
          footer={
            <>
              <Button
                icon={Download}
                onClick={() => {
                  download(`${run.name || 'run'}.${exported.format === 'text' ? 'txt' : exported.format}`, exported.text);
                  toast.success('Saved');
                }}
              >
                Save to a file
              </Button>
              <Button variant="primary" onClick={() => setExported(null)}>
                Close
              </Button>
            </>
          }
        >
          <CodeBlock code={exported.text} maxHeight={420} />
        </Dialog>
      )}
    </div>
  );
}

/** Hands the text to the browser as a file. Nothing leaves the machine. */
function download(name: string, text: string): void {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
}

// -------------------------------------------------------------------- live

/**
 * What is known while the run is in flight: the job's progress, and the job's
 * own line per device. The lines are the backend's words and their level is
 * the backend's judgement — nothing here guesses a state from the text.
 */
function Live({ run: detail }: { run: RunDetail }) {
  const { run } = detail;
  const devices = useDevices();
  const watched = useRpc<Job>(
    'jobs.get',
    { id: run.job_id },
    { enabled: run.job_id !== '', invalidateOn: ['jobs.changed', 'jobs.log'] },
  );
  const current = watched.data;
  const lines = liveDevices(current?.log);
  const nameOf = (deviceId: string) =>
    (devices.data ?? []).find((device) => device.id === deviceId)?.name ?? deviceId;
  const done = current?.done ?? 0;
  const total = current?.total === 0 ? run.devices : (current?.total ?? run.devices);

  return (
    <Panel title="While it runs">
      <div className={styles.live}>
        <ProgressBar
          value={done}
          max={Math.max(total, 1)}
          valueText={`${formatCount(done)} of ${plural(total, 'device')}`}
        />
        <p className={styles.hint}>
          {formatCount(done)} of {plural(total, 'device')} finished. What each device printed is written to the run when
          the whole run ends; what the job knows so far is below.
        </p>
        {current === undefined && run.job_id === '' && (
          <p className={styles.hint}>
            This run’s record carries no job id, so its progress cannot be followed from here — only its result, when it
            lands.
          </p>
        )}
        {lines.size > 0 && (
          <ul className={styles.liveList}>
            {[...lines.values()].map((line) => (
              <li key={line.deviceId} data-level={line.level}>
                <strong>{nameOf(line.deviceId)}</strong>
                <span>{line.message}</span>
              </li>
            ))}
          </ul>
        )}
        {generalLines(current?.log).length > 0 && (
          <details className={styles.full}>
            <summary>The job’s own log</summary>
            <CodeBlock
              copyable={false}
              code={generalLines(current?.log)
                .map((line) => `${line.level === 'info' ? '' : `${line.level}: `}${line.message}`)
                .join('\n')}
              maxHeight={180}
            />
          </details>
        )}
      </div>
    </Panel>
  );
}

// ------------------------------------------------------------------ search

/** Every line of this run's output that matches, with the device it came from. */
function SearchOutput({ runId }: { runId: string }) {
  const [query, setQuery] = useState('');
  const [regex, setRegex] = useState(false);
  const [asked, setAsked] = useState<{ query: string; regex: boolean } | null>(null);

  const matches = useRpc<SearchMatch[]>(
    'runner.search',
    asked === null ? undefined : ({ run_id: runId, query: asked.query, regex: asked.regex } satisfies SearchParams),
    { enabled: asked !== null },
  );

  const columns: TableColumn<SearchMatch>[] = [
    { id: 'device', header: 'Device', width: 180, required: true, sortValue: (match) => match.name, cell: (match) => match.name },
    { id: 'command', header: 'Command', width: 180, mono: true, sortValue: (match) => match.command, cell: (match) => match.command },
    { id: 'line_no', header: 'Line', width: 70, align: 'right', sortValue: (match) => match.line_no, cell: (match) => match.line_no },
    { id: 'line', header: 'What it says', mono: true, cell: (match) => <span title={match.line}>{match.line}</span> },
  ];

  return (
    <div className={styles.stack}>
      <form
        className={styles.searchBar}
        onSubmit={(event) => {
          event.preventDefault();
          if (query.trim() !== '') setAsked({ query: query.trim(), regex });
        }}
      >
        <Field label="Find in everything this run collected">
          <Input value={query} placeholder="serial number" onChange={(event) => setQuery(event.target.value)} />
        </Field>
        <Switch checked={regex} onChange={(event) => setRegex(event.target.checked)}>
          Regular expression
        </Switch>
        <Button type="submit" variant="primary" icon={Search} disabled={query.trim() === ''}>
          Search
        </Button>
      </form>

      {matches.error && (
        <ErrorState title="That search could not be run" error={matches.error} onRetry={() => void matches.refetch()} />
      )}
      {asked !== null && matches.data !== undefined && (
        <Table
          id="automation.search"
          aria-label="Matching lines"
          columns={columns}
          rows={matches.data}
          rowKey={(match) => `${match.device_id}|${match.command}|${match.line_no}`}
          loading={matches.isLoading}
          empty={
            <EmptyState
              size="compact"
              icon={Search}
              title="No line matches"
              description={`Nothing in this run’s output contains “${asked.query}”.`}
            />
          }
        />
      )}
      {asked === null && (
        <p className={styles.hint}>
          Searches the output already stored for this run — no device is touched. A regular expression is matched line by
          line.
        </p>
      )}
    </div>
  );
}
