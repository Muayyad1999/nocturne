/**
 * Ansible interoperability — and the one tab that leads with a warning.
 *
 * Everything else on this screen either reads a device or drafts a change.
 * `ansible.run` starts somebody else's playbook: it has no backup, no drift
 * check, no plan bound to an approval, no verification and **no rollback**,
 * and Nocturne's own inventory and configuration history are not updated by
 * whatever it changes. The backend says so on every run — the first line of
 * the job log, the run record, and `ansible.status` — so the caveats are the
 * first thing on this tab, are not dismissible, and are repeated in the
 * confirmation before a run starts. A run that cannot promise what a change
 * promises must not look like one.
 *
 * What it *does* give is worth having: the inventory is generated from this
 * inventory with the same filters, credentials reach the playbook only
 * through the child process's environment, `--check` is on by default, the
 * output is streamed into a cancellable job with the known secrets scrubbed
 * out, and a host Ansible could not reach comes back as `unreached` like any
 * other device Nocturne could not reach.
 */

import { Download, FileCode, Play, Terminal } from 'lucide-react';
import { useState } from 'react';
import { useRpcMutation } from '@/api/hooks';
import type {
  AnsibleRunParams,
  AnsibleStarted,
  ExportedPlaybook,
  ExportPlaybookParams,
  Template,
} from '@/api/types/automation';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { Field } from '@/design/Field';
import { Icon } from '@/design/Icon';
import { Input } from '@/design/Input';
import { KeyValue } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { Select } from '@/design/Select';
import { Skeleton } from '@/design/Skeleton';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { plural } from '@/lib/format';
import styles from './Automation.module.css';
import { Caveat } from './Caveat';
import { useAnsibleStatus, useTemplates } from './data';
import { DeviceSelect } from './DeviceSelect';
import { given } from './given';
import { EMPTY_SELECTION, toSelector, type SelectionDraft } from './selector';
import { VariablesForm } from './Variables';

export interface AnsibleTabProps {
  onOpenJob: (jobId: string) => void;
}

export function AnsibleTab({ onOpenJob }: AnsibleTabProps) {
  const status = useAnsibleStatus();

  if (status.isLoading) return <Skeleton shape="block" height={280} />;

  const data = status.data;
  const usable = data?.installed === true && !given(data.unsupported);

  return (
    <div className={styles.stack}>
      {/* The caveats come before anything that could start a run. */}
      <Caveat caveats={data?.caveats ?? []} />

      <Panel title="Ansible on this machine">
        <div className={styles.ansibleStatus}>
          <StatusPill tone={usable ? 'ok' : 'idle'}>
            {data?.installed === true ? 'Installed' : 'Not installed'}
          </StatusPill>
          <KeyValue
            layout="rows"
            items={[
              { label: 'Version', value: data?.version ?? null, mono: true },
              { label: 'Binary', value: data?.path ?? null, mono: true },
              {
                label: 'Collections',
                value:
                  !given(data?.collections) ? (
                    <span className={styles.muted}>ansible-galaxy could not be asked — not “none installed”</span>
                  ) : data.collections.length === 0 ? (
                    'none'
                  ) : (
                    <span className={styles.chips}>
                      {data.collections.map((collection) => (
                        <Badge key={collection} variant="outline">
                          {collection}
                        </Badge>
                      ))}
                    </span>
                  ),
              },
            ]}
          />
          {given(data?.unsupported) && (
            <p className={styles.unsupported} role="note">
              <Icon icon={Terminal} size={13} /> {data.unsupported}
            </p>
          )}
          <p className={styles.hint}>
            A device with an enable secret gets <code>ansible_become_method: enable</code>, and a device type Nocturne
            can map gets <code>ansible.netcommon.network_cli</code>. Both come from the{' '}
            <code>ansible.netcommon</code> collection: without it, the play fails.
          </p>
        </div>
      </Panel>

      <ExportPanel />
      <RunPanel usable={usable} caveats={data?.caveats ?? []} onOpenJob={onOpenJob} />
    </div>
  );
}

// ------------------------------------------------------------------ export

