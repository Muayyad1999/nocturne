/**
 * One alert, in full: what fired, what it measured, and what can be done
 * about it — next to the series it was measured on, around the time it fired.
 *
 * The chart is only drawn when the rule is actually about a measurement. A
 * `config_changed` rule has no series and a rule whose metric is a glob
 * stands for several, so the alert's own instance decides; where neither
 * gives one series, the drawer says so instead of drawing something plausible.
 */

import { Check, CircleCheck, Clock, ChevronDown } from 'lucide-react';
import { useState } from 'react';
import { Link } from 'react-router';
import { useRpc } from '@/api/hooks';
import type { Alert, AlertRule } from '@/api/types/monitor';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { Drawer } from '@/design/Drawer';
import { DropdownMenu } from '@/design/DropdownMenu';
import { Field } from '@/design/Field';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Textarea } from '@/design/Textarea';
import { formatCount } from '@/lib/format';
import { formatDateTime } from '@/lib/time';
import { AlertStateCell, useAlertActions } from './Alerts';
import { MetricChart } from './MetricChart';
import styles from './Monitoring.module.css';
import { stepFor } from './series';
import { conditionMetric, conditionText, metricFormat, metricLabel, severityLabel, severityTone } from './vocab';

const BEFORE_MS = 15 * 60_000;
const AFTER_MS = 15 * 60_000;

const SNOOZE_CHOICES: readonly { label: string; minutes: number }[] = [
  { label: 'For 1 hour', minutes: 60 },
  { label: 'For 4 hours', minutes: 240 },
  { label: 'For a day', minutes: 1_440 },
];

export interface AlertDrawerProps {
  alert: Alert;
  onClose: () => void;
}

export function AlertDrawer({ alert, onClose }: AlertDrawerProps) {
  const [note, setNote] = useState('');
  const actions = useAlertActions();
  const rules = useRpc<AlertRule[]>('alerts.rules.list', undefined, {
    invalidateOn: ['monitor.status_changed'],
  });
  const rule = rules.data?.find((entry) => entry.id === alert.rule_id);

  const instance = alert.instance ?? '';
  const chartMetric = rule ? conditionMetric(rule.condition) : null;
  const globbed =
    rule?.condition.type === 'threshold' || rule?.condition.type === 'no_data'
      ? /[*?]/.test(rule.condition.metric)
      : false;

  const since = alert.started_at - BEFORE_MS;
  const until = (alert.resolved_at ?? Date.now()) + AFTER_MS;

  const facts: KeyValueItem[] = [
    { label: 'Severity', value: <StatusPill tone={severityTone(alert.severity)}>{severityLabel(alert.severity)}</StatusPill> },
    { label: 'State', value: <AlertStateCell alert={alert} /> },
    {
      label: 'Device',
      value: alert.device_id ? (
        <span className={styles.cell}>
          <Link to={`/monitoring/health?device=${encodeURIComponent(alert.device_id)}`}>{alert.device}</Link>
          {alert.lab && <LabPill />}
        </span>
      ) : (
        'Not about one device'
      ),
    },
    ...(alert.instance ? [{ label: 'Where', value: alert.instance, mono: true }] : []),
    { label: 'Rule', value: alert.rule_name },
    ...(rule ? [{ label: 'Fires when', value: conditionText(rule.condition) }] : []),
    {
      label: 'Measured',
      value:
        alert.value == null
          ? alert.rule_id
            ? 'Nothing was measured: this rule is about a state, not a number.'
            : 'Whatever raised this recorded no number with it.'
          : metricFormat(chartMetric ?? '')(alert.value),
    },
    { label: 'Raised', value: formatDateTime(alert.started_at) },
    { label: 'Last seen', value: formatDateTime(alert.last_seen_at) },
    { label: 'Seen', value: alert.count === 1 ? 'Once' : `${formatCount(alert.count)} times` },
    ...(alert.acked_at
      ? [{ label: 'Acknowledged', value: `${formatDateTime(alert.acked_at)}${alert.ack_note ? ` — ${alert.ack_note}` : ''}` }]
      : []),
    ...(alert.snoozed_until
      ? [{ label: 'Snoozed until', value: formatDateTime(alert.snoozed_until) }]
      : []),
    ...(alert.resolved_at ? [{ label: 'Resolved', value: formatDateTime(alert.resolved_at) }] : []),
  ];

  const openStill = alert.state !== 'resolved';

  return (
    <Drawer
      open
      size="lg"
      onClose={onClose}
      title={alert.title}
      description={
        <span className={styles.cell}>
          <Badge tone={severityTone(alert.severity)}>{severityLabel(alert.severity)}</Badge>
          {alert.rule_name}
        </span>
      }
      footer={
        openStill ? (
          <>
            <DropdownMenu
              aria-label="Snooze this alert"
              placement="top-start"
              items={
                alert.snoozed_until
                  ? [{ label: 'Cancel the snooze', onSelect: () => actions.snooze(alert, 0) }]
                  : SNOOZE_CHOICES.map((choice) => ({
                      label: choice.label,
                      onSelect: () => actions.snooze(alert, choice.minutes),
                    }))
              }
              trigger={
                <Button icon={Clock} trailingIcon={ChevronDown}>
                  Snooze
                </Button>
              }
            />
            {alert.state !== 'acknowledged' && (
              <Button
                icon={Check}
                loading={actions.pending}
                onClick={() => actions.acknowledge(alert, note.trim() || undefined)}
              >
                Acknowledge
              </Button>
            )}
            <Button
              variant="primary"
              icon={CircleCheck}
              loading={actions.pending}
              onClick={() => void actions.resolve(alert, note.trim() || undefined)}
            >
              Resolve…
            </Button>
          </>
        ) : undefined
      }
    >
      <div className={styles.facts}>
        <p className={styles.hint}>{alert.detail}</p>

        <KeyValue items={facts} />

        {chartMetric ? (
          <MetricChart
            title={`${metricLabel(chartMetric)} around the alert`}
            subtitle={`${alert.device ?? 'the device'}${instance ? ` · ${instance}` : ''}, from a quarter of an hour before it fired`}
            lines={[{ device_id: alert.device_id ?? '', metric: chartMetric, instance }]}
            since={since}
            until={until}
            step_s={stepFor(until - since)}
            height={180}
          />
        ) : (
          <p className={styles.hint}>
            {!alert.rule_id
              ? `This was raised by ${alert.rule_name}, not by a monitoring rule, so there is no series behind it.`
              : !rule
                ? 'The rule behind this alert has been deleted, so there is no way to know what it watched.'
                : globbed
                  ? `This rule watches every metric matching ${rule.condition.type === 'threshold' || rule.condition.type === 'no_data' ? rule.condition.metric : ''}, and the alert records where it fired but not which of them: there is no one series to draw.`
                  : 'This rule is about a state rather than a measurement, so there is no series to draw.'}
          </p>
        )}

        {openStill && (
          <Field label="Note" hint="Kept with the alert when you acknowledge or resolve it.">
            <Textarea
              rows={2}
              aria-label="Note"
              placeholder="What you found, or who is on it"
              value={note}
              onChange={(event) => setNote(event.target.value)}
            />
          </Field>
        )}
      </div>
    </Drawer>
  );
}
