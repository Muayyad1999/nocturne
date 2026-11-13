/**
 * When things run by themselves: schedules (a clock) and triggers (an event).
 *
 * Two rules of the backend are worth having on screen, because they are the
 * reason this is safe to leave switched on:
 *
 * * **nothing that changes a device is on the allow-list.** A schedule may
 *   call `config.backup`, `collect.run`, `discovery.scan`, `runner.run`,
 *   `playbooks.run`, `compliance.run`, `insights.scan`, `reports.generate` or
 *   `tasks.run` — and a method this build does not have is refused at save
 *   time, with that as the reason.
 * * **one run at a time.** A schedule whose previous run is still going does
 *   not start a second, and a trigger holds its burst instead of overlapping.
 *
 * The next five firings come from `schedules.preview`, in this machine's
 * local time, which is also what the backend walks: 02:30 stays 02:30 across
 * a clock change.
 */

import { CalendarClock, Play, Radio, Pencil, Trash, Zap } from 'lucide-react';
import { useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  ActionCall,
  Deleted,
  IdParams,
  Preview,
  PreviewParams,
  Schedule,
  ScheduleSaveParams,
  Task,
  Trigger,
  TriggerSaveParams,
  When,
} from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { formatCount } from '@/lib/format';
import { formatDateTime, formatUtcOffset } from '@/lib/time';
import styles from './Automation.module.css';
import { useSchedules, useTasks, useTriggers } from './data';
import { given, orNone } from './given';
import { varsFromText } from './Variables';
import { CRON_HELP, debounceText, whenText } from './vocab';

export interface SchedulesTabProps {
  creating: boolean;
  onCreatingChange: (creating: boolean) => void;
}

export function SchedulesTab({ creating, onCreatingChange }: SchedulesTabProps) {
  return (
    <div className={styles.stack}>
      <Schedules creating={creating} onCreatingChange={onCreatingChange} />
      <Triggers />
    </div>
  );
}

// --------------------------------------------------------------- schedules

