import { Building2, Copy, Inbox, Pencil, Plus, Server, SquareTerminal, Trash } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { KeyValue } from '@/design/KeyValue';
import { Sparkline } from '@/design/Sparkline';
import { StatusDot } from '@/design/StatusDot';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Table, type TableColumn } from '@/design/Table';
import { TimeAgo } from '@/design/TimeAgo';
import { toast } from '@/design/Toast';
import { Tree, type TreeNode } from '@/design/Tree';
import { formatCount, formatDuration, formatPercent } from '@/lib/format';
import { compareIp } from '@/lib/net';
import { Example, Section } from '../parts';
import { SAMPLE_NOW, sampleDevices, type SampleDevice } from '../sample';
import styles from '../Showcase.module.css';

const COLUMNS: TableColumn<SampleDevice>[] = [
  {
    id: 'name',
    header: 'Device',
    required: true,
    sortValue: (device) => device.name,
    cell: (device) => (
      <span className={styles.inline} style={{ gap: 8 }}>
        <span className="mono">{device.name}</span>
        {device.lab && <LabPill />}
      </span>
    ),
  },
  {
    id: 'status',
    header: 'Status',
    width: 136,
    sortValue: (device) => device.statusLabel,
    cell: (device) => <StatusPill tone={device.status}>{device.statusLabel}</StatusPill>,
  },
  {
    id: 'address',
    header: 'Address',
    width: 150,
    mono: true,
    compare: (a, b) => compareIp(a.address, b.address),
    cell: (device) => device.address,
  },
  { id: 'site', header: 'Site', width: 130, sortValue: (device) => device.site, cell: (device) => device.site },
  {
    id: 'platform',
    header: 'Platform',
    width: 150,
    defaultHidden: true,
    sortValue: (device) => device.platform,
    cell: (device) => device.platform,
  },
  {
    id: 'cpu',
    header: 'CPU',
    width: 132,
    align: 'right',
    sortValue: (device) => device.cpu,
    cell: (device) => (
      <span className={styles.inline} style={{ gap: 8, justifyContent: 'flex-end', flexWrap: 'nowrap' }}>
        <Sparkline
          values={device.history}
          width={56}
          height={18}
          domain={[0, 100]}
          label={`CPU of ${device.name} over the last hour`}
        />
        <span style={{ width: 34 }}>{formatPercent(device.cpu)}</span>
      </span>
    ),
  },
  {
    id: 'uptime',
    header: 'Uptime',
    width: 96,
    align: 'right',
    sortValue: (device) => device.uptime,
    cell: (device) => formatDuration(device.uptime),
  },
  {
    id: 'seen',
    header: 'Last seen',
    width: 120,
    align: 'right',
    sortValue: (device) => device.seenAt,
    // Relative to the sample's own clock, so the showcase does not age.
    cell: (device) => <TimeAgo at={Date.now() - (SAMPLE_NOW - device.seenAt)} />,
  },
];

const SITES: TreeNode[] = [
  {
    id: 'amman',
    label: 'Amman',
    icon: Building2,
    trailing: formatCount(86),
    children: [
      {
        id: 'dc1',
        label: 'DC1',
        trailing: formatCount(52),
        children: [
          { id: 'core-sw-01', label: 'core-sw-01', icon: Server, trailing: <StatusDot tone="ok" label="Up" /> },
          { id: 'core-sw-02', label: 'core-sw-02', icon: Server, trailing: <StatusDot tone="warn" label="Degraded" /> },
          { id: 'edge-fw-01', label: 'edge-fw-01', icon: Server, trailing: <StatusDot tone="crit" label="Down" /> },
        ],
      },
      { id: 'dc2', label: 'DC2', trailing: formatCount(34), children: [{ id: 'dist-sw-07', label: 'dist-sw-07', icon: Server }] },
    ],
  },
  { id: 'irbid', label: 'Irbid campus', icon: Building2, trailing: formatCount(41), children: [{ id: 'access-sw-11', label: 'access-sw-11', icon: Server }] },
  { id: 'aqaba', label: 'Aqaba port', icon: Building2, trailing: formatCount(12) },
];

