/**
 * The rules that turn lines into signals.
 *
 * A rule is a pattern over a syslog line — or over the message the receiver
 * writes for a trap — and what to raise when it matches: optionally only
 * when enough lines arrive inside a window (a threshold), counted per value
 * of some captures, and with a way for its alert to end again. The list
 * says all of that in three columns, and says which platform each rule is
 * for: a built-in rule covers the platform it names and no other.
 *
 * A built-in rule's definition is the application's. Its switch, its scope
 * and its throttle are the user's; the switch is on the row.
 *
 * Above the list is what the rules hold in memory (`logs.status`, `rules`):
 * how many (device, instance) keys they are counting, and the three numbers
 * that mean something was not done — keys forgotten, matches held back, and
 * calls to the monitor that were dropped. A count kept in memory starts
 * again when Nocturne restarts, and the pane says so.
 */

import { Plus, SlidersHorizontal, Trash } from 'lucide-react';
import { useMemo, useState } from 'react';
import type { LogRule, LogsStatus } from '@/api/types/logs';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { confirm } from '@/design/Dialog';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { formatCount, plural } from '@/lib/format';
import { useDeleteRule, useLogRules, useSaveRule } from './data';
import styles from './Logs.module.css';
import type { Capabilities } from './panes';
import { counter, type Counter } from './receivers';
import { RuleDrawer } from './RuleDrawer';
import {
  RULE_STREAMS,
  alertTone,
  clearingPhrase,
  filterRules,
  platformsOf,
  raisesPhrase,
  thresholdPhrase,
} from './rules';

export interface RulesPaneProps {
  capabilities: Capabilities;
  status: LogsStatus | undefined;
}

/** `null` while nothing is open; `rule: null` for a rule that does not exist yet. */
type Open = { rule: LogRule | null } | null;