function ExportPanel() {
  const templates = useTemplates();
  const [templateId, setTemplateId] = useState('');
  const [changeId, setChangeId] = useState('');
  const [vars, setVars] = useState<Record<string, unknown>>({});
  const [selection, setSelection] = useState<SelectionDraft>(EMPTY_SELECTION);
  const [result, setResult] = useState<ExportedPlaybook | null>(null);

  const exportPlaybook = useRpcMutation<ExportedPlaybook, ExportPlaybookParams>('ansible.export_playbook', {
    errorToast: false,
  });
  const template: Template | undefined = (templates.data ?? []).find((entry) => entry.id === templateId);

  return (
    <Panel
      title="Export as a playbook"
      actions={
        <Button
          variant="primary"
          icon={Download}
          loading={exportPlaybook.isPending}
          disabled={templateId === '' && changeId.trim() === ''}
          onClick={() =>
            exportPlaybook.mutate(
              {
                ...(changeId.trim() === '' ? { template_id: templateId } : { change_id: changeId.trim() }),
                ...toSelector(selection),
                ...(Object.keys(vars).length > 0 ? { vars } : {}),
              },
              { onSuccess: setResult },
            )
          }
        >
          Generate
        </Button>
      }
    >
      <div className={styles.form}>
        <p className={styles.hint}>
          The playbook and the inventory for a change or a template, as text. Neither contains a secret: the inventory
          refers to credentials as <code>{"{{ lookup('env', 'NOCTURNE_PW_1') }}"}</code>. A device type whose module
          Nocturne is not certain of is refused by name rather than guessed at.
        </p>
        <Field label="From a template">
          <Select
            value={templateId}
            options={[
              { value: '', label: 'Not from a template' },
              ...(templates.data ?? []).map((entry) => ({ value: entry.id, label: entry.name })),
            ]}
            onChange={(event) => setTemplateId(event.target.value)}
          />
        </Field>
        {template !== undefined && <VariablesForm variables={template.variables} value={vars} onChange={setVars} />}
        <Field label="…or from a change" hint="The id of a change that has been planned.">
          <Input value={changeId} placeholder="change id" onChange={(event) => setChangeId(event.target.value)} />
        </Field>
        {templateId !== '' && <DeviceSelect id="ansible" value={selection} onChange={setSelection} compact />}

        {exportPlaybook.error !== null && (
          <div className={styles.refusal} role="alert">
            <h5>
              <Icon icon={FileCode} size={13} /> Nothing was generated
            </h5>
            <p>{exportPlaybook.error.message}</p>
          </div>
        )}
        {result !== null && (
          <>
            <p className={styles.caveatLine} role="note">
              <Icon icon={Terminal} size={13} />
              <span>{result.caveat}</span>
            </p>
            <CodeBlock title="playbook.yml" code={result.playbook} lineNumbers maxHeight={320} />
            <CodeBlock title="inventory.yml" code={result.inventory} lineNumbers maxHeight={320} />
          </>
        )}
      </div>
    </Panel>
  );
}

// --------------------------------------------------------------------- run

function RunPanel({
  usable,
  caveats,
  onOpenJob,
}: {
  usable: boolean;
  caveats: readonly string[];
  onOpenJob: (jobId: string) => void;
}) {
  const [path, setPath] = useState('');
  const [limit, setLimit] = useState('');
  const [check, setCheck] = useState(true);
  const [dryRun, setDryRun] = useState(false);
  const [selection, setSelection] = useState<SelectionDraft>(EMPTY_SELECTION);
  const [reported, setReported] = useState<unknown>(null);

  const start = useRpcMutation<AnsibleStarted, AnsibleRunParams>('ansible.run', { errorToast: false });

  if (!usable) {
    return (
      <Panel title="Run a playbook">
        <EmptyState
          size="compact"
          icon={Terminal}
          title="Not from here"
          description="Running a playbook needs a local ansible-playbook. Where it is missing, or where Nocturne runs as a server, this path is not offered — the export above still works, and so does everything else on this screen."
        />
      </Panel>
    );
  }

  const run = async () => {
    const ok = await confirm({
      title: dryRun ? 'Report what would be run?' : `Run ${path} on the selected devices?`,
      description: (
        <>
          {dryRun ? (
            'Nocturne reports the command line, the inventory and the devices, and starts nothing.'
          ) : (
            <>
              <strong>This is not a change.</strong> {caveats[0]}
            </>
          )}
        </>
      ),
      confirmLabel: dryRun ? 'Report it' : 'Run the playbook',
      tone: dryRun ? 'default' : 'danger',
    });
    if (!ok) return;
    start.mutate(
      {
        playbook_path: path.trim(),
        ...toSelector(selection),
        ...(limit.trim() === '' ? {} : { limit: limit.trim() }),
        check,
        ...(dryRun ? { dry_run: true } : {}),
      },
      {
        onSuccess: (result) => {
          if (typeof result.job_id === 'string' && result.job_id !== '') onOpenJob(result.job_id);
          else setReported(result);
        },
      },
    );
  };

  return (
    <Panel
      title="Run a playbook"
      actions={
        <Button
          variant={dryRun ? 'secondary' : 'danger'}
          icon={Play}
          loading={start.isPending}
          disabled={path.trim() === ''}
          onClick={() => void run()}
        >
          {dryRun ? 'Report what would run…' : 'Run outside the pipeline…'}
        </Button>
      }
    >
      <div className={styles.form}>
        <Field label="Playbook path on this machine" required>
          <Input value={path} placeholder="/srv/playbooks/ntp.yml" onChange={(event) => setPath(event.target.value)} />
        </Field>
        <Switch checked={check} onChange={(event) => setCheck(event.target.checked)}>
          Pass --check — what check mode means is each module’s business
        </Switch>
        <Switch checked={dryRun} onChange={(event) => setDryRun(event.target.checked)}>
          Dry run — report the command line, the inventory and the devices, and start nothing
        </Switch>
        <Field
          label="--limit"
          hint="Left empty, the generated hosts are used. -i, -l and the password prompts are refused with the reason."
        >
          <Input value={limit} onChange={(event) => setLimit(event.target.value)} />
        </Field>
        <DeviceSelect id="ansibleRun" value={selection} onChange={setSelection} compact />
        <p className={styles.hint}>
          The inventory is generated from these devices, written at mode 0600 in a 0700 directory, and removed however
          the run ends. Credentials are passed only in the process environment. The per-device outcome is parsed from{' '}
          <code>PLAY RECAP</code>, so a playbook with a custom stdout callback may report {plural(0, 'host')}.
        </p>

        {start.error !== null && (
          <div className={styles.refusal} role="alert">
            <h5>
              <Icon icon={Terminal} size={13} /> Nothing was started
            </h5>
            <p>{start.error.message}</p>
          </div>
        )}
        {reported !== null && <CodeBlock title="What would be run" code={JSON.stringify(reported, null, 2)} />}
      </div>
    </Panel>
  );
}
