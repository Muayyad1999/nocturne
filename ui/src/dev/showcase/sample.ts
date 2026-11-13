/**
 * Sample data for the showcase. It is generated deterministically, so
 * screenshots are stable, and it uses documentation address ranges
 * (192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24, 2001:db8::/32) so nothing
 * here can be mistaken for a real network.
 */

import type { StatusTone } from '@/design/tone';

/** A small seeded generator (mulberry32): the same numbers on every run. */
export function seeded(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export interface SampleDevice {
  id: string;
  name: string;
  address: string;
  site: string;
  platform: string;
  status: StatusTone;
  statusLabel: string;
  lab: boolean;
  cpu: number | null;
  uptime: number | null;
  seenAt: number;
  history: (number | null)[];
}

const SITES = ['Amman DC1', 'Amman DC2', 'Irbid campus', 'Aqaba port', 'Zarqa plant'];
const PLATFORMS = ['Cisco IOS-XE', 'MikroTik RouterOS', 'Arista EOS', 'Juniper Junos', 'FortiOS'];
const ROLES = ['core-sw', 'dist-sw', 'access-sw', 'edge-rtr', 'edge-fw'];
const STATES: readonly (readonly [StatusTone, string, number])[] = [
  ['ok', 'Up', 0.72],
  ['warn', 'Degraded', 0.1],
  ['crit', 'Down', 0.06],
  ['idle', 'Unmonitored', 0.06],
  ['maint', 'Maintenance', 0.06],
];

/** Fixed reference time so relative times in screenshots do not drift. */
export const SAMPLE_NOW = Date.UTC(2026, 10, 13, 2, 47, 0);

export function sampleDevices(count: number, seed = 7): SampleDevice[] {
  const random = seeded(seed);
  const pick = <T>(items: readonly T[]): T => items[Math.floor(random() * items.length)] as T;

  return Array.from({ length: count }, (_, index) => {
    const role = pick(ROLES);
    const roll = random();
    let cumulative = 0;
    const state = STATES.find(([, , share]) => (cumulative += share) >= roll) ?? STATES[0];
    const [status, statusLabel] = state as readonly [StatusTone, string, number];
    const reachable = status === 'ok' || status === 'warn' || status === 'maint';
    const base = 15 + random() * 55;
    return {
      id: `dev-${index + 1}`,
      name: `${role}-${String(index + 1).padStart(2, '0')}`,
      address: index % 9 === 8 ? `2001:db8:${(index + 16).toString(16)}::1` : `192.0.2.${(index % 250) + 1}`,
      site: pick(SITES),
      platform: pick(PLATFORMS),
      status,
      statusLabel,
      lab: random() < 0.12,
      cpu: reachable ? Math.round(base + random() * 20) : null,
      uptime: reachable ? Math.round(random() * 240 * 86_400_000) : null,
      seenAt: SAMPLE_NOW - Math.round(random() * (reachable ? 90_000 : 6 * 3_600_000)),
      history: Array.from({ length: 12 }, (_, step) =>
        reachable ? Math.round(base + Math.sin(step / 2 + index) * 9 + random() * 8) : null,
      ),
    };
  });
}

export interface SampleSeries {
  timestamps: number[];
  inbound: (number | null)[];
  outbound: (number | null)[];
  errors: (number | null)[];
}

/** Six hours of interface traffic at one-minute resolution, with a gap in the middle. */
export function sampleTraffic(points = 360, seed = 11): SampleSeries {
  const random = seeded(seed);
  const start = SAMPLE_NOW - points * 60_000;
  const timestamps: number[] = [];
  const inbound: (number | null)[] = [];
  const outbound: (number | null)[] = [];
  const errors: (number | null)[] = [];
  let drift = 0;

  for (let index = 0; index < points; index++) {
    timestamps.push(start + index * 60_000);
    // The poller missed a few samples: gaps stay gaps.
    if (index > 200 && index < 214) {
      inbound.push(null);
      outbound.push(null);
      errors.push(null);
      continue;
    }
    drift += (random() - 0.5) * 18e6;
    const wave = Math.sin(index / 28) * 160e6 + Math.sin(index / 7) * 35e6;
    inbound.push(Math.max(0, 520e6 + wave + drift + random() * 40e6));
    outbound.push(Math.max(0, 310e6 + wave * 0.6 - drift * 0.4 + random() * 30e6));
    errors.push(index > 280 && index < 300 ? Math.round(random() * 40) : Math.round(random() * 2));
  }
  return { timestamps, inbound, outbound, errors };
}

export const SAMPLE_CONFIG_BEFORE = `hostname core-sw-01
!
vlan 10
 name USERS
vlan 20
 name VOICE
!
interface GigabitEthernet1/0/1
 description uplink to dist-sw-01
 switchport mode trunk
 switchport trunk allowed vlan 10,20
!
interface GigabitEthernet1/0/2
 description printer
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/3
 description unused
 shutdown
!
interface GigabitEthernet1/0/4
 description access port 4
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/5
 description access port 5
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/6
 description access port 6
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/7
 description access port 7
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/8
 description access port 8
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
ip route 0.0.0.0 0.0.0.0 198.51.100.1
!
snmp-server community public RO
snmp-server location Amman DC1
!
ntp server 203.0.113.10
line vty 0 4
 transport input ssh
`;

export const SAMPLE_CONFIG_AFTER = `hostname core-sw-01
!
vlan 10
 name USERS
vlan 20
 name VOICE
vlan 30
 name CAMERAS
!
interface GigabitEthernet1/0/1
 description uplink to dist-sw-01
 switchport mode trunk
 switchport trunk allowed vlan 10,20,30
!
interface GigabitEthernet1/0/2
 description printer
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/3
 description camera lobby
 switchport mode access
 switchport access vlan 30
!
interface GigabitEthernet1/0/4
 description access port 4
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/5
 description access port 5
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/6
 description access port 6
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/7
 description access port 7
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
interface GigabitEthernet1/0/8
 description access port 8
 switchport mode access
 switchport access vlan 10
 spanning-tree portfast
!
ip route 0.0.0.0 0.0.0.0 198.51.100.1
!
snmp-server location Amman DC1
!
ntp server 203.0.113.10
ntp server 203.0.113.11
line vty 0 4
 transport input ssh
`;

export const SAMPLE_OUTPUT = `core-sw-01# show ip interface brief
Interface              IP-Address      OK? Method Status                Protocol
Vlan10                 192.0.2.1       YES NVRAM  up                    up
Vlan20                 198.51.100.1    YES NVRAM  up                    up
GigabitEthernet1/0/1   unassigned      YES unset  up                    up
GigabitEthernet1/0/2   unassigned      YES unset  up                    up
GigabitEthernet1/0/3   unassigned      YES unset  administratively down down`;
