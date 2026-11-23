/**
 * Configuration templates: the body, its variables, and what it renders to on
 * a real device — with the one thing about them that matters written where it
 * cannot be missed.
 *
 * **Applying a template does not configure anything.** `templates.apply`
 * hands the rendered lines to the change pipeline as a *draft*: it still has
 * to be planned, approved against its hash and applied, with a backup, a
 * drift check, verification and a rollback. The screen says that above the
 * button, in the confirmation, and in the toast afterwards, because a
 * template that looked like it pushed configuration would be the most
 * dangerous control in the application.
 *
 * A `commands` template is the exception in the other direction: it renders
 * to read-only commands and `templates.apply` starts a *run*, not a change.
 * The button changes its words accordingly.
 */

import { Copy, FileCode, Pencil, Play, Save, Trash, Wand } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type {
  Applied,
  ApplyParams,
  Deleted,
  DuplicateParams,
  IdParams,
  Rendered,
  RenderParams,
  Started,
  Template,
  TemplateKind,
  TemplateSaveParams,
  TemplateValidation,
  ValidateTemplateParams,
  Variable,
} from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Combobox } from '@/design/Combobox';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { Textarea } from '@/design/Textarea';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { plural } from '@/lib/format';
import styles from './Automation.module.css';
import { CodeEditor } from './CodeEditor';
import { useDevices, useTemplates } from './data';
import { DeviceSelect } from './DeviceSelect';
import { given } from './given';
import { EMPTY_SELECTION, toSelector, type SelectionDraft } from './selector';
import { Declarations, missingVariables, VariablesForm } from './Variables';

export interface TemplatesTabProps {
  creating: boolean;
  onCreatingChange: (creating: boolean) => void;
  onOpenRun: (runId: string) => void;
  onOpenChange: (changeId: string) => void;
}

const NEW_BODY = '! One configuration line per line.\n! {{ device.name }} and the variables below are in scope.\n';

export function TemplatesTab({ creating, onCreatingChange, onOpenRun, onOpenChange }: TemplatesTabProps) {
  const templates = useTemplates();
  const [openId, setOpenId] = useState<string | null>(null);
  const remove = useRpcMutation<Deleted, IdParams>('templates.delete', {
    invalidates: ['templates.'],
    successToast: 'The template was deleted',
  });
  const duplicate = useRpcMutation<Template, DuplicateParams>('templates.duplicate', {
    invalidates: ['templates.'],
  });

  const open = (templates.data ?? []).find((entry) => entry.id === openId);

  const columns: TableColumn<Template>[] = [
    {
      id: 'name',
      header: 'Template',
      required: true,
      sortValue: (template) => template.name,
      cell: (template) => (
        <span className={styles.cell}>
          <span className={styles.cellText} title={template.description || template.name}>
            {template.name}
          </span>
          {template.builtin && (
            <Badge variant="outline" title="Shipped with Nocturne: duplicate it to change it">
              Built in
            </Badge>
          )}
        </span>
      ),
    },
    {
      id: 'kind',
      header: 'Renders to',
      width: 190,
      sortValue: (template) => template.kind,
      cell: (template) => (
        <Badge tone={template.kind === 'config' ? 'accent' : 'neutral'}>
          {template.kind === 'config' ? 'Configuration → change' : 'Read-only commands'}
        </Badge>
      ),
    },
    {
      id: 'profiles',
      header: 'Device types',
      width: 200,
      cell: (template) =>
        template.profiles.length === 0 ? (
          <span className={styles.muted}>Any</span>
        ) : (
          template.profiles.join(', ')
        ),
    },
    {
      id: 'variables',
      header: 'Vars',
      width: 76,
      align: 'right',
      sortValue: (template) => template.variables.length,
      cell: (template) => template.variables.length,
    },
    {
      id: 'updated',
      header: 'Changed',
      width: 120,
      sortValue: (template) => template.updated_at,
      // A built-in has no date: it was not changed, it was shipped.
      cell: (template) =>
        template.updated_at === 0 ? (
          <span className={styles.muted} title="Shipped with Nocturne">
            Never
          </span>
        ) : (
          <TimeAgo at={template.updated_at} />
        ),
    },
  ];

  if (templates.error) {
    return (
      <ErrorState title="Templates could not be read" error={templates.error} onRetry={() => void templates.refetch()} />
    );
  }

  if (creating || open !== undefined) {
    return (
      <TemplateEditor
        key={open?.id ?? 'new'}
        template={open}
        onClose={() => {
          setOpenId(null);
          onCreatingChange(false);
        }}
        onOpenRun={onOpenRun}
        onOpenChange={onOpenChange}
      />
    );
  }

  return (
    <Table
      id="automation.templates"
      aria-label="Configuration templates"
      columns={columns}
      rows={templates.data ?? []}
      rowKey={(template) => template.id}
      loading={templates.isLoading}
      onRowActivate={(template) => setOpenId(template.id)}
      rowActions={(template) => [
        { label: template.builtin ? 'Open (read-only)' : 'Edit…', icon: Pencil, onSelect: () => setOpenId(template.id) },
        {
          label: 'Duplicate',
          icon: Copy,
          onSelect: () =>
            duplicate.mutate(
              { id: template.id },
              { onSuccess: (copy) => setOpenId(copy.id) },
            ),
        },
        ...(template.builtin
          ? []
          : [
              {
                label: 'Delete',
                icon: Trash,
                tone: 'danger' as const,
                onSelect: async () => {
                  const ok = await confirm({
                    title: `Delete the template “${template.name}”?`,
                    description: 'Tasks that render it will stop working. Changes already drafted from it are untouched.',
                    confirmLabel: 'Delete',
                    tone: 'danger',
                  });
                  if (ok) remove.mutate({ id: template.id });
                },
              },
            ]),
      ]}
      empty={
        <EmptyState
          size="compact"
          icon={FileCode}
          title="No template"
          description="A template renders configuration for a device from a few variables, and hands the result to the change pipeline."
          action={
            <Button variant="primary" onClick={() => onCreatingChange(true)}>
              New template
            </Button>
          }
        />
      }
    />
  );
}

