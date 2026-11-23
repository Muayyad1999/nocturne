/**
 * One rule: what it matches, when it raises, what ends its alert — and what
 * it would do with a line.
 *
 * The engine is the judge of whether a rule can work. `logs.rules.save`
 * answers `invalid` with the reason, in a sentence, and that sentence is
 * shown **beside the field it is about**, unchanged (`fieldOfRuleError`).
 * The only thing refused before the engine sees it is a number field that
 * does not hold a number, because that cannot be sent at all.
 *
 * A built-in rule's definition is the application's: its pattern, its
 * threshold and how it clears are shown, not edited, together with the line
 * it was tested against and where that line came from — quoted from a
 * vendor's documentation, or synthetic and said to be. Its switch, its scope
 * and its throttle are the user's.
 *
 * The test box hands the rule *as it is in the form* and one pasted line to
 * `logs.rules.test`, which stores nothing. For a rule on the trap stream the
 * line is a trap's message, and the answer has a notification name and no
 * severity. A match that raises nothing by itself — a threshold wants more
 * of them — is said to be exactly that, in the engine's words.
 */

import { FlaskConical, Save, Trash, X } from 'lucide-react';
import { useState } from 'react';
import type { Device } from '@/api/types/core';
import type { LogRule, RuleScope, SignalKind } from '@/api/types/logs';
import { Badge } from '@/design/Badge';
import { Button, IconButton } from '@/design/Button';
import { CodeBlock } from '@/design/CodeBlock';
import { Combobox } from '@/design/Combobox';
import { confirm } from '@/design/Dialog';
import { Drawer } from '@/design/Drawer';
import { Field } from '@/design/Field';
import { Input } from '@/design/Input';
import { KeyValue } from '@/design/KeyValue';
import { Select } from '@/design/Select';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { TagInput } from '@/design/TagInput';
import { Textarea } from '@/design/Textarea';
import { useDeleteRule, useDevices, useSaveRule, useTestRule } from './data';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';
import { SEVERITY_NAMES } from './query';
import {
  ALERT_SEVERITIES,
  RULE_STREAMS,
  SIGNAL_KINDS,
  captureNames,
  clearingPhrase,
  draftOfRule,
  fieldOfRuleError,
  lineSource,
  raisesPhrase,
  ruleParams,
  scopePhrase,
  testReading,
  thresholdPhrase,
  type RuleDraft,
  type RuleErrors,
} from './rules';

export interface RuleDrawerProps {
  /** `null` for a rule that does not exist yet. */
  rule: LogRule | null;
  /** Every rule, for the "only while this rule's alert is open" choice. */
  rules: readonly LogRule[];
  capabilities: Capabilities;
  onClose: () => void;
}

