/**
 * A device's live tables, as its own `show` commands print them: routes, ARP,
 * MAC, VLANs, spanning tree, neighbours, OSPF, DHCP and NAT.
 *
 * `labs.node.tables` only answers for a running lab, which is honest — a
 * stopped simulator has no MAC table to report — so the drawer says that
 * rather than showing empty tables.
 */

import { Table2 } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useRpc } from '@/api/hooks';
import type { NodeTables } from '@/api/types/lab';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { Drawer } from '@/design/Drawer';
import { Skeleton } from '@/design/Skeleton';
import { Table, type TableColumn } from '@/design/Table';
import { TabPanel, Tabs } from '@/design/Tabs';
import styles from './TablesDrawer.module.css';

export interface TablesDrawerProps {
  open: boolean;
  onClose: () => void;
  labId: string;
  labRunning: boolean;
  node: string | null;
}

type TabId = 'routes' | 'arp' | 'mac' | 'vlans' | 'stp' | 'neighbors' | 'ospf' | 'dhcp' | 'nat';

const TABS: { id: TabId; label: string }[] = [
  { id: 'routes', label: 'Routes' },
  { id: 'arp', label: 'ARP' },
  { id: 'mac', label: 'MAC' },
  { id: 'vlans', label: 'VLANs' },
  { id: 'stp', label: 'Spanning tree' },
  { id: 'neighbors', label: 'Neighbours' },
  { id: 'ospf', label: 'OSPF' },
  { id: 'dhcp', label: 'DHCP' },
  { id: 'nat', label: 'NAT' },
];

export function TablesDrawer({ open, onClose, labId, labRunning, node }: TablesDrawerProps) {
  const [tab, setTab] = useState<TabId>('routes');
  const tables = useRpc<NodeTables>(
    'labs.node.tables',
    { lab_id: labId, node },
    { enabled: open && node !== null && labRunning, invalidateOn: ['labs.changed'] },
  );

  return (
    <Drawer
      open={open}
      onClose={onClose}
      size="lg"
      title={node ? `${node} tables` : 'Tables'}
      description="What the device would print for each of these, right now."
    >
      {!labRunning ? (
        <EmptyState
          icon={Table2}
          size="compact"
          title="The lab is not running"
          description="A stopped simulator has no routing, ARP or MAC table to show. Start the lab to see them."
        />
      ) : (
        <>
          <Tabs
            id="lab-tables"
            aria-label="Device tables"
            tabs={TABS}
            value={tab}
            onChange={(id) => setTab(id as TabId)}
          />
          <TabPanel tabs="lab-tables" value={tab} className={styles.body}>
            {tables.isPending && <Skeleton height={160} shape="block" />}
            {tables.error && (
              <ErrorState
                title="The tables could not be read"
                error={tables.error}
                onRetry={() => void tables.refetch()}
                size="compact"
              />
            )}
            {tables.data && <TablePanel tab={tab} tables={tables.data} />}
          </TabPanel>
        </>
      )}
    </Drawer>
  );
}

