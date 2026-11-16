/**
 * The words and units the monitoring screen uses, in one place.
 *
 * Monitoring speaks in metric names (`if.in_bps`), condition tags
 * (`rtt_baseline`) and severities. None of those belong on screen as they
 * are, and each needs the right formatter: a rate is not a percentage and a
 * missing value is not a zero (`lib/format` renders that as a dash).
 */

import type { Condition, Severity } from '@/api/types/monitor';
import type { StatusTone, Tone } from '@/design/tone';
import {
  formatCount,
  formatDuration,
  formatNumber,
  formatPercent,
  formatRate,
  type Numeric,
} from '@/lib/format';

// ------------------------------------------------------------------ metrics

interface MetricInfo {
  label: string;
  /** How a value of this metric reads. */
  format: (value: Numeric) => string;
  /** A fixed upper bound for a chart's y-axis, where the metric has one. */
  max?: number;
  /** What the instance of this metric is, for a column heading. */
  instance?: string;
}

const MILLISECONDS = (value: Numeric) =>
  value == null || !Number.isFinite(value) ? formatNumber(value) : `${formatNumber(value, { digits: 2 })} ms`;

/**
 * A per-second count. Errors are usually none at all and occasionally a
 * trickle, so a tenth is too coarse for an axis that spans a tenth: under one
 * a second decimal is kept, and a clean zero stays a clean zero.
 */
const PER_SECOND = (value: Numeric) => {
  if (value == null || !Number.isFinite(value)) return formatNumber(value);
  if (value === 0) return '0/s';
  const digits = Math.abs(value) < 1 ? 2 : 1;
  return `${formatNumber(value, { digits })}/s`;
};

/**
 * A state, not a number: only 1 and 0 have a name. An axis tick in between
 * (uPlot splits 0…1 into quarters) is left blank rather than labelled "Down"
 * four times over.
 */
const UP_DOWN = (value: Numeric) => {
  if (value == null || !Number.isFinite(value)) return formatNumber(value);
  if (value === 1) return 'Up';
  if (value === 0) return 'Down';
  return '';
};

const METRICS: Record<string, MetricInfo> = {
  'icmp.rtt_ms': { label: 'Round-trip time', format: MILLISECONDS },
  'icmp.loss_pct': { label: 'Packet loss', format: (value) => formatPercent(value), max: 100 },
  up: { label: 'Reachable', format: UP_DOWN, max: 1 },
  uptime_s: {
    label: 'Uptime',
    format: (value) => (value == null ? formatDuration(value) : formatDuration(value * 1000)),
  },
  'cpu.pct': { label: 'CPU', format: (value) => formatPercent(value), max: 100 },
  'mem.pct': { label: 'Memory', format: (value) => formatPercent(value), max: 100 },
  'temp.c': {
    label: 'Temperature',
    format: (value) =>
      value == null || !Number.isFinite(value) ? formatNumber(value) : `${formatNumber(value, { digits: 1 })} °C`,
    instance: 'Sensor',
  },
  'if.in_bps': { label: 'Traffic in', format: formatRate, instance: 'Interface' },
  'if.out_bps': { label: 'Traffic out', format: formatRate, instance: 'Interface' },
  'if.util_pct': { label: 'Utilisation', format: (value) => formatPercent(value), max: 100, instance: 'Interface' },
  'if.in_err_ps': { label: 'Errors in', format: PER_SECOND, instance: 'Interface' },
  'if.out_err_ps': { label: 'Errors out', format: PER_SECOND, instance: 'Interface' },
  'if.in_disc_ps': { label: 'Discards in', format: PER_SECOND, instance: 'Interface' },
  'if.out_disc_ps': { label: 'Discards out', format: PER_SECOND, instance: 'Interface' },
  'if.oper': { label: 'Line protocol', format: UP_DOWN, max: 1, instance: 'Interface' },
};

/** What a metric is called on screen. An unknown metric keeps its own name. */
export function metricLabel(metric: string): string {
  return METRICS[metric]?.label ?? metric;
}

/** How a value of this metric is written, missing values included. */
export function metricFormat(metric: string): (value: Numeric) => string {
  return METRICS[metric]?.format ?? formatCount;
}

