import { describe, expect, it } from 'vitest';
import type {
  Coverage,
  DeviceCoverage,
  Mismatch,
  Reconciliation,
  Unobserved,
} from '@/api/types/ipam';
import {
  coverageReading,
  foundSomething,
  groupMismatches,
  sections,
  silentDevices,
  truncatedNote,
  unobservedReading,
} from './reconciliation';

function device(over: Partial<DeviceCoverage> = {}): DeviceCoverage {
  return {
    device_id: 'd1',
    device: 'Campus/DS1',
    site: 'HQ',
    lab: false,
    interfaces_at: 5_000,
    arp_at: 5_000,
    addresses_configured: 6,
    arp_entries: 13,
    reason: null,
    ...over,
  };
}

function coverage(over: Partial<Coverage> = {}): Coverage {
  return {
    devices_total: 3,
    devices_with_addresses: 3,
    devices_never_collected: 0,
    oldest_collection: 4_000,
    newest_collection: 5_000,
    devices: [device(), device({ device_id: 'd2' }), device({ device_id: 'd3' })],
    ...over,
  };
}

function report(over: Partial<Reconciliation> = {}): Reconciliation {
  return {
    at: 6_000,
    coverage: coverage(),
    undocumented: [],
    unobserved: [],
    mismatches: [],
    mask_mismatches: [],
    prefixes_not_documented: [],
    prefixes_without_evidence: [],
    conflicts: [],
    truncated: [],
    refused: [],
    ...over,
  };
}

function unobserved(over: Partial<Unobserved> = {}): Unobserved {
  return {
    ip: '10.20.1.50',
    scope: '',
    prefix_id: 'p1',
    prefix: '10.20.1.0/24',
    prefix_documented: true,
    hostname: 'printer-1',
    status: 'active',
    documented_at: 1_000,
    last_seen: null,
    had_coverage: false,
    summary: 'in the plan',
    ...over,
  } as Unobserved;
}

describe('coverage comes first', () => {
  const now = 10_000;

  it('an empty inventory is not full coverage of nothing', () => {
    const reading = coverageReading(
      coverage({
        devices_total: 0,
        devices_with_addresses: 0,
        devices: [],
        oldest_collection: null,
        newest_collection: null,
      }),
      now,
    );
    expect(reading.kind).toBe('none');
    expect(reading.headline).toBe('Nothing has been collected');
    expect(reading.detail).toContain('nothing below is evidence of anything');
  });

  it('devices that exist but were never read is coverage of none, not of all', () => {
    const reading = coverageReading(
      coverage({ devices_total: 4, devices_with_addresses: 0, devices_never_collected: 4 }),
      now,
    );
    expect(reading.kind).toBe('none');
    expect(reading.tone).toBe('warn');
    expect(reading.headline).toContain('No addresses read from 4 devices');
  });

  it('says how many were never collected from, and that the plan is unchecked there', () => {
    const reading = coverageReading(
      coverage({ devices_total: 5, devices_with_addresses: 3, devices_never_collected: 2 }),
      now,
    );
    expect(reading.kind).toBe('partial');
    expect(reading.headline).toContain('3 of 5 devices reported addresses');
    expect(reading.detail).toContain('2 devices have never been collected from');
    expect(reading.detail).toContain('unchecked, not confirmed');
  });

  it('only full coverage lets "nothing answered" mean anything', () => {
    const reading = coverageReading(coverage(), now);
    expect(reading.kind).toBe('full');
    expect(reading.detail).toContain('really means nothing answered');
    expect(reading.headline).toContain('most recently 5s ago');
  });

  it('a device read with no address, and one never read, are both silent', () => {
    const rows = silentDevices(
      coverage({
        devices: [
          device(),
          device({ device_id: 'quiet', addresses_configured: 0 }),
          device({ device_id: 'never', interfaces_at: null, reason: 'never collected' }),
        ],
      }),
    );
    expect(rows.map((row) => row.device_id)).toEqual(['quiet', 'never']);
  });
});