function TablePanel({ tab, tables }: { tab: TabId; tables: NodeTables }) {
  switch (tab) {
    case 'routes':
      return (
        <Rows
          id="routes"
          label="Routing table"
          rows={tables.routes}
          columns={[
            { id: 'code', header: '', cell: (row) => row.code, width: 56, mono: true },
            { id: 'prefix', header: 'Prefix', cell: (row) => row.prefix, mono: true },
            {
              id: 'ad',
              header: 'AD/metric',
              cell: (row) => `${row.ad}/${row.metric}`,
              align: 'right',
              mono: true,
            },
            { id: 'next', header: 'Next hop', cell: (row) => row.next_hop || '—', mono: true },
            { id: 'if', header: 'Interface', cell: (row) => row.interface, mono: true },
          ]}
        />
      );
    case 'arp':
      return (
        <Rows
          id="arp"
          label="ARP table"
          rows={tables.arp}
          columns={[
            { id: 'ip', header: 'Address', cell: (row) => row.ip, mono: true },
            { id: 'mac', header: 'MAC', cell: (row) => row.mac, mono: true },
            { id: 'if', header: 'Interface', cell: (row) => row.interface, mono: true },
            { id: 'age', header: 'Age (s)', cell: (row) => row.age_s, align: 'right', mono: true },
          ]}
        />
      );
    case 'mac':
      return (
        <Rows
          id="mac"
          label="MAC address table"
          rows={tables.mac}
          columns={[
            { id: 'vlan', header: 'VLAN', cell: (row) => row.vlan, align: 'right', mono: true },
            { id: 'mac', header: 'MAC', cell: (row) => row.mac, mono: true },
            { id: 'kind', header: 'Type', cell: (row) => row.kind },
            { id: 'port', header: 'Port', cell: (row) => row.port, mono: true },
          ]}
        />
      );
    case 'vlans':
      return (
        <Rows
          id="vlans"
          label="VLANs"
          rows={tables.vlans}
          columns={[
            { id: 'id', header: 'ID', cell: (row) => row.id, align: 'right', mono: true },
            { id: 'name', header: 'Name', cell: (row) => row.name },
            { id: 'status', header: 'Status', cell: (row) => row.status },
            { id: 'ports', header: 'Ports', cell: (row) => row.ports.join(', ') || '—', mono: true },
          ]}
        />
      );
    case 'stp':
      return (
        <Rows
          id="stp"
          label="Spanning tree"
          rows={tables.stp}
          columns={[
            { id: 'vlan', header: 'VLAN', cell: (row) => row.vlan, align: 'right', mono: true },
            { id: 'if', header: 'Interface', cell: (row) => row.interface, mono: true },
            { id: 'role', header: 'Role', cell: (row) => row.role },
            { id: 'state', header: 'State', cell: (row) => row.state },
            { id: 'cost', header: 'Cost', cell: (row) => row.cost, align: 'right', mono: true },
            { id: 'root', header: 'Root', cell: (row) => (row.is_root ? 'this bridge' : row.root_id), mono: true },
          ]}
        />
      );
    case 'neighbors':
      return (
        <Rows
          id="neighbors"
          label="Neighbours"
          rows={tables.neighbors}
          columns={[
            { id: 'protocol', header: 'By', cell: (row) => row.protocol },
            { id: 'local', header: 'Local port', cell: (row) => row.local_if, mono: true },
            { id: 'device', header: 'Device', cell: (row) => row.device, mono: true },
            { id: 'remote', header: 'Remote port', cell: (row) => row.remote_if, mono: true },
            { id: 'mgmt', header: 'Address', cell: (row) => row.mgmt_ip || '—', mono: true },
          ]}
        />
      );
    case 'ospf':
      return (
        <Rows
          id="ospf"
          label="OSPF neighbours"
          rows={tables.ospf_neighbors}
          columns={[
            { id: 'id', header: 'Neighbour', cell: (row) => row.neighbor_id, mono: true },
            { id: 'state', header: 'State', cell: (row) => row.state },
            { id: 'dead', header: 'Dead (s)', cell: (row) => row.dead_time_s, align: 'right', mono: true },
            { id: 'address', header: 'Address', cell: (row) => row.address, mono: true },
            { id: 'if', header: 'Interface', cell: (row) => row.interface, mono: true },
          ]}
        />
      );
    case 'dhcp':
      return (
        <Rows
          id="dhcp"
          label="DHCP bindings"
          rows={tables.dhcp_bindings}
          columns={[
            { id: 'ip', header: 'Address', cell: (row) => row.ip, mono: true },
            { id: 'mac', header: 'MAC', cell: (row) => row.mac, mono: true },
            { id: 'pool', header: 'Pool', cell: (row) => row.pool },
            { id: 'expires', header: 'Expires (s)', cell: (row) => row.expires_s, align: 'right', mono: true },
          ]}
        />
      );
    case 'nat':
      return (
        <Rows
          id="nat"
          label="NAT translations"
          rows={tables.nat}
          columns={[
            { id: 'protocol', header: 'Protocol', cell: (row) => row.protocol },
            { id: 'ig', header: 'Inside global', cell: (row) => row.inside_global, mono: true },
            { id: 'il', header: 'Inside local', cell: (row) => row.inside_local, mono: true },
            { id: 'ol', header: 'Outside local', cell: (row) => row.outside_local, mono: true },
            { id: 'og', header: 'Outside global', cell: (row) => row.outside_global, mono: true },
          ]}
        />
      );
  }
}

interface RowsProps<Row extends object> {
  id: string;
  label: string;
  rows: Row[];
  columns: TableColumn<Row>[];
}

/**
 * The simulator's table rows have no identity of their own — two identical
 * MAC entries are two rows — so their position is the key.
 */
function Rows<Row extends object>({ id, label, rows, columns }: RowsProps<Row>) {
  const keyed = useMemo(() => rows.map((row, index) => ({ ...row, _key: String(index) })), [rows]);
  return (
    <Table
      id={`lab-tables-${id}`}
      aria-label={label}
      rows={keyed}
      columns={columns}
      rowKey={(row) => row._key}
      density="compact"
      maxHeight={440}
      empty={`The device has nothing in its ${label.toLowerCase()}.`}
    />
  );
}