// ------------------------------------------------------------------ editor

function TemplateEditor({
  template,
  onClose,
  onOpenRun,
  onOpenChange,
}: {
  template: Template | undefined;
  onClose: () => void;
  onOpenRun: (runId: string) => void;
  onOpenChange: (changeId: string) => void;
}) {
  const readOnly = template?.builtin ?? false;
  const [name, setName] = useState(template?.name ?? '');
  const [description, setDescription] = useState(template?.description ?? '');
  const [body, setBody] = useState(template?.body ?? NEW_BODY);
  const [variables, setVariables] = useState<Variable[]>(template?.variables ?? []);
  const [vars, setVars] = useState<Record<string, unknown>>({});
  const [deviceId, setDeviceId] = useState<string | null>(null);
  const [selection, setSelection] = useState<SelectionDraft>(EMPTY_SELECTION);

  const devices = useDevices();
  const save = useRpcMutation<Template, TemplateSaveParams>('templates.save', {
    invalidates: ['templates.'],
    successToast: 'The template was saved',
    errorToast: 'The template could not be saved',
  });
  const duplicate = useRpcMutation<Template, DuplicateParams>('templates.duplicate', {
    invalidates: ['templates.'],
  });

  // Validation and the preview follow the body and the values a moment
  // behind the typing: otherwise every keystroke is a rendering on the backend.
  const [checked, setChecked] = useState(body);
  const [settledVars, setSettledVars] = useState(vars);
  useEffect(() => {
    const timer = setTimeout(() => {
      setChecked(body);
      setSettledVars(vars);
    }, 500);
    return () => clearTimeout(timer);
  }, [body, vars]);
  const validation = useRpc<TemplateValidation>('templates.validate', {
    body: checked,
    variables,
  } satisfies ValidateTemplateParams);

  const missing = missingVariables(variables, vars);
  const renderParams: RenderParams | undefined =
    deviceId === null
      ? undefined
      : {
          device_ids: [deviceId],
          ...(template !== undefined && template.body === checked
            ? { template_id: template.id }
            : { body: checked, variables }),
          ...(Object.keys(settledVars).length > 0 ? { vars: settledVars } : {}),
        };
  const rendered = useRpc<Rendered[]>('templates.render', renderParams, {
    enabled: renderParams !== undefined && missingVariables(variables, settledVars).length === 0,
  });

  const [kind, setKind] = useState<TemplateKind>(template?.kind ?? 'config');
  const apply = useRpcMutation<Applied & Started, ApplyParams>('templates.apply', {
    errorToast: kind === 'config' ? 'No change was drafted' : 'The run could not be started',
  });

  const applyNow = async () => {
    if (template === undefined) return;
    const ok = await confirm({
      title: kind === 'config' ? 'Draft a change from this template?' : 'Run these commands?',
      description:
        kind === 'config'
          ? 'The template is rendered for each selected device and handed to the change pipeline as a draft. Nothing is sent to a device: the change still has to be planned, approved and applied.'
          : 'The template renders to read-only commands, which are sent as a run.',
      confirmLabel: kind === 'config' ? 'Draft the change' : 'Run',
    });
    if (!ok) return;
    apply.mutate(
      { template_id: template.id, ...toSelector(selection), ...(Object.keys(vars).length > 0 ? { vars } : {}) },
      {
        onSuccess: (result) => {
          const changeId = result.change_id;
          if (typeof result.run_id === 'string') onOpenRun(result.run_id);
          else if (typeof changeId === 'string') {
            toast.success('A change was drafted', {
              description: 'Nothing has been applied. Open it to plan and approve it.',
              action: { label: 'Open the change', onSelect: () => onOpenChange(changeId) },
            });
          }
        },
      },
    );
  };

  const problems = validation.data?.problems ?? [];

  return (
    <div className={styles.stack}>
      <div className={styles.editorBar}>
        <Button onClick={onClose}>← All templates</Button>
        <span className={styles.spacer} />
        {readOnly && template !== undefined ? (
          <>
            <StatusPill tone="idle">Built in, read-only</StatusPill>
            <Button icon={Copy} loading={duplicate.isPending} onClick={() => duplicate.mutate({ id: template.id })}>
              Duplicate to edit
            </Button>
          </>
        ) : (
          <Button
            variant="primary"
            icon={Save}
            loading={save.isPending}
            disabled={name.trim() === '' || body.trim() === ''}
            onClick={() =>
              save.mutate({
                ...(template === undefined ? {} : { id: template.id }),
                name: name.trim(),
                description: description.trim(),
                kind,
                body,
                variables,
                profiles: template?.profiles ?? [],
                tags: template?.tags ?? [],
              })
            }
          >
            Save
          </Button>
        )}
      </div>

      <div className={styles.templateGrid}>
        <Panel title="Body" className={styles.templateBody}>
          <div className={styles.form}>
            <Field label="Name" required>
              <Input value={name} disabled={readOnly} onChange={(event) => setName(event.target.value)} />
            </Field>
            <Field label="What it is for">
              <Textarea
                rows={2}
                value={description}
                disabled={readOnly}
                onChange={(event) => setDescription(event.target.value)}
              />
            </Field>
            <Field
              label="Renders to"
              hint={
                kind === 'config'
                  ? 'Configuration lines, which become a change — never sent from here.'
                  : 'Read-only commands, which become a run.'
              }
            >
              <Select
                value={kind}
                disabled={readOnly}
                options={[
                  { value: 'config', label: 'Configuration → a change' },
                  { value: 'commands', label: 'Read-only commands → a run' },
                ]}
                onChange={(event) => setKind(event.target.value as TemplateKind)}
              />
            </Field>
            <Field
              label="Template body"
              hint="MiniJinja. `device`, `facts`, `interfaces`, `neighbors`, `vlans` and every variable are in scope. Undefined is strict: a name nothing provides is an error, never a blank."
            >
              <CodeEditor
                label="Template body"
                value={body}
                onChange={setBody}
                problems={problems}
                minRows={14}
              />
            </Field>
            {validation.data !== undefined && (
              <div className={styles.validation} data-ok={validation.data.ok || undefined}>
                <StatusPill tone={validation.data.ok ? 'ok' : 'crit'}>
                  {validation.data.ok ? 'The body parses' : `${plural(problems.length, 'problem')}`}
                </StatusPill>
                <ul>
                  {problems.map((problem) => (
                    <li key={`${problem.line ?? 0}-${problem.message}`}>
                      {given(problem.line) && <strong>Line {problem.line}: </strong>}
                      {problem.message}
                    </li>
                  ))}
                </ul>
                {validation.data.used_variables.length > 0 && (
                  <p className={styles.hint}>
                    It reads: {validation.data.used_variables.join(', ')}.
                  </p>
                )}
              </div>
            )}
          </div>
        </Panel>

        <Panel title="Variables" className={styles.templateVars}>
          <Declarations
            value={variables}
            onChange={setVariables}
            undeclared={validation.data?.undeclared ?? []}
            disabled={readOnly}
          />
          <h5 className={styles.subheading}>Values for the preview</h5>
          <VariablesForm variables={variables} value={vars} onChange={setVars} />
          {variables.length > 0 && missing.length > 0 && (
            <p className={styles.hint}>
              {missing.join(', ')} {missing.length === 1 ? 'has' : 'have'} no value yet, so nothing can be rendered.
            </p>
          )}
        </Panel>

        <Panel title="Rendered for a real device" className={styles.templatePreview}>
          <div className={styles.form}>
            <Field
              label="Preview against"
              hint="Rendered with that device's own facts and last collected data. Nothing is sent."
            >
              <Combobox
                options={(devices.data ?? []).map((device) => ({
                  value: device.id,
                  label: device.name,
                  description: device.profile || 'type detected on connect',
                }))}
                value={deviceId}
                clearable
                placeholder="Choose a device…"
                onChange={setDeviceId}
              />
            </Field>
            {deviceId === null && (
              <p className={styles.hint}>
                Pick a device to see exactly what this would produce for it, before anything is drafted.
              </p>
            )}
            {rendered.error && (
              <div className={styles.refusal} role="alert">
                <h5>
                  <Icon icon={Wand} size={13} /> It would not render
                </h5>
                <p>{rendered.error.message}</p>
              </div>
            )}
            {(rendered.data ?? []).map((result) => (
              <div key={result.device_id}>
                {result.ok && given(result.lines) ? (
                  <CodeBlock
                    title={`${result.name}: ${plural(result.lines.length, 'line')}`}
                    code={result.lines.join('\n')}
                    lineNumbers
                    maxHeight={260}
                  />
                ) : (
                  <div className={styles.refusal} role="alert">
                    <h5>{result.name}</h5>
                    <p>{result.error ?? 'It did not render, and no reason was given.'}</p>
                  </div>
                )}
              </div>
            ))}
          </div>
        </Panel>
      </div>

      {template !== undefined && (
        <Panel
          title={kind === 'config' ? 'Apply to devices — as a change' : 'Run on devices'}
          actions={
            <Button
              variant="primary"
              icon={kind === 'config' ? Wand : Play}
              loading={apply.isPending}
              disabled={missing.length > 0}
              onClick={() => void applyNow()}
            >
              {kind === 'config' ? 'Draft a change…' : 'Run…'}
            </Button>
          }
        >
          <p className={styles.pipeline} role="note">
            <Icon icon={Wand} size={14} />
            <span>
              {kind === 'config' ? (
                <>
                  <strong>This drafts a change and touches nothing.</strong> The rendered lines become one change with a
                  target per device, which then goes the usual way: plan, risk and protected resources, an approval
                  bound to that plan, backup, apply, verify, roll back on failure.
                </>
              ) : (
                <>
                  <strong>This sends read-only commands.</strong> A commands template renders to reads, so it becomes a
                  run — the refusal comes first if any line is not read-only on a target.
                </>
              )}
            </span>
          </p>
          <DeviceSelect id="template" value={selection} onChange={setSelection} compact />
        </Panel>
      )}
    </div>
  );
}