describe('"nobody looked" and "it is not there" are different sentences', () => {
  const now = 1_000_000;

  it('says nobody looked when nothing that could see it was collected', () => {
    const reading = unobservedReading(unobserved({ had_coverage: false }), now);
    expect(reading.headline).toBe('Nobody looked');
    expect(reading.tone).toBe('idle');
    expect(reading.detail).toContain('no evidence either way');
  });

  it('says it is not there when devices were read and none showed it', () => {
    const reading = unobservedReading(unobserved({ had_coverage: true }), now);
    expect(reading.headline).toBe('Not on the network');
    expect(reading.detail).toContain('none of them show it');
    expect(reading.detail).toContain('never been seen');
  });

  it('names when it was last seen, when it ever was', () => {
    const reading = unobservedReading(
      unobserved({ had_coverage: true, last_seen: now - 86_400_000 }),
      now,
    );
    expect(reading.detail).toContain('last seen 1d ago');
  });
});

describe('the sections of a report', () => {
  it('lists every section with its count, including the empty ones', () => {
    const list = sections(report());
    expect(list.map((section) => section.id)).toEqual([
      'undocumented',
      'unobserved',
      'mismatches',
      'mask_mismatches',
      'prefixes_not_documented',
      'prefixes_without_evidence',
      'conflicts',
    ]);
    expect(list.every((section) => section.count === 0)).toBe(true);
    // An empty section says what finding nothing means.
    expect(list[0]?.empty).toBe('Every address the network showed has a record.');
    expect(foundSomething(report())).toBe(false);
  });

  it('marks a capped list so its count reads as a floor', () => {
    const capped = report({
      undocumented: [],
      truncated: ['undocumented'],
    });
    expect(sections(capped)[0]?.truncated).toBe(true);
    expect(truncatedNote(capped)).toContain('a floor and not a total');
    expect(truncatedNote(report())).toBeNull();
  });

  it('knows when there is something to show', () => {
    expect(
      foundSomething(report({ unobserved: [unobserved()] })),
    ).toBe(true);
  });
});

describe('the mismatches of a report', () => {
  const now = 3_000_000;

  function mismatch(over: Partial<Mismatch> = {}): Mismatch {
    return {
      ip: '10.10.10.2',
      scope: 'lab:x',
      field: 'mac',
      documented: 'aa:bb:cc:dd:ee:ff',
      observed: '02:4e:43:d0:fd:40',
      device_id: 'd-ds1',
      device: 'Campus/DS1',
      interface: 'Vlan10',
      collected_at: now - 60_000,
      ...over,
    };
  }

  it('is one contradiction per address and field, with every device under it', () => {
    const grouped = groupMismatches(
      [
        mismatch(),
        mismatch({ device_id: 'd-ds2', device: 'Campus/DS2' }),
        mismatch({ device_id: 'd-ds1', device: 'Campus/DS1' }),
      ],
      () => undefined,
      now,
    );
    expect(grouped).toHaveLength(1);
    expect(grouped[0]?.evidence).toEqual([
      'Campus/DS1 (Vlan10) · read 1m ago',
      'Campus/DS2 (Vlan10) · read 1m ago',
    ]);
  });

  it('names the devices on both sides of a device mismatch, and keeps an id it cannot name', () => {
    const names = new Map([['d-as1', 'Campus/AS1']]);
    const [named] = groupMismatches(
      [mismatch({ field: 'device', documented: 'd-as1', observed: 'd-ds1' })],
      (id) => names.get(id),
      now,
    );
    expect(named?.documented).toBe('Campus/AS1');
    expect(named?.observed).toBe('d-ds1');
  });

  it('leaves a MAC mismatch exactly as the backend wrote it', () => {
    const [row] = groupMismatches([mismatch()], () => 'never used', now);
    expect(row?.documented).toBe('aa:bb:cc:dd:ee:ff');
  });

  it('keeps two addresses apart even when they disagree about the same thing', () => {
    const grouped = groupMismatches(
      [mismatch(), mismatch({ ip: '10.10.10.3' })],
      () => undefined,
      now,
    );
    expect(grouped.map((row) => row.ip)).toEqual(['10.10.10.2', '10.10.10.3']);
  });
});
