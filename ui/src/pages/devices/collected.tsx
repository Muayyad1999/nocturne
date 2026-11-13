/**
 * The tables for the data a device was read for, one per `Kind`.
 *
 * Every column renders what the backend returned and nothing else: a value
 * the device did not give is an em dash, never a zero. The columns are
 * defined here so the Interfaces tab and the Data tab agree.
 */

import type { ReactNode } from 'react';
import { Table, type TableColumn } from '@/design/Table';
import { Badge } from '@/design/Badge';
import { StatusDot } from '@/design/StatusDot';
import { Tooltip } from '@/design/Tooltip';
import { EMPTY, formatBytes, formatCount, formatDuration, formatPercent, formatRate } from '@/lib/format';
import { compareIp, compareNatural } from '@/lib/net';
import type {
  ArpEntry,
  Facts,
  Health,
  Interface,
  InventoryItem,
  Kind,
  MacEntry,
  Neighbor,
  Route,
  Vlan,
} from '@/api/types/core';
import styles from './collected.module.css';

/** Text, or the dash that says the device did not tell us. */
const text = (value: string | null | undefined): ReactNode => (value ? value : EMPTY);

/** An interface's operational state as a dot and a word. */
export function interfaceState(row: Interface): { tone: 'ok' | 'crit' | 'idle'; label: string } {
  if (row.admin_up === false) return { tone: 'idle', label: 'Disabled' };
  if (row.oper_up === true) return { tone: 'ok', label: 'Up' };
  if (row.oper_up === false) return { tone: 'crit', label: 'Down' };
  return { tone: 'idle', label: 'Unknown' };
}

export const INTERFACE_COLUMNS: TableColumn<Interface>[] = [
  {
    id: 'state',
    header: 'State',
    width: 92,
    sortValue: (row) => interfaceState(row).label,
    cell: (row) => {
      const state = interfaceState(row);
      return (
        <span className={styles.inline}>
          <StatusDot tone={state.tone} label={state.label} />
          {state.label}
        </span>
      );
    },
  },
  {
    id: 'name',
    header: 'Interface',
    required: true,
    mono: true,
    width: 190,
    compare: (a, b) => compareNatural(a.name, b.name),
    cell: (row) => row.name,
  },
  {
    id: 'description',
    header: 'Description',
    sortValue: (row) => row.description,
    cell: (row) => <span className={styles.cell}>{text(row.description)}</span>,
  },
  {
    id: 'mode',
    header: 'Mode / VLAN',
    width: 130,
    sortValue: (row) => row.mode,
    cell: (row) => {
      if (!row.mode && row.vlan == null) return EMPTY;
      return (
        <span className={styles.inline}>
          {row.mode && <Badge variant="outline">{row.mode}</Badge>}
          {row.vlan != null && <span className="mono">{row.vlan}</span>}
        </span>
      );
    },
  },
  {
    id: 'address',
    header: 'Address',
    width: 150,
    mono: true,
    compare: (a, b) => compareIp(a.ipv4[0] ?? '', b.ipv4[0] ?? ''),
    cell: (row) => {
      const addresses = [...row.ipv4, ...row.ipv6];
      if (addresses.length === 0) return EMPTY;
      return (
        <Tooltip content={addresses.join(', ')}>
          <span className={styles.cell}>{addresses[0]}</span>
        </Tooltip>
      );
    },
  },
  {
    id: 'speed',
    header: 'Speed',
    width: 92,
    align: 'right',
    sortValue: (row) => row.speed_bps,
    cell: (row) => formatRate(row.speed_bps),
  },
  {
    id: 'in',
    header: 'In',
    width: 92,
    align: 'right',
    sortValue: (row) => row.counters.in_rate_bps,
    cell: (row) => formatRate(row.counters.in_rate_bps),
  },
  {
    id: 'out',
    header: 'Out',
    width: 92,
    align: 'right',
    sortValue: (row) => row.counters.out_rate_bps,
    cell: (row) => formatRate(row.counters.out_rate_bps),
  },
  {
    id: 'errors',
    header: 'Errors',
    width: 90,
    align: 'right',
    sortValue: (row) => errorTotal(row),
    cell: (row) => {
      const total = errorTotal(row);
      if (total == null) return EMPTY;
      return <span className={total > 0 ? styles.bad : undefined}>{formatCount(total)}</span>;
    },
  },
  {
    id: 'mac',
    header: 'MAC',
    width: 150,
    mono: true,
    defaultHidden: true,
    sortValue: (row) => row.mac,
    cell: (row) => text(row.mac),
  },
  {
    id: 'mtu',
    header: 'MTU',
    width: 80,
    align: 'right',
    defaultHidden: true,
    sortValue: (row) => row.mtu,
    cell: (row) => (row.mtu == null ? EMPTY : formatCount(row.mtu)),
  },
];

