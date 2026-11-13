/**
 * The alert-rule editor.
 *
 * The one thing here that is not an ordinary form is **Test**: it sends the
 * rule *as it stands in this drawer* to `alerts.rules.test`, which evaluates
 * it against what is already stored and answers with the devices it would
 * fire on — without raising anything. Nothing is sent as an `id`, because the
 * backend would then test the saved rule instead of the edited one.
 */

import { FlaskConical, Save } from 'lucide-react';
import { useState } from 'react';
import { useRpcMutation } from '@/api/hooks';
import type {
  AlertRule,
  AlertRuleSaveParams,
  AlertRuleTestResult,
  Condition,
  InterfaceSelector,
  Severity,
  ThresholdOp,
} from '@/api/types/monitor';
import { Button } from '@/design/Button';
import { Checkbox } from '@/design/Checkbox';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { Select } from '@/design/Select';
import { Switch } from '@/design/Switch';
import { TagInput } from '@/design/TagInput';
import { formatCount } from '@/lib/format';
import { useChannels } from './data';
import styles from './Monitoring.module.css';
import {
  CONDITION_KINDS,
  SEVERITIES,
  conditionName,
  conditionText,
  metricLabel,
  severityLabel,
  type ConditionKind,
} from './vocab';

/** The metrics a threshold or no-data rule is usually written against. */
const METRIC_CHOICES = [
  'cpu.pct',
  'mem.pct',
  'temp.c',
  'icmp.rtt_ms',
  'icmp.loss_pct',
  'if.util_pct',
  'if.in_bps',
  'if.out_bps',
  'if.in_err_ps',
  'if.out_err_ps',
  'if.*_err_ps',
  'if.*_disc_ps',
];

interface Form {
  name: string;
  enabled: boolean;
  severity: Severity;
  for_s: number;
  kind: ConditionKind;
  metric: string;
  instance: string;
  op: ThresholdOp;
  value: number;
  clear_value: string;
  only: InterfaceSelector;
  no_data_for_s: number;
  multiple: number;
  window_s: number;
  sites: string[];
  roles: string[];
  tags: string[];
  include_lab: boolean;
  notify: string[];
}

function toForm(rule: AlertRule | null): Form {
  const condition = rule?.condition;
  return {
    name: rule?.name ?? '',
    enabled: rule?.enabled ?? true,
    severity: rule?.severity ?? 'medium',
    for_s: rule?.for_s ?? 0,
    kind: (condition?.type ?? 'threshold') as ConditionKind,
    metric: condition?.type === 'threshold' || condition?.type === 'no_data' ? condition.metric : 'cpu.pct',
    instance: condition?.type === 'threshold' ? (condition.instance ?? '') : '',
    op: condition?.type === 'threshold' ? condition.op : 'gt',
    value: condition?.type === 'threshold' ? condition.value : 90,
    clear_value:
      condition?.type === 'threshold' && condition.clear_value != null ? String(condition.clear_value) : '',
    only: condition?.type === 'interface_down' ? condition.only : 'uplinks',
    no_data_for_s: condition?.type === 'no_data' ? condition.for_s : 900,
    multiple: condition?.type === 'rtt_baseline' ? condition.multiple : 5,
    window_s: condition?.type === 'rtt_baseline' ? condition.window_s : 86_400,
    sites: rule?.scope.sites ?? [],
    roles: rule?.scope.roles ?? [],
    tags: rule?.scope.tags ?? [],
    include_lab: rule?.scope.include_lab ?? false,
    notify: rule?.notify ?? [],
  };
}

function toCondition(form: Form): Condition {
  switch (form.kind) {
    case 'device_down':
      return { type: 'device_down' };
    case 'config_changed':
      return { type: 'config_changed' };
    case 'interface_down':
      return { type: 'interface_down', only: form.only };
    case 'no_data':
      return { type: 'no_data', metric: form.metric, for_s: form.no_data_for_s };
    case 'rtt_baseline':
      return { type: 'rtt_baseline', multiple: form.multiple, window_s: form.window_s };
    case 'threshold':
      return {
        type: 'threshold',
        metric: form.metric,
        instance: form.instance.trim() || null,
        op: form.op,
        value: form.value,
        clear_value: form.clear_value.trim() === '' ? null : Number(form.clear_value),
      };
  }
}

export interface RuleDrawerProps {
  rule: AlertRule | null;
  onClose: () => void;
}