/** A fixed top for the y-axis where the metric has one (a percentage, a state). */
export function metricMax(metric: string): number | undefined {
  return METRICS[metric]?.max;
}

/** What this metric's instance is, when it has one. */
export function instanceLabel(metric: string): string {
  return METRICS[metric]?.instance ?? 'Instance';
}

/** A metric and its instance as one name: "Traffic in · Gi1/0/1". */
export function seriesLabel(metric: string, instance: string): string {
  return instance ? `${metricLabel(metric)} · ${instance}` : metricLabel(metric);
}

// --------------------------------------------------------------- severities

export const SEVERITIES: readonly Severity[] = ['critical', 'high', 'medium', 'low', 'info'];

const SEVERITY_LABEL: Record<Severity, string> = {
  critical: 'Critical',
  high: 'High',
  medium: 'Medium',
  low: 'Low',
  info: 'Info',
};

/**
 * Colour carries the level, the word carries the meaning: the two most
 * serious levels are told apart by their label, never by hue alone.
 */
const SEVERITY_TONE: Record<Severity, StatusTone> = {
  critical: 'crit',
  high: 'crit',
  medium: 'warn',
  low: 'info',
  info: 'info',
};

export function severityLabel(severity: Severity): string {
  return SEVERITY_LABEL[severity] ?? severity;
}

export function severityTone(severity: Severity): StatusTone {
  return SEVERITY_TONE[severity] ?? 'info';
}

/** Highest first, as the lists are ordered. */
export function severityRank(severity: Severity): number {
  const rank = SEVERITIES.indexOf(severity);
  return rank === -1 ? SEVERITIES.length : rank;
}

/**
 * The worst severity in `alerts.summary`'s own count, or `null` when nothing
 * is open. Taken from the backend's answer rather than from counting rows, so
 * the tile and the list can never disagree.
 */
export function worstOpenSeverity(bySeverity: Record<string, number> | undefined): Severity | null {
  const open = SEVERITIES.filter((severity) => (bySeverity?.[severity] ?? 0) > 0);
  return worstSeverity(open);
}

/** The worst of the severities given, or `null` when there are none. */
export function worstSeverity(severities: readonly Severity[]): Severity | null {
  let worst: Severity | null = null;
  for (const severity of severities) {
    if (worst === null || severityRank(severity) < severityRank(worst)) worst = severity;
  }
  return worst;
}

// --------------------------------------------------------------- conditions

export const CONDITION_KINDS = [
  'device_down',
  'interface_down',
  'threshold',
  'no_data',
  'config_changed',
  'rtt_baseline',
] as const;

export type ConditionKind = (typeof CONDITION_KINDS)[number];

const CONDITION_NAME: Record<ConditionKind, string> = {
  device_down: 'Device down',
  interface_down: 'Interface down',
  threshold: 'Metric threshold',
  no_data: 'No data',
  config_changed: 'Configuration changed',
  rtt_baseline: 'Round-trip time above its baseline',
};

export function conditionName(kind: ConditionKind): string {
  return CONDITION_NAME[kind];
}

const SELECTOR_TEXT: Record<string, string> = {
  uplinks: 'an interface with a neighbour',
  described: 'a described interface',
  all: 'any interface',
};

/** A sentence that says what the rule watches for, with its numbers in it. */
export function conditionText(condition: Condition): string {
  switch (condition.type) {
    case 'device_down':
      return 'The device stops answering.';
    case 'interface_down': {
      const which = SELECTOR_TEXT[condition.only] ?? 'any interface';
      return `${which[0]?.toUpperCase()}${which.slice(1)} is administratively up while its line protocol is down.`;
    }
    case 'threshold': {
      const format = metricFormat(condition.metric);
      const where = condition.instance ? ` on ${condition.instance}` : '';
      const clear = condition.clear_value == null ? '' : `, clearing at ${format(condition.clear_value)}`;
      return `${metricLabel(condition.metric)}${where} goes ${condition.op === 'gt' ? 'above' : 'below'} ${format(condition.value)}${clear}.`;
    }
    case 'no_data':
      return `Nothing has been recorded for ${metricLabel(condition.metric)} in ${formatDuration(condition.for_s * 1000)}.`;
    case 'config_changed':
      return 'The configuration of a device in scope changed.';
    case 'rtt_baseline':
      return `Round-trip time is more than ${formatCount(condition.multiple)}× its median over the last ${formatDuration(condition.window_s * 1000)}.`;
    case 'check_state': {
      const which =
        condition.check && condition.check !== '*'
          ? `The service check “${condition.check}”`
          : 'A service check';
      const kinds = condition.kinds.length ? ` (${condition.kinds.join(', ')})` : '';
      // `cannot_run` is worded as what it is — monitoring that cannot tell —
      // because calling it a failure is exactly what the backend refuses to
      // do.
      const what =
        condition.state === 'cannot_run'
          ? 'cannot run, so nothing is known about what it watches'
          : condition.state === 'warning'
            ? 'answers with a warning, such as a certificate about to expire'
            : 'finds the service it watches not answering';
      return `${which}${kinds} ${what}.`;
    }
  }
}

