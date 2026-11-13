import { describe, expect, test } from 'vitest';
import type { Condition, Scope } from '@/api/types/monitor';
import {
  conditionMetric,
  conditionText,
  deviceStatus,
  metricFormat,
  metricLabel,
  scopeText,
  seriesLabel,
  worstSeverity,
} from './vocab';

const scope: Scope = { sites: [], roles: [], tags: [], device_ids: [], include_lab: false };

describe('metrics in words', () => {
  test('each metric is formatted in its own unit', () => {
    expect(metricFormat('if.in_bps')(940_000_000)).toBe('940 Mb/s');
    expect(metricFormat('cpu.pct')(42)).toBe('42%');
    expect(metricFormat('icmp.rtt_ms')(1.5)).toBe('1.50 ms');
    expect(metricFormat('if.in_err_ps')(12.25)).toBe('12.3/s');
    // A tenth is too coarse for an axis that spans a tenth.
    expect(metricFormat('if.in_err_ps')(0.04)).toBe('0.04/s');
    expect(metricFormat('if.in_err_ps')(0)).toBe('0/s');
    // An up/down axis labels only the two values it can have.
    expect(metricFormat('up')(1)).toBe('Up');
    expect(metricFormat('up')(0.25)).toBe('');
    expect(metricFormat('uptime_s')(93_600)).toBe('1d 2h');
    expect(metricFormat('up')(0)).toBe('Down');
  });

  test('a value that was never measured is a dash, never a zero', () => {
    for (const metric of ['if.in_bps', 'cpu.pct', 'icmp.rtt_ms', 'up', 'uptime_s']) {
      expect(metricFormat(metric)(null)).toBe('—');
      expect(metricFormat(metric)(undefined)).toBe('—');
    }
  });

  test('a metric nobody has a name for keeps the backend name', () => {
    expect(metricLabel('fan.rpm')).toBe('fan.rpm');
    expect(seriesLabel('if.util_pct', 'Gi1/0/1')).toBe('Utilisation · Gi1/0/1');
    expect(seriesLabel('cpu.pct', '')).toBe('CPU');
  });
});

describe('rule conditions in words', () => {
  test('a threshold says what it watches, which way and where it clears', () => {
    const condition: Condition = {
      type: 'threshold',
      metric: 'cpu.pct',
      instance: null,
      op: 'gt',
      value: 90,
      clear_value: 80,
    };
    expect(conditionText(condition)).toBe('CPU goes above 90%, clearing at 80%.');
  });

  test('every condition the backend can send has a sentence', () => {
    const conditions: Condition[] = [
      { type: 'device_down' },
      { type: 'interface_down', only: 'uplinks' },
      { type: 'threshold', metric: 'if.*_err_ps', instance: 'Gi0/*', op: 'gt', value: 10, clear_value: null },
      { type: 'no_data', metric: 'cpu.pct', for_s: 600 },
      { type: 'config_changed' },
      { type: 'rtt_baseline', multiple: 5, window_s: 86_400 },
    ];
    for (const condition of conditions) {
      expect(conditionText(condition)).toMatch(/\.$/);
      expect(conditionText(condition).length).toBeGreaterThan(10);
    }
  });

  test('a condition that is not a measurement gets no chart', () => {
    expect(conditionMetric({ type: 'config_changed' })).toBeNull();
    // A glob stands for several series: the alert's instance decides, not the rule.
    expect(
      conditionMetric({
        type: 'threshold',
        metric: 'if.*_err_ps',
        instance: null,
        op: 'gt',
        value: 10,
        clear_value: null,
      }),
    ).toBeNull();
    expect(conditionMetric({ type: 'device_down' })).toBe('up');
  });
});

describe('what a device status means', () => {
  test('never polled, not monitored, up and down are four different states', () => {
    const monitored = { monitor: { enabled: true } };
    expect(deviceStatus({ ...monitored, state: { reachable: true, last_checked: 1 } })).toBe('up');
    expect(deviceStatus({ ...monitored, state: { reachable: false, last_checked: 1 } })).toBe('down');
    expect(deviceStatus({ ...monitored, state: { reachable: null, last_checked: null } })).toBe('unknown');
    expect(deviceStatus({ monitor: { enabled: false }, state: { reachable: true, last_checked: 1 } })).toBe(
      'unmonitored',
    );
  });
});

describe('severity and scope', () => {
  test('the worst of several severities wins', () => {
    expect(worstSeverity(['low', 'critical', 'medium'])).toBe('critical');
    expect(worstSeverity(['info', 'low'])).toBe('low');
    expect(worstSeverity([])).toBeNull();
  });

  test('an empty scope is every device, and the lab is called out', () => {
    expect(scopeText(scope)).toBe('Every device');
    expect(scopeText({ ...scope, include_lab: true })).toBe('Every device, lab included');
    expect(scopeText({ ...scope, sites: ['HQ'], roles: ['core'] })).toBe('Sites HQ, roles core');
  });
});
