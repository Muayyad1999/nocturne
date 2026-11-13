/**
 * Playbooks: a few steps, in order, per device.
 *
 * The YAML is validated by the backend — `playbooks.validate` numbers its
 * complaints by step — and the marker goes in the editor's gutter on the line
 * the step starts, so a problem is read where it was written.
 *
 * A playbook's `template` steps are gathered into **one change at the end of
 * a run**, and nothing is sent: the run's own header says so when it has
 * drafted one. A step that fails stops that device only, which is why the
 * per-device step results are the interesting part of a playbook run.
 */

import { CirclePlay, FileCode, Pencil, Save, Trash } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  Deleted,
  IdParams,
  Playbook,
  PlaybookRunParams,
  PlaybookSaveParams,
  PlaybookValidation,
  Run,
  Started,
} from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { Textarea } from '@/design/Textarea';
import { TimeAgo } from '@/design/TimeAgo';
import { plural } from '@/lib/format';
import styles from './Automation.module.css';
import { CodeEditor } from './CodeEditor';
import { usePlaybooks } from './data';
import { DeviceSelect } from './DeviceSelect';
import { given } from './given';
import { stepStartLines } from './playbook';
import { Tally } from './RunTab';
import { EMPTY_SELECTION, toSelector, type SelectionDraft } from './selector';

const EXAMPLE = `name: Pre-change health check
description: What to look at before touching anything
steps:
  - name: Collect state
    collect: [facts, interfaces, neighbors]
  - name: Save evidence
    commands: [show version]
    register: evidence
  - name: Uplinks are up
    assert:
      that: "interfaces | selectattr('oper_up') | list | length >= 2"
      message: "{{ device.name }} has fewer than two uplinks up"
`;

export interface PlaybooksTabProps {
  creating: boolean;
  onCreatingChange: (creating: boolean) => void;
  onOpenRun: (runId: string) => void;
}

export function PlaybooksTab({ creating, onCreatingChange, onOpenRun }: PlaybooksTabProps) {
  const playbooks = usePlaybooks();
  const [openId, setOpenId] = useState<string | null>(null);
  const remove = useRpcMutation<Deleted, IdParams>('playbooks.delete', {
    invalidates: ['playbooks.'],
    successToast: 'The playbook was deleted',
  });

  const open = (playbooks.data ?? []).find((entry) => entry.id === openId);

  const columns: TableColumn<Playbook>[] = [
    {
      id: 'name',
      header: 'Playbook',
      required: true,
      sortValue: (playbook) => playbook.name,
      cell: (playbook) => (
        <span className={styles.cellText} title={playbook.description || playbook.name}>
          {playbook.name}
        </span>
      ),
    },
    {
      id: 'description',
      header: 'What it does',
      cell: (playbook) =>
        playbook.description === '' ? (
          <span className={styles.muted}>No description</span>
        ) : (
          <span className={styles.cellText} title={playbook.description}>
            {playbook.description}
          </span>
        ),
    },
    {
      id: 'updated',
      header: 'Changed',
      width: 120,
      sortValue: (playbook) => playbook.updated_at,
      cell: (playbook) => <TimeAgo at={playbook.updated_at} />,
    },
  ];

  if (playbooks.error) {
    return (
      <ErrorState title="Playbooks could not be read" error={playbooks.error} onRetry={() => void playbooks.refetch()} />
    );
  }

  if (creating || open !== undefined) {
    return (
      <PlaybookEditor
        key={open?.id ?? 'new'}
        playbook={open}
        onClose={() => {
          setOpenId(null);
          onCreatingChange(false);
        }}
        onOpenRun={onOpenRun}
      />
    );
  }

  return (
    <div className={styles.stack}>
      <Table
        id="automation.playbooks"
        aria-label="Playbooks"
        columns={columns}
        rows={playbooks.data ?? []}
        rowKey={(playbook) => playbook.id}
        loading={playbooks.isLoading}
        onRowActivate={(playbook) => setOpenId(playbook.id)}
        rowActions={(playbook) => [
          { label: 'Edit…', icon: Pencil, onSelect: () => setOpenId(playbook.id) },
          {
            label: 'Delete',
            icon: Trash,
            tone: 'danger' as const,
            onSelect: async () => {
              const ok = await confirm({
                title: `Delete the playbook “${playbook.name}”?`,
                description: 'Tasks and schedules that point at it will stop working and say so.',
                confirmLabel: 'Delete',
                tone: 'danger',
              });
              if (ok) remove.mutate({ id: playbook.id });
            },
          },
        ]}
        empty={
          <EmptyState
            size="compact"
            icon={FileCode}
            title="No playbook"
            description="A playbook is a few steps in order per device: collect, read, assert, back up, render a template. Nothing in it configures a device by itself."
            action={
              <Button variant="primary" onClick={() => onCreatingChange(true)}>
                New playbook
              </Button>
            }
          />
        }
      />
      <PlaybookRuns onOpenRun={onOpenRun} />
    </div>
  );
}

