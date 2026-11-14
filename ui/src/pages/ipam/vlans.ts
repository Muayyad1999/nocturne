/**
 * VLANs: the record, what the devices call it, and where the two differ.
 * No React in here.
 *
 * `ipam.vlans` merges a VLAN *record* with the VLANs the devices reported,
 * keeping each device's own name in `devices[].name`. Two traps:
 *
 * 1. **A VLAN with no record still has a row.** `documented: false` and
 *    `id: ""` mean the row exists only because devices report that VLAN — and
 *    such a row carries `created_at: 0` and `updated_at: 0`, which through any
 *    relative-time formatter reads as 1970 ("56 years ago"). {@link recordAge}
 *    returns `null` for it so the caller can write "no record" instead.
 * 2. **A name mismatch is only a mismatch against something.** Where there is
 *    a record, a device whose name differs disagrees with the plan; where
 *    there is none, devices that disagree with *each other* are worth
 *    flagging, and a single device's name is not a disagreement at all.
 *    {@link nameDisagreements} tells those cases apart.
 */

import type { VlanDevice, VlanRow } from '@/api/types/ipam';
import type { StatusTone } from '@/design/tone';
import { formatCount, plural } from '@/lib/format';

export type VlanState = 'both' | 'documented' | 'observed';

export interface VlanStateWords {
  label: string;
  meaning: string;
  tone: StatusTone;
}

export const VLAN_STATE_WORDS: Record<VlanState, VlanStateWords> = {
  both: {
    label: 'Documented and seen',
    meaning: 'A person wrote this VLAN down and devices report it.',
    tone: 'ok',
  },
  documented: {
    label: 'Documented only',
    meaning: 'A person wrote this VLAN down and no collected device reports it.',
    tone: 'info',
  },
  observed: {
    label: 'Seen only',
    meaning: 'Devices report this VLAN and nobody has written it down.',
    tone: 'warn',
  },
};

export function vlanState(row: VlanRow): VlanState {
  if (row.documented && row.observed) return 'both';
  return row.documented ? 'documented' : 'observed';
}

/**
 * When the record was last written, or `null` when there is no record — and
 * `0`, which the backend sends for a row that is only observed, counts as no
 * record rather than as the first second of 1970.
 */
export function recordAge(row: VlanRow): number | null {
  if (!row.documented) return null;
  return row.updated_at > 0 ? row.updated_at : null;
}

export interface NameDisagreement {
  /** What it is being compared with: the record, or the other devices. */
  against: 'record' | 'devices';
  expected: string;
  devices: VlanDevice[];
}

/**
 * Where the names for one VLAN do not agree.
 *
 * Against a record, every device whose name differs from it is listed. With no
 * record, the most common name among the devices is taken as the expected one
 * and the odd ones out are listed; devices that all agree produce nothing, and
 * so does a single device, which cannot disagree with anybody.
 */
export function nameDisagreements(row: VlanRow): NameDisagreement | null {
  const named = row.devices.filter((device) => device.name !== '');
  if (named.length === 0) return null;

  const same = (a: string, b: string) => a.trim().toLowerCase() === b.trim().toLowerCase();

  if (row.documented && row.name !== '') {
    const odd = named.filter((device) => !same(device.name, row.name));
    return odd.length === 0 ? null : { against: 'record', expected: row.name, devices: odd };
  }

  const tally = new Map<string, VlanDevice[]>();
  for (const device of named) {
    const key = device.name.trim().toLowerCase();
    const group = tally.get(key);
    if (group) group.push(device);
    else tally.set(key, [device]);
  }
  if (tally.size < 2) return null;
  const groups = [...tally.values()].sort((a, b) => b.length - a.length);
  const majority = groups[0];
  if (!majority?.[0]) return null;
  return {
    against: 'devices',
    expected: majority[0].name,
    devices: groups.slice(1).flat(),
  };
}

/** "on 5 devices · 24 ports" — what the network says, not what the plan says. */
export function presenceText(row: VlanRow): string {
  if (row.devices.length === 0) return 'No collected device reports it';
  const ports = row.devices.reduce((total, device) => total + device.ports, 0);
  return `${plural(row.devices.length, 'device')} · ${formatCount(ports)} ${
    ports === 1 ? 'port' : 'ports'
  }`;
}

/** Does any device reporting this VLAN belong to a lab? */
export function hasLabDevice(row: VlanRow): boolean {
  return row.devices.some((device) => device.lab);
}
