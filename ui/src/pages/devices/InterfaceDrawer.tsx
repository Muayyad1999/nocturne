/**
 * One interface in detail: what it is, what is on it, and what its counters
 * say. Opened from a row of the Interfaces table.
 *
 * The neighbour and the MAC addresses come from the data already collected
 * for this device, so the drawer never goes back to the device to fill itself.
 */

import { Network } from 'lucide-react';
import { useRpc } from '@/api/hooks';
import type { Interface, MacEntry, Neighbor, Stored } from '@/api/types/core';
import { Drawer } from '@/design/Drawer';
import { EmptyState } from '@/design/EmptyState';
import { KeyValue, type KeyValueItem } from '@/design/KeyValue';
import { Panel } from '@/design/Panel';
import { StatusPill } from '@/design/StatusPill';
import { formatCount, formatRate } from '@/lib/format';
import { interfaceState, KindTable } from './collected';
import styles from './DeviceDetail.module.css';

export interface InterfaceDrawerProps {
  open: boolean;
  row: Interface | null;
  deviceId: string;
  onClose: () => void;
}

export function InterfaceDrawer({ open, row, deviceId, onClose }: InterfaceDrawerProps) {
  const neighbours = useRpc<Stored | null>(
    'collect.get',
    { id: deviceId, kind: 'neighbors' },
    { enabled: open, invalidateOn: ['collect.updated'] },
  );
  const macs = useRpc<Stored | null>(
    'collect.get',
    { id: deviceId, kind: 'mac_table' },
    { enabled: open, invalidateOn: ['collect.updated'] },
  );

  if (!row) return null;
  const state = interfaceState(row);

  const neighbour = (neighbours.data?.rows as Neighbor[] | undefined)?.find(
    (entry) => entry.local_interface === row.name,
  );
  const seen = ((macs.data?.rows as MacEntry[] | undefined) ?? []).filter(
    (entry) => entry.interface === row.name,
  );

  const facts: KeyValueItem[] = [
    { label: 'Description', value: row.description || null },
    { label: 'Mode', value: row.mode || null },
    { label: 'VLAN', value: row.vlan ?? null, mono: true },
    { label: 'Speed', value: row.speed_bps == null ? null : formatRate(row.speed_bps) },
    { label: 'Duplex', value: row.duplex || null },
    { label: 'MTU', value: row.mtu == null ? null : formatCount(row.mtu), mono: true },
    { label: 'MAC address', value: row.mac || null, mono: true },
    { label: 'Media', value: row.kind || null },
    {
      label: 'IPv4',
      value: row.ipv4.length > 0 ? row.ipv4.join(', ') : null,
      mono: true,
    },
    {
      label: 'IPv6',
      value: row.ipv6.length > 0 ? row.ipv6.join(', ') : null,
      mono: true,
    },
  ];

  const counters: KeyValueItem[] = [
    { label: 'In', value: counter(row, 'in_octets', 'bytes'), mono: true },
    { label: 'Out', value: counter(row, 'out_octets', 'bytes'), mono: true },
    { label: 'In packets', value: counter(row, 'in_packets', 'count'), mono: true },
    { label: 'Out packets', value: counter(row, 'out_packets', 'count'), mono: true },
    { label: 'In rate', value: rate(row.counters.in_rate_bps), mono: true },
    { label: 'Out rate', value: rate(row.counters.out_rate_bps), mono: true },
    { label: 'In errors', value: counter(row, 'in_errors', 'count'), mono: true },
    { label: 'Out errors', value: counter(row, 'out_errors', 'count'), mono: true },
    { label: 'In discards', value: counter(row, 'in_discards', 'count'), mono: true },
    { label: 'Out discards', value: counter(row, 'out_discards', 'count'), mono: true },
    { label: 'CRC errors', value: counter(row, 'crc', 'count'), mono: true },
  ];
  const anyCounter = counters.some((item) => item.value !== null);

  return (
    <Drawer
      open={open}
      onClose={onClose}
      size="md"
      title={row.name}
      description={
        <span className={styles.subtitle}>
          <StatusPill tone={state.tone}>{state.label}</StatusPill>
          {row.description && <span>{row.description}</span>}
        </span>
      }
    >
      <div className={styles.tabBody}>
        <Panel title="Interface">
          <KeyValue items={facts} />
        </Panel>

        <Panel title="Counters">
          {anyCounter ? (
            <KeyValue items={counters} />
          ) : (
            <EmptyState
              size="compact"
              icon={Network}
              title="No counters"
              description="This device type does not report counters for its interfaces, or they have not been collected yet."
            />
          )}
        </Panel>

        <Panel title="Neighbour">
          {neighbour ? (
            <KeyValue
              items={[
                { label: 'Device', value: neighbour.device || null },
                { label: 'Its port', value: neighbour.remote_interface || null, mono: true },
                { label: 'Address', value: neighbour.address || null, mono: true },
                { label: 'Platform', value: neighbour.platform || null },
                { label: 'Seen by', value: neighbour.protocol || null },
              ]}
            />
          ) : (
            <EmptyState
              size="compact"
              icon={Network}
              title="Nothing on this port"
              description={
                neighbours.data
                  ? 'No neighbour was reported on this port the last time the device was read.'
                  : 'Neighbours have not been collected from this device yet.'
              }
            />
          )}
        </Panel>

        <Panel title={`MAC addresses seen on it (${formatCount(seen.length)})`} padding="none">
          {seen.length > 0 ? (
            // The same table the MAC tab uses, so a port and the whole device
            // read the same way. Keyed by position: a MAC may legitimately
            // appear twice on one port, in two VLANs.
            <KindTable
              kind="mac_table"
              rows={seen}
              maxHeight={220}
              empty={null}
            />
          ) : (
            <EmptyState
              size="compact"
              icon={Network}
              title="No MAC addresses"
              description={
                macs.data
                  ? 'The MAC table holds nothing for this port.'
                  : 'The MAC table has not been collected from this device yet.'
              }
            />
          )}
        </Panel>
      </div>
    </Drawer>
  );
}

function counter(row: Interface, key: keyof Interface['counters'], as: 'bytes' | 'count') {
  const value = row.counters[key];
  if (value == null) return null;
  return as === 'bytes' ? `${formatCount(value)} B` : formatCount(value);
}

function rate(value: number | null | undefined) {
  return value == null ? null : formatRate(value);
}