/**
 * The series a rule's alert should be charted against. `null` where the
 * condition is not about a measurement (nothing is drawn rather than
 * something plausible), and `null` too for a metric glob, which stands for
 * several series at once: the alert's own instance says which one it was.
 */
export function conditionMetric(condition: Condition): string | null {
  switch (condition.type) {
    case 'device_down':
      return 'up';
    case 'interface_down':
      return 'if.oper';
    case 'threshold':
    case 'no_data':
      return /[*?]/.test(condition.metric) ? null : condition.metric;
    case 'rtt_baseline':
      return 'icmp.rtt_ms';
    case 'check_state':
      // A check alert's instance is the check's *name*, while its series is
      // keyed by the check's id, so the chart cannot be resolved from the
      // condition alone: nothing is drawn rather than the wrong series.
      return null;
    case 'config_changed':
      return null;
  }
}

// ------------------------------------------------------------------- scopes

/** "Every device" or the facets a scope names, as one short phrase. */
export function scopeText(scope: {
  sites: string[];
  roles: string[];
  tags: string[];
  device_ids: string[];
  include_lab: boolean;
}): string {
  const parts: string[] = [];
  if (scope.sites.length) parts.push(`sites ${scope.sites.join(', ')}`);
  if (scope.roles.length) parts.push(`roles ${scope.roles.join(', ')}`);
  if (scope.tags.length) parts.push(`tags ${scope.tags.join(', ')}`);
  if (scope.device_ids.length) parts.push(`${formatCount(scope.device_ids.length)} named devices`);
  const where = parts.length === 0 ? 'Every device' : parts.join(', ').replace(/^./, (first) => first.toUpperCase());
  return scope.include_lab ? `${where}, lab included` : where;
}

// ----------------------------------------------------------- device status

export type DeviceStatus = 'up' | 'down' | 'unknown' | 'unmonitored';

interface StatusInfo {
  label: string;
  tone: StatusTone;
  /** Why the row reads the way it does, for the cell's tooltip. */
  help: string;
}

export const DEVICE_STATUS: Record<DeviceStatus, StatusInfo> = {
  up: { label: 'Up', tone: 'ok', help: 'The last poll was answered.' },
  down: { label: 'Down', tone: 'crit', help: 'Several polls in a row went unanswered.' },
  unknown: {
    label: 'Not polled',
    tone: 'idle',
    help: 'Monitoring is on for this device but it has not been polled yet.',
  },
  unmonitored: {
    label: 'Not monitored',
    tone: 'idle',
    help: 'Monitoring is switched off for this device, so nothing is known about it.',
  },
};

/**
 * Four different things, and they must not look alike: polled and answering,
 * polled and silent, monitored but not yet polled, and not monitored at all.
 */
export function deviceStatus(device: {
  monitor: { enabled: boolean };
  state: { reachable?: boolean | null; last_checked?: number | null };
}): DeviceStatus {
  if (!device.monitor.enabled) return 'unmonitored';
  if (device.state.last_checked == null || device.state.reachable == null) return 'unknown';
  return device.state.reachable ? 'up' : 'down';
}

// ------------------------------------------------------------------- other

/** The tone of a count badge: quiet at zero, loud when it is a problem. */
export function countTone(count: number, tone: Tone): Tone {
  return count > 0 ? tone : 'neutral';
}