// ------------------------------------------------------------------ editor

function PlaybookEditor({
  playbook,
  onClose,
  onOpenRun,
}: {
  playbook: Playbook | undefined;
  onClose: () => void;
  onOpenRun: (runId: string) => void;
}) {
  const [name, setName] = useState(playbook?.name ?? '');
  const [description, setDescription] = useState(playbook?.description ?? '');
  const [yaml, setYaml] = useState(playbook?.yaml ?? EXAMPLE);
  const [selection, setSelection] = useState<SelectionDraft>(EMPTY_SELECTION);
  const [dryRun, setDryRun] = useState(false);

  const save = useRpcMutation<Playbook, PlaybookSaveParams>('playbooks.save', {
    invalidates: ['playbooks.'],
    successToast: 'The playbook was saved',
    errorToast: 'The playbook could not be saved',
  });
  const start = useRpcMutation<Started, PlaybookRunParams>('playbooks.run', {
    errorToast: 'The playbook could not be started',
  });

  const [checked, setChecked] = useState(yaml);
  useEffect(() => {
    const timer = setTimeout(() => setChecked(yaml), 500);
    return () => clearTimeout(timer);
  }, [yaml]);
  const validation = useRpc<PlaybookValidation>('playbooks.validate', { yaml: checked });

  // A problem is reported by step; the marker goes on the line that step starts.
  const stepLines = stepStartLines(checked);
  const problems = (validation.data?.problems ?? []).map((problem) => ({
    line: given(problem.step) ? stepLines[problem.step - 1] : undefined,
    message: given(problem.step) ? `Step ${problem.step}: ${problem.message}` : problem.message,
  }));

  const runNow = async () => {
    const ok = await confirm({
      title: dryRun ? 'Walk the steps without touching anything?' : 'Run this playbook on the selected devices?',
      description: dryRun
        ? 'A dry run walks the steps against what was last collected. No session is opened and no change is created.'
        : 'Steps run in order per device. Reads are sent; template steps are gathered into one change at the end, which is a draft — nothing is applied.',
      confirmLabel: dryRun ? 'Dry run' : 'Run',
    });
    if (!ok) return;
    start.mutate(
      {
        ...toSelector(selection),
        ...(playbook === undefined ? { yaml } : { playbook_id: playbook.id }),
        ...(dryRun ? { dry_run: true } : {}),
      },
      { onSuccess: (result) => onOpenRun(result.run_id) },
    );
  };

  return (
    <div className={styles.stack}>
      <div className={styles.editorBar}>
        <Button onClick={onClose}>← All playbooks</Button>
        <span className={styles.spacer} />
        <Button
          variant="primary"
          icon={Save}
          loading={save.isPending}
          disabled={name.trim() === ''}
          onClick={() =>
            save.mutate({
              ...(playbook === undefined ? {} : { id: playbook.id }),
              name: name.trim(),
              description: description.trim(),
              yaml,
            })
          }
        >
          Save
        </Button>
      </div>

      <div className={styles.playbookGrid}>
        <Panel title="The playbook">
          <div className={styles.form}>
            <Field label="Name" required>
              <Input value={name} onChange={(event) => setName(event.target.value)} />
            </Field>
            <Field label="What it does">
              <Textarea rows={2} value={description} onChange={(event) => setDescription(event.target.value)} />
            </Field>
            <Field
              label="Steps, as YAML"
              hint="One thing per step: collect, commands, assert, backup, template, set or pause — with name, when, register and ignore_errors."
            >
              <CodeEditor label="Steps, as YAML" value={yaml} onChange={setYaml} problems={problems} minRows={18} />
            </Field>
            {validation.data !== undefined && (
              <div className={styles.validation} data-ok={validation.data.ok || undefined}>
                <StatusPill tone={validation.data.ok ? 'ok' : 'crit'}>
                  {validation.data.ok
                    ? `${plural(validation.data.steps, 'step')}, all valid`
                    : plural(problems.length, 'problem')}
                </StatusPill>
                <ul>
                  {problems.map((problem) => (
                    <li key={problem.message}>{problem.message}</li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        </Panel>

        <Panel
          title="Run it"
          actions={
            <Button
              variant="primary"
              icon={CirclePlay}
              loading={start.isPending}
              disabled={validation.data?.ok !== true}
              onClick={() => void runNow()}
            >
              {dryRun ? 'Dry run…' : 'Run…'}
            </Button>
          }
        >
          <div className={styles.form}>
            <Switch checked={dryRun} onChange={(event) => setDryRun(event.target.checked)}>
              Dry run — walk the steps against what was last collected
            </Switch>
            <DeviceSelect id="playbook" value={selection} onChange={setSelection} compact />
            {playbook === undefined && (
              <p className={styles.hint}>
                This playbook has not been saved, so the run is sent the YAML as it stands.
              </p>
            )}
          </div>
        </Panel>
      </div>
    </div>
  );
}

// -------------------------------------------------------------------- runs

function PlaybookRuns({ onOpenRun }: { onOpenRun: (runId: string) => void }) {
  const runs = useRpc<Run[]>('playbooks.runs', { limit: 20 }, { invalidateOn: ['automation.runs.changed'] });

  const columns: TableColumn<Run>[] = [
    {
      id: 'name',
      header: 'Run',
      required: true,
      cell: (run) => (
        <span className={styles.cell}>
          <span className={styles.cellText}>{run.name}</span>
          {run.dry_run && <Badge tone="info">Dry run</Badge>}
          {run.change_ids.length > 0 && (
            <Badge tone="accent" title="Drafted by this run; nothing has been applied">
              {plural(run.change_ids.length, 'change')} drafted
            </Badge>
          )}
        </span>
      ),
    },
    { id: 'devices', header: 'Devices', width: 210, cell: (run) => <Tally run={run} /> },
    { id: 'started', header: 'Started', width: 120, sortValue: (run) => run.started_at, cell: (run) => <TimeAgo at={run.started_at} /> },
  ];

  return (
    <Panel title="Playbook runs" padding="none">
      <Table
        id="automation.playbookRuns"
        aria-label="Playbook runs"
        columns={columns}
        rows={runs.data ?? []}
        rowKey={(run) => run.id}
        loading={runs.isLoading}
        defaultSort={{ columnId: 'started', direction: 'desc' }}
        onRowActivate={(run) => onOpenRun(run.id)}
        empty={
          <EmptyState
            size="compact"
            icon={CirclePlay}
            title="No playbook has been run yet"
            description="A playbook run keeps every step's result per device, including the devices it never reached."
          />
        }
      />
    </Panel>
  );
}