function Schedules({ creating, onCreatingChange }: SchedulesTabProps) {
  const schedules = useSchedules();
  const [editing, setEditing] = useState<Schedule | null>(null);
  const remove = useRpcMutation<Deleted, IdParams>('schedules.delete', {
    invalidates: ['schedules.'],
    successToast: 'The schedule was deleted',
  });
  const save = useRpcMutation<Schedule, ScheduleSaveParams>('schedules.save', { invalidates: ['schedules.'] });
  const runNow = useRpcMutation<unknown, IdParams>('schedules.run_now', {
    errorToast: 'It could not be run now',
  });

  const columns: TableColumn<Schedule>[] = [
    {
      id: 'enabled',
      header: 'On',
      width: 78,
      sortValue: (schedule) => (schedule.enabled ? 0 : 1),
      cell: (schedule) => (
        <Switch
          checked={schedule.enabled}
          aria-label={`${schedule.name} is ${schedule.enabled ? 'on' : 'off'}`}
          onChange={(event) =>
            save.mutate({
              id: schedule.id,
              name: schedule.name,
              enabled: event.target.checked,
              when: schedule.when,
              action: schedule.action,
            })
          }
        />
      ),
    },
    {
      id: 'name',
      header: 'Schedule',
      required: true,
      sortValue: (schedule) => schedule.name,
      cell: (schedule) => (
        <span className={styles.cellText} title={schedule.name}>
          {schedule.name}
        </span>
      ),
    },
    {
      id: 'when',
      header: 'When',
      width: 220,
      sortValue: (schedule) => schedule.when.type,
      cell: (schedule) => whenText(schedule.when),
    },
    {
      id: 'action',
      header: 'Runs',
      width: 200,
      sortValue: (schedule) => schedule.action.method,
      cell: (schedule) => <code className={styles.method}>{schedule.action.method}</code>,
    },
    {
      id: 'next',
      header: 'Next',
      width: 150,
      sortValue: (schedule) => orNone(schedule.next_run_at) ?? Number.MAX_SAFE_INTEGER,
      cell: (schedule) =>
        schedule.enabled && given(schedule.next_run_at) ? (
          <span title={`${formatDateTime(schedule.next_run_at)} ${formatUtcOffset(schedule.next_run_at)}`}>
            <TimeAgo at={schedule.next_run_at} />
          </span>
        ) : (
          <span className={styles.muted}>{schedule.enabled ? 'Not worked out' : 'Switched off'}</span>
        ),
    },
    {
      id: 'last',
      header: 'Last',
      width: 160,
      sortValue: (schedule) => schedule.last_run_at ?? 0,
      cell: (schedule) =>
        !given(schedule.last_run_at) ? (
          <span className={styles.muted}>Never</span>
        ) : (
          <span className={styles.cell}>
            <StatusPill tone={schedule.last_ok === true ? 'ok' : 'crit'}>
              {schedule.last_ok === true ? 'Ran' : 'Failed'}
            </StatusPill>
            <TimeAgo at={schedule.last_run_at} />
          </span>
        ),
    },
  ];

  if (schedules.error) {
    return (
      <ErrorState title="Schedules could not be read" error={schedules.error} onRetry={() => void schedules.refetch()} />
    );
  }

  return (
    <>
      <Panel title="Schedules" padding="none" actions={<span className={styles.muted}>Local time ({formatUtcOffset(Date.now())})</span>}>
        <Table
          id="automation.schedules"
          aria-label="Schedules"
          columns={columns}
          rows={schedules.data ?? []}
          rowKey={(schedule) => schedule.id}
          loading={schedules.isLoading}
          onRowActivate={(schedule) => setEditing(schedule)}
          rowActions={(schedule) => [
            {
              label: 'Run it now…',
              icon: Play,
              onSelect: async () => {
                const ok = await confirm({
                  title: `Run “${schedule.name}” now?`,
                  description: (
                    <>
                      It calls <code>{schedule.action.method}</code> straight away, without waiting for its next
                      turn. If its previous run is still going, the backend refuses rather than overlapping.
                    </>
                  ),
                  confirmLabel: 'Run now',
                });
                if (ok) {
                  runNow.mutate({ id: schedule.id }, { onSuccess: () => toast.success(`${schedule.name} was started`) });
                }
              },
            },
            { label: 'Edit…', icon: Pencil, onSelect: () => setEditing(schedule) },
            {
              label: 'Delete',
              icon: Trash,
              tone: 'danger' as const,
              onSelect: async () => {
                const ok = await confirm({
                  title: `Delete the schedule “${schedule.name}”?`,
                  description: 'Nothing will run on this clock any more.',
                  confirmLabel: 'Delete',
                  tone: 'danger',
                });
                if (ok) remove.mutate({ id: schedule.id });
              },
            },
          ]}
          empty={
            <EmptyState
              size="compact"
              icon={CalendarClock}
              title="Nothing runs on a clock yet"
              description="A schedule calls one method — a task, a backup, a collection, a scan — at a time of your choosing. Nothing that changes a device can be scheduled."
              action={
                <Button variant="primary" onClick={() => onCreatingChange(true)}>
                  New schedule
                </Button>
              }
            />
          }
        />
      </Panel>

      {(creating || editing !== null) && (
        <ScheduleDrawer
          schedule={editing}
          onClose={() => {
            setEditing(null);
            onCreatingChange(false);
          }}
        />
      )}
    </>
  );
}

const METHODS: { value: string; label: string }[] = [
  { value: 'tasks.run', label: 'tasks.run — a saved task' },
  { value: 'runner.run', label: 'runner.run — commands across a filter' },
  { value: 'playbooks.run', label: 'playbooks.run — a playbook' },
  { value: 'config.backup', label: 'config.backup — back up configuration' },
  { value: 'collect.run', label: 'collect.run — collect from devices' },
  { value: 'discovery.scan', label: 'discovery.scan — discover devices' },
  { value: 'compliance.run', label: 'compliance.run — check compliance' },
  { value: 'insights.scan', label: 'insights.scan — look for insights' },
  { value: 'reports.generate', label: 'reports.generate — generate a report' },
];

type Day = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';

const DAYS: { id: Day; label: string }[] = [
  { id: 'mon', label: 'Mon' },
  { id: 'tue', label: 'Tue' },
  { id: 'wed', label: 'Wed' },
  { id: 'thu', label: 'Thu' },
  { id: 'fri', label: 'Fri' },
  { id: 'sat', label: 'Sat' },
  { id: 'sun', label: 'Sun' },
];

