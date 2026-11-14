import { describe, expect, it } from 'vitest';
import type { VlanDevice, VlanRow } from '@/api/types/ipam';
import { nameDisagreements, presenceText, recordAge, vlanState } from './vlans';

function on(device: string, name: string, over: Partial<VlanDevice> = {}): VlanDevice {
  return {
    device_id: device,
    device,
    name,
    status: 'active',
    ports: 2,
    collected_at: 5_000,
    lab: false,
    ...over,
  };
}

function vlan(over: Partial<VlanRow> = {}): VlanRow {
  return {
    id: '',
    vid: 10,
    name: 'Staff',
    description: '',
    prefix_ids: [],
    source: 'discovered',
    created_at: 0,
    updated_at: 0,
    devices: [],
    prefixes: [],
    documented: false,
    observed: true,
    ...over,
  };
}

describe('a VLAN record and a VLAN the devices report', () => {
  it('tells documented, seen and both apart', () => {
    expect(vlanState(vlan())).toBe('observed');
    expect(vlanState(vlan({ documented: true, observed: false }))).toBe('documented');
    expect(vlanState(vlan({ documented: true, observed: true }))).toBe('both');
  });

  it('has no record time for a VLAN with no record, rather than the epoch', () => {
    // The backend sends created_at/updated_at of 0 for an observed-only VLAN,
    // and 0 through a relative formatter reads as "56 years ago".
    expect(recordAge(vlan())).toBeNull();
    expect(recordAge(vlan({ documented: true, updated_at: 0 }))).toBeNull();
    expect(recordAge(vlan({ documented: true, updated_at: 9_000 }))).toBe(9_000);
  });

  it('says what the network reports, and says so plainly when it reports nothing', () => {
    expect(presenceText(vlan({ devices: [on('AS1', 'Staff'), on('AS2', 'Staff', { ports: 1 })] }))).toBe(
      '2 devices · 3 ports',
    );
    expect(presenceText(vlan({ documented: true, observed: false }))).toBe(
      'No collected device reports it',
    );
  });
});

describe('name disagreements', () => {
  it('measures every device against the record when there is one', () => {
    const found = nameDisagreements(
      vlan({
        documented: true,
        name: 'Staff wired',
        devices: [on('AS1', 'Staff'), on('AS2', 'Staff wired'), on('DS1', 'STAFF WIRED')],
      }),
    );
    expect(found?.against).toBe('record');
    expect(found?.expected).toBe('Staff wired');
    // Case alone is not a disagreement; a different word is.
    expect(found?.devices.map((device) => device.device)).toEqual(['AS1']);
  });

  it('finds nothing when the devices agree with the record', () => {
    expect(
      nameDisagreements(
        vlan({ documented: true, name: 'Staff', devices: [on('AS1', 'Staff'), on('AS2', 'Staff')] }),
      ),
    ).toBeNull();
  });

  it('with no record, compares the devices with each other and names the odd ones out', () => {
    const found = nameDisagreements(
      vlan({ devices: [on('AS1', 'Staff'), on('AS2', 'Staff'), on('AS3', 'Users')] }),
    );
    expect(found?.against).toBe('devices');
    expect(found?.expected).toBe('Staff');
    expect(found?.devices.map((device) => device.device)).toEqual(['AS3']);
  });

  it('one device cannot disagree with anybody', () => {
    expect(nameDisagreements(vlan({ devices: [on('AS1', 'Staff')] }))).toBeNull();
    expect(nameDisagreements(vlan({ devices: [] }))).toBeNull();
  });

  it('ignores a device that reported no name at all', () => {
    expect(
      nameDisagreements(vlan({ devices: [on('AS1', 'Staff'), on('AS2', '')] })),
    ).toBeNull();
  });
});