export function RuleDrawer({ rule, rules, capabilities, onClose }: RuleDrawerProps) {
  const [draft, setDraft] = useState<RuleDraft>(() => draftOfRule(rule ?? undefined));
  const [line, setLine] = useState(rule?.test_line ?? '');
  const [local, setLocal] = useState<RuleErrors>({});
  const devices = useDevices(capabilities.devices);
  const save = useSaveRule();
  const remove = useDeleteRule();
  const test = useTestRule();

  const set = <K extends keyof RuleDraft>(key: K, value: RuleDraft[K]) =>
    setDraft((current) => ({ ...current, [key]: value }));

  // The engine's refusal, placed beside the field it is about. What was
  // found here before sending (text that is not a number) takes precedence
  // for its own field.
  const errors: RuleErrors = { ...local };
  if (save.error) {
    const field = fieldOfRuleError(save.error.message);
    errors[field] ??= save.error.message;
  }

  const builtin = draft.builtin;
  const editable = capabilities.ruleSave;
  const captures = captureNames(draft.pattern);
  const isAlert = draft.emitKind === 'alert';
  const isTrap = draft.stream === 'trap';

  const submit = () => {
    const { params, errors: found } = ruleParams(draft);
    setLocal(found);
    if (Object.keys(found).length > 0) return;
    save.mutate(params, { onSuccess: onClose });
  };

  const run = () => {
    const { params, errors: found } = ruleParams(draft);
    setLocal(found);
    if (Object.keys(found).length > 0) return;
    // What is saved for a built-in is its three settings; what is *tested*
    // is the rule itself, so the definition goes with them.
    test.mutate({ rule: builtin && rule ? { ...rule, ...params } : params, line });
  };

  const destroy = async () => {
    if (!rule) return;
    const confirmed = await confirm({
      title: `Delete the rule “${rule.name}”?`,
      description:
        'What it was counting goes with it. An alert it raised stays as it is in the alert list: nothing is left that could say the condition ended.',
      confirmLabel: 'Delete the rule',
      tone: 'danger',
    });
    if (confirmed) remove.mutate({ id: rule.id }, { onSuccess: onClose });
  };

  const reading = test.data ? testReading(test.data, draft.stream) : null;
  const source = lineSource(draft.testLineSource);
  // Rules that raise an alert, other than this one: what `after` can name.
  const followable = rules.filter((other) => other.emit.kind === 'alert' && other.id !== draft.id);

  return (
    <Drawer
      open
      onClose={onClose}
      size="lg"
      title={rule ? rule.name : 'New rule'}
      description={
        builtin
          ? `Built-in · ${draft.platform}`
          : rule
            ? 'A rule of your own'
            : 'A pattern, and what to raise when it matches'
      }
      footer={
        <>
          {rule && !builtin && capabilities.ruleDelete && (
            <Button icon={Trash} variant="ghost" loading={remove.isPending} onClick={() => void destroy()}>
              Delete…
            </Button>
          )}
          {editable && (
            <Button variant="primary" icon={Save} loading={save.isPending} onClick={submit}>
              Save
            </Button>
          )}
        </>
      }
    >
      <div className={styles.form}>
        {errors.form && (
          <p className={styles.fieldError} role="alert">
            {errors.form}
          </p>
        )}

        {builtin ? (
          <>
            <p className={styles.note} role="note">
              This rule ships with Nocturne. Its definition is the application’s and is rewritten when a
              new version changes it; its switch, its scope and its throttle are yours and survive an
              upgrade.
            </p>
            <Definition rule={rule} draft={draft} rules={rules} />
          </>
        ) : (
          <>
            <Field label="Name" required error={errors.name}>
              <Input
                value={draft.name}
                disabled={!editable}
                onChange={(event) => set('name', event.target.value)}
                placeholder="ACL denies"
              />
            </Field>

            <div className={styles.formRow}>
              <Field label="Reads" hint={RULE_STREAMS.find((entry) => entry.value === draft.stream)?.help}>
                <Select
                  value={draft.stream}
                  disabled={!editable}
                  options={RULE_STREAMS.map(({ value, label }) => ({ value, label }))}
                  onChange={(event) => set('stream', event.target.value)}
                />
              </Field>
              {!isTrap && (
                <Field label="Only lines at least" hint="A line that came without a severity never passes a floor.">
                  <Select
                    value={draft.minSeverity}
                    disabled={!editable}
                    onChange={(event) => set('minSeverity', event.target.value)}
                  >
                    <option value="">Any severity</option>
                    {SEVERITY_NAMES.map((name, index) => (
                      <option key={name} value={String(index)}>
                        {name} ({index}) and worse
                      </option>
                    ))}
                  </Select>
                </Field>
              )}
            </div>

            <Field
              label="Pattern"
              required
              error={errors.pattern}
              hint={
                isTrap
                  ? 'A regular expression over the trap’s message: its name, then its bindings by name — linkDown ifIndex=3 ifAdminStatus=up(1) …'
                  : 'A regular expression over the line as the device sent it. Name a group with (?P<name>…) to count by it or use it in a title.'
              }
            >
              <Textarea
                mono
                rows={2}
                value={draft.pattern}
                disabled={!editable}
                spellCheck={false}
                onChange={(event) => set('pattern', event.target.value)}
              />
            </Field>

            <div className={styles.formRow}>
              <Field label="Raises" hint={SIGNAL_KINDS.find((kind) => kind.value === draft.emitKind)?.help}>
                <Select
                  value={draft.emitKind}
                  disabled={!editable}
                  options={SIGNAL_KINDS.map(({ value, label }) => ({ value, label }))}
                  onChange={(event) => set('emitKind', event.target.value as SignalKind)}
                />
              </Field>
              {isAlert && (
                <Field label="Severity" error={errors.severity}>
                  <Select
                    value={draft.severity}
                    disabled={!editable}
                    options={ALERT_SEVERITIES.map((severity) => ({ value: severity, label: severity }))}
                    onChange={(event) => set('severity', event.target.value)}
                  />
                </Field>
              )}
              {(draft.emitKind === 'interface_down' || draft.emitKind === 'interface_up') && (
                <Field label="Group that holds the interface" hint="The name of the capture, without braces.">
                  <Input
                    mono
                    value={draft.interfaceGroup}
                    disabled={!editable}
                    onChange={(event) => set('interfaceGroup', event.target.value)}
                  />
                </Field>
              )}
            </div>

            {isAlert && (
              <Field
                label="Alert title"
                required
                error={errors.title}
                hint={
                  captures.length === 0
                    ? 'May use the pattern’s groups in braces, once it names some.'
                    : `May use the pattern’s groups: ${captures.map((name) => `{${name}}`).join(' ')}`
                }
              >
                <Input
                  value={draft.title}
                  disabled={!editable}
                  onChange={(event) => set('title', event.target.value)}
                  placeholder="{interface} is down"
                />
              </Field>
            )}

            <fieldset className={styles.group}>
              <legend className={styles.groupLegend}>Threshold</legend>
              <Switch
                checked={draft.thresholdOn}
                disabled={!editable}
                onChange={(event) => set('thresholdOn', event.target.checked)}
              >
                Raise only when several matching lines arrive inside a window
              </Switch>
              {draft.thresholdOn ? (
                <>
                  <div className={styles.formRow}>
                    <Field label="Lines" error={errors.count} hint="1 raises on every line: one instance per group.">
                      <Input
                        mono
                        inputMode="numeric"
                        value={draft.count}
                        disabled={!editable}
                        onChange={(event) => set('count', event.target.value)}
                      />
                    </Field>
                    <Field label="Inside (seconds)" error={errors.window} hint="The window slides and is half open.">
                      <Input
                        mono
                        inputMode="numeric"
                        value={draft.window}
                        disabled={!editable}
                        onChange={(event) => set('window', event.target.value)}
                      />
                    </Field>
                  </div>
                  <Field
                    label="Counted per"
                    error={errors.group_by}
                    hint={
                      captures.length === 0
                        ? 'Groups of the pattern. With none, the lines are counted per device.'
                        : `Groups of the pattern: ${captures.join(', ')}. With none, the lines are counted per device.`
                    }
                  >
                    <TagInput
                      mono
                      value={draft.groupBy}
                      disabled={!editable}
                      onChange={(tags) => set('groupBy', tags)}
                      placeholder="source"
                    />
                  </Field>
                  <p className={styles.muted}>
                    The count is kept in memory and starts again when Nocturne restarts.
                  </p>
                </>
              ) : (
                <p className={styles.muted}>Without one, every matching line raises (inside the throttle below).</p>
              )}
            </fieldset>

            <fieldset className={styles.group}>
              <legend className={styles.groupLegend}>What ends the alert</legend>
              {isAlert ? (
                <>
                  <Field
                    label="Its opposite event"
                    error={errors.clear_pattern}
                    hint="A line matching this clears the alert. It has to capture the same groups the lines are counted per, or it could not say which alert it clears."
                  >
                    <Textarea
                      mono
                      rows={2}
                      value={draft.clearPattern}
                      disabled={!editable}
                      spellCheck={false}
                      onChange={(event) => set('clearPattern', event.target.value)}
                    />
                  </Field>
                  <Field
                    label="Clear after this many quiet seconds"
                    error={errors.clear_after}
                    hint="No matching line for this long, for that device and instance. Empty: no quiet period."
                  >
                    <Input
                      mono
                      inputMode="numeric"
                      value={draft.clearAfter}
                      disabled={!editable}
                      onChange={(event) => set('clearAfter', event.target.value)}
                    />
                  </Field>
                  <p className={styles.muted}>
                    With neither, the alert stays open until a person closes it in the alert list.
                  </p>
                </>
              ) : (
                <p className={styles.muted}>
                  Only a rule that raises an alert has something to clear.
                  {errors.clear_pattern ? ` ${errors.clear_pattern}` : ''}
                </p>
              )}
            </fieldset>

            <Field
              label="Only while this rule’s alert is open"
              error={errors.after}
              hint="For the same device and instance: “a login that succeeds straight after a burst of failures”."
            >
              <Select value={draft.after} disabled={!editable} onChange={(event) => set('after', event.target.value)}>
                <option value="">Always</option>
                {followable.map((other) => (
                  <option key={other.id} value={other.id}>
                    {other.name}
                  </option>
                ))}
                {draft.after !== '' && !followable.some((other) => other.id === draft.after) && (
                  <option value={draft.after}>{draft.after}</option>
                )}
              </Select>
            </Field>
          </>
        )}

        <fieldset className={styles.group}>
          <legend className={styles.groupLegend}>Yours to set</legend>
          <Switch
            checked={draft.enabled}
            disabled={!editable}
            onChange={(event) => set('enabled', event.target.checked)}
          >
            Switched on
          </Switch>
          <Field
            label="Throttle (seconds)"
            error={errors.throttle}
            hint="At most one signal per rule, device and instance in this long; the rest are counted."
          >
            <Input
              mono
              inputMode="numeric"
              value={draft.throttle}
              disabled={!editable}
              onChange={(event) => set('throttle', event.target.value)}
            />
          </Field>
          <ScopeFields
            scope={draft.scope}
            devices={devices.data ?? []}
            disabled={!editable}
            onChange={(scope) => set('scope', scope)}
          />
        </fieldset>

        {builtin ? (
          <section aria-label="The line it was tested against" className={styles.group}>
            <h4 className={styles.groupLegend}>The line it was tested against</h4>
            {draft.testLine === '' ? (
              <p className={styles.muted}>This rule states no line.</p>
            ) : (
              <CodeBlock code={draft.testLine} copyable wrap />
            )}
            <p className={styles.hintDetail}>
              <Badge tone={source.kind === 'synthetic' ? 'warn' : 'neutral'} variant="outline">
                {source.label}
              </Badge>{' '}
              {source.text}
            </p>
            <p className={styles.muted}>No built-in rule’s line was recorded from a device.</p>
          </section>
        ) : (
          <fieldset className={styles.group}>
            <legend className={styles.groupLegend}>A line it is meant to match</legend>
            <Field label="Line" hint="Kept with the rule, so the next person can see what it was written for.">
              <Textarea
                mono
                rows={2}
                value={draft.testLine}
                disabled={!editable}
                spellCheck={false}
                onChange={(event) => set('testLine', event.target.value)}
              />
            </Field>
            <Field label="Where it came from" hint="A device’s own log, a vendor’s documentation, or “synthetic” if you wrote it by hand.">
              <Input
                value={draft.testLineSource}
                disabled={!editable}
                onChange={(event) => set('testLineSource', event.target.value)}
              />
            </Field>
          </fieldset>
        )}

        {capabilities.ruleTest && (
          <section aria-label="Try the rule on a line" className={styles.group}>
            <h4 className={styles.groupLegend}>Try it on a line</h4>
            <Field
              label={isTrap ? 'A trap’s message' : 'A log line'}
              hint={
                isTrap
                  ? 'As the Lines pane shows it for the trap stream. Nothing is stored and nothing is raised.'
                  : 'As a device sends it, with or without its <priority>. Nothing is stored and nothing is raised.'
              }
            >
              <Textarea
                mono
                rows={3}
                value={line}
                spellCheck={false}
                onChange={(event) => setLine(event.target.value)}
              />
            </Field>
            <div>
              <Button icon={FlaskConical} loading={test.isPending} disabled={line.trim() === ''} onClick={run}>
                Test the rule
              </Button>
            </div>
            {test.error && (
              <p className={styles.fieldError} role="alert">
                {test.error.message}
              </p>
            )}
            {reading && !test.error && (
              <div className={styles.testResult} role="status" data-testid="rule-test-result">
                <StatusPill tone={reading.tone}>{reading.headline}</StatusPill>
                {reading.sentences.map((sentence) => (
                  <p key={sentence} className={styles.hintDetail}>
                    {sentence}
                  </p>
                ))}
                <KeyValue
                  items={reading.facts.map((fact) => ({ label: fact.label, value: fact.value, mono: fact.mono }))}
                />
              </div>
            )}
          </section>
        )}
      </div>
    </Drawer>
  );
}

