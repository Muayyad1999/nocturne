/**
 * Automation — run something across many devices, and keep what they said.
 *
 * The route is `automation/:tab?`; `?run=<id>` opens one run instead of a tab,
 * which keeps the link shareable and leaves the route table to the shell.
 * Everything on screen comes from `nocturne-automation`
 * (`docs/dev/features/automation.md`, types in `src/api/types/automation.ts`)
 * and is kept up to date by its events rather than by polling.
 *
 * Two promises this screen has to keep, because the feature is the one that
 * reaches the most devices at once:
 *
 * 1. **A run is not a change.** `runner.run` sends only commands the device's
 *    own policy calls read-only, and the backend refuses the whole run up
 *    front if one of them is not. A template "apply" drafts a change and
 *    touches nothing. The one exception, `ansible.run`, says on every screen
 *    it appears on that it has none of the change pipeline's guarantees.
 * 2. **A run never shrinks.** Every device the selection picked has a result,
 *    and "answered", "failed", "not reached" and "skipped" are four different
 *    things with four different words (`vocab.ts`).
 *
 * Automation is a feature crate, so this build may not have it: the screen
 * asks `app.methods` once and says so plainly rather than letting every call
 * fail.
 */

import { CalendarClock, ClipboardList, FileCode, Play, SquareTerminal, Terminal, Workflow } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useNavigate, useParams, useSearchParams } from 'react-router';
import { useEvent, useHasMethod } from '@/api/hooks';
import type { Run } from '@/api/types/automation';
import { registerCommand } from '@/app/CommandPalette';
import { destination } from '@/app/destinations';
import { usePageHeader } from '@/app/Header';
import { Page } from '@/app/Page';
import { useStatusItem } from '@/app/StatusBar';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { Skeleton } from '@/design/Skeleton';
import { TabPanel, Tabs, type TabItem } from '@/design/Tabs';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount } from '@/lib/format';
import { AnsibleTab } from './Ansible';
import styles from './Automation.module.css';
import { useRun, useSchedules } from './data';
import { given, orNone } from './given';
import { PlaybooksTab } from './Playbooks';
import { RunTab } from './RunTab';
import { RunView } from './RunView';
import { SchedulesTab } from './Schedules';
import { TasksTab } from './Tasks';
import { TemplatesTab } from './Templates';
import { reportRun } from './useRun';
import { whenText } from './vocab';

const { label: areaLabel, path: areaPath, icon: areaIcon } = destination('automation');

const TABS: readonly TabItem[] = [
  { id: 'run', label: 'Run', icon: Play },
  { id: 'tasks', label: 'Tasks', icon: ClipboardList },
  { id: 'templates', label: 'Templates', icon: FileCode },
  { id: 'playbooks', label: 'Playbooks', icon: Workflow },
  { id: 'schedules', label: 'Schedules', icon: CalendarClock },
  { id: 'ansible', label: 'Ansible', icon: Terminal },
];

const DEFAULT_TAB = 'run';

/** The toolbar's primary action, per tab. Nothing for the tabs that own theirs. */
const NEW_LABEL: Record<string, string> = {
  tasks: 'New task',
  templates: 'New template',
  playbooks: 'New playbook',
  schedules: 'New schedule',
};

export default function AutomationPage() {
  const installed = useHasMethod('runner.run');
  if (installed === undefined) return <Loading />;
  if (!installed) return <NotInstalled />;
  return <Automation />;
}

function Loading() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <Skeleton shape="block" height={32} width={420} />
      <Skeleton shape="block" height={320} />
    </Page>
  );
}

function NotInstalled() {
  usePageHeader({ title: areaLabel });
  return (
    <Page>
      <EmptyState
        icon={areaIcon}
        title="Automation is not in this build"
        description="This copy of Nocturne was assembled without the automation feature, so nothing can be run across devices from here. The rest of the application is unaffected."
      />
    </Page>
  );
}