function ScheduleDrawer({ schedule, onClose }: { schedule: Schedule | null; onClose: () => void }) {
  const [name, setName] = useState(schedule?.name ?? '');
  const [enabled, setEnabled] = useState(schedule?.enabled ?? true);
  const [type, setType] = useState<When['type']>(schedule?.when.type ?? 'daily');
  const [at, setAt] = useState(schedule?.when.type === 'daily' ? schedule.when.at : '02:30');
  const [days, setDays] = useState<Day[]>(schedule?.when.type === 'daily' ? [...(schedule.when.days ?? [])] : []);
  const [everyS, setEveryS] = useState(
    schedule?.when.type === 'interval' ? String(schedule.when.every_s) : '3600',
  );
  const [expr, setExpr] = useState(schedule?.when.type === 'cron' ? schedule.when.expr : '0 3 * * *');
  const [method, setMethod] = useState(schedule?.action.method ?? 'tasks.run');
  const [taskId, setTaskId] = useState(
    typeof schedule?.action.params?.['id'] === 'string' ? (schedule.action.params['id'] as string) : '',
  );
  const [paramsText, setParamsText] = useState(
    schedule === null || schedule.action.method === 'tasks.run'
      ? ''
      : JSON.stringify(schedule.action.params ?? {}, null, 0),
  );

  const tasks = useTasks();
  const save = useRpcMutation<Schedule, ScheduleSaveParams>('schedules.save', {
    invalidates: ['schedules.'],
    errorToast: 'The schedule could not be saved',
  });

  const when: When =
    type === 'interval'
      ? { type: 'interval', every_s: Math.max(1, Number.parseInt(everyS, 10) || 0) }
      : type === 'cron'
        ? { type: 'cron', expr }
        : { type: 'daily', at, ...(days.length > 0 ? { days } : {}) };

  const preview = useRpc<Preview>('schedules.preview', { when, count: 5 } satisfies PreviewParams);
  const parsed = method === 'tasks.run' ? { id: taskId } : varsFromText(paramsText);
  const action: ActionCall = { method, ...(parsed === null ? {} : { params: parsed }) };
  const incomplete =
    name.trim() === '' || parsed === null || (method === 'tasks.run' && taskId === '');

  return (
    <Drawer
      open
      onClose={onClose}
      size="lg"
      title={schedule === null ? 'New schedule' : `Edit ${schedule.name}`}
      description="A clock and one method. Nothing that configures a device can be scheduled."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            loading={save.isPending}
            disabled={incomplete}
            onClick={() =>
              save.mutate(
                { ...(schedule === null ? {} : { id: schedule.id }), name: name.trim(), enabled, when, action },
                { onSuccess: onClose },
              )
            }
          >
            {schedule === null ? 'Create the schedule' : 'Save'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field label="Name" required>
          <Input value={name} onChange={(event) => setName(event.target.value)} />
        </Field>
        <Switch checked={enabled} onChange={(event) => setEnabled(event.target.checked)}>
          Enabled
        </Switch>

        <h4 className={styles.subheading}>When</h4>
        <Field label="Kind of schedule">
          <Select
            value={type}
            options={[
              { value: 'daily', label: 'At a time of day' },
              { value: 'interval', label: 'Every so often' },
              { value: 'cron', label: 'A cron expression' },
            ]}
            onChange={(event) => setType(event.target.value as When['type'])}
          />
        </Field>
        {type === 'daily' && (
          <>
            <Field label="At" hint="24-hour, local time. A time that does not exist on a clock-change day is skipped.">
              <Input value={at} placeholder="02:30" onChange={(event) => setAt(event.target.value)} />
            </Field>
            <fieldset className={styles.days}>
              <legend>On these days (none means every day)</legend>
              {DAYS.map((day) => (
                <label key={day.id}>
                  <input
                    type="checkbox"
                    checked={days.includes(day.id)}
                    onChange={(event) =>
                      setDays(
                        event.target.checked ? [...days, day.id] : days.filter((entry) => entry !== day.id),
                      )
                    }
                  />
                  {day.label}
                </label>
              ))}
            </fieldset>
          </>
        )}
        {type === 'interval' && (
          <Field label="Every, in seconds" hint="From the moment it is saved.">
            <Input
              value={everyS}
              inputMode="numeric"
              onChange={(event) => setEveryS(event.target.value.replace(/[^0-9]/g, ''))}
            />
          </Field>
        )}
        {type === 'cron' && (
          <Field label="Cron expression" hint={CRON_HELP}>
            <Input value={expr} onChange={(event) => setExpr(event.target.value)} />
          </Field>
        )}

        <div className={styles.preview}>
          <h5>
            <Icon icon={CalendarClock} size={13} /> The next {formatCount(preview.data?.next.length ?? 5)} firings
          </h5>
          {preview.error ? (
            <p className={styles.previewError}>{preview.error.message}</p>
          ) : (
            <ol>
              {(preview.data?.next ?? []).map((instant) => (
                <li key={instant}>
                  <span className={styles.mono}>{formatDateTime(instant)}</span>{' '}
                  <span className={styles.muted}>
                    {formatUtcOffset(instant)} · <TimeAgo at={instant} />
                  </span>
                </li>
              ))}
            </ol>
          )}
          <p className={styles.hint}>{whenText(when)}, in this machine's local time.</p>
        </div>

        <h4 className={styles.subheading}>What it runs</h4>
        <Field label="Method" hint="Only these may be scheduled, and a method this build does not have is refused when you save.">
          <Select
            value={method}
            options={METHODS}
            onChange={(event) => setMethod(event.target.value)}
          />
        </Field>
        {method === 'tasks.run' ? (
          <Field label="Task" required>
            <Select
              value={taskId}
              options={[
                { value: '', label: 'Choose a task…' },
                ...(tasks.data ?? []).map((task: Task) => ({ value: task.id, label: task.name })),
              ]}
              onChange={(event) => setTaskId(event.target.value)}
            />
          </Field>
        ) : (
          <Field
            label="Parameters as JSON"
            hint="Exactly what the method takes, e.g. {&quot;filter&quot;:{&quot;lab&quot;:true},&quot;commands&quot;:[&quot;show version&quot;]}."
            error={paramsText.trim() !== '' && varsFromText(paramsText) === null ? 'This is not valid JSON' : undefined}
          >
            <Input value={paramsText} placeholder="{}" onChange={(event) => setParamsText(event.target.value)} />
          </Field>
        )}
      </div>
    </Drawer>
  );
}

// ---------------------------------------------------------------- triggers

function Triggers() {
  const triggers = useTriggers();
  const [editing, setEditing] = useState<Trigger | 'new' | null>(null);
  const save = useRpcMutation<Trigger, TriggerSaveParams>('triggers.save', { invalidates: ['triggers.'] });
  const remove = useRpcMutation<Deleted, IdParams>('triggers.delete', {
    invalidates: ['triggers.'],
    successToast: 'The trigger was deleted',
  });
  const fire = useRpcMutation<unknown, IdParams>('triggers.fire_now', { errorToast: 'It could not be fired' });

  const columns: TableColumn<Trigger>[] = [
    {
      id: 'enabled',
      header: 'On',
      width: 78,
      cell: (trigger) => (
        <Switch
          checked={trigger.enabled}
          aria-label={`${trigger.name} is ${trigger.enabled ? 'on' : 'off'}`}
          onChange={(event) =>
            save.mutate({
              id: trigger.id,
              name: trigger.name,
              enabled: event.target.checked,
              topic: trigger.topic,
              action: trigger.action,
            })
          }
        />
      ),
    },
    {
      id: 'name',
      header: 'Trigger',
      required: true,
      sortValue: (trigger) => trigger.name,
      cell: (trigger) => (
        <span className={styles.cell}>
          <span className={styles.cellText}>{trigger.name}</span>
          {trigger.pass_device && (
            <Badge variant="outline" title="The device the event was about is passed to the action">
              passes the device
            </Badge>
          )}
        </span>
      ),
    },
    {
      id: 'topic',
      header: 'On the event',
      width: 190,
      sortValue: (trigger) => trigger.topic,
      cell: (trigger) => <code className={styles.method}>{trigger.topic}</code>,
    },
    {
      id: 'debounce',
      header: 'Bursts',
      width: 230,
      sortValue: (trigger) => trigger.debounce_s,
      cell: (trigger) => <span className={styles.cellText}>{debounceText(trigger.debounce_s)}</span>,
    },
    {
      id: 'action',
      header: 'Runs',
      width: 170,
      cell: (trigger) => <code className={styles.method}>{trigger.action.method}</code>,
    },
    {
      id: 'fired',
      header: 'Fired',
      width: 150,
      sortValue: (trigger) => trigger.last_fired_at ?? 0,
      cell: (trigger) =>
        !given(trigger.last_fired_at) ? (
          <span className={styles.muted}>Never</span>
        ) : (
          <span className={styles.cell}>
            <TimeAgo at={trigger.last_fired_at} />
            <span className={styles.muted}>({formatCount(trigger.fired_count)}×)</span>
          </span>
        ),
    },
  ];

  if (triggers.error) {
    return <ErrorState title="Triggers could not be read" error={triggers.error} onRetry={() => void triggers.refetch()} />;
  }

  return (
    <>
      <Panel
        title="Triggers"
        padding="none"
        actions={
          <Button icon={Zap} onClick={() => setEditing('new')}>
            New trigger
          </Button>
        }
      >
        <Table
          id="automation.triggers"
          aria-label="Triggers"
          columns={columns}
          rows={triggers.data ?? []}
          rowKey={(trigger) => trigger.id}
          loading={triggers.isLoading}
          onRowActivate={(trigger) => setEditing(trigger)}
          rowActions={(trigger) => [
            {
              label: 'Fire it now…',
              icon: Play,
              onSelect: async () => {
                const ok = await confirm({
                  title: `Fire “${trigger.name}” now?`,
                  description: (
                    <>
                      It calls <code>{trigger.action.method}</code> as if the event had just arrived, with no device
                      from an event to pass it.
                    </>
                  ),
                  confirmLabel: 'Fire now',
                });
                if (ok) fire.mutate({ id: trigger.id }, { onSuccess: () => toast.success(`${trigger.name} fired`) });
              },
            },
            { label: 'Edit…', icon: Pencil, onSelect: () => setEditing(trigger) },
            {
              label: 'Delete',
              icon: Trash,
              tone: 'danger' as const,
              onSelect: async () => {
                const ok = await confirm({
                  title: `Delete the trigger “${trigger.name}”?`,
                  description: 'Nothing will run on this event any more.',
                  confirmLabel: 'Delete',
                  tone: 'danger',
                });
                if (ok) remove.mutate({ id: trigger.id });
              },
            },
          ]}
          empty={
            <EmptyState
              size="compact"
              icon={Radio}
              title="Nothing runs on an event yet"
              description="A trigger watches an event topic — alerts.raised, devices.state — collects the burst and fires once. It may not watch what automation publishes itself: that would be a loop."
              action={
                <Button variant="primary" onClick={() => setEditing('new')}>
                  New trigger
                </Button>
              }
            />
          }
        />
      </Panel>

      {editing !== null && (
        <TriggerDrawer trigger={editing === 'new' ? null : editing} onClose={() => setEditing(null)} />
      )}
    </>
  );
}

const TOPICS = [
  'alerts.raised',
  'alerts.changed',
  'devices.state',
  'devices.changed',
  'changes.changed',
  'config.changed',
  'discovery.found',
  'jobs.changed',
];

function TriggerDrawer({ trigger, onClose }: { trigger: Trigger | null; onClose: () => void }) {
  const [name, setName] = useState(trigger?.name ?? '');
  const [enabled, setEnabled] = useState(trigger?.enabled ?? true);
  const [topic, setTopic] = useState(trigger?.topic ?? 'alerts.raised');
  const [matchText, setMatchText] = useState(
    given(trigger?.match_payload) ? JSON.stringify(trigger.match_payload) : '',
  );
  const [debounce, setDebounce] = useState(String(trigger?.debounce_s ?? 30));
  const [passDevice, setPassDevice] = useState(trigger?.pass_device ?? true);
  const [method, setMethod] = useState(trigger?.action.method ?? 'tasks.run');
  const [taskId, setTaskId] = useState(
    typeof trigger?.action.params?.['id'] === 'string' ? (trigger.action.params['id'] as string) : '',
  );
  const [paramsText, setParamsText] = useState(
    trigger === null || trigger.action.method === 'tasks.run' ? '' : JSON.stringify(trigger.action.params ?? {}),
  );

  const tasks = useTasks();
  const save = useRpcMutation<Trigger, TriggerSaveParams>('triggers.save', {
    invalidates: ['triggers.'],
    errorToast: 'The trigger could not be saved',
  });

  const parsedMatch = matchText.trim() === '' ? undefined : varsFromText(matchText);
  const match = parsedMatch ?? undefined;
  const matchInvalid = parsedMatch === null;
  const params = method === 'tasks.run' ? { id: taskId } : varsFromText(paramsText);
  const incomplete =
    name.trim() === '' || topic.trim() === '' || matchInvalid || params === null || (method === 'tasks.run' && taskId === '');

  return (
    <Drawer
      open
      onClose={onClose}
      size="lg"
      title={trigger === null ? 'New trigger' : `Edit ${trigger.name}`}
      description="Run something when the rest of the application says something happened."
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            loading={save.isPending}
            disabled={incomplete}
            onClick={() =>
              save.mutate(
                {
                  ...(trigger === null ? {} : { id: trigger.id }),
                  name: name.trim(),
                  enabled,
                  topic: topic.trim(),
                  ...(match === undefined ? {} : { match_payload: match }),
                  debounce_s: Math.max(0, Number.parseInt(debounce, 10) || 0),
                  pass_device: passDevice,
                  action: { method, ...(params === null ? {} : { params }) },
                },
                { onSuccess: onClose },
              )
            }
          >
            {trigger === null ? 'Create the trigger' : 'Save'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field label="Name" required>
          <Input value={name} onChange={(event) => setName(event.target.value)} />
        </Field>
        <Switch checked={enabled} onChange={(event) => setEnabled(event.target.checked)}>
          Enabled
        </Switch>
        <Field
          label="Event topic"
          required
          hint="An exact topic (alerts.raised) or a prefix ending in a dot (alerts.). Topics automation publishes itself are refused: that would be a loop."
        >
          <Input value={topic} list="automation-topics" onChange={(event) => setTopic(event.target.value)} />
        </Field>
        <datalist id="automation-topics">
          {TOPICS.map((entry) => (
            <option key={entry} value={entry} />
          ))}
        </datalist>
        <Field
          label="Only when the payload matches"
          hint='JSON pointers to values, e.g. {"/severity": "critical"}. Leave empty for every event on the topic.'
          error={matchInvalid ? 'This is not valid JSON' : undefined}
        >
          <Input value={matchText} placeholder="{}" onChange={(event) => setMatchText(event.target.value)} />
        </Field>
        <Field label="Collect events for, in seconds" hint={debounceText(Number.parseInt(debounce, 10) || 0)}>
          <Input
            value={debounce}
            inputMode="numeric"
            onChange={(event) => setDebounce(event.target.value.replace(/[^0-9]/g, ''))}
          />
        </Field>
        <Switch checked={passDevice} onChange={(event) => setPassDevice(event.target.checked)}>
          Pass the device the event was about to the action
        </Switch>

        <h4 className={styles.subheading}>What it runs</h4>
        <Field label="Method">
          <Select value={method} options={METHODS} onChange={(event) => setMethod(event.target.value)} />
        </Field>
        {method === 'tasks.run' ? (
          <Field label="Task" required>
            <Select
              value={taskId}
              options={[
                { value: '', label: 'Choose a task…' },
                ...(tasks.data ?? []).map((task: Task) => ({ value: task.id, label: task.name })),
              ]}
              onChange={(event) => setTaskId(event.target.value)}
            />
          </Field>
        ) : (
          <Field
            label="Parameters as JSON"
            error={paramsText.trim() !== '' && varsFromText(paramsText) === null ? 'This is not valid JSON' : undefined}
          >
            <Input value={paramsText} placeholder="{}" onChange={(event) => setParamsText(event.target.value)} />
          </Field>
        )}
      </div>
    </Drawer>
  );
}
