/**
 * Saved tasks: the thing a schedule or a trigger points at.
 *
 * Each row shows its selection *as a filter*, in words, because that is what
 * it is — "every lab device of type cisco_ios", not the eleven devices that
 * happened to match when it was written. `tasks.devices` turns that into a
 * count on demand, in the drawer and in the confirmation before a run.
 */

import { ClipboardList, Play, Pencil, Trash } from 'lucide-react';
import { useState } from 'react';
import { useRpcMutation } from '@/api/hooks';
import type { Deleted, IdParams, Selector, Started, Task, TaskRunParams } from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import styles from './Automation.module.css';
import { useTasks } from './data';
import { given } from './given';
import { describeSelection } from './selector';
import { TaskDrawer, type TaskDraft } from './TaskDrawer';

const ACTION_WORDS: Record<Task['action']['type'], string> = {
  commands: 'Commands',
  playbook: 'Playbook',
  template: 'Template → change',
  ansible: 'Ansible',
};

export interface TasksTabProps {
  onOpenRun: (runId: string) => void;
  /** Set by the toolbar's primary action. */
  creating: boolean;
  onCreatingChange: (creating: boolean) => void;
}

export function TasksTab({ onOpenRun, creating, onCreatingChange }: TasksTabProps) {
  const tasks = useTasks();
  const [editing, setEditing] = useState<Task | null>(null);
  const remove = useRpcMutation<Deleted, IdParams>('tasks.delete', {
    invalidates: ['tasks.'],
    successToast: 'The task was deleted',
  });
  const run = useRpcMutation<Started & { change_id?: string }, TaskRunParams>('tasks.run', {
    errorToast: 'The task could not be started',
  });

  const startTask = async (task: Task, dryRun: boolean) => {
    const ok = await confirm({
      title: dryRun ? `Dry run “${task.name}”?` : `Run “${task.name}” now?`,
      description: (
        <>
          <strong>{describeSelection(task.select)}</strong>.{' '}
          {dryRun
            ? 'A dry run says what would happen and touches nothing.'
            : task.action.type === 'template'
              ? 'The template is rendered and handed to the change pipeline as a draft. No device is configured.'
              : task.action.type === 'ansible'
                ? 'This starts ansible-playbook outside the change pipeline: no backup, no verification and no rollback.'
                : 'Only read-only commands are sent.'}
        </>
      ),
      confirmLabel: dryRun ? 'Dry run' : 'Run now',
    });
    if (!ok) return;
    run.mutate(
      { id: task.id, ...(dryRun ? { dry_run: true } : {}) },
      {
        onSuccess: (result) => {
          if (typeof result.run_id === 'string') onOpenRun(result.run_id);
          else if (typeof result.change_id === 'string') {
            toast.success('A change was drafted', {
              description: 'Nothing has been applied: it still needs a plan and an approval.',
            });
          }
        },
      },
    );
  };

  const columns: TableColumn<Task>[] = [
    {
      id: 'name',
      header: 'Task',
      required: true,
      sortValue: (task) => task.name,
      cell: (task) => (
        <span className={styles.cell}>
          <span className={styles.cellText} title={task.description || task.name}>
            {task.name}
          </span>
          {task.tags.map((tag) => (
            <Badge key={tag} variant="outline">
              {tag}
            </Badge>
          ))}
        </span>
      ),
    },
    {
      id: 'action',
      header: 'Does',
      width: 150,
      sortValue: (task) => task.action.type,
      cell: (task) => (
        <Badge tone={task.action.type === 'ansible' ? 'warn' : task.action.type === 'template' ? 'accent' : 'neutral'}>
          {ACTION_WORDS[task.action.type]}
        </Badge>
      ),
    },
    {
      id: 'select',
      header: 'Where',
      sortValue: (task) => describeSelection(task.select),
      cell: (task) => (
        <span className={styles.cellText} title={describeSelection(task.select)}>
          {describeSelection(task.select)}
        </span>
      ),
    },
    {
      id: 'last',
      header: 'Last run',
      width: 150,
      sortValue: (task) => task.last_run_at ?? 0,
      cell: (task) =>
        !given(task.last_run_at) ? (
          <span className={styles.muted}>Never</span>
        ) : (
          <span className={styles.cell}>
            <StatusPill tone={task.last_ok === true ? 'ok' : 'warn'}>
              {task.last_ok === true ? 'Clean' : 'Had problems'}
            </StatusPill>
            <TimeAgo at={task.last_run_at} />
          </span>
        ),
    },
  ];

  if (tasks.error) {
    return <ErrorState title="Tasks could not be read" error={tasks.error} onRetry={() => void tasks.refetch()} />;
  }

  const draft: TaskDraft = {
    name: '',
    select: { filter: {} } as Selector,
    action: { type: 'commands', commands: [] },
  };

  return (
    <>
      <Table
        id="automation.tasks"
        aria-label="Saved tasks"
        columns={columns}
        rows={tasks.data ?? []}
        rowKey={(task) => task.id}
        loading={tasks.isLoading}
        onRowActivate={(task) => setEditing(task)}
        rowActions={(task) => [
          { label: 'Run now…', icon: Play, onSelect: () => void startTask(task, false) },
          { label: 'Dry run…', onSelect: () => void startTask(task, true) },
          ...(given(task.last_run_id)
            ? [{ label: 'Open the last run', onSelect: () => onOpenRun(task.last_run_id as string) }]
            : []),
          { label: 'Edit…', icon: Pencil, onSelect: () => setEditing(task) },
          {
            label: 'Delete',
            icon: Trash,
            tone: 'danger' as const,
            onSelect: async () => {
              const ok = await confirm({
                title: `Delete the task “${task.name}”?`,
                description: 'Schedules and triggers that point at it will stop working and say so.',
                confirmLabel: 'Delete',
                tone: 'danger',
              });
              if (ok) remove.mutate({ id: task.id });
            },
          },
        ]}
        empty={
          <EmptyState
            size="compact"
            icon={ClipboardList}
            title="No saved task"
            description="A task is a filter and one thing to do to whatever it matches — the thing a schedule or a trigger points at."
            action={
              <Button variant="primary" onClick={() => onCreatingChange(true)}>
                New task
              </Button>
            }
          />
        }
      />

      {(creating || editing !== null) && (
        <TaskDrawer
          open
          task={
            editing === null
              ? draft
              : {
                  id: editing.id,
                  name: editing.name,
                  description: editing.description,
                  select: editing.select,
                  action: editing.action,
                  tags: editing.tags,
                }
          }
          onClose={() => {
            setEditing(null);
            onCreatingChange(false);
          }}
        />
      )}
    </>
  );
}