/** In and out errors added up, or null when the device reported neither. */
export function errorTotal(row: Interface): number | null {
  const { in_errors, out_errors } = row.counters;
  if (in_errors == null && out_errors == null) return null;
  return (in_errors ?? 0) + (out_errors ?? 0);
}

const NEIGHBOR_COLUMNS: TableColumn<Neighbor>[] = [
  {
    id: 'local',
    header: 'Local port',
    width: 180,
    mono: true,
    required: true,
    compare: (a, b) => compareNatural(a.local_interface, b.local_interface),
    cell: (row) => text(row.local_interface),
  },
  {
    id: 'device',
    header: 'Neighbour',
    sortValue: (row) => row.device,
    cell: (row) => <span className={styles.cell}>{text(row.device)}</span>,
  },
  {
    id: 'remote',
    header: 'Its port',
    width: 180,
    mono: true,
    sortValue: (row) => row.remote_interface,
    cell: (row) => text(row.remote_interface),
  },
  {
    id: 'address',
    header: 'Address',
    width: 140,
    mono: true,
    compare: (a, b) => compareIp(a.address, b.address),
    cell: (row) => text(row.address),
  },
  {
    id: 'platform',
    header: 'Platform',
    width: 180,
    sortValue: (row) => row.platform,
    cell: (row) => <span className={styles.cell}>{text(row.platform)}</span>,
  },
  {
    id: 'protocol',
    header: 'Seen by',
    width: 90,
    sortValue: (row) => row.protocol,
    cell: (row) => (row.protocol ? <Badge variant="outline">{row.protocol}</Badge> : EMPTY),
  },
];

const VLAN_COLUMNS: TableColumn<Vlan>[] = [
  {
    id: 'id',
    header: 'VLAN',
    width: 80,
    align: 'right',
    mono: true,
    required: true,
    sortValue: (row) => row.id,
    cell: (row) => (row.id == null ? EMPTY : row.id),
  },
  { id: 'name', header: 'Name', width: 200, sortValue: (row) => row.name, cell: (row) => text(row.name) },
  {
    id: 'status',
    header: 'Status',
    width: 110,
    sortValue: (row) => row.status,
    cell: (row) => text(row.status),
  },
  {
    id: 'interfaces',
    header: 'Ports',
    mono: true,
    sortValue: (row) => row.interfaces.length,
    cell: (row) =>
      row.interfaces.length === 0 ? (
        EMPTY
      ) : (
        <Tooltip content={row.interfaces.join(', ')}>
          <span className={styles.cell}>{row.interfaces.join(', ')}</span>
        </Tooltip>
      ),
  },
];

const MAC_COLUMNS: TableColumn<MacEntry>[] = [
  {
    id: 'mac',
    header: 'MAC address',
    width: 180,
    mono: true,
    required: true,
    sortValue: (row) => row.mac,
    cell: (row) => text(row.mac),
  },
  {
    id: 'vlan',
    header: 'VLAN',
    width: 80,
    align: 'right',
    mono: true,
    sortValue: (row) => row.vlan,
    cell: (row) => (row.vlan == null ? EMPTY : row.vlan),
  },
  {
    id: 'interface',
    header: 'Port',
    width: 190,
    mono: true,
    compare: (a, b) => compareNatural(a.interface, b.interface),
    cell: (row) => text(row.interface),
  },
  { id: 'kind', header: 'Kind', sortValue: (row) => row.kind, cell: (row) => text(row.kind) },
];

