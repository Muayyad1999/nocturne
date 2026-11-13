/**
 * What the Devices screens know about a device beyond its record: how to say
 * its state in words, how to read a transport or a device type, and the local
 * search that filters the list as you type.
 *
 * Everything here is pure, so the screens stay about layout.
 */

import type { StatusTone } from '@/design/tone';
import type { DeviceRow, Kind, Transport } from '@/api/types/core';

/** A device's reachability as a tone and a word. `null` is unknown, never down. */
export interface DeviceStatus {
  tone: StatusTone;
  label: string;
  /** `up`, `down` or `unknown`: the value `devices.list` filters on. */
  value: 'up' | 'down' | 'unknown';
}

export function deviceStatus(device: DeviceRow): DeviceStatus {
  switch (device.state.reachable) {
    case true:
      return { tone: 'ok', label: 'Up', value: 'up' };
    case false:
      return { tone: 'crit', label: 'Down', value: 'down' };
    default:
      return { tone: 'idle', label: 'Unknown', value: 'unknown' };
  }
}

export const TRANSPORT_LABEL: Record<Transport, string> = {
  ssh: 'SSH',
  telnet: 'Telnet',
  serial: 'Serial console',
  sim: 'Simulated',
  none: 'Monitor only',
};

/** The transports a person may choose for a device: `sim` belongs to a lab. */
export const TRANSPORT_OPTIONS: readonly { value: Transport; label: string; hint: string }[] = [
  { value: 'ssh', label: 'SSH', hint: 'The usual way. Port 22 unless you say otherwise.' },
  { value: 'telnet', label: 'Telnet', hint: 'Unencrypted. Port 23 unless you say otherwise.' },
  { value: 'serial', label: 'Serial console', hint: 'The address is the port name (COM3, /dev/ttyUSB0); the port field is the baud rate.' },
  { value: 'none', label: 'Monitor only', hint: 'No CLI access: Nocturne only checks that it answers.' },
];

/** The default TCP port of a transport, for the port field's placeholder. */
export function defaultPort(transport: Transport): string {
  switch (transport) {
    case 'ssh':
      return '22';
    case 'telnet':
      return '23';
    case 'serial':
      return '9600';
    default:
      return '';
  }
}

/** The device type a device is actually treated as, and whether that was detected. */
export function effectiveProfile(device: DeviceRow): { id: string; detected: boolean } | null {
  if (device.profile) return { id: device.profile, detected: false };
  const detected = device.state.detected_profile;
  return detected ? { id: detected, detected: true } : null;
}

/** The collected kinds, in the order the detail page shows them. */
export const KIND_LABEL: Record<Kind, string> = {
  facts: 'Facts',
  interfaces: 'Interfaces',
  neighbors: 'Neighbours',
  vlans: 'VLANs',
  mac_table: 'MAC table',
  arp: 'ARP table',
  routes: 'Routes',
  health: 'Health',
  inventory: 'Hardware',
};

/** Whether the kind's name takes a plural verb: "facts have", "health has". */
export const KIND_PLURAL: Record<Kind, boolean> = {
  facts: true,
  interfaces: true,
  neighbors: true,
  vlans: true,
  mac_table: false,
  arp: false,
  routes: true,
  health: false,
  inventory: false,
};

/** One sentence per kind, for the state that says it was never collected. */
export const KIND_DESCRIPTION: Record<Kind, string> = {
  facts: 'Model, serial number, software version and uptime, as the device reports them.',
  interfaces: 'Every port: its state, description, speed, addresses and counters.',
  neighbors: 'What the device sees on its ports over CDP, LLDP or MNDP.',
  vlans: 'The VLANs the device knows and the ports in each.',
  mac_table: 'Which MAC address was last seen on which port.',
  arp: 'The device’s IP-to-MAC bindings.',
  routes: 'The routing table, with protocol, next hop and metric.',
  health: 'Processor, memory and temperature as the device reports them.',
  inventory: 'Chassis, modules, power supplies and transceivers with their serial numbers.',
};

/**
 * The text a device is searched by: the same fields the backend matches on,
 * so typing here and filtering there agree.
 */
export function searchText(device: DeviceRow): string {
  const facts = device.state.facts;
  return [
    device.name,
    device.address,
    device.site,
    device.role,
    device.tags.join(' '),
    device.profile,
    device.notes,
    facts?.hostname,
    facts?.model,
    facts?.serial,
    facts?.os_version,
  ]
    .filter(Boolean)
    .join(' ')
    .toLowerCase();
}

/**
 * Every whitespace-separated word must appear somewhere in the device, which
 * is exactly what `devices.list`'s `query` does. Filtering here instead of
 * over RPC keeps a 2 000-device list instant as the user types.
 */
export function matchesQuery(haystack: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return words.every((word) => haystack.includes(word));
}

/** Which devices the list shows: the lab switch, the facet chips, then the text. */
export interface DeviceFilterState {
  query: string;
  /** `all` keeps lab devices in the list, `hide` leaves them out, `only` shows nothing else. */
  lab: 'all' | 'hide' | 'only';
  site: string | null;
  role: string | null;
  tag: string | null;
  profile: string | null;
  status: 'up' | 'down' | 'unknown' | null;
}

export const EMPTY_FILTER: DeviceFilterState = {
  query: '',
  lab: 'all',
  site: null,
  role: null,
  tag: null,
  profile: null,
  status: null,
};

export function isFiltered(filter: DeviceFilterState): boolean {
  return (
    filter.query !== '' ||
    filter.lab !== 'all' ||
    filter.site !== null ||
    filter.role !== null ||
    filter.tag !== null ||
    filter.profile !== null ||
    filter.status !== null
  );
}

/** How many facet chips are active, for the "Clear filters" button. */
export function activeFacetCount(filter: DeviceFilterState): number {
  return [filter.site, filter.role, filter.tag, filter.profile, filter.status].filter(
    (value) => value !== null,
  ).length;
}

const sameText = (wanted: string | null, actual: string) =>
  wanted === null || wanted.toLowerCase() === actual.toLowerCase();

/** Applies the whole filter to the rows `devices.list` returned. */
export function filterDevices(
  devices: readonly DeviceRow[],
  filter: DeviceFilterState,
): DeviceRow[] {
  const query = filter.query.trim();
  return devices.filter((device) => {
    const isLab = device.lab != null;
    if (filter.lab === 'hide' && isLab) return false;
    if (filter.lab === 'only' && !isLab) return false;
    if (!sameText(filter.site, device.site)) return false;
    if (!sameText(filter.role, device.role)) return false;
    if (!sameText(filter.profile, device.profile)) return false;
    if (filter.tag !== null && !device.tags.some((tag) => tag.toLowerCase() === filter.tag?.toLowerCase())) {
      return false;
    }
    if (filter.status !== null && deviceStatus(device).value !== filter.status) return false;
    return query === '' || matchesQuery(searchText(device), query);
  });
}