function Automation() {
  const { tab } = useParams();
  const [search, setSearch] = useSearchParams();
  const navigate = useNavigate();
  const current = TABS.some((entry) => entry.id === tab) ? (tab as string) : DEFAULT_TAB;
  const runId = search.get('run');

  /** The run whose outcome this screen will report once, when it ends. */
  const [watched, setWatched] = useState<string | null>(null);
  const [creating, setCreating] = useState<string | null>(null);
  const schedules = useSchedules();

  const openRun = (next: string) => {
    setWatched(next);
    const params = new URLSearchParams(search);
    params.set('run', next);
    setSearch(params);
  };

  // `ansible.run` answers with a job and no run id; the run record it opens
  // arrives on `automation.runs.changed` carrying that job id.
  const [watchedJob, setWatchedJob] = useState<string | null>(null);
  useEvent<Run>('automation.runs.changed', (run) => {
    if (watchedJob !== null && run.job_id === watchedJob) {
      setWatchedJob(null);
      openRun(run.id);
    }
  });

  const soonest = (schedules.data ?? [])
    .filter((schedule) => schedule.enabled && given(schedule.next_run_at))
    .sort((left, right) => (orNone(left.next_run_at) ?? 0) - (orNone(right.next_run_at) ?? 0))[0];
  useStatusItem(
    schedules.data === undefined || schedules.data.length === 0
      ? null
      : {
          id: 'automation.schedules',
          order: 40,
          label: `${formatCount(schedules.data.filter((schedule) => schedule.enabled).length)} scheduled`,
          tone: soonest === undefined ? 'idle' : 'ok',
          detail:
            soonest === undefined
              ? 'Every schedule is switched off.'
              : `Next: ${soonest.name} — ${whenText(soonest.when)}.`,
          onSelect: () => navigate(`${areaPath}/schedules`),
        },
  );

  useEffect(() => {
    const unregister = [
      registerCommand({
        id: 'automation.run',
        title: 'Run commands on devices',
        group: 'Automation',
        keywords: ['automation', 'show', 'command', 'runner'],
        icon: SquareTerminal,
        run: () => void navigate(`${areaPath}/run`),
      }),
      registerCommand({
        id: 'automation.task',
        title: 'New automation task',
        group: 'Automation',
        keywords: ['automation', 'task', 'saved'],
        icon: ClipboardList,
        run: () => {
          void navigate(`${areaPath}/tasks`);
          setCreating('tasks');
        },
      }),
      registerCommand({
        id: 'automation.template',
        title: 'New configuration template',
        group: 'Automation',
        keywords: ['automation', 'template', 'jinja'],
        icon: FileCode,
        run: () => {
          void navigate(`${areaPath}/templates`);
          setCreating('templates');
        },
      }),
      registerCommand({
        id: 'automation.schedule',
        title: 'New schedule',
        group: 'Automation',
        keywords: ['automation', 'cron', 'nightly'],
        icon: CalendarClock,
        run: () => {
          void navigate(`${areaPath}/schedules`);
          setCreating('schedules');
        },
      }),
    ];
    return () => {
      for (const remove of unregister) remove();
    };
  }, [navigate]);

  usePageHeader({
    title: runId === null ? areaLabel : 'Run',
    breadcrumbs: runId === null ? undefined : [{ label: areaLabel, to: `${areaPath}/${current}` }],
  });

  if (runId !== null) {
    return (
      <Page>
        <Toolbar>
          <Button
            onClick={() => {
              const params = new URLSearchParams(search);
              params.delete('run');
              setSearch(params, { replace: true });
            }}
          >
            ← All runs
          </Button>
        </Toolbar>
        <RunView key={runId} runId={runId} />
        {watched !== null && <Reporter runId={watched} onReported={() => setWatched(null)} />}
      </Page>
    );
  }

  const label = NEW_LABEL[current];

  return (
    <Page>
      <Toolbar>
        <Tabs
          id="automation"
          aria-label="Automation"
          tabs={TABS}
          value={current}
          onChange={(nextTab) => navigate(`${areaPath}/${nextTab}`)}
        />
        <ToolbarSpacer />
        {label !== undefined && (
          <Button variant="primary" onClick={() => setCreating(current)}>
            {label}
          </Button>
        )}
      </Toolbar>

      <TabPanel tabs="automation" value={current} className={styles.panel}>
        {current === 'run' && <RunTab onOpenRun={openRun} />}
        {current === 'tasks' && (
          <TasksTab
            onOpenRun={openRun}
            creating={creating === 'tasks'}
            onCreatingChange={(on) => setCreating(on ? 'tasks' : null)}
          />
        )}
        {current === 'templates' && (
          <TemplatesTab
            creating={creating === 'templates'}
            onCreatingChange={(on) => setCreating(on ? 'templates' : null)}
            onOpenRun={openRun}
            onOpenChange={(changeId) => void navigate(`/changes/${changeId}`)}
          />
        )}
        {current === 'playbooks' && (
          <PlaybooksTab
            creating={creating === 'playbooks'}
            onCreatingChange={(on) => setCreating(on ? 'playbooks' : null)}
            onOpenRun={openRun}
          />
        )}
        {current === 'schedules' && (
          <SchedulesTab
            creating={creating === 'schedules'}
            onCreatingChange={(on) => setCreating(on ? 'schedules' : null)}
          />
        )}
        {current === 'ansible' && <AnsibleTab onOpenJob={setWatchedJob} />}
      </TabPanel>

      {watched !== null && <Reporter runId={watched} onReported={() => setWatched(null)} />}
    </Page>
  );
}

/**
 * Reports a run's outcome once, when it ends — from the run record, which is
 * where the truth about the devices is. It lives at the page level so the
 * report arrives even if the person has moved to another tab meanwhile.
 */
function Reporter({ runId, onReported }: { runId: string; onReported: () => void }) {
  const detail = useRun(runId);
  const done = useRef(false);
  const run = detail.data?.run;

  useEffect(() => {
    if (run === undefined || !given(run.finished_at) || done.current) return;
    done.current = true;
    reportRun(run);
    onReported();
  }, [run, onReported]);

  return null;
}