const ARP_COLUMNS: TableColumn<ArpEntry>[] = [
  {
    id: 'ip',
    header: 'IP address',
    width: 160,
    mono: true,
    required: true,
    compare: (a, b) => compareIp(a.ip, b.ip),
    cell: (row) => text(row.ip),
  },
  {
    id: 'mac',
    header: 'MAC address',
    width: 180,
    mono: true,
    sortValue: (row) => row.mac,
    cell: (row) => text(row.mac),
  },
  {
    id: 'interface',
    header: 'Interface',
    width: 190,
    mono: true,
    compare: (a, b) => compareNatural(a.interface, b.interface),
    cell: (row) => text(row.interface),
  },
  { id: 'age', header: 'Age', sortValue: (row) => row.age, cell: (row) => text(row.age) },
];

const ROUTE_COLUMNS: TableColumn<Route>[] = [
  {
    id: 'prefix',
    header: 'Destination',
    width: 190,
    mono: true,
    required: true,
    compare: (a, b) => compareIp(a.prefix.split('/')[0] ?? '', b.prefix.split('/')[0] ?? ''),
    cell: (row) => text(row.prefix),
  },
  {
    id: 'protocol',
    header: 'Learned by',
    width: 120,
    sortValue: (row) => row.protocol,
    cell: (row) => (row.protocol ? <Badge variant="outline">{row.protocol}</Badge> : EMPTY),
  },
  {
    id: 'next_hop',
    header: 'Next hop',
    width: 160,
    mono: true,
    compare: (a, b) => compareIp(a.next_hop, b.next_hop),
    cell: (row) => text(row.next_hop),
  },
  {
    id: 'interface',
    header: 'Interface',
    mono: true,
    compare: (a, b) => compareNatural(a.interface, b.interface),
    cell: (row) => text(row.interface),
  },
  {
    id: 'metric',
    header: 'Distance / metric',
    width: 140,
    align: 'right',
    mono: true,
    sortValue: (row) => row.metric,
    cell: (row) =>
      row.distance == null && row.metric == null
        ? EMPTY
        : `${row.distance ?? EMPTY}/${row.metric ?? EMPTY}`,
  },
  {
    id: 'vrf',
    header: 'VRF',
    width: 110,
    defaultHidden: true,
    sortValue: (row) => row.vrf,
    cell: (row) => text(row.vrf),
  },
];

const INVENTORY_COLUMNS: TableColumn<InventoryItem>[] = [
  {
    id: 'name',
    header: 'Part',
    width: 200,
    required: true,
    sortValue: (row) => row.name,
    cell: (row) => <span className={styles.cell}>{text(row.name)}</span>,
  },
  {
    id: 'description',
    header: 'Description',
    sortValue: (row) => row.description,
    cell: (row) => <span className={styles.cell}>{text(row.description)}</span>,
  },
  {
    id: 'part_id',
    header: 'Part number',
    width: 170,
    mono: true,
    sortValue: (row) => row.part_id,
    cell: (row) => text(row.part_id),
  },
  {
    id: 'serial',
    header: 'Serial',
    width: 170,
    mono: true,
    sortValue: (row) => row.serial,
    cell: (row) => text(row.serial),
  },
];

/** `facts` and `health` are one row about the whole device: a key-value list. */
const FACTS_COLUMNS: TableColumn<{ label: string; value: ReactNode }>[] = [
  { id: 'label', header: 'Fact', width: 220, required: true, cell: (row) => row.label },
  { id: 'value', header: 'Value', mono: true, required: true, cell: (row) => row.value },
];

/** The key-value rows a `facts` document holds. */
export function factRows(facts: Facts): { label: string; value: ReactNode }[] {
  return [
    { label: 'Host name', value: text(facts.hostname) },
    { label: 'Vendor', value: text(facts.vendor) },
    { label: 'Model', value: text(facts.model) },
    { label: 'Operating system', value: text(facts.os) },
    { label: 'Version', value: text(facts.os_version) },
    { label: 'Serial number', value: text(facts.serial) },
    {
      label: 'Uptime',
      value: facts.uptime_s == null ? EMPTY : formatDuration(facts.uptime_s * 1000),
    },
    ...Object.entries(facts.extra ?? {}).map(([label, value]) => ({ label, value: text(value) })),
  ];
}

