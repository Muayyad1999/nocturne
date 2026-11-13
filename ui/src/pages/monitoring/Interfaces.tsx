/**
 * The Interfaces tab: every port monitoring has seen, across the network.
 *
 * `monitor.interfaces` filters, sorts and pages in SQL — a campus has a
 * hundred thousand ports — so the toolbar sends the query rather than sifting
 * rows here. Every rate is `null` when the interval could not be measured
 * (the first poll, a failed read, a device that rebooted): those cells say
 * "not measured", never 0 b/s.
 */

import { CableCar } from 'lucide-react';
import { useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { useRpc } from '@/api/hooks';
import type { IfStatus, InterfaceQueryParams } from '@/api/types/monitor';
import { Badge } from '@/design/Badge';
import { Button } from '@/design/Button';
import { EmptyState } from '@/design/EmptyState';
import { ErrorState } from '@/design/ErrorState';
import { SearchInput } from '@/design/SearchInput';
import { Select } from '@/design/Select';
import { LabPill, StatusPill } from '@/design/StatusPill';
import { Switch } from '@/design/Switch';
import { Table, type TableColumn } from '@/design/Table';
import { Toolbar, ToolbarSpacer } from '@/design/Toolbar';
import { EMPTY, formatCount, formatPercent, formatRate } from '@/lib/format';
import { InterfaceDrawer } from './InterfaceDrawer';
import styles from './Monitoring.module.css';
import { metricFormat } from './vocab';

const PAGE = 200;

const SORTS: readonly { value: string; label: string }[] = [
  { value: 'device', label: 'Device and name' },
  { value: 'util', label: 'Busiest' },
  { value: 'in_bps', label: 'Most traffic in' },
  { value: 'out_bps', label: 'Most traffic out' },
  { value: 'errors', label: 'Most errors' },
];

export interface InterfaceTableProps {
  /** One device's ports, instead of the whole network. */
  deviceId?: string;
  /** Leaves out the device column and the device-wide empty state. */
  compact?: boolean;
}

export function InterfacesTab() {
  const [search, setSearch] = useSearchParams();
  // The status strip links here with ?problems=1 when something wants a look.
  const problems = search.get('problems') === '1';
  return (
    <InterfaceTable
      initialProblemsOnly={problems}
      onProblemsOnlyChange={(next) => {
        const params = new URLSearchParams(search);
        if (next) params.set('problems', '1');
        else params.delete('problems');
        setSearch(params, { replace: true });
      }}
    />
  );
}

export function InterfaceTable({
  deviceId,
  compact = false,
  initialProblemsOnly = false,
  onProblemsOnlyChange,
}: InterfaceTableProps & {
  initialProblemsOnly?: boolean;
  onProblemsOnlyChange?: (value: boolean) => void;
}) {
  const [text, setText] = useState('');
  const [sort, setSort] = useState(deviceId ? 'device' : 'util');
  const [problemsOnly, setProblemsOnly] = useState(initialProblemsOnly);
  const [limit, setLimit] = useState(PAGE);
  const [open, setOpen] = useState<IfStatus | null>(null);

  const params: InterfaceQueryParams = {
    ...(deviceId ? { device_id: deviceId } : {}),
    ...(text.trim() ? { filter: text.trim() } : {}),
    problem_only: problemsOnly,
    sort,
    limit,
  };
  const interfaces = useRpc<IfStatus[]>('monitor.interfaces', params, {
    invalidateOn: ['monitor.interfaces', 'devices.changed'],
  });
  const rows = interfaces.data ?? [];

  const columns: TableColumn<IfStatus>[] = [
    {
      id: 'state',
      header: 'State',
      width: 110,
      required: true,
      sortValue: (row) => interfaceRank(row),
      cell: (row) => <InterfaceState row={row} />,
    },
    ...(compact
      ? []
      : [
          {
            id: 'device',
            header: 'Device',
            width: 200,
            sortValue: (row: IfStatus) => row.device,
            cell: (row: IfStatus) => (
              <span className={styles.cell}>
                <Link
                  className={styles.cellText}
                  to={`/monitoring/health?device=${encodeURIComponent(row.device_id)}`}
                >
                  {row.device}
                </Link>
                {row.lab && <LabPill />}
              </span>
            ),
          } satisfies TableColumn<IfStatus>,
        ]),
    {
      id: 'name',
      header: 'Interface',
      width: 190,
      mono: true,
      required: true,
      sortValue: (row) => row.name,
      cell: (row) => (
        <span className={styles.cell}>
          <span className={styles.cellText}>{row.name}</span>
          {row.uplink && <Badge variant="outline" title="A neighbour has been collected on this port">Uplink</Badge>}
        </span>
      ),
    },
    {
      id: 'description',
      header: 'Description',
      sortValue: (row) => row.description,
      cell: (row) =>
        row.description ? (
          <span className={styles.cellText} title={row.description}>
            {row.description}
          </span>
        ) : (
          <span className={styles.missing}>{EMPTY}</span>
        ),
    },
    {
      id: 'speed',
      header: 'Speed',
      width: 92,
      align: 'right',
      sortValue: (row) => row.speed_bps ?? null,
      cell: (row) =>
        row.speed_bps == null ? (
          <Missing title="The device did not say how fast this port is." />
        ) : (
          formatRate(row.speed_bps)
        ),
    },
    {
      id: 'in',
      header: 'In',
      width: 100,
      align: 'right',
      sortValue: (row) => row.in_bps ?? null,
      cell: (row) => <Rate value={row.in_bps} />,
    },
    {
      id: 'out',
      header: 'Out',
      width: 100,
      align: 'right',
      sortValue: (row) => row.out_bps ?? null,
      cell: (row) => <Rate value={row.out_bps} />,
    },
    {
      id: 'util',
      header: 'Used',
      width: 84,
      align: 'right',
      sortValue: (row) => row.util_pct ?? null,
      cell: (row) =>
        row.util_pct == null ? (
          <Missing title="Utilisation needs both a rate and the port's speed; one of them is missing." />
        ) : (
          <span
            className={row.util_pct >= 90 ? styles.high : undefined}
            title={row.util_pct >= 90 ? 'At or above 90 % of the port speed' : undefined}
          >
            {formatPercent(row.util_pct)}
          </span>
        ),
    },
    {
      id: 'errors',
      header: 'Errors',
      width: 100,
      align: 'right',
      sortValue: (row) => (row.in_err_ps ?? 0) + (row.out_err_ps ?? 0),
      cell: (row) =>
        row.in_err_ps == null && row.out_err_ps == null ? (
          <Missing title="No error counters have been read from this port yet." />
        ) : (
          <span title={`${metricFormat('if.in_err_ps')(row.in_err_ps)} in, ${metricFormat('if.out_err_ps')(row.out_err_ps)} out`}>
            {metricFormat('if.in_err_ps')((row.in_err_ps ?? 0) + (row.out_err_ps ?? 0))}
          </span>
        ),
    },
  ];

  if (interfaces.isError) {
    return (
      <ErrorState
        title="The interfaces could not be loaded"
        error={interfaces.error}
        onRetry={() => void interfaces.refetch()}
      />
    );
  }

  return (
    <>
      <Toolbar className={styles.filters}>
        <SearchInput
          className={styles.search}
          size="sm"
          aria-label="Search interfaces"
          placeholder={deviceId ? 'Port or description' : 'Device, port or description'}
          value={text}
          onChange={(value) => {
            setText(value);
            setLimit(PAGE);
          }}
        />
        <Select
          size="sm"
          className={styles.control}
          aria-label="Sort interfaces"
          options={SORTS}
          value={sort}
          onChange={(event) => setSort(event.target.value)}
        />
        <Switch
          checked={problemsOnly}
          onChange={(event) => {
            setProblemsOnly(event.target.checked);
            setLimit(PAGE);
            onProblemsOnlyChange?.(event.target.checked);
          }}
        >
          Needs a look
        </Switch>
        <span className={styles.count}>
          {interfaces.isPending
            ? ''
            : `${formatCount(rows.length)}${rows.length >= limit ? '+' : ''} ${rows.length === 1 ? 'interface' : 'interfaces'}`}
        </span>
        <ToolbarSpacer />
      </Toolbar>

      <Table
        className={styles.table}
        id={compact ? 'monitoring.device.interfaces' : 'monitoring.interfaces'}
        aria-label="Interfaces"
        fill
        columns={columns}
        rows={rows}
        rowKey={(row) => `${row.device_id}:${row.name}`}
        loading={interfaces.isPending}
        onRowActivate={setOpen}
        rowActions={(row) => [{ label: 'Open', onSelect: () => setOpen(row) }]}
        empty={
          <EmptyState
            size="compact"
            icon={CableCar}
            title={problemsOnly ? 'Nothing needs a look' : text ? 'Nothing matches' : 'No interfaces have been read yet'}
            description={
              problemsOnly
                ? 'No interface is down while administratively up, taking errors, or at 90 % and above.'
                : text
                  ? 'No interface matches what you typed.'
                  : 'Monitoring records interfaces from SNMP, or over the command line where there is no SNMP. Nothing has arrived yet.'
            }
          />
        }
      />

      {rows.length >= limit && (
        <div className={styles.more}>
          <Button size="sm" onClick={() => setLimit(limit + PAGE)} loading={interfaces.isFetching}>
            Show more
          </Button>
          <span className={styles.count}>Showing the first {formatCount(limit)}.</span>
        </div>
      )}

      {open && <InterfaceDrawer row={open} onClose={() => setOpen(null)} />}
    </>
  );
}

/** Down while admin up is a fault; down because somebody shut it is not. */
function interfaceRank(row: IfStatus): number {
  if (row.admin_up === true && row.oper_up === false) return 0;
  if (row.oper_up == null || row.admin_up == null) return 2;
  return row.oper_up ? 3 : 1;
}

export function InterfaceState({ row }: { row: IfStatus }) {
  if (row.admin_up == null && row.oper_up == null) {
    return (
      <StatusPill tone="idle" title="The device did not report this port's state.">
        Unknown
      </StatusPill>
    );
  }
  if (row.admin_up === false) {
    return (
      <StatusPill tone="idle" title="Administratively shut down.">
        Shut
      </StatusPill>
    );
  }
  if (row.oper_up === false) {
    return (
      <StatusPill tone="crit" title="Administratively up, but the line protocol is down.">
        Down
      </StatusPill>
    );
  }
  return <StatusPill tone="ok">Up</StatusPill>;
}

function Rate({ value }: { value: number | null }) {
  if (value == null) return <Missing title="No rate could be worked out for the last interval: a gap, not nothing." />;
  return <>{formatRate(value)}</>;
}

function Missing({ title }: { title: string }) {
  return (
    <span className={styles.missing} title={title}>
      Not measured
    </span>
  );
}