export function RulesPane({ capabilities, status }: RulesPaneProps) {
  const rules = useLogRules();
  const save = useSaveRule();
  const remove = useDeleteRule();
  const [text, setText] = useState('');
  const [platform, setPlatform] = useState('');
  const [stream, setStream] = useState('');
  const [open, setOpen] = useState<Open>(null);
  /** The engine's refusal of a row's switch, shown under the list's toolbar. */
  const [refusal, setRefusal] = useState<string | null>(null);

  const all = rules.data ?? [];
  const shown = useMemo(() => filterRules(all, { text, platform, stream }), [all, text, platform, stream]);
  const platforms = useMemo(() => platformsOf(all), [all]);
  const own = all.filter((rule) => !rule.builtin).length;

  const toggle = (rule: LogRule, enabled: boolean) => {
    setRefusal(null);
    // A built-in takes its switch, scope and throttle; a rule of one's own is
    // sent whole, because the document is what is stored.
    save.mutate(
      rule.builtin
        ? { id: rule.id, enabled, scope: rule.scope, throttle_s: rule.throttle_s }
        : { ...rule, enabled },
      { onError: (error) => setRefusal(error.message) },
    );
  };

  const destroy = async (rule: LogRule) => {
    const confirmed = await confirm({
      title: `Delete the rule “${rule.name}”?`,
      description:
        'What it was counting goes with it. An alert it raised stays as it is in the alert list: nothing is left that could say the condition ended.',
      confirmLabel: 'Delete the rule',
      tone: 'danger',
    });
    if (confirmed) remove.mutate({ id: rule.id });
  };

  const columns: readonly TableColumn<LogRule>[] = [
    {
      id: 'name',
      header: 'Rule',
      required: true,
      cell: (rule) => <span title={rule.name}>{rule.name}</span>,
      sortValue: (rule) => rule.name,
    },
    {
      id: 'platform',
      // A built-in rule covers the platform it names and no other; a rule
      // with no platform is one of the user's own.
      header: 'Built in for',
      width: 200,
      cell: (rule) =>
        rule.builtin ? (
          <span title={rule.platform}>{rule.platform}</span>
        ) : (
          <Badge tone="accent" variant="outline">
            Your own
          </Badge>
        ),
      sortValue: (rule) => (rule.builtin ? rule.platform : ''),
    },
    {
      id: 'stream',
      header: 'Reads',
      width: 104,
      cell: (rule) => RULE_STREAMS.find((entry) => entry.value === rule.stream)?.label ?? rule.stream,
      sortValue: (rule) => rule.stream,
    },
    {
      id: 'raises',
      header: 'Raises',
      width: 144,
      cell: (rule) =>
        rule.emit.kind === 'alert' ? (
          <StatusPill tone={alertTone(rule.emit.severity)}>{raisesPhrase(rule.emit)}</StatusPill>
        ) : (
          raisesPhrase(rule.emit)
        ),
      sortValue: (rule) => raisesPhrase(rule.emit),
    },
    {
      id: 'when',
      header: 'When',
      width: 190,
      cell: (rule) => <span title={thresholdPhrase(rule)}>{thresholdPhrase(rule)}</span>,
    },
    {
      id: 'clears',
      header: 'Cleared by',
      width: 170,
      cell: (rule) =>
        rule.emit.kind === 'alert' ? (
          clearingPhrase(rule)
        ) : (
          <span className={styles.muted}>{clearingPhrase(rule)}</span>
        ),
    },
    {
      id: 'enabled',
      header: 'On',
      width: 80,
      align: 'center',
      required: true,
      cell: (rule) =>
        capabilities.ruleSave ? (
          <Switch
            checked={rule.enabled}
            aria-busy={(save.isPending && save.variables?.id === rule.id) || undefined}
            className={save.isPending && save.variables?.id === rule.id ? styles.busySwitch : undefined}
            aria-label={`${rule.name}: switched ${rule.enabled ? 'on' : 'off'}`}
            onChange={(event) => {
              // One save at a time: the answer of the one in flight decides what the list shows.
              if (save.isPending) return;
              toggle(rule, event.target.checked);
            }}
            onClick={(event) => event.stopPropagation()}
          />
        ) : rule.enabled ? (
          'On'
        ) : (
          <span className={styles.muted}>Off</span>
        ),
      sortValue: (rule) => (rule.enabled ? 0 : 1),
    },
  ];

  return (
    <div className={styles.paneBody}>
      <RuleMemory status={status} />

      <Toolbar className={styles.toolbar}>
        <SearchInput
          className={styles.search}
          value={text}
          onChange={setText}
          aria-label="Search the rules"
          placeholder="A name, a mnemonic, a pattern"
        />
        <Select
          className={styles.filterControlWide}
          size="sm"
          aria-label="Platform"
          value={platform}
          onChange={(event) => setPlatform(event.target.value)}
        >
          <option value="">Every platform</option>
          {platforms.map((entry) => (
            <option key={entry} value={entry}>
              {entry}
            </option>
          ))}
          <option value="own">Your own rules</option>
        </Select>
        <Select
          className={styles.filterControl}
          size="sm"
          aria-label="Reads"
          value={stream}
          onChange={(event) => setStream(event.target.value)}
        >
          <option value="">Lines and traps</option>
          {RULE_STREAMS.map((entry) => (
            <option key={entry.value} value={entry.value}>
              {entry.label}
            </option>
          ))}
        </Select>
        <ToolbarSpacer />
        <span className={styles.muted}>
          {rules.data
            ? `${plural(all.length - own, 'built-in rule')}, ${formatCount(own)} of your own`
            : null}
        </span>
        {capabilities.ruleSave && (
          <Button variant="primary" icon={Plus} onClick={() => setOpen({ rule: null })}>
            New rule
          </Button>
        )}
      </Toolbar>

      {refusal !== null && (
        <p className={styles.fieldError} role="alert">
          {refusal}
        </p>
      )}

      {rules.isError ? (
        <ErrorState title="The rules could not be read" error={rules.error} onRetry={() => void rules.refetch()} />
      ) : (
        <Table
          id="logs.rules"
          columns={columns}
          rows={shown}
          rowKey={(rule) => rule.id}
          loading={rules.isPending}
          defaultSort={{ columnId: 'platform', direction: 'asc' }}
          onRowActivate={(rule) => setOpen({ rule })}
          rowActions={(rule) => [
            { label: 'Open', onSelect: () => setOpen({ rule }) },
            ...(capabilities.ruleDelete && !rule.builtin
              ? [
                  { type: 'separator' as const },
                  { label: 'Delete…', icon: Trash, tone: 'danger' as const, onSelect: () => void destroy(rule) },
                ]
              : []),
          ]}
          maxHeight="calc(100vh - 360px)"
          empty={
            <EmptyState
              size="compact"
              icon={SlidersHorizontal}
              title={all.length === 0 ? 'No rules' : 'No rule matches this filter'}
              description={
                all.length === 0
                  ? 'The built-in rules are written when the application starts; none is stored here yet.'
                  : 'Clear the search or the filters to see every rule.'
              }
            />
          }
          aria-label="Log rules"
        />
      )}

      <p className={styles.muted}>
        The built-in rules cover Cisco IOS / IOS-XE, MikroTik RouterOS and the notifications SNMPv2-MIB
        and IF-MIB define. Other platforms are not covered. No built-in rule’s test line was recorded
        from a device: each rule says where its line came from.
      </p>

      {open !== null && (
        <RuleDrawer
          key={open.rule?.id ?? 'new'}
          rule={open.rule}
          rules={all}
          capabilities={capabilities}
          onClose={() => setOpen(null)}
        />
      )}
    </div>
  );
}

