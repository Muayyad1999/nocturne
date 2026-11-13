/**
 * A saved task: what to run, and where — the "where" as a filter.
 *
 * `tasks.devices` is called with the selection *as it is on screen*, before
 * anything is saved, so the question "what does this actually mean right now"
 * is answered by the backend while the task is being written rather than the
 * first time it fires at three in the morning.
 *
 * A task's action is one of four things, and two of them are not a run at
 * all: a `template` task drafts a change, and an `ansible` task runs somebody
 * else's playbook outside the change pipeline. The form says which, every
 * time, rather than treating "run" as one word.
 */

import { useState } from 'react';
import { useRpcMutation } from '@/api/hooks';
import type { Action, Task, TaskSaveParams } from '@/api/types/automation';
import { Button } from '@/design/Button';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Switch } from '@/design/Switch';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import styles from './Automation.module.css';
import { Caveat } from './Caveat';
import { CodeEditor } from './CodeEditor';
import { useAnsibleStatus, usePlaybooks, useTemplates } from './data';
import { DeviceSelect } from './DeviceSelect';
import { given } from './given';
import { fromSelector, toSelector, type SelectionDraft } from './selector';
import { VariablesForm, varsFromText } from './Variables';

export interface TaskDraft {
  id?: string;
  name: string;
  description?: string;
  select: Task['select'];
  action: Action;
  tags?: string[];
}

export interface TaskDrawerProps {
  open: boolean;
  /** The task to edit, or a draft to start from. */
  task: TaskDraft;
  onClose: () => void;
  onSaved?: (task: Task) => void;
}

type ActionType = Action['type'];

const ACTIONS: { value: ActionType; label: string }[] = [
  { value: 'commands', label: 'Run read-only commands' },
  { value: 'playbook', label: 'Run a playbook' },
  { value: 'template', label: 'Render a template into a change' },
  { value: 'ansible', label: 'Run an Ansible playbook (outside the change pipeline)' },
];

const WHAT_HAPPENS: Record<ActionType, string> = {
  commands: 'Commands are sent and their answers kept. Nothing is configured.',
  playbook:
    'Each step runs in order per device. A playbook’s template steps are gathered into one change at the end — nothing is applied.',
  template: 'The template is rendered per device and handed to the change pipeline as a draft. No device is touched.',
  ansible:
    'Nocturne starts `ansible-playbook` on this machine. This is the one path with none of the change pipeline’s guarantees.',
};