export function RuleDrawer({ rule, onClose }: RuleDrawerProps) {
  const [form, setForm] = useState<Form>(() => toForm(rule));
  const [result, setResult] = useState<AlertRuleTestResult | null>(null);
  const channels = useChannels();

  const save = useRpcMutation<AlertRule, AlertRuleSaveParams>('alerts.rules.save', {
    invalidates: ['alerts.'],
    successToast: rule ? 'Rule saved' : 'Rule created',
    errorToast: false,
  });
  const test = useRpcMutation<AlertRuleTestResult, { rule: AlertRule }>('alerts.rules.test', {
    errorToast: false,
  });

  const set = (patch: Partial<Form>) => {
    setForm((current) => ({ ...current, ...patch }));
    setResult(null);
  };

  const condition = toCondition(form);
  /** The rule as typed, for the test call and for the sentence under it. */
  const asRule: AlertRule = {
    id: rule?.id ?? '',
    name: form.name.trim() || 'Unnamed rule',
    enabled: form.enabled,
    builtin: rule?.builtin ?? false,
    severity: form.severity,
    scope: {
      sites: form.sites,
      roles: form.roles,
      tags: form.tags,
      device_ids: rule?.scope.device_ids ?? [],
      include_lab: form.include_lab,
    },
    condition,
    for_s: form.for_s,
    notify: form.notify,
    created_at: rule?.created_at ?? 0,
    updated_at: rule?.updated_at ?? 0,
  };

  const needsMetric = form.kind === 'threshold' || form.kind === 'no_data';

  return (
    <Drawer
      open
      size="lg"
      onClose={onClose}
      title={rule ? `Edit ${rule.name}` : 'New alert rule'}
      description={
        rule?.builtin ? 'A built-in rule: it can be changed or switched off, but not deleted.' : undefined
      }
      footer={
        <>
          <Button
            icon={FlaskConical}
            loading={test.isPending}
            onClick={() => test.mutate({ rule: asRule }, { onSuccess: setResult, onError: () => setResult(null) })}
          >
            Test against what is stored
          </Button>
          <Button
            variant="primary"
            icon={Save}
            loading={save.isPending}
            disabled={form.name.trim() === ''}
            onClick={() =>
              save.mutate(
                {
                  ...(rule ? { id: rule.id } : {}),
                  name: form.name.trim(),
                  enabled: form.enabled,
                  severity: form.severity,
                  scope: asRule.scope,
                  condition,
                  for_s: form.for_s,
                  notify: form.notify,
                },
                { onSuccess: onClose },
              )
            }
          >
            Save
          </Button>
        </>
      }
    >
      <div className={styles.form}>
        {save.error && <p className={styles.error}>{save.error.message}</p>}

        <Field label="Name" required>
          <Input
            aria-label="Name"
            value={form.name}
            placeholder="What this rule is about"
            onChange={(event) => set({ name: event.target.value })}
          />
        </Field>

        <div className={styles.row}>
          <Field label="Severity">
            <Select
              aria-label="Severity"
              value={form.severity}
              options={SEVERITIES.map((severity) => ({ value: severity, label: severityLabel(severity) }))}
              onChange={(event) => set({ severity: event.target.value as Severity })}
            />
          </Field>
          <Field label="Hold for" hint="The condition must last this long before an alert is raised.">
            <Input
              aria-label="Hold for"
              type="number"
              min={0}
              max={86_400}
              value={String(form.for_s)}
              trailing={<span className={styles.muted}>seconds</span>}
              onChange={(event) => set({ for_s: Math.max(0, Number(event.target.value) || 0) })}
            />
          </Field>
        </div>

        <Field label="Fires when" hint={conditionText(condition)}>
          <Select
            aria-label="Fires when"
            value={form.kind}
            options={CONDITION_KINDS.map((kind) => ({ value: kind, label: conditionName(kind) }))}
            onChange={(event) => set({ kind: event.target.value as ConditionKind })}
          />
        </Field>

        {needsMetric && (
          <div className={styles.row}>
            <Field label="Metric" hint="A name, or a pattern such as if.*_err_ps.">
              <Input
                aria-label="Metric"
                mono
                list="monitoring-metrics"
                value={form.metric}
                onChange={(event) => set({ metric: event.target.value })}
              />
              <datalist id="monitoring-metrics">
                {METRIC_CHOICES.map((metric) => (
                  <option key={metric} value={metric}>
                    {metricLabel(metric)}
                  </option>
                ))}
              </datalist>
            </Field>
            {form.kind === 'no_data' && (
              <Field label="Missing for" hint="How long nothing may be recorded before this fires.">
                <Input
                  aria-label="Missing for"
                  type="number"
                  min={1}
                  value={String(form.no_data_for_s)}
                  trailing={<span className={styles.muted}>seconds</span>}
                  onChange={(event) => set({ no_data_for_s: Math.max(1, Number(event.target.value) || 1) })}
                />
              </Field>
            )}
          </div>
        )}

        {form.kind === 'threshold' && (
          <>
            <div className={styles.row}>
              <Field label="Goes">
                <Select
                  aria-label="Goes"
                  value={form.op}
                  options={[
                    { value: 'gt', label: 'Above' },
                    { value: 'lt', label: 'Below' },
                  ]}
                  onChange={(event) => set({ op: event.target.value as ThresholdOp })}
                />
              </Field>
              <Field label="Value">
                <Input
                  aria-label="Value"
                  type="number"
                  value={String(form.value)}
                  onChange={(event) => set({ value: Number(event.target.value) || 0 })}
                />
              </Field>
              <Field label="Clears at" hint="Optional. Between the two the alert neither fires nor resolves.">
                <Input
                  aria-label="Clears at"
                  type="number"
                  placeholder="None"
                  value={form.clear_value}
                  onChange={(event) => set({ clear_value: event.target.value })}
                />
              </Field>
            </div>
            <Field
              label="Only these instances"
              hint="A pattern over interface names or sensors, such as Gi0/*. Empty means any."
            >
              <Input
                aria-label="Only these instances"
                mono
                placeholder="Any"
                value={form.instance}
                onChange={(event) => set({ instance: event.target.value })}
              />
            </Field>
          </>
        )}

        {form.kind === 'interface_down' && (
          <Field label="Which interfaces">
            <Select
              aria-label="Which interfaces"
              value={form.only}
              options={[
                { value: 'uplinks', label: 'Only those with a collected neighbour (uplinks)' },
                { value: 'described', label: 'Only those with a description' },
                { value: 'all', label: 'Every interface' },
              ]}
              onChange={(event) => set({ only: event.target.value as InterfaceSelector })}
            />
          </Field>
        )}

        {form.kind === 'rtt_baseline' && (
          <div className={styles.row}>
            <Field label="Times the median">
              <Input
                aria-label="Times the median"
                type="number"
                min={1.1}
                step={0.5}
                value={String(form.multiple)}
                onChange={(event) => set({ multiple: Number(event.target.value) || 5 })}
              />
            </Field>
            <Field label="Baseline window" hint="At least ten minutes.">
              <Input
                aria-label="Baseline window"
                type="number"
                min={600}
                value={String(form.window_s)}
                trailing={<span className={styles.muted}>seconds</span>}
                onChange={(event) => set({ window_s: Number(event.target.value) || 86_400 })}
              />
            </Field>
          </div>
        )}

        <Field label="Only these sites" hint="Empty means every site. The same for roles and tags.">
          <TagInput aria-label="Only these sites" value={form.sites} onChange={(sites) => set({ sites })} />
        </Field>
        <div className={styles.row}>
          <Field label="Only these roles">
            <TagInput aria-label="Only these roles" value={form.roles} onChange={(roles) => set({ roles })} />
          </Field>
          <Field label="Only these tags">
            <TagInput aria-label="Only these tags" value={form.tags} onChange={(tags) => set({ tags })} />
          </Field>
        </div>

        <Switch checked={form.include_lab} onChange={(event) => set({ include_lab: event.target.checked })}>
          Include simulated lab devices
        </Switch>
        <Switch checked={form.enabled} onChange={(event) => set({ enabled: event.target.checked })}>
          Rule is on
        </Switch>

        <Field
          label="Notify"
          hint={
            form.notify.length === 0
              ? 'Nothing ticked: every enabled channel is told, as long as the alert is severe enough for it.'
              : 'Only the channels ticked are told.'
          }
        >
          <div className={styles.stackTight}>
            {(channels.data ?? []).map((channel) => (
              <Checkbox
                key={channel.id}
                checked={form.notify.includes(channel.id)}
                onChange={(event) =>
                  set({
                    notify: event.target.checked
                      ? [...form.notify, channel.id]
                      : form.notify.filter((id) => id !== channel.id),
                  })
                }
              >
                {channel.name}
              </Checkbox>
            ))}
            {channels.data?.length === 0 && (
              <p className={styles.hint}>No channels yet: alerts are recorded but go nowhere.</p>
            )}
          </div>
        </Field>

        {test.error && <p className={styles.error}>{test.error.message}</p>}

        {result && (
          <div className={styles.testResult} role="status">
            <strong>
              {result.matches.length === 0
                ? `Nothing would fire right now across ${formatCount(result.devices_in_scope)} ${result.devices_in_scope === 1 ? 'device' : 'devices'} in scope.`
                : `Would fire on ${formatCount(result.matches.length)} of ${formatCount(result.devices_in_scope)} ${result.devices_in_scope === 1 ? 'device' : 'devices'} in scope.`}
            </strong>
            {result.note && <span className={styles.hint}>{result.note}</span>}
            {result.matches.length > 0 && (
              <ul className={styles.matches}>
                {result.matches.slice(0, 50).map((match) => (
                  <li key={`${match.device_id}:${match.instance ?? ''}`} className={styles.match}>
                    <span className={styles.matchDevice}>
                      {match.device}
                      {match.instance ? ` · ${match.instance}` : ''}
                    </span>
                    <span className={styles.matchDetail}>{match.detail}</span>
                  </li>
                ))}
              </ul>
            )}
            {result.matches.length > 50 && (
              <span className={styles.hint}>Showing the first 50 of {formatCount(result.matches.length)}.</span>
            )}
          </div>
        )}
      </div>
    </Drawer>
  );
}
