/**
 * Drafting a change: pick the devices, say what should happen, plan it.
 *
 * The three steps are the spec's, and the order matters — a change is "these
 * devices should get this configuration", and planning is the last thing that
 * happens, because planning is what reads every device and turns an intention
 * into commands. Nothing here touches a device except *Preview*, which asks
 * the engine what it would do on one device and creates nothing.
 */

import { ArrowLeft, ArrowRight, Play, Server, Wand2 } from 'lucide-react';
import { lazy, Suspense, useMemo, useState } from 'react';
import { useNavigate } from 'react-router';
import { useRpc, useRpcMutation } from '@/api/hooks';
import type { RpcError } from '@/api/errors';
import type {
  Change,
  ChangeCreatedWithJob,
  ChangesCreateParams,
  ChangesPreviewParams,
  Intent,
  Options,
  Plan,
} from '@/api/types/changes';
import type { DeviceRow } from '@/api/types/core';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { Dialog } from '@/design/Dialog';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Panel } from '@/design/Panel';
import { RadioGroup } from '@/design/RadioGroup';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Textarea } from '@/design/Textarea';
import { plural } from '@/lib/format';
import { RISK_LABEL, riskTone } from './change';
import { CommandList } from './CommandList';
import { useHasMethod } from '@/api/hooks';
import { TemplatePicker, type TemplateChoice } from './TemplatePicker';
import styles from './NewChange.module.css';

export interface NewChangeProps {
  open: boolean;
  onClose: () => void;
  /** Pre-selected device, when the change was started from somewhere else. */
  deviceId?: string;
}

type Source = 'lines' | 'intended' | 'template';

const STEPS = ['Devices', 'What to do', 'Review and plan'] as const;

/**
 * CodeMirror is most of this screen's code and only step 2 needs it, so it
 * loads when somebody is about to type configuration — never when they are
 * only reading a change that already exists.
 */
const LinesEditor = lazy(() =>
  import('./LinesEditor').then((module) => ({ default: module.LinesEditor })),
);

/** Holds the box's height while the editor is on its way, so nothing jumps. */
function EditorFallback() {
  return <div className={styles.editorFallback} aria-hidden />;
}

function intentOf(source: Source, lines: string, config: string, exact: boolean): Intent {
  if (source === 'intended') return { type: 'intended', config, exact };
  return { type: 'lines', lines: lines.split('\n').filter((line) => line.trim() !== '') };
}