/** A built-in rule's definition, read-only. */
function Definition({
  rule,
  draft,
  rules,
}: {
  rule: LogRule | null;
  draft: RuleDraft;
  rules: readonly LogRule[];
}) {
  if (!rule) return null;
  const after = rule.after ? (rules.find((other) => other.id === rule.after)?.name ?? rule.after) : null;
  return (
    <>
      <CodeBlock code={rule.pattern} title="Pattern" copyable wrap />
      {rule.clear_pattern != null && rule.clear_pattern !== '' && (
        <CodeBlock code={rule.clear_pattern} title="Its opposite event" copyable wrap />
      )}
      <KeyValue
        items={[
          { label: 'For', value: draft.platform },
          { label: 'Reads', value: RULE_STREAMS.find((entry) => entry.value === rule.stream)?.label ?? rule.stream },
          { label: 'Raises', value: raisesPhrase(rule.emit) },
          { label: 'Alert title', value: rule.emit.kind === 'alert' ? rule.emit.title : null, mono: true },
          { label: 'When', value: thresholdPhrase(rule) },
          { label: 'Cleared by', value: clearingPhrase(rule) },
          { label: 'Only while open', value: after },
          {
            label: 'Only lines at least',
            value: rule.min_severity == null ? null : `${SEVERITY_NAMES[rule.min_severity] ?? rule.min_severity} and worse`,
          },
        ]}
      />
    </>
  );
}