/** What the rules hold in memory, from `logs.status`. Only what the engine gives. */
function RuleMemory({ status }: { status: LogsStatus | undefined }) {
  const rules = status?.rules;
  if (!rules) return null;
  const counters = [
    counter(rules, 'tracked', 'Keys being counted', {
      hint: 'One per rule, device and instance the rules are counting or throttling right now.',
    }),
    counter(rules, 'open_alerts', 'Alerts waiting to clear', {
      hint: 'Alerts a rule will clear by its opposite event or its quiet period, as far as this process knows.',
    }),
    counter(rules, 'forgotten', 'Keys forgotten', {
      attention: true,
      hint: 'A rule reached its ceiling of keys and let the oldest go. A forgotten key starts counting again from nothing.',
    }),
    counter(rules, 'held_back', 'Matches held back', {
      attention: true,
      hint: 'One device spoke for more than 256 instances of a rule inside one window. Each speaks on its next matching line after the window.',
    }),
    counter(rules, 'undelivered', 'Calls to the monitor dropped', {
      attention: true,
      hint: 'More than 4,096 raises and clears were waiting for the monitor. The signals behind them were stored and published all the same.',
    }),
  ].filter((entry): entry is Counter => entry !== null);
  if (counters.length === 0) return null;
  const ceiling = counter(rules, 'max_per_rule', '');
  return (
    <section aria-label="What the rules hold in memory" className={styles.memory}>
      <dl className={styles.counters}>
        {counters.map((entry) => (
          <div
            key={entry.id}
            className={styles.counter}
            data-counter={entry.id}
            data-attention={(entry.attention && entry.value > 0) || undefined}
            title={entry.hint}
          >
            <dt>{entry.label}</dt>
            <dd>{formatCount(entry.value)}</dd>
          </div>
        ))}
      </dl>
      <p className={styles.muted}>
        Counts and throttles are kept in memory: they start again when Nocturne restarts.
        {ceiling ? ` A rule tracks at most ${formatCount(ceiling.value)} keys.` : ''}
      </p>
    </section>
  );
}