export function TaskDrawer({ open, task, onClose, onSaved }: TaskDrawerProps) {
  const [name, setName] = useState(task.name);
  const [description, setDescription] = useState(task.description ?? '');
  const [tags, setTags] = useState<string[]>(task.tags ?? []);
  const [selection, setSelection] = useState<SelectionDraft>(fromSelector(task.select));
  const [type, setType] = useState<ActionType>(task.action.type);
  const [commands, setCommands] = useState(
    task.action.type === 'commands' ? task.action.commands.join('\n') : '',
  );
  const [parse, setParse] = useState(task.action.type === 'commands' ? (task.action.parse ?? false) : false);
  const [playbookId, setPlaybookId] = useState(
    task.action.type === 'playbook' ? (task.action.playbook_id ?? '') : '',
  );
  const [templateId, setTemplateId] = useState(task.action.type === 'template' ? task.action.template_id : '');
  const [vars, setVars] = useState<Record<string, unknown>>(
    task.action.type === 'template' || task.action.type === 'playbook' ? (task.action.vars ?? {}) : {},
  );
  const [varsText, setVarsText] = useState(
    task.action.type === 'playbook' && given(task.action.vars) ? JSON.stringify(task.action.vars) : '',
  );
  const [path, setPath] = useState(task.action.type === 'ansible' ? task.action.playbook_path : '');
  const [check, setCheck] = useState(task.action.type === 'ansible' ? (task.action.check ?? true) : true);

  const templates = useTemplates();
  const playbooks = usePlaybooks();
  const ansible = useAnsibleStatus();
  const save = useRpcMutation<Task, TaskSaveParams>('tasks.save', {
    invalidates: ['tasks.'],
    errorToast: 'The task could not be saved',
  });

  const template = (templates.data ?? []).find((entry) => entry.id === templateId);

  const action = (): Action => {
    switch (type) {
      case 'commands':
        return {
          type: 'commands',
          commands: commands.split('\n').map((line) => line.trim()).filter((line) => line !== ''),
          ...(parse ? { parse: true } : {}),
        };
      case 'playbook':
        return { type: 'playbook', playbook_id: playbookId, ...(Object.keys(vars).length > 0 ? { vars } : {}) };
      case 'template':
        return { type: 'template', template_id: templateId, ...(Object.keys(vars).length > 0 ? { vars } : {}) };
      case 'ansible':
        return { type: 'ansible', playbook_path: path, check };
    }
  };

  const incomplete =
    name.trim() === '' ||
    (type === 'commands' && commands.trim() === '') ||
    (type === 'playbook' && playbookId === '') ||
    (type === 'template' && templateId === '') ||
    (type === 'ansible' && path.trim() === '');

  return (
    <Drawer
      open={open}
      onClose={onClose}
      size="lg"
      title={task.id === undefined ? 'New task' : `Edit ${task.name}`}
      description="A saved piece of repetitive work: a filter, and one thing to do to whatever it matches."
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
                  ...(task.id === undefined ? {} : { id: task.id }),
                  name: name.trim(),
                  description: description.trim(),
                  select: toSelector(selection),
                  action: action(),
                  tags,
                },
                {
                  onSuccess: (saved) => {
                    onSaved?.(saved);
                    onClose();
                  },
                },
              )
            }
          >
            {task.id === undefined ? 'Create the task' : 'Save'}
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        <Field label="Name" required>
          <Input value={name} onChange={(event) => setName(event.target.value)} />
        </Field>
        <Field label="What it is for" hint="One sentence, for whoever finds it running at 03:00.">
          <Textarea rows={2} value={description} onChange={(event) => setDescription(event.target.value)} />
        </Field>
        <Field label="Tags">
          <TagInput value={tags} onChange={setTags} />
        </Field>

        <h4 className={styles.subheading}>Where it runs</h4>
        <DeviceSelect id="task" value={selection} onChange={setSelection} />

        <h4 className={styles.subheading}>What it does</h4>
        <Field label="Action">
          <Select
            value={type}
            options={ACTIONS}
            onChange={(event) => setType(event.target.value as ActionType)}
          />
        </Field>
        <p className={styles.hint}>{WHAT_HAPPENS[type]}</p>

        {type === 'commands' && (
          <>
            <Field label="Commands, one per line" hint="Read-only commands only.">
              <CodeEditor
                label="Commands, one per line"
                value={commands}
                onChange={setCommands}
                placeholder="show version"
                minRows={6}
              />
            </Field>
            <Switch checked={parse} onChange={(event) => setParse(event.target.checked)}>
              Parse the output where a parser covers the command
            </Switch>
          </>
        )}

        {type === 'playbook' && (
          <Field label="Playbook" required>
            <Select
              value={playbookId}
              options={[
                { value: '', label: 'Choose a playbook…' },
                ...(playbooks.data ?? []).map((entry) => ({ value: entry.id, label: entry.name })),
              ]}
              onChange={(event) => setPlaybookId(event.target.value)}
            />
          </Field>
        )}

        {type === 'template' && (
          <>
            <Field label="Template" required>
              <Select
                value={templateId}
                options={[
                  { value: '', label: 'Choose a template…' },
                  ...(templates.data ?? [])
                    .filter((entry) => entry.kind === 'config')
                    .map((entry) => ({ value: entry.id, label: entry.name })),
                ]}
                onChange={(event) => setTemplateId(event.target.value)}
              />
            </Field>
            {template !== undefined && (
              <VariablesForm variables={template.variables} value={vars} onChange={setVars} />
            )}
          </>
        )}

        {type === 'ansible' && (
          <>
            <Caveat caveats={ansible.data?.caveats ?? []} brief />
            <Field label="Playbook path on this machine" required hint="Absolute, or relative to where the server runs.">
              <Input value={path} onChange={(event) => setPath(event.target.value)} />
            </Field>
            <Switch checked={check} onChange={(event) => setCheck(event.target.checked)}>
              Pass --check (what check mode means is each module’s business)
            </Switch>
          </>
        )}

        {type === 'playbook' && (
          <Field
            label="Variables as JSON"
            hint="Merged over the playbook’s own vars. Leave empty for none."
            error={varsText.trim() !== '' && varsFromText(varsText) === null ? 'This is not valid JSON' : undefined}
          >
            <Input
              value={varsText}
              placeholder="{}"
              onChange={(event) => {
                setVarsText(event.target.value);
                const parsed = varsFromText(event.target.value);
                if (parsed !== null) setVars(parsed);
              }}
            />
          </Field>
        )}
      </div>
    </Drawer>
  );
}