export function DataDisplay() {
  const devices = useMemo(() => sampleDevices(12), []);
  const manyDevices = useMemo(() => sampleDevices(5000, 21), []);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set(['dev-2', 'dev-3']));
  const [node, setNode] = useState<string | null>('core-sw-01');

  const rowActions = (device: SampleDevice) => [
    { label: 'Open terminal', icon: SquareTerminal, onSelect: () => toast.info(`Terminal to ${device.name}`) },
    { label: 'Copy address', icon: Copy, onSelect: () => toast.info(`Copied ${device.address}`) },
    { label: 'Edit', icon: Pencil, onSelect: () => toast.info(`Edit ${device.name}`) },
    { type: 'separator' as const },
    { label: 'Remove', icon: Trash, tone: 'danger' as const, onSelect: () => toast.info(`Remove ${device.name}`) },
  ];

  const emptyState = (
    <EmptyState
      size="compact"
      icon={Inbox}
      title="No devices yet"
      description="Devices you add or discover appear here with their live state."
      action={
        <Button variant="primary" size="sm" icon={Plus}>
          Add device
        </Button>
      }
    />
  );

  return (
    <>
      <Section
        title="Table"
        description="Sticky header, sortable columns (addresses and interface names sort the way engineers expect), selection, row actions as a button and as a context menu, a column chooser, and keyboard navigation: arrows move between rows, Space selects, Enter opens."
      >
        <Example title={`Compact · ${formatCount(selected.size)} selected · sort and columns are remembered`} span={3} layout="fill">
          <Table
            id="showcase.devices"
            aria-label="Devices"
            columns={COLUMNS}
            rows={devices}
            rowKey={(device) => device.id}
            defaultSort={{ columnId: 'name', direction: 'asc' }}
            selection={{ selected, onChange: setSelected }}
            rowActions={rowActions}
            onRowActivate={(device) => toast.info(`Open ${device.name}`)}
            empty={emptyState}
          />
        </Example>
        <Example title={`Virtualised · ${formatCount(manyDevices.length)} rows · comfortable`} span={3} layout="fill">
          <Table
            aria-label="All devices"
            density="comfortable"
            columns={COLUMNS}
            rows={manyDevices}
            rowKey={(device) => device.id}
            maxHeight={300}
            empty={emptyState}
          />
        </Example>
        <Example title="Loading" span={2} layout="fill">
          <Table aria-label="Devices, loading" columns={COLUMNS.slice(0, 4)} rows={[]} rowKey={(device) => device.id} loading empty={emptyState} />
        </Example>
        <Example title="Empty" layout="fill">
          <Table aria-label="Devices, empty" columns={COLUMNS.slice(0, 2)} rows={[]} rowKey={(device) => device.id} empty={emptyState} />
        </Example>
      </Section>

      <Section title="KeyValue and Tree">
        <Example title="KeyValue: rows" layout="fill">
          <KeyValue
            items={[
              { label: 'Hostname', value: 'core-sw-01', mono: true },
              { label: 'Address', value: '192.0.2.1', mono: true },
              { label: 'Platform', value: 'Cisco IOS-XE 17.9.4' },
              { label: 'Uptime', value: formatDuration(41 * 86_400_000 + 7 * 3_600_000) },
              { label: 'Serial', value: null },
            ]}
          />
        </Example>
        <Example title="KeyValue: stacked" layout="fill">
          <KeyValue
            layout="stacked"
            items={[
              { label: 'Location', value: 'Amman DC1, row C, rack 14' },
              { label: 'Contact', value: 'noc@example.net' },
              { label: 'Last backup', value: <TimeAgo at={Date.now() - 6 * 3_600_000} /> },
            ]}
          />
        </Example>
        <Example title="Tree: arrows walk it, type to jump" layout="fill">
          <Tree aria-label="Sites" nodes={SITES} selected={node} onSelect={setNode} defaultExpanded={['amman', 'dc1']} />
        </Example>
      </Section>
    </>
  );
}