/** The key-value rows a `health` document holds. */
export function healthRows(health: Health): { label: string; value: ReactNode }[] {
  const used = health.memory_used;
  const total = health.memory_total;
  return [
    { label: 'Processor', value: formatPercent(health.cpu_pct) },
    {
      label: 'Memory used',
      value:
        used == null
          ? EMPTY
          : total == null
            ? formatBytes(used)
            : `${formatBytes(used)} of ${formatBytes(total)} (${formatPercent((used / total) * 100)})`,
    },
    {
      label: 'Temperature',
      value:
        health.temperature_c == null ? EMPTY : `${health.temperature_c.toFixed(1)} °C`,
    },
    {
      label: 'Uptime',
      value: health.uptime_s == null ? EMPTY : formatDuration(health.uptime_s * 1000),
    },
  ];
}

/** A stable key per row: collected tables may legitimately repeat a value. */
type Keyed<T> = T & { __key: string };

function keyed<T>(rows: readonly T[]): Keyed<T>[] {
  return rows.map((row, index) => ({ ...row, __key: String(index) }) as Keyed<T>);
}

export interface KindTableProps {
  kind: Kind;
  /** The rows exactly as `collect.get` returned them. */
  rows: readonly unknown[];
  empty: ReactNode;
  /** Remembers sort and column choices; one id per kind. */
  id?: string;
  maxHeight?: number | string;
  fill?: boolean;
  onRowActivate?: (row: unknown) => void;
}

/**
 * One collected kind as its own table. `facts` and `health` describe the
 * device as a whole, so they are shown as label and value rather than a grid.
 */
export function KindTable({ kind, rows, empty, id, maxHeight, fill, onRowActivate }: KindTableProps) {
  const common = { 'aria-label': KIND_TABLE_LABEL[kind], empty, id, maxHeight, fill } as const;

  switch (kind) {
    case 'interfaces':
      return (
        <Table
          {...common}
          columns={INTERFACE_COLUMNS}
          rows={keyed(rows as Interface[])}
          rowKey={(row) => row.__key}
          defaultSort={{ columnId: 'name', direction: 'asc' }}
          onRowActivate={onRowActivate}
        />
      );
    case 'neighbors':
      return (
        <Table
          {...common}
          columns={NEIGHBOR_COLUMNS}
          rows={keyed(rows as Neighbor[])}
          rowKey={(row) => row.__key}
          defaultSort={{ columnId: 'local', direction: 'asc' }}
        />
      );
    case 'vlans':
      return (
        <Table
          {...common}
          columns={VLAN_COLUMNS}
          rows={keyed(rows as Vlan[])}
          rowKey={(row) => row.__key}
          defaultSort={{ columnId: 'id', direction: 'asc' }}
        />
      );
    case 'mac_table':
      return (
        <Table
          {...common}
          columns={MAC_COLUMNS}
          rows={keyed(rows as MacEntry[])}
          rowKey={(row) => row.__key}
        />
      );
    case 'arp':
      return (
        <Table
          {...common}
          columns={ARP_COLUMNS}
          rows={keyed(rows as ArpEntry[])}
          rowKey={(row) => row.__key}
        />
      );
    case 'routes':
      return (
        <Table
          {...common}
          columns={ROUTE_COLUMNS}
          rows={keyed(rows as Route[])}
          rowKey={(row) => row.__key}
        />
      );
    case 'inventory':
      return (
        <Table
          {...common}
          columns={INVENTORY_COLUMNS}
          rows={keyed(rows as InventoryItem[])}
          rowKey={(row) => row.__key}
        />
      );
    default: {
      const first = rows[0];
      const pairs =
        kind === 'facts'
          ? first
            ? factRows(first as Facts)
            : []
          : first
            ? healthRows(first as Health)
            : [];
      return (
        <Table
          {...common}
          columns={FACTS_COLUMNS}
          rows={pairs}
          rowKey={(row) => row.label}
        />
      );
    }
  }
}

/** What each kind's table is called, for assistive technology. */
const KIND_TABLE_LABEL: Record<Kind, string> = {
  facts: 'Device facts',
  interfaces: 'Interfaces',
  neighbors: 'Neighbours',
  vlans: 'VLANs',
  mac_table: 'MAC address table',
  arp: 'ARP table',
  routes: 'Routing table',
  health: 'Device health',
  inventory: 'Hardware inventory',
};