export function NewChange({ open, onClose, deviceId }: NewChangeProps) {
  const navigate = useNavigate();
  const [step, setStep] = useState(0);
  const [selected, setSelected] = useState<ReadonlySet<string>>(
    () => new Set(deviceId ? [deviceId] : []),
  );
  const [search, setSearch] = useState('');
  const [source, setSource] = useState<Source>('lines');
  const [lines, setLines] = useState('');
  const [config, setConfig] = useState('');
  const [exact, setExact] = useState(false);
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [ticket, setTicket] = useState('');
  const [options, setOptions] = useState<Options>({});
  const [preview, setPreview] = useState<{ device: string; plan: Plan } | null>(null);

  const hasTemplates = useHasMethod('templates.list') === true;

  const devices = useRpc<DeviceRow[]>('devices.list', undefined, {
    invalidateOn: ['devices.changed'],
  });

  const rows = useMemo(() => {
    const all = devices.data ?? [];
    const needle = search.trim().toLowerCase();
    if (needle === '') return all;
    return all.filter((device) =>
      [device.name, device.address, device.site, device.role].some((field) =>
        field.toLowerCase().includes(needle),
      ),
    );
  }, [devices.data, search]);

  const chosen = useMemo(
    () => (devices.data ?? []).filter((device) => selected.has(device.id)),
    [devices.data, selected],
  );

  const intent = intentOf(source, lines, config, exact);
  const hasIntent =
    source === 'intended' ? config.trim() !== '' : (intent as { lines: string[] }).lines.length > 0;

  const create = useRpcMutation<Change | ChangeCreatedWithJob, ChangesCreateParams>(
    'changes.create',
    { invalidates: ['changes.'], errorToast: false },
  );
  const previewOne = useRpcMutation<Plan, ChangesPreviewParams>('changes.preview', {
    errorToast: false,
  });

  const reset = () => {
    setStep(0);
    setSelected(new Set(deviceId ? [deviceId] : []));
    setSearch('');
    setSource('lines');
    setLines('');
    setConfig('');
    setExact(false);
    setTitle('');
    setDescription('');
    setTicket('');
    setOptions({});
    setPreview(null);
    create.reset();
    previewOne.reset();
  };

  const close = () => {
    reset();
    onClose();
  };

  const plan = () => {
    create.mutate(
      {
        title: title.trim(),
        description: description.trim() || undefined,
        ticket: ticket.trim() || undefined,
        targets: chosen.map((device) => ({ device_id: device.id, intent })),
        options,
        plan: true,
      },
      {
        onSuccess: (result) => {
          const change = 'change' in result ? result.change : result;
          reset();
          onClose();
          void navigate(`/changes/${change.id}`);
        },
      },
    );
  };

  const canContinue = step === 0 ? chosen.length > 0 : step === 1 ? hasIntent : title.trim() !== '';
  const error = (create.error ?? null) as RpcError | null;

  const takeTemplate = (choice: TemplateChoice) => {
    setLines(choice.text);
    setSource('lines');
    if (title.trim() === '') setTitle(choice.name);
  };

  return (
    <Dialog
      open={open}
      onClose={close}
      size="lg"
      title="New change"
      description={`Step ${step + 1} of 3 — ${STEPS[step]}`}
      footer={
        <>
          {step > 0 && (
            <Button icon={ArrowLeft} onClick={() => setStep((at) => at - 1)}>
              Back
            </Button>
          )}
          <Button onClick={close}>Cancel</Button>
          {step < 2 ? (
            <Button
              variant="primary"
              trailingIcon={ArrowRight}
              disabled={!canContinue}
              onClick={() => setStep((at) => at + 1)}
            >
              Continue
            </Button>
          ) : (
            <Button
              variant="primary"
              icon={Play}
              disabled={!canContinue}
              loading={create.isPending}
              onClick={plan}
            >
              Plan on {plural(chosen.length, 'device')}
            </Button>
          )}
        </>
      }
    >
      <div className={styles.body}>
        <ol className={styles.stepper} aria-label="Steps">
          {STEPS.map((name, index) => (
            <li key={name} className={styles.stepItem} data-state={index === step ? 'now' : index < step ? 'done' : 'next'}>
              <span className={styles.stepNumber}>{index + 1}</span>
              {name}
            </li>
          ))}
        </ol>

        {step === 0 && (
          <div className={styles.step}>
            <p className={styles.lead}>
              Which devices should get this change? Every one of them is read and planned
              separately, and each gets its own commands.
            </p>
            <SearchInput
              value={search}
              onChange={setSearch}
              placeholder="Search by name, address, site or role"
              aria-label="Search devices"
            />
            {devices.isPending ? (
              <p className={styles.lead}>Reading the inventory…</p>
            ) : rows.length === 0 ? (
              <p className={styles.lead}>
                {(devices.data ?? []).length === 0
                  ? 'There are no devices yet. Add one on the Devices screen, or start a lab.'
                  : 'No device matches that search.'}
              </p>
            ) : (
              <ul className={styles.devices} aria-label="Devices">
                {rows.map((device) => (
                  <li key={device.id} className={styles.device}>
                    <Checkbox
                      checked={selected.has(device.id)}
                      onChange={(event) =>
                        setSelected((was) => {
                          const next = new Set(was);
                          if (event.target.checked) next.add(device.id);
                          else next.delete(device.id);
                          return next;
                        })
                      }
                    >
                      <span className={styles.deviceName}>{device.name}</span>
                    </Checkbox>
                    <span className={styles.deviceMeta}>
                      {device.address || device.site || device.role || ''}
                    </span>
                    {device.lab && <LabPill />}
                    {device.protected && <Badge tone="crit">Protected</Badge>}
                  </li>
                ))}
              </ul>
            )}
            <p className={styles.count}>{plural(chosen.length, 'device')} chosen</p>
          </div>
        )}

        {step === 1 && (
          <div className={styles.step}>
            <RadioGroup
              aria-label="What this change is"
              value={source}
              onChange={(value) => setSource(value as Source)}
              options={[
                {
                  value: 'lines',
                  label: 'Lines',
                  description: 'Configuration-mode lines. What you type is what is sent.',
                },
                {
                  value: 'intended',
                  label: 'Intended',
                  description:
                    'The configuration as it should be. Nocturne works out the commands by comparing it with the device.',
                },
                {
                  value: 'template',
                  label: 'From template',
                  description: hasTemplates
                    ? 'A saved piece of configuration, filled in.'
                    : 'Not available in this build: it has no template library.',
                  disabled: !hasTemplates,
                },
              ]}
            />

            {source === 'lines' && (
              <Field
                label="Configuration lines"
                hint="Section headers included. Abbreviations are refused when the change is planned: write keywords out in full."
              >
                <Suspense fallback={<EditorFallback />}>
                  <LinesEditor
                    label="Configuration lines"
                    value={lines}
                    onChange={setLines}
                    placeholder={'interface GigabitEthernet0/1\n description uplink to core\n exit'}
                  />
                </Suspense>
              </Field>
            )}

            {source === 'intended' && (
              <>
                <Field
                  label="Intended configuration"
                  hint="A whole configuration or one section of it, as it should read on the device."
                >
                  <Suspense fallback={<EditorFallback />}>
                    <LinesEditor
                      label="Intended configuration"
                      value={config}
                      onChange={setConfig}
                      placeholder={'interface GigabitEthernet0/1\n description uplink to core\n switchport mode trunk'}
                    />
                  </Suspense>
                </Field>
                <RadioGroup
                  aria-label="How to compare it"
                  value={exact ? 'exact' : 'additive'}
                  onChange={(value) => setExact(value === 'exact')}
                  options={[
                    {
                      value: 'additive',
                      label: 'Must be present',
                      description: 'Anything else on the device is left alone.',
                    },
                    {
                      value: 'exact',
                      label: 'Must be exactly this',
                      description: 'What is not in the text above is removed from the device.',
                    },
                  ]}
                />
              </>
            )}

            {source === 'template' && hasTemplates && (
              <TemplatePicker onChoose={takeTemplate} />
            )}

            <div className={styles.previewRow}>
              <Button
                icon={Wand2}
                disabled={!hasIntent || chosen.length === 0}
                loading={previewOne.isPending}
                onClick={() => {
                  const first = chosen[0];
                  if (!first) return;
                  previewOne.mutate(
                    { device_id: first.id, intent, options },
                    {
                      onSuccess: (result) => setPreview({ device: first.name, plan: result }),
                    },
                  );
                }}
              >
                Preview on {chosen[0]?.name ?? 'a device'}
              </Button>
              <span className={styles.previewHint}>
                Reads that one device and shows what it would be sent. Creates nothing.
              </span>
            </div>

            {previewOne.error && (
              <p className={styles.error} role="alert">
                {previewOne.error.message}
              </p>
            )}

            {preview && (
              <Panel title={`Preview on ${preview.device}`} padding="md">
                <div className={styles.preview}>
                  <div className={styles.previewBadges}>
                    <StatusPill tone={riskTone(preview.plan.risk.level)}>
                      {RISK_LABEL[preview.plan.risk.level]}
                    </StatusPill>
                    {preview.plan.protected_hits.length > 0 && (
                      <Badge tone="crit">
                        {plural(preview.plan.protected_hits.length, 'protected resource')}
                      </Badge>
                    )}
                  </div>
                  <CommandList
                    commands={preview.plan.commands}
                    label={`Preview commands for ${preview.device}`}
                  />
                  <ul className={styles.reasons}>
                    {preview.plan.risk.reasons.map((reason) => (
                      <li key={reason}>{reason}</li>
                    ))}
                  </ul>
                </div>
              </Panel>
            )}
          </div>
        )}

        {step === 2 && (
          <div className={styles.step}>
            <Field label="Title" required hint="What this change does, in a few words.">
              <Input
                value={title}
                onChange={(event) => setTitle(event.target.value)}
                placeholder="Guest VLAN on the east access switches"
                required
              />
            </Field>
            <Field label="Change reference" hint="A ticket or change-request number, if you have one.">
              <Input
                value={ticket}
                onChange={(event) => setTicket(event.target.value)}
                placeholder="CHG-1042"
              />
            </Field>
            <Field label="Description">
              <Textarea
                value={description}
                onChange={(event) => setDescription(event.target.value)}
                rows={2}
                placeholder="Why this is being done"
              />
            </Field>

            <Panel title="How it will be applied" collapsible defaultCollapsed storageId="changes.options">
              <div className={styles.options}>
                <Checkbox
                  checked={options.save ?? true}
                  onChange={(event) => setOptions((was) => ({ ...was, save: event.target.checked }))}
                >
                  Write to the startup configuration once it has verified
                </Checkbox>
                <Checkbox
                  checked={options.stop_on_failure ?? true}
                  onChange={(event) =>
                    setOptions((was) => ({ ...was, stop_on_failure: event.target.checked }))
                  }
                >
                  Stop at the first device that fails
                </Checkbox>
                <Field label="Devices at a time" hint="One after another is the safe default.">
                  <Select
                    value={String(options.parallel ?? 1)}
                    onChange={(event) =>
                      setOptions((was) => ({ ...was, parallel: Number(event.target.value) }))
                    }
                    options={[1, 2, 4, 8].map((value) => ({
                      value: String(value),
                      label: value === 1 ? 'One at a time' : `${value} at a time`,
                    }))}
                  />
                </Field>
                <Field
                  label="Dead-man switch"
                  hint="The device undoes the change by itself if Nocturne never comes back."
                >
                  <Select
                    value={options.guard_minutes === undefined ? 'auto' : String(options.guard_minutes)}
                    onChange={(event) =>
                      setOptions((was) => ({
                        ...was,
                        guard_minutes:
                          event.target.value === 'auto' ? undefined : Number(event.target.value),
                      }))
                    }
                    options={[
                      { value: 'auto', label: 'Decide from the risk' },
                      { value: '0', label: 'Never arm it' },
                      { value: '5', label: '5 minutes' },
                      { value: '10', label: '10 minutes' },
                      { value: '30', label: '30 minutes' },
                    ]}
                  />
                </Field>
              </div>
            </Panel>

            <div className={styles.summary}>
              <Server size={14} aria-hidden />
              <span>
                {plural(chosen.length, 'device')}: {chosen.map((device) => device.name).join(', ')}
              </span>
            </div>

            {error && (
              <p className={styles.error} role="alert" data-testid="create-error">
                {error.message}
              </p>
            )}
            <p className={styles.lead}>
              Planning connects to each device, reads its running configuration and works out the
              exact commands. It changes nothing.
            </p>
          </div>
        )}
      </div>
    </Dialog>
  );
}