function ScopeFields({
  scope,
  devices,
  disabled,
  onChange,
}: {
  scope: RuleScope;
  devices: readonly Device[];
  disabled: boolean;
  onChange: (scope: RuleScope) => void;
}) {
  const names = new Map(devices.map((device) => [device.id, device.name]));
  const unchosen = devices.filter((device) => !scope.device_ids.includes(device.id));
  return (
    <>
      <p className={styles.muted}>Applies to: {scopePhrase(scope)}. Leave everything empty for every device.</p>
      <div className={styles.formRow}>
        <Field label="Sites">
          <TagInput value={scope.sites} disabled={disabled} onChange={(sites) => onChange({ ...scope, sites })} />
        </Field>
        <Field label="Roles">
          <TagInput value={scope.roles} disabled={disabled} onChange={(roles) => onChange({ ...scope, roles })} />
        </Field>
      </div>
      <div className={styles.formRow}>
        <Field label="Tags">
          <TagInput value={scope.tags} disabled={disabled} onChange={(tags) => onChange({ ...scope, tags })} />
        </Field>
        <Field label="Profiles" hint="Device profile ids, such as cisco_ios.">
          <TagInput
            mono
            value={scope.profiles}
            disabled={disabled}
            onChange={(profiles) => onChange({ ...scope, profiles })}
          />
        </Field>
      </div>
      <Field label="Devices">
        <Combobox
          value={null}
          disabled={disabled || unchosen.length === 0}
          placeholder={devices.length === 0 ? 'No devices in the inventory' : 'Add a device…'}
          options={unchosen.map((device) => ({ value: device.id, label: device.name }))}
          onChange={(id) => id && onChange({ ...scope, device_ids: [...scope.device_ids, id] })}
          emptyText="No such device"
        />
      </Field>
      {scope.device_ids.length > 0 && (
        <ul className={styles.chosen} aria-label="Chosen devices">
          {scope.device_ids.map((id) => (
            <li key={id}>
              {names.get(id) ?? id}
              {!disabled && (
                <IconButton
                  icon={X}
                  size="sm"
                  variant="ghost"
                  aria-label={`Remove ${names.get(id) ?? id}`}
                  onClick={() => onChange({ ...scope, device_ids: scope.device_ids.filter((other) => other !== id) })}
                />
              )}
            </li>
          ))}
        </ul>
      )}
    </>
  );
}
