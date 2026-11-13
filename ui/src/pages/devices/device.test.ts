import { describe, expect, it } from 'vitest';
import type { DeviceRow, Job } from '@/api/types/core';
import {
  deviceStatus,
  EMPTY_FILTER,
  effectiveProfile,
  filterDevices,
  matchesQuery,
  searchText,
} from './device';
import { describeOutcome } from './useCollect';

function device(over: Partial<DeviceRow> = {}): DeviceRow {
  return {
    id: over.name ?? 'id',
    name: 'sw1',
    address: '10.0.0.1',
    profile: '',
    transport: 'ssh',
    site: '',
    role: '',
    tags: [],
    notes: '',
    monitor: { enabled: true, icmp: true, snmp: false, interval_s: 60 },
    protected: false,
    created_at: 0,
    updated_at: 0,
    state: { device_id: 'id' },
    ...over,
  };
}

describe('a device’s status', () => {
  it('is unknown until something has checked, never down', () => {
    expect(deviceStatus(device()).value).toBe('unknown');
    expect(deviceStatus(device()).tone).toBe('idle');
    expect(deviceStatus(device({ state: { device_id: 'id', reachable: true } })).value).toBe('up');
    expect(deviceStatus(device({ state: { device_id: 'id', reachable: false } })).value).toBe('down');
  });
});

describe('the device type a device is treated as', () => {
  it('prefers what was configured, falls back to what was detected', () => {
    expect(effectiveProfile(device({ profile: 'cisco_ios' }))).toEqual({
      id: 'cisco_ios',
      detected: false,
    });
    expect(
      effectiveProfile(device({ state: { device_id: 'id', detected_profile: 'arista_eos' } })),
    ).toEqual({ id: 'arista_eos', detected: true });
    expect(effectiveProfile(device())).toBeNull();
  });
});

describe('searching the list', () => {
  it('needs every word, anywhere in the device, as the backend does', () => {
    const haystack = searchText(
      device({ name: 'CORE-SW1', site: 'HQ', state: { device_id: 'id', facts: {
        hostname: 'core-sw1', vendor: 'Cisco', model: 'C9300', os: 'IOS-XE',
        os_version: '17.9.4', serial: 'FOC1', uptime_s: 1, extra: {},
      } } }),
    );
    expect(matchesQuery(haystack, 'core hq')).toBe(true);
    expect(matchesQuery(haystack, 'c9300')).toBe(true);
    expect(matchesQuery(haystack, 'core branch')).toBe(false);
    expect(matchesQuery(haystack, '')).toBe(true);
  });

  it('applies the lab switch, the facets and the text together', () => {
    const devices = [
      device({ name: 'hq-core', site: 'HQ', role: 'core', tags: ['critical'] }),
      device({ name: 'hq-access', site: 'HQ', role: 'access' }),
      device({ name: 'lab-r1', lab: { lab_id: 'lab', node: 'R1' } }),
    ];
    expect(filterDevices(devices, EMPTY_FILTER)).toHaveLength(3);
    expect(filterDevices(devices, { ...EMPTY_FILTER, lab: 'hide' })).toHaveLength(2);
    expect(filterDevices(devices, { ...EMPTY_FILTER, lab: 'only' })).toHaveLength(1);
    expect(filterDevices(devices, { ...EMPTY_FILTER, site: 'hq' })).toHaveLength(2);
    expect(filterDevices(devices, { ...EMPTY_FILTER, tag: 'CRITICAL' })).toHaveLength(1);
    expect(
      filterDevices(devices, { ...EMPTY_FILTER, site: 'HQ', query: 'access' }),
    ).toHaveLength(1);
    expect(filterDevices(devices, { ...EMPTY_FILTER, status: 'up' })).toHaveLength(0);
  });
});

function job(result: unknown, over: Partial<Job> = {}): Job {
  return {
    id: 'j1',
    kind: 'collect.run',
    title: 'Collect',
    state: 'succeeded',
    origin: 'user',
    device_ids: [],
    done: 1,
    total: 1,
    message: '',
    started_at: 0,
    log: [],
    log_dropped: 0,
    result,
    ...over,
  };
}

describe('what a finished collection says', () => {
  it('names the device when there was one', () => {
    const outcome = describeOutcome(
      job({
        failed: 0,
        devices: [{ device_id: 'a', name: 'core-sw1', collected: ['facts'], problems: [] }],
      }),
    );
    expect(outcome).toEqual({ title: 'Collected from core-sw1', detail: undefined, failed: false });
  });

  it('separates a kind the device type cannot read from one that failed', () => {
    const outcome = describeOutcome(
      job({
        failed: 0,
        devices: [
          {
            device_id: 'a',
            name: 'core-sw1',
            collected: ['facts'],
            problems: [
              { kind: 'routes', code: 'unsupported', message: 'no steps' },
              { kind: 'arp', code: 'failed', message: 'the command was rejected' },
            ],
          },
        ],
      }),
    );
    expect(outcome.failed).toBe(false);
    expect(outcome.detail).toBe('ARP table could not be read');
    expect(outcome.detail, 'an unsupported kind is not reported as a failure').not.toContain(
      'Routes',
    );
  });

  it('says so when nothing could be reached', () => {
    const outcome = describeOutcome(
      job({
        failed: 1,
        devices: [
          {
            device_id: 'a',
            name: 'core-sw1',
            collected: [],
            problems: [],
            error: 'core-sw1: connection refused',
          },
        ],
      }),
    );
    expect(outcome.failed).toBe(true);
    expect(outcome.title).toBe('Could not collect from core-sw1');
    expect(outcome.detail).toContain('connection refused');
  });
});
